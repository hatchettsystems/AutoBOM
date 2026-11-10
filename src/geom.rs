//! Geometry for the preview: reads the edges of a part's solids (lines, circles, ellipses, splines) from the STEP
//! file, in millimetres, so the window can draw it and measure it. It draws the outline of the part, not shaded faces.

use crate::step::{statements, Arg, Entity, Parser};
use std::collections::{HashMap, HashSet};
use std::f64::consts::PI;

pub type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn add(a: V3, b: V3) -> V3 { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
fn mul(a: V3, k: f64) -> V3 { [a[0] * k, a[1] * k, a[2] * k] }
fn dot(a: V3, b: V3) -> f64 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn cross(a: V3, b: V3) -> V3 { [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]] }
fn norm(a: V3) -> V3 { let l = dot(a, a).sqrt(); if l < 1e-12 { [0.0, 0.0, 1.0] } else { mul(a, 1.0 / l) } }

/// The outline of a part: polylines in millimetres plus the box around them.
#[derive(Debug, Clone, Default)]
pub struct Wire {
    pub lines: Vec<Vec<V3>>,
    pub min: V3,
    pub max: V3,
}

impl Wire {
    pub fn is_empty(&self) -> bool { self.lines.is_empty() }
    pub fn size(&self) -> V3 { sub(self.max, self.min) }
    pub fn center(&self) -> V3 { mul(add(self.min, self.max), 0.5) }
    fn finish(&mut self) {
        let mut mn = [f64::MAX; 3];
        let mut mx = [f64::MIN; 3];
        for l in &self.lines { for p in l { for k in 0..3 { mn[k] = mn[k].min(p[k]); mx[k] = mx[k].max(p[k]); } } }
        if self.lines.is_empty() { mn = [0.0; 3]; mx = [0.0; 3]; }
        self.min = mn;
        self.max = mx;
    }
    pub fn merge(parts: Vec<Wire>) -> Wire {
        let mut w = Wire::default();
        for p in parts { w.lines.extend(p.lines); }
        w.finish();
        w
    }
}

pub struct Geo {
    entities: HashMap<u32, Entity>,
    /// Entities written as several constituents sharing one id (rational splines, units).
    complex: HashMap<u32, Vec<Entity>>,
    /// Millimetres per file length unit.
    pub unit_mm: f64,
}

const KEEP: &[&str] = &[
    "MANIFOLD_SOLID_BREP", "BREP_WITH_VOIDS", "FACETED_BREP", "FACETED_BREP_WITH_VOIDS", "SHELL_BASED_SURFACE_MODEL",
    "CLOSED_SHELL", "OPEN_SHELL", "ORIENTED_CLOSED_SHELL", "ORIENTED_OPEN_SHELL", "ADVANCED_FACE", "FACE_SURFACE", "FACE", "FACE_BOUND", "FACE_OUTER_BOUND",
    "EDGE_LOOP", "ORIENTED_EDGE", "EDGE_CURVE", "VERTEX_POINT", "CARTESIAN_POINT", "DIRECTION", "AXIS2_PLACEMENT_3D", "LINE", "VECTOR",
    "CIRCLE", "ELLIPSE", "B_SPLINE_CURVE_WITH_KNOTS", "SURFACE_CURVE", "SEAM_CURVE", "INTERSECTION_CURVE", "POLYLINE",
    "CONVERSION_BASED_UNIT",
];

impl Geo {
    pub fn load(path: &std::path::Path) -> Result<Geo, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("Could not read the file: {e}"))?;
        let text = String::from_utf8_lossy(&bytes);
        let mut g = Geo { entities: HashMap::new(), complex: HashMap::new(), unit_mm: 1.0 };
        let mut unit_found = false;
        statements(&text, |s| {
            let Some(eq) = s.find('=') else { return };
            let Ok(id) = s[1..eq].trim().parse::<u32>() else { return };
            let rest = s[eq + 1..].trim();
            if let Some(inner) = rest.strip_prefix('(') {
                let inner = inner.strip_suffix(')').unwrap_or(inner);
                let mut p = Parser::new(inner);
                let mut parts = Vec::new();
                loop {
                    p.skip_ws();
                    if p.eof() { break; }
                    let name = p.word();
                    if name.is_empty() { break; }
                    p.skip_ws();
                    let args = if p.peek() == Some(b'(') { p.args() } else { Vec::new() };
                    parts.push(Entity { name, args });
                }
                if !unit_found && parts.iter().any(|e| e.name == "LENGTH_UNIT") {
                    if let Some(si) = parts.iter().find(|e| e.name == "SI_UNIT") {
                        let prefix = match si.args.first() { Some(Arg::Enum(e)) => e.as_str(), _ => "" };
                        g.unit_mm = match prefix { "MILLI" => 1.0, "CENTI" => 10.0, "DECI" => 100.0, "KILO" => 1e6, "MICRO" => 0.001, _ => 1000.0 };
                        unit_found = true;
                    }
                }
                if !unit_found && parts.iter().any(|e| e.name == "LENGTH_UNIT") {
                    if let Some(c) = parts.iter().find(|e| e.name == "CONVERSION_BASED_UNIT") {
                        match c.args.first().and_then(|a| a.as_str()).map(|n| n.to_ascii_uppercase()).as_deref() {
                            Some("INCH") => { g.unit_mm = 25.4; unit_found = true; }
                            Some("FOOT") => { g.unit_mm = 304.8; unit_found = true; }
                            _ => {}
                        }
                    }
                }
                if parts.iter().any(|e| e.name.starts_with("B_SPLINE")) { g.complex.insert(id, parts); }
                return;
            }
            let Some(open) = rest.find('(') else { return };
            let name = rest[..open].trim();
            if !KEEP.contains(&name) { return; }
            let args = Parser::new(&rest[open..]).args();
            if name == "CONVERSION_BASED_UNIT" && !unit_found {
                if let Some(n) = args.first().and_then(|a| a.as_str()) {
                    let n = n.to_ascii_uppercase();
                    if n == "INCH" { g.unit_mm = 25.4; unit_found = true; } else if n == "FOOT" { g.unit_mm = 304.8; unit_found = true; }
                }
            }
            g.entities.insert(id, Entity { name: name.to_string(), args });
        })?;
        Ok(g)
    }

    fn num(a: Option<&Arg>) -> f64 { match a { Some(Arg::Real(r)) => *r, Some(Arg::Int(i)) => *i as f64, _ => 0.0 } }
    fn triple(a: &Arg) -> V3 { let l = a.as_list(); [Self::num(l.first()), Self::num(l.get(1)), Self::num(l.get(2))] }

    fn point(&self, id: u32) -> Option<V3> {
        let e = self.entities.get(&id)?;
        if e.name != "CARTESIAN_POINT" { return None; }
        Some(Self::triple(e.args.get(1)?))
    }
    fn vertex(&self, id: u32) -> Option<V3> {
        let e = self.entities.get(&id)?;
        self.point(e.args.get(1)?.as_ref()?)
    }
    fn dir(&self, id: u32) -> Option<V3> {
        let e = self.entities.get(&id)?;
        if e.name != "DIRECTION" { return None; }
        Some(norm(Self::triple(e.args.get(1)?)))
    }
    /// origin, x, y, z of an AXIS2_PLACEMENT_3D
    fn frame(&self, id: u32) -> Option<(V3, V3, V3, V3)> {
        let e = self.entities.get(&id)?;
        if e.name != "AXIS2_PLACEMENT_3D" { return None; }
        let o = self.point(e.args.get(1)?.as_ref()?)?;
        let z = e.args.get(2).and_then(|a| a.as_ref()).and_then(|d| self.dir(d)).unwrap_or([0.0, 0.0, 1.0]);
        let mut x = e.args.get(3).and_then(|a| a.as_ref()).and_then(|d| self.dir(d)).unwrap_or_else(|| {
            if z[0].abs() < 0.9 { norm(cross([1.0, 0.0, 0.0], z)) } else { norm(cross([0.0, 1.0, 0.0], z)) }
        });
        x = norm(sub(x, mul(z, dot(x, z))));
        Some((o, x, norm(cross(z, x)), z))
    }

    /// The outline of every solid in `solids`.
    pub fn wire(&self, solids: &[u32]) -> Wire {
        let mut w = Wire::default();
        let mut seen_edges: HashSet<u32> = HashSet::new();
        let mut seen: HashSet<u32> = HashSet::new();
        for s in solids {
            let mut stack: Vec<u32> = vec![*s];
            while let Some(id) = stack.pop() {
                if !seen.insert(id) { continue; }
                let Some(e) = self.entities.get(&id) else { continue };
                match e.name.as_str() {
                    "MANIFOLD_SOLID_BREP" | "FACETED_BREP" => { if let Some(r) = e.args.get(1).and_then(|a| a.as_ref()) { stack.push(r); } }
                    "BREP_WITH_VOIDS" | "FACETED_BREP_WITH_VOIDS" => {
                        if let Some(r) = e.args.get(1).and_then(|a| a.as_ref()) { stack.push(r); }
                    }
                    "SHELL_BASED_SURFACE_MODEL" => { for r in e.args.get(1).map(|a| a.as_list()).unwrap_or(&[]) { if let Some(r) = r.as_ref() { stack.push(r); } } }
                    "CLOSED_SHELL" | "OPEN_SHELL" => { for r in e.args.get(1).map(|a| a.as_list()).unwrap_or(&[]) { if let Some(r) = r.as_ref() { stack.push(r); } } }
                    "ORIENTED_CLOSED_SHELL" | "ORIENTED_OPEN_SHELL" => { if let Some(r) = e.args.get(2).and_then(|a| a.as_ref()) { stack.push(r); } }
                    "ADVANCED_FACE" | "FACE_SURFACE" | "FACE" => { for r in e.args.get(1).map(|a| a.as_list()).unwrap_or(&[]) { if let Some(r) = r.as_ref() { stack.push(r); } } }
                    "FACE_BOUND" | "FACE_OUTER_BOUND" => { if let Some(r) = e.args.get(1).and_then(|a| a.as_ref()) { stack.push(r); } }
                    "EDGE_LOOP" => { for r in e.args.get(1).map(|a| a.as_list()).unwrap_or(&[]) { if let Some(r) = r.as_ref() { stack.push(r); } } }
                    "ORIENTED_EDGE" => {
                        if let Some(ec) = e.args.get(3).and_then(|a| a.as_ref()) {
                            if seen_edges.insert(ec) { if let Some(l) = self.edge(ec) { w.lines.push(l.into_iter().map(|p| mul(p, self.unit_mm)).collect()); } }
                        }
                    }
                    _ => {}
                }
            }
        }
        w.finish();
        w
    }

    fn edge(&self, id: u32) -> Option<Vec<V3>> {
        let e = self.entities.get(&id)?;
        if e.name != "EDGE_CURVE" { return None; }
        let a = self.vertex(e.args.get(1)?.as_ref()?)?;
        let b = self.vertex(e.args.get(2)?.as_ref()?)?;
        let same = !matches!(e.args.get(4), Some(Arg::Enum(v)) if v == "F");
        let cid = e.args.get(3)?.as_ref()?;
        Some(self.curve(cid, a, b, same).unwrap_or_else(|| vec![a, b]))
    }

    fn curve(&self, id: u32, a: V3, b: V3, same: bool) -> Option<Vec<V3>> {
        if let Some(parts) = self.complex.get(&id) { return self.spline(parts, a, b); }
        let c = self.entities.get(&id)?;
        match c.name.as_str() {
            "LINE" => Some(vec![a, b]),
            "CIRCLE" | "ELLIPSE" => {
                let (o, x, y, _) = self.frame(c.args.get(1)?.as_ref()?)?;
                let ra = Self::num(c.args.get(2));
                let rb = if c.name == "ELLIPSE" { Self::num(c.args.get(3)) } else { ra };
                if ra <= 0.0 || rb <= 0.0 { return None; }
                let ang = |p: V3| { let d = sub(p, o); (dot(d, y) / rb).atan2(dot(d, x) / ra) };
                let (mut s, mut e) = (ang(a), ang(b));
                if !same { std::mem::swap(&mut s, &mut e); }
                let mut sweep = e - s;
                while sweep < 0.0 { sweep += 2.0 * PI; }
                if sweep < 1e-9 { sweep = 2.0 * PI; }
                let n = ((sweep / (2.0 * PI)) * 96.0).ceil().max(4.0) as usize;
                Some((0..=n).map(|i| {
                    let t = s + sweep * i as f64 / n as f64;
                    add(o, add(mul(x, ra * t.cos()), mul(y, rb * t.sin())))
                }).collect())
            }
            "B_SPLINE_CURVE_WITH_KNOTS" => self.spline(std::slice::from_ref(c), a, b),
            "SURFACE_CURVE" | "SEAM_CURVE" | "INTERSECTION_CURVE" => self.curve(c.args.get(1)?.as_ref()?, a, b, same),
            "POLYLINE" => Some(c.args.get(1)?.as_list().iter().filter_map(|r| self.point(r.as_ref()?)).collect()),
            _ => None,
        }
    }

    /// A B-spline (rational or not) cut down to the stretch between the edge's two vertices.
    fn spline(&self, parts: &[Entity], a: V3, b: V3) -> Option<Vec<V3>> {
        let find = |n: &str| parts.iter().find(|e| e.name == n);
        let base = find("B_SPLINE_CURVE").or_else(|| find("B_SPLINE_CURVE_WITH_KNOTS"))?;
        let kn = find("B_SPLINE_CURVE_WITH_KNOTS")?;
        let deg = Self::num(base.args.get(if base.name == "B_SPLINE_CURVE" && parts.len() > 1 { 0 } else { 1 })) as usize;
        let ctrl: Vec<V3> = base.args.get(if base.name == "B_SPLINE_CURVE" && parts.len() > 1 { 1 } else { 2 })?.as_list().iter().filter_map(|r| self.point(r.as_ref()?)).collect();
        // complex entities carry (mults, knots, spec) after the base's own arguments; plain ones carry (.., mults, knots, spec) at 6,7
        let (mults_arg, knots_arg) = if parts.len() > 1 || kn.args.len() <= 3 { (kn.args.first()?, kn.args.get(1)?) } else { (kn.args.get(6)?, kn.args.get(7)?) };
        let weights: Option<Vec<f64>> = parts.iter().find(|e| e.name == "RATIONAL_B_SPLINE_CURVE").and_then(|e| e.args.get(0)).map(|w| w.as_list().iter().map(|x| Self::num(Some(x))).collect());
        let mut knots: Vec<f64> = Vec::new();
        for (m, k) in mults_arg.as_list().iter().zip(knots_arg.as_list()) { for _ in 0..(Self::num(Some(m)) as usize) { knots.push(Self::num(Some(k))); } }
        let n = ctrl.len();
        if deg == 0 || n <= deg || knots.len() != n + deg + 1 { return None; }
        let (t0, t1) = (knots[deg], knots[n]);
        let m = 96usize;
        let pts: Vec<V3> = (0..=m).map(|i| {
            let t = t0 + (t1 - t0) * i as f64 / m as f64;
            let t = if i == m { t1 - 1e-12 * (t1 - t0).abs().max(1.0) } else { t };
            let mut k = deg; while k + 1 < n && t >= knots[k + 1] { k += 1; }
            let mut d: Vec<[f64; 4]> = (0..=deg).map(|j| {
                let idx = j + k - deg;
                let w = weights.as_ref().and_then(|w| w.get(idx)).copied().unwrap_or(1.0);
                [ctrl[idx][0] * w, ctrl[idx][1] * w, ctrl[idx][2] * w, w]
            }).collect();
            for r in 1..=deg {
                for j in (r..=deg).rev() {
                    let l = knots[j + k - deg];
                    let den = knots[j + 1 + k - r] - l;
                    let al = if den.abs() < 1e-14 { 0.0 } else { (t - l) / den };
                    for c in 0..4 { d[j][c] = (1.0 - al) * d[j - 1][c] + al * d[j][c]; }
                }
            }
            let h = d[deg];
            if h[3].abs() < 1e-14 { [h[0], h[1], h[2]] } else { [h[0] / h[3], h[1] / h[3], h[2] / h[3]] }
        }).collect();
        let near = |p: V3| pts.iter().enumerate().min_by(|x, y| { let dx = sub(*x.1, p); let dy = sub(*y.1, p); dot(dx, dx).partial_cmp(&dot(dy, dy)).unwrap() }).map(|x| x.0).unwrap_or(0);
        let (i, j) = (near(a), near(b));
        if i == j { return Some(pts); }
        Some(if i < j { pts[i..=j].to_vec() } else { pts[j..=i].iter().rev().cloned().collect() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOX: &str = "ISO-10303-21;\nDATA;\n#1=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));\n\
#10=CARTESIAN_POINT('',(0.,0.,0.));#11=CARTESIAN_POINT('',(40.,0.,0.));#12=CARTESIAN_POINT('',(40.,20.,0.));#13=CARTESIAN_POINT('',(0.,20.,0.));\n\
#14=CARTESIAN_POINT('',(0.,0.,5.));#15=CARTESIAN_POINT('',(40.,0.,5.));\n\
#20=VERTEX_POINT('',#10);#21=VERTEX_POINT('',#11);#22=VERTEX_POINT('',#12);#23=VERTEX_POINT('',#13);#24=VERTEX_POINT('',#14);#25=VERTEX_POINT('',#15);\n\
#30=DIRECTION('',(1.,0.,0.));#31=VECTOR('',#30,1.);#32=LINE('',#10,#31);\n\
#40=EDGE_CURVE('',#20,#21,#32,.T.);#41=EDGE_CURVE('',#21,#22,#32,.T.);#42=EDGE_CURVE('',#22,#23,#32,.T.);#43=EDGE_CURVE('',#20,#24,#32,.T.);\n\
#50=ORIENTED_EDGE('',*,*,#40,.T.);#51=ORIENTED_EDGE('',*,*,#41,.T.);#52=ORIENTED_EDGE('',*,*,#42,.T.);#53=ORIENTED_EDGE('',*,*,#43,.T.);\n\
#60=EDGE_LOOP('',(#50,#51,#52,#53));#61=FACE_OUTER_BOUND('',#60,.T.);#62=ADVANCED_FACE('',(#61),#99,.T.);#63=CLOSED_SHELL('',(#62));#64=MANIFOLD_SOLID_BREP('',#63);\nENDSEC;";

    fn write(name: &str, t: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("autobom-geom-{}-{}.step", std::process::id(), name));
        std::fs::write(&p, t).unwrap();
        p
    }

    #[test]
    fn straight_edges_and_size() {
        let p = write("box", BOX);
        let g = Geo::load(&p).unwrap();
        let w = g.wire(&[64]);
        let _ = std::fs::remove_file(p);
        assert_eq!(w.lines.len(), 4);
        assert_eq!(w.size(), [40.0, 20.0, 5.0]);
        assert_eq!(g.unit_mm, 1.0);
    }

    #[test]
    fn circles_and_inches() {
        let t = "DATA;\n#1=(CONVERSION_BASED_UNIT('INCH',#9)LENGTH_UNIT()NAMED_UNIT(#8));\n#2=CARTESIAN_POINT('',(0.,0.,0.));#3=DIRECTION('',(0.,0.,1.));#4=DIRECTION('',(1.,0.,0.));\n\
#5=AXIS2_PLACEMENT_3D('',#2,#3,#4);#6=CIRCLE('',#5,1.);#10=CARTESIAN_POINT('',(1.,0.,0.));#11=VERTEX_POINT('',#10);#12=EDGE_CURVE('',#11,#11,#6,.T.);\n#13=ORIENTED_EDGE('',*,*,#12,.T.);#14=EDGE_LOOP('',(#13));#15=FACE_BOUND('',#14,.T.);#16=ADVANCED_FACE('',(#15),#9,.T.);#17=CLOSED_SHELL('',(#16));#18=MANIFOLD_SOLID_BREP('',#17);\nENDSEC;";
        let p = write("circle", t);
        let g = Geo::load(&p).unwrap();
        let w = g.wire(&[18]);
        let _ = std::fs::remove_file(p);
        assert_eq!(g.unit_mm, 25.4);
        let s = w.size();
        assert!((s[0] - 50.8).abs() < 0.05 && (s[1] - 50.8).abs() < 0.05, "{s:?}");
    }
}

//! A small, forgiving reader for STEP (ISO 10303-21) files.
//!
//! It does not read geometry. It reads what a bill of materials needs: which products exist, what they are
//! called, which products sit inside which (the assembly tree), and how many solid bodies each product owns.
//! Big files are fine: statements the tool does not care about are skipped without being parsed.

use std::collections::{HashMap, HashSet};

/// One value inside an entity's argument list.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Null,
    Derived,
    Int(i64),
    Real(f64),
    Str(String),
    Enum(String),
    Ref(u32),
    List(Vec<Arg>),
    Typed(String, Vec<Arg>),
}

impl Arg {
    pub fn as_str(&self) -> Option<&str> {
        if let Arg::Str(s) = self { Some(s) } else { None }
    }
    pub fn as_ref(&self) -> Option<u32> {
        if let Arg::Ref(r) = self { Some(*r) } else { None }
    }
    pub fn as_list(&self) -> &[Arg] {
        if let Arg::List(l) = self { l } else { &[] }
    }
}

#[derive(Debug, Clone)]
pub struct Entity {
    pub name: String,
    pub args: Vec<Arg>,
}

/// Entity types worth parsing in full.
const WANTED: &[&str] = &[
    "PRODUCT",
    "PRODUCT_DEFINITION_FORMATION",
    "PRODUCT_DEFINITION_FORMATION_WITH_SPECIFIED_SOURCE",
    "PRODUCT_DEFINITION",
    "PRODUCT_DEFINITION_SHAPE",
    "NEXT_ASSEMBLY_USAGE_OCCURRENCE",
    "SHAPE_DEFINITION_REPRESENTATION",
    "SHAPE_REPRESENTATION",
    "ADVANCED_BREP_SHAPE_REPRESENTATION",
    "MANIFOLD_SURFACE_SHAPE_REPRESENTATION",
    "FACETED_BREP_SHAPE_REPRESENTATION",
    "GEOMETRICALLY_BOUNDED_SURFACE_SHAPE_REPRESENTATION",
    "GEOMETRICALLY_BOUNDED_WIREFRAME_SHAPE_REPRESENTATION",
    "TESSELLATED_SHAPE_REPRESENTATION",
    "SHAPE_REPRESENTATION_RELATIONSHIP",
    "REPRESENTATION_RELATIONSHIP",
];

/// Entity types that are a solid (or surface) body.
const SOLIDS: &[&str] = &[
    "MANIFOLD_SOLID_BREP",
    "BREP_WITH_VOIDS",
    "FACETED_BREP",
    "FACETED_BREP_WITH_VOIDS",
    "SHELL_BASED_SURFACE_MODEL",
    "TESSELLATED_SOLID",
    "CSG_SOLID",
];

#[derive(Debug, Default)]
pub struct StepFile {
    pub entities: HashMap<u32, Entity>,
    /// Ids of solid-body entities (their arguments are never needed).
    pub solids: HashSet<u32>,
    /// Relationships that carry a transformation are placements (assembly structure), not geometry links.
    pub placement_relationships: HashSet<u32>,
    pub statements: usize,
    pub skipped: usize,
}

/// Calls `f` with each cleaned `#id = ...` statement of the DATA section.
pub(crate) fn statements(text: &str, mut f: impl FnMut(&str)) -> Result<(), String> {
    let bytes = text.as_bytes();
    let data = find_data_section(bytes).ok_or("This does not look like a STEP file (no DATA section found).")?;
    let mut i = data;
    let n = bytes.len();
    while i < n {
        // find the end of the next statement, respecting 'strings' and /* comments */
        let start = i;
        let mut in_str = false;
        while i < n {
            let c = bytes[i];
            if in_str {
                if c == b'\'' {
                    if i + 1 < n && bytes[i + 1] == b'\'' { i += 2; continue; }
                    in_str = false;
                }
            } else if c == b'\'' {
                in_str = true;
            } else if c == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
                i += 2;
                while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') { i += 1; }
                i += 1;
            } else if c == b';' {
                break;
            }
            i += 1;
        }
        let stmt = &text[start..i.min(n)];
        i += 1;
        let cleaned;
        let s = if stmt.contains("/*") { cleaned = strip_comments(stmt); cleaned.trim() } else { stmt.trim() };
        if s.is_empty() { continue; }
        if s.starts_with("ENDSEC") { break; }
        if !s.starts_with('#') { continue; }
        f(s);
    }
    Ok(())
}

/// Parses a STEP file's text.
pub fn parse(text: &str) -> Result<StepFile, String> {
    let mut out = StepFile::default();
    statements(text, |s| { out.statements += 1; handle_statement(s, &mut out); })?;
    Ok(out)
}

/// Removes /* comments */ (outside strings) from a statement.
fn strip_comments(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::new();
    let (mut i, mut last, mut in_str) = (0, 0, false);
    while i < b.len() {
        if in_str {
            if b[i] == b'\'' { if i + 1 < b.len() && b[i + 1] == b'\'' { i += 2; continue; } in_str = false; }
        } else if b[i] == b'\'' { in_str = true; }
        else if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            out.push_str(&s[last..i]);
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') { i += 1; }
            i += 2;
            last = i.min(b.len());
            continue;
        }
        i += 1;
    }
    out.push_str(&s[last.min(s.len())..]);
    out
}

fn find_data_section(b: &[u8]) -> Option<usize> {
    let key = b"DATA;";
    let mut i = 0;
    while i + key.len() <= b.len() {
        if &b[i..i + key.len()] == key { return Some(i + key.len()); }
        i += 1;
    }
    None
}

fn handle_statement(s: &str, out: &mut StepFile) {
    // "#12 = NAME(args)"  or  "#12 = (NAME1(args) NAME2(args))"
    let eq = match s.find('=') { Some(e) => e, None => { out.skipped += 1; return; } };
    let id: u32 = match s[1..eq].trim().parse() { Ok(v) => v, Err(_) => { out.skipped += 1; return; } };
    let rest = s[eq + 1..].trim();
    if let Some(inner) = rest.strip_prefix('(') {
        // complex entity: several constituents share one id
        let inner = inner.strip_suffix(')').unwrap_or(inner);
        let mut p = Parser::new(inner);
        let mut parts: Vec<Entity> = Vec::new();
        loop {
            p.skip_ws();
            if p.eof() { break; }
            let name = p.word();
            if name.is_empty() { break; }
            p.skip_ws();
            let args = if p.peek() == Some(b'(') { p.args() } else { Vec::new() };
            parts.push(Entity { name, args });
        }
        let has_rel = parts.iter().any(|e| e.name == "REPRESENTATION_RELATIONSHIP");
        let has_trans = parts.iter().any(|e| e.name == "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION");
        let has_shape_rel = parts.iter().any(|e| e.name == "SHAPE_REPRESENTATION_RELATIONSHIP");
        if has_rel && has_trans {
            out.placement_relationships.insert(id);
        }
        // keep the constituent that carries the arguments we use
        let pick = parts.iter().position(|e| e.name == "REPRESENTATION_RELATIONSHIP")
            .or_else(|| parts.iter().position(|e| WANTED.contains(&e.name.as_str())));
        if let Some(k) = pick {
            let mut e = parts.swap_remove(k);
            if e.name == "REPRESENTATION_RELATIONSHIP" && has_shape_rel { e.name = "SHAPE_REPRESENTATION_RELATIONSHIP".into(); }
            out.entities.insert(id, e);
        } else if parts.iter().any(|e| SOLIDS.contains(&e.name.as_str())) {
            out.solids.insert(id);
        }
        return;
    }
    let open = match rest.find('(') { Some(o) => o, None => { out.skipped += 1; return; } };
    let name = rest[..open].trim();
    if SOLIDS.contains(&name) { out.solids.insert(id); return; }
    if !WANTED.contains(&name) { return; }
    let mut p = Parser::new(&rest[open..]);
    let args = p.args();
    out.entities.insert(id, Entity { name: name.to_string(), args });
}

pub(crate) struct Parser<'a> {
    b: &'a [u8],
    s: &'a str,
    i: usize,
}

impl<'a> Parser<'a> {
    pub(crate) fn new(s: &'a str) -> Self { Parser { b: s.as_bytes(), s, i: 0 } }
    pub(crate) fn eof(&self) -> bool { self.i >= self.b.len() }
    pub(crate) fn peek(&self) -> Option<u8> { self.b.get(self.i).copied() }
    pub(crate) fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() { self.i += 1; }
            else if c == b'/' && self.b.get(self.i + 1) == Some(&b'*') {
                self.i += 2;
                while self.i + 1 < self.b.len() && !(self.b[self.i] == b'*' && self.b[self.i + 1] == b'/') { self.i += 1; }
                self.i += 2;
            } else { break; }
        }
    }
    pub(crate) fn word(&mut self) -> String {
        let st = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == b'_' { self.i += 1; } else { break; }
        }
        self.s[st..self.i].to_string()
    }
    /// "( a, b, c )"
    pub(crate) fn args(&mut self) -> Vec<Arg> {
        self.skip_ws();
        if self.peek() != Some(b'(') { return Vec::new(); }
        self.i += 1;
        let mut v = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => break,
                Some(b')') => { self.i += 1; break; }
                Some(b',') => { self.i += 1; }
                _ => v.push(self.value()),
            }
        }
        v
    }
    fn value(&mut self) -> Arg {
        self.skip_ws();
        match self.peek() {
            Some(b'$') => { self.i += 1; Arg::Null }
            Some(b'*') => { self.i += 1; Arg::Derived }
            Some(b'\'') => Arg::Str(self.string()),
            Some(b'#') => {
                self.i += 1;
                let st = self.i;
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) { self.i += 1; }
                Arg::Ref(self.s[st..self.i].parse().unwrap_or(0))
            }
            Some(b'.') if matches!(self.b.get(self.i + 1), Some(c) if c.is_ascii_alphabetic()) => {
                self.i += 1;
                let st = self.i;
                while matches!(self.peek(), Some(c) if c != b'.') { self.i += 1; }
                let e = self.s[st..self.i].to_string();
                self.i += 1;
                Arg::Enum(e)
            }
            Some(b'(') => Arg::List(self.args()),
            Some(c) if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' => self.number(),
            Some(c) if c.is_ascii_alphabetic() => {
                let name = self.word();
                self.skip_ws();
                if self.peek() == Some(b'(') { Arg::Typed(name, self.args()) } else { Arg::Enum(name) }
            }
            _ => { self.i += 1; Arg::Null }
        }
    }
    fn number(&mut self) -> Arg {
        let st = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.' | b'E' | b'e') { self.i += 1; } else { break; }
        }
        let t = &self.s[st..self.i];
        if let Ok(v) = t.parse::<i64>() { return Arg::Int(v); }
        Arg::Real(t.parse::<f64>().unwrap_or(0.0))
    }
    fn string(&mut self) -> String {
        // starts at the opening quote; '' is one apostrophe
        self.i += 1;
        let st = self.i;
        let mut raw = String::new();
        let mut last = st;
        while self.i < self.b.len() {
            if self.b[self.i] == b'\'' {
                if self.b.get(self.i + 1) == Some(&b'\'') {
                    raw.push_str(&self.s[last..self.i]);
                    raw.push('\'');
                    self.i += 2;
                    last = self.i;
                    continue;
                }
                break;
            }
            self.i += 1;
        }
        raw.push_str(&self.s[last..self.i.min(self.b.len())]);
        self.i += 1;
        decode_string(&raw)
    }
}

/// STEP's escape sequences: \\ \X\HH \X2\HHHH..\X0\ \X4\HHHHHHHH..\X0\ \S\c and the \P?\ code page switches.
pub fn decode_string(s: &str) -> String {
    if !s.contains('\\') { return s.to_string(); }
    let b: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let hex = |t: &[char]| -> Option<u32> { u32::from_str_radix(&t.iter().collect::<String>(), 16).ok() };
    while i < b.len() {
        if b[i] != '\\' { out.push(b[i]); i += 1; continue; }
        // a backslash: look at what follows
        if i + 1 < b.len() && b[i + 1] == '\\' { out.push('\\'); i += 2; continue; }
        if i + 3 < b.len() && b[i + 1] == 'X' && b[i + 2] == '\\' {
            if let Some(v) = hex(&b[i + 3..(i + 5).min(b.len())]) { if let Some(c) = char::from_u32(v) { out.push(c); } }
            i += 5; continue;
        }
        if i + 3 < b.len() && b[i + 1] == 'X' && (b[i + 2] == '2' || b[i + 2] == '4') && b[i + 3] == '\\' {
            let w = if b[i + 2] == '2' { 4 } else { 8 };
            let mut j = i + 4;
            while j + w <= b.len() && !(b[j] == '\\') {
                if let Some(v) = hex(&b[j..j + w]) { if let Some(c) = char::from_u32(v) { out.push(c); } }
                j += w;
            }
            // skip the closing \X0\
            if j + 3 < b.len() && b[j] == '\\' && b[j + 1] == 'X' && b[j + 2] == '0' && b[j + 3] == '\\' { j += 4; }
            i = j; continue;
        }
        if i + 3 < b.len() && b[i + 1] == 'S' && b[i + 2] == '\\' {
            if let Some(c) = char::from_u32(b[i + 3] as u32 + 128) { out.push(c); }
            i += 4; continue;
        }
        if i + 3 < b.len() && b[i + 1] == 'P' && b[i + 3] == '\\' { i += 4; continue; }
        out.push('\\');
        i += 1;
    }
    out
}

/// What the BOM tool needs to know about a file.
#[derive(Debug, Clone, Default)]
pub struct Product {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Solid bodies this product owns directly.
    pub solids: usize,
    /// The ids of those solid bodies.
    pub solid_ids: Vec<u32>,
    /// The products placed inside it (one entry per placement, repeats included).
    pub children: Vec<u32>,
}

#[derive(Debug, Default)]
pub struct Model {
    /// Keyed by the PRODUCT_DEFINITION entity id.
    pub products: HashMap<u32, Product>,
    pub roots: Vec<u32>,
    pub has_shape_info: bool,
    pub statements: usize,
    pub skipped: usize,
}

pub fn build_model(f: &StepFile) -> Model {
    let mut m = Model { statements: f.statements, skipped: f.skipped, ..Default::default() };
    let get = |id: u32| f.entities.get(&id);
    // formation -> product
    let product_of_formation = |fid: u32| -> Option<(String, String, String)> {
        let fe = get(fid)?;
        let pid = fe.args.get(2)?.as_ref()?;
        let pe = get(pid)?;
        if pe.name != "PRODUCT" { return None; }
        let id = pe.args.first().and_then(|a| a.as_str()).unwrap_or("").to_string();
        let name = pe.args.get(1).and_then(|a| a.as_str()).unwrap_or("").to_string();
        let desc = pe.args.get(2).and_then(|a| a.as_str()).unwrap_or("").to_string();
        Some((id, name, desc))
    };
    let mut pd_ids: Vec<u32> = f.entities.iter().filter(|(_, e)| e.name == "PRODUCT_DEFINITION").map(|(k, _)| *k).collect();
    pd_ids.sort_unstable();
    for pd in &pd_ids {
        let e = &f.entities[pd];
        if let Some(fid) = e.args.get(2).and_then(|a| a.as_ref()) {
            if let Some((id, name, desc)) = product_of_formation(fid) {
                let shown = if name.trim().is_empty() { id.clone() } else { name };
                m.products.insert(*pd, Product { id, name: shown, description: desc, ..Default::default() });
            }
        }
    }
    // placements
    let mut child_of: HashSet<u32> = HashSet::new();
    let mut naus: Vec<(u32, u32, u32)> = f.entities.iter().filter(|(_, e)| e.name == "NEXT_ASSEMBLY_USAGE_OCCURRENCE")
        .filter_map(|(id, e)| Some((*id, e.args.get(3)?.as_ref()?, e.args.get(4)?.as_ref()?))).collect();
    naus.sort_unstable();
    for (_, parent, child) in &naus {
        if m.products.contains_key(parent) && m.products.contains_key(child) {
            m.products.get_mut(parent).unwrap().children.push(*child);
            child_of.insert(*child);
        }
    }
    // geometry: PRODUCT_DEFINITION_SHAPE -> product definition, SHAPE_DEFINITION_REPRESENTATION -> representation
    let mut pds_to_pd: HashMap<u32, u32> = HashMap::new();
    for (id, e) in &f.entities {
        if e.name == "PRODUCT_DEFINITION_SHAPE" {
            if let Some(d) = e.args.get(2).and_then(|a| a.as_ref()) {
                if m.products.contains_key(&d) { pds_to_pd.insert(*id, d); }
            }
        }
    }
    // plain geometry links between representations (not placements)
    let mut links: HashMap<u32, Vec<u32>> = HashMap::new();
    for (id, e) in &f.entities {
        if (e.name == "SHAPE_REPRESENTATION_RELATIONSHIP" || e.name == "REPRESENTATION_RELATIONSHIP") && !f.placement_relationships.contains(id) {
            if let (Some(a), Some(b)) = (e.args.get(2).and_then(|x| x.as_ref()), e.args.get(3).and_then(|x| x.as_ref())) {
                links.entry(a).or_default().push(b);
                links.entry(b).or_default().push(a);
            }
        }
    }
    let direct_solids = |rep: u32| -> Vec<u32> {
        match get(rep) {
            Some(r) => r.args.get(1).map(|a| a.as_list().iter().filter_map(|x| x.as_ref()).filter(|i| f.solids.contains(i)).collect()).unwrap_or_default(),
            None => Vec::new(),
        }
    };
    let mut seen_reps: HashMap<u32, HashSet<u32>> = HashMap::new();
    let mut sdrs: Vec<(u32, u32)> = f.entities.iter().filter(|(_, e)| e.name == "SHAPE_DEFINITION_REPRESENTATION")
        .filter_map(|(_, e)| Some((e.args.first()?.as_ref()?, e.args.get(1)?.as_ref()?))).collect();
    sdrs.sort_unstable();
    for (pds, rep) in sdrs {
        let pd = match pds_to_pd.get(&pds) { Some(p) => *p, None => continue };
        m.has_shape_info = true;
        let seen = seen_reps.entry(pd).or_default();
        // the representation itself, and any plain geometry representation linked to it
        let mut stack = vec![rep];
        while let Some(r) = stack.pop() {
            if !seen.insert(r) { continue; }
            if let Some(p) = m.products.get_mut(&pd) { let d = direct_solids(r); p.solids += d.len(); p.solid_ids.extend(d); }
            if let Some(ls) = links.get(&r) { for l in ls { stack.push(*l); } }
        }
    }
    let mut roots: Vec<u32> = m.products.keys().filter(|k| !child_of.contains(k)).copied().collect();
    roots.sort_unstable();
    m.roots = roots;
    m
}

/// One placed part: a body (or bodies) of a product, in the place it sits in the assembly.
#[derive(Debug, Clone)]
pub struct PartInstance {
    /// The product's own name, as written in the file.
    pub raw: String,
    /// The name worth showing: a product called SOLID takes the name of the component above it.
    pub name: String,
    /// Names from the top of the assembly down to (and including) the product.
    pub path: Vec<String>,
    /// The product definition this part belongs to (its geometry is looked up by it).
    pub pd: u32,
}

pub fn is_solid_name(n: &str) -> bool {
    let t = n.trim().to_ascii_lowercase();
    let rest = match t.strip_prefix("solid") { Some(r) => r, None => return false };
    let r = rest.trim_matches(|c: char| c.is_whitespace() || c == '_' || c == '-' || c == '(' || c == ')' || c == '[' || c == ']');
    r.is_empty() || r.chars().all(|c| c.is_ascii_digit())
}

/// Fusion exports a body that sits inside a component as "SOLID". The name that means something is the
/// component's, one level up (or further, if that is also called SOLID).
pub fn resolve_part_name(raw: &str, ancestors: &[String]) -> String {
    if !is_solid_name(raw) { return raw.to_string(); }
    for a in ancestors.iter().rev() {
        if !a.trim().is_empty() && !is_solid_name(a) { return a.clone(); }
    }
    raw.to_string()
}

#[derive(Debug, Default)]
pub struct Extracted {
    pub parts: Vec<PartInstance>,
    /// Components with no solid bodies (empty components) that were left out.
    pub empty_components: usize,
    pub cycles: usize,
}

/// Walks the assembly tree and lists every placed part. Quantities multiply down the tree: a sub-assembly
/// placed twice, holding four screws, gives eight.
pub fn extract_parts(m: &Model) -> Extracted {
    let mut out = Extracted { parts: Vec::new(), empty_components: 0, cycles: 0 };
    let mut path: Vec<u32> = Vec::new();
    for r in &m.roots { walk(m, *r, &mut path, &mut Vec::new(), &mut out); }
    out
}

fn walk(m: &Model, pd: u32, path: &mut Vec<u32>, names: &mut Vec<String>, out: &mut Extracted) {
    if path.contains(&pd) { out.cycles += 1; return; }
    let p = match m.products.get(&pd) { Some(p) => p, None => return };
    path.push(pd);
    names.push(p.name.clone());
    // with no geometry information anywhere in the file, every leaf counts as one part
    let own = if m.has_shape_info { p.solids } else if p.children.is_empty() { 1 } else { 0 };
    if own > 0 {
        // a body-only product called SOLID takes its component's name; a product's own real name stays
        let ancestors = &names[..names.len() - 1];
        let name = resolve_part_name(&p.name, ancestors);
        for _ in 0..own {
            out.parts.push(PartInstance { raw: p.name.clone(), name: name.clone(), path: names.clone(), pd });
        }
    } else if p.children.is_empty() {
        out.empty_components += 1;
    }
    for c in &p.children { walk(m, *c, path, names, out); }
    names.pop();
    path.pop();
}

/// Reads a file and returns every placed part.
pub fn read_step_file(path: &std::path::Path) -> Result<(Model, Extracted), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read the file: {e}"))?;
    let text = String::from_utf8_lossy(&bytes);
    let f = parse(&text)?;
    let model = build_model(&f);
    if model.products.is_empty() {
        return Err("No products were found in this STEP file, so there is nothing to list.".into());
    }
    let ex = extract_parts(&model);
    Ok((model, ex))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_and_escapes() {
        assert_eq!(decode_string("plain"), "plain");
        assert_eq!(decode_string("a\\\\b"), "a\\b");
        assert_eq!(decode_string("M3\\X\\D7 8"), "M3\u{d7} 8"); // ×
        assert_eq!(decode_string("\\X2\\00C400D6\\X0\\"), "ÄÖ");
        assert_eq!(decode_string("caf\\S\\i"), "caf\u{e9}");
    }

    #[test]
    fn parses_arguments() {
        let f = parse("ISO-10303-21;\nDATA;\n#5 = PRODUCT('It''s; a name','n2',$,(#8,#9));\n#6 = MANIFOLD_SOLID_BREP('',#7);\nENDSEC;").unwrap();
        let e = &f.entities[&5];
        assert_eq!(e.name, "PRODUCT");
        assert_eq!(e.args[0].as_str(), Some("It's; a name"));
        assert_eq!(e.args[2], Arg::Null);
        assert_eq!(e.args[3].as_list().len(), 2);
        assert!(f.solids.contains(&6));
    }

    #[test]
    fn complex_entities_and_comments() {
        let f = parse("DATA;\n/* a comment; with a semicolon */\n#1 = (REPRESENTATION_RELATIONSHIP('a','',#2,#3)REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#4)SHAPE_REPRESENTATION_RELATIONSHIP());\nENDSEC;").unwrap();
        assert!(f.placement_relationships.contains(&1));
        assert_eq!(f.entities[&1].args[2].as_ref(), Some(2));
    }

    #[test]
    fn solid_names() {
        for n in ["SOLID", "solid", "Solid1", "SOLID (2)", "SOLID_3", " SOLID "] { assert!(is_solid_name(n), "{n}"); }
        for n in ["Solid state relay", "Solidify", "M3 screw", ""] { assert!(!is_solid_name(n), "{n}"); }
        let up = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(resolve_part_name("SOLID", &up(&["Gantry", "Idler Bracket v3"])), "Idler Bracket v3");
        assert_eq!(resolve_part_name("SOLID", &up(&["Gantry", "Cable Guide", "SOLID"])), "Cable Guide");
        assert_eq!(resolve_part_name("SOLID", &up(&[])), "SOLID");
        assert_eq!(resolve_part_name("M3 screw", &up(&["Gantry"])), "M3 screw");
    }

    #[test]
    fn not_a_step_file() {
        assert!(parse("hello world").is_err());
    }
}

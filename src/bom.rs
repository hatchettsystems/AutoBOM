//! Turning a list of placed parts into proposed bill-of-materials entries, and keeping a bill of materials
//! up to date when the model changes. The rules are the same as the BladeDocs website's Auto BOM tool:
//!
//! * identical names are counted together;
//! * mirrored copies ("(Mirror)", "mirrored") and numbered copies ("<2>", "(2)", "#2", ":2") are tallied with the original;
//! * names that only look alike ("Post L" / "Post R", "Bracket-2") are put to the user as a question;
//! * components taken out of an entry go back into the queue as entries of their own.

use crate::money::{natural_cmp, norm_cost, parse_cost};
use crate::step::PartInstance;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

pub const CATEGORIES: [(&str, &str); 3] = [("purchased", "Purchased"), ("printed", "Printed"), ("modified", "Modified")];

pub fn category_label(c: &str) -> &'static str {
    CATEGORIES.iter().find(|x| x.0 == c).map(|x| x.1).unwrap_or("Purchased")
}

/// A line of the bill of materials.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Line {
    pub part: String,
    pub qty: String,
    #[serde(default = "default_cat")]
    pub cat: String,
    #[serde(default)]
    pub cost: String,
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub note: String,
    /// Made by the tool (so it can be flagged if its parts disappear). Lines you type in by hand are never flagged.
    #[serde(default)]
    pub auto: bool,
    /// The part names (lower case) from the STEP file this line stands for.
    #[serde(default)]
    pub covers: Vec<String>,
}

fn default_cat() -> String { "purchased".into() }

impl Line {
    pub fn qty_num(&self) -> Option<u64> { self.qty.trim().parse().ok() }
    pub fn line_total(&self) -> Option<f64> {
        Some(parse_cost(&self.cost)? * self.qty_num()? as f64)
    }
}

pub fn sort_lines(lines: &mut [Line]) {
    lines.sort_by(|a, b| natural_cmp(&a.part, &b.part));
}

/// What the tool has been told, kept between runs so questions are not asked twice.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Prefs {
    /// Names (lower case) you said belong with the part they look like.
    #[serde(default)]
    pub merged: Vec<String>,
    /// Names (lower case) you said are separate parts.
    #[serde(default)]
    pub separate: Vec<String>,
    /// Entries you chose to ignore for good (group keys).
    #[serde(default)]
    pub ignored: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Flags {
    pub mirror: bool,
    pub inst: bool,
    pub unsure: bool,
}

fn re(cell: &'static OnceLock<Regex>, p: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(p).unwrap())
}

/// Strips instance counters and mirror markers from a name and reports what was found.
pub fn bom_base(name: &str) -> (String, Flags) {
    static INST: OnceLock<Regex> = OnceLock::new();
    static COPY: OnceLock<Regex> = OnceLock::new();
    static MIRR_END: OnceLock<Regex> = OnceLock::new();
    static MIRR_START: OnceLock<Regex> = OnceLock::new();
    static LR: OnceLock<Regex> = OnceLock::new();
    static NUM: OnceLock<Regex> = OnceLock::new();
    let mut t = name.trim().to_string();
    let mut f = Flags::default();
    for _ in 0..4 {
        let before = t.clone();
        let r = re(&INST, r"(?i)\s*(?:<\d+>|\(\d+\)|[:#]\d+)\s*$");
        if r.is_match(&t) { f.inst = true; t = r.replace(&t, "").to_string(); }
        let r = re(&COPY, r"(?i)\s*[\(\[]?\bcopy(?:\s*\d+)?\b[\)\]]?\s*$");
        if r.is_match(&t) { f.inst = true; t = r.replace(&t, "").to_string(); }
        let r = re(&MIRR_END, r"(?i)[\s_-]*[\(\[]?\b(?:mirror(?:ed|red)?|mirr?)\b[\)\]]?\s*$");
        if r.is_match(&t) { f.mirror = true; t = r.replace(&t, "").to_string(); }
        let r = re(&MIRR_START, r"(?i)^\s*[\(\[]?\bmirror(?:ed|red)?\b[\)\]]?[\s_-]+");
        if r.is_match(&t) { f.mirror = true; t = r.replace(&t, "").to_string(); }
        let r = re(&LR, r"(?i)([A-Za-z\)])(?:[\s_-]+|\s*\()(?:left|right|lh|rh|l|r)\)?\s*$");
        if r.is_match(&t) { f.unsure = true; t = r.replace(&t, "$1").to_string(); }
        let r = re(&NUM, r"([A-Za-z])[_-]\d{1,2}\s*$");
        if r.is_match(&t) { f.unsure = true; t = r.replace(&t, "$1").to_string(); }
        if t == before { break; }
    }
    (t.split_whitespace().collect::<Vec<_>>().join(" "), f)
}

/// Tidies a CAD name for display: drops ".f3d" and turns underscores into spaces; long standards-style hardware
/// names ("ANSI B18.3 - M3 ... Steel ...") are cut down to the part that matters.
pub fn tidy(name: &str) -> String {
    static F3D: OnceLock<Regex> = OnceLock::new();
    static HW: OnceLock<Regex> = OnceLock::new();
    let n = re(&F3D, r"(?i)\.f3d\s*$").replace(name, "").trim().to_string();
    let hw = re(&HW, r"(?i)^(.*?)\s*(?:ANSI|ASME|ISO|DIN)\s+[A-Z0-9.\- ]+?\s*-\s*(M\d.*?)\s+(?:Steel|Stainless|Grade|Alloy|Brass|Aluminum|Zinc).*$");
    if let Some(c) = hw.captures(&n) {
        let s = format!("{} {}", &c[1], &c[2]).replace("Forged ", "").replace("Hexagon ", "");
        return s.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    n.replace('_', " ")
}

#[derive(Debug, Clone)]
pub struct Member {
    pub name: String,
    pub flags: Flags,
    pub count: usize,
    /// A few example places this part sits in the assembly.
    pub paths: Vec<String>,
}

impl Member {
    pub fn tag(&self) -> &'static str {
        if self.flags.unsure { "similar, please confirm" } else if self.flags.mirror { "mirrored copy" } else if self.flags.inst { "numbered copy" } else { "" }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action { Pending, Added, Updated, Skipped, Ignored }

#[derive(Debug, Clone, Default)]
pub struct Form {
    pub part: String,
    pub qty: String,
    pub cat: String,
    pub cost: String,
    pub urls: Vec<String>,
    pub desc: String,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct Group {
    pub key: String,
    pub name: String,
    pub members: Vec<Member>,
    /// Members still waiting for an answer to "are these the same part?".
    pub ask: Vec<usize>,
    pub answer: Option<bool>,
    pub sel: Vec<bool>,
    pub action: Action,
    pub form: Form,
    pub form_ready: bool,
    pub qty_auto: bool,
    /// The existing BOM line this entry updates.
    pub line: Option<usize>,
    /// What the existing line said before (for the "BOM says X, the model has Y" note).
    pub prev_qty: String,
    pub orphan: Option<usize>,
}

impl Group {
    pub fn new(key: String, name: String, members: Vec<Member>, ask: Vec<usize>) -> Group {
        let sel = members.iter().enumerate().map(|(i, _)| !ask.contains(&i)).collect();
        let answer = if ask.is_empty() { Some(true) } else { None };
        Group { key, name, members, ask, answer, sel, action: Action::Pending, form: Form::default(), form_ready: false, qty_auto: true, line: None, prev_qty: String::new(), orphan: None }
    }
    pub fn qty(&self) -> usize {
        self.members.iter().zip(&self.sel).filter(|(_, s)| **s).map(|(m, _)| m.count).sum()
    }
    pub fn pending_question(&self) -> bool { !self.ask.is_empty() && self.answer.is_none() }
    pub fn selected_names(&self) -> Vec<String> {
        self.members.iter().zip(&self.sel).filter(|(_, s)| **s).map(|(m, _)| m.name.to_lowercase()).collect()
    }
}

fn natural_group_cmp(a: &Group, b: &Group) -> std::cmp::Ordering { natural_cmp(&a.name, &b.name) }

/// Groups the parts into proposed entries.
pub fn plan(parts: &[PartInstance], prefs: &Prefs) -> Vec<Group> {
    // unique names, with how many of each and a few places they sit
    let mut uniq: BTreeMap<String, Member> = BTreeMap::new();
    for p in parts {
        let name = tidy(&p.name);
        let e = uniq.entry(name.clone()).or_insert_with(|| Member { name: name.clone(), flags: Flags::default(), count: 0, paths: Vec::new() });
        e.count += 1;
        if e.paths.len() < 3 {
            let path = p.path.iter().take(p.path.len().saturating_sub(1)).cloned().collect::<Vec<_>>().join(" / ");
            if !path.is_empty() && !e.paths.contains(&path) { e.paths.push(path); }
        }
    }
    let mut map: BTreeMap<String, (Vec<Member>, Vec<String>)> = BTreeMap::new();
    for (name, mut m) in uniq {
        let lower = name.to_lowercase();
        let (base, mut f) = bom_base(&name);
        let mut key = base.to_lowercase();
        if prefs.separate.contains(&lower) {
            // you said this one is its own part
            key = lower.clone();
            f = Flags::default();
        } else if prefs.merged.contains(&lower) {
            f.unsure = false;
        }
        m.flags = f;
        let e = map.entry(key).or_default();
        e.0.push(m);
        e.1.push(base);
    }
    let mut groups = Vec::new();
    for (key, (members, bases)) in map {
        if prefs.ignored.contains(&key) { continue; }
        let mut ask: Vec<usize> = members.iter().enumerate().filter(|(_, m)| m.flags.unsure).map(|(i, _)| i).collect();
        let names: HashSet<String> = members.iter().map(|m| m.name.to_lowercase()).collect();
        if !ask.is_empty() && ask.len() == members.len() && names.len() == 1 { ask.clear(); }
        // name the entry after a plain member (no mirror/numbered marker) where there is one
        let mut best = 0usize;
        let mut best_score = usize::MAX;
        for (i, m) in members.iter().enumerate() {
            if ask.contains(&i) { continue; }
            let s = m.flags.mirror as usize + m.flags.inst as usize;
            if s < best_score { best_score = s; best = i; }
        }
        let name = if best_score == usize::MAX { bases[0].clone() } else { bases[best].clone() };
        groups.push(Group::new(key, if name.is_empty() { members[0].name.clone() } else { name }, members, ask));
    }
    groups.sort_by(natural_group_cmp);
    groups
}

/// Finds the existing BOM line an entry belongs to, by the part names it covers or by its name.
pub fn find_line(g: &Group, lines: &[Line], taken: &HashSet<usize>) -> Option<usize> {
    let names: HashSet<String> = g.members.iter().map(|m| m.name.to_lowercase()).collect();
    if let Some(i) = lines.iter().enumerate().position(|(i, l)| !taken.contains(&i) && l.covers.iter().any(|c| names.contains(c))) { return Some(i); }
    lines.iter().enumerate().position(|(i, l)| !taken.contains(&i) && bom_base(&l.part).0.to_lowercase() == g.key)
}

/// A line that already says exactly what the model says needs no attention.
pub fn is_up_to_date(g: &Group, l: &Line) -> bool {
    if g.pending_question() { return false; }
    let covers: HashSet<&String> = l.covers.iter().collect();
    g.members.iter().all(|m| covers.contains(&m.name.to_lowercase())) && l.qty_num() == Some(g.qty() as u64) && l.covers.iter().all(|c| g.members.iter().any(|m| &m.name.to_lowercase() == c))
}

/// An entry's form, filled from the existing line when there is one.
pub fn prefill(g: &mut Group, lines: &[Line]) {
    if g.form_ready { return; }
    g.form_ready = true;
    match g.line.and_then(|i| lines.get(i)) {
        Some(l) => {
            g.prev_qty = l.qty.clone();
            g.form = Form { part: if l.part.is_empty() { g.name.clone() } else { l.part.clone() }, qty: String::new(), cat: l.cat.clone(), cost: l.cost.clone(), urls: l.urls.clone(), desc: l.desc.clone(), note: l.note.clone() };
        }
        None => g.form = Form { part: g.name.clone(), cat: "purchased".into(), ..Default::default() },
    }
    if g.qty_auto { g.form.qty = g.qty().to_string(); }
}

/// Writes a confirmed entry into the bill of materials. Returns whether it added a line or updated one.
pub fn apply(g: &mut Group, lines: &mut Vec<Line>) -> Action {
    let f = &g.form;
    let covers = g.selected_names();
    let urls: Vec<String> = f.urls.iter().map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).take(5).collect();
    let line = Line { part: f.part.trim().to_string(), qty: f.qty.trim().to_string(), cat: if f.cat.is_empty() { "purchased".into() } else { f.cat.clone() }, cost: norm_cost(&f.cost), urls, desc: f.desc.clone(), note: f.note.clone(), auto: true, covers };
    match g.line.filter(|i| *i < lines.len()) {
        Some(i) => {
            // keep any names the line already covered that are still wanted nowhere else
            lines[i] = line;
            g.action = Action::Updated;
        }
        None => {
            lines.push(line);
            g.line = Some(lines.len() - 1);
            g.action = Action::Added;
        }
    }
    g.action
}

/// Components taken out of an entry become entries of their own. Returns the new entries.
pub fn requeue_deselected(g: &mut Group) -> Vec<Group> {
    let mut left: Vec<Member> = Vec::new();
    let mut keep: Vec<Member> = Vec::new();
    let mut keep_sel = Vec::new();
    for (m, s) in g.members.drain(..).zip(g.sel.drain(..)) {
        if s { keep.push(m); keep_sel.push(true); } else { left.push(m); }
    }
    g.members = keep;
    g.sel = keep_sel;
    g.ask.clear();
    left.into_iter().map(|mut m| {
        m.flags = Flags::default();
        let key = m.name.to_lowercase();
        let name = m.name.clone();
        Group::new(key, name, vec![m], Vec::new())
    }).collect()
}

/// When the user answers "no, keep them separate": the look-alikes become entries of their own.
pub fn split_unsure(g: &mut Group) -> Vec<Group> {
    let ask: Vec<usize> = g.ask.clone();
    let mut out = Vec::new();
    let mut keep = Vec::new();
    let mut keep_sel = Vec::new();
    for (i, (m, s)) in g.members.drain(..).zip(g.sel.drain(..)).enumerate() {
        if ask.contains(&i) {
            let mut m = m;
            m.flags = Flags::default();
            let key = m.name.to_lowercase();
            let name = m.name.clone();
            out.push(Group::new(key, name, vec![m], Vec::new()));
        } else { keep.push(m); keep_sel.push(s); }
    }
    g.members = keep;
    g.sel = keep_sel;
    g.ask.clear();
    g.answer = Some(true);
    out
}

/// Lines made by the tool whose parts are no longer in the model at all.
pub fn orphans(lines: &[Line], parts_lower: &HashSet<String>) -> Vec<usize> {
    lines.iter().enumerate().filter(|(_, l)| l.auto && !l.covers.is_empty() && l.covers.iter().all(|c| !parts_lower.contains(c))).map(|(i, _)| i).collect()
}

/// Totals for the footer: (this list, all lists), each None when a priced line has no usable quantity.
pub fn totals(lines: &[Line], cat: Option<&str>) -> (f64, bool) {
    let mut sum = 0.0;
    let mut all = true;
    for l in lines.iter().filter(|l| cat.map(|c| l.cat == c).unwrap_or(true)) {
        match l.line_total() { Some(t) => sum += t, None => if !l.cost.trim().is_empty() { all = false } }
    }
    (sum, all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(name: &str) -> PartInstance { PartInstance { raw: name.into(), name: name.into(), path: vec!["Root".into(), name.into()] } }
    fn parts(v: &[(&str, usize)]) -> Vec<PartInstance> { v.iter().flat_map(|(n, c)| std::iter::repeat(part(n)).take(*c)).collect() }

    #[test]
    fn base_names() {
        for (n, base, mirror, inst, unsure) in [
            ("M3 screw", "M3 screw", false, false, false), ("M3 screw (Mirror)", "M3 screw", true, false, false),
            ("M3 screw <2>", "M3 screw", false, true, false), ("M3 screw L", "M3 screw", false, false, true),
            ("Post (mirrored)", "Post", true, false, false), ("M3x4(Mirror)", "M3x4", true, false, false),
            ("Bracket-2", "Bracket", false, false, true), ("Bearing 608", "Bearing 608", false, false, false),
            ("M3 x 8", "M3 x 8", false, false, false), ("Idler:3", "Idler", false, true, false),
        ] {
            let (b, f) = bom_base(n);
            assert_eq!((b.as_str(), f.mirror, f.inst, f.unsure), (base, mirror, inst, unsure), "{n}");
        }
    }

    #[test]
    fn groups_and_questions() {
        let g = plan(&parts(&[("M3x4", 4), ("M3x4(Mirror)", 2), ("M3x4 <2>", 1), ("Post L", 1), ("Post R", 1), ("Base plate", 1)]), &Prefs::default());
        assert_eq!(g.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["Base plate", "M3x4", "Post"]);
        let screws = &g[1];
        assert_eq!(screws.qty(), 7);
        assert!(!screws.pending_question());
        let post = &g[2];
        assert!(post.pending_question());
        assert_eq!(post.qty(), 0); // nothing counted until answered
    }

    #[test]
    fn answers_are_remembered() {
        let p = parts(&[("Post L", 1), ("Post R", 1)]);
        let merged = plan(&p, &Prefs { merged: vec!["post l".into(), "post r".into()], ..Default::default() });
        assert_eq!(merged.len(), 1);
        assert!(!merged[0].pending_question());
        assert_eq!(merged[0].qty(), 2);
        let sep = plan(&p, &Prefs { separate: vec!["post l".into(), "post r".into()], ..Default::default() });
        assert_eq!(sep.len(), 2);
    }

    #[test]
    fn one_name_all_unsure_is_not_a_question() {
        let g = plan(&parts(&[("Bracket L", 3)]), &Prefs::default());
        assert_eq!(g.len(), 1);
        assert!(!g[0].pending_question());
        assert_eq!(g[0].qty(), 3);
    }

    #[test]
    fn deselected_parts_return_to_the_queue() {
        let mut g = plan(&parts(&[("M3x4", 4), ("M3x4(Mirror)", 2)]), &Prefs::default()).remove(0);
        let i = g.members.iter().position(|m| m.name.contains("Mirror")).unwrap();
        g.sel[i] = false;
        assert_eq!(g.qty(), 4);
        let back = requeue_deselected(&mut g);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, "M3x4(Mirror)");
        assert_eq!(back[0].qty(), 2);
        assert_eq!(g.members.len(), 1);
    }

    #[test]
    fn rerun_keeps_links_and_updates_quantities() {
        let mut lines: Vec<Line> = Vec::new();
        let mut g = plan(&parts(&[("M3x4", 4), ("Nut", 2)]), &Prefs::default());
        for x in g.iter_mut() { prefill(x, &lines); }
        g[0].form.cost = ".62".into();
        g[0].form.urls = vec!["https://shop.example/m3".into()];
        for x in g.iter_mut() { apply(x, &mut lines); }
        assert_eq!(lines.len(), 2);
        assert_eq!(lines.iter().find(|l| l.part == "M3x4").unwrap().cost, "$0.62");
        // the model gains two screws and loses the nut
        let mut g2 = plan(&parts(&[("M3x4", 6)]), &Prefs::default());
        let mut taken = HashSet::new();
        g2[0].line = find_line(&g2[0], &lines, &taken);
        taken.insert(g2[0].line.unwrap());
        assert!(!is_up_to_date(&g2[0], &lines[g2[0].line.unwrap()]));
        prefill(&mut g2[0], &lines);
        assert_eq!(g2[0].form.qty, "6");
        assert_eq!(g2[0].prev_qty, "4");
        assert_eq!(g2[0].form.cost, "$0.62");
        assert_eq!(g2[0].form.urls, vec!["https://shop.example/m3".to_string()]);
        let parts_lower: HashSet<String> = ["m3x4".to_string()].into_iter().collect();
        let gone = orphans(&lines, &parts_lower);
        assert_eq!(gone.len(), 1);
        assert_eq!(lines[gone[0]].part, "Nut");
        apply(&mut g2[0], &mut lines);
        assert_eq!(lines.iter().find(|l| l.part == "M3x4").unwrap().qty, "6");
        assert_eq!(lines.iter().find(|l| l.part == "M3x4").unwrap().cost, "$0.62");
    }

    #[test]
    fn totals_use_the_right_numbers() {
        let l = |p: &str, q: &str, c: &str, cat: &str| Line { part: p.into(), qty: q.into(), cost: c.into(), cat: cat.into(), ..Default::default() };
        let lines = vec![l("a", "3", "$0.62", "purchased"), l("b", "2", "$10.00", "printed"), l("c", "1", "free", "purchased")];
        let (all, ok) = totals(&lines, None);
        assert!((all - 21.86).abs() < 1e-9 && !ok);
        let (p, ok) = totals(&lines, Some("printed"));
        assert!((p - 20.0).abs() < 1e-9 && ok);
    }

    #[test]
    fn tidy_names() {
        assert_eq!(tidy("Bracket_left.f3d"), "Bracket left");
        assert_eq!(tidy("Hex Nut"), "Hex Nut");
    }
}

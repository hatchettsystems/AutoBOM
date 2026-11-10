//! A bill of materials on disk: the project file the tool keeps between runs, and CSV / JSON in and out.
//! The CSV is the same layout the BladeDocs website exports, so the two can be swapped freely.

use crate::bom::{category_label, Line, Prefs, CATEGORIES};
use crate::money::{fmt_money, norm_cost, parse_cost, cost_symbol};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Project {
    /// The STEP file this bill of materials was last built from (shown for reference only).
    #[serde(default)]
    pub step_path: String,
    #[serde(default)]
    pub lines: Vec<Line>,
    #[serde(default)]
    pub prefs: Prefs,
}

impl Project {
    pub fn load(path: &Path) -> Result<Project, String> {
        let t = std::fs::read_to_string(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        serde_json::from_str(&t).map_err(|e| format!("That is not a project file this tool can read: {e}"))
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let t = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, t).map_err(|e| format!("Could not save {}: {e}", path.display()))
    }
}

/// Spreadsheet programs run text that starts with = + - @ as a formula, so such cells are defused.
fn cell(v: &str) -> String {
    let mut t = v.to_string();
    if t.starts_with(|c| matches!(c, '=' | '+' | '-' | '@' | '\t' | '\r')) { t.insert(0, '\''); }
    if t.contains(['"', ',', '\n', '\r']) { format!("\"{}\"", t.replace('"', "\"\"")) } else { t }
}

pub fn to_csv(lines: &[Line]) -> String {
    let nl = lines.iter().map(|l| l.urls.len()).max().unwrap_or(1).max(1);
    let mut head: Vec<String> = ["List", "Part", "Quantity", "Description", "Unit cost", "Line total"].iter().map(|s| s.to_string()).collect();
    for i in 0..nl { head.push(format!("Link {}", i + 1)); }
    head.push("Used in steps".into());
    head.push("Note".into());
    let mut out = String::from("\u{feff}");
    out.push_str(&head.iter().map(|h| cell(h)).collect::<Vec<_>>().join(","));
    out.push_str("\r\n");
    let mut sorted: Vec<&Line> = lines.iter().collect();
    sorted.sort_by(|a, b| crate::money::natural_cmp(&a.part, &b.part));
    for l in sorted {
        let total = l.line_total().map(|t| fmt_money(t, &cost_symbol(&l.cost))).unwrap_or_default();
        let mut row = vec![category_label(&l.cat).to_string(), l.part.clone(), l.qty.clone(), l.desc.clone(), norm_cost(&l.cost), total];
        for i in 0..nl { row.push(l.urls.get(i).cloned().unwrap_or_default()); }
        row.push(String::new());
        row.push(l.note.clone());
        out.push_str(&row.iter().map(|c| cell(c)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    out
}

/// A minimal CSV reader: quotes, doubled quotes, commas and newlines inside quotes, CRLF.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_q = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_q {
            if c == '"' { if chars.peek() == Some(&'"') { field.push('"'); chars.next(); } else { in_q = false; } } else { field.push(c); }
        } else {
            match c {
                '"' => in_q = true,
                ',' => { row.push(std::mem::take(&mut field)); }
                '\r' => {}
                '\n' => { row.push(std::mem::take(&mut field)); rows.push(std::mem::take(&mut row)); }
                _ => field.push(c),
            }
        }
    }
    if !field.is_empty() || !row.is_empty() { row.push(field); rows.push(row); }
    rows.into_iter().filter(|r| r.iter().any(|c| !c.trim().is_empty())).collect()
}

/// Reads a bill of materials CSV (this tool's, or the website's) into lines. Lines you import have no link to
/// STEP parts yet; they are matched to parts by name the next time a STEP file is read.
pub fn from_csv(text: &str) -> Result<Vec<Line>, String> {
    let rows = parse_csv(text);
    let head = rows.first().ok_or("That file is empty.")?;
    let col = |name: &str| head.iter().position(|h| h.trim().eq_ignore_ascii_case(name));
    let part = col("Part").ok_or("This does not look like a bill of materials: there is no \"Part\" column.")?;
    let (qty, desc, cost, note, list) = (col("Quantity"), col("Description"), col("Unit cost"), col("Note"), col("List"));
    let links: Vec<usize> = head.iter().enumerate().filter(|(_, h)| h.trim().to_ascii_lowercase().starts_with("link")).map(|(i, _)| i).collect();
    let get = |r: &Vec<String>, i: Option<usize>| -> String {
        let v = i.and_then(|i| r.get(i)).cloned().unwrap_or_default();
        match v.strip_prefix('\'') { Some(rest) if rest.starts_with(|c| matches!(c, '=' | '+' | '-' | '@')) => rest.to_string(), _ => v }
    };
    let mut out = Vec::new();
    for r in &rows[1..] {
        let p = get(r, Some(part));
        if p.trim().is_empty() { continue; }
        let label = get(r, list).to_lowercase();
        let cat = CATEGORIES.iter().find(|c| c.1.to_lowercase() == label || c.0 == label).map(|c| c.0).unwrap_or("purchased").to_string();
        let urls: Vec<String> = links.iter().map(|i| get(r, Some(*i)).trim().to_string()).filter(|u| u.starts_with("https://")).take(5).collect();
        out.push(Line { part: p.trim().to_string(), qty: get(r, qty).trim().to_string(), cat, cost: norm_cost(&get(r, cost)), urls, desc: get(r, desc), note: get(r, note), auto: false, covers: Vec::new() });
    }
    if out.is_empty() { return Err("That file has no bill of materials lines.".into()); }
    Ok(out)
}

/// The same lines in the field names the BladeDocs BOM uses.
pub fn to_json(lines: &[Line]) -> String {
    let mut sorted: Vec<&Line> = lines.iter().collect();
    sorted.sort_by(|a, b| crate::money::natural_cmp(&a.part, &b.part));
    let v: Vec<serde_json::Value> = sorted.iter().map(|l| serde_json::json!({
        "part": l.part, "qty": l.qty, "cat": l.cat, "cost": norm_cost(&l.cost), "url": l.urls.first().cloned().unwrap_or_default(),
        "urls": l.urls, "desc": l.desc, "note": l.note,
    })).collect();
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

/// A plain-text table for the console.
pub fn to_text(lines: &[Line]) -> String {
    let mut sorted: Vec<&Line> = lines.iter().collect();
    sorted.sort_by(|a, b| crate::money::natural_cmp(&a.part, &b.part));
    let w = sorted.iter().map(|l| l.part.chars().count()).max().unwrap_or(4).clamp(4, 48);
    let mut s = String::new();
    for l in sorted {
        let name: String = l.part.chars().take(48).collect();
        let tot = parse_cost(&l.cost).map(|_| "").unwrap_or("");
        s.push_str(&format!("{:>5}  {:<w$}  {:<10} {}{}\n", l.qty, name, category_label(&l.cat), norm_cost(&l.cost), tot, w = w));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_round_trip() {
        let lines = vec![
            Line { part: "M3 x 8, black".into(), qty: "24".into(), cat: "purchased".into(), cost: "$0.10".into(), urls: vec!["https://a.example/1".into(), "https://b.example/2".into()], desc: "He said \"hi\"".into(), note: "=danger".into(), ..Default::default() },
            Line { part: "Duct".into(), qty: "1".into(), cat: "printed".into(), ..Default::default() },
        ];
        let csv = to_csv(&lines);
        assert!(csv.starts_with('\u{feff}'));
        assert!(csv.contains("'=danger"), "formulas are defused");
        assert!(csv.contains("$2.40"), "line totals are written");
        let back = from_csv(&csv).unwrap();
        assert_eq!(back.len(), 2);
        let m3 = back.iter().find(|l| l.part.starts_with("M3")).unwrap();
        assert_eq!(m3.part, "M3 x 8, black");
        assert_eq!(m3.urls.len(), 2);
        assert_eq!(m3.desc, "He said \"hi\"");
        assert_eq!(m3.note, "=danger");
        assert_eq!(back.iter().find(|l| l.part == "Duct").unwrap().cat, "printed");
    }

    #[test]
    fn reads_the_website_export() {
        let csv = "\u{feff}List,Part,Quantity,Description,Unit cost,Line total,Link 1,Link 2,Used in steps,Note\r\nPurchased,Bearing 608,8,,.5,$4.00,https://x.example/b,,1 2,\r\n";
        let l = from_csv(csv).unwrap();
        assert_eq!(l[0].cost, "$0.50");
        assert_eq!(l[0].urls, vec!["https://x.example/b".to_string()]);
    }

    #[test]
    fn project_files() {
        let dir = std::env::temp_dir().join(format!("autobom-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = Project { step_path: "x.step".into(), lines: vec![Line { part: "A".into(), qty: "1".into(), ..Default::default() }], prefs: Prefs { merged: vec!["a".into()], ..Default::default() } };
        let f = dir.join("t.autobom.json");
        p.save(&f).unwrap();
        let q = Project::load(&f).unwrap();
        assert_eq!(q.lines.len(), 1);
        assert_eq!(q.prefs.merged, vec!["a".to_string()]);
        assert!(Project::load(&dir.join("missing.json")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

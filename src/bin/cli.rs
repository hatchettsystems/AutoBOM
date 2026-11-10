//! The command-line version: reads a STEP file and writes the bill of materials without any window.
//!
//!   autobom-cli model.step --csv bom.csv
//!   autobom-cli model.step --project my.autobom.json --save my.autobom.json --csv bom.csv --json bom.json
//!
//! Entries with no open question are accepted as proposed. Look-alike names (Post L / Post R) are left alone
//! unless you say --merge-similar (count them together) or --keep-similar (separate lines).

use autobom::bom::Prefs;
use autobom::project::{from_csv, to_csv, to_json, to_text, Project};
use autobom::session::Session;
use autobom::step::read_step_file;
use std::path::PathBuf;
use std::process::exit;

const HELP: &str = "autobom-cli: build a bill of materials from a STEP file

USAGE
  autobom-cli <model.step> [options]

OPTIONS
  --project <file>     start from a saved project (keeps costs, links, descriptions, and earlier answers)
  --import <bom.csv>   start from a BOM CSV (this tool's or the BladeDocs website's)
  --save <file>        write the project file when done
  --csv <file>         write the BOM as CSV (same layout as the website's export)
  --json <file>        write the BOM as JSON
  --merge-similar      count look-alike names (Post L / Post R) together
  --keep-similar       keep look-alike names as separate lines
  --parts              list every part found in the file and exit
  -h, --help           this text

Run with no arguments to open the window version (autobom).";

fn fail(m: &str) -> ! { eprintln!("error: {m}"); exit(1) }

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut step, mut project, mut import, mut save, mut csv, mut json) = (None, None, None, None, None, None);
    let (mut merge, mut keep, mut list_parts) = (false, false, false);
    while let Some(a) = args.next() {
        let mut next = |what: &str| args.next().unwrap_or_else(|| fail(&format!("{a} needs {what}")));
        match a.as_str() {
            "-h" | "--help" => { println!("{HELP}"); return; }
            "--project" => project = Some(PathBuf::from(next("a file"))),
            "--import" => import = Some(PathBuf::from(next("a file"))),
            "--save" => save = Some(PathBuf::from(next("a file"))),
            "--csv" => csv = Some(PathBuf::from(next("a file"))),
            "--json" => json = Some(PathBuf::from(next("a file"))),
            "--merge-similar" => merge = true,
            "--keep-similar" => keep = true,
            "--parts" => list_parts = true,
            s if s.starts_with('-') => fail(&format!("unknown option {s}. Try --help")),
            _ => step = Some(PathBuf::from(a)),
        }
    }
    let step = step.unwrap_or_else(|| { println!("{HELP}"); exit(2) });
    let (model, ex) = read_step_file(&step).unwrap_or_else(|e| fail(&e));
    eprintln!("Read {} products, {} placed parts ({} empty components left out).", model.products.len(), ex.parts.len(), ex.empty_components);
    if ex.cycles > 0 { eprintln!("Warning: {} circular placements were ignored.", ex.cycles); }
    if list_parts {
        let mut names: std::collections::BTreeMap<String, usize> = Default::default();
        for p in &ex.parts { *names.entry(p.name.clone()).or_default() += 1; }
        for (n, c) in names { println!("{c:>5}  {n}"); }
        return;
    }
    let mut proj = match (&project, &import) {
        (Some(p), _) => Project::load(p).unwrap_or_else(|e| fail(&e)),
        (None, Some(c)) => {
            let t = std::fs::read_to_string(c).unwrap_or_else(|e| fail(&format!("could not read {}: {e}", c.display())));
            Project { lines: from_csv(&t).unwrap_or_else(|e| fail(&e)), prefs: Prefs::default(), ..Default::default() }
        }
        _ => Project::default(),
    };
    proj.step_path = step.display().to_string();
    let mut s = Session::start(proj, &ex.parts);
    // look-alikes: answer them if asked to
    let mut open: Vec<String> = Vec::new();
    for k in 0..s.queue.len() {
        s.i = k;
        if s.queue[k].pending_question() {
            if merge { s.answer_yes(); } else if keep { s.answer_no(); }
        }
    }
    s.i = 0;
    s.finished = s.queue.is_empty();
    let done = s.accept_remaining();
    for g in &s.queue {
        if g.orphan.is_some() { open.push(format!("  line \"{}\": its parts are not in this file any more (kept as is)", g.name)); }
        else if g.action == autobom::bom::Action::Pending { open.push(format!("  \"{}\": are these the same part? {}", g.name, g.members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(" / "))); }
    }
    eprintln!("{done} entries confirmed, {} already up to date.", s.already);
    if !open.is_empty() {
        eprintln!("{} entries need a decision (use --merge-similar or --keep-similar, or the window version):", open.len());
        for o in &open { eprintln!("{o}"); }
    }
    let lines = &s.project.lines;
    if let Some(p) = &csv { std::fs::write(p, to_csv(lines)).unwrap_or_else(|e| fail(&format!("could not write {}: {e}", p.display()))); eprintln!("Wrote {}", p.display()); }
    if let Some(p) = &json { std::fs::write(p, to_json(lines)).unwrap_or_else(|e| fail(&format!("could not write {}: {e}", p.display()))); eprintln!("Wrote {}", p.display()); }
    if let Some(p) = &save { s.project.save(p).unwrap_or_else(|e| fail(&e)); eprintln!("Wrote {}", p.display()); }
    if csv.is_none() && json.is_none() { print!("{}", to_text(&s.project.lines)); }
}

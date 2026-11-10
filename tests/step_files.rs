use autobom::bom::{plan, Prefs};
use autobom::project::{to_csv, from_csv};
use autobom::session::Session;
use autobom::step::read_step_file;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn fixture(n: &str) -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(n) }

fn counts(parts: &[autobom::step::PartInstance]) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for p in parts { *m.entry(p.name.clone()).or_default() += 1; }
    m
}

#[test]
fn a_real_cad_export() {
    // written by OpenCASCADE: bodies are separate products called SOLID, and a sub-assembly is placed twice
    let (model, ex) = read_step_file(&fixture("gantry.step")).unwrap();
    assert_eq!(model.roots.len(), 1);
    let c = counts(&ex.parts);
    assert_eq!(ex.parts.len(), 20);
    assert_eq!(c["Base plate"], 1);
    assert_eq!(c["Idler Bracket v3"], 2, "a SOLID body takes its component's name, once per placement");
    assert_eq!(c["Motor Mount"], 2, "two bodies in one component");
    assert_eq!(c["Carriage"], 2, "a SOLID directly inside a sub-assembly takes the sub-assembly's name");
    assert_eq!(c["Hex nut"], 4, "a sub-assembly placed twice holding two nuts gives four");
    assert_eq!((c["M3x4"], c["M3x4(Mirror)"], c["M3x4 <2>"]), (4, 2, 1));
    assert!(!c.contains_key("SOLID"), "nothing is left called SOLID");
    let path = &ex.parts.iter().find(|p| p.name == "Idler Bracket v3").unwrap().path;
    assert_eq!(path.first().map(|s| s.as_str()), Some("Gantry v1"));
}

#[test]
fn the_bom_for_a_real_export() {
    let (_, ex) = read_step_file(&fixture("gantry.step")).unwrap();
    let g = plan(&ex.parts, &Prefs::default());
    let names: Vec<_> = g.iter().map(|x| (x.name.as_str(), x.qty(), x.pending_question())).collect();
    assert_eq!(names, [("Base plate", 1, false), ("Carriage", 2, false), ("Hex nut", 4, false), ("Idler Bracket v3", 2, false), ("M3x4", 7, false), ("Motor Mount", 2, false), ("Post", 0, true)]);
}

#[test]
fn fusion_style_file() {
    let (model, ex) = read_step_file(&fixture("fusion_like.step")).unwrap();
    assert!(model.has_shape_info);
    let c = counts(&ex.parts);
    assert_eq!(c["Bracket:1"], 4, "a component with two bodies placed twice: two bodies each time");
    assert_eq!(c["Screw:1"], 1);
    assert_eq!(c["Screw:2"], 1);
    assert_eq!(c["Screw(Mirror):1"], 1);
    assert_eq!(c["Gro\u{df}e Platte"], 1, "escaped characters are read, and geometry in a linked representation is found");
    assert_eq!(ex.empty_components, 1);
    assert_eq!(ex.parts.len(), 8);
    let g = plan(&ex.parts, &Prefs::default());
    let screws = g.iter().find(|x| x.name == "Screw").unwrap();
    assert_eq!(screws.qty(), 3, ":1, :2 and the mirror tally together");
    assert_eq!(g.iter().find(|x| x.name == "Bracket").unwrap().qty(), 4);
}

#[test]
fn the_whole_thing_end_to_end() {
    let (_, ex) = read_step_file(&fixture("gantry.step")).unwrap();
    let mut s = Session::start(Default::default(), &ex.parts);
    s.answer_yes_all_for_test();
    assert!(s.accept_remaining() >= 6);
    let csv = to_csv(&s.project.lines);
    let back = from_csv(&csv).unwrap();
    let find = |n: &str| back.iter().find(|l| l.part == n).map(|l| l.qty.clone());
    assert_eq!(find("M3x4").as_deref(), Some("7"));
    assert_eq!(find("Post").as_deref(), Some("2"));
    assert_eq!(find("Hex nut").as_deref(), Some("4"));
    let order: Vec<_> = back.iter().map(|l| l.part.as_str()).collect();
    let mut sorted = order.clone(); sorted.sort_by(|a, b| autobom::money::natural_cmp(a, b));
    assert_eq!(order, sorted, "alphabetical");
    // a rerun on the same file has nothing to do
    let again = Session::start(s.project.clone(), &ex.parts);
    assert!(again.finished && again.already == 7);
}

#[test]
fn garbage_is_refused_politely() {
    let dir = std::env::temp_dir().join(format!("autobom-garbage-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("x.step");
    std::fs::write(&f, "hello").unwrap();
    assert!(read_step_file(&f).unwrap_err().contains("STEP"));
    std::fs::write(&f, "ISO-10303-21;\nDATA;\n#1 = CARTESIAN_POINT('',(0,0,0));\nENDSEC;").unwrap();
    assert!(read_step_file(&f).unwrap_err().contains("No products"));
    assert!(read_step_file(&dir.join("missing.step")).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

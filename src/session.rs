//! One run of the review: the queue of proposed entries and what you do with them. The window and the
//! command-line tool both drive this, so they behave the same.

use crate::bom::*;
use crate::project::Project;
use crate::step::PartInstance;
use std::collections::HashSet;

pub struct Session {
    pub project: Project,
    pub queue: Vec<Group>,
    pub i: usize,
    /// Entries that already match the STEP file and were left alone.
    pub already: usize,
    pub parts_total: usize,
    pub note: Option<String>,
    pub finished: bool,
}

impl Session {
    pub fn start(mut project: Project, parts: &[PartInstance]) -> Session {
        let groups = plan(parts, &project.prefs);
        let mut taken: HashSet<usize> = HashSet::new();
        let mut queue = Vec::new();
        let mut already = 0;
        for mut g in groups {
            let line = find_line(&g, &project.lines, &taken);
            if let Some(i) = line { taken.insert(i); }
            g.line = line;
            if let Some(i) = line {
                if is_up_to_date(&g, &project.lines[i]) { already += 1; continue; }
                // a line you typed or imported has no links to parts yet; if it already says the right thing, adopt it
                let l = &project.lines[i];
                if l.covers.is_empty() && !g.pending_question() && l.qty_num() == Some(g.qty() as u64) {
                    let names = g.selected_names();
                    let l = &mut project.lines[i];
                    l.covers = names;
                    l.auto = true;
                    already += 1;
                    continue;
                }
            }
            queue.push(g);
        }
        let present: HashSet<String> = parts.iter().map(|p| tidy(&p.name).to_lowercase()).collect();
        for i in orphans(&project.lines, &present) {
            let mut g = Group::new(String::new(), project.lines[i].part.clone(), Vec::new(), Vec::new());
            g.orphan = Some(i);
            queue.push(g);
        }
        let finished = queue.is_empty();
        Session { project, queue, i: 0, already, parts_total: parts.len(), note: None, finished }
    }

    pub fn current(&mut self) -> Option<&mut Group> {
        if self.finished { return None; }
        let i = self.i;
        let lines = self.project.lines.clone();
        let g = self.queue.get_mut(i)?;
        if g.orphan.is_none() { prefill(g, &lines); }
        Some(g)
    }

    /// Answers every open question "yes" (a convenience for tests and the command line).
    #[doc(hidden)]
    pub fn answer_yes_all_for_test(&mut self) {
        for k in 0..self.queue.len() { self.i = k; if self.queue[k].pending_question() { self.answer_yes(); } }
        self.i = 0;
    }

    pub fn remaining(&self) -> usize { self.queue.len().saturating_sub(self.i) }

    /// Moves on, putting any deselected components back in the queue first.
    fn advance(&mut self) {
        let i = self.i;
        let extra = match self.queue.get_mut(i) { Some(g) if g.orphan.is_none() => requeue_deselected(g), _ => Vec::new() };
        if !extra.is_empty() {
            self.note = Some(format!("{} deselected component{} put back in the queue.", extra.len(), if extra.len() == 1 { " was" } else { "s were" }));
            for (k, g) in extra.into_iter().enumerate() { self.queue.insert(i + 1 + k, g); }
        } else { self.note = None; }
        if self.i + 1 >= self.queue.len() { self.finished = true; } else { self.i += 1; }
    }

    pub fn confirm(&mut self) -> Result<(), String> {
        let i = self.i;
        let lines = self.project.lines.clone();
        let g = self.queue.get_mut(i).ok_or("Nothing to confirm.")?;
        if g.orphan.is_none() { prefill(g, &lines); }
        if g.orphan.is_some() { return Err("Choose what to do with this line.".into()); }
        if g.pending_question() { return Err("Answer the question about the look-alike parts first.".into()); }
        if g.form.part.trim().is_empty() { return Err("Give the entry a name, or skip it.".into()); }
        apply(g, &mut self.project.lines);
        self.advance();
        Ok(())
    }

    pub fn skip(&mut self) {
        if let Some(g) = self.queue.get_mut(self.i) { g.action = Action::Skipped; }
        self.advance();
    }

    /// Never ask about this entry again.
    pub fn ignore(&mut self) {
        let key = self.queue.get(self.i).map(|g| g.key.clone()).unwrap_or_default();
        if !key.is_empty() && !self.project.prefs.ignored.contains(&key) { self.project.prefs.ignored.push(key); }
        if let Some(g) = self.queue.get_mut(self.i) { g.action = Action::Ignored; }
        self.advance();
    }

    pub fn back(&mut self) { if self.finished { self.finished = false; } else if self.i > 0 { self.i -= 1; } self.note = None; }

    pub fn toggle_member(&mut self, m: usize, on: bool) {
        if let Some(g) = self.queue.get_mut(self.i) {
            if m < g.sel.len() { g.sel[m] = on; if g.qty_auto { g.form.qty = g.qty().to_string(); } }
        }
    }

    /// "Yes, they are the same part."
    pub fn answer_yes(&mut self) {
        if let Some(g) = self.queue.get_mut(self.i) {
            for m in g.ask.clone() { g.sel[m] = true; let n = g.members[m].name.to_lowercase(); if !self.project.prefs.merged.contains(&n) { self.project.prefs.merged.push(n); } }
            g.ask.clear();
            g.answer = Some(true);
            if g.qty_auto { g.form.qty = g.qty().to_string(); }
        }
    }

    /// "No, keep them separate": the look-alikes become entries of their own, right after this one.
    pub fn answer_no(&mut self) {
        let i = self.i;
        let (extra, empty) = match self.queue.get_mut(i) {
            Some(g) => {
                for m in g.ask.clone() { let n = g.members[m].name.to_lowercase(); if !self.project.prefs.separate.contains(&n) { self.project.prefs.separate.push(n); } }
                let extra = split_unsure(g);
                (extra, g.members.is_empty())
            }
            None => return,
        };
        if empty { self.queue.splice(i..=i, extra); } else { for (k, g) in extra.into_iter().enumerate() { self.queue.insert(i + 1 + k, g); } if let Some(g) = self.queue.get_mut(i) { if g.qty_auto { g.form.qty = g.qty().to_string(); } } }
    }

    /// For an entry whose parts are gone from the model: keep the line (and stop tracking it), or remove it.
    pub fn orphan_keep(&mut self) {
        if let Some(idx) = self.queue.get(self.i).and_then(|g| g.orphan) {
            if let Some(l) = self.project.lines.get_mut(idx) { l.covers.clear(); l.auto = false; }
            if let Some(g) = self.queue.get_mut(self.i) { g.action = Action::Skipped; }
            self.advance();
        }
    }

    pub fn orphan_remove(&mut self) {
        if let Some(idx) = self.queue.get(self.i).and_then(|g| g.orphan) {
            self.remove_line(idx);
            if let Some(g) = self.queue.get_mut(self.i) { g.action = Action::Skipped; }
            self.advance();
        }
    }

    pub fn remove_line(&mut self, idx: usize) {
        if idx >= self.project.lines.len() { return; }
        self.project.lines.remove(idx);
        for g in self.queue.iter_mut() {
            if let Some(l) = g.line { if l == idx { g.line = None; } else if l > idx { g.line = Some(l - 1); } }
            if let Some(o) = g.orphan { if o > idx { g.orphan = Some(o - 1); } }
        }
    }

    /// Confirms every remaining entry that has no open question, exactly as proposed. Questions and lines whose
    /// parts are gone are left for a person.
    pub fn accept_remaining(&mut self) -> usize {
        let mut n = 0;
        let mut k = self.i;
        while k < self.queue.len() {
            let ok = { let g = &self.queue[k]; g.orphan.is_none() && !g.pending_question() && g.action == Action::Pending };
            if ok {
                self.i = k;
                let lines = self.project.lines.clone();
                prefill(&mut self.queue[k], &lines);
                if self.confirm().is_ok() { n += 1; }
            }
            k += 1;
        }
        match self.queue.iter().position(|g| g.action == Action::Pending) {
            Some(p) => { self.i = p; self.finished = false; }
            None => { self.finished = true; self.i = self.queue.len().saturating_sub(1); }
        }
        n
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        let c = |a: Action| self.queue.iter().filter(|g| g.action == a).count();
        (c(Action::Added), c(Action::Updated), c(Action::Skipped) + c(Action::Ignored))
    }

    pub fn summary(&self) -> String {
        let (a, u, s) = self.counts();
        format!("{a} added, {u} updated, {s} skipped, {} already up to date", self.already)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::step::PartInstance;

    fn part(name: &str) -> PartInstance { PartInstance { raw: name.into(), name: name.into(), path: vec!["Root".into(), name.into()], pd: 0 } }
    fn parts(v: &[(&str, usize)]) -> Vec<PartInstance> { v.iter().flat_map(|(n, c)| std::iter::repeat(part(n)).take(*c)).collect() }

    #[test]
    fn the_whole_run() {
        let mut s = Session::start(Project::default(), &parts(&[("Base plate", 1), ("M3x4", 4), ("M3x4(Mirror)", 2), ("Post L", 1), ("Post R", 1)]));
        assert_eq!(s.queue.len(), 3);
        // Base plate
        s.current().unwrap().form.cost = ".5".into();
        s.confirm().unwrap();
        // M3x4: take the mirror out, it goes back in the queue
        { let g = s.current().unwrap(); assert_eq!(g.name, "M3x4"); }
        let mi = s.queue[s.i].members.iter().position(|m| m.name.contains("Mirror")).unwrap();
        s.toggle_member(mi, false);
        assert_eq!(s.current().unwrap().form.qty, "4");
        s.confirm().unwrap();
        assert!(s.note.as_deref().unwrap().contains("put back in the queue"));
        assert_eq!(s.queue.len(), 4);
        assert_eq!(s.current().unwrap().name, "M3x4(Mirror)");
        s.confirm().unwrap();
        // Post: must answer first
        assert!(s.confirm().is_err());
        s.answer_yes();
        assert_eq!(s.current().unwrap().form.qty, "2");
        s.confirm().unwrap();
        assert!(s.finished);
        let names: Vec<_> = s.project.lines.iter().map(|l| (l.part.clone(), l.qty.clone())).collect();
        assert!(names.contains(&("Base plate".into(), "1".into())) && names.contains(&("M3x4".into(), "4".into())) && names.contains(&("M3x4(Mirror)".into(), "2".into())) && names.contains(&("Post".into(), "2".into())), "{names:?}");
        assert_eq!(s.project.lines.iter().find(|l| l.part == "Base plate").unwrap().cost, "$0.50");
        assert!(s.project.prefs.merged.contains(&"post l".to_string()));
    }

    #[test]
    fn saying_no_splits_them() {
        let mut s = Session::start(Project::default(), &parts(&[("Post L", 1), ("Post R", 1)]));
        assert_eq!(s.queue.len(), 1);
        s.answer_no();
        assert_eq!(s.queue.len(), 2);
        assert!(s.project.prefs.separate.contains(&"post l".to_string()));
        s.confirm().unwrap();
        s.confirm().unwrap();
        assert!(s.finished);
        assert_eq!(s.project.lines.len(), 2);
        // and the answer is remembered on the next run
        let s2 = Session::start(Project { prefs: s.project.prefs.clone(), lines: s.project.lines.clone(), ..Default::default() }, &parts(&[("Post L", 1), ("Post R", 1)]));
        assert!(s2.finished && s2.already == 2);
    }

    #[test]
    fn a_rerun_only_shows_what_changed() {
        let mut s = Session::start(Project::default(), &parts(&[("A", 2), ("B", 1), ("C", 3)]));
        s.current().unwrap().form.urls = vec!["https://a.example".into()];
        assert_eq!(s.accept_remaining(), 3);
        let prev = s.project.clone();
        // A gains one, B stays, C is gone, D is new
        let mut s2 = Session::start(prev, &parts(&[("A", 3), ("B", 1), ("D", 1)]));
        assert_eq!(s2.already, 1);
        let names: Vec<String> = s2.queue.iter().map(|g| g.name.clone()).collect();
        assert_eq!(names, ["A", "D", "C"]);
        assert_eq!(s2.queue[2].orphan.is_some(), true);
        { let g = s2.current().unwrap(); assert_eq!(g.form.qty, "3"); assert_eq!(g.prev_qty, "2"); assert_eq!(g.form.urls, vec!["https://a.example".to_string()]); }
        s2.confirm().unwrap();
        s2.confirm().unwrap(); // D
        assert!(s2.confirm().is_err()); // the orphan needs a choice
        s2.orphan_remove();
        assert!(s2.finished);
        assert!(s2.project.lines.iter().all(|l| l.part != "C"));
        assert_eq!(s2.project.lines.iter().find(|l| l.part == "A").unwrap().qty, "3");
    }

    #[test]
    fn imported_lines_are_adopted() {
        let lines = vec![Line { part: "M3x4".into(), qty: "4".into(), cost: "$0.10".into(), ..Default::default() }];
        let s = Session::start(Project { lines, ..Default::default() }, &parts(&[("M3x4", 4)]));
        assert!(s.finished);
        assert_eq!(s.project.lines[0].covers, vec!["m3x4".to_string()]);
        assert_eq!(s.project.lines[0].cost, "$0.10");
    }
}

//! The window version of Auto BOM: open a STEP file, review the proposed entries, export the bill of materials.

#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use autobom::bom::{category_label, sort_lines, Action, Line, Prefs, CATEGORIES};
use autobom::money::{cost_symbol, fmt_money, natural_cmp, norm_cost};
use autobom::project::{from_csv, to_csv, to_json, Project};
use autobom::session::Session;
use autobom::step::{read_step_file, PartInstance};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};


// Adobe-style dark theme: charcoal panels, flat controls, one blue accent.
const BG0: egui::Color32 = egui::Color32::from_rgb(0x1e, 0x1e, 0x1e); // canvas / inputs
const BG1: egui::Color32 = egui::Color32::from_rgb(0x28, 0x28, 0x28); // panels
const BG2: egui::Color32 = egui::Color32::from_rgb(0x32, 0x32, 0x32); // headers, buttons
const BG3: egui::Color32 = egui::Color32::from_rgb(0x44, 0x44, 0x44); // hover
const LINE: egui::Color32 = egui::Color32::from_rgb(0x12, 0x12, 0x12);
const TEXT: egui::Color32 = egui::Color32::from_rgb(0xc8, 0xc8, 0xc8);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x2d, 0x8c, 0xeb);

fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = BG1;
    v.window_fill = BG1;
    v.extreme_bg_color = BG0;
    v.faint_bg_color = egui::Color32::from_rgb(0x2d, 0x2d, 0x2d);
    v.window_stroke = egui::Stroke::new(1.0, LINE);
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    v.hyperlink_color = ACCENT;
    let r = egui::CornerRadius::same(2);
    v.window_corner_radius = r;
    v.menu_corner_radius = r;
    for (w, fill) in [(&mut v.widgets.noninteractive, BG1), (&mut v.widgets.inactive, BG2), (&mut v.widgets.hovered, BG3), (&mut v.widgets.active, ACCENT), (&mut v.widgets.open, BG3)] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.corner_radius = r;
        w.bg_stroke = egui::Stroke::new(1.0, LINE);
    }
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    ctx.set_visuals(v);
    ctx.style_mut(|st| {
        st.spacing.item_spacing = egui::vec2(6.0, 4.0);
        st.spacing.button_padding = egui::vec2(8.0, 2.0);
        st.spacing.interact_size.y = 20.0;
        use egui::{FontFamily::Proportional, FontId, TextStyle::*};
        st.text_styles.insert(Body, FontId::new(12.0, Proportional));
        st.text_styles.insert(Button, FontId::new(12.0, Proportional));
        st.text_styles.insert(Small, FontId::new(10.5, Proportional));
        st.text_styles.insert(Heading, FontId::new(14.0, Proportional));
    });
}

/// A panel title strip, like the tab header on top of a Photoshop panel.
fn panel_header(ui: &mut egui::Ui, title: &str) {
    egui::Frame::new().fill(BG2).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(title.to_uppercase()).size(10.5).strong().color(egui::Color32::from_rgb(0xe6, 0xe6, 0xe6)));
    });
}

/// A workspace tab: flat, with an accent underline when active.
fn workspace_tab(ui: &mut egui::Ui, cur: &mut Tab, me: Tab, label: &str) {
    let on = *cur == me;
    let r = ui.add(egui::Button::new(egui::RichText::new(label).color(if on { egui::Color32::WHITE } else { TEXT })).fill(if on { BG1 } else { egui::Color32::TRANSPARENT }).stroke(egui::Stroke::NONE).corner_radius(0));
    if on { ui.painter().hline(r.rect.x_range(), r.rect.bottom() + 1.0, egui::Stroke::new(2.0, ACCENT)); }
    if r.clicked() { *cur = me; }
}

/// The primary action button (blue, like Photoshop's OK).
fn primary(ui: &mut egui::Ui, enabled: bool, label: &str) -> egui::Response {
    ui.add_enabled(enabled, egui::Button::new(egui::RichText::new(label).color(egui::Color32::WHITE).strong()).fill(ACCENT))
}

#[derive(PartialEq, Clone, Copy)]
enum Tab { Review, Bom, Parts }

struct Loaded { path: PathBuf, parts: Vec<PartInstance>, summary: String }

struct App {
    project: Project,
    parts: Vec<PartInstance>,
    session: Option<Session>,
    tab: Tab,
    list: String,
    status: String,
    error: Option<String>,
    loading: Option<Receiver<Result<Loaded, String>>>,
    project_path: Option<PathBuf>,
    startup_file: Option<PathBuf>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> App {
        apply_theme(&cc.egui_ctx);
        App { project: Project::default(), parts: Vec::new(), session: None, tab: Tab::Review, list: "all".into(),
            status: "Open a STEP file to begin.".into(), error: None, loading: None, project_path: None,
            startup_file: std::env::args_os().nth(1).map(PathBuf::from) }
    }

    fn open_step(&mut self, ctx: &egui::Context, path: PathBuf) {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        self.loading = Some(rx);
        self.error = None;
        std::thread::spawn(move || {
            let r = read_step_file(&path).map(|(model, ex)| {
                let summary = format!("Read {} products, {} placed parts{}.", model.products.len(), ex.parts.len(),
                    if ex.empty_components > 0 { format!(" ({} empty components left out)", ex.empty_components) } else { String::new() });
                Loaded { path: path.clone(), parts: ex.parts, summary }
            });
            let _ = tx.send(r);
            ctx.request_repaint();
        });
    }

    fn poll_loading(&mut self) {
        let Some(rx) = &self.loading else { return };
        if let Ok(r) = rx.try_recv() {
            self.loading = None;
            match r {
                Ok(l) => {
                    self.project.step_path = l.path.display().to_string();
                    self.parts = l.parts;
                    let s = Session::start(self.project.clone(), &self.parts);
                    self.status = format!("{} {}", l.summary, s.summary());
                    self.session = Some(s);
                    self.tab = Tab::Review;
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    /// Pulls the lines and answers out of the running review back into the project.
    fn sync(&mut self) { if let Some(s) = &self.session { self.project = s.project.clone(); } }

    /// Puts the project into a fresh review (after opening a project file or importing a CSV).
    fn restart(&mut self) {
        self.session = if self.parts.is_empty() { None } else { Some(Session::start(self.project.clone(), &self.parts)) };
    }

    fn lines_mut(&mut self) -> &mut Vec<Line> {
        match &mut self.session { Some(s) => &mut s.project.lines, None => &mut self.project.lines }
    }
    fn lines(&self) -> &Vec<Line> {
        match &self.session { Some(s) => &s.project.lines, None => &self.project.lines }
    }

    fn menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open STEP file…").clicked() {
                    ui.close_menu();
                    if let Some(p) = rfd::FileDialog::new().add_filter("STEP files", &["step", "stp", "STEP", "STP"]).pick_file() { self.open_step(ctx, p); }
                }
                if ui.button("Open project…").clicked() {
                    ui.close_menu();
                    if let Some(p) = rfd::FileDialog::new().add_filter("Auto BOM project", &["json"]).pick_file() {
                        match Project::load(&p) {
                            Ok(pr) => { self.project = pr; self.project_path = Some(p); self.restart(); self.status = "Project opened.".into(); }
                            Err(e) => self.error = Some(e),
                        }
                    }
                }
                if ui.button("Save project").clicked() {
                    ui.close_menu();
                    self.save_project(false);
                }
                if ui.button("Save project as…").clicked() {
                    ui.close_menu();
                    self.save_project(true);
                }
                ui.separator();
                if ui.button("Import BOM from CSV…").clicked() {
                    ui.close_menu();
                    if let Some(p) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).pick_file() {
                        match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| from_csv(&t)) {
                            Ok(lines) => {
                                self.sync();
                                self.project = Project { step_path: self.project.step_path.clone(), lines, prefs: Prefs::default() };
                                self.restart();
                                self.status = "BOM imported.".into();
                            }
                            Err(e) => self.error = Some(e),
                        }
                    }
                }
                if ui.button("Export CSV…").clicked() {
                    ui.close_menu();
                    self.sync();
                    if let Some(p) = rfd::FileDialog::new().set_file_name("bom.csv").add_filter("CSV", &["csv"]).save_file() {
                        self.write(&p, to_csv(&self.project.lines));
                    }
                }
                if ui.button("Export JSON…").clicked() {
                    ui.close_menu();
                    self.sync();
                    if let Some(p) = rfd::FileDialog::new().set_file_name("bom.json").add_filter("JSON", &["json"]).save_file() {
                        self.write(&p, to_json(&self.project.lines));
                    }
                }
                ui.separator();
                if ui.button("Quit").clicked() { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
            });
            ui.separator();
            let mut t = self.tab;
            workspace_tab(ui, &mut t, Tab::Review, "Review");
            workspace_tab(ui, &mut t, Tab::Bom, "Bill of Materials");
            workspace_tab(ui, &mut t, Tab::Parts, "Parts");
            self.tab = t;
        });
    }

    fn write(&mut self, p: &Path, text: String) {
        match std::fs::write(p, text) {
            Ok(()) => self.status = format!("Saved {}", p.display()),
            Err(e) => self.error = Some(format!("Could not save {}: {e}", p.display())),
        }
    }

    fn save_project(&mut self, ask: bool) {
        self.sync();
        let path = if ask || self.project_path.is_none() {
            rfd::FileDialog::new().set_file_name("bom.autobom.json").add_filter("Auto BOM project", &["json"]).save_file()
        } else { self.project_path.clone() };
        if let Some(p) = path {
            match self.project.save(&p) {
                Ok(()) => { self.status = format!("Saved {}", p.display()); self.project_path = Some(p); }
                Err(e) => self.error = Some(e),
            }
        }
    }

    // ---------------------------------------------------------------- review
    fn review(&mut self, ui: &mut egui::Ui) {
        let Some(s) = self.session.as_mut() else {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.heading("Auto BOM");
                ui.label("Open a STEP file from the File menu. The parts are read from the assembly and proposed as bill of materials entries,\nand you confirm, edit or skip each one.");
            });
            return;
        };
        if s.finished || s.queue.is_empty() {
            ui.heading("Review finished");
            ui.label(s.summary());
            if s.queue.is_empty() { ui.label("Everything in the model is already in the bill of materials."); }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if !s.queue.is_empty() && ui.button("◀ Back").clicked() { s.back(); }
                ui.label("Use the Bill of materials tab to look over and export the result.");
            });
            return;
        }
        let total = s.queue.len();
        let idx = s.i;
        ui.horizontal(|ui| {
            ui.heading(format!("Entry {} of {}", idx + 1, total));
            ui.add(egui::ProgressBar::new((idx as f32) / (total as f32)).desired_width(200.0));
            if ui.button("Accept all sure entries").on_hover_text("Confirms everything left that has no open question, as proposed").clicked() {
                let n = s.accept_remaining();
                s.note = Some(format!("{n} entries accepted."));
            }
        });
        if let Some(n) = &s.note { ui.colored_label(egui::Color32::LIGHT_BLUE, n); }
        ui.separator();

        let mut do_confirm = false;
        let mut do_skip = false;
        let mut do_ignore = false;
        let mut do_back = false;
        let mut yes = false;
        let mut no = false;
        let mut keep = false;
        let mut remove = false;
        let mut toggles: Vec<(usize, bool)> = Vec::new();
        let mut err: Option<String> = None;

        if s.current().is_none() { return; }
        {
            let g = &mut s.queue[idx];
            egui::ScrollArea::vertical().show(ui, |ui| {
                if g.orphan.is_some() {
                    ui.colored_label(egui::Color32::YELLOW, format!("“{}” is in the bill of materials, but its parts are no longer in the model.", g.name));
                    ui.label("Keep the line (it stops being tracked against the model) or remove it.");
                    ui.horizontal(|ui| {
                        if ui.button("Keep line").clicked() { keep = true; }
                        if ui.button("Remove line").clicked() { remove = true; }
                    });
                    return;
                }
                if g.line.is_some() && !g.prev_qty.is_empty() {
                    ui.colored_label(egui::Color32::YELLOW, format!("The bill of materials says {}, the model has {}.", g.prev_qty, g.qty()));
                }
                if g.pending_question() {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.strong("Are these the same part?");
                        let names: Vec<String> = g.ask.iter().map(|&m| format!("“{}” ×{}", g.members[m].name, g.members[m].count)).collect();
                        ui.label(format!("These look like “{}” but are named differently: {}.", g.name, names.join(", ")));
                        ui.label("Yes counts them together; No gives each its own entry. Your answer is remembered.");
                        ui.horizontal(|ui| {
                            if ui.button("Yes, same part").clicked() { yes = true; }
                            if ui.button("No, separate").clicked() { no = true; }
                        });
                    });
                }
                ui.add_space(4.0);
                egui::Grid::new("form").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label("Part");
                    ui.add(egui::TextEdit::singleline(&mut g.form.part).desired_width(360.0));
                    ui.end_row();
                    ui.label("Quantity");
                    if ui.add(egui::TextEdit::singleline(&mut g.form.qty).desired_width(80.0)).changed() { g.qty_auto = g.form.qty.trim() == g.qty().to_string(); }
                    ui.end_row();
                    ui.label("List");
                    egui::ComboBox::from_id_salt("cat").selected_text(category_label(&g.form.cat)).show_ui(ui, |ui| {
                        for (k, l) in CATEGORIES { ui.selectable_value(&mut g.form.cat, k.to_string(), l); }
                    });
                    ui.end_row();
                    ui.label("Unit cost");
                    let r = ui.add(egui::TextEdit::singleline(&mut g.form.cost).desired_width(120.0).hint_text("$0.00"));
                    if r.lost_focus() { g.form.cost = norm_cost(&g.form.cost); }
                    ui.end_row();
                    ui.label("Links");
                    ui.vertical(|ui| {
                        let mut drop = None;
                        for (k, u) in g.form.urls.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(u).desired_width(360.0).hint_text("https://…"));
                                if ui.small_button("✕").clicked() { drop = Some(k); }
                            });
                        }
                        if let Some(k) = drop { g.form.urls.remove(k); }
                        if g.form.urls.len() < 5 && ui.small_button("+ Add link").clicked() { g.form.urls.push(String::new()); }
                    });
                    ui.end_row();
                    ui.label("Description");
                    ui.add(egui::TextEdit::multiline(&mut g.form.desc).desired_width(360.0).desired_rows(2));
                    ui.end_row();
                    ui.label("Note");
                    ui.add(egui::TextEdit::multiline(&mut g.form.note).desired_width(360.0).desired_rows(2));
                    ui.end_row();
                });
                ui.add_space(6.0);
                ui.strong("Components counted in this entry");
                ui.label("Untick any that are a different part; they come back as their own entries.");
                for (m, mem) in g.members.iter().enumerate() {
                    if g.ask.contains(&m) && g.answer.is_none() { continue; }
                    let mut on = g.sel[m];
                    let tag = mem.tag();
                    let label = if tag.is_empty() { format!("{} ×{}", mem.name, mem.count) } else { format!("{} ×{}  ({tag})", mem.name, mem.count) };
                    let r = ui.checkbox(&mut on, label);
                    if !mem.paths.is_empty() { r.on_hover_text(mem.paths.join("\n")); }
                    if on != g.sel[m] { toggles.push((m, on)); }
                }
            });
        }
        for (m, on) in toggles { s.toggle_member(m, on); }
        if yes { s.answer_yes(); }
        if no { s.answer_no(); }
        if keep { s.orphan_keep(); }
        if remove { s.orphan_remove(); }

        let is_orphan = s.queue.get(s.i).map(|g| g.orphan.is_some()).unwrap_or(false);
        let blocked = s.queue.get(s.i).map(|g| g.pending_question()).unwrap_or(false);
        if !is_orphan {
            ui.separator();
            ui.horizontal(|ui| {
                if ui.add_enabled(s.i > 0, egui::Button::new("◀ Back")).clicked() { do_back = true; }
                if ui.button("Skip").on_hover_text("Leave this one out for now").clicked() { do_skip = true; }
                if ui.button("Ignore forever").on_hover_text("Never ask about this part again").clicked() { do_ignore = true; }
                if primary(ui, !blocked, "Confirm ▶").on_disabled_hover_text("Answer the question first").clicked() { do_confirm = true; }
            });
        }
        if do_back { s.back(); }
        if do_skip { s.skip(); }
        if do_ignore { s.ignore(); }
        if do_confirm { if let Err(e) = s.confirm() { err = Some(e); } }
        if let Some(e) = err { self.error = Some(e); }
    }

    // ---------------------------------------------------------------- bill of materials
    fn bom(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.list, "all".into(), "All");
            for (k, l) in CATEGORIES { ui.selectable_value(&mut self.list, k.to_string(), l); }
            ui.separator();
            if ui.button("+ Add line").clicked() {
                let cat = if self.list == "all" { "purchased".to_string() } else { self.list.clone() };
                self.lines_mut().push(Line { part: "New part".into(), qty: "1".into(), cat, ..Default::default() });
            }
            if ui.button("Sort A–Z").clicked() { sort_lines(self.lines_mut()); }
        });
        ui.separator();
        let list = self.list.clone();
        let mut remove: Option<usize> = None;
        let mut order: Vec<usize> = (0..self.lines().len()).filter(|&i| list == "all" || self.lines()[i].cat == list).collect();
        {
            let lines = self.lines();
            order.sort_by(|&a, &b| natural_cmp(&lines[a].part, &lines[b].part));
        }
        let lines = self.lines_mut_ref();
        egui::ScrollArea::both().show(ui, |ui| {
            egui::Grid::new("bom").striped(true).num_columns(8).spacing([10.0, 6.0]).show(ui, |ui| {
                for h in ["Part", "Qty", "List", "Unit cost", "Line total", "Link", "Note", ""] { ui.strong(h); }
                ui.end_row();
                for &i in &order {
                    let l = &mut lines[i];
                    ui.add(egui::TextEdit::singleline(&mut l.part).desired_width(200.0));
                    ui.add(egui::TextEdit::singleline(&mut l.qty).desired_width(44.0));
                    egui::ComboBox::from_id_salt(("cat", i)).selected_text(category_label(&l.cat)).width(90.0).show_ui(ui, |ui| {
                        for (k, lab) in CATEGORIES { ui.selectable_value(&mut l.cat, k.to_string(), lab); }
                    });
                    let r = ui.add(egui::TextEdit::singleline(&mut l.cost).desired_width(80.0));
                    if r.lost_focus() { l.cost = norm_cost(&l.cost); }
                    ui.label(l.line_total().map(|t| fmt_money(t, &cost_symbol(&l.cost))).unwrap_or_default());
                    if l.urls.is_empty() { l.urls.push(String::new()); }
                    ui.add(egui::TextEdit::singleline(&mut l.urls[0]).desired_width(160.0));
                    ui.add(egui::TextEdit::singleline(&mut l.note).desired_width(160.0));
                    if ui.small_button("🗑").on_hover_text("Remove this line").clicked() { remove = Some(i); }
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            let mut sum = 0.0;
            let mut sym = String::new();
            for &i in &order { if let Some(t) = lines[i].line_total() { sum += t; if sym.is_empty() { sym = cost_symbol(&lines[i].cost); } } }
            ui.strong(format!("{} line{}, total {}", order.len(), if order.len() == 1 { "" } else { "s" }, fmt_money(sum, &sym)));
        });
        if let Some(i) = remove {
            match &mut self.session { Some(s) => s.remove_line(i), None => { self.project.lines.remove(i); } }
        }
        // drop the empty link placeholders added for editing
        for l in self.lines_mut().iter_mut() { l.urls.retain(|u| !u.trim().is_empty()); }
    }

    fn lines_mut_ref(&mut self) -> &mut Vec<Line> { self.lines_mut() }

    /// The left panel: every proposed entry in a list, like the Layers panel. Click one to jump to it.
    fn queue_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("queue").default_width(230.0).frame(egui::Frame::new().fill(BG1).stroke(egui::Stroke::new(1.0, LINE))).show(ctx, |ui| {
            panel_header(ui, "Entries");
            let Some(s) = self.session.as_mut() else { ui.add_space(8.0); ui.label("  Nothing open."); return; };
            let cur = s.i;
            let mut jump = None;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (k, g) in s.queue.iter().enumerate() {
                    let (mark, col) = match g.action {
                        Action::Added | Action::Updated => ("●", egui::Color32::from_rgb(0x4c, 0xc2, 0x6e)),
                        Action::Skipped | Action::Ignored => ("○", egui::Color32::GRAY),
                        Action::Pending if g.pending_question() || g.orphan.is_some() => ("●", egui::Color32::from_rgb(0xf0, 0xb4, 0x29)),
                        Action::Pending => ("◌", TEXT),
                    };
                    let on = k == cur && !s.finished;
                    let (rect, r) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::click());
                    if on { ui.painter().rect_filled(rect, 0, ACCENT.gamma_multiply(0.55)); } else if r.hovered() { ui.painter().rect_filled(rect, 0, BG3); } else if k % 2 == 1 { ui.painter().rect_filled(rect, 0, egui::Color32::from_rgb(0x2d, 0x2d, 0x2d)); }
                    ui.painter().text(rect.left_center() + egui::vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, mark, egui::FontId::proportional(11.0), col);
                    ui.painter().text(rect.left_center() + egui::vec2(24.0, 0.0), egui::Align2::LEFT_CENTER, &g.name, egui::FontId::proportional(12.0), if on { egui::Color32::WHITE } else { TEXT });
                    if g.orphan.is_none() { ui.painter().text(rect.right_center() - egui::vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, format!("×{}", g.qty()), egui::FontId::proportional(11.0), egui::Color32::GRAY); }
                    if r.clicked() { jump = Some(k); }
                }
            });
            if let Some(k) = jump { s.i = k; s.finished = false; s.note = None; }
        });
    }

    fn parts_tab(&mut self, ui: &mut egui::Ui) {
        if self.parts.is_empty() { ui.label("No STEP file open."); return; }
        let mut names: std::collections::BTreeMap<String, usize> = Default::default();
        for p in &self.parts { *names.entry(p.name.clone()).or_default() += 1; }
        ui.label(format!("{} placed parts, {} different names.", self.parts.len(), names.len()));
        let mut v: Vec<_> = names.into_iter().collect();
        v.sort_by(|a, b| natural_cmp(&a.0, &b.0));
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("parts").striped(true).num_columns(2).show(ui, |ui| {
                ui.strong("Qty"); ui.strong("Part"); ui.end_row();
                for (n, c) in v { ui.label(c.to_string()); ui.label(n); ui.end_row(); }
            });
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_loading();
        if let Some(p) = self.startup_file.take() { self.open_step(ctx, p); }
        egui::TopBottomPanel::top("menu").frame(egui::Frame::new().fill(BG0).inner_margin(egui::Margin::symmetric(6, 2))).show(ctx, |ui| self.menu(ui, ctx));
        egui::TopBottomPanel::bottom("status").frame(egui::Frame::new().fill(BG0).inner_margin(egui::Margin::symmetric(8, 3))).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.loading.is_some() { ui.spinner(); ui.label("Reading the STEP file…"); } else { ui.label(&self.status); }
            });
        });
        if self.tab == Tab::Review { self.queue_panel(ctx); }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG1).inner_margin(egui::Margin::same(10))).show(ctx, |ui| match self.tab {
            Tab::Review => self.review(ui),
            Tab::Bom => self.bom(ui),
            Tab::Parts => self.parts_tab(ui),
        });
        if let Some(e) = self.error.clone() {
            let mut open = true;
            egui::Window::new("Something went wrong").open(&mut open).collapsible(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.label(&e);
                if ui.button("OK").clicked() { self.error = None; }
            });
            if !open { self.error = None; }
        }
        // files dropped onto the window open as STEP files
        let dropped = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()));
        if let Some(p) = dropped { self.open_step(ctx, p); }
    }
}

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([980.0, 680.0]).with_title("Auto BOM").with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("Auto BOM", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

#[allow(dead_code)]
fn _unused(_: Action) {}

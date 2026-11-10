//! The window version of Auto BOM: open a STEP file, review the proposed entries, export the bill of materials.

#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use autobom::bom::{category_label, sort_lines, tidy, Action, Line, Prefs, CATEGORIES};
use autobom::money::{cost_symbol, fmt_money, natural_cmp, norm_cost};
use autobom::project::{from_csv, to_csv, to_json, Project};
use autobom::session::Session;
use autobom::geom::{Geo, Wire, V3};
use autobom::step::{read_step_file, PartInstance};
use std::collections::HashMap;
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

struct Loaded { path: PathBuf, parts: Vec<PartInstance>, summary: String, solids: HashMap<u32, Vec<u32>>, geo: Option<Geo> }

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
    geo: Option<Geo>,
    pd_solids: HashMap<u32, Vec<u32>>,
    name_pds: HashMap<String, Vec<u32>>,
    wires: HashMap<String, Option<Wire>>,
    view_name: Option<String>,
    cam: Cam,
}

#[derive(Clone, Copy)]
struct Cam { yaw: f32, pitch: f32, zoom: f32, pan: egui::Vec2 }
impl Default for Cam { fn default() -> Cam { Cam { yaw: -0.6, pitch: 0.5, zoom: 1.0, pan: egui::Vec2::ZERO } } }

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> App {
        apply_theme(&cc.egui_ctx);
        App { project: Project::default(), parts: Vec::new(), session: None, tab: Tab::Review, list: "all".into(),
            status: "Open a STEP file to begin.".into(), error: None, loading: None, project_path: None,
            startup_file: std::env::args_os().nth(1).map(PathBuf::from),
            geo: None, pd_solids: HashMap::new(), name_pds: HashMap::new(), wires: HashMap::new(), view_name: None, cam: Cam::default() }
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
                let solids = model.products.iter().map(|(k, p)| (*k, p.solid_ids.clone())).collect();
                let geo = Geo::load(&path).ok();
                Loaded { path: path.clone(), parts: ex.parts, summary, solids, geo }
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
                    self.geo = l.geo;
                    self.pd_solids = l.solids;
                    self.wires.clear();
                    self.view_name = None;
                    self.name_pds.clear();
                    for p in &self.parts {
                        let v = self.name_pds.entry(tidy(&p.name).to_lowercase()).or_default();
                        if !v.contains(&p.pd) { v.push(p.pd); }
                    }
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


    /// The wireframe for a part name (cached). None when the file has no geometry we can draw.
    fn wire_for(&mut self, key: &str) -> Option<Wire> {
        if !self.wires.contains_key(key) {
            let w = match (&self.geo, self.name_pds.get(key)) {
                (Some(g), Some(pds)) => {
                    let parts: Vec<Wire> = pds.iter().filter_map(|pd| self.pd_solids.get(pd)).map(|ids| g.wire(ids)).collect();
                    let w = Wire::merge(parts);
                    if w.is_empty() { None } else { Some(w) }
                }
                _ => None,
            };
            self.wires.insert(key.to_string(), w);
        }
        self.wires[key].clone()
    }

    /// The right-hand viewport: the selected part drawn as an outline with X / Y / Z tape measures in millimetres.
    fn viewport_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("viewport").default_width(480.0).min_width(260.0).frame(egui::Frame::new().fill(BG1).stroke(egui::Stroke::new(1.0, LINE))).show(ctx, |ui| {
            panel_header(ui, "Viewport");
            // in Review the part shown is one of the components counted in the current entry
            if self.tab == Tab::Review {
                if let Some(s) = &self.session {
                    if let Some(g) = s.queue.get(s.i).filter(|_| !s.finished) {
                        let names: Vec<String> = g.members.iter().enumerate().filter(|(m, _)| g.sel[*m] && !(g.ask.contains(m) && g.answer.is_none())).map(|(_, m)| m.name.clone()).collect();
                        let keys: Vec<String> = names.iter().map(|n| tidy(n).to_lowercase()).collect();
                        if !keys.iter().any(|k| Some(k) == self.view_name.as_ref()) { self.view_name = keys.first().cloned(); }
                        if names.len() > 1 {
                            ui.horizontal(|ui| {
                                ui.label("Showing");
                                let cur = names.iter().zip(&keys).find(|(_, k)| Some(*k) == self.view_name.as_ref()).map(|(n, _)| n.clone()).unwrap_or_default();
                                egui::ComboBox::from_id_salt("vpart").selected_text(cur).show_ui(ui, |ui| {
                                    for (n, k) in names.iter().zip(&keys) { ui.selectable_value(&mut self.view_name, Some(k.clone()), n); }
                                });
                            });
                        }
                    } else { self.view_name = None; }
                }
            }
            let Some(key) = self.view_name.clone() else { ui.add_space(8.0); ui.label("  Select a part to see it here."); return; };
            let title = self.name_pds.get(&key).and_then(|pds| self.parts.iter().find(|p| Some(&p.pd) == pds.first())).map(|p| p.name.clone()).unwrap_or(key.clone());
            let wire = self.wire_for(&key);
            ui.label(egui::RichText::new(title).strong().color(egui::Color32::WHITE));
            match wire {
                None => { ui.label(if self.geo.is_none() { "This file's geometry could not be read." } else { "No drawable outline for this part (the file may hold it as a mesh)." }); }
                Some(w) => {
                    let z = w.size();
                    ui.horizontal(|ui| {
                        ui.colored_label(AXIS_X, format!("X {} mm", fmt_mm(z[0])));
                        ui.colored_label(AXIS_Y, format!("Y {} mm", fmt_mm(z[1])));
                        ui.colored_label(AXIS_Z, format!("Z {} mm", fmt_mm(z[2])));
                    });
                    let h = (ui.available_height() - 22.0).max(200.0);
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click_and_drag());
                    if resp.dragged_by(egui::PointerButton::Primary) {
                        let d = resp.drag_delta();
                        self.cam.yaw += d.x * 0.01;
                        self.cam.pitch = (self.cam.pitch + d.y * 0.01).clamp(-1.5, 1.5);
                    }
                    if resp.dragged_by(egui::PointerButton::Secondary) || resp.dragged_by(egui::PointerButton::Middle) { self.cam.pan += resp.drag_delta(); }
                    if resp.hovered() {
                        let sc = ui.input(|i| i.smooth_scroll_delta.y);
                        if sc != 0.0 { self.cam.zoom = (self.cam.zoom * (1.0 + sc * 0.002)).clamp(0.2, 20.0); }
                    }
                    if resp.double_clicked() { self.cam = Cam::default(); }
                    draw_viewport(&ui.painter_at(rect), rect, &w, &self.cam);
                    ui.label(egui::RichText::new("Drag: orbit   Right-drag: pan   Scroll: zoom   Double-click: reset").size(10.0).color(egui::Color32::GRAY));
                }
            }
        });
    }

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
        ui.label(format!("{} placed parts, {} different names. Click one to see it in the viewport.", self.parts.len(), names.len()));
        let mut v: Vec<_> = names.into_iter().collect();
        v.sort_by(|a, b| natural_cmp(&a.0, &b.0));
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("parts").striped(true).num_columns(2).show(ui, |ui| {
                ui.strong("Qty"); ui.strong("Part"); ui.end_row();
                for (n, c) in v {
                    ui.label(c.to_string());
                    let key = tidy(&n).to_lowercase();
                    let on = self.view_name.as_deref() == Some(key.as_str());
                    if ui.selectable_label(on, n).clicked() { self.view_name = Some(key); }
                    ui.end_row();
                }
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
        if self.tab != Tab::Bom { self.viewport_panel(ctx); }
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


const AXIS_X: egui::Color32 = egui::Color32::from_rgb(0xe8, 0x4d, 0x4d);
const AXIS_Y: egui::Color32 = egui::Color32::from_rgb(0x5c, 0xc8, 0x5c);
const AXIS_Z: egui::Color32 = egui::Color32::from_rgb(0x4d, 0x9b, 0xf0);

fn fmt_mm(v: f64) -> String { if v >= 100.0 { format!("{v:.1}") } else if v >= 1.0 { format!("{v:.2}") } else { format!("{v:.3}") } }

/// A round step size (1, 2, 5 x 10^n) that gives about `n` divisions of `len`.
fn nice_step(len: f64, n: f64) -> f64 {
    let raw = (len / n).max(1e-9);
    let mag = 10f64.powf(raw.log10().floor());
    let f = raw / mag;
    mag * if f < 1.5 { 1.0 } else if f < 3.5 { 2.0 } else if f < 7.5 { 5.0 } else { 10.0 }
}

fn draw_viewport(p: &egui::Painter, rect: egui::Rect, w: &Wire, cam: &Cam) {
    p.rect_filled(rect, 0, BG0);
    // faint floor grid under the part's footprint, for scale
    let c = w.center();
    let sz = w.size();
    let big = sz[0].max(sz[1]).max(sz[2]).max(1e-6);
    let (sy, cy) = cam.yaw.sin_cos();
    let (sp, cp) = cam.pitch.sin_cos();
    let scale = (rect.width().min(rect.height()) as f64 * 0.40 * cam.zoom as f64) / (big * 0.9);
    let ctr = rect.center() + cam.pan;
    // returns (screen point, depth)
    let proj = |q: V3| -> (egui::Pos2, f32) {
        let (x, y, z) = ((q[0] - c[0]) as f32, (q[1] - c[1]) as f32, (q[2] - c[2]) as f32);
        let x1 = cy * x - sy * y;
        let y1 = sy * x + cy * y;
        let up = z * cp + y1 * sp;
        let depth = y1 * cp - z * sp;
        (egui::pos2(ctr.x + x1 * scale as f32, ctr.y - up * scale as f32), depth)
    };
    let (mn, mx) = (w.min, w.max);
    // the part
    let line = egui::Stroke::new(1.2, egui::Color32::from_rgb(0xb4, 0xcc, 0xe6));
    for l in &w.lines {
        let pts: Vec<egui::Pos2> = l.iter().map(|q| proj(*q).0).collect();
        p.add(egui::Shape::line(pts, line));
    }
    // bounding box
    let bx = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40));
    let corner = |i: usize| [if i & 1 == 0 { mn[0] } else { mx[0] }, if i & 2 == 0 { mn[1] } else { mx[1] }, if i & 4 == 0 { mn[2] } else { mx[2] }];
    for i in 0..8usize { for b in [1usize, 2, 4] { if i & b == 0 { p.line_segment([proj(corner(i)).0, proj(corner(i | b)).0], bx); } } }

    // tape measures sit in front of the part: pick the sides that face the camera
    let off = big * 0.10;
    let front = |a: f64, b: f64, pa: V3, pb: V3| if proj(pa).1 <= proj(pb).1 { a } else { b };
    let yv = front(mn[1] - off, mx[1] + off, [c[0], mn[1] - off, mn[2]], [c[0], mx[1] + off, mn[2]]);
    let xv = front(mn[0] - off, mx[0] + off, [mn[0] - off, c[1], mn[2]], [mx[0] + off, c[1], mn[2]]);
    let zf = mn[2];
    let xo = if xv < c[0] { mx[0] } else { mn[0] };
    tape(p, &proj, proj(c).0, [mn[0], yv, zf], [mx[0], yv, zf], 0, AXIS_X, "X");
    tape(p, &proj, proj(c).0, [xv, mn[1], zf], [xv, mx[1], zf], 1, AXIS_Y, "Y");
    tape(p, &proj, proj(c).0, [xo, yv, mn[2]], [xo, yv, mx[2]], 2, AXIS_Z, "Z");
    // orientation gizmo
    let o = rect.left_bottom() + egui::vec2(34.0, -34.0);
    for (k, col, name) in [(0usize, AXIS_X, "X"), (1, AXIS_Y, "Y"), (2, AXIS_Z, "Z")] {
        let mut d = [0.0; 3]; d[k] = 1.0;
        let a = proj(c).0;
        let b = proj([c[0] + d[0] * big, c[1] + d[1] * big, c[2] + d[2] * big]).0;
        let v = (b - a).normalized() * 22.0;
        p.line_segment([o, o + v], egui::Stroke::new(2.0, col));
        p.text(o + v * 1.35, egui::Align2::CENTER_CENTER, name, egui::FontId::proportional(10.0), col);
    }
}

/// One tape measure: a ruler line from `a` to `b` in model space with ticks every round step and a label at the far end.
fn tape(p: &egui::Painter, proj: &dyn Fn(V3) -> (egui::Pos2, f32), away: egui::Pos2, a: V3, b: V3, axis: usize, col: egui::Color32, name: &str) {
    let len = (b[axis] - a[axis]).abs();
    if len <= 0.0 { return; }
    let (pa, pb) = (proj(a).0, proj(b).0);
    p.line_segment([pa, pb], egui::Stroke::new(2.0, col));
    let dir = pb - pa;
    let mut tick_dir = if dir.length() > 0.1 { egui::vec2(-dir.y, dir.x).normalized() } else { egui::vec2(0.0, 1.0) };
    if tick_dir.dot((pa + pb.to_vec2()).to_vec2() * 0.5 - away.to_vec2()) < 0.0 { tick_dir = -tick_dir; }
    let step = nice_step(len, 6.0);
    let at = |t: f64| { let mut q = a; q[axis] = a[axis] + t * (b[axis] - a[axis]).signum(); q };
    let mut t = 0.0;
    let mut n = 0;
    while t <= len + 1e-9 && n < 200 {
        let s = proj(at(t)).0;
        p.line_segment([s - tick_dir * 4.0, s + tick_dir * 4.0], egui::Stroke::new(1.5, col));
        if n % 1 == 0 && t > 0.0 && (len - t) > step * 0.4 {
            p.text(s + tick_dir * 12.0, egui::Align2::CENTER_CENTER, if step >= 1.0 { format!("{t:.0}") } else { format!("{t:.1}") }, egui::FontId::proportional(9.0), col);
        }
        t += step; n += 1;
    }
    // end cap and total
    p.line_segment([pb - tick_dir * 7.0, pb + tick_dir * 7.0], egui::Stroke::new(2.0, col));
    let tag = format!("{name} {} mm", fmt_mm(len));
    let pos = pa + dir * 0.5 + tick_dir * 30.0;
    let galley = p.layout_no_wrap(tag, egui::FontId::proportional(11.0), egui::Color32::WHITE);
    let r = egui::Rect::from_center_size(pos, galley.size() + egui::vec2(8.0, 4.0));
    p.rect_filled(r, 2, col.gamma_multiply(0.85));
    p.galley(r.center() - galley.size() / 2.0, galley, egui::Color32::WHITE);
}

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1360.0, 800.0]).with_title("Auto BOM").with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("Auto BOM", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

#[allow(dead_code)]
fn _unused(_: Action) {}

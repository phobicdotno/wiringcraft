//! WiringCraft desktop application.
//!
//! Usage: `wiringcraft-app [project-folder]`. Without a folder the last project is reopened.

mod actions;
mod canvas;
mod chrome;
mod icons;
mod panels;
mod prefs;
mod state;
mod theme;

use std::path::PathBuf;

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, Pos2, RichText, Stroke, Vec2};

use actions::Action;
use canvas::Editor;
use prefs::{Chrome, Prefs};
use state::{Open, Selection, Tool};

enum Dialog {
    OpenPath(String),
    Commit(String),
    NewBranch(String),
    History(Vec<wc_vcs::subway::Row>),
    Library(Vec<wc_library::ItemStatus>),
    Message(String),
}

/// Work a dialog asks for after it closes.
type Then = Box<dyn FnOnce(&mut App)>;

struct App {
    prefs: Prefs,
    open: Option<Open>,
    ed: Editor,
    chrome: chrome::ChromeState,
    panels: panels::PanelState,
    dialog: Option<Dialog>,
}

struct StatusView<'a> {
    open: Option<&'a Open>,
    ed: &'a Editor,
    prefs: &'a Prefs,
}

impl chrome::Status for StatusView<'_> {
    fn active(&self, a: Action) -> bool {
        match a {
            Action::Select => self.ed.tool == Tool::Select,
            Action::PlaceSymbol => self.ed.tool == Tool::Place,
            Action::DrawWire => self.ed.tool == Tool::Wire,
            Action::ToggleGrid => self.prefs.show_grid,
            Action::ToggleChrome => self.prefs.chrome == Chrome::Ribbon,
            _ => false,
        }
    }

    fn enabled(&self, a: Action) -> bool {
        let has = self.open.is_some();
        match a {
            Action::Open | Action::ToggleChrome | Action::ToggleGrid => true,
            Action::Save => self.open.is_some_and(|o| o.dirty),
            Action::Undo => self.open.is_some_and(Open::can_undo),
            Action::Redo => self.open.is_some_and(Open::can_redo),
            Action::Delete => has && self.ed.selection.is_some(),
            Action::Rotate => has && matches!(self.ed.selection, Some(Selection::Symbol(_))),
            _ => has,
        }
    }
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, dir: Option<PathBuf>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::light());
        let mut prefs = Prefs::load();
        let mut app = Self {
            prefs: Prefs::default(),
            open: None,
            ed: Editor::default(),
            chrome: Default::default(),
            panels: Default::default(),
            dialog: None,
        };
        let dir = dir.or_else(|| prefs.recent.first().cloned());
        if let Some(d) = dir {
            app.open_dir(&d, &mut prefs);
        }
        app.prefs = prefs;
        app
    }

    fn open_dir(&mut self, dir: &std::path::Path, prefs: &mut Prefs) {
        match Open::load(dir) {
            Ok(o) => {
                self.ed = Editor::default();
                self.ed.message = format!("Opened {}", dir.display());
                self.open = Some(o);
                prefs.remember(dir.to_path_buf());
                prefs.save();
            }
            Err(e) => self.dialog = Some(Dialog::Message(format!("Could not open {}:\n{e}", dir.display()))),
        }
    }

    fn run(&mut self, a: Action) {
        let page = self.ed.page;
        match a {
            Action::Open => {
                let start = self
                    .open
                    .as_ref()
                    .map(|o| o.dir.display().to_string())
                    .unwrap_or_default();
                self.dialog = Some(Dialog::OpenPath(start));
            }
            Action::ToggleChrome => {
                self.prefs.chrome = match self.prefs.chrome {
                    Chrome::Classic => Chrome::Ribbon,
                    Chrome::Ribbon => Chrome::Classic,
                };
                self.prefs.save();
            }
            Action::ToggleGrid => {
                self.prefs.show_grid = !self.prefs.show_grid;
                self.prefs.save();
            }
            Action::Select => {
                self.ed.tool = Tool::Select;
                self.ed.wire = None;
            }
            Action::PlaceSymbol => {
                self.ed.tool = Tool::Place;
                self.ed.wire = None;
            }
            Action::DrawWire => self.ed.tool = Tool::Wire,
            Action::ZoomIn => self.zoom(1.25),
            Action::ZoomOut => self.zoom(0.8),
            Action::ZoomFit => self.ed.view.fit = true,
            _ => {}
        }
        let Some(open) = self.open.as_mut() else { return };
        match a {
            Action::Save => match open.save() {
                Ok(n) => self.ed.message = format!("Saved ({n} files changed)."),
                Err(e) => self.dialog = Some(Dialog::Message(format!("Saving failed:\n{e}"))),
            },
            Action::Undo => {
                if open.undo() {
                    self.ed.selection = None;
                }
            }
            Action::Redo => {
                if open.redo() {
                    self.ed.selection = None;
                }
            }
            Action::Delete => {
                if let Some(sel) = self.ed.selection.take() {
                    open.delete(page, &sel);
                }
            }
            Action::Rotate => {
                if let Some(Selection::Symbol(id)) = &self.ed.selection {
                    open.rotate(page, id);
                }
            }
            Action::AddPage => {
                self.ed.page = open.add_page();
                self.ed.selection = None;
                self.ed.view.fit = true;
            }
            Action::ExportPdf => self.dialog = Some(Dialog::Message(export_pdf(open))),
            Action::LibraryStatus | Action::LibraryUpdate => {
                match wc_library::Library::open(&open.dir, &open.project) {
                    Ok(lib) => {
                        if a == Action::LibraryUpdate {
                            open.checkpoint();
                            match wc_library::update(&open.dir, &mut open.project, &lib, None) {
                                Ok(done) => {
                                    let (syms, _) = wc_core::store::vendored_symbols(&open.dir, &open.project);
                                    open.symbols = syms;
                                    self.ed.message = format!("Updated {} library items.", done.len());
                                }
                                Err(e) => self.ed.message = format!("Update failed: {e}"),
                            }
                        }
                        self.dialog = Some(Dialog::Library(wc_library::status(&open.dir, &open.project, &lib)));
                    }
                    Err(e) => self.dialog = Some(Dialog::Message(format!("The library could not be opened:\n{e}"))),
                }
            }
            Action::Commit => self.dialog = Some(Dialog::Commit(String::new())),
            Action::NewBranch => self.dialog = Some(Dialog::NewBranch(String::new())),
            Action::History => match wc_vcs::subway::history(&wc_vcs::Git::new(&open.dir), 200) {
                Ok(rows) => self.dialog = Some(Dialog::History(rows)),
                Err(e) => self.dialog = Some(Dialog::Message(format!("No history yet:\n{e}"))),
            },
            _ => {}
        }
    }

    fn zoom(&mut self, f: f32) {
        self.ed.view.zoom = (self.ed.view.zoom * f).clamp(0.3, 60.0);
    }

    fn shortcuts(&mut self, ctx: &egui::Context) -> Vec<Action> {
        let mut out = Vec::new();
        let ctrl = |k| KeyboardShortcut::new(Modifiers::COMMAND, k);
        ctx.input_mut(|i| {
            for (k, a) in [
                (Key::S, Action::Save),
                (Key::Z, Action::Undo),
                (Key::Y, Action::Redo),
                (Key::O, Action::Open),
                (Key::N, Action::AddPage),
                (Key::P, Action::ExportPdf),
                (Key::K, Action::Commit),
            ] {
                if i.consume_shortcut(&ctrl(k)) {
                    out.push(a);
                }
            }
        });
        if ctx.wants_keyboard_input() || self.dialog.is_some() {
            return out;
        }
        ctx.input(|i| {
            for (k, a) in [
                (Key::S, Action::PlaceSymbol),
                (Key::W, Action::DrawWire),
                (Key::R, Action::Rotate),
                (Key::Delete, Action::Delete),
                (Key::G, Action::ToggleGrid),
                (Key::F, Action::ZoomFit),
                (Key::Plus, Action::ZoomIn),
                (Key::Minus, Action::ZoomOut),
            ] {
                if i.key_pressed(k) && i.modifiers.is_none() {
                    out.push(a);
                }
            }
            if i.key_pressed(Key::Escape) {
                if self.ed.wire.is_some() {
                    self.ed.wire = None;
                } else {
                    out.push(Action::Select);
                }
            }
        });
        out
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let mut keep = true;
        let mut then: Option<Then> = None;
        let title = match &dialog {
            Dialog::OpenPath(_) => "Open project",
            Dialog::Commit(_) => "Record a version",
            Dialog::NewBranch(_) => "New branch",
            Dialog::History(_) => "History",
            Dialog::Library(_) => "Library status",
            Dialog::Message(_) => "WiringCraft",
        };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(true)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| match &mut dialog {
                Dialog::Message(m) => {
                    ui.label(m.as_str());
                    if ui.button("OK").clicked() {
                        keep = false;
                    }
                }
                Dialog::OpenPath(path) => {
                    ui.label("Project folder (the one holding project.toml):");
                    let r = ui.add(egui::TextEdit::singleline(path).desired_width(420.0));
                    for recent in &self.prefs.recent {
                        if ui.link(recent.display().to_string()).clicked() {
                            *path = recent.display().to_string();
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Open").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
                            let p = PathBuf::from(path.trim());
                            then = Some(Box::new(move |app: &mut App| {
                                let mut prefs = std::mem::take(&mut app.prefs);
                                app.open_dir(&p, &mut prefs);
                                app.prefs = prefs;
                            }));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Commit(msg) => {
                    ui.label("What changed?");
                    ui.add(egui::TextEdit::multiline(msg).desired_rows(3).desired_width(380.0));
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!msg.trim().is_empty(), egui::Button::new("Commit"))
                            .clicked()
                        {
                            let m = msg.trim().to_owned();
                            then = Some(Box::new(move |app: &mut App| app.commit(&m)));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::NewBranch(name) => {
                    ui.label("Branch name (for example customer-changes):");
                    ui.text_edit_singleline(name);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!name.trim().is_empty(), egui::Button::new("Create and switch"))
                            .clicked()
                        {
                            let n = name.trim().to_owned();
                            then = Some(Box::new(move |app: &mut App| app.branch(&n)));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::History(rows) => {
                    subway(ui, rows);
                    if ui.button("Close").clicked() {
                        keep = false;
                    }
                }
                Dialog::Library(items) => {
                    let t = theme::Theme::of(ui.ctx());
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        egui::Grid::new("libstatus")
                            .striped(true)
                            .num_columns(3)
                            .show(ui, |ui| {
                                for it in items.iter() {
                                    let (c, s) = match it.state {
                                        wc_library::State::UpToDate => {
                                            (egui::Color32::from_rgb(0x2e, 0x8b, 0x57), "up to date")
                                        }
                                        wc_library::State::Changed => (t.selection, "newer in library"),
                                        wc_library::State::NotLocked => (t.wire, "not pinned"),
                                        wc_library::State::GoneFromLibrary => (t.error, "gone from library"),
                                    };
                                    chrome::dot(ui, c);
                                    ui.label(RichText::new(&it.id).monospace());
                                    ui.label(s);
                                    ui.end_row();
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        if items.iter().any(|i| i.state == wc_library::State::Changed)
                            && ui.button("Take all newer versions").clicked()
                        {
                            then = Some(Box::new(|app: &mut App| app.run(Action::LibraryUpdate)));
                            keep = false;
                        }
                        if ui.button("Close").clicked() {
                            keep = false;
                        }
                    });
                }
            });
        if keep {
            self.dialog = Some(dialog);
        }
        if let Some(f) = then {
            f(self);
        }
    }

    fn commit(&mut self, msg: &str) {
        let Some(open) = self.open.as_mut() else { return };
        if let Err(e) = open.save() {
            self.dialog = Some(Dialog::Message(format!("Saving failed:\n{e}")));
            return;
        }
        let git = wc_vcs::Git::new(&open.dir);
        let res = if git.is_repo() { Ok(()) } else { git.init() }.and_then(|()| git.commit(msg));
        match res {
            Ok(Some(id)) => self.ed.message = format!("Recorded version {}.", id.get(..8).unwrap_or(&id)),
            Ok(None) => self.ed.message = "Nothing changed since the last version.".into(),
            Err(e) => self.dialog = Some(Dialog::Message(format!("Commit failed:\n{e}"))),
        }
        open.branch = git.current_branch().unwrap_or_default();
    }

    fn branch(&mut self, name: &str) {
        let Some(open) = self.open.as_mut() else { return };
        let git = wc_vcs::Git::new(&open.dir);
        match git.create_branch(name).and_then(|()| git.switch(name)) {
            Ok(()) => {
                open.branch = name.to_owned();
                self.ed.message = format!("Now on branch {name}.");
            }
            Err(e) => self.dialog = Some(Dialog::Message(format!("Could not create the branch:\n{e}"))),
        }
    }
}

/// The version history drawn as a subway map: one coloured line per branch lane.
fn subway(ui: &mut egui::Ui, rows: &[wc_vcs::subway::Row]) {
    const LINES: [egui::Color32; 6] = [
        egui::Color32::from_rgb(0xd3, 0x3f, 0x2f),
        egui::Color32::from_rgb(0x1f, 0x6f, 0xc5),
        egui::Color32::from_rgb(0x2a, 0x9d, 0x4b),
        egui::Color32::from_rgb(0xe0, 0x9a, 0x00),
        egui::Color32::from_rgb(0x8a, 0x3f, 0xc2),
        egui::Color32::from_rgb(0x12, 0x9a, 0xa0),
    ];
    let lane_w = 18.0;
    let row_h = 26.0;
    let lanes = rows
        .iter()
        .flat_map(|r| std::iter::once(r.lane).chain(r.edges.iter().map(|e| e.lane)))
        .max()
        .unwrap_or(0)
        + 1;
    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
        let width = lanes as f32 * lane_w + 420.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, rows.len() as f32 * row_h), egui::Sense::hover());
        let p = ui.painter();
        let at = |lane: usize, row: usize| {
            Pos2::new(
                rect.min.x + 10.0 + lane as f32 * lane_w,
                rect.min.y + row_h / 2.0 + row as f32 * row_h,
            )
        };
        let colour = |lane: usize| LINES.get(lane % LINES.len()).copied().unwrap_or(egui::Color32::GRAY);
        for (i, r) in rows.iter().enumerate() {
            for e in &r.edges {
                let a = at(r.lane, i);
                let b = at(e.lane, i + 1);
                let c = at(e.lane, e.to_row.saturating_sub(1).max(i + 1));
                let d = at(rows.get(e.to_row).map(|t| t.lane).unwrap_or(e.lane), e.to_row);
                p.add(egui::Shape::line(
                    vec![a, b, c, d],
                    Stroke::new(4.0_f32, colour(e.lane)),
                ));
            }
        }
        for (i, r) in rows.iter().enumerate() {
            let c = at(r.lane, i);
            p.circle_filled(c, 6.0, egui::Color32::WHITE);
            p.circle_stroke(c, 6.0, Stroke::new(3.0_f32, colour(r.lane)));
            let mut label = String::new();
            for name in &r.commit.refs {
                label.push_str(&format!("[{name}] "));
            }
            label.push_str(&r.commit.subject);
            let x = rect.min.x + 10.0 + lanes as f32 * lane_w + 8.0;
            let font = egui::FontId::proportional(13.0);
            let tc = if r.commit.head {
                ui.visuals().strong_text_color()
            } else {
                ui.visuals().text_color()
            };
            p.text(Pos2::new(x, c.y), egui::Align2::LEFT_CENTER, label, font, tc);
        }
    });
}

fn export_pdf(open: &Open) -> String {
    let name: String = open
        .project
        .meta
        .name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    let dir = open.dir.join("export");
    let path = dir.join(format!("{}.pdf", if name.is_empty() { "project" } else { &name }));
    let (bytes, report) = wc_pdf::export(&open.project, &open.symbols, &wc_pdf::Options::default());
    match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, bytes)) {
        Ok(()) => {
            let mut m = format!("Exported {} pages to\n{}", report.pages, path.display());
            if !report.warnings.is_empty() {
                m.push_str("\n\nWarnings:\n");
                m.push_str(&report.warnings.join("\n"));
            }
            m
        }
        Err(e) => format!("Export failed: {e}"),
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let status = StatusView {
            open: self.open.as_ref(),
            ed: &self.ed,
            prefs: &self.prefs,
        };
        let mut actions = chrome::show(ctx, self.prefs.chrome, &mut self.chrome, &status);
        actions.extend(self.shortcuts(ctx));
        for a in actions {
            self.run(a);
        }

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| panels::status_bar(ui, self.open.as_ref(), &self.ed));

        if let Some(open) = self.open.as_mut() {
            egui::SidePanel::left("navigator")
                .default_width(220.0)
                .show(ctx, |ui| panels::navigator(ui, open, &mut self.ed));
            egui::SidePanel::right("inspector")
                .default_width(260.0)
                .show(ctx, |ui| {
                    panels::library(ui, open, &mut self.ed, &mut self.panels);
                    ui.separator();
                    panels::properties(ui, open, &mut self.ed, &mut self.panels);
                });
            egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx, |ui| {
                canvas::show(ui, open, &mut self.ed, self.prefs.show_grid, self.prefs.snap);
            });
        } else {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(120.0);
                    ui.heading("WiringCraft");
                    ui.label("Open a project folder to start (File → Open, Ctrl+O).");
                    ui.label("Create one from the command line with: wiringcraft new <folder>");
                });
            });
        }
        self.dialogs(ctx);

        let title = match &self.open {
            Some(o) => format!(
                "{}{} – WiringCraft",
                if o.dirty { "• " } else { "" },
                o.project.meta.name
            ),
            None => "WiringCraft".into(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }
}

fn main() -> eframe::Result {
    let dir = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 860.0])
            .with_title("WiringCraft"),
        ..Default::default()
    };
    eframe::run_native("WiringCraft", options, Box::new(|cc| Ok(Box::new(App::new(cc, dir)))))
}

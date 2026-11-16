//! WiringCraft desktop application.
//!
//! Usage: `wiringcraft-app [project-folder]`. Without a folder the last project is reopened.

mod actions;
mod canvas;
mod chrome;
mod edit;
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
use wc_core::project::PageSize;

/// Title-block fields offered in the project properties, in this order.
const TITLE_FIELDS: &[(&str, &str)] = &[
    ("customer", "Customer"),
    ("project_number", "Project number"),
    ("plant", "Plant"),
    ("drawn_by", "Drawn by"),
    ("checked_by", "Checked by"),
    ("revision", "Revision"),
    ("date", "Date"),
];

enum Dialog {
    OpenPath(String),
    NewProject {
        dir: String,
        name: String,
    },
    ProjectProps {
        name: String,
        fields: Vec<(String, String)>,
    },
    PageProps {
        number: String,
        title: String,
        size: PageSize,
        error: Option<String>,
    },
    DeletePage(usize),
    /// Reports (parts, devices, wires, nets, terminals, cables) as tables.
    Reports(wc_report::Kind),
    /// Write a new text at this point on the current page.
    Text(wc_core::geom::Point, String, bool),
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
    clipboard: edit::Clipboard,
    /// Text for the system clipboard after a copy, so Ctrl+V reaches the app as a paste even
    /// when the system clipboard was empty.
    copied_text: Option<String>,
}

struct StatusView<'a> {
    open: Option<&'a Open>,
    ed: &'a Editor,
    prefs: &'a Prefs,
    clipboard: &'a edit::Clipboard,
}

impl chrome::Status for StatusView<'_> {
    fn active(&self, a: Action) -> bool {
        match a {
            Action::Select => self.ed.tool == Tool::Select,
            Action::PlaceSymbol => self.ed.tool == Tool::Place,
            Action::DrawWire => self.ed.tool == Tool::Wire,
            Action::TextTool => self.ed.tool == Tool::Text,
            Action::LineTool => self.ed.tool == Tool::Line,
            Action::RectTool => self.ed.tool == Tool::Rect,
            Action::ToggleGrid => self.prefs.show_grid,
            Action::ToggleChrome => self.prefs.chrome == Chrome::Ribbon,
            _ => false,
        }
    }

    fn enabled(&self, a: Action) -> bool {
        let has = self.open.is_some();
        match a {
            Action::NewProject | Action::Open | Action::ToggleChrome | Action::ToggleGrid => true,
            Action::DeletePage => self.open.is_some_and(|o| o.project.pages.len() > 1),
            Action::PageUp => has && self.ed.page > 0,
            Action::PageDown => self.open.is_some_and(|o| self.ed.page + 1 < o.project.pages.len()),
            Action::Save => self.open.is_some_and(|o| o.dirty),
            Action::Undo => self.open.is_some_and(Open::can_undo),
            Action::Redo => self.open.is_some_and(Open::can_redo),
            Action::Delete | Action::Copy | Action::Cut => has && !self.ed.selection.is_empty(),
            Action::Rotate | Action::Mirror => {
                has && self.ed.selection.iter().any(|s| matches!(s, Selection::Symbol(_)))
            }
            Action::Paste => has && !self.clipboard.is_empty(),
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
            clipboard: edit::Clipboard::default(),
            copied_text: None,
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
            Action::NewProject => {
                let parent = self
                    .open
                    .as_ref()
                    .and_then(|o| o.dir.parent())
                    .map(|d| d.join("new-project").display().to_string())
                    .unwrap_or_default();
                self.dialog = Some(Dialog::NewProject {
                    dir: parent,
                    name: String::new(),
                });
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
            Action::TextTool | Action::LineTool | Action::RectTool => {
                self.ed.tool = match a {
                    Action::TextTool => Tool::Text,
                    Action::LineTool => Tool::Line,
                    _ => Tool::Rect,
                };
                self.ed.wire = None;
                self.ed.shape_start = None;
                self.ed.message = match a {
                    Action::TextTool => "Click where the text goes.",
                    _ => "Click the first point.",
                }
                .into();
            }
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
                    self.ed.selection.clear();
                }
            }
            Action::Redo => {
                if open.redo() {
                    self.ed.selection.clear();
                }
            }
            Action::Delete => {
                let sel = std::mem::take(&mut self.ed.selection);
                open.delete_many(page, &sel);
            }
            Action::Rotate => open.rotate_many(page, &self.ed.selection),
            Action::Mirror => open.mirror_many(page, &self.ed.selection),
            Action::SelectAll => {
                self.ed.tool = Tool::Select;
                self.ed.selection = open.select_all(page);
            }
            Action::Copy | Action::Cut => {
                let clip = open.copy(page, &self.ed.selection);
                if !clip.is_empty() {
                    let n = clip.symbols.len() + clip.wires.len();
                    self.copied_text = Some(clip.summary());
                    self.clipboard = clip;
                    if a == Action::Cut {
                        let sel = std::mem::take(&mut self.ed.selection);
                        open.delete_many(page, &sel);
                        self.ed.message = format!("Cut {n} items.");
                    } else {
                        self.ed.message = format!("Copied {n} items.");
                    }
                }
            }
            Action::Paste => {
                // At the pointer when it is over the sheet, otherwise a little below the original.
                let at = self
                    .ed
                    .cursor
                    .unwrap_or(self.clipboard.anchor + wc_core::geom::Point::new(10.0, 10.0));
                let pasted = open.paste(page, &self.clipboard, at);
                if !pasted.is_empty() {
                    self.ed.tool = Tool::Select;
                    self.ed.selection = pasted;
                }
            }
            Action::AddPage => {
                self.ed.page = open.add_page();
                self.ed.selection.clear();
                self.ed.view.fit = true;
            }
            Action::DuplicatePage => {
                if let Some(i) = open.duplicate_page(page) {
                    self.ed.page = i;
                    self.ed.selection.clear();
                    self.ed.view.fit = true;
                }
            }
            Action::PageUp | Action::PageDown => {
                if let Some(i) = open.move_page(page, a == Action::PageUp) {
                    self.ed.page = i;
                }
            }
            Action::PageProps => {
                if let Some(p) = open.project.pages.get(page) {
                    self.dialog = Some(Dialog::PageProps {
                        number: p.number.clone(),
                        title: p.title.clone(),
                        size: p.size,
                        error: None,
                    });
                }
            }
            Action::DeletePage => self.dialog = Some(Dialog::DeletePage(page)),
            Action::ProjectProps => {
                let block = &open.project.meta.title_block;
                let mut fields: Vec<(String, String)> = TITLE_FIELDS
                    .iter()
                    .map(|(k, _)| ((*k).to_owned(), block.get(*k).cloned().unwrap_or_default()))
                    .collect();
                for (k, v) in block {
                    if !TITLE_FIELDS.iter().any(|(f, _)| f == k) {
                        fields.push((k.clone(), v.clone()));
                    }
                }
                self.dialog = Some(Dialog::ProjectProps {
                    name: open.project.meta.name.clone(),
                    fields,
                });
            }
            Action::ExportPdf => self.dialog = Some(Dialog::Message(export_pdf(open))),
            Action::Reports => self.dialog = Some(Dialog::Reports(wc_report::Kind::Parts)),
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
                (Key::R, Action::Reports),
                (Key::A, Action::SelectAll),
            ] {
                if k == Key::N && i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, k)) {
                    out.push(Action::NewProject);
                } else if i.consume_shortcut(&ctrl(k)) {
                    out.push(a);
                }
            }
        });
        if ctx.wants_keyboard_input() || self.dialog.is_some() {
            return out;
        }
        ctx.input(|i| {
            // The window turns Ctrl+C, Ctrl+X and Ctrl+V into clipboard events, not key presses.
            for e in &i.events {
                match e {
                    egui::Event::Copy => out.push(Action::Copy),
                    egui::Event::Cut => out.push(Action::Cut),
                    egui::Event::Paste(_) => out.push(Action::Paste),
                    _ => {}
                }
            }
            for (k, a) in [
                (Key::S, Action::PlaceSymbol),
                (Key::W, Action::DrawWire),
                (Key::R, Action::Rotate),
                (Key::M, Action::Mirror),
                (Key::T, Action::TextTool),
                (Key::L, Action::LineTool),
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
                if self.ed.shape_start.is_some() {
                    self.ed.shape_start = None;
                } else if self.ed.wire.is_some() {
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
            Dialog::NewProject { .. } => "New project",
            Dialog::ProjectProps { .. } => "Project properties",
            Dialog::PageProps { .. } => "Page properties",
            Dialog::DeletePage(_) => "Delete page",
            Dialog::Text(..) => "Text",
            Dialog::Reports(_) => "Reports",
            Dialog::Commit(_) => "Record a version",
            Dialog::NewBranch(_) => "New branch",
            Dialog::History(_) => "History",
            Dialog::Library(_) => "Library status",
            Dialog::Message(_) => "WiringCraft",
        };
        let wide = matches!(dialog, Dialog::Reports(_));
        let screen = ctx.screen_rect();
        egui::Window::new(title)
            .collapsible(false)
            .resizable(true)
            .min_width(if wide { (screen.width() * 0.75).min(1100.0) } else { 0.0 })
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
                Dialog::NewProject { dir, name } => {
                    egui::Grid::new("newproj")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Folder");
                            ui.add(egui::TextEdit::singleline(dir).desired_width(380.0));
                            ui.end_row();
                            ui.label("Name");
                            ui.add(
                                egui::TextEdit::singleline(name)
                                    .hint_text("the folder name")
                                    .desired_width(380.0),
                            );
                            ui.end_row();
                        });
                    ui.label(
                        RichText::new("A new folder with one cover page, recorded as a git repository.")
                            .color(theme::Theme::of(ui.ctx()).faint),
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!dir.trim().is_empty(), egui::Button::new("Create"))
                            .clicked()
                        {
                            let d = PathBuf::from(dir.trim());
                            let n = name.trim().to_owned();
                            then = Some(Box::new(move |app: &mut App| app.new_project(&d, &n)));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::ProjectProps { name, fields } => {
                    egui::Grid::new("projprops")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Project name");
                            ui.add(egui::TextEdit::singleline(name).desired_width(300.0));
                            ui.end_row();
                            for (k, v) in fields.iter_mut() {
                                let label = TITLE_FIELDS.iter().find(|(f, _)| f == k).map_or(k.as_str(), |(_, l)| l);
                                ui.label(label);
                                ui.add(egui::TextEdit::singleline(v).desired_width(300.0));
                                ui.end_row();
                            }
                        });
                    ui.label(
                        RichText::new("These fields fill the title block on every page.")
                            .color(theme::Theme::of(ui.ctx()).faint),
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!name.trim().is_empty(), egui::Button::new("OK"))
                            .clicked()
                        {
                            let n = name.clone();
                            let f = fields.clone();
                            then = Some(Box::new(move |app: &mut App| {
                                if let Some(o) = app.open.as_mut()
                                    && let Err(e) = o.set_project_props(&n, &f)
                                {
                                    app.ed.message = e;
                                }
                            }));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::PageProps {
                    number,
                    title,
                    size,
                    error,
                } => {
                    egui::Grid::new("pageprops")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Number");
                            ui.add(egui::TextEdit::singleline(number).desired_width(120.0));
                            ui.end_row();
                            ui.label("Title");
                            ui.add(egui::TextEdit::singleline(title).desired_width(300.0));
                            ui.end_row();
                            ui.label("Sheet size");
                            egui::ComboBox::from_id_salt("pagesize")
                                .selected_text(size_label(*size))
                                .show_ui(ui, |ui| {
                                    for s in [PageSize::A4, PageSize::A3, PageSize::A2] {
                                        ui.selectable_value(size, s, size_label(s));
                                    }
                                });
                            ui.end_row();
                        });
                    if let Some(e) = error {
                        ui.label(RichText::new(e.as_str()).color(theme::Theme::of(ui.ctx()).error));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked()
                            && let Some(o) = self.open.as_mut()
                        {
                            match o.set_page_props(self.ed.page, number, title, *size) {
                                Ok(i) => {
                                    self.ed.page = i;
                                    self.ed.view.fit = true;
                                    keep = false;
                                }
                                Err(e) => *error = Some(e),
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Reports(kind) => {
                    if let Some(o) = self.open.as_ref() {
                        let inputs = wc_report::Inputs {
                            project: &o.project,
                            symbols: &o.symbols,
                            parts: &o.parts,
                        };
                        ui.horizontal(|ui| {
                            for k in wc_report::Kind::ALL {
                                let label = report_title(k);
                                if ui.selectable_label(*kind == k, label).clicked() {
                                    *kind = k;
                                }
                            }
                        });
                        ui.separator();
                        let table = wc_report::build(*kind, &inputs);
                        report_table(ui, &table);
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("Save as CSV").clicked() {
                                let path = o.dir.join("export").join(format!("{}.csv", kind.name()));
                                let res = std::fs::create_dir_all(o.dir.join("export"))
                                    .and_then(|()| std::fs::write(&path, table.to_csv()));
                                self.ed.message = match res {
                                    Ok(()) => format!("Saved {}", path.display()),
                                    Err(e) => format!("Could not save {}: {e}", path.display()),
                                };
                            }
                            if ui.button("Close").clicked() {
                                keep = false;
                            }
                            ui.label(
                                RichText::new(format!("{} rows", table.rows.len()))
                                    .color(theme::Theme::of(ui.ctx()).faint),
                            );
                        });
                    } else {
                        keep = false;
                    }
                }
                Dialog::Text(at, text, focused) => {
                    let r = ui.add(egui::TextEdit::singleline(text).desired_width(320.0));
                    if !*focused {
                        r.request_focus();
                        *focused = true;
                    }
                    ui.horizontal(|ui| {
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        if (ui
                            .add_enabled(!text.trim().is_empty(), egui::Button::new("OK"))
                            .clicked()
                            || enter)
                            && !text.trim().is_empty()
                        {
                            if let Some(o) = self.open.as_mut()
                                && let Some(sel) = o.add_graphic(
                                    self.ed.page,
                                    wc_core::geom::Shape::Text {
                                        at: *at,
                                        text: text.trim().to_owned(),
                                        height: 3.5,
                                        align: wc_core::geom::Align::Left,
                                    },
                                )
                            {
                                self.ed.select_only(sel);
                            }
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                            keep = false;
                        }
                    });
                }
                Dialog::DeletePage(i) => {
                    let i = *i;
                    let what = self
                        .open
                        .as_ref()
                        .and_then(|o| o.project.pages.get(i))
                        .map(|p| {
                            format!(
                                "page {} \u{201c}{}\u{201d} with {} symbols and {} wires",
                                p.number,
                                p.title,
                                p.symbols.len(),
                                p.wires.len()
                            )
                        })
                        .unwrap_or_default();
                    ui.label(format!("Delete {what}? Undo brings it back."));
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            if let Some(o) = self.open.as_mut() {
                                match o.delete_page(i) {
                                    Ok(next) => {
                                        self.ed.page = next;
                                        self.ed.selection.clear();
                                        self.ed.wire = None;
                                        self.ed.view.fit = true;
                                    }
                                    Err(e) => self.ed.message = e,
                                }
                            }
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

    fn new_project(&mut self, dir: &std::path::Path, name: &str) {
        let name = if name.is_empty() {
            dir.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Project".into())
        } else {
            name.to_owned()
        };
        if let Err(e) = wc_core::store::create(dir, &name) {
            self.dialog = Some(Dialog::Message(format!("Could not create the project:\n{e}")));
            return;
        }
        // A starter library next to the app, when there is one, so the symbol picker is not empty.
        if let Some(lib) = starter_library()
            && let Ok(mut p) = wc_core::store::load(dir)
        {
            p.meta.libraries.push(wc_core::project::LibrarySource {
                name: "starter".into(),
                kind: "folder".into(),
                location: lib.display().to_string(),
                reference: None,
            });
            let _ = wc_core::store::save(dir, &p);
        }
        let git = wc_vcs::Git::new(dir);
        let _ = git.init().and_then(|()| git.commit("New project").map(|_| ()));
        let mut prefs = std::mem::take(&mut self.prefs);
        self.open_dir(dir, &mut prefs);
        self.prefs = prefs;
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

fn size_label(s: PageSize) -> &'static str {
    match s {
        PageSize::A4 => "A4 landscape",
        PageSize::A3 => "A3 landscape",
        PageSize::A2 => "A2 landscape",
    }
}

/// The starter symbol library: `library/` beside the executable or in the source tree.
fn starter_library() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    let candidates = [
        exe.as_ref().and_then(|e| e.parent()).map(|d| d.join("library")),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../library")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|d| d.join("symbols").is_dir())
        .and_then(|d| d.canonicalize().ok())
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

fn report_title(k: wc_report::Kind) -> &'static str {
    match k {
        wc_report::Kind::Parts => "Parts",
        wc_report::Kind::Devices => "Devices",
        wc_report::Kind::Wires => "Wires",
        wc_report::Kind::Nets => "Nets",
        wc_report::Kind::Terminals => "Terminals",
        wc_report::Kind::Cables => "Cables",
    }
}

/// A report as a striped grid that scrolls both ways.
fn report_table(ui: &mut egui::Ui, t: &wc_report::Table) {
    let theme = theme::Theme::of(ui.ctx());
    egui::ScrollArea::both()
        .max_height(ui.ctx().screen_rect().height() * 0.6)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if t.rows.is_empty() {
                ui.label(RichText::new("Nothing to list yet.").color(theme.faint));
                return;
            }
            egui::Grid::new(("report", t.title.as_str()))
                .striped(true)
                .spacing([14.0, 4.0])
                .show(ui, |ui| {
                    for c in &t.columns {
                        ui.label(RichText::new(c).strong());
                    }
                    ui.end_row();
                    for r in &t.rows {
                        for c in r {
                            ui.label(c);
                        }
                        ui.end_row();
                    }
                });
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
    // Report pages follow the schematic pages, as in a finished drawing set.
    let inputs = wc_report::Inputs {
        project: &open.project,
        symbols: &open.symbols,
        parts: &open.parts,
    };
    let options = wc_pdf::Options {
        reports: wc_report::Kind::ALL
            .into_iter()
            .map(|k| wc_report::build(k, &inputs))
            .filter(|t| !t.rows.is_empty())
            .collect(),
        ..Default::default()
    };
    let (bytes, report) = wc_pdf::export(&open.project, &open.symbols, &options);
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
            clipboard: &self.clipboard,
        };
        let mut actions = chrome::show(ctx, self.prefs.chrome, &mut self.chrome, &status);
        actions.extend(self.shortcuts(ctx));
        for a in actions {
            self.run(a);
        }
        if let Some(text) = self.copied_text.take() {
            ctx.copy_text(text);
        }

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| panels::status_bar(ui, self.open.as_ref(), &self.ed));

        let mut page_actions = Vec::new();
        if let Some(open) = self.open.as_mut() {
            egui::SidePanel::left("navigator")
                .default_width(220.0)
                .show(ctx, |ui| panels::navigator(ui, open, &mut self.ed, &mut page_actions));
            egui::SidePanel::right("inspector")
                .default_width(260.0)
                .max_width(420.0)
                .show(ctx, |ui| {
                    panels::library(ui, open, &mut self.ed, &mut self.panels);
                    ui.separator();
                    egui::ScrollArea::vertical().id_salt("properties").show(ui, |ui| {
                        ui.set_max_width(ui.available_width());
                        panels::properties(ui, open, &mut self.ed, &mut self.panels);
                    });
                });
            egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx, |ui| {
                canvas::show(ui, open, &mut self.ed, self.prefs.show_grid, self.prefs.snap);
            });
        } else {
            let mut start = None;
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(120.0);
                    ui.heading("WiringCraft");
                    ui.label("Open a project folder or create a new one to start.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.add_space(ui.available_width() / 2.0 - 110.0);
                        if ui.button("New project…").clicked() {
                            start = Some(Action::NewProject);
                        }
                        if ui.button("Open…").clicked() {
                            start = Some(Action::Open);
                        }
                    });
                });
            });
            if let Some(a) = start {
                self.run(a);
            }
        }
        for a in page_actions {
            self.run(a);
        }
        if let Some(at) = self.ed.ask_text.take() {
            self.dialog = Some(Dialog::Text(at, String::new(), false));
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

//! WiringCraft desktop application.
//!
//! Usage: `wiringcraft-app [project-folder]`. Without a folder the last project is reopened.

mod actions;
mod agent;
mod agent_panel;
mod applog;
mod canvas;
mod chrome;
mod edit;
mod feedback;
mod find_window;
mod icons;
mod import_symbols;
mod panel_view;
mod panels;
mod prefs;
mod screenshot;
mod share;
mod state;
mod symbol_editor;
mod template_form;
mod theme;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, Pos2, RichText, Stroke, Vec2};

use actions::Action;
use canvas::Editor;
use prefs::{Chrome, Prefs};
use state::{Open, Selection, Tool};
use wc_core::project::PageSize;

/// Title-block fields offered in the project properties, in this order.
use wc_core::project::TITLE_FIELDS;

enum Dialog {
    /// The folder field, and why the last Browse could not be used.
    OpenPath(prefs::FolderField, Option<String>),
    /// Save as without a save dialog: the folder for the copy, and why the last try failed.
    SaveAsPath(prefs::FolderField, Option<String>),
    NewProject {
        /// The folder the project folder goes in.
        location: prefs::FolderField,
        name: String,
        /// Why the last Browse could not be used.
        error: Option<String>,
    },
    ProjectProps {
        name: String,
        fields: Vec<(String, String)>,
        /// The project's locations as (path, description), edited in place.
        locations: Vec<(String, String)>,
        /// The locations as the dialog opened with them: when they are unchanged, OK leaves the
        /// project's own (an assistant command may have changed them meanwhile).
        loaded_locations: Vec<(String, String)>,
    },
    /// The cover page form: changes show on the page as they are typed; `since` is the undo
    /// mark from when it opened, so OK keeps them as one undo step and Cancel takes them back.
    Cover {
        name: String,
        fields: Vec<(String, String)>,
        since: usize,
        before: state::PropsSnapshot,
        /// The project's history count the buffer last matched; when it moves without the
        /// form (an undo, the assistant), the buffer is reloaded from the project.
        seen: u64,
    },
    /// The page template form: changes show on every page as they are made; `since` is the undo
    /// mark from when it opened, so OK keeps them as one undo step and Cancel takes them back.
    Template {
        draft: wc_core::template::PageTemplate,
        since: usize,
        before: wc_core::template::PageTemplate,
        /// The project's history count the draft last matched (see `Cover`).
        seen: u64,
    },
    PageProps {
        /// The page the dialog edits, by id: in a shared session someone else can remove an
        /// earlier page while it is open, which moves every later page's index.
        id: String,
        number: String,
        title: String,
        size: PageSize,
        overview: bool,
        /// The page's own title-block fields, key and value, as the form shows them; a blank
        /// value shows the project's as a hint and is not stored.
        fields: Vec<(String, String)>,
        /// The fields as the dialog opened: only the ones changed since are saved, so a value
        /// changed meanwhile (by the assistant, or someone in a shared session) is kept.
        loaded: Vec<(String, String)>,
        error: Option<String>,
    },
    DeletePage(usize),
    /// Project check findings.
    Check(Vec<wc_report::Finding>),
    /// Reports (parts, devices, wires, nets, terminals, cables, PLC I/O, contents, locations) as tables.
    Reports(wc_report::Kind),
    /// Write a new text at this point on the current page.
    Text(wc_core::geom::Point, String, bool),
    /// Pin a review comment at this point on the current page.
    Comment(wc_core::geom::Point, String, bool),
    /// The recorded versions of one page, newest first.
    PageHistory(String, Vec<wc_vcs::PageVersion>),
    Commit(String),
    NewBranch(String),
    History(Vec<wc_vcs::subway::Row>),
    Library(Vec<wc_library::ItemStatus>),
    Message(String),
    /// Help, Report a problem: the masked log to look at before anything is sent.
    Report {
        log: String,
        include: bool,
    },
    /// Name and description for a macro made from the copied selection.
    SaveMacro {
        clip: edit::Clipboard,
        name: String,
        description: String,
        error: Option<String>,
    },
    /// Macros to insert.
    Macros(Vec<state::MacroEntry>),
    /// Library, Import symbols: the library folders to import into and the one chosen.
    ImportSymbols {
        targets: Vec<import_symbols::ImportTarget>,
        chosen: usize,
        /// Overwrite symbols already in the library folder (off: they are kept and listed).
        replace: bool,
    },
    /// WAGO smartScript marker export: which markers, the text lines of each, the language of
    /// the header row, and optionally one terminal strip.
    WagoMarking {
        /// Terminals, wires, devices, cables (the order of [`wc_report::MarkerKind::ALL`]).
        kinds: [bool; 4],
        /// The first line, and up to two more.
        lines: [Option<wc_report::LineField>; 3],
        language: wc_report::Language,
        strip: Option<String>,
        /// The project's terminal strips, for the strip menu.
        strips: Vec<String>,
        /// Between the numbers of a multi-level terminal's levels on its marker.
        level_separator: String,
    },
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
    /// The interface theme currently applied to the context.
    applied_theme: theme::Mode,
    /// The AI assistant's conversation and panel.
    assistant: agent::Assistant,
    /// Web pages to open in the browser on the next frame.
    open_url: Vec<String>,
    /// The last status message and message box written to the log, so each is logged once.
    logged: (String, String),
    /// The shared work session, when one is running, and its windows.
    share: share::Share,
    /// The symbol editor window, while it is open.
    symbol_editor: Option<symbol_editor::SymbolEditor>,
    /// The find and replace window, while it is open.
    find: Option<find_window::FindState>,
    /// A picker an action asked for, shown once the frame's actions have run.
    pending: Option<Pending>,
}

/// A file or folder picker an action needs before it can be carried out. It is shown after the
/// frame's actions have run, so an action the assistant runs returns at once and the picker
/// waits for the person instead.
enum Pending {
    Open,
    SaveAs,
    Export(Export),
    Marking(wc_report::MarkingOptions),
    ReportCsv(wc_report::Kind),
    /// The WAGO configurator's article list to read back.
    WagoList,
}

/// The whole-project exports that ask where to go.
#[derive(Debug, Clone, Copy)]
enum Export {
    Pdf,
    Drawings,
    Aml,
    WagoConfigurator,
}

struct StatusView<'a> {
    open: Option<&'a Open>,
    ed: &'a Editor,
    prefs: &'a Prefs,
    clipboard: &'a edit::Clipboard,
    sharing: bool,
    /// Undo and Redo in a shared session: whether this person has something to take back.
    share_undo: bool,
    share_redo: bool,
}

impl chrome::Status for StatusView<'_> {
    fn active(&self, a: Action) -> bool {
        match a {
            Action::Select => self.ed.tool == Tool::Select,
            Action::PlaceSymbol => self.ed.tool == Tool::Place,
            Action::DrawWire => self.ed.tool == Tool::Wire,
            Action::TextTool => self.ed.tool == Tool::Text,
            Action::CommentTool => self.ed.tool == Tool::Comment,
            Action::LineTool => self.ed.tool == Tool::Line,
            Action::RectTool => self.ed.tool == Tool::Rect,
            Action::CircleTool => self.ed.tool == Tool::Circle,
            Action::ArcTool => self.ed.tool == Tool::Arc,
            Action::PolylineTool => self.ed.tool == Tool::Polyline,
            Action::ToggleGrid => self.prefs.show_grid,
            Action::ToggleAutoWire => self.prefs.auto_wire,
            Action::ToggleChrome => self.prefs.chrome == Chrome::Ribbon,
            Action::ToggleTheme => self.prefs.theme == theme::Mode::Dark,
            Action::Assistant => self.prefs.show_assistant,
            _ => false,
        }
    }

    fn enabled(&self, a: Action) -> bool {
        let has = self.open.is_some();
        if self.open.is_some_and(|o| held_for(a, self.ed, &o.project).is_some()) {
            return false;
        }
        match a {
            Action::NewProject
            | Action::Open
            | Action::ToggleChrome
            | Action::ToggleTheme
            | Action::ToggleGrid
            | Action::ToggleAutoWire
            | Action::Assistant
            | Action::ReportProblem
            | Action::RequestFunction
            | Action::ReportMissing
            | Action::JoinSession => true,
            Action::ShareSession => has || self.sharing,
            // In a session, undo takes back this person's own changes through the host.
            Action::Undo if self.sharing => self.share_undo,
            Action::Redo if self.sharing => self.share_redo,
            Action::DeletePage => self.open.is_some_and(|o| o.project.pages.len() > 1),
            Action::ArrangePanel | Action::DeletePanel => has && self.ed.panel.is_some(),
            Action::PageUp => has && self.ed.page > 0,
            Action::PageDown => self.open.is_some_and(|o| self.ed.page + 1 < o.project.pages.len()),
            Action::Save => self.open.is_some_and(|o| o.dirty),
            Action::Undo => self.open.is_some_and(Open::can_undo),
            Action::Redo => self.open.is_some_and(Open::can_redo),
            Action::Delete if self.ed.panel.is_some() => has && self.ed.panel_pick.is_some(),
            Action::Delete | Action::Copy | Action::Cut | Action::SaveMacro => has && !self.ed.selection.is_empty(),
            Action::Rotate | Action::Mirror => {
                has && self.ed.selection.iter().any(|s| matches!(s, Selection::Symbol(_)))
            }
            Action::Paste => has && !self.clipboard.is_empty(),
            _ => has,
        }
    }
}

impl App {
    /// Keep the open project in step with the shared session, at the end of the frame so this
    /// frame's change goes out. Nothing goes out while a button is held or the live cover form
    /// is open, so one action is one change.
    fn sync_session(&mut self, ctx: &egui::Context) {
        if !self.share.active() {
            self.ed.held_by.clear();
            if let Some(o) = self.open.as_mut() {
                o.held_by_others.clear();
            }
            return;
        }
        // While an assistant command runs, its files are its own: nothing is sent, and a joined
        // copy waits, so the dirty project is not saved over the command's changes.
        let busy = ctx.input(|i| i.pointer.any_down())
            || matches!(self.dialog, Some(Dialog::Cover { .. } | Dialog::Template { .. }))
            || self.assistant.command_running();
        self.share.set_command_running(self.assistant.command_running());
        self.sync_session_now(busy);
        // Others' changes arrive without any input here.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    /// The work of `sync_session`, also used by the assistant's undo so the changes it made
    /// earlier in the same answer are in the session's history first.
    fn sync_session_now(&mut self, busy: bool) {
        if !self.share.active() {
            return;
        }
        let page = self.open.as_ref().and_then(|o| match self.ed.panel {
            Some(_) => None,
            None => o.project.pages.get(self.ed.page).map(|p| p.id.clone()),
        });
        // The page under the canvas, also under an open panel: it is found again after the
        // sync, and so is what was selected on it.
        let mut shown = self
            .open
            .as_ref()
            .and_then(|o| o.project.pages.get(self.ed.page))
            .map(|p| p.id.clone());
        // The shown page's graphics, so a selection follows its items when a merge moves them.
        let graphics_before = self
            .open
            .as_ref()
            .and_then(|o| o.project.pages.get(self.ed.page))
            .map(|p| (p.id.clone(), p.graphics.clone()));
        if let Some(joined) = self.share.frame(self.open.as_mut(), page, busy) {
            // The joined copy replaces the open project: keep its unsaved work first.
            if let Some(o) = self.open.as_mut()
                && o.dirty
                && let Err(e) = o.save()
            {
                self.share.stop();
                self.dialog = Some(Dialog::Message(format!(
                    "Left the session: the open project could not be saved before switching to the shared copy:\n{e}"
                )));
                return;
            }
            self.ed = Editor::default();
            self.panels.reset();
            self.dialog = None;
            self.prefs.remember(joined.dir.clone());
            self.prefs.save();
            self.open = Some(joined);
            // The id belonged to the project left behind.
            shown = None;
        }
        // Others may have added or removed pages before this one: keep showing the same page.
        if let (Some(id), Some(o)) = (&shown, &self.open)
            && let Some(i) = o.project.pages.iter().position(|p| &p.id == id)
        {
            self.ed.page = i;
        }
        // A page shown in the navigator may have been removed by someone else.
        if let Some(o) = &self.open
            && self.ed.page >= o.project.pages.len()
        {
            self.ed.page = o.project.pages.len().saturating_sub(1);
        }
        if let Some((id, old)) = &graphics_before {
            match self.open.as_ref().and_then(|o| o.project.pages.get(self.ed.page)) {
                Some(p) if &p.id == id => edit::remap_graphics(&mut self.ed.selection, old, &p.graphics),
                // The page was removed: what was selected on it is not on the page shown now.
                _ => self.ed.selection.clear(),
            }
        }
        self.ed.held_by = self.share.held_by_others();
        if let Some(o) = self.open.as_mut() {
            o.held_by_others = self.ed.held_by.clone();
        }
        if let Some(note) = self.share.take_notes().pop() {
            applog::info(&format!("shared session: {note}"));
            self.ed.message = note;
        }
    }

    fn new(cc: &eframe::CreationContext<'_>, dir: Option<PathBuf>) -> Self {
        let mut prefs = Prefs::load();
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, prefs.theme);
        let mut app = Self {
            prefs: Prefs::default(),
            open: None,
            ed: Editor::default(),
            chrome: Default::default(),
            panels: Default::default(),
            dialog: None,
            clipboard: edit::Clipboard::default(),
            copied_text: None,
            applied_theme: prefs.theme,
            assistant: agent::Assistant::default(),
            open_url: Vec::new(),
            logged: Default::default(),
            share: Default::default(),
            symbol_editor: None,
            find: None,
            pending: None,
        };
        let dir = dir.or_else(|| prefs.recent.first().cloned());
        if let Some(d) = dir {
            app.open_dir(&d, &mut prefs);
        }
        app.prefs = prefs;
        app
    }

    fn open_dir(&mut self, dir: &std::path::Path, prefs: &mut Prefs) {
        let dir = &match prefs::absolute(dir) {
            Ok(d) => d,
            Err(e) => {
                self.dialog = Some(Dialog::Message(format!("Could not open the project: {e}")));
                return;
            }
        };
        match Open::load(dir) {
            Ok(o) => {
                self.ed = Editor::default();
                self.panels.reset();
                self.ed.message = format!("Opened {}", dir.display());
                self.open = Some(o);
                prefs.remember(dir.to_path_buf());
                prefs.save();
            }
            Err(e) => {
                applog::warn(&format!("could not open {}: {e}", dir.display()));
                self.dialog = Some(Dialog::Message(format!("Could not open {}:\n{e}", dir.display())));
            }
        }
    }

    /// Save the open project as a copy in `d` and work on in the copy; the error says why not.
    fn save_as_to(&mut self, d: &Path) -> Result<(), String> {
        let Some(open) = self.open.as_mut() else {
            return Err("No project is open.".into());
        };
        let (d, links) = open.save_as(d).map_err(|e| format!("Save as failed:\n{e}"))?;
        let mut m = format!("Saved as {}; you are working in the copy now.", d.display());
        if links > 0 {
            m.push_str(&format!(" {links} link(s) in the project folder were not copied."));
        }
        self.ed.message = m;
        self.prefs.remember(d);
        self.prefs.save();
        Ok(())
    }

    /// Show the picker an action asked for, then carry the action out with what was picked.
    fn ask(&mut self, p: Pending) {
        if let Pending::Open = p {
            let start = self
                .open
                .as_ref()
                .map(|o| o.dir.display().to_string())
                .unwrap_or_default();
            match pick_folder("Open project", &start) {
                Picked::Path(d) if d.join(wc_core::store::PROJECT_FILE).is_file() => {
                    let mut prefs = std::mem::take(&mut self.prefs);
                    self.open_dir(&d, &mut prefs);
                    self.prefs = prefs;
                }
                Picked::Path(d) => {
                    let why = format!(
                        "{} has no {}: pick the project's own folder.",
                        d.display(),
                        wc_core::store::PROJECT_FILE
                    );
                    let field = prefs::FolderField::typed(d.display().to_string());
                    self.dialog = Some(Dialog::OpenPath(field, Some(why)));
                }
                // Without a picker the folder can still be typed, or picked from the recent ones.
                Picked::Failed(e) => {
                    self.dialog = Some(Dialog::OpenPath(prefs::FolderField::typed(start), Some(e)));
                }
                Picked::Cancelled => {}
            }
            return;
        }
        let Some(open) = self.open.as_mut() else { return };
        match p {
            Pending::Open => {}
            Pending::SaveAs => {
                let parent = open
                    .dir
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(prefs::projects_dir);
                let name = open
                    .dir
                    .file_name()
                    .map(|n| format!("{} copy", n.to_string_lossy()))
                    .unwrap_or_default();
                match pick_save(
                    "Save project as: the name is the new project folder",
                    &parent,
                    &name,
                    None,
                ) {
                    Picked::Path(d) => {
                        if let Err(e) = self.save_as_to(&d) {
                            self.dialog = Some(Dialog::Message(e));
                        }
                    }
                    // Without a save dialog the folder can be typed.
                    Picked::Failed(e) => {
                        let field = prefs::FolderField::typed(parent.join(&name).display().to_string());
                        self.dialog = Some(Dialog::SaveAsPath(
                            field,
                            Some(format!("{e} Type the folder for the copy.")),
                        ));
                    }
                    Picked::Cancelled => {}
                }
            }
            Pending::Export(Export::Drawings) => {
                let default = open.dir.join("export").join("drawings");
                let (dir, note) = match pick_folder("Export the pages as drawings into", &default.display().to_string())
                {
                    Picked::Path(d) => (d, String::new()),
                    Picked::Failed(e) => (default, format!("{e} So they went to the usual place.\n\n")),
                    Picked::Cancelled => return,
                };
                self.dialog = Some(Dialog::Message(note + &export_drawings_to(open, &dir)));
            }
            Pending::Export(Export::WagoConfigurator) if nothing_for_configurator(open) => {
                self.dialog = Some(Dialog::Message(export_wago_configurator_to(
                    open,
                    &open.dir.join("export"),
                )));
            }
            Pending::Marking(options) if !marking_strip_listed(open, &options) => {
                self.dialog = Some(Dialog::Message(export_wago_marking_to(
                    open,
                    &options,
                    &open.dir.join("export"),
                )));
            }
            Pending::Export(kind) => {
                let (title, default, filter): (&str, PathBuf, (&str, &[&str])) = match kind {
                    Export::Pdf => ("Export PDF", export_file(open, "pdf"), ("PDF", &["pdf"])),
                    Export::Aml => (
                        "Export AutomationML",
                        export_file(open, "aml"),
                        ("AutomationML", &["aml"]),
                    ),
                    _ => (
                        "Export for the WAGO configurator",
                        open.dir.join("export").join("wago-configurator.csv"),
                        ("CSV list", &["csv"]),
                    ),
                };
                let Some((path, note)) = export_target(title, &default, filter) else {
                    return;
                };
                let msg = match kind {
                    Export::Pdf => export_pdf_to(open, &path),
                    Export::Aml => export_aml_to(open, &path),
                    _ => export_wago_configurator_to(open, &path),
                };
                self.dialog = Some(Dialog::Message(note + &msg));
            }
            Pending::Marking(options) => {
                let default = wago_marking_file(open, &options);
                let Some((path, note)) = export_target("Export WAGO markers", &default, ("CSV list", &["csv"])) else {
                    return;
                };
                self.dialog = Some(Dialog::Message(note + &export_wago_marking_to(open, &options, &path)));
            }
            Pending::WagoList => {
                let file = match pick_open(
                    "WAGO configurator list with articles",
                    &open.dir.join("export"),
                    ("CSV list", &["csv", "txt"]),
                ) {
                    Picked::Path(f) => f,
                    Picked::Failed(e) => {
                        self.dialog = Some(Dialog::Message(e));
                        return;
                    }
                    Picked::Cancelled => return,
                };
                let msg = match std::fs::read(&file) {
                    Ok(bytes) => open
                        .import_wago_configured(&bytes)
                        .unwrap_or_else(|e| format!("Nothing imported: {e}")),
                    Err(e) => format!("{}: {e}", file.display()),
                };
                self.dialog = Some(Dialog::Message(msg));
            }
            Pending::ReportCsv(kind) => {
                let default = open.dir.join("export").join(format!("{}.csv", kind.name()));
                let Some((path, note)) = export_target("Save report as CSV", &default, ("CSV", &["csv"])) else {
                    return;
                };
                let inputs = wc_report::Inputs {
                    project: &open.project,
                    symbols: &open.symbols,
                    parts: &open.parts,
                };
                let table = wc_report::build(kind, &inputs);
                self.ed.message = match write_creating(&path, table.to_csv()) {
                    Ok(()) => format!("{}Saved {}", note.replace("\n\n", " "), path.display()),
                    Err(e) => format!("Could not save {}: {e}", path.display()),
                };
            }
        }
    }

    fn run(&mut self, a: Action) {
        let page = self.ed.page;
        applog::info(&format!("command {a:?}"));
        if let Some(name) = self.open.as_ref().and_then(|o| held_for(a, &self.ed, &o.project)) {
            self.ed.message = format!("{name} has this page open; it is read-only here until they leave it.");
            return;
        }
        if a == Action::ReportProblem {
            self.dialog = Some(Dialog::Report {
                log: applog::tail(REPORT_LOG_BYTES),
                include: false,
            });
            return;
        }
        let kind = match a {
            Action::RequestFunction => Some(feedback::Kind::Function),
            Action::ReportMissing => Some(feedback::Kind::Missing),
            _ => None,
        };
        if let Some(kind) = kind {
            self.open_url
                .push(feedback::form_url(kind, &feedback::Request::default()));
            self.ed.message = "Opened the form on GitHub in your browser; what you send goes on the todo list.".into();
            return;
        }
        match a {
            Action::ShareSession => {
                self.share.show_share();
                return;
            }
            Action::Find => {
                if self.open.is_some() {
                    self.find.get_or_insert_with(Default::default);
                }
                return;
            }
            Action::JoinSession => {
                self.share.show_join();
                return;
            }
            _ => {}
        }
        match a {
            Action::Open => self.pending = Some(Pending::Open),
            Action::NewProject => {
                let location = self
                    .open
                    .as_ref()
                    .and_then(|o| o.dir.parent())
                    .map(Path::to_path_buf)
                    .unwrap_or_else(prefs::projects_dir);
                // A default location that cannot be shown as text is not guessed at: the field
                // starts empty (Create stays off) and says why.
                let mut field = prefs::FolderField::default();
                let error = field.pick(location).err();
                self.dialog = Some(Dialog::NewProject {
                    location: field,
                    name: String::new(),
                    error,
                });
            }
            Action::ToggleChrome => {
                self.prefs.chrome = match self.prefs.chrome {
                    Chrome::Classic => Chrome::Ribbon,
                    Chrome::Ribbon => Chrome::Classic,
                };
                self.prefs.save();
            }
            Action::ToggleTheme => {
                self.prefs.theme = match self.prefs.theme {
                    theme::Mode::Light => theme::Mode::Dark,
                    theme::Mode::Dark => theme::Mode::Light,
                };
                self.prefs.save();
            }
            Action::ToggleGrid => {
                self.prefs.show_grid = !self.prefs.show_grid;
                self.prefs.save();
            }
            Action::ToggleAutoWire => {
                self.prefs.auto_wire = !self.prefs.auto_wire;
                self.prefs.save();
                self.ed.message = if self.prefs.auto_wire {
                    "Auto-wiring is on: pins that line up get a wire (hold Alt to skip once).".into()
                } else {
                    "Auto-wiring is off.".into()
                };
            }
            Action::Assistant => {
                self.prefs.show_assistant = !self.prefs.show_assistant;
                self.prefs.save();
            }
            Action::Select => {
                self.ed.tool = Tool::Select;
                self.ed.wire = None;
                self.ed.shape_points.clear();
            }
            Action::PlaceSymbol => {
                self.ed.tool = Tool::Place;
                self.ed.wire = None;
                self.ed.shape_points.clear();
            }
            Action::DrawWire => {
                self.ed.tool = Tool::Wire;
                self.ed.shape_points.clear();
            }
            Action::TextTool
            | Action::LineTool
            | Action::RectTool
            | Action::CircleTool
            | Action::ArcTool
            | Action::PolylineTool
            | Action::CommentTool => {
                self.ed.tool = match a {
                    Action::TextTool => Tool::Text,
                    Action::CommentTool => Tool::Comment,
                    Action::LineTool => Tool::Line,
                    Action::CircleTool => Tool::Circle,
                    Action::ArcTool => Tool::Arc,
                    Action::PolylineTool => Tool::Polyline,
                    _ => Tool::Rect,
                };
                self.ed.wire = None;
                self.ed.shape_points.clear();
                self.ed.message = match a {
                    Action::TextTool => "Click where the text goes.",
                    Action::CommentTool => "Click where the comment goes.",
                    Action::CircleTool => "Click the centre of the circle.",
                    Action::ArcTool => "Click the centre of the arc.",
                    _ => "Click the first point.",
                }
                .into();
            }
            Action::ZoomIn => self.zoom(1.25),
            Action::ZoomOut => self.zoom(0.8),
            Action::ZoomFit => self.ed.view.fit = true,
            // Undoing in a shared session takes back only this person's own change: the whole
            // project is not swapped, so other people's work stays.
            Action::Undo | Action::Redo if self.share.active() => {
                // The host's undo writes the project folder, and a guest's sync would send a
                // command's half-written work: neither while a command owns the files.
                if self.assistant.command_running() {
                    self.ed.message = "Undo and Redo wait until the assistant's command has finished.".into();
                    return;
                }
                // Changes not synced yet (an assistant's, made with no frame in between) go into
                // the session's history first, so Undo takes back the latest one.
                self.sync_session_now(false);
                let r = if a == Action::Undo {
                    self.share.undo()
                } else {
                    self.share.redo()
                };
                // The host's refusal comes back in the session's notes, which the status line
                // shows; one made here is shown now.
                if let Err(e) = r {
                    self.ed.message = e;
                }
                self.ed.selection.clear();
                return;
            }
            _ => {}
        }
        let Some(open) = self.open.as_mut() else { return };
        match a {
            Action::Save if self.assistant.command_running() => {
                self.ed.message = "Saving waits until the assistant's command has finished.".into();
            }
            Action::Save => match open.save() {
                Ok(n) => self.ed.message = format!("Saved ({n} files changed)."),
                Err(e) => self.dialog = Some(Dialog::Message(format!("Saving failed:\n{e}"))),
            },
            Action::SaveAs if self.share.active() => {
                self.ed.message = "Save as is not available in a shared session: it would end the session. \
                                   Stop sharing (or leave) first."
                    .into();
            }
            Action::SaveAs if self.assistant.command_running() => {
                self.ed.message = "Saving waits until the assistant's command has finished.".into();
            }
            Action::SaveAs => self.pending = Some(Pending::SaveAs),
            Action::Undo | Action::Redo => {
                let done = if a == Action::Undo { open.undo() } else { open.redo() };
                if done {
                    self.ed.selection.clear();
                    if let Some(w) = open.take_step_warning() {
                        self.ed.message = format!("Done, but {w}.");
                    }
                } else if let Some(e) = open.take_undo_error() {
                    self.dialog = Some(Dialog::Message(format!("Undo or redo was not done:\n{e}")));
                }
            }
            Action::Delete if self.ed.panel.is_some() => {
                if let (Some(pi), Some(item)) = (self.ed.panel, self.ed.panel_pick.take()) {
                    open.checkpoint();
                    if let Some(panel) = open.project.panels.get_mut(pi)
                        && item < panel.items.len()
                    {
                        panel.items.remove(item);
                    }
                }
            }
            Action::NewPanel => {
                open.checkpoint();
                let n = (1..)
                    .find(|n| !open.project.panels.iter().any(|p| p.id == format!("panel{n}")))
                    .unwrap_or(1);
                let panel = wc_core::panel::Panel::with_rails(format!("panel{n}"), format!("Panel {n}"), 600.0, 800.0);
                open.project.panels.push(panel);
                open.project.panels.sort_by(|a, b| a.id.cmp(&b.id));
                self.ed.panel = open.project.panels.iter().position(|p| p.id == format!("panel{n}"));
                self.ed.panel_pick = None;
                self.ed.view.fit = true;
                self.ed.message = "New panel layout. Page menu → Arrange devices puts the devices on its rails.".into();
            }
            Action::ArrangePanel => {
                if let Some(pi) = self.ed.panel
                    && pi < open.project.panels.len()
                {
                    open.checkpoint();
                    let mut panels = std::mem::take(&mut open.project.panels);
                    let done = panels
                        .get_mut(pi)
                        .map(|p| wc_core::panel::arrange(p, &open.project, &open.parts, None))
                        .unwrap_or_default();
                    open.project.panels = panels;
                    let mut m = format!("Placed {} device(s) on the rails.", done.placed.len());
                    if !done.estimated.is_empty() {
                        m.push_str(&format!(
                            "\n\nNo Width/Height on the part, drawn 18 x 90 mm:\n{}",
                            done.estimated.join(", ")
                        ));
                    }
                    if !done.did_not_fit.is_empty() {
                        m.push_str(&format!(
                            "\n\nNo room left on the rails for:\n{}",
                            done.did_not_fit.join(", ")
                        ));
                    }
                    self.dialog = Some(Dialog::Message(m));
                }
            }
            Action::DeletePanel => {
                if let Some(pi) = self.ed.panel.take()
                    && pi < open.project.panels.len()
                {
                    open.checkpoint();
                    open.project.panels.remove(pi);
                    self.ed.panel_pick = None;
                    self.ed.view.fit = true;
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
                        id: p.id.clone(),
                        number: p.number.clone(),
                        title: p.title.clone(),
                        size: p.size,
                        overview: p.overview,
                        fields: own_fields(p),
                        loaded: own_fields(p),
                        error: None,
                    });
                }
            }
            Action::DeletePage => self.dialog = Some(Dialog::DeletePage(page)),
            Action::ProjectProps => {
                let locations: Vec<(String, String)> = open
                    .project
                    .meta
                    .locations
                    .iter()
                    .map(|l| (l.path.clone(), l.description.clone()))
                    .collect();
                self.dialog = Some(Dialog::ProjectProps {
                    name: open.project.meta.name.clone(),
                    fields: form_fields(&open.project.meta.title_block),
                    loaded_locations: locations.clone(),
                    locations,
                });
            }
            Action::CoverPage => {
                // Compare page ids: a new cover can take the index the shown page had.
                let shown = open.project.pages.get(self.ed.page).map(|p| p.id.clone());
                let i = match open.cover_page() {
                    Ok(i) => i,
                    Err(e) => {
                        self.dialog = Some(Dialog::Message(e));
                        return;
                    }
                };
                let cover = open.project.pages.get(i).map(|p| p.id.clone());
                if shown != cover || self.ed.panel.is_some() {
                    self.ed.panel = None;
                    self.ed.selection.clear();
                    self.ed.wire = None;
                }
                self.ed.page = i;
                self.ed.view.fit = true;
                self.dialog = Some(Dialog::Cover {
                    name: open.project.meta.name.clone(),
                    fields: form_fields(&open.project.meta.title_block),
                    since: open.start_live_form(),
                    before: open.props_snapshot(),
                    seen: open.history_count(),
                });
            }
            Action::PageTemplate => {
                let template = open.project.meta.template.clone();
                self.dialog = Some(Dialog::Template {
                    draft: template.clone(),
                    since: open.start_live_form(),
                    before: template,
                    seen: open.history_count(),
                });
            }
            Action::ExportPdf => self.pending = Some(Pending::Export(Export::Pdf)),
            Action::SaveMacro => {
                let clip = open.copy(page, &self.ed.selection);
                if !clip.is_empty() {
                    self.dialog = Some(Dialog::SaveMacro {
                        clip,
                        name: String::new(),
                        description: String::new(),
                        error: None,
                    });
                }
            }
            Action::InsertMacro => self.dialog = Some(Dialog::Macros(open.macros())),
            Action::ExportDrawings => self.pending = Some(Pending::Export(Export::Drawings)),
            Action::ExportAml => self.pending = Some(Pending::Export(Export::Aml)),
            Action::ExportWagoMarking => {
                let strip = self.ed.marking_strip.take();
                self.dialog = Some(wago_marking_dialog(open, strip));
            }
            Action::ExportWagoConfigurator => self.pending = Some(Pending::Export(Export::WagoConfigurator)),
            Action::ImportWagoConfigurator => self.pending = Some(Pending::WagoList),
            Action::NumberWires => {
                let done = open.number_wires(wc_core::numbering::Rule::PageColumn);
                self.dialog = Some(Dialog::Message(done.report()));
            }
            Action::Reports => self.dialog = Some(Dialog::Reports(wc_report::Kind::Parts)),
            Action::Check => {
                let found = run_check(open);
                let errors = found
                    .iter()
                    .filter(|f| f.severity == wc_report::Severity::Error)
                    .count();
                self.ed.message = format!("Project check: {errors} errors, {} findings in all.", found.len());
                self.dialog = Some(Dialog::Check(found));
            }
            Action::NewSymbol | Action::EditSymbol => {
                let (lib, lib_error) = match wc_library::Library::open(&open.dir, &open.project) {
                    Ok(l) => (Some(l), None),
                    Err(e) => (None, Some(format!("The project's library could not be opened: {e}"))),
                };
                let targets = lib.as_ref().map(|l| folder_targets(l, open)).unwrap_or_default();
                let selected = self.ed.selection.iter().find_map(|s| match s {
                    Selection::Symbol(item) => open
                        .project
                        .pages
                        .get(self.ed.page)
                        .and_then(|pg| pg.symbols.iter().find(|p| &p.id == item))
                        .map(|p| p.symbol.clone()),
                    _ => None,
                });
                let mut ed = match (a, selected, lib.as_ref()) {
                    (Action::EditSymbol, Some(id), Some(lib)) => match lib.symbol(&id) {
                        Ok((s, source)) => symbol_editor::SymbolEditor::edit(s, &source, targets),
                        Err(err) => {
                            let mut e = symbol_editor::SymbolEditor::new(targets);
                            e.message = format!("Symbol {id} could not be opened from the library: {err}");
                            e
                        }
                    },
                    _ => symbol_editor::SymbolEditor::new(targets),
                };
                ed.owner = Some(open.dir.clone());
                if let Some(e) = lib_error {
                    ed.message = e;
                }
                if a == Action::EditSymbol {
                    ed.choices = open.library.clone();
                    if ed.opened_as.is_none() && ed.message.is_empty() {
                        ed.message = "Pick the symbol to change under \"Open a library symbol\".".into();
                    }
                }
                self.symbol_editor = Some(ed);
            }
            Action::ImportSymbols => match wc_library::Library::open(&open.dir, &open.project) {
                Ok(lib) => {
                    self.dialog = Some(Dialog::ImportSymbols {
                        targets: import_symbols::targets(
                            &open.dir,
                            folder_targets(&lib, open),
                            &open
                                .project
                                .meta
                                .libraries
                                .iter()
                                .map(|l| l.name.clone())
                                .collect::<Vec<_>>(),
                        ),
                        chosen: 0,
                        replace: false,
                    });
                }
                Err(e) => self.dialog = Some(Dialog::Message(format!("The library could not be opened:\n{e}"))),
            },
            Action::LibraryStatus | Action::LibraryUpdate => {
                match wc_library::Library::open(&open.dir, &open.project) {
                    Ok(lib) => {
                        if a == Action::LibraryUpdate {
                            match open.update_library(&lib, None) {
                                Ok(done) => self.ed.message = format!("Updated {} library items.", done.len()),
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
            Action::PageHistory => {
                if let Some(pg) = open.project.pages.get(page) {
                    let (id, number) = (pg.id.clone(), pg.number.clone());
                    match wc_vcs::page_history(&wc_vcs::Git::new(&open.dir), &id, 100) {
                        Ok(v) => self.dialog = Some(Dialog::PageHistory(number, v)),
                        Err(e) => self.dialog = Some(Dialog::Message(format!("No history yet:\n{e}"))),
                    }
                }
            }
            _ => {}
        }
    }

    fn zoom(&mut self, f: f32) {
        self.ed.view.zoom = (self.ed.view.zoom * f).clamp(0.3, 60.0);
    }

    fn shortcuts(&mut self, ctx: &egui::Context) -> Vec<Action> {
        // An open dialog takes the keyboard: a shortcut would change the project under it (the
        // cover form's Cancel takes back only what the form changed).
        let dialog_open = self.dialog.is_some();
        let mut out = ctrl_shortcuts(ctx, dialog_open);
        if dialog_open {
            return out;
        }
        if ctx.wants_keyboard_input() {
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
                (Key::C, Action::CommentTool),
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
            if i.key_pressed(Key::Enter) && i.modifiers.is_none() && self.ed.tool == Tool::Polyline {
                self.ed.finish_shape = true;
            }
            if i.key_pressed(Key::Escape) {
                if !self.ed.shape_points.is_empty() {
                    self.ed.shape_points.clear();
                    self.ed.message = canvas::shape_hint(self.ed.tool, 0).into();
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
            Dialog::OpenPath(..) => "Open project",
            Dialog::SaveAsPath(..) => "Save project as",
            Dialog::NewProject { .. } => "New project",
            Dialog::ProjectProps { .. } => "Project properties",
            Dialog::Cover { .. } => "Cover page",
            Dialog::Template { .. } => "Page template",
            Dialog::PageProps { .. } => "Page properties",
            Dialog::DeletePage(_) => "Delete page",
            Dialog::Text(..) => "Text",
            Dialog::Comment(..) => "Comment",
            Dialog::PageHistory(..) => "Page history",
            Dialog::Reports(_) => "Reports",
            Dialog::Check(_) => "Project check",
            Dialog::Commit(_) => "Record a version",
            Dialog::NewBranch(_) => "New branch",
            Dialog::History(_) => "History",
            Dialog::Library(_) => "Library status",
            Dialog::Message(_) => "WiringCraft",
            Dialog::SaveMacro { .. } => "Save as macro",
            Dialog::Macros(_) => "Insert macro",
            Dialog::ImportSymbols { .. } => "Import symbols",
            Dialog::Report { .. } => "Report a problem",
            Dialog::WagoMarking { .. } => "WAGO marking",
        };
        let wide = matches!(dialog, Dialog::Reports(_) | Dialog::Check(_));
        let screen = ctx.screen_rect();
        egui::Window::new(title)
            .collapsible(false)
            .resizable(true)
            .min_width(if wide { (screen.width() * 0.75).min(1100.0) } else { 0.0 })
            // The cover form sits at the side, so the page it changes stays in view.
            .anchor(
                if matches!(dialog, Dialog::Cover { .. } | Dialog::Template { .. }) {
                    egui::Align2::RIGHT_TOP
                } else {
                    egui::Align2::CENTER_CENTER
                },
                if matches!(dialog, Dialog::Cover { .. } | Dialog::Template { .. }) {
                    Vec2::new(-16.0, 96.0)
                } else {
                    Vec2::ZERO
                },
            )
            .show(ctx, |ui| match &mut dialog {
                Dialog::SaveMacro {
                    clip,
                    name,
                    description,
                    error,
                } => {
                    ui.label(format!(
                        "{} symbols, {} wires and {} drawings. Saved in the project's macros folder; copy the \
                         file to a library's macros folder to share it.",
                        clip.symbols.len(),
                        clip.wires.len(),
                        clip.graphics.len()
                    ));
                    egui::Grid::new("macro")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Name");
                            ui.add(
                                egui::TextEdit::singleline(name)
                                    .hint_text("Motor starter DOL")
                                    .desired_width(320.0),
                            );
                            ui.end_row();
                            ui.label("Description");
                            ui.add(egui::TextEdit::singleline(description).desired_width(320.0));
                            ui.end_row();
                        });
                    if let Some(e) = error {
                        ui.colored_label(theme::Theme::of(ui.ctx()).error, e.as_str());
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!name.trim().is_empty(), egui::Button::new("Save"))
                            .clicked()
                            && let Some(open) = &self.open
                        {
                            match open.save_macro(clip, name, description) {
                                Ok(path) => {
                                    self.ed.message = format!("Saved macro {}", path.display());
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
                Dialog::Macros(list) => {
                    if list.is_empty() {
                        ui.label(
                            "No macros yet. Select symbols and wires, then Insert > Save as macro. Library \
                             sources can hold macros in a macros folder too.",
                        );
                    }
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        egui::Grid::new("macros").striped(true).num_columns(3).show(ui, |ui| {
                            for m in list.iter() {
                                ui.vertical(|ui| {
                                    ui.set_min_width(320.0);
                                    ui.strong(m.name.as_str());
                                    if let Some(e) = &m.error {
                                        ui.colored_label(theme::Theme::of(ui.ctx()).error, e.as_str());
                                    } else if !m.description.is_empty() {
                                        ui.label(m.description.as_str());
                                    }
                                });
                                ui.label(m.source.as_str());
                                if ui.add_enabled(m.error.is_none(), egui::Button::new("Insert")).clicked() {
                                    let path = m.path.clone();
                                    let name = m.name.clone();
                                    then = Some(Box::new(move |app: &mut App| app.insert_macro(&path, &name)));
                                    keep = false;
                                }
                                ui.end_row();
                            }
                        });
                    });
                    if ui.button("Close").clicked() {
                        keep = false;
                    }
                }
                Dialog::Report { log, include } => {
                    ui.label(
                        "This is WiringCraft's log of what happened recently. Home folders, keys, tokens, \
                         email addresses and device tags are already hidden, and no drawing is in it. Messages \
                         WiringCraft showed are in it and can name a page, so look it over first. Nothing is \
                         sent unless you choose to.",
                    );
                    if let Some(e) = applog::error() {
                        ui.colored_label(
                            theme::Theme::of(ui.ctx()).error,
                            format!("The log could not be written: {e}"),
                        );
                    }
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        let shown = if log.is_empty() {
                            "(The log is empty.)"
                        } else {
                            log.as_str()
                        };
                        ui.add(egui::Label::new(RichText::new(shown).monospace().small()).wrap());
                    });
                    ui.checkbox(include, "Include this log in my report");
                    ui.horizontal(|ui| {
                        if ui.button("Open the report form").clicked() {
                            let (request, copy) = report_request(log, *include);
                            if let Some(copy) = copy {
                                self.copied_text = Some(copy);
                                self.ed.message = "The log is on the clipboard: paste it into the report if the \
                                                   form shows only its end."
                                    .into();
                            }
                            self.open_url
                                .push(feedback::form_url(feedback::Kind::Problem, &request));
                            applog::info(if *include {
                                "report form opened with the log"
                            } else {
                                "report form opened without the log"
                            });
                            keep = false;
                        }
                        if let Some(dir) = applog::dir()
                            && ui.button("Show the log folder").clicked()
                        {
                            self.open_url.push(format!("file://{}", dir.display()));
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Message(m) => {
                    // A long message (an import listing every symbol) scrolls; OK stays in view.
                    egui::ScrollArea::vertical()
                        .max_height((screen.height() * 0.6).max(120.0))
                        .show(ui, |ui| ui.label(m.as_str()));
                    if ui.button("OK").clicked() {
                        keep = false;
                    }
                }
                Dialog::OpenPath(path, error) => {
                    ui.label("Project folder (the one holding project.toml):");
                    let r = ui
                        .horizontal(|ui| {
                            let r = ui.add(egui::TextEdit::singleline(&mut path.text).desired_width(340.0));
                            if ui.button("Browse…").clicked() {
                                match pick_folder("Open project", &path.text) {
                                    Picked::Path(d) => *error = path.pick(d).err(),
                                    Picked::Failed(e) => *error = Some(e),
                                    Picked::Cancelled => {}
                                }
                            }
                            r
                        })
                        .inner;
                    for recent in &self.prefs.recent {
                        if ui.link(recent.display().to_string()).clicked() {
                            *error = path.pick(recent.clone()).err();
                        }
                    }
                    if let Some(e) = error {
                        ui.label(RichText::new(e.as_str()).color(theme::Theme::of(ui.ctx()).error));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Open").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
                            match path.path() {
                                Ok(p) => {
                                    then = Some(Box::new(move |app: &mut App| {
                                        let mut prefs = std::mem::take(&mut app.prefs);
                                        app.open_dir(&p, &mut prefs);
                                        app.prefs = prefs;
                                    }));
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
                Dialog::SaveAsPath(path, error) => {
                    ui.label("Folder for the copy (new, or empty):");
                    let r = ui.add(egui::TextEdit::singleline(&mut path.text).desired_width(380.0));
                    if let Some(e) = error {
                        ui.label(RichText::new(e.as_str()).color(theme::Theme::of(ui.ctx()).error));
                    }
                    ui.horizontal(|ui| {
                        let go = ui.button("Save as").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                        if go {
                            match path.path() {
                                Ok(p) => {
                                    then = Some(Box::new(move |app: &mut App| {
                                        if let Err(e) = app.save_as_to(&p) {
                                            app.dialog = Some(Dialog::Message(e));
                                        }
                                    }));
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
                Dialog::NewProject { location, name, error } => {
                    egui::Grid::new("newproj")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Name");
                            ui.add(
                                egui::TextEdit::singleline(name)
                                    .hint_text("also the folder name")
                                    .desired_width(380.0),
                            );
                            ui.end_row();
                            ui.label("Location");
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut location.text).desired_width(300.0));
                                if ui.button("Browse…").clicked() {
                                    match pick_folder("Where to create the project", &location.text) {
                                        Picked::Path(d) => *error = location.pick(d).err(),
                                        Picked::Failed(e) => *error = Some(e),
                                        Picked::Cancelled => {}
                                    }
                                }
                            });
                            ui.end_row();
                        });
                    // The folder follows the name and the location, whichever is typed last.
                    let target = location.path().and_then(|l| prefs::new_project_dir(&l, name));
                    let t = theme::Theme::of(ui.ctx());
                    if let Some(e) = error {
                        ui.label(RichText::new(e.as_str()).color(t.error));
                    }
                    match &target {
                        Ok(d) => ui.label(RichText::new(format!("Creates {}", d.display())).color(t.muted)),
                        Err(e) => ui.label(RichText::new(e.as_str()).color(t.error)),
                    };
                    ui.label(
                        RichText::new("A new folder with one cover page, recorded as a git repository.").color(t.faint),
                    );
                    ui.horizontal(|ui| {
                        if let Ok(d) = target
                            && ui
                                .add_enabled(!location.is_empty(), egui::Button::new("Create"))
                                .clicked()
                        {
                            let n = name.trim().to_owned();
                            then = Some(Box::new(move |app: &mut App| app.new_project(&d, &n)));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::ProjectProps {
                    name,
                    fields,
                    locations,
                    loaded_locations,
                } => {
                    egui::Grid::new("projprops")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Project name");
                            ui.add(egui::TextEdit::singleline(name).desired_width(300.0));
                            ui.end_row();
                            for (k, v) in fields.iter_mut() {
                                ui.label(wc_core::project::title_field_label(k));
                                ui.add(egui::TextEdit::singleline(v).desired_width(300.0));
                                ui.end_row();
                            }
                        });
                    ui.label(
                        RichText::new("These fields fill the title block on every page.")
                            .color(theme::Theme::of(ui.ctx()).faint),
                    );
                    ui.add_space(8.0);
                    ui.label(RichText::new("Locations").strong());
                    let mut drop = None;
                    // A long list scrolls; OK and Cancel stay in view below it.
                    egui::ScrollArea::vertical()
                        .id_salt("projlocations-scroll")
                        .max_height(220.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            egui::Grid::new("projlocations")
                                .num_columns(3)
                                .spacing([8.0, 6.0])
                                .show(ui, |ui| {
                                    for (k, (path, description)) in locations.iter_mut().enumerate() {
                                        ui.add(egui::TextEdit::singleline(path).hint_text("+L1").desired_width(100.0));
                                        ui.add(
                                            egui::TextEdit::singleline(description)
                                                .hint_text("What it is")
                                                .desired_width(250.0),
                                        );
                                        if ui.button("Remove").clicked() {
                                            drop = Some(k);
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                    if let Some(k) = drop {
                        locations.remove(k);
                    }
                    if ui.button("Add location").clicked() {
                        locations.push((String::new(), String::new()));
                    }
                    let bad = wc_core::project::location_rows(locations).err();
                    if let Some(why) = &bad {
                        ui.colored_label(theme::Theme::of(ui.ctx()).error, why);
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!name.trim().is_empty() && bad.is_none(), egui::Button::new("OK"))
                            .clicked()
                        {
                            let n = name.clone();
                            let f = fields.clone();
                            // Untouched locations are left as the project has them now.
                            let l = (locations != loaded_locations).then(|| locations.clone());
                            then = Some(Box::new(move |app: &mut App| {
                                if let Some(o) = app.open.as_mut()
                                    && let Err(e) = o.set_project_props(&n, &f, l.as_deref())
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
                Dialog::Cover {
                    name,
                    fields,
                    since,
                    before,
                    seen,
                } => {
                    if let Some(now) = self.open.as_ref().and_then(|o| o.props_if_changed(seen)) {
                        *name = now.name;
                        *fields = form_fields(&now.title_block);
                    }
                    let mut changed = false;
                    egui::Grid::new("cover")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Project name");
                            changed |= ui.add(egui::TextEdit::singleline(name).desired_width(300.0)).changed();
                            ui.end_row();
                            for (k, v) in fields.iter_mut() {
                                ui.label(wc_core::project::title_field_label(k));
                                changed |= ui.add(egui::TextEdit::singleline(v).desired_width(300.0)).changed();
                                ui.end_row();
                            }
                        });
                    if changed && let Some(o) = self.open.as_mut() {
                        o.set_project_props_live(name, fields);
                        *seen = o.history_count();
                    }
                    // The logo is its own undo step, taken at once (Cancel leaves it).
                    if let Some(o) = self.open.as_mut() {
                        ui.horizontal(|ui| {
                            ui.label("Logo");
                            let current = o.project.meta.logo.as_ref().map(|l| l.file.clone());
                            ui.label(
                                RichText::new(current.as_deref().unwrap_or("none"))
                                    .color(theme::Theme::of(ui.ctx()).faint),
                            );
                            if ui
                                .button("Logo...")
                                .on_hover_text("Pick a PNG or JPEG picture; it is copied into the project")
                                .clicked()
                                && let Some(src) = rfd::FileDialog::new()
                                    .set_title("Project logo")
                                    .add_filter("Pictures", &["png", "jpg", "jpeg"])
                                    .pick_file()
                            {
                                match o.set_logo_from(&src) {
                                    Ok(file) => self.ed.message = format!("Logo set to {file}"),
                                    Err(e) => self.ed.message = e,
                                }
                                *seen = o.history_count();
                            }
                            if current.is_some() && ui.button("Remove").clicked() {
                                o.remove_logo();
                                *seen = o.history_count();
                            }
                        });
                    }
                    ui.label(
                        RichText::new(
                            "Shown on the cover and in the title block of every page as you type. \
                             Add free text on the cover with the Text tool. Drag the logo on the \
                             cover to move it, or its bottom-right corner to size it.",
                        )
                        .color(theme::Theme::of(ui.ctx()).faint),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() {
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            if let Some(o) = self.open.as_mut() {
                                o.revert_live(*since, before);
                            }
                            keep = false;
                        }
                    });
                }
                Dialog::Template {
                    draft,
                    since,
                    before,
                    seen,
                } => {
                    // A change made elsewhere (an undo, the assistant) is shown in the form.
                    if let Some(o) = self.open.as_ref()
                        && o.history_count() != *seen
                    {
                        draft.clone_from(&o.project.meta.template);
                        *seen = o.history_count();
                    }
                    let keys = self
                        .open
                        .as_ref()
                        .map(|o| template_form::value_keys(&o.project))
                        .unwrap_or_default();
                    if template_form::show(ui, draft, &keys)
                        && let Some(o) = self.open.as_mut()
                    {
                        o.set_template_live(draft);
                        *seen = o.history_count();
                    }
                    ui.label(
                        RichText::new(
                            "Every page shows each change at once. The layout is kept in \
                             project.toml; a project on the built-in layout leaves it out.",
                        )
                        .color(theme::Theme::of(ui.ctx()).faint),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() {
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            if let Some(o) = self.open.as_mut() {
                                o.revert_template(*since, before);
                            }
                            keep = false;
                        }
                    });
                }
                Dialog::PageProps {
                    id,
                    number,
                    title,
                    size,
                    overview,
                    fields,
                    loaded,
                    error,
                } => {
                    let project_fields = self
                        .open
                        .as_ref()
                        .map(|o| o.project.meta.title_block.clone())
                        .unwrap_or_default();
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
                                .selected_text(size.label())
                                .show_ui(ui, |ui| {
                                    for s in PageSize::NAMED {
                                        ui.selectable_value(size, s, s.label());
                                    }
                                    if !matches!(size, PageSize::Custom { .. }) {
                                        ui.selectable_value(size, custom_from(*size), "Custom size…");
                                    }
                                });
                            if let PageSize::Custom { width, height } = size {
                                ui.end_row();
                                ui.label("Custom size (mm)");
                                ui.horizontal(|ui| {
                                    let range = PageSize::MIN_MM..=PageSize::MAX_MM;
                                    ui.add(egui::DragValue::new(width).range(range.clone()).suffix(" wide"));
                                    ui.add(egui::DragValue::new(height).range(range).suffix(" high"));
                                });
                            }
                            ui.end_row();
                            ui.label("Overview");
                            ui.checkbox(overview, "Line diagram").on_hover_text(
                                "Shows devices again that schematic pages draw in detail. Its symbols do not count as drawings in the project check.",
                            );
                            ui.end_row();
                        });
                    ui.add_space(6.0);
                    ui.label(RichText::new("Title block on this page").strong()).on_hover_text(
                        "Values here win over the project's on this page only. Leave a field empty to show the project's value (shown greyed).",
                    );
                    egui::Grid::new("pagefields")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            for (key, value) in fields.iter_mut() {
                                ui.label(wc_core::project::title_field_label(key));
                                let hint = project_fields.get(key.as_str()).map_or("", String::as_str);
                                ui.add(
                                    egui::TextEdit::singleline(value)
                                        .hint_text(hint)
                                        .desired_width(200.0),
                                );
                                ui.end_row();
                            }
                        });
                    if let Some(e) = error {
                        ui.label(RichText::new(e.as_str()).color(theme::Theme::of(ui.ctx()).error));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked()
                            && let Some(o) = self.open.as_mut()
                        {
                            let own = changed_fields(loaded, fields);
                            match confirm_page_props(o, &self.ed, id, number, title, *size, *overview, &own)
                            {
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
                Dialog::Check(found) => {
                    let t = theme::Theme::of(ui.ctx());
                    if found.is_empty() {
                        ui.label("No problems found.");
                    }
                    let mut go: Option<(String, Option<String>)> = None;
                    egui::ScrollArea::vertical()
                        .max_height(ui.ctx().screen_rect().height() * 0.6)
                        .show(ui, |ui| {
                            egui::Grid::new("check")
                                .striped(true)
                                .spacing([12.0, 4.0])
                                .show(ui, |ui| {
                                    for f in found.iter() {
                                        let c = match f.severity {
                                            wc_report::Severity::Error => t.error,
                                            wc_report::Severity::Warning => t.selection,
                                            wc_report::Severity::Note => t.faint,
                                        };
                                        ui.label(RichText::new(f.severity.label()).color(c));
                                        let page = f
                                            .page_id
                                            .as_deref()
                                            .and_then(|id| self.open.as_ref().and_then(|o| o.project.page(id)))
                                            .map(|pg| pg.number.clone())
                                            .unwrap_or_default();
                                        ui.label(page);
                                        match &f.page_id {
                                            Some(pid) => {
                                                if ui.link(&f.message).on_hover_text("Show it").clicked() {
                                                    go = Some((pid.clone(), f.item.clone()));
                                                }
                                            }
                                            None => {
                                                ui.label(&f.message);
                                            }
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("Check again").clicked()
                            && let Some(o) = self.open.as_ref()
                        {
                            *found = run_check(o);
                        }
                        if ui.button("Close").clicked() {
                            keep = false;
                        }
                    });
                    if let Some((pid, item)) = go
                        && let Some(o) = self.open.as_ref()
                        && let Some(i) = o.project.pages.iter().position(|p| p.id == pid)
                    {
                        self.ed.page = i;
                        self.ed.tool = Tool::Select;
                        self.ed.view.fit = true;
                        self.ed.selection = item
                            .and_then(|it| {
                                let pg = &o.project.pages[i];
                                if pg.symbol(&it).is_some() {
                                    Some(Selection::Symbol(it))
                                } else if pg.wires.iter().any(|w| w.id == it) {
                                    Some(Selection::Wire(it))
                                } else {
                                    None
                                }
                            })
                            .into_iter()
                            .collect();
                    }
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
                            if ui.button("Save as CSV…").clicked() {
                                let kind = *kind;
                                then = Some(Box::new(move |app: &mut App| app.ask(Pending::ReportCsv(kind))));
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
                                && let Some(why) = o.held_reason(self.ed.page)
                            {
                                self.ed.message = why;
                            } else if let Some(o) = self.open.as_mut()
                                && let Some(sel) = o.add_graphic(
                                    self.ed.page,
                                    wc_core::geom::Shape::Text {
                                        at: *at,
                                        text: text.trim().to_owned(),
                                        height: 3.5,
                                        align: wc_core::geom::Align::Left,
                                        angle: 0.0,
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
                Dialog::Comment(at, text, focused) => {
                    ui.label(
                        RichText::new(format!("As {}", state::author_name())).color(theme::Theme::of(ui.ctx()).faint),
                    );
                    let r = ui.add(egui::TextEdit::multiline(text).desired_width(360.0).desired_rows(4));
                    if !*focused {
                        r.request_focus();
                        *focused = true;
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!text.trim().is_empty(), egui::Button::new("Add comment"))
                            .clicked()
                            && let Some(o) = self.open.as_mut()
                            && let Some(page) = o.project.pages.get(self.ed.page).map(|p| p.id.clone())
                        {
                            let mut next = o.project.clone();
                            match next.add_comment(&page, Some(*at), &state::author_name(), state::now_secs(), text) {
                                Ok(id) => {
                                    o.checkpoint();
                                    o.project = next;
                                    self.ed.selection.clear();
                                    self.ed.message = format!("Comment {id} added.");
                                    self.ed.comment = Some(id);
                                    self.ed.tool = Tool::Select;
                                }
                                Err(err) => self.ed.message = err.to_string(),
                            }
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                            keep = false;
                        }
                    });
                }
                Dialog::PageHistory(page, versions) => {
                    let t = theme::Theme::of(ui.ctx());
                    if versions.is_empty() {
                        ui.label(format!("Page {page} has no recorded versions yet."));
                        ui.label(
                            RichText::new("Record a version (Versions menu) to start its history.").color(t.faint),
                        );
                    }
                    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                        for v in versions.iter() {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(&v.commit[..v.commit.len().min(8)])
                                        .monospace()
                                        .color(t.faint),
                                );
                                ui.label(RichText::new(&v.subject).strong());
                            });
                            ui.label(
                                RichText::new(format!("{}  ·  {}", v.author, wc_core::review::utc_date(v.time)))
                                    .color(t.muted)
                                    .small(),
                            );
                            for c in &v.changes {
                                ui.label(RichText::new(format!("   {c}")).small());
                            }
                            if v.changes.is_empty() {
                                ui.label(
                                    RichText::new("   no changes to symbols or wires")
                                        .small()
                                        .color(t.faint),
                                );
                            }
                            ui.add_space(6.0);
                        }
                    });
                    if ui.button("Close").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        keep = false;
                    }
                }
                Dialog::WagoMarking {
                    kinds,
                    lines,
                    language,
                    strip,
                    strips,
                    level_separator,
                } => {
                    use wc_report::{Language, LineField, MarkerKind};
                    let field_label = |f: LineField| match f {
                        LineField::Marker => "Marker text",
                        LineField::Tag => "Tag",
                        LineField::Description => "Description",
                        LineField::Location => "Location",
                    };
                    ui.horizontal(|ui| {
                        ui.label("Markers");
                        for (k, on) in MarkerKind::ALL.iter().zip(kinds.iter_mut()) {
                            ui.checkbox(on, k.name());
                        }
                    });
                    egui::Grid::new("wago-marking")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Terminal strip");
                            egui::ComboBox::from_id_salt("wago-strip")
                                .selected_text(strip.clone().unwrap_or_else(|| "All strips".into()))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(strip, None, "All strips");
                                    for s in strips.iter() {
                                        ui.selectable_value(strip, Some(s.clone()), s);
                                    }
                                });
                            ui.end_row();
                            // A line is offered only after a chosen one, so the columns stay
                            // in order; clearing a line clears the ones after it.
                            let mut open_line = true;
                            for (n, line) in lines.iter_mut().enumerate() {
                                if !open_line {
                                    *line = None;
                                }
                                ui.label(format!("Line {}", n + 1));
                                ui.add_enabled_ui(open_line, |ui| {
                                    egui::ComboBox::from_id_salt(("wago-line", n))
                                        .selected_text(line.map_or("(none)", field_label))
                                        .show_ui(ui, |ui| {
                                            if n > 0 {
                                                ui.selectable_value(line, None, "(none)");
                                            }
                                            for f in LineField::ALL {
                                                ui.selectable_value(line, Some(f), field_label(f));
                                            }
                                        });
                                });
                                open_line = line.is_some();
                                ui.end_row();
                            }
                            ui.label("Language");
                            egui::ComboBox::from_id_salt("wago-language")
                                .selected_text(language.name())
                                .show_ui(ui, |ui| {
                                    for l in Language::ALL {
                                        ui.selectable_value(language, l, l.name());
                                    }
                                });
                            ui.end_row();
                            ui.label("Level separator");
                            ui.add(egui::TextEdit::singleline(level_separator).desired_width(60.0))
                                .on_hover_text(
                                    "Between the numbers of a multi-level terminal's levels on its one marker \
                                     (a terminal's level is its Level property)",
                                );
                            ui.end_row();
                        });
                    ui.weak(
                        "The marker text is a device's Marker property when it has one, otherwise its tag. \
                         With more than one line, smartScript maps the Line columns to the marker's lines.",
                    );
                    if strip.is_some() && kinds.iter().skip(1).any(|k| *k) {
                        ui.weak("The strip limits the terminals; the other kinds are exported in full.");
                    }
                    let none = !kinds.iter().any(|k| *k);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!none, egui::Button::new("Export")).clicked() {
                            let options = wc_report::MarkingOptions {
                                kinds: MarkerKind::ALL
                                    .into_iter()
                                    .zip(kinds.iter())
                                    .filter(|(_, on)| **on)
                                    .map(|(k, _)| k)
                                    .collect(),
                                lines: chosen_lines(lines),
                                language: *language,
                                strip: strip.clone(),
                                level_separator: level_separator.clone(),
                            };
                            then = Some(Box::new(move |app: &mut App| app.ask(Pending::Marking(options))));
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
                    // In a shared session too: undo there takes back this person's own change.
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
                Dialog::ImportSymbols {
                    targets,
                    chosen,
                    replace,
                } => {
                    ui.label(
                        "Pick DXF, SVG, KiCad (.kicad_sym) or QElectroTech (.elmt) files. Each is read by its \
                         extension and its symbols are written into the library folder below.",
                    );
                    ui.horizontal(|ui| {
                        ui.label("Library folder");
                        let shown = |t: &import_symbols::ImportTarget| t.label();
                        egui::ComboBox::from_id_salt("import_lib")
                            .selected_text(targets.get(*chosen).map(shown).unwrap_or_default())
                            .show_ui(ui, |ui| {
                                for (k, t) in targets.iter().enumerate() {
                                    ui.selectable_value(chosen, k, shown(t));
                                }
                            });
                    });
                    ui.checkbox(replace, "Replace symbols already in the library")
                        .on_hover_text("Off: a symbol with the same id stays as it is, and the result lists it");
                    ui.horizontal(|ui| {
                        if ui.button("Choose files and import…").clicked()
                            && let Some(target) = targets.get(*chosen)
                            && let Some(files) = rfd::FileDialog::new()
                                .set_title("Import symbols")
                                .add_filter("Symbol files", import_symbols::EXTENSIONS)
                                .pick_files()
                        {
                            let target = target.clone();
                            let root = target.root.clone();
                            let replace = *replace;
                            then = Some(Box::new(move |app: &mut App| {
                                let out = import_symbols::import(&files, &root, replace);
                                if let Some(open) = app.open.as_mut() {
                                    // The project's own folder is listed first once something is in it.
                                    if !out.written.is_empty()
                                        && let Some(source) = target.declare
                                    {
                                        open.checkpoint();
                                        open.project.meta.libraries.insert(0, source);
                                        open.dirty = true;
                                    }
                                    open.refresh_library();
                                }
                                app.ed.message =
                                    format!("Imported {} symbols, {} failed.", out.written.len(), out.failed.len());
                                app.dialog = Some(Dialog::Message(out.report(&root)));
                            }));
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
                                        wc_library::State::WrongId => (t.error, "library file has another id"),
                                    };
                                    chrome::dot(ui, c);
                                    ui.label(RichText::new(&it.id).monospace());
                                    if it.state == wc_library::State::WrongId {
                                        // Names the source and the id its file declares.
                                        ui.label(format!("{s}: {}", it.changes.join("; ")));
                                    } else {
                                        ui.label(s);
                                    }
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
            self.dialog = Some(Dialog::Message(format!(
                "Could not create the project in\n{}:\n{e}",
                dir.display()
            )));
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

    /// Insert a macro on the current page, selected so it can be dragged into place.
    fn insert_macro(&mut self, path: &std::path::Path, name: &str) {
        let page = self.ed.page;
        let Some(open) = self.open.as_mut() else { return };
        match open.insert_macro(page, path) {
            Ok(items) if !items.is_empty() => {
                self.ed.tool = Tool::Select;
                self.ed.message = format!("Inserted macro {name}: drag it into place.");
                self.ed.selection = items;
            }
            Ok(_) => self.ed.message = format!("Macro {name} is empty."),
            Err(e) => self.dialog = Some(Dialog::Message(format!("Could not insert {name}:\n{e}"))),
        }
    }

    fn commit(&mut self, msg: &str) {
        if self.assistant.command_running() {
            self.ed.message = "Recording a version waits until the assistant's command has finished.".into();
            return;
        }
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

/// What the folder picker gave.
#[derive(Debug, Clone, PartialEq)]
enum Picked {
    Path(PathBuf),
    Cancelled,
    /// The picker could not open; the reason is for the dialog.
    Failed(String),
}

/// The system's folder picker, starting in `start` (or its nearest existing parent). Blocks the
/// window while open, like any modal dialog.
fn pick_folder(title: &str, start: &str) -> Picked {
    // On Linux rfd tries the desktop portal, then zenity; both are given their chance.
    let mut dialog = rfd::FileDialog::new().set_title(title);
    if let Some(d) = picker_start(start) {
        dialog = dialog.set_directory(d);
    }
    applog::take_picker_errors();
    let shown = std::time::Instant::now();
    let r = dialog.pick_folder();
    match picker_outcome(r, shown.elapsed(), &applog::take_picker_errors()) {
        Picked::Failed(e) => Picked::Failed(format!("{e} Type the folder instead.")),
        p => p,
    }
}

/// The system's save dialog, in `dir` when it exists, with `name` filled in and, when given,
/// only files of one kind shown. Blocks the window while open, like any modal dialog.
fn pick_save(title: &str, dir: &Path, name: &str, filter: Option<(&str, &[&str])>) -> Picked {
    let mut dialog = rfd::FileDialog::new().set_title(title).set_file_name(name);
    if let Some(d) = dir.ancestors().find(|d| !d.as_os_str().is_empty() && d.is_dir()) {
        dialog = dialog.set_directory(d);
    }
    if let Some((label, extensions)) = filter {
        dialog = dialog.add_filter(label, extensions);
    }
    applog::take_picker_errors();
    let shown = std::time::Instant::now();
    let r = dialog.save_file();
    picker_outcome(r, shown.elapsed(), &applog::take_picker_errors())
}

/// The system's file picker for one file of a kind, in `dir` or its nearest existing folder.
/// Blocks the window while open, like any modal dialog.
fn pick_open(title: &str, dir: &Path, filter: (&str, &[&str])) -> Picked {
    let mut dialog = rfd::FileDialog::new().set_title(title).add_filter(filter.0, filter.1);
    if let Some(d) = dir.ancestors().find(|d| !d.as_os_str().is_empty() && d.is_dir()) {
        dialog = dialog.set_directory(d);
    }
    applog::take_picker_errors();
    let shown = std::time::Instant::now();
    let r = dialog.pick_file();
    picker_outcome(r, shown.elapsed(), &applog::take_picker_errors())
}

/// Where an export goes: the file picked in the save dialog, or `default` with a note when the
/// dialog could not open; `None` when the person cancelled.
fn export_target(title: &str, default: &Path, filter: (&str, &[&str])) -> Option<(PathBuf, String)> {
    let dir = default.parent().unwrap_or(default);
    let name = default
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match pick_save(title, dir, &name, Some(filter)) {
        Picked::Path(p) => Some((p, String::new())),
        Picked::Failed(e) => Some((
            default.to_path_buf(),
            format!("{e} So it was saved in the usual place.\n\n"),
        )),
        Picked::Cancelled => None,
    }
}

/// Write a file, making its folder first.
fn write_creating(path: &Path, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, bytes)
}

/// The usual place for a whole-project export: `export/<project name>.<extension>`.
fn export_file(open: &Open, extension: &str) -> PathBuf {
    let name: String = open
        .project
        .meta
        .name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    let stem = if name.is_empty() { "project" } else { &name };
    open.dir.join("export").join(format!("{stem}.{extension}"))
}

/// The picker reports a failure the same way as Cancel. On Linux rfd logs it when the desktop
/// portal fails and then zenity does too (`errors`, see [`applog::take_picker_errors`]); a
/// portal failure alone is not one, since zenity then showed. Otherwise an answer quicker than
/// anyone can click means it never showed.
fn picker_outcome(r: Option<PathBuf>, open_for: std::time::Duration, errors: &[String]) -> Picked {
    const NO_PORTAL: &str = "The file picker could not open. On Linux it needs a desktop portal \
                             (xdg-desktop-portal) or zenity in a desktop session.";
    match r {
        Some(d) => Picked::Path(d),
        None if errors.iter().any(|e| e.contains("with zenity")) => {
            applog::warn(&format!(
                "folder picker: could not open after {open_for:?}: {}",
                errors.join("; ")
            ));
            Picked::Failed(NO_PORTAL.to_owned())
        }
        // Only when rfd logged nothing: a portal failure alone means zenity showed instead.
        None if errors.is_empty() && open_for < std::time::Duration::from_millis(150) => {
            applog::warn(&format!(
                "folder picker: no answer after {open_for:?}; taken as not opened"
            ));
            Picked::Failed(NO_PORTAL.to_owned())
        }
        None => Picked::Cancelled,
    }
}

/// Where the folder picker opens for what is typed in a dialog: the projects folder for a blank
/// field, otherwise the same rule as the dialogs (a bare name is inside the projects folder),
/// then the nearest folder of that path that exists.
fn picker_start(typed: &str) -> Option<PathBuf> {
    // Text that cannot be used yet (a drive-relative path) opens the projects folder.
    let start = if typed.trim().is_empty() {
        prefs::projects_dir()
    } else {
        prefs::project_path(typed).unwrap_or_else(|_| prefs::projects_dir())
    };
    start
        .ancestors()
        .find(|d| !d.as_os_str().is_empty() && d.is_dir())
        .map(Path::to_path_buf)
}

/// The window and taskbar icon (assets/icon, made by tools/gen_icon.py), built into the
/// program. An unreadable picture gives an empty icon, so the system default shows.
fn window_icon() -> egui::IconData {
    const PNG: &[u8] = include_bytes!("../../../assets/icon/wiringcraft-256.png");
    image::load_from_memory_with_format(PNG, image::ImageFormat::Png)
        .map(|img| {
            let img = img.into_rgba8();
            let (width, height) = img.dimensions();
            egui::IconData {
                rgba: img.into_raw(),
                width,
                height,
            }
        })
        .unwrap_or_else(|e| {
            eprintln!("wiringcraft-app: the built-in icon could not be read ({e}); using the system default");
            egui::IconData::default()
        })
}

/// The main window: size, title and icon.
fn main_viewport() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_inner_size([1400.0, 860.0])
        .with_min_inner_size([900.0, 560.0])
        .with_title("WiringCraft")
        .with_icon(window_icon())
}

/// The project's library folders that symbols can be written into, in the project's order (the
/// first is its own). Only folders: a git source is a cache that the next sync overwrites. Each
/// source is paired with its declaration by position (`Library::open` keeps the order), so a git
/// source sharing a folder's name is never offered.
fn folder_targets(lib: &wc_library::Library, open: &Open) -> Vec<symbol_editor::Target> {
    lib.sources
        .iter()
        .zip(&open.project.meta.libraries)
        .filter(|(_, decl)| decl.kind == "folder")
        .map(|(s, _)| (s.name.clone(), s.root.clone()))
        .collect()
}

/// The starter symbol library: `library/` beside the executable or in the source tree.
fn starter_library() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../library");
    find_library(exe.as_deref().and_then(Path::parent), &source)
}

/// The first `library/` with a `symbols` folder: next to the program (Windows install, download
/// folder), the Linux package layout (/usr/bin -> /usr/share/wiringcraft), the macOS app bundle
/// (Contents/MacOS -> Contents/Resources), then the source checkout this was built from.
fn find_library(exe_dir: Option<&Path>, source: &Path) -> Option<PathBuf> {
    let candidates = [
        exe_dir.map(|d| d.join("library")),
        exe_dir.map(|d| d.join("../share/wiringcraft/library")),
        exe_dir.map(|d| d.join("../Resources/library")),
        Some(source.to_path_buf()),
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

fn run_check(open: &Open) -> Vec<wc_report::Finding> {
    wc_report::check(&wc_report::Inputs {
        project: &open.project,
        symbols: &open.symbols,
        parts: &open.parts,
    })
}

fn report_title(k: wc_report::Kind) -> &'static str {
    match k {
        wc_report::Kind::Parts => "Parts",
        wc_report::Kind::Devices => "Devices",
        wc_report::Kind::Wires => "Wires",
        wc_report::Kind::Nets => "Nets",
        wc_report::Kind::Terminals => "Terminals",
        wc_report::Kind::Cables => "Cables",
        wc_report::Kind::Plc => "PLC I/O",
        wc_report::Kind::Contents => "Contents",
        wc_report::Kind::Locations => "Locations",
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

/// A project's title-block fields as form rows: the standard fields first (empty when not
/// set), then any others the project has.
/// The Ctrl shortcuts pressed this frame, none while a dialog is open.
fn ctrl_shortcuts(ctx: &egui::Context, dialog_open: bool) -> Vec<Action> {
    let mut out = Vec::new();
    if dialog_open {
        return out;
    }
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
            (Key::E, Action::Check),
            (Key::A, Action::SelectAll),
            (Key::F, Action::Find),
            (Key::J, Action::Assistant),
        ] {
            // With Shift first: Ctrl+Shift+N is a new project, Ctrl+Shift+Z redoes (a plain Ctrl
            // shortcut would also take them, as it lets Shift through).
            let shifted = match k {
                Key::N => Some(Action::NewProject),
                Key::Z => Some(Action::Redo),
                Key::S => Some(Action::SaveAs),
                _ => None,
            };
            if let Some(sa) = shifted
                && i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, k))
            {
                out.push(sa);
            } else if i.consume_shortcut(&ctrl(k)) {
                out.push(a);
            }
        }
    });
    out
}

fn form_fields(block: &std::collections::BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut fields: Vec<(String, String)> = TITLE_FIELDS
        .iter()
        .map(|(k, _)| ((*k).to_owned(), block.get(*k).cloned().unwrap_or_default()))
        .collect();
    for (k, v) in block {
        if !TITLE_FIELDS.iter().any(|(f, _)| f == k) {
            fields.push((k.clone(), v.clone()));
        }
    }
    fields
}

fn export_pdf(open: &Open) -> String {
    export_pdf_to(open, &export_file(open, "pdf"))
}

/// [`export_pdf`] to the file `path`.
fn export_pdf_to(open: &Open, path: &Path) -> String {
    // The table of contents goes before the schematic pages and the other reports after them,
    // as in a finished drawing set.
    let inputs = wc_report::Inputs {
        project: &open.project,
        symbols: &open.symbols,
        parts: &open.parts,
    };
    let pictures = wc_pdf::read_pictures(&open.dir, &open.project);
    let options = wc_pdf::Options {
        reports: wc_report::Kind::ALL
            .into_iter()
            .map(|k| wc_report::build(k, &inputs))
            .filter(|t| !t.rows.is_empty())
            .collect(),
        underlays: pictures.files,
        unreadable: pictures.unreadable,
        panels: open.project.panels.clone(),
        ..Default::default()
    };
    let (bytes, report) = wc_pdf::export(&open.project, &open.symbols, &options);
    match write_creating(path, bytes) {
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

/// Devices and connections as AutomationML in `export/<project>.aml`.
fn export_aml(open: &Open) -> String {
    export_aml_to(open, &export_file(open, "aml"))
}

/// [`export_aml`] to the file `path`.
fn export_aml_to(open: &Open, path: &Path) -> String {
    let inputs = wc_report::Inputs {
        project: &open.project,
        symbols: &open.symbols,
        parts: &open.parts,
    };
    // A fixed time stamp keeps repeated exports of the same project identical, so they diff
    // cleanly in git.
    let (text, counts) = wc_report::automationml(&inputs, "");
    match write_creating(path, text) {
        Ok(()) => {
            let mut m = format!(
                "Exported {} devices and {} connections to\n{}",
                counts.devices,
                counts.links,
                path.display()
            );
            if counts.untagged_pins > 0 {
                m.push_str(&format!(
                    "\n\n{} connected pin(s) on symbols without a tag were left out.",
                    counts.untagged_pins
                ));
            }
            m
        }
        Err(e) => format!("Export failed: {e}"),
    }
}

/// The marker lines chosen in the dialog, up to the first one left at (none): a later line never
/// moves up into an empty one's column.
fn chosen_lines(lines: &[Option<wc_report::LineField>]) -> Vec<wc_report::LineField> {
    lines.iter().map_while(|l| *l).collect()
}

/// The WAGO marking dialog: every kind (only terminals when a strip is given), one marker line.
fn wago_marking_dialog(open: &Open, strip: Option<String>) -> Dialog {
    let inputs = wc_report::Inputs {
        project: &open.project,
        symbols: &open.symbols,
        parts: &open.parts,
    };
    Dialog::WagoMarking {
        kinds: if strip.is_some() {
            [true, false, false, false]
        } else {
            [true; 4]
        },
        lines: [Some(wc_report::LineField::Marker), None, None],
        language: wc_report::Language::En,
        strip,
        strips: wc_report::terminal_strips(&inputs),
        level_separator: wc_report::LEVEL_SEPARATOR.to_owned(),
    }
}

/// The project's terminal strips for the WAGO configurator, written to
/// `export/wago-configurator.csv`.
#[cfg(test)]
fn export_wago_configurator(open: &Open) -> String {
    export_wago_configurator_to(open, &open.dir.join("export").join("wago-configurator.csv"))
}

/// Whether the project has no terminal strip for the configurator list (the export then says
/// so without asking where to save).
fn nothing_for_configurator(open: &Open) -> bool {
    let inputs = wc_report::Inputs {
        project: &open.project,
        symbols: &open.symbols,
        parts: &open.parts,
    };
    wc_report::configurator_list(&inputs).table.rows.is_empty()
}

/// Whether the strip a marker list is limited to (if any) has terminals to list.
fn marking_strip_listed(open: &Open, options: &wc_report::MarkingOptions) -> bool {
    let inputs = wc_report::Inputs {
        project: &open.project,
        symbols: &open.symbols,
        parts: &open.parts,
    };
    options
        .strip
        .as_ref()
        .is_none_or(|s| wc_report::terminal_strips(&inputs).contains(s))
}

/// [`export_wago_configurator`] to the file `path`.
fn export_wago_configurator_to(open: &Open, path: &Path) -> String {
    let inputs = wc_report::Inputs {
        project: &open.project,
        symbols: &open.symbols,
        parts: &open.parts,
    };
    let wc_report::Configurator { table: t, warnings, .. } = wc_report::configurator_list(&inputs);
    if t.rows.is_empty() {
        return "Nothing exported: the project has no numbered terminals on its schematic pages (terminals drawn \
                only on an overview page are not listed)."
            .to_owned();
    }
    match write_creating(path, wc_report::to_smartscript_csv(&t)) {
        Ok(()) => {
            let strips: std::collections::BTreeSet<&str> =
                t.rows.iter().filter_map(|r| r.get(2)).map(String::as_str).collect();
            let mut msg = format!(
                "Exported {} terminal strips ({} rows) to\n{}\n\nIn the WAGO configurator, import it as an \
                 Excel/CSV article list and map the Article and Quantity columns.",
                strips.len(),
                t.rows.len(),
                path.display()
            );
            for w in warnings.into_iter().chain(wc_report::level_warnings(inputs.project)) {
                msg.push_str("\n\nWarning: ");
                msg.push_str(&w);
            }
            msg
        }
        Err(e) => format!("Export failed: {e}"),
    }
}

/// The markers `options` asks for as a smartScript CSV in `export`: `wago-marking.csv`, or
/// `wago-marking-<strip>.csv` for one strip.
#[cfg(test)]
fn export_wago_marking(open: &Open, options: &wc_report::MarkingOptions) -> String {
    export_wago_marking_to(open, options, &wago_marking_file(open, options))
}

/// The usual file for a marker list: `export/wago-marking.csv`, or
/// `export/wago-marking-<strip>.csv` for one strip.
fn wago_marking_file(open: &Open, options: &wc_report::MarkingOptions) -> PathBuf {
    let name = match &options.strip {
        Some(s) => format!("wago-marking-{}.csv", file_stem(s)),
        None => "wago-marking.csv".to_owned(),
    };
    open.dir.join("export").join(name)
}

/// [`export_wago_marking`] to the file `path`.
fn export_wago_marking_to(open: &Open, options: &wc_report::MarkingOptions, path: &Path) -> String {
    let inputs = wc_report::Inputs {
        project: &open.project,
        symbols: &open.symbols,
        parts: &open.parts,
    };
    // The strip may have no numbered terminal on a schematic page (only on an overview), or be
    // gone since the dialog opened: say so instead of writing an empty list.
    if let Some(s) = &options.strip
        && !wc_report::terminal_strips(&inputs).contains(s)
    {
        return format!(
            "Nothing exported: terminal strip {s} has no terminals on the schematic pages (terminals drawn \
             only on an overview page are not listed)."
        );
    }
    let t = wc_report::marking_list_with(&inputs, options);
    let csv = wc_report::to_smartscript_csv(&t);
    match write_creating(path, csv) {
        Ok(()) => {
            let columns = if t.columns.len() > 4 {
                "Line columns"
            } else {
                "marker column"
            };
            let mut msg = format!(
                "Exported {} markers to\n{}\n\nIn WAGO smartScript, import it as Excel/CSV data and map the {columns}.",
                t.rows.len(),
                path.display()
            );
            for w in wc_report::formula_warnings(&t)
                .into_iter()
                .chain(wc_report::level_warnings(inputs.project))
            {
                msg.push_str("\n\nWarning: ");
                msg.push_str(&w);
            }
            msg
        }
        Err(e) => format!("Export failed: {e}"),
    }
}

/// A strip tag as part of a file name: characters a file name cannot hold become `_`
/// (`-X1` stays, `a/b` becomes `a_b`).
fn file_stem(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || "-+=._".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Every page as SVG and DXF in `export/drawings`, named by page number.
fn export_drawings(open: &Open) -> String {
    export_drawings_to(open, &open.dir.join("export").join("drawings"))
}

/// [`export_drawings`] into the folder `dir`.
fn export_drawings_to(open: &Open, dir: &Path) -> String {
    let pictures = wc_pdf::read_pictures(&open.dir, &open.project);
    let options = wc_pdf::Options {
        underlays: pictures.files,
        unreadable: pictures.unreadable,
        ..Default::default()
    };
    let (pages, report) = wc_pdf::page_drawings(&open.project, &open.symbols, &options);
    let mut used = std::collections::BTreeSet::new();
    let written = std::fs::create_dir_all(dir).and_then(|()| {
        for page in &pages {
            let base = if page.number.trim().is_empty() {
                &page.id
            } else {
                page.number.trim()
            };
            let mut stem: String = base
                .chars()
                .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
                .collect();
            while !used.insert(stem.clone()) {
                stem.push('_');
            }
            std::fs::write(dir.join(format!("{stem}.svg")), wc_pdf::svg(page))?;
            std::fs::write(dir.join(format!("{stem}.dxf")), wc_pdf::dxf(page))?;
        }
        Ok(())
    });
    match written {
        Ok(()) => {
            let mut m = format!("Exported {} pages as SVG and DXF to\n{}", pages.len(), dir.display());
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
        // The last edit is done: jumpers follow their terminals before anything shows or copies them.
        if let Some(o) = self.open.as_mut() {
            o.follow_terminals();
        }
        let status = StatusView {
            open: self.open.as_ref(),
            ed: &self.ed,
            prefs: &self.prefs,
            clipboard: &self.clipboard,
            sharing: self.share.active(),
            share_undo: self.share.can_undo(),
            share_redo: self.share.can_redo(),
        };
        for url in std::mem::take(&mut self.open_url) {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
        let mut actions = chrome::show(ctx, self.prefs.chrome, &mut self.chrome, &status);
        let keys = self.shortcuts(ctx);
        actions.extend(self.route_to_symbol_editor(keys));
        for a in actions {
            self.run(a);
        }
        // So did any the actions made (a move by arrow key, say): the panels show jumpers that
        // match the terminals.
        if let Some(o) = self.open.as_mut() {
            o.follow_terminals();
        }
        self.log_messages();
        self.assistant_poll(ctx);
        if let Some(p) = self.pending.take() {
            self.ask(p);
        }
        if self.applied_theme != self.prefs.theme {
            theme::apply(ctx, self.prefs.theme);
            self.applied_theme = self.prefs.theme;
        }
        if let Some(text) = self.copied_text.take() {
            ctx.copy_text(text);
        }

        let t = theme::Theme::of(ctx);
        let side = |fill| {
            egui::Frame::new()
                .fill(fill)
                .stroke(Stroke::new(1.0_f32, t.border))
                .inner_margin(egui::Margin::symmetric(10, 8))
        };
        egui::TopBottomPanel::bottom("status")
            .frame(side(t.toolbar_bg).inner_margin(egui::Margin::symmetric(10, 4)))
            .show_separator_line(false)
            .show(ctx, |ui| panels::status_bar(ui, self.open.as_ref(), &self.ed));
        self.assistant_panel(ctx);

        let mut page_actions = Vec::new();
        if let Some(open) = self.open.as_mut() {
            egui::SidePanel::left("navigator")
                .frame(side(t.panel))
                .show_separator_line(false)
                .default_width(230.0)
                .show(ctx, |ui| panels::navigator(ui, open, &mut self.ed, &mut page_actions));
            egui::SidePanel::right("inspector")
                .frame(side(t.panel))
                .show_separator_line(false)
                .default_width(280.0)
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
                self.ed.auto_wire = self.prefs.auto_wire;
                canvas::show(ui, open, &mut self.ed, self.prefs.show_grid, self.prefs.snap);
                let held = open
                    .project
                    .pages
                    .get(self.ed.page)
                    .and_then(|p| self.ed.held_by.get(&p.id));
                if let Some(name) = held
                    && self.ed.panel.is_none()
                {
                    read_only_banner(ui, &t, name);
                }
            });
        } else {
            let mut start = None;
            let mut recent = None;
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(t.backdrop))
                .show(ctx, |ui| {
                    welcome(ui, &t, &self.prefs.recent, &mut start, &mut recent);
                });
            if let Some(d) = recent {
                let mut prefs = std::mem::take(&mut self.prefs);
                self.open_dir(&d, &mut prefs);
                self.prefs = prefs;
            }
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
        if let Some(at) = self.ed.ask_comment.take() {
            self.dialog = Some(Dialog::Comment(at, String::new(), false));
        }
        self.dialogs(ctx);
        self.share
            .windows(ctx, self.open.as_mut(), self.assistant.command_running());
        self.symbol_editor_window(ctx);
        self.find_window(ctx);
        self.sync_session(ctx);

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

impl App {
    /// The symbol editor, while open. A saved symbol shows up in the place list at once; when
    /// the project already uses it, its copy in the project is updated too (one undo step).
    /// While the symbol editor is open, Undo and Delete work on the symbol being drawn, and
    /// the shortcuts that change the schematic are held back.
    fn route_to_symbol_editor(&mut self, keys: Vec<Action>) -> Vec<Action> {
        let Some(ed) = self.symbol_editor.as_mut() else {
            return keys;
        };
        keys.into_iter()
            .filter(|a| match a {
                Action::Undo => {
                    ed.undo();
                    false
                }
                Action::Redo => {
                    ed.redo();
                    false
                }
                Action::Delete => {
                    ed.delete_selected();
                    false
                }
                Action::Cut | Action::Copy | Action::Paste | Action::Rotate | Action::Mirror | Action::SelectAll => {
                    false
                }
                _ => true,
            })
            .collect()
    }

    fn find_window(&mut self, ctx: &egui::Context) {
        let (Some(find), Some(open)) = (self.find.as_mut(), self.open.as_ref()) else {
            self.find = None;
            return;
        };
        match find.show(ctx, &open.project) {
            find_window::FindOutcome::None => {}
            find_window::FindOutcome::Close => self.find = None,
            find_window::FindOutcome::Go { page, select } => self.show_found(&page, select),
            find_window::FindOutcome::Replace => self.replace_found(),
        }
    }

    /// Show a find hit: open its page and select the item.
    fn show_found(&mut self, page: &str, select: Option<Selection>) {
        let Some(o) = self.open.as_ref() else { return };
        let Some(i) = o.project.pages.iter().position(|p| p.id == page) else {
            self.ed.message = "That page is gone.".into();
            return;
        };
        self.ed.panel = None;
        if self.ed.page != i {
            // A wire started on the other page cannot end on this one.
            self.ed.wire = None;
        }
        self.ed.page = i;
        self.ed.tool = Tool::Select;
        self.ed.view.fit = true;
        self.ed.selection = select.into_iter().collect();
    }

    /// Replace in the ticked find hits, as one undo step.
    fn replace_found(&mut self) {
        let (Some(find), Some(open)) = (self.find.as_mut(), self.open.as_mut()) else {
            return;
        };
        let hits = find.chosen_hits();
        find.message = match open.replace_found(&hits, &find.query, &find.with, find.options()) {
            Ok(n) => find_window::replaced_note(n, &hits),
            Err(e) => e,
        };
        // The items replaced may no longer match; what is selected may be gone.
        self.ed.selection.clear();
    }

    fn symbol_editor_window(&mut self, ctx: &egui::Context) {
        let Some(ed) = self.symbol_editor.as_mut() else { return };
        // The editor belongs to the project it was opened in: its library folders are that
        // project's. Another project opened since closes it.
        if ed.owner.as_deref() != self.open.as_ref().map(|o| o.dir.as_path()) {
            self.symbol_editor = None;
            return;
        }
        let lib = self
            .open
            .as_ref()
            .map(|o| wc_library::Library::open(&o.dir, &o.project).map_err(|e| e.to_string()));
        let mut read = |id: &str| match &lib {
            Some(Ok(l)) => l.symbol(id).map_err(|e| e.to_string()),
            Some(Err(e)) => Err(e.clone()),
            None => Err("no project is open".to_owned()),
        };
        let (saved, keep) = ed.show(ctx, &mut read);
        if !keep {
            self.symbol_editor = None;
        }
        if let Some(saved) = saved {
            self.symbol_saved(saved);
        }
    }

    /// After the symbol editor saved: tell what the project gets, and update its copy when the
    /// pages use the symbol.
    fn symbol_saved(&mut self, saved: crate::symbol_editor::Saved) {
        let Some(open) = self.open.as_mut() else { return };
        open.refresh_library();
        let lib = match wc_library::Library::open(&open.dir, &open.project) {
            Ok(l) => l,
            Err(e) => {
                self.ed.message = format!("Symbol {} saved, but the project's copy was not updated: {e}", saved.id);
                return;
            }
        };
        // The project takes a symbol from the first library that has it: when that is another
        // folder, the saved file is not what the project gets, now or when it is placed.
        if let Some(first) = lib.locate(wc_core::lock::ItemKind::Symbol, &saved.id)
            && first.source != saved.source
        {
            // A used symbol stays as the project has it, which may be an older version or a
            // copy from yet another library; an unused one would come from the first library.
            let keeps = if open.symbols.contains_key(&saved.id) {
                "the project keeps the copy it has".to_owned()
            } else {
                format!("placing it takes the one from {}", first.source)
            };
            self.ed.message = format!(
                "Symbol {} saved into {}, but library {} comes first and has a symbol with the same id; {keeps}.",
                saved.id, saved.source, first.source
            );
            return;
        }
        if !open.symbols.contains_key(&saved.id) {
            self.ed.message = format!("Symbol {} saved; it is in the place list.", saved.id);
            return;
        }
        // Wires on a pin the new version no longer has would point at nothing: say which, and
        // leave the project's copy as it is.
        let dangling = state::dangling_pins(&open.project, &saved.id, &saved.symbol);
        if !dangling.is_empty() {
            self.ed.message = format!(
                "Symbol {} saved, but the project keeps its old version: wires end on pins it no longer has ({}). Rename those pins back, or move the wires, then update the library.",
                saved.id,
                dangling.join(", ")
            );
            return;
        }
        match open.take_saved_symbol(&lib, &saved.id) {
            Ok(()) => {
                open.refresh_library();
                self.ed.message = format!("Symbol {} saved and updated on the pages that use it.", saved.id);
            }
            Err(e) => {
                self.ed.message = format!("Symbol {} saved, but the project's copy was not updated: {e}", saved.id)
            }
        }
    }
}

/// The page properties dialog's OK: refused when the page was removed, or when someone else
/// opened it while the dialog was open.
/// A page's own title-block fields as the page properties show them, key and value.
fn own_fields(p: &wc_core::Page) -> Vec<(String, String)> {
    p.title_field_keys()
        .into_iter()
        .map(|k| {
            let v = p.title_block.get(&k).cloned().unwrap_or_default();
            (k, v)
        })
        .collect()
}

/// The fields of `draft` whose value differs from when the dialog opened (`loaded`).
fn changed_fields(
    loaded: &[(String, String)],
    draft: &[(String, String)],
) -> std::collections::BTreeMap<String, String> {
    draft
        .iter()
        .filter(|(k, v)| {
            loaded
                .iter()
                .find(|(lk, _)| lk == k)
                .is_none_or(|(_, lv)| lv.trim() != v.trim())
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod page_field_tests {
    use super::changed_fields;

    #[test]
    fn only_fields_changed_in_the_dialog_are_saved() {
        let row = |k: &str, v: &str| (k.to_owned(), v.to_owned());
        let loaded = [row("revision", "A"), row("date", ""), row("checked_by", "OH")];
        let draft = [row("revision", "A"), row("date", "2026-11-18"), row("checked_by", " ")];
        let got = changed_fields(&loaded, &draft);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got["date"], "2026-11-18");
        assert_eq!(got["checked_by"], " ", "a cleared field is sent to be cleared");
        assert!(!got.contains_key("revision"), "unchanged: a newer value is kept");
    }
}

#[allow(clippy::too_many_arguments)]
fn confirm_page_props(
    o: &mut Open,
    ed: &Editor,
    id: &str,
    number: &str,
    title: &str,
    size: wc_core::PageSize,
    overview: bool,
    fields: &std::collections::BTreeMap<String, String>,
) -> Result<usize, String> {
    let i = o
        .project
        .pages
        .iter()
        .position(|p| p.id == id)
        .ok_or_else(|| "This page was removed by another participant.".to_owned())?;
    if let Some(name) = ed.held_by.get(id) {
        return Err(format!(
            "{name} has this page open; it is read-only here until they leave it."
        ));
    }
    o.set_page_props(i, number, title, size, overview, Some(fields))
}

/// Who holds the page action `a` would change, in a shared session: then it is refused. Page
/// actions (properties, delete, move up or down) are about the current page even while a panel
/// is shown; canvas actions only when the page itself is shown, as in the panel view they work
/// on the panel.
fn held_for(a: Action, ed: &Editor, project: &wc_core::Project) -> Option<String> {
    // With a panel shown these still work on the schematic page under it (Delete goes to the
    // panel first), so they follow that page's hold.
    let on_page_under_panel =
        ed.panel.is_some() && matches!(a, Action::Paste | Action::Cut | Action::Rotate | Action::Mirror);
    let name = if a.edits_page_itself() || on_page_under_panel {
        let page = project.pages.get(ed.page)?;
        // Moving a page up or down renumbers its neighbour too.
        let neighbour = match a {
            Action::PageUp => ed.page.checked_sub(1).and_then(|i| project.pages.get(i)),
            Action::PageDown => project.pages.get(ed.page + 1),
            _ => None,
        };
        ed.held_by
            .get(&page.id)
            .or_else(|| neighbour.and_then(|n| ed.held_by.get(&n.id)))
            .map(String::as_str)
    } else if a.edits_page() {
        ed.held_page(project)
    } else {
        None
    };
    name.map(str::to_owned)
}

/// A note across the top of the canvas: someone else has this page open, so it is read-only.
fn read_only_banner(ui: &egui::Ui, t: &theme::Theme, name: &str) {
    let p = ui.painter();
    let galley = p.layout_no_wrap(
        format!("Read-only: {name} has this page open"),
        egui::FontId::proportional(13.0),
        t.text,
    );
    let top = ui.max_rect().center_top() + Vec2::new(0.0, 12.0);
    let rect = egui::Rect::from_center_size(
        top + Vec2::new(0.0, galley.size().y / 2.0 + 6.0),
        galley.size() + Vec2::new(24.0, 12.0),
    );
    p.rect_filled(rect, 6.0, t.surface);
    p.rect_stroke(rect, 6.0, Stroke::new(1.0_f32, t.warning), egui::StrokeKind::Inside);
    p.galley(rect.min + Vec2::new(12.0, 6.0), galley, t.text);
}

/// The start screen shown when no project is open: a card with the two ways in and the
/// recently opened projects.
fn welcome(
    ui: &mut egui::Ui,
    t: &theme::Theme,
    recent: &[PathBuf],
    start: &mut Option<Action>,
    open_recent: &mut Option<PathBuf>,
) {
    let width = 440.0;
    ui.add_space((ui.available_height() * 0.18).max(24.0));
    ui.vertical_centered(|ui| {
        ui.set_max_width(width);
        egui::Frame::new()
            .fill(t.surface)
            .stroke(Stroke::new(1.0_f32, t.border))
            .corner_radius(12.0)
            .shadow(ui.visuals().window_shadow)
            .inner_margin(egui::Margin::same(28))
            .show(ui, |ui| {
                ui.set_width(width - 56.0);
                ui.vertical_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(44.0), egui::Sense::hover());
                    chrome::logo(ui.painter(), r, t);
                    ui.add_space(8.0);
                    ui.label(RichText::new("WiringCraft").font(theme::strong(22.0)).color(t.text));
                    ui.label(RichText::new("Electrical schematics, kept in plain files.").color(t.muted));
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        ui.add_space((ui.available_width() - 236.0) / 2.0);
                        let primary = egui::Button::new(RichText::new("New project").color(egui::Color32::WHITE))
                            .fill(t.accent)
                            .min_size(Vec2::new(114.0, 30.0));
                        if ui.add(primary).clicked() {
                            *start = Some(Action::NewProject);
                        }
                        if ui
                            .add(egui::Button::new("Open folder…").min_size(Vec2::new(114.0, 30.0)))
                            .clicked()
                        {
                            *start = Some(Action::Open);
                        }
                    });
                });
                if !recent.is_empty() {
                    ui.add_space(18.0);
                    panels::section(ui, "Recent projects", None);
                    for d in recent.iter().take(6) {
                        let name = d
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| d.display().to_string());
                        let r = panels::row(ui, false, panels::Lead::Dot(t.accent), RichText::new(name), false)
                            .on_hover_text(d.display().to_string());
                        if r.clicked() {
                            *open_recent = Some(d.clone());
                        }
                    }
                }
            });
    });
}

/// How much of the log a problem report shows and copies (GitHub takes about 65 000 characters).
const REPORT_LOG_BYTES: usize = 60_000;
/// How much of the log's end goes into the report form itself (the link has a size limit).
const REPORT_FORM_BYTES: usize = 1_500;

/// The report form's fields and the text for the clipboard: the log goes in only when the person
/// chose to include it and there is one.
fn report_request(log: &str, include: bool) -> (feedback::Request, Option<String>) {
    let mut request = feedback::Request::default();
    if include && !log.is_empty() {
        request.detail = report_detail(log);
        return (request, Some(log.to_owned()));
    }
    (request, None)
}

/// The "how to make it happen again" field of a report: the end of the log.
fn report_detail(log: &str) -> String {
    let mut start = log.len().saturating_sub(REPORT_FORM_BYTES);
    while !log.is_char_boundary(start) {
        start += 1;
    }
    let tail = &log[start..];
    let tail = if start > 0 {
        tail.split_once('\n').map_or(tail, |(_, rest)| rest)
    } else {
        tail
    };
    format!("End of the WiringCraft log (paste the whole log from the clipboard if it helps):\n```\n{tail}```\n")
}

impl App {
    /// Write new status messages and message boxes to the log, once each.
    fn log_messages(&mut self) {
        for (level, text) in self.new_messages() {
            applog::log(level, &text);
        }
    }

    /// Status messages and message boxes not logged yet. A message box is logged once while it
    /// is open; after it closes, the same text is logged again the next time it shows.
    fn new_messages(&mut self) -> Vec<(applog::Level, String)> {
        let mut new = Vec::new();
        if !self.ed.message.is_empty() && self.ed.message != self.logged.0 {
            self.logged.0 = self.ed.message.clone();
            new.push((applog::Level::Info, self.logged.0.clone()));
        }
        match &self.dialog {
            Some(Dialog::Message(m)) if *m != self.logged.1 => {
                self.logged.1 = m.clone();
                new.push((applog::Level::Warn, m.clone()));
            }
            Some(Dialog::Message(_)) => {}
            _ => self.logged.1.clear(),
        }
        new
    }
}

/// The custom sheet "Custom size…" starts from: the size shown, as it is drawn (A4 portrait
/// gives 210 x 297 mm).
fn custom_from(size: PageSize) -> PageSize {
    let (w, h) = size.mm();
    PageSize::Custom {
        width: w.round() as u32,
        height: h.round() as u32,
    }
}

fn main() -> eframe::Result {
    // Started by Claude Code as the assistant's MCP server: pass messages to the running app.
    let mut args = std::env::args_os();
    if args.nth(1).is_some_and(|a| a == "--mcp-bridge") {
        let addr = args
            .next()
            .map(|a| a.to_string_lossy().into_owned())
            .unwrap_or_default();
        let token = std::env::var(wc_agent::bridge::TOKEN_VAR).unwrap_or_default();
        if let Err(e) = wc_agent::bridge::proxy(&addr, &token) {
            eprintln!("wiringcraft-app --mcp-bridge {addr}: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    applog::init();
    let dir = std::env::args_os().nth(1).map(PathBuf::from);
    let wanted = std::env::var(RENDERER_VAR).ok();
    let options = eframe::NativeOptions {
        viewport: main_viewport(),
        renderer: renderer_for(wanted.as_deref()),
        ..Default::default()
    };
    // No usable OpenGL (a remote desktop session, a virtual machine, a missing graphics driver
    // often offer only OpenGL 1.1): start again on wgpu (Direct3D, Metal or Vulkan, with a
    // software fallback). A window system cannot be started twice in one process, so this is a
    // new process. Some OpenGL failures panic inside eframe instead of returning an error, so a
    // panic before the app has started falls back the same way. Set WIRINGCRAFT_RENDERER=wgpu
    // to skip the first try.
    let fallback = falls_back(wanted.as_deref());
    if fallback {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            default_hook(info);
            if !STARTED.load(Ordering::SeqCst) {
                // Leave at once: waiting here would keep this process's half-made window on
                // screen while the new one runs.
                restart_with_wgpu(&format!("{info}"), false);
            }
        }));
    }
    let result = eframe::run_native(
        "WiringCraft",
        options,
        Box::new(|cc| {
            STARTED.store(true, Ordering::SeqCst);
            Ok(Box::new(App::new(cc, dir)))
        }),
    );
    if let Err(e) = &result
        && fallback
        && !STARTED.load(Ordering::SeqCst)
    {
        restart_with_wgpu(&e.to_string(), true);
    }
    result
}

/// Environment variable that picks the graphics backend: `glow` (OpenGL, the default) or `wgpu`.
const RENDERER_VAR: &str = "WIRINGCRAFT_RENDERER";

/// Set once the window and its renderer are up; a failure after that is not a renderer problem.
static STARTED: AtomicBool = AtomicBool::new(false);

/// The backend to start with: `wgpu` when asked for, OpenGL otherwise.
fn renderer_for(wanted: Option<&str>) -> eframe::Renderer {
    match wanted {
        Some("wgpu") => eframe::Renderer::Wgpu,
        _ => eframe::Renderer::Glow,
    }
}

/// Whether a failure before the app starts should try again on wgpu: only for the default
/// choice, so a backend picked by hand, and the second try itself, never loop.
fn falls_back(wanted: Option<&str>) -> bool {
    wanted.is_none()
}

/// Run this program again on wgpu with the same arguments, then leave: with the new process's
/// exit code when `wait`, at once otherwise.
fn restart_with_wgpu(why: &str, wait: bool) {
    eprintln!("wiringcraft-app: OpenGL could not start ({why}); starting again with wgpu");
    let child = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .env(RENDERER_VAR, "wgpu")
            .spawn()
    });
    match child {
        Ok(mut c) if wait => std::process::exit(c.wait().ok().and_then(|s| s.code()).unwrap_or(1)),
        Ok(_) => std::process::exit(0),
        Err(err) => eprintln!("wiringcraft-app: could not start again with wgpu: {err}"),
    }
}

#[cfg(test)]
mod renderer_tests {
    use super::*;

    #[test]
    fn ctrl_shortcuts_wait_while_a_dialog_is_open() {
        let ctx = egui::Context::default();
        let press = |dialog_open: bool| {
            let raw = egui::RawInput {
                modifiers: Modifiers::COMMAND,
                events: vec![egui::Event::Key {
                    key: Key::N,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::COMMAND,
                }],
                ..Default::default()
            };
            let mut got = Vec::new();
            let _ = ctx.run(raw, |ctx| got = ctrl_shortcuts(ctx, dialog_open));
            got
        };
        assert!(press(true).is_empty(), "Ctrl+N does nothing under a dialog");
        assert!(matches!(press(false).as_slice(), [Action::AddPage]));
    }

    #[test]
    fn ctrl_shift_z_redoes_and_ctrl_z_undoes() {
        let ctx = egui::Context::default();
        let press = |modifiers: Modifiers, key: Key| {
            let raw = egui::RawInput {
                modifiers,
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            };
            let mut got = Vec::new();
            let _ = ctx.run(raw, |ctx| got = ctrl_shortcuts(ctx, false));
            got
        };
        let shift = Modifiers::COMMAND | Modifiers::SHIFT;
        assert_eq!(press(shift, Key::Z), [Action::Redo]);
        assert_eq!(press(Modifiers::COMMAND, Key::Z), [Action::Undo]);
        assert_eq!(press(Modifiers::COMMAND, Key::Y), [Action::Redo]);
        assert_eq!(press(shift, Key::N), [Action::NewProject]);
    }

    #[test]
    fn opengl_is_the_default_and_wgpu_can_be_picked() {
        assert_eq!(renderer_for(None), eframe::Renderer::Glow);
        assert_eq!(renderer_for(Some("glow")), eframe::Renderer::Glow);
        assert_eq!(renderer_for(Some("wgpu")), eframe::Renderer::Wgpu);
        assert_eq!(renderer_for(Some("other")), eframe::Renderer::Glow);
    }

    #[test]
    fn only_the_default_choice_falls_back_so_the_second_try_never_loops() {
        assert!(falls_back(None));
        assert!(!falls_back(Some("wgpu")), "the second try runs with wgpu set");
        assert!(!falls_back(Some("glow")), "OpenGL picked by hand is not overruled");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod library_tests {
    use super::*;

    #[test]
    fn wires_on_pins_a_new_symbol_version_drops_are_listed() {
        use wc_core::{Endpoint, Page, PlacedSymbol, Project, Wire};
        let mut p = Project::new("x");
        let mut page = Page::new("p1", "3", "Motor");
        page.symbols.push(PlacedSymbol {
            id: "s1".into(),
            symbol: "my.coil".into(),
            at: wc_core::geom::Point::new(0.0, 0.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(wc_core::DeviceTag::parse("-K1").unwrap()),
        });
        let pin = |p: &str| Endpoint::Pin {
            item: "s1".into(),
            pin: p.into(),
        };
        page.wires.push(Wire {
            id: "w1".into(),
            from: pin("A1"),
            to: pin("A2"),
            via: Vec::new(),
            number: None,
            potential: None,
            cable: None,
            route: Default::default(),
            spec: Default::default(),
        });
        p.pages.push(page);
        let mut ed = symbol_editor::SymbolEditor::new(Vec::new());
        ed.set_tool(symbol_editor::Tool::Pin);
        ed.pin_name = "A1".into();
        ed.click(wc_core::geom::Point::new(0.0, -5.0));
        assert_eq!(state::dangling_pins(&p, "my.coil", &ed.symbol), ["page 3 -K1:A2"]);
        assert!(state::dangling_pins(&p, "other", &ed.symbol).is_empty());
    }

    /// Makes `<root>/<rel>/symbols` and returns `<root>/<rel>` canonicalized.
    fn lib(root: &Path, rel: &str) -> PathBuf {
        std::fs::create_dir_all(root.join(rel).join("symbols")).unwrap();
        root.join(rel).canonicalize().unwrap()
    }

    #[test]
    fn the_library_is_found_in_each_install_layout() {
        let t = tempfile::tempdir().unwrap();
        let none = t.path().join("no-source");
        // Linux package: /usr/bin/wiringcraft-app with /usr/share/wiringcraft/library.
        let linux = t.path().join("usr");
        let want = lib(&linux, "share/wiringcraft/library");
        std::fs::create_dir_all(linux.join("bin")).unwrap();
        assert_eq!(find_library(Some(&linux.join("bin")), &none), Some(want));
        // macOS bundle: Contents/MacOS with Contents/Resources/library.
        let mac = t.path().join("WiringCraft.app/Contents");
        let want = lib(&mac, "Resources/library");
        std::fs::create_dir_all(mac.join("MacOS")).unwrap();
        assert_eq!(find_library(Some(&mac.join("MacOS")), &none), Some(want));
        // Nothing installed and no source checkout: no library.
        let empty = t.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(find_library(Some(&empty), &none), None);
    }

    #[test]
    fn a_library_next_to_the_program_wins() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("app");
        let beside = lib(&app, "library");
        let source = lib(t.path(), "source/library");
        assert_eq!(find_library(Some(&app), &source), Some(beside));
        assert_eq!(find_library(None, &source), Some(source));
    }
}

#[cfg(test)]
mod icon_tests {
    #[test]
    fn the_window_icon_is_built_in() {
        let icon = super::window_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }

    #[test]
    fn the_main_window_carries_the_icon() {
        let icon = super::main_viewport().icon;
        assert_eq!(icon.map(|i| (i.width, i.height)), Some((256, 256)));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod picker_tests {
    use super::*;

    #[test]
    fn a_picker_that_never_showed_says_so_instead_of_doing_nothing() {
        use std::time::Duration;
        let d = PathBuf::from("/x");
        assert_eq!(picker_outcome(Some(d.clone()), Duration::ZERO, &[]), Picked::Path(d));
        assert_eq!(picker_outcome(None, Duration::from_secs(2), &[]), Picked::Cancelled);
        assert!(matches!(
            picker_outcome(None, Duration::from_millis(5), &[]),
            Picked::Failed(e) if e.contains("desktop portal")
        ));
        // Typed text the dialogs refuse opens the picker in the projects folder.
        if cfg!(windows) {
            assert_eq!(picker_start("C:foo"), picker_start(""));
        }
    }

    #[test]
    fn a_picker_that_fails_slowly_is_reported_and_a_cancel_after_a_portal_failure_is_not() {
        use std::time::Duration;
        // What rfd logs when the portal times out and zenity is not there.
        applog::listen_to_picker();
        applog::take_picker_errors();
        log::error!(target: "rfd::backend::xdg_desktop_portal", "Failed to pick folder: Portal request timed out");
        log::error!(
            target: "rfd::backend::xdg_desktop_portal",
            "Failed to pick folder with zenity: No such file or directory (os error 2)"
        );
        log::error!(target: "eframe", "not from the picker");
        let errors = applog::take_picker_errors();
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(applog::take_picker_errors().is_empty(), "taken once");
        assert!(matches!(
            picker_outcome(None, Duration::from_secs(25), &errors),
            Picked::Failed(e) if e.contains("desktop portal")
        ));
        // The portal failed but zenity showed and the user cancelled it.
        let portal_only = &errors[..1];
        assert_eq!(
            picker_outcome(None, Duration::from_secs(3), portal_only),
            Picked::Cancelled
        );
        let d = PathBuf::from("/x");
        assert_eq!(
            picker_outcome(Some(d.clone()), Duration::from_secs(3), &errors),
            Picked::Path(d)
        );
    }

    #[test]
    fn the_picker_opens_where_the_dialog_would_put_the_project() {
        let nearest = |p: PathBuf| {
            p.ancestors()
                .find(|d| !d.as_os_str().is_empty() && d.is_dir())
                .map(Path::to_path_buf)
        };
        // A bare name is looked up under the projects folder, never the folder the app started
        // in, whether or not that name exists there.
        let name = "some-project-name";
        assert_eq!(picker_start(name), nearest(prefs::projects_dir().join(name)));
        assert_eq!(picker_start("  "), nearest(prefs::projects_dir()));
        // A full path opens at its nearest existing folder (inside a fresh, empty folder).
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("not-there").join("deeper");
        assert_eq!(
            picker_start(&deep.display().to_string()),
            Some(tmp.path().to_path_buf())
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod marking_tests {
    use super::*;

    #[test]
    fn terminal_strips_are_exported_for_the_wago_configurator() {
        let (_tmp, mut o) = state::tests::project();
        let msg = export_wago_configurator(&o);
        assert!(msg.starts_with("Nothing exported"), "{msg}");
        let path = o.dir.join("export").join("wago-configurator.csv");
        assert!(!path.exists());

        for (x, tag) in [(20.0, "-X1.2"), (40.0, "-X1.1"), (60.0, "-X2.1")] {
            let id = o.place(1, "wc.terminal", wc_core::geom::Point::new(x, 20.0)).unwrap();
            let s = o.project.pages[1].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(wc_core::DeviceTag::parse(tag).unwrap());
        }
        let msg = export_wago_configurator(&o);
        assert!(msg.contains("Exported 2 terminal strips (3 rows)"), "{msg}");
        let csv = std::fs::read_to_string(&path).unwrap();
        let rows: Vec<&str> = csv.lines().map(|l| l.split(';').next().unwrap()).collect();
        assert_eq!(rows, ["\u{feff}Index", "-X1/T1", "-X1/T2", "-X2/T1"]);
        assert!(csv.contains("-X1/T1;Terminal;-X1;1;1;"), "{csv}");
    }

    #[test]
    fn articles_chosen_in_the_wago_configurator_come_back_as_one_undo_step() {
        let (_tmp, mut o) = state::tests::project();
        for (x, tag) in [(20.0, "-X1.1"), (40.0, "-X1.2")] {
            let id = o.place(1, "wc.terminal", wc_core::geom::Point::new(x, 20.0)).unwrap();
            let s = o.project.pages[1].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(wc_core::DeviceTag::parse(tag).unwrap());
        }
        let x1 = wc_core::DeviceTag::parse("-X1").unwrap();
        o.project
            .meta
            .add_jumper(&x1, &["1".to_owned(), "2".to_owned()])
            .unwrap();
        export_wago_configurator(&o);
        let csv = std::fs::read_to_string(o.dir.join("export").join("wago-configurator.csv"))
            .unwrap()
            .replace("-X1/T1;Terminal;-X1;1;1;;;", "-X1/T1;Terminal;-X1;1;1;;EX-TB-2.5;")
            .replace("-X1/T2;Terminal;-X1;2;2;;;", "-X1/T2;Terminal;-X1;2;2;;NO-SUCH;")
            .replace("-X1/J1;Jumper;-X1;;;;;", "-X1/J1;Jumper;-X1;;;;EX-TB-2.5;");
        let before = o.undo_mark();
        let pid = "example.ex-tb-2-5";
        assert!(!o.project.lock.parts.contains_key(pid), "not pinned yet");
        let lock = o.project.lock.clone();
        let msg = o.import_wago_configured(csv.as_bytes()).unwrap();
        assert!(o.project.lock.parts.contains_key(pid), "taken and pinned");
        assert!(
            msg.starts_with(
                "WAGO configurator: 2 device(s) or jumper(s) changed, 0 row(s) already up to date, 1 not matched."
            ),
            "{msg}"
        );
        assert_eq!(o.project.meta.jumpers[0].part.as_deref(), Some("example.ex-tb-2-5"));
        assert!(msg.contains("NO-SUCH"), "{msg}");
        let x11 = wc_core::DeviceTag::parse("-X1.1").unwrap();
        assert_eq!(
            o.project.devices.get(&x11).and_then(|d| d.part.as_deref()),
            Some("example.ex-tb-2-5")
        );
        assert!(o.parts.contains_key("example.ex-tb-2-5"), "loaded");
        assert_eq!(o.undo_mark(), before + 1);
        assert!(o.undo());
        assert!(o.project.devices.get(&x11).and_then(|d| d.part.as_deref()).is_none());
        assert!(
            o.project.meta.jumpers[0].part.is_none(),
            "the jumper in the same undo step"
        );
        assert_eq!(o.project.lock, lock, "and the pin");
        o.save().unwrap();
        assert!(!wc_core::store::load(&o.dir).unwrap().lock.parts.contains_key(pid));
        assert!(o.redo());
        assert!(o.project.lock.parts.contains_key(pid), "Redo brings the pin back");
        assert_eq!(o.project.devices.get(&x11).and_then(|d| d.part.as_deref()), Some(pid));
        // Undone again, then given by hand: the copy left behind is pinned again.
        assert!(o.undo());
        o.set_device_part(&x11, Some(pid)).unwrap();
        assert!(o.project.lock.parts.contains_key(pid));
        assert!(o.undo());
        assert_eq!(o.project.lock, lock, "setting a part is one undo step with its pin");
        assert!(o.import_wago_configured(b"Article\n1\n").is_err());
    }

    #[test]
    fn a_wago_jumper_row_is_matched_to_the_jumper_after_a_terminal_change_still_in_progress() {
        let (_tmp, mut o) = state::tests::project();
        let mut ids = Vec::new();
        for (x, tag) in [(20.0, "-X1.1"), (40.0, "-X1.2")] {
            let id = o.place(1, "wc.terminal", wc_core::geom::Point::new(x, 20.0)).unwrap();
            let s = o.project.pages[1].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(wc_core::DeviceTag::parse(tag).unwrap());
            ids.push(id);
        }
        let x1 = wc_core::DeviceTag::parse("-X1").unwrap();
        o.project
            .meta
            .add_jumper(&x1, &["1".to_owned(), "2".to_owned()])
            .unwrap();
        export_wago_configurator(&o);
        let csv = std::fs::read_to_string(o.dir.join("export").join("wago-configurator.csv"))
            .unwrap()
            .replace("-X1/J1;Jumper;-X1;;;;;", "-X1/J1;Jumper;-X1;;;;EX-TB-2.5;");
        assert!(csv.contains("EX-TB-2.5"), "{csv}");

        // Terminal 2 becomes 5 after the list was exported, and the jumper has not followed yet.
        o.checkpoint();
        let s = o.project.pages[1].symbols.iter_mut().find(|s| s.id == ids[1]).unwrap();
        s.tag = Some(wc_core::DeviceTag::parse("-X1.5").unwrap());
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "2"], "not followed yet");

        let mark = o.undo_mark();
        let msg = o.import_wago_configured(csv.as_bytes()).unwrap();
        assert_eq!(
            o.undo_mark(),
            mark,
            "nothing taken, no undo step: Undo goes to the terminal edit"
        );
        assert!(msg.contains("0 device(s) or jumper(s) changed"), "{msg}");
        assert!(msg.contains("1 not matched"), "{msg}");
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "5"], "followed");
        assert!(
            o.project.meta.jumpers[0].part.is_none(),
            "the row stood for terminals 1 and 2"
        );
    }

    #[test]
    fn one_strips_markers_are_exported_from_its_menu_with_the_chosen_lines() {
        let (_tmp, mut o) = state::tests::project();
        let a = o
            .place(1, "wc.terminal", wc_core::geom::Point::new(20.0, 20.0))
            .unwrap();
        let b = o
            .place(1, "wc.terminal", wc_core::geom::Point::new(40.0, 20.0))
            .unwrap();
        let x1 = wc_core::DeviceTag::parse("-X1").unwrap();
        o.project.devices.entry(x1).or_default().description = "Field terminals".into();
        for (id, tag) in [(&a, "-X1.1"), (&b, "-X2.1")] {
            let s = o.project.pages[1].symbols.iter_mut().find(|s| s.id == *id).unwrap();
            s.tag = Some(wc_core::DeviceTag::parse(tag).unwrap());
        }

        // The devices list offers the strip of a terminal, and nothing for other devices.
        let index = o.project.tag_index();
        let uses = |t: &str| index.get(&wc_core::DeviceTag::parse(t).unwrap()).cloned().unwrap();
        assert_eq!(
            panels::terminal_strip_of(&o, &wc_core::DeviceTag::parse("-X1.1").unwrap(), &uses("-X1.1")),
            Some("-X1".into())
        );
        let c = o.place(1, "wc.coil", wc_core::geom::Point::new(60.0, 20.0)).unwrap();
        let coil = o.project.pages[1]
            .symbols
            .iter()
            .find(|s| s.id == c)
            .unwrap()
            .tag
            .clone()
            .unwrap();
        let index = o.project.tag_index();
        assert_eq!(panels::terminal_strip_of(&o, &coil, &index[&coil]), None);

        // From that menu the dialog opens with only the strip's terminals.
        let Dialog::WagoMarking {
            kinds, strip, strips, ..
        } = wago_marking_dialog(&o, Some("-X1".into()))
        else {
            unreachable!("not the marking dialog");
        };
        assert_eq!(kinds, [true, false, false, false]);
        assert_eq!(strip.as_deref(), Some("-X1"));
        assert_eq!(strips, ["-X1", "-X2"]);

        let options = wc_report::MarkingOptions {
            kinds: vec![wc_report::MarkerKind::Terminals],
            lines: vec![wc_report::LineField::Marker, wc_report::LineField::Description],
            language: wc_report::Language::De,
            strip,
            level_separator: wc_report::LEVEL_SEPARATOR.to_owned(),
        };
        let msg = export_wago_marking(&o, &options);
        assert!(
            msg.contains("Exported 1 markers") && msg.contains("Line columns"),
            "{msg}"
        );
        let csv = std::fs::read_to_string(o.dir.join("export").join("wago-marking--X1.csv")).unwrap();
        assert_eq!(
            csv,
            "\u{feff}Typ;Gruppe;Zeile 1;Zeile 2;Anzahl\r\nKlemme;-X1;1;Field terminals;1\r\n"
        );
        assert_eq!(file_stem("=A+C1/X1"), "=A+C1_X1");
        use wc_report::LineField::{Description, Marker};
        assert_eq!(chosen_lines(&[Some(Marker), None, Some(Description)]), [Marker]);
        assert_eq!(
            chosen_lines(&[Some(Description), Some(Marker), None]),
            [Description, Marker]
        );

        // A strip drawn only on an overview page has nothing to export: no file is written.
        o.project.pages[1].overview = true;
        std::fs::remove_file(o.dir.join("export").join("wago-marking--X1.csv")).unwrap();
        let msg = export_wago_marking(&o, &options);
        assert!(msg.starts_with("Nothing exported") && msg.contains("-X1"), "{msg}");
        assert!(!o.dir.join("export").join("wago-marking--X1.csv").exists());
    }
}

#[cfg(test)]
mod page_size_tests {
    use super::*;

    #[test]
    fn a_custom_sheet_starts_from_the_size_shown() {
        assert_eq!(
            custom_from(PageSize::A4Portrait),
            PageSize::Custom {
                width: 210,
                height: 297
            }
        );
        assert_eq!(
            custom_from(PageSize::A3),
            PageSize::Custom {
                width: 420,
                height: 297
            }
        );
    }
}

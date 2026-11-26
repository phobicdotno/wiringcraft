//! WiringCraft desktop application.
//!
//! Usage: `wiringcraft-app [project-folder]`. Without a folder the last project is reopened.

mod actions;
mod agent;
mod agent_panel;
mod applog;
mod black_box_tool;
mod canvas;
mod chrome;
mod edit;
mod feedback;
mod find_window;
mod icons;
mod import_symbols;
mod imports;
mod layers;
mod live_check;
mod macro_form;
mod panel_view;
mod panels;
mod prefs;
mod racks_form;
mod report_layout_form;
mod rules_form;
mod screenshot;
mod share;
mod state;
mod strip_editor;
mod symbol_editor;
mod template_form;
mod theme;
mod translations_form;

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
        /// A base project to start from: a project folder or a git repository URL. Empty for
        /// a blank project.
        base: String,
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
        /// The project's revisions, edited in place; left alone on OK while unchanged, as
        /// the locations are.
        revisions: Vec<state::RevisionRow>,
        loaded_revisions: Vec<state::RevisionRow>,
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
    /// The PLC racks and their cards, shown in the project as they are typed (see `Template`).
    Racks {
        draft: Vec<wc_core::plc_rack::PlcRack>,
        since: usize,
        before: Vec<wc_core::plc_rack::PlcRack>,
        seen: u64,
        /// The overview's warnings and the project revision they were made for, so they are
        /// worked out again only after a change.
        warnings: Option<(u64, Vec<String>)>,
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
    /// The project's texts with their translations, saved to `translations.toml`.
    Translations(translations_form::Form),
    /// The language to export the PDF in: an index into `langs` (the dictionary's languages,
    /// the source first), and a second one shown with it (`second - 1`; 0 for none).
    PdfLanguage {
        langs: Vec<String>,
        first: usize,
        second: usize,
    },
    /// Project check findings.
    Check(Vec<wc_report::Finding>),
    /// The project's check rules, edited as rows; `loaded` is how the project had them when
    /// the dialog opened.
    Rules {
        rows: Vec<rules_form::RuleRow>,
        loaded: Vec<wc_core::check_rule::CheckRule>,
        /// The folder of the project the rules are of: another project opened meanwhile
        /// does not get them.
        dir: PathBuf,
    },
    /// Reports (parts, devices, wires, nets, terminals, cables, PLC I/O, contents, locations) as tables.
    Reports(wc_report::Kind),
    /// One report's layout, edited next to a preview of the report in it.
    ReportLayout {
        form: Box<report_layout_form::Form>,
        /// The folder of the project the layout is of: another project opened meanwhile
        /// does not get it.
        dir: PathBuf,
    },
    /// Write a new text at this point on the current page.
    Text(wc_core::geom::Point, String, bool),
    /// Pin a review comment at this point on the current page.
    Comment(wc_core::geom::Point, String, bool),
    /// Name a connection point of the black box being drawn, at this edge and offset.
    BoxPin {
        side: wc_core::BoxSide,
        offset: f64,
        name: String,
        focused: bool,
        error: Option<String>,
    },
    /// Tag the black box being drawn and put it on the page.
    BoxTag {
        tag: String,
        focused: bool,
        error: Option<String>,
    },
    /// The recorded versions of one page, newest first.
    PageHistory(String, Vec<wc_vcs::PageVersion>),
    Commit(String),
    NewBranch(String),
    History(Vec<wc_vcs::subway::Row>),
    Library(Vec<wc_library::ItemStatus>),
    Message(String),
    /// Save refused: this project file waits to be written again after a failed Undo or Redo,
    /// but was changed outside WiringCraft since (#662). Asks which version to keep.
    ChangedOutside(PathBuf),
    /// A base project being fetched from this git URL for a new project ([`App::fetch`]).
    Fetching(String),
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
    /// File > Import without a file picker: the file (and, for parts with no folder chosen,
    /// the library folder) typed, and why the picker could not be used.
    ImportPath {
        kind: imports::Kind,
        target: Option<import_symbols::ImportTarget>,
        file: prefs::FolderField,
        lib: prefs::FolderField,
        error: Option<String>,
    },
    /// File > Import, after the file is picked: the options the command line has for the
    /// format, with its defaults filled in.
    ImportOptions {
        file: PathBuf,
        /// The library folder for parts, chosen before the file.
        lib: Option<PathBuf>,
        target: Option<import_symbols::ImportTarget>,
        form: Box<imports::Form>,
        error: Option<String>,
    },
    /// Attach footprint drawing / Attach 3D model without a file picker: the file typed, and
    /// why the picker could not be used or the typed path was refused.
    AttachPath {
        kind: panels::Attach,
        part: String,
        file: prefs::FolderField,
        error: Option<String>,
    },
    /// Parts import: the library folder to write into, then the file.
    ImportParts {
        kind: imports::Kind,
        targets: Vec<import_symbols::ImportTarget>,
        /// An index into `targets`; one past the end is another folder, picked after the file.
        chosen: usize,
    },
    /// A project macro's placeholders and value sets (Insert macro, Placeholders…).
    MacroEdit {
        form: Box<macro_form::Form>,
        error: Option<String>,
    },
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

/// Whether a dialog belongs to the open project, so it is closed when another project is
/// opened. Only the dialogs that do not refer to it stay: opening or making a project, a
/// message, a fetch in progress and the problem report.
fn bound_to_project(d: &Dialog) -> bool {
    !matches!(
        d,
        Dialog::OpenPath(..)
            | Dialog::NewProject { .. }
            | Dialog::Message(_)
            | Dialog::Fetching(_)
            | Dialog::Report { .. }
    )
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
    /// The project folder open when the waiting picker was asked for: it is for that project.
    pending_for: Option<PathBuf>,
    /// The terminal strip editor, while open.
    strip_editor: Option<strip_editor::StripEditor>,
    /// The layer list window is open.
    show_layers: bool,
    /// The project check that runs after each edit (Check while drawing).
    live: live_check::LiveCheck,
    /// A base project being fetched from a git URL for a new project, off the window thread.
    fetch: Option<BaseFetch>,
    /// File > Import running on a worker thread ([`App::poll_import`]).
    importing: Option<Importing>,
}

/// File > Import running on a worker thread, and what to do with the project when it ends.
struct Importing {
    job: imports::Job,
    kind: imports::Kind,
    file: PathBuf,
    /// The library folder parts go into.
    lib: Option<PathBuf>,
    /// The library folder chosen in the parts dialog, declared in the project when it is new.
    target: Option<import_symbols::ImportTarget>,
}

/// Said when something waits for an import that is running.
const IMPORT_RUNNING: &str = "An import is running; wait until it is done.";

/// The actions that would change or select on the schematic under the symbol editor: held back
/// while it is open, from the keys, the menu, the toolbar and the assistant alike.
fn held_by_symbol_editor(a: Action) -> bool {
    matches!(
        a,
        Action::Cut
            | Action::Copy
            | Action::Paste
            | Action::Duplicate
            | Action::Rotate
            | Action::Mirror
            | Action::SelectAll
    )
}

/// Why `what` (an action's label, or what an assistant tool does) was refused while the symbol
/// editor is open.
fn symbol_editor_refusal(what: &str) -> String {
    format!("{what} was not done: close the symbol editor first.")
}

/// The actions that do not change the project, its folder or which project is open: the only
/// ones carried out while an import runs.
fn allowed_while_importing(a: Action) -> bool {
    matches!(
        a,
        Action::ZoomIn
            | Action::ZoomOut
            | Action::ZoomFit
            | Action::ToggleGrid
            | Action::ToggleTheme
            | Action::ToggleChrome
            | Action::Layers
            | Action::Assistant
            | Action::RequestFunction
            | Action::ReportMissing
    )
}

/// The view controls shown in the import window: while it is up the window takes every click,
/// so the toolbar's own are out of reach. All are [`allowed_while_importing`].
const IMPORT_VIEW_CONTROLS: &[Action] = &[
    Action::ZoomIn,
    Action::ZoomOut,
    Action::ZoomFit,
    Action::ToggleGrid,
    Action::ToggleTheme,
    Action::Layers,
];

/// Make a new project's folder a repository with its first version.
fn record_new_project(dir: &std::path::Path) -> Result<(), wc_vcs::Error> {
    let git = wc_vcs::Git::new(dir);
    git.init()?;
    git.commit("New project").map(|_| ())
}

/// A new project's base project being fetched from a git repository on a thread of its own
/// ([`App::start_new_project`]), so a slow server does not hold the window up.
struct BaseFetch {
    /// Where the new project goes.
    dir: PathBuf,
    /// The repository it comes from, shown while it is fetched.
    source: String,
    /// The project that was open when the fetch started: the new one replaces only that one.
    was_open: Option<PathBuf>,
    /// The project is made and waits to be opened: Cancel no longer applies.
    made: bool,
    /// Set to stop the fetch: nothing is made then.
    cancel: std::sync::Arc<AtomicBool>,
    /// The outcome: made, why not, or `None` when it was cancelled.
    done: std::sync::mpsc::Receiver<Result<(), Option<String>>>,
}

/// A file or folder picker an action needs before it can be carried out. It is shown after the
/// frame's actions have run, so an action the assistant runs returns at once and the picker
/// waits for the person instead.
enum Pending {
    Open,
    SaveAs,
    Export(Export),
    Marking(wc_report::MarkingOptions),
    /// Reports > Save as CSV; `true` for Excel: `;`-separated with a UTF-8 byte order mark.
    ReportCsv(wc_report::Kind, bool),
    /// File > Import; for parts, the library folder picked in the dialog (none: ask with the
    /// folder picker).
    Import(imports::Kind, Option<import_symbols::ImportTarget>),
    /// The WAGO configurator's article list to read back.
    WagoList,
    /// The panel layout's footprint drawing or 3D model for this part.
    Attach(panels::Attach, String),
}

/// Queue the picker `p` for the project in `dir` unless another waits: false when one does.
fn queue_pending(
    pending: &mut Option<Pending>,
    pending_for: &mut Option<PathBuf>,
    p: Pending,
    dir: Option<PathBuf>,
) -> bool {
    if pending.is_some() {
        return false;
    }
    *pending = Some(p);
    *pending_for = dir;
    true
}

/// Whether a picker queued while `asked_for` was open may still be shown with `now` open:
/// Open project always, the others only for the same project.
fn pending_applies(p: &Pending, asked_for: Option<&Path>, now: Option<&Path>) -> bool {
    matches!(p, Pending::Open) || asked_for == now
}

/// The whole-project exports that ask where to go.
#[derive(Debug, Clone)]
enum Export {
    /// The PDF, with its texts in these languages (none: as drawn).
    Pdf(Vec<String>),
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
    /// The layer list window is open.
    show_layers: bool,
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
            Action::DimensionTool => self.ed.tool == Tool::Dimension,
            Action::CloudTool => self.ed.tool == Tool::Cloud,
            Action::RevisionTableTool => self.ed.tool == Tool::RevisionTable,
            Action::BlackBoxTool => self.ed.tool == Tool::BlackBox,
            Action::ToggleGrid => self.prefs.show_grid,
            Action::Layers => self.show_layers,
            Action::ToggleAutoWire => self.prefs.auto_wire,
            Action::LiveCheck => self.prefs.live_check,
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
            | Action::Layers
            | Action::ToggleAutoWire
            | Action::LiveCheck
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
            Action::PrevPage => has && self.ed.page > 0,
            Action::NextPage => self.open.is_some_and(|o| self.ed.page + 1 < o.project.pages.len()),
            Action::Save => self.open.is_some_and(|o| o.dirty),
            Action::Undo => self.open.is_some_and(Open::can_undo),
            Action::Redo => self.open.is_some_and(Open::can_redo),
            Action::Delete if self.ed.panel.is_some() => has && self.ed.panel_pick.is_some(),
            Action::Delete | Action::Copy | Action::Cut | Action::Duplicate | Action::SaveMacro => {
                has && !self.ed.selection.is_empty()
            }
            // Only a device turns; a picked rail or duct does not.
            Action::Rotate if self.ed.panel.is_some() => {
                has && self.open.as_ref().is_some_and(|o| {
                    let picked = self.ed.panel.zip(self.ed.panel_pick);
                    let item = picked.and_then(|(pi, i)| o.project.panels.get(pi)?.items.get(i));
                    matches!(item, Some(wc_core::panel::PanelItem::Device { .. }))
                })
            }
            Action::Mirror if self.ed.panel.is_some() => false,
            Action::Rotate | Action::Mirror => {
                has && self.ed.selection.iter().any(|s| matches!(s, Selection::Symbol(_)))
            }
            Action::Paste => has && !self.clipboard.is_empty(),
            _ => has,
        }
    }
}

impl App {
    /// Why the window does not save while a command of the assistant runs.
    pub(crate) const SAVE_WAITS: &str = "Saving waits until the assistant's command has finished.";

    /// Why the Translations dialog does not save into a folder that is not the open project's.
    pub(crate) const TRANSLATIONS_ELSEWHERE: &str = "Not saved: this dialog was opened for another project folder than the one that is open now. Close it and open Translations again.";

    /// OK in the Translations dialog: save `form`'s dictionary into the open project, as one
    /// undo step however many cells changed (#589). A guest in a shared session is refused as
    /// the assistant's `translations` tool refuses, and nothing is written: the session does
    /// not carry the file. A form that is not for the open project is refused too, and nothing
    /// is written: its save would be in no undo history (after Save as the form follows the
    /// project into the copy, [`App::save_copy`]). While a command of the assistant runs
    /// (`command_running`) nothing is saved, as with Save: what changes on disk meanwhile is
    /// taken as the command's change.
    pub(crate) fn save_translations(
        open: Option<&mut Open>,
        share: &share::Share,
        command_running: bool,
        form: &translations_form::Form,
    ) -> Result<String, String> {
        if command_running {
            return Err(App::SAVE_WAITS.to_owned());
        }
        if share.active() && !share.is_host() {
            return Err(Open::TRANSLATIONS_GUEST.to_owned());
        }
        match open {
            Some(open) if form.is_for(&open.dir) => open.translations_change(|_| form.save()),
            _ => Err(App::TRANSLATIONS_ELSEWHERE.to_owned()),
        }
    }

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
            || matches!(
                self.dialog,
                Some(Dialog::Cover { .. } | Dialog::Template { .. } | Dialog::Racks { .. })
            )
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
        // The panel shown and its picked item, found again after the sync: a merge replaces
        // whole panels, so their places in the lists can change.
        let panel_before = self.open.as_ref().and_then(|o| panel_pick_before(&self.ed, &o.project));
        // The shown page's graphics, so a selection follows its items when a merge moves them.
        let graphics_before = self
            .open
            .as_ref()
            .and_then(|o| o.project.pages.get(self.ed.page))
            .map(|p| (p.id.clone(), p.graphics.clone(), p.clouds.clone()));
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
            // The fresh editor keeps the user's hidden and locked layers.
            self.apply_layers();
            self.panels.reset();
            self.dialog = None;
            self.prefs.remember(joined.dir.clone());
            self.prefs.save();
            self.open = Some(joined);
            // The id belonged to the project left behind.
            shown = None;
        } else if let (Some((id, pick)), Some(o)) = (&panel_before, &self.open) {
            follow_panel_pick(&mut self.ed, &o.project, id, pick.as_ref());
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
        let waiting = self.share.unanswered_edits() > 0;
        // A refused change of the page ends the wait: what comes back there is not the paste
        // (the host may have added an equal drawing in the meantime).
        let refused = self.share.take_refused_pages();
        if self
            .ed
            .waiting_graphics
            .as_ref()
            .is_some_and(|w| refused.contains(&w.page))
        {
            self.ed.waiting_graphics = None;
        }
        // A selection made since the last sync is the user's, and ends the wait (looked at
        // before this sync moves selected drawings to where they are now).
        if self
            .ed
            .waiting_graphics
            .as_ref()
            .is_some_and(|w| w.selection != self.ed.selection)
        {
            self.ed.waiting_graphics = None;
        }
        if let Some((id, old, old_clouds)) = &graphics_before {
            match self.open.as_ref().and_then(|o| o.project.pages.get(self.ed.page)) {
                Some(p) if &p.id == id => {
                    edit::remap_clouds(&mut self.ed.selection, old_clouds, &p.clouds);
                    let before = self.ed.selection.clone();
                    edit::remap_graphics(&mut self.ed.selection, old, &p.graphics);
                    // A change of this person's that the host has not answered can show the
                    // page without what it added (a whole page waits for the host): what was
                    // selected of it is selected again once it is back.
                    let lost = edit::lost_graphics(&before, old, &p.graphics);
                    if waiting && !lost.is_empty() {
                        let w = self.ed.waiting_graphics.get_or_insert_with(|| edit::WaitingGraphics {
                            page: id.clone(),
                            items: Vec::new(),
                            selection: Vec::new(),
                        });
                        if &w.page == id {
                            w.items.extend(lost);
                        }
                    }
                }
                // The page was removed: what was selected on it is not on the page shown now.
                _ => self.ed.selection.clear(),
            }
        }
        if let Some(mut w) = self.ed.waiting_graphics.take()
            && let Some(p) = self.open.as_ref().and_then(|o| o.project.pages.get(self.ed.page))
            && p.id == w.page
        {
            edit::restore_graphics(&mut w.items, &mut self.ed.selection, &p.graphics);
            // A drawing back on a layer hidden or locked meanwhile is not selected again.
            let layers = &self.ed.layers;
            self.ed.selection.retain(|s| layers.may_select(s));
            // Still away with every change answered: the host refused it.
            if !w.items.is_empty() && waiting {
                w.selection = self.ed.selection.clone();
                self.ed.waiting_graphics = Some(w);
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
            pending_for: None,
            show_layers: false,
            strip_editor: None,
            live: Default::default(),
            fetch: None,
            importing: None,
        };
        let dir = dir.or_else(|| prefs.recent.first().cloned());
        if let Some(d) = dir {
            app.open_dir(&d, &mut prefs);
        }
        app.prefs = prefs;
        app
    }

    fn open_dir(&mut self, dir: &std::path::Path, prefs: &mut Prefs) {
        // The import writes to the open project's folder and is taken into it when it ends.
        if self.importing.is_some() {
            self.dialog = Some(Dialog::Message(format!("The project was not opened: {IMPORT_RUNNING}")));
            return;
        }
        let dir = &match prefs::absolute(dir) {
            Ok(d) => d,
            Err(e) => {
                self.dialog = Some(Dialog::Message(format!("Could not open the project: {e}")));
                return;
            }
        };
        match Open::load(dir) {
            Ok(o) => {
                // A form of the old project (Delete page, Save as, properties...) must not act
                // on the new one.
                if self.dialog.as_ref().is_some_and(bound_to_project) {
                    self.dialog = None;
                }
                self.ed = Editor::default();
                // The fresh editor keeps the user's hidden and locked layers (`prefs` is the
                // user's list here: `self.prefs` may be taken out while this runs).
                self.ed.layers = prefs.layers.clone();
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
        let (d, links) = self.save_copy(d)?;
        let mut m = format!("Saved as {}; you are working in the copy now.", d.display());
        if !links.is_empty() {
            // Named, so a file the project reaches through a link can be put in the copy by hand.
            let shown: Vec<String> = links.iter().take(5).map(|l| l.display().to_string()).collect();
            let more = links.len().saturating_sub(shown.len());
            m.push_str(&format!(
                " {} link(s) in the project folder were not copied: {}{}.",
                links.len(),
                shown.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            ));
        }
        self.ed.message = m;
        self.prefs.remember(d);
        self.prefs.save();
        Ok(())
    }

    /// The project's part of [`App::save_as_to`]: copy the open project into `d` and work on
    /// there. Returns the copy's folder and the links left out. A Translations dialog open for
    /// the project goes on in the copy, which holds the same `translations.toml`: its OK then
    /// saves there, as an undo step of the project that is open, not into the original.
    pub(crate) fn save_copy(&mut self, d: &Path) -> Result<(PathBuf, Vec<PathBuf>), String> {
        if let Some(why) = self.save_as_refused() {
            return Err(why.into());
        }
        let Some(open) = self.open.as_mut() else {
            return Err("No project is open.".into());
        };
        let from = open.dir.clone();
        let saved = open.save_as(d).map_err(|e| format!("Save as failed:\n{e}"))?;
        if let Some(Dialog::Translations(form)) = self.dialog.as_mut()
            && form.is_for(&from)
        {
            form.dir = open.dir.clone();
        }
        Ok(saved)
    }

    /// Why Save as cannot be done now, checked again when its dialog would open: the assistant
    /// can start a command or a session can start between the action and its dialog.
    fn save_as_refused(&self) -> Option<&'static str> {
        if self.importing.is_some() {
            Some(IMPORT_RUNNING)
        } else if self.share.active() {
            Some(
                "Save as is not available in a shared session: it would end the session. Stop sharing (or leave) first.",
            )
        } else if self.assistant.command_running() {
            Some("Saving waits until the assistant's command has finished.")
        } else {
            None
        }
    }

    /// Show the picker an action asked for, then carry the action out with what was picked.
    fn ask(&mut self, p: Pending) {
        if let (Pending::SaveAs, Some(why)) = (&p, self.save_as_refused()) {
            self.ed.message = why.into();
            return;
        }
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
                Picked::Failed(e) | Picked::Unsure(e) => {
                    self.dialog = Some(Dialog::OpenPath(prefs::FolderField::typed(start), Some(e)));
                }
                Picked::Cancelled => {}
            }
            return;
        }
        if let Pending::Import(kind, target) = p {
            self.import(kind, target);
            return;
        }
        let Some(open) = self.open.as_mut() else { return };
        match p {
            Pending::Open | Pending::Import(..) => {}
            Pending::Attach(kind, part) => match attach_target(kind, &part, &open.dir, &pick_open) {
                Some(Ok(file)) => self.ed.message = kind.apply(open, &part, &file),
                Some(Err(form)) => self.dialog = Some(form),
                None => {}
            },
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
                    Picked::Failed(e) | Picked::Unsure(e) => {
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
                    Picked::Failed(e) => {
                        // No folder field here: the picker's "type the folder" hint does not apply.
                        let e = e.strip_suffix(TYPE_INSTEAD).unwrap_or(&e);
                        let note = format!("{e} So they go to the usual place, {}.\n\n", default.display());
                        (default, note)
                    }
                    // Perhaps a quick Cancel: nothing is written.
                    Picked::Unsure(e) => {
                        let e = e.strip_suffix(TYPE_INSTEAD).unwrap_or(&e);
                        self.dialog = Some(Dialog::Message(format!("{e} Nothing was exported.")));
                        return;
                    }
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
                if let Some(msg) = export_whole(open, kind, &pick_save) {
                    self.dialog = Some(Dialog::Message(msg));
                }
            }
            Pending::Marking(options) => self.dialog = Some(export_markers(open, &options, &pick_save)),
            Pending::WagoList => {
                let file = match pick_open(
                    "WAGO configurator list with articles",
                    &open.dir.join("export"),
                    ("CSV list", &["csv", "txt"]),
                ) {
                    Picked::Path(f) => f,
                    Picked::Failed(e) | Picked::Unsure(e) => {
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
            Pending::ReportCsv(kind, excel) => {
                let default = open.dir.join("export").join(format!("{}.csv", kind.name()));
                let title = if excel {
                    "Save report as CSV for Excel"
                } else {
                    "Save report as CSV"
                };
                let (path, note) = match export_target(title, &default, ("CSV", &["csv"]), &pick_save) {
                    Ok(target) => target,
                    Err(why) => {
                        if let Some(why) = why {
                            self.dialog = Some(Dialog::Message(why));
                        }
                        return;
                    }
                };
                let inputs = wc_report::Inputs {
                    project: &open.project,
                    symbols: &open.symbols,
                    parts: &open.parts,
                };
                let table = wc_report::build(kind, &inputs);
                self.ed.message = match write_creating(&path, report_csv(&table, excel)) {
                    Ok(()) => format!("{}Saved {}", note.replace("\n\n", " "), path.display()),
                    Err(e) => format!("Could not save {}: {e}", path.display()),
                };
            }
        }
    }

    /// File > Import: pick the file (and, for parts, the library folder), then run the import
    /// the CLI runs and show what it said.
    fn import(&mut self, kind: imports::Kind, target: Option<import_symbols::ImportTarget>) {
        if let Some(why) = self.import_refused() {
            self.dialog = Some(Dialog::Message(why.to_owned()));
            return;
        }
        let Some(open) = self.open.as_ref() else { return };
        let typed = |file: String, why: String| Dialog::ImportPath {
            kind,
            target: target.clone(),
            file: prefs::FolderField::typed(file),
            lib: prefs::FolderField::default(),
            error: Some(why),
        };
        let file = match pick_open(kind.title(), &open.dir, kind.filter()) {
            Picked::Path(f) => f,
            // Without a picker (or perhaps one that never showed) the path can still be typed.
            Picked::Failed(e) | Picked::Unsure(e) => {
                self.dialog = Some(typed(String::new(), e));
                return;
            }
            Picked::Cancelled => return,
        };
        let lib = if let Some(t) = target.as_ref().filter(|_| kind.needs_library()) {
            Some(t.root.clone())
        } else if kind.needs_library() {
            let start = open.dir.display().to_string();
            match pick_folder("Library folder for the imported parts", &start) {
                Picked::Path(d) => Some(d),
                Picked::Failed(e) | Picked::Unsure(e) => {
                    self.dialog = Some(typed(file.display().to_string(), e));
                    return;
                }
                Picked::Cancelled => return,
            }
        } else {
            None
        };
        self.import_picked(kind, &file, lib.as_deref(), target);
    }

    /// The file is chosen: ask for the options the format has ([`imports::Kind::options`]), or
    /// start at once when it has none.
    fn import_picked(
        &mut self,
        kind: imports::Kind,
        file: &Path,
        lib: Option<&Path>,
        target: Option<import_symbols::ImportTarget>,
    ) {
        if kind.options().is_empty() {
            self.import_from(kind, file, lib, target, &imports::Options::default());
            return;
        }
        let Some(open) = self.open.as_ref() else { return };
        let mut form = imports::Form::new(open, kind, file);
        // The library folders a parts import offers; without the project's library list only
        // another folder can be chosen, and the dialog says why.
        let mut error = None;
        if form.has(imports::Opt::Library) {
            match wc_library::Library::open(&open.dir, &open.project) {
                Ok(library) => form.libraries = library_targets(&library, open),
                Err(e) => {
                    error = Some(format!(
                        "The project's library folders could not be listed: {e}. Another folder can still be chosen."
                    ));
                }
            }
        }
        self.dialog = Some(Dialog::ImportOptions {
            file: file.to_path_buf(),
            lib: lib.map(Path::to_path_buf),
            target,
            form: Box::new(form),
            error,
        });
    }

    /// Why an import cannot run now: it writes to the project folder, which a shared session or
    /// a running assistant command owns.
    fn import_refused(&self) -> Option<&'static str> {
        if self.importing.is_some() {
            Some(IMPORT_RUNNING)
        } else if self.assistant.busy() {
            // Its turn's undo step would take the import's in with it.
            Some("The assistant is working on the project; import when it is done.")
        } else if self.share.active() {
            Some("Imports are not available in a shared session: leave the session, import, and share again.")
        } else if self.assistant.command_running() {
            Some("The assistant is running a command on the project; import when it is done.")
        } else {
            None
        }
    }

    /// Start the import of `file` (parts into `lib`) with `options` on a worker thread; [`App::poll_import`]
    /// shows what it says while it runs and takes the result in when it ends. `target` is the
    /// library folder chosen in the parts dialog, declared in the project when it is new.
    fn import_from(
        &mut self,
        kind: imports::Kind,
        file: &Path,
        lib: Option<&Path>,
        target: Option<import_symbols::ImportTarget>,
        options: &imports::Options,
    ) {
        // Checked again here: the typed path form stays open while a session or a command starts.
        if let Some(why) = self.import_refused() {
            self.dialog = Some(Dialog::Message(why.to_owned()));
            return;
        }
        let Some(open) = self.open.as_mut() else { return };
        applog::info(&format!("import {kind:?} {}", file.display()));
        let job = imports::start(open, kind, file, lib, self.ed.page, options);
        // Nothing half done is left: a tool's preview, a wire being drawn.
        self.ed.wire = None;
        self.ed.shape_points.clear();
        self.dialog = None;
        self.importing = Some(Importing {
            job,
            kind,
            file: file.to_path_buf(),
            lib: lib.map(Path::to_path_buf),
            target,
        });
    }

    /// The import that runs: take its result in once the worker has ended (one Undo step, and
    /// the message of what it said), else ask for the next frame to look again.
    fn poll_import(&mut self, ctx: &egui::Context) {
        let Some(running) = self.importing.as_ref() else { return };
        let Some((r, log)) = running.job.poll() else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        };
        let Some(running) = self.importing.take() else { return };
        self.end_import(running, r, &log);
    }

    /// Take an ended import's result into the project it was started on.
    fn end_import(&mut self, running: Importing, r: wc_import::run::Res<()>, log: &wc_import::run::Log) {
        let Importing {
            job,
            kind,
            file,
            lib,
            target,
        } = running;
        // Opening another project is refused while it runs; should it be gone all the same, the
        // result is not taken into another one.
        let Some(open) = self.open.as_mut().filter(|o| o.dir == job.dir) else {
            self.dialog = Some(Dialog::Message(format!(
                "The import of {} ended, but the project it was for ({}) is no longer open: open it again to see what it holds.",
                job.name,
                job.dir.display()
            )));
            return;
        };
        let (done, msg) = job.finish(open, r, log);
        // The project's own folder is listed first once parts are in it, as for symbols; so is
        // a new folder chosen outside the library list.
        let source = match (target, lib.as_deref()) {
            (Some(t), _) => t.declare,
            (None, Some(l)) if kind.needs_library() || kind == imports::Kind::Archive => {
                new_folder_source(&open.dir, l, &open.project.meta.libraries)
            }
            (None, _) => None,
        };
        // An archive may hold no symbol library or parts list: its folder is listed only once
        // something was written there, also when the folder was there before. An archive that
        // failed on a later entry has still written what came before: the folder is listed then
        // too, so its parts are not left where the project does not look.
        let written = job.wrote_to_library();
        if (done || kind == imports::Kind::Archive)
            && written
            && let Some(source) = source
        {
            open.checkpoint();
            open.project.meta.libraries.insert(0, source);
            open.dirty = true;
            open.refresh_library();
        }
        applog::info(&format!("import {kind:?} {} ended", file.display()));
        self.prune_selection();
        self.dialog = Some(Dialog::Message(msg));
    }

    /// While an import runs: a window with what it has said so far, over the editor, which
    /// takes no clicks until the import is done. The window takes the clicks for everything
    /// under it, so the view controls that still work ([`IMPORT_VIEW_CONTROLS`]) and the layer
    /// list are shown in it.
    fn import_window(&mut self, ctx: &egui::Context) {
        let Some(running) = self.importing.as_ref() else { return };
        let lines = running.job.lines();
        let name = running.job.name.clone();
        let mut actions = Vec::new();
        let mut layers_changed = false;
        egui::Modal::new(egui::Id::new("importing")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.strong(format!("Importing {name}…"));
            });
            ui.label("The project can be edited again when the import is done.");
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if lines.is_empty() {
                        ui.weak("Reading the file…");
                    }
                    for line in &lines {
                        ui.label(RichText::new(line).monospace());
                    }
                });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.weak("View:");
                for a in IMPORT_VIEW_CONTROLS {
                    let info = a.info();
                    let on = *a == Action::Layers && self.show_layers;
                    if ui.selectable_label(on, info.label).on_hover_text(info.tip).clicked() {
                        actions.push(*a);
                    }
                }
            });
            if self.show_layers {
                ui.separator();
                layers_changed = self.prefs.layers.ui(ui);
            }
        });
        if layers_changed {
            self.layers_changed();
        }
        for a in actions {
            self.run(a);
        }
    }

    /// Closing the window would stop the import half way through writing the project folder:
    /// while one runs, a close that was asked for is called off.
    fn hold_close_while_importing(&mut self, ctx: &egui::Context) {
        if self.importing.is_some() && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.ed.message = format!("The window was not closed: {IMPORT_RUNNING}");
        }
    }

    /// Carry out an action. A picker it asks for waits until the frame's actions have run; while
    /// one waits, another is not taken (the first stays and the action says so).
    fn run(&mut self, a: Action) {
        let earlier = self.pending.take();
        self.carry_out(a);
        match (earlier, self.pending.is_some()) {
            (Some(e), true) => {
                self.pending = Some(e);
                self.ed.message = format!(
                    "{} was not done: another file dialog is waiting to open; try again after it.",
                    a.info().label
                );
            }
            (Some(e), false) => self.pending = Some(e),
            (None, true) => self.pending_for = self.open.as_ref().map(|o| o.dir.clone()),
            (None, false) => {}
        }
    }

    /// Run the command picked in the canvas right-click menu, if any, as its toolbar button
    /// would be (so a page someone else holds refuses it the same way).
    fn run_menu_action(&mut self) {
        if let Some(a) = self.ed.menu_action.take() {
            self.run(a);
        }
    }

    /// Show the picker that waits, if it is still for the project open now.
    fn ask_pending(&mut self) {
        // A picker asked for before the import started waits until it is done.
        if self.importing.is_some() {
            return;
        }
        let Some(p) = self.pending.take() else { return };
        let asked_for = self.pending_for.take();
        let now = self.open.as_ref().map(|o| o.dir.clone());
        if !pending_applies(&p, asked_for.as_deref(), now.as_deref()) {
            self.ed.message = "The file dialog was not shown: another project was opened before it could.".into();
            return;
        }
        self.ask(p);
    }

    /// Queue the picker `p` from a dialog, for the project open now, as [`App::run`] does for an
    /// action; a picker already waiting stays and this one is not taken.
    fn queue(&mut self, p: Pending) {
        let dir = self.open.as_ref().map(|o| o.dir.clone());
        if !queue_pending(&mut self.pending, &mut self.pending_for, p, dir) {
            self.ed.message = "Not done: another file dialog is waiting to open; try again after it.".into();
        }
    }

    fn carry_out(&mut self, a: Action) {
        let page = self.ed.page;
        applog::info(&format!("command {a:?}"));
        // The import owns the project folder until its result is taken in.
        if self.importing.is_some() && !allowed_while_importing(a) {
            self.ed.message = format!("{} was not done: {IMPORT_RUNNING}", a.info().label);
            return;
        }
        if let Some(name) = self.open.as_ref().and_then(|o| held_for(a, &self.ed, &o.project)) {
            self.ed.message = format!("{name} has this page open; it is read-only here until they leave it.");
            return;
        }
        // The keys are held back while the symbol editor is open; the menu, toolbar and
        // assistant are too, so the schematic does not change or get selected under the editor.
        if held_by_symbol_editor(a) && self.symbol_editor.is_some() {
            self.ed.message = symbol_editor_refusal(a.info().label);
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
            Action::TerminalStrips => {
                if let Some(o) = self.open.as_ref() {
                    self.strip_editor
                        .get_or_insert_with(|| strip_editor::StripEditor::for_project(o));
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
                    base: String::new(),
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
            Action::Layers => self.show_layers = !self.show_layers,
            Action::ToggleAutoWire => {
                self.prefs.auto_wire = !self.prefs.auto_wire;
                self.prefs.save();
                self.ed.message = if self.prefs.auto_wire {
                    "Auto-wiring is on: pins that line up get a wire (hold Alt to skip once).".into()
                } else {
                    "Auto-wiring is off.".into()
                };
            }
            Action::LiveCheck => {
                self.prefs.live_check = !self.prefs.live_check;
                self.prefs.save();
                self.ed.message = if self.prefs.live_check {
                    "Checking while drawing: problems are marked on the canvas shortly after each edit.".into()
                } else {
                    "Checking while drawing is off; Check (Ctrl+E) still checks the whole project.".into()
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
            Action::BlackBoxTool => {
                self.ed.tool = Tool::BlackBox;
                self.ed.wire = None;
                self.ed.shape_points.clear();
                self.ed.black_box = None;
                self.ed.message = black_box_tool::hint(None).into();
            }
            Action::TextTool
            | Action::LineTool
            | Action::RectTool
            | Action::CircleTool
            | Action::ArcTool
            | Action::PolylineTool
            | Action::DimensionTool
            | Action::CloudTool
            | Action::RevisionTableTool
            | Action::CommentTool => {
                self.ed.tool = match a {
                    Action::TextTool => Tool::Text,
                    Action::CommentTool => Tool::Comment,
                    Action::LineTool => Tool::Line,
                    Action::CircleTool => Tool::Circle,
                    Action::ArcTool => Tool::Arc,
                    Action::PolylineTool => Tool::Polyline,
                    Action::DimensionTool => Tool::Dimension,
                    Action::CloudTool => Tool::Cloud,
                    Action::RevisionTableTool => Tool::RevisionTable,
                    _ => Tool::Rect,
                };
                self.ed.wire = None;
                self.ed.shape_points.clear();
                self.ed.message = match a {
                    Action::TextTool => "Click where the text goes.",
                    Action::CommentTool => "Click where the comment goes.",
                    Action::CircleTool => "Click the centre of the circle.",
                    Action::ArcTool => "Click the centre of the arc.",
                    Action::DimensionTool => "Click the first point to measure from.",
                    Action::CloudTool => "Click one corner of the area the cloud goes around.",
                    Action::RevisionTableTool => "Click where the revision table's top-left corner goes.",
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
                // A panel device drag going on ends here: Escape must not put back a start
                // position the history no longer has.
                self.ed.panel_drag = None;
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
                self.ed.message = App::SAVE_WAITS.into();
            }
            Action::Save => match open.save() {
                Ok(n) => self.ed.message = format!("Saved ({n} files changed)."),
                // A file changed outside that Save would write over asks first (#662).
                Err(e) => {
                    // Only when that is what stopped this save: another error is shown as it is.
                    self.dialog = Some(match open.changed_outside() {
                        Some(path) if e.contains(state::Open::CHANGED_OUTSIDE) => Dialog::ChangedOutside(path),
                        _ => Dialog::Message(format!("Saving failed:\n{e}")),
                    });
                }
            },
            Action::SaveAs if self.share.active() => {
                self.ed.message = "Save as is not available in a shared session: it would end the session. \
                                   Stop sharing (or leave) first."
                    .into();
            }
            Action::SaveAs if self.assistant.command_running() => {
                self.ed.message = App::SAVE_WAITS.into();
            }
            Action::SaveAs => self.pending = Some(Pending::SaveAs),
            Action::Undo | Action::Redo => {
                // As in a shared session: a panel device drag going on ends here.
                self.ed.panel_drag = None;
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
            // Before the panel's own Delete: a panel device being dragged stays too.
            Action::Delete | Action::Cut if self.ed.dragging() => {
                // A delete would change the drag's history under it, and Escape could no
                // longer take the move back (#863).
                self.ed.message = "Let go of the move first, then delete.".into();
            }
            Action::Delete if self.ed.panel.is_some() => {
                if let (Some(pi), Some(item)) = (self.ed.panel, self.ed.panel_pick.take()) {
                    open.delete_panel_item(pi, item);
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
            Action::Rotate if self.ed.panel.is_some() => {
                if let (Some(pi), Some(item)) = (self.ed.panel, self.ed.panel_pick)
                    && !open.rotate_panel_device(pi, item)
                {
                    self.ed.message = "Turned, the device would not fit on the plate.".into();
                }
            }
            Action::Rotate => open.rotate_many(page, &self.ed.selection),
            Action::Mirror => open.mirror_many(page, &self.ed.selection),
            Action::SelectAll => {
                self.ed.tool = Tool::Select;
                self.ed.selection = self.prefs.layers.keep(open.select_all(page));
            }
            Action::Copy | Action::Cut => {
                let clip = open.copy(page, &self.ed.selection);
                if !clip.is_empty() {
                    let n = clip.count();
                    self.copied_text = Some(clip.summary());
                    self.clipboard = clip;
                    if a == Action::Cut {
                        // Revision clouds and the revision table are not copied: they stay,
                        // selected, rather than go without a way back by Paste.
                        let (kept, sel): (Vec<_>, Vec<_>) = std::mem::take(&mut self.ed.selection)
                            .into_iter()
                            .partition(|s| matches!(s, Selection::Cloud(_) | Selection::RevisionTable));
                        open.delete_many(page, &sel);
                        self.ed.selection = kept;
                        self.ed.message = format!("Cut {n}.");
                    } else {
                        self.ed.message = format!("Copied {n}.");
                    }
                }
            }
            Action::Paste => {
                // At the pointer when it is over the sheet, otherwise a little below the original.
                let at = self
                    .ed
                    .cursor
                    .unwrap_or(self.clipboard.anchor + wc_core::geom::Point::new(10.0, 10.0));
                match open.paste(page, &self.clipboard, at) {
                    Ok(pasted) if !pasted.is_empty() => {
                        self.ed.tool = Tool::Select;
                        self.ed.selection = pasted;
                    }
                    Ok(_) => {}
                    Err(e) => self.ed.message = format!("Nothing was pasted: {e}"),
                }
            }
            Action::Duplicate => {
                // A copy one grid step down and right, through the paste (one undo step, next
                // free tags), without touching the clipboard.
                let clip = open.copy(page, &self.ed.selection);
                let step = if self.prefs.snap > 0.0 { self.prefs.snap } else { 2.5 };
                match open.paste(page, &clip, clip.anchor + wc_core::geom::Point::new(step, step)) {
                    Ok(copies) if !copies.is_empty() => {
                        let n = copies.len();
                        self.ed.message = format!("Duplicated {n} {}.", if n == 1 { "item" } else { "items" });
                        self.ed.tool = Tool::Select;
                        self.ed.selection = copies;
                    }
                    Ok(_) => {}
                    Err(e) => self.ed.message = format!("Nothing was duplicated: {e}"),
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
            // Only the page shown changes, as a click in the page tree: no page is renumbered.
            Action::PrevPage | Action::NextPage => {
                let to = match a {
                    Action::PrevPage => page.checked_sub(1),
                    _ => Some(page + 1),
                };
                // A wire started on this page cannot end on another one.
                if let Some(i) = to.filter(|&i| i < open.project.pages.len())
                    && self.ed.show_page(i)
                {
                    self.ed.wire = None;
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
                    revisions: state::RevisionRow::of(&open.project.meta.revisions),
                    loaded_revisions: state::RevisionRow::of(&open.project.meta.revisions),
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
            Action::PlcRacks => {
                let racks = open.project.meta.racks.clone();
                self.dialog = Some(Dialog::Racks {
                    draft: racks.clone(),
                    since: open.start_live_form(),
                    before: racks,
                    seen: open.history_count(),
                    warnings: None,
                });
            }
            Action::ExportPdf => {
                // A project with translations is asked which language to print, then where to save;
                // one without is asked only where, and printed in the language it is drawn in.
                match wc_core::translate::Dictionary::load(&open.dir) {
                    Ok(d) if d.languages().len() > 1 => {
                        self.dialog = Some(Dialog::PdfLanguage {
                            langs: d.languages(),
                            first: 0,
                            second: 0,
                        });
                    }
                    Ok(_) => self.pending = Some(Pending::Export(Export::Pdf(Vec::new()))),
                    Err(e) => self.dialog = Some(Dialog::Message(format!("Export failed: {e}"))),
                }
            }
            Action::Translations => {
                self.dialog = Some(match translations_form::Form::open(&open.dir, &open.project) {
                    Ok(f) => Dialog::Translations(f),
                    Err(e) => Dialog::Message(format!("The translations could not be read:\n{e}")),
                });
            }
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
            a if imports::Kind::of(a).is_some() => {
                if let Some(kind) = imports::Kind::of(a) {
                    match kind
                        .needs_library()
                        .then(|| wc_library::Library::open(&open.dir, &open.project))
                    {
                        Some(Ok(lib)) => {
                            self.dialog = Some(Dialog::ImportParts {
                                kind,
                                targets: library_targets(&lib, open),
                                chosen: 0,
                            });
                        }
                        // Without the project's library list the folder picker asks instead,
                        // and the status line says why the list is not offered.
                        Some(Err(e)) => {
                            self.ed.message = format!(
                                "The project's libraries could not be read ({e}); pick the library folder for the parts."
                            );
                            self.pending = Some(Pending::Import(kind, None));
                        }
                        None => self.pending = Some(Pending::Import(kind, None)),
                    }
                }
            }
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
                    ed.choices = open.library.iter().map(|e| (e.id.clone(), e.name.clone())).collect();
                    if ed.opened_as.is_none() && ed.message.is_empty() {
                        ed.message = "Pick the symbol to change under \"Open a library symbol\".".into();
                    }
                }
                self.symbol_editor = Some(ed);
            }
            Action::ImportSymbols => match wc_library::Library::open(&open.dir, &open.project) {
                Ok(lib) => {
                    self.dialog = Some(Dialog::ImportSymbols {
                        targets: library_targets(&lib, open),
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
                                Ok(done) => {
                                    // The symbol list, editor choices and search show the new
                                    // names and descriptions.
                                    open.refresh_library();
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
        self.ed.zoom_keys(f);
    }

    /// The arrow keys move the selection on the page by one snap step, one undo step a press.
    /// With auto-wiring on, pins of moved symbols that now line up are wired in the same step,
    /// as when a move drag ends. Nothing moves while a drag is under way.
    fn nudge_by_arrows(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some()
            || ctx.wants_keyboard_input()
            || self.importing.is_some()
            || self.symbol_editor.is_some()
            || self.ed.panel.is_some()
            || self.ed.dragging()
            || self.ed.selection.is_empty()
        {
            return;
        }
        let steps = ctx.input(|i| arrow_steps(i, self.prefs.snap));
        if steps.is_empty() {
            return;
        }
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if let Some(name) = held_for(Action::Rotate, &self.ed, &open.project) {
            self.ed.message = format!("{name} has this page open; it is read-only here until they leave it.");
            return;
        }
        let moved: std::collections::BTreeSet<String> = self
            .ed
            .selection
            .iter()
            .filter_map(|s| match s {
                Selection::Symbol(id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        let mut wired = 0;
        for d in steps {
            open.checkpoint();
            open.move_by(self.ed.page, &self.ed.selection, d);
            if self.prefs.auto_wire && !moved.is_empty() {
                wired += open.auto_wire(self.ed.page, &moved);
            }
        }
        if wired > 0 {
            self.ed.message = if wired == 1 {
                "Connected 1 wire to pins that line up.".to_owned()
            } else {
                format!("Connected {wired} wires to pins that line up.")
            };
        }
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
                // Mac laptop keyboards have no Delete key: their "delete" sends Backspace.
                (Key::Backspace, Action::Delete),
                (Key::G, Action::ToggleGrid),
                (Key::F, Action::ZoomFit),
                (Key::Plus, Action::ZoomIn),
                // The unshifted key that has + on it on many keyboards (US: = and +).
                (Key::Equals, Action::ZoomIn),
                (Key::Minus, Action::ZoomOut),
                (Key::PageUp, Action::PrevPage),
                (Key::PageDown, Action::NextPage),
            ] {
                // + often needs Shift, so the zoom-in keys also count with Shift held.
                // Shift and nothing else: `shift_only` does not look at the Mac's Ctrl key.
                let zoom_in = matches!(k, Key::Plus | Key::Equals);
                let ok =
                    |m: &Modifiers| m.is_none() || (zoom_in && m.shift && !(m.ctrl || m.alt || m.command || m.mac_cmd));
                if pressed_with(i, k, ok) {
                    out.push(a);
                }
            }
            // A shape half drawn in the symbol editor takes Enter and Escape; the editor
            // handles them itself, so the schematic's drawing is left alone.
            if self.symbol_editor.as_ref().is_some_and(|e| e.drawing_shape()) {
                return;
            }
            if pressed_with(i, Key::Enter, Modifiers::is_none)
                && matches!(self.ed.tool, Tool::Polyline | Tool::BlackBox)
            {
                self.ed.finish_shape = true;
            }
            if i.key_pressed(Key::Escape) && self.ed.escape() {
                out.push(Action::Select);
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
            Dialog::Racks { .. } => "PLC racks",
            Dialog::PageProps { .. } => "Page properties",
            Dialog::DeletePage(_) => "Delete page",
            Dialog::Text(..) => "Text",
            Dialog::Comment(..) => "Comment",
            Dialog::BoxPin { .. } => "Connection point",
            Dialog::BoxTag { .. } => "Black box",
            Dialog::PageHistory(..) => "Page history",
            Dialog::Reports(_) => "Reports",
            Dialog::ReportLayout { .. } => "Report layout",
            Dialog::Check(_) => "Project check",
            Dialog::Translations(_) => "Translations",
            Dialog::PdfLanguage { .. } => "Export PDF",
            Dialog::Rules { .. } => "Check rules",
            Dialog::Commit(_) => "Record a version",
            Dialog::NewBranch(_) => "New branch",
            Dialog::History(_) => "History",
            Dialog::Library(_) => "Library status",
            Dialog::Message(_) => "WiringCraft",
            Dialog::ChangedOutside(_) => "Save",
            Dialog::Fetching(_) => "New project",
            Dialog::SaveMacro { .. } => "Save as macro",
            Dialog::Macros(_) => "Insert macro",
            Dialog::MacroEdit { .. } => "Macro placeholders",
            Dialog::ImportSymbols { .. } => "Import symbols",
            Dialog::ImportParts { .. } => "Import parts",
            Dialog::ImportPath { .. } => "Import",
            Dialog::ImportOptions { .. } => "Import options",
            Dialog::AttachPath { kind, .. } => kind.title(),
            Dialog::Report { .. } => "Report a problem",
            Dialog::WagoMarking { .. } => "WAGO marking",
        };
        let wide = matches!(
            dialog,
            Dialog::Reports(_) | Dialog::ReportLayout { .. } | Dialog::Check(_) | Dialog::Translations(_)
        );
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
                            && let Some(open) = &mut self.open
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
                        egui::Grid::new("macros").striped(true).num_columns(4).show(ui, |ui| {
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
                                // A project macro's placeholders and variants are edited here; a
                                // library's macros belong to the library.
                                if m.project {
                                    if ui
                                        .button("Placeholders…")
                                        .on_hover_text("Declare its placeholders and the value sets of its variants")
                                        .clicked()
                                    {
                                        let path = m.path.clone();
                                        then = Some(Box::new(move |app: &mut App| app.edit_macro(&path)));
                                        keep = false;
                                    }
                                } else {
                                    ui.label("");
                                }
                                if m.value_sets.is_empty() || m.error.is_some() {
                                    if ui.add_enabled(m.error.is_none(), egui::Button::new("Insert")).clicked() {
                                        let path = m.path.clone();
                                        let name = m.name.clone();
                                        then = Some(Box::new(move |app: &mut App| app.insert_macro(&path, &name, None)));
                                        keep = false;
                                    }
                                } else {
                                    // A macro with variants asks which one to insert.
                                    ui.menu_button("Insert…", |ui| {
                                        ui.label("Which variant?");
                                        for set in &m.value_sets {
                                            if ui.button(set.as_str()).clicked() {
                                                let (path, name, set) = (m.path.clone(), m.name.clone(), set.clone());
                                                then = Some(Box::new(move |app: &mut App| {
                                                    app.insert_macro(&path, &name, Some(&set))
                                                }));
                                                keep = false;
                                                ui.close_menu();
                                            }
                                        }
                                    });
                                }
                                ui.end_row();
                            }
                        });
                    });
                    if ui.button("Close").clicked() {
                        keep = false;
                    }
                }
                Dialog::MacroEdit { form, error } => {
                    egui::ScrollArea::vertical()
                        .id_salt("macro-edit-scroll")
                        .max_height(ui.ctx().screen_rect().height() * 0.6)
                        .show(ui, |ui| macro_form::show(ui, form));
                    if let Some(e) = error {
                        ui.colored_label(theme::Theme::of(ui.ctx()).error, e.as_str());
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            // Saved through the project, so the save is one undo step.
                            let form = form.clone();
                            then = Some(Box::new(move |app: &mut App| {
                                let Some(open) = app.open.as_mut() else {
                                    return;
                                };
                                let saved = if app.share.active() {
                                    Err(Open::MACRO_SAVE_SHARED.to_owned())
                                } else {
                                    open.save_macro_form(&form)
                                };
                                match saved {
                                    Ok(path) => {
                                        app.ed.message = format!("Saved macro {}", path.display());
                                        app.dialog = Some(Dialog::Macros(open.macros()));
                                    }
                                    Err(e) => {
                                        app.dialog = Some(Dialog::MacroEdit { form, error: Some(e) });
                                    }
                                }
                            }));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            then = Some(Box::new(|app: &mut App| {
                                if let Some(open) = &app.open {
                                    app.dialog = Some(Dialog::Macros(open.macros()));
                                }
                            }));
                            keep = false;
                        }
                    });
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
                Dialog::Fetching(source) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Fetching the base project from\n{source}…"));
                    });
                    if ui.button("Cancel").clicked() {
                        then = Some(Box::new(|app: &mut App| app.cancel_fetch()));
                        keep = false;
                    }
                }
                Dialog::ChangedOutside(path) => {
                    ui.label(format!(
                        "Not saved: {} was changed outside WiringCraft after an Undo or Redo could not \
                         write it as the project needs it.\n\nKeep the version changed outside, or write \
                         the project's version over it?",
                        path.display()
                    ));
                    ui.horizontal(|ui| {
                        for (label, keep_outside) in
                            [("Keep the outside version", true), ("Write the project's version", false)]
                        {
                            if ui.button(label).clicked() {
                                let path = path.clone();
                                then = Some(Box::new(move |app: &mut App| {
                                    if let Some(open) = app.open.as_mut() {
                                        if keep_outside {
                                            open.keep_outside_version(&path);
                                        } else {
                                            open.write_over_outside_version(&path);
                                        }
                                    }
                                    app.carry_out(Action::Save);
                                }));
                                keep = false;
                            }
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
                                match pick_folder_for("Open project", path) {
                                    Picked::Path(d) => *error = path.pick(d).err(),
                                    Picked::Failed(e) | Picked::Unsure(e) => *error = Some(e),
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
                        if go && path.is_empty() {
                            // A blank field would mean the projects folder itself.
                            *error = Some("Type the folder for the copy.".into());
                        } else if go {
                            match path.path() {
                                Ok(p) => {
                                    let field = path.clone();
                                    // Refused: the form comes back with the path, to edit and retry.
                                    then = Some(Box::new(move |app: &mut App| {
                                        if let Err(e) = app.save_as_to(&p) {
                                            app.dialog = Some(Dialog::SaveAsPath(field, Some(e)));
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
                Dialog::NewProject {
                    location,
                    name,
                    base,
                    error,
                } => {
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
                                    match pick_folder_for("Where to create the project", location) {
                                        Picked::Path(d) => *error = location.pick(d).err(),
                                        Picked::Failed(e) | Picked::Unsure(e) => *error = Some(e),
                                        Picked::Cancelled => {}
                                    }
                                }
                            });
                            ui.end_row();
                            ui.label("Start from");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(base)
                                        .hint_text("blank, or a base project folder or git URL")
                                        .desired_width(300.0),
                                );
                                if ui.button("Browse…").clicked() {
                                    match pick_folder("Base project to start from", base) {
                                        Picked::Path(d) => match d.to_str() {
                                            Some(t) => *base = t.to_owned(),
                                            None => *error = Some("That folder's name is not text.".into()),
                                        },
                                        Picked::Failed(e) | Picked::Unsure(e) => *error = Some(e),
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
                        RichText::new(if base.trim().is_empty() {
                            "A new folder with one cover page, recorded as a git repository."
                        } else {
                            "A new folder with the base project's pages, settings, title block and libraries \
                             (not its comments or history), recorded as a git repository. {name} and {date} in \
                             its title block and page titles become this project's name and today's date."
                        })
                        .color(t.faint),
                    );
                    ui.horizontal(|ui| {
                        if let Ok(d) = target
                            && ui
                                .add_enabled(!location.is_empty(), egui::Button::new("Create"))
                                .clicked()
                        {
                            let n = name.trim().to_owned();
                            let b = Some(base.trim().to_owned()).filter(|b| !b.is_empty());
                            then = Some(Box::new(move |app: &mut App| app.start_new_project(&d, &n, b.as_deref())));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Translations(form) => {
                    translations_form::show(ui, form);
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() {
                            let running = self.assistant.command_running();
                            match App::save_translations(self.open.as_mut(), &self.share, running, form) {
                                Ok(m) => {
                                    then = Some(Box::new(move |app: &mut App| app.ed.message = m));
                                    keep = false;
                                }
                                Err(e)
                                    if e == Open::TRANSLATIONS_GUEST
                                        || e == App::SAVE_WAITS
                                        || e == App::TRANSLATIONS_ELSEWHERE =>
                                {
                                    form.error = Some(e);
                                }
                                Err(e) => form.error = Some(format!("Saving failed: {e}")),
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::PdfLanguage { langs, first, second } => {
                    egui::Grid::new("pdflang").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                        ui.label("Language");
                        egui::ComboBox::from_id_salt("pdflang1")
                            .selected_text(langs.get(*first).cloned().unwrap_or_default())
                            .show_ui(ui, |ui| {
                                for (k, l) in langs.iter().enumerate() {
                                    ui.selectable_value(first, k, l);
                                }
                            });
                        ui.end_row();
                        ui.label("Shown with");
                        let none = "(none)".to_owned();
                        egui::ComboBox::from_id_salt("pdflang2")
                            .selected_text(if *second == 0 { none.clone() } else { langs.get(*second - 1).cloned().unwrap_or(none.clone()) })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(second, 0, none);
                                for (k, l) in langs.iter().enumerate() {
                                    ui.selectable_value(second, k + 1, l);
                                }
                            });
                        ui.end_row();
                    });
                    ui.label(
                        RichText::new("Texts without a translation are printed as drawn and listed as warnings.")
                            .color(theme::Theme::of(ui.ctx()).faint),
                    );
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("Export").clicked() {
                            let mut chosen: Vec<String> = langs.get(*first).cloned().into_iter().collect();
                            if let Some(l) = second.checked_sub(1).and_then(|k| langs.get(k))
                                && !chosen.contains(l)
                            {
                                chosen.push(l.clone());
                            }
                            then = Some(Box::new(move |app: &mut App| {
                                app.ask(Pending::Export(Export::Pdf(chosen)));
                            }));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Rules { rows, loaded, dir } => {
                    rules_form::show(ui, rows);
                    let bad = rules_form::rules_of(rows).err();
                    if let Some(why) = &bad {
                        ui.colored_label(theme::Theme::of(ui.ctx()).error, why);
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.add_enabled(bad.is_none(), egui::Button::new("OK")).clicked() {
                            let (rows, loaded, dir) = (rows.clone(), loaded.clone(), dir.clone());
                            then = Some(Box::new(move |app: &mut App| {
                                if let Err(e) = app.apply_rules(&rows, &loaded, &dir) {
                                    app.ed.message = e;
                                }
                            }));
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                            then = Some(Box::new(|app: &mut App| app.show_check()));
                        }
                    });
                }
                Dialog::ProjectProps {
                    name,
                    fields,
                    locations,
                    loaded_locations,
                    revisions,
                    loaded_revisions,
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
                    ui.add_space(8.0);
                    ui.label(RichText::new("Revisions").strong());
                    let mut drop = None;
                    egui::ScrollArea::vertical()
                        .id_salt("projrevisions-scroll")
                        .max_height(180.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            egui::Grid::new("projrevisions")
                                .num_columns(5)
                                .spacing([8.0, 6.0])
                                .show(ui, |ui| {
                                    for h in ["Rev.", "Date", "Description", "By", ""] {
                                        ui.strong(h);
                                    }
                                    ui.end_row();
                                    for (k, r) in revisions.iter_mut().enumerate() {
                                        // A listed revision keeps its letter: its clouds carry it.
                                        let new = r.was.trim().is_empty();
                                        ui.add_enabled(
                                            new,
                                            egui::TextEdit::singleline(&mut r.letter).hint_text("A").desired_width(40.0),
                                        );
                                        ui.add(egui::TextEdit::singleline(&mut r.date).hint_text("2026-11-19").desired_width(90.0));
                                        ui.add(egui::TextEdit::singleline(&mut r.text).hint_text("What changed").desired_width(200.0));
                                        ui.add(egui::TextEdit::singleline(&mut r.author).desired_width(60.0));
                                        if ui.button("Remove").clicked() {
                                            drop = Some(k);
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                    if let Some(k) = drop {
                        revisions.remove(k);
                    }
                    if ui.button("Add revision").clicked() {
                        revisions.push(state::RevisionRow::default());
                    }
                    let rev_bad = self
                        .open
                        .as_ref()
                        .and_then(|o| o.revision_draft_problem(loaded_revisions, revisions));
                    let bad = wc_core::project::location_rows(locations).err().or(rev_bad);
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
                            let r = (revisions != loaded_revisions).then(|| revisions.clone());
                            let loaded = loaded_revisions.clone();
                            then = Some(Box::new(move |app: &mut App| {
                                let Some(o) = app.open.as_mut() else {
                                    return;
                                };
                                // Checked again: a sync may have come in since the dialog was drawn.
                                if let Some(why) = r.as_deref().and_then(|r| o.revision_draft_problem(&loaded, r)) {
                                    app.ed.message = why;
                                } else if let Err(e) = o.set_project_props(&n, &f, l.as_deref(), r.as_deref()) {
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
                Dialog::Racks {
                    draft,
                    since,
                    before,
                    seen,
                    warnings,
                } => {
                    // A change made elsewhere (an undo, the assistant) is shown in the form.
                    if let Some(o) = self.open.as_ref()
                        && o.history_count() != *seen
                    {
                        draft.clone_from(&o.project.meta.racks);
                        *seen = o.history_count();
                    }
                    if racks_form::show(ui, draft)
                        && let Some(o) = self.open.as_mut()
                    {
                        o.set_racks_live(draft);
                        *seen = o.history_count();
                    }
                    if let Some(o) = self.open.as_ref()
                        && warnings.as_ref().is_none_or(|(rev, _)| *rev != o.revision())
                    {
                        *warnings = Some((o.revision(), rack_warnings(o)));
                    }
                    let warnings = warnings.as_ref().map(|(_, w)| w.as_slice()).unwrap_or_default();
                    let t = theme::Theme::of(ui.ctx());
                    // Many cards can give many warnings: they scroll, so OK and Cancel stay
                    // on screen.
                    let height = (ui.ctx().screen_rect().height() * 0.15).min(140.0);
                    egui::ScrollArea::vertical()
                        .id_salt("rack-warnings")
                        .max_height(height)
                        .show(ui, |ui| {
                            for w in warnings {
                                ui.label(RichText::new(w).color(t.warning));
                            }
                        });
                    ui.label(
                        RichText::new(
                            "The PLC cards report shows each change at once. The racks are kept \
                             in project.toml.",
                        )
                        .color(t.faint),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() {
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            if let Some(o) = self.open.as_mut() {
                                o.revert_racks(*since, before);
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
                        if ui
                            .button("Rules…")
                            .on_hover_text("Add, change, reorder or delete the project's own check rules")
                            .clicked()
                        {
                            keep = false;
                            then = Some(Box::new(|app: &mut App| app.edit_rules()));
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
                                then = Some(Box::new(move |app: &mut App| app.ask(Pending::ReportCsv(kind, false))));
                            }
                            if ui
                                .button("Save as CSV for Excel (semicolon)…")
                                .on_hover_text("Semicolon-separated with a UTF-8 byte order mark, so Excel opens it in columns with æ ø å intact")
                                .clicked()
                            {
                                let kind = *kind;
                                then = Some(Box::new(move |app: &mut App| app.ask(Pending::ReportCsv(kind, true))));
                            }
                            if ui.button("Save as Excel").clicked() {
                                let path = o.dir.join("export").join(format!("{}.xlsx", kind.name()));
                                let res = std::fs::create_dir_all(o.dir.join("export"))
                                    .and_then(|()| table.to_xlsx())
                                    .and_then(|bytes| std::fs::write(&path, bytes));
                                self.ed.message = match res {
                                    Ok(()) => format!("Saved {}", path.display()),
                                    Err(e) => format!("Could not save {}: {e}", path.display()),
                                };
                            }
                            if ui
                                .button("Layout…")
                                .on_hover_text("Pick, order and rename the columns, sort and group the rows")
                                .clicked()
                            {
                                let k = *kind;
                                then = Some(Box::new(move |app: &mut App| app.edit_report_layout(k)));
                                keep = false;
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
                Dialog::ReportLayout { form, dir } => {
                    if let Some(o) = self.open.as_ref() {
                        // The form and preview scroll; OK and Cancel stay in view below them.
                        egui::ScrollArea::vertical()
                            .id_salt("reportlayout-scroll")
                            .max_height(ui.ctx().screen_rect().height() * 0.6)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                report_layout_form::show(ui, form);
                                // What the layout names that the report or the title block does
                                // not have, in the scroll area so OK and Cancel stay in view.
                                for p in form.problems(&o.project) {
                                    ui.colored_label(theme::Theme::of(ui.ctx()).warning, p);
                                }
                                ui.separator();
                                let inputs = wc_report::Inputs {
                                    project: &o.project,
                                    symbols: &o.symbols,
                                    parts: &o.parts,
                                };
                                let draft = form.layout();
                                let table = wc_report::build_with(form.kind, &inputs, draft.as_ref());
                                ui.label(RichText::new(&table.title).strong());
                                report_table(ui, &table);
                            });
                        let bad = form.problem();
                        if let Some(why) = &bad {
                            ui.colored_label(theme::Theme::of(ui.ctx()).error, why);
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.add_enabled(bad.is_none(), egui::Button::new("OK")).clicked() {
                                let (form, dir) = (form.as_ref().clone(), dir.clone());
                                then = Some(Box::new(move |app: &mut App| {
                                    if let Err(e) = app.apply_report_layout(&form, &dir) {
                                        app.ed.message = e;
                                    }
                                }));
                                keep = false;
                            }
                            if ui.button("Built-in layout").on_hover_text("Clear the form to the report's own columns").clicked() {
                                **form = form.cleared();
                            }
                            if ui.button("Cancel").clicked() {
                                let k = form.kind;
                                then = Some(Box::new(move |app: &mut App| app.dialog = Some(Dialog::Reports(k))));
                                keep = false;
                            }
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
                            self.add_text(*at, text);
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
                        {
                            self.add_comment_at(*at, text);
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                            keep = false;
                        }
                    });
                }
                Dialog::BoxPin {
                    side,
                    offset,
                    name,
                    focused,
                    error,
                } => {
                    ui.label(format!("On the {} edge, {offset:.1} mm along it.", side_name(*side)));
                    let r = ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text("Connection point name")
                            .desired_width(240.0),
                    );
                    if !*focused {
                        r.request_focus();
                        *focused = true;
                    }
                    if let Some(e) = error.as_ref() {
                        ui.colored_label(theme::Theme::of(ui.ctx()).error, e);
                    }
                    ui.horizontal(|ui| {
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        if ui.button("Add").clicked() || enter {
                            match self.ed.black_box.as_mut() {
                                Some(d) => match d.add_pin(*side, *offset, name) {
                                    Ok(()) => {
                                        self.ed.message = black_box_tool::hint(Some(d)).into();
                                        keep = false;
                                    }
                                    Err(e) => *error = Some(e),
                                },
                                None => keep = false,
                            }
                        }
                        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                            keep = false;
                        }
                    });
                }
                Dialog::BoxTag { tag, focused, error } => {
                    let pins = self.ed.black_box.as_ref().map_or(0, |d| d.pins.len());
                    ui.label(format!(
                        "{pins} connection point{}. The tag can be left empty.",
                        if pins == 1 { "" } else { "s" }
                    ));
                    let r = ui.add(egui::TextEdit::singleline(tag).hint_text("-A1").desired_width(240.0));
                    if !*focused {
                        r.request_focus();
                        *focused = true;
                    }
                    if let Some(e) = error.as_ref() {
                        ui.colored_label(theme::Theme::of(ui.ctx()).error, e);
                    }
                    ui.horizontal(|ui| {
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        if ui.button("Put on page").clicked() || enter {
                            match self.add_black_box(tag) {
                                Ok(()) => keep = false,
                                Err(e) => *error = Some(e),
                            }
                        }
                        if ui.button("Back").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
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
                Dialog::ImportPath {
                    kind,
                    target,
                    file,
                    lib,
                    error,
                } => {
                    ui.label(kind.title());
                    let (label, extensions) = kind.filter();
                    ui.label(format!("File ({label}: .{}), full path:", extensions.join(", .")));
                    let r = ui.add(egui::TextEdit::singleline(&mut file.text).desired_width(380.0));
                    let ask_lib = kind.needs_library() && target.is_none();
                    if ask_lib {
                        ui.label("Library folder for the parts:");
                        ui.add(egui::TextEdit::singleline(&mut lib.text).desired_width(380.0));
                    }
                    if let Some(e) = error {
                        ui.label(RichText::new(e.as_str()).color(theme::Theme::of(ui.ctx()).error));
                    }
                    ui.horizontal(|ui| {
                        let go = ui.button("Import").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                        if go {
                            let folder = typed_library(ask_lib, lib);
                            match file.path().and_then(|f| folder.map(|l| (f, l))) {
                                Ok((f, _)) if !f.is_file() => *error = Some(format!("{} is not a file.", f.display())),
                                Ok((f, l)) => {
                                    let (kind, target) = (*kind, target.clone());
                                    let l = l.or_else(|| target.as_ref().map(|t| t.root.clone()));
                                    then = Some(Box::new(move |app: &mut App| {
                                        app.import_picked(kind, &f, l.as_deref(), target);
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
                Dialog::ImportOptions {
                    file,
                    lib,
                    target,
                    form,
                    error,
                } => {
                    ui.label(form.kind.title());
                    ui.label(file.display().to_string());
                    egui::Grid::new("import_options").num_columns(2).show(ui, |ui| {
                        if form.has(imports::Opt::Scale) {
                            ui.label("Scale").on_hover_text(
                                "The drawing's coordinates are multiplied by this to get mm: 1 for a drawing in mm, 10 for one in cm, 25.4 for one in inches",
                            );
                            ui.add(egui::TextEdit::singleline(&mut form.scale).desired_width(80.0));
                            ui.end_row();
                        }
                        if form.has(imports::Opt::Project) {
                            ui.label("Project")
                                .on_hover_text("The archive holds several projects; one is imported");
                            let shown = form.projects.get(form.project).cloned().unwrap_or_default();
                            egui::ComboBox::from_id_salt("import_swe_project")
                                .selected_text(shown)
                                .show_ui(ui, |ui| {
                                    for (k, name) in form.projects.iter().enumerate() {
                                        ui.selectable_value(&mut form.project, k, name);
                                    }
                                });
                            ui.end_row();
                            if let Some(note) = &form.projects_note {
                                ui.label("");
                                ui.label(RichText::new(note.as_str()).weak());
                                ui.end_row();
                            }
                        }
                        if form.has(imports::Opt::FirstPage) {
                            ui.label("First page number")
                                .on_hover_text("The first new page takes this number; the others follow it");
                            let numbered = !(form.kind == imports::Kind::Swe && form.keep_numbers);
                            ui.add_enabled(
                                numbered,
                                egui::TextEdit::singleline(&mut form.first_page).desired_width(80.0),
                            );
                            ui.end_row();
                            if form.kind == imports::Kind::Swe {
                                ui.label("");
                                ui.checkbox(&mut form.keep_numbers, "Keep the source's page numbers");
                                ui.end_row();
                            }
                        }
                        if form.has(imports::Opt::Library) {
                            ui.label("Symbol libraries and parts lists").on_hover_text(
                                "KiCad symbol libraries and parts lists inside the archive go into a library folder; without one they are listed, not imported",
                            );
                            egui::ComboBox::from_id_salt("import_archive_lib")
                                .selected_text(form.library_label(form.library))
                                .show_ui(ui, |ui| {
                                    for k in 0..=form.libraries.len() + 1 {
                                        let label = form.library_label(k);
                                        ui.selectable_value(&mut form.library, k, label);
                                    }
                                });
                            ui.end_row();
                            if form.other_library() {
                                ui.label("");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut form.library_folder.text)
                                            .hint_text("library folder")
                                            .desired_width(300.0),
                                    );
                                    if ui.button("Browse…").clicked() {
                                        let picked = if form.library_folder.is_empty() {
                                            pick_folder("Library folder", &file.display().to_string())
                                        } else {
                                            pick_folder_for("Library folder", &form.library_folder)
                                        };
                                        match picked {
                                            Picked::Path(d) => *error = form.library_folder.pick(d).err(),
                                            Picked::Failed(e) | Picked::Unsure(e) => {
                                                *error = Some(format!("{e} Type the library folder instead."));
                                            }
                                            Picked::Cancelled => {}
                                        }
                                    }
                                });
                                ui.end_row();
                            }
                        }
                        if form.has(imports::Opt::SymbolMap) {
                            ui.label("Symbol map").on_hover_text(
                                "Optional: a text file with one `source name = symbol id` per line, which gives a source block or macro a library symbol",
                            );
                            ui.horizontal(|ui| {
                                // The hint is cut short in a narrow field: it is the hover text too.
                                let hint = imports::symbol_map_hint();
                                ui.add(
                                    egui::TextEdit::singleline(&mut form.symbol_map.text)
                                        .hint_text(hint.as_str())
                                        .desired_width(300.0),
                                )
                                .on_hover_text(hint);
                                if ui.button("Browse…").clicked() {
                                    let start = file.parent().unwrap_or(file.as_path());
                                    match pick_open("Symbol map", start, ("Symbol map (text file)", &["txt", "map", "*"])) {
                                        Picked::Path(f) => *error = form.symbol_map.pick(f).err(),
                                        Picked::Failed(e) | Picked::Unsure(e) => {
                                            *error = Some(format!("{e} Type the symbol map's path instead."));
                                        }
                                        Picked::Cancelled => {}
                                    }
                                }
                            });
                            ui.end_row();
                        }
                    });
                    if let Some(e) = error {
                        ui.label(RichText::new(e.as_str()).color(theme::Theme::of(ui.ctx()).error));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Import").clicked() {
                            match form.options() {
                                Ok(options) => {
                                    // An archive's library folder is chosen here, a parts
                                    // import's before the file.
                                    let l = options.lib.clone().or_else(|| lib.clone());
                                    let target = form.library_target().cloned().or_else(|| target.clone());
                                    let (kind, f) = (form.kind, file.clone());
                                    then = Some(Box::new(move |app: &mut App| {
                                        app.import_from(kind, &f, l.as_deref(), target, &options);
                                    }));
                                    keep = false;
                                }
                                Err(e) => *error = Some(e),
                            }
                        }
                        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                            keep = false;
                        }
                    });
                }
                Dialog::AttachPath {
                    kind,
                    part,
                    file,
                    error,
                } => {
                    let (label, extensions) = kind.filter();
                    ui.label(format!("{} for {part}", kind.title()));
                    ui.label(format!("File ({label}: .{}), full path:", extensions.join(", .")));
                    let r = ui.add(egui::TextEdit::singleline(&mut file.text).desired_width(380.0));
                    if let Some(e) = error {
                        ui.label(RichText::new(e.as_str()).color(theme::Theme::of(ui.ctx()).error));
                    }
                    ui.horizontal(|ui| {
                        let go = ui.button("Attach").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                        if go {
                            match typed_file(file) {
                                Ok(f) => {
                                    let (kind, part) = (*kind, part.clone());
                                    then = Some(Box::new(move |app: &mut App| {
                                        if let Some(open) = app.open.as_mut() {
                                            app.ed.message = kind.apply(open, &part, &f);
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
                Dialog::ImportParts { kind, targets, chosen } => {
                    ui.label(kind.title());
                    ui.horizontal(|ui| {
                        ui.label("Library folder");
                        let other = "Another folder…".to_owned();
                        let shown = |k: usize| targets.get(k).map_or_else(|| other.clone(), |t| t.label());
                        egui::ComboBox::from_id_salt("import_parts_lib")
                            .selected_text(shown(*chosen))
                            .show_ui(ui, |ui| {
                                for k in 0..=targets.len() {
                                    ui.selectable_value(chosen, k, shown(k));
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Choose the file and import…").clicked() {
                            let pending = Pending::Import(*kind, targets.get(*chosen).cloned());
                            then = Some(Box::new(move |app: &mut App| app.queue(pending)));
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
                                        open.edited();
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

    /// Open the check rules dialog on the project's rules.
    fn edit_rules(&mut self) {
        if let Some(o) = self.open.as_ref() {
            let loaded = o.project.meta.rules.clone();
            self.dialog = Some(Dialog::Rules {
                rows: rules_form::rows_of(&loaded),
                loaded,
                dir: o.dir.clone(),
            });
        }
    }

    /// Open the layout dialog for report `kind` on the project's layout for it.
    fn edit_report_layout(&mut self, kind: wc_report::Kind) {
        if let Some(o) = self.open.as_ref() {
            let inputs = wc_report::Inputs {
                project: &o.project,
                symbols: &o.symbols,
                parts: &o.parts,
            };
            let builtin = wc_report::build_builtin(kind, &inputs).columns;
            self.dialog = Some(Dialog::ReportLayout {
                form: Box::new(report_layout_form::Form::new(
                    kind,
                    builtin,
                    wc_report::layout_for(&o.project, kind),
                )),
                dir: o.dir.clone(),
            });
        }
    }

    /// Keep the layout dialog's layout (one undo step) and show the report in it. As with
    /// [`App::apply_rules`], a layout changed in the project while the dialog was open keeps
    /// the dialog open on the draft, and another project open now gets nothing.
    fn apply_report_layout(&mut self, form: &report_layout_form::Form, dir: &std::path::Path) -> Result<(), String> {
        let layout = form.kept()?;
        let Some(o) = self.open.as_mut().filter(|o| o.dir == dir) else {
            return Err("The report layout was not kept: another project was opened while it was edited.".into());
        };
        let now = wc_report::layout_for(&o.project, form.kind).cloned();
        if form.edited() {
            if now != form.loaded {
                self.dialog = Some(Dialog::ReportLayout {
                    // Against the layout as it is now: OK keeps the draft whenever it differs
                    // from that one, even if it went back to the layout the dialog opened on.
                    form: Box::new(report_layout_form::Form {
                        initial: report_layout_form::Form::new(form.kind, form.builtin.clone(), now.as_ref()).layout(),
                        loaded: now,
                        ..form.clone()
                    }),
                    dir: dir.to_path_buf(),
                });
                return Err(
                    "The report layout changed while you edited it. Your edits are still open: OK keeps them in place of the new layout, Cancel keeps the new one."
                        .into(),
                );
            }
            o.set_report_layout(form.kind, layout);
        }
        self.dialog = Some(Dialog::Reports(form.kind));
        Ok(())
    }

    /// Show the project check, run now.
    fn show_check(&mut self) {
        if let Some(o) = self.open.as_ref() {
            self.dialog = Some(Dialog::Check(run_check(o)));
        }
    }

    /// Keep the rules of the check rules dialog (one undo step), then show the check with
    /// them. Rows left as they were loaded leave the project's rules as they are now (the
    /// assistant may have changed them meanwhile). `dir` is the project the dialog opened on:
    /// when another one is open now, nothing is changed.
    fn apply_rules(
        &mut self,
        rows: &[rules_form::RuleRow],
        loaded: &[wc_core::check_rule::CheckRule],
        dir: &std::path::Path,
    ) -> Result<(), String> {
        let rules = rules_form::rules_of(rows)?;
        let Some(o) = self.open.as_mut().filter(|o| o.dir == dir) else {
            return Err("The rules were not kept: another project was opened while they were edited.".into());
        };
        // Compared as rows: a value the dialog does not write (a cable rule's max) is no edit.
        if rows != rules_form::rows_of(loaded) {
            // The rules changed since the dialog opened (the assistant, someone in a shared
            // session): the draft would put back what they changed. It stays open on the
            // rules as they are now, so OK again keeps the draft and Cancel theirs.
            if o.project.meta.rules != loaded {
                self.dialog = Some(Dialog::Rules {
                    rows: rows.to_vec(),
                    loaded: o.project.meta.rules.clone(),
                    dir: dir.to_path_buf(),
                });
                return Err(
                    "The check rules changed while you edited them. Your edits are still open: OK keeps them in place of the new rules, Cancel keeps the new rules."
                        .into(),
                );
            }
            o.set_check_rules(rules);
        }
        self.show_check();
        Ok(())
    }

    /// The name for a new project in `dir`: `name`, or the folder's name when it is blank.
    fn project_name(dir: &std::path::Path, name: &str) -> String {
        if name.is_empty() {
            dir.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Project".into())
        } else {
            name.to_owned()
        }
    }

    /// New project, Create: as [`App::new_project`], except that a base project in a git
    /// repository is fetched on a thread of its own ([`App::fetch`]), with a note and Cancel,
    /// and the project opens once it is there.
    fn start_new_project(&mut self, dir: &std::path::Path, name: &str, base: Option<&str>) {
        let Some(source) = base.filter(|b| wc_vcs::base::is_url(b)) else {
            self.new_project(dir, name, base);
            return;
        };
        if let Some(f) = &self.fetch {
            if f.made {
                self.dialog = None;
                self.ed.message = "The last new project is made and opens once the open project's work is done; create this one after that.".into();
            } else {
                // The running fetch keeps its note and Cancel.
                self.dialog = Some(Dialog::Fetching(f.source.clone()));
                self.ed.message = "A base project is being fetched already; wait for it or cancel it first.".into();
            }
            return;
        }
        let name = Self::project_name(dir, name);
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let (tx, done) = std::sync::mpsc::channel();
        let (to, from, stop) = (dir.to_path_buf(), source.to_owned(), cancel.clone());
        std::thread::spawn(move || {
            let made = wc_vcs::base::create_from_base_until(&from, &to, &name, &wc_core::base::today(), &stop);
            let _ = tx.send(made.map(|_| ()).map_err(|e| match e {
                wc_vcs::Error::Cancelled => None,
                e => Some(e.to_string()),
            }));
        });
        self.fetch = Some(BaseFetch {
            dir: dir.to_path_buf(),
            source: source.to_owned(),
            was_open: self.open.as_ref().map(|o| o.dir.clone()),
            made: false,
            cancel,
            done,
        });
        self.dialog = Some(Dialog::Fetching(source.to_owned()));
    }

    /// Stop the base project being fetched: nothing is made.
    fn cancel_fetch(&mut self) {
        match &self.fetch {
            Some(f) if f.made => {
                self.ed.message =
                    "The new project is made already; it opens once the open project's work is done.".into();
            }
            Some(f) => {
                f.cancel.store(true, Ordering::Relaxed);
                self.ed.message = "Cancelling the new project…".into();
            }
            None => {}
        }
    }

    /// Take in the base project fetched for a new project, once it is there: the project opens,
    /// or the reason it could not be made shows. Until then the window keeps drawing.
    fn poll_fetch(&mut self, ctx: &egui::Context) {
        let Some(f) = self.fetch.as_mut() else { return };
        if !f.made {
            let failed = match f.done.try_recv() {
                Ok(Ok(())) => {
                    f.made = true;
                    None
                }
                Ok(Err(e)) => Some(e),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Some("the fetch stopped without an answer".into()))
                }
            };
            // Cancelled or failed: said at once, whatever else is open.
            if let Some(why) = failed {
                let Some(f) = self.fetch.take() else { return };
                let fetching = matches!(self.dialog, Some(Dialog::Fetching(_)));
                if fetching {
                    self.dialog = None;
                }
                match why {
                    None => self.ed.message = "Cancelled: the new project was not made.".into(),
                    Some(e) => {
                        let m = format!("Could not create the project in\n{}:\n{e}", f.dir.display());
                        if fetching || self.dialog.is_none() {
                            self.dialog = Some(Dialog::Message(m));
                        } else {
                            self.ed.message = m.replace('\n', " ");
                        }
                    }
                }
                return;
            }
        }
        // Made. The open project is not saved or replaced while something still works on it:
        // an assistant turn or command, an open dialog, or the symbol or terminal strip editor.
        // The new project waits until they are done, and Cancel is no longer offered.
        if self.assistant.busy()
            || self.assistant.command_running()
            || self.importing.is_some()
            || self.symbol_editor.is_some()
            || self.strip_editor.is_some()
            || self.dialog.as_ref().is_some_and(|d| !matches!(d, Dialog::Fetching(_)))
        {
            if matches!(self.dialog, Some(Dialog::Fetching(_))) {
                self.dialog = None;
            }
            self.ed.message = "The new project is made; it opens once the open project's work is done.".into();
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        let Some(f) = self.fetch.take() else { return };
        if matches!(self.dialog, Some(Dialog::Fetching(_))) {
            self.dialog = None;
        }
        if self.open.as_ref().map(|o| &o.dir) != f.was_open.as_ref() {
            // Another project was opened meanwhile: it stays, and the new one waits.
            let recorded = record_new_project(&f.dir);
            self.prefs.remember(f.dir.clone());
            self.prefs.save();
            self.dialog = Some(Dialog::Message(match recorded {
                Ok(()) => format!(
                    "The new project is ready in\n{}\nOpen it from File, Open when you want it.",
                    f.dir.display()
                ),
                Err(e) => format!(
                    "The new project is in\n{}\nbut its first version could not be recorded:\n{e}",
                    f.dir.display()
                ),
            }));
        } else {
            self.finish_new_project(&f.dir, false);
        }
    }

    /// Create a project in `dir`: blank, or from the base project (folder or git URL) `base`.
    fn new_project(&mut self, dir: &std::path::Path, name: &str, base: Option<&str>) {
        let name = Self::project_name(dir, name);
        let made = match base {
            Some(b) => {
                wc_vcs::base::create_from_base(b, dir, &name, &wc_core::base::today()).map_err(|e| e.to_string())
            }
            None => wc_core::store::create(dir, &name).map_err(|e| e.to_string()),
        };
        if let Err(e) = made {
            self.dialog = Some(Dialog::Message(format!(
                "Could not create the project in\n{}:\n{e}",
                dir.display()
            )));
            return;
        }
        self.finish_new_project(dir, base.is_none());
    }

    /// After a project was made in `dir`: a blank one gets the starter library, then it is
    /// recorded as a first version and opened.
    fn finish_new_project(&mut self, dir: &std::path::Path, blank: bool) {
        // A starter library next to the app, when there is one, so the symbol picker is not
        // empty. A base project brings its own library sources.
        if blank
            && let Some(lib) = starter_library()
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
        let recorded = record_new_project(dir);
        // The new project replaces the open one: keep its unsaved work first.
        if let Some(o) = self.open.as_mut()
            && o.dirty
            && let Err(e) = o.save()
        {
            let mut m = format!(
                "The new project is in\n{}\nbut the open project could not be saved first, so it stays open:\n{e}",
                dir.display()
            );
            if let Err(e) = recorded {
                m.push_str(&format!(
                    "\nThe new project's first version could not be recorded either:\n{e}"
                ));
            }
            self.dialog = Some(Dialog::Message(m));
            return;
        }
        let mut prefs = std::mem::take(&mut self.prefs);
        self.open_dir(dir, &mut prefs);
        self.prefs = prefs;
        if let Err(e) = recorded {
            self.dialog = Some(Dialog::Message(match self.dialog.take() {
                // Opening it failed too: both reasons show.
                Some(Dialog::Message(m)) => format!("{m}\nIts first version could not be recorded either:\n{e}"),
                _ => format!("The new project is open, but its first version could not be recorded:\n{e}"),
            }));
        }
    }

    /// Insert a macro on the current page, selected so it can be dragged into place.
    /// Open the placeholders dialog on the project macro in `path`.
    fn edit_macro(&mut self, path: &std::path::Path) {
        self.dialog = Some(match macro_form::Form::open(path) {
            Ok(form) => Dialog::MacroEdit {
                form: Box::new(form),
                error: None,
            },
            Err(e) => Dialog::Message(format!("The macro could not be read:\n{e}")),
        });
    }

    fn insert_macro(&mut self, path: &std::path::Path, name: &str, value_set: Option<&str>) {
        let page = self.ed.page;
        let Some(open) = self.open.as_mut() else { return };
        match open.insert_macro(page, path, value_set) {
            Ok(items) if !items.is_empty() => {
                self.ed.tool = Tool::Select;
                let name = match value_set {
                    Some(set) => format!("{name} ({set})"),
                    None => name.to_owned(),
                };
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
    /// No path, at once, and no error logged: the picker may never have shown, or the person
    /// cancelled very quickly. A fallback that only asks (a folder field) is fine; nothing is
    /// written on it.
    Unsure(String),
}

/// The system's folder picker, starting in `start` (or its nearest existing parent). Blocks the
/// window while open, like any modal dialog.
fn pick_folder(title: &str, start: &str) -> Picked {
    pick_folder_in(title, picker_start(start))
}

/// [`pick_folder`] for a folder field: a folder picked into it earlier starts the picker
/// exactly as picked (a name ending in a space included), typed text as [`pick_folder`] reads it.
fn pick_folder_for(title: &str, field: &prefs::FolderField) -> Picked {
    pick_folder_in(title, field_picker_start(field))
}

/// Where [`pick_folder_for`] opens for `field`.
fn field_picker_start(field: &prefs::FolderField) -> Option<PathBuf> {
    match field.path() {
        Ok(p) if !field.is_empty() => nearest_dir(&p),
        _ => picker_start(&field.text),
    }
}

/// The system's folder picker, opening in `start` when given.
fn pick_folder_in(title: &str, start: Option<PathBuf>) -> Picked {
    // On Linux rfd tries the desktop portal, then zenity; both are given their chance.
    let mut dialog = rfd::FileDialog::new().set_title(title);
    if let Some(d) = start {
        dialog = dialog.set_directory(d);
    }
    applog::take_picker_errors();
    let shown = std::time::Instant::now();
    let r = dialog.pick_folder();
    match picker_outcome(r, shown.elapsed(), &applog::take_picker_errors()) {
        Picked::Failed(e) => Picked::Failed(format!("{e}{TYPE_INSTEAD}")),
        Picked::Unsure(e) => Picked::Unsure(format!("{e}{TYPE_INSTEAD}")),
        p => p,
    }
}

/// The system's save dialog, in `dir` when it exists, with `name` filled in and, when given,
/// only files of one kind shown. Blocks the window while open, like any modal dialog.
fn pick_save(title: &str, dir: &Path, name: &str, filter: Option<(&str, &[&str])>) -> Picked {
    let mut dialog = rfd::FileDialog::new().set_title(title).set_file_name(name);
    if let Some(d) = nearest_dir(dir) {
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
    if let Some(d) = nearest_dir(dir) {
        dialog = dialog.set_directory(d);
    }
    applog::take_picker_errors();
    let shown = std::time::Instant::now();
    let r = dialog.pick_file();
    picker_outcome(r, shown.elapsed(), &applog::take_picker_errors())
}

/// A file picker: [`pick_open`], or a stand-in for it in tests.
type PickOpen<'a> = &'a dyn Fn(&str, &Path, (&str, &[&str])) -> Picked;

/// The file to attach as `kind` to `part`, asked for with the file picker `pick` opening in
/// `dir`: the file, or where no picker can open (or one perhaps never showed) the form to type
/// its path in, with why; `None` when the person cancelled.
fn attach_target(kind: panels::Attach, part: &str, dir: &Path, pick: PickOpen<'_>) -> Option<Result<PathBuf, Dialog>> {
    match pick(kind.title(), dir, kind.filter()) {
        Picked::Path(f) => Some(Ok(f)),
        Picked::Failed(e) | Picked::Unsure(e) => Some(Err(Dialog::AttachPath {
            kind,
            part: part.to_owned(),
            file: prefs::FolderField::default(),
            error: Some(format!("{e} Type the file's path instead.")),
        })),
        Picked::Cancelled => None,
    }
}

/// The file typed in a path form, when it is one.
fn typed_file(field: &prefs::FolderField) -> Result<PathBuf, String> {
    let f = field.path()?;
    match std::fs::metadata(&f) {
        Ok(m) if m.is_file() => Ok(f),
        Ok(_) => Err(format!("{} is not a file.", f.display())),
        Err(e) => Err(format!("{}: {e}", f.display())),
    }
}

/// A save dialog: [`pick_save`], or a stand-in for it in tests.
type PickSave<'a> = &'a dyn Fn(&str, &Path, &str, Option<(&str, &[&str])>) -> Picked;

/// Where an export goes: the file picked in the save dialog `pick`, or `default` with a note
/// when the dialog could not open; `Err(None)` when the person cancelled, and `Err` with what
/// to say when the dialog closed at once without a file (perhaps a quick Cancel, so nothing is
/// written).
fn export_target(
    title: &str,
    default: &Path,
    filter: (&str, &[&str]),
    pick: PickSave<'_>,
) -> Result<(PathBuf, String), Option<String>> {
    let dir = default.parent().unwrap_or(default);
    let name = default
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match pick(title, dir, &name, Some(filter)) {
        Picked::Path(p) => Ok((p, String::new())),
        Picked::Failed(e) => Ok((
            default.to_path_buf(),
            format!("{e} So it goes to the usual place, {}.\n\n", default.display()),
        )),
        Picked::Unsure(e) => Err(Some(format!("{e} Nothing was written."))),
        Picked::Cancelled => Err(None),
    }
}

/// The WAGO marker export to the file asked for with the save dialog `pick`: the message it
/// leaves, or on Cancel the options dialog again, as it was set, to try again.
fn export_markers(open: &Open, options: &wc_report::MarkingOptions, pick: PickSave<'_>) -> Dialog {
    let default = wago_marking_file(open, options);
    match export_target("Export WAGO markers", &default, ("CSV list", &["csv"]), pick) {
        Ok((path, note)) => Dialog::Message(note + &export_wago_marking_to(open, options, &path)),
        Err(Some(why)) => Dialog::Message(why),
        Err(None) => wago_marking_dialog_with(open, options),
    }
}

/// A whole-project export to the file asked for with the save dialog `pick`: what it says, or
/// `None` when the person cancelled (nothing is written then, nor when the dialog closed at
/// once without a file).
fn export_whole(open: &Open, kind: Export, pick: PickSave<'_>) -> Option<String> {
    let (title, default, filter): (&str, PathBuf, (&str, &[&str])) = match &kind {
        Export::Pdf(_) => ("Export PDF", export_file(open, "pdf"), ("PDF", &["pdf"])),
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
    let (path, note) = match export_target(title, &default, filter, pick) {
        Ok(target) => target,
        Err(why) => return why,
    };
    let msg = match kind {
        Export::Pdf(langs) => export_pdf_to(open, &langs, &path),
        Export::Aml => export_aml_to(open, &path),
        _ => export_wago_configurator_to(open, &path),
    };
    Some(note + &msg)
}

/// A report's CSV text: plain comma CSV, or for Excel (`excel`) `;`-separated with a leading
/// UTF-8 byte order mark, as `wiringcraft report --sep ";"` writes it, so Excel in a
/// comma-decimal locale splits the columns and reads æ ø å right.
fn report_csv(table: &wc_report::Table, excel: bool) -> String {
    if excel {
        format!("\u{feff}{}", table.to_csv_with(';'))
    } else {
        table.to_csv()
    }
}

/// Write a file, making its folder first.
fn write_creating(path: &Path, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, bytes)
}

/// What [`pick_folder`] adds to its failure: callers with a folder field can take it.
const TYPE_INSTEAD: &str = " Type the folder instead.";

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
        // A very quick Cancel looks the same, so this is not taken for a failure to write on.
        None if errors.is_empty() && open_for < std::time::Duration::from_millis(150) => {
            applog::warn(&format!(
                "folder picker: no answer after {open_for:?}; maybe not opened"
            ));
            Picked::Unsure(
                "The file picker closed at once without a choice. If it did not show, it could not open: \
                 on Linux it needs a desktop portal (xdg-desktop-portal) or zenity in a desktop session."
                    .to_owned(),
            )
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
    nearest_dir(&start)
}

/// The folder a picker opens in for `p`: `p` or its nearest folder that exists.
fn nearest_dir(p: &Path) -> Option<PathBuf> {
    p.ancestors()
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

/// The library folder typed into the import form, when the form asks for one: a blank field is
/// refused rather than taken as the projects folder.
fn typed_library(ask: bool, field: &prefs::FolderField) -> Result<Option<PathBuf>, String> {
    if !ask {
        Ok(None)
    } else if field.is_empty() {
        Err("Type the library folder for the parts.".to_owned())
    } else {
        field.path().map(Some)
    }
}

/// The source to add to the project for a parts folder chosen outside its library list (with
/// "Another folder…" or the folder picker): None when `folder` is already one of `declared`
/// (compared as given and as canonical paths). Its name is the folder's name, made unique
/// among `declared`, since library proposals find their destination by name.
fn new_folder_source(
    project_dir: &Path,
    folder: &Path,
    declared: &[wc_core::project::LibrarySource],
) -> Option<wc_core::project::LibrarySource> {
    let same = |a: &Path, b: &Path| {
        a == b
            || matches!(
                (a.canonicalize(), b.canonicalize()),
                (Ok(x), Ok(y)) if x == y
            )
    };
    let listed = declared.iter().any(|d| {
        d.kind == "folder" && {
            let at = Path::new(&d.location);
            let at = if at.is_absolute() {
                at.to_path_buf()
            } else {
                project_dir.join(at)
            };
            same(&at, folder)
        }
    });
    if listed {
        return None;
    }
    let base = folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "parts".to_owned());
    let taken = |n: &str| declared.iter().any(|d| d.name == n);
    let name = std::iter::once(base.clone())
        .chain((2..).map(|k| format!("{base}-{k}")))
        .find(|n| !taken(n))
        .unwrap_or(base);
    Some(wc_core::project::LibrarySource {
        name,
        kind: "folder".into(),
        location: folder.display().to_string(),
        reference: None,
    })
}

/// Where imported symbols or parts can go: the project's own library folder first, then its
/// other library folders ([`import_symbols::targets`]).
fn library_targets(lib: &wc_library::Library, open: &Open) -> Vec<import_symbols::ImportTarget> {
    let taken: Vec<String> = open.project.meta.libraries.iter().map(|l| l.name.clone()).collect();
    import_symbols::targets(&open.dir, folder_targets(lib, open), &taken)
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

/// The project check, with the check rules of the project's library sources as they are now.
fn run_check(open: &Open) -> Vec<wc_report::Finding> {
    wc_report::check_with(
        &wc_report::Inputs {
            project: &open.project,
            symbols: &open.symbols,
            parts: &open.parts,
        },
        &wc_library::library_rules(&open.dir, &open.project),
    )
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
        wc_report::Kind::PlcCards => "PLC cards",
        wc_report::Kind::Contents => "Contents",
        wc_report::Kind::Locations => "Locations",
        wc_report::Kind::Networks => "Networks",
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

/// The moves of the arrow keys pressed this frame without modifiers, in order: `step`
/// millimetres a press, y pointing down the page. Each press (or key repeat) is one move, and
/// each key event's own modifiers decide whether it is a nudge.
fn arrow_steps(i: &egui::InputState, step: f64) -> Vec<wc_core::geom::Point> {
    i.events
        .iter()
        .filter_map(|e| match e {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if modifiers.is_none() => match key {
                Key::ArrowLeft => Some((-step, 0.0)),
                Key::ArrowRight => Some((step, 0.0)),
                Key::ArrowUp => Some((0.0, -step)),
                Key::ArrowDown => Some((0.0, step)),
                _ => None,
            },
            _ => None,
        })
        .map(|(x, y)| wc_core::geom::Point::new(x, y))
        .collect()
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
            (Key::D, Action::Duplicate),
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

/// Whether `key` was pressed this frame with modifiers that `ok` accepts. It looks at the
/// modifiers held at the press itself: the frame's modifiers can differ, as when Ctrl is let go
/// before the frame runs, and Ctrl+R must not then rotate.
fn pressed_with(i: &egui::InputState, key: Key, ok: impl Fn(&Modifiers) -> bool) -> bool {
    i.events
        .iter()
        .any(|e| matches!(e, egui::Event::Key { key: k, pressed: true, modifiers, .. } if *k == key && ok(modifiers)))
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

/// What is wrong with the project's PLC racks, as the PLC cards report would say.
fn rack_warnings(open: &Open) -> Vec<String> {
    wc_report::plc_rack_warnings(&wc_report::Inputs {
        project: &open.project,
        symbols: &open.symbols,
        parts: &open.parts,
    })
}

fn export_pdf(open: &Open) -> String {
    export_pdf_in(open, &[])
}

/// [`export_pdf`] with the project's texts in `langs` (one language, or two shown together)
/// from its `translations.toml`; empty prints them as drawn. Texts without a translation are
/// listed as warnings.
fn export_pdf_in(open: &Open, langs: &[String]) -> String {
    export_pdf_to(open, langs, &export_file(open, "pdf"))
}

/// [`export_pdf_in`] to the file `path`.
fn export_pdf_to(open: &Open, langs: &[String], path: &Path) -> String {
    let (translated, missing) = if langs.is_empty() {
        (std::borrow::Cow::Borrowed(&open.project), Vec::new())
    } else {
        let dict = match wc_core::translate::Dictionary::load(&open.dir) {
            Ok(d) => d,
            Err(e) => return format!("Export failed: {e}"),
        };
        let (p, m) = wc_core::translate::translate(&open.project, &dict, langs);
        (std::borrow::Cow::Owned(p), m)
    };
    let project: &wc_core::Project = &translated;
    // The table of contents goes before the schematic pages and the other reports after them,
    // as in a finished drawing set.
    let inputs = wc_report::Inputs {
        project,
        symbols: &open.symbols,
        parts: &open.parts,
    };
    let pictures = wc_pdf::read_pictures(&open.dir, project);
    let options = wc_pdf::Options {
        reports: wc_report::Kind::ALL
            .into_iter()
            .map(|k| wc_report::build(k, &inputs))
            .filter(|t| !t.rows.is_empty())
            .collect(),
        underlays: pictures.files,
        unreadable: pictures.unreadable,
        panels: project.panels.clone(),
        networks: project.meta.networks.clone(),
        footprints: wc_pdf::footprints(open.parts.values()),
        ..Default::default()
    };
    let (bytes, report) = wc_pdf::export(project, &open.symbols, &options);
    let mut warnings = report.warnings;
    // The text itself stays out: this message goes to the diagnostic log, which never holds
    // drawing content. File, Translations, "Only missing" lists the texts.
    warnings.extend(missing.iter().map(|m| {
        format!(
            "no translation for a text ({}); see File, Translations, Only missing",
            m.used.join(", ")
        )
    }));
    match write_creating(path, bytes) {
        Ok(()) => {
            let mut m = format!("Exported {} pages to\n{}", report.pages, path.display());
            if !warnings.is_empty() {
                m.push_str("\n\nWarnings:\n");
                m.push_str(&warnings.join("\n"));
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

/// The WAGO marking dialog set as `options` were, to try again after the save dialog was
/// cancelled.
fn wago_marking_dialog_with(open: &Open, options: &wc_report::MarkingOptions) -> Dialog {
    let mut d = wago_marking_dialog(open, options.strip.clone());
    if let Dialog::WagoMarking {
        kinds,
        lines,
        language,
        level_separator,
        ..
    } = &mut d
    {
        for (on, k) in kinds.iter_mut().zip(wc_report::MarkerKind::ALL) {
            *on = options.kinds.contains(&k);
        }
        let mut chosen = options.lines.iter().copied();
        for l in lines.iter_mut() {
            *l = chosen.next();
        }
        *language = options.language;
        level_separator.clone_from(&options.level_separator);
    }
    d
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

/// The configurator list (as `export_wago_configurator` writes it) to the file `path`.
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

/// The marker list (as `export_wago_marking` writes it) to the file `path`.
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

/// Whether two paths name the same folder: compared as the file system resolves them (links,
/// `..`, letter case), or as written when either cannot be resolved (not there yet).
fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Every page as SVG and DXF in `export/drawings`, named by page number, and every panel
/// layout, named `panel-<name>`.
fn export_drawings(open: &Open) -> String {
    export_drawings_to(open, &open.dir.join("export").join("drawings"))
}

/// [`export_drawings`] into the folder `dir`.
fn export_drawings_to(open: &Open, dir: &Path) -> String {
    let pictures = wc_pdf::read_pictures(&open.dir, &open.project);
    let options = wc_pdf::Options {
        underlays: pictures.files,
        unreadable: pictures.unreadable,
        panels: open.project.panels.clone(),
        footprints: wc_pdf::footprints(open.parts.values()),
        ..Default::default()
    };
    let (pages, report) = wc_pdf::page_drawings(&open.project, &open.symbols, &options);
    let mut used = std::collections::BTreeSet::new();
    let files: Vec<(String, &wc_pdf::PageDrawing)> = pages
        .iter()
        .map(|page| {
            // A panel layout is named after itself (`panel-Main_cabinet`), a page by its number.
            let mut stem = if page.panel {
                wc_pdf::panel_file_stem(page)
            } else {
                let base = if page.number.trim().is_empty() {
                    &page.id
                } else {
                    page.number.trim()
                };
                base.chars()
                    .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
                    .collect()
            };
            // Compared without case: `A1` and `a1` are one file name on Windows and macOS.
            while !used.insert(stem.to_lowercase()) {
                stem.push('_');
            }
            (stem, page)
        })
        .collect();
    // The project's own drawings folder is the export's: a new export replaces the last one.
    // Another folder may hold other files with the same names, which are never replaced.
    let own = same_dir(dir, &open.dir.join("export").join("drawings"));
    if !own {
        let taken: Vec<String> = files
            .iter()
            .flat_map(|(stem, _)| [format!("{stem}.svg"), format!("{stem}.dxf")])
            .filter(|name| dir.join(name).exists())
            .collect();
        if let Some(first) = taken.first() {
            return format!(
                "Nothing exported: {} already has {} of the files this export writes ({first} and \
                 others), and they are not replaced. Pick an empty folder.",
                dir.display(),
                taken.len()
            );
        }
    }
    // In another folder, what a failed export made is taken away again, so it can be retried;
    // a file that was there before is never touched (it was refused above or by create_new).
    // A file counts as made the moment create_new made it, before anything is written to it.
    let mut made: Vec<PathBuf> = Vec::new();
    let mut write = |path: PathBuf, bytes: String| {
        if own {
            return std::fs::write(path, bytes);
        }
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        made.push(path);
        f.write_all(bytes.as_bytes())
    };
    let written = std::fs::create_dir_all(dir).and_then(|()| {
        for (stem, page) in &files {
            for (path, bytes) in [
                (dir.join(format!("{stem}.svg")), wc_pdf::svg(page)),
                (dir.join(format!("{stem}.dxf")), wc_pdf::dxf(page)),
            ] {
                write(path, bytes)?;
            }
        }
        Ok(())
    });
    if written.is_err() {
        for path in &made {
            if let Err(e) = std::fs::remove_file(path) {
                applog::warn(&format!(
                    "could not remove {} after the failed export: {e}",
                    path.display()
                ));
            }
        }
    }
    match written {
        Ok(()) => {
            let panels = pages.iter().filter(|p| p.panel).count();
            let mut m = format!("Exported {} pages", pages.len() - panels);
            if panels > 0 {
                m.push_str(&format!(" and {panels} panel layout(s)"));
            }
            m.push_str(&format!(" as SVG and DXF to\n{}", dir.display()));
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
        self.hold_close_while_importing(ctx);
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
            show_layers: self.show_layers,
        };
        for url in std::mem::take(&mut self.open_url) {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
        let mut actions = chrome::show(ctx, self.prefs.chrome, &mut self.chrome, &status);
        let keys = self.shortcuts(ctx);
        self.nudge_by_arrows(ctx);
        // While an import runs no key edits the symbol being drawn either: the keys go to the
        // actions, which wait for the import.
        if self.importing.is_some() {
            actions.extend(keys);
        } else {
            actions.extend(self.route_to_symbol_editor(keys));
        }
        for a in actions {
            self.run(a);
        }
        // So did any the actions made (a move by arrow key, say): the panels show jumpers that
        // match the terminals.
        if let Some(o) = self.open.as_mut() {
            o.follow_terminals();
        }
        self.live_check(ctx);
        self.poll_fetch(ctx);
        self.poll_import(ctx);
        self.log_messages();
        self.assistant_poll(ctx);
        self.ask_pending();
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
            .show(ctx, |ui| {
                let live = self.prefs.live_check.then(|| {
                    (
                        self.live.status(&self.ed.live),
                        self.live.busy(),
                        self.live.failed.is_some(),
                    )
                });
                if panels::status_bar(ui, self.open.as_ref(), &self.ed, live) {
                    self.dialog = Some(Dialog::Check(self.ed.live.clone()));
                }
            });
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
            // Keyboard edits wait while a dialog, an import or the symbol editor has the keyboard,
            // as the schematic's arrow keys do.
            let keyboard_edits = self.dialog.is_none() && self.importing.is_none() && self.symbol_editor.is_none();
            egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx, |ui| {
                self.ed.auto_wire = self.prefs.auto_wire;
                self.ed.layers = self.prefs.layers.clone();
                canvas::show(
                    ui,
                    open,
                    &mut self.ed,
                    self.prefs.show_grid,
                    self.prefs.snap,
                    keyboard_edits,
                );
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
        self.run_menu_action();
        // The picker opens on the next frame, outside the panel that asked for it.
        if let Some((kind, part)) = self.ed.ask_attach.take() {
            self.queue(Pending::Attach(kind, part));
            ctx.request_repaint();
        }
        if let Some(at) = self.ed.ask_text.take() {
            self.dialog = Some(Dialog::Text(at, String::new(), false));
        }
        if let Some(at) = self.ed.ask_comment.take() {
            self.dialog = Some(Dialog::Comment(at, String::new(), false));
        }
        // A click on the box while one of its dialogs is open does not replace that dialog.
        if let Some((side, offset)) = self.ed.ask_box_pin.take()
            && self.dialog.is_none()
            && let Some(d) = self.ed.black_box.as_ref()
        {
            self.dialog = Some(Dialog::BoxPin {
                side,
                offset,
                name: d.next_pin_name(),
                focused: false,
                error: None,
            });
        }
        if std::mem::take(&mut self.ed.ask_box_tag) && self.dialog.is_none() && self.ed.black_box.is_some() {
            let tag = self
                .open
                .as_ref()
                .and_then(|o| o.next_tag(self.ed.page, "A"))
                .map(|t| t.to_string())
                .unwrap_or_default();
            self.dialog = Some(Dialog::BoxTag {
                tag,
                focused: false,
                error: None,
            });
        }
        self.dialogs(ctx);
        // While an import runs, the windows that edit the project, a library or the session
        // wait (they show again when it is done), not only behind the import window.
        if self.importing.is_none() {
            self.share
                .windows(ctx, self.open.as_mut(), self.assistant.command_running());
            self.symbol_editor_window(ctx);
            self.find_window(ctx);
            self.strip_editor_window(ctx);
            self.layers_window(ctx);
        }
        // The import window shows the layer list itself: it takes the clicks for any other.
        self.import_window(ctx);
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
    /// Check while drawing: start a check once the last edit has settled, and show the
    /// findings of one that finished.
    fn live_check(&mut self, ctx: &egui::Context) {
        let Some(o) = self.open.as_ref().filter(|_| self.prefs.live_check) else {
            self.live.clear();
            self.ed.live.clear();
            return;
        };
        // A keystroke, a click or a drag may edit (even one that does not report itself in the
        // revision): each one starts the wait again.
        let editing = ctx.input(|i| {
            i.pointer.any_down()
                || i.events.iter().any(|e| {
                    matches!(
                        e,
                        egui::Event::Key { .. }
                            | egui::Event::Text(_)
                            | egui::Event::Paste(_)
                            | egui::Event::Cut
                            | egui::Event::PointerButton { .. }
                    )
                })
        });
        let poll = self.live.poll(o, editing, std::time::Instant::now());
        // Show findings only once they are of exactly these inputs: a finished check, or the
        // checker's findings after the editor was replaced (the same project opened again, a
        // session joined). While a check is pending they may be of other inputs (another
        // project), so the editor keeps what it shows.
        if let Some(found) = self.live.settled()
            && self.ed.live != found
        {
            self.ed.live = found.to_vec();
        }
        if let Some(after) = poll.again {
            ctx.request_repaint_after(after);
        }
    }

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
                a if held_by_symbol_editor(*a) => {
                    // Held back, and said so, as from the menu.
                    self.ed.message = symbol_editor_refusal(a.info().label);
                    false
                }
                // The page behind the symbol editor stays as it is.
                Action::PrevPage | Action::NextPage => false,
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

    /// The Text dialog's OK: a free text at `at` on the page shown. Not on a page someone else
    /// holds, nor while the drawings layer is hidden or locked (it may have changed while the
    /// dialog was open).
    fn add_text(&mut self, at: wc_core::geom::Point, text: &str) {
        let Some(o) = self.open.as_mut() else { return };
        if let Some(why) = o.held_reason(self.ed.page) {
            self.ed.message = why;
            return;
        }
        if let Some(why) = self.prefs.layers.refusal(layers::Layer::Drawings) {
            self.ed.message = why;
            return;
        }
        let shape = wc_core::geom::Shape::Text {
            at,
            text: text.trim().to_owned(),
            height: 3.5,
            align: wc_core::geom::Align::Left,
            angle: 0.0,
        };
        if let Some(sel) = o.add_graphic(self.ed.page, shape) {
            self.ed.select_only(sel);
        }
    }

    /// The black box dialog's button: the box drawn with the black box tool goes on its page,
    /// tagged `tag` (none when empty), and is selected.
    fn add_black_box(&mut self, tag: &str) -> Result<(), String> {
        let o = self.open.as_mut().ok_or("no project is open")?;
        let d = self.ed.black_box.clone().ok_or("no black box is being drawn")?;
        let (a, b) = d.rect().ok_or("the black box has no rectangle yet")?;
        let page = o
            .project
            .pages
            .iter()
            .position(|p| p.id == d.page_id)
            .ok_or("the page the box was drawn on is gone")?;
        if let Some(why) = o.held_reason(page) {
            return Err(why);
        }
        let tag = match tag.trim() {
            "" => None,
            t => Some(wc_core::DeviceTag::parse(t).map_err(|e| e.to_string())?),
        };
        let id = o.add_black_box(page, a, b, &d.pins, tag)?;
        self.ed.black_box = None;
        // The page the box went on is shown (the view may have moved meanwhile), so the
        // selection names the new box and not an item of the same id elsewhere.
        self.ed.page = page;
        self.ed.panel = None;
        self.ed.select_only(Selection::Symbol(id));
        self.ed.message = format!(
            "Black box with {} connection point{} put on the page.",
            d.pins.len(),
            if d.pins.len() == 1 { "" } else { "s" }
        );
        Ok(())
    }

    /// The Comment dialog's button: a review comment at `at` on the page shown, unless the
    /// comments layer is hidden or locked by now.
    fn add_comment_at(&mut self, at: wc_core::geom::Point, text: &str) {
        if let Some(why) = self.prefs.layers.refusal(layers::Layer::Comments) {
            self.ed.message = why;
            return;
        }
        let Some(o) = self.open.as_mut() else { return };
        let Some(page) = o.project.pages.get(self.ed.page).map(|p| p.id.clone()) else {
            return;
        };
        let mut next = o.project.clone();
        match next.add_comment(&page, Some(at), &state::author_name(), state::now_secs(), text) {
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
    }

    /// The layer list: show, hide and lock what the canvas shows. Kept in the preferences.
    fn layers_window(&mut self, ctx: &egui::Context) {
        if !self.show_layers {
            return;
        }
        let mut open = true;
        let mut changed = false;
        egui::Window::new("Layers")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| changed = self.prefs.layers.ui(ui));
        if changed {
            self.layers_changed();
        }
        self.show_layers = open;
    }

    /// The layer list changed: save it and apply it to the editor at once, so a command run
    /// before the canvas is drawn again (Delete, Cut, Rotate…) does not act on an item that
    /// was just hidden or locked.
    fn layers_changed(&mut self) {
        self.prefs.save();
        self.apply_layers();
    }

    /// Hand the layer list to the editor and drop what it no longer lets be selected.
    fn apply_layers(&mut self) {
        self.ed.layers = self.prefs.layers.clone();
        let layers = &self.ed.layers;
        self.ed.selection.retain(|s| layers.may_select(s));
    }

    /// The terminal strip editor, while open and its project is.
    fn strip_editor_window(&mut self, ctx: &egui::Context) {
        let (Some(ed), Some(open)) = (self.strip_editor.as_mut(), self.open.as_mut()) else {
            self.strip_editor = None;
            return;
        };
        // The window belongs to the project it was opened in: another project opened since
        // (or a shared session joined) closes it, so nothing typed in it lands there.
        if ed.owner.as_deref() != Some(open.dir.as_path()) {
            self.strip_editor = None;
            return;
        }
        if !ed.show(ctx, open) {
            self.strip_editor = None;
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
        // A hit on a hidden or locked layer would be dropped from the selection on the next
        // frame without a word: say why instead.
        let note = select.as_ref().and_then(|s| self.prefs.layers.unselectable_note(s));
        self.ed.selection = self.prefs.layers.keep(select.into_iter().collect());
        // Each go-to shows only its own note: a note from an earlier hit is cleared.
        let note = note.unwrap_or_default();
        if let Some(find) = self.find.as_mut() {
            find.message = note.clone();
        }
        self.ed.message = note;
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
mod report_csv_tests {
    use super::report_csv;

    fn table() -> wc_report::Table {
        wc_report::Table {
            columns: vec!["Tag".into(), "Note".into()],
            rows: vec![vec!["K1".into(), "Relé, 24 V".into()]],
            ..Default::default()
        }
    }

    #[test]
    fn excel_csv_is_semicolon_separated_with_a_utf8_bom() {
        let bytes = report_csv(&table(), true).into_bytes();
        let mut want = vec![0xEF, 0xBB, 0xBF];
        want.extend_from_slice("Tag;Note\r\nK1;Relé, 24 V\r\n".as_bytes());
        assert_eq!(bytes, want);
    }

    #[test]
    fn plain_csv_is_comma_separated_without_a_bom() {
        assert_eq!(report_csv(&table(), false), "Tag,Note\r\nK1,\"Relé, 24 V\"\r\n");
    }
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

/// The shown panel's id and its picked item with that item's place, kept before a shared-session
/// sync so `follow_panel_pick` can find them again afterwards.
fn panel_pick_before(
    ed: &Editor,
    project: &wc_core::Project,
) -> Option<(String, Option<(usize, wc_core::panel::PanelItem)>)> {
    let p = project.panels.get(ed.panel?)?;
    let pick = ed.panel_pick.and_then(|i| Some((i, p.items.get(i)?.clone())));
    Some((p.id.clone(), pick))
}

/// After a shared-session sync: show panel `id` and pick its item again, wherever they are now.
/// A device is found by its tag; a rail or duct by its kind and exact place and size, at its old
/// place in the list when it is still there. A panel or item that is gone is no longer shown or
/// picked.
fn follow_panel_pick(
    ed: &mut Editor,
    project: &wc_core::Project,
    id: &str,
    pick: Option<&(usize, wc_core::panel::PanelItem)>,
) {
    use wc_core::panel::PanelItem;
    let Some((pi, panel)) = project.panels.iter().enumerate().find(|(_, p)| p.id == id) else {
        ed.panel = None;
        ed.panel_pick = None;
        return;
    };
    ed.panel = Some(pi);
    ed.panel_pick = pick.and_then(|(old, item)| match item {
        PanelItem::Device { tag, .. } => panel
            .items
            .iter()
            .position(|i| matches!(i, PanelItem::Device { tag: t, .. } if t == tag)),
        PanelItem::Rail { .. } | PanelItem::Duct { .. } => {
            if panel.items.get(*old) == Some(item) {
                Some(*old)
            } else {
                panel.items.iter().position(|i| i == item)
            }
        }
    });
}

/// Who holds the page action `a` would change, in a shared session: then it is refused. Page
/// actions (properties, delete, move up or down) are about the current page even while a panel
/// is shown; canvas actions only when the page itself is shown, as in the panel view they work
/// on the panel.
fn held_for(a: Action, ed: &Editor, project: &wc_core::Project) -> Option<String> {
    // With a panel shown these still work on the schematic page under it (Delete and Rotate go
    // to the panel's picked device instead), so they follow that page's hold.
    let on_page_under_panel =
        ed.panel.is_some() && matches!(a, Action::Paste | Action::Duplicate | Action::Cut | Action::Mirror);
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
    fn arrow_keys_step_the_selection_by_the_snap() {
        use wc_core::geom::Point;
        let ctx = egui::Context::default();
        // `frame` is the frame's modifiers, `keys` each press with its own.
        let press = |frame: Modifiers, keys: &[(Key, Modifiers)]| {
            let raw = egui::RawInput {
                modifiers: frame,
                events: keys
                    .iter()
                    .map(|&(key, modifiers)| egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    })
                    .collect(),
                ..Default::default()
            };
            let mut got = Vec::new();
            let _ = ctx.run(raw, |ctx| got = ctx.input(|i| arrow_steps(i, 2.5)));
            got
        };
        let none = Modifiers::NONE;
        assert_eq!(press(none, &[(Key::ArrowLeft, none)]), [Point::new(-2.5, 0.0)]);
        assert_eq!(press(none, &[(Key::ArrowDown, none)]), [Point::new(0.0, 2.5)]);
        assert_eq!(
            press(none, &[(Key::ArrowRight, none), (Key::ArrowUp, none)]),
            [Point::new(2.5, 0.0), Point::new(0.0, -2.5)]
        );
        // Two presses of one key in a frame are two steps.
        assert_eq!(
            press(none, &[(Key::ArrowRight, none), (Key::ArrowRight, none)]),
            [Point::new(2.5, 0.0), Point::new(2.5, 0.0)]
        );
        assert!(press(none, &[(Key::A, none)]).is_empty());
        assert!(
            press(Modifiers::COMMAND, &[(Key::ArrowLeft, Modifiers::COMMAND)]).is_empty(),
            "Ctrl+arrow is not a nudge"
        );
        // Each press's own modifiers count, not the frame's: a Ctrl+arrow in a frame that ends
        // with no modifier held is not a nudge, a plain arrow after Ctrl was let go is.
        assert!(press(none, &[(Key::ArrowLeft, Modifiers::COMMAND)]).is_empty());
        assert_eq!(
            press(
                Modifiers::COMMAND,
                &[(Key::ArrowLeft, none), (Key::ArrowUp, Modifiers::COMMAND)]
            ),
            [Point::new(-2.5, 0.0)]
        );
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
    fn a_queued_parts_picker_is_for_its_project_and_dropped_after_a_switch() {
        let (mut pending, mut pending_for) = (None, None);
        let a = PathBuf::from("a");
        let parts = Pending::Import(imports::Kind::PartsCsv, None);
        assert!(queue_pending(&mut pending, &mut pending_for, parts, Some(a.clone())));
        // The first picker stays while it waits.
        assert!(!queue_pending(
            &mut pending,
            &mut pending_for,
            Pending::Open,
            Some(a.clone())
        ));
        let Some(p) = pending.take() else {
            unreachable!("the parts picker was not queued");
        };
        assert!(matches!(p, Pending::Import(imports::Kind::PartsCsv, None)));
        assert!(pending_applies(&p, pending_for.as_deref(), Some(a.as_path())));
        assert!(!pending_applies(&p, pending_for.as_deref(), Some(Path::new("b"))));
        assert!(!pending_applies(&p, pending_for.as_deref(), None));
        assert!(pending_applies(&Pending::Open, None, Some(Path::new("b"))));
    }

    #[test]
    fn a_picker_that_never_showed_says_so_instead_of_doing_nothing() {
        use std::time::Duration;
        let d = PathBuf::from("/x");
        assert_eq!(picker_outcome(Some(d.clone()), Duration::ZERO, &[]), Picked::Path(d));
        assert_eq!(picker_outcome(None, Duration::from_secs(2), &[]), Picked::Cancelled);
        // An answer at once with nothing logged is said, but it may be a quick Cancel: it is
        // not a failure an export writes to its usual place on.
        assert!(matches!(
            picker_outcome(None, Duration::from_millis(5), &[]),
            Picked::Unsure(e) if e.contains("desktop portal")
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
    fn browse_starts_in_the_folder_picked_even_when_its_name_ends_in_a_space() {
        let tmp = tempfile::tempdir().unwrap();
        let spaced = tmp.path().join("Shared copy ");
        // Windows drops a trailing space from a folder name: nothing to test there.
        if std::fs::create_dir(&spaced).is_err()
            || !std::fs::read_dir(tmp.path())
                .unwrap()
                .any(|e| e.is_ok_and(|e| e.file_name() == "Shared copy "))
        {
            return;
        }
        let trimmed = tmp.path().join("Shared copy");
        std::fs::create_dir(&trimmed).unwrap();
        let mut field = prefs::FolderField::default();
        field.pick(spaced.clone()).unwrap();
        assert_eq!(field_picker_start(&field), Some(spaced));
        // Typed text is read as the dialogs read it (trimmed).
        let typed = prefs::FolderField::typed(format!("{} ", trimmed.display()));
        assert_eq!(field_picker_start(&typed), Some(trimmed));
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
    fn an_export_goes_only_where_the_save_dialog_says_and_nowhere_on_cancel() {
        let (tmp, o) = state::tests::project();
        let usual = export_file(&o, "aml");
        let cancel = |_: &str, _: &Path, _: &str, _: Option<(&str, &[&str])>| Picked::Cancelled;
        assert!(export_whole(&o, Export::Aml, &cancel).is_none());
        assert!(!usual.exists() && !usual.parent().unwrap().exists());

        let picked = tmp.path().join("elsewhere").join("plant.aml");
        let to = picked.clone();
        let pick = move |_: &str, _: &Path, _: &str, _: Option<(&str, &[&str])>| Picked::Path(to.clone());
        let msg = export_whole(&o, Export::Aml, &pick).unwrap();
        assert!(msg.contains("Exported"), "{msg}");
        assert!(picked.is_file());
        assert!(!usual.exists());

        // A dialog that closed at once without a file may have been cancelled: it says so and
        // writes nothing.
        let unsure = |_: &str, _: &Path, _: &str, _: Option<(&str, &[&str])>| Picked::Unsure("At once.".into());
        let msg = export_whole(&o, Export::Aml, &unsure).unwrap();
        assert!(msg.contains("Nothing was written"), "{msg}");
        assert!(!usual.exists());

        // Without a dialog it goes to the usual place, and the note names it without claiming
        // it was saved before the export says so.
        let failed = |_: &str, _: &Path, _: &str, _: Option<(&str, &[&str])>| Picked::Failed("No dialog.".into());
        let msg = export_whole(&o, Export::Aml, &failed).unwrap();
        assert!(msg.contains("So it goes to the usual place"), "{msg}");
        assert!(usual.is_file());
    }

    #[test]
    fn attaching_a_footprint_or_model_without_a_file_picker_offers_a_typed_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // The picker is asked for the kind of file each button attaches, in the project folder.
        let seen = std::cell::RefCell::new(Vec::new());
        let failed = |title: &str, at: &Path, filter: (&str, &[&str])| {
            seen.borrow_mut()
                .push((title.to_owned(), at.to_path_buf(), filter.1.to_vec().join(",")));
            Picked::Failed("The file picker could not open.".into())
        };
        for kind in [panels::Attach::Footprint, panels::Attach::Model] {
            // Without a picker the form to type the path opens, with the reason and for the part.
            let Some(Err(Dialog::AttachPath {
                kind: k,
                part,
                file,
                error,
            })) = attach_target(kind, "io.8", dir, &failed)
            else {
                unreachable!("no typed path form");
            };
            assert_eq!((k, part.as_str(), file.text.as_str()), (kind, "io.8", ""));
            let error = error.unwrap_or_default();
            assert!(
                error.contains("could not open") && error.contains("Type the file's path"),
                "{error}"
            );
        }
        assert_eq!(
            *seen.borrow(),
            [
                ("Footprint drawing".to_owned(), dir.to_path_buf(), "dxf".to_owned()),
                ("3D model".to_owned(), dir.to_path_buf(), "step,stp,STEP,STP".to_owned()),
            ]
        );
        // A picker that perhaps never showed offers the form too.
        let unsure = |_: &str, _: &Path, _: (&str, &[&str])| Picked::Unsure("At once.".into());
        assert!(matches!(
            attach_target(panels::Attach::Model, "io.8", dir, &unsure),
            Some(Err(Dialog::AttachPath { .. }))
        ));
        // A picked file is attached; Cancel does nothing.
        let dxf = dir.join("io-8.dxf");
        let picked = |_: &str, _: &Path, _: (&str, &[&str])| Picked::Path(dxf.clone());
        assert!(matches!(
            attach_target(panels::Attach::Footprint, "io.8", dir, &picked),
            Some(Ok(f)) if f == dxf
        ));
        let cancel = |_: &str, _: &Path, _: (&str, &[&str])| Picked::Cancelled;
        assert!(attach_target(panels::Attach::Footprint, "io.8", dir, &cancel).is_none());

        // The typed path must name a file.
        let typed = prefs::FolderField::typed(dxf.display().to_string());
        // A missing file says why it cannot be read; a folder is not a file.
        let missing = typed_file(&typed).unwrap_err();
        assert!(missing.starts_with(&format!("{}: ", dxf.display())), "{missing}");
        let folder = prefs::FolderField::typed(dir.display().to_string());
        assert!(typed_file(&folder).unwrap_err().contains("is not a file"));
        std::fs::write(&dxf, "0\nEOF\n").unwrap();
        assert_eq!(typed_file(&typed), Ok(dxf.clone()));
        assert!(typed_file(&prefs::FolderField::default()).is_err());
    }

    #[test]
    fn the_marking_options_come_back_after_a_cancelled_save_dialog() {
        let (_tmp, o) = state::tests::project();
        let options = wc_report::MarkingOptions {
            kinds: vec![wc_report::MarkerKind::Terminals],
            lines: vec![wc_report::LineField::Marker, wc_report::LineField::Description],
            language: wc_report::Language::De,
            strip: Some("-X1".into()),
            level_separator: "/".into(),
        };
        // Through the export itself, with its save dialog cancelled.
        let cancel = |_: &str, _: &Path, _: &str, _: Option<(&str, &[&str])>| Picked::Cancelled;
        let Dialog::WagoMarking {
            kinds,
            lines,
            language,
            strip,
            level_separator,
            ..
        } = export_markers(&o, &options, &cancel)
        else {
            unreachable!("not the marking dialog");
        };
        assert_eq!(kinds, [true, false, false, false]);
        assert_eq!(
            lines,
            [
                Some(wc_report::LineField::Marker),
                Some(wc_report::LineField::Description),
                None
            ]
        );
        assert_eq!(language, wc_report::Language::De);
        assert_eq!(level_separator, "/");
        assert_eq!(strip.as_deref(), Some("-X1"));
        assert!(!o.dir.join("export").exists(), "nothing written");
    }

    #[test]
    fn pages_whose_numbers_differ_only_in_case_get_their_own_drawing_files() {
        let (_tmp, mut o) = state::tests::project();
        o.project.pages[0].number = "A1".into();
        o.project.pages[1].number = "a1".into();
        let own = o.dir.join("export").join("drawings");
        assert!(export_drawings_to(&o, &own).starts_with("Exported 2 pages"));
        let mut names: Vec<String> = std::fs::read_dir(&own)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_lowercase())
            .collect();
        names.sort();
        assert_eq!(names, ["a1.dxf", "a1.svg", "a1_.dxf", "a1_.svg"]);
    }

    #[test]
    fn drawings_include_a_file_per_panel_layout() {
        let (_tmp, mut o) = state::tests::project();
        o.project.pages.truncate(1);
        o.project.pages[0].number = "1".into();
        o.project.panels.push(wc_core::panel::Panel::with_rails(
            "plate1",
            "Main cabinet",
            600.0,
            800.0,
        ));
        let own = o.dir.join("export").join("drawings");
        let msg = export_drawings_to(&o, &own);
        assert!(msg.starts_with("Exported 1 pages and 1 panel layout(s)"), "{msg}");
        let mut names: Vec<String> = std::fs::read_dir(&own)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["1.dxf", "1.svg", "panel-Main_cabinet.dxf", "panel-Main_cabinet.svg"]
        );
    }

    #[test]
    fn drawings_replace_their_own_folder_also_when_it_is_picked_by_another_route() {
        let (_tmp, o) = state::tests::project();
        let own = o.dir.join("export").join("drawings");
        assert!(export_drawings_to(&o, &own).starts_with("Exported"));
        std::fs::create_dir_all(o.dir.join("export").join("other")).unwrap();
        let around = o.dir.join("export").join("other").join("..").join("drawings");
        let msg = export_drawings_to(&o, &around);
        assert!(msg.starts_with("Exported"), "{msg}");
        assert!(same_dir(&around, &own));
        assert!(!same_dir(&o.dir.join("export").join("other"), &own));
        // Not there yet: compared as written.
        assert!(same_dir(&o.dir.join("missing"), &o.dir.join("missing")));
    }

    #[test]
    fn drawings_replace_their_own_folder_but_not_other_files_in_a_picked_one() {
        let (tmp, o) = state::tests::project();
        let own = o.dir.join("export").join("drawings");
        assert!(export_drawings_to(&o, &own).starts_with("Exported"));
        assert!(
            export_drawings_to(&o, &own).starts_with("Exported"),
            "a new export replaces the last"
        );

        let other = tmp.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        let name = std::fs::read_dir(&own).unwrap().next().unwrap().unwrap().file_name();
        std::fs::write(other.join(&name), "someone else's").unwrap();
        let msg = export_drawings_to(&o, &other);
        assert!(
            msg.starts_with("Nothing exported") && msg.contains("not replaced"),
            "{msg}"
        );
        assert_eq!(std::fs::read_to_string(other.join(&name)).unwrap(), "someone else's");
        assert_eq!(std::fs::read_dir(&other).unwrap().count(), 1, "nothing else written");

        let empty = tmp.path().join("empty");
        assert!(export_drawings_to(&o, &empty).starts_with("Exported"));
    }

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
        assert_eq!(rows, ["\u{feff}Index", "-X1/T:1", "-X1/T:2", "-X2/T:1"]);
        assert!(csv.contains("-X1/T:1;Terminal;-X1;1;1;"), "{csv}");
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
            .replace("-X1/T:1;Terminal;-X1;1;1;;;", "-X1/T:1;Terminal;-X1;1;1;;EX-TB-2.5;")
            .replace("-X1/T:2;Terminal;-X1;2;2;;;", "-X1/T:2;Terminal;-X1;2;2;;NO-SUCH;")
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

/// How the connection point dialog names an edge of a black box.
fn side_name(side: wc_core::BoxSide) -> &'static str {
    match side {
        wc_core::BoxSide::Top => "top",
        wc_core::BoxSide::Bottom => "bottom",
        wc_core::BoxSide::Left => "left",
        wc_core::BoxSide::Right => "right",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod nudge_tests {
    use super::*;
    use wc_core::geom::Point;

    /// An app with the test project open on its second page, a contact at (50, 20) and a coil
    /// at `coil` on it, the coil selected; snap 5 mm.
    fn app(coil: Point, auto_wire: bool) -> (tempfile::TempDir, App, egui::Context) {
        let (tmp, mut open) = state::tests::project();
        let page = open.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        open.place(page, "wc.contact.no", Point::new(50.0, 20.0)).unwrap();
        let k = open.place(page, "wc.coil", coil).unwrap();
        let prefs = Prefs {
            snap: 5.0,
            auto_wire,
            ..Prefs::default()
        };
        let mut ed = Editor::default();
        ed.page = page;
        ed.select_only(Selection::Symbol(k));
        let app = App {
            prefs,
            open: Some(open),
            ed,
            chrome: Default::default(),
            panels: Default::default(),
            dialog: None,
            clipboard: Default::default(),
            copied_text: None,
            applied_theme: theme::Mode::Light,
            assistant: Default::default(),
            open_url: Vec::new(),
            logged: Default::default(),
            share: Default::default(),
            symbol_editor: None,
            find: None,
            pending: None,
            pending_for: None,
            strip_editor: None,
            show_layers: false,
            live: Default::default(),
            fetch: None,
            importing: None,
        };
        (tmp, app, egui::Context::default())
    }

    /// One frame with `key` pressed; `focus_text` gives a text field the keyboard first.
    fn press(app: &mut App, ctx: &egui::Context, key: Key, focus_text: bool) {
        let raw = egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            if focus_text {
                ctx.memory_mut(|m| m.request_focus(egui::Id::new("a text field")));
            }
            app.nudge_by_arrows(ctx);
        });
    }

    fn coil_at(app: &App) -> Point {
        let open = app.open.as_ref().unwrap();
        open.project.pages[app.ed.page].symbols[1].at
    }

    fn wires(app: &App) -> usize {
        let open = app.open.as_ref().unwrap();
        open.project.pages[app.ed.page].wires.len()
    }

    #[test]
    fn an_arrow_press_through_the_app_frame_moves_the_selection() {
        let (_tmp, mut app, ctx) = app(Point::new(80.0, 60.0), false);
        let mut frame = eframe::Frame::_new_kittest();
        // As App::new does; the fonts take effect from the next frame.
        theme::install_fonts(&ctx);
        theme::apply(&ctx, app.prefs.theme);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            eframe::App::update(&mut app, ctx, &mut frame)
        });
        assert_eq!(coil_at(&app), Point::new(80.0, 60.0));
        let raw = egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::ArrowRight,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| eframe::App::update(&mut app, ctx, &mut frame));
        assert_eq!(
            coil_at(&app),
            Point::new(85.0, 60.0),
            "the app's frame nudges the selection"
        );
    }

    #[test]
    fn each_arrow_press_is_one_snap_step_and_one_undo_step() {
        let (_tmp, mut app, ctx) = app(Point::new(80.0, 60.0), false);
        press(&mut app, &ctx, Key::ArrowRight, false);
        assert_eq!(coil_at(&app), Point::new(85.0, 60.0));
        press(&mut app, &ctx, Key::ArrowDown, false);
        assert_eq!(coil_at(&app), Point::new(85.0, 65.0));
        assert!(app.open.as_mut().unwrap().undo());
        assert_eq!(coil_at(&app), Point::new(85.0, 60.0), "one undo takes back one press");
        assert!(app.open.as_mut().unwrap().undo());
        assert_eq!(
            coil_at(&app),
            Point::new(80.0, 60.0),
            "the second undo takes back the first press"
        );
    }

    #[test]
    fn nothing_moves_while_a_text_field_has_the_keyboard_or_a_drag_runs() {
        let (_tmp, mut app, ctx) = app(Point::new(80.0, 60.0), false);
        let revision = app.open.as_ref().unwrap().revision();
        press(&mut app, &ctx, Key::ArrowRight, true);
        assert_eq!(coil_at(&app), Point::new(80.0, 60.0));
        app.ed.start_move_drag();
        press(&mut app, &ctx, Key::ArrowRight, false);
        assert_eq!(
            coil_at(&app),
            Point::new(80.0, 60.0),
            "no nudge in the middle of a move drag"
        );
        // Nothing selected (and the drag over): a press takes no undo step.
        let page = app.ed.page;
        app.ed = Editor::default();
        app.ed.page = page;
        press(&mut app, &ctx, Key::ArrowRight, false);
        assert_eq!(
            app.open.as_ref().unwrap().revision(),
            revision,
            "no press took an undo step or changed the project"
        );
    }

    #[test]
    fn a_nudge_wires_pins_that_line_up_only_with_auto_wiring_on() {
        for on in [true, false] {
            let (_tmp, mut app, ctx) = app(Point::new(55.0, 60.0), on);
            press(&mut app, &ctx, Key::ArrowLeft, false);
            assert_eq!(coil_at(&app), Point::new(50.0, 60.0));
            assert_eq!(wires(&app), usize::from(on), "auto-wiring {on}");
            // The wire is part of the press's undo step.
            assert!(app.open.as_mut().unwrap().undo());
            assert_eq!((coil_at(&app), wires(&app)), (Point::new(55.0, 60.0), 0));
        }
    }

    #[test]
    fn delete_waits_for_a_panel_device_drag_to_end() {
        use wc_core::panel::{Panel, PanelItem};
        let (_tmp, mut app, _ctx) = app(Point::new(80.0, 60.0), false);
        let open = app.open.as_mut().unwrap();
        open.project.panels.push(Panel {
            id: "panel1".into(),
            name: "Panel 1".into(),
            width: 400.0,
            height: 300.0,
            items: vec![PanelItem::Device {
                tag: wc_core::DeviceTag::parse("-K1").unwrap(),
                at: Point::new(100.0, 100.0),
                width: 40.0,
                height: 60.0,
                rotation: wc_core::geom::Rotation::R0,
            }],
        });
        let panel = open.project.panels.len() - 1;
        app.ed.panel = Some(panel);
        app.ed.panel_pick = Some(0);
        let items = |app: &App| app.open.as_ref().unwrap().project.panels[panel].items.len();
        app.ed.panel_drag = Some((Point::new(100.0, 100.0), None, Point::new(0.0, 0.0)));
        app.carry_out(Action::Delete);
        assert_eq!(items(&app), 1, "the held device stays");
        assert_eq!(app.ed.panel_pick, Some(0), "and stays picked");
        assert!(app.ed.message.contains("Let go"), "{}", app.ed.message);
        app.ed.panel_drag = None;
        app.carry_out(Action::Delete);
        assert_eq!(items(&app), 0, "after letting go, Delete removes it");
    }

    #[test]
    fn an_arrow_press_through_the_app_frame_leaves_the_panel_device_alone_behind_a_dialog() {
        use wc_core::panel::{Panel, PanelItem};
        let (_tmp, mut app, ctx) = app(Point::new(80.0, 60.0), false);
        let open = app.open.as_mut().unwrap();
        open.project.panels.push(Panel {
            id: "panel1".into(),
            name: "Panel 1".into(),
            width: 400.0,
            height: 300.0,
            items: vec![PanelItem::Device {
                tag: wc_core::DeviceTag::parse("-K1").unwrap(),
                at: Point::new(100.0, 100.0),
                width: 40.0,
                height: 60.0,
                rotation: wc_core::geom::Rotation::R0,
            }],
        });
        let panel = open.project.panels.len() - 1;
        app.ed.panel = Some(panel);
        app.ed.panel_pick = Some(0);
        let mut frame = eframe::Frame::_new_kittest();
        theme::install_fonts(&ctx);
        theme::apply(&ctx, app.prefs.theme);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            eframe::App::update(&mut app, ctx, &mut frame)
        });
        let device_at = |app: &App| {
            let open = app.open.as_ref().unwrap();
            match open.project.panels[panel].items.first() {
                Some(PanelItem::Device { at, .. }) => *at,
                _ => unreachable!("the device is there"),
            }
        };
        let undo_steps = |app: &App| app.open.as_ref().unwrap().undo_steps();
        let right = || egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::ArrowRight,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            ..Default::default()
        };

        // A message box is open: the press leaves the device and the undo history alone.
        app.dialog = Some(Dialog::Message("Something to read.".into()));
        let steps = undo_steps(&app);
        let _ = ctx.run(right(), |ctx| eframe::App::update(&mut app, ctx, &mut frame));
        assert!(app.dialog.is_some(), "the message box is still open");
        assert_eq!(device_at(&app), Point::new(100.0, 100.0));
        assert_eq!(undo_steps(&app), steps);

        // With the message box closed, the same press moves the device one step.
        app.dialog = None;
        let _ = ctx.run(right(), |ctx| eframe::App::update(&mut app, ctx, &mut frame));
        assert_eq!(device_at(&app), Point::new(105.0, 100.0));
        assert_eq!(undo_steps(&app), steps + 1);
        assert_eq!(
            coil_at(&app),
            Point::new(80.0, 60.0),
            "the page behind the panel stays put"
        );
    }
}

#[cfg(test)]
mod held_tests {
    use super::*;

    #[test]
    fn rotate_on_a_panel_is_not_held_by_the_page_under_it() {
        let mut project = wc_core::Project::new("x");
        project.pages.push(wc_core::Page::new("p1", "1", "Mains"));
        let mut ed = Editor::default();
        ed.page = project.pages.len() - 1;
        ed.held_by.insert("p1".into(), "Kim".into());
        // On the page itself, Rotate turns its symbols: held.
        assert_eq!(held_for(Action::Rotate, &ed, &project).as_deref(), Some("Kim"));
        // With a panel shown, Rotate turns the picked panel device: not held; Paste still is.
        ed.panel = Some(0);
        ed.panel_pick = Some(0);
        assert_eq!(held_for(Action::Rotate, &ed, &project), None);
        assert_eq!(held_for(Action::Paste, &ed, &project).as_deref(), Some("Kim"));
    }

    #[test]
    fn the_panel_pick_follows_its_device_after_a_sync() {
        use wc_core::panel::{Panel, PanelItem};
        let device = |tag: &str, x: f64| PanelItem::Device {
            tag: wc_core::DeviceTag::parse(tag).unwrap_or_default(),
            at: wc_core::Point::new(x, 0.0),
            width: 10.0,
            height: 10.0,
            rotation: wc_core::geom::Rotation::R0,
        };
        let mut project = wc_core::Project::new("x");
        let mut a = Panel::new("a", "A", 100.0, 100.0);
        a.items = vec![device("-K2", 30.0), device("-K1", 0.0)];
        project.panels = vec![Panel::new("0", "first", 10.0, 10.0), a];
        let mut ed = Editor::default();
        // Picked at place 0 before the sync.
        let k1 = Some((0, device("-K1", 0.0)));
        // Another panel came first and the devices were reordered: still panel "a", still -K1.
        follow_panel_pick(&mut ed, &project, "a", k1.as_ref());
        assert_eq!((ed.panel, ed.panel_pick), (Some(1), Some(1)));
        // The device was taken off the plate: nothing picked, so Rotate and Delete do nothing.
        project.panels[1].items.remove(1);
        follow_panel_pick(&mut ed, &project, "a", k1.as_ref());
        assert_eq!((ed.panel, ed.panel_pick), (Some(1), None));
        // The panel was removed.
        follow_panel_pick(&mut ed, &project, "gone", k1.as_ref());
        assert_eq!((ed.panel, ed.panel_pick), (None, None));
    }

    #[test]
    fn a_picked_rail_or_duct_stays_picked_after_a_sync_and_delete_takes_it() {
        use wc_core::panel::{Panel, PanelItem};
        let rail = |y: f64| PanelItem::Rail {
            at: wc_core::Point::new(20.0, y),
            length: 560.0,
        };
        let duct = PanelItem::Duct {
            min: wc_core::Point::new(0.0, 0.0),
            max: wc_core::Point::new(600.0, 60.0),
        };
        let (_tmp, mut o) = crate::state::tests::project();
        let mut panel = Panel::new("panel1", "Panel 1", 600.0, 800.0);
        panel.items = vec![duct.clone(), rail(150.0), rail(300.0)];
        o.project.panels.push(panel);
        let mut ed = Editor::default();
        ed.panel = Some(0);
        for picked in [1, 0] {
            ed.panel_pick = Some(picked);
            // A sync that changed nothing keeps the pick.
            let Some((id, pick)) = panel_pick_before(&ed, &o.project) else {
                unreachable!("a panel is shown")
            };
            follow_panel_pick(&mut ed, &o.project, &id, pick.as_ref());
            assert_eq!((ed.panel, ed.panel_pick), (Some(0), Some(picked)));
        }
        // Delete then takes the picked item off in one undo step.
        let steps = o.undo_steps();
        let (Some(pi), Some(item)) = (ed.panel, ed.panel_pick.take()) else {
            unreachable!("the duct is picked")
        };
        assert!(o.delete_panel_item(pi, item));
        assert_eq!(o.undo_steps(), steps + 1);
        assert_eq!(o.project.panels[0].items, vec![rail(150.0), rail(300.0)]);

        // Another person put a duct first: the same rail is found at its new place.
        ed.panel_pick = Some(1);
        let pick = panel_pick_before(&ed, &o.project).and_then(|(_, p)| p);
        o.project.panels[0].items.insert(0, duct.clone());
        follow_panel_pick(&mut ed, &o.project, "panel1", pick.as_ref());
        assert_eq!(ed.panel_pick, Some(2));
        // The rail was moved or removed by the other person: nothing picked.
        o.project.panels[0].items[2] = rail(310.0);
        follow_panel_pick(&mut ed, &o.project, "panel1", pick.as_ref());
        assert_eq!(ed.panel_pick, None);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod menu_action_tests {
    use super::*;

    /// An app on a test project with one symbol placed and selected on the first page.
    fn app() -> (tempfile::TempDir, App, String) {
        let (tmp, mut open) = crate::state::tests::project();
        let id = open.place(0, "wc.coil", wc_core::Point::new(90.0, 60.0)).unwrap();
        let mut ed = Editor::default();
        ed.selection = vec![Selection::Symbol(id.clone())];
        let app = App {
            prefs: Prefs::default(),
            open: Some(open),
            ed,
            chrome: Default::default(),
            panels: Default::default(),
            dialog: None,
            clipboard: Default::default(),
            copied_text: None,
            applied_theme: crate::theme::Mode::Light,
            assistant: Default::default(),
            open_url: Vec::new(),
            logged: Default::default(),
            share: Default::default(),
            symbol_editor: None,
            find: None,
            pending: None,
            pending_for: None,
            strip_editor: None,
            show_layers: false,
            live: Default::default(),
            fetch: None,
            importing: None,
        };
        (tmp, app, id)
    }

    fn rotation(a: &App, id: &str) -> wc_core::geom::Rotation {
        let open = a.open.as_ref().unwrap();
        open.project.pages[0]
            .symbols
            .iter()
            .find(|s| s.id == id)
            .unwrap()
            .rotation
    }

    #[test]
    fn rotate_picked_in_the_menu_turns_the_selected_symbol() {
        let (_tmp, mut a, id) = app();
        assert_eq!(rotation(&a, &id), wc_core::geom::Rotation::R0);
        a.ed.menu_action = Some(Action::Rotate);
        a.run_menu_action();
        assert_eq!(rotation(&a, &id), wc_core::geom::Rotation::R90);
        assert!(a.ed.menu_action.is_none(), "run once");
        assert!(a.open.as_ref().unwrap().can_undo());
    }

    #[test]
    fn rotate_picked_in_the_menu_on_a_held_page_changes_nothing() {
        let (_tmp, mut a, id) = app();
        let page_id = a.open.as_ref().unwrap().project.pages[0].id.clone();
        a.ed.held_by.insert(page_id, "Kim".into());
        let before = a.open.as_ref().unwrap().project.pages[0].clone();
        a.ed.menu_action = Some(Action::Rotate);
        a.run_menu_action();
        assert_eq!(rotation(&a, &id), wc_core::geom::Rotation::R0);
        assert_eq!(a.open.as_ref().unwrap().project.pages[0], before);
        assert!(a.ed.message.contains("Kim has this page open"), "{}", a.ed.message);
        assert!(a.ed.menu_action.is_none());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod import_folder_tests {
    use super::*;

    #[test]
    fn a_blank_library_field_is_refused_not_taken_as_the_projects_folder() {
        let blank = prefs::FolderField::typed("  ".to_owned());
        assert!(typed_library(true, &blank).is_err());
        // Not asked: the field is not read at all.
        assert_eq!(typed_library(false, &blank), Ok(None));
        let typed = prefs::FolderField::typed("parts".to_owned());
        assert!(matches!(typed_library(true, &typed), Ok(Some(_))));
    }

    #[test]
    fn a_new_folder_chosen_for_parts_is_declared_and_a_listed_one_is_not() {
        let t = tempfile::tempdir().unwrap();
        let project = t.path().join("proj");
        let listed = project.join("library");
        let other = t.path().join("vendor");
        std::fs::create_dir_all(&listed).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let declared = vec![wc_core::project::LibrarySource {
            name: "vendor".into(),
            kind: "folder".into(),
            location: "library".into(),
            reference: None,
        }];
        // Listed already (relative to the project): nothing to add.
        assert_eq!(new_folder_source(&project, &listed, &declared), None);
        // New: added as a folder source, its name made unique among the declared ones.
        let got = new_folder_source(&project, &other, &declared).unwrap();
        assert_eq!(got.kind, "folder");
        assert_eq!(got.name, "vendor-2");
        assert_eq!(Path::new(&got.location), other.as_path());
    }
}

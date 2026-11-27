//! The open project and everything the user is doing with it, independent of drawing.

use eframe::egui;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wc_core::geom::{Point, Rotation};
use wc_core::lock::ItemKind;
use wc_core::plc_rack::PlcRack;
use wc_core::project::{PageNumberError, PageSize};
use wc_core::route::{self, Axis, Route};
use wc_core::template::PageTemplate;
use wc_core::{DevicePart, DeviceTag, Endpoint, Page, Part, PlacedSymbol, Project, Symbol, Wire, store};

/// How a part is listed in the picker: manufacturer, order number and description.
pub fn part_label(p: &Part) -> String {
    let head: Vec<&str> = [p.manufacturer.as_str(), p.order_number.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    match (head.is_empty(), p.description.is_empty()) {
        (true, true) => p.id.clone(),
        (true, false) => p.description.clone(),
        (false, true) => head.join(" "),
        (false, false) => format!("{} \u{2013} {}", head.join(" "), p.description),
    }
}

/// The parts the project uses that `parts` does not hold but whose copy in the project's
/// library (`vendor/parts/`) is there and cannot be read, by id with the reason: the project
/// check reports those as unreadable, not as missing
/// ([`wc_report::check_with_unreadable`]). An id that cannot name a file there at all is
/// unreadable too, with the reason the id is refused.
pub fn unreadable_parts(dir: &Path, project: &Project, parts: &BTreeMap<String, Part>) -> BTreeMap<String, String> {
    unreadable_parts_looked_up(dir, project, parts, Path::try_exists)
}

/// [`unreadable_parts`], with `exists` telling whether a copy's file is there: tests give a
/// lookup that fails, which permission bits cannot do when they run as root.
fn unreadable_parts_looked_up(
    dir: &Path,
    project: &Project,
    parts: &BTreeMap<String, Part>,
    exists: impl Fn(&Path) -> std::io::Result<bool>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for id in project.used_parts() {
        if parts.contains_key(&id) {
            continue;
        }
        let path = match store::vendor_path(dir, ItemKind::Part, &id) {
            Ok(path) => path,
            Err(e) => {
                out.insert(id, e.to_string());
                continue;
            }
        };
        // Only a copy known to be absent is missing: one that cannot even be looked for (a
        // folder that may not be entered) is unreadable, with that reason.
        match exists(&path) {
            Ok(false) => {}
            Ok(true) => {
                if let Err(e) = store::read_vendored_part(dir, &id) {
                    out.insert(id, e.to_string());
                }
            }
            Err(e) => {
                out.insert(id, format!("{}: {e}", path.display()));
            }
        }
    }
    out
}

/// What a click on the canvas does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Place,
    Wire,
    Text,
    Line,
    Rect,
    Circle,
    Arc,
    Polyline,
    Dimension,
    Comment,
    /// A revision cloud: two corners.
    Cloud,
    /// The page's revision table: one click for its top-left corner.
    RevisionTable,
    BlackBox,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Selection {
    Symbol(String),
    Wire(String),
    /// A free drawing on the page (line, rectangle, circle, text), by its index in `graphics`.
    Graphic(usize),
    /// A revision cloud, by its index in the page's `clouds`.
    Cloud(usize),
    /// The page's revision table.
    RevisionTable,
}

/// One part of a wire's spec, edited on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecPart {
    Section,
    Colour,
    Kind,
}

impl SpecPart {
    pub const ALL: [SpecPart; 3] = [SpecPart::Section, SpecPart::Colour, SpecPart::Kind];

    pub fn label(self) -> &'static str {
        match self {
            SpecPart::Section => "Cross-section",
            SpecPart::Colour => "Colour",
            SpecPart::Kind => "Wire type",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            SpecPart::Section => "e.g. 1.5 mm²",
            SpecPart::Colour => "e.g. BU",
            SpecPart::Kind => "e.g. H07V-K",
        }
    }

    pub fn of(self, s: &wc_core::WireSpec) -> &Option<String> {
        match self {
            SpecPart::Section => &s.section,
            SpecPart::Colour => &s.colour,
            SpecPart::Kind => &s.kind,
        }
    }

    pub fn of_mut(self, s: &mut wc_core::WireSpec) -> &mut Option<String> {
        match self {
            SpecPart::Section => &mut s.section,
            SpecPart::Colour => &mut s.colour,
            SpecPart::Kind => &mut s.kind,
        }
    }
}

/// A macro found in the project or a library source.
#[derive(Debug, Clone)]
pub struct MacroEntry {
    /// `project` or the library source's name, as shown.
    pub source: String,
    /// Whether it is the project's own macro (a library's belongs to the library), whatever
    /// the library source is called.
    pub project: bool,
    pub path: PathBuf,
    pub name: String,
    pub description: String,
    /// Its value sets, by name: inserting it asks which one to use.
    pub value_sets: Vec<String>,
    /// Why the file could not be read, if it could not.
    pub error: Option<String>,
}

/// One revision as the project properties dialog edits it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RevisionRow {
    /// Its letter when the dialog opened; empty for a revision added in the dialog, whose
    /// letter is typed. A listed revision keeps its letter: clouds are tagged with it.
    pub was: String,
    pub letter: String,
    pub date: String,
    pub text: String,
    pub author: String,
}

impl RevisionRow {
    /// The rows for the project's revisions.
    pub fn of(revisions: &[wc_core::Revision]) -> Vec<RevisionRow> {
        revisions
            .iter()
            .map(|r| RevisionRow {
                was: r.letter.clone(),
                letter: r.letter.clone(),
                date: r.date.clone(),
                text: r.text.clone(),
                author: r.author.clone(),
            })
            .collect()
    }
}

/// What [`Open::number_wires`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WireNumbering {
    pub done: wc_core::numbering::Numbered,
    /// Numbers of the pages left alone because someone else has them open.
    pub skipped: Vec<String>,
}

impl WireNumbering {
    /// One line for the person who asked.
    pub fn report(&self) -> String {
        let mut s = format!(
            "Numbered {} connection(s); {} wire(s) changed. Existing numbers were kept.",
            self.done.connections, self.done.wires
        );
        if !self.skipped.is_empty() {
            s.push_str(&format!(
                " Page(s) {} were left as they are: someone else has them open. Number again once they are free.",
                self.skipped.join(", ")
            ));
        }
        s
    }
}

/// The project and its undo history, set aside by [`Open::set_aside`] and put back whole by
/// [`Open::put_back`]: a field added to the history belongs here, so that `without_changes` and
/// `back_to`, which take a change back through it, restore it. The library-update rollback
/// (`roll_back_update`) keeps its own copy and needs the field added there too.
struct Kept {
    project: Project,
    undo: Vec<Step>,
    redo: Vec<Step>,
    undo_dropped: usize,
    history: u64,
    form_step: Option<u64>,
    dirty: bool,
    terminals_before: Option<wc_core::project::TerminalNumbers>,
}

/// One undo or redo step: the project as it is on the other side of the change, and the
/// project files the change rewrote (a macro's placeholders saved or a translation set, say),
/// which Undo puts back and Redo writes again.
#[derive(Debug, Clone, PartialEq)]
struct Step {
    project: Project,
    files: Vec<FileEdit>,
    /// Tells this step apart from every other, for finding it again (see
    /// [`Open::take_back_move`]). A step keeps it when Undo and Redo move it between the two
    /// histories.
    id: u64,
    /// How far the running move drag had moved its items in `project` (see
    /// [`Open::drag_offset`]); zero outside a drag.
    offset: Point,
    /// Later steps were folded into this one (an assistant turn, a command): what happened in
    /// between is no longer in the history, so [`Open::take_back_move`] cannot check it.
    folded: bool,
}

impl Step {
    fn new(project: Project, files: Vec<FileEdit>, offset: Point) -> Step {
        Step {
            project,
            files,
            id: next_revision(),
            offset,
            folded: false,
        }
    }
}

/// A project file a change rewrote: what it held before and after, `None` when there was no
/// file (`translations.toml` before the first translation, say).
#[derive(Debug, Clone, PartialEq)]
struct FileEdit {
    /// Relative to the project folder, so after Save as it names the file in the copy.
    path: PathBuf,
    before: Option<String>,
    after: Option<String>,
}

/// Write `edits` (in project folder `dir`) as they were before them (`undo`, newest first) or
/// after them, each replaced whole ([`restore_file`]), so a failed write leaves the file as it
/// was; a file there was none of is removed. Nothing is written when a file was changed outside
/// since ([`changed_outside`]; `waiting` are the files that wait to be written again, each
/// taken to hold what it waits for). When one cannot be written, those already written are put
/// back as they were just before this call ([`put_back_files`]: one changed outside since it
/// was written stays as it is). The error names every file of `edits` left other than written:
/// the one that failed with its reason, those put back, those not put back with their reason,
/// and those not reached. Returns the files written, for [`put_back_files`] when what follows
/// fails. A failure also returns the files written already that a failed write did not put
/// back, each with what it should hold again (as it was just before), for
/// [`Open::keep_unwritten`]; one left alone because it was changed outside is not among them.
fn write_edits(dir: &Path, edits: &[FileEdit], undo: bool, waiting: &Waiting) -> Result<Written, (String, Unwritten)> {
    let order: Vec<&FileEdit> = if undo {
        edits.iter().rev().collect()
    } else {
        edits.iter().collect()
    };
    changed_outside(dir, &order, undo, waiting).map_err(|e| (e, Vec::new()))?;
    let mut written: Written = Vec::new();
    for (i, e) in order.iter().enumerate() {
        let text = if undo { &e.before } else { &e.after };
        let wrote = text.as_deref().map(|t| t.as_bytes().to_vec());
        let path = dir.join(&e.path);
        let done = snapshot_file(&path).and_then(|was| {
            restore_file(&path, wrote.as_deref())
                .map(|()| was)
                .map_err(|err| format!("{} could not be written: {err}", path.display()))
        });
        match done {
            // A file the step edits twice is put back once, as it was before the first write.
            Ok(was) => match written.iter_mut().find(|f| f.path == path) {
                Some(f) => f.wrote = wrote,
                None => written.push(WrittenFile { path, was, wrote }),
            },
            Err(mut msg) => {
                let (back, stuck, unwritten) = put_back_files(&written);
                let left: Vec<String> = order
                    .iter()
                    .skip(i + 1)
                    .map(|e| dir.join(&e.path).display().to_string())
                    .collect();
                for (what, files) in [
                    ("put back as it was", back),
                    ("written already and not put back", stuck),
                    ("not written", left),
                ] {
                    if !files.is_empty() {
                        msg.push_str(&format!("; {what}: {}", files.join("; ")));
                    }
                }
                return Err((msg, unwritten));
            }
        }
    }
    Ok(written)
}

/// The files of `order` (the edits of [`write_edits`], in the order it writes them, paths
/// relative to `dir`) that another program changed, each once (#657). The first edit of a file
/// is compared with the file: it must hold what that edit left there, the text after the edit
/// for `undo`, the text before it otherwise. Each later edit of the same file (a step made of
/// several changes, [`Open::squash_undo`]) is compared with what the edit written just before
/// it leaves there: when the two do not join, the file was changed outside between them, and
/// writing them one after the other would lose that change. A file of `waiting` (one that
/// waits to be written again, [`Open::keep_unwritten`]) is taken to hold what it waits for: it
/// holds what this window left there when the write failed, not another program's change. An
/// error when a file cannot be read.
fn outside_changes<'a>(
    dir: &Path,
    order: &[&'a FileEdit],
    undo: bool,
    waiting: &Waiting,
) -> Result<Vec<&'a PathBuf>, String> {
    let mut holds: BTreeMap<&PathBuf, &Option<String>> = BTreeMap::new();
    let mut changed: Vec<&PathBuf> = Vec::new();
    for e in order {
        let (expected, writes) = if undo {
            (&e.after, &e.before)
        } else {
            (&e.before, &e.after)
        };
        let same = match holds.insert(&e.path, writes) {
            Some(held) => held == expected,
            None => {
                let now = match waiting.get(&e.path) {
                    Some(should) => should.clone(),
                    None => snapshot_file(&dir.join(&e.path))?,
                };
                now.as_deref() == expected.as_deref().map(str::as_bytes)
            }
        };
        if !same && !changed.contains(&&e.path) {
            changed.push(&e.path);
        }
    }
    Ok(changed)
}

/// Refuse to write `order` (the edits of [`write_edits`], in the order it writes them) when
/// another program changed one of its files since ([`outside_changes`]): writing the recorded
/// text would lose that change.
fn changed_outside(dir: &Path, order: &[&FileEdit], undo: bool, waiting: &Waiting) -> Result<(), String> {
    let changed = outside_changes(dir, order, undo, waiting)?;
    if changed.is_empty() {
        return Ok(());
    }
    let changed: Vec<String> = changed.iter().map(|p| dir.join(p).display().to_string()).collect();
    Err(format!(
        "Not done, and nothing was changed: {} was changed outside this window since, and this would write over that change.",
        changed.join("; ")
    ))
}

/// A file [`write_edits`] wrote: as it was just before its first write of it (`was`) and what
/// its last write of it left there (`wrote`), `None` for no file.
#[derive(Debug)]
struct WrittenFile {
    path: PathBuf,
    was: Option<Vec<u8>>,
    wrote: Option<Vec<u8>>,
}

/// Files [`write_edits`] wrote, each once, in the order first written.
type Written = Vec<WrittenFile>;

/// Files a write failed on, each with what it should hold (`None`: no file).
type Unwritten = Vec<(PathBuf, Option<Vec<u8>>)>;

/// Project files that wait to be written again ([`Open::keep_unwritten`]), by path relative to
/// the project folder, each with what it should hold (`None`: no file).
type Waiting = BTreeMap<PathBuf, Option<Vec<u8>>>;

/// Put the files of `written` back as they were before [`write_edits`] wrote them, newest
/// first (a change made outside before that stays). A file that no longer holds what was
/// written was changed outside since: it is left as it is. Returns those put back, those that
/// were not with their reason, and the files a failed write left, each with what it should
/// hold. A file left alone because it was changed outside (or that cannot be read, so that is
/// not known) is not among the last: writing it later would lose that change.
fn put_back_files(written: &Written) -> (Vec<String>, Vec<String>, Unwritten) {
    let (mut back, mut stuck, mut unwritten) = (Vec::new(), Vec::new(), Vec::new());
    for f in written.iter().rev() {
        match snapshot_file(&f.path) {
            Ok(now) if now == f.wrote => match restore_file(&f.path, f.was.as_deref()) {
                Ok(()) => back.push(f.path.display().to_string()),
                Err(e) => {
                    stuck.push(format!("{}: {e}", f.path.display()));
                    unwritten.push((f.path.clone(), f.was.clone()));
                }
            },
            Ok(_) => stuck.push(format!(
                "{}: changed outside this window since it was written, so it is left as it is",
                f.path.display()
            )),
            Err(e) => stuck.push(format!("{}: {e}", f.path.display())),
        }
    }
    (back, stuck, unwritten)
}

/// What [`Open::back_to`] goes back to, or [`Open::keep`] builds on.
pub(crate) struct Mark {
    kept: Kept,
    /// The symbols loaded at the mark, so one the change replaced (a pinned version loaded
    /// over another, say) is put back too.
    symbols: BTreeMap<String, Symbol>,
    parts: std::collections::BTreeSet<String>,
    /// The symbol list as it was: a symbol the change loaded may have added or changed a row.
    library: Vec<LibraryEntry>,
    /// The vendored symbol and part files there were, or why that is not known.
    files: Result<std::collections::BTreeSet<(ItemKind, String)>, String>,
    /// Where the change's own project file edits start in [`Open::mark_edits`].
    edits_from: usize,
}

/// The vendored symbol and part files by kind and id (plain files only, not links), from folders
/// inside the project folder. Only names are read. An error (with its reason) when a folder that
/// may be there cannot be checked or listed, or lies outside the project: then nothing is known
/// to be new.
fn vendor_names(dir: &Path) -> Result<std::collections::BTreeSet<(ItemKind, String)>, String> {
    let root = dir.canonicalize().map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut out = std::collections::BTreeSet::new();
    for kind in [ItemKind::Symbol, ItemKind::Part] {
        let folder = dir.join(store::VENDOR_DIR).join(kind.dir());
        let at = |e: std::io::Error| format!("{}: {e}", folder.display());
        // Only a folder known to be missing is skipped; one that cannot be checked is unknown.
        if !folder.try_exists().map_err(at)? {
            continue;
        }
        let real = folder.canonicalize().map_err(at)?;
        if !real.starts_with(&root) {
            return Err(format!("{} lies outside the project", folder.display()));
        }
        for e in std::fs::read_dir(real).map_err(at)? {
            let e = e.map_err(at)?;
            let path = e.path();
            if e.file_type().map_err(at)?.is_file()
                && path.extension().is_some_and(|x| x == "json")
                && let Some(id) = path.file_stem().and_then(|s| s.to_str())
            {
                out.insert((kind, id.to_owned()));
            }
        }
    }
    Ok(out)
}

/// One symbol in the symbol list: what the list shows, searches and offers to place.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LibraryEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Searched like the name, as `wiringcraft library search` does.
    pub category: String,
    pub letter_code: String,
}

impl LibraryEntry {
    /// The entry for symbol `s` listed under `id`.
    pub fn of(id: &str, s: &Symbol) -> Self {
        Self {
            id: id.to_owned(),
            name: s.name.clone(),
            description: s.description.clone(),
            category: s.category.clone(),
            letter_code: s.letter_code.clone(),
        }
    }

    /// Whether every word of `query` appears, ignoring case, in the id, name, category,
    /// letter code or description. An empty query matches everything.
    pub fn matches(&self, query: &str) -> bool {
        let hay = format!(
            "{} {} {} {} {}",
            self.id, self.name, self.category, self.letter_code, self.description
        )
        .to_lowercase();
        query.to_lowercase().split_whitespace().all(|w| hay.contains(w))
    }
}

/// What [`Open::take_parts`] took into the project: parts loaded and part copies written.
#[derive(Default)]
struct TakenParts {
    loaded: Vec<String>,
    copies: Vec<PathBuf>,
}

pub struct Open {
    pub dir: PathBuf,
    pub project: Project,
    /// The project's vendored symbols.
    pub symbols: BTreeMap<String, Symbol>,
    /// Every symbol in the project and its library sources (for the picker), by id.
    pub library: Vec<LibraryEntry>,
    /// The check rules of the project's library sources, read with the library
    /// ([`Open::refresh_library`]) so checks while drawing need not read files.
    pub library_rules: Vec<wc_core::check_rule::RuleSet>,
    /// The project's vendored parts that devices use.
    pub parts: BTreeMap<String, Part>,
    /// Every part in the project and its library sources, as (id, label), for the part picker.
    pub part_library: Vec<(String, String)>,
    pub dirty: bool,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Snapshots dropped from the front of `undo` because it was full, so marks stay valid.
    undo_dropped: usize,
    /// Counts every change to the undo history (checkpoint, undo, redo, squash).
    history: u64,
    /// Changes with every edit of the project, its symbols or its parts that goes through
    /// this type (an undo step, a form's typing, a library reload, [`Open::touch`]), and
    /// differs between any two opened projects: checks while drawing look at this instead of
    /// comparing the whole project each frame.
    revision: u64,
    /// Changes with every library reload ([`Open::refresh_library`]), which may find the
    /// files of the project's library copy changed while the loaded project stays the same.
    library_load: u64,
    /// The `history` count right after a properties form made its own undo step, so later
    /// typing in the form joins that step only while nothing else has touched the history.
    form_step: Option<u64>,
    pub branch: String,
    /// Underlay pictures loaded as textures, by file; `None` when the file cannot be shown.
    pub underlays: BTreeMap<String, Option<egui::TextureHandle>>,
    /// Why a picture in `underlays` has no texture, by file: missing, unreadable, outside the
    /// project, or not a picture.
    pub picture_problems: BTreeMap<String, String>,
    /// The terminal numbering before the change in progress (taken at its checkpoint), so
    /// jumpers can follow their terminals once it is done.
    terminals_before: Option<wc_core::project::TerminalNumbers>,
    /// How far the running select-tool move drag has moved its items in `project`: kept with
    /// every undo and redo step taken during the drag, so Escape can take back from each just
    /// the part of the move it holds (#863). Zero outside a drag.
    drag_offset: Point,
    /// In a shared session, the pages someone else has open (page id to their name). Tools that
    /// work on every page leave these alone; the session would refuse the change otherwise.
    pub held_by_others: BTreeMap<String, String>,
    /// Every version of a vendored symbol this window has loaded, by content hash, so Undo
    /// and Redo can put back the version the restored lock names.
    symbol_versions: BTreeMap<String, Symbol>,
    /// Every version of a vendored part this window has loaded, by content hash, so Undo and
    /// Redo can put back the version the restored lock names (#498).
    part_versions: BTreeMap<String, Part>,
    /// Symbols whose vendored file may hold another version than the one loaded, after a
    /// failed Undo or Redo (or a command's result) could not be put back. The next Undo, Redo
    /// or save writes them again.
    stale_symbol_files: std::collections::BTreeSet<String>,
    /// Vendored files a failed library update could not put back, by path: they may still
    /// hold the library's new version while the lock pins the old one. The next save writes
    /// each again first, and does not go ahead while one cannot be.
    unrestored_files: BTreeMap<PathBuf, UnrestoredFile>,
    /// Project files (a macro) that a failed Undo or Redo, or a change taken back, could not
    /// write as the project now needs them, by path relative to the project folder (absolute
    /// when outside it), with what each should hold (`None`: no file). The next save writes
    /// each again first, and does not go ahead while one cannot be. Saving that macro on
    /// purpose lets its entry go.
    unwritten_files: Waiting,
    /// For each of [`Open::unwritten_files`], what the file held when WiringCraft last wrote
    /// (or failed to write) it, so the next save can tell a change made outside since then
    /// and does not write over it unasked (#662). Once the user chose to write over it, what
    /// it held then: a later change is asked about again.
    unwritten_found: BTreeMap<PathBuf, Found>,
    /// For each of [`Open::unwritten_files`] that is a symbol file [`Open::back_to`] queued to
    /// be repaired: the symbol id and the version (content hash) it is to hold, so a change
    /// taken back around it can retarget or let it go when the pin it puts back differs.
    symbol_repairs: BTreeMap<PathBuf, (String, String)>,
    /// Project files rewritten while a [`Mark`] is open (`marks` counts them), oldest first,
    /// so [`Open::back_to`] puts them back even when their undo steps fell off the history.
    mark_edits: Vec<FileEdit>,
    marks: usize,
    /// `translations.toml` as it was when a command of the assistant started
    /// ([`Open::before_translations_command`]); the inner `None` is no file.
    command_translations: Option<Result<Option<String>, String>>,
    /// The undo mark of the assistant turn running ([`Open::start_turn`]): while its first
    /// step is the oldest, a full history folds the steps after it into it instead of
    /// dropping it, so [`Open::squash_undo`] still makes the turn one step (#641).
    turn: Option<usize>,
    /// Tests: whether each coming symbol file write fails, in order.
    #[cfg(test)]
    pub(crate) fail_symbol_writes: std::collections::VecDeque<bool>,
    /// Tests: whether each coming part file write of Undo or Redo fails, in order.
    #[cfg(test)]
    pub(crate) fail_part_writes: std::collections::VecDeque<bool>,
    /// Tests: a file that turns into a folder once the library update has read its backups,
    /// so the update's write of it fails after other items were already written, and so does
    /// putting it back.
    #[cfg(test)]
    block_after_snapshot: Option<PathBuf>,
    /// Tests: a part copy that a refused [`Open::ensure_parts`] takes back turns into a folder
    /// with a file in it first, so removing it fails.
    #[cfg(test)]
    block_copy_removal: bool,
    /// Tests: a file whose backup read fails, though the file is a plain readable one.
    #[cfg(test)]
    unreadable: Option<PathBuf>,
    /// Tests: files (in a library or the project) rewritten once the update's backups are
    /// taken, as if someone saved them in between.
    #[cfg(test)]
    rewrite_after_snapshot: Vec<(PathBuf, String)>,
    /// Devices renamed here, old tag to new, one map per change in order (one change may
    /// rename several devices at once); a shared session takes them
    /// ([`Open::take_device_renames`]) to keep a rename that waits together.
    device_renames: Vec<BTreeMap<DeviceTag, DeviceTag>>,
    /// Why the last Undo or Redo was not done, when it failed.
    undo_error: Option<String>,
    /// Parts the last Undo or Redo could not load, with the reason, though the step was done.
    step_warning: Option<String>,
}

/// The most Undo steps kept; older ones are dropped.
const UNDO_STEPS: usize = 200;

/// Hands out [`Open::revision`]s: one counter for every opened project, so a project opened
/// after another never shows the same revision.
fn next_revision() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// The history as it was before a library update, so a failed one leaves no trace in it.
struct HistoryState {
    redo: Vec<Step>,
    /// The two oldest Undo steps and the count dropped before them, when the update's
    /// checkpoint pushes one out or folds the second into the first ([`Open::trim_undo`]).
    front: Option<(Vec<Step>, usize)>,
    dirty: bool,
    history: u64,
}

impl Open {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let project = store::load(dir).map_err(|e| e.to_string())?;
        let (symbols, _missing) = store::vendored_symbols(dir, &project);
        let mut me = Self {
            dir: dir.to_path_buf(),
            project,
            symbols,
            library: Vec::new(),
            library_rules: Vec::new(),
            parts: BTreeMap::new(),
            part_library: Vec::new(),
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            undo_dropped: 0,
            history: 0,
            revision: next_revision(),
            library_load: next_revision(),
            form_step: None,
            branch: wc_vcs::Git::new(dir).current_branch().unwrap_or_default(),
            underlays: BTreeMap::new(),
            picture_problems: BTreeMap::new(),
            terminals_before: None,
            drag_offset: Point::new(0.0, 0.0),
            held_by_others: BTreeMap::new(),
            symbol_versions: BTreeMap::new(),
            part_versions: BTreeMap::new(),
            stale_symbol_files: Default::default(),
            unrestored_files: BTreeMap::new(),
            unwritten_files: BTreeMap::new(),
            unwritten_found: BTreeMap::new(),
            symbol_repairs: BTreeMap::new(),
            mark_edits: Vec::new(),
            command_translations: None,
            marks: 0,
            turn: None,
            #[cfg(test)]
            fail_symbol_writes: Default::default(),
            #[cfg(test)]
            fail_part_writes: Default::default(),
            #[cfg(test)]
            block_after_snapshot: None,
            #[cfg(test)]
            block_copy_removal: false,
            #[cfg(test)]
            unreadable: None,
            #[cfg(test)]
            rewrite_after_snapshot: Vec::new(),
            device_renames: Vec::new(),
            undo_error: None,
            step_warning: None,
        };
        // Locked symbols no page uses are loaded too, so Undo can put back the version a
        // library update replaces.
        me.reload_symbols();
        me.refresh_library();
        Ok(me)
    }

    /// The texture for a page's underlay picture, loaded on first use. Pictures larger than the
    /// graphics card takes are scaled down to fit.
    pub fn underlay_texture(&mut self, ctx: &egui::Context, u: &wc_core::Underlay) -> Option<egui::TextureHandle> {
        self.picture_texture(ctx, &u.file)
    }

    /// The texture for a picture file in the project (an underlay or the logo), loaded on first
    /// use; `None` when it is missing, outside the project or not a picture, with the reason in
    /// [`Open::picture_problems`].
    pub fn picture_texture(&mut self, ctx: &egui::Context, file: &str) -> Option<egui::TextureHandle> {
        if let Some(t) = self.underlays.get(file) {
            return t.clone();
        }
        let max_side = ctx.input(|i| i.max_texture_side).max(1) as u32;
        let img = match wc_pdf::read_picture(&self.dir, file) {
            Ok(Some(bytes)) => image::load_from_memory(&bytes).map_err(|e| format!("not a picture: {e}")),
            Ok(None) => Err("the file is missing".to_owned()),
            Err(why) => Err(why),
        };
        let tex = match img {
            Ok(img) => {
                self.picture_problems.remove(file);
                let img = if img.width() > max_side || img.height() > max_side {
                    img.resize(max_side, max_side, image::imageops::FilterType::Triangle)
                } else {
                    img
                };
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let pixels = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                Some(ctx.load_texture(format!("picture:{file}"), pixels, egui::TextureOptions::LINEAR))
            }
            Err(why) => {
                self.picture_problems.insert(file.to_owned(), why);
                None
            }
        };
        self.underlays.insert(file.to_owned(), tex.clone());
        tex
    }

    /// Copy the picture at `src` into the project's `pictures` folder and make it the project
    /// logo, as one undo step. A different file of the same name already there is kept and the
    /// copy gets a new name, so an undo can still show the old logo. Returns the project path.
    pub fn set_logo_from(&mut self, src: &Path) -> Result<String, String> {
        let file = wc_core::Logo::copy_into(&self.dir, src)?;
        // A picture shown before under this name may have been another one.
        self.underlays.remove(&file);
        let cover = self.project.meta.logo.as_ref().and_then(|l| l.cover);
        self.checkpoint();
        self.project.meta.logo = Some(wc_core::Logo {
            file: file.clone(),
            cover,
        });
        Ok(file)
    }

    /// Take the logo off the cover and the title blocks, as one undo step. Its file stays in the
    /// project, so an undo brings it back.
    pub fn remove_logo(&mut self) {
        if self.project.meta.logo.is_some() {
            self.checkpoint();
            self.project.meta.logo = None;
        }
    }

    /// Put the logo's box on the cover page at `min`..`max`. The caller takes the undo
    /// checkpoint, once for a whole drag.
    pub fn set_logo_place(&mut self, min: Point, max: Point) {
        if let Some(logo) = self.project.meta.logo.as_mut() {
            logo.cover = Some((min, max));
            self.edited();
        }
    }

    /// Rebuild the symbol list from the loaded vendored symbols and the library sources, so
    /// each entry carries the name and description of the version in use.
    fn relist_symbols(&mut self) {
        let mut items: BTreeMap<String, LibraryEntry> = self
            .symbols
            .iter()
            .map(|(k, s)| (k.clone(), LibraryEntry::of(k, s)))
            .collect();
        if let Ok(lib) = wc_library::Library::open(&self.dir, &self.project) {
            for id in lib.index(ItemKind::Symbol).into_keys() {
                if !items.contains_key(&id)
                    && let Ok((s, _)) = lib.symbol(&id)
                {
                    let entry = LibraryEntry::of(&id, &s);
                    items.insert(id, entry);
                }
            }
        }
        self.library = items.into_values().collect();
    }

    pub fn refresh_library(&mut self) {
        self.relist_symbols();
        self.library_rules = wc_library::library_rules(&self.dir, &self.project);
        self.library_load = next_revision();
        self.touch();

        self.parts = self
            .project
            .used_parts()
            .into_iter()
            .filter_map(|id| store::read_vendored_part(&self.dir, &id).ok().map(|p| (id, p)))
            .collect();
        // A part file a failed update could not put back may hold another version, or none:
        // the version the lock pins stays loaded until the file is written again.
        for f in self.unrestored_files.values() {
            if f.kind == ItemKind::Part
                && self.project.lock.get(f.kind, &f.id).map(|e| &e.hash) == f.pin.as_ref()
                && let Some(old) = f.was.as_deref().and_then(|b| serde_json::from_slice::<Part>(b).ok())
            {
                self.parts.insert(f.id.clone(), old);
            }
        }
        let mut parts: BTreeMap<String, String> = self.parts.iter().map(|(k, p)| (k.clone(), part_label(p))).collect();
        if let Ok(lib) = wc_library::Library::open(&self.dir, &self.project) {
            for id in lib.index(ItemKind::Part).into_keys() {
                if !parts.contains_key(&id)
                    && let Ok((p, _)) = lib.part(&id)
                {
                    parts.insert(id, part_label(&p));
                }
            }
        }
        self.part_library = parts.into_iter().collect();
    }

    /// Make the maker's outline drawing in the DXF file `dxf` (front view, mm at 1:1) the
    /// footprint of part `id` (#484), as `wiringcraft import footprint-dxf` does: the part is
    /// stored with the footprint in the library source the project takes it from, and the
    /// project's pinned copy follows as one undo step, so the layouts draw the outline at once.
    /// Returns what was done, with the drawing's warnings.
    pub fn attach_footprint(&mut self, id: &str, dxf: &Path) -> Result<String, String> {
        let text = std::fs::read(dxf).map_err(|e| format!("{}: {e}", dxf.display()))?;
        let (footprint, warnings) = wc_import::dxf::footprint(&String::from_utf8_lossy(&text))
            .map_err(|e| format!("{}: {e}", dxf.display()))?;
        let (w, h) = (footprint.width, footprint.height);
        let msg = self.store_in_library_part(
            id,
            |part| part.footprint = Some(footprint),
            |a, b| a.footprint == b.footprint,
            |path| {
                format!(
                    "Footprint of {id}: {} x {} mm, stored in {}",
                    wc_core::geom::format_length(w),
                    wc_core::geom::format_length(h),
                    path.display()
                )
            },
        )?;
        Ok(with_warnings(msg, &warnings))
    }

    /// Store the size of the maker's 3D model in the STEP file `step` with part `id` (#501), as
    /// `wiringcraft import model-step` does: the part is written with the model size in the
    /// library source the project takes it from, and the project's pinned copy follows as one
    /// undo step, so a part without a width and height or footprint gets its size on the plate.
    /// Returns what was done, with the model's warnings.
    pub fn attach_model(&mut self, id: &str, step: &Path) -> Result<String, String> {
        let text = std::fs::read(step).map_err(|e| format!("{}: {e}", step.display()))?;
        let (size, warnings) = wc_import::step::model_size(&String::from_utf8_lossy(&text))
            .map_err(|e| format!("{}: {e}", step.display()))?;
        let msg = self.store_in_library_part(
            id,
            |part| part.model = Some(size),
            |a, b| a.model == b.model,
            |path| {
                format!(
                    "3D model of {id}: {} x {} x {} mm, stored in {}",
                    wc_core::geom::format_length(size.width),
                    wc_core::geom::format_length(size.height),
                    wc_core::geom::format_length(size.depth),
                    path.display()
                )
            },
        )?;
        Ok(with_warnings(msg, &warnings))
    }

    /// Change part `id` with `change` in the library source the project takes it from and write
    /// it back; then the project's pinned copy follows through the same update as Library →
    /// Update (one undo step, a copy changed in the project is not overwritten). `same` tells
    /// whether the project's copy holds the change; `done` says what was stored where.
    fn store_in_library_part(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut Part),
        same: impl Fn(&Part, &Part) -> bool,
        done: impl FnOnce(&Path) -> String,
    ) -> Result<String, String> {
        let lib = wc_library::Library::open(&self.dir, &self.project).map_err(|e| e.to_string())?;
        let path = lib
            .locate(ItemKind::Part, id)
            .ok_or_else(|| format!("part {id} is in none of the project's libraries"))?
            .path;
        let mut part: Part = store::read_json(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if part.id != id {
            return Err(format!("{} holds part {:?}, not {id:?}", path.display(), part.id));
        }
        change(&mut part);
        let json = store::to_json(&part).map_err(|e| e.to_string())?;
        store::write_if_changed(&path, &json).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut msg = done(&path);
        if self.project.lock.parts.contains_key(id) {
            let before = self.parts.get(id).and_then(wc_core::panel::part_size);
            let lib = wc_library::Library::open(&self.dir, &self.project).map_err(|e| e.to_string())?;
            let only = [id.to_owned()];
            // Whether the update took a new copy (and so an undo step): the pin changes. The
            // undo history's length cannot tell once it is full.
            let pin = self.project.lock.parts.get(id).map(|e| e.hash.clone());
            let updated = self.update_library_of(&lib, Some(&only), Some(ItemKind::Part));
            let follows = self.parts.get(id).is_some_and(|p| same(p, &part));
            match updated {
                // Devices of the part on the plates take its new size, in the same undo step.
                Ok(_) if follows && self.project.lock.parts.get(id).map(|e| &e.hash) != pin.as_ref() => {
                    let after = self.parts.get(id).and_then(wc_core::panel::part_size);
                    msg.push_str(&self.resize_panel_devices(id, before, after));
                }
                Ok(_) if follows => {}
                Ok(_) => msg.push_str(&format!(
                    ". The project's copy of {id} was not updated (it differs from the library's); update it from the library"
                )),
                Err(e) => msg.push_str(&format!(". The project's copy of {id} was not updated: {e}")),
            }
        }
        Ok(msg)
    }

    /// Devices of part `id` on the panel layouts, drawn at the size the part had (`before`, or
    /// [`wc_core::panel::DEFAULT_SIZE`] when it had none), take its new size `after` (#501). A
    /// device sized by hand keeps its size; one that would no longer fit on its plate keeps its
    /// size and is named in the returned note. No undo step of its own: this follows a part update.
    fn resize_panel_devices(&mut self, id: &str, before: Option<(f64, f64)>, after: Option<(f64, f64)>) -> String {
        use wc_core::panel::PanelItem;
        let Some(after) = after else { return String::new() };
        let was = before.unwrap_or(wc_core::panel::DEFAULT_SIZE);
        if was == after {
            return String::new();
        }
        let of_part = |tag: &DeviceTag| self.project.devices.get(tag).and_then(|d| d.part.as_deref()) == Some(id);
        let mut todo = Vec::new();
        for (pi, panel) in self.project.panels.iter().enumerate() {
            for item in &panel.items {
                if let PanelItem::Device {
                    tag,
                    at,
                    width,
                    height,
                    rotation,
                } = item
                    && (*width, *height) == was
                    && of_part(tag)
                {
                    todo.push((pi, tag.clone(), *at, *rotation));
                }
            }
        }
        let (mut resized, mut kept) = (0usize, Vec::new());
        for (pi, tag, at, rotation) in todo {
            let Some(panel) = self.project.panels.get_mut(pi) else {
                continue;
            };
            match panel.place_device(tag.clone(), at, after, rotation) {
                Ok(()) => resized += 1,
                Err(why) => kept.push(format!("{tag} on {} ({why})", panel.name)),
            }
        }
        let mut note = String::new();
        if resized > 0 {
            note.push_str(&format!(
                ". {resized} device{} on the panel layouts now {} x {} mm",
                if resized == 1 { "" } else { "s" },
                wc_core::geom::format_length(after.0),
                wc_core::geom::format_length(after.1)
            ));
        }
        if !kept.is_empty() {
            note.push_str(&format!(
                ". Kept their size, the new one would not fit on the plate: {}",
                kept.join("; ")
            ));
        }
        note
    }

    /// Give a device a part (taken from the library into the project when needed), or none.
    pub fn set_device_part(&mut self, tag: &DeviceTag, part: Option<&str>) -> Result<(), String> {
        let current = self.project.devices.get(tag).and_then(|d| d.part.as_deref());
        if current == part {
            return Ok(());
        }
        let lock = self.project.lock.clone();
        if let Some(id) = part
            && let Err(e) = self.ensure_part(id)
        {
            self.project.lock = lock;
            return Err(e);
        }
        self.checkpoint_with_lock(lock);
        self.project.devices.entry(tag.clone()).or_default().part = part.map(str::to_owned);
        Ok(())
    }

    /// Write the articles of a list that came back from the WAGO configurator onto the terminal
    /// and strip devices and the jumpers, as one undo step (see [`wc_import::wago`]). Articles are looked up
    /// among the project's own parts, then the libraries'; a library part that is used is taken
    /// into the project. Returns the report: what changed and every row not matched, with why.
    pub fn import_wago_configured(&mut self, bytes: &[u8]) -> Result<String, String> {
        let (rows, mut warnings) = wc_import::wago::read_configured(bytes).map_err(|e| e.to_string())?;
        let (candidates, more) = wc_import::wago::Candidates::load(&self.dir, &self.project);
        warnings.extend(more);
        // Jumpers follow a terminal change still in progress first, so the rows are matched to
        // the jumpers as they will be.
        self.follow_terminals();
        let plan = wc_import::wago::plan(
            &wc_report::Inputs {
                project: &self.project,
                symbols: &self.symbols,
                parts: &candidates.all(),
            },
            &candidates,
            &rows,
        );
        let mut unmatched = plan.unmatched.clone();
        let mut taken = plan.clone();
        // The lock as it was: the undo step goes back to it, and a part that cannot be used
        // loses the pin it may have got on the way.
        let lock = self.project.lock.clone();
        for id in plan.parts() {
            let r = candidates
                .take(&self.dir, &mut self.project, id)
                .and_then(|()| self.ensure_part(id));
            if let Err(why) = r {
                match lock.parts.get(id) {
                    Some(e) => {
                        self.project.lock.parts.insert(id.to_owned(), e.clone());
                    }
                    None => {
                        self.project.lock.parts.remove(id);
                    }
                }
                for a in plan.assignments.iter().filter(|a| a.part == id) {
                    unmatched.push(format!(
                        "line {}: part {id} cannot be taken into the project: {why}",
                        a.line
                    ));
                }
                taken.assignments.retain(|a| a.part != id);
            }
        }
        // Applied to a copy first: an undo step only for a change that is made, and pins only
        // for the parts the result uses.
        let mut staged = self.project.clone();
        let changed = wc_import::wago::apply(&mut staged, &taken);
        let used = staged.used_parts();
        for id in plan.parts() {
            if !used.contains(id) && !lock.parts.contains_key(id) {
                self.project.lock.parts.remove(id);
            }
        }
        if changed > 0 {
            self.checkpoint_with_lock(lock);
            wc_import::wago::apply(&mut self.project, &taken);
        } else {
            // Nothing applies: no pin either, so the lock stays as saved and undoable.
            self.project.lock = lock;
        }
        let mut msg = format!(
            "WAGO configurator: {changed} device(s) or jumper(s) changed, {} row(s) already up to date, {} not matched.",
            plan.unchanged,
            unmatched.len()
        );
        if !unmatched.is_empty() {
            msg.push_str("\n\nNot matched:");
            for u in &unmatched {
                msg.push_str("\n- ");
                msg.push_str(u);
            }
        }
        for w in warnings {
            msg.push_str("\n\nWarning: ");
            msg.push_str(&w);
        }
        Ok(msg)
    }

    /// Make sure a part is in the project (taken from the library when needed) and loaded.
    fn ensure_part(&mut self, id: &str) -> Result<(), String> {
        let pin = self.project.lock.parts.get(id).cloned();
        if pin.is_some() && self.parts.contains_key(id) {
            return Ok(());
        }
        let vendored = store::vendor_path(&self.dir, ItemKind::Part, id).map_err(|e| e.to_string())?;
        let lib = || wc_library::Library::open(&self.dir, &self.project).map_err(|e| e.to_string());
        let exists = vendored
            .try_exists()
            .map_err(|e| format!("part {id}: {} cannot be checked: {e}", vendored.display()))?;
        match (&pin, exists) {
            // No copy of a pinned part: put back from the library only in the version it pins,
            // the very part checked, and the pin kept as it is.
            (Some(pin), false) => {
                // The source the pin names first, then every other: any may still have the
                // pinned version when the named source has moved on.
                let fits = |p: &Part| version_fits(&p.id, &p.hash(), id, &pin.hash);
                let p = lib()?
                    .find_version(ItemKind::Part, id, &pin.source, fits)
                    .map_err(|why| {
                        format!(
                            "part {id}: its copy in the project is missing and no library has the version the project pins ({why}); put the copy back or update the part from the library"
                        )
                    })?;
                store::write_vendored_part(&self.dir, &p).map_err(|e| e.to_string())?;
            }
            // No copy and no pin: a new part, taken from the library and pinned.
            (None, false) => {
                let lib = lib()?;
                wc_library::take(&self.dir, &mut self.project, &lib, ItemKind::Part, id).map_err(|e| e.to_string())?;
            }
            // A pinned copy: used only when it is the version the pin names.
            (Some(pin), true) => {
                let copy = store::read_vendored_part(&self.dir, id).map_err(|e| e.to_string())?;
                if copy.hash() != pin.hash || copy.id != id {
                    return Err(format!(
                        "part {id}: its copy in the project is not the version the project pins; put the pinned copy back or update the part from the library"
                    ));
                }
            }
            // A copy without its pin (an undone change took the pin back): pinned again as it
            // is, never replaced by a newer library version, to the library that has the part.
            (None, true) => {
                let copy = store::read_vendored_part(&self.dir, id).map_err(|e| e.to_string())?;
                let (found, source) = lib()?.part(id).map_err(|e| match e {
                    wc_library::Error::NotFound { .. } => {
                        format!("part {id} has a copy in the project but no pin, and no library has it to pin it to")
                    }
                    e => format!("part {id} has a copy in the project but no pin, and its library file cannot be read to pin it: {e}"),
                })?;
                if found.id != id {
                    return Err(format!(
                        "part {id} has a copy in the project but no pin, and the library file for it ({source}) declares id {:?}",
                        found.id
                    ));
                }
                self.project.lock.parts.insert(
                    id.to_owned(),
                    wc_core::LockEntry {
                        source,
                        hash: copy.hash(),
                    },
                );
            }
        }
        let p = store::read_vendored_part(&self.dir, id).map_err(|e| e.to_string())?;
        self.parts.insert(id.to_owned(), p);
        Ok(())
    }

    /// Check that every part can be used (taken from the library when needed) without pinning
    /// any of them, then run `then`, the rest of the change: when a part or `then` is refused,
    /// the copies and loaded parts the check got are taken back (those there before are kept),
    /// so a refused change leaves no part copy behind. `then` runs with the lock as it was.
    pub fn check_parts_then<'a, R>(
        &mut self,
        ids: impl IntoIterator<Item = &'a str>,
        then: impl FnOnce(&mut Self) -> Result<R, String>,
    ) -> Result<R, String> {
        let lock = self.project.lock.clone();
        let (taken, r) = self.take_parts(ids);
        self.project.lock = lock.clone();
        match r.and_then(|()| then(self)) {
            Ok(v) => Ok(v),
            r => self.give_back(taken, lock, r),
        }
    }

    /// [`Open::ensure_part`] for every part, all or none: when one cannot be used, the copies
    /// and loaded parts the earlier ones got here are taken back (those there before are kept),
    /// and so is the lock.
    fn ensure_parts<'a>(&mut self, ids: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
        let lock = self.project.lock.clone();
        let (taken, r) = self.take_parts(ids);
        if r.is_err() {
            return self.give_back(taken, lock, r);
        }
        r
    }

    /// [`Open::ensure_part`] for each part until one cannot be used, with what this call added:
    /// the parts loaded here and the copies written that were not there.
    fn take_parts<'a>(&mut self, ids: impl IntoIterator<Item = &'a str>) -> (TakenParts, Result<(), String>) {
        let mut taken = TakenParts::default();
        let mut r = Ok(());
        for id in ids {
            let was_loaded = self.parts.contains_key(id);
            let missing = store::vendor_path(&self.dir, ItemKind::Part, id)
                .ok()
                .filter(|path| !path.try_exists().unwrap_or(true));
            r = self.ensure_part(id);
            if !was_loaded && self.parts.contains_key(id) {
                taken.loaded.push(id.to_owned());
            }
            taken.copies.extend(missing.filter(|path| path.exists()));
            if r.is_err() {
                break;
            }
        }
        (taken, r)
    }

    /// Take back what [`Open::take_parts`] added and put `lock` back, for a change refused with
    /// `r`; a copy that cannot be removed is named in the error.
    fn give_back<R>(&mut self, taken: TakenParts, lock: wc_core::Lockfile, r: Result<R, String>) -> Result<R, String> {
        for id in taken.loaded {
            self.parts.remove(&id);
        }
        let mut left = Vec::new();
        for path in taken.copies {
            #[cfg(test)]
            if self.block_copy_removal {
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::create_dir(&path).and_then(|()| std::fs::write(path.join("x"), ""));
            }
            if let Err(e) = std::fs::remove_file(&path) {
                left.push(format!("{} ({e})", path.display()));
            }
        }
        self.project.lock = lock;
        if left.is_empty() {
            r
        } else {
            r.map_err(|e| {
                format!(
                    "{e}; these part copies taken into the project could not be removed: {}",
                    left.join("; ")
                )
            })
        }
    }

    /// Replace a device's accessories (its parts after the main part). New parts are taken from
    /// the library into the project; a quantity of 0 is taken as 1. One undo step.
    pub fn set_device_accessories(&mut self, tag: &DeviceTag, accessories: Vec<DevicePart>) -> Result<(), String> {
        let accessories: Vec<DevicePart> = accessories
            .into_iter()
            .map(|a| DevicePart { qty: a.qty.max(1), ..a })
            .collect();
        if self.project.devices.get(tag).map_or(&[][..], |d| &d.accessories) == accessories.as_slice() {
            return Ok(());
        }
        let lock = self.project.lock.clone();
        // Parts taken before one that cannot be used are taken back, copy and pin.
        self.ensure_parts(accessories.iter().map(|a| a.part.as_str()))?;
        self.checkpoint_with_lock(lock);
        self.project.devices.entry(tag.clone()).or_default().accessories = accessories;
        Ok(())
    }

    /// Set (or with an empty value, remove) one of a device's own properties.
    pub fn set_device_property(&mut self, tag: &DeviceTag, key: &str, value: &str) {
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty() {
            return;
        }
        let current = self.project.devices.get(tag).and_then(|d| d.properties.get(key));
        if current.map(String::as_str) == Some(value) || (current.is_none() && value.is_empty()) {
            return;
        }
        self.checkpoint();
        let d = self.project.devices.entry(tag.clone()).or_default();
        if value.is_empty() {
            d.properties.remove(key);
        } else {
            d.properties.insert(key.to_owned(), value.to_owned());
        }
    }

    /// Run `f` and then take back everything it changed in the project and its undo history:
    /// for showing a form on a page someone else holds, where a field that still commits must
    /// leave no trace (no change, no undo step, redo kept, not marked as changed).
    pub(crate) fn without_changes<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let kept = self.set_aside();
        let r = f(self);
        self.put_back(kept);
        r
    }

    /// The project as it is and its undo history: the undo and redo steps are moved out (the
    /// history is empty after), the rest copied.
    fn set_aside(&mut self) -> Kept {
        Kept {
            project: self.project.clone(),
            undo: std::mem::take(&mut self.undo),
            redo: std::mem::take(&mut self.redo),
            undo_dropped: self.undo_dropped,
            history: self.history,
            form_step: self.form_step,
            dirty: self.dirty,
            terminals_before: self.terminals_before.clone(),
        }
    }

    /// Back to the project and undo history set aside in `kept`.
    fn put_back(&mut self, kept: Kept) {
        let Kept {
            project,
            undo,
            redo,
            undo_dropped,
            history,
            form_step,
            dirty,
            terminals_before,
        } = kept;
        self.project = project;
        self.undo = undo;
        self.redo = redo;
        self.undo_dropped = undo_dropped;
        self.history = history;
        self.form_step = form_step;
        self.dirty = dirty;
        self.touch();
        self.terminals_before = terminals_before;
    }

    /// Take a snapshot before a change, so it can be undone.
    /// [`Open::checkpoint`] for a change that pinned library items before it: the undo step
    /// keeps the lock as it was (`lock`), so Undo takes those pins back and Redo brings them.
    fn checkpoint_with_lock(&mut self, lock: wc_core::Lockfile) {
        let now = std::mem::replace(&mut self.project.lock, lock);
        self.checkpoint();
        self.project.lock = now;
    }

    /// Which edit of the project is shown: see the `revision` field.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Which library reload is shown: see the `library_load` field.
    pub fn library_load(&self) -> u64 {
        self.library_load
    }

    /// Note an edit of the project, its symbols or its parts made without an undo step (a
    /// shared session's sync, say), so checks while drawing see it.
    pub fn touch(&mut self) {
        self.revision = next_revision();
    }

    /// Note unsaved work: an edit, as [`Open::touch`].
    pub fn edited(&mut self) {
        self.dirty = true;
        self.touch();
    }

    pub fn checkpoint(&mut self) {
        // The change before this one is done: its jumpers follow their terminals first, so the
        // snapshot and this change start from that.
        self.follow_terminals();
        self.terminals_before = Some(self.terminal_numbers());
        self.undo
            .push(Step::new(self.project.clone(), Vec::new(), self.drag_offset));
        self.trim_undo();
        self.redo.clear();
        self.edited();
        self.history += 1;
    }

    /// Which terminal each placed terminal symbol is, as the terminal plan numbers them.
    pub fn terminal_numbers(&self) -> wc_core::project::TerminalNumbers {
        wc_core::project::terminal_numbers(&self.project, |id| self.symbols.get(id).map(|s| s.role))
    }

    /// Keep jumpers on the terminals they joined, after the change since the last checkpoint
    /// (a terminal renamed, moved along its strip or deleted). Returns whether a jumper changed.
    pub fn follow_terminals(&mut self) -> bool {
        let Some(before) = self.terminals_before.take() else {
            return false;
        };
        let symbols = &self.symbols;
        let changed =
            wc_core::project::follow_terminals_in(&mut self.project, &before, |id| symbols.get(id).map(|s| s.role));
        // Keep tracking: the next change before a checkpoint (the next frame of one drag) is
        // measured from here. Without jumpers or a strip layout there is nothing to follow.
        if self.project.meta.follows_terminals() {
            self.terminals_before = Some(self.terminal_numbers());
        }
        changed
    }

    /// Follow the terminals from `before` (numbers taken earlier) to now: for taking a change
    /// back, the jumpers go back with their terminals the way they followed them there.
    pub fn follow_terminals_from(&mut self, before: wc_core::project::TerminalNumbers) -> bool {
        self.terminals_before = Some(before);
        self.follow_terminals()
    }

    /// Forget the change in progress without following it: the project was replaced as a whole
    /// (undo, redo, a command's result), and its jumpers already belong to its drawing.
    pub fn forget_terminal_change(&mut self) {
        self.terminals_before = None;
    }

    /// The project as it was before the latest change, from the undo history: for taking back
    /// only part of that change (in a shared session, the part that waits here).
    pub fn before_last_change(&self) -> Option<&Project> {
        self.undo.last().map(|s| &s.project)
    }

    /// The id of the latest undo step: right after a [`Open::checkpoint`], the step it took.
    pub fn last_step_id(&self) -> Option<u64> {
        self.undo.last().map(|s| s.id)
    }

    /// A select-tool move drag begins: its checkpoint, from which [`Open::drag_move_by`]
    /// counts how far the items moved.
    pub fn begin_drag_move(&mut self) {
        self.drag_offset = Point::new(0.0, 0.0);
        self.checkpoint();
    }

    /// One frame of a move drag: [`Open::move_by`], counted in the drag's offset.
    pub fn drag_move_by(&mut self, page: usize, sel: &[Selection], d: Point) {
        self.move_by(page, sel, d);
        self.drag_offset = self.drag_offset + d;
    }

    /// A move drag ended: later steps are no part of it.
    pub fn end_drag_move(&mut self) {
        self.drag_offset = Point::new(0.0, 0.0);
    }

    /// Whether the undo step `id` is still in the undo history.
    pub fn has_undo_step(&self, id: u64) -> bool {
        self.undo.iter().any(|s| s.id == id)
    }

    /// Take back a move of `items` on page `page` whose checkpoint is the undo step `id`,
    /// keeping the changes recorded after it (a rotate during the move, #863): the items go back
    /// by as far as the drag had moved them, now and in every later undo and redo step (each by
    /// its own part of the move), and the move's own step is dropped, so Undo and Redo then work
    /// as if the changes after it were made at the start position. Jumpers and strip layouts
    /// follow the terminals back in each. Refuses with the reason, changing nothing, when that
    /// step is no longer in the history (the move was undone, or trimmed off the oldest end),
    /// carries file edits a move never makes, has later steps folded into it or after it (an
    /// assistant turn, whose steps in between can no longer be checked), or a drawing, cloud or
    /// wire on the page was added or deleted since (`items` names those by place in their list).
    pub fn take_back_move(&mut self, id: u64, page: &str, items: &[Selection]) -> Result<(), &'static str> {
        let Some(at) = self.undo.iter().position(|s| s.id == id) else {
            return Err("the move is no longer in the undo history");
        };
        let Some(first) = self.undo.get(at) else {
            return Err("the move is no longer in the undo history");
        };
        let counts = |p: &Project| p.page(page).map(|p| (p.graphics.len(), p.clouds.len(), p.wires.len()));
        let start = counts(&first.project);
        let later = self.undo.iter().skip(at + 1).chain(&self.redo).map(|s| &s.project);
        // Dropping the move's step would shift the start of an assistant turn begun after it,
        // and the turn would no longer fold into one step.
        if self.turn.is_some_and(|t| t > self.undo_dropped + at) {
            return Err("an assistant turn started after it is still running");
        }
        if self.undo.iter().skip(at).chain(&self.redo).any(|s| s.folded) {
            return Err("an assistant turn during it was recorded as one step");
        }
        if !first.files.is_empty() {
            return Err("project files were changed with it");
        }
        if later.chain([&self.project]).any(|p| counts(p) != start) {
            return Err("items were added or deleted during it");
        }
        // The change in progress (the move's frames since the last checkpoint) is followed first.
        self.follow_terminals();
        let symbols = &self.symbols;
        let role = |id: &str| symbols.get(id).map(|s| s.role);
        let back = |p: &mut Project, d: Point| {
            let before = p
                .meta
                .follows_terminals()
                .then(|| wc_core::project::terminal_numbers(p, role));
            if let Some(pg) = p.page_mut(page) {
                crate::edit::move_items(pg, items, Point::new(-d.x, -d.y));
            }
            if let Some(before) = before {
                wc_core::project::follow_terminals_in(p, &before, role);
            }
        };
        for s in self.undo.iter_mut().skip(at + 1).chain(self.redo.iter_mut()) {
            back(&mut s.project, s.offset);
            s.offset = Point::new(0.0, 0.0);
        }
        back(&mut self.project, self.drag_offset);
        self.drag_offset = Point::new(0.0, 0.0);
        if self.terminals_before.is_some() {
            self.terminals_before = Some(self.terminal_numbers());
        }
        self.undo.remove(at);
        self.history += 1;
        self.edited();
        Ok(())
    }

    /// Forget the latest undo step, once its change was taken back another way.
    pub fn forget_last_change(&mut self) {
        if self.undo.pop().is_some() {
            self.redo.clear();
            self.history += 1;
            self.touch();
        }
    }

    /// Give page `id` the number `to` in every undo and redo step where it has `from`: the
    /// session renumbered it from `from`, and an Undo here must not bring that number back.
    /// A step with another number for it (one this copy set) keeps that. A step that now
    /// changes nothing next to the one after it (this copy had set the number the session set
    /// too) is dropped, so an Undo never takes back nothing; once the project has the
    /// session's number too, [`Open::forget_steps_like_now`] drops the ones next to it.
    pub fn renumber_in_history(&mut self, id: &str, from: &str, to: &str) {
        for step in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            if let Some(page) = step.project.page_mut(id)
                && page.number == from
            {
                page.number = to.to_owned();
                step.project.sort_pages();
            }
        }
        // A step that rewrote project files still changes something.
        let same = |a: &mut Step, b: &mut Step| a.project == b.project && a.files.is_empty();
        self.undo.dedup_by(same);
        self.redo.dedup_by(same);
    }

    /// Drop the latest undo and redo steps that `like_now` finds the same as the project now
    /// (an Undo or Redo of them would change nothing), down to the first that is not.
    pub fn forget_steps_like_now(&mut self, like_now: impl Fn(&Project, &Project) -> bool) {
        while self
            .undo
            .last()
            .is_some_and(|s| s.files.is_empty() && like_now(&s.project, &self.project))
        {
            self.undo.pop();
        }
        while self
            .redo
            .last()
            .is_some_and(|s| s.files.is_empty() && like_now(&s.project, &self.project))
        {
            self.redo.pop();
        }
    }

    /// The devices renamed here since the last call: old tag to new, one map per change, in
    /// order.
    pub fn take_device_renames(&mut self) -> Vec<BTreeMap<DeviceTag, DeviceTag>> {
        std::mem::take(&mut self.device_renames)
    }

    /// Note that device `from` was renamed to `to` here.
    pub(crate) fn note_device_rename(&mut self, from: &DeviceTag, to: &DeviceTag) {
        self.device_renames
            .push([(from.clone(), to.clone())].into_iter().collect());
    }

    /// Drop the oldest undo steps beyond [`UNDO_STEPS`]. While the first step of the running
    /// assistant turn is the oldest, the step after it is folded into it instead (its file
    /// edits go with the first's), so the turn stays whole for [`Open::squash_undo`] and no
    /// file edit is lost. Marks taken after the turn began stay valid either way.
    fn trim_undo(&mut self) {
        while self.undo.len() > UNDO_STEPS {
            // A change inside a [`Mark`] has a history of its own; the turn's step is not in it.
            let held = self.marks == 0 && self.turn.is_some_and(|t| t <= self.undo_dropped);
            if held {
                let next = self.undo.remove(1);
                if let Some(first) = self.undo.first_mut() {
                    first.files.extend(next.files);
                    // What the folded step did is no longer checkable on its own.
                    first.folded = true;
                }
            } else {
                self.undo.remove(0);
            }
            self.undo_dropped += 1;
        }
    }

    /// A mark in the undo history, for [`Open::squash_undo`].
    pub fn undo_mark(&self) -> usize {
        self.undo_dropped + self.undo.len()
    }

    /// An assistant turn starts: returns its undo mark, for [`Open::squash_undo`] once it is
    /// done. Until then the full history keeps the turn's first step (see `turn`).
    pub fn start_turn(&mut self) -> usize {
        let mark = self.undo_mark();
        self.turn = Some(mark);
        mark
    }

    /// Make every change since `mark` one undo step, so one Undo takes all of it back
    /// (the assistant's changes in one turn, for example).
    pub fn squash_undo(&mut self, mark: usize) {
        let turn = self.turn.take_if(|t| *t == mark).is_some();
        let start = match mark.checked_sub(self.undo_dropped) {
            Some(start) => start,
            // The turn's first step was kept as the oldest while later ones were folded into it.
            None if turn && !self.undo.is_empty() => 0,
            None => return,
        };
        if self.undo.len() > start + 1 {
            // The files the later steps rewrote are taken back and written with the first.
            let later: Vec<FileEdit> = self.undo.drain(start + 1..).flat_map(|s| s.files).collect();
            if let Some(first) = self.undo.get_mut(start) {
                first.files.extend(later);
                first.folded = true;
            }
            self.history += 1;
            self.touch();
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// How many steps Undo can take back.
    #[cfg(test)]
    pub(crate) fn undo_steps(&self) -> usize {
        self.undo.len()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        // The change being undone is followed first, so redo brings it back whole.
        self.follow_terminals();
        match self.undo.pop() {
            Some(prev) => {
                let written = match write_edits(&self.dir, &prev.files, true, &self.unwritten_files) {
                    Ok(written) => written,
                    Err((e, unwritten)) => {
                        self.keep_unwritten(unwritten, false);
                        self.undo_error = Some(e);
                        self.undo.push(prev);
                        return false;
                    }
                };
                self.forget_terminal_change();
                let Step {
                    project,
                    files,
                    id,
                    offset,
                    folded,
                } = prev;
                let now = std::mem::replace(&mut self.project, project);
                // The step keeps its id across Undo and Redo, so a move redone during its drag
                // is still found by Escape.
                self.redo.push(Step {
                    project: now,
                    files,
                    id,
                    offset: std::mem::replace(&mut self.drag_offset, offset),
                    folded,
                });
                let left = self
                    .redo
                    .last()
                    .map(|s| s.project.lock.parts.clone())
                    .unwrap_or_default();
                if let Err(e) = self.sync_vendored(&left) {
                    // The vendored files could not follow: keep the project as it was.
                    if let Some(cur) = self.redo.pop() {
                        // The files go back as they were before this step, not as recorded:
                        // a change made to one outside since stays.
                        let (_, stuck, unwritten) = put_back_files(&written);
                        self.keep_unwritten(unwritten, false);
                        let e = if stuck.is_empty() {
                            e
                        } else {
                            format!("{e}\nProject files not put back: {}", stuck.join("; "))
                        };
                        let restored = std::mem::replace(&mut self.project, cur.project);
                        self.undo_error = Some(e);
                        let offset = std::mem::replace(&mut self.drag_offset, cur.offset);
                        // The step goes back whole: its id and folded mark too.
                        self.undo.push(Step {
                            project: restored,
                            offset,
                            ..cur
                        });
                    }
                    return false;
                }
                // Only now are the files as the project needs them: a failed sync puts them back.
                self.written_as_needed(&written);
                let sources = self.redo.last().map(|s| &s.project.meta.libraries) != Some(&self.project.meta.libraries);
                self.follow_step(sources);
                self.edited();
                self.history += 1;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        self.follow_terminals();
        match self.redo.pop() {
            Some(next) => {
                let written = match write_edits(&self.dir, &next.files, false, &self.unwritten_files) {
                    Ok(written) => written,
                    Err((e, unwritten)) => {
                        self.keep_unwritten(unwritten, false);
                        self.undo_error = Some(e);
                        self.redo.push(next);
                        return false;
                    }
                };
                self.forget_terminal_change();
                let Step {
                    project,
                    files,
                    id,
                    offset,
                    folded,
                } = next;
                let now = std::mem::replace(&mut self.project, project);
                // The step keeps its id across Undo and Redo, so a move redone during its drag
                // is still found by Escape.
                self.undo.push(Step {
                    project: now,
                    files,
                    id,
                    offset: std::mem::replace(&mut self.drag_offset, offset),
                    folded,
                });
                let left = self
                    .undo
                    .last()
                    .map(|s| s.project.lock.parts.clone())
                    .unwrap_or_default();
                if let Err(e) = self.sync_vendored(&left) {
                    if let Some(cur) = self.undo.pop() {
                        // The files go back as they were before this step, not as recorded:
                        // a change made to one outside since stays.
                        let (_, stuck, unwritten) = put_back_files(&written);
                        self.keep_unwritten(unwritten, false);
                        let e = if stuck.is_empty() {
                            e
                        } else {
                            format!("{e}\nProject files not put back: {}", stuck.join("; "))
                        };
                        let restored = std::mem::replace(&mut self.project, cur.project);
                        self.undo_error = Some(e);
                        let offset = std::mem::replace(&mut self.drag_offset, cur.offset);
                        // The step goes back whole: its id and folded mark too.
                        self.redo.push(Step {
                            project: restored,
                            offset,
                            ..cur
                        });
                    }
                    return false;
                }
                self.written_as_needed(&written);
                let sources = self.undo.last().map(|s| &s.project.meta.libraries) != Some(&self.project.meta.libraries);
                self.follow_step(sources);
                self.edited();
                self.history += 1;
                true
            }
            None => false,
        }
    }

    /// After an undo or redo: rebuild the library lists when the step added or removed a library
    /// source (`sources_changed`), so symbols of a source that is gone are no longer offered and
    /// those of one that came back are, and otherwise rebuild the symbol list, so a symbol whose
    /// version the step changed is listed with its own name and description; then load every part a device uses that is not loaded,
    /// since a redo can bring back an assignment whose part an earlier rebuild dropped.
    fn follow_step(&mut self, sources_changed: bool) {
        if sources_changed {
            self.refresh_library();
        } else {
            // The step may have brought back another version of a vendored symbol: list it
            // with its own name and description.
            self.relist_symbols();
        }
        let missing: Vec<String> = self
            .project
            .used_parts()
            .into_iter()
            .filter(|id| !self.parts.contains_key(id))
            .collect();
        let mut failed = Vec::new();
        for id in missing {
            match store::read_vendored_part(&self.dir, &id) {
                Ok(p) => {
                    self.parts.insert(id, p);
                }
                Err(e) => failed.push(format!("{id} ({e})")),
            }
        }
        self.step_warning =
            (!failed.is_empty()).then(|| format!("these parts could not be loaded: {}", failed.join("; ")));
    }

    /// Load the vendored symbols again after the library changed them, keeping the versions
    /// they replace so Undo can bring those back.
    pub fn reload_symbols(&mut self) {
        for sym in self.symbols.values() {
            self.symbol_versions.insert(sym.hash(), sym.clone());
        }
        let (mut syms, _) = store::vendored_symbols(&self.dir, &self.project);
        // Locked symbols no page uses yet are kept too, so the version the lock names is
        // known for Redo and for the next placement.
        for id in self.project.lock.symbols.keys() {
            if !syms.contains_key(id)
                && let Ok(sym) = store::read_vendored_symbol(&self.dir, id)
            {
                syms.insert(id.clone(), sym);
            }
        }
        // A file that could not be put back still holds another version: the loaded one stays
        // while the lock still names it, so the next save writes it. A lock that now names
        // the file's version (the symbol was taken again) makes the file right.
        for id in self.stale_symbol_files.clone() {
            let wanted = self.project.lock.symbols.get(&id).map(|e| e.hash.clone());
            match self.symbols.get(&id) {
                Some(loaded) if wanted.as_ref().is_none_or(|h| *h == loaded.hash()) => {
                    syms.insert(id.clone(), loaded.clone());
                }
                _ => {
                    self.stale_symbol_files.remove(&id);
                }
            }
        }
        self.symbols = syms;
        self.touch();
    }

    /// After Undo or Redo: the parts whose pin the step changed (`left` is the part lock of the
    /// state left), then the vendored symbols, as the restored lock names them. When the
    /// symbols cannot follow, the part files written are put back first, so a failed step
    /// leaves nothing half done.
    fn sync_vendored(&mut self, left: &BTreeMap<String, wc_core::LockEntry>) -> Result<(), String> {
        let writes = self.sync_parts(left)?;
        if let Err(e) = self.sync_symbols() {
            let unrestored = self.undo_part_writes(writes, left);
            return Err(with_unrestored(e, &unrestored));
        }
        // The files written now hold the version the lock names: a backup still waiting from
        // an earlier failure is done with, and a later save must not write it over the copy.
        for w in &writes {
            self.unrestored_files.remove(&w.path);
        }
        Ok(())
    }

    /// After Undo or Redo: put back the project copy of each part whose pin the step changed
    /// (`left` being the part lock of the state left), in the project folder and, when it is
    /// loaded, in memory, so the copy is the version the restored lock names. A part whose pin
    /// the step did not touch is left alone, so a copy changed in the project keeps its
    /// changes. Returns what was written, for [`Open::undo_part_writes`]; when a file cannot
    /// be written, the files already written are put back and nothing changes.
    fn sync_parts(&mut self, left: &BTreeMap<String, wc_core::LockEntry>) -> Result<Vec<PartWrite>, String> {
        let mut wanted: Vec<(Part, PathBuf)> = Vec::new();
        for (id, entry) in &self.project.lock.parts {
            if left.get(id) == Some(entry) {
                continue;
            }
            let path = store::vendor_path(&self.dir, ItemKind::Part, id).map_err(|e| e.to_string())?;
            let file = store::read_vendored_part(&self.dir, id);
            let file_fits = file.as_ref().is_ok_and(|p| p.hash() == entry.hash && p.id == *id);
            let loaded_fits = self.parts.get(id).is_none_or(|p| p.hash() == entry.hash);
            if file_fits && loaded_fits {
                continue;
            }
            let version = match (self.part_versions.get(&entry.hash), file) {
                (Some(p), _) => p.clone(),
                (None, Ok(p)) if file_fits => p,
                (None, Ok(_)) => {
                    return Err(format!(
                        "the version of part {id} this step names is not known any more"
                    ));
                }
                (None, Err(e)) => {
                    return Err(format!(
                        "the version of part {id} this step names is not known any more, and its file cannot be read: {e}"
                    ));
                }
            };
            wanted.push((version, path));
        }
        let mut writes: Vec<PartWrite> = Vec::new();
        for (want, path) in wanted {
            let was = match snapshot_file(&path) {
                Ok(was) => was,
                Err(e) => {
                    let unrestored = self.undo_part_writes(writes, left);
                    return Err(with_unrestored(
                        format!("part {} could not be put back: {e}", want.id),
                        &unrestored,
                    ));
                }
            };
            if let Err(e) = self.write_part_file(&want) {
                let unrestored = self.undo_part_writes(writes, left);
                return Err(with_unrestored(
                    format!("part {} could not be put back: {e}", want.id),
                    &unrestored,
                ));
            }
            self.part_versions.insert(want.hash(), want.clone());
            let loaded = self.parts.get(&want.id).cloned();
            if let Some(old) = &loaded {
                self.part_versions.insert(old.hash(), old.clone());
                self.parts.insert(want.id.clone(), want.clone());
            }
            writes.push(PartWrite {
                id: want.id.clone(),
                path,
                was,
                loaded,
            });
        }
        Ok(writes)
    }

    /// Take back part files [`Open::sync_parts`] wrote, newest first, with the loaded parts they
    /// replaced. A file that cannot be put back is kept as an unrestored file under its pin in
    /// `pins` (the lock the project goes back to): the pinned version stays loaded, and the
    /// next save writes the file first. Returns what could not be put back.
    fn undo_part_writes(&mut self, writes: Vec<PartWrite>, pins: &BTreeMap<String, wc_core::LockEntry>) -> Vec<String> {
        let mut unrestored = Vec::new();
        for w in writes.into_iter().rev() {
            if let Some(old) = w.loaded {
                self.parts.insert(w.id.clone(), old);
            }
            #[cfg(test)]
            let blocked = self.fail_part_writes.pop_front() == Some(true);
            #[cfg(not(test))]
            let blocked = false;
            let put_back = if blocked {
                Err(std::io::Error::other("test write failure"))
            } else {
                restore_file(&w.path, w.was.as_deref())
            };
            if let Err(e) = put_back {
                unrestored.push(format!("{} ({e})", w.id));
                let pin = pins.get(&w.id).map(|e| e.hash.clone());
                self.register_unrestored(
                    w.path,
                    UnrestoredFile {
                        kind: ItemKind::Part,
                        id: w.id,
                        was: w.was,
                        pin,
                    },
                );
            }
        }
        unrestored
    }

    fn write_part_file(&mut self, part: &Part) -> Result<(), String> {
        #[cfg(test)]
        if self.fail_part_writes.pop_front() == Some(true) {
            return Err("test write failure".into());
        }
        store::write_vendored_part(&self.dir, part)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// After Undo or Redo: put back the vendored symbol versions the restored lock names, both
    /// in memory and in the project folder, so the pages draw what the lock says. When a file
    /// cannot be written, the files already written are put back and nothing changes.
    fn sync_symbols(&mut self) -> Result<(), String> {
        let mut wanted: Vec<(String, Symbol)> = Vec::new();
        for (id, entry) in &self.project.lock.symbols {
            if let Some(loaded) = self.symbols.get(id).filter(|s| s.hash() == entry.hash) {
                // Loaded already; its file is written again only when it could not be put back.
                if self.stale_symbol_files.contains(id) {
                    wanted.push((id.clone(), loaded.clone()));
                }
                continue;
            }
            // The version the lock names: kept from earlier, or still in the project folder.
            if let Some(sym) = self.symbol_versions.get(&entry.hash) {
                wanted.push((id.clone(), sym.clone()));
                continue;
            }
            match store::read_vendored_symbol(&self.dir, id) {
                Ok(sym) if sym.hash() == entry.hash => wanted.push((id.clone(), sym)),
                Ok(_) => {
                    return Err(format!(
                        "the version of symbol {id} this step names is not known any more"
                    ));
                }
                Err(e) => {
                    return Err(format!(
                        "the version of symbol {id} this step names is not known any more, and its file cannot be read: {e}"
                    ));
                }
            }
        }
        for (n, (id, want)) in wanted.iter().enumerate() {
            if let Err(e) = self.write_symbol_file(want) {
                // Put back the files already written; say which could not be, and remember
                // them so the next Undo or Redo writes them again.
                let mut unrestored = Vec::new();
                for (done, _) in wanted.iter().take(n) {
                    let Some(old) = self.symbols.get(done).cloned() else {
                        continue;
                    };
                    if let Err(e) = self.write_symbol_file(&old) {
                        unrestored.push(format!("{done} ({e})"));
                        self.stale_symbol_files.insert(done.clone());
                    }
                }
                let mut msg = format!("symbol {id} could not be put back: {e}");
                if !unrestored.is_empty() {
                    msg.push_str(&format!(
                        "; these symbol files now hold the other version and could not be restored: {}. The next Undo or Redo writes them again.",
                        unrestored.join(", ")
                    ));
                }
                return Err(msg);
            }
        }
        for (id, _) in &wanted {
            self.stale_symbol_files.remove(id);
            self.symbol_file_written(id);
        }
        for (id, want) in wanted {
            if let Some(old) = self.symbols.insert(id, want) {
                self.symbol_versions.insert(old.hash(), old);
            }
        }
        // A symbol the restored step neither locks nor uses is let go, kept as a version for
        // Redo: still loaded, it would let the next placement skip pinning it in the lock.
        let used = self.project.used_symbols();
        let gone: Vec<String> = self
            .symbols
            .keys()
            .filter(|id| !self.project.lock.symbols.contains_key(*id) && !used.contains(*id))
            .cloned()
            .collect();
        for id in gone {
            if let Some(old) = self.symbols.remove(&id) {
                self.symbol_versions.insert(old.hash(), old);
            }
        }
        Ok(())
    }

    pub(crate) fn write_symbol_file(&mut self, sym: &Symbol) -> Result<(), String> {
        #[cfg(test)]
        if self.fail_symbol_writes.pop_front() == Some(true) {
            return Err("test write failure".into());
        }
        store::write_vendored_symbol(&self.dir, sym)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Remember that a symbol's vendored file may hold another version than the loaded one;
    /// the next save, Undo or Redo writes the loaded version again.
    pub(crate) fn mark_symbol_file_stale(&mut self, id: &str) {
        self.stale_symbol_files.insert(id.to_owned());
    }

    /// Write the loaded version of every symbol whose file may hold another one. A save does
    /// not go ahead while one of them cannot be written, since the saved lock would not match.
    fn rewrite_stale_symbol_files(&mut self) -> Result<(), String> {
        for id in self.stale_symbol_files.clone() {
            let Some(sym) = self.symbols.get(&id).cloned() else {
                self.stale_symbol_files.remove(&id);
                continue;
            };
            self.write_symbol_file(&sym).map_err(|e| {
                format!("not saved: the file of symbol {id} still holds another version and cannot be written: {e}")
            })?;
            self.stale_symbol_files.remove(&id);
        }
        Ok(())
    }

    /// Take in what an import wrote to the project folder, as one Undo step. Call it with no
    /// changes left unsaved (they would be replaced). Returns whether the project changed.
    pub fn adopt_saved(&mut self) -> Result<bool, String> {
        let fresh = Open::load(&self.dir)?;
        self.branch = fresh.branch;
        let changed = fresh.project != self.project;
        // The versions the import replaced are kept, so its undo step can put them back: the
        // loaded parts too, whose files and pins an import can replace the same way.
        self.archive_symbols();
        for p in self.parts.values() {
            self.part_versions.insert(p.hash(), p.clone());
        }
        // And the versions the import pinned: a part no device uses is not loaded below, and
        // Redo needs its version once Undo has put the old file back.
        for (id, entry) in &fresh.project.lock.parts {
            if self.project.lock.parts.get(id) != Some(entry)
                && let Ok(part) = store::read_vendored_part(&self.dir, id)
                && part.hash() == entry.hash
            {
                self.part_versions.insert(entry.hash.clone(), part);
            }
        }
        self.symbols.extend(fresh.symbols);
        // An import can write a picture over a file shown before (a PDF scan at the same page).
        self.underlays.clear();
        self.picture_problems.clear();
        if changed {
            self.forget_terminal_change();
            self.checkpoint();
            self.project = fresh.project;
            self.forget_terminal_change();
            self.dirty = false;
        }
        self.refresh_library();
        Ok(changed)
    }

    /// Keep the loaded symbol versions, so a step that replaces them can be undone.
    pub fn archive_symbols(&mut self) {
        for sym in self.symbols.values() {
            self.symbol_versions.insert(sym.hash(), sym.clone());
        }
    }

    /// Take the newer library versions of the project's items (only `only`, when given), as
    /// one undo step. An update that would leave wires on pins a new symbol version no longer
    /// has is taken back, and the wires are named.
    pub fn update_library(
        &mut self,
        lib: &wc_library::Library,
        only: Option<&[String]>,
    ) -> Result<Vec<(ItemKind, String)>, String> {
        self.update_library_of(lib, only, None)
    }

    /// [`Open::update_library`], limited to items of `kind` when given: a symbol that shares a
    /// part's id is left alone when only the part is updated. An update that finds nothing to
    /// take changes nothing, not even the undo history.
    fn update_library_of(
        &mut self,
        lib: &wc_library::Library,
        only: Option<&[String]>,
        kind: Option<ItemKind>,
    ) -> Result<Vec<(ItemKind, String)>, String> {
        let all: Vec<_> = wc_library::status(&self.dir, &self.project, lib)
            .into_iter()
            .filter(|s| kind.is_none_or(|k| s.kind == k))
            .collect();
        // An item named on purpose that the project does not have, or whose library file is
        // wrong, stops the update.
        if let Some(bad) = wc_library::unknown_named(&all, only).or_else(|| wc_library::wrong_named(&all, only)) {
            return Err(bad.to_string());
        }
        // Exactly these versions are taken, as seen here: an item whose library file changes
        // after this (from a wrong id to a new version, or to yet another version) is not
        // written without a backup; it waits for the next update.
        let todo = wc_library::changed(&all, only);
        if todo.is_empty() {
            return Ok(Vec::new());
        }
        // Checked before anything is written: a new version that drops a wired pin stops the
        // whole update.
        // The pins are checked on the very version the update will take: one that changed in the
        // library since it was looked at is not taken at all.
        let mut dangling: Vec<String> = Vec::new();
        for (kind, id, hash) in &todo {
            let Some((s, _)) = (*kind == ItemKind::Symbol).then(|| lib.symbol(id).ok()).flatten() else {
                continue;
            };
            if s.hash() != *hash {
                return Err(format!(
                    "nothing was updated: the symbol {id} changed in the library while it was being updated; update again"
                ));
            }
            dangling.extend(dangling_pins(&self.project, id, &s));
        }
        if !dangling.is_empty() {
            dangling.sort();
            return Err(format!(
                "nothing was updated: wires end on pins the new versions no longer have ({}). Move those wires first.",
                dangling.join(", ")
            ));
        }
        // Every file the update may write, and the lock, are kept here: a failure puts them
        // back directly, whatever undo can or cannot restore.
        let files: Vec<KeptFile> = todo
            .iter()
            .filter_map(|(kind, id, _)| Some((*kind, id, store::vendor_path(&self.dir, *kind, id).ok()?)))
            .map(|(kind, id, path)| {
                #[cfg(test)]
                if self.unreadable.as_ref() == Some(&path) {
                    return Err(format!("{} cannot be read: test read failure", path.display()));
                }
                let was = snapshot_file(&path)?;
                Ok(KeptFile {
                    kind,
                    id: id.clone(),
                    path,
                    was,
                })
            })
            .collect::<Result<_, String>>()
            .map_err(|e| format!("nothing was updated: {e}"))?;
        let lock = self.project.lock.clone();
        let history = HistoryState {
            redo: self.redo.clone(),
            front: (self.undo.len() >= UNDO_STEPS).then(|| (self.undo[..2].to_vec(), self.undo_dropped)),
            dirty: self.dirty,
            history: self.history,
        };
        #[cfg(test)]
        for (path, text) in std::mem::take(&mut self.rewrite_after_snapshot) {
            std::fs::write(&path, text).map_err(|e| format!("test: {} cannot be rewritten: {e}", path.display()))?;
        }
        self.archive_symbols();
        // The parts too, loaded or not, so Undo can put back the copies the update replaces.
        for p in self.parts.values() {
            self.part_versions.insert(p.hash(), p.clone());
        }
        for (kind, id, _) in &todo {
            if *kind == ItemKind::Part
                && let Ok(p) = store::read_vendored_part(&self.dir, id)
            {
                self.part_versions.insert(p.hash(), p);
            }
        }
        self.checkpoint();
        // Tests: a file turns into a folder once the update read its backups, so writing it,
        // and putting it back, fails.
        #[cfg(test)]
        let block = self.block_after_snapshot.take();
        let after_backup = move || {
            #[cfg(test)]
            if let Some(path) = block {
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::create_dir(&path);
            }
        };
        let done = match wc_library::update_items_then(&self.dir, &mut self.project, lib, &todo, after_backup) {
            Ok(done) => done,
            Err(e) => {
                // `update_items` put back what it wrote, and only the undo step goes. When it
                // could not, the files it wrote (only those: one it never wrote may have been
                // changed since by someone else) are tried again here, so one that still
                // cannot be put back is known.
                let written: &[KeptFile] = match &e {
                    wc_library::Error::NotPutBack { written, .. } => {
                        let wrote = &todo[..(*written).min(todo.len())];
                        let n = files
                            .iter()
                            .take_while(|f| wrote.iter().any(|(k, id, _)| *k == f.kind && *id == f.id))
                            .count();
                        &files[..n]
                    }
                    _ => &[],
                };
                let unrestored = self.roll_back_update(written, lock, history);
                return Err(if unrestored.is_empty() {
                    e.to_string()
                } else {
                    format!(
                        "{e}; what was already updated could not be taken back: {}",
                        unrestored.join("; ")
                    )
                });
            }
        };
        self.reload_symbols();
        // The loaded parts follow their new files too, so a copy compared with the lock later
        // (after a refused session update puts the old pin back) is the version on disk.
        for (kind, id) in &done {
            if *kind == ItemKind::Part {
                match store::read_vendored_part(&self.dir, id) {
                    Ok(p) => {
                        self.part_versions.insert(p.hash(), p.clone());
                        self.parts.insert(id.clone(), p);
                    }
                    Err(_) => {
                        self.parts.remove(id);
                    }
                }
            }
        }
        let mut dangling: Vec<String> = done
            .iter()
            .filter(|(kind, _)| *kind == ItemKind::Symbol)
            .filter_map(|(_, id)| Some(dangling_pins(&self.project, id, self.symbols.get(id)?)))
            .flatten()
            .collect();
        if dangling.is_empty() {
            return Ok(done);
        }
        dangling.sort();
        let wires = dangling.join(", ");
        let unrestored = self.roll_back_update(&files, lock, history);
        if !unrestored.is_empty() {
            return Err(format!(
                "wires end on pins the new versions no longer have ({wires}), and the update could not be taken back: {}",
                unrestored.join("; ")
            ));
        }
        Err(format!(
            "nothing was updated: wires end on pins the new versions no longer have ({wires}). Move those wires first."
        ))
    }

    /// Take back a library update that failed: the files it wrote and the lock as they were
    /// (`files`, `lock`), its undo step dropped, and the history as it was before it (`was`: the
    /// Redo list, the oldest Undo step its checkpoint pushed out, whether there were unsaved
    /// changes). Returns what could not be put back. A file that could not be put back keeps its
    /// old version loaded and is written again before the next save.
    fn roll_back_update(&mut self, files: &[KeptFile], lock: wc_core::Lockfile, was: HistoryState) -> Vec<String> {
        let mut failed: Vec<&KeptFile> = Vec::new();
        let mut unrestored: Vec<String> = Vec::new();
        for f in files {
            if let Err(err) = self.restore_kept(f) {
                unrestored.push(format!(
                    "{} ({err}); the old version stays loaded and is written before the next save",
                    f.path.display()
                ));
                failed.push(f);
            }
        }
        self.reload_symbols();
        if !self.undo()
            && let Some(why) = self.take_undo_error()
        {
            unrestored.push(why);
        }
        // Whatever undo managed, the lock is the one the files above match.
        self.project.lock = lock;
        self.redo = was.redo;
        if let Some((front, dropped)) = was.front {
            let first = 1.min(self.undo.len());
            self.undo.splice(..first, front);
            self.undo_dropped = dropped;
        }
        self.dirty = was.dirty;
        self.history = was.history;
        self.reload_symbols();
        for f in failed {
            self.keep_unrestored(f);
        }
        unrestored
    }

    /// Put a file a library update wrote back as it was.
    fn restore_kept(&self, f: &KeptFile) -> std::io::Result<()> {
        restore_file(&f.path, f.was.as_deref())
    }

    /// After file `f` could not be put back: the version the lock pins stays loaded (the file
    /// may hold the library's new one), and the file waits to be written before the next save.
    fn keep_unrestored(&mut self, f: &KeptFile) {
        let pin = self.project.lock.get(f.kind, &f.id).map(|e| e.hash.clone());
        if f.kind == ItemKind::Symbol
            && let Some(old) = pin.as_ref().and_then(|h| self.symbol_versions.get(h)).cloned()
        {
            self.symbols.insert(f.id.clone(), old);
            self.stale_symbol_files.insert(f.id.clone());
        }
        self.register_unrestored(
            f.path.clone(),
            UnrestoredFile {
                kind: f.kind,
                id: f.id.clone(),
                was: f.was.clone(),
                pin,
            },
        );
        if f.kind == ItemKind::Part
            && let Some(old) = self
                .unrestored_files
                .get(&f.path)
                .and_then(|u| u.was.as_deref())
                .and_then(|b| serde_json::from_slice::<Part>(b).ok())
        {
            self.parts.insert(f.id.clone(), old);
        }
    }

    /// Note a file that could not be put back, to be written before the next save. A file still
    /// waiting from an earlier failure under the same pin keeps that backup: this one is of the
    /// file as the earlier failure left it. The file still has to be written: unsaved work, so
    /// Save is offered and asked for.
    fn register_unrestored(&mut self, path: PathBuf, file: UnrestoredFile) {
        if self
            .unrestored_files
            .get(&path)
            .is_none_or(|known| known.pin != file.pin)
        {
            self.unrestored_files.insert(path, file);
        }
        self.edited();
    }

    /// Write again every file a failed library update could not put back. One whose item the
    /// lock no longer pins in that version was written since by what changed the pin, and is
    /// let go. A save does not go ahead while one cannot be written: the saved lock would name
    /// a version its file does not hold.
    fn put_back_unrestored_files(&mut self) -> Result<(), String> {
        for (path, f) in self.unrestored_files.clone() {
            let pin = self.project.lock.get(f.kind, &f.id).map(|e| e.hash.clone());
            if pin == f.pin {
                // The folder may have gone with whatever blocked the file.
                let folder = match (&f.was, path.parent()) {
                    (Some(_), Some(dir)) => std::fs::create_dir_all(dir),
                    _ => Ok(()),
                };
                folder
                    .and_then(|()| restore_file(&path, f.was.as_deref()))
                    .map_err(|e| {
                        format!(
                            "not saved: the file of {} {} ({}) still holds another version and cannot be put back: {e}",
                            kind_name(f.kind),
                            f.id,
                            path.display()
                        )
                    })?;
            }
            self.unrestored_files.remove(&path);
        }
        Ok(())
    }

    /// Note project files left holding something other than what the project now needs, each
    /// with what it should hold, to be written before the next save. The file still has to be
    /// written: unsaved work, so Save is offered and asked for.
    ///
    /// A failed Undo or Redo leaves the project as it was, so a file already waiting keeps
    /// what it waits for (this failure's snapshot may be of the file as an earlier failure
    /// left it). A change taken back changes the project: `replace` puts its target in.
    fn keep_unwritten(&mut self, files: Unwritten, replace: bool) {
        if files.is_empty() {
            return;
        }
        for (path, should) in files {
            let rel = path
                .strip_prefix(&self.dir)
                .map_or_else(|_| path.clone(), Path::to_path_buf);
            // What the file holds now, as WiringCraft left it: a later change is from outside.
            self.unwritten_found
                .insert(rel.clone(), Found::of(snapshot_file(&path)));
            self.symbol_repairs.remove(&rel);
            if replace {
                self.unwritten_files.insert(rel, should);
            } else {
                self.unwritten_files.entry(rel).or_insert(should);
            }
        }
        self.edited();
    }

    /// A file no longer waits to be written again.
    fn drop_unwritten(&mut self, rel: &Path) {
        self.unwritten_files.remove(rel);
        self.unwritten_found.remove(rel);
        self.symbol_repairs.remove(rel);
    }

    /// The first project file waiting to be written again ([`Open::keep_unwritten`]) that was
    /// changed outside WiringCraft since: Save does not write over it until
    /// [`Open::keep_outside_version`] or [`Open::write_over_outside_version`] says what to do.
    pub fn changed_outside(&self) -> Option<PathBuf> {
        self.outside_change().map(|(path, _)| path)
    }

    /// [`Open::changed_outside`], with a note on a file that cannot be read (now or when
    /// WiringCraft left it).
    fn outside_change(&self) -> Option<(PathBuf, String)> {
        self.unwritten_files.iter().find_map(|(rel, should)| {
            let found = self.unwritten_found.get(rel)?;
            let path = self.dir.join(rel);
            found
                .change(snapshot_file(&path), should, &path)
                .map(|note| (path, note))
        })
    }

    /// Keep the version of `path` changed outside WiringCraft: what was waiting to be written
    /// to it is dropped, and the next save leaves the file as it is.
    pub fn keep_outside_version(&mut self, path: &Path) {
        let rel = path.strip_prefix(&self.dir).unwrap_or(path).to_path_buf();
        // A symbol file kept that way is not written with the loaded version either.
        if let Some((id, _)) = self.symbol_repairs.get(&rel) {
            self.stale_symbol_files.remove(id);
        }
        self.drop_unwritten(&rel);
    }

    /// Write what the project needs over the version of `path` changed outside WiringCraft on
    /// the next save.
    pub fn write_over_outside_version(&mut self, path: &Path) {
        let rel = path.strip_prefix(&self.dir).unwrap_or(path).to_path_buf();
        // Consent covers the version there now only: a later change is asked about again.
        let now = match snapshot_file(path) {
            Ok(bytes) => Found::Bytes(bytes),
            Err(_) => Found::WriteOverUnread(stamp_of(path)),
        };
        if let Some(found) = self.unwritten_found.get_mut(&rel) {
            *found = now;
        }
    }

    /// Files an Undo or Redo wrote as the project now needs them: none of them waits to be
    /// written again.
    fn written_as_needed(&mut self, written: &Written) {
        for f in written {
            let rel = f.path.strip_prefix(&self.dir).unwrap_or(&f.path).to_path_buf();
            self.drop_unwritten(&rel);
        }
    }

    /// Put project files a change taken back rewrote (`edits`, oldest first) back as they were
    /// before it, each replaced whole (removed when there was none). The change is taken back
    /// either way, so a file written stays written. A file another program changed since
    /// ([`outside_changes`], each file checked on its own) is left as it is, so that change is
    /// not lost, and is not written later either; the others are put back all the same. One
    /// that cannot be written is kept to be written again before the next save
    /// ([`Open::keep_unwritten`]). The error names each file not put back with its reason.
    fn write_back(&mut self, edits: &[FileEdit]) -> Result<(), String> {
        // A file rewritten more than once goes back to what it held before the first.
        let mut targets: Vec<&FileEdit> = Vec::new();
        for e in edits {
            if !targets.iter().any(|t| t.path == e.path) {
                targets.push(e);
            }
        }
        let (mut changed, mut errors, mut failed) = (Vec::new(), Vec::new(), Vec::new());
        let mut unwritten = Vec::new();
        for first in targets.into_iter().rev() {
            let rel = &first.path;
            let path = self.dir.join(rel);
            let own: Vec<&FileEdit> = edits.iter().rev().filter(|e| e.path == *rel).collect();
            match outside_changes(&self.dir, &own, true, &self.unwritten_files) {
                Ok(found) if found.is_empty() => {}
                Ok(_) => {
                    // Left as it is for good: nothing waits to be written over it either.
                    let rel = rel.clone();
                    self.drop_unwritten(&rel);
                    changed.push(path.display().to_string());
                    continue;
                }
                Err(why) => {
                    errors.push(format!("Not put back: {why}"));
                    continue;
                }
            }
            let should = first.before.as_deref().map(|t| t.as_bytes().to_vec());
            match restore_file(&path, should.as_deref()) {
                Ok(()) => {
                    self.drop_unwritten(rel);
                }
                Err(e) => {
                    failed.push(format!(
                        "{} could not be written: {e}; it is written again on the next Save",
                        path.display()
                    ));
                    unwritten.push((path, should));
                }
            }
        }
        self.keep_unwritten(unwritten, true);
        if !changed.is_empty() {
            errors.push(format!(
                "Not put back: {} was changed outside this window since, and putting it back would write over that change. It is left as it is.",
                changed.join("; ")
            ));
        }
        if !failed.is_empty() {
            errors.push(failed.join("; "));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }

    /// Write again every project file a failed Undo or Redo, or a change taken back, could not
    /// write. A save does not go ahead while one cannot be: the saved project would not match
    /// the file (a macro's undo history would be lost).
    /// Part of the message of a save refused by [`Open::changed_outside`], for the app to tell it
    /// from other save errors.
    pub(crate) const CHANGED_OUTSIDE: &str =
        "was changed outside WiringCraft after an Undo or Redo could not write it as the project needs it";

    fn put_back_unwritten_files(&mut self) -> Result<(), String> {
        // Nothing is written while one of them was changed outside since: that is not lost
        // unasked.
        if let Some((path, unread)) = self.outside_change() {
            return Err(format!(
                "not saved: {} {}{unread}. Keep that version, or write the project's version over it.",
                path.display(),
                Self::CHANGED_OUTSIDE
            ));
        }
        for (rel, should) in self.unwritten_files.clone() {
            let path = self.dir.join(&rel);
            let cannot = |e: std::io::Error| {
                format!(
                    "not saved: {} cannot be written as the project needs it: {e}",
                    path.display()
                )
            };
            let folder = match (&should, path.parent()) {
                (Some(_), Some(dir)) => std::fs::create_dir_all(dir),
                _ => Ok(()),
            };
            folder.map_err(cannot)?;
            // The check above and this write are separate steps: another program can change
            // the file in between. So the file is moved aside first and checked again as
            // moved (#670); only what the check admits is deleted or replaced.
            match self.unwritten_found.get(&rel) {
                Some(found) => match repair_file(&path, should.as_deref(), found).map_err(cannot)? {
                    Repair::Done => {}
                    Repair::ChangedOutside(note) => {
                        return Err(format!(
                            "not saved: {} {}{note}. Keep that version, or write the project's version over it.",
                            path.display(),
                            Self::CHANGED_OUTSIDE
                        ));
                    }
                },
                // Nothing known about what was there: written as before.
                None => restore_file(&path, should.as_deref()).map_err(cannot)?,
            }
            // A symbol file repaired with the loaded version is not written again unchecked.
            if let Some((id, hash)) = self.symbol_repairs.get(&rel)
                && self.symbols.get(id).is_some_and(|s| s.hash() == *hash)
            {
                self.stale_symbol_files.remove(id);
            }
            self.drop_unwritten(&rel);
        }
        Ok(())
    }

    /// A project file written on purpose (a macro saved): what was waiting to be written to
    /// it is let go.
    fn written_on_purpose(&mut self, path: &Path) {
        let rel = path.strip_prefix(&self.dir).unwrap_or(path).to_path_buf();
        self.drop_unwritten(&rel);
    }

    /// Why the last Undo or Redo failed, once.
    pub fn take_undo_error(&mut self) -> Option<String> {
        self.undo_error.take()
    }

    /// Parts the last Undo or Redo could not load, though the step was done; once.
    pub fn take_step_warning(&mut self) -> Option<String> {
        self.step_warning.take()
    }

    /// After the symbol editor saved symbol `id` into the library: take that symbol (only the
    /// symbol, never a part with the same id) into the project, as one undo step.
    pub fn take_saved_symbol(&mut self, lib: &wc_library::Library, id: &str) -> Result<(), String> {
        // Taken into a copy first: a failure leaves the project and its history as they were
        // (take writes the file only once the library symbol is read, and nothing after).
        self.archive_symbols();
        let mut next = self.project.clone();
        let entry = wc_library::take(&self.dir, &mut next, lib, ItemKind::Symbol, id).map_err(|e| e.to_string())?;
        self.checkpoint();
        self.project.lock.symbols.insert(id.to_owned(), entry);
        self.reload_symbols();
        Ok(())
    }

    pub fn save(&mut self) -> Result<usize, String> {
        self.follow_terminals();
        self.put_back_unrestored_files()?;
        self.put_back_unwritten_files()?;
        self.rewrite_stale_symbol_files()?;
        let n = store::save(&self.dir, &self.project).map_err(|e| e.to_string())?.len();
        self.dirty = false;
        Ok(n)
    }

    /// Save the project as a copy in `target`, which must not exist yet or be an empty folder,
    /// and go on working in the copy. Everything in the project folder is copied (its history
    /// included), then the changes not saved yet are saved in the copy only, so the original
    /// stays as it was last saved. Library folders given relative to the project and lying
    /// outside it are pointed at the same folders from the new place; the project's own library
    /// (inside it) is copied and stays the copy's own. Returns the folder the copy is in and the
    /// links in the project folder (relative to it) that were left out of the copy. When saving
    /// in the copy fails, the project goes on being worked on in its old folder.
    pub fn save_as(&mut self, target: &Path) -> Result<(PathBuf, Vec<PathBuf>), String> {
        // Resolved as the file system walks it (a link, then `..`), and that same folder is
        // checked, copied into and worked in, so the check and the copy cannot part ways.
        let target = destination(&crate::prefs::absolute(target)?)?;
        let from = self.dir.clone();
        // Both walked as the file system walks them, so another letter case on Windows, a link
        // or `..` (in either, a link followed by `..` too) cannot hide that the target is
        // inside the project.
        let from_real = destination(&from)?;
        if target.starts_with(&from_real) {
            return Err(format!(
                "{} is inside the project folder; pick a place outside it.",
                target.display()
            ));
        }
        // A linked git worktree's `.git` is a file pointing into another repository: a copy of
        // it would record its versions in the original's history.
        if from.join(".git").is_file() {
            return Err(format!(
                "{} is a linked git worktree; its history belongs to another repository, so it \
                 cannot be copied as the copy's own. Copy the folder by hand and start a new history.",
                from.display()
            ));
        }
        // A repository whose git config names its own work tree would go on recording the
        // original's files from the copy. With `extensions.worktreeConfig` git also reads
        // `config.worktree`, whose `core.worktree` wins over the common one.
        let git = from.join(".git");
        if git_worktree_set(&git.join("config")) || git_worktree_set(&git.join("config.worktree")) {
            return Err(format!(
                "{}'s git history names its own work tree (core.worktree), so a copy would record \
                 the original's files. Copy the folder by hand and start a new history.",
                from.display()
            ));
        }
        let existed = target.exists();
        if existed && !target.is_dir() {
            return Err(format!(
                "{} already exists; pick a new name or an empty folder.",
                target.display()
            ));
        }
        if existed {
            let mut entries =
                std::fs::read_dir(&target).map_err(|e| format!("{} cannot be read: {e}", target.display()))?;
            let empty = match entries.next() {
                None => true,
                Some(Ok(_)) => false,
                Some(Err(e)) => return Err(format!("{} cannot be read: {e}", target.display())),
            };
            if !empty {
                return Err(format!(
                    "{} already exists; pick a new name or an empty folder.",
                    target.display()
                ));
            }
        }
        // What a failed copy made is taken away again, so the same place can be tried again;
        // the folder was new or empty, so all in it is the copy's. Says what is left when that
        // fails.
        let discard = || -> String {
            let r = if existed {
                std::fs::read_dir(&target).and_then(|d| {
                    d.into_iter().try_for_each(|e| {
                        let e = e?;
                        if e.file_type()?.is_dir() {
                            std::fs::remove_dir_all(e.path())
                        } else {
                            std::fs::remove_file(e.path())
                        }
                    })
                })
            } else {
                std::fs::remove_dir_all(&target)
            };
            match r {
                Ok(()) => format!("The copy in {} was taken away again", target.display()),
                Err(e) => {
                    crate::applog::warn(&format!("could not clear the failed copy in {}: {e}", target.display()));
                    format!(
                        "The copy in {} could not all be taken away again ({e}); remove what is left by hand",
                        target.display()
                    )
                }
            }
        };
        // A new folder is made here, once: another save to the same place that got there first
        // makes this one fail before anything is copied, and nothing of that one is taken away.
        if !existed {
            target
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::create_dir(&target))
                .map_err(|e| format!("Could not make {}: {e}", target.display()))?;
        }
        let mut links = Vec::new();
        copy_dir(&from, &target, Path::new(""), &mut links)
            .map_err(|e| format!("Copying to {} failed: {e}\n{}.", target.display(), discard()))?;
        // A relative library folder outside the project is the same folder seen from the copy;
        // one inside it was copied along. The file system walks it as far as it exists (a link
        // followed by `..` goes where the file system says, not where the text does), also when
        // the library folder itself is missing now; a route it cannot walk (through a file)
        // stays the original's own, as a full path.
        // A library inside the project whose route in the copy is not text (its real folder
        // has a name that is not UTF-8) cannot be saved without changing that route: refused,
        // naming the library.
        let repoint = |libs: &mut Vec<wc_core::project::LibrarySource>| -> Result<(), String> {
            for lib in libs {
                if lib.kind == "folder" && Path::new(&lib.location).is_relative() {
                    let Ok(at) = destination(&from.join(&lib.location)) else {
                        lib.location = from.join(&lib.location).display().to_string();
                        continue;
                    };
                    lib.location = match at.strip_prefix(&from_real) {
                        // Inside the project (`library`, or `../design/library` from `design`):
                        // copied along, so the copy's own, written from the copy's root.
                        Ok(inside) if inside.as_os_str().is_empty() => ".".into(),
                        Ok(inside) => match inside.to_str() {
                            Some(t) => t.to_owned(),
                            None => {
                                return Err(format!(
                                    "library {} lies in the project folder at {}, a name that cannot \
                                     be saved as text; rename that folder and try again.",
                                    lib.name,
                                    inside.display()
                                ));
                            }
                        },
                        // A route the saved text can hold: the relative one when it is valid
                        // text, otherwise the original's own route (through any link) as a
                        // full path, which stays text.
                        Err(_) => match relative_to(&at, &target).as_deref().and_then(Path::to_str) {
                            Some(t) => t.to_owned(),
                            None => from.join(&lib.location).display().to_string(),
                        },
                    };
                }
            }
            Ok(())
        };
        // Undo and Redo must not bring the old folders back into the copy: their libraries are
        // pointed from the copy too, all worked out before anything changes.
        let mut libraries = self.project.meta.libraries.clone();
        let mut steps: Vec<_> = self
            .undo
            .iter()
            .chain(self.redo.iter())
            .map(|s| s.project.meta.libraries.clone())
            .collect();
        if let Err(e) = repoint(&mut libraries).and_then(|()| steps.iter_mut().try_for_each(&repoint)) {
            return Err(format!(
                "{e}\n{}; you are still working in {}.",
                discard(),
                from.display()
            ));
        }
        let (old_libraries, old_dirty) = (
            std::mem::replace(&mut self.project.meta.libraries, libraries),
            self.dirty,
        );
        // The save in the copy repairs stale symbol files there; the original still needs them.
        let old_stale = self.stale_symbol_files.clone();
        // Files a failed update could not put back are written again in the copy, under the
        // copy's folder; the original's records stay as they were when the copy fails.
        let old_unrestored = self.unrestored_files.clone();
        // Project files waiting to be written are named relative to the folder: the copy's.
        let old_unwritten = self.unwritten_files.clone();
        let old_found = self.unwritten_found.clone();
        let old_repairs = self.symbol_repairs.clone();
        self.unrestored_files = old_unrestored
            .iter()
            .map(|(path, f)| {
                let path = path
                    .strip_prefix(&from)
                    .map_or_else(|_| path.clone(), |rel| target.join(rel));
                (path, f.clone())
            })
            .collect();
        self.dir = target.clone();
        if let Err(e) = self.save() {
            self.dir = from.clone();
            self.project.meta.libraries = old_libraries;
            self.dirty = old_dirty;
            self.stale_symbol_files = old_stale;
            self.unrestored_files = old_unrestored;
            self.unwritten_files = old_unwritten;
            self.unwritten_found = old_found;
            self.symbol_repairs = old_repairs;
            return Err(format!(
                "{e}\n{}; you are still working in {}.",
                discard(),
                from.display()
            ));
        }
        for (s, libs) in self.undo.iter_mut().chain(self.redo.iter_mut()).zip(steps) {
            s.project.meta.libraries = libs;
        }
        Ok((target, links))
    }

    /// Make sure a library symbol is vendored and pinned before it is placed. A symbol the
    /// project already pins (one no page uses is not loaded at open) is placed in the version
    /// it pins: its copy in the project, or that version put back from the library. Only a
    /// symbol with no pin is taken from the library as it is now.
    /// [`Open::ensure_symbol`] for each of `ids`, all or none: when one can't be loaded, the
    /// pins and loaded symbols the others added are taken back, and the error names the symbol.
    /// Terminal tracking goes back with them: a copy put back restarted it from that copy.
    pub fn ensure_symbols<'a>(&mut self, ids: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
        self.touch();
        let (lock, symbols) = (self.project.lock.clone(), self.symbols.clone());
        let library = self.library.clone();
        // Putting a terminal copy back follows terminals: jumpers, rail orders and the device
        // data and panel places of spares a strip took along all go back too.
        let (meta, terminals) = (self.project.meta.clone(), self.terminals_before.clone());
        let (devices, panels) = (self.project.devices.clone(), self.project.panels.clone());
        for id in ids {
            if let Err(e) = self.ensure_symbol(id) {
                self.project.lock = lock;
                self.symbols = symbols;
                self.library = library;
                self.project.meta = meta;
                self.project.devices = devices;
                self.project.panels = panels;
                self.terminals_before = terminals;
                let named = e.starts_with(&format!("symbol {id}:"));
                return Err(if named { e } else { format!("symbol {id}: {e}") });
            }
        }
        Ok(())
    }

    pub fn ensure_symbol(&mut self, id: &str) -> Result<(), String> {
        if let Some(loaded) = self.symbols.get(id) {
            // Loaded at open from its file, which may hold another version than the pin names.
            let Some(pin) = self
                .project
                .lock
                .symbols
                .get(id)
                .filter(|pin| loaded.hash() != pin.hash)
            else {
                return Ok(());
            };
            // The pinned copy may have been put back since: the file is read again.
            let refused = |why: String| {
                format!(
                    "symbol {id}: its copy in the project is not the version the project pins ({why}); put the pinned copy back or update the symbol from the library"
                )
            };
            let pin = pin.clone();
            let vendored = store::vendor_path(&self.dir, ItemKind::Symbol, id).map_err(|e| e.to_string())?;
            let fixed = if matches!(vendored.try_exists(), Ok(false)) {
                // The copy is gone: the pinned version is looked for in the libraries, as when
                // nothing was loaded.
                self.pinned_symbol(id, &pin)?
            } else {
                let fixed = store::read_vendored_symbol(&self.dir, id).map_err(|e| refused(e.to_string()))?;
                version_fits(&fixed.id, &fixed.hash(), id, &pin.hash).map_err(refused)?;
                fixed
            };
            // Terminals were numbered with the copy loaded so far: the change in progress is
            // settled with it, and tracking starts again from the pinned copy, so putting it
            // back does not read as terminals moved.
            self.follow_terminals();
            self.load_symbol(id, fixed);
            if self.terminals_before.is_some() {
                self.terminals_before = Some(self.terminal_numbers());
            }
            return Ok(());
        }
        let s = match self.project.lock.symbols.get(id).cloned() {
            Some(pin) => self.pinned_symbol(id, &pin)?,
            None => {
                let lib = wc_library::Library::open(&self.dir, &self.project).map_err(|e| e.to_string())?;
                wc_library::take(&self.dir, &mut self.project, &lib, ItemKind::Symbol, id)
                    .map_err(|e| e.to_string())?;
                store::read_vendored_symbol(&self.dir, id).map_err(|e| e.to_string())?
            }
        };
        self.load_symbol(id, s);
        Ok(())
    }

    /// Load `s` as the project's symbol `id`, and list it with its own name and description.
    fn load_symbol(&mut self, id: &str, s: Symbol) {
        let entry = LibraryEntry::of(id, &s);
        match self.library.iter_mut().find(|e| e.id == id) {
            Some(row) => *row = entry,
            None => {
                let at = self.library.partition_point(|e| e.id.as_str() < id);
                self.library.insert(at, entry);
            }
        }
        self.symbols.insert(id.to_owned(), s);
    }

    /// The version of symbol `id` that `pin` names: its copy in the project, or, when there is
    /// none, that version put back from the libraries (written as the copy).
    fn pinned_symbol(&self, id: &str, pin: &wc_core::lock::LockEntry) -> Result<Symbol, String> {
        let vendored = store::vendor_path(&self.dir, ItemKind::Symbol, id).map_err(|e| e.to_string())?;
        let exists = vendored
            .try_exists()
            .map_err(|e| format!("symbol {id}: {} cannot be checked: {e}", vendored.display()))?;
        let fits = |s: &Symbol| version_fits(&s.id, &s.hash(), id, &pin.hash);
        if exists {
            let copy = store::read_vendored_symbol(&self.dir, id).map_err(|e| e.to_string())?;
            if fits(&copy).is_err() {
                return Err(format!(
                    "symbol {id}: its copy in the project is not the version the project pins; put the pinned copy back or update the symbol from the library"
                ));
            }
            return Ok(copy);
        }
        // The source the pin names first, then every other: any may still have the pinned
        // version.
        let s = wc_library::Library::open(&self.dir, &self.project)
            .map_err(|e| e.to_string())?
            .find_version(ItemKind::Symbol, id, &pin.source, fits)
            .map_err(|why| {
                format!(
                    "symbol {id}: its copy in the project is missing and no library has the version the project pins ({why}); put the copy back or update the symbol from the library"
                )
            })?;
        store::write_vendored_symbol(&self.dir, &s).map_err(|e| e.to_string())?;
        Ok(s)
    }

    /// Suggest the next free tag for a letter code, e.g. `=PUMP+CAB1-K3`. Function and location
    /// are taken from the tags already on the page (the most common pair), so a new device lands
    /// in the same plant section and cabinet as its neighbours.
    pub fn next_tag(&self, page: usize, letter: &str) -> Option<DeviceTag> {
        if letter.is_empty() {
            return None;
        }
        let mut counts: BTreeMap<(Vec<String>, Vec<String>), usize> = BTreeMap::new();
        for t in self
            .project
            .pages
            .get(page)
            .into_iter()
            .flat_map(|p| p.symbols.iter())
            .filter_map(|s| s.tag.as_ref())
        {
            *counts.entry((t.function.clone(), t.location.clone())).or_default() += 1;
        }
        let (function, location) = counts
            .into_iter()
            .max_by_key(|(_, n)| *n)
            .map(|(k, _)| k)
            .unwrap_or_default();
        // Device data counts too: a deleted symbol's device keeps its entry (and part), which a
        // new device must not take over.
        let product = wc_core::placement::free_product(&wc_core::placement::used_tags(&self.project), letter)?;
        let mut text = String::new();
        if !function.is_empty() {
            text.push_str(&format!("={}", function.join(".")));
        }
        if !location.is_empty() {
            text.push_str(&format!("+{}", location.join(".")));
        }
        text.push_str(&format!("-{product}"));
        DeviceTag::parse(&text).ok()
    }

    pub fn place(&mut self, page: usize, symbol: &str, at: Point) -> Result<String, String> {
        self.place_turned(page, symbol, at, Rotation::R0, false)
    }

    /// Place `symbol` at `at` on page `page`, turned by `rotation` and mirrored when `mirror`
    /// is set, as one undo step. Returns the new item's id.
    pub fn place_turned(
        &mut self,
        page: usize,
        symbol: &str,
        at: Point,
        rotation: Rotation,
        mirror: bool,
    ) -> Result<String, String> {
        self.ensure_symbol(symbol)?;
        let letter = self
            .symbols
            .get(symbol)
            .map(|s| s.letter_code.clone())
            .unwrap_or_default();
        let tag = self.next_tag(page, &letter);
        self.checkpoint();
        if let Some(t) = &tag {
            self.project.devices.entry(t.clone()).or_default();
        }
        let pg = self.project.pages.get_mut(page).ok_or("no page")?;
        let id = pg.next_id("s");
        pg.symbols.push(PlacedSymbol {
            id: id.clone(),
            symbol: symbol.to_owned(),
            at,
            rotation,
            mirror,
            tag,
        });
        Ok(id)
    }

    /// Draw a black box on page `page`: a rectangle with corners `a` and `b` (page mm) and the
    /// connection points `pins` on its edges (offsets from the box's left or top edge), tagged
    /// `tag`, as one undo step. Like `wiringcraft box`, the box gets its own project symbol
    /// `box.<page>.<id>`, pinned with source `project`, and the item and symbol ids are ones no
    /// earlier box used (a deleted box keeps its symbol file and lock entry). Returns the
    /// item id; nothing changes when the box is refused or its symbol file cannot be written.
    pub fn add_black_box(
        &mut self,
        page: usize,
        a: Point,
        b: Point,
        pins: &[wc_core::BoxPin],
        tag: Option<DeviceTag>,
    ) -> Result<String, String> {
        let (min, max) = (
            Point::new(a.x.min(b.x), a.y.min(b.y)),
            Point::new(a.x.max(b.x), a.y.max(b.y)),
        );
        self.add_black_box_sized(page, min, (max.x - min.x, max.y - min.y), pins, tag)
    }

    /// [`Self::add_black_box`] from the box's top-left corner `at` and its size (width, height
    /// in mm) as given: no size worked out from two corners, which could fall a hair short of a
    /// point at the end of an edge.
    pub fn add_black_box_sized(
        &mut self,
        page: usize,
        at: Point,
        (width, height): (f64, f64),
        pins: &[wc_core::BoxPin],
        tag: Option<DeviceTag>,
    ) -> Result<String, String> {
        let pg = self.project.pages.get(page).ok_or("no page")?;
        let (id, symbol_id) = {
            let mut n = 1usize;
            loop {
                let id = format!("s{n}");
                n += 1;
                if pg.symbol(&id).is_some() || pg.wires.iter().any(|w| w.id == id) {
                    continue;
                }
                let symbol_id = format!("box.{}.{id}", pg.id);
                if symbol_id.len() > 128 {
                    return Err(format!(
                        "page id {:?} is too long to name a box symbol (box.<page>.<id> must stay within 128 characters)",
                        pg.id
                    ));
                }
                let path = store::vendor_path(&self.dir, ItemKind::Symbol, &symbol_id).map_err(|e| e.to_string())?;
                let exists = path
                    .try_exists()
                    .map_err(|e| format!("{} cannot be checked: {e}", path.display()))?;
                if !exists
                    && !self.project.lock.symbols.contains_key(&symbol_id)
                    && !self.symbols.contains_key(&symbol_id)
                {
                    break (id, symbol_id);
                }
            }
        };
        let name = tag
            .as_ref()
            .map_or_else(|| "Black box".to_owned(), |t| format!("Black box {t}"));
        let symbol = wc_core::black_box(&symbol_id, &name, width, height, pins).map_err(|e| e.to_string())?;
        self.write_symbol_file(&symbol)?;
        self.checkpoint();
        if let Some(t) = &tag {
            self.project.devices.entry(t.clone()).or_default();
        }
        self.project.lock.symbols.insert(
            symbol_id.clone(),
            wc_core::LockEntry {
                source: "project".into(),
                hash: symbol.hash(),
            },
        );
        self.symbol_versions.insert(symbol.hash(), symbol.clone());
        self.symbols.insert(symbol_id.clone(), symbol);
        if let Some(pg) = self.project.pages.get_mut(page) {
            pg.symbols.push(PlacedSymbol {
                id: id.clone(),
                symbol: symbol_id,
                at,
                rotation: Rotation::R0,
                mirror: false,
                tag,
            });
        }
        Ok(id)
    }

    /// Change black box `item` on page `page` to `width` by `height` mm (`size`) with the connection
    /// points `pins`, as one undo step. `renamed` maps a point's old name to its new one, and the
    /// wires on it follow; `removed` names the old points that were taken off (a new point may
    /// reuse such a name, and is still another point). A wire on a point that is gone keeps its
    /// end where the point was, as a free end. The box's symbol gets a new version (its file and
    /// lock entry); Undo puts the earlier one back. A removed point's free end may not land on
    /// another wire's free end, which would join them. Returns warnings: the wires on points that
    /// are gone or moved, and other boxes that place the same symbol (they change too). Nothing
    /// changes when the box is refused or its symbol file cannot be written.
    pub fn edit_black_box(
        &mut self,
        page: usize,
        item: &str,
        size: (f64, f64),
        pins: &[wc_core::BoxPin],
        renamed: &BTreeMap<String, String>,
        removed: &std::collections::BTreeSet<String>,
    ) -> Result<Vec<String>, String> {
        self.edit_black_box_moved(page, item, size, pins, renamed, removed, Point::new(0.0, 0.0))
    }

    /// [`Self::edit_black_box`], and box `item` itself also moves by `moved` (page mm) in the
    /// same undo step: how a box sized from a near corner keeps its opposite corner in place
    /// (#529). Wires on `item` are checked against where its points end up on the page; other
    /// boxes that place the same symbol stay where they are.
    #[allow(clippy::too_many_arguments)]
    pub fn edit_black_box_moved(
        &mut self,
        page: usize,
        item: &str,
        (width, height): (f64, f64),
        pins: &[wc_core::BoxPin],
        renamed: &BTreeMap<String, String>,
        removed: &std::collections::BTreeSet<String>,
        moved: Point,
    ) -> Result<Vec<String>, String> {
        let pg = self.project.pages.get(page).ok_or("no page")?;
        let placed = pg.symbol(item).ok_or_else(|| format!("no item {item} on this page"))?;
        let symbol_id = placed.symbol.clone();
        let moves = moved.x != 0.0 || moved.y != 0.0;
        let old = self
            .symbols
            .get(&symbol_id)
            .cloned()
            .ok_or_else(|| format!("symbol {symbol_id} is not loaded"))?;
        if wc_core::black_box_layout(&old).is_none() {
            return Err(format!("{item} is not a black box"));
        }
        // The version Undo goes back to is the one the lock pins: a file changed outside the
        // app since would be lost.
        if self.project.lock.symbols.get(&symbol_id).map(|e| e.hash.as_str()) != Some(old.hash().as_str()) {
            return Err(format!(
                "the file of symbol {symbol_id} is not the version the project pins (changed outside the app?); reopen the project before editing the box"
            ));
        }
        // Every page that places the box changes with it: none may be held by someone else.
        if let Some((p, who)) = self.project.pages.iter().find_map(|p| {
            let who = self.held_by_others.get(&p.id)?;
            p.symbols.iter().any(|s| s.symbol == symbol_id).then_some((p, who))
        }) {
            return Err(format!(
                "{who} has page {} open, which places this box too; it can be changed once they leave it",
                p.number
            ));
        }
        let edit = wc_core::box_edit::BoxEdit {
            page,
            item,
            size: (width, height),
            pins,
            renamed,
            removed,
            moved,
        };
        let Some(plan) = wc_core::box_edit::plan(&self.project, &old, &edit).map_err(|e| e.to_string())? else {
            return Ok(Vec::new());
        };
        let new = plan.symbol.clone();
        self.check_box_file_unchanged(&symbol_id, &old)?;
        self.check_symbol_repair_baseline(&symbol_id)?;
        self.write_symbol_file(&new)?;
        // The file holds the loaded version again: a later change on disk is checked for, and a
        // repair a refused change queued for it would write an older version over this one.
        self.stale_symbol_files.remove(&symbol_id);
        self.symbol_file_written(&symbol_id);
        self.checkpoint();
        self.project.lock.symbols.insert(
            symbol_id.clone(),
            wc_core::LockEntry {
                source: "project".into(),
                hash: new.hash(),
            },
        );
        self.symbol_versions.insert(old.hash(), old);
        self.symbol_versions.insert(new.hash(), new.clone());
        self.symbols.insert(symbol_id.clone(), new);
        plan.apply_ends(&mut self.project);
        if moves
            && let Some(s) = self
                .project
                .pages
                .get_mut(page)
                .and_then(|p| p.symbols.iter_mut().find(|s| s.id == item))
        {
            s.at = s.at + moved;
        }
        Ok(plan.warnings)
    }

    /// Before the app writes the file of symbol `id` while a repair waits for it: refused when
    /// another program changed the file since the app last wrote it, so that change is not
    /// lost unasked; Save asks which version to keep.
    fn check_symbol_repair_baseline(&self, id: &str) -> Result<(), String> {
        let Ok(path) = store::vendor_path(&self.dir, ItemKind::Symbol, id) else {
            return Ok(());
        };
        let rel = path.strip_prefix(&self.dir).unwrap_or(&path);
        let (Some(should), Some(found)) = (self.unwritten_files.get(rel), self.unwritten_found.get(rel)) else {
            return Ok(());
        };
        match found.change(snapshot_file(&path), should, &path) {
            Some(note) => Err(format!(
                "the file of symbol {id} was changed outside WiringCraft{note}; save the project and choose which version to keep before editing the box"
            )),
            None => Ok(()),
        }
    }

    /// The file of symbol `id` was written as the project needs it: a repair waiting for it
    /// would write another version over it, so it is let go.
    fn symbol_file_written(&mut self, id: &str) {
        if let Ok(path) = store::vendor_path(&self.dir, ItemKind::Symbol, id) {
            let rel = path.strip_prefix(&self.dir).unwrap_or(&path).to_path_buf();
            self.drop_unwritten(&rel);
        }
    }

    /// Before a black box edit writes its symbol file: the file must still hold `loaded`, the
    /// version the edit starts from. Another program (`wiringcraft box-edit`) may have changed
    /// it since the project was loaded, and writing over it would lose that change (#608). A
    /// file the app itself could not put back ([`Self::mark_symbol_file_stale`]) is the app's
    /// to write, and a missing file has nothing to lose.
    fn check_box_file_unchanged(&self, symbol_id: &str, loaded: &Symbol) -> Result<(), String> {
        if self.stale_symbol_files.contains(symbol_id) {
            return Ok(());
        }
        let path = store::vendor_path(&self.dir, ItemKind::Symbol, symbol_id).map_err(|e| e.to_string())?;
        let exists = path
            .try_exists()
            .map_err(|e| format!("{} cannot be checked: {e}", path.display()))?;
        if !exists {
            return Ok(());
        }
        let reload = "reopen the project to load it before editing the box";
        match store::read_vendored_symbol(&self.dir, symbol_id) {
            Ok(on_disk) if on_disk.hash() == loaded.hash() => Ok(()),
            Ok(_) => Err(format!(
                "the file of symbol {symbol_id} changed on disk since it was loaded; {reload}"
            )),
            Err(e) => Err(format!(
                "the file of symbol {symbol_id} cannot be read ({e}), so it may have changed on disk since it was loaded; {reload}"
            )),
        }
    }

    pub fn add_wire(&mut self, page: usize, from: Endpoint, to: Endpoint, via: Vec<Point>) {
        self.checkpoint();
        if let Some(pg) = self.project.pages.get_mut(page) {
            let id = pg.next_id("w");
            pg.wires.push(Wire {
                id,
                from,
                to,
                via,
                number: None,
                potential: None,
                cable: None,
                route: Default::default(),
                spec: Default::default(),
            });
        }
    }

    /// Draw a wire between each pair of pins that line up or touch, for the symbols in `items`
    /// (just placed or moved). Part of the caller's undo step: no checkpoint is taken here.
    /// Returns how many wires were added.
    pub fn auto_wire(&mut self, page: usize, items: &std::collections::BTreeSet<String>) -> usize {
        let Some(pg) = self.project.pages.get(page) else {
            return 0;
        };
        let pairs = wc_core::autowire::suggest(pg, |id| self.symbols.get(id), items);
        let Some(pg) = self.project.pages.get_mut(page) else {
            return 0;
        };
        for (from, to) in &pairs {
            let id = pg.next_id("w");
            pg.wires.push(Wire {
                id,
                from: from.clone(),
                to: to.clone(),
                via: Vec::new(),
                number: None,
                potential: None,
                cable: None,
                route: Default::default(),
                spec: Default::default(),
            });
        }
        if !pairs.is_empty() {
            self.edited();
        }
        pairs.len()
    }

    pub fn add_page(&mut self) -> usize {
        self.checkpoint();
        let id = self.free_page_id();
        let number = self.next_page_number();
        self.project.pages.push(Page::new(id.clone(), number, "New page"));
        self.project.sort_pages();
        self.index_of(&id)
    }

    fn free_page_id(&self) -> String {
        let mut n = 1;
        while self.project.pages.iter().any(|p| p.id == format!("p{n:04}")) {
            n += 1;
        }
        format!("p{n:04}")
    }

    fn index_of(&self, id: &str) -> usize {
        self.project.pages.iter().position(|p| p.id == id).unwrap_or(0)
    }

    /// Change a page's number, title and size, and with `fields` its own title-block fields
    /// (key to value; a blank value clears the field so the project's shows again, a key left
    /// out is kept). A number another page already uses is refused. All of it is one undo step.
    pub fn set_page_props(
        &mut self,
        page: usize,
        number: &str,
        title: &str,
        size: PageSize,
        overview: bool,
        fields: Option<&BTreeMap<String, String>>,
    ) -> Result<usize, String> {
        let Some(current) = self.project.pages.get(page) else {
            return Err("No such page.".into());
        };
        let id = current.id.clone();
        let number = match self.project.free_page_number(number, Some(&id)) {
            Ok(number) => number,
            Err(PageNumberError::Empty) => return Err("The page number cannot be empty.".into()),
            Err(PageNumberError::Taken { number, .. }) => return Err(format!("Page {number} already exists.")),
        };
        let number = number.as_str();
        let mut own = current.clone();
        for (k, v) in fields.into_iter().flatten() {
            own.set_title_field(k, v);
        }
        if current.number == number
            && current.title == title
            && current.size == size
            && current.overview == overview
            && current.title_block == own.title_block
        {
            return Ok(page);
        }
        self.checkpoint();
        if let Some(p) = self.project.pages.get_mut(page) {
            p.number = number.to_owned();
            p.title = title.trim().to_owned();
            p.size = size;
            p.overview = overview;
            p.title_block = own.title_block;
        }
        self.project.sort_pages();
        Ok(self.index_of(&id))
    }

    /// Why `page` cannot be changed here: in a shared session someone else has it open.
    pub fn held_reason(&self, page: usize) -> Option<String> {
        let id = &self.project.pages.get(page)?.id;
        self.held_by_others
            .get(id)
            .map(|name| format!("{name} has this page open; it is read-only here until they leave it."))
    }

    /// The pages someone else holds, as they are now: what [`Open::held_change`] compares with.
    pub fn held_pages(&self) -> Vec<Page> {
        self.project
            .pages
            .iter()
            .filter(|p| self.held_by_others.contains_key(&p.id))
            .cloned()
            .collect()
    }

    /// Why a change is refused: it changed or removed one of the pages someone else held when
    /// [`Open::held_pages`] gave `before`.
    pub fn held_change(&self, before: &[Page]) -> Option<String> {
        before.iter().find_map(|old| {
            let who = self.held_by_others.get(&old.id).map_or("Someone", String::as_str);
            match self.project.pages.iter().position(|p| p.id == old.id) {
                None => Some(format!(
                    "it would remove page {}, which {who} has open; it is read-only here until they leave it.",
                    old.number
                )),
                Some(i) if self.project.pages[i] != *old => Some(format!(
                    "it would change page {}, which {who} has open; it is read-only here until they leave it.",
                    old.number
                )),
                Some(_) => None,
            }
        })
    }

    /// Queue the file of symbol `id` to be written with `sym` by the next save, as a repair
    /// [`Open::back_to`] tracks. A file already waiting keeps what it held when first queued,
    /// so a change made outside since is still asked about; otherwise what the app `wrote` to
    /// it (what it holds now when not known). The error says why it could not be queued.
    fn queue_symbol_repair(&mut self, id: &str, sym: &Symbol, wrote: Option<&Symbol>) -> Result<(), String> {
        let why = |e: wc_core::Error| format!("The file of symbol {id} is not written back: {e}");
        let path = store::vendor_path(&self.dir, ItemKind::Symbol, id).map_err(why)?;
        let bytes = Some(store::to_json(sym).map_err(why)?.into_bytes());
        let wrote = wrote.map(store::to_json).transpose().map_err(why)?;
        let rel = path
            .strip_prefix(&self.dir)
            .map_or_else(|_| path.clone(), Path::to_path_buf);
        if self.unwritten_found.contains_key(&rel) {
            self.unwritten_files.insert(rel.clone(), bytes);
            self.edited();
        } else {
            self.keep_unwritten(vec![(path, bytes)], true);
            // A change another program made after the app wrote the file is asked about too.
            if let Some(json) = wrote {
                self.unwritten_found
                    .insert(rel.clone(), Found::Bytes(Some(json.into_bytes())));
            }
        }
        self.symbol_repairs.insert(rel, (id.to_owned(), sym.hash()));
        Ok(())
    }

    /// Start a change that may be refused as a whole: the project and the symbols and parts
    /// loaded now are kept, and the undo history is set aside, so the change's own undo steps
    /// collect apart from it. [`Open::keep`] or [`Open::back_to`] ends it. Undo and redo
    /// must not run in between.
    /// The steps set aside still count in [`Open::undo_mark`], so a mark taken in between
    /// (a form opened by the change) stays valid after [`Open::keep`].
    pub(crate) fn mark(&mut self) -> Mark {
        let kept = self.set_aside();
        self.undo_dropped += kept.undo.len();
        self.marks += 1;
        Mark {
            kept,
            symbols: self.symbols.clone(),
            parts: self.parts.keys().cloned().collect(),
            library: self.library.clone(),
            files: vendor_names(&self.dir),
            edits_from: self.mark_edits.len(),
        }
    }

    /// A [`Mark`] ends: the file edits kept for it are let go once no mark is open.
    fn end_mark(&mut self) {
        self.marks = self.marks.saturating_sub(1);
        if self.marks == 0 {
            self.mark_edits.clear();
        }
    }

    /// The change since `mark` stays: its undo steps go after the earlier ones (the oldest
    /// dropped beyond the usual limit), and redo is kept only when it made no undo step.
    pub(crate) fn keep(&mut self, mark: Mark) {
        self.end_mark();
        let mut undo = mark.kept.undo;
        // The set-aside steps counted as dropped while the change ran; they are back now.
        self.undo_dropped -= undo.len();
        undo.append(&mut self.undo);
        self.undo = undo;
        self.trim_undo();
        if self.history == mark.kept.history {
            self.redo = mark.kept.redo;
        }
    }

    /// Take back everything changed since `mark`: the project, the symbols and parts it
    /// loaded, its undo steps, and the change marks; the undo and redo history are as before.
    /// A symbol or part file the change added (there was none, and the project as it was does
    /// not pin it) is removed; files that were there are never touched, nor rewritten. One that
    /// cannot be removed is reported, and is harmless: an unpinned part copy is pinned again
    /// when it is next used. Project files the change rewrote (a macro saved, a translation
    /// set) are put back. One of those another program changed since is left as it is, so that
    /// change is not lost, and the error names it; the others are put back all the same.
    pub(crate) fn back_to(&mut self, mark: Mark) -> Result<(), String> {
        // Project files the change rewrote are put back, newest first; they are kept apart
        // from the undo history, which may have dropped their steps.
        let edits = self.mark_edits.split_off(mark.edits_from.min(self.mark_edits.len()));
        self.end_mark();
        // Every problem is reported, the files not put back first. A file another program
        // changed since is left as it is and named; the others are put back all the same, so
        // no file of the change stays behind unnamed.
        let mut errors: Vec<String> = self.write_back(&edits).err().into_iter().collect();
        // The symbol versions the change pinned: the files it wrote hold those.
        let pinned_by_change = self.project.lock.symbols.clone();
        self.put_back(mark.kept);
        if !self.unwritten_files.is_empty() {
            // A file still to be written is unsaved work, whatever the history put back says.
            self.edited();
        }
        // Symbols the change loaded are let go and those it replaced are put back, with the
        // symbol list as it was. One put back that is the version the project pins again (a
        // box edit taken back, say) is written to its file by the next save, as its file may
        // hold the replaced version: that is unsaved work. Only a version the change pinned
        // is taken for one it wrote: one another program wrote and that was read in meanwhile
        // is left in its file.
        let pin = |o: &Self, id: &str| o.project.lock.symbols.get(id).map(|e| e.hash.clone());
        for (id, was) in &mark.symbols {
            let replaced = self.symbols.get(id).filter(|now| now.hash() != was.hash());
            let written = replaced
                .filter(|now| pinned_by_change.get(id).is_some_and(|e| e.hash == now.hash()))
                .cloned();
            if let Some(wrote) = written
                && pin(self, id) == Some(was.hash())
            {
                self.stale_symbol_files.insert(id.clone());
                // Queued with what the app wrote there, so the next save does not write over a
                // change another program made to it since.
                if let Err(e) = self.queue_symbol_repair(id, was, Some(&wrote)) {
                    errors.push(e);
                }
                self.edited();
            }
        }
        // A file waiting for the pinned version that is now loaded, where the symbol put back
        // is another version (a box edit taken back inside a change that is taken back too):
        // the next save writes that pinned version to the file all the same.
        for id in self.stale_symbol_files.clone() {
            let (Some(now), Some(was)) = (self.symbols.get(&id), mark.symbols.get(&id)) else {
                continue;
            };
            if now.hash() != was.hash() && pin(self, &id) == Some(now.hash()) {
                let now = now.clone();
                if let Err(e) = self.queue_symbol_repair(&id, &now, None) {
                    errors.push(e);
                }
            }
        }
        self.symbols = mark.symbols;
        // A file waiting to be written (from a change taken back inside this one, say) is let
        // go when the loaded symbol is gone or not the version the project pins, so a save
        // never writes a version the lock does not name.
        let stale = std::mem::take(&mut self.stale_symbol_files);
        self.stale_symbol_files = stale
            .into_iter()
            .filter(|id| {
                self.symbols
                    .get(id)
                    .is_some_and(|s| pin(self, id).is_none_or(|h| h == s.hash()))
            })
            .collect();
        // A repair queued by a change taken back inside this one is for the version that change
        // pinned. When the pin put back now is another one, the repair is retargeted to the
        // loaded symbol if that is the pinned version (its baseline kept, so a change made
        // outside since is still asked about), or let go otherwise.
        for (rel, (id, hash)) in self.symbol_repairs.clone() {
            let wanted = pin(self, &id);
            if wanted.as_deref() == Some(hash.as_str()) {
                continue;
            }
            match self.symbols.get(&id).filter(|s| wanted == Some(s.hash())).cloned() {
                Some(sym) => match store::to_json(&sym) {
                    Ok(json) => {
                        self.unwritten_files.insert(rel.clone(), Some(json.into_bytes()));
                        self.symbol_repairs.insert(rel, (id.clone(), sym.hash()));
                        self.stale_symbol_files.insert(id);
                    }
                    // The queued version is not the pinned one: let go, and the next save writes
                    // the pinned one as the loaded symbol.
                    Err(e) => {
                        errors.push(format!("The file of symbol {id} is not written back as queued: {e}"));
                        self.drop_unwritten(&rel);
                        self.stale_symbol_files.insert(id);
                    }
                },
                None => self.drop_unwritten(&rel),
            }
        }
        if !self.stale_symbol_files.is_empty() || !self.symbol_repairs.is_empty() {
            // A file still to be written back is unsaved work, also after a change taken back
            // inside this one queued it.
            self.edited();
        }
        self.parts.retain(|id, _| mark.parts.contains(id));
        self.library = mark.library;
        let (before, now) = match (mark.files, vendor_names(&self.dir)) {
            (Ok(before), Ok(now)) => (before, now),
            (Err(e), _) | (_, Err(e)) => {
                errors.push(format!("Library files it may have added were not checked: {e}"));
                return Err(errors.join("\n"));
            }
        };
        let lock = &self.project.lock;
        let mut failed = Vec::new();
        for (kind, id) in now.difference(&before) {
            let pinned = match kind {
                ItemKind::Symbol => lock.symbols.contains_key(id),
                ItemKind::Part => lock.parts.contains_key(id),
            };
            if pinned {
                continue;
            }
            if let Ok(path) = store::vendor_path(&self.dir, *kind, id)
                && let Err(e) = std::fs::remove_file(&path)
            {
                failed.push(format!("{}: {e}", path.display()));
            }
        }
        if !failed.is_empty() {
            errors.push(format!(
                "Library files it added could not be removed: {}",
                failed.join("; ")
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }

    /// Delete a page. The last page of a project cannot be deleted. Returns the page to show next.
    pub fn delete_page(&mut self, page: usize) -> Result<usize, String> {
        if self.project.pages.len() <= 1 {
            return Err("A project needs at least one page.".into());
        }
        if page >= self.project.pages.len() {
            return Err("No such page.".into());
        }
        if let Some(why) = self.held_reason(page) {
            return Err(why);
        }
        self.checkpoint();
        self.project.pages.remove(page);
        Ok(page.min(self.project.pages.len() - 1))
    }

    /// Move a page one place up or down by swapping its number with its neighbour's.
    /// Returns the page's new index, or `None` when it is already at that end.
    pub fn move_page(&mut self, page: usize, up: bool) -> Option<usize> {
        let other = if up { page.checked_sub(1)? } else { page + 1 };
        if other >= self.project.pages.len() || page >= self.project.pages.len() {
            return None;
        }
        // Both pages get a new number: refused when someone else has either open.
        if self.held_reason(page).is_some() || self.held_reason(other).is_some() {
            return None;
        }
        self.checkpoint();
        let id = self.project.pages[page].id.clone();
        let a = self.project.pages[page].number.clone();
        let b = std::mem::replace(&mut self.project.pages[other].number, a);
        self.project.pages[page].number = b;
        self.project.sort_pages();
        Some(self.index_of(&id))
    }

    /// Copy a page as a new last page. Devices on the copy get the next free tags (one new device
    /// per original device, keeping description and part) and wire numbers are cleared.
    /// Macros of the project (`macros/`) and of its library sources, project first.
    pub fn macros(&self) -> Vec<MacroEntry> {
        let mut found: Vec<(bool, String, String, PathBuf)> = wc_core::macros::list_macros(&self.dir)
            .into_iter()
            .map(|(id, path)| (true, "project".to_owned(), id, path))
            .collect();
        if let Ok(lib) = wc_library::Library::open(&self.dir, &self.project) {
            found.extend(lib.macros().into_iter().map(|(s, id, path)| (false, s, id, path)));
        }
        found
            .into_iter()
            .map(|(project, source, id, path)| {
                let (name, description, value_sets, error) = match wc_core::macros::read_macro(&path) {
                    Ok(m) => {
                        let error = m.check().err().map(|e| e.to_string());
                        let sets = m.value_sets.into_iter().map(|v| v.name).collect();
                        (m.name, m.description, sets, error)
                    }
                    Err(e) => (id.clone(), String::new(), Vec::new(), Some(e.to_string())),
                };
                MacroEntry {
                    source,
                    project,
                    path,
                    name,
                    description,
                    value_sets,
                    error,
                }
            })
            .collect()
    }

    /// Save a copied group as a macro of this project. Returns the file written.
    pub fn save_macro(
        &mut self,
        clip: &crate::edit::Clipboard,
        name: &str,
        description: &str,
    ) -> Result<PathBuf, String> {
        if name.trim().is_empty() {
            return Err("Give the macro a name.".into());
        }
        let id = wc_core::macros::macro_id(name);
        let path = wc_core::macros::write_macro(&self.dir, &id, &clip.to_macro(name, description))
            .map_err(|e| e.to_string())?;
        self.written_on_purpose(&path);
        Ok(path)
    }

    /// Why a project macro's placeholders and value sets are not saved in a shared session: its
    /// Undo and Redo go through the session's history, which does not hold project files.
    pub(crate) const MACRO_SAVE_SHARED: &str = "Macros are not edited in a shared session: the session's Undo \
         could not take the edit back. Stop sharing (or leave) first.";

    /// Save a project macro's placeholders and value sets from `form` ([`crate::macro_form::Form::save`])
    /// as one undo step: Undo puts the file back as it was, Redo writes the edit again. Nothing
    /// is recorded when the file is the same. Returns the file written.
    pub fn save_macro_form(&mut self, form: &crate::macro_form::Form) -> Result<PathBuf, String> {
        let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
        // A file waiting to be repaired holds the wrong text: Undo, or a change taken back,
        // goes back to what the project needs it to hold.
        let rel = form.path.strip_prefix(&self.dir).unwrap_or(&form.path);
        let before = match self.unwritten_files.get(rel) {
            Some(Some(bytes)) => {
                String::from_utf8(bytes.clone()).map_err(|e| format!("{}: {e}", form.path.display()))?
            }
            _ => read(&form.path)?,
        };
        let path = form.save()?;
        self.written_on_purpose(&path);
        let after = read(&path)?;
        if after != before {
            self.record_file_edit(FileEdit {
                path: path.strip_prefix(&self.dir).unwrap_or(&path).to_path_buf(),
                before: Some(before),
                after: Some(after),
            });
        }
        Ok(path)
    }

    /// Make a project file this window just rewrote one undo step: Undo puts it back as it
    /// was, Redo writes it again, and a change taken back whole ([`Open::back_to`]) puts it
    /// back too.
    fn record_file_edit(&mut self, edit: FileEdit) {
        if self.marks > 0 {
            self.mark_edits.push(edit.clone());
        }
        self.checkpoint();
        if let Some(step) = self.undo.last_mut() {
            step.files.push(edit);
        }
    }

    /// Why a guest's window does not change the translations in a shared session: the session
    /// does not carry `translations.toml`, so the edit would stay in the guest's own copy.
    pub(crate) const TRANSLATIONS_GUEST: &str = "Not done: in a shared session only the host can change the translations (the session does not carry translations.toml). Ask the host to make this change.";

    /// The project's `translations.toml` as it is on disk: `None` when there is none, an error
    /// when it cannot be read as text.
    fn read_translations(&self) -> Result<Option<String>, String> {
        let path = self.dir.join(wc_core::translate::TRANSLATIONS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{} cannot be read: {e}", path.display())),
        }
    }

    /// Make what `translations.toml` holds now one undo step, when it is not what it held
    /// `before`: a project file edit, like a macro saved ([`Open::save_macro_form`]). Returns
    /// whether it had changed.
    fn note_translations_since(&mut self, before: Option<String>) -> Result<bool, String> {
        let after = self.read_translations()?;
        if after == before {
            return Ok(false);
        }
        let path = PathBuf::from(wc_core::translate::TRANSLATIONS_FILE);
        // Written on purpose: what was waiting to be written to it is let go. It held the
        // wrong text while it waited, so Undo, or a change taken back, goes back to what the
        // project needed it to hold.
        let before = match self.unwritten_files.get(&path) {
            Some(Some(bytes)) => {
                Some(String::from_utf8(bytes.clone()).map_err(|e| format!("{}: {e}", self.dir.join(&path).display()))?)
            }
            Some(None) => None,
            None => before,
        };
        self.drop_unwritten(&path);
        if after != before {
            self.record_file_edit(FileEdit { path, before, after });
        }
        Ok(true)
    }

    /// Run `f`, which may write `translations.toml` in the project folder it is given, and make
    /// what it changed there one undo step (#589): Undo puts the file back as it was just
    /// before `f` (removed when there was none), Redo writes it again. Nothing is recorded when
    /// the file is the same. A file that cannot be read as text refuses the change with the
    /// read error before `f` runs, so it is never written over. When `f` fails and what it left
    /// cannot be read either, the error gives both reasons: there is no undo step for it.
    pub fn translations_change<R, E: std::fmt::Display>(
        &mut self,
        f: impl FnOnce(&Path) -> Result<R, E>,
    ) -> Result<R, String> {
        let before = self.read_translations()?;
        let r = f(&self.dir);
        // Whatever `f` answered, what it left in the file is what Undo takes back.
        let noted = self.note_translations_since(before);
        match (r, noted) {
            (Ok(r), Ok(_)) => Ok(r),
            (Ok(_), Err(why)) => Err(why),
            (Err(e), Ok(_)) => Err(e.to_string()),
            (Err(e), Err(why)) => Err(format!("{e}\n{}: {why}", Self::TRANSLATIONS_NO_STEP)),
        }
    }

    /// What is said when a change of `translations.toml` could not be made an undo step,
    /// before the reason.
    pub(crate) const TRANSLATIONS_NO_STEP: &str = "Undo will not take back what was changed in translations.toml";

    /// Before a command that may write `translations.toml` runs: keep the file as it is, so
    /// [`Open::note_translations`] after the command makes an undo step of the command's
    /// change. A file that cannot be read is kept as that, with the reason: no step is made
    /// for it, and [`Open::note_translations`] says why. No file is a file to start from.
    pub fn before_translations_command(&mut self) {
        self.command_translations = Some(self.read_translations());
    }

    /// After the command [`Open::before_translations_command`] was called for: what it changed
    /// in `translations.toml` is one undo step. Returns whether there is such a step. An error,
    /// with the reason, when the file could not be read before the command or cannot be after
    /// it: what the command changed in it, if anything, is then no undo step.
    pub fn note_translations(&mut self) -> Result<bool, String> {
        let no_step = |why: String| format!("{}: {why}", Self::TRANSLATIONS_NO_STEP);
        match self.command_translations.take() {
            Some(Ok(before)) => self.note_translations_since(before).map_err(no_step),
            Some(Err(why)) => Err(no_step(why)),
            None => Ok(false),
        }
    }

    /// Insert a macro on a page where it was drawn (its anchor), like a paste, with its
    /// placeholders filled from the value set named `value_set` (a macro with value sets needs
    /// one). Its devices get the macro's data, values filled in. Returns the inserted items,
    /// selected so they can be moved into place.
    pub fn insert_macro(
        &mut self,
        page: usize,
        path: &Path,
        value_set: Option<&str>,
    ) -> Result<Vec<Selection>, String> {
        if let Some(why) = self.held_reason(page) {
            return Err(why);
        }
        let m = wc_core::macros::read_macro(path)
            .and_then(|m| m.apply(value_set, &Default::default()))
            .map_err(|e| e.to_string())?;
        let clip = crate::edit::Clipboard::from(m);
        let at = clip.anchor;
        self.paste(page, &clip, at)
    }

    /// Number the wires by `rule`, keeping numbers already given. One undo step; nothing is
    /// recorded when no number changes. In a shared session the pages someone else has open
    /// keep their wires as they are; their numbers are listed in `skipped`, and a later run
    /// gives their wires the numbers their connections got elsewhere.
    pub fn number_wires(&mut self, rule: wc_core::numbering::Rule) -> WireNumbering {
        self.follow_terminals();
        let mut project = self.project.clone();
        let symbols = &self.symbols;
        let done = wc_core::numbering::number_wires(
            &mut project,
            rule,
            false,
            |id| symbols.get(id).map(|s| s.role),
            |page, end| match end {
                wc_core::Endpoint::Point { at } => Some(*at),
                wc_core::Endpoint::Pin { item, pin } => {
                    let placed = page.symbol(item)?;
                    wc_pdf::pin_position(placed, symbols.get(&placed.symbol)?, pin)
                }
            },
        );
        let mut done = done;
        let mut skipped = Vec::new();
        if !self.held_by_others.is_empty() {
            for (new, old) in project.pages.iter_mut().zip(&self.project.pages) {
                if self.held_by_others.contains_key(&new.id) && new != old {
                    skipped.push(old.number.clone());
                    *new = old.clone();
                }
            }
            if !skipped.is_empty() {
                // Count again what is left: only the pages that were really changed.
                let mut numbers = std::collections::BTreeSet::new();
                done.wires = 0;
                for (new, old) in project.pages.iter().zip(&self.project.pages) {
                    for (w, was) in new.wires.iter().zip(&old.wires) {
                        if w.number != was.number {
                            done.wires += 1;
                            numbers.insert(w.number.clone());
                        }
                    }
                }
                done.connections = numbers.len();
            }
        }
        if done.wires > 0 {
            self.checkpoint();
            self.project = project;
        }
        WireNumbering { done, skipped }
    }

    /// Replace `query` with `with` in the chosen find `hits`, as one undo step
    /// ([`wc_core::find::replace`]). Nothing changes when a renamed tag clashes or is not a
    /// tag, or when the change reaches a page someone else in the session has open: the reason
    /// comes back. Returns how many values changed.
    pub fn replace_found(
        &mut self,
        hits: &[wc_core::find::Hit],
        query: &str,
        with: &str,
        opts: wc_core::find::FindOptions,
    ) -> Result<usize, String> {
        self.follow_terminals();
        let mut project = self.project.clone();
        let (n, renamed) = wc_core::find::replace_renaming(&mut project, hits, query, with, opts)?;
        let held: Vec<String> = project
            .pages
            .iter()
            .zip(&self.project.pages)
            .filter(|(new, old)| new != old)
            .filter_map(|(new, _)| {
                self.held_by_others
                    .get(&new.id)
                    .map(|who| format!("page {} ({who})", new.number))
            })
            .collect();
        if !held.is_empty() {
            return Err(format!(
                "Nothing was replaced: it would change {}, which someone else has open.",
                held.join(", ")
            ));
        }
        if n > 0 {
            self.checkpoint();
            self.project = project;
            if !renamed.is_empty() {
                self.device_renames.push(renamed);
            }
        }
        Ok(n)
    }

    pub fn duplicate_page(&mut self, page: usize) -> Option<usize> {
        let mut copy = self.project.pages.get(page)?.clone();
        self.checkpoint();
        copy.id = self.free_page_id();
        copy.number = self.next_page_number();
        copy.title = format!("{} (copy)", copy.title);
        // A project has one cover page; its copy is an ordinary page.
        copy.cover = false;
        let mut renamed: BTreeMap<DeviceTag, DeviceTag> = BTreeMap::new();
        for s in &mut copy.symbols {
            let Some(old) = s.tag.clone() else { continue };
            let new = match renamed.get(&old) {
                Some(t) => t.clone(),
                None => {
                    let Some(t) = self.free_tag_like(&old) else { continue };
                    let device = self.project.devices.get(&old).cloned().unwrap_or_default();
                    self.project.devices.insert(t.clone(), device);
                    renamed.insert(old, t.clone());
                    t
                }
            };
            s.tag = Some(new);
        }
        for w in &mut copy.wires {
            w.number = None;
        }
        let id = copy.id.clone();
        self.project.pages.push(copy);
        self.project.sort_pages();
        Some(self.index_of(&id))
    }

    /// A free tag with the same function, location and letter code as `like`.
    pub(crate) fn free_tag_like(&self, like: &DeviceTag) -> Option<DeviceTag> {
        wc_core::placement::free_tag_like(&wc_core::placement::used_tags(&self.project), like)
    }

    fn next_page_number(&self) -> String {
        let n = self
            .project
            .pages
            .iter()
            .filter_map(|p| p.number.parse::<u32>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        n.to_string()
    }

    /// Set the project name, title-block fields and, when given, the whole list of locations as
    /// (path, description): empty rows are left out, a bad, doubled or pathless row is refused.
    /// Empty title-block values are dropped. One undo step.
    pub fn set_project_props(
        &mut self,
        name: &str,
        fields: &[(String, String)],
        locations: Option<&[(String, String)]>,
        revisions: Option<&[RevisionRow]>,
    ) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("The project name cannot be empty.".into());
        }
        let revisions = revisions.map(|rows| self.revised(rows)).transpose()?;
        // The copy below replaces the meta: jumpers follow the change before it first.
        self.follow_terminals();
        let mut meta = self.project.meta.clone();
        meta.name = name.to_owned();
        meta.title_block = title_block_of(fields);
        if let Some(locations) = locations {
            meta.locations = wc_core::project::location_rows(locations)?;
        }
        if let Some(revisions) = revisions {
            meta.revisions = revisions;
        }
        if meta == self.project.meta {
            return Ok(());
        }
        self.checkpoint();
        self.project.meta = meta;
        Ok(())
    }

    /// Why the revision rows of the properties dialog (`draft`, loaded as `loaded`) cannot be
    /// applied, if they cannot: also when the project's revisions changed since the dialog
    /// loaded them (someone else in a shared session), as the rows would put back what they
    /// had then. Rows left as loaded are not applied, so they are never refused.
    pub fn revision_draft_problem(&self, loaded: &[RevisionRow], draft: &[RevisionRow]) -> Option<String> {
        if draft == loaded {
            return None;
        }
        if RevisionRow::of(&self.project.meta.revisions) != loaded {
            return Some(
                "The revisions were changed by someone else since this dialog opened. Cancel and \
                 open it again to edit them as they are now."
                    .to_owned(),
            );
        }
        self.revised(draft).err()
    }

    /// The project's revisions as the rows of the properties dialog give them: a row with
    /// the letter it was loaded with (`was`) changes that revision, a row without one adds a
    /// revision after the others, and a revision no row has is removed. Refused with the reason
    /// when a letter is empty, taken or unprintable, or a removed revision still has a cloud.
    pub fn revised(&self, rows: &[RevisionRow]) -> Result<Vec<wc_core::Revision>, String> {
        let mut p = Project::new("");
        p.meta = self.project.meta.clone();
        p.pages = self.project.pages.clone();
        let kept: Vec<&str> = rows.iter().map(|r| r.was.trim()).filter(|w| !w.is_empty()).collect();
        let gone: Vec<String> = p
            .meta
            .revisions
            .iter()
            .map(|r| r.letter.clone())
            .filter(|l| !kept.contains(&l.as_str()))
            .collect();
        let err = |e: wc_core::Error| e.to_string();
        for l in gone {
            p.remove_revision(&l).map_err(err)?;
        }
        for r in rows {
            if r.was.trim().is_empty() {
                p.add_revision(&r.letter, &r.date, &r.text, &r.author).map_err(err)?;
            } else {
                p.edit_revision(&r.was, &r.date, &r.text, &r.author).map_err(err)?;
            }
        }
        Ok(p.meta.revisions)
    }

    /// Replace the project's check rules ([`wc_core::check_rule`]). One undo step; nothing
    /// changes when they are the same.
    pub fn set_check_rules(&mut self, rules: Vec<wc_core::check_rule::CheckRule>) {
        if rules == self.project.meta.rules {
            return;
        }
        self.checkpoint();
        self.project.meta.rules = rules;
    }

    /// Replace the project's `rules_off` (the library check rules it turns off). One undo
    /// step; nothing changes when it is the same.
    pub fn set_rules_off(&mut self, off: Vec<String>) {
        if off == self.project.meta.rules_off {
            return;
        }
        self.checkpoint();
        self.project.meta.rules_off = off;
    }

    /// Keep `layout` as the project's layout for report `kind`, in place of its first one
    /// (later ones for the kind are left, unused, for [`wc_report::layout_warnings`] to name);
    /// `None` drops every one of the kind, so the report is built in. One undo step; nothing
    /// changes when it is the same.
    pub fn set_report_layout(&mut self, kind: wc_report::Kind, layout: Option<wc_core::report_layout::ReportLayout>) {
        let mut reports = self.project.meta.reports.clone();
        match layout {
            Some(l) => match reports.iter_mut().find(|r| r.kind == kind.name()) {
                Some(slot) => *slot = l,
                None => reports.push(l),
            },
            None => reports.retain(|r| r.kind != kind.name()),
        }
        if reports == self.project.meta.reports {
            return;
        }
        self.checkpoint();
        self.project.meta.reports = reports;
    }

    /// Whether [`Open::cover_page`] would mark the first page as the cover: no page is marked
    /// yet, and the first one is an empty page titled Cover.
    pub fn cover_marks_first(&self) -> bool {
        self.project.cover_page().is_none()
            && self.project.pages.first().is_some_and(|first| {
                first.title.trim().eq_ignore_ascii_case("cover")
                    && first.symbols.is_empty()
                    && first.wires.is_empty()
                    && first.graphics.is_empty()
                    && first.underlay.is_none()
            })
    }

    /// The cover page's index. A project without one gets one: an empty first page titled
    /// "Cover" (as new projects started) is marked as the cover, otherwise a new cover page
    /// numbered `0` goes in front of the others. Creating or marking it is one undo step. Fails,
    /// changing nothing, when a page already uses a number that sorts as low as `0`.
    pub fn cover_page(&mut self) -> Result<usize, String> {
        if let Some(i) = self.project.cover_page() {
            return Ok(i);
        }
        if self.cover_marks_first() {
            self.checkpoint();
            if let Some(first) = self.project.pages.first_mut() {
                first.cover = true;
            }
            return Ok(0);
        }
        // `0` sorts before every other number unless one sorts as low (`0`, `00`, an empty one).
        let lowest = wc_core::project::natural_key("0");
        if let Some(p) = self
            .project
            .pages
            .iter()
            .find(|p| wc_core::project::natural_key(&p.number) <= lowest)
        {
            return Err(format!(
                "Page {:?} already comes first. Renumber it, then add the cover page in front.",
                p.number
            ));
        }
        self.checkpoint();
        let id = self.free_page_id();
        let mut page = Page::new(id.clone(), "0", "Cover");
        page.cover = true;
        if let Some(first) = self.project.pages.first() {
            page.size = first.size;
        }
        self.project.pages.push(page);
        self.project.sort_pages();
        Ok(self.index_of(&id))
    }

    /// Mark the page at `page` as the project's cover page (`on`), or as a normal page again,
    /// as one undo step. Any other cover page loses the mark: a project has one.
    pub fn set_page_cover(&mut self, page: usize, on: bool) -> Result<(), String> {
        let current = self.project.pages.get(page).ok_or("No such page.")?;
        let others = self.project.pages.iter().enumerate().any(|(i, p)| i != page && p.cover);
        if current.cover == on && !(on && others) {
            return Ok(());
        }
        self.checkpoint();
        self.project.set_cover(page, on);
        Ok(())
    }

    /// A properties form opens: its live edits will make a fresh undo step. Returns the undo
    /// mark to pass to [`Open::revert_live`].
    pub fn start_live_form(&mut self) -> usize {
        self.form_step = None;
        self.undo_mark()
    }

    /// Counts every change to the undo history. A form that shows project values compares it
    /// with the count it last saw to notice changes it did not make (an undo, the assistant).
    pub fn history_count(&self) -> u64 {
        self.history
    }

    /// The project's name and fields when the history moved since `seen` (a change the form
    /// did not make), updating `seen`; `None` when nothing else changed.
    pub fn props_if_changed(&self, seen: &mut u64) -> Option<PropsSnapshot> {
        if *seen == self.history {
            return None;
        }
        *seen = self.history;
        Some(self.props_snapshot())
    }

    /// Set the project name and fields while a form is open, so the page shows each change at
    /// once. The form's changes since [`Open::start_live_form`] stay one undo step while nothing
    /// else changes the history. An empty name is not applied (it is being typed).
    pub fn set_project_props_live(&mut self, name: &str, fields: &[(String, String)]) {
        let name = name.trim();
        let block = title_block_of(fields);
        let name_changes = !name.is_empty() && self.project.meta.name != name;
        if !name_changes && self.project.meta.title_block == block {
            return;
        }
        self.live_form_step();
        if name_changes {
            self.project.meta.name = name.to_owned();
        }
        self.project.meta.title_block = block;
    }

    /// Set the page template while its form is open, so every page shows each change at once.
    /// Like [`Open::set_project_props_live`], the form's changes stay one undo step while
    /// nothing else changes the history.
    pub fn set_template_live(&mut self, template: &PageTemplate) {
        if self.project.meta.template == *template {
            return;
        }
        self.live_form_step();
        self.project.meta.template = template.clone();
    }

    /// Before a live form's change: keep adding to the form's own undo step only while nothing
    /// else has touched the history since; after any other change (a page added, another
    /// properties edit, an undo) the form starts a new step.
    fn live_form_step(&mut self) {
        if self.form_step == Some(self.history) {
            self.edited();
        } else {
            self.checkpoint();
            self.form_step = Some(self.history);
        }
    }

    /// Take back what [`Open::set_template_live`] changed since `since` (the form was
    /// cancelled), back to `before`. When the form's own undo step is the last one and holds
    /// nothing else, it is undone; otherwise the template alone is put back as a new step.
    pub fn revert_template(&mut self, since: usize, before: &PageTemplate) {
        self.revert_meta(since, before, |m| &mut m.template);
    }

    /// Show `racks` as the project's PLC racks at once, as one undo step with the form's
    /// other changes (see [`Open::set_template_live`]).
    pub fn set_racks_live(&mut self, racks: &[PlcRack]) {
        if self.project.meta.racks == racks {
            return;
        }
        self.live_form_step();
        self.project.meta.racks = racks.to_vec();
    }

    /// Take back what [`Open::set_racks_live`] changed since `since` (the form was
    /// cancelled), back to `before`, as [`Open::revert_template`] does for the template.
    pub fn revert_racks(&mut self, since: usize, before: &[PlcRack]) {
        self.revert_meta(since, &before.to_vec(), |m| &mut m.racks);
    }

    /// Put the part of the project's settings that `field` picks back to `before` after a live
    /// form was cancelled. When the form's own undo step is the last one and holds nothing
    /// else, it is undone; otherwise that part alone is put back as a new step.
    fn revert_meta<T: Clone + PartialEq>(
        &mut self,
        since: usize,
        before: &T,
        field: impl Fn(&mut wc_core::project::ProjectMeta) -> &mut T,
    ) {
        let mut now = self.project.meta.clone();
        let current = field(&mut now).clone();
        let only_the_form = self.undo_mark() == since + 1
            && self.undo.last().is_some_and(|prev| {
                let mut p = prev.project.clone();
                let mut was = p.meta.clone();
                let ok = *field(&mut was) == *before;
                *field(&mut p.meta) = current.clone();
                p == self.project && ok && prev.files.is_empty()
            });
        // When the undo cannot be done (the symbol files could not follow it), that part alone
        // is put back below, so Cancel still leaves it as it was.
        if only_the_form && self.undo() {
            self.redo.clear();
            return;
        }
        if current != *before {
            self.checkpoint();
            *field(&mut self.project.meta) = before.clone();
        }
    }

    /// Whether the last undo step differs from the project only in its name and title-block
    /// fields (it is a properties form's step, with nothing else in it).
    fn last_step_changed_only_props(&self) -> bool {
        self.undo.last().is_some_and(|prev| {
            let mut p = prev.project.clone();
            p.meta.name.clone_from(&self.project.meta.name);
            p.meta.title_block.clone_from(&self.project.meta.title_block);
            p == self.project && prev.files.is_empty()
        })
    }

    /// The project name and fields as they are, for [`Open::revert_live`].
    pub fn props_snapshot(&self) -> PropsSnapshot {
        PropsSnapshot {
            name: self.project.meta.name.clone(),
            title_block: self.project.meta.title_block.clone(),
        }
    }

    /// Take back what [`Open::set_project_props_live`] changed since `since` (the form was
    /// cancelled), back to `before` (a [`Open::props_snapshot`] taken when the form opened).
    /// When the form's own undo step is the last one and holds nothing else, it is undone;
    /// otherwise (the page was edited meanwhile, or the assistant folded the step into its
    /// own) only the name and fields are put back, as a new undo step.
    pub fn revert_live(&mut self, since: usize, before: &PropsSnapshot) {
        let only_the_form = self.undo_mark() == since + 1
            && self.last_step_changed_only_props()
            && self.undo.last().is_some_and(|prev| {
                prev.project.meta.name == before.name && prev.project.meta.title_block == before.title_block
            });
        if only_the_form {
            if self.undo() {
                self.redo.clear();
            }
            return;
        }
        if self.project.meta.name != before.name || self.project.meta.title_block != before.title_block {
            self.checkpoint();
            self.project.meta.name.clone_from(&before.name);
            self.project.meta.title_block.clone_from(&before.title_block);
        }
    }

    /// Put a wire into a cable as a core, or take it out (`None`). The cable device is created
    /// when it does not exist yet.
    pub fn set_wire_cable(&mut self, page: usize, wire: &str, cable: Option<wc_core::CableCore>) {
        let current = self
            .project
            .pages
            .get(page)
            .and_then(|p| p.wires.iter().find(|w| w.id == wire))
            .map(|w| w.cable.clone());
        if current.is_none() || current == Some(cable.clone()) {
            return;
        }
        self.checkpoint();
        if let Some(c) = &cable {
            self.project.devices.entry(c.cable.clone()).or_default();
        }
        if let Some(w) = self
            .project
            .pages
            .get_mut(page)
            .and_then(|p| p.wires.iter_mut().find(|w| w.id == wire))
        {
            w.cable = cable;
        }
    }

    /// Jumper terminals of a strip together, as one undo step. See [`wc_core::project::ProjectMeta::add_jumper`].
    pub fn add_jumper(&mut self, strip: &DeviceTag, terminals: &[String]) -> Result<(), String> {
        self.follow_terminals();
        let mut meta = self.project.meta.clone();
        meta.add_jumper(strip, terminals)?;
        if meta != self.project.meta {
            self.checkpoint();
            self.project.meta = meta;
        }
        Ok(())
    }

    /// Give the jumper that holds `terminal` of `strip` a part (taken from the library into
    /// the project when needed), or none, as one undo step.
    pub fn set_jumper_part(&mut self, strip: &DeviceTag, terminal: &str, part: Option<&str>) -> Result<(), String> {
        self.follow_terminals();
        let mut meta = self.project.meta.clone();
        if !meta.set_jumper_part(strip, terminal, part)? {
            return Ok(());
        }
        let lock = self.project.lock.clone();
        if let Some(id) = part
            && let Err(e) = self.ensure_part(id)
        {
            self.project.lock = lock;
            return Err(e);
        }
        self.checkpoint_with_lock(lock);
        self.project.meta.jumpers = meta.jumpers;
        Ok(())
    }

    /// Whether jumpering `terminals` of `strip` is worth offering: `Ok(true)` when it changes the
    /// jumpers, `Ok(false)` when they are already jumpered together, the reason when refused.
    pub fn jumper_offer(&self, strip: &DeviceTag, terminals: &[String]) -> Result<bool, String> {
        let mut after = self.project.meta.clone();
        after.add_jumper(strip, terminals)?;
        Ok(after != self.project.meta)
    }

    /// Take a terminal of a strip out of its jumper, as one undo step. Returns whether it was in one.
    pub fn remove_from_jumper(&mut self, strip: &DeviceTag, terminal: &str) -> bool {
        self.remove_from_jumpers(strip, &[terminal.to_owned()])
    }

    /// Take terminals of a strip out of their jumpers, all as one undo step. Returns whether any
    /// of them was in one.
    pub fn remove_from_jumpers(&mut self, strip: &DeviceTag, terminals: &[String]) -> bool {
        self.follow_terminals();
        let mut meta = self.project.meta.clone();
        let mut changed = false;
        for t in terminals {
            changed |= meta.remove_from_jumper(strip, t.trim());
        }
        if !changed {
            return false;
        }
        self.checkpoint();
        self.project.meta = meta;
        true
    }

    /// Every terminal strip with its terminals in rail order, each with whether it is a spare
    /// (see [`wc_core::project::strip_terminals`]).
    pub fn strip_terminals(&self) -> BTreeMap<DeviceTag, Vec<(String, bool)>> {
        wc_core::project::strip_terminals(&self.project, |id| self.symbols.get(id).map(|s| s.role))
    }

    /// Change how `strip` is arranged (terminal order, spares) with `f`, which gets the strip's
    /// current rail order; one undo step when anything changed.
    fn edit_strip(
        &mut self,
        strip: &DeviceTag,
        f: impl FnOnce(&mut wc_core::Project, &[(String, bool)]) -> Result<(), String>,
    ) -> Result<(), String> {
        self.follow_terminals();
        let current = self.strip_terminals().remove(strip).unwrap_or_default();
        let mut project = self.project.clone();
        f(&mut project, &current)?;
        if project.meta != self.project.meta || project.devices != self.project.devices {
            self.checkpoint();
            self.project.meta = project.meta;
            self.project.devices = project.devices;
        }
        Ok(())
    }

    /// Move terminal `number` of `strip` to place `to` (0 is first) on the rail.
    pub fn move_terminal(&mut self, strip: &DeviceTag, number: &str, to: usize) -> Result<(), String> {
        self.edit_strip(strip, |p, current| {
            p.meta.move_terminal(strip, number, to, current).map(|_| ())
        })
    }

    /// Add spare terminal `number` to `strip`, right after terminal `after` or at the end.
    pub fn add_spare(&mut self, strip: &DeviceTag, number: &str, after: Option<&str>) -> Result<(), String> {
        self.edit_strip(strip, |p, current| p.add_spare(strip, number, after, current))
    }

    /// Take spare terminal `number` off `strip` (and out of its jumper); one that is not drawn
    /// also loses its device entry.
    pub fn remove_spare(&mut self, strip: &DeviceTag, number: &str) {
        let _ = self.edit_strip(strip, |p, current| {
            p.remove_spare(strip, number, current);
            Ok(())
        });
    }

    /// Put `strip` back in the order by terminal number; its spares go at the end.
    pub fn reset_terminal_order(&mut self, strip: &DeviceTag) {
        let _ = self.edit_strip(strip, |p, _| p.meta.set_terminal_order(strip, &[]).map(|_| ()));
    }

    /// Set one part of the spec (section, colour or type) on wires of a page; `None` clears it.
    /// One undo step for all of them; nothing happens when no wire changes. Returns how many
    /// wires changed.
    pub fn set_wire_spec(&mut self, page: usize, wires: &[String], part: SpecPart, value: Option<String>) -> usize {
        let Some(p) = self.project.pages.get(page) else {
            return 0;
        };
        let changes = p
            .wires
            .iter()
            .filter(|w| wires.contains(&w.id) && *part.of(&w.spec) != value)
            .count();
        if changes == 0 {
            return 0;
        }
        self.checkpoint();
        if let Some(p) = self.project.pages.get_mut(page) {
            for w in p.wires.iter_mut().filter(|w| wires.contains(&w.id)) {
                *part.of_mut(&mut w.spec) = value.clone();
            }
        }
        changes
    }

    /// Cables used anywhere in the project, for the cable picker.
    pub fn cables(&self) -> Vec<DeviceTag> {
        let set: std::collections::BTreeSet<DeviceTag> = self
            .project
            .pages
            .iter()
            .flat_map(|p| p.wires.iter())
            .filter_map(|w| w.cable.as_ref().map(|c| c.cable.clone()))
            .collect();
        set.into_iter().collect()
    }

    /// Position of a pin on a page.
    pub fn pin_pos(&self, page: &Page, item: &str, pin: &str) -> Option<Point> {
        let placed = page.symbol(item)?;
        let sym = self.symbols.get(&placed.symbol)?;
        sym.pin(pin)
            .map(|p| p.at.transform(placed.rotation, placed.mirror) + placed.at)
    }

    pub fn end_pos(&self, page: &Page, e: &Endpoint) -> Option<Point> {
        match e {
            Endpoint::Point { at } => Some(*at),
            Endpoint::Pin { item, pin } => self.pin_pos(page, item, pin),
        }
    }

    /// Which way a wire leaves this end: along the pin's direction, or `None` for a free point.
    pub fn end_axis(&self, page: &Page, e: &Endpoint) -> Option<Axis> {
        let Endpoint::Pin { item, pin } = e else {
            return None;
        };
        let placed = page.symbol(item)?;
        let pin = self.symbols.get(&placed.symbol)?.pin(pin)?;
        Some(Axis::of(pin.dir, placed.rotation))
    }

    /// Redraw these wires between the same two ends in `style`, as one undo step. Returns how
    /// many wires were changed; wires with an end on a missing symbol are left alone.
    pub fn set_route(&mut self, page: usize, wires: &[String], style: Route) -> usize {
        let Some(pg) = self.project.pages.get(page) else {
            return 0;
        };
        let changes: Vec<(String, Vec<Point>)> = pg
            .wires
            .iter()
            .filter(|w| wires.contains(&w.id))
            .filter_map(|w| {
                let a = self.end_pos(pg, &w.from)?;
                let b = self.end_pos(pg, &w.to)?;
                let via = route::via(a, self.end_axis(pg, &w.from), b, self.end_axis(pg, &w.to), style);
                (via != w.via || w.route != style).then(|| (w.id.clone(), via))
            })
            .collect();
        if changes.is_empty() {
            return 0;
        }
        self.checkpoint();
        if let Some(pg) = self.project.pages.get_mut(page) {
            for (id, via) in &changes {
                if let Some(w) = pg.wires.iter_mut().find(|w| &w.id == id) {
                    w.via = via.clone();
                    w.route = style;
                }
            }
        }
        changes.len()
    }
}

/// The project name and title-block fields at one moment (see [`Open::props_snapshot`]).
#[derive(Debug, Clone, PartialEq)]
pub struct PropsSnapshot {
    pub name: String,
    pub title_block: std::collections::BTreeMap<String, String>,
}

/// The name review comments are signed with: `WIRINGCRAFT_USER`, then git's `user.name`, then
/// the login name.
/// Title-block fields from a form: trimmed, with empty keys and values dropped.
/// `msg` with each warning appended.
fn with_warnings(mut msg: String, warnings: &[String]) -> String {
    for w in warnings {
        msg.push_str(&format!(". Warning: {w}"));
    }
    msg
}

fn title_block_of(fields: &[(String, String)]) -> BTreeMap<String, String> {
    fields
        .iter()
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .collect()
}

pub fn author_name() -> String {
    std::env::var("WIRINGCRAFT_USER")
        .ok()
        .filter(|a| !a.trim().is_empty())
        .or_else(|| {
            std::process::Command::new("git")
                .args(["config", "user.name"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                .filter(|a| !a.is_empty())
        })
        .or_else(|| std::env::var("USER").ok())
        .or_else(|| std::env::var("USERNAME").ok())
        .unwrap_or_else(|| "someone".into())
}

/// Seconds since 1970 (UTC).
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
        .unwrap_or(0)
}

/// Wire ends on pins of symbol `id` that `symbol` does not have: `page 3 -K1:13` each
/// ([`wc_core::Project::dangling_pins`]).
pub fn dangling_pins(project: &wc_core::Project, id: &str, symbol: &wc_core::Symbol) -> Vec<String> {
    project.dangling_pins(id, symbol)
}

/// A file's bytes before it is changed, or `None` when there is no such file. Any other
/// read error is returned: a file that is there but cannot be read must not be taken for absent
/// (and then removed when putting things back).
/// A vendored file a library update may write, as it was before: the item it holds and its
/// bytes (`None` when there was no file).
struct KeptFile {
    kind: ItemKind,
    id: String,
    path: PathBuf,
    was: Option<Vec<u8>>,
}

/// A part file Undo or Redo wrote ([`Open::sync_parts`]): what it held before, and the part
/// loaded before, if any, so a step that cannot be finished puts both back.
struct PartWrite {
    id: String,
    path: PathBuf,
    was: Option<Vec<u8>>,
    loaded: Option<Part>,
}

/// `err`, naming the part files that could not be put back after it.
fn with_unrestored(mut err: String, unrestored: &[String]) -> String {
    if !unrestored.is_empty() {
        err.push_str(&format!(
            "; these part files now hold the other version and could not be put back: {}. The pinned version stays loaded and the next save writes them first.",
            unrestored.join(", ")
        ));
    }
    err
}

/// A vendored file a failed library update, Undo or Redo could not put back
/// ([`Open::unrestored_files`]): its item, what it held, and the version the lock pinned then.
#[derive(Clone)]
struct UnrestoredFile {
    kind: ItemKind,
    id: String,
    was: Option<Vec<u8>>,
    pin: Option<String>,
}

fn kind_name(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Symbol => "symbol",
        ItemKind::Part => "part",
    }
}

/// What a project file waiting to be written again held when WiringCraft left it
/// ([`Open::keep_unwritten`]), or when the user chose to write over it.
#[derive(Clone, Debug)]
enum Found {
    /// Its bytes (`None`: no file there).
    Bytes(Option<Vec<u8>>),
    /// It could not be read (why): any file there that is not what the project needs counts
    /// as changed outside.
    Unread(String),
    /// It could not be read when the user chose to write over it (its [`Stamp`] then): the
    /// save may write over it while it stays that entry; one changed since is asked about
    /// again.
    WriteOverUnread(Option<Stamp>),
}

/// What tells one version of an unreadable entry from another: its kind, size and times.
type Stamp = (bool, u64, Option<std::time::SystemTime>, Option<std::time::SystemTime>);

fn stamp_of(path: &Path) -> Option<Stamp> {
    let m = std::fs::symlink_metadata(path).ok()?;
    Some((m.is_dir(), m.len(), m.modified().ok(), m.created().ok()))
}

impl Found {
    fn of(snapshot: Result<Option<Vec<u8>>, String>) -> Self {
        snapshot.map_or_else(Self::Unread, Self::Bytes)
    }

    /// Whether `now` (what the file at `path` holds, [`snapshot_file`]) was changed outside
    /// WiringCraft since it was found like this, with a note for the message (empty when there
    /// is nothing to add); `None` when the project's version (`should`) may go over it.
    fn change(&self, now: Result<Option<Vec<u8>>, String>, should: &Option<Vec<u8>>, path: &Path) -> Option<String> {
        match (self, now) {
            (Found::Bytes(found), Ok(now)) => (now != *found && now != *should).then(String::new),
            // Not known what WiringCraft left there: a file there now may be from outside.
            (Found::Unread(why), Ok(now)) => (now.is_some() && now != *should)
                .then(|| format!(" (it could not be read when WiringCraft left it: {why})")),
            (Found::WriteOverUnread(_), Ok(now)) => (now.is_some() && now != *should).then(String::new),
            // Not readable now: what it holds is not known, so it is not written over unless
            // the user chose to.
            (Found::Bytes(_) | Found::Unread(_), Err(why)) => Some(format!(" (it cannot be read now: {why})")),
            // Consent covers the unreadable entry the user saw only, told by its metadata.
            (Found::WriteOverUnread(seen), Err(why)) => {
                (stamp_of(path) != *seen).then(|| format!(" (it cannot be read now, and changed since: {why})"))
            }
        }
    }
}

/// How [`repair_file`] ended.
enum Repair {
    /// The file holds the project's version (or is gone, when it should be).
    Done,
    /// Another program changed the file just before the repair: it is left as that program
    /// left it, with a note for the message ([`Found::change`]).
    ChangedOutside(String),
}

/// Put the project's version (`should`; `None`: no file) at `path`, where a repair was waiting,
/// without losing a change another program makes at the same moment (#670).
///
/// A file there is first moved aside (renamed within its folder, one step), so no later write
/// can land in it unseen. What was moved is then checked against `found`
/// ([`Found::change`]): when it was changed outside, it is put back as it was, and the result
/// says so. Otherwise the project's version is put in place without replacing an entry that
/// appeared there meanwhile (that is reported as changed outside too). The moved file is let
/// go only when it still holds what was checked; one written again in between (through a
/// handle opened before the move) is kept next to it, and the result names where. Anything
/// else than a file or link at `path`, such as a folder, is repaired by [`restore_file`] as
/// before. An error when a step fails; the moved file is put back first, or the error says
/// where it is kept.
fn repair_file(path: &Path, should: Option<&[u8]>, found: &Found) -> std::io::Result<Repair> {
    #[cfg(test)]
    if tests::restore_fails(path) {
        return Err(std::io::Error::other("blocked by the test"));
    }
    let moved = match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() || m.is_symlink() => {
            // The name is reserved first (a new empty file, `create_new`), so the move can only
            // replace this call's own placeholder, never an entry another program made there.
            let aside = reserve_beside(path, "aside")?;
            if let Err(e) = std::fs::rename(path, &aside) {
                let _ = std::fs::remove_file(&aside);
                return Err(e);
            }
            Some(aside)
        }
        Ok(_) => return restore_file(path, should).map(|()| Repair::Done),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let now = match &moved {
        Some(aside) => snapshot_file(aside),
        None => Ok(None),
    };
    let stamp = moved.as_deref().and_then(stamp_of);
    #[cfg(test)]
    if let Some(aside) = &moved {
        tests::write_moved_after_check(aside);
    }
    #[cfg(test)]
    tests::run_hook("moved aside", path);
    let put_back = |aside: &Path| match place_new(aside, path) {
        Ok(true) => Ok(()),
        Ok(false) => Err(std::io::Error::other(format!(
            "another file appeared there meanwhile; the one that was there is kept as {}",
            aside.display()
        ))),
        Err(e) => Err(std::io::Error::other(format!(
            "{e}; the file that was there is kept as {}",
            aside.display()
        ))),
    };
    // The moved file is removed only while it holds what was checked: read again just before.
    let let_go = |aside: &Path| -> std::io::Result<Option<String>> {
        let same = match (&now, snapshot_file(aside)) {
            (Ok(was), Ok(is)) => *was == is && stamp_of(aside) == stamp,
            (Err(_), _) | (_, Err(_)) => stamp_of(aside) == stamp,
        };
        if !same {
            return Ok(Some(format!(
                " (it was written again while it was repaired; that version is kept as {})",
                aside.display()
            )));
        }
        std::fs::remove_file(aside).map_err(|e| {
            std::io::Error::other(format!(
                "the project's version is in place, but the file that was there could not be removed from {}: {e}",
                aside.display()
            ))
        })?;
        Ok(None)
    };
    let stamp_at = moved.as_deref().unwrap_or(path);
    if let Some(note) = found.change(now.clone(), &should.map(<[u8]>::to_vec), stamp_at) {
        if let Some(aside) = &moved {
            put_back(aside)?;
        }
        return Ok(Repair::ChangedOutside(note));
    }
    let placed = match should {
        // Already the project's version: the moved file itself goes back.
        Some(bytes) if now.as_ref().is_ok_and(|n| n.as_deref() == Some(bytes)) => match &moved {
            Some(aside) => place_new(aside, path),
            None => Ok(true),
        },
        Some(bytes) => write_staging_beside(path, bytes).and_then(|new| {
            let placed = place_new(&new, path);
            if std::fs::symlink_metadata(&new).is_ok() {
                let _ = std::fs::remove_file(&new);
            }
            placed
        }),
        None => match std::fs::symlink_metadata(path) {
            Ok(_) => Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(e) => Err(e),
        },
    };
    match placed {
        Ok(placed) => {
            let kept = match &moved {
                Some(aside) if std::fs::symlink_metadata(aside).is_ok() => let_go(aside)?,
                _ => None,
            };
            Ok(match (placed, kept) {
                (_, Some(note)) => Repair::ChangedOutside(note),
                (true, None) => Repair::Done,
                // An entry appeared where the project's version was to go: another program's.
                (false, None) => Repair::ChangedOutside(" (it appeared again while it was being written)".into()),
            })
        }
        Err(e) => {
            if let Some(aside) = &moved {
                put_back(aside).map_err(|again| std::io::Error::other(format!("{e}; then {again}")))?;
            }
            Err(e)
        }
    }
}

/// Write `bytes` to a new file next to `path` ([`unused_beside`]), created here and only here
/// (`create_new`: never through a link or into a file someone else put at that name).
fn write_staging_beside(path: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    for _ in 0..10 {
        let new = unused_beside(path, "new")?;
        #[cfg(test)]
        tests::run_hook("staging name chosen", &new);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&new) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(bytes).and_then(|()| file.sync_all()) {
                    drop(file);
                    let _ = std::fs::remove_file(&new);
                    return Err(e);
                }
                return Ok(new);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free name next to it"))
}

/// A new empty file next to `path` ([`unused_beside`]), created here (`create_new`), that holds
/// the name for a file of a repair (`what`) until it is renamed over.
fn reserve_beside(path: &Path, what: &str) -> std::io::Result<PathBuf> {
    for _ in 0..10 {
        let name = unused_beside(path, what)?;
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&name) {
            Ok(_) => return Ok(name),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free name next to it"))
}

/// A path next to `path` that nothing is at yet, for a file of a repair (`what`).
fn unused_beside(path: &Path, what: &str) -> std::io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| std::io::Error::other("no file name"))?;
    for n in 0..1000u32 {
        let mut candidate = std::ffi::OsString::from(".");
        candidate.push(name);
        candidate.push(format!(".wc-{what}-{}-{n}", std::process::id()));
        let candidate = path.with_file_name(candidate);
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free name next to it"))
}

/// Move the file at `from` to `to` without replacing anything at `to`: `false` (and `from`
/// left as it is) when something is there. Done by a hard link, then removing `from`; where
/// the file system has no hard links, by a rename once nothing is seen at `to`.
fn place_new(from: &Path, to: &Path) -> std::io::Result<bool> {
    match std::fs::hard_link(from, to) {
        Ok(()) => {
            std::fs::remove_file(from)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::Unsupported || e.kind() == std::io::ErrorKind::PermissionDenied => {
            if std::fs::symlink_metadata(to).is_ok() {
                return Ok(false);
            }
            std::fs::rename(from, to)?;
            Ok(true)
        }
        Err(e) => Err(e),
    }
}

fn snapshot_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{} cannot be read: {e}", path.display())),
    }
}

/// Put a file back as [`snapshot_file`] found it: untouched when it already holds those bytes,
/// otherwise replaced whole ([`wc_core::store::replace_whole`]: written next to it, never
/// through a link there, then renamed over it), so a failed write leaves the file as it was
/// rather than cut short.
fn restore_file(path: &Path, was: Option<&[u8]>) -> std::io::Result<()> {
    #[cfg(test)]
    if tests::restore_fails(path) {
        return Err(std::io::Error::other("blocked by the test"));
    }
    match was {
        Some(bytes) => {
            if std::fs::read(path).is_ok_and(|now| now == bytes) {
                return Ok(());
            }
            wc_core::store::replace_whole(path, bytes).map_err(std::io::Error::other)
        }
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        },
    }
}

/// Copy a folder and everything in it. Links are not followed: a link is left out, so a copy
/// never reaches outside the folder. Each link left out is added to `links`, as its path in
/// the folder copied (`at` is where `from` is in it).
fn copy_dir(from: &Path, to: &Path, at: &Path, links: &mut Vec<PathBuf>) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &dest, &at.join(entry.file_name()), links)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &dest)?;
        } else {
            links.push(at.join(entry.file_name()));
        }
    }
    Ok(())
}

/// A path's parts with `.` dropped and `..` taken back where it can be (lexically).
fn lexical_parts(p: &Path) -> Vec<std::path::Component<'_>> {
    use std::path::Component;
    let mut out: Vec<Component<'_>> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir if matches!(out.last(), Some(Component::Normal(_))) => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// Whether a git config file sets `core.worktree` (a work tree elsewhere than its folder).
fn git_worktree_set(config: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(config) else {
        return false;
    };
    let mut in_core = false;
    for line in text.lines().map(str::trim) {
        // A section header: `[core]`, maybe followed by a comment (which may hold quotes) or by
        // a key on the same line. `[core "x"]` is another section.
        let key_line = match line.strip_prefix('[') {
            Some(header) => {
                let (name, rest) = header.split_once(']').unwrap_or((header, ""));
                in_core = name.trim().eq_ignore_ascii_case("core");
                rest.trim()
            }
            None => line,
        };
        if in_core && !key_line.starts_with(['#', ';']) {
            let key = key_line.split(['=', '#', ';']).next().unwrap_or_default().trim();
            if key.eq_ignore_ascii_case("worktree") {
                return true;
            }
        }
    }
    false
}

/// A full path as the file system walks it, one part at a time: each part that exists is
/// resolved (links and letter case), a `..` goes up from the folder reached so far, and a
/// part that does not exist yet is taken as written. `/w/link/../copy` with `link` pointing at
/// `/w/a/b` is `/w/a/copy`; so is `/w/missing/../link/../copy`. A file with more parts after
/// it (`/w/document/../copy`), or a part that cannot be looked up, is refused with the reason.
fn destination(p: &Path) -> Result<PathBuf, String> {
    use std::path::Component;
    let parts: Vec<Component<'_>> = p.components().collect();
    let mut out = PathBuf::new();
    for (k, c) in parts.iter().enumerate() {
        match c {
            Component::Prefix(_) | Component::RootDir => out.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => {
                let next = out.join(name);
                match std::fs::canonicalize(&next) {
                    Ok(real) => {
                        let more = parts[k + 1..].iter().any(|c| !matches!(c, Component::CurDir));
                        if more && !real.is_dir() {
                            return Err(format!("{} is a file, not a folder.", next.display()));
                        }
                        out = without_verbatim(real);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => out = next,
                    Err(e) => return Err(format!("{} cannot be looked up: {e}", next.display())),
                }
            }
        }
    }
    Ok(out)
}

/// A resolved Windows path in its usual form (`C:\x`, not `\\?\C:\x`); others as they are.
fn without_verbatim(p: PathBuf) -> PathBuf {
    use std::path::{Component, Prefix};
    let mut parts = p.components();
    if let Some(Component::Prefix(pre)) = parts.next()
        && let Prefix::VerbatimDisk(d) = pre.kind()
    {
        return PathBuf::from(format!("{}:", char::from(d))).join(parts.as_path());
    }
    p
}

/// `path` as seen from the folder `base`, both full paths: `../library` and the like. `None`
/// when they share no root (another drive).
fn relative_to(path: &Path, base: &Path) -> Option<PathBuf> {
    let (p, b) = (lexical_parts(path), lexical_parts(base));
    let common = p.iter().zip(&b).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return None;
    }
    let mut out = PathBuf::new();
    for _ in common..b.len() {
        out.push("..");
    }
    for c in p.iter().skip(common) {
        out.push(c.as_os_str());
    }
    Some(if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    })
}

/// Whether a library item read for `id` is the version pinned as `pin` (its hash), or why
/// not: a file that holds another item than its name says, or another version.
fn version_fits(item_id: &str, hash: &str, id: &str, pin: &str) -> Result<(), String> {
    if item_id != id {
        Err(format!("its file holds {item_id}"))
    } else if hash != pin {
        Err("another version".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;

    /// A part copy that cannot be looked for (say, in a folder that may not be entered) is
    /// reported as unreadable with the reason, not as missing (#1035). The failing lookup is
    /// injected, so this holds when the tests run as root too.
    #[test]
    fn a_part_copy_that_cannot_be_looked_for_is_unreadable_with_the_reason() {
        let (_tmp, mut o) = project();
        o.project.devices.insert(
            DeviceTag::parse("-K1").unwrap(),
            wc_core::project::Device {
                part: Some("maker.contactor".into()),
                ..Default::default()
            },
        );
        assert!(
            unreadable_parts(&o.dir, &o.project, &BTreeMap::new()).is_empty(),
            "absent is missing"
        );
        let found = unreadable_parts_looked_up(&o.dir, &o.project, &BTreeMap::new(), |_| {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        });
        let why = found.get("maker.contactor").map(String::as_str).unwrap_or_default();
        assert!(why.contains("maker.contactor.json"), "names the file: {found:?}");
        assert!(why.to_lowercase().contains("permission denied"), "and why: {found:?}");
    }

    /// A used part id that cannot name a file in the project's library is reported as
    /// unreadable with the reason the id is refused, not skipped; a valid id whose copy is
    /// absent stays missing (not listed here).
    #[test]
    fn a_used_part_id_that_cannot_name_a_file_is_unreadable_with_the_reason() {
        let (_tmp, mut o) = project();
        for (tag, part) in [("-K1", "../escape"), ("-K2", "maker.absent")] {
            o.project.devices.insert(
                DeviceTag::parse(tag).unwrap(),
                wc_core::project::Device {
                    part: Some(part.into()),
                    ..Default::default()
                },
            );
        }
        let found = unreadable_parts(&o.dir, &o.project, &BTreeMap::new());
        let why = found.get("../escape").map(String::as_str).unwrap_or_default();
        assert!(why.contains("must be 1-128"), "keeps the reason: {found:?}");
        assert!(!found.contains_key("maker.absent"), "absent is missing: {found:?}");
        assert_eq!(found.len(), 1, "{found:?}");
    }

    thread_local! {
        /// A file whose writes by `restore_file` fail, after letting this many through.
        static FAIL_RESTORE: std::cell::RefCell<Vec<(PathBuf, usize)>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    thread_local! {
        /// When `restore_file` is asked to write the first file, the second is given this
        /// text first, once: another program's write in the middle of a step.
        static OUTSIDE_WRITE: std::cell::RefCell<Vec<(PathBuf, PathBuf, String)>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Have `target` written with `text`, as by another program, just before `restore_file`
    /// next writes `at` (in this test's thread).
    fn write_outside_before(at: &Path, target: &Path, text: &str) {
        OUTSIDE_WRITE.with(|w| {
            w.borrow_mut()
                .push((at.to_path_buf(), target.to_path_buf(), text.to_owned()));
        });
    }

    /// Make `restore_file` fail on `path` (in this test's thread) after `pass` writes of it.
    fn fail_restores(path: &Path, pass: usize) {
        FAIL_RESTORE.with(|f| f.borrow_mut().push((path.to_path_buf(), pass)));
    }

    fn let_restores_through() {
        FAIL_RESTORE.with(|f| f.borrow_mut().clear());
    }

    thread_local! {
        /// Text written into the next file a repair moves aside, after it was checked: a
        /// program writing through a handle it opened before the move.
        static WRITE_MOVED: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    }

    type Hook = Box<dyn FnOnce(&Path)>;

    thread_local! {
        /// Steps run once at a named point of a repair (in this test's thread): another
        /// program acting at that moment.
        static HOOKS: std::cell::RefCell<Vec<(&'static str, Hook)>> = std::cell::RefCell::new(Vec::new());
    }

    #[cfg(unix)]
    fn at(point: &'static str, step: impl FnOnce(&Path) + 'static) {
        HOOKS.with(|h| h.borrow_mut().push((point, Box::new(step))));
    }

    pub(super) fn run_hook(point: &str, path: &Path) {
        let step = HOOKS.with(|h| {
            let mut h = h.borrow_mut();
            let i = h.iter().position(|(p, _)| *p == point)?;
            Some(h.remove(i).1)
        });
        if let Some(step) = step {
            step(path);
        }
    }

    pub(super) fn write_moved_after_check(aside: &Path) {
        if let Some(text) = WRITE_MOVED.with(|w| w.borrow_mut().take()) {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(aside).unwrap();
            f.write_all(text.as_bytes()).unwrap();
        }
    }

    pub(super) fn restore_fails(path: &Path) -> bool {
        let outside: Vec<(PathBuf, String)> = OUTSIDE_WRITE.with(|w| {
            let mut w = w.borrow_mut();
            let (now, later) = std::mem::take(&mut *w).into_iter().partition(|(at, _, _)| at == path);
            *w = later;
            let now: Vec<(PathBuf, PathBuf, String)> = now;
            now.into_iter().map(|(_, target, text)| (target, text)).collect()
        });
        for (target, text) in outside {
            // A test that cannot write its own file fails on what it reads back.
            let _ = std::fs::write(target, text);
        }
        FAIL_RESTORE.with(|f| {
            f.borrow_mut()
                .iter_mut()
                .find(|(p, _)| p == path)
                .is_some_and(|(_, pass)| match pass.checked_sub(1) {
                    Some(left) => {
                        *pass = left;
                        false
                    }
                    None => true,
                })
        })
    }

    /// A project with a macro saved twice from its form (descriptions "A coil", then "Two"),
    /// and the file's text after each save.
    #[test]
    fn a_failed_undo_or_redo_keeps_the_steps_mark_for_taking_a_move_back() {
        use wc_core::geom::Shape;
        let line = |x: f64| Shape::Line {
            a: Point::new(x, 100.0),
            b: Point::new(x + 10.0, 100.0),
        };
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let Some(g) = o.add_graphic(1, line(10.0)) else {
            unreachable!()
        };
        let page = o.project.pages[1].id.clone();
        // A move's checkpoint, then an assistant turn that deletes the line and draws another:
        // the same count of drawings, folded into one step.
        o.begin_drag_move();
        let Some(id) = o.last_step_id() else { unreachable!() };
        o.drag_move_by(1, std::slice::from_ref(&g), Point::new(-40.0, 0.0));
        let mark = o.start_turn();
        o.delete_many(1, std::slice::from_ref(&g));
        o.add_graphic(1, line(200.0));
        o.squash_undo(mark);
        let symbol = o.project.lock.symbols.keys().next().unwrap().clone();
        let fail = |o: &mut Open| {
            o.stale_symbol_files.insert(symbol.clone());
            o.fail_symbol_writes = [true].into();
        };
        let pass = |o: &mut Open| {
            o.stale_symbol_files.clear();
            o.fail_symbol_writes = Default::default();
        };
        // A failed Undo puts the folded step back as it was.
        fail(&mut o);
        assert!(!o.undo());
        assert!(o.take_undo_error().is_some());
        pass(&mut o);
        assert!(o.take_back_move(id, &page, std::slice::from_ref(&g)).is_err());
        // The same for a failed Redo, with the folded step waiting in the redo history.
        assert!(o.undo(), "{:?}", o.take_undo_error());
        fail(&mut o);
        assert!(!o.redo());
        assert!(o.take_undo_error().is_some());
        pass(&mut o);
        assert!(o.take_back_move(id, &page, std::slice::from_ref(&g)).is_err());
    }

    #[test]
    fn a_move_is_not_taken_back_under_an_assistant_turn_begun_after_it() {
        let (_tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let sel = [Selection::Symbol(id)];
        let page = o.project.pages[1].id.clone();
        o.begin_drag_move();
        let Some(step) = o.last_step_id() else { unreachable!() };
        o.drag_move_by(1, &sel, Point::new(-10.0, 0.0));
        let mark = o.start_turn();
        o.rotate_many(1, &sel);
        o.rotate_many(1, &sel);
        assert!(o.take_back_move(step, &page, &sel).is_err());
        let steps = o.undo_steps();
        o.squash_undo(mark);
        assert_eq!(o.undo_steps(), steps - 1, "the turn's two steps fold into one");
    }

    fn macro_edited_twice() -> (tempfile::TempDir, Open, PathBuf, [String; 3]) {
        let (tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let clip = o.copy(1, &[Selection::Symbol(id)]);
        let path = o.save_macro(&clip, "Coil", "").unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        let mut texts = vec![first];
        for d in ["A coil", "Two"] {
            let mut form = crate::macro_form::Form::open(&path).unwrap();
            form.description = d.into();
            o.save_macro_form(&form).unwrap();
            texts.push(std::fs::read_to_string(&path).unwrap());
        }
        let Ok(texts) = <[String; 3]>::try_from(texts) else {
            unreachable!("three texts")
        };
        (tmp, o, path, texts)
    }

    #[test]
    fn a_macro_a_change_taken_back_cannot_write_is_written_by_the_next_save() {
        let (_tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let clip = o.copy(1, &[Selection::Symbol(id)]);
        let path = o.save_macro(&clip, "Coil", "").unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        o.save().unwrap();
        let m = o.mark();
        for d in ["Gone", "Gone again"] {
            let mut form = crate::macro_form::Form::open(&path).unwrap();
            form.description = d.into();
            o.save_macro_form(&form).unwrap();
        }
        let refused = std::fs::read_to_string(&path).unwrap();
        fail_restores(&path, 0);
        let err = o.back_to(m).unwrap_err();
        assert!(err.contains("written again on the next Save"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), refused, "still blocked");
        assert!(o.dirty, "Save is offered");

        // Save refuses while the file cannot be written, then writes it once it can.
        let err = o.save().unwrap_err();
        assert!(err.starts_with("not saved"), "{err}");
        let_restores_through();
        o.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first, "as before the turn");
        assert!(o.unwritten_files.is_empty());
    }

    #[test]
    fn a_macro_undo_cannot_put_back_is_written_by_the_next_save_and_a_save_on_purpose_wins() {
        // Undo writes the macro, then a symbol file fails, and the macro cannot be put back.
        let (_tmp, mut o, path, [_, one, two]) = macro_edited_twice();
        let symbol = o.project.lock.symbols.keys().next().unwrap().clone();
        o.stale_symbol_files.insert(symbol.clone());
        o.fail_symbol_writes = [true].into();
        fail_restores(&path, 1);
        assert!(!o.undo());
        let err = o.take_undo_error().unwrap_or_default();
        assert!(err.contains("not put back"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), one, "left as Undo wrote it");
        let_restores_through();
        o.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), two, "as the project still is");

        // The same with Redo, and then the macro is saved on purpose: that is kept.
        o.stale_symbol_files.clear();
        o.fail_symbol_writes = Default::default();
        assert!(o.undo());
        o.stale_symbol_files.insert(symbol);
        o.fail_symbol_writes = [true].into();
        fail_restores(&path, 1);
        assert!(!o.redo());
        assert!(o.take_undo_error().is_some());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), two, "left as Redo wrote it");
        let_restores_through();
        assert_eq!(o.unwritten_files.len(), 1);
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.description = "Mine".into();
        o.save_macro_form(&form).unwrap();
        assert!(o.unwritten_files.is_empty());
        let mine = std::fs::read_to_string(&path).unwrap();
        o.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), mine);
    }

    /// A macro waiting to be written again after a failed Undo, then changed outside the app:
    /// Save does not write over it and names it; keeping the outside version lets the save go
    /// through with that version, and writing the project's version over it is a choice too
    /// (#662).
    #[test]
    fn a_save_does_not_write_over_a_waiting_macro_changed_outside() {
        let (_tmp, mut o, path, [_, one, two]) = macro_edited_twice();
        let symbol = o.project.lock.symbols.keys().next().unwrap().clone();
        let fail_undo = |o: &mut Open| {
            o.stale_symbol_files.insert(symbol.clone());
            o.fail_symbol_writes = [true].into();
            fail_restores(&path, 1);
            assert!(!o.undo());
            assert!(o.take_undo_error().is_some());
            let_restores_through();
            o.stale_symbol_files.clear();
            o.fail_symbol_writes = Default::default();
        };
        fail_undo(&mut o);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), one, "left as Undo wrote it");
        assert_eq!(o.changed_outside(), None, "as WiringCraft left it");

        // Changed outside: not written over, and the message names the file.
        let outside = format!("{two}\n# edited outside\n");
        std::fs::write(&path, &outside).unwrap();
        assert_eq!(o.changed_outside(), Some(path.clone()));
        let err = o.save().unwrap_err();
        assert!(err.contains(&path.display().to_string()), "{err}");
        assert!(err.contains("changed outside"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), outside);

        // Keep the outside version: the save goes through and leaves it.
        o.keep_outside_version(&path);
        assert_eq!(o.changed_outside(), None);
        o.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), outside);

        // Or write the project's version over it.
        std::fs::write(&path, &two).unwrap();
        fail_undo(&mut o);
        std::fs::write(&path, &outside).unwrap();
        assert!(o.save().is_err());
        o.write_over_outside_version(&path);
        o.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), two, "as the project needs it");
    }

    /// Another program writes a waiting macro after Save's check found it as WiringCraft left
    /// it, just before the repair: the edit is not lost, and Save names the file (#670).
    #[test]
    fn a_save_does_not_lose_a_waiting_file_changed_right_after_its_check() {
        let (_tmp, mut o, path, [_, _, two]) = macro_edited_twice();
        let symbol = o.project.lock.symbols.keys().next().unwrap().clone();
        o.stale_symbol_files.insert(symbol);
        o.fail_symbol_writes = [true].into();
        fail_restores(&path, 1);
        assert!(!o.undo());
        assert!(o.take_undo_error().is_some());
        let_restores_through();
        o.stale_symbol_files.clear();
        o.fail_symbol_writes = Default::default();
        assert_eq!(o.changed_outside(), None, "as WiringCraft left it");

        let outside = format!("{two}\n# edited between the check and the repair\n");
        write_outside_before(&path, &path, &outside);
        let err = o.save().unwrap_err();
        assert!(err.contains(&path.display().to_string()), "{err}");
        assert!(err.contains(Open::CHANGED_OUTSIDE), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), outside, "the edit survives");
        assert_eq!(o.changed_outside(), Some(path.clone()), "still asked about");
        let dir = path.parent().unwrap();
        let left: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".wc-"))
            .collect();
        assert!(left.is_empty(), "nothing left beside it: {left:?}");

        // Keeping it lets the save through with that version.
        o.keep_outside_version(&path);
        o.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), outside);
    }

    /// When the project's version cannot be staged (every staging name taken), the file that
    /// was moved aside goes back where it was, untouched (#670).
    #[test]
    fn a_repair_that_cannot_stage_puts_the_file_back() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("m.yaml");
        std::fs::write(&path, "left").unwrap();
        for n in 0..1000u32 {
            let name = format!(".m.yaml.wc-new-{}-{n}", std::process::id());
            std::fs::write(tmp.path().join(name), "").unwrap();
        }
        let left = Found::Bytes(Some(b"left".to_vec()));
        let err = repair_file(&path, Some(b"new"), &left).err().unwrap();
        assert!(err.to_string().contains("no free name"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), b"left", "put back");
        let aside = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().contains("wc-aside"))
            .count();
        assert_eq!(aside, 0, "nothing left aside");
    }

    /// A link another program puts at the staging name right after it was chosen is not
    /// written through: the file it points at stays as it was (#670).
    #[cfg(unix)]
    #[test]
    fn a_link_put_at_the_staging_name_is_not_written_through() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("m.yaml");
        std::fs::write(&path, "left").unwrap();
        let target = tmp.path().join("outside.txt");
        std::fs::write(&target, "theirs").unwrap();
        let t = target.clone();
        at("staging name chosen", move |new| {
            std::os::unix::fs::symlink(&t, new).unwrap()
        });
        let left = Found::Bytes(Some(b"left".to_vec()));
        assert!(matches!(repair_file(&path, Some(b"new"), &left).unwrap(), Repair::Done));
        assert_eq!(std::fs::read(&target).unwrap(), b"theirs", "not written through");
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    /// A dangling link another program puts where a file is to be deleted, after it was moved
    /// aside, counts as changed outside: it is left there and the repair keeps waiting (#670).
    #[cfg(unix)]
    #[test]
    fn a_dangling_link_put_where_a_deleted_file_was_is_reported() {
        let (_tmp, mut o, path, _) = macro_edited_twice();
        let held = std::fs::read(&path).unwrap();
        let rel = path.strip_prefix(&o.dir).unwrap().to_path_buf();
        o.unwritten_files.insert(rel.clone(), None);
        o.unwritten_found.insert(rel.clone(), Found::Bytes(Some(held)));
        let nowhere = path.with_file_name("nowhere");
        at("moved aside", move |p| std::os::unix::fs::symlink(&nowhere, p).unwrap());
        let err = o.save().unwrap_err();
        assert!(err.contains(Open::CHANGED_OUTSIDE), "{err}");
        assert!(std::fs::symlink_metadata(&path).unwrap().is_symlink(), "left there");
        assert!(o.unwritten_files.contains_key(&rel), "still waiting");
    }

    /// A reserved name is a file made by the reservation itself, so a second reservation (or
    /// another program) never gets the same one (#670).
    #[test]
    fn a_reserved_repair_name_is_held_by_a_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("m.yaml");
        let first = reserve_beside(&path, "aside").unwrap();
        assert!(first.is_file());
        let second = reserve_beside(&path, "aside").unwrap();
        assert_ne!(first, second);
        assert!(second.is_file());
    }

    /// The repair itself: a waiting deletion or replacement is done when the file holds what
    /// the check admitted, and a file changed outside is put back untouched (#670).
    #[test]
    fn a_repair_moves_the_file_aside_and_checks_it_first() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("m.yaml");
        let left = Found::Bytes(Some(b"left".to_vec()));
        // Replaced when it holds what WiringCraft left.
        std::fs::write(&path, "left").unwrap();
        assert!(matches!(repair_file(&path, Some(b"new"), &left).unwrap(), Repair::Done));
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        // Deleted when it should be gone.
        std::fs::write(&path, "left").unwrap();
        assert!(matches!(repair_file(&path, None, &left).unwrap(), Repair::Done));
        assert!(!path.exists());
        // Written where nothing was left, and nothing is.
        assert!(matches!(
            repair_file(&path, Some(b"new"), &Found::Bytes(None)).unwrap(),
            Repair::Done
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        // Changed outside: neither deleted nor replaced.
        std::fs::write(&path, "theirs").unwrap();
        for should in [None, Some(&b"new"[..])] {
            let Repair::ChangedOutside(_) = repair_file(&path, should, &left).unwrap() else {
                unreachable!("changed outside")
            };
            assert_eq!(std::fs::read(&path).unwrap(), b"theirs");
        }
        assert_eq!(
            std::fs::read_dir(tmp.path()).unwrap().count(),
            1,
            "nothing left beside it"
        );
        // Written again through an old handle after the check: that version is kept beside it
        // and named, not let go.
        std::fs::write(&path, "left").unwrap();
        WRITE_MOVED.with(|w| *w.borrow_mut() = Some("+late".into()));
        let Repair::ChangedOutside(note) = repair_file(&path, Some(b"new"), &left).unwrap() else {
            unreachable!("written again")
        };
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let kept: Vec<PathBuf> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| *p != path)
            .collect();
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(std::fs::read(&kept[0]).unwrap(), b"left+late");
        assert!(note.contains(&kept[0].display().to_string()), "{note}");
    }

    /// A repair noted while the file could not be read (a folder in its place): a macro file
    /// put there later by another program is not written over unasked, and the message says
    /// why the file was not known (#662).
    #[test]
    fn a_save_does_not_write_over_a_file_put_where_an_unreadable_one_was() {
        let (_tmp, mut o, path, [_, _, two]) = macro_edited_twice();
        let held = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        o.keep_unwritten(vec![(path.clone(), Some(two.clone().into_bytes()))], true);
        std::fs::remove_dir(&path).unwrap();
        let outside = format!("{two}\n# put here by another program\n");
        std::fs::write(&path, &outside).unwrap();
        assert_eq!(o.changed_outside(), Some(path.clone()));
        let err = o.save().unwrap_err();
        assert!(
            err.contains(Open::CHANGED_OUTSIDE) && err.contains("could not be read"),
            "{err}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), outside);
        // The project's version is the same as what it held: nothing to ask.
        std::fs::write(&path, &held).unwrap();
        assert_eq!(o.changed_outside(), None);
        o.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), two);
    }

    /// Writing the project's version over one file changed outside covers that version only:
    /// an edit made to it afterwards, while another file holds the save up, is asked about
    /// again and not written over (#662).
    #[test]
    fn consent_to_write_over_covers_only_the_version_it_was_given_for() {
        let (_tmp, mut o) = project();
        let [a, b] = ["a.toml", "b.toml"].map(|n| o.dir.join(n));
        std::fs::write(&a, "a0").unwrap();
        std::fs::write(&b, "b0").unwrap();
        o.keep_unwritten(
            vec![
                (a.clone(), Some(b"a-project".to_vec())),
                (b.clone(), Some(b"b-project".to_vec())),
            ],
            true,
        );
        std::fs::write(&a, "a1").unwrap();
        std::fs::write(&b, "b1").unwrap();
        assert_eq!(o.changed_outside(), Some(a.clone()));
        o.write_over_outside_version(&a);
        // B still holds the save up; then A is edited outside again.
        assert_eq!(o.changed_outside(), Some(b.clone()));
        assert!(o.save().is_err());
        std::fs::write(&a, "a2").unwrap();
        o.keep_outside_version(&b);
        assert_eq!(o.changed_outside(), Some(a.clone()), "the new edit is asked about");
        assert!(o.save().unwrap_err().contains(Open::CHANGED_OUTSIDE));
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a2");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b1");
        o.write_over_outside_version(&a);
        o.save().unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a-project");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b1");
    }

    /// A waiting file that cannot be read any more (here a folder put in its place) is not
    /// written over unasked: Save refuses, names it with the read error, and leaves it.
    #[test]
    fn a_save_does_not_write_over_a_waiting_file_that_cannot_be_read_now() {
        let (_tmp, mut o) = project();
        let a = o.dir.join("a.toml");
        std::fs::write(&a, "a0").unwrap();
        o.keep_unwritten(vec![(a.clone(), Some(b"a-project".to_vec()))], true);
        std::fs::remove_file(&a).unwrap();
        std::fs::create_dir(&a).unwrap();
        std::fs::write(a.join("inside"), "kept").unwrap();
        let err = o.save().unwrap_err();
        assert!(
            err.contains(Open::CHANGED_OUTSIDE) && err.contains("cannot be read now"),
            "{err}"
        );
        assert_eq!(std::fs::read_to_string(a.join("inside")).unwrap(), "kept");
    }

    #[test]
    fn a_save_does_not_write_over_a_waiting_file_unreadable_then_and_now_unasked() {
        let (_tmp, mut o) = project();
        let a = o.dir.join("a.toml");
        // Unreadable when WiringCraft left it (a folder stands in for a file that cannot be
        // read: the tests run as root) and still unreadable at the save.
        std::fs::create_dir(&a).unwrap();
        std::fs::write(a.join("inside"), "kept").unwrap();
        o.keep_unwritten(vec![(a.clone(), Some(b"a-project".to_vec()))], true);
        let err = o.save().unwrap_err();
        assert!(
            err.contains(Open::CHANGED_OUTSIDE) && err.contains("cannot be read now"),
            "{err}"
        );
        assert_eq!(std::fs::read_to_string(a.join("inside")).unwrap(), "kept");
        // Consent covers that unreadable entry only: one changed since is asked about again.
        o.write_over_outside_version(&a);
        assert_eq!(o.changed_outside(), None);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(a.join("more"), "added").unwrap();
        assert_eq!(o.changed_outside(), Some(a.clone()));
        assert!(o.save().unwrap_err().contains("changed since"));
        // Once the user chose to write over it, the save goes on and says why it fails.
        o.write_over_outside_version(&a);
        assert_eq!(o.changed_outside(), None);
        let err = o.save().unwrap_err();
        assert!(!err.contains(Open::CHANGED_OUTSIDE), "{err}");
    }

    #[test]
    fn failed_undos_in_a_row_keep_the_first_repair_until_one_goes_through() {
        let (_tmp, mut o, path, [_, one, two]) = macro_edited_twice();
        let symbol = o.project.lock.symbols.keys().next().unwrap().clone();
        let fail_sync = |o: &mut Open| {
            o.stale_symbol_files.insert(symbol.clone());
            o.fail_symbol_writes = [true].into();
        };
        // The macro is left as Undo wrote it ("one") and waits to be "two" again.
        fail_sync(&mut o);
        fail_restores(&path, 1);
        assert!(!o.undo());
        assert!(o.take_undo_error().is_some());
        let_restores_through();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), one);
        // Fails again, its put back too: its snapshot ("one") does not replace the repair.
        fail_sync(&mut o);
        fail_restores(&path, 1);
        assert!(!o.undo());
        assert!(o.take_undo_error().is_some());
        let_restores_through();
        // Fails again after writing the macro, and puts it back as found: the repair stays.
        fail_sync(&mut o);
        assert!(!o.undo());
        assert!(o.take_undo_error().is_some());
        assert_eq!(o.unwritten_files.values().next(), Some(&Some(two.clone().into_bytes())));
        o.stale_symbol_files.clear();
        o.fail_symbol_writes = Default::default();
        o.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), two);
    }

    #[test]
    fn a_macro_saved_while_it_waits_for_a_repair_is_taken_back_to_the_repair() {
        let (_tmp, mut o, path, [_, one, two]) = macro_edited_twice();
        let symbol = o.project.lock.symbols.keys().next().unwrap().clone();
        // A failed Undo leaves the macro as "one" while the project needs "two".
        o.stale_symbol_files.insert(symbol);
        o.fail_symbol_writes = [true].into();
        fail_restores(&path, 1);
        assert!(!o.undo());
        assert!(o.take_undo_error().is_some());
        let_restores_through();
        o.stale_symbol_files.clear();
        o.fail_symbol_writes = Default::default();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), one);
        // A turn saves the macro, then is taken back: the file goes back to "two".
        let m = o.mark();
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.description = "Turn".into();
        o.save_macro_form(&form).unwrap();
        assert!(o.unwritten_files.is_empty());
        o.back_to(m).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), two);
        // The same when the write back fails: "two" waits for the next Save.
        let m = o.mark();
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.description = "Turn again".into();
        o.save_macro_form(&form).unwrap();
        fail_restores(&path, 0);
        assert!(o.back_to(m).is_err());
        let_restores_through();
        o.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), two);
    }

    #[test]
    fn a_step_editing_one_macro_twice_is_put_back_once_as_it_was_before() {
        let (_tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let clip = o.copy(1, &[Selection::Symbol(id)]);
        let path = o.save_macro(&clip, "Coil", "").unwrap();
        let mark = o.undo_mark();
        for d in ["B", "C"] {
            let mut form = crate::macro_form::Form::open(&path).unwrap();
            form.description = d.into();
            o.save_macro_form(&form).unwrap();
        }
        o.squash_undo(mark);
        let c = std::fs::read_to_string(&path).unwrap();
        let symbol = o.project.lock.symbols.keys().next().unwrap().clone();
        o.stale_symbol_files.insert(symbol);
        o.fail_symbol_writes = [true].into();
        // Undo writes the file twice (to B, then A); putting it back to C fails.
        fail_restores(&path, 2);
        assert!(!o.undo());
        assert!(o.take_undo_error().is_some());
        let_restores_through();
        assert_eq!(o.unwritten_files.len(), 1);
        assert_eq!(o.unwritten_files.values().next(), Some(&Some(c.clone().into_bytes())));
        o.stale_symbol_files.clear();
        o.fail_symbol_writes = Default::default();
        o.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), c);
    }

    #[test]
    fn a_multi_file_undo_that_cannot_put_a_file_back_has_it_written_by_the_next_save() {
        let (_tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let clip = o.copy(1, &[Selection::Symbol(id)]);
        let a = o.save_macro(&clip, "Coil A", "").unwrap();
        let b = o.save_macro(&clip, "Coil B", "").unwrap();
        // Both macros edited in one undo step (an assistant's turn).
        let mark = o.undo_mark();
        for path in [&a, &b] {
            let mut form = crate::macro_form::Form::open(path).unwrap();
            form.description = "Edited".into();
            o.save_macro_form(&form).unwrap();
        }
        o.squash_undo(mark);
        let (a_now, b_now) = (
            std::fs::read_to_string(&a).unwrap(),
            std::fs::read_to_string(&b).unwrap(),
        );
        // Undo writes b first (newest first), then a fails, and b cannot be put back.
        fail_restores(&a, 0);
        fail_restores(&b, 1);
        assert!(!o.undo());
        let err = o.take_undo_error().unwrap_or_default();
        assert!(err.contains("written already and not put back"), "{err}");
        assert_ne!(std::fs::read_to_string(&b).unwrap(), b_now, "left as Undo wrote it");
        let err = o.save().unwrap_err();
        assert!(err.starts_with("not saved"), "{err}");
        let_restores_through();
        o.save().unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), a_now);
        assert_eq!(std::fs::read_to_string(&b).unwrap(), b_now);
    }

    #[test]
    fn a_path_is_made_relative_to_another_folder() {
        let r = |a: &str, b: &str| relative_to(Path::new(a), Path::new(b));
        assert_eq!(r("/w/lib", "/w/copy"), Some(PathBuf::from("../lib")));
        assert_eq!(r("/w/old/../lib", "/w/x/copy"), Some(PathBuf::from("../../lib")));
        assert_eq!(r("/w/copy/sub", "/w/copy"), Some(PathBuf::from("sub")));
    }

    #[test]
    fn save_as_copies_the_project_keeps_the_original_and_points_libraries_at_the_same_folder() {
        let (tmp, mut o) = project();
        let from = o.dir.clone();
        let before = std::fs::read(from.join(store::PROJECT_FILE)).unwrap();
        o.project.meta.libraries.push(wc_core::project::LibrarySource {
            name: "shared".into(),
            kind: "folder".into(),
            location: "../library".into(),
            reference: None,
        });
        // The project's own library, inside it, goes along with the copy.
        o.project.meta.libraries.push(wc_core::project::LibrarySource {
            name: "own".into(),
            kind: "folder".into(),
            location: "library".into(),
            reference: None,
        });
        // The same library written through the folder above: still the project's own.
        o.project.meta.libraries.push(wc_core::project::LibrarySource {
            name: "own, the long way".into(),
            kind: "folder".into(),
            location: "../p/library".into(),
            reference: None,
        });
        std::fs::create_dir_all(from.join("library").join("symbols")).unwrap();
        std::fs::write(from.join("library").join("symbols").join("own.json"), "{}").unwrap();
        o.checkpoint();
        o.project.meta.name = "Changed".into();
        o.dirty = true;
        let to = tmp.path().join("copies").join("plant 2");
        // The folder as the file system names it (a temp folder can be reached through a link
        // or a short name).
        assert_eq!(o.save_as(&to).unwrap(), (destination(&to).unwrap(), Vec::new()));
        assert_eq!(o.dir, destination(&to).unwrap());
        assert!(!o.dirty);
        // The original is as it was last saved; the copy has the change and its library is
        // the same folder seen from the new place.
        assert_eq!(std::fs::read(from.join(store::PROJECT_FILE)).unwrap(), before);
        let copy = Open::load(&to).unwrap();
        assert_eq!(copy.project.meta.name, "Changed");
        let lib = copy.project.meta.libraries.iter().find(|l| l.name == "shared").unwrap();
        assert_eq!(Path::new(&lib.location), Path::new("../../library"));
        let own = copy.project.meta.libraries.iter().find(|l| l.name == "own").unwrap();
        assert_eq!(own.location, "library");
        let long = copy
            .project
            .meta
            .libraries
            .iter()
            .find(|l| l.name == "own, the long way")
            .unwrap();
        assert_eq!(Path::new(&long.location), Path::new("library"));
        // The copy holds the library's files itself, where its libraries point.
        assert!(to.join(&long.location).join("symbols").join("own.json").is_file());
        assert_eq!(copy.project.pages.len(), o.project.pages.len());
        // Undo goes back to the name before, not to the library folders seen from the original.
        assert!(o.undo());
        let shared = o.project.meta.libraries.iter().find(|l| l.name == "shared").unwrap();
        assert_eq!(Path::new(&shared.location), Path::new("../../library"));
        // Not over a folder with something in it, and not inside the project itself.
        assert!(o.save_as(&from).unwrap_err().contains("already exists"));
        assert!(o.save_as(&to.join("inner")).unwrap_err().contains("inside the project"));
    }

    #[test]
    fn a_destination_is_resolved_as_the_file_system_walks_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = destination(tmp.path()).unwrap();
        std::fs::create_dir_all(root.join("a").join("b")).unwrap();
        // `..` in the part that does not exist yet goes up from the folder reached so far.
        assert_eq!(
            destination(&root.join("new").join("..").join("copy")).unwrap(),
            root.join("copy")
        );
        assert_eq!(
            destination(&root.join("a").join("..").join("copy")).unwrap(),
            root.join("copy")
        );
        assert_eq!(
            destination(&root.join("a").join("b").join("x")).unwrap(),
            root.join("a").join("b").join("x")
        );
        // A file is not a folder to go through, even to come back out of it with `..`.
        std::fs::write(root.join("document"), "text").unwrap();
        let err = destination(&root.join("document").join("..").join("copy")).unwrap_err();
        assert!(err.contains("is a file"), "{err}");
        assert!(destination(&root.join("document").join("copy")).is_err());
        // The file itself is a destination (Save as then says it already exists).
        assert_eq!(destination(&root.join("document")).unwrap(), root.join("document"));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_after_a_missing_folder_and_its_dots_is_still_followed() {
        let (tmp, mut o) = project();
        // `link` points at a folder inside the project; `missing/..` before it changes nothing.
        std::fs::create_dir_all(o.dir.join("inner")).unwrap();
        std::os::unix::fs::symlink(o.dir.join("inner"), tmp.path().join("link")).unwrap();
        let typed = tmp
            .path()
            .join("missing")
            .join("..")
            .join("link")
            .join("..")
            .join("copy");
        let err = o.save_as(&typed).unwrap_err();
        assert!(err.contains("inside the project"), "{err}");
        assert!(!o.dir.join("copy").exists() && !tmp.path().join("copy").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_project_opened_through_a_link_and_dots_refuses_a_copy_inside_its_real_folder() {
        let (tmp, mut o) = project();
        // The project is at `design/p`; it was opened as `link/../p`, `link` pointing at
        // `design/inner`.
        let design = tmp.path().join("design");
        std::fs::create_dir_all(design.join("inner")).unwrap();
        let name = o.dir.file_name().unwrap().to_owned();
        std::fs::rename(&o.dir, design.join(&name)).unwrap();
        std::os::unix::fs::symlink(design.join("inner"), tmp.path().join("link")).unwrap();
        o.dir = tmp.path().join("link").join("..").join(&name);
        let err = o.save_as(&design.join(&name).join("copy")).unwrap_err();
        assert!(err.contains("inside the project"), "{err}");
        // Beside it is fine.
        o.save_as(&design.join("copy")).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_library_reached_through_a_link_and_dots_keeps_the_folder_the_link_leads_to() {
        let (tmp, mut o) = project();
        let x = tmp.path().join("x");
        std::fs::create_dir_all(x.join("inner")).unwrap();
        std::os::unix::fs::symlink(x.join("inner"), tmp.path().join("link")).unwrap();
        // `../link/../library` is `x/library`, which does not exist yet.
        o.project.meta.libraries.push(wc_core::project::LibrarySource {
            name: "later".into(),
            kind: "folder".into(),
            location: "../link/../library".into(),
            reference: None,
        });
        let to = tmp.path().join("copies").join("c");
        o.save_as(&to).unwrap();
        let lib = o.project.meta.libraries.iter().find(|l| l.name == "later").unwrap();
        // Made later, it is the folder the copy reaches.
        std::fs::create_dir_all(x.join("library")).unwrap();
        assert_eq!(
            std::fs::canonicalize(to.join(&lib.location)).unwrap(),
            std::fs::canonicalize(x.join("library")).unwrap()
        );
    }

    #[test]
    // Unix only: Windows takes `..` back by the text of a path (so does `std::path::absolute`
    // there), so `document\..\copy` is the folder `copy`, as Windows itself reads it.
    #[cfg(unix)]
    fn save_as_through_a_file_is_refused_and_writes_nothing() {
        let (tmp, mut o) = project();
        std::fs::write(tmp.path().join("document"), "text").unwrap();
        let typed = tmp.path().join("document").join("..").join("copy");
        let err = o.save_as(&typed).unwrap_err();
        assert!(err.contains("is a file"), "{err}");
        assert!(!tmp.path().join("copy").exists());
    }

    #[cfg(unix)]
    #[test]
    fn save_as_through_a_link_and_dots_into_the_project_is_refused() {
        let (tmp, mut o) = project();
        // `link` points at a folder inside the project: `link/../copy` is inside it too.
        std::fs::create_dir_all(o.dir.join("inner")).unwrap();
        std::os::unix::fs::symlink(o.dir.join("inner"), tmp.path().join("link")).unwrap();
        let typed = tmp.path().join("link").join("..").join("copy");
        let err = o.save_as(&typed).unwrap_err();
        assert!(err.contains("inside the project"), "{err}");
        assert!(!o.dir.join("copy").exists() && !tmp.path().join("copy").exists());
    }

    #[test]
    fn a_failed_save_as_leaves_the_symbol_repairs_for_the_original() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        o.place(1, "wc.lamp", Point::new(60.0, 20.0)).unwrap();
        o.save().unwrap();
        // Both files on disk hold another version than the loaded one.
        let (coil, lamp) = (o.symbols["wc.coil"].clone(), o.symbols["wc.lamp"].clone());
        for s in [&coil, &lamp] {
            let mut other = s.clone();
            other.name = "Other version".into();
            store::write_vendored_symbol(&o.dir, &other).unwrap();
            o.mark_symbol_file_stale(&s.id);
        }
        // In the copy the coil is repaired, then the lamp cannot be written: Save as fails.
        o.fail_symbol_writes = [false, true].into();
        let from = o.dir.clone();
        assert!(o.save_as(&tmp.path().join("copy")).is_err());
        assert_eq!(o.dir, from);
        // Saving the original still repairs both of its files.
        o.save().unwrap();
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), coil);
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.lamp").unwrap(), lamp);
    }

    #[test]
    fn save_as_refuses_a_history_that_names_its_own_work_tree() {
        let (tmp, mut o) = project();
        std::fs::create_dir_all(o.dir.join(".git")).unwrap();
        std::fs::write(
            o.dir.join(".git").join("config"),
            "[core]\n\tbare = false\n\tworktree = /elsewhere\n",
        )
        .unwrap();
        let to = tmp.path().join("copy");
        assert!(o.save_as(&to).unwrap_err().contains("core.worktree"));
        assert!(!to.exists());
        // A comment after the header, quotes and all, or the key on the header's line.
        for config in [
            "[core] # \"project worktree\"\n\tworktree = /elsewhere\n",
            "[Core] worktree = /elsewhere\n",
            "[remote \"origin\"]\n\turl = x\n[ core ]\n\tWorkTree=/elsewhere\n",
        ] {
            std::fs::write(o.dir.join(".git").join("config"), config).unwrap();
            assert!(o.save_as(&to).unwrap_err().contains("core.worktree"), "{config}");
        }
        // Another section's `worktree`, or a commented one, is not core's.
        for config in [
            "[core \"x\"]\n\tworktree = /elsewhere\n",
            "[core]\n\t# worktree = /elsewhere\n",
        ] {
            assert!(!git_worktree_set_in(config), "{config}");
        }
        // Without it the history is the copy's own.
        std::fs::write(o.dir.join(".git").join("config"), "[core]\n\tbare = false\n").unwrap();
        o.save_as(&to).unwrap();
    }

    #[test]
    fn save_as_refuses_a_work_tree_named_in_the_per_worktree_config() {
        let (tmp, mut o) = project();
        let git = o.dir.join(".git");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(
            git.join("config"),
            "[core]\n\tbare = false\n[extensions]\n\tworktreeConfig = true\n",
        )
        .unwrap();
        std::fs::write(git.join("config.worktree"), "[core]\n\tworktree = /elsewhere\n").unwrap();
        let to = tmp.path().join("copy");
        assert!(o.save_as(&to).unwrap_err().contains("core.worktree"));
        assert!(!to.exists());
        // Without it the history is the copy's own.
        std::fs::write(git.join("config.worktree"), "[core]\n\tsparseCheckout = false\n").unwrap();
        o.save_as(&to).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_library_inside_the_project_whose_real_name_is_not_text_is_refused_and_the_original_stays() {
        use std::os::unix::ffi::OsStrExt;
        let (tmp, mut o) = project();
        let odd = o.dir.join(std::ffi::OsStr::from_bytes(b"lib-\xff"));
        // macOS file systems refuse a name that is not UTF-8: nothing to test there.
        if std::fs::create_dir_all(&odd).is_err() {
            return;
        }
        std::os::unix::fs::symlink(&odd, o.dir.join("lib-link")).unwrap();
        o.project.meta.libraries.push(wc_core::project::LibrarySource {
            name: "own".into(),
            kind: "folder".into(),
            location: "lib-link".into(),
            reference: None,
        });
        let from = o.dir.clone();
        let libraries = o.project.meta.libraries.clone();
        let to = tmp.path().join("copy");
        let err = o.save_as(&to).unwrap_err();
        assert!(err.contains("library own"), "{err}");
        assert_eq!(o.dir, from);
        assert_eq!(o.project.meta.libraries, libraries);
        assert!(!to.exists());
    }

    #[test]
    fn save_as_puts_a_waiting_part_file_back_in_the_copy_and_leaves_the_original_record() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        changed_part_in_mine(&o, tmp.path(), pid);
        let coil = o.symbols["wc.coil"].clone();
        let lib = library_first(&mut o, tmp.path(), &coil);
        o.save().unwrap();
        let part_file = store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap();
        let before = std::fs::read(&part_file).unwrap();
        o.block_after_snapshot = Some(part_file.clone());
        assert!(o.update_library(&lib, None).is_err());
        assert_eq!(o.unrestored_files.len(), 1);
        // A symbol file waits for its repair too.
        let mut other = coil.clone();
        other.name = "Other version".into();
        store::write_vendored_symbol(&o.dir, &other).unwrap();
        o.mark_symbol_file_stale("wc.coil");
        // The blocker is copied along: writing the part in the copy fails, so Save as fails and
        // the original keeps its record, still under its own folder.
        let from = o.dir.clone();
        assert!(o.save_as(&tmp.path().join("copy")).is_err());
        assert_eq!(o.dir, from);
        assert_eq!(o.unrestored_files.keys().collect::<Vec<_>>(), [&part_file]);
        // With the blocker gone the part is written in the copy, not in the original.
        std::fs::remove_dir(&part_file).unwrap();
        let to = tmp.path().join("copy2");
        o.save_as(&to).unwrap();
        let copied = store::vendor_path(&to, ItemKind::Part, pid).unwrap();
        assert_eq!(std::fs::read(&copied).unwrap(), before);
        assert!(!part_file.exists(), "the original's file is not written by the copy");
        assert!(o.unrestored_files.is_empty());
        assert_eq!(store::read_vendored_symbol(&to, "wc.coil").unwrap(), coil);
        assert_eq!(store::read_vendored_symbol(&from, "wc.coil").unwrap(), other);
    }

    fn git_worktree_set_in(config: &str) -> bool {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config");
        std::fs::write(&path, config).unwrap();
        git_worktree_set(&path)
    }

    #[cfg(unix)]
    #[test]
    fn a_library_reached_through_a_link_to_a_name_that_is_not_text_keeps_a_text_route() {
        use std::os::unix::ffi::OsStrExt;
        let (tmp, mut o) = project();
        let odd = tmp.path().join(std::ffi::OsStr::from_bytes(b"shop-\xff"));
        // macOS file systems refuse a name that is not UTF-8: nothing to test there.
        if std::fs::create_dir_all(&odd).is_err() {
            return;
        }
        std::os::unix::fs::symlink(&odd, tmp.path().join("shop-link")).unwrap();
        o.project.meta.libraries.push(wc_core::project::LibrarySource {
            name: "shop".into(),
            kind: "folder".into(),
            location: "../shop-link".into(),
            reference: None,
        });
        let from = o.dir.clone();
        o.save_as(&tmp.path().join("copies").join("copy")).unwrap();
        let lib = o.project.meta.libraries.iter().find(|l| l.name == "shop").unwrap();
        assert_eq!(Path::new(&lib.location), from.join("../shop-link"));
        assert!(Path::new(&lib.location).is_dir());
    }

    #[test]
    fn save_as_refuses_a_linked_git_worktree_and_leaves_nothing_behind() {
        let (tmp, mut o) = project();
        std::fs::write(o.dir.join(".git"), "gitdir: /elsewhere/.git/worktrees/p\n").unwrap();
        let to = tmp.path().join("copy");
        assert!(o.save_as(&to).unwrap_err().contains("linked git worktree"));
        assert!(!to.exists());
    }

    #[cfg(unix)]
    #[test]
    fn save_as_follows_a_link_before_the_dots_of_a_library_folder() {
        let (tmp, mut o) = project();
        // `link/../library` is where the file system puts it: next to the link's target.
        let deep = tmp.path().join("srv").join("shared").join("deep");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(tmp.path().join("srv").join("shared").join("library")).unwrap();
        std::os::unix::fs::symlink(&deep, tmp.path().join("link")).unwrap();
        o.project.meta.libraries.push(wc_core::project::LibrarySource {
            name: "shared".into(),
            kind: "folder".into(),
            location: "../link/../library".into(),
            reference: None,
        });
        let to = tmp.path().join("copy");
        o.save_as(&to).unwrap();
        let lib = o.project.meta.libraries.iter().find(|l| l.name == "shared").unwrap();
        assert_eq!(Path::new(&lib.location), Path::new("../srv/shared/library"));
    }
    use wc_core::project::LibrarySource;

    pub(crate) fn project() -> (tempfile::TempDir, Open) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        let mut p = store::create(&dir, "Test").unwrap();
        let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library");
        p.meta.libraries.push(LibrarySource {
            name: "starter".into(),
            kind: "folder".into(),
            location: lib.display().to_string(),
            reference: None,
        });
        p.pages.push(Page::new("p0002", "2", "Control"));
        store::save(&dir, &p).unwrap();
        let open = Open::load(&dir).unwrap();
        (tmp, open)
    }

    #[test]
    fn a_part_file_that_cannot_be_read_is_not_taken_for_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing.json");
        assert_eq!(super::snapshot_file(&missing).unwrap(), None);
        // A directory where the file should be: there, but not readable as a file.
        let unreadable = tmp.path().join("part.json");
        std::fs::create_dir(&unreadable).unwrap();
        let err = super::snapshot_file(&unreadable).unwrap_err();
        assert!(err.contains("cannot be read"), "{err}");
    }

    #[test]
    fn a_part_file_is_put_back_whole_or_not_at_all() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("part.json");
        std::fs::write(&path, b"new").unwrap();
        // The replacement cannot be written next to the file: the file keeps what it had.
        std::fs::create_dir(tmp.path().join("part.json.tmp")).unwrap();
        assert!(super::restore_file(&path, Some(b"old")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        // A file that already holds the bytes is not written at all.
        assert!(super::restore_file(&path, Some(b"new")).is_ok());
        std::fs::remove_dir(tmp.path().join("part.json.tmp")).unwrap();
        super::restore_file(&path, Some(b"old")).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        super::restore_file(&path, None).unwrap();
        assert!(!path.exists());
        super::restore_file(&path, None).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_part_file_is_never_put_back_through_a_link_at_its_staging_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("part.json");
        std::fs::write(&path, b"new").unwrap();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, b"keep me").unwrap();
        std::os::unix::fs::symlink(&outside, tmp.path().join("part.json.tmp")).unwrap();
        super::restore_file(&path, Some(b"old")).unwrap();
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep me");
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert!(std::fs::symlink_metadata(tmp.path().join("part.json.tmp")).is_err());
    }

    #[test]
    fn a_black_box_is_drawn_wired_undone_and_kept_by_a_save() {
        use wc_core::{BoxPin, BoxSide};
        let (_tmp, mut o) = project();
        let pins = [
            BoxPin {
                name: "IN".into(),
                side: BoxSide::Left,
                offset: 5.0,
            },
            BoxPin {
                name: "OUT".into(),
                side: BoxSide::Right,
                offset: 5.0,
            },
        ];
        let tag = DeviceTag::parse("-A1").unwrap();
        // Corners in any order: the box's top-left is its place.
        let id = o
            .add_black_box(
                1,
                Point::new(60.0, 50.0),
                Point::new(40.0, 40.0),
                &pins,
                Some(tag.clone()),
            )
            .unwrap();
        let page_id = o.project.pages[1].id.clone();
        let symbol_id = format!("box.{page_id}.{id}");
        let placed = o.project.pages[1].symbol(&id).unwrap().clone();
        assert_eq!(
            (placed.symbol.as_str(), placed.at),
            (symbol_id.as_str(), Point::new(40.0, 40.0))
        );
        assert_eq!(placed.tag.as_ref(), Some(&tag));
        assert!(o.project.devices.contains_key(&tag));
        assert_eq!(o.project.lock.symbols[&symbol_id].source, "project");
        assert_eq!(
            store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(),
            o.symbols[&symbol_id]
        );
        let page = o.project.pages[1].clone();
        assert_eq!(o.pin_pos(&page, &id, "OUT"), Some(Point::new(60.0, 45.0)));

        // It wires like any symbol.
        let from = Endpoint::Pin {
            item: id.clone(),
            pin: "OUT".into(),
        };
        o.add_wire(
            1,
            from,
            Endpoint::Point {
                at: Point::new(80.0, 45.0),
            },
            Vec::new(),
        );
        assert_eq!(o.project.pages[1].wires.len(), 1);

        // One undo step for the box: undoing the wire and then the box takes both away.
        assert!(o.undo() && o.undo());
        assert!(o.project.pages[1].symbol(&id).is_none());
        assert!(!o.project.lock.symbols.contains_key(&symbol_id));
        assert!(!o.symbols.contains_key(&symbol_id));
        assert!(o.redo());
        assert_eq!(o.project.pages[1].symbol(&id), Some(&placed));
        assert_eq!(o.symbols[&symbol_id].pins.len(), 2);

        // A second box after the first was undone does not take over its symbol id.
        assert!(o.undo());
        let again = o
            .add_black_box(1, Point::new(10.0, 10.0), Point::new(20.0, 20.0), &[], None)
            .unwrap();
        assert_ne!(format!("box.{page_id}.{again}"), symbol_id);
        assert!(!o.can_redo());
        assert!(o.undo());
        assert!(o.redo());

        o.save().unwrap();
        let reopened = Open::load(&o.dir).unwrap();
        let box_id = format!("box.{page_id}.{again}");
        assert_eq!(reopened.project.pages[1].symbol(&again).unwrap().symbol, box_id);
        assert_eq!(reopened.symbols[&box_id], o.symbols[&box_id]);
        assert_eq!(reopened.symbols[&box_id].category, "Black box");
        // The first box, undone, left no symbol file behind (#490).
        assert!(store::read_vendored_symbol(&o.dir, &symbol_id).is_err());
    }

    #[test]
    fn a_black_box_is_resized_and_its_points_edited_as_one_undo_step() {
        use wc_core::{BoxPin, BoxSide};
        let pin = |name: &str, side, offset| BoxPin {
            name: name.into(),
            side,
            offset,
        };
        let (_tmp, mut o) = project();
        let pins = [
            pin("IN", BoxSide::Left, 5.0),
            pin("OUT", BoxSide::Right, 5.0),
            pin("PE", BoxSide::Bottom, 10.0),
        ];
        let id = o
            .add_black_box(1, Point::new(40.0, 40.0), Point::new(60.0, 50.0), &pins, None)
            .unwrap();
        let symbol_id = format!("box.{}.{id}", o.project.pages[1].id);
        for p in ["IN", "OUT", "PE"] {
            o.add_wire(
                1,
                Endpoint::Pin {
                    item: id.clone(),
                    pin: p.into(),
                },
                Endpoint::Point {
                    at: Point::new(0.0, 0.0),
                },
                Vec::new(),
            );
        }
        let wire_on = |o: &Open, i: usize| match &o.project.pages[1].wires[i].from {
            Endpoint::Pin { pin, .. } => pin.clone(),
            Endpoint::Point { .. } => String::new(),
        };
        let old = o.symbols[&symbol_id].clone();
        let old_lock = o.project.lock.symbols[&symbol_id].clone();
        let steps = o.undo_mark();

        // Wider and taller; OUT renamed to Q and kept, PE removed, a new point A added.
        let renamed: BTreeMap<String, String> = [("OUT".to_owned(), "Q".to_owned())].into_iter().collect();
        let removed: std::collections::BTreeSet<String> = ["PE".to_owned()].into_iter().collect();
        let new_pins = [
            pin("IN", BoxSide::Left, 5.0),
            pin("Q", BoxSide::Right, 5.0),
            pin("A", BoxSide::Top, 10.0),
        ];
        let warnings = o
            .edit_black_box(1, &id, (30.0, 20.0), &new_pins, &renamed, &removed)
            .unwrap();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[0].contains("connection point Q") && warnings[0].contains("moved"),
            "{warnings:?}"
        );
        assert!(
            warnings[1].contains("connection point PE") && warnings[1].contains("gone"),
            "{warnings:?}"
        );
        assert_eq!(o.undo_mark(), steps + 1, "one undo step");
        let new = o.symbols[&symbol_id].clone();
        assert_eq!(wc_core::black_box_layout(&new), Some((30.0, 20.0, new_pins.to_vec())));
        assert_eq!(store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(), new);
        assert_eq!(o.project.lock.symbols[&symbol_id].hash, new.hash());
        assert_eq!(wire_on(&o, 1), "Q", "the wire follows the renamed point");
        assert_eq!(
            o.project.pages[1].wires[2].from,
            Endpoint::Point {
                at: Point::new(50.0, 50.0)
            },
            "the wire on the removed point keeps its end where the point was"
        );
        let page = o.project.pages[1].clone();
        assert_eq!(o.pin_pos(&page, &id, "Q"), Some(Point::new(70.0, 45.0)));

        // Undo puts back the earlier version: symbol, file, lock and the wire's point.
        assert!(o.undo());
        assert_eq!(o.symbols[&symbol_id], old);
        assert_eq!(store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(), old);
        assert_eq!(o.project.lock.symbols[&symbol_id], old_lock);
        assert_eq!(wire_on(&o, 1), "OUT");
        assert_eq!(wire_on(&o, 2), "PE");
        assert!(o.redo());
        assert_eq!(o.symbols[&symbol_id], new);
        assert_eq!(store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(), new);

        // Nothing changed: no undo step. A refused box changes nothing either.
        let steps = o.undo_mark();
        assert!(
            o.edit_black_box(1, &id, (30.0, 20.0), &new_pins, &BTreeMap::new(), &Default::default())
                .unwrap()
                .is_empty()
        );
        let off = [pin("IN", BoxSide::Left, 50.0)];
        assert!(
            o.edit_black_box(1, &id, (30.0, 20.0), &off, &BTreeMap::new(), &Default::default())
                .is_err()
        );
        o.fail_symbol_writes.push_back(true);
        assert!(
            o.edit_black_box(1, &id, (40.0, 20.0), &new_pins, &BTreeMap::new(), &Default::default())
                .is_err()
        );
        assert_eq!(o.undo_mark(), steps);
        assert_eq!(o.symbols[&symbol_id], new);

        // Role and letter code are kept.
        let mut kept = o.symbols[&symbol_id].clone();
        kept.letter_code = "A".into();
        kept.role = wc_core::symbol::SymbolRole::Main;
        o.symbols.insert(symbol_id.clone(), kept.clone());
        o.project.lock.symbols.get_mut(&symbol_id).unwrap().hash = kept.hash();
        store::write_vendored_symbol(&o.dir, &kept).unwrap();
        o.edit_black_box(1, &id, (35.0, 20.0), &new_pins, &BTreeMap::new(), &Default::default())
            .unwrap();
        let edited = &o.symbols[&symbol_id];
        assert_eq!(
            (edited.letter_code.as_str(), edited.role),
            ("A", wc_core::symbol::SymbolRole::Main)
        );

        // A loaded version the lock does not pin (its file changed outside the app) is not
        // overwritten: Undo could not bring the pinned one back.
        let mut stray = o.symbols[&symbol_id].clone();
        stray.name = "changed outside".into();
        o.symbols.insert(symbol_id.clone(), stray);
        let steps = o.undo_mark();
        let err = o
            .edit_black_box(1, &id, (40.0, 20.0), &new_pins, &BTreeMap::new(), &Default::default())
            .unwrap_err();
        assert!(err.contains("not the version the project pins"), "{err}");
        assert_eq!(o.undo_mark(), steps);
        o.symbols.insert(
            symbol_id.clone(),
            o.symbol_versions[&o.project.lock.symbols[&symbol_id].hash].clone(),
        );

        // A page someone else holds that places the box refuses the edit.
        let held = o.project.pages[1].id.clone();
        o.held_by_others.insert(held.clone(), "Ola".into());
        let err = o
            .edit_black_box(1, &id, (40.0, 20.0), &new_pins, &BTreeMap::new(), &Default::default())
            .unwrap_err();
        assert!(err.contains("Ola has page"), "{err}");
        assert_eq!(o.undo_mark(), steps);
        o.held_by_others.remove(&held);

        // So does another held page that places the same box, though it is not the one edited:
        // the symbol file, the lock and the undo history stay as they were.
        o.place(0, &symbol_id, Point::new(20.0, 20.0)).unwrap();
        let other = o.project.pages[0].id.clone();
        o.held_by_others.insert(other.clone(), "Kari".into());
        let before = o.symbols[&symbol_id].clone();
        let lock_before = o.project.lock.symbols[&symbol_id].clone();
        let steps = o.undo_mark();
        let err = o
            .edit_black_box(1, &id, (40.0, 20.0), &new_pins, &BTreeMap::new(), &Default::default())
            .unwrap_err();
        assert!(err.contains("Kari has page 1 open"), "{err}");
        assert_eq!(o.undo_mark(), steps);
        assert_eq!(o.symbols[&symbol_id], before);
        assert_eq!(store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(), before);
        assert_eq!(o.project.lock.symbols[&symbol_id], lock_before);
        o.held_by_others.remove(&other);
        assert!(
            o.edit_black_box(1, &id, (40.0, 20.0), &new_pins, &BTreeMap::new(), &Default::default())
                .is_ok()
        );

        // Only a black box is edited this way.
        let coil = o.place(1, "wc.coil", Point::new(120.0, 20.0)).unwrap();
        let err = o
            .edit_black_box(1, &coil, (30.0, 20.0), &[], &BTreeMap::new(), &Default::default())
            .unwrap_err();
        assert!(err.contains("not a black box"), "{err}");
    }

    #[test]
    fn a_box_file_changed_outside_the_app_is_not_overwritten_by_an_edit() {
        use wc_core::{BoxPin, BoxSide};
        let (_tmp, mut o) = project();
        let pins = [BoxPin {
            name: "1".into(),
            side: BoxSide::Left,
            offset: 5.0,
        }];
        let id = o
            .add_black_box(1, Point::new(40.0, 40.0), Point::new(60.0, 50.0), &pins, None)
            .unwrap();
        let symbol_id = format!("box.{}.{id}", o.project.pages[1].id);
        o.save().unwrap();
        // Another program writes another box into the file; the project still pins the first.
        let outside = wc_core::black_box(&symbol_id, "Black box", 50.0, 50.0, &pins).unwrap();
        store::write_vendored_symbol(&o.dir, &outside).unwrap();
        let mut reopened = Open::load(&o.dir).unwrap();
        let lock = reopened.project.lock.symbols[&symbol_id].clone();
        let steps = reopened.undo_mark();
        let result = reopened.edit_black_box(1, &id, (30.0, 20.0), &pins, &BTreeMap::new(), &Default::default());
        if reopened.symbols.get(&symbol_id).map(Symbol::hash) == Some(lock.hash.clone()) {
            // The pinned version was loaded instead of the file's: editing it is safe.
            assert!(result.is_ok(), "{result:?}");
        } else {
            let err = result.unwrap_err();
            assert!(err.contains("not the version the project pins"), "{err}");
            assert_eq!(
                store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(),
                outside,
                "file kept"
            );
            assert_eq!(reopened.project.lock.symbols[&symbol_id], lock, "lock kept");
            assert_eq!(reopened.undo_mark(), steps, "no undo step");
        }
    }

    #[test]
    fn a_box_file_changed_on_disk_while_open_is_not_overwritten_by_an_edit() {
        use wc_core::{BoxPin, BoxSide};
        let (_tmp, mut o) = project();
        let pins = [BoxPin {
            name: "1".into(),
            side: BoxSide::Left,
            offset: 5.0,
        }];
        let none = BTreeMap::new();
        let id = o
            .add_black_box(1, Point::new(40.0, 40.0), Point::new(60.0, 50.0), &pins, None)
            .unwrap();
        let symbol_id = format!("box.{}.{id}", o.project.pages[1].id);
        // Edits one after another in the app, and Undo and Redo between them, which write
        // the file themselves, go ahead.
        o.edit_black_box(1, &id, (30.0, 20.0), &pins, &none, &Default::default())
            .unwrap();
        o.edit_black_box(1, &id, (40.0, 20.0), &pins, &none, &Default::default())
            .unwrap();
        assert!(o.undo());
        o.edit_black_box(1, &id, (35.0, 20.0), &pins, &none, &Default::default())
            .unwrap();
        assert!(o.undo());
        assert!(o.redo());
        o.edit_black_box(1, &id, (45.0, 25.0), &pins, &none, &Default::default())
            .unwrap();
        assert_eq!(
            store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(),
            o.symbols[&symbol_id],
            "the file holds the last edit"
        );
        // Another program (`wiringcraft box-edit`) resizes the box while the project is open.
        let outside = wc_core::black_box(&symbol_id, "Black box", 50.0, 50.0, &pins).unwrap();
        store::write_vendored_symbol(&o.dir, &outside).unwrap();
        let loaded = o.symbols[&symbol_id].clone();
        let lock = o.project.lock.symbols[&symbol_id].clone();
        let at = o.project.pages[1].symbol(&id).unwrap().at;
        let steps = o.undo_mark();
        for result in [
            o.edit_black_box(1, &id, (30.0, 20.0), &pins, &none, &Default::default()),
            // Sized from a near corner on the canvas, so the box moves too.
            o.edit_black_box_moved(
                1,
                &id,
                (30.0, 20.0),
                &pins,
                &none,
                &Default::default(),
                Point::new(5.0, 0.0),
            ),
        ] {
            let err = result.unwrap_err();
            assert!(
                err.contains("changed on disk") && err.contains("reopen the project"),
                "{err}"
            );
        }
        assert_eq!(
            store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(),
            outside,
            "file kept"
        );
        assert_eq!(o.symbols[&symbol_id], loaded, "loaded version kept");
        assert_eq!(o.project.lock.symbols[&symbol_id], lock, "lock kept");
        assert_eq!(o.project.pages[1].symbol(&id).unwrap().at, at, "box not moved");
        assert_eq!(o.undo_mark(), steps, "no undo step");
        // A file that cannot be read any more is not written over either.
        let path = store::vendor_path(&o.dir, ItemKind::Symbol, &symbol_id).unwrap();
        std::fs::write(&path, "not json").unwrap();
        let err = o
            .edit_black_box(1, &id, (30.0, 20.0), &pins, &none, &Default::default())
            .unwrap_err();
        assert!(err.contains("cannot be read"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");
        // A file the app could not put back is the app's to write: an edit that cannot be
        // written keeps it so, and one that is written ends that, so a change on disk after it
        // is refused again.
        o.mark_symbol_file_stale(&symbol_id);
        o.fail_symbol_writes.push_back(true);
        assert!(
            o.edit_black_box(1, &id, (30.0, 20.0), &pins, &none, &Default::default())
                .is_err()
        );
        assert!(o.stale_symbol_files.contains(&symbol_id), "still to be written");
        o.edit_black_box(1, &id, (30.0, 20.0), &pins, &none, &Default::default())
            .unwrap();
        assert!(!o.stale_symbol_files.contains(&symbol_id), "written");
        assert_eq!(
            store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(),
            o.symbols[&symbol_id]
        );
        store::write_vendored_symbol(&o.dir, &outside).unwrap();
        let steps = o.undo_mark();
        let err = o
            .edit_black_box(1, &id, (40.0, 20.0), &pins, &none, &Default::default())
            .unwrap_err();
        assert!(err.contains("changed on disk"), "{err}");
        assert_eq!(
            store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(),
            outside,
            "file kept"
        );
        assert_eq!(o.undo_mark(), steps, "no undo step");
    }

    #[test]
    fn a_removed_point_whose_name_is_reused_lets_go_of_its_wire() {
        use wc_core::{BoxPin, BoxSide};
        let pin = |name: &str, side, offset| BoxPin {
            name: name.into(),
            side,
            offset,
        };
        let (_tmp, mut o) = project();
        let pins = [pin("1", BoxSide::Left, 5.0), pin("2", BoxSide::Right, 5.0)];
        let id = o
            .add_black_box(1, Point::new(40.0, 40.0), Point::new(60.0, 50.0), &pins, None)
            .unwrap();
        for p in ["1", "2"] {
            o.add_wire(
                1,
                Endpoint::Pin {
                    item: id.clone(),
                    pin: p.into(),
                },
                Endpoint::Point {
                    at: Point::new(0.0, 0.0),
                },
                Vec::new(),
            );
        }
        // Point 1 taken off; point 2 renamed 1 and moved where 1 was; a new 2 where 2 was.
        let renamed: BTreeMap<String, String> = [("2".to_owned(), "1".to_owned())].into_iter().collect();
        let removed: std::collections::BTreeSet<String> = ["1".to_owned()].into_iter().collect();
        let warnings = o
            .edit_black_box(
                1,
                &id,
                (20.0, 10.0),
                &[pin("1", BoxSide::Left, 5.0), pin("2", BoxSide::Right, 5.0)],
                &renamed,
                &removed,
            )
            .unwrap();
        assert!(
            warnings.iter().any(|w| w.contains("w1") && w.contains("gone")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("w2") && w.contains("moved")),
            "{warnings:?}"
        );
        let wires = &o.project.pages[1].wires;
        assert_eq!(
            wires[0].from,
            Endpoint::Point {
                at: Point::new(40.0, 45.0)
            },
            "not left on the point that took its name"
        );
        assert_eq!(
            wires[1].from,
            Endpoint::Pin {
                item: id.clone(),
                pin: "1".into()
            }
        );
        // A swap keeps both points and both wires.
        let swap: BTreeMap<String, String> = [("1".to_owned(), "2".to_owned()), ("2".to_owned(), "1".to_owned())]
            .into_iter()
            .collect();
        o.edit_black_box(
            1,
            &id,
            (20.0, 10.0),
            &[pin("2", BoxSide::Left, 5.0), pin("1", BoxSide::Right, 5.0)],
            &swap,
            &Default::default(),
        )
        .unwrap();
        assert_eq!(
            o.project.pages[1].wires[1].from,
            Endpoint::Pin {
                item: id.clone(),
                pin: "2".into()
            }
        );
    }

    #[test]
    fn removing_a_box_point_where_another_wire_has_a_free_end_is_refused() {
        use wc_core::{BoxPin, BoxSide};
        let (_tmp, mut o) = project();
        let pins = [BoxPin {
            name: "1".into(),
            side: BoxSide::Left,
            offset: 5.0,
        }];
        let id = o
            .add_black_box(1, Point::new(40.0, 40.0), Point::new(60.0, 50.0), &pins, None)
            .unwrap();
        let free = |x, y| Endpoint::Point { at: Point::new(x, y) };
        let on_1 = Endpoint::Pin {
            item: id.clone(),
            pin: "1".into(),
        };
        o.add_wire(1, on_1.clone(), free(0.0, 0.0), Vec::new());
        // Another wire ends freely where point 1 is (40, 45), not joined to it.
        o.add_wire(1, free(40.0, 45.0), free(0.0, 90.0), Vec::new());
        let removed: std::collections::BTreeSet<String> = ["1".to_owned()].into_iter().collect();
        let before = o.project.clone();
        let symbol = o.symbols[&format!("box.{}.{id}", o.project.pages[1].id)].clone();
        let steps = o.undo_mark();
        let err = o
            .edit_black_box(1, &id, (20.0, 10.0), &[], &BTreeMap::new(), &removed)
            .unwrap_err();
        assert!(
            err.contains("wire w1") && err.contains("wire w2") && err.contains("joined"),
            "{err}"
        );
        assert_eq!(o.project, before, "nothing changes");
        assert_eq!(o.symbols[&symbol.id], symbol);
        assert_eq!(o.undo_mark(), steps);

        // With that wire end moved away the point comes off, and its wire keeps a free end.
        o.project.pages[1].wires[1].from = free(30.0, 45.0);
        o.edit_black_box(1, &id, (20.0, 10.0), &[], &BTreeMap::new(), &removed)
            .unwrap();
        assert_eq!(o.project.pages[1].wires[0].from, free(40.0, 45.0));
    }

    #[test]
    fn an_undone_black_box_leaves_no_symbol_file_after_a_save_and_redo_brings_it_back() {
        let (_tmp, mut o) = project();
        let id = o
            .add_black_box(1, Point::new(10.0, 10.0), Point::new(30.0, 20.0), &[], None)
            .unwrap();
        let symbol_id = format!("box.{}.{id}", o.project.pages[1].id);
        let placed = o.project.pages[1].symbol(&id).unwrap().clone();
        assert!(store::read_vendored_symbol(&o.dir, &symbol_id).is_ok());
        assert!(o.undo());
        o.save().unwrap();
        assert!(
            store::read_vendored_symbol(&o.dir, &symbol_id).is_err(),
            "no page places it and the lock does not pin it"
        );

        // Redo after that save puts the box and its file back.
        assert!(o.redo());
        assert_eq!(o.project.pages[1].symbol(&id), Some(&placed));
        o.save().unwrap();
        let reopened = Open::load(&o.dir).unwrap();
        assert_eq!(reopened.project.pages[1].symbol(&id), Some(&placed));
        assert_eq!(reopened.symbols[&symbol_id], o.symbols[&symbol_id]);
    }

    #[test]
    fn a_refused_black_box_leaves_the_project_as_it_was() {
        use wc_core::{BoxPin, BoxSide};
        let (_tmp, mut o) = project();
        let before = o.project.clone();
        let off_edge = [BoxPin {
            name: "1".into(),
            side: BoxSide::Top,
            offset: 50.0,
        }];
        let err = o
            .add_black_box(1, Point::new(0.0, 0.0), Point::new(10.0, 10.0), &off_edge, None)
            .unwrap_err();
        assert!(err.contains("off its edge"), "{err}");
        let err = o
            .add_black_box(1, Point::new(5.0, 5.0), Point::new(5.0, 20.0), &[], None)
            .unwrap_err();
        assert!(err.contains("width and height above zero"), "{err}");
        // The symbol file cannot be written: nothing is placed or pinned.
        o.fail_symbol_writes.push_back(true);
        assert!(
            o.add_black_box(1, Point::new(0.0, 0.0), Point::new(10.0, 10.0), &[], None)
                .is_err()
        );
        assert_eq!(o.project, before);
        assert!(!o.can_undo());
    }

    #[test]
    fn undo_puts_back_the_symbol_version_a_library_update_replaced() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let old = o.symbols["wc.coil"].clone();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        o.checkpoint();
        store::write_vendored_symbol(&o.dir, &new).unwrap();
        o.project.lock.symbols.get_mut("wc.coil").unwrap().hash = new.hash();
        o.reload_symbols();
        assert_eq!(o.symbols["wc.coil"], new);

        assert!(o.undo());
        assert_eq!(o.symbols["wc.coil"], old, "the cache follows the lock");
        assert_eq!(
            store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(),
            old,
            "and the file"
        );
        assert!(o.redo());
        assert_eq!(o.symbols["wc.coil"], new);
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), new);
    }

    #[test]
    fn undo_and_redo_list_the_symbol_version_they_bring_back() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let old = o.symbols["wc.coil"].clone();
        let mut new = old.clone();
        new.description = "Solenoid winding".into();
        o.checkpoint();
        store::write_vendored_symbol(&o.dir, &new).unwrap();
        o.project.lock.symbols.get_mut("wc.coil").unwrap().hash = new.hash();
        o.reload_symbols();
        o.refresh_library();
        let listed = |o: &Open| o.library.iter().find(|e| e.id == "wc.coil").unwrap().clone();
        assert_eq!(listed(&o).description, new.description);

        assert!(o.undo(), "{:?}", o.take_undo_error());
        assert_eq!(listed(&o).description, old.description);
        assert!(!listed(&o).matches("solenoid"));
        assert!(o.redo(), "{:?}", o.take_undo_error());
        assert_eq!(listed(&o).description, new.description);
        assert!(listed(&o).matches("solenoid"));
    }

    #[test]
    fn redo_puts_back_a_new_version_of_a_symbol_no_page_uses() {
        let (_tmp, mut o) = project();
        o.ensure_symbol("wc.coil").unwrap();
        let old = o.symbols["wc.coil"].clone();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        o.checkpoint();
        store::write_vendored_symbol(&o.dir, &new).unwrap();
        o.project.lock.symbols.get_mut("wc.coil").unwrap().hash = new.hash();
        o.reload_symbols();
        assert_eq!(o.symbols["wc.coil"], new, "kept though no page uses it");
        assert!(o.undo());
        assert_eq!(o.symbols["wc.coil"], old);
        assert!(o.redo());
        assert_eq!(o.symbols["wc.coil"], new);
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), new);
        assert_eq!(o.project.lock.symbols["wc.coil"].hash, new.hash());
    }

    #[test]
    fn a_pinned_symbol_whose_copy_is_gone_is_put_back_in_the_version_it_pins() {
        let (tmp, mut o) = project();
        o.ensure_symbol("wc.coil").unwrap();
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        // Its copy gone when the project is opened: pinned, but not loaded.
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap()).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        assert!(!o.symbols.contains_key("wc.coil"));
        // A newer version wins for the id; the source the pin names still has the old one.
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        library_first(&mut o, tmp.path(), &new);
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        assert_eq!(o.symbols["wc.coil"], old, "placed in the version pinned");
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), old);
        assert_eq!(o.project.lock.symbols["wc.coil"].hash, old.hash());
    }

    #[test]
    fn a_pinned_symbol_put_back_is_listed_with_its_own_description() {
        let (tmp, mut o) = project();
        o.ensure_symbol("wc.coil").unwrap();
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap()).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        // The library that wins for the id has a newer version with another description.
        let mut new = old.clone();
        new.description = "Solenoid winding".into();
        library_first(&mut o, tmp.path(), &new);
        o.refresh_library();
        let listed = |o: &Open| o.library.iter().find(|e| e.id == "wc.coil").unwrap().clone();
        assert_eq!(listed(&o).description, new.description);
        // Placing puts back the pinned version: the list follows it.
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        assert_eq!(o.symbols["wc.coil"], old);
        assert_eq!(listed(&o).description, old.description);
        assert!(!listed(&o).matches("solenoid"));
        assert_eq!(o.library.iter().filter(|e| e.id == "wc.coil").count(), 1);
    }

    #[test]
    fn a_refused_change_that_loaded_a_symbol_puts_the_symbol_list_back() {
        let (tmp, mut o) = project();
        o.ensure_symbol("wc.coil").unwrap();
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap()).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        let mut new = old.clone();
        new.description = "Solenoid winding".into();
        library_first(&mut o, tmp.path(), &new);
        o.refresh_library();
        let m = o.mark();
        let listed = o.library.clone();
        // The change loads the pinned version, which changes its row, and is then refused.
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        assert_ne!(o.library, listed);
        // A symbol only this project has gets a row of its own.
        let mut own = old.clone();
        own.id = "test.own".into();
        o.load_symbol("test.own", own);
        o.back_to(m).unwrap();
        assert_eq!(o.library, listed, "the symbol list is as it was at the mark");
    }

    #[test]
    fn a_pinned_symbol_loaded_in_another_version_is_not_placed() {
        let (_tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let clip = o.copy(1, &[Selection::Symbol(id)]);
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        // The file is changed outside the app: the project still pins the old version.
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        store::write_vendored_symbol(&o.dir, &new).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        assert_eq!(o.symbols["wc.coil"], new, "loaded as the file has it");
        let err = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap_err();
        assert!(err.contains("not the version the project pins"), "{err}");
        // Nor pasted, or inserted from a macro.
        let before = o.project.clone();
        let err = o.paste(1, &clip, Point::new(80.0, 80.0)).unwrap_err();
        assert!(err.contains("not the version the project pins"), "{err}");
        assert_eq!(o.project, before);
        // The pinned copy put back: placed with it, without opening the project again.
        store::write_vendored_symbol(&o.dir, &old).unwrap();
        o.place(1, "wc.coil", Point::new(40.0, 20.0)).unwrap();
        assert_eq!(o.symbols["wc.coil"], old);
    }

    #[test]
    fn a_pinned_symbol_loaded_in_another_version_whose_file_is_gone_comes_from_the_library() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        store::write_vendored_symbol(&o.dir, &new).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        assert_eq!(o.symbols["wc.coil"], new);
        // The wrong copy removed: the library still has the pinned version.
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap()).unwrap();
        o.place(1, "wc.coil", Point::new(40.0, 20.0)).unwrap();
        assert_eq!(o.symbols["wc.coil"], old);
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), old);
    }

    #[test]
    fn a_refused_change_puts_back_a_symbol_it_replaced_with_the_pinned_version() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        store::write_vendored_symbol(&o.dir, &new).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        assert_eq!(o.symbols["wc.coil"], new, "loaded as the file has it");
        // The pinned copy put back, then a change places with it and is refused.
        store::write_vendored_symbol(&o.dir, &old).unwrap();
        let m = o.mark();
        o.place(1, "wc.coil", Point::new(40.0, 20.0)).unwrap();
        assert_eq!(o.symbols["wc.coil"], old, "the change loaded the pinned version");
        o.back_to(m).unwrap();
        assert_eq!(o.symbols["wc.coil"], new, "the symbol loaded at the mark is back");
        // It is not the pinned version, so its file is left as it is.
        assert!(!o.stale_symbol_files.contains("wc.coil"));
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), old);
    }

    /// A saved project with one black box on page 1: its item and symbol ids.
    fn saved_box() -> (tempfile::TempDir, Open, String, String) {
        let (tmp, mut o) = project();
        let id = o
            .add_black_box(1, Point::new(40.0, 40.0), Point::new(60.0, 50.0), &box_pin(), None)
            .unwrap();
        let symbol_id = format!("box.{}.{id}", o.project.pages[1].id);
        o.save().unwrap();
        (tmp, o, id, symbol_id)
    }

    /// The one point of the box [`saved_box`] places.
    fn box_pin() -> [wc_core::BoxPin; 1] {
        [wc_core::BoxPin {
            name: "IN".into(),
            side: wc_core::BoxSide::Left,
            offset: 5.0,
        }]
    }

    /// Black box `id` on page 1 made 30 mm wide, its point kept.
    fn widen_box(o: &mut Open, id: &str) {
        o.edit_black_box(1, id, (30.0, 10.0), &box_pin(), &BTreeMap::new(), &Default::default())
            .unwrap();
    }

    #[test]
    fn a_refused_box_edit_is_unsaved_work_until_its_file_is_written_back() {
        let (_tmp, mut o, id, symbol_id) = saved_box();
        let old = o.symbols[&symbol_id].clone();
        let m = o.mark();
        widen_box(&mut o, &id);
        assert_ne!(store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(), old);
        o.back_to(m).unwrap();
        assert_eq!(o.symbols[&symbol_id], old);
        assert!(o.dirty, "the box file still holds the refused edit");
        o.save().unwrap();
        assert_eq!(store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(), old);
    }

    #[test]
    fn a_box_edit_repeated_after_a_refusal_is_not_saved_over_by_the_repair() {
        let (_tmp, mut o, id, symbol_id) = saved_box();
        let m = o.mark();
        widen_box(&mut o, &id);
        o.back_to(m).unwrap();
        // The same edit again, kept: its file holds the version the project now pins.
        widen_box(&mut o, &id);
        let accepted = o.symbols[&symbol_id].clone();
        assert_eq!(o.project.lock.symbols[&symbol_id].hash, accepted.hash());
        o.save().unwrap();
        assert_eq!(
            store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(),
            accepted,
            "the repair queued by the refusal does not write the old version over it"
        );
    }

    #[test]
    fn a_box_edit_repeated_over_a_file_changed_outside_while_its_repair_waits_is_refused() {
        let (_tmp, mut o, id, symbol_id) = saved_box();
        let old = o.symbols[&symbol_id].clone();
        let m = o.mark();
        widen_box(&mut o, &id);
        o.back_to(m).unwrap();
        let mut other = old.clone();
        other.name = "Box, changed".into();
        store::write_vendored_symbol(&o.dir, &other).unwrap();
        let err = o
            .edit_black_box(1, &id, (30.0, 10.0), &box_pin(), &BTreeMap::new(), &Default::default())
            .unwrap_err();
        assert!(err.contains("changed outside"), "{err}");
        assert_eq!(store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(), other);
        // The repair still waits, so Save still asks about that version.
        let err = o.save().unwrap_err();
        assert!(err.contains(Open::CHANGED_OUTSIDE), "{err}");
    }

    #[test]
    fn an_undo_that_writes_a_symbol_file_lets_its_pending_repair_go() {
        let (_tmp, mut o, id, symbol_id) = saved_box();
        let first = o.symbols[&symbol_id].clone();
        widen_box(&mut o, &id);
        // A further edit is refused: its repair waits to write the widened version.
        let m = o.mark();
        o.edit_black_box(1, &id, (40.0, 10.0), &box_pin(), &BTreeMap::new(), &Default::default())
            .unwrap();
        o.back_to(m).unwrap();
        // Undo takes the widening back and writes the first version to the file.
        assert!(o.undo());
        assert_eq!(o.project.lock.symbols[&symbol_id].hash, first.hash());
        o.save().unwrap();
        assert_eq!(
            store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(),
            first,
            "the repair does not write the widened version over what Undo wrote"
        );
    }

    #[test]
    fn a_symbol_file_changed_outside_before_a_box_edit_is_refused_is_not_written_over_unasked() {
        let (_tmp, mut o, id, symbol_id) = saved_box();
        let old = o.symbols[&symbol_id].clone();
        let m = o.mark();
        widen_box(&mut o, &id);
        // Another program changes the box file before the edit is refused.
        let mut other = old.clone();
        other.name = "Box, changed".into();
        store::write_vendored_symbol(&o.dir, &other).unwrap();
        o.back_to(m).unwrap();
        let err = o.save().unwrap_err();
        assert!(err.contains(Open::CHANGED_OUTSIDE), "{err}");
        assert_eq!(store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(), other);
    }

    #[test]
    fn a_symbol_file_repaired_on_save_is_not_written_again_unchecked() {
        let (_tmp, mut o, id, symbol_id) = saved_box();
        let old = o.symbols[&symbol_id].clone();
        let m = o.mark();
        widen_box(&mut o, &id);
        o.back_to(m).unwrap();
        // The checked repair writes the file; a second, unchecked write would fail here.
        o.fail_symbol_writes = [true].into();
        o.save().unwrap();
        assert_eq!(store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(), old);
    }

    #[test]
    fn nested_refused_changes_never_save_a_symbol_version_the_project_does_not_pin() {
        let (_tmp, o, id, symbol_id) = saved_box();
        let pinned = o.symbols[&symbol_id].clone();
        // The box file is changed outside the app: loaded as the file has it.
        let mut other = pinned.clone();
        other.name = "Box, changed".into();
        store::write_vendored_symbol(&o.dir, &other).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        assert_eq!(o.symbols[&symbol_id], other);
        let listed = o.library.clone();
        // The pinned copy put back; a change loads it and edits the box in an inner change.
        store::write_vendored_symbol(&o.dir, &pinned).unwrap();
        let outer = o.mark();
        o.ensure_symbol(&symbol_id).unwrap();
        o.refresh_library();
        let inner = o.mark();
        widen_box(&mut o, &id);
        o.back_to(inner).unwrap();
        assert!(o.stale_symbol_files.contains(&symbol_id));
        o.back_to(outer).unwrap();
        assert_eq!(o.symbols[&symbol_id], other, "the symbol loaded at the mark is back");
        assert_eq!(o.library, listed, "and so is its list row");
        assert!(o.dirty, "the box file still holds the refused edit");
        o.save().unwrap();
        assert_eq!(
            store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(),
            pinned,
            "the file holds the version the project pins, not the refused edit or the loaded one"
        );
    }

    #[test]
    fn a_repair_queued_by_a_refused_change_follows_the_pin_an_enclosing_refusal_puts_back() {
        let (_tmp, mut o, id, symbol_id) = saved_box();
        let first = o.symbols[&symbol_id].clone();
        // The outer change edits the box: the project pins that version.
        let outer = o.mark();
        widen_box(&mut o, &id);
        let pinned = o.symbols[&symbol_id].clone();
        assert_ne!(pinned, first);
        // Another program changes the box file and it is read in; then the pinned copy is put
        // back, and a middle change loads it and edits the box in an inner change.
        let mut other = pinned.clone();
        other.name = "Box, changed".into();
        store::write_vendored_symbol(&o.dir, &other).unwrap();
        o.reload_symbols();
        assert_eq!(o.symbols[&symbol_id], other);
        store::write_vendored_symbol(&o.dir, &pinned).unwrap();
        let middle = o.mark();
        o.ensure_symbol(&symbol_id).unwrap();
        let inner = o.mark();
        o.edit_black_box(1, &id, (40.0, 10.0), &box_pin(), &BTreeMap::new(), &Default::default())
            .unwrap();
        o.back_to(inner).unwrap();
        o.back_to(middle).unwrap();
        // The middle refusal queued the middle pin for the box file; the outer refusal pins the
        // first version again.
        o.back_to(outer).unwrap();
        assert_eq!(o.project.lock.symbols[&symbol_id].hash, first.hash());
        assert_eq!(o.symbols[&symbol_id], first);
        assert!(o.dirty, "the box file still holds the refused edit");
        o.save().unwrap();
        assert_eq!(
            store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(),
            first,
            "the file holds the version the project pins again, not one a refused change pinned"
        );
    }

    #[test]
    fn a_symbol_file_changed_outside_while_its_repair_waits_is_not_written_over_unasked() {
        let (_tmp, mut o, id, symbol_id) = saved_box();
        let old = o.symbols[&symbol_id].clone();
        let m = o.mark();
        widen_box(&mut o, &id);
        o.back_to(m).unwrap();
        // Another program changes the box file before the repair is saved.
        let mut other = old.clone();
        other.name = "Box, changed".into();
        store::write_vendored_symbol(&o.dir, &other).unwrap();
        let err = o.save().unwrap_err();
        assert!(err.contains(Open::CHANGED_OUTSIDE), "{err}");
        assert_eq!(store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(), other);
        // Keeping that version leaves it in its file.
        let path = o.changed_outside().unwrap();
        o.keep_outside_version(&path);
        o.save().unwrap();
        assert_eq!(store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(), other);
    }

    #[test]
    fn a_symbol_repair_written_over_an_outside_change_on_request_holds_the_pinned_version() {
        let (_tmp, mut o, id, symbol_id) = saved_box();
        let old = o.symbols[&symbol_id].clone();
        let m = o.mark();
        widen_box(&mut o, &id);
        o.back_to(m).unwrap();
        let mut other = old.clone();
        other.name = "Box, changed".into();
        store::write_vendored_symbol(&o.dir, &other).unwrap();
        assert!(o.save().is_err());
        let path = o.changed_outside().unwrap();
        o.write_over_outside_version(&path);
        o.save().unwrap();
        assert_eq!(store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(), old);
    }

    #[test]
    fn a_box_edit_refused_inside_a_refused_change_is_unsaved_work() {
        let (_tmp, mut o, id, symbol_id) = saved_box();
        let old = o.symbols[&symbol_id].clone();
        let outer = o.mark();
        let inner = o.mark();
        widen_box(&mut o, &id);
        o.back_to(inner).unwrap();
        o.back_to(outer).unwrap();
        assert!(o.dirty, "the box file still holds the refused edit");
        o.save().unwrap();
        assert_eq!(store::read_vendored_symbol(&o.dir, &symbol_id).unwrap(), old);
    }

    #[test]
    fn a_symbol_file_another_program_wrote_during_a_refused_change_is_left_as_it_is() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        let m = o.mark();
        // Another program writes another version, which the change reads in.
        let mut other = old.clone();
        other.name = "Coil, changed".into();
        store::write_vendored_symbol(&o.dir, &other).unwrap();
        o.reload_symbols();
        assert_eq!(o.symbols["wc.coil"], other);
        o.back_to(m).unwrap();
        assert_eq!(o.symbols["wc.coil"], old);
        assert!(!o.stale_symbol_files.contains("wc.coil"));
        o.save().unwrap();
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), other);
    }

    #[test]
    fn putting_back_a_pinned_terminal_copy_keeps_jumpers_on_their_terminals() {
        let (_tmp, mut o) = project();
        let terminal = |id: &str, role| Symbol {
            source: String::new(),
            id: id.into(),
            name: "Terminal".into(),
            category: String::new(),
            letter_code: "X".into(),
            role,
            graphics: vec![],
            pins: vec![],
            tag_at: Default::default(),
            description: String::new(),
        };
        let pinned = terminal("test.term", wc_core::symbol::SymbolRole::Terminal);
        store::write_vendored_symbol(&o.dir, &pinned).unwrap();
        let other = terminal("test.term2", wc_core::symbol::SymbolRole::Terminal);
        store::write_vendored_symbol(&o.dir, &other).unwrap();
        o.project.lock.symbols.insert(
            "test.term".into(),
            wc_core::lock::LockEntry {
                source: "starter".into(),
                hash: pinned.hash(),
            },
        );
        // A (the pinned symbol), B and C on strip -X1, numbered 1, 2, 3; A and B jumpered.
        for (id, x, symbol) in [
            ("a", 10.0, "test.term"),
            ("b", 20.0, "test.term2"),
            ("c", 30.0, "test.term2"),
        ] {
            o.project.pages[1].symbols.push(PlacedSymbol {
                id: id.into(),
                symbol: symbol.into(),
                at: Point::new(x, 10.0),
                rotation: Default::default(),
                mirror: false,
                tag: Some(DeviceTag::parse("-X1").unwrap()),
            });
        }
        let x1 = DeviceTag::parse("-X1").unwrap();
        o.project.meta.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        o.save().unwrap();
        // Opened with another copy, in which A's symbol is no terminal (B and C are 1 and 2
        // then); an edit meanwhile.
        store::write_vendored_symbol(&o.dir, &terminal("test.term", wc_core::symbol::SymbolRole::Main)).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        o.checkpoint();
        o.project.pages[1].title = "Terminals".into();
        // The pinned copy put back, then the symbol placed: nothing moved, so the jumper stays.
        store::write_vendored_symbol(&o.dir, &pinned).unwrap();
        o.place(1, "test.term", Point::new(80.0, 10.0)).unwrap();
        o.checkpoint();
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "2"]);
    }

    #[test]
    fn a_terminal_copy_put_back_by_a_paste_that_fails_later_leaves_jumpers_alone() {
        let (_tmp, mut o) = project();
        let terminal = |id: &str, role| Symbol {
            source: String::new(),
            id: id.into(),
            name: "Terminal".into(),
            category: String::new(),
            letter_code: "X".into(),
            role,
            graphics: vec![],
            pins: vec![],
            tag_at: Default::default(),
            description: String::new(),
        };
        let pinned = terminal("test.term", wc_core::symbol::SymbolRole::Terminal);
        store::write_vendored_symbol(&o.dir, &pinned).unwrap();
        let other = terminal("test.term2", wc_core::symbol::SymbolRole::Terminal);
        store::write_vendored_symbol(&o.dir, &other).unwrap();
        o.project.lock.symbols.insert(
            "test.term".into(),
            wc_core::lock::LockEntry {
                source: "starter".into(),
                hash: pinned.hash(),
            },
        );
        for (id, x, symbol) in [
            ("a", 10.0, "test.term"),
            ("b", 20.0, "test.term2"),
            ("c", 30.0, "test.term2"),
        ] {
            o.project.pages[1].symbols.push(PlacedSymbol {
                id: id.into(),
                symbol: symbol.into(),
                at: Point::new(x, 10.0),
                rotation: Default::default(),
                mirror: false,
                tag: Some(DeviceTag::parse("-X1").unwrap()),
            });
        }
        let x1 = DeviceTag::parse("-X1").unwrap();
        o.project.meta.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        o.project.meta.strips.push(wc_core::project::StripLayout {
            strip: x1.clone(),
            order: vec!["2".into(), "1".into()],
            spares: vec![],
        });
        o.save().unwrap();
        // Opened with a copy in which A is no terminal: B and C are 1 and 2 here.
        store::write_vendored_symbol(&o.dir, &terminal("test.term", wc_core::symbol::SymbolRole::Main)).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        o.checkpoint();
        let jumpers = o.project.meta.jumpers.clone();
        // A change in progress: B and C swap places, so they swap numbers (C 1, B 2). The rail
        // order (C, B) follows it once.
        let page = &mut o.project.pages[1].symbols;
        let (b, c) = (
            page.iter().position(|s| s.id == "b").unwrap(),
            page.iter().position(|s| s.id == "c").unwrap(),
        );
        let (at_b, at_c) = (page[b].at, page[c].at);
        page[b].at = at_c;
        page[c].at = at_b;
        // A paste puts the pinned copy back, then fails on a symbol no library has.
        store::write_vendored_symbol(&o.dir, &pinned).unwrap();
        assert!(o.ensure_symbols(["test.term", "no.such.symbol"]).is_err());
        // Nothing was added, and the next step does not read the rollback as terminals moved.
        o.checkpoint();
        assert_eq!(o.project.meta.jumpers, jumpers);
        assert_eq!(
            o.project.meta.strip_layout(&x1).map(|l| l.order.clone()),
            Some(vec!["1".into(), "2".into()])
        );
    }

    #[test]
    fn a_pinned_symbol_is_put_back_from_the_second_of_two_libraries_with_one_name() {
        let (tmp, mut o) = project();
        o.ensure_symbol("wc.coil").unwrap();
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap()).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        library_first(&mut o, tmp.path(), &new);
        // A second folder also named "mine", after it, with the pinned version.
        let second = tmp.path().join("mine2");
        std::fs::create_dir_all(second.join("symbols")).unwrap();
        std::fs::write(
            second.join("symbols/wc.coil.json"),
            serde_json::to_string(&old).unwrap(),
        )
        .unwrap();
        o.project.meta.libraries.retain(|l| l.name == "mine");
        o.project.meta.libraries.push(LibrarySource {
            name: "mine".into(),
            kind: "folder".into(),
            location: second.display().to_string(),
            reference: None,
        });
        o.project.lock.symbols.get_mut("wc.coil").unwrap().source = "mine".into();
        o.ensure_symbol("wc.coil").unwrap();
        assert_eq!(o.symbols["wc.coil"], old);
    }

    #[test]
    fn a_library_file_holding_another_symbol_is_named_as_such() {
        let (tmp, mut o) = project();
        o.ensure_symbol("wc.coil").unwrap();
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap()).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        let mut wrong = old.clone();
        wrong.id = "wc.other".into();
        library_first(&mut o, tmp.path(), &old);
        std::fs::write(
            tmp.path().join("mine/symbols/wc.coil.json"),
            serde_json::to_string(&wrong).unwrap(),
        )
        .unwrap();
        o.project.lock.symbols.get_mut("wc.coil").unwrap().source = "mine".into();
        o.project.meta.libraries.retain(|l| l.name == "mine");
        let err = o.ensure_symbol("wc.coil").unwrap_err();
        assert!(err.contains("mine: its file holds wc.other"), "{err}");
    }

    #[test]
    fn a_pinned_symbol_is_put_back_from_any_library_that_has_its_version() {
        let (tmp, mut o) = project();
        o.ensure_symbol("wc.coil").unwrap();
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap()).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        // The pin names a library that has moved on ("extra"), and another one wins for the id
        // ("mine"); only the last one ("starter") still has the pinned version.
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        library_first(&mut o, tmp.path(), &new);
        let extra = tmp.path().join("extra");
        std::fs::create_dir_all(extra.join("symbols")).unwrap();
        std::fs::write(extra.join("symbols/wc.coil.json"), serde_json::to_string(&new).unwrap()).unwrap();
        o.project.meta.libraries.insert(
            1,
            LibrarySource {
                name: "extra".into(),
                kind: "folder".into(),
                location: extra.display().to_string(),
                reference: None,
            },
        );
        o.project.lock.symbols.get_mut("wc.coil").unwrap().source = "extra".into();
        o.ensure_symbol("wc.coil").unwrap();
        assert_eq!(o.symbols["wc.coil"], old);
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), old);
    }

    #[test]
    fn a_pinned_symbol_no_library_has_in_its_version_is_not_placed() {
        let (tmp, mut o) = project();
        o.ensure_symbol("wc.coil").unwrap();
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap()).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        library_first(&mut o, tmp.path(), &new);
        // The pin names the newer library only: nowhere has the pinned version.
        o.project.lock.symbols.get_mut("wc.coil").unwrap().source = "mine".into();
        o.project.meta.libraries.retain(|l| l.name == "mine");
        let lock = o.project.lock.clone();
        let err = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap_err();
        assert!(
            err.contains("mine: another version"),
            "what each library had is said: {err}"
        );
        assert_eq!(o.project.lock, lock, "the pin is kept");
        assert!(
            !store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil")
                .unwrap()
                .exists()
        );
    }

    #[test]
    fn a_paste_or_macro_needing_a_pinned_symbol_no_library_has_adds_nothing() {
        let (tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let mut clip = o.copy(1, &[Selection::Symbol(id)]);
        // A breaker, which the library has and the project doesn't yet: loaded before the coil
        // (ids are loaded in order), and taken back with it.
        let mut breaker = clip.symbols[0].clone();
        breaker.symbol = "wc.breaker.1p".into();
        breaker.tag = None;
        clip.symbols.insert(0, breaker);
        let old = o.symbols["wc.coil"].clone();
        o.save().unwrap();
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap()).unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        library_first(&mut o, tmp.path(), &new);
        o.project.lock.symbols.get_mut("wc.coil").unwrap().source = "mine".into();
        o.project.meta.libraries.retain(|l| l.name == "mine");
        let starter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/symbols/wc.breaker.1p.json");
        std::fs::copy(starter, tmp.path().join("mine/symbols/wc.breaker.1p.json")).unwrap();
        let before = o.project.clone();
        let listed = o.library.clone();
        let undo = o.can_undo();
        let err = o.paste(1, &clip, Point::new(80.0, 80.0)).unwrap_err();
        assert!(err.contains("wc.coil"), "the symbol is named: {err}");
        assert_eq!(o.project, before, "nothing is added, not even the breaker's pin");
        assert!(!o.symbols.contains_key("wc.breaker.1p"));
        assert_eq!(o.library, listed, "the symbol list goes back too");
        assert_eq!(o.can_undo(), undo, "no undo step");
        let path = o.save_macro(&clip, "Coil", "").unwrap();
        let err = o.insert_macro(1, &path, None).unwrap_err();
        assert!(err.contains("another version"), "{err}");
        assert_eq!(o.project.pages, before.pages, "the macro adds nothing either");
    }

    /// A library folder named "mine", put ahead of the others, holding `sym`.
    pub(crate) fn library_first(o: &mut Open, root: &Path, sym: &Symbol) -> wc_library::Library {
        let mine = root.join("mine");
        std::fs::create_dir_all(mine.join("symbols")).unwrap();
        std::fs::write(
            mine.join(format!("symbols/{}.json", sym.id)),
            serde_json::to_string(sym).unwrap(),
        )
        .unwrap();
        o.project.meta.libraries.insert(
            0,
            LibrarySource {
                name: "mine".into(),
                kind: "folder".into(),
                location: mine.display().to_string(),
                reference: None,
            },
        );
        wc_library::Library::open(&o.dir, &o.project).unwrap()
    }

    /// Put a changed copy of a vendored part into the "mine" library folder.
    fn changed_part_in_mine(o: &Open, root: &Path, pid: &str) -> wc_core::Part {
        let mut part = store::read_vendored_part(&o.dir, pid).unwrap();
        part.description = "Changed in the library".into();
        std::fs::create_dir_all(root.join("mine/parts")).unwrap();
        std::fs::write(
            root.join(format!("mine/parts/{pid}.json")),
            serde_json::to_string(&part).unwrap(),
        )
        .unwrap();
        part
    }

    #[test]
    fn a_maker_dxf_is_attached_as_a_parts_footprint_and_the_projects_copy_follows() {
        let (tmp, mut o) = project();
        let pid = "maker.io-8";
        let mine = tmp.path().join("mine");
        let part = Part {
            id: pid.into(),
            manufacturer: "Maker".into(),
            order_number: "IO-8".into(),
            ..Part::default()
        };
        wc_library::write_part(&mine, &part).unwrap();
        // A symbol that shares the part's id, pinned, with a newer drawing in the library.
        let mut sym = o.symbols.get("wc.coil").cloned().unwrap_or_else(|| {
            o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
            o.symbols["wc.coil"].clone()
        });
        sym.id = pid.into();
        wc_library::write_symbol(&mine, &sym).unwrap();
        o.project.meta.libraries.insert(
            0,
            LibrarySource {
                name: "mine".into(),
                kind: "folder".into(),
                location: mine.display().to_string(),
                reference: None,
            },
        );
        o.place(1, pid, Point::new(80.0, 20.0)).unwrap();
        let sym_pin = o.project.lock.symbols[pid].clone();
        let sym_file = std::fs::read(store::vendor_path(&o.dir, ItemKind::Symbol, pid).unwrap()).unwrap();
        let mut newer = sym.clone();
        newer.name = "Newer drawing".into();
        wc_library::write_symbol(&mine, &newer).unwrap();
        let tag = DeviceTag::parse("-A1").unwrap();
        o.set_device_part(&tag, Some(pid)).unwrap();
        o.refresh_library();
        let pin = o.project.lock.parts[pid].hash.clone();
        assert!(o.parts[pid].footprint.is_none());

        // A 22.5 x 100 mm outline drawn away from the origin, with an entity it cannot read.
        let dxf = tmp.path().join("io-8.dxf");
        std::fs::write(
            &dxf,
            "0\nSECTION\n2\nENTITIES\n0\nLWPOLYLINE\n8\n0\n90\n4\n70\n1\n\
             10\n200\n20\n50\n10\n222.5\n20\n50\n10\n222.5\n20\n150\n10\n200\n20\n150\n\
             0\n3DFACE\n8\n0\n10\n0\n20\n0\n30\n0\n\
             0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        let err = o.attach_footprint("no.such", &dxf).unwrap_err();
        assert!(err.contains("none of the project's libraries"), "{err}");
        let msg = o.attach_footprint(pid, &dxf).unwrap();
        assert!(msg.contains("22.5 x 100 mm"), "{msg}");
        assert!(msg.contains("3DFACE"), "the entity not read is named: {msg}");
        assert!(!msg.contains("not updated"), "{msg}");
        // Stored with the part in its library, and the project's copy follows at once.
        let in_lib: Part = store::read_json(&mine.join("parts").join(format!("{pid}.json"))).unwrap();
        let f = in_lib.footprint.as_ref().unwrap();
        assert_eq!((f.width, f.height), (22.5, 100.0));
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), in_lib);
        assert_eq!(o.parts[pid].footprint, in_lib.footprint, "the layout draws it now");
        assert_ne!(o.project.lock.parts[pid].hash, pin);
        // Only the part is updated: the symbol of the same id keeps its pin and its file.
        assert_eq!(o.project.lock.symbols[pid], sym_pin);
        assert_eq!(
            std::fs::read(store::vendor_path(&o.dir, ItemKind::Symbol, pid).unwrap()).unwrap(),
            sym_file
        );
        // Attaching the same drawing again changes nothing, and keeps Redo.
        o.checkpoint();
        o.project.pages[1].title = "Renamed".into();
        assert!(o.undo());
        let undo_steps = o.undo.len();
        o.attach_footprint(pid, &dxf).unwrap();
        assert!(o.can_redo(), "Redo is kept");
        assert_eq!(o.undo.len(), undo_steps, "no empty undo step");
        // One undo step takes the project's pin back.
        assert!(o.undo());
        assert_eq!(o.project.lock.parts[pid].hash, pin);
        // And the project's copy with it, in the folder and loaded (#498).
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap().hash(), pin);
        assert_eq!(o.parts[pid].hash(), pin);
        assert!(o.parts[pid].footprint.is_none());
        // Redo brings the new copy back the same way.
        assert!(o.redo());
        assert_eq!(o.project.lock.parts[pid].hash, in_lib.hash());
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), in_lib);
        assert_eq!(o.parts[pid], in_lib);
        // A copy that cannot be written leaves the step undone, and everything as it was.
        o.fail_part_writes.push_back(true);
        assert!(!o.undo());
        let err = o.take_undo_error().unwrap_or_default();
        assert!(err.contains(&format!("part {pid} could not be put back")), "{err}");
        assert_eq!(o.project.lock.parts[pid].hash, in_lib.hash());
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), in_lib);
        assert_eq!(o.parts[pid], in_lib);
        assert_eq!(o.project.lock.symbols[pid], sym_pin);
        assert!(o.undo());
        assert_eq!(o.parts[pid].hash(), pin);

        // A drawing it cannot read changes nothing.
        let bad = tmp.path().join("bad.dxf");
        std::fs::write(&bad, "0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nEOF\n").unwrap();
        assert!(o.attach_footprint(pid, &bad).is_err());
        let still: Part = store::read_json(&mine.join("parts").join(format!("{pid}.json"))).unwrap();
        assert_eq!(still, in_lib);
    }

    #[test]
    fn a_maker_step_model_sizes_a_part_and_one_undo_takes_it_back() {
        let (tmp, mut o) = project();
        let pid = "maker.relay";
        let mine = tmp.path().join("mine");
        let part = Part {
            id: pid.into(),
            manufacturer: "Maker".into(),
            order_number: "RL-1".into(),
            ..Part::default()
        };
        wc_library::write_part(&mine, &part).unwrap();
        o.project.meta.libraries.insert(
            0,
            LibrarySource {
                name: "mine".into(),
                kind: "folder".into(),
                location: mine.display().to_string(),
                reference: None,
            },
        );
        let tag = DeviceTag::parse("-K1").unwrap();
        o.set_device_part(&tag, Some(pid)).unwrap();
        o.refresh_library();
        let pin = o.project.lock.parts[pid].hash.clone();
        assert_eq!(
            wc_core::panel::part_size(&o.parts[pid]),
            None,
            "no size before the model"
        );
        // On a plate: -K1 at the size of a part without one, -K2 sized by hand, -K3 at the
        // plate's right edge, where the wider model would not fit.
        let (k2, k3) = (DeviceTag::parse("-K2").unwrap(), DeviceTag::parse("-K3").unwrap());
        o.set_device_part(&k2, Some(pid)).unwrap();
        o.set_device_part(&k3, Some(pid)).unwrap();
        let mut plate = wc_core::panel::Panel::new("pnl1", "Plate", 200.0, 300.0);
        let default = wc_core::panel::DEFAULT_SIZE;
        let r0 = wc_core::geom::Rotation::R0;
        plate
            .place_device(tag.clone(), Point::new(10.0, 10.0), default, r0)
            .unwrap();
        plate
            .place_device(k2.clone(), Point::new(50.0, 10.0), (30.0, 30.0), r0)
            .unwrap();
        plate
            .place_device(k3.clone(), Point::new(200.0 - default.0, 150.0), default, r0)
            .unwrap();
        o.project.panels.push(plate);
        let plate_before = o.project.panels[0].clone();
        let size_of = |o: &Open, t: &DeviceTag| match o.project.panels[0].device(t) {
            Some(wc_core::panel::PanelItem::Device { width, height, .. }) => (*width, *height),
            _ => unreachable!("on the plate"),
        };

        // A 22.5 x 90 x 70 mm box in metres, as the maker's model.
        let mut data = String::new();
        let mut id = 1;
        for x in [0.0, 0.0225] {
            for y in [0.0, 0.09] {
                for z in [0.0, 0.07] {
                    data.push_str(&format!(
                        "#{id}=CARTESIAN_POINT('',({x:?},{y:?},{z:?}));#{}=VERTEX_POINT('',#{id});\n",
                        id + 1
                    ));
                    id += 2;
                }
            }
        }
        let stp = tmp.path().join("relay.stp");
        std::fs::write(
            &stp,
            format!(
                "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n#900=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT($,.METRE.));\n{data}ENDSEC;\nEND-ISO-10303-21;\n"
            ),
        )
        .unwrap();
        let err = o.attach_model("no.such", &stp).unwrap_err();
        assert!(err.contains("none of the project's libraries"), "{err}");
        // A full undo history: the update still counts as one step.
        for _ in 0..UNDO_STEPS + 5 {
            o.checkpoint();
        }
        let undo_steps = o.undo.len();
        assert_eq!(undo_steps, UNDO_STEPS);
        let msg = o.attach_model(pid, &stp).unwrap();
        assert!(msg.contains("22.5 x 90 x 70 mm"), "{msg}");
        assert!(!msg.contains("not updated"), "{msg}");
        assert_eq!(o.undo.len(), undo_steps, "one undo step, the oldest dropped");
        // Stored with the part in its library, and the project's copy follows at once.
        let in_lib: Part = store::read_json(&mine.join("parts").join(format!("{pid}.json"))).unwrap();
        assert!(in_lib.model.is_some());
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), in_lib);
        assert_eq!(o.parts[pid].model, in_lib.model);
        assert_eq!(
            wc_core::panel::part_size(&o.parts[pid]),
            Some((22.5, 90.0)),
            "sized by the model"
        );
        assert_eq!(wc_core::panel::part_depth(&o.parts[pid]), Some(70.0));
        // The device drawn at the default size takes the model's; the others keep theirs.
        assert_eq!(size_of(&o, &tag), (22.5, 90.0));
        assert_eq!(size_of(&o, &k2), (30.0, 30.0));
        assert_eq!(size_of(&o, &k3), default);
        assert!(msg.contains("1 device on the panel layouts now 22.5 x 90 mm"), "{msg}");
        assert!(msg.contains("-K3 on Plate (-K3: a 22.5 x 90 mm outline"), "{msg}");
        // Undo puts the part back as it was, and the plate with it.
        assert!(o.undo());
        assert_eq!(o.project.panels[0], plate_before);
        assert_eq!(o.project.lock.parts[pid].hash, pin);
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap().hash(), pin);
        assert!(o.parts[pid].model.is_none());
        assert_eq!(wc_core::panel::part_size(&o.parts[pid]), None);
        assert!(o.redo());
        assert_eq!(o.parts[pid], in_lib);

        // A file that is not a whole STEP model changes nothing.
        let bad = tmp.path().join("bad.stp");
        std::fs::write(&bad, "ISO-10303-21;\nDATA;\n").unwrap();
        let err = o.attach_model(pid, &bad).unwrap_err();
        assert!(err.contains("incomplete"), "{err}");
        let still: Part = store::read_json(&mine.join("parts").join(format!("{pid}.json"))).unwrap();
        assert_eq!(still, in_lib);
    }

    #[test]
    fn a_jumper_part_is_one_undo_step_with_its_pin() {
        let (_tmp, mut o) = project();
        let x1 = DeviceTag::parse("-X1").unwrap();
        for (x, tag) in [(20.0, "-X1.1"), (40.0, "-X1.2")] {
            let id = o.place(1, "wc.terminal", Point::new(x, 20.0)).unwrap();
            let s = o.project.pages[1].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(DeviceTag::parse(tag).unwrap());
        }
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        let pid = "example.ex-tb-2-5";
        let lock = o.project.lock.clone();
        assert!(!lock.parts.contains_key(pid));
        o.set_jumper_part(&x1, "2", Some(pid)).unwrap();
        assert!(o.project.lock.parts.contains_key(pid));
        assert!(o.undo());
        assert_eq!(o.project.lock, lock, "Undo takes the pin back with the part");
        assert!(o.project.meta.jumpers[0].part.is_none());
        assert!(o.redo());
        assert!(o.project.lock.parts.contains_key(pid), "Redo brings it back");
        assert_eq!(o.project.meta.jumpers[0].part.as_deref(), Some(pid));
    }

    #[test]
    fn a_part_copy_without_its_pin_is_pinned_as_it_is_not_replaced() {
        let (tmp, mut o) = project();
        let pid = "example.ex-ct-9-24dc";
        let tag = DeviceTag::parse("-K1").unwrap();
        o.set_device_part(&tag, Some(pid)).unwrap();
        // The pin is gone (an undone import left the copy), and the library has a newer one.
        let copy = store::read_vendored_part(&o.dir, pid).unwrap();
        o.project.lock.parts.remove(pid);
        o.parts.remove(pid);
        changed_part_in_mine(&o, tmp.path(), pid);
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let coil = o.symbols["wc.coil"].clone();
        library_first(&mut o, tmp.path(), &coil);
        let other = DeviceTag::parse("-K2").unwrap();
        o.set_device_part(&other, Some(pid)).unwrap();
        assert_eq!(
            store::read_vendored_part(&o.dir, pid).unwrap(),
            copy,
            "the copy is kept"
        );
        assert_eq!(o.project.lock.parts[pid].hash, copy.hash(), "and pinned as it is");

        // A pinned part whose copy is missing is not replaced by another library version: the
        // pinned one is put back from a library that still has it, though the pin names
        // another and a newer one comes first.
        o.parts.remove(pid);
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap()).unwrap();
        let third = DeviceTag::parse("-K3").unwrap();
        o.set_device_part(&third, Some(pid)).unwrap();
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), copy);
        o.parts.remove(pid);
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap()).unwrap();
        // Pinned to a source that still has the pinned version, though another one comes first
        // with a newer one: the copy comes from the source the pin names.
        let pin = o.project.lock.parts[pid].clone();
        o.project.lock.parts.get_mut(pid).unwrap().source = "starter".into();
        let sixth = DeviceTag::parse("-K6").unwrap();
        o.set_device_part(&sixth, Some(pid)).unwrap();
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), copy);
        o.parts.remove(pid);
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap()).unwrap();
        o.project.lock.parts.insert(pid.into(), pin);
        // Pinned to a source that has moved on, while the source that comes first still has the
        // pinned version: the copy comes from that one, and the pin stays as it is.
        let pin = o.project.lock.parts[pid].clone();
        assert_eq!(pin.source, "mine");
        o.project.meta.libraries.rotate_left(1);
        let seventh = DeviceTag::parse("-K7").unwrap();
        o.set_device_part(&seventh, Some(pid)).unwrap();
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), copy);
        assert_eq!(o.project.lock.parts[pid], pin);
        o.project.meta.libraries.rotate_right(1);
        o.parts.remove(pid);
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap()).unwrap();
        // With the pinned version back in the library, the copy is put back and the pin kept.
        let pin = o.project.lock.parts[pid].clone();
        wc_library::write_part(&tmp.path().join("mine"), &copy).unwrap();
        o.set_device_part(&DeviceTag::parse("-K8").unwrap(), Some(pid)).unwrap();
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), copy);
        assert_eq!(o.project.lock.parts[pid], pin);

        // A copy that is not the pinned version is not used.
        let mut edited = copy.clone();
        edited.description = "Edited by hand".into();
        store::write_vendored_part(&o.dir, &edited).unwrap();
        o.parts.remove(pid);
        let fifth = DeviceTag::parse("-K5").unwrap();
        let err = o.set_device_part(&fifth, Some(pid)).unwrap_err();
        assert!(err.contains("not the version the project pins"), "{err}");
        store::write_vendored_part(&o.dir, &copy).unwrap();

        // A copy without its pin whose library file cannot be read says why.
        o.project.lock.parts.remove(pid);
        o.parts.remove(pid);
        std::fs::write(tmp.path().join(format!("mine/parts/{pid}.json")), "{ not json").unwrap();
        let fourth = DeviceTag::parse("-K4").unwrap();
        let err = o.set_device_part(&fourth, Some(pid)).unwrap_err();
        assert!(
            err.contains("cannot be read") && err.contains("line 1"),
            "the parse error is kept: {err}"
        );
    }

    #[test]
    fn an_update_refused_for_a_dropped_pin_writes_no_part_either() {
        let (tmp, mut o) = project();
        let s1 = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let s2 = o.place(1, "wc.coil", Point::new(60.0, 20.0)).unwrap();
        let pin = |item: &str| Endpoint::Pin {
            item: item.to_owned(),
            pin: "A2".into(),
        };
        o.project.pages[1].wires.push(Wire {
            id: "w1".into(),
            from: pin(&s1),
            to: pin(&s2),
            via: Vec::new(),
            number: None,
            potential: None,
            cable: None,
            route: Default::default(),
            spec: Default::default(),
        });
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        let part_file = store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap();
        let part_before = std::fs::read(&part_file).unwrap();
        changed_part_in_mine(&o, tmp.path(), pid);
        let mut new = o.symbols["wc.coil"].clone();
        new.pins.retain(|p| p.name != "A2");
        let lib = library_first(&mut o, tmp.path(), &new);
        let lock = o.project.lock.clone();
        let err = o.update_library(&lib, None).unwrap_err();
        assert!(err.starts_with("nothing was updated") && err.contains(":A2"), "{err}");
        assert_eq!(
            std::fs::read(&part_file).unwrap(),
            part_before,
            "the part file is untouched"
        );
        assert_eq!(o.project.lock, lock);
    }

    #[test]
    fn a_change_taken_back_leaves_no_part_file_and_keeps_undo_marks_in_step() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let (undo_mark, can_undo) = (o.undo_mark(), o.can_undo());
        let pid = "example.ex-ct-9-24dc";
        let part_file = store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap();
        // A refused change that took a part from the library: its file goes with it.
        let mark = o.mark();
        assert_eq!(o.undo_mark(), undo_mark, "the set-aside steps still count");
        o.ensure_part(pid).unwrap();
        o.checkpoint();
        assert!(part_file.exists());
        o.back_to(mark).unwrap();
        assert!(!part_file.exists(), "no unpinned part file is left");
        assert!(!o.project.lock.parts.contains_key(pid));
        assert_eq!((o.undo_mark(), o.can_undo()), (undo_mark, can_undo));
        // Taken again later, it is pinned (and so sent to the session).
        o.ensure_part(pid).unwrap();
        assert!(o.project.lock.parts.contains_key(pid));
        // A file that was there before stays, even when the change only read it.
        o.parts.remove(pid);
        let mark = o.mark();
        o.ensure_part(pid).unwrap();
        o.back_to(mark).unwrap();
        assert!(part_file.exists(), "a file that was there before stays");
        // An unpinned copy left behind (a failed removal) is pinned when it is next used.
        o.project.lock.parts.remove(pid);
        o.parts.remove(pid);
        o.ensure_part(pid).unwrap();
        assert!(
            o.project.lock.parts.contains_key(pid),
            "the leftover copy is pinned again"
        );
        // A vendor folder that cannot be checked (here: a link out of the project) is reported,
        // and nothing in it is removed.
        #[cfg(unix)]
        {
            let parts = o.dir.join(store::VENDOR_DIR).join(ItemKind::Part.dir());
            let outside = _tmp.path().join("outside-parts");
            std::fs::rename(&parts, &outside).unwrap();
            std::os::unix::fs::symlink(&outside, &parts).unwrap();
            let mark = o.mark();
            let err = o.back_to(mark).unwrap_err();
            assert!(
                err.contains("not checked") && err.contains("outside the project"),
                "{err}"
            );
            assert!(part_file.exists(), "nothing removed");
            std::fs::remove_file(&parts).unwrap();
            std::fs::rename(&outside, &parts).unwrap();
        }
        // A mark taken inside a kept change (a form it opened) means the same step afterwards.
        let mark = o.mark();
        let inside = o.start_live_form();
        o.keep(mark);
        assert_eq!(inside, o.undo_mark());
    }

    #[test]
    fn undo_and_redo_of_a_part_update_put_its_project_copy_back() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        let old = store::read_vendored_part(&o.dir, pid).unwrap();
        o.parts.insert(pid.to_owned(), old.clone());
        let new = changed_part_in_mine(&o, tmp.path(), pid);
        let coil = o.symbols["wc.coil"].clone();
        let lib = library_first(&mut o, tmp.path(), &coil);
        let done = o.update_library(&lib, None).unwrap();
        assert_eq!(done, vec![(ItemKind::Part, pid.to_owned())]);
        assert_eq!(o.parts[pid], new);
        // Undo: the lock, the file and the loaded part all name the old version.
        assert!(o.undo());
        assert_eq!(o.project.lock.parts[pid].hash, old.hash());
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), old);
        assert_eq!(o.parts[pid], old);
        o.ensure_part(pid).unwrap();
        // Redo: the new one again.
        assert!(o.redo());
        assert_eq!(o.project.lock.parts[pid].hash, new.hash());
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), new);
        assert_eq!(o.parts[pid], new);
        o.ensure_part(pid).unwrap();
        // A copy changed in the project, in a step that does not touch the pin, is left alone.
        o.checkpoint();
        o.project.pages[1].title = "Renamed".into();
        let mut edited = new.clone();
        edited.description = "Edited in the project".into();
        o.parts.insert(pid.to_owned(), edited.clone());
        assert!(o.undo());
        assert_eq!(o.parts[pid], edited);
    }

    #[test]
    fn an_update_naming_an_unknown_id_takes_nothing_and_keeps_the_history() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        o.place(1, "wc.coil", Point::new(60.0, 20.0)).unwrap();
        changed_part_in_mine(&o, tmp.path(), pid);
        let coil = o.symbols["wc.coil"].clone();
        let lib = library_first(&mut o, tmp.path(), &coil);
        // Something to undo and something to redo.
        assert!(o.undo());
        let (undo, redo, history, dirty) = (o.undo.len(), o.redo.len(), o.history, o.dirty);
        assert!(undo > 0 && redo > 0);
        let lock = o.project.lock.clone();
        assert!(lock.parts.contains_key(pid), "the part is pinned");
        let file = std::fs::read(store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap()).unwrap();
        // The known part has a newer version, but the unknown id stops the whole update.
        let only = [pid.to_owned(), "nosuch.item".to_owned()];
        let err = o.update_library(&lib, Some(&only)).unwrap_err();
        assert!(err.contains("nosuch.item"), "{err}");
        assert_eq!(o.project.lock, lock);
        assert_eq!(
            std::fs::read(store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap()).unwrap(),
            file
        );
        assert_eq!(
            (o.undo.len(), o.redo.len(), o.history, o.dirty),
            (undo, redo, history, dirty)
        );
        // The known id alone is taken.
        let done = o.update_library(&lib, Some(&only[..1])).unwrap();
        assert_eq!(done, vec![(ItemKind::Part, pid.to_owned())]);
    }

    #[test]
    fn undo_puts_back_a_loaded_part_an_import_replaced() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        o.save().unwrap();
        o.part_versions.clear();
        let old = o.parts[pid].clone();
        // As a second import does: the part's file and its pin replaced on disk.
        let mut new = old.clone();
        new.description = "From the second import".into();
        store::write_vendored_part(&o.dir, &new).unwrap();
        let mut p = store::load(&o.dir).unwrap();
        p.lock.parts.get_mut(pid).unwrap().hash = new.hash();
        store::save(&o.dir, &p).unwrap();
        assert!(o.adopt_saved().unwrap());
        assert!(o.undo(), "{:?}", o.take_undo_error());
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), old);
        // Redo puts the imported version back, also when no device uses the part, so it was
        // never loaded.
        assert!(!o.project.used_parts().contains(pid));
        assert!(o.redo(), "{:?}", o.take_undo_error());
        assert_eq!(o.project.lock.parts[pid].hash, new.hash());
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), new);
    }

    #[test]
    fn undo_puts_back_a_part_that_is_not_loaded_and_was_never_loaded_here() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        let old = store::read_vendored_part(&o.dir, pid).unwrap();
        // As after reopening: no device uses the part, so it is pinned but not loaded, and
        // no version of it was seen.
        o.parts.remove(pid);
        o.part_versions.clear();
        let new = changed_part_in_mine(&o, tmp.path(), pid);
        let coil = o.symbols["wc.coil"].clone();
        let lib = library_first(&mut o, tmp.path(), &coil);
        o.update_library(&lib, None).unwrap();
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), new);
        assert!(o.undo(), "{:?}", o.take_undo_error());
        assert_eq!(o.project.lock.parts[pid].hash, old.hash());
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), old);
        // Not used by any device, and not loaded once the library is read again.
        o.refresh_library();
        assert!(!o.parts.contains_key(pid));
        assert!(o.redo(), "{:?}", o.take_undo_error());
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), new);
    }

    #[test]
    fn a_part_file_undo_cannot_put_back_is_written_by_the_next_save() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let ids = ["example.ex-ct-9-24dc", "example.ex-tb-2-5"];
        let mut old = Vec::new();
        let mut new = Vec::new();
        for (n, id) in ids.iter().enumerate() {
            let tag = DeviceTag::parse(&format!("-K{}", n + 1)).unwrap();
            o.set_device_part(&tag, Some(id)).unwrap();
            old.push(store::read_vendored_part(&o.dir, id).unwrap());
            new.push(changed_part_in_mine(&o, tmp.path(), id));
        }
        let coil = o.symbols["wc.coil"].clone();
        let lib = library_first(&mut o, tmp.path(), &coil);
        o.update_library(&lib, None).unwrap();
        o.save().unwrap();
        assert!(!o.dirty);
        let lock = o.project.lock.clone();
        // Undo writes the first part, fails on the second, and cannot put the first back.
        o.fail_part_writes = [false, true, true].into();
        assert!(!o.undo());
        assert!(o.dirty, "the file left to put back is unsaved work");
        let err = o.take_undo_error().unwrap_or_default();
        assert!(err.contains("could not be put back: example.ex-ct-9-24dc"), "{err}");
        assert_eq!(o.project.lock, lock, "the step stays where it was");
        // The pinned (new) version stays loaded, though the file holds the old one.
        assert_eq!(o.parts[ids[0]], new[0]);
        assert_eq!(store::read_vendored_part(&o.dir, ids[0]).unwrap(), old[0]);
        o.refresh_library();
        assert_eq!(o.parts[ids[0]], new[0], "the pinned version stays loaded");
        // The next save writes it first.
        o.save().unwrap();
        assert_eq!(store::read_vendored_part(&o.dir, ids[0]).unwrap(), new[0]);
        assert_eq!(store::read_vendored_part(&o.dir, ids[1]).unwrap(), new[1]);
    }

    #[test]
    fn a_second_failed_undo_keeps_the_first_backup_of_a_part_file() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let ids = ["example.ex-ct-9-24dc", "example.ex-tb-2-5"];
        let mut new = Vec::new();
        for (n, id) in ids.iter().enumerate() {
            let tag = DeviceTag::parse(&format!("-K{}", n + 1)).unwrap();
            o.set_device_part(&tag, Some(id)).unwrap();
            new.push(changed_part_in_mine(&o, tmp.path(), id));
        }
        let coil = o.symbols["wc.coil"].clone();
        let lib = library_first(&mut o, tmp.path(), &coil);
        o.update_library(&lib, None).unwrap();
        // Two failed Undos under the same pin: the second one's backup is of the file as the
        // first left it, so the first backup (the pinned version) is the one kept.
        for _ in 0..2 {
            o.fail_part_writes = [false, true, true].into();
            assert!(!o.undo());
            let _ = o.take_undo_error();
        }
        o.refresh_library();
        o.save().unwrap();
        assert_eq!(store::read_vendored_part(&o.dir, ids[0]).unwrap(), new[0]);
        assert_eq!(o.parts[ids[0]], new[0]);
    }

    #[test]
    fn a_backup_a_later_step_made_right_is_not_written_by_a_save() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let ids = ["example.ex-ct-9-24dc", "example.ex-tb-2-5"];
        let mut new = Vec::new();
        for (n, id) in ids.iter().enumerate() {
            let tag = DeviceTag::parse(&format!("-K{}", n + 1)).unwrap();
            o.set_device_part(&tag, Some(id)).unwrap();
            new.push(changed_part_in_mine(&o, tmp.path(), id));
        }
        let coil = o.symbols["wc.coil"].clone();
        let lib = library_first(&mut o, tmp.path(), &coil);
        o.update_library(&lib, None).unwrap();
        // A failed Undo leaves a backup of the first part's file; Undo and Redo then succeed
        // and write that file themselves.
        o.fail_part_writes = [false, true, true].into();
        assert!(!o.undo());
        let _ = o.take_undo_error();
        assert!(o.undo(), "{:?}", o.take_undo_error());
        assert!(o.redo(), "{:?}", o.take_undo_error());
        // The project copy changed afterwards keeps its change through a save.
        let mut edited = new[0].clone();
        edited.description = "Changed in the project".into();
        store::write_vendored_part(&o.dir, &edited).unwrap();
        o.save().unwrap();
        assert_eq!(store::read_vendored_part(&o.dir, ids[0]).unwrap(), edited);
    }

    #[test]
    fn an_undo_whose_symbols_cannot_follow_puts_its_part_files_back() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.set_device_part(&DeviceTag::parse("-K1").unwrap(), Some(pid)).unwrap();
        let new = changed_part_in_mine(&o, tmp.path(), pid);
        let mut coil = o.symbols["wc.coil"].clone();
        coil.name = "Coil, changed".into();
        let lib = library_first(&mut o, tmp.path(), &coil);
        o.update_library(&lib, None).unwrap();
        let lock = o.project.lock.clone();
        o.fail_symbol_writes = [true].into();
        assert!(!o.undo());
        assert_eq!(o.project.lock, lock);
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), new);
        assert_eq!(o.parts[pid], new);
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), coil);
        assert!(o.undo(), "{:?}", o.take_undo_error());
        assert_ne!(store::read_vendored_part(&o.dir, pid).unwrap(), new);
    }

    #[test]
    fn an_update_takes_only_the_items_it_looked_at() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        changed_part_in_mine(&o, tmp.path(), pid);
        let old = o.symbols["wc.coil"].clone();
        // The library's coil file names another id: not an update when the update looks.
        let mut wrong = old.clone();
        wrong.id = "wc.other".into();
        let lib = library_first(&mut o, tmp.path(), &wrong);
        let file = tmp.path().join("mine/symbols/wc.coil.json");
        std::fs::write(&file, serde_json::to_string(&wrong).unwrap()).unwrap();
        // Fixed and changed right after the backups are taken.
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        o.rewrite_after_snapshot = vec![(file, serde_json::to_string(&new).unwrap())];
        let revision = o.revision();
        let done = o.update_library(&lib, None).unwrap();
        assert_eq!(done, vec![(ItemKind::Part, pid.to_owned())]);
        assert_ne!(o.revision(), revision, "checks while drawing see the update");
        assert_eq!(
            store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(),
            old,
            "the coil it did not look at is not taken"
        );
        assert_eq!(o.project.lock.symbols["wc.coil"].hash, old.hash());
    }

    #[test]
    fn an_update_takes_no_newer_version_than_it_looked_at_and_keeps_redo() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        // An edit undone before the update: Redo must still bring it back after a failed one.
        o.checkpoint();
        o.project.pages[1].title = "Renamed".into();
        assert!(o.undo());
        assert!(o.can_redo());
        let part_file = store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap();
        let part_before = std::fs::read(&part_file).unwrap();
        let mut seen = changed_part_in_mine(&o, tmp.path(), pid);
        let old = o.symbols["wc.coil"].clone();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        let lib = library_first(&mut o, tmp.path(), &new);
        // The part changes again right after the backups are taken.
        seen.description = "Changed again".into();
        o.rewrite_after_snapshot = vec![(
            tmp.path().join(format!("mine/parts/{pid}.json")),
            serde_json::to_string(&seen).unwrap(),
        )];
        let lock = o.project.lock.clone();
        let err = o.update_library(&lib, None).unwrap_err();
        assert!(
            err.contains("changed in the library while it was being updated"),
            "{err}"
        );
        assert_eq!(
            std::fs::read(&part_file).unwrap(),
            part_before,
            "the part file is as it was"
        );
        assert_eq!(
            store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(),
            old,
            "and the coil"
        );
        assert_eq!(o.project.lock, lock);
        assert!(o.can_redo(), "the earlier Redo is kept");
        assert!(o.redo());
        assert_eq!(o.project.pages[1].title, "Renamed");
    }

    #[test]
    fn an_update_that_writes_nothing_leaves_a_file_someone_else_changed_since() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        let mut seen = changed_part_in_mine(&o, tmp.path(), pid);
        let coil = o.symbols["wc.coil"].clone();
        let lib = library_first(&mut o, tmp.path(), &coil);
        // After the backups: the library part changes again, so the update stops before it
        // writes anything, and someone else saves the project's copy of the part.
        seen.description = "Changed again".into();
        let part_file = store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap();
        let mut theirs = o.parts[pid].clone();
        theirs.description = "Saved by someone else".into();
        let theirs = serde_json::to_string(&theirs).unwrap();
        o.rewrite_after_snapshot = vec![
            (
                tmp.path().join(format!("mine/parts/{pid}.json")),
                serde_json::to_string(&seen).unwrap(),
            ),
            (part_file.clone(), theirs.clone()),
        ];
        assert!(o.update_library(&lib, None).is_err());
        assert_eq!(
            std::fs::read_to_string(&part_file).unwrap(),
            theirs,
            "their change stays"
        );
        assert!(o.unrestored_files.is_empty());
    }

    #[test]
    fn a_failed_update_leaves_a_full_undo_history_and_a_saved_project_as_they_were() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        for n in 0..UNDO_STEPS {
            o.checkpoint();
            o.project.pages[1].title = format!("Step {n}");
        }
        let mut seen = changed_part_in_mine(&o, tmp.path(), pid);
        let coil = o.symbols["wc.coil"].clone();
        let lib = library_first(&mut o, tmp.path(), &coil);
        o.save().unwrap();
        let (undo, history, dropped) = (o.undo.clone(), o.history, o.undo_dropped);
        seen.description = "Changed again".into();
        o.rewrite_after_snapshot = vec![(
            tmp.path().join(format!("mine/parts/{pid}.json")),
            serde_json::to_string(&seen).unwrap(),
        )];
        assert!(o.update_library(&lib, None).is_err());
        assert_eq!(o.undo, undo, "the oldest Undo step is still there");
        assert_eq!((o.history, o.undo_dropped), (history, dropped));
        assert!(!o.dirty, "nothing to save");
    }

    #[test]
    fn an_update_whose_later_write_fails_takes_back_the_symbol_written_before_it() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        changed_part_in_mine(&o, tmp.path(), pid);
        let old = o.symbols["wc.coil"].clone();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        let lib = library_first(&mut o, tmp.path(), &new);
        // Once the backups are taken, a folder where the part file goes: symbols are taken
        // first, then this write fails.
        let part_file = store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap();
        o.block_after_snapshot = Some(part_file.clone());
        let lock = o.project.lock.clone();
        let err = o.update_library(&lib, None).unwrap_err();
        assert!(
            !err.starts_with("nothing was updated"),
            "the update itself failed: {err}"
        );
        assert_eq!(o.project.lock, lock, "the lock is as before");
        assert_eq!(o.symbols["wc.coil"], old);
        assert_eq!(
            store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(),
            old,
            "and the symbol file"
        );
        assert!(!o.can_redo());
    }

    #[test]
    fn a_symbol_file_an_update_could_not_put_back_keeps_its_old_version_until_save_writes_it() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let old = o.symbols["wc.coil"].clone();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        let lib = library_first(&mut o, tmp.path(), &new);
        o.save().unwrap();
        let symbol_file = store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap();
        let before = std::fs::read(&symbol_file).unwrap();
        // The symbol's file turns into a folder once the backups are taken: the update cannot
        // write it, nor put it back.
        o.block_after_snapshot = Some(symbol_file.clone());
        let lock = o.project.lock.clone();
        let err = o.update_library(&lib, None).unwrap_err();
        assert!(err.contains("written before the next save"), "{err}");
        assert_eq!(o.project.lock, lock);
        assert_eq!(o.symbols["wc.coil"], old, "the old version is still drawn");
        // Save refuses while the file cannot be written, and leaves the saved lock alone.
        let lock_file = std::fs::read(o.dir.join(store::LOCK_FILE)).unwrap();
        o.dirty = true;
        let err = o.save().unwrap_err();
        assert!(err.starts_with("not saved") && err.contains("wc.coil"), "{err}");
        assert_eq!(std::fs::read(o.dir.join(store::LOCK_FILE)).unwrap(), lock_file);
        // Once the folder is gone, Save writes the old file back, then the lock.
        std::fs::remove_dir(&symbol_file).unwrap();
        o.save().unwrap();
        assert_eq!(std::fs::read(&symbol_file).unwrap(), before);
        assert!(o.unrestored_files.is_empty());
        assert_eq!(o.symbols["wc.coil"], old);
    }

    #[test]
    fn a_part_file_an_update_could_not_put_back_keeps_its_old_version_until_save_writes_it() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        let old = o.parts[pid].clone();
        changed_part_in_mine(&o, tmp.path(), pid);
        let coil = o.symbols["wc.coil"].clone();
        let lib = library_first(&mut o, tmp.path(), &coil);
        o.save().unwrap();
        let part_file = store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap();
        let before = std::fs::read(&part_file).unwrap();
        o.block_after_snapshot = Some(part_file.clone());
        let err = o.update_library(&lib, None).unwrap_err();
        assert!(err.contains("written before the next save"), "{err}");
        assert_eq!(o.parts[pid], old, "the old version stays loaded");
        o.dirty = true;
        assert!(o.save().unwrap_err().contains(pid));
        // The blocker goes with the parts folder itself: Save makes the folder again.
        std::fs::remove_dir(&part_file).unwrap();
        std::fs::remove_dir(part_file.parent().unwrap()).unwrap();
        o.save().unwrap();
        assert_eq!(std::fs::read(&part_file).unwrap(), before);
    }

    #[test]
    fn a_part_waiting_to_be_put_back_marks_the_project_unsaved_and_survives_a_refresh_and_a_second_failure() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        let old = o.parts[pid].clone();
        changed_part_in_mine(&o, tmp.path(), pid);
        let coil = o.symbols["wc.coil"].clone();
        let lib = library_first(&mut o, tmp.path(), &coil);
        o.save().unwrap();
        let part_file = store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap();
        let before = std::fs::read(&part_file).unwrap();
        o.block_after_snapshot = Some(part_file.clone());
        assert!(o.update_library(&lib, None).is_err());
        assert!(o.dirty, "the file still has to be written");
        // A refresh reads the parts from disk again: the old version stays loaded.
        o.refresh_library();
        assert_eq!(o.parts[pid], old);
        // The folder goes, but the file is still missing; a second update fails the same way
        // and snapshots that absence. The first backup is kept.
        std::fs::remove_dir(&part_file).unwrap();
        o.block_after_snapshot = Some(part_file.clone());
        assert!(o.update_library(&lib, None).is_err());
        assert_eq!(o.parts[pid], old);
        std::fs::remove_dir(&part_file).unwrap();
        o.save().unwrap();
        assert_eq!(std::fs::read(&part_file).unwrap(), before);
        assert!(o.unrestored_files.is_empty());
    }

    #[test]
    fn a_file_waiting_to_be_put_back_is_let_go_once_the_lock_pins_another_version() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        changed_part_in_mine(&o, tmp.path(), pid);
        let coil = o.symbols["wc.coil"].clone();
        let lib = library_first(&mut o, tmp.path(), &coil);
        let part_file = store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap();
        o.block_after_snapshot = Some(part_file.clone());
        assert!(o.update_library(&lib, None).is_err());
        assert_eq!(o.unrestored_files.len(), 1);
        // The update goes through once the folder is gone: the old file is not written over it.
        std::fs::remove_dir(&part_file).unwrap();
        o.update_library(&lib, None).unwrap();
        let taken = std::fs::read(&part_file).unwrap();
        o.save().unwrap();
        assert_eq!(std::fs::read(&part_file).unwrap(), taken);
        assert!(o.unrestored_files.is_empty());
    }

    #[test]
    fn an_update_whose_part_file_cannot_be_read_changes_nothing_and_keeps_the_file() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        changed_part_in_mine(&o, tmp.path(), pid);
        let old = o.symbols["wc.coil"].clone();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        let lib = library_first(&mut o, tmp.path(), &new);
        // The part file is a plain, removable file whose read fails.
        let part_file = store::vendor_path(&o.dir, ItemKind::Part, pid).unwrap();
        let bytes = std::fs::read(&part_file).unwrap();
        o.unreadable = Some(part_file.clone());
        let lock = o.project.lock.clone();
        let mark = o.undo_mark();
        let err = o.update_library(&lib, None).unwrap_err();
        assert!(
            err.starts_with("nothing was updated") && err.contains("cannot be read"),
            "{err}"
        );
        assert_eq!(std::fs::read(&part_file).unwrap(), bytes, "the file is kept as it was");
        assert_eq!(o.project.lock, lock);
        assert_eq!(
            store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(),
            old,
            "no symbol written"
        );
        assert_eq!(o.undo_mark(), mark, "no undo step");
    }

    #[test]
    fn undo_after_reopening_puts_back_a_locked_symbol_no_page_uses() {
        let (tmp, mut o) = project();
        o.ensure_symbol("wc.coil").unwrap();
        o.save().unwrap();
        let old = store::read_vendored_symbol(&o.dir, "wc.coil").unwrap();
        let mut o = Open::load(&o.dir).unwrap();
        let mut new = old.clone();
        new.name = "Coil, changed".into();
        let lib = library_first(&mut o, tmp.path(), &new);
        let done = o.update_library(&lib, None).unwrap();
        assert!(done.contains(&(ItemKind::Symbol, "wc.coil".to_owned())));
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), new);
        assert!(o.undo());
        assert_eq!(o.symbols["wc.coil"], old);
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), old);
    }

    #[test]
    fn a_library_update_that_drops_a_wired_pin_is_taken_back() {
        let (tmp, mut o) = project();
        let s1 = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let s2 = o.place(1, "wc.coil", Point::new(60.0, 20.0)).unwrap();
        let pin = |item: &str| Endpoint::Pin {
            item: item.to_owned(),
            pin: "A2".into(),
        };
        o.project.pages[1].wires.push(Wire {
            id: "w1".into(),
            from: pin(&s1),
            to: pin(&s2),
            via: Vec::new(),
            number: None,
            potential: None,
            cable: None,
            route: Default::default(),
            spec: Default::default(),
        });
        let old = o.symbols["wc.coil"].clone();
        let mut new = old.clone();
        new.pins.retain(|p| p.name != "A2");
        let lib = library_first(&mut o, tmp.path(), &new);
        let before = o.project.clone();
        let err = o.update_library(&lib, None).unwrap_err();
        assert!(err.contains(":A2"), "{err}");
        assert_eq!(o.project, before, "nothing was updated");
        assert_eq!(o.symbols["wc.coil"], old);
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), old);
        assert!(!o.can_redo(), "no step is left to redo into the broken version");
    }

    #[test]
    fn a_symbol_taken_back_by_undo_is_pinned_again_when_it_is_placed() {
        let (_tmp, mut o) = project();
        let lib = wc_library::Library::open(&o.dir, &o.project).unwrap();
        o.take_saved_symbol(&lib, "wc.lamp").unwrap();
        assert!(o.symbols.contains_key("wc.lamp"));
        assert!(o.undo());
        assert!(!o.project.lock.symbols.contains_key("wc.lamp"));
        assert!(!o.symbols.contains_key("wc.lamp"), "not locked and not used: let go");
        assert!(o.redo(), "the version is kept for Redo");
        assert!(o.symbols.contains_key("wc.lamp") && o.project.lock.symbols.contains_key("wc.lamp"));
        assert!(o.undo());
        o.place(1, "wc.lamp", Point::new(20.0, 20.0)).unwrap();
        assert!(
            o.project.lock.symbols.contains_key("wc.lamp"),
            "every placed symbol is pinned"
        );
    }

    #[test]
    fn an_undo_whose_symbol_version_is_gone_says_why_the_file_could_not_be_read() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let mut new = o.symbols["wc.coil"].clone();
        new.name = "Coil, changed".into();
        o.checkpoint();
        store::write_vendored_symbol(&o.dir, &new).unwrap();
        o.project.lock.symbols.get_mut("wc.coil").unwrap().hash = new.hash();
        o.reload_symbols();
        o.symbol_versions.clear();
        let file = store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap();
        std::fs::write(&file, "not a symbol").unwrap();
        assert!(!o.undo());
        let err = o.take_undo_error().unwrap_or_default();
        let reason = err.split("cannot be read: ").nth(1).unwrap_or_default();
        assert!(!reason.trim().is_empty(), "the parse error follows: {err}");
        let parse = store::read_vendored_symbol(&o.dir, "wc.coil").unwrap_err().to_string();
        assert!(err.ends_with(&parse), "{err} / {parse}");
    }

    #[test]
    fn a_rollback_that_fails_is_written_again_by_the_next_undo() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        o.place(1, "wc.lamp", Point::new(60.0, 20.0)).unwrap();
        let (coil, lamp) = (o.symbols["wc.coil"].clone(), o.symbols["wc.lamp"].clone());
        o.checkpoint();
        let mut coil2 = coil.clone();
        coil2.name = "Coil, changed".into();
        let mut lamp2 = lamp.clone();
        lamp2.name = "Lamp, changed".into();
        for s in [&coil2, &lamp2] {
            store::write_vendored_symbol(&o.dir, s).unwrap();
            o.project.lock.symbols.get_mut(&s.id).unwrap().hash = s.hash();
        }
        o.reload_symbols();
        // Undo: the coil file is written, the lamp write fails, and putting the coil back fails.
        o.fail_symbol_writes = [false, true, true].into();
        assert!(!o.undo());
        let err = o.take_undo_error().unwrap_or_default();
        assert!(err.contains("could not be restored: wc.coil"), "{err}");
        assert_eq!(o.symbols["wc.coil"], coil2, "memory follows the lock");
        assert_eq!(
            store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(),
            coil,
            "the file does not"
        );
        // A step whose lock names what memory already holds still writes the coil file again.
        o.checkpoint();
        assert!(o.undo());
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), coil2);
        // And the step that failed now goes through.
        assert!(o.undo());
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.coil").unwrap(), coil);
        assert_eq!(store::read_vendored_symbol(&o.dir, "wc.lamp").unwrap(), lamp);
    }

    #[test]
    fn an_undo_whose_symbol_file_cannot_be_written_changes_nothing() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let mut new = o.symbols["wc.coil"].clone();
        new.name = "Coil, changed".into();
        o.checkpoint();
        store::write_vendored_symbol(&o.dir, &new).unwrap();
        o.project.lock.symbols.get_mut("wc.coil").unwrap().hash = new.hash();
        o.reload_symbols();
        let after = o.project.clone();
        // A folder where the symbol file goes: the write fails, even for root.
        let file = store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil").unwrap();
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        assert!(!o.undo());
        assert!(o.take_undo_error().is_some_and(|e| e.contains("wc.coil")));
        assert_eq!(o.project, after, "the project stays as it was");
        assert_eq!(o.symbols["wc.coil"], new);
        assert!(o.can_undo() && !o.can_redo(), "the step is still there to undo");
    }

    #[test]
    fn saving_a_symbol_leaves_a_part_with_the_same_id_alone() {
        let (tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let pid = "example.ex-ct-9-24dc";
        o.ensure_part(pid).unwrap();
        let part = store::read_vendored_part(&o.dir, pid).unwrap();
        let locked = o.project.lock.parts[pid].clone();
        // A library folder ahead of the starter one with a changed part and a symbol, same id.
        let mine = tmp.path().join("mine");
        std::fs::create_dir_all(mine.join("symbols")).unwrap();
        std::fs::create_dir_all(mine.join("parts")).unwrap();
        let mut changed = part.clone();
        changed.description = "Changed in the library".into();
        std::fs::write(
            mine.join(format!("parts/{pid}.json")),
            serde_json::to_string(&changed).unwrap(),
        )
        .unwrap();
        let mut sym = o.symbols["wc.coil"].clone();
        sym.id = pid.into();
        std::fs::write(
            mine.join(format!("symbols/{pid}.json")),
            serde_json::to_string(&sym).unwrap(),
        )
        .unwrap();
        o.project.meta.libraries.insert(
            0,
            LibrarySource {
                name: "mine".into(),
                kind: "folder".into(),
                location: mine.display().to_string(),
                reference: None,
            },
        );
        let lib = wc_library::Library::open(&o.dir, &o.project).unwrap();
        o.take_saved_symbol(&lib, pid).unwrap();
        assert!(o.project.lock.symbols.contains_key(pid));
        assert_eq!(o.project.lock.parts[pid], locked, "the part's lock entry is unchanged");
        assert_eq!(store::read_vendored_part(&o.dir, pid).unwrap(), part, "and its copy");
    }

    #[test]
    fn a_symbol_save_that_fails_leaves_the_history_alone() {
        let (_tmp, mut o) = project();
        o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let before = o.project.clone();
        let steps = o.undo.len();
        let lib = wc_library::Library::open(&o.dir, &o.project).unwrap();
        assert!(o.take_saved_symbol(&lib, "no.such-symbol").is_err());
        assert_eq!(o.project, before, "the project stays as it was");
        assert_eq!(o.undo.len(), steps, "and no empty undo step was added");
    }

    fn fields(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[cfg(unix)]
    #[test]
    fn a_logo_is_not_copied_through_a_pictures_link_out_of_the_project() {
        let (tmp, mut o) = project();
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, o.dir.join(wc_core::Logo::DIR)).unwrap();
        let src = tmp.path().join("logo.png");
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3])))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        std::fs::write(&src, png).unwrap();
        let err = o.set_logo_from(&src).unwrap_err();
        assert!(err.contains("out of the project"), "{err}");
        assert_eq!(
            std::fs::read_dir(&outside).unwrap().count(),
            0,
            "nothing written outside"
        );
        assert!(o.project.meta.logo.is_none());
    }

    #[test]
    fn a_logo_that_cannot_be_shown_keeps_the_reason() {
        let (_tmp, mut o) = project();
        let ctx = egui::Context::default();
        assert!(o.picture_texture(&ctx, "pictures/none.png").is_none());
        assert_eq!(
            o.picture_problems.get("pictures/none.png").map(String::as_str),
            Some("the file is missing")
        );
        std::fs::create_dir_all(o.dir.join("pictures")).unwrap();
        std::fs::write(o.dir.join("pictures/bad.png"), "not a picture").unwrap();
        assert!(o.picture_texture(&ctx, "pictures/bad.png").is_none());
        assert!(o.picture_problems["pictures/bad.png"].starts_with("not a picture"));
        assert!(o.picture_texture(&ctx, "../x.png").is_none());
        assert!(o.picture_problems["../x.png"].contains("not inside the project"));
    }

    #[test]
    fn a_picked_logo_is_copied_into_the_project_as_one_undo_step() {
        let (tmp, mut o) = project();
        let png = |rgb: [u8; 3]| {
            let mut out = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3, 2, image::Rgb(rgb)))
                .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
                .unwrap();
            out
        };
        let src = tmp.path().join("Our Logo.png");
        std::fs::write(&src, png([255, 0, 0])).unwrap();
        assert_eq!(o.set_logo_from(&src).unwrap(), "pictures/Our_Logo.png");
        assert_eq!(
            std::fs::read(o.dir.join("pictures/Our_Logo.png")).unwrap(),
            png([255, 0, 0])
        );
        // The same picture again reuses the file; another one of the same name gets its own.
        assert_eq!(o.set_logo_from(&src).unwrap(), "pictures/Our_Logo.png");
        std::fs::write(&src, png([0, 0, 255])).unwrap();
        assert_eq!(o.set_logo_from(&src).unwrap(), "pictures/Our_Logo-2.png");
        assert_eq!(
            std::fs::read(o.dir.join("pictures/Our_Logo.png")).unwrap(),
            png([255, 0, 0])
        );
        // Moving it on the cover is kept when another picture is picked.
        o.checkpoint();
        o.set_logo_place(Point::new(10.0, 10.0), Point::new(40.0, 30.0));
        std::fs::write(&src, png([0, 255, 0])).unwrap();
        o.set_logo_from(&src).unwrap();
        let logo = o.project.meta.logo.clone().unwrap();
        assert_eq!(logo.cover, Some((Point::new(10.0, 10.0), Point::new(40.0, 30.0))));
        assert!(o.undo());
        assert_eq!(o.project.meta.logo.as_ref().unwrap().file, "pictures/Our_Logo-2.png");
        o.remove_logo();
        assert!(o.project.meta.logo.is_none());
        assert!(o.undo(), "removing is one step too");
        assert!(o.project.meta.logo.is_some());
        // Saved with the project and loaded again.
        o.save().unwrap();
        let again = Open::load(&o.dir).unwrap();
        assert_eq!(again.project.meta.logo, o.project.meta.logo);
        // Not a picture: refused, nothing changes.
        let txt = tmp.path().join("logo.txt");
        std::fs::write(&txt, "hello").unwrap();
        let before = o.project.meta.logo.clone();
        assert!(o.set_logo_from(&txt).unwrap_err().contains("PNG or JPEG"));
        assert_eq!(o.project.meta.logo, before);
    }

    #[test]
    fn a_change_made_without_changes_leaves_no_trace_in_the_undo_history() {
        let (_tmp, mut o) = project();
        o.checkpoint();
        o.project.pages[0].title = "First".into();
        assert!(o.undo());
        o.dirty = false;
        let (project, mark) = (o.project.clone(), o.undo_mark());
        assert!(o.can_redo());
        o.without_changes(|o| {
            o.checkpoint();
            o.project.pages[0].title = "Second".into();
        });
        assert_eq!(o.project, project);
        assert_eq!(o.undo_mark(), mark, "no undo step");
        assert!(o.can_redo(), "redo kept");
        assert!(!o.dirty, "not marked as changed");
    }

    #[test]
    fn the_cover_form_shows_changes_as_typed_and_keeps_them_as_one_undo_step() {
        let (_tmp, mut o) = project();
        assert_eq!(o.cover_page(), Ok(0), "a new project's first page is its cover");
        assert!(!o.can_undo(), "nothing to create");
        o.start_live_form();
        for typed in ["A", "AC", "ACME"] {
            o.set_project_props_live("Test", &fields(&[("customer", typed)]));
            assert_eq!(o.project.meta.title_block["customer"], typed, "shown at once");
        }
        o.set_project_props_live("", &fields(&[("customer", "ACME")]));
        assert_eq!(o.project.meta.name, "Test", "an empty name is still being typed");
        o.set_project_props_live("Pumps", &fields(&[("customer", "ACME"), ("revision", "")]));
        assert_eq!(o.project.meta.name, "Pumps");
        assert_eq!(o.project.meta.title_block.len(), 1, "empty fields are dropped");
        assert!(o.undo());
        assert_eq!(o.project.meta.name, "Test");
        assert!(
            o.project.meta.title_block.is_empty(),
            "one undo takes the whole form back"
        );
        assert!(!o.can_undo());

        // Cancel takes the typed changes back and leaves nothing to redo.
        let since = o.start_live_form();
        let before = o.props_snapshot();
        o.set_project_props_live("Test", &fields(&[("customer", "Other")]));
        o.revert_live(since, &before);
        assert!(o.project.meta.title_block.is_empty());
        assert!(!o.can_redo());
        o.revert_live(since, &before);
        assert!(!o.can_undo(), "nothing changed, nothing undone");
    }

    #[test]
    fn the_template_form_is_one_undo_step_and_cancel_puts_it_back() {
        let (_tmp, mut o) = project();
        o.start_live_form();
        let before = o.project.meta.template.clone();
        let mut draft = before.clone();
        for columns in [11, 12] {
            draft.columns = columns;
            o.set_template_live(&draft);
            assert_eq!(o.project.meta.template.columns, columns, "shown at once");
        }
        draft.boxes[0].label = "Client".into();
        o.set_template_live(&draft);
        // Saved with the project and read back.
        o.save().unwrap();
        let text = std::fs::read_to_string(o.dir.join("project.toml")).unwrap();
        assert!(text.contains("[template]") && text.contains("Client"), "{text}");
        assert_eq!(store::load(&o.dir).unwrap().meta.template, draft);
        assert!(o.undo(), "one step");
        assert_eq!(o.project.meta.template, before);
        assert!(!o.can_undo());

        // Cancel takes the form's changes back and leaves nothing to redo.
        let since2 = o.start_live_form();
        draft.columns = 8;
        o.set_template_live(&draft);
        o.revert_template(since2, &before);
        assert_eq!(o.project.meta.template, before);
        assert!(!o.can_redo() && !o.can_undo());
    }

    #[test]
    fn the_racks_form_saves_a_rack_with_two_cards_and_keeps_unknown_keys() {
        use wc_core::plc_rack::PlcCard;
        let (_tmp, mut o) = project();
        // A rack written by hand, with a key this version does not know.
        let toml = std::fs::read_to_string(o.dir.join("project.toml")).unwrap();
        let hand = format!("{toml}\n[[rack]]\nname = \"-A1\"\nlocation = \"cabinet 2\"\n");
        std::fs::write(o.dir.join("project.toml"), hand).unwrap();
        o.project = store::load(&o.dir).unwrap();
        let before = o.project.meta.racks.clone();
        assert_eq!(before.len(), 1);

        o.start_live_form();
        let mut draft = before.clone();
        let card = |slot, kind: &str, start: &str| PlcCard {
            slot,
            kind: kind.into(),
            channels: 16,
            start: start.into(),
            ..PlcCard::default()
        };
        draft[0].cards.push(card(1, "DI 16x24 V DC", "%IX0.0"));
        o.set_racks_live(&draft);
        draft[0].cards.push(card(2, "DO 16x24 V DC", "%QX0.0"));
        draft[0].description = "Main PLC".into();
        o.set_racks_live(&draft);
        assert_eq!(o.project.meta.racks, draft, "shown at once");
        // The PLC cards report follows.
        let inputs = wc_report::Inputs {
            project: &o.project,
            symbols: &o.symbols,
            parts: &o.parts,
        };
        let cards = wc_report::build(wc_report::Kind::PlcCards, &inputs);
        assert_eq!(cards.rows.len(), 32, "a row per channel of both cards");

        o.save().unwrap();
        let text = std::fs::read_to_string(o.dir.join("project.toml")).unwrap();
        assert!(text.contains("location = \"cabinet 2\""), "{text}");
        assert!(text.contains("%QX0.0") && text.contains("Main PLC"), "{text}");
        assert_eq!(store::load(&o.dir).unwrap().meta.racks, draft);
        assert!(o.undo(), "one step");
        assert_eq!(o.project.meta.racks, before);

        // Cancel takes the form's changes back.
        let since2 = o.start_live_form();
        o.set_racks_live(&draft);
        o.revert_racks(since2, &before);
        assert_eq!(o.project.meta.racks, before);
        assert!(!o.can_redo());
    }

    /// The cover form's flow in `main.rs`: it refreshes its buffer when the history count moved
    /// without it, and records the count after each of its own edits.
    #[test]
    fn the_form_buffer_follows_an_undo_so_a_later_edit_keeps_it() {
        let (_tmp, mut o) = project();
        o.start_live_form();
        let mut buffer = fields(&[("customer", "A")]);
        o.set_project_props_live("Test", &buffer);
        let mut seen = o.history_count();
        assert!(
            o.props_if_changed(&mut seen).is_none(),
            "the form's own edit is not news"
        );
        // Undo from the toolbar while the form is open: the form reloads what it shows.
        assert!(o.undo());
        let now = o.props_if_changed(&mut seen).unwrap();
        buffer = now.title_block.into_iter().collect();
        // Editing another field now does not bring customer A back.
        buffer.push(("revision".into(), "B".into()));
        o.set_project_props_live("Test", &buffer);
        assert!(!o.project.meta.title_block.contains_key("customer"));
        assert_eq!(o.project.meta.title_block["revision"], "B");
    }

    #[test]
    fn cancel_restores_opening_props_after_undo_and_other_property_edits() {
        let (_tmp, mut o) = project();
        o.set_project_props("Opening", &fields(&[("customer", "Opening")]), None, None)
            .unwrap();
        let since = o.start_live_form();
        let before = o.props_snapshot();
        o.set_project_props_live("Form", &fields(&[("customer", "Form")]));
        assert!(o.undo());
        assert!(o.undo());
        o.set_project_props("First", &fields(&[("customer", "First")]), None, None)
            .unwrap();
        o.set_project_props("Second", &fields(&[("customer", "Second")]), None, None)
            .unwrap();
        // The last step looks like a form step but is not the form's: Cancel still restores
        // what the form opened with.
        assert_eq!(o.undo_mark(), since + 1);
        assert!(o.last_step_changed_only_props());
        o.revert_live(since, &before);
        assert_eq!(o.props_snapshot(), before);
    }

    #[test]
    fn form_edits_after_an_undo_stay_one_step() {
        let (_tmp, mut o) = project();
        o.checkpoint();
        o.project.pages[0].title = "Edited".into();
        o.start_live_form();
        assert!(o.undo());
        o.set_project_props_live("Test", &fields(&[("customer", "A")]));
        o.set_project_props_live("Test", &fields(&[("customer", "B")]));
        assert!(o.undo());
        assert!(o.project.meta.title_block.is_empty(), "one Undo takes both edits back");
        assert!(!o.can_undo());
    }

    #[test]
    fn a_form_edit_after_another_change_is_its_own_undo_step() {
        let (_tmp, mut o) = project();
        o.start_live_form();
        o.set_project_props_live("Test", &fields(&[("customer", "A")]));
        // Another change while the form is open.
        o.checkpoint();
        o.project.pages[0].title = "Added meanwhile".into();
        o.set_project_props_live("Test", &fields(&[("customer", "B")]));
        // OK, then Undo: only the second form edit goes back.
        assert!(o.undo());
        assert_eq!(o.project.meta.title_block["customer"], "A");
        assert_eq!(o.project.pages[0].title, "Added meanwhile");

        // A properties change made another way in between is its own step too.
        o.form_step = None;
        o.start_live_form();
        o.set_project_props_live("Test", &fields(&[("customer", "A")]));
        o.set_project_props("Test", &fields(&[("customer", "X")]), None, None)
            .unwrap();
        o.set_project_props_live("Test", &fields(&[("customer", "B")]));
        assert!(o.undo());
        assert_eq!(o.project.meta.title_block["customer"], "X");
    }

    #[test]
    fn cancel_puts_the_fields_back_when_the_form_step_is_not_the_last_or_was_folded_away() {
        let (_tmp, mut o) = project();
        // A page edit made while the form is open stays; only the fields go back.
        let since = o.start_live_form();
        let before = o.props_snapshot();
        o.set_project_props_live("Test", &fields(&[("customer", "ACME")]));
        o.checkpoint();
        o.project.pages[0].title = "Edited meanwhile".into();
        o.revert_live(since, &before);
        assert!(o.project.meta.title_block.is_empty());
        assert_eq!(o.project.pages[0].title, "Edited meanwhile");

        // An assistant turn that began before the form folds the form's step into its own.
        let turn = o.undo_mark();
        o.checkpoint();
        o.project.pages[0].title = "By the assistant".into();
        let since = o.start_live_form();
        let before = o.props_snapshot();
        o.set_project_props_live("Test", &fields(&[("customer", "ACME")]));
        o.squash_undo(turn);
        assert!(o.undo_mark() <= since);
        o.revert_live(since, &before);
        assert!(o.project.meta.title_block.is_empty(), "the form's change is taken back");
        assert_eq!(o.project.pages[0].title, "By the assistant", "the assistant's stays");
    }

    #[test]
    fn a_project_without_a_cover_gets_one_in_front() {
        let (_tmp, mut o) = project();
        o.project.pages[0].cover = false;
        o.project.pages[0].title = "Overview".into();
        let i = o.cover_page().unwrap();
        let cover = &o.project.pages[i];
        assert!(cover.cover);
        assert_eq!((i, cover.number.as_str(), cover.title.as_str()), (0, "0", "Cover"));
        assert_eq!(o.project.pages.len(), 3);
        assert_eq!(o.cover_page(), Ok(0), "found again, not made twice");
        assert!(o.undo());
        assert_eq!(o.project.pages.len(), 2, "making it is one undo step");

        // An empty first page titled "Cover" (how projects started) becomes the cover.
        o.project.pages[0].title = "Cover".into();
        assert_eq!(o.cover_page(), Ok(0));
        assert_eq!(o.project.pages.len(), 2);
        assert!(o.project.pages[0].cover);

        // Its copy is a normal page: a project keeps one cover.
        let copy = o.duplicate_page(0).unwrap();
        assert!(!o.project.pages[copy].cover);
    }

    #[test]
    fn a_first_page_titled_cover_with_a_drawing_on_it_is_kept_and_a_cover_goes_in_front() {
        let (_tmp, mut o) = project();
        o.project.pages[0].cover = false;
        o.project.pages[0].graphics.push(wc_core::geom::Shape::Line {
            a: Point::new(0.0, 0.0),
            b: Point::new(10.0, 0.0),
        });
        let i = o.cover_page().unwrap();
        assert_eq!((i, o.project.pages[i].number.as_str()), (0, "0"));
        assert!(!o.project.pages[1].cover, "the drawn page stays as it was");
    }

    #[test]
    fn a_page_numbered_zero_keeps_its_place_and_no_cover_is_made() {
        let (_tmp, mut o) = project();
        o.project.pages[0].cover = false;
        o.project.pages[0].title = "Index".into();
        o.project.pages[0].number = "0".into();
        o.project.sort_pages();
        let before = o.project.clone();
        assert!(o.cover_page().unwrap_err().contains("\"0\""));
        assert_eq!(o.project, before);
        assert!(!o.can_undo(), "nothing changed");
    }

    #[test]
    fn changing_the_route_redraws_selected_wires_in_one_undo_step() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let k1 = o.place(page, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let k2 = o.place(page, "wc.coil", Point::new(60.0, 80.0)).unwrap();
        let pin = |item: &str, pin: &str| Endpoint::Pin {
            item: item.into(),
            pin: pin.into(),
        };
        o.add_wire(page, pin(&k1, "A2"), pin(&k2, "A1"), Vec::new());
        o.add_wire(
            page,
            pin(&k2, "A2"),
            Endpoint::Point {
                at: Point::new(100.0, 140.0),
            },
            Vec::new(),
        );
        let ids: Vec<String> = o.project.pages[page].wires.iter().map(|w| w.id.clone()).collect();
        let before = o.project.pages.clone();
        assert_eq!(o.set_route(page, &ids, Route::Sloped), 2);
        for w in &o.project.pages[page].wires {
            assert_eq!(w.route, Route::Sloped);
            let a = o.end_pos(&o.project.pages[page], &w.from).unwrap();
            let b = o.end_pos(&o.project.pages[page], &w.to).unwrap();
            let expected = route::via(
                a,
                o.end_axis(&o.project.pages[page], &w.from),
                b,
                o.end_axis(&o.project.pages[page], &w.to),
                Route::Sloped,
            );
            assert_eq!(w.via, expected);
            assert!(!w.via.is_empty(), "the ends are not lined up, so the wire bends");
        }
        assert_eq!(o.set_route(page, &ids, Route::Sloped), 0, "nothing left to change");
        assert!(o.undo());
        assert_eq!(o.project.pages, before, "one undo puts both wires back");
    }

    #[test]
    fn a_rotated_coil_leaves_its_pins_sideways() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let k1 = o.place(page, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let end = Endpoint::Pin {
            item: k1.clone(),
            pin: "A1".into(),
        };
        assert_eq!(o.end_axis(&o.project.pages[page], &end), Some(Axis::Vertical));
        o.project.pages[page].symbols[0].rotation = Rotation::R90;
        assert_eq!(o.end_axis(&o.project.pages[page], &end), Some(Axis::Horizontal));
        let free = Endpoint::Point {
            at: Point::new(0.0, 0.0),
        };
        assert_eq!(o.end_axis(&o.project.pages[page], &free), None);
    }

    #[test]
    fn auto_wiring_is_part_of_the_placing_undo_step() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        o.place(page, "wc.contact.no", Point::new(50.0, 20.0)).unwrap();
        let before = o.project.pages.clone();
        let coil = o.place(page, "wc.coil", Point::new(50.0, 60.0)).unwrap();
        let items = std::iter::once(coil.clone()).collect();
        assert_eq!(o.auto_wire(page, &items), 1);
        let w = &o.project.pages[page].wires[0];
        assert_eq!(
            (&w.from, &w.to),
            (
                &Endpoint::Pin {
                    item: coil,
                    pin: "A1".into()
                },
                &Endpoint::Pin {
                    item: "s1".into(),
                    pin: "14".into()
                }
            )
        );
        assert_eq!(o.auto_wire(page, &items), 0, "a wired pin is not wired again");
        assert!(o.undo());
        assert_eq!(o.project.pages, before, "one undo removes the symbol and its wire");
    }

    #[test]
    fn the_symbol_list_keeps_and_searches_descriptions() {
        let (_tmp, o) = project();
        let coil = o.library.iter().find(|e| e.id == "wc.coil").unwrap();
        assert!(coil.description.contains("relay"), "{coil:?}");
        // Every word must appear, in any order and case, across id, name and description.
        assert!(coil.matches(""));
        assert!(coil.matches("CONTACTOR relay"));
        assert!(coil.matches("coil contactor"));
        assert!(!coil.matches("contactor timer"));
        let hits: Vec<&str> = o
            .library
            .iter()
            .filter(|e| e.matches("relay contactor"))
            .map(|e| e.id.as_str())
            .collect();
        assert!(hits.contains(&"wc.coil"), "{hits:?}");
        // After placing, the vendored copy keeps its description too.
        let mut o = o;
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        o.refresh_library();
        let coil = o.library.iter().find(|e| e.id == "wc.coil").unwrap();
        assert!(coil.matches("relay"));
    }

    #[test]
    fn the_symbol_list_searches_category_and_letter_code() {
        let (_tmp, o) = project();
        let breaker = o.library.iter().find(|e| e.id == "wc.breaker.1p").unwrap();
        // Neither word is in its id, name or description, only in its category and letter code.
        let text = format!("{} {} {}", breaker.id, breaker.name, breaker.description).to_lowercase();
        assert!(!text.contains("protection") && !text.contains('q'), "{text}");
        assert!(breaker.matches("protection Q"), "{breaker:?}");
        assert!(breaker.matches("q breaker"));
        assert!(!breaker.matches("protection relays"));
    }

    #[test]
    fn placing_vendors_the_symbol_and_tags_the_device() {
        let (_tmp, mut o) = project();
        assert!(o.library.iter().any(|e| e.id == "wc.coil"));
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let id = o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        assert!(o.symbols.contains_key("wc.coil"));
        assert!(o.project.lock.symbols.contains_key("wc.coil"));
        let tag = o.project.pages[page].symbol(&id).unwrap().tag.clone().unwrap();
        assert_eq!(tag.to_string(), "-K1");
        // The next one on a page with =A+B tags inherits them and skips the used number.
        o.project.pages[page].symbols[0].tag = Some(DeviceTag::parse("=PUMP+CAB1-K1").unwrap());
        let id2 = o.place(page, "wc.coil", Point::new(80.0, 50.0)).unwrap();
        assert_eq!(
            o.project.pages[page]
                .symbol(&id2)
                .unwrap()
                .tag
                .as_ref()
                .unwrap()
                .to_string(),
            "=PUMP+CAB1-K2"
        );
    }

    #[test]
    fn placing_skips_the_tag_of_a_deleted_symbols_device_data() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let id = o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        o.project
            .devices
            .get_mut(&DeviceTag::parse("-K1").unwrap())
            .unwrap()
            .description = "old".into();
        o.project.pages[page].remove_symbols_and_wires(&[id.as_str()], &[]);
        let id2 = o.place(page, "wc.coil", Point::new(80.0, 50.0)).unwrap();
        let tag = o.project.pages[page].symbol(&id2).unwrap().tag.clone().unwrap();
        // -K1 keeps its device data, so the new coil does not take it over.
        assert_eq!(tag.to_string(), "-K2");
        assert_eq!(o.project.devices[&tag].description, "");
    }

    #[test]
    fn undo_redo_and_delete_drop_attached_wires() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let a = o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        o.add_wire(
            page,
            Endpoint::Pin {
                item: a.clone(),
                pin: "A1".into(),
            },
            Endpoint::Point {
                at: Point::new(50.0, 20.0),
            },
            vec![],
        );
        assert_eq!(o.project.pages[page].wires.len(), 1);
        o.delete_many(page, &[Selection::Symbol(a)]);
        assert!(o.project.pages[page].symbols.is_empty());
        assert!(o.project.pages[page].wires.is_empty());
        assert!(o.undo());
        assert_eq!(o.project.pages[page].wires.len(), 1);
        assert!(o.redo());
        assert!(o.project.pages[page].wires.is_empty());
        // Saving writes the page and clears the dirty flag.
        o.save().unwrap();
        assert!(!o.dirty);
        let again = Open::load(&o.dir).unwrap();
        assert!(again.project.pages[page].symbols.is_empty());
    }

    #[test]
    fn hit_testing_finds_symbols_wires_and_pins() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let a = o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        o.add_wire(
            page,
            Endpoint::Point {
                at: Point::new(100.0, 20.0),
            },
            Endpoint::Point {
                at: Point::new(100.0, 80.0),
            },
            vec![],
        );
        let pg = o.project.pages[page].clone();
        assert_eq!(
            o.hit(&pg, Point::new(50.0, 50.0), 1.0),
            Some(Selection::Symbol(a.clone()))
        );
        assert!(matches!(
            o.hit(&pg, Point::new(100.5, 40.0), 1.0),
            Some(Selection::Wire(_))
        ));
        assert_eq!(o.hit(&pg, Point::new(150.0, 150.0), 1.0), None);
        let a1 = o.pin_pos(&pg, &a, "A1").unwrap();
        let (item, pin, _) = o.pin_near(&pg, a1 + Point::new(0.5, 0.5), 2.0).unwrap();
        assert_eq!((item.as_str(), pin.as_str()), (a.as_str(), "A1"));
    }

    #[test]
    fn a_page_someone_else_has_open_is_neither_deleted_nor_written_on() {
        // A Delete page or Text dialog confirmed after the page became held changes nothing.
        let (_tmp, mut o) = project();
        let id = o.project.pages[1].id.clone();
        o.held_by_others.insert(id, "Ola".into());
        let before = o.project.clone();
        let text = wc_core::geom::Shape::Text {
            at: wc_core::geom::Point::new(10.0, 10.0),
            text: "Note".into(),
            height: 3.5,
            align: wc_core::geom::Align::Left,
            angle: 0.0,
        };
        assert!(o.delete_page(1).is_err_and(|e| e.contains("Ola")));
        assert!(o.add_graphic(1, text.clone()).is_none());
        assert!(o.held_reason(1).is_some_and(|e| e.contains("Ola")));
        // Nor renumbered by moving its neighbour past it, nor given a macro or a paste.
        assert_eq!(o.move_page(0, false), None);
        let path = o.dir.join("missing.wcmacro");
        assert!(o.insert_macro(1, &path, None).is_err_and(|e| e.contains("Ola")));
        let clip = crate::edit::Clipboard {
            graphics: vec![text.clone()],
            ..Default::default()
        };
        assert!(
            o.paste(1, &clip, wc_core::geom::Point::new(0.0, 0.0))
                .unwrap()
                .is_empty()
        );
        assert_eq!(o.project, before);
        assert!(!o.can_undo(), "nothing was recorded");
        // The other page is not held.
        assert!(o.held_reason(0).is_none());
        assert!(o.add_graphic(0, text).is_some());
    }

    #[test]
    fn a_pages_own_title_fields_are_one_undo_step_and_saved() {
        let (_tmp, mut o) = project();
        o.project.meta.title_block.insert("revision".into(), "A".into());
        let p = o.project.pages[1].clone();
        let mut fields = BTreeMap::new();
        fields.insert("revision".to_owned(), " B ".to_owned());
        fields.insert("checked_by".to_owned(), "OH".to_owned());
        let i = o
            .set_page_props(1, &p.number, "Renamed", p.size, p.overview, Some(&fields))
            .unwrap();
        let page = &o.project.pages[i];
        assert_eq!(page.title_field(&o.project.meta, "revision"), "B");
        assert_eq!(page.title_block["checked_by"], "OH");
        o.save().unwrap();
        let disk = store::load(&o.dir).unwrap();
        assert_eq!(
            disk.pages[i].title_block, o.project.pages[i].title_block,
            "saved in the page file"
        );

        // Clearing a field falls back to the project's; a key left out is kept.
        let mut clear = BTreeMap::new();
        clear.insert("revision".to_owned(), String::new());
        o.set_page_props(i, &p.number, "Renamed", p.size, p.overview, Some(&clear))
            .unwrap();
        let page = &o.project.pages[i];
        assert_eq!(page.title_field(&o.project.meta, "revision"), "A");
        assert_eq!(page.title_block.get("checked_by").map(String::as_str), Some("OH"));
        // The same values again change nothing and record nothing.
        let mark = o.undo_mark();
        o.set_page_props(i, &p.number, "Renamed", p.size, p.overview, Some(&clear))
            .unwrap();
        assert_eq!(o.undo_mark(), mark);

        // Each change was one undo step: title and fields go back together.
        assert!(o.undo());
        assert_eq!(
            o.project.pages[i].title_block.get("revision").map(String::as_str),
            Some("B")
        );
        assert!(o.undo());
        assert_eq!(o.project.pages[i].title, p.title);
        assert!(o.project.pages[i].title_block.is_empty());
    }

    #[test]
    fn pages_can_be_renamed_moved_duplicated_and_deleted() {
        let (_tmp, mut o) = project();
        assert_eq!(o.project.pages.len(), 2);
        // Renumbering resorts; a taken number is refused.
        assert!(o.set_page_props(0, "2", "Cover", PageSize::A3, false, None).is_err());
        let i = o
            .set_page_props(0, "10", "Cover sheet", PageSize::A4, true, None)
            .unwrap();
        assert!(o.project.pages[i].overview);
        assert_eq!(i, 1);
        assert_eq!(o.project.pages[1].title, "Cover sheet");
        assert_eq!(o.project.pages[1].size, PageSize::A4);
        // Moving swaps numbers with the neighbour.
        assert_eq!(o.move_page(1, true), Some(0));
        assert_eq!(o.project.pages[0].number, "2");
        assert_eq!(o.project.pages[0].id, "p0001");
        assert_eq!(o.move_page(0, true), None);
        // Duplicating gives the copy new tags for the same devices.
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let a = o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        let old = o.project.pages[page].symbol(&a).unwrap().tag.clone().unwrap();
        o.project.devices.get_mut(&old).unwrap().description = "Pump contactor".into();
        let c = o.duplicate_page(page).unwrap();
        let new = o.project.pages[c].symbols[0].tag.clone().unwrap();
        assert_ne!(new, old);
        assert_eq!(new.letter_code(), old.letter_code());
        assert_eq!(o.project.devices[&new].description, "Pump contactor");
        assert_eq!(o.project.pages.len(), 3);
        // Delete down to one page, which stays.
        o.delete_page(c).unwrap();
        o.delete_page(0).unwrap();
        assert!(o.delete_page(0).is_err());
        assert_eq!(o.project.pages.len(), 1);
        o.save().unwrap();
        assert_eq!(Open::load(&o.dir).unwrap().project.pages.len(), 1);
    }

    #[test]
    fn wires_join_and_leave_cables() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let free = |x| Endpoint::Point {
            at: Point::new(x, 10.0),
        };
        o.add_wire(page, free(10.0), free(20.0), vec![]);
        let id = o.project.pages[page].wires[0].id.clone();
        let w1 = DeviceTag::parse("-W1").unwrap();
        o.set_wire_cable(
            page,
            &id,
            Some(wc_core::CableCore {
                cable: w1.clone(),
                core: "BN".into(),
            }),
        );
        assert!(o.project.devices.contains_key(&w1));
        assert_eq!(o.cables(), vec![w1]);
        o.set_wire_cable(page, &id, None);
        assert!(o.cables().is_empty());
        assert!(o.undo());
        assert_eq!(
            o.project.pages[page].wires[0].cable.as_ref().map(|c| c.core.as_str()),
            Some("BN")
        );
    }

    #[test]
    fn jumpers_are_added_and_removed_in_one_undo_step_each() {
        let (_tmp, mut o) = project();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mark = o.undo_mark();
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        assert_eq!(o.undo_mark(), mark + 1);
        o.add_jumper(&x1, &["2".into(), "1".into()]).unwrap();
        assert_eq!(o.undo_mark(), mark + 1, "the same jumper again is no change");
        assert!(o.add_jumper(&x1, &["1".into()]).is_err());
        o.save().unwrap();
        assert_eq!(Open::load(&o.dir).unwrap().project.meta.jumpers.len(), 1);
        assert!(o.remove_from_jumper(&x1, "1"));
        assert!(o.project.meta.jumpers.is_empty());
        assert!(!o.remove_from_jumper(&x1, "1"));
        assert!(o.undo());
        assert_eq!(o.project.meta.jumpers.len(), 1);
    }

    #[test]
    fn the_jumper_button_is_offered_only_when_it_changes_something() {
        let (_tmp, mut o) = project();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let v = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(
            o.jumper_offer(&x1, &v(&["1", "1"])).is_err(),
            "two placements of one terminal"
        );
        assert_eq!(o.jumper_offer(&x1, &v(&["1", "2"])), Ok(true));
        o.add_jumper(&x1, &v(&["1", "2"])).unwrap();
        assert_eq!(o.jumper_offer(&x1, &v(&["1", "2"])), Ok(false), "already jumpered");
        assert_eq!(o.jumper_offer(&x1, &v(&["2", "3"])), Ok(true));
    }

    #[test]
    fn several_wires_get_a_section_colour_and_type_in_one_step() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let free = |x| Endpoint::Point {
            at: Point::new(x, 10.0),
        };
        o.add_wire(page, free(10.0), free(20.0), vec![]);
        o.add_wire(page, free(30.0), free(40.0), vec![]);
        let ids: Vec<String> = o.project.pages[page].wires.iter().map(|w| w.id.clone()).collect();
        let mark = o.undo_mark();
        assert_eq!(o.set_wire_spec(page, &ids, SpecPart::Colour, Some("BU".into())), 2);
        assert_eq!(o.undo_mark(), mark + 1, "one undo step for both");
        assert_eq!(
            o.set_wire_spec(page, &ids, SpecPart::Colour, Some("BU".into())),
            0,
            "no change, no step"
        );
        assert_eq!(o.undo_mark(), mark + 1);
        o.set_wire_spec(page, &ids[..1], SpecPart::Section, Some("1.5 mm²".into()));
        let w = &o.project.pages[page].wires;
        assert_eq!(
            (w[0].spec.colour.as_deref(), w[1].spec.colour.as_deref()),
            (Some("BU"), Some("BU"))
        );
        assert_eq!(
            (w[0].spec.section.as_deref(), w[1].spec.section.as_deref()),
            (Some("1.5 mm²"), None)
        );
        o.save().unwrap();
        let back = Open::load(&o.dir).unwrap();
        assert_eq!(
            back.project.pages[page].wires[0].spec,
            o.project.pages[page].wires[0].spec
        );
        // Clearing a part the wires differ in clears it on all of them.
        assert_eq!(o.set_wire_spec(page, &ids, SpecPart::Section, None), 1);
        assert!(o.project.pages[page].wires.iter().all(|w| w.spec.section.is_none()));
        assert!(o.undo() && o.undo() && o.undo());
        assert!(o.project.pages[page].wires.iter().all(|w| w.spec.is_empty()));
    }

    #[test]
    fn a_jumper_gets_a_part_from_the_library_as_one_undo_step() {
        let (_tmp, mut o) = project();
        let x1 = DeviceTag::parse("-X1").unwrap();
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        assert!(o.set_jumper_part(&x1, "9", Some("example.ex-tb-2-5")).is_err());
        assert!(o.set_jumper_part(&x1, "1", Some("no.such.part")).is_err());
        assert_eq!(o.project.meta.jumpers[0].part, None);
        o.set_jumper_part(&x1, "2", Some("example.ex-tb-2-5")).unwrap();
        assert_eq!(o.project.meta.jumpers[0].part.as_deref(), Some("example.ex-tb-2-5"));
        assert!(o.parts.contains_key("example.ex-tb-2-5"), "loaded for the reports");
        o.save().unwrap();
        let back = Open::load(&o.dir).unwrap();
        assert!(back.parts.contains_key("example.ex-tb-2-5"), "loaded again on opening");
        o.set_jumper_part(&x1, "1", None).unwrap();
        assert_eq!(o.project.meta.jumpers[0].part, None);
        assert!(o.undo());
        assert_eq!(o.project.meta.jumpers[0].part.as_deref(), Some("example.ex-tb-2-5"));
    }

    #[test]
    fn devices_get_parts_from_the_library_and_own_properties() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let a = o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        let tag = o.project.pages[page].symbol(&a).unwrap().tag.clone().unwrap();
        let (id, label) = o
            .part_library
            .iter()
            .find(|(id, _)| id.contains("ct-9"))
            .cloned()
            .unwrap();
        assert!(!label.is_empty());
        o.set_device_part(&tag, Some(&id)).unwrap();
        assert_eq!(o.project.devices[&tag].part.as_deref(), Some(id.as_str()));
        assert!(o.parts.contains_key(&id));
        assert!(o.set_device_part(&tag, Some("no.such.part")).is_err());
        o.set_device_property(&tag, "coil voltage", "24 V DC");
        assert_eq!(o.project.devices[&tag].properties["coil voltage"], "24 V DC");
        o.set_device_property(&tag, "coil voltage", "");
        assert!(o.project.devices[&tag].properties.is_empty());
        o.set_device_part(&tag, None).unwrap();
        assert_eq!(o.project.devices[&tag].part, None);
        assert!(o.undo());
        assert_eq!(o.project.devices[&tag].part.as_deref(), Some(id.as_str()));
    }

    #[test]
    fn devices_get_accessories_with_quantities() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let a = o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        let tag = o.project.pages[page].symbol(&a).unwrap().tag.clone().unwrap();
        let id = o.part_library.first().map(|(id, _)| id.clone()).unwrap();
        o.set_device_accessories(
            &tag,
            vec![DevicePart {
                part: id.clone(),
                qty: 0,
            }],
        )
        .unwrap();
        assert_eq!(
            o.project.devices[&tag].accessories,
            [DevicePart {
                part: id.clone(),
                qty: 1
            }]
        );
        assert!(o.parts.contains_key(&id), "taken into the project");
        assert!(o.project.used_parts().contains(&id));
        assert!(
            o.set_device_accessories(
                &tag,
                vec![DevicePart {
                    part: "no.such.part".into(),
                    qty: 1
                }]
            )
            .is_err()
        );
        assert_eq!(
            o.project.devices[&tag].accessories.len(),
            1,
            "a failed change keeps what was there"
        );
        assert!(o.undo());
        assert!(o.project.devices.get(&tag).is_none_or(|d| d.accessories.is_empty()));
    }

    #[test]
    fn a_refused_accessory_list_takes_no_part_into_the_project() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let a = o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        let tag = o.project.pages[page].symbol(&a).unwrap().tag.clone().unwrap();
        let id = o
            .part_library
            .iter()
            .map(|(id, _)| id.clone())
            .find(|id| !o.parts.contains_key(id))
            .unwrap();
        let copy = store::vendor_path(&o.dir, ItemKind::Part, &id).unwrap();
        assert!(!copy.exists());
        let list = vec![
            DevicePart {
                part: id.clone(),
                qty: 1,
            },
            DevicePart {
                part: "no.such.part".into(),
                qty: 1,
            },
        ];
        assert!(
            o.check_parts_then(list.iter().map(|a| a.part.as_str()), |_| Ok(()))
                .is_err()
        );
        assert!(!copy.exists(), "no copy left by a refused check");
        assert!(!o.parts.contains_key(&id), "not loaded by a refused check");
        assert!(o.set_device_accessories(&tag, list).is_err());
        assert!(!copy.exists(), "no copy left by a refused change");
        assert!(!o.parts.contains_key(&id), "not loaded by a refused change");
        assert!(!o.project.lock.parts.contains_key(&id));
        let good = vec![DevicePart {
            part: id.clone(),
            qty: 1,
        }];
        o.check_parts_then(good.iter().map(|a| a.part.as_str()), |_| Ok(()))
            .unwrap();
        o.set_device_accessories(&tag, good).unwrap();
        assert!(copy.exists());
        assert!(o.parts.contains_key(&id));
        assert!(o.project.lock.parts.contains_key(&id));
    }

    #[test]
    fn a_part_copy_that_cannot_be_taken_back_is_named_with_its_reason() {
        let (_tmp, mut o) = project();
        let id = o
            .part_library
            .iter()
            .map(|(id, _)| id.clone())
            .find(|id| !o.parts.contains_key(id))
            .unwrap();
        let copy = store::vendor_path(&o.dir, ItemKind::Part, &id).unwrap();
        let ids = [id.as_str(), "no.such.part"];
        let plain = o.check_parts_then(ids, |_| Ok(())).unwrap_err();
        assert!(!plain.contains("could not be removed"), "{plain}");
        o.block_copy_removal = true;
        let e = o.check_parts_then(ids, |_| Ok(())).unwrap_err();
        assert!(e.starts_with(&plain), "the lookup error is kept: {e}");
        assert!(e.contains("could not be removed"), "{e}");
        assert!(e.contains(&copy.display().to_string()), "the copy is named: {e}");
        assert!(!o.parts.contains_key(&id), "not loaded by a refused check");
        assert!(!o.project.lock.parts.contains_key(&id));
    }

    #[test]
    fn project_props_drop_empty_title_block_fields() {
        let (_tmp, mut o) = project();
        assert!(o.set_project_props(" ", &[], None, None).is_err());
        o.set_project_props(
            "Pump station",
            &[("customer".into(), "Acme".into()), ("revision".into(), " ".into())],
            None,
            None,
        )
        .unwrap();
        assert_eq!(o.project.meta.name, "Pump station");
        assert_eq!(o.project.meta.title_block.len(), 1);
        assert!(o.undo());
        assert_eq!(o.project.meta.name, "Test");
    }

    #[test]
    fn a_jumper_stays_on_its_terminals_when_one_moves_past_another() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        let (a, b, c) = (put(10.0), put(20.0), put(30.0));
        // Numbered by drawing order: a 1, b 2, c 3. Jumper a and b.
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        // Move a to the far right: now b 1, c 2, a 3. The jumper must still join a and b.
        o.checkpoint();
        o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(40.0, 20.0);
        assert!(o.follow_terminals());
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "3"]);
        let n = o.terminal_numbers();
        let num = |id: &str| n[&(o.project.pages[page].id.clone(), id.to_owned())].1.clone();
        assert_eq!((num(&a), num(&b), num(&c)), ("3".into(), "1".into(), "2".into()));
        // Undo puts drawing and jumper back together, with nothing left to follow.
        assert!(o.undo());
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "2"]);
        assert!(!o.follow_terminals());
    }

    #[test]
    fn a_drag_followed_frame_by_frame_keeps_the_jumper() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        let (a, b, _c) = (put(10.0), put(20.0), put(30.0));
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        // One drag, one checkpoint, two frames: a passes b, then c.
        o.checkpoint();
        let mut to = |x: f64| {
            o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(x, 20.0);
            o.follow_terminals();
        };
        to(25.0);
        to(40.0);
        let n = o.terminal_numbers();
        let num = |id: &str| n[&(o.project.pages[page].id.clone(), id.to_owned())].1.clone();
        let mut want = vec![num(&a), num(&b)];
        want.sort();
        assert_eq!(o.project.meta.jumpers[0].terminals, want);
    }

    #[test]
    fn a_terminal_turned_into_another_symbol_leaves_its_jumper() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        let (a, _b, _c, _d) = (put(10.0), put(20.0), put(30.0), put(40.0));
        o.add_jumper(&x1, &["1".into(), "2".into(), "3".into()]).unwrap();
        // a becomes a coil: b, c, d are now 1, 2, 3 and the jumper keeps b and c only.
        o.checkpoint();
        o.project.pages[page]
            .symbols
            .iter_mut()
            .find(|s| s.id == a)
            .unwrap()
            .symbol = "wc.coil".into();
        o.follow_terminals();
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "2"]);
    }

    #[test]
    fn an_edit_after_a_move_keeps_the_followed_jumper() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        let (a, _b, _c) = (put(10.0), put(20.0), put(30.0));
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        // Move a past b and c, then make unrelated edits that copy the project or its meta
        // first: none of them may bring back the stale jumper.
        o.checkpoint();
        o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(40.0, 20.0);
        let x2 = DeviceTag::parse("-X2").unwrap();
        assert!(!o.remove_from_jumper(&x2, "1"));
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "3"]);
        // An unnumbered wire, so numbering really replaces the project with its copy.
        let k1 = o.place(page, "wc.coil", Point::new(20.0, 120.0)).unwrap();
        let k2 = o.place(page, "wc.coil", Point::new(60.0, 160.0)).unwrap();
        let pin = |item: &str, pin: &str| Endpoint::Pin {
            item: item.into(),
            pin: pin.into(),
        };
        o.add_wire(page, pin(&k1, "A2"), pin(&k2, "A1"), Vec::new());
        o.checkpoint();
        o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(5.0, 20.0);
        assert!(o.number_wires(wc_core::numbering::Rule::Sequential).done.wires > 0);
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "2"]);
        o.set_device_property(&x2, "note", "x");
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "2"]);
    }

    #[test]
    fn a_renamed_terminal_keeps_its_place_on_the_rail() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |at: f64, tag: &str| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(DeviceTag::parse(tag).unwrap());
            id
        };
        let (three, _four) = (put(10.0, "-X1.3"), put(20.0, "-X1.4"));
        o.checkpoint();
        o.project
            .meta
            .set_terminal_order(&x1, &["4".into(), "3".into()])
            .unwrap();
        // -X1.3 renamed to -X1.7 (no jumper on the strip): it stays after 4.
        o.checkpoint();
        o.project.pages[page]
            .symbols
            .iter_mut()
            .find(|s| s.id == three)
            .unwrap()
            .tag = Some(DeviceTag::parse("-X1.7").unwrap());
        o.checkpoint();
        assert_eq!(o.project.meta.strip_layout(&x1).unwrap().order, ["4", "7"]);
    }

    #[test]
    fn jumper_edits_after_a_move_start_from_the_followed_jumper() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        let (a, _b, _c, _d) = (put(10.0), put(20.0), put(30.0), put(50.0));
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        // Move a past b and c (b 1, c 2, a 3, d 4), then jumper a (now 3) to d: one jumper of
        // a, b and d.
        o.checkpoint();
        o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(40.0, 20.0);
        o.add_jumper(&x1, &["3".into(), "4".into()]).unwrap();
        assert_eq!(o.project.meta.jumpers.len(), 1);
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "3", "4"]);
        // Move a back to the left (a 1, b 2, c 3, d 4), then take d out: a and b remain.
        o.checkpoint();
        o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(5.0, 20.0);
        assert!(o.remove_from_jumper(&x1, "4"));
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "2"]);
    }

    #[test]
    fn a_jumper_part_set_after_a_move_keeps_the_followed_jumper() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        let (a, _b, _c) = (put(10.0), put(20.0), put(30.0));
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        // Move a past c (b 1, c 2, a 3): the jumper of a and b is now 1-3.
        o.checkpoint();
        o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(40.0, 20.0);
        o.set_jumper_part(&x1, "1", Some("example.ex-tb-2-5")).unwrap();
        assert_eq!(o.project.meta.jumpers.len(), 1);
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "3"]);
        assert_eq!(o.project.meta.jumpers[0].part.as_deref(), Some("example.ex-tb-2-5"));
    }

    #[test]
    fn a_terminal_whose_page_becomes_an_overview_does_not_take_another_terminals_number() {
        let (_tmp, mut o) = project();
        let first = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        o.project.pages.push(Page::new("p0003", "3", "More"));
        let second = o.project.pages.len() - 1;
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |page: usize, at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            let s = o.project.pages[page].symbols.iter_mut().find(|s| s.id == id).unwrap();
            s.tag = Some(x1.clone());
            id
        };
        // a on the first page, b and c on the second: a 1, b 2, c 3. Jumper a and c.
        let (_a, _b, _c) = (put(first, 10.0), put(second, 10.0), put(second, 20.0));
        o.add_jumper(&x1, &["1".into(), "3".into()]).unwrap();
        // The first page becomes an overview: b 1, c 2. a is no terminal of the plan any more,
        // and must not pass its old number 1 on to b.
        o.checkpoint();
        o.project.pages[first].overview = true;
        o.follow_terminals();
        assert!(
            o.project.meta.jumpers.iter().all(|j| j.terminals != ["1", "2"]),
            "{:?}",
            o.project.meta.jumpers
        );
        assert!(o.project.meta.jumpers.is_empty(), "only c is left of the jumper");
    }

    #[test]
    fn redo_brings_back_a_followed_move_and_an_overview_page_keeps_its_jumpers() {
        let (_tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let x1 = DeviceTag::parse("-X1").unwrap();
        let mut put = |at: f64| {
            let id = o.place(page, "wc.terminal", Point::new(at, 20.0)).unwrap();
            o.project.pages[page]
                .symbols
                .iter_mut()
                .find(|s| s.id == id)
                .unwrap()
                .tag = Some(x1.clone());
            id
        };
        let (a, _b, _c) = (put(10.0), put(20.0), put(30.0));
        o.add_jumper(&x1, &["1".into(), "2".into()]).unwrap();
        o.checkpoint();
        o.project.pages[page].symbols.iter_mut().find(|s| s.id == a).unwrap().at = Point::new(40.0, 20.0);
        // Undo right away (no checkpoint in between), then redo: the move comes back followed.
        assert!(o.undo());
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "2"]);
        assert!(o.redo());
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "3"]);
        // Marking the page as an overview does not delete the jumper.
        o.checkpoint();
        o.project.pages[page].overview = true;
        o.follow_terminals();
        assert_eq!(o.project.meta.jumpers[0].terminals, ["1", "3"]);
    }

    #[test]
    fn project_props_set_the_locations_in_one_undo_step() {
        let (_tmp, mut o) = project();
        let name = o.project.meta.name.clone();
        let mark = o.undo_mark();
        let rows = |r: &[(&str, &str)]| {
            r.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect::<Vec<_>>()
        };
        o.set_project_props(
            &name,
            &[],
            Some(&rows(&[("+L2", "Room"), ("", ""), ("L1", "Cabinet")])),
            None,
        )
        .unwrap();
        assert_eq!(o.undo_mark(), mark + 1);
        let paths: Vec<&str> = o.project.meta.locations.iter().map(|l| l.path.as_str()).collect();
        assert_eq!(paths, ["L1", "L2"]);
        assert!(
            o.set_project_props(&name, &[], Some(&rows(&[("+L1-K1", "")])), None)
                .is_err()
        );
        assert!(
            o.set_project_props(&name, &[], Some(&rows(&[("L1", "a"), ("+L1", "b")])), None)
                .is_err(),
            "twice"
        );
        assert!(
            o.set_project_props(&name, &[], Some(&rows(&[("", "Cabinet")])), None)
                .is_err(),
            "no path"
        );
        assert_eq!(o.project.meta.locations.len(), 2, "a bad row changes nothing");
        o.set_project_props(&name, &[], None, None).unwrap();
        assert_eq!(o.project.meta.locations.len(), 2, "None keeps them");
        o.set_project_props(&name, &[], Some(&[]), None).unwrap();
        assert!(o.project.meta.locations.is_empty());
        assert!(o.undo());
        assert_eq!(o.project.meta.locations.len(), 2);
    }

    #[test]
    fn undo_and_redo_of_a_new_library_source_update_the_library_list() {
        let (_tmp, mut o) = project();
        let root = o.dir.join("library");
        let sym = wc_core::Symbol {
            source: String::new(),
            id: "test.imported".into(),
            name: "Imported".into(),
            category: String::new(),
            letter_code: String::new(),
            role: wc_core::symbol::SymbolRole::Standalone,
            graphics: vec![],
            pins: vec![],
            tag_at: Default::default(),
            description: String::new(),
        };
        wc_library::write_symbol(&root, &sym).unwrap();
        let listed = |o: &Open| o.library.iter().any(|e| e.id == "test.imported");
        o.refresh_library();
        assert!(!listed(&o), "the folder is not a source yet");

        // As Library -> Import symbols does the first time it writes into the project's folder.
        o.checkpoint();
        o.project.meta.libraries.insert(
            0,
            LibrarySource {
                name: "project".into(),
                kind: "folder".into(),
                location: "library".into(),
                reference: None,
            },
        );
        o.refresh_library();
        assert!(listed(&o));
        assert!(o.undo());
        assert!(!listed(&o), "undo removed the source, so its symbols go");
        assert!(o.redo());
        assert!(listed(&o), "redo brings them back");

        // A part assigned after the source: undo both steps, then redo both. The rebuild on the
        // first redo drops the part (no device uses it yet); the second redo loads it again.
        let part = Part {
            id: "acme.relay".into(),
            manufacturer: "Acme".into(),
            ..Default::default()
        };
        store::write_vendored_part(&o.dir, &part).unwrap();
        o.checkpoint();
        o.project.devices.insert(
            DeviceTag::parse("-K1").unwrap(),
            wc_core::Device {
                part: Some(part.id.clone()),
                ..Default::default()
            },
        );
        o.parts.insert(part.id.clone(), part.clone());
        assert!(o.undo() && o.undo());
        assert!(!o.parts.contains_key("acme.relay"));
        assert!(o.redo() && o.redo());
        assert_eq!(
            o.parts.get("acme.relay"),
            Some(&part),
            "the device's part is loaded again"
        );
        assert_eq!(o.take_step_warning(), None);

        // A part file that is gone: the step is done, and the warning names the part and why.
        assert!(o.undo());
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Part, "acme.relay").unwrap()).unwrap();
        o.parts.remove("acme.relay");
        assert!(o.redo());
        let warning = o.take_step_warning().unwrap_or_default();
        assert!(warning.contains("acme.relay"), "{warning}");
    }

    #[test]
    fn a_macro_saved_from_its_form_is_undone_and_redone_with_the_change_around_it() {
        let (_tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let clip = o.copy(1, &[Selection::Symbol(id)]);
        let path = o.save_macro(&clip, "Coil", "").unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        let read = || wc_core::macros::read_macro(&path).unwrap().description;

        // Saved alone, it is one undo step; saved the same, it is none.
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.description = "A coil".into();
        let steps = o.undo_steps();
        o.save_macro_form(&form).unwrap();
        assert_eq!(o.undo_steps(), steps + 1);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(o.undo());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first, "the file as it was");
        assert!(o.redo());
        assert_eq!(read(), "A coil");
        let form = crate::macro_form::Form::open(&path).unwrap();
        o.save_macro_form(&form).unwrap();
        assert_eq!(o.undo_steps(), steps + 1, "nothing changed, no step");

        // With a page edit in one squashed step (an assistant's turn), one Undo takes back both.
        let mark = o.undo_mark();
        let pages = o.project.pages.clone();
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.description = "A relay coil".into();
        o.save_macro_form(&form).unwrap();
        o.place(1, "wc.coil", Point::new(60.0, 20.0)).unwrap();
        o.squash_undo(mark);
        assert_eq!(o.undo_steps(), steps + 2);
        assert!(o.undo());
        assert_eq!(o.project.pages, pages);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        assert!(o.redo());
        assert_eq!(read(), "A relay coil");
        assert_ne!(o.project.pages, pages);

        // A change taken back whole (a failed turn) puts the file back too.
        let m = o.mark();
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.description = "Gone".into();
        o.save_macro_form(&form).unwrap();
        o.back_to(m).unwrap();
        assert_eq!(read(), "A relay coil");
        assert_eq!(o.undo_steps(), steps + 2);
    }

    #[test]
    fn project_file_edits_are_replaced_whole_and_a_failed_one_puts_back_the_others() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("a.json"), "A0").unwrap();
        std::fs::write(dir.join("b.json"), "B0").unwrap();
        let edit = |name: &str, before: &str, after: &str| FileEdit {
            path: PathBuf::from(name),
            before: Some(before.into()),
            after: Some(after.into()),
        };
        let edits = [edit("a.json", "A0", "A1"), edit("b.json", "B0", "B1")];
        // A folder where b's staging file goes: its write fails before b is touched.
        std::fs::create_dir(dir.join("b.json.tmp")).unwrap();
        let (err, _) = write_edits(dir, &edits, false, &Waiting::new()).unwrap_err();
        assert!(err.contains("b.json"), "{err}");
        assert_eq!(std::fs::read_to_string(dir.join("a.json")).unwrap(), "A0", "put back");
        assert_eq!(
            std::fs::read_to_string(dir.join("b.json")).unwrap(),
            "B0",
            "whole, as it was"
        );
        std::fs::remove_dir(dir.join("b.json.tmp")).unwrap();
        write_edits(dir, &edits, false, &Waiting::new()).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("b.json")).unwrap(), "B1");
    }

    #[test]
    fn a_failed_replay_puts_files_back_as_they_were_and_names_every_file_left() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        for (name, text) in [("a.json", "A0"), ("b.json", "B0"), ("c.json", "C0")] {
            std::fs::write(dir.join(name), text).unwrap();
        }
        let edit = |name: &str, before: &str, after: &str| FileEdit {
            path: PathBuf::from(name),
            before: Some(before.into()),
            after: Some(after.into()),
        };
        let edits = [
            edit("a.json", "A0", "A1"),
            edit("b.json", "B0", "B1"),
            edit("c.json", "C0", "C1"),
        ];
        std::fs::create_dir(dir.join("b.json.tmp")).unwrap();
        let (err, _) = write_edits(dir, &edits, false, &Waiting::new()).unwrap_err();
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
        assert_eq!(read("a.json"), "A0", "put back");
        assert_eq!((read("b.json"), read("c.json")), ("B0".into(), "C0".into()));
        assert!(err.contains("b.json could not be written"), "{err}");
        assert!(err.contains("put back as it was:") && err.contains("a.json"), "{err}");
        assert!(err.contains("not written:") && err.contains("c.json"), "{err}");
    }

    #[test]
    fn a_replay_writes_nothing_when_a_file_was_changed_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let edit = |name: &str, before: Option<&str>, after: Option<&str>| FileEdit {
            path: PathBuf::from(name),
            before: before.map(str::to_owned),
            after: after.map(str::to_owned),
        };
        // b was written twice in the step: the file is compared with what the step left last.
        let edits = [
            edit("a.json", Some("A0"), Some("A1")),
            edit("b.json", None, Some("B1")),
            edit("b.json", Some("B1"), Some("B2")),
            edit("c.json", Some("C0"), None),
        ];
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
        let all = || [read("a.json"), read("b.json"), read("c.json")];
        let texts = |t: [Option<&str>; 3]| t.map(|t| t.map(str::to_owned));
        let put = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();

        // Redo: every file as it was before the step, but c, changed outside since.
        put("a.json", "A0");
        put("c.json", "outside");
        let (err, _) = write_edits(dir, &edits, false, &Waiting::new()).unwrap_err();
        assert!(err.contains("c.json") && err.contains("changed outside"), "{err}");
        assert!(!err.contains("a.json") && !err.contains("b.json"), "{err}");
        assert_eq!(all(), texts([Some("A0"), None, Some("outside")]), "nothing written");
        put("c.json", "C0");
        write_edits(dir, &edits, false, &Waiting::new()).unwrap();
        assert_eq!(all(), texts([Some("A1"), Some("B2"), None]));

        // Undo: b is gone and c is there again, where the step left neither so.
        std::fs::remove_file(dir.join("b.json")).unwrap();
        put("c.json", "new");
        let (err, _) = write_edits(dir, &edits, true, &Waiting::new()).unwrap_err();
        assert!(err.contains("b.json") && err.contains("c.json"), "{err}");
        assert!(!err.contains("a.json"), "{err}");
        assert_eq!(all(), texts([Some("A1"), None, Some("new")]), "nothing written");
        put("b.json", "B2");
        std::fs::remove_file(dir.join("c.json")).unwrap();
        write_edits(dir, &edits, true, &Waiting::new()).unwrap();
        assert_eq!(all(), texts([Some("A0"), None, Some("C0")]));
    }

    #[test]
    fn a_replay_is_refused_when_a_file_was_changed_outside_between_two_edits_of_the_step() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let edit = |name: &str, before: &str, after: &str| FileEdit {
            path: PathBuf::from(name),
            before: Some(before.into()),
            after: Some(after.into()),
        };
        // One step: b went B0 to B1, another program wrote "outside", then the step went on
        // from that to B2. Undo would write "outside" and then B0 over it.
        let edits = [
            edit("a.json", "A0", "A1"),
            edit("b.json", "B0", "B1"),
            edit("b.json", "outside", "B2"),
        ];
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
        let put = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
        put("a.json", "A1");
        put("b.json", "B2");
        let (err, _) = write_edits(dir, &edits, true, &Waiting::new()).unwrap_err();
        assert!(err.contains("b.json") && err.contains("changed outside"), "{err}");
        assert!(err.contains("nothing was changed") && !err.contains("a.json"), "{err}");
        assert_eq!((read("a.json"), read("b.json")), ("A1".into(), "B2".into()));
        // Redo meets the same break, with every file as it was before the step.
        put("a.json", "A0");
        put("b.json", "B0");
        let (err, _) = write_edits(dir, &edits, false, &Waiting::new()).unwrap_err();
        assert!(err.contains("b.json") && err.contains("changed outside"), "{err}");
        assert_eq!((read("a.json"), read("b.json")), ("A0".into(), "B0".into()));
        // Edits that join are one step both ways.
        let edits = [
            edit("a.json", "A0", "A1"),
            edit("b.json", "B0", "B1"),
            edit("b.json", "B1", "B2"),
        ];
        write_edits(dir, &edits, false, &Waiting::new()).unwrap();
        assert_eq!((read("a.json"), read("b.json")), ("A1".into(), "B2".into()));
        write_edits(dir, &edits, true, &Waiting::new()).unwrap();
        assert_eq!((read("a.json"), read("b.json")), ("A0".into(), "B0".into()));
    }

    #[test]
    fn files_written_are_not_put_back_over_a_change_made_outside_since() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let edit = |name: &str, before: Option<&str>, after: Option<&str>| FileEdit {
            path: PathBuf::from(name),
            before: before.map(str::to_owned),
            after: after.map(str::to_owned),
        };
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
        let put = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
        put("a.json", "A0");
        put("b.json", "B0");
        put("c.json", "C0");
        // b is written twice, c is removed and d is new.
        let edits = [
            edit("a.json", Some("A0"), Some("A1")),
            edit("b.json", Some("B0"), Some("B1")),
            edit("b.json", Some("B1"), Some("B2")),
            edit("c.json", Some("C0"), None),
            edit("d.json", None, Some("D1")),
        ];
        let written = write_edits(dir, &edits, false, &Waiting::new()).unwrap();
        // Before what follows fails, another program saves b, writes c again and removes d.
        put("b.json", "outside");
        put("c.json", "new");
        std::fs::remove_file(dir.join("d.json")).unwrap();
        let (back, stuck, unwritten) = put_back_files(&written);
        assert!(unwritten.is_empty(), "none is written later: {unwritten:?}");
        assert_eq!(read("a.json").as_deref(), Some("A0"), "put back");
        assert_eq!(read("b.json").as_deref(), Some("outside"), "left as it is");
        assert_eq!(read("c.json").as_deref(), Some("new"), "left as it is");
        assert_eq!(read("d.json"), None);
        assert_eq!(back.len(), 1, "{back:?}");
        assert!(back[0].ends_with("a.json"), "{back:?}");
        // Each file left is named once, with why.
        assert_eq!(stuck.len(), 3, "{stuck:?}");
        for name in ["b.json", "c.json", "d.json"] {
            assert!(
                stuck.iter().any(|s| s.contains(name) && s.contains("changed outside")),
                "{name}: {stuck:?}"
            );
        }
        // Untouched since, every file goes back, b once, as it was before its first write.
        put("b.json", "B0");
        put("c.json", "C0");
        let written = write_edits(dir, &edits, false, &Waiting::new()).unwrap();
        assert_eq!(read("b.json").as_deref(), Some("B2"));
        let (back, stuck, unwritten) = put_back_files(&written);
        assert!(stuck.is_empty() && unwritten.is_empty(), "{stuck:?}");
        assert_eq!(back.len(), 4);
        assert_eq!(
            [read("a.json"), read("b.json"), read("c.json"), read("d.json")],
            [Some("A0".into()), Some("B0".into()), Some("C0".into()), None]
        );
    }

    #[test]
    fn a_change_taken_back_leaves_a_file_changed_outside_and_puts_the_others_back() {
        let (_tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let clip = o.copy(1, &[Selection::Symbol(id)]);
        let path = o.save_macro(&clip, "Coil", "").unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        let translations = o.dir.join(wc_core::translate::TRANSLATIONS_FILE);
        assert!(translate(&mut o, "Control", "Styring"));
        let was = translations_file(&o).unwrap_or_default();
        let (project, steps) = (o.project.clone(), o.undo_steps());
        let save_macro = |o: &mut Open| {
            let mut form = crate::macro_form::Form::open(&path).unwrap();
            form.description = "Gone".into();
            o.save_macro_form(&form).unwrap();
        };

        // The change saves the macro and sets a translation.
        let mark = o.mark();
        save_macro(&mut o);
        o.checkpoint();
        o.project.pages[0].title = "Main".into();
        assert!(translate(&mut o, "Main", "Hoved"));
        // Another program saves the translations before the change is taken back.
        let outside = "source = \"en\"\n\n[[text]]\nen = \"Control\"\nnb = \"Kontroll\"\n";
        std::fs::write(&translations, outside).unwrap();
        let err = o.back_to(mark).unwrap_err();
        assert!(
            err.contains(&translations.display().to_string()) && err.contains("changed outside"),
            "{err}"
        );
        assert!(
            err.contains("left as it is") && !err.contains("nothing was changed"),
            "{err}"
        );
        assert!(
            !err.contains(&path.display().to_string()),
            "the macro is not named: {err}"
        );
        assert_eq!(translations_file(&o).as_deref(), Some(outside), "left as it is");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first, "the macro is put back");
        assert_eq!((&o.project, o.undo_steps()), (&project, steps));
        assert!(o.mark_edits.is_empty());
        assert!(o.unwritten_files.is_empty(), "nothing waits for the next Save");

        // Changed outside between the change's two translations: left as well.
        std::fs::write(&translations, &was).unwrap();
        let mark = o.mark();
        save_macro(&mut o);
        assert!(translate(&mut o, "Main", "Hoved"));
        std::fs::write(&translations, outside).unwrap();
        assert!(translate(&mut o, "Main", "Hoved"));
        let left = translations_file(&o);
        let err = o.back_to(mark).unwrap_err();
        assert!(err.contains("changed outside"), "{err}");
        assert_eq!(translations_file(&o), left, "left as it is");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first, "the macro is put back");

        // With no file changed outside, both go back and nothing is reported.
        std::fs::write(&translations, &was).unwrap();
        let mark = o.mark();
        save_macro(&mut o);
        assert!(translate(&mut o, "Main", "Hoved"));
        assert!(translate(&mut o, "Control", "Kontroll"));
        o.back_to(mark).unwrap();
        assert_eq!(translations_file(&o).as_deref(), Some(was.as_str()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
        assert_eq!((&o.project, o.undo_steps()), (&project, steps));
    }

    #[test]
    fn a_file_left_alone_for_an_outside_change_is_not_written_by_the_next_save() {
        // A change taken back: the macro cannot be written, the translations were changed
        // outside. Only the macro waits for the next Save.
        let (_tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let clip = o.copy(1, &[Selection::Symbol(id)]);
        let path = o.save_macro(&clip, "Coil", "").unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        let translations = o.dir.join(wc_core::translate::TRANSLATIONS_FILE);
        o.save().unwrap();
        let mark = o.mark();
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.description = "Gone".into();
        o.save_macro_form(&form).unwrap();
        assert!(translate(&mut o, "Control", "Styring"));
        let outside = "source = \"en\"\n\n[[text]]\nen = \"Control\"\nnb = \"Kontroll\"\n";
        std::fs::write(&translations, outside).unwrap();
        fail_restores(&path, 0);
        let err = o.back_to(mark).unwrap_err();
        assert!(
            err.contains("changed outside") && err.contains("written again on the next Save"),
            "{err}"
        );
        let waiting: Vec<PathBuf> = o.unwritten_files.keys().cloned().collect();
        assert_eq!(waiting, [path.strip_prefix(&o.dir).unwrap().to_path_buf()]);
        let_restores_through();
        o.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first, "written by the Save");
        assert_eq!(translations_file(&o).as_deref(), Some(outside), "left as it is");

        // A failed Undo: b is written, another program then saves it, a fails. Putting b back
        // would write over that change, and so would the next Save.
        let a = path;
        let b = o.save_macro(&clip, "Coil B", "").unwrap();
        let step = o.undo_mark();
        for path in [&a, &b] {
            let mut form = crate::macro_form::Form::open(path).unwrap();
            form.description = "Edited".into();
            o.save_macro_form(&form).unwrap();
        }
        o.squash_undo(step);
        let a_now = std::fs::read_to_string(&a).unwrap();
        fail_restores(&a, 0);
        write_outside_before(&a, &b, "outside");
        assert!(!o.undo());
        let err = o.take_undo_error().unwrap_or_default();
        assert!(err.contains("not put back") && err.contains("changed outside"), "{err}");
        assert!(o.unwritten_files.is_empty(), "{:?}", o.unwritten_files);
        let_restores_through();
        o.save().unwrap();
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "outside", "left as it is");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), a_now);
    }

    #[test]
    fn translations_set_while_the_file_waits_for_a_repair_are_kept_and_taken_back_to_the_repair() {
        let (_tmp, mut o) = project();
        let path = o.dir.join(wc_core::translate::TRANSLATIONS_FILE);
        assert!(translate(&mut o, "Control", "Styring"));
        let needed = translations_file(&o);
        // A change taken back cannot write the file: it waits for the next Save.
        let mark = o.mark();
        assert!(translate(&mut o, "Control", "Kontroll"));
        fail_restores(&path, 0);
        assert!(o.back_to(mark).is_err());
        let_restores_through();
        assert_eq!(o.unwritten_files.len(), 1);
        // Set on purpose now: the Save keeps it, and Undo goes back to what was needed.
        assert!(translate(&mut o, "Control", "Regulering"));
        assert!(o.unwritten_files.is_empty());
        let mine = translations_file(&o);
        o.save().unwrap();
        assert_eq!(translations_file(&o), mine);
        assert!(o.undo(), "{:?}", o.take_undo_error());
        assert_eq!(translations_file(&o), needed);
    }

    #[test]
    fn a_step_squashed_over_an_outside_change_of_the_translations_is_refused_whole() {
        let (_tmp, mut o) = project();
        let path = o.dir.join(wc_core::translate::TRANSLATIONS_FILE);
        let mark = o.undo_mark();
        o.checkpoint();
        o.project.pages[0].title = "Main".into();
        assert!(translate(&mut o, "Main", "Hoved"));
        let outside = "source = \"en\"\n\n[[text]]\nen = \"Control\"\nnb = \"Kontroll\"\n";
        std::fs::write(&path, outside).unwrap();
        assert!(translate(&mut o, "Main", "Hoved"));
        o.squash_undo(mark);
        let (project, steps, file) = (o.project.clone(), o.undo_steps(), translations_file(&o));
        assert_eq!(steps, 1);
        assert!(!o.undo());
        let err = o.take_undo_error().unwrap_or_default();
        assert!(
            err.contains(wc_core::translate::TRANSLATIONS_FILE) && err.contains("changed outside"),
            "{err}"
        );
        assert_eq!(translations_file(&o), file, "left as it is");
        assert_eq!((&o.project, o.undo_steps()), (&project, steps), "nothing changed");
        assert!(!o.can_redo());
    }

    #[test]
    fn a_change_taken_back_reports_a_file_not_put_back_and_the_library_check_together() {
        let (_tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let clip = o.copy(1, &[Selection::Symbol(id)]);
        let path = o.save_macro(&clip, "Coil", "").unwrap();
        let mut m = o.mark();
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.description = "Gone".into();
        o.save_macro_form(&form).unwrap();
        // The macro cannot be put back, and the library files were not known.
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        std::fs::create_dir(&tmp).unwrap();
        m.files = Err("not listed".into());
        let err = o.back_to(m).unwrap_err();
        assert!(
            err.contains(&format!("{} could not be written", path.display())),
            "{err}"
        );
        assert!(err.contains("not checked: not listed"), "{err}");
    }

    #[test]
    fn undo_and_redo_that_fail_on_library_files_leave_a_macro_as_it_was() {
        let (_tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let clip = o.copy(1, &[Selection::Symbol(id)]);
        let path = o.save_macro(&clip, "Coil", "").unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.description = "A coil".into();
        o.save_macro_form(&form).unwrap();
        let edited = std::fs::read_to_string(&path).unwrap();
        let symbol = o.project.lock.symbols.keys().next().unwrap().clone();
        // The macro is written, then the step's symbol file cannot be.
        let fail = |o: &mut Open| {
            o.stale_symbol_files.insert(symbol.clone());
            o.fail_symbol_writes = [true].into();
        };
        fail(&mut o);
        assert!(!o.undo());
        assert!(o.take_undo_error().is_some());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), edited, "Undo");
        assert!(o.undo(), "{:?}", o.take_undo_error());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
        fail(&mut o);
        assert!(!o.redo());
        assert!(o.take_undo_error().is_some());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first, "Redo");
    }

    #[test]
    fn undo_and_redo_do_not_write_over_a_macro_changed_outside() {
        let (_tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let clip = o.copy(1, &[Selection::Symbol(id)]);
        let path = o.save_macro(&clip, "Coil", "").unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.description = "A coil".into();
        o.save_macro_form(&form).unwrap();
        let edited = std::fs::read_to_string(&path).unwrap();
        o.project.pages[0].title = "With the macro".into();
        let (project, steps) = (o.project.clone(), o.undo_steps());
        let name = path.display().to_string();

        // Another program writes the macro after the edit: Undo leaves it and says why.
        std::fs::write(&path, "outside").unwrap();
        assert!(!o.undo());
        let err = o.take_undo_error().unwrap_or_default();
        assert!(err.contains(&name) && err.contains("changed outside"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "outside");
        assert_eq!((&o.project, o.undo_steps()), (&project, steps), "nothing changed");
        assert!(!o.can_redo());
        // With the file as the edit left it, Undo goes through.
        std::fs::write(&path, &edited).unwrap();
        assert!(o.undo(), "{:?}", o.take_undo_error());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);

        // The same for Redo.
        let undone = o.project.clone();
        std::fs::write(&path, "outside").unwrap();
        assert!(!o.redo());
        let err = o.take_undo_error().unwrap_or_default();
        assert!(err.contains(&name) && err.contains("changed outside"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "outside");
        assert_eq!((&o.project, o.undo_steps()), (&undone, steps - 1), "nothing changed");
        std::fs::write(&path, &first).unwrap();
        assert!(o.redo(), "{:?}", o.take_undo_error());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
        assert_eq!(o.project, project);
    }

    #[test]
    fn after_save_as_undo_and_redo_of_a_macro_edit_rewrite_the_copy() {
        let (tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let clip = o.copy(1, &[Selection::Symbol(id)]);
        let path = o.save_macro(&clip, "Coil", "").unwrap();
        let rel = path.strip_prefix(&o.dir).unwrap().to_path_buf();
        let first = std::fs::read_to_string(&path).unwrap();
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.description = "A coil".into();
        o.save_macro_form(&form).unwrap();
        let edited = std::fs::read_to_string(&path).unwrap();
        // One step undone before Save as, so the redo history has it too.
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.description = "A relay coil".into();
        o.save_macro_form(&form).unwrap();
        let last = std::fs::read_to_string(&path).unwrap();
        assert!(o.undo());
        let to = tmp.path().join("copy");
        o.save_as(&to).unwrap();
        let copy = o.dir.join(&rel);
        assert!(o.redo());
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), last);
        assert!(o.undo());
        assert!(o.undo());
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), first);
        // The original keeps the file as it was when copied.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
    }

    #[test]
    fn an_assistant_turn_longer_than_the_history_stays_one_step_with_its_macro_edit() {
        let (_tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let clip = o.copy(1, &[Selection::Symbol(id)]);
        let path = o.save_macro(&clip, "Coil", "").unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        // Earlier steps, which a full history drops before the turn's.
        for n in 0..5 {
            o.checkpoint();
            o.project.pages[0].title = format!("Before {n}");
        }
        let before = o.project.clone();
        let turn = o.start_turn();
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.description = "Saved by the assistant".into();
        o.save_macro_form(&form).unwrap();
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        // Taken in the turn by a tool of its own, after steps were folded or not.
        let mut inner = 0;
        for n in 0..UNDO_STEPS + 50 {
            if n == UNDO_STEPS {
                inner = o.undo_mark();
            }
            o.checkpoint();
            o.project.pages[0].title = format!("Turn {n}");
        }
        o.squash_undo(inner);
        assert_eq!(o.undo_mark(), inner + 1, "a mark taken in the turn stays valid");
        form.description = "Saved again".into();
        o.save_macro_form(&form).unwrap();
        o.project.pages[0].title = "Last".into();
        o.squash_undo(turn);
        assert_eq!(o.undo_steps(), 1, "the turn is one step");
        assert!(o.undo());
        assert_eq!(o.project, before);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
        assert!(!o.can_undo());
        assert!(o.redo());
        assert_eq!(o.project.pages[0].title, "Last");
        assert!(std::fs::read_to_string(&path).unwrap().contains("Saved again"));
    }

    #[test]
    fn a_change_taken_back_puts_a_macro_back_after_its_step_fell_off_the_history() {
        let (_tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let clip = o.copy(1, &[Selection::Symbol(id)]);
        let path = o.save_macro(&clip, "Coil", "").unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        let m = o.mark();
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.description = "Gone".into();
        o.save_macro_form(&form).unwrap();
        for _ in 0..=UNDO_STEPS {
            o.checkpoint();
        }
        assert!(o.undo.iter().all(|s| s.files.is_empty()), "the step fell off");
        o.back_to(m).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
        assert!(o.mark_edits.is_empty());
    }

    /// The project's `translations.toml` as it is on disk: `None` only when there is no such
    /// entry. Anything else that cannot be read as text (a folder there, say) fails the test,
    /// so it never stands for a removed file.
    fn translations_file(o: &Open) -> Option<String> {
        let path = o.dir.join(wc_core::translate::TRANSLATIONS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) => {
                assert_eq!(
                    e.kind(),
                    std::io::ErrorKind::NotFound,
                    "{} cannot be read: {e}",
                    path.display()
                );
                None
            }
        }
    }

    #[test]
    fn a_folder_in_place_of_the_translations_does_not_read_as_no_file() {
        let (_tmp, o) = project();
        assert_eq!(translations_file(&o), None);
        std::fs::create_dir(o.dir.join(wc_core::translate::TRANSLATIONS_FILE)).unwrap();
        let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| translations_file(&o)));
        assert!(read.is_err(), "a folder there is not a removed file");
    }

    /// Set a translation into `nb` the way the assistant's tool does. Returns whether the file
    /// changed.
    fn translate(o: &mut Open, text: &str, to: &str) -> bool {
        o.translations_change(|dir| wc_core::translate::set_translation(dir, text, "nb", to))
            .unwrap()
    }

    #[test]
    fn undo_and_redo_write_translations_toml_back_with_the_project() {
        let (_tmp, mut o) = project();
        assert_eq!(translations_file(&o), None);
        assert!(translate(&mut o, "Control", "Styring"));
        let first = translations_file(&o);
        o.checkpoint();
        o.project.pages[0].title = "Main".into();
        assert!(translate(&mut o, "Main", "Hoved"));
        let second = translations_file(&o);
        assert_ne!(first, second);
        assert_eq!(o.undo_steps(), 3);
        // Setting what is there already is no step.
        assert!(!translate(&mut o, "Main", "Hoved"));
        assert_eq!(o.undo_steps(), 3);

        assert!(o.undo(), "{:?}", o.take_undo_error());
        assert_eq!(translations_file(&o), first);
        assert_eq!(o.project.pages[0].title, "Main", "the page was not part of that step");
        assert!(o.undo());
        assert_ne!(o.project.pages[0].title, "Main");
        assert_eq!(translations_file(&o), first);
        assert!(o.undo());
        assert_eq!(
            translations_file(&o),
            None,
            "there was no file before the first translation"
        );
        assert!(!o.can_undo());

        assert!(o.redo() && o.redo() && o.redo());
        assert_eq!(translations_file(&o), second);
        assert_eq!(o.project.pages[0].title, "Main");
    }

    #[test]
    fn changes_squashed_into_one_step_take_their_translation_with_them() {
        let (_tmp, mut o) = project();
        let mark = o.undo_mark();
        o.checkpoint();
        o.project.pages[0].title = "Main".into();
        assert!(translate(&mut o, "Main", "Hoved"));
        o.checkpoint();
        o.project.pages[1].title = "Pumps".into();
        assert!(translate(&mut o, "Pumps", "Pumper"));
        o.squash_undo(mark);
        let (after, translated) = (o.project.clone(), translations_file(&o));
        assert_eq!(o.undo_steps(), 1);
        assert!(o.undo());
        assert_eq!(translations_file(&o), None);
        assert_eq!(o.project.pages[1].title, "Control");
        assert!(o.redo());
        assert_eq!(o.project, after);
        assert_eq!(translations_file(&o), translated);
    }

    #[test]
    fn what_the_translations_dialog_saves_is_one_undo_step() {
        let (_tmp, mut o) = project();
        // Written by another program while the project is open: not part of any step, and
        // what Undo of the dialog's save goes back to.
        let was = "source = \"en\"\n\n[[text]]\nen = \"Control\"\nnb = \"Styring\"\n";
        std::fs::write(o.dir.join(wc_core::translate::TRANSLATIONS_FILE), was).unwrap();
        let mut form = crate::translations_form::Form::open(&o.dir, &o.project).unwrap();
        form.new_lang = "de".into();
        form.add_language().unwrap();
        for row in &mut form.rows {
            if row.text == "Control" {
                row.cells = vec!["Kontroll".into(), "Steuerung".into()];
            }
        }
        let said = o.translations_change(|_| form.save()).unwrap();
        assert!(said.starts_with("Saved 2 translations"), "{said}");
        assert_eq!(o.undo_steps(), 1, "both cells are one step");
        let saved = translations_file(&o);
        assert!(o.undo());
        assert_eq!(translations_file(&o).as_deref(), Some(was));
        assert!(!o.can_undo());
        assert!(o.redo());
        assert_eq!(translations_file(&o), saved);
        // A dialog closed with nothing changed makes no step.
        let form = crate::translations_form::Form::open(&o.dir, &o.project).unwrap();
        o.translations_change(|_| form.save()).unwrap();
        assert_eq!(o.undo_steps(), 1);
    }

    #[test]
    fn undo_and_redo_do_not_write_over_translations_changed_outside() {
        let (_tmp, mut o) = project();
        let path = o.dir.join(wc_core::translate::TRANSLATIONS_FILE);
        assert!(translate(&mut o, "Control", "Styring"));
        let first = translations_file(&o).unwrap_or_default();
        // Changed outside before the next edit: that edit's step starts from it.
        let outside = "source = \"en\"\n\n[[text]]\nen = \"Control\"\nnb = \"Kontroll\"\n";
        std::fs::write(&path, outside).unwrap();
        assert!(translate(&mut o, "Main", "Hoved"));
        let second = translations_file(&o);
        assert!(o.undo());
        assert_eq!(translations_file(&o).as_deref(), Some(outside));

        // The step before it left another text than the file holds now: Undo of it would
        // write over the outside change, so it is refused, as a macro's step is.
        let (project, steps) = (o.project.clone(), o.undo_steps());
        assert!(!o.undo());
        let err = o.take_undo_error().unwrap_or_default();
        assert!(
            err.contains(wc_core::translate::TRANSLATIONS_FILE) && err.contains("changed outside"),
            "{err}"
        );
        assert_eq!(translations_file(&o).as_deref(), Some(outside), "left as it is");
        assert_eq!((&o.project, o.undo_steps()), (&project, steps), "nothing changed");
        // With the file as that step left it, Undo goes through and removes it.
        std::fs::write(&path, &first).unwrap();
        assert!(o.undo(), "{:?}", o.take_undo_error());
        assert_eq!(translations_file(&o), None);

        // Redo of the first translation expects no file: one written outside meanwhile stays.
        std::fs::write(&path, outside).unwrap();
        assert!(!o.redo());
        let err = o.take_undo_error().unwrap_or_default();
        assert!(err.contains("changed outside"), "{err}");
        assert_eq!(translations_file(&o).as_deref(), Some(outside), "left as it is");
        assert_eq!(o.undo_steps(), steps - 1);
        std::fs::remove_file(&path).unwrap();
        assert!(o.redo(), "{:?}", o.take_undo_error());
        assert_eq!(translations_file(&o).as_deref(), Some(first.as_str()));
        // The next Redo expects the outside text its edit started from.
        assert!(!o.redo());
        assert!(o.take_undo_error().is_some());
        std::fs::write(&path, outside).unwrap();
        assert!(o.redo(), "{:?}", o.take_undo_error());
        assert_eq!(translations_file(&o), second);
    }

    #[test]
    fn a_change_taken_back_as_a_whole_puts_the_translations_back() {
        let (_tmp, mut o) = project();
        assert!(translate(&mut o, "Control", "Styring"));
        let was = translations_file(&o);
        let mark = o.mark();
        o.checkpoint();
        o.project.pages[0].title = "Main".into();
        assert!(translate(&mut o, "Main", "Hoved"));
        o.back_to(mark).unwrap();
        assert_eq!(translations_file(&o), was);
        assert_eq!(o.undo_steps(), 1);
        // A change that stays keeps its translation step after the earlier ones.
        let mark = o.mark();
        assert!(translate(&mut o, "Control", "Kontroll"));
        o.keep(mark);
        assert_eq!(o.undo_steps(), 2);
        assert!(o.undo());
        assert_eq!(translations_file(&o), was);
        assert!(o.undo());
        assert_eq!(translations_file(&o), None);
    }

    #[test]
    fn a_step_that_only_changed_a_translation_is_not_dropped_as_one_that_changes_nothing() {
        let (_tmp, mut o) = project();
        assert!(translate(&mut o, "Control", "Styring"));
        o.forget_steps_like_now(|a, b| a == b);
        o.renumber_in_history("p0002", "7", "8");
        assert_eq!(o.undo_steps(), 1);
        assert!(o.undo());
        o.forget_steps_like_now(|a, b| a == b);
        o.renumber_in_history("p0002", "7", "8");
        assert!(o.can_redo());
        assert!(o.redo());
        assert!(translations_file(&o).is_some());
    }

    #[test]
    fn an_undo_that_cannot_write_the_translations_changes_nothing() {
        let (_tmp, mut o) = project();
        assert!(translate(&mut o, "Control", "Styring"));
        o.checkpoint();
        o.project.pages[0].title = "Main".into();
        assert!(translate(&mut o, "Main", "Hoved"));
        let (project, translated) = (o.project.clone(), translations_file(&o));
        // A folder where the staging file goes: the write fails.
        let staging = o.dir.join(format!("{}.tmp", wc_core::translate::TRANSLATIONS_FILE));
        std::fs::create_dir(&staging).unwrap();
        assert!(!o.undo());
        assert!(o.take_undo_error().is_some());
        assert_eq!(o.project, project);
        assert_eq!(translations_file(&o), translated);
        assert_eq!(o.undo_steps(), 3);
        std::fs::remove_dir(&staging).unwrap();
        assert!(o.undo(), "{:?}", o.take_undo_error());
        assert_ne!(translations_file(&o), translated);
    }

    #[test]
    fn a_translations_file_that_cannot_be_read_refuses_the_change_and_is_not_written_over() {
        let (_tmp, mut o) = project();
        let file = o.dir.join(wc_core::translate::TRANSLATIONS_FILE);
        // Not UTF-8.
        let bytes = [0xff_u8, 0xfe, 0x00, 0x41];
        std::fs::write(&file, bytes).unwrap();
        let mut ran = false;
        let err = o
            .translations_change(|dir| {
                ran = true;
                std::fs::write(dir.join(wc_core::translate::TRANSLATIONS_FILE), "source = \"en\"\n")
            })
            .unwrap_err();
        assert!(err.contains("cannot be read"), "{err}");
        assert!(!ran, "refused before anything is written");
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
        assert_eq!(o.undo_steps(), 0);
        // A command's change of such a file is no step either.
        o.before_translations_command();
        let err = o.note_translations().unwrap_err();
        assert!(
            err.starts_with(Open::TRANSLATIONS_NO_STEP) && err.contains("cannot be read"),
            "{err}"
        );
        assert_eq!(o.undo_steps(), 0);
        // No command started: nothing to note, and nothing to report.
        assert_eq!(o.note_translations(), Ok(false));
    }

    #[test]
    fn a_translations_file_left_unreadable_is_reported_with_its_reason() {
        let (_tmp, mut o) = project();
        let file = o.dir.join(wc_core::translate::TRANSLATIONS_FILE);
        let bytes = [0xff_u8, 0xfe, 0x00, 0x41];
        // A change that fails after it left the file unreadable: both reasons.
        let err = o
            .translations_change(|_| {
                std::fs::write(&file, bytes).and_then(|()| Err::<(), _>(std::io::Error::other("no room")))
            })
            .unwrap_err();
        assert!(err.contains("no room"), "{err}");
        assert!(
            err.contains(Open::TRANSLATIONS_NO_STEP) && err.contains("cannot be read"),
            "{err}"
        );
        assert_eq!(o.undo_steps(), 0);
        // A command that leaves it so: no step, and the reason is not lost. No file before
        // the command is a file to start from.
        std::fs::remove_file(&file).unwrap();
        o.before_translations_command();
        std::fs::write(&file, bytes).unwrap();
        let err = o.note_translations().unwrap_err();
        assert!(
            err.starts_with(Open::TRANSLATIONS_NO_STEP) && err.contains("cannot be read"),
            "{err}"
        );
        assert_eq!(o.undo_steps(), 0);
        std::fs::remove_file(&file).unwrap();
        o.before_translations_command();
        std::fs::write(&file, "source = \"en\"\n").unwrap();
        assert_eq!(o.note_translations(), Ok(true));
        assert_eq!(o.undo_steps(), 1);
    }

    #[test]
    fn a_change_taken_back_puts_the_translations_back_when_the_macro_cannot_be_written() {
        let (_tmp, mut o) = project();
        let id = o.place(1, "wc.coil", Point::new(20.0, 20.0)).unwrap();
        let clip = o.copy(1, &[Selection::Symbol(id)]);
        let path = o.save_macro(&clip, "Coil", "").unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        let mark = o.mark();
        assert!(translate(&mut o, "Control", "Styring"));
        let mut form = crate::macro_form::Form::open(&path).unwrap();
        form.description = "Gone".into();
        o.save_macro_form(&form).unwrap();
        // The macro, written last, is put back first and fails: the translations still go back.
        fail_restores(&path, 0);
        let err = o.back_to(mark).unwrap_err();
        assert!(err.contains("written again on the next Save"), "{err}");
        assert_eq!(translations_file(&o), None, "put back all the same");
        assert!(o.mark_edits.is_empty());
        let_restores_through();
        o.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
    }

    #[test]
    fn a_translation_change_that_fails_makes_no_step() {
        let (_tmp, mut o) = project();
        assert!(translate(&mut o, "Control", "Styring"));
        let was = translations_file(&o);
        let err = o.translations_change(|_| Err::<(), _>("no")).unwrap_err();
        assert_eq!(err, "no");
        assert_eq!(translations_file(&o), was);
        assert_eq!(o.undo_steps(), 1);
        assert!(!o.can_redo());
    }
}

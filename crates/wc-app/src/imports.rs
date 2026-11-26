//! File > Import: projects, drawings and parts in the formats `wiringcraft import` reads, run
//! through the same functions as the CLI ([`wc_import::run`]). The file and, for parts, the
//! library folder come from the system pickers. The import runs on a worker thread ([`Job`])
//! while the window shows what it has said so far; at the end what it said is shown in a
//! message. The options the command line has for a format ([`Opt`]) are asked for in a small
//! dialog after the file is picked ([`Form`]), with the command line's defaults filled in.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

use wc_import::run;

use crate::actions::Action;
use crate::import_symbols::ImportTarget;
use crate::state::Open;

/// The formats File > Import offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Eplan,
    Swe,
    SweLibrary,
    Aml,
    PageDwg,
    PageDxf,
    PlcCsv,
    Pdf,
    Archive,
    PartsEdz,
    PartsBmecat,
    PartsCsv,
}

impl Kind {
    /// The import an action starts, if it is one.
    pub fn of(a: Action) -> Option<Kind> {
        Some(match a {
            Action::ImportEplan => Kind::Eplan,
            Action::ImportSwe => Kind::Swe,
            Action::ImportSweLibrary => Kind::SweLibrary,
            Action::ImportAml => Kind::Aml,
            Action::ImportPageDwg => Kind::PageDwg,
            Action::ImportPageDxf => Kind::PageDxf,
            Action::ImportPlcCsv => Kind::PlcCsv,
            Action::ImportPdf => Kind::Pdf,
            Action::ImportArchive => Kind::Archive,
            Action::ImportPartsEdz => Kind::PartsEdz,
            Action::ImportPartsBmecat => Kind::PartsBmecat,
            Action::ImportPartsCsv => Kind::PartsCsv,
            _ => return None,
        })
    }

    /// The file picker's title.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Eplan => "Import an EPLAN project (.elk, or a zip of the project)",
            Kind::Swe => "Import a SolidWorks Electrical project archive",
            Kind::SweLibrary => "Import the libraries of a SolidWorks Electrical archive",
            Kind::Aml => "Import AutomationML",
            Kind::PageDwg => "Import a DWG drawing onto the current page",
            Kind::PageDxf => "Import a DXF drawing onto the current page",
            Kind::PlcCsv => "Import PLC addresses from a CSV or Excel list",
            Kind::Pdf => "Import PDF pages",
            Kind::Archive => "Import another tool's project archive",
            Kind::PartsEdz => "Import parts from an EDZ file",
            Kind::PartsBmecat => "Import parts from a BMEcat catalogue",
            Kind::PartsCsv => "Import parts from a CSV or Excel list",
        }
    }

    /// The file picker's filter: the extensions the CLI's import reads.
    pub fn filter(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Kind::Eplan => ("EPLAN project", &["elk", "zip"]),
            Kind::Swe => ("SolidWorks Electrical archive or database", &["tewzip", "sqlite"]),
            Kind::SweLibrary => ("SolidWorks Electrical archive", &["tewzip"]),
            Kind::Aml => ("AutomationML", &["aml", "xml"]),
            Kind::PageDwg => ("DWG drawing", &["dwg"]),
            Kind::PageDxf => ("DXF drawing", &["dxf"]),
            Kind::PlcCsv => ("CSV or Excel list", &["csv", "txt", "xlsx"]),
            Kind::Pdf => ("PDF", &["pdf"]),
            Kind::Archive => ("Project archive", &["zip"]),
            Kind::PartsEdz => ("EPLAN Data Exchange", &["edz"]),
            Kind::PartsBmecat => ("BMEcat catalogue", &["xml"]),
            Kind::PartsCsv => ("CSV or Excel list", &["csv", "txt", "xlsx"]),
        }
    }

    /// Parts imports write into a library folder, which is asked for after the file.
    pub fn needs_library(self) -> bool {
        matches!(self, Kind::PartsEdz | Kind::PartsBmecat | Kind::PartsCsv)
    }

    /// The page number the CLI numbers this import's new pages from by default.
    fn first_page(self) -> usize {
        match self {
            Kind::Eplan => 400,
            Kind::Archive => 300,
            Kind::Pdf => 200,
            _ => 100,
        }
    }
}

/// An option of an import that `wiringcraft import` has too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opt {
    /// Millimetres per drawing unit (`--scale`).
    Scale,
    /// The number of the first new page (`--first-page`).
    FirstPage,
    /// Which project of a SolidWorks Electrical archive that holds several (`--pick`).
    Project,
    /// A text file that says which library symbol to draw for which source block or macro
    /// (`--symbol-map`).
    SymbolMap,
    /// The library folder an archive's KiCad symbol libraries and parts lists go into
    /// (`--lib`).
    Library,
}

impl Kind {
    /// The options asked for after the file is picked; none: the import starts at once.
    pub fn options(self) -> &'static [Opt] {
        match self {
            Kind::PageDwg | Kind::PageDxf => &[Opt::Scale],
            Kind::Swe => &[Opt::Project, Opt::FirstPage, Opt::SymbolMap],
            Kind::PartsEdz => &[Opt::SymbolMap],
            Kind::Archive => &[Opt::FirstPage, Opt::Library],
            Kind::Eplan | Kind::Aml | Kind::Pdf => &[Opt::FirstPage],
            _ => &[],
        }
    }
}

/// What an import runs with besides its file. The default is the command line's.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// A drawing's coordinates are multiplied by this to get mm.
    pub scale: f64,
    /// The number of the first new page. None: the command line's default for the format, or
    /// the number after the project's highest page when that is higher ([`first_free_page`]).
    pub first_page: Option<usize>,
    /// SolidWorks Electrical: keep the source's page numbers (the command line without
    /// `--first-page`) instead of numbering from `first_page`.
    pub keep_numbers: bool,
    /// Which project of a SolidWorks Electrical archive, counted from 1.
    pub pick: usize,
    /// The symbol map file of a SolidWorks Electrical or EDZ import; none by default.
    pub symbol_map: Option<PathBuf>,
    /// The library folder an archive's KiCad symbol libraries and parts lists are imported
    /// into. None, the default: they are listed with a warning, not imported.
    pub lib: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            scale: 1.0,
            first_page: None,
            keep_numbers: false,
            pick: 1,
            symbol_map: None,
            lib: None,
        }
    }
}

/// The options dialog: an import's options as typed, starting from the command line's defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    pub kind: Kind,
    pub scale: String,
    /// The first page number: the command line's default, or the next free one after it.
    pub first_page: String,
    /// SolidWorks Electrical: the source's page numbers are kept and `first_page` is not used.
    pub keep_numbers: bool,
    /// The projects of a SolidWorks Electrical archive, as shown (number, folder, name); the
    /// choice is offered when there are several.
    pub projects: Vec<String>,
    /// The project chosen, an index into `projects`.
    pub project: usize,
    /// The symbol map file, typed or picked with Browse; empty: none.
    pub symbol_map: crate::prefs::FolderField,
    /// Archive: the project's library folders its KiCad symbol libraries and parts lists can go
    /// into, as a parts import offers them; filled in by the app.
    pub libraries: Vec<ImportTarget>,
    /// Archive: 0 for not importing them (the command line without `--lib`), then the folders
    /// of `libraries` counted from 1, then one more for another folder.
    pub library: usize,
    /// Archive: the other folder, typed or picked with Browse.
    pub library_folder: crate::prefs::FolderField,
}

/// The projects of the SolidWorks Electrical archive `file` as the options dialog lists them.
/// None for a file that is no archive (a bare database, or a file the import will refuse).
fn swe_projects(file: &Path) -> Vec<String> {
    std::fs::File::open(file)
        .ok()
        .and_then(|f| wc_import::swe::archive_projects(std::io::BufReader::new(f)).ok())
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, p)| match p.name.is_empty() {
            true => format!("{}: {}", i + 1, p.folder),
            false => format!("{}: {} ({})", i + 1, p.folder, p.name),
        })
        .collect()
}

impl Form {
    /// The form for importing `file` as `kind` into `open`, with the defaults filled in.
    pub fn new(open: &Open, kind: Kind, file: &Path) -> Self {
        Self {
            kind,
            scale: "1".to_owned(),
            first_page: first_free_page(open, kind.first_page())
                .map(|n| n.to_string())
                .unwrap_or_default(),
            keep_numbers: false,
            projects: if kind == Kind::Swe {
                swe_projects(file)
            } else {
                Vec::new()
            },
            project: 0,
            symbol_map: crate::prefs::FolderField::default(),
            libraries: Vec::new(),
            library: 0,
            library_folder: crate::prefs::FolderField::default(),
        }
    }

    /// Whether the form shows `opt`. An archive with one project has no project to choose.
    pub fn has(&self, opt: Opt) -> bool {
        self.kind.options().contains(&opt) && (opt != Opt::Project || self.projects.len() > 1)
    }

    /// What the library choice `k` is called in the dialog.
    pub fn library_label(&self, k: usize) -> String {
        match k.checked_sub(1) {
            None => "Do not import them".to_owned(),
            Some(i) => self
                .libraries
                .get(i)
                .map_or_else(|| "Another folder…".to_owned(), ImportTarget::label),
        }
    }

    /// The project library folder chosen for an archive, if one is.
    pub fn library_target(&self) -> Option<&ImportTarget> {
        self.library.checked_sub(1).and_then(|i| self.libraries.get(i))
    }

    /// Another folder than the project's library folders is chosen for an archive.
    pub fn other_library(&self) -> bool {
        self.has(Opt::Library) && self.library > self.libraries.len()
    }

    /// What was typed, as the import's options, or what to correct.
    pub fn options(&self) -> Result<Options, String> {
        let mut o = Options::default();
        if self.has(Opt::Scale) {
            // A decimal comma is read as a point.
            o.scale = self
                .scale
                .trim()
                .replace(',', ".")
                .parse::<f64>()
                .ok()
                .filter(|s| s.is_finite() && *s > 0.0)
                .ok_or_else(|| "The scale must be a number above 0, for example 1 or 0.1.".to_owned())?;
        }
        if self.has(Opt::Project) {
            o.pick = self.project.min(self.projects.len() - 1) + 1;
        }
        if self.has(Opt::FirstPage) {
            o.keep_numbers = self.kind == Kind::Swe && self.keep_numbers;
            if !o.keep_numbers {
                let n = self
                    .first_page
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| "The first page number must be a whole number, for example 100.".to_owned())?;
                o.first_page = Some(n);
            }
        }
        if self.has(Opt::Library) && self.library > 0 {
            o.lib = Some(match self.library_target() {
                Some(t) => t.root.clone(),
                None if self.library_folder.is_empty() => {
                    return Err(
                        "Type or pick the library folder for the symbol libraries and parts lists, or choose not to import them."
                            .to_owned(),
                    );
                }
                None => self.library_folder.path()?,
            });
        }
        if self.has(Opt::SymbolMap) && !self.symbol_map.is_empty() {
            let map = self.symbol_map.path()?;
            if !map.is_file() {
                return Err(format!("The symbol map {} is not a file.", map.display()));
            }
            o.symbol_map = Some(map);
        }
        Ok(o)
    }
}

/// The first page number for an import: `default`, or the number after the project's highest
/// numeric page when that is higher, so the new pages never take a number in use. None when
/// the highest page number has no number after it.
pub fn first_free_page(open: &Open, default: usize) -> Option<usize> {
    open.project
        .pages
        .iter()
        .filter_map(|pg| pg.number.trim().parse::<usize>().ok())
        .try_fold(default, |first, n| n.checked_add(1).map(|next| first.max(next)))
}

/// What an import does with the open project when it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Adopt {
    /// Nothing was written (the project could not be saved first, or no folder was picked).
    Nothing,
    /// Parts went into a library folder: the project's library list is read again.
    Library,
    /// The import wrote to the project folder: what it wrote comes in as one Undo step.
    Project,
}

/// The import itself, as it runs on the worker thread: everything it needs is taken from the
/// open project before it starts.
type Work = Box<dyn FnOnce(&mut run::Log) -> run::Res<()> + Send>;

/// An import running on a thread of its own ([`start`]), so the window keeps drawing. While
/// it runs the project folder is the import's: the app blocks edits, saving and opening
/// another project until [`Job::finish`] has taken the result in.
pub struct Job {
    kind: Kind,
    /// The file's name, for the messages.
    pub name: String,
    /// The project folder it writes to (or whose library list it reads again).
    pub dir: PathBuf,
    lib: Option<PathBuf>,
    /// Said before the import's own result (the changes saved first).
    note: String,
    adopt: Adopt,
    /// What the import has said so far, line by line, as it says it.
    lines: Arc<Mutex<Vec<String>>>,
    done: mpsc::Receiver<(run::Res<()>, run::Log)>,
}

/// Start importing `file` as `kind`: parts into the library folder `lib`, everything else into
/// the open project (a DWG or DXF drawing onto page `page`), with the options `options`.
/// Changes not saved yet are saved first, since the import works on the project folder. The
/// import runs on a worker thread; a reason it cannot start (no free page number, say) comes
/// back from the job as its result.
pub fn start(open: &mut Open, kind: Kind, file: &Path, lib: Option<&Path>, page: usize, options: &Options) -> Job {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut note = String::new();
    let f = file.to_path_buf();
    let map = options.symbol_map.clone();
    let (adopt, work): (Adopt, Result<Work, String>) = if kind.needs_library() {
        match lib.map(Path::to_path_buf) {
            Some(lib) => {
                let work: Work = match kind {
                    Kind::PartsEdz => Box::new(move |log| run::parts_edz(&f, &lib, map.as_deref(), log)),
                    Kind::PartsBmecat => Box::new(move |log| run::parts_bmecat(&f, &lib, log)),
                    _ => Box::new(move |log| run::parts_csv(&f, &lib, log)),
                };
                (Adopt::Library, Ok(work))
            }
            None => (
                Adopt::Nothing,
                Err("no library folder was picked for the parts".to_owned()),
            ),
        }
    } else {
        let saved = if open.dirty {
            note = "The changes not saved yet were saved first.\n".to_owned();
            open.save().map(|_| ())
        } else {
            Ok(())
        };
        match saved {
            Err(e) => (
                Adopt::Nothing,
                Err(format!("the project could not be saved first: {e}")),
            ),
            // What was written comes in also when a part of an archive failed.
            Ok(()) => (Adopt::Project, project_work(open, kind, f, page, options)),
        }
    };
    let (lines, done) = spawn(work);
    Job {
        kind,
        name,
        dir: open.dir.clone(),
        lib: lib.map(Path::to_path_buf).or_else(|| options.lib.clone()),
        note,
        adopt,
        lines,
        done,
    }
}

/// What the worker has said so far, and where its result arrives.
type Running = (Arc<Mutex<Vec<String>>>, mpsc::Receiver<(run::Res<()>, run::Log)>);

/// Run `work` on a thread of its own; a reason it cannot run is the result at once. A worker
/// that panics sends nothing: the closed channel tells [`Job::poll`] it stopped.
fn spawn(work: Result<Work, String>) -> Running {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let (tx, done) = mpsc::channel();
    match work {
        // Nothing to run: the reason is the result, taken in on the next look.
        Err(e) => {
            let _ = tx.send((Err(e), run::Log::default()));
        }
        Ok(work) => {
            let said = lines.clone();
            std::thread::spawn(move || {
                let mut log = run::Log::echoing(move |m| {
                    let (run::Msg::Out(s) | run::Msg::Err(s)) = m;
                    if let Ok(mut said) = said.lock() {
                        said.push(s.clone());
                    }
                });
                let r = work(&mut log);
                let _ = tx.send((r, log));
            });
        }
    }
    (lines, done)
}

/// The import of `file` into the project folder, ready to run on the worker thread, or why
/// it cannot run.
fn project_work(open: &Open, kind: Kind, f: PathBuf, page: usize, options: &Options) -> Result<Work, String> {
    let dir = open.dir.clone();
    let scale = options.scale;
    // Only for the imports that add pages, numbered from a free number.
    let first = || match options.first_page {
        Some(n) => Ok(n),
        None => first_free_page(open, kind.first_page()).ok_or_else(|| {
            "no page number is free after the project's highest one; renumber that page first".to_owned()
        }),
    };
    Ok(match kind {
        Kind::Eplan => {
            let n = first()?;
            Box::new(move |log| run::eplan(&dir, &f, n, log))
        }
        Kind::Swe => {
            let n = if options.keep_numbers { None } else { Some(first()?) };
            let (pick, map) = (options.pick, options.symbol_map.clone());
            Box::new(move |log| run::swe(&dir, &f, pick, n, map.as_deref(), log))
        }
        Kind::SweLibrary => Box::new(move |log| run::swe_library(&dir, &f, None, log)),
        Kind::Aml => {
            let n = first()?;
            Box::new(move |log| run::aml(&dir, &f, n, log))
        }
        Kind::Pdf => {
            let n = first()?;
            Box::new(move |log| run::pdf(&dir, &f, n, log))
        }
        Kind::Archive => {
            let n = first()?;
            let lib = options.lib.clone();
            Box::new(move |log| run::archive(&dir, &f, n, lib.as_deref(), log))
        }
        Kind::PlcCsv => Box::new(move |log| run::plc_csv(&dir, &f, log)),
        _ => {
            let Some(pg) = open.project.pages.get(page) else {
                return Err("there is no page to draw it on; add a page first".to_owned());
            };
            let id = pg.id.clone();
            if kind == Kind::PageDxf {
                Box::new(move |log| run::page_dxf(&dir, &f, &id, scale, log))
            } else {
                let converter = std::env::var("WIRINGCRAFT_DWG2DXF").ok();
                Box::new(move |log| run::import_page_dwg(&dir, &f, &id, scale, converter, log))
            }
        }
    })
}

impl Job {
    /// What the import has said so far.
    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().map(|l| l.clone()).unwrap_or_default()
    }

    /// The import's result once the worker has ended, without waiting.
    pub fn poll(&self) -> Option<(run::Res<()>, run::Log)> {
        match self.done.try_recv() {
            Ok(r) => Some(r),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(self.stopped()),
        }
    }

    /// Wait for the worker to end and give its result.
    #[cfg(test)]
    pub fn wait(&self) -> (run::Res<()>, run::Log) {
        self.done.recv().unwrap_or_else(|_| self.stopped())
    }

    /// The result of a worker that ended without one (it panicked), with what it said so far.
    fn stopped(&self) -> (run::Res<()>, run::Log) {
        let mut log = run::Log::default();
        for line in self.lines() {
            log.out(line);
        }
        // What it wrote before it stopped is still taken in (a project import as an undo step).
        (
            Err("the import stopped part way without an answer; what it wrote before that is kept".to_owned()),
            log,
        )
    }

    /// Take the ended import's result `r` and `log` into `open`: what it wrote to the project
    /// folder comes in as one Undo step. Returns the text for the message dialog (the import's
    /// report and warnings, or why it failed), and whether it was done.
    pub fn finish(&self, open: &mut Open, r: run::Res<()>, log: &run::Log) -> (bool, String) {
        let name = &self.name;
        // Why the project could not be read back after the import wrote to its folder.
        let mut unread: Option<String> = None;
        match self.adopt {
            Adopt::Nothing => {}
            Adopt::Library => open.refresh_library(),
            Adopt::Project => {
                if let Err(e) = open.adopt_saved() {
                    unread = Some(e);
                }
                // An archive's symbol libraries and parts lists went into a library folder.
                if self.lib.is_some() {
                    open.refresh_library();
                }
            }
        }
        let said = log.text();
        let done = r.is_ok() && unread.is_none();
        let head = match (r, unread) {
            (Ok(()), None) => format!("Imported {name}."),
            (Err(e), None) => format!("{name} was not imported: {e}"),
            // The import may have written to the project folder: what the editor shows is older.
            (r, Some(why)) => {
                let failed = r.err().map(|e| format!(" Part of it failed: {e}.")).unwrap_or_default();
                format!(
                    "{name} was imported into the project folder, but the project could not be read back: {why}.{failed} \
                     Open the project again before you edit or import anything else."
                )
            }
        };
        let mut msg = format!("{}{head}", self.note);
        if let Some(lib) = self.lib.as_ref().filter(|_| self.kind.needs_library()) {
            msg.push_str(&format!("\nLibrary folder: {}", lib.display()));
        }
        if !said.is_empty() {
            msg.push_str("\n\n");
            msg.push_str(&said);
        }
        (done, app_wording(&msg))
    }
}

/// Import `file` as `kind` with the default options and wait for it ([`import_with`]).
#[cfg(test)]
pub fn import(open: &mut Open, kind: Kind, file: &Path, lib: Option<&Path>, page: usize) -> (bool, String) {
    import_with(open, kind, file, lib, page, &Options::default())
}

/// Import `file` as `kind` and wait for it: [`start`], then [`Job::finish`]. Returns the text
/// for the message dialog and whether it was done. The app polls instead ([`Job::poll`]).
#[cfg(test)]
pub fn import_with(
    open: &mut Open,
    kind: Kind,
    file: &Path,
    lib: Option<&Path>,
    page: usize,
    options: &Options,
) -> (bool, String) {
    let job = start(open, kind, file, lib, page, options);
    let (r, log) = job.wait();
    job.finish(open, r, &log)
}

/// The shared import code words some advice for the command line (its options); in the app
/// the same advice is given in terms of what can be done here.
fn app_wording(text: &str) -> String {
    const SAID: &[(&str, &str)] = &[
        (
            "remove it or import with --first-page to number the pages from a free number",
            "remove or renumber that page, then import again",
        ),
        (
            "import with --first-page to number the pages from a free number",
            "renumber the page that has it, then import again",
        ),
        (
            "pick another --first-page",
            "remove or renumber that page, then import again",
        ),
        (" (use --pick)", ""),
        (
            "pass --lib <folder>",
            "choose a library folder for them in the import options",
        ),
        ("; run `wiringcraft inspect` to see what is there", ""),
        (
            ": use -C <folder> or create one with wiringcraft new <folder>",
            ": open the project again, or another one",
        ),
    ];
    let mut out = text.to_owned();
    for (cli, app) in SAID {
        out = out.replace(cli, app);
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn every_import_action_has_a_kind_and_a_filter() {
        let kinds: Vec<Kind> = Action::ALL.iter().filter_map(|a| Kind::of(*a)).collect();
        assert_eq!(kinds.len(), 12);
        for k in kinds {
            let (label, extensions) = k.filter();
            assert!(!label.is_empty() && !extensions.is_empty(), "{k:?}");
            assert!(!k.title().is_empty());
        }
        assert_eq!(Kind::of(Action::Save), None);
    }

    fn project() -> (tempfile::TempDir, Open) {
        let tmp = tempfile::tempdir().ok();
        let Some(tmp) = tmp else {
            unreachable!("no temp folder");
        };
        let dir = tmp.path().join("p");
        let made = wc_core::store::save(&dir, &wc_core::Project::new("t"));
        assert!(made.is_ok(), "{made:?}");
        let open = Open::load(&dir);
        let Ok(open) = open else {
            unreachable!("{:?}", open.err());
        };
        (tmp, open)
    }

    #[test]
    fn parts_csv_goes_into_the_picked_library_folder() {
        let (tmp, mut open) = project();
        let csv = tmp.path().join("parts.csv");
        let lib = tmp.path().join("lib");
        assert!(std::fs::write(&csv, "order number;manufacturer;description\nX-1;Acme;Relay\n").is_ok());
        let (_, msg) = import(&mut open, Kind::PartsCsv, &csv, Some(&lib), 0);
        assert!(msg.starts_with("Imported parts.csv."), "{msg}");
        assert!(msg.contains("imported 1 part(s)"), "{msg}");
        assert!(!open.dirty);
    }

    #[test]
    fn a_file_that_cannot_be_read_says_why_and_changes_nothing() {
        let (tmp, mut open) = project();
        let before = open.project.clone();
        let (_, msg) = import(&mut open, Kind::Aml, &tmp.path().join("missing.aml"), None, 0);
        assert!(msg.starts_with("missing.aml was not imported"), "{msg}");
        assert_eq!(open.project, before);
        assert!(!open.can_undo());
    }

    #[test]
    fn imported_pages_come_in_as_one_undo_step_with_unsaved_changes_saved_first() {
        let (tmp, mut open) = project();
        open.add_page();
        assert!(open.dirty);
        let aml = tmp.path().join("plant.aml");
        assert!(std::fs::write(&aml, AML).is_ok());
        let (_, msg) = import(&mut open, Kind::Aml, &aml, None, 0);
        assert!(msg.contains("saved first"), "{msg}");
        assert!(msg.contains("Imported plant.aml."), "{msg}");
        assert!(open.project.pages.len() > 1, "{msg}");
        assert!(
            open.project.devices.keys().any(|t| t.to_string().contains("K1")),
            "{msg}"
        );
        assert!(!open.dirty);
        assert!(open.undo());
        assert_eq!(open.project.pages.len(), 1);
    }

    #[test]
    fn new_pages_are_numbered_after_the_highest_page() {
        let (_tmp, mut open) = project();
        assert_eq!(first_free_page(&open, 100), Some(100));
        let i = open.add_page();
        if let Some(pg) = open.project.pages.get_mut(i) {
            pg.number = "250".into();
        }
        assert_eq!(first_free_page(&open, 100), Some(251));
        assert_eq!(first_free_page(&open, 400), Some(400));
    }

    #[test]
    fn no_free_page_number_after_the_largest_is_refused_not_wrapped() {
        let (tmp, mut open) = project();
        let i = open.add_page();
        if let Some(pg) = open.project.pages.get_mut(i) {
            pg.number = usize::MAX.to_string();
        }
        assert_eq!(first_free_page(&open, 100), None);
        let aml = tmp.path().join("plant.aml");
        assert!(std::fs::write(&aml, AML).is_ok());
        let (done, msg) = import(&mut open, Kind::Aml, &aml, None, 0);
        assert!(!done, "{msg}");
        assert!(msg.contains("no page number is free"), "{msg}");
    }

    #[test]
    fn a_dxf_drawing_goes_onto_the_current_page() {
        let (tmp, mut open) = project();
        let page = open.add_page();
        let dxf = tmp.path().join("plan.dxf");
        let text = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0\n20\n0\n11\n10\n21\n0\n0\nENDSEC\n0\nEOF\n";
        assert!(std::fs::write(&dxf, text).is_ok());
        let (done, msg) = import(&mut open, Kind::PageDxf, &dxf, None, page);
        assert!(done, "{msg}");
        let shapes = open.project.pages.get(page).map_or(0, |pg| pg.graphics.len());
        assert_eq!(shapes, 1, "{msg}");
    }

    /// A DXF drawing with one line from the origin to x = 10.
    const DXF_LINE: &str = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0\n20\n0\n11\n10\n21\n0\n0\nENDSEC\n0\nEOF\n";

    /// How wide the shapes on `page` are together, in mm.
    fn drawn_width(open: &Open, page: usize) -> f64 {
        let xs: Vec<f64> = open
            .project
            .pages
            .get(page)
            .map(|pg| {
                pg.graphics
                    .iter()
                    .map(|g| g.bounds())
                    .flat_map(|(a, b)| [a.x, b.x])
                    .collect()
            })
            .unwrap_or_default();
        let (lo, hi) = xs
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), x| (lo.min(*x), hi.max(*x)));
        if xs.is_empty() { 0.0 } else { hi - lo }
    }

    #[test]
    fn a_drawing_comes_in_at_the_scale_given_as_the_command_line_scales_it() {
        let (tmp, mut open) = project();
        let page = open.add_page();
        let dxf = tmp.path().join("plan.dxf");
        assert!(std::fs::write(&dxf, DXF_LINE).is_ok());
        let form = Form {
            scale: "2,5".to_owned(),
            ..Form::new(&open, Kind::PageDxf, &dxf)
        };
        let options = form.options();
        let Ok(options) = options else {
            unreachable!("{options:?}");
        };
        let (done, msg) = import_with(&mut open, Kind::PageDxf, &dxf, None, page, &options);
        assert!(done, "{msg}");
        assert!((drawn_width(&open, page) - 25.0).abs() < 1e-6, "{msg}");
        // The default is the command line's: 1.
        let other = open.add_page();
        let (done, msg) = import(&mut open, Kind::PageDxf, &dxf, None, other);
        assert!(done, "{msg}");
        assert!((drawn_width(&open, other) - 10.0).abs() < 1e-6, "{msg}");
    }

    #[test]
    fn the_options_form_shows_only_what_the_format_has_with_the_defaults() {
        let (tmp, open) = project();
        let file = tmp.path().join("x");
        for kind in [Kind::PageDwg, Kind::PageDxf] {
            let form = Form::new(&open, kind, &file);
            assert!(form.has(Opt::Scale), "{kind:?}");
            assert_eq!(form.options(), Ok(Options::default()), "{kind:?}");
        }
        // The command line's default first page, or the next free number after it.
        for (kind, default) in [
            (Kind::Eplan, 400),
            (Kind::Swe, 100),
            (Kind::Aml, 100),
            (Kind::Pdf, 200),
            (Kind::Archive, 300),
        ] {
            let form = Form::new(&open, kind, &file);
            assert!(form.has(Opt::FirstPage) && !form.has(Opt::Scale), "{kind:?}");
            assert_eq!(form.first_page, default.to_string(), "{kind:?}");
            assert_eq!(form.options().map(|o| o.first_page), Ok(Some(default)), "{kind:?}");
        }
        for bad in ["", "abc", "-3", "1.5"] {
            let form = Form {
                first_page: bad.to_owned(),
                ..Form::new(&open, Kind::Pdf, &file)
            };
            assert!(form.options().is_err(), "{bad:?}");
        }
        // Keeping the source's numbers is the SolidWorks import's alone, and needs no number.
        let keep = |kind| Form {
            first_page: String::new(),
            keep_numbers: true,
            ..Form::new(&open, kind, &file)
        };
        assert_eq!(
            keep(Kind::Swe).options().map(|o| (o.keep_numbers, o.first_page)),
            Ok((true, None))
        );
        assert!(keep(Kind::Eplan).options().is_err());
        for kind in [Kind::PlcCsv, Kind::PartsBmecat, Kind::PartsCsv, Kind::SweLibrary] {
            assert!(kind.options().is_empty(), "{kind:?}");
        }
        // An archive's library folder: none by default, as the command line without `--lib`.
        let form = Form::new(&open, Kind::Archive, &file);
        assert!(form.has(Opt::Library));
        assert_eq!(form.options().map(|o| o.lib), Ok(None));
        for kind in [Kind::Eplan, Kind::Swe, Kind::Aml, Kind::Pdf, Kind::PartsEdz] {
            assert!(!Form::new(&open, kind, &file).has(Opt::Library), "{kind:?}");
        }
        // The symbol map is the SolidWorks project's and the EDZ parts' alone.
        for kind in [Kind::Eplan, Kind::Aml, Kind::Pdf, Kind::Archive, Kind::PageDxf] {
            assert!(!Form::new(&open, kind, &file).has(Opt::SymbolMap), "{kind:?}");
        }
        for bad in ["", "0", "-1", "abc", "inf", "NaN"] {
            let form = Form {
                scale: bad.to_owned(),
                ..Form::new(&open, Kind::PageDwg, &file)
            };
            assert!(form.options().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn new_pages_are_numbered_from_the_first_page_given() {
        let (tmp, mut open) = project();
        let aml = tmp.path().join("plant.aml");
        assert!(std::fs::write(&aml, AML).is_ok());
        let form = Form {
            first_page: "730".to_owned(),
            ..Form::new(&open, Kind::Aml, &aml)
        };
        let options = form.options();
        let Ok(options) = options else {
            unreachable!("{options:?}");
        };
        let before: Vec<String> = open.project.pages.iter().map(|pg| pg.number.clone()).collect();
        let (done, msg) = import_with(&mut open, Kind::Aml, &aml, None, 0, &options);
        assert!(done, "{msg}");
        let new: Vec<String> = open
            .project
            .pages
            .iter()
            .map(|pg| pg.number.clone())
            .filter(|n| !before.contains(n))
            .collect();
        assert_eq!(new.first().map(String::as_str), Some("730"), "{new:?}");
        // The form offers the number after it for the next import.
        assert!(
            Form::new(&open, Kind::Aml, &aml)
                .first_page
                .parse::<usize>()
                .is_ok_and(|n| n > 730)
        );
    }

    /// Write a zip at `path` with `entries` (name, bytes).
    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) -> bool {
        use std::io::Write;
        let Ok(file) = std::fs::File::create(path) else {
            return false;
        };
        let mut z = zip::ZipWriter::new(file);
        let opt = zip::write::SimpleFileOptions::default();
        let written = entries
            .iter()
            .all(|(name, bytes)| z.start_file(*name, opt).is_ok() && z.write_all(bytes).is_ok());
        written && z.finish().is_ok()
    }

    #[test]
    fn the_project_chosen_from_a_solidworks_archive_is_the_one_imported() {
        let (tmp, mut open) = project();
        let archive = tmp.path().join("plant.tewzip");
        // The databases are not real: the import gets as far as taking the chosen one out.
        assert!(write_zip(
            &archive,
            &[
                ("Projects/12/Database/tew_project_data.sqlite", b"twelve"),
                ("Projects/3/Database/tew_project_data.sqlite", b"three"),
            ]
        ));
        let mut form = Form::new(&open, Kind::Swe, &archive);
        assert!(form.has(Opt::Project));
        assert_eq!(form.projects, ["1: Projects/3", "2: Projects/12"]);
        // The first is chosen, as the command line picks 1.
        assert_eq!(form.options().map(|o| o.pick), Ok(1));
        form.project = 1;
        let options = form.options();
        let Ok(options) = options else {
            unreachable!("{options:?}");
        };
        assert_eq!(options.pick, 2);
        let (_, msg) = import_with(&mut open, Kind::Swe, &archive, None, 0, &options);
        assert!(msg.contains("importing number 2 (Projects/12)"), "{msg}");
        let (_, msg) = import(&mut open, Kind::Swe, &archive, None, 0);
        assert!(msg.contains("importing number 1 (Projects/3)"), "{msg}");

        // One project, or a file that is no archive: nothing to choose.
        let single = tmp.path().join("one.tewzip");
        assert!(write_zip(
            &single,
            &[("Projects/3/Database/tew_project_data.sqlite", b"three")]
        ));
        assert!(!Form::new(&open, Kind::Swe, &single).has(Opt::Project));
        let form = Form::new(&open, Kind::Swe, &tmp.path().join("missing.tewzip"));
        assert!(!form.has(Opt::Project) && form.has(Opt::FirstPage));
        assert_eq!(form.options().map(|o| o.pick), Ok(1));
    }

    /// The symbol the one part in the library folder `lib` points at.
    fn part_symbol(lib: &Path) -> Option<String> {
        let file = std::fs::read_dir(lib.join("parts")).ok()?.flatten().next()?.path();
        let part: wc_core::Part = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
        part.symbol
    }

    #[test]
    fn an_edz_part_takes_the_library_symbol_the_symbol_map_gives_its_macro() {
        let (tmp, mut open) = project();
        let edz = tmp.path().join("contactor.edz");
        let xml = r#"<partsmanagement><part P_ARTICLE_PARTNR="ACM.K-1" P_ARTICLE_MANUFACTURER="ACM" P_ARTICLE_DESCR1="en_US@Contactor;" P_ARTICLE_MACRO="ACM\ACM_K1.ema"/></partsmanagement>"#;
        assert!(write_zip(&edz, &[("parts.xml", xml.as_bytes()), ("ACM_K1.ema", b"m")]));

        // No map by default, as on the command line: the part comes in without a symbol.
        let form = Form::new(&open, Kind::PartsEdz, &edz);
        assert!(form.has(Opt::SymbolMap) && !form.has(Opt::FirstPage));
        assert_eq!(form.options(), Ok(Options::default()));
        let plain = tmp.path().join("plain");
        let (done, msg) = import(&mut open, Kind::PartsEdz, &edz, Some(&plain), 0);
        assert!(done, "{msg}");
        assert!(msg.contains("imported 1 part(s)"), "{msg}");
        assert_eq!(part_symbol(&plain), None, "{msg}");

        let map = tmp.path().join("macros.txt");
        assert!(std::fs::write(&map, "ACM_K1 = wc.coil\n").is_ok());
        let mut form = Form::new(&open, Kind::PartsEdz, &edz);
        form.symbol_map.text = map.display().to_string();
        let options = form.options();
        let Ok(options) = options else {
            unreachable!("{options:?}");
        };
        assert_eq!(options.symbol_map.as_deref(), Some(map.as_path()));
        let lib = tmp.path().join("lib");
        let (done, msg) = import_with(&mut open, Kind::PartsEdz, &edz, Some(&lib), 0, &options);
        assert!(done, "{msg}");
        assert_eq!(part_symbol(&lib).as_deref(), Some("wc.coil"), "{msg}");

        // A map that is not there is refused in the form, before the import starts; so it is
        // for a SolidWorks Electrical project, which takes the same file.
        for kind in [Kind::PartsEdz, Kind::Swe] {
            let mut form = Form::new(&open, kind, &edz);
            assert!(form.has(Opt::SymbolMap), "{kind:?}");
            form.symbol_map.text = tmp.path().join("missing.txt").display().to_string();
            assert!(form.options().is_err_and(|e| e.contains("is not a file")), "{kind:?}");
            form.symbol_map.text = map.display().to_string();
            assert_eq!(form.options().map(|o| o.symbol_map), Ok(Some(map.clone())), "{kind:?}");
        }
    }

    /// A SolidWorks Electrical project database with one page and a coil K1 drawn as the block
    /// `Coil`.
    fn swe_database(path: &Path) -> bool {
        let made = || -> rusqlite::Result<Vec<u8>> {
            let c = rusqlite::Connection::open_in_memory()?;
            c.execute_batch(SWE_DATABASE)?;
            Ok(c.serialize(rusqlite::MAIN_DB)?.to_vec())
        };
        made().is_ok_and(|bytes| std::fs::write(path, bytes).is_ok())
    }

    const SWE_DATABASE: &str = "
CREATE TABLE tew_bundle (bun_id INTEGER, bun_tag TEXT);
CREATE TABLE tew_file (fil_id INTEGER, fil_orderno INTEGER, fil_filetype INTEGER, fil_title TEXT, fil_fol_id INTEGER,
  fil_titleblock TEXT DEFAULT '', fil_bun_id INTEGER DEFAULT 1);
CREATE TABLE tew_folder (fol_id INTEGER, fol_name TEXT);
CREATE TABLE tew_translatedtext (tra_objectid INTEGER, tra_objectno INTEGER, tra_lan_strid TEXT, tra_0 TEXT);
CREATE TABLE tew_component (com_id INTEGER, com_tag TEXT, com_tagpath TEXT, com_type INTEGER,
  com_com_id INTEGER, com_tagorderno INTEGER, com_fun_id INTEGER, com_loc_id INTEGER, com_parenttagpath TEXT);
CREATE TABLE tew_buildofmaterial (bom_id INTEGER, bom_objectid INTEGER, bom_orderno INTEGER, bom_reference TEXT, bom_manufacturer TEXT);
CREATE TABLE tew_symbol (sym_id INTEGER, sym_fil_id INTEGER, sym_com_id INTEGER, sym_blockname TEXT, sym_posx REAL, sym_posy REAL,
  sym_angle REAL, sym_scalex REAL, sym_scaley REAL, sym_posendx REAL, sym_posendy REAL);
CREATE TABLE tew_symbolpt (spt_sym_id INTEGER, spt_sct_no INTEGER, spt_no INTEGER, spt_dx REAL, spt_dy REAL);
CREATE TABLE tew_componentelement (cel_id INTEGER, cel_sym_id INTEGER, cel_sct_no INTEGER, cel_com_id INTEGER, cel_orderno INTEGER);
CREATE TABLE tew_componentterminal (cte_cel_id INTEGER, cte_no INTEGER, cte_txt TEXT);
CREATE TABLE tew_line (lin_id INTEGER, lin_fil_id INTEGER, lin_ptstartx REAL, lin_ptstarty REAL, lin_ptendx REAL, lin_ptendy REAL, lin_equ_id INTEGER);
CREATE TABLE tew_equipotential (equ_id INTEGER, equ_text TEXT);
CREATE TABLE tew_cable (cab_id INTEGER, cab_tag TEXT, cab_reference TEXT, cab_manufacturer TEXT);
CREATE TABLE tew_cablewire (caw_cab_id INTEGER, caw_no INTEGER, caw_color TEXT);
CREATE TABLE tew_wire (wir_id INTEGER, wir_cab_id INTEGER, wir_caw_no INTEGER, wir_tag TEXT,
  wir_cel_idfrom INTEGER, wir_cte_nofrom INTEGER, wir_cel_idto INTEGER, wir_cte_noto INTEGER,
  wir_sectiontype INTEGER, wir_sectionorgauge TEXT, wir_color TEXT);
CREATE TABLE tew_linewirepair (lwp_lin_id INTEGER, lwp_wir_id INTEGER, lwp_wir_orderno INTEGER);
INSERT INTO tew_bundle VALUES (1, '1');
INSERT INTO tew_folder VALUES (7, 'Power');
INSERT INTO tew_file (fil_id, fil_orderno, fil_filetype, fil_title, fil_fol_id) VALUES (10, 1, 12, '01', 7);
INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type) VALUES (100, 'K1', '', -1);
INSERT INTO tew_symbol VALUES (1, 10, 100, 'Coil', 100, 200, 0, 1, 1, NULL, NULL);
INSERT INTO tew_symbolpt VALUES (1, 0, 0, 0, 5), (1, 0, 1, 0, -5);
INSERT INTO tew_componentelement (cel_id, cel_sym_id, cel_sct_no) VALUES (50, 1, 0);
INSERT INTO tew_componentterminal VALUES (50, 0, 'A1'), (50, 1, 'A2');
";

    #[test]
    fn a_solidworks_import_reads_the_symbol_map_given_and_none_by_default() {
        let (tmp, mut open) = project();
        let db = tmp.path().join("plant.sqlite");
        assert!(swe_database(&db));
        // The map names a symbol no library has, which the import says of a map it has read.
        let map = tmp.path().join("blocks.txt");
        assert!(std::fs::write(&map, "Coil = nolib.coil\n").is_ok());
        let unknown = "symbol map names nolib.coil";

        let (done, msg) = import(&mut open, Kind::Swe, &db, None, 0);
        assert!(done, "{msg}");
        assert!(!msg.contains(unknown), "{msg}");
        let devices = open.project.devices.len();
        assert!(devices > 0, "{msg}");
        assert!(open.undo());

        let mut form = Form::new(&open, Kind::Swe, &db);
        form.symbol_map.text = map.display().to_string();
        let options = form.options();
        let Ok(options) = options else {
            unreachable!("{options:?}");
        };
        let (done, msg) = import_with(&mut open, Kind::Swe, &db, None, 0, &options);
        assert!(done, "{msg}");
        assert!(msg.contains(unknown), "{msg}");
        assert_eq!(open.project.devices.len(), devices, "{msg}");

        // A map that cannot be read as one stops the import with the line that is wrong.
        assert!(std::fs::write(&map, "no equals sign\n").is_ok());
        let (done, msg) = import_with(&mut open, Kind::Swe, &db, None, 0, &options);
        assert!(!done && msg.contains("line 1"), "{msg}");
    }

    #[test]
    fn an_archives_kicad_library_and_parts_list_go_into_the_library_folder_chosen() {
        let (tmp, mut open) = project();
        let archive = tmp.path().join("vendor.zip");
        let kicad = br#"(kicad_symbol_lib
  (symbol "K" (property "Reference" "K" (at 0 0 0))
    (symbol "K_1_1" (pin passive line (at 0 3 270) (length 1) (name "~") (number "A1")))))"#;
        let parts = b"order number;manufacturer;description\nX-1;Acme;Relay\n";
        assert!(write_zip(
            &archive,
            &[("symbols/My Relays.kicad_sym", kicad), ("lists/parts.csv", parts)]
        ));
        let files = |dir: PathBuf| std::fs::read_dir(dir).map_or(0, |d| d.flatten().count());

        // By default they are listed, not imported, and the advice names the dialog's option.
        let own = open.dir.join(crate::import_symbols::PROJECT_LIBRARY);
        let (done, msg) = import(&mut open, Kind::Archive, &archive, None, 0);
        assert!(done, "{msg}");
        assert!(
            msg.contains("KiCad symbols, not imported: choose a library folder"),
            "{msg}"
        );
        assert!(
            msg.contains("parts list, not imported: choose a library folder"),
            "{msg}"
        );
        assert!(!msg.contains("--"), "{msg}");
        assert!(!own.exists(), "{msg}");

        // The project's own library folder, offered first as for a parts import.
        let mut form = Form::new(&open, Kind::Archive, &archive);
        form.libraries = crate::import_symbols::targets(&open.dir, Vec::new(), &[]);
        assert_eq!(form.library_label(0), "Do not import them");
        assert_eq!(form.library_label(form.libraries.len() + 1), "Another folder…");
        form.library = 1;
        assert_eq!(form.library_target().map(|t| t.root.clone()), Some(own.clone()));
        let options = form.options();
        let Ok(options) = options else {
            unreachable!("{options:?}");
        };
        assert_eq!(options.lib.as_deref(), Some(own.as_path()));
        let (done, msg) = import_with(&mut open, Kind::Archive, &archive, None, 0, &options);
        assert!(done, "{msg}");
        assert!(!msg.contains("not imported"), "{msg}");
        assert!(msg.contains("imported 1 part(s)"), "{msg}");
        assert!(own.join("symbols").join("kicad.my_relays.k.json").exists(), "{msg}");
        assert_eq!(files(own.join("parts")), 1, "{msg}");

        // Another folder: it must be given, and is the one written to.
        form.library = form.libraries.len() + 1;
        assert!(form.other_library() && form.library_target().is_none());
        assert!(form.options().is_err_and(|e| e.contains("library folder")));
        let other = tmp.path().join("shared lib");
        form.library_folder.text = other.display().to_string();
        let options = form.options();
        let Ok(options) = options else {
            unreachable!("{options:?}");
        };
        assert_eq!(options.lib.as_deref(), Some(other.as_path()));
        let (done, msg) = import_with(&mut open, Kind::Archive, &archive, None, 0, &options);
        assert!(done, "{msg}");
        assert!(other.join("symbols").join("kicad.my_relays.k.json").exists(), "{msg}");
        assert_eq!(files(other.join("parts")), 1, "{msg}");
    }

    #[test]
    fn a_worker_that_panics_ends_the_import_saying_it_stopped_part_way() {
        let (_tmp, mut open) = project();
        let before = open.project.clone();
        let work: Work = Box::new(|log| {
            log.out("read 3 pages".to_owned());
            // Unwinds as a panic does, without the panic message in the test output.
            std::panic::resume_unwind(Box::new("the importer broke"))
        });
        let (lines, done) = spawn(Ok(work));
        let job = Job {
            kind: Kind::Aml,
            name: "plant.aml".to_owned(),
            dir: open.dir.clone(),
            lib: None,
            note: String::new(),
            adopt: Adopt::Project,
            lines,
            done,
        };
        // As the app looks: without waiting, until the worker is gone.
        let mut ended = None;
        for _ in 0..1000 {
            ended = job.poll();
            if ended.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let Some((r, log)) = ended else {
            unreachable!("the import did not end");
        };
        assert!(matches!(&r, Err(e) if e.contains("stopped part way")), "{r:?}");
        assert!(log.text().contains("read 3 pages"), "{}", log.text());
        let (done, msg) = job.finish(&mut open, r, &log);
        assert!(!done, "{msg}");
        assert!(
            msg.starts_with("plant.aml was not imported: the import stopped part way"),
            "{msg}"
        );
        assert!(msg.contains("read 3 pages"), "{msg}");
        assert_eq!(open.project, before);
    }

    #[test]
    fn advice_names_no_command_line_option() {
        for said in [
            "page number 5 is taken; import with --first-page to number the pages from a free number",
            "page dxf-300 exists; remove it or import with --first-page to number the pages from a free number",
            "page pdf-200 exists; pick another --first-page",
            "no project 2 in the archive (use --pick)",
            "warning: a.kicad_sym: KiCad symbols, not imported: pass --lib <folder>",
            "no importable PDF, DXF, AutomationML, KiCad symbols, CSV lists or Excel lists inside; run `wiringcraft inspect` to see what is there",
            "no WiringCraft project in /tmp/x (no project.toml): use -C <folder> or create one with wiringcraft new <folder>",
        ] {
            let app = app_wording(said);
            assert!(!app.contains("remove it or"), "{app}");
            assert!(!app.contains("--") && !app.contains("wiringcraft"), "{app}");
        }
    }

    /// A small AutomationML plant: two devices and a wire.
    pub(crate) const AML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<CAEXFile FileName="plant.aml" SchemaVersion="3.0" xmlns="http://www.dke.de/CAEX">
  <InstanceHierarchy Name="Plant">
    <InternalElement Name="=PUMP" ID="f1">
      <InternalElement Name="K1" ID="e1">
        <Attribute Name="ReferenceDesignation"><Value>=PUMP+CAB1-K1</Value></Attribute>
        <ExternalInterface Name="A1" ID="i1"/>
      </InternalElement>
      <InternalElement Name="X1" ID="e2">
        <ExternalInterface Name="1" ID="i3"/>
      </InternalElement>
      <InternalLink Name="101" RefPartnerSideA="i1" RefPartnerSideB="i3"/>
    </InternalElement>
  </InstanceHierarchy>
</CAEXFile>
"#;
}

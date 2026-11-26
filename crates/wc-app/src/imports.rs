//! File > Import: projects, drawings and parts in the formats `wiringcraft import` reads, run
//! through the same functions as the CLI ([`wc_import::run`]). The file and, for parts, the
//! library folder come from the system pickers. The import runs on a worker thread ([`Job`])
//! while the window shows what it has said so far; at the end what it said is shown in a
//! message.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

use wc_import::run;

use crate::actions::Action;
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
/// the open project (a DWG or DXF drawing onto page `page`). Changes not saved yet are saved
/// first, since the import works on the project folder. The import runs on a worker thread;
/// a reason it cannot start (no free page number, say) comes back from the job as its result.
pub fn start(open: &mut Open, kind: Kind, file: &Path, lib: Option<&Path>, page: usize) -> Job {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut note = String::new();
    let f = file.to_path_buf();
    let (adopt, work): (Adopt, Result<Work, String>) = if kind.needs_library() {
        match lib.map(Path::to_path_buf) {
            Some(lib) => {
                let work: Work = match kind {
                    Kind::PartsEdz => Box::new(move |log| run::parts_edz(&f, &lib, None, log)),
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
            Ok(()) => (Adopt::Project, project_work(open, kind, f, page)),
        }
    };
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
    Job {
        kind,
        name,
        dir: open.dir.clone(),
        lib: lib.map(Path::to_path_buf),
        note,
        adopt,
        lines,
        done,
    }
}

/// The import of `file` into the project folder, ready to run on the worker thread, or why
/// it cannot run.
fn project_work(open: &Open, kind: Kind, f: PathBuf, page: usize) -> Result<Work, String> {
    let dir = open.dir.clone();
    // Only for the imports that add pages, numbered from a free number.
    let first = || {
        first_free_page(open, kind.first_page()).ok_or_else(|| {
            "no page number is free after the project's highest one; renumber that page first".to_owned()
        })
    };
    Ok(match kind {
        Kind::Eplan => {
            let n = first()?;
            Box::new(move |log| run::eplan(&dir, &f, n, log))
        }
        Kind::Swe => {
            let n = first()?;
            Box::new(move |log| run::swe(&dir, &f, 1, Some(n), None, log))
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
            Box::new(move |log| run::archive(&dir, &f, n, None, log))
        }
        Kind::PlcCsv => Box::new(move |log| run::plc_csv(&dir, &f, log)),
        _ => {
            let Some(pg) = open.project.pages.get(page) else {
                return Err("there is no page to draw it on; add a page first".to_owned());
            };
            let id = pg.id.clone();
            if kind == Kind::PageDxf {
                Box::new(move |log| run::page_dxf(&dir, &f, &id, 1.0, log))
            } else {
                let converter = std::env::var("WIRINGCRAFT_DWG2DXF").ok();
                Box::new(move |log| run::import_page_dwg(&dir, &f, &id, 1.0, converter, log))
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

/// Import `file` as `kind` and wait for it: [`start`], then [`Job::finish`]. Returns the text
/// for the message dialog and whether it was done. The app polls instead ([`Job::poll`]).
#[cfg(test)]
pub fn import(open: &mut Open, kind: Kind, file: &Path, lib: Option<&Path>, page: usize) -> (bool, String) {
    let job = start(open, kind, file, lib, page);
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
            "the app does not import these from an archive yet",
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

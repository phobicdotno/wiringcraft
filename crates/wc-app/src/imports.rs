//! File > Import: projects, drawings and parts in the formats `wiringcraft import` reads, run
//! through the same functions as the CLI ([`wc_import::run`]). The file and, for parts, the
//! library folder come from the system pickers; what the import says is shown in a message.

use std::path::Path;

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
/// numeric page when that is higher, so the new pages never take a number in use.
pub fn first_free_page(open: &Open, default: usize) -> usize {
    open.project
        .pages
        .iter()
        .filter_map(|pg| pg.number.trim().parse::<usize>().ok())
        .map(|n| n + 1)
        .fold(default, usize::max)
}

/// Import `file` as `kind`: parts into the library folder `lib`, everything else into the open
/// project (a DWG drawing onto page `page`). Changes not saved yet are saved first, since the
/// import works on the project folder; what it added comes back as one Undo step. Returns the
/// text for the message dialog (the import's report and warnings, or why it failed), and
/// whether it was done.
pub fn import(open: &mut Open, kind: Kind, file: &Path, lib: Option<&Path>, page: usize) -> (bool, String) {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut log = run::Log::default();
    let mut note = String::new();
    let r = if kind.needs_library() {
        match lib {
            Some(lib) => {
                let r = match kind {
                    Kind::PartsEdz => run::parts_edz(file, lib, None, &mut log),
                    Kind::PartsBmecat => run::parts_bmecat(file, lib, &mut log),
                    _ => run::parts_csv(file, lib, &mut log),
                };
                open.refresh_library();
                r
            }
            None => Err("no library folder was picked for the parts".to_owned()),
        }
    } else {
        let saved = if open.dirty {
            note = "The changes not saved yet were saved first.\n".to_owned();
            open.save().map(|_| ())
        } else {
            Ok(())
        };
        match saved {
            Err(e) => Err(format!("the project could not be saved first: {e}")),
            Ok(()) => {
                let dir = open.dir.clone();
                let first = first_free_page(open, kind.first_page());
                let r = match kind {
                    Kind::Eplan => run::eplan(&dir, file, first, &mut log),
                    Kind::Swe => run::swe(&dir, file, 1, None, None, &mut log),
                    Kind::SweLibrary => run::swe_library(&dir, file, None, &mut log),
                    Kind::Aml => run::aml(&dir, file, first, &mut log),
                    Kind::Pdf => run::pdf(&dir, file, first, &mut log),
                    Kind::Archive => run::archive(&dir, file, first, None, &mut log),
                    Kind::PlcCsv => run::plc_csv(&dir, file, &mut log),
                    _ => match open.project.pages.get(page) {
                        Some(pg) if kind == Kind::PageDxf => run::page_dxf(&dir, file, &pg.id.clone(), 1.0, &mut log),
                        Some(pg) => {
                            let id = pg.id.clone();
                            let converter = std::env::var("WIRINGCRAFT_DWG2DXF").ok();
                            run::import_page_dwg(&dir, file, &id, 1.0, converter, &mut log)
                        }
                        None => Err("there is no page to draw it on; add a page first".to_owned()),
                    },
                };
                // What was written comes in also when a part of an archive failed.
                match open.adopt_saved() {
                    Ok(_) => r,
                    Err(e) => r.and(Err(format!("imported, but the project could not be read back: {e}"))),
                }
            }
        }
    };
    let said = log.text();
    let done = r.is_ok();
    let head = match r {
        Ok(()) => format!("Imported {name}."),
        Err(e) => format!("{name} was not imported: {e}"),
    };
    let mut msg = format!("{note}{head}");
    if let Some(lib) = lib.filter(|_| kind.needs_library()) {
        msg.push_str(&format!("\nLibrary folder: {}", lib.display()));
    }
    if !said.is_empty() {
        msg.push_str("\n\n");
        msg.push_str(&said);
    }
    (done, app_wording(&msg))
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
mod tests {
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
        assert_eq!(first_free_page(&open, 100), 100);
        let i = open.add_page();
        if let Some(pg) = open.project.pages.get_mut(i) {
            pg.number = "250".into();
        }
        assert_eq!(first_free_page(&open, 100), 251);
        assert_eq!(first_free_page(&open, 400), 400);
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

    const AML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
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

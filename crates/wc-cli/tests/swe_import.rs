//! `wiringcraft import swe` run as a program on a small SolidWorks Electrical project database
//! built here, so what the command saves is what is checked, not the functions it calls.

use std::path::Path;
use std::process::Command;

use wc_core::{DeviceTag, PlacedSymbol, Point, store};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The tables the importer reads, as the module docs of `wc_import::swe` describe them.
const SCHEMA: &str = "
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
";

/// A project database with one schematic page: strip X1 with terminals 2 and 9, and a jumper
/// between them recorded in the terminal strip editor (a wire record no line carries).
fn database(path: &Path) -> TestResult {
    let c = rusqlite::Connection::open(path)?;
    c.execute_batch(SCHEMA)?;
    c.execute_batch(
        "
INSERT INTO tew_bundle VALUES (1, '1');
INSERT INTO tew_folder VALUES (7, 'Power');
INSERT INTO tew_file (fil_id, fil_orderno, fil_filetype, fil_title, fil_fol_id) VALUES (10, 1, 12, '01', 7);
INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type, com_com_id, com_tagorderno)
  VALUES (200, 'X1', '-X1', 3, NULL, 0), (201, '2', '-X1 2', 4, 200, 1), (202, '9', '-X1 9', 4, 200, 2);
INSERT INTO tew_symbol VALUES (20, 10, 201, 'Terminal', 150, 200, 0, 1, 1, NULL, NULL),
  (21, 10, 202, 'Terminal', 170, 200, 0, 1, 1, NULL, NULL);
INSERT INTO tew_symbolpt VALUES (20, 0, 0, 0, 5), (20, 0, 1, 0, -5), (21, 0, 0, 0, 5), (21, 0, 1, 0, -5);
INSERT INTO tew_componentelement VALUES (60, 20, 0, 201, 0), (61, 21, 0, 202, 0);
INSERT INTO tew_componentterminal VALUES (60, 0, NULL), (60, 1, NULL), (61, 0, NULL), (61, 1, NULL);
INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag, wir_cel_idfrom, wir_cte_nofrom, wir_cel_idto, wir_cte_noto)
  VALUES (90, NULL, -1, '', 60, 0, 61, 1);
",
    )?;
    Ok(())
}

/// The program's stdout; an error with its stderr when it fails.
fn run(dir: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8(out.stderr)?.into());
    }
    Ok(String::from_utf8(out.stdout)?)
}

/// The jumpers' terminal sets, each sorted, in order.
fn jumper_sets(p: &wc_core::Project) -> Vec<Vec<String>> {
    let mut sets: Vec<Vec<String>> = p
        .meta
        .jumpers
        .iter()
        .map(|j| {
            let mut t = j.terminals.clone();
            t.sort();
            t
        })
        .collect();
    sets.sort();
    sets
}

#[test]
fn an_imported_jumper_stays_on_the_sources_terminals_when_the_import_renumbers_the_strip() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("p");
    let status = Command::new(env!("CARGO_BIN_EXE_wiringcraft"))
        .arg("new")
        .arg(&dir)
        .arg("--no-git")
        .status()?;
    assert!(status.success());

    // The project's own strip: -X1.1, then two terminals tagged with only the strip (numbered 2
    // and 3), jumpered.
    let term = wc_core::Symbol {
        id: "test.term".into(),
        name: "Terminal".into(),
        category: String::new(),
        letter_code: "X".into(),
        role: wc_core::symbol::SymbolRole::Terminal,
        graphics: vec![],
        pins: vec![],
        tag_at: Default::default(),
        description: String::new(),
    };
    store::write_vendored_symbol(&dir, &term)?;
    let terminal = |id: &str, x: f64, tag: &str| -> Result<PlacedSymbol, wc_core::Error> {
        Ok(PlacedSymbol {
            id: id.into(),
            symbol: "test.term".into(),
            at: Point::new(x, 10.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(DeviceTag::parse(tag)?),
        })
    };
    let mut p = store::load(&dir)?;
    p.pages[0].symbols.extend([
        terminal("a", 10.0, "-X1.1")?,
        terminal("b", 20.0, "-X1")?,
        terminal("c", 30.0, "-X1")?,
    ]);
    let x1 = DeviceTag::parse("-X1")?;
    p.meta.add_jumper(&x1, &["2".into(), "3".into()])?;
    store::save(&dir, &p)?;

    // The import brings -X1.2 and -X1.9, jumpered: the strip-only terminals become 10 and 11.
    let db = tmp.path().join("project.sqlite");
    database(&db)?;
    let out = run(&dir, &["import", "swe", db.to_str().ok_or("path")?])?;
    assert!(out.contains("terminal jumpers"), "{out}");

    // Saved as the command left it: the project's jumper follows its terminals, the imported
    // one stays on the source's.
    let p = store::load(&dir)?;
    assert_eq!(jumper_sets(&p), [vec!["10", "11"], vec!["2", "9"]]);

    // And the WAGO configurator list built from the saved project says the same.
    let csv = tmp.path().join("wago.csv");
    run(&dir, &["export", "wago-configurator", csv.to_str().ok_or("path")?])?;
    let csv = std::fs::read_to_string(&csv)?;
    let mut connects: Vec<String> = csv
        .lines()
        .map(|l| l.split(';').collect::<Vec<_>>())
        .filter(|r| r.get(1) == Some(&"Jumper"))
        .filter_map(|r| r.get(14).map(|s| (*s).to_owned()))
        .collect();
    connects.sort();
    assert_eq!(connects, ["10, 11", "2, 9"], "{csv}");
    Ok(())
}

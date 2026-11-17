//! Tests on small databases built here with the table layout the module docs describe. No real
//! project files are used.
#![allow(clippy::unwrap_used)]

use super::*;

const SCHEMA: &str = "
CREATE TABLE tew_file (fil_id INTEGER, fil_orderno INTEGER, fil_filetype INTEGER, fil_title TEXT, fil_fol_id INTEGER,
  fil_titleblock TEXT DEFAULT '');
CREATE TABLE tew_folder (fol_id INTEGER, fol_name TEXT);
CREATE TABLE tew_translatedtext (tra_objectid INTEGER, tra_objectno INTEGER, tra_lan_strid TEXT, tra_0 TEXT);
CREATE TABLE tew_component (com_id INTEGER, com_tag TEXT, com_tagpath TEXT, com_type INTEGER);
CREATE TABLE tew_buildofmaterial (bom_id INTEGER, bom_objectid INTEGER, bom_orderno INTEGER, bom_reference TEXT, bom_manufacturer TEXT);
CREATE TABLE tew_symbol (sym_id INTEGER, sym_fil_id INTEGER, sym_com_id INTEGER, sym_blockname TEXT, sym_posx REAL, sym_posy REAL,
  sym_angle REAL, sym_scalex REAL, sym_scaley REAL, sym_posendx REAL, sym_posendy REAL);
CREATE TABLE tew_symbolpt (spt_sym_id INTEGER, spt_sct_no INTEGER, spt_no INTEGER, spt_dx REAL, spt_dy REAL);
CREATE TABLE tew_componentelement (cel_id INTEGER, cel_sym_id INTEGER, cel_sct_no INTEGER, cel_com_id INTEGER, cel_orderno INTEGER);
CREATE TABLE tew_componentterminal (cte_cel_id INTEGER, cte_no INTEGER, cte_txt TEXT);
CREATE TABLE tew_line (lin_id INTEGER, lin_fil_id INTEGER, lin_ptstartx REAL, lin_ptstarty REAL, lin_ptendx REAL, lin_ptendy REAL, lin_equ_id INTEGER);
CREATE TABLE tew_equipotential (equ_id INTEGER, equ_text TEXT);
";

/// A two-page project: a relay coil K1 (pins A1 above, A2 below) and a fuse F1 rotated by 90
/// degrees on a schematic page, wired F1 -> K1:A1 with potential 101; a report page; a frame
/// without connection points.
fn sample() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(SCHEMA).unwrap();
    c.execute_batch(
        "
INSERT INTO tew_folder VALUES (7, 'Power');
INSERT INTO tew_file (fil_id, fil_orderno, fil_filetype, fil_title, fil_fol_id) VALUES (10, 2, 12, '01', 7), (11, 1, 2, '00', 7);
INSERT INTO tew_translatedtext VALUES (10, -1, 'de', 'Stromlaufplan'), (10, -1, 'en', 'Main supply'),
  (100, -1, 'en', 'Pump relay');
INSERT INTO tew_component VALUES (100, 'K1', '=F1+L1+L2-K1', -1), (101, 'F1', '', -1), (102, '', '', -1);
INSERT INTO tew_buildofmaterial VALUES (1, 100, 1, 'RLY-24', 'Maker A'), (2, 100, 2, 'SOCKET', 'Maker B');
-- K1 at (100, 200): A1 5 mm above, A2 5 mm below (y up).
INSERT INTO tew_symbol VALUES (1, 10, 100, 'Coil', 100, 200, 0, 1, 1, NULL, NULL);
INSERT INTO tew_symbolpt VALUES (1, 0, 0, 0, 5), (1, 0, 1, 0, -5);
INSERT INTO tew_componentelement (cel_id, cel_sym_id, cel_sct_no) VALUES (50, 1, 0);
INSERT INTO tew_componentterminal VALUES (50, 0, 'A1'), (50, 1, 'A2');
-- F1 at (60, 220) rotated 90: offsets (+-4, 0) land 4 mm above and below.
INSERT INTO tew_symbol VALUES (2, 10, 101, 'Fuse', 60, 220, 90, 1, 1, NULL, NULL);
INSERT INTO tew_symbolpt VALUES (2, 0, 0, -4, 0), (2, 0, 1, 4, 0);
-- A frame without points, and a symbol on the report page.
INSERT INTO tew_symbol VALUES (3, 10, NULL, 'Frame', 0, 0, 0, 1, 1, NULL, NULL);
INSERT INTO tew_symbol VALUES (4, 11, 102, 'Text', 10, 10, 0, 1, 1, NULL, NULL);
-- Wire F1 top (60, 224) -> (100, 224) -> K1:A1 (100, 205).
INSERT INTO tew_equipotential VALUES (900, '101');
INSERT INTO tew_line VALUES (1, 10, 60, 224, 100, 224, 900), (2, 10, 100, 224, 100, 205, 900);
",
    )
    .unwrap();
    c
}

#[test]
fn place_scales_then_rotates() {
    let p = place(Point::new(10.0, 20.0), 90.0, (1.0, 1.0), Point::new(4.0, 0.0));
    assert!((p.x - 10.0).abs() < 1e-9 && (p.y - 24.0).abs() < 1e-9);
    // Mirrored in x, no rotation.
    let p = place(Point::new(10.0, 20.0), 0.0, (-1.0, 1.0), Point::new(4.0, 1.0));
    assert!((p.x - 6.0).abs() < 1e-9 && (p.y - 21.0).abs() < 1e-9);
}

#[test]
fn read_pages_components_symbols_and_lines() {
    let p = read(&sample()).unwrap();
    assert_eq!(p.pages.len(), 2);
    let sch = p.pages.iter().find(|pg| pg.id == 10).unwrap();
    assert_eq!(sch.title, "Main supply", "English text wins over German");
    assert_eq!(sch.folder, "Power");
    let k1 = &p.components[&100];
    assert_eq!(k1.tag, "=F1+L1+L2-K1");
    assert_eq!(k1.description, "Pump relay");
    assert_eq!(
        (k1.part_number.as_str(), k1.manufacturer.as_str()),
        ("RLY-24", "Maker A"),
        "first part"
    );
    assert_eq!(p.components[&101].tag, "F1", "bare tag when there is no tag path");
    let coil = p.symbols.iter().find(|s| s.id == 1).unwrap();
    let names: Vec<&str> = coil.pins.iter().map(|q| q.name.as_str()).collect();
    assert_eq!(names, ["A1", "A2"]);
    assert_eq!(coil.pins[0].at, Point::new(100.0, 205.0));
    let fuse = p.symbols.iter().find(|s| s.id == 2).unwrap();
    assert_eq!(
        fuse.pins.iter().map(|q| q.name.as_str()).collect::<Vec<_>>(),
        ["1", "2"],
        "numbered without terminals"
    );
    assert!((fuse.pins[1].at.y - 224.0).abs() < 1e-9);
    assert_eq!(p.lines.len(), 2);
    assert_eq!(p.lines[0].potential.as_deref(), Some("101"));
}

#[test]
fn device_tags_from_tag_paths() {
    assert_eq!(device_tag("=F1+L1+L2-K1").unwrap().to_string(), "=F1+L1.L2-K1");
    assert_eq!(device_tag("K3").unwrap().to_string(), "-K3");
    assert_eq!(device_tag("-X 1/2").unwrap().to_string(), "-X_1_2");
    assert!(device_tag("").is_none());
    assert!(device_tag("+L1").is_none(), "a location alone is not a device");
}

#[test]
fn build_draws_schematic_pages_with_joined_wires() {
    let p = read(&sample()).unwrap();
    let (built, warnings) = build(&p, 5);
    assert_eq!(built.pages.len(), 1, "the report page is not drawn");
    assert!(warnings.iter().any(|w| w.contains("1 page(s)")));
    assert!(
        warnings.iter().any(|w| w.contains("without connection points")),
        "the frame is reported"
    );
    let page = &built.pages[0];
    assert_eq!(page.number, "5");
    assert_eq!(page.title, "Power 01: Main supply");
    assert_eq!(page.size, PageSize::A3);
    assert_eq!(page.symbols.len(), 2);
    let k1 = page.symbols.iter().find(|s| s.id == "s1").unwrap();
    assert_eq!(k1.tag.as_ref().unwrap().to_string(), "=F1+L1.L2-K1");
    // y flipped on an A3 sheet (297 mm high).
    assert_eq!(k1.at, Point::new(100.0, 97.0));
    // Wire 1: F1 pin 2 -> free point; wire 2: free point -> K1:A1.
    assert_eq!(page.wires.len(), 2);
    assert_eq!(
        page.wires[0].from,
        Endpoint::Pin {
            item: "s2".into(),
            pin: "2".into()
        }
    );
    assert_eq!(
        page.wires[0].to,
        Endpoint::Point {
            at: Point::new(100.0, 73.0)
        }
    );
    assert_eq!(
        page.wires[1].to,
        Endpoint::Pin {
            item: "s1".into(),
            pin: "A1".into()
        }
    );
    assert_eq!(page.wires[1].number.as_deref(), Some("101"));
    // The coil's box symbol: pins above and below, pointing out of the box.
    let sym = built.symbols.iter().find(|s| s.id == k1.symbol).unwrap();
    let a1 = sym.pin("A1").unwrap();
    assert_eq!(a1.at, Point::new(0.0, -5.0));
    assert_eq!(a1.dir, PinDir::Up);
    assert_eq!(sym.pin("A2").unwrap().dir, PinDir::Down);
    // Devices with part data; the component only on the report page stays out.
    let dev = &built.devices[&DeviceTag::parse("=F1+L1.L2-K1").unwrap()];
    assert_eq!(dev.description, "Pump relay");
    assert_eq!(dev.properties["PartNumber"], "RLY-24");
    assert_eq!(dev.properties["Manufacturer"], "Maker A");
    assert!(built.devices.contains_key(&DeviceTag::parse("-F1").unwrap()));
    assert_eq!(built.devices.len(), 2);
}

#[test]
fn identical_symbols_share_one_library_symbol() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_component VALUES (103, 'K2', '', -1);
         INSERT INTO tew_symbol VALUES (5, 10, 103, 'Coil', 150, 200, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (5, 0, 0, 0, 5), (5, 0, 1, 0, -5);
         INSERT INTO tew_componentelement (cel_id, cel_sym_id, cel_sct_no) VALUES (51, 5, 0);
         INSERT INTO tew_componentterminal VALUES (51, 0, 'A1'), (51, 1, 'A2');",
    )
    .unwrap();
    let (built, _) = build(&read(&c).unwrap(), 1);
    let page = &built.pages[0];
    let k1 = page.symbols.iter().find(|s| s.id == "s1").unwrap();
    let k2 = page.symbols.iter().find(|s| s.id == "s5").unwrap();
    assert_eq!(k1.symbol, k2.symbol);
    assert_eq!(built.symbols.len(), 2, "one coil box, one fuse box");
}

#[test]
fn duplicate_tags_are_made_unique() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_component VALUES (104, 'K1', '=F1+L1+L2-K1', -1);
         INSERT INTO tew_symbol VALUES (6, 10, 104, 'Coil', 200, 200, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (6, 0, 0, 0, 5);",
    )
    .unwrap();
    let (built, _) = build(&read(&c).unwrap(), 1);
    assert!(built.devices.contains_key(&DeviceTag::parse("=F1+L1.L2-K1_2").unwrap()));
}

#[test]
fn large_pages_move_to_a2_and_odd_rows_are_skipped() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_line VALUES (3, 10, 400, 380, 500, 380, NULL);
         INSERT INTO tew_symbol VALUES (7, 10, NULL, 'Broken', NULL, 5, 0, 1, 1, NULL, NULL);",
    )
    .unwrap();
    let p = read(&c).unwrap();
    assert!(p.warnings.iter().any(|w| w.contains("without a position")));
    let (built, _) = build(&p, 1);
    assert_eq!(built.pages[0].size, PageSize::A2);
}

#[test]
fn missing_tables_are_an_error_not_a_panic() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE tew_file (fil_id INTEGER);").unwrap();
    assert!(read(&c).is_err());
}

#[test]
fn archive_databases_are_found_and_opened() {
    use std::io::Write;
    let db = sample().serialize(rusqlite::MAIN_DB).unwrap().to_vec();
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opt = zip::write::SimpleFileOptions::default();
        for name in [
            "Projects/12/Database/tew_project_data.sqlite",
            "Projects/3/Database/tew_project_data.sqlite",
            "Projects/3/Snapshots/9/Database/tew_project_data.sqlite",
        ] {
            z.start_file(name, opt).unwrap();
            z.write_all(&db).unwrap();
        }
        z.start_file("Archive.ini", opt).unwrap();
        z.write_all(b"[ARCHIVEINFO]\n").unwrap();
        z.finish().unwrap();
    }
    let found = databases_in_archive(buf.get_ref()).unwrap();
    let folders: Vec<&str> = found.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(
        folders,
        ["Projects/3", "Projects/12"],
        "numeric order, snapshots skipped"
    );
    let p = read(&open(&found[0].1).unwrap()).unwrap();
    assert_eq!(p.symbols.len(), 4);
    assert!(databases_in_archive(b"not a zip").is_err());
}

#[test]
fn arrows_become_interruption_points_with_one_tag_per_pair() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_file (fil_id, fil_orderno, fil_filetype, fil_title, fil_fol_id) VALUES (12, 3, 0, '02', 7);
         INSERT INTO tew_component VALUES (200, '', '=F1+L1-', 5);
         INSERT INTO tew_symbol VALUES (8, 10, 200, 'Arrow out', 140, 224, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (8, 0, 0, 0, 0);
         INSERT INTO tew_symbol VALUES (9, 12, 200, 'Arrow in', 20, 100, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (9, 0, 0, 0, 0);",
    )
    .unwrap();
    let (built, warnings) = build(&read(&c).unwrap(), 1);
    assert_eq!(built.pages.len(), 2, "a schematic sheet (type 0) is drawn too");
    let out = built.pages[0].symbols.iter().find(|s| s.id == "s8").unwrap();
    let inn = built.pages[1].symbols.iter().find(|s| s.id == "s9").unwrap();
    assert_eq!(out.tag, inn.tag);
    assert_eq!(out.tag.as_ref().unwrap().to_string(), "-OD1");
    let sym = built.symbols.iter().find(|s| s.id == out.symbol).unwrap();
    assert_eq!(sym.role, wc_core::symbol::SymbolRole::Interruption);
    assert!(
        !built.devices.contains_key(&DeviceTag::parse("-OD1").unwrap()),
        "arrows are not devices"
    );
    // Only the sample's untagged text component is reported, not the arrow.
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("1 component(s) without a usable tag"))
    );
}

#[test]
fn one_database_is_taken_from_an_archive_stream() {
    use std::io::Write;
    let db = sample().serialize(rusqlite::MAIN_DB).unwrap().to_vec();
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opt = zip::write::SimpleFileOptions::default();
        for name in [
            "Projects/12/Database/tew_project_data.sqlite",
            "Projects/3/Database/tew_project_data.sqlite",
        ] {
            z.start_file(name, opt).unwrap();
            z.write_all(&db).unwrap();
        }
        z.finish().unwrap();
    }
    buf.set_position(0);
    let got = database_from_archive(buf.clone(), 2).unwrap();
    assert_eq!((got.folder.as_str(), got.count), ("Projects/12", 2));
    assert_eq!(got.bytes, db);
    assert!(database_from_archive(buf.clone(), 0).is_err());
    assert!(database_from_archive(buf, 3).is_err());
}

#[test]
fn sheet_size_from_the_title_block_name() {
    assert_eq!(frame_size("FRAME_BASE_A2_EN-20200101120000"), Some(PageSize::A2));
    assert_eq!(frame_size("FRAME_BASEA3X_EN"), Some(PageSize::A3));
    assert_eq!(frame_size("frame a4 portrait"), Some(PageSize::A4));
    assert_eq!(frame_size("A20 custom"), None);
    assert_eq!(frame_size("Info page"), None);
}

#[test]
fn box_size_is_relative_and_a_shared_function_is_left_out() {
    let c = sample();
    // A black box 35 wide and 40 high below its insertion point, on an A2 frame page.
    c.execute_batch(
        "UPDATE tew_file SET fil_titleblock = 'BASE_A2_EN' WHERE fil_id = 10;
         INSERT INTO tew_component VALUES (105, 'A1', '=F1+L1-A1', -1);
         INSERT INTO tew_symbol VALUES (10, 10, 105, 'Box', 20, 300, 0, 1, 1, 35, -40);",
    )
    .unwrap();
    // K1 and A1 both carry =F1, F1 has no function: not shared by all, kept.
    let (built, _) = build(&read(&c).unwrap(), 1);
    let page = &built.pages[0];
    assert_eq!(page.size, PageSize::A2, "named by the title block");
    let a1 = page.symbols.iter().find(|s| s.id == "s10").unwrap();
    assert_eq!(a1.tag.as_ref().unwrap().to_string(), "=F1+L1-A1");
    let sym = built.symbols.iter().find(|s| s.id == a1.symbol).unwrap();
    assert_eq!(
        sym.graphics[0],
        Shape::Rect {
            min: Point::new(0.0, 0.0),
            max: Point::new(35.0, 40.0)
        }
    );

    // With =F1 on every device it goes.
    c.execute_batch("UPDATE tew_component SET com_tagpath = '=F1-F1' WHERE com_id = 101;")
        .unwrap();
    let (built, warnings) = build(&read(&c).unwrap(), 1);
    let a1 = built.pages[0].symbols.iter().find(|s| s.id == "s10").unwrap();
    assert_eq!(a1.tag.as_ref().unwrap().to_string(), "+L1-A1");
    assert!(warnings.iter().any(|w| w.contains("function =F1")));
}

#[test]
fn a_strip_drawn_as_several_symbols_gets_distinct_pin_names() {
    let c = sample();
    // Strip X1: two terminal symbols, each two points, terminals without text.
    c.execute_batch(
        "INSERT INTO tew_component VALUES (300, 'X1', '-X1', 3);
         INSERT INTO tew_symbol VALUES (20, 10, 300, 'Terminal', 200, 100, 0, 1, 1, NULL, NULL),
                                       (21, 10, 300, 'Terminal', 205, 100, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (20, 0, 0, 0, 5), (20, 0, 1, 0, -5), (21, 0, 0, 0, 5), (21, 0, 1, 0, -5);
         INSERT INTO tew_componentelement VALUES (60, 20, 0, 300, 0), (61, 21, 0, 300, 1);
         INSERT INTO tew_componentterminal VALUES (60, 0, ''), (60, 1, ''), (61, 0, ''), (61, 1, '');",
    )
    .unwrap();
    let p = read(&c).unwrap();
    let names = |id: i64| -> Vec<String> {
        p.symbols
            .iter()
            .find(|s| s.id == id)
            .unwrap()
            .pins
            .iter()
            .map(|q| q.name.clone())
            .collect()
    };
    assert_eq!(names(20), ["1.1", "1.2"]);
    assert_eq!(names(21), ["2.1", "2.2"]);
    // A device with one symbol keeps plain numbers (the fuse).
    assert_eq!(names(2), ["1", "2"]);
}

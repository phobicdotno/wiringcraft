//! Tests on small databases built here with the table layout the module docs describe. No real
//! project files are used.
#![allow(clippy::unwrap_used)]

use super::*;

const SCHEMA: &str = "
CREATE TABLE tew_bundle (bun_id INTEGER, bun_tag TEXT);
CREATE TABLE tew_file (fil_id INTEGER, fil_orderno INTEGER, fil_filetype INTEGER, fil_title TEXT, fil_fol_id INTEGER,
  fil_titleblock TEXT DEFAULT '', fil_bun_id INTEGER DEFAULT 1);
CREATE TABLE tew_folder (fol_id INTEGER, fol_name TEXT);
CREATE TABLE tew_translatedtext (tra_objectid INTEGER, tra_objectno INTEGER, tra_lan_strid TEXT, tra_0 TEXT);
CREATE TABLE tew_component (com_id INTEGER, com_tag TEXT, com_tagpath TEXT, com_type INTEGER,
  com_com_id INTEGER, com_tagorderno INTEGER);
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
  wir_cel_idfrom INTEGER, wir_cte_nofrom INTEGER, wir_cel_idto INTEGER, wir_cte_noto INTEGER);
CREATE TABLE tew_linewirepair (lwp_lin_id INTEGER, lwp_wir_id INTEGER, lwp_wir_orderno INTEGER);
";

/// One book with a folder `Power`: a schematic page 01 with a relay coil K1 (pins A1 above, A2
/// below) and a fuse F1 rotated by 90 degrees, wired F1 -> K1:A1 with potential 101 through
/// core 1 (brown) of cable W1; a report page 00; a frame without connection points.
fn sample() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(SCHEMA).unwrap();
    c.execute_batch(
        "
INSERT INTO tew_bundle VALUES (1, '1');
INSERT INTO tew_folder VALUES (7, 'Power');
INSERT INTO tew_file (fil_id, fil_orderno, fil_filetype, fil_title, fil_fol_id) VALUES (10, 2, 12, '01', 7), (11, 1, 2, '00', 7);
INSERT INTO tew_translatedtext VALUES (10, -1, 'de', 'Stromlaufplan'), (10, -1, 'en', 'Main supply'),
  (100, -1, 'en', 'Pump relay');
INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type) VALUES (100, 'K1', '=F1+L1+L2-K1', -1), (101, 'F1', '', -1), (102, '', '', -1);
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
-- Wire F1 top (60, 224) -> (100, 224) -> K1:A1 (100, 205), core 1 of W1.
INSERT INTO tew_equipotential VALUES (900, '101');
INSERT INTO tew_line VALUES (1, 10, 60, 224, 100, 224, 900), (2, 10, 100, 224, 100, 205, 900);
INSERT INTO tew_cable VALUES (40, 'W1', 'CBL-3G1', 'Maker C'), (41, 'W2', '', '');
INSERT INTO tew_cablewire VALUES (40, 1, 'BN'), (40, 2, 'BU'), (40, 3, 'GNYE');
INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag) VALUES (70, 40, 1, ''), (71, NULL, -1, 'X9');
INSERT INTO tew_linewirepair VALUES (1, 70, 1), (2, 70, 1);
",
    )
    .unwrap();
    c
}

fn page<'a>(built: &'a Built, n: &str) -> &'a Page {
    built.pages.iter().find(|p| p.number == n).unwrap()
}

#[test]
fn place_rotates_then_scales() {
    let near = |p: Point, x: f64, y: f64| (p.x - x).abs() < 1e-9 && (p.y - y).abs() < 1e-9;
    let p = place(Point::new(10.0, 20.0), 90.0, (1.0, 1.0), Point::new(4.0, 0.0));
    assert!(near(p, 10.0, 24.0), "{p:?}");
    // Mirrored in x, no rotation.
    let p = place(Point::new(10.0, 20.0), 0.0, (-1.0, 1.0), Point::new(4.0, 1.0));
    assert!(near(p, 6.0, 21.0), "{p:?}");
    // Mirrored in x and turned by 90 degrees: (4, 1) turns to (-1, 4), the mirror makes (1, 4).
    // Scaling first would give (9, 16).
    let p = place(Point::new(10.0, 20.0), 90.0, (-1.0, 1.0), Point::new(4.0, 1.0));
    assert!(near(p, 11.0, 24.0), "{p:?}");
    // The same at 270 degrees: (4, 1) turns to (1, -4), the mirror makes (-1, -4).
    let p = place(Point::new(10.0, 20.0), 270.0, (-1.0, 1.0), Point::new(4.0, 1.0));
    assert!(near(p, 9.0, 16.0), "{p:?}");
    // A uniform scale of 2 with a turn.
    let p = place(Point::new(0.0, 0.0), 90.0, (2.0, 2.0), Point::new(1.0, 0.0));
    assert!(near(p, 0.0, 2.0), "{p:?}");
}

#[test]
fn read_pages_components_symbols_lines_and_cables() {
    let p = read(&sample()).unwrap();
    assert_eq!(p.pages.len(), 2);
    assert_eq!(p.books, 1);
    let sch = p.pages.iter().find(|pg| pg.id == 10).unwrap();
    assert_eq!(sch.title, "Main supply", "English text wins over German");
    assert_eq!(
        (sch.folder.as_str(), sch.book.as_str(), sch.label.as_str()),
        ("Power", "1", "01")
    );
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
    assert_eq!(p.lines[0].wires, [70]);
    assert_eq!(p.cables[&40].cores.len(), 3);
    assert_eq!(p.wires[&70].cable, Some(40));
    assert_eq!(p.wires[&71].cable, None);
    // The only warning: K1's second part (see `extra_parts_of_a_component_are_reported`).
    assert_eq!(p.warnings.len(), 1, "{:?}", p.warnings);
}

#[test]
fn device_tags_from_tag_paths() {
    assert_eq!(device_tag("=F1+L1+L2-K1").unwrap().to_string(), "=F1+L1.L2-K1");
    assert_eq!(device_tag("K3").unwrap().to_string(), "-K3");
    assert_eq!(device_tag("-X 1/2").unwrap().to_string(), "-X_1_2");
    assert_eq!(device_tag("W.110").unwrap().to_string(), "-W.110", "dots are levels");
    assert!(device_tag("").is_none());
    assert!(device_tag("+L1").is_none(), "a location alone is not a device");
}

#[test]
fn build_draws_schematic_pages_numbered_like_the_source() {
    let p = read(&sample()).unwrap();
    let (built, warnings, report) = build(&p, None);
    assert_eq!(built.pages.len(), 1, "the report page is not drawn");
    let pg = &built.pages[0];
    assert_eq!(pg.number, "01", "the source's own page label");
    assert_eq!(pg.title, "Power: Main supply");
    assert_eq!(pg.size, PageSize::A3);
    assert_eq!(pg.symbols.len(), 2);
    let k1 = pg.symbols.iter().find(|s| s.id == "s1").unwrap();
    assert_eq!(k1.tag.as_ref().unwrap().to_string(), "=F1+L1.L2-K1");
    // y flipped on an A3 sheet (297 mm high).
    assert_eq!(k1.at, Point::new(100.0, 97.0));
    // Wire 1: F1 pin 2 -> free point; wire 2: free point -> K1:A1, both on core 1 of W1.
    assert_eq!(pg.wires.len(), 2);
    assert_eq!(
        pg.wires[0].from,
        Endpoint::Pin {
            item: "s2".into(),
            pin: "2".into()
        }
    );
    assert_eq!(
        pg.wires[0].to,
        Endpoint::Point {
            at: Point::new(100.0, 73.0)
        }
    );
    assert_eq!(
        pg.wires[1].to,
        Endpoint::Pin {
            item: "s1".into(),
            pin: "A1".into()
        }
    );
    // The equipotential number is the net name; the wire has no tag of its own here.
    assert_eq!(pg.wires[1].potential.as_deref(), Some("101"));
    assert_eq!(pg.wires[1].number, None);
    let core = pg.wires[1].cable.as_ref().unwrap();
    assert_eq!((core.cable.to_string().as_str(), core.core.as_str()), ("-W1", "1 BN"));
    // The coil's box symbol: pins above and below, pointing out of the box.
    let sym = built.symbols.iter().find(|s| s.id == k1.symbol).unwrap();
    let a1 = sym.pin("A1").unwrap();
    assert_eq!(a1.at, Point::new(0.0, -5.0));
    assert_eq!(a1.dir, PinDir::Up);
    assert_eq!(sym.pin("A2").unwrap().dir, PinDir::Down);
    // Devices: both tagged components, and both cables.
    let dev = &built.devices[&DeviceTag::parse("=F1+L1.L2-K1").unwrap()];
    assert_eq!(dev.description, "Pump relay");
    assert_eq!(dev.properties["PartNumber"], "RLY-24");
    assert_eq!(dev.properties["Manufacturer"], "Maker A");
    assert!(built.devices.contains_key(&DeviceTag::parse("-F1").unwrap()));
    let w1 = &built.devices[&DeviceTag::parse("-W1").unwrap()];
    assert_eq!(
        (w1.description.as_str(), w1.properties["Cores"].as_str()),
        ("Cable", "3")
    );
    assert!(built.devices.contains_key(&DeviceTag::parse("-W2").unwrap()));
    assert_eq!(built.devices.len(), 4);
    // The report says what came in and why the rest did not.
    let row = |w: &str| report.iter().find(|r| r.what == w).unwrap();
    assert_eq!((row("pages").source, row("pages").imported), (2, 1));
    assert!(row("pages").note.contains("1 report"));
    assert_eq!((row("components").source, row("components").imported), (3, 2));
    assert!(row("components").note.contains("1 without a usable tag"));
    assert_eq!((row("symbols").source, row("symbols").imported), (4, 2));
    assert!(row("symbols").note.contains("1 on pages that are not imported"));
    assert!(row("symbols").note.contains("1 without connection points"));
    assert_eq!((row("wires").source, row("wires").imported), (2, 1));
    assert_eq!((row("cables").source, row("cables").imported), (2, 2));
    assert!(row("cables").note.contains("1 not on any imported wire"));
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn several_books_number_pages_with_the_book_and_first_page_renumbers() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_bundle VALUES (2, '2');
         INSERT INTO tew_file (fil_id, fil_orderno, fil_filetype, fil_title, fil_fol_id, fil_bun_id) VALUES (12, 3, 12, '01', NULL, 2);",
    )
    .unwrap();
    let p = read(&c).unwrap();
    let (built, _, _) = build(&p, None);
    let numbers: Vec<&str> = built.pages.iter().map(|pg| pg.number.as_str()).collect();
    assert_eq!(numbers, ["1.01", "2.01"]);
    let (built, _, _) = build(&p, Some(7));
    let numbers: Vec<&str> = built.pages.iter().map(|pg| pg.number.as_str()).collect();
    assert_eq!(numbers, ["7", "8"]);
}

#[test]
fn a_t_joint_splits_the_line_it_lands_on() {
    let c = sample();
    // A line across from (60, 240) to (140, 240), and one ending on its middle from below.
    c.execute_batch("INSERT INTO tew_line VALUES (3, 10, 60, 240, 140, 240, NULL), (4, 10, 100, 230, 100, 240, NULL);")
        .unwrap();
    let (built, _, report) = build(&read(&c).unwrap(), None);
    let pg = &built.pages[0];
    // 2 sample wires + the long line in two halves + the stub.
    assert_eq!(pg.wires.len(), 5);
    let joint = Point::new(100.0, 57.0);
    let at_joint = pg
        .wires
        .iter()
        .filter(|w| w.from == Endpoint::Point { at: joint } || w.to == Endpoint::Point { at: joint })
        .count();
    assert_eq!(at_joint, 3, "both halves and the stub meet at the joint");
    let lines = report.iter().find(|r| r.what == "wire lines").unwrap();
    assert!(lines.note.contains("1 extra segments"));
}

#[test]
fn identical_symbols_share_one_library_symbol() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type) VALUES (103, 'K2', '', -1);
         INSERT INTO tew_symbol VALUES (5, 10, 103, 'Coil', 150, 200, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (5, 0, 0, 0, 5), (5, 0, 1, 0, -5);
         INSERT INTO tew_componentelement (cel_id, cel_sym_id, cel_sct_no) VALUES (51, 5, 0);
         INSERT INTO tew_componentterminal VALUES (51, 0, 'A1'), (51, 1, 'A2');",
    )
    .unwrap();
    let (built, _, _) = build(&read(&c).unwrap(), None);
    let pg = &built.pages[0];
    let k1 = pg.symbols.iter().find(|s| s.id == "s1").unwrap();
    let k2 = pg.symbols.iter().find(|s| s.id == "s5").unwrap();
    assert_eq!(k1.symbol, k2.symbol);
    assert_eq!(built.symbols.len(), 2, "one coil box, one fuse box");
}

#[test]
fn duplicate_tags_are_made_unique() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type) VALUES (104, 'K1', '=F1+L1+L2-K1', -1);
         INSERT INTO tew_symbol VALUES (6, 10, 104, 'Coil', 200, 200, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (6, 0, 0, 0, 5);",
    )
    .unwrap();
    let (built, warnings, _) = build(&read(&c).unwrap(), None);
    assert!(built.devices.contains_key(&DeviceTag::parse("=F1+L1.L2-K1_2").unwrap()));
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("1 device or cable tag(s) were already taken")),
        "{warnings:?}"
    );
}

#[test]
fn extra_parts_of_a_component_are_reported() {
    let p = read(&sample()).unwrap();
    assert_eq!(p.components[&100].part_number, "RLY-24");
    assert!(
        p.warnings.iter().any(|w| w.starts_with("1 extra part(s) left out")),
        "{:?}",
        p.warnings
    );
}

#[test]
fn a_page_number_used_twice_is_reported() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_file (fil_id, fil_orderno, fil_filetype, fil_title, fil_fol_id) VALUES (12, 3, 12, '01', 7);",
    )
    .unwrap();
    let (built, warnings, _) = build(&read(&c).unwrap(), None);
    assert_eq!(built.pages.len(), 2);
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("1 page number(s) were used twice")),
        "{warnings:?}"
    );
}

#[test]
fn a_line_with_several_wire_numbers_is_reported() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag) VALUES (73, NULL, -1, 'X8'), (74, NULL, -1, 'X7');
         INSERT INTO tew_linewirepair VALUES (1, 73, 2), (1, 74, 3);",
    )
    .unwrap();
    let (_, _, report) = build(&read(&c).unwrap(), None);
    let wires = report.iter().find(|r| r.what == "wires").unwrap();
    assert!(
        wires.note.contains("1 line(s) carry several wire numbers"),
        "{}",
        wires.note
    );
    assert!(
        wires
            .note
            .contains("1 other wire number(s) on those lines are left off that line's wire"),
        "{}",
        wires.note
    );
}

#[test]
fn a_wire_only_on_a_zero_length_line_is_reported_as_such() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_line VALUES (3, 10, 300, 300, 300, 300, NULL);
         INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag) VALUES (75, NULL, -1, 'Z1');
         INSERT INTO tew_linewirepair VALUES (3, 75, 1);",
    )
    .unwrap();
    let (_, _, report) = build(&read(&c).unwrap(), None);
    let wires = report.iter().find(|r| r.what == "wires").unwrap();
    assert!(wires.note.contains("1 only on lines of zero length"), "{}", wires.note);
}

#[test]
fn large_pages_move_to_a2_and_odd_rows_are_counted() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_line VALUES (3, 10, 400, 380, 500, 380, NULL);
         INSERT INTO tew_symbol VALUES (7, 10, NULL, 'Broken', NULL, 5, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbol VALUES (8, 10, NULL, 'Text angle', 5, 5, 'ninety', 1, 1, NULL, NULL);
         INSERT INTO tew_line VALUES (5, 10, 1, 1, 1, 1, NULL);",
    )
    .unwrap();
    let p = read(&c).unwrap();
    assert!(p.warnings.iter().any(|w| w.contains("without a usable position")));
    assert!(
        p.warnings
            .iter()
            .any(|w| w.contains("1 symbol row(s) could not be read")),
        "{:?}",
        p.warnings
    );
    let (built, _, report) = build(&p, None);
    assert_eq!(built.pages[0].size, PageSize::A2);
    let lines = report.iter().find(|r| r.what == "wire lines").unwrap();
    assert!(lines.note.contains("1 of zero length"));
}

#[test]
fn missing_tables_are_an_error_not_a_panic() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE tew_file (fil_id INTEGER);").unwrap();
    assert!(read(&c).is_err());
}

#[test]
fn huge_numbers_do_not_overflow() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_symbol VALUES (9, 10, NULL, 'Big', 30, 30, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (9, 0, 9223372036854775807, 0, 0);",
    )
    .unwrap();
    let p = read(&c).unwrap();
    let s = p.symbols.iter().find(|s| s.id == 9).unwrap();
    assert_eq!(s.pins[0].name, i64::MAX.to_string());
}

#[test]
fn arrows_become_interruption_points_with_one_tag_per_pair() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_file (fil_id, fil_orderno, fil_filetype, fil_title, fil_fol_id) VALUES (12, 3, 0, '02', 7);
         INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type) VALUES (200, '', '=F1+L1-', 5);
         INSERT INTO tew_symbol VALUES (8, 10, 200, 'Arrow out', 140, 224, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (8, 0, 0, 0, 0);
         INSERT INTO tew_symbol VALUES (9, 12, 200, 'Arrow in', 20, 100, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (9, 0, 0, 0, 0);",
    )
    .unwrap();
    let (built, _, report) = build(&read(&c).unwrap(), None);
    assert_eq!(built.pages.len(), 2, "a schematic sheet (type 0) is drawn too");
    let out = page(&built, "01").symbols.iter().find(|s| s.id == "s8").unwrap();
    let inn = page(&built, "02").symbols.iter().find(|s| s.id == "s9").unwrap();
    assert_eq!(out.tag, inn.tag);
    assert_eq!(out.tag.as_ref().unwrap().to_string(), "-OD1");
    let sym = built.symbols.iter().find(|s| s.id == out.symbol).unwrap();
    assert_eq!(sym.role, SymbolRole::Interruption);
    assert!(
        !built.devices.contains_key(&DeviceTag::parse("-OD1").unwrap()),
        "arrows are not devices"
    );
    let arrows = report.iter().find(|r| r.what == "page link arrows").unwrap();
    assert_eq!((arrows.source, arrows.imported), (1, 1));
}

#[test]
fn a_strip_drawn_as_several_symbols_gets_distinct_pin_names() {
    let c = sample();
    // Strip X1: two terminal symbols, each two points, terminals without text.
    c.execute_batch(
        "INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type) VALUES (300, 'X1', '-X1', 3);
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

#[test]
fn sheet_size_from_the_title_block_name() {
    assert_eq!(frame_size("FRAME_BASE_A2_EN-20200101120000"), Some(PageSize::A2));
    assert_eq!(frame_size("SHEETA3X"), Some(PageSize::A3));
    assert_eq!(frame_size("frame a4 portrait"), Some(PageSize::A4));
    assert_eq!(frame_size("A20 custom"), None);
    assert_eq!(frame_size("Info page"), None);
}

#[test]
fn box_size_is_relative_and_a_shared_function_becomes_a_property() {
    let c = sample();
    // A black box 35 wide and 40 high below its insertion point, on an A2 frame page.
    c.execute_batch(
        "UPDATE tew_file SET fil_titleblock = 'SHEET_A2_X' WHERE fil_id = 10;
         INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type) VALUES (105, 'A1', '=F1+L1-A1', -1);
         INSERT INTO tew_symbol VALUES (10, 10, 105, 'Box', 20, 300, 0, 1, 1, 35, -40);",
    )
    .unwrap();
    // K1 and A1 carry =F1, F1 has no function: not shared by all, kept.
    let (built, _, _) = build(&read(&c).unwrap(), None);
    let pg = &built.pages[0];
    assert_eq!(pg.size, PageSize::A2, "named by the title block");
    let a1 = pg.symbols.iter().find(|s| s.id == "s10").unwrap();
    assert_eq!(a1.tag.as_ref().unwrap().to_string(), "=F1+L1-A1");
    let sym = built.symbols.iter().find(|s| s.id == a1.symbol).unwrap();
    assert_eq!(
        sym.graphics[0],
        Shape::Rect {
            min: Point::new(0.0, 0.0),
            max: Point::new(35.0, 40.0)
        }
    );

    // With =F1 on every device it leaves the tags and stays as a property.
    c.execute_batch("UPDATE tew_component SET com_tagpath = '=F1-F1' WHERE com_id = 101;")
        .unwrap();
    let (built, warnings, _) = build(&read(&c).unwrap(), None);
    let a1 = built.pages[0].symbols.iter().find(|s| s.id == "s10").unwrap();
    assert_eq!(a1.tag.as_ref().unwrap().to_string(), "+L1-A1");
    assert_eq!(
        built.devices[&DeviceTag::parse("+L1-A1").unwrap()].properties["Function"],
        "=F1"
    );
    assert!(warnings.iter().any(|w| w.contains("function =F1")));
}

fn archive(dbs: &[(&str, &[u8])]) -> std::io::Cursor<Vec<u8>> {
    use std::io::Write;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opt = zip::write::SimpleFileOptions::default();
        for (name, db) in dbs {
            z.start_file(*name, opt).unwrap();
            z.write_all(db).unwrap();
        }
        z.start_file("Archive.ini", opt).unwrap();
        z.write_all(b"[ARCHIVEINFO]\n").unwrap();
        z.finish().unwrap();
    }
    buf.set_position(0);
    buf
}

#[test]
fn one_database_is_taken_from_an_archive_stream() {
    let db = sample().serialize(rusqlite::MAIN_DB).unwrap().to_vec();
    let buf = archive(&[
        ("Projects/12/Database/tew_project_data.sqlite", &db),
        ("Projects/3/Database/tew_project_data.sqlite", &db),
        ("Projects/3/Snapshots/9/Database/tew_project_data.sqlite", &db),
    ]);
    let got = database_from_archive(buf.clone(), 2).unwrap();
    assert_eq!(
        (got.folder.as_str(), got.count),
        ("Projects/12", 2),
        "numeric order, snapshots skipped"
    );
    assert_eq!(got.bytes, db);
    let first = database_from_archive(buf.clone(), 1).unwrap();
    assert_eq!(first.folder, "Projects/3");
    let p = read(&open(&first.bytes).unwrap()).unwrap();
    assert_eq!(p.symbols.len(), 4);
    assert!(database_from_archive(buf.clone(), 0).is_err());
    assert!(database_from_archive(buf, 3).is_err());
    assert!(database_from_archive(std::io::Cursor::new(b"not a zip".to_vec()), 1).is_err());
}

#[test]
fn ends_drawn_slightly_apart_are_joined() {
    let c = sample();
    // 0.3 mm short of K1:A2 (100, 195), and two lines whose ends miss each other by 0.2 mm.
    c.execute_batch(
        "INSERT INTO tew_line VALUES (3, 10, 100, 194.7, 100, 180, NULL),
                                    (4, 10, 20, 150, 40, 150, NULL), (5, 10, 40.2, 150, 40.2, 130, NULL);",
    )
    .unwrap();
    let (built, _, _) = build(&read(&c).unwrap(), None);
    let pg = &built.pages[0];
    assert!(pg.wires.iter().any(|w| w.from
        == Endpoint::Pin {
            item: "s1".into(),
            pin: "A2".into()
        }));
    let corner = Point::new(40.0, 147.0);
    let at_corner = pg
        .wires
        .iter()
        .filter(|w| w.to == Endpoint::Point { at: corner } || w.from == Endpoint::Point { at: corner })
        .count();
    assert_eq!(at_corner, 2, "the second line's end moved onto the first line's end");
}

#[test]
fn repeated_fallback_names_stay_unique() {
    let (_, pins) = box_symbol(
        "X",
        Point::new(0.0, 0.0),
        None,
        &[
            ("A".into(), Point::new(0.0, 0.0)),
            ("A#3".into(), Point::new(5.0, 0.0)),
            ("A".into(), Point::new(10.0, 0.0)),
        ],
    );
    let names: BTreeSet<&str> = pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names.len(), 3, "{pins:?}");
}

#[test]
fn blocks_that_clean_up_alike_get_their_own_ids() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_symbol VALUES (30, 10, NULL, 'Relay A', 200, 100, 0, 1, 1, NULL, NULL),
                                       (31, 10, NULL, 'Relay-A', 250, 100, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (30, 0, 0, 0, 5), (31, 0, 0, 0, 5), (31, 0, 1, 0, -5);",
    )
    .unwrap();
    let (built, _, _) = build(&read(&c).unwrap(), None);
    let a = built.pages[0].symbols.iter().find(|s| s.id == "s30").unwrap();
    let b = built.pages[0].symbols.iter().find(|s| s.id == "s31").unwrap();
    assert_ne!(a.symbol, b.symbol);
    let ids: BTreeSet<&str> = built.symbols.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids.len(), built.symbols.len(), "no id used twice");
}

#[test]
fn points_that_overflow_in_the_transform_are_left_out() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_symbol VALUES (32, 10, NULL, 'Huge', 10, 10, 0, 1e308, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (32, 0, 0, 2, 0);",
    )
    .unwrap();
    let p = read(&c).unwrap();
    assert!(p.symbols.iter().find(|s| s.id == 32).unwrap().pins.is_empty());
    assert!(p.warnings.iter().any(|w| w.contains("cannot be computed")));
}

#[test]
fn ends_of_different_nets_drawn_close_stay_apart() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_equipotential VALUES (901, '202'), (902, '203');
         INSERT INTO tew_line VALUES (3, 10, 20, 150, 40, 150, 901), (4, 10, 40.2, 150, 40.2, 130, 902),
                                    (5, 10, 60, 120, 140, 120, 901), (6, 10, 100, 100, 100, 119.8, 902);",
    )
    .unwrap();
    let (built, _, report) = build(&read(&c).unwrap(), None);
    let pg = &built.pages[0];
    let ends: BTreeSet<String> = pg
        .wires
        .iter()
        .filter(|w| w.potential.as_deref() == Some("203"))
        .flat_map(|w| [format!("{:?}", w.from), format!("{:?}", w.to)])
        .collect();
    let other: BTreeSet<String> = pg
        .wires
        .iter()
        .filter(|w| w.potential.as_deref() == Some("202"))
        .flat_map(|w| [format!("{:?}", w.from), format!("{:?}", w.to)])
        .collect();
    assert!(ends.is_disjoint(&other), "no shared end between nets 202 and 203");
    let lines = report.iter().find(|r| r.what == "wire lines").unwrap();
    assert!(!lines.note.contains("extra segments"), "no T-joint across nets");
}

#[test]
fn a_net_without_a_number_keeps_its_own_name() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_equipotential VALUES (905, ''), (906, '');
         INSERT INTO tew_line VALUES (3, 10, 20, 150, 40, 150, 905), (4, 10, 20, 120, 40, 120, 906);",
    )
    .unwrap();
    let (built, _, _) = build(&read(&c).unwrap(), None);
    let names: BTreeSet<Option<String>> = built.pages[0]
        .wires
        .iter()
        .filter(|w| {
            w.from
                == Endpoint::Point {
                    at: Point::new(20.0, 147.0),
                }
                || w.from
                    == Endpoint::Point {
                        at: Point::new(20.0, 177.0),
                    }
        })
        .map(|w| w.potential.clone())
        .collect();
    let want: BTreeSet<Option<String>> = [Some("net905".to_owned()), Some("net906".to_owned())]
        .into_iter()
        .collect();
    assert_eq!(names, want);
}

#[test]
fn a_line_on_two_cable_cores_is_reported() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag) VALUES (72, 40, 2, '');
         INSERT INTO tew_linewirepair VALUES (1, 72, 2);",
    )
    .unwrap();
    let (built, _, report) = build(&read(&c).unwrap(), None);
    let core = built.pages[0].wires[0].cable.as_ref().unwrap();
    assert_eq!(core.core, "1 BN", "the first core is shown");
    let wires = report.iter().find(|r| r.what == "wires").unwrap();
    assert!(
        wires.note.contains("1 line(s) carry several cable cores"),
        "{}",
        wires.note
    );
    assert!(
        wires
            .note
            .contains("1 other cable core(s) on those lines are left off that line's wire"),
        "{}",
        wires.note
    );
}

#[test]
fn a_zero_length_line_left_out_does_not_take_a_pin() {
    let c = sample();
    // Net 202's line has both ends within 0.5 mm of the free K1:A2 (100, 195): it snaps to zero
    // length and is left out. Net 203, ending 0.3 mm off the pin, must still get it.
    c.execute_batch(
        "INSERT INTO tew_equipotential VALUES (901, '202'), (902, '203');
         INSERT INTO tew_line VALUES (3, 10, 100.2, 195, 100.3, 195, 901), (4, 10, 99.7, 195, 70, 195, 902);",
    )
    .unwrap();
    let (built, _, report) = build(&read(&c).unwrap(), None);
    let a2 = Endpoint::Pin {
        item: "s1".into(),
        pin: "A2".into(),
    };
    let on_a2: Vec<Option<&str>> = built.pages[0]
        .wires
        .iter()
        .filter(|w| w.from == a2 || w.to == a2)
        .map(|w| w.potential.as_deref())
        .collect();
    assert_eq!(on_a2, [Some("203")]);
    let lines = report.iter().find(|r| r.what == "wire lines").unwrap();
    assert!(lines.note.contains("1 of zero length"), "{}", lines.note);
}

#[test]
fn a_near_end_of_another_net_does_not_snap_onto_a_taken_pin() {
    let c = sample();
    // Net 101 reaches K1:A1 (100, 205) exactly in the sample; a line of net 202 ends 0.3 mm off it.
    c.execute_batch(
        "INSERT INTO tew_equipotential VALUES (901, '202');
         INSERT INTO tew_line VALUES (3, 10, 100.3, 205, 130, 205, 901);",
    )
    .unwrap();
    let (built, _, _) = build(&read(&c).unwrap(), None);
    let other = built.pages[0]
        .wires
        .iter()
        .find(|w| w.potential.as_deref() == Some("202"))
        .unwrap();
    assert_eq!(
        other.from,
        Endpoint::Point {
            at: Point::new(100.3, 92.0)
        },
        "stays off the pin"
    );
}

#[test]
fn the_first_net_to_snap_onto_a_free_pin_takes_it() {
    let c = sample();
    // K1:A2 (100, 195) is free; nets 202 and 203 both end within 0.5 mm of it, 202 first.
    c.execute_batch(
        "INSERT INTO tew_equipotential VALUES (901, '202'), (902, '203');
         INSERT INTO tew_line VALUES (3, 10, 100.2, 195, 130, 195, 901), (4, 10, 99.7, 195, 70, 195, 902);",
    )
    .unwrap();
    let (built, _, _) = build(&read(&c).unwrap(), None);
    let a2 = Endpoint::Pin {
        item: "s1".into(),
        pin: "A2".into(),
    };
    let on_a2: Vec<Option<&str>> = built.pages[0]
        .wires
        .iter()
        .filter(|w| w.from == a2 || w.to == a2)
        .map(|w| w.potential.as_deref())
        .collect();
    assert_eq!(on_a2, [Some("202")]);
}

#[test]
fn a_fallback_net_name_never_takes_a_source_name() {
    let taken: BTreeSet<String> = ["net7".to_owned(), "net7_2".to_owned()].into_iter().collect();
    assert_eq!(fallback_net(7, &taken), "net7_3");
    assert_eq!(fallback_net(8, &taken), "net8");
}

#[test]
fn a_line_diagram_page_is_marked_as_an_overview() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_file (fil_id, fil_orderno, fil_filetype, fil_title, fil_fol_id) VALUES (12, 3, 1, '02', 7);",
    )
    .unwrap();
    let (built, _, _) = build(&read(&c).unwrap(), None);
    assert!(!page(&built, "01").overview, "a schematic page is not");
    assert!(page(&built, "02").overview, "a line diagram is");
}

#[test]
fn locations_come_in_with_their_descriptions_and_devices_point_to_them() {
    let c = sample();
    c.execute_batch(
        "CREATE TABLE tew_location (loc_id INTEGER, loc_loc_id INTEGER, loc_tagpath TEXT);
         INSERT INTO tew_location VALUES (30, -1, '+L1'), (31, 30, '+L1+L2'), (32, -1, ''), (33, -1, '+L1');
         INSERT INTO tew_translatedtext VALUES (30, -1, 'de', 'Schrank'), (30, -1, 'en', 'Cabinet'),
           (31, -1, 'en', 'Door');",
    )
    .unwrap();
    let p = read(&c).unwrap();
    assert_eq!(p.locations.len(), 4);
    let (built, _, report) = build(&p, None);
    let got: Vec<(&str, &str)> = built
        .locations
        .iter()
        .map(|l| (l.path.as_str(), l.description.as_str()))
        .collect();
    assert_eq!(got, [("L1", "Cabinet"), ("L1.L2", "Door")], "English wins");
    // -K1 is tagged =F1+L1+L2-K1 in the sample: its location is one of the imported ones.
    let k1 = built.devices.keys().find(|t| t.product == ["K1"]).unwrap();
    assert_eq!(k1.location.join("."), "L1.L2");
    let row = report.iter().find(|r| r.what == "locations").unwrap();
    assert_eq!((row.source, row.imported), (4, 2));
    assert!(row.note.contains("1 without a usable tag path"), "{}", row.note);
    assert!(row.note.contains("1 with the same tag path as another"), "{}", row.note);
}

#[test]
fn a_database_without_a_location_table_still_imports() {
    let (built, warnings, report) = build(&read(&sample()).unwrap(), None);
    assert!(built.locations.is_empty());
    assert!(!warnings.iter().any(|w| w.contains("location")), "{warnings:?}");
    let row = report.iter().find(|r| r.what == "locations").unwrap();
    assert_eq!((row.source, row.imported), (0, 0));
}

#[test]
fn a_location_path_is_cleaned_up_like_the_tags_of_its_devices() {
    let c = sample();
    c.execute_batch(
        "CREATE TABLE tew_location (loc_id INTEGER, loc_loc_id INTEGER, loc_tagpath TEXT);
         INSERT INTO tew_location VALUES (40, -1, '+Room 1'), (41, -1, '+A/B');
         INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type) VALUES (105, 'K5', '+Room 1-K5', -1), (106, 'K6', '+A/B-K6', -1);
         INSERT INTO tew_symbol VALUES (7, 10, 105, 'Coil', 250, 200, 0, 1, 1, NULL, NULL),
           (8, 10, 106, 'Coil', 280, 200, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (7, 0, 0, 0, 5), (8, 0, 0, 0, 5);",
    )
    .unwrap();
    let (built, _, _) = build(&read(&c).unwrap(), None);
    let paths: BTreeSet<String> = built.locations.iter().map(|l| l.path.clone()).collect();
    for product in ["K5", "K6"] {
        let tag = built.devices.keys().find(|t| t.product == [product]).unwrap();
        let loc = tag.location.join(".");
        assert!(paths.contains(&loc), "{product} is in {loc}, imported: {paths:?}");
    }
    assert!(paths.contains("Room_1"), "{paths:?}");
}

#[test]
fn location_descriptions_survive_blank_english_and_a_late_repeat_and_bad_rows_say_why() {
    let c = sample();
    c.execute_batch(
        "CREATE TABLE tew_location (loc_id INTEGER, loc_loc_id INTEGER, loc_tagpath TEXT);
         INSERT INTO tew_location VALUES (50, -1, '+L7'), (51, -1, '+L8'), (52, -1, '+L8'), (NULL, -1, '+L9');
         INSERT INTO tew_translatedtext VALUES (50, -1, 'de', 'Schrank'), (50, -1, 'en', '   '),
           (52, -1, 'en', 'Cabinet');",
    )
    .unwrap();
    let p = read(&c).unwrap();
    assert!(
        p.warnings
            .iter()
            .any(|w| w.starts_with("1 location row(s) could not be read") && w.contains("first:")),
        "{:?}",
        p.warnings
    );
    let (built, _, _) = build(&p, None);
    let got: Vec<(&str, &str)> = built
        .locations
        .iter()
        .map(|l| (l.path.as_str(), l.description.as_str()))
        .collect();
    assert!(
        got.contains(&("L7", "Schrank")),
        "blank English does not hide German: {got:?}"
    );
    assert!(
        got.contains(&("L8", "Cabinet")),
        "a later repeat fills the description: {got:?}"
    );
}

/// The sample plus terminal strip X1 with terminals 1 and 2 (source order 2, 1; tag paths written
/// with a space), drawn at (150, 200) and (170, 200); a line joins X1.1's lower point to K1:A2.
/// Wire records: X1.1 lower to K1:A2 (drawn so), X1.2 upper to K1:A1 (not drawn so), and one to
/// a terminal that is not drawn.
fn sample_with_strip() -> Connection {
    let c = sample();
    c.execute_batch(
        "
INSERT INTO tew_component VALUES (200, 'X1', '-X1', 3, NULL, 0), (201, '1', '-X1 1', 4, 200, 2), (202, '2', '-X1 2', 4, 200, 1);
INSERT INTO tew_symbol VALUES (20, 10, 201, 'Terminal', 150, 200, 0, 1, 1, NULL, NULL),
  (21, 10, 202, 'Terminal', 170, 200, 0, 1, 1, NULL, NULL);
INSERT INTO tew_symbolpt VALUES (20, 0, 0, 0, 5), (20, 0, 1, 0, -5), (21, 0, 0, 0, 5), (21, 0, 1, 0, -5);
INSERT INTO tew_componentelement VALUES (60, 20, 0, 201, 0), (61, 21, 0, 202, 0);
INSERT INTO tew_componentterminal VALUES (60, 0, NULL), (60, 1, NULL), (61, 0, NULL), (61, 1, NULL);
INSERT INTO tew_line VALUES (10, 10, 150, 195, 100, 195, NULL), (11, 10, 170, 205, 170, 215, NULL);
INSERT INTO tew_linewirepair VALUES (10, 80, 1), (11, 81, 1);
INSERT INTO tew_wire VALUES (80, NULL, -1, 'N1', 60, 1, 50, 1), (81, NULL, -1, 'N2', 61, 0, 50, 0),
  (82, NULL, -1, '', 60, 0, 99, 0), (83, NULL, -1, '', 0, 0, 0, 0);
",
    )
    .unwrap();
    c
}

#[test]
fn terminal_strips_come_in_with_their_terminals_and_wire_ends_are_checked() {
    let p = read(&sample_with_strip()).unwrap();
    assert_eq!(p.components[&201].parent, Some(200));
    assert_eq!((p.components[&202].order, p.components[&202].name.as_str()), (1, "2"));
    assert_eq!((p.wires[&80].from, p.wires[&80].to), (Some((60, 1)), Some((50, 1))));
    assert_eq!(p.wires[&83].from, None, "element 0 is no end");
    let x1 = p.symbols.iter().find(|s| s.id == 20).unwrap();
    assert_eq!(x1.pins[1].terminal, Some((60, 1)));

    let (built, warnings, report) = build(&p, None);
    for t in ["-X1", "-X1.1", "-X1.2"] {
        assert!(
            built.devices.contains_key(&DeviceTag::parse(t).unwrap()),
            "{t}: {:?}",
            built.devices.keys()
        );
    }
    let pg = page(&built, "01");
    let placed = pg.symbols.iter().find(|s| s.id == "s20").unwrap();
    assert_eq!(placed.tag.as_ref().unwrap().to_string(), "-X1.1");
    let sym = built.symbols.iter().find(|s| s.id == placed.symbol).unwrap();
    assert_eq!(sym.role, SymbolRole::Terminal);
    let coil = pg.symbols.iter().find(|s| s.id == "s1").unwrap();
    let coil_sym = built.symbols.iter().find(|s| s.id == coil.symbol).unwrap();
    assert_eq!(coil_sym.role, SymbolRole::Standalone);

    let row = |what: &str| report.iter().find(|c| c.what == what).unwrap().clone();
    let strips = row("terminal strips");
    assert_eq!((strips.source, strips.imported), (1, 1));
    assert!(
        strips.note.starts_with("1 whose source order differs"),
        "{}",
        strips.note
    );
    let terms = row("terminals");
    assert_eq!((terms.source, terms.imported, terms.note.as_str()), (2, 2, ""));
    let ends = row("wire ends (from/to terminals)");
    assert_eq!((ends.source, ends.imported), (3, 1), "{}", ends.note);
    assert!(
        ends.note.contains("1 join terminals that are not connected"),
        "{}",
        ends.note
    );
    assert!(
        ends.note.contains("1 with a terminal that is not drawn"),
        "{}",
        ends.note
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("terminal strip(s) -X1 have their terminals in another order")),
        "{warnings:?}"
    );
    let apart = warnings
        .iter()
        .find(|w| w.contains("not connected on the imported drawing"))
        .unwrap();
    assert!(apart.contains("wire N2: -X1.2:1 (page 01)"), "{apart}");
    assert!(!apart.contains("N1"), "{apart}");
}

#[test]
fn a_terminal_without_a_strip_keeps_its_own_tag() {
    let c = sample();
    c.execute_batch("INSERT INTO tew_component VALUES (300, '5', '-X9-5', 4, NULL, 0);")
        .unwrap();
    let (built, _, report) = build(&read(&c).unwrap(), None);
    assert!(built.devices.contains_key(&DeviceTag::parse("-X9.5").unwrap()));
    let terms = report.iter().find(|c| c.what == "terminals").unwrap();
    assert_eq!((terms.source, terms.imported), (1, 0));
    assert!(terms.note.starts_with("1 not on a strip"), "{}", terms.note);
}

#[test]
fn imported_terminals_show_in_the_terminal_plan_per_strip() {
    let c = sample_with_strip();
    // A terminal whose number has a dot, and a wire record with only one end.
    c.execute_batch(
        "INSERT INTO tew_component VALUES (203, '3.1', '-X1-3.1', 4, 200, 3), (204, '1+2', '-X1-1+2', 4, 200, 4);
         INSERT INTO tew_wire VALUES (84, NULL, -1, '', 60, 0, NULL, NULL);",
    )
    .unwrap();
    let (built, _, report) = build(&read(&c).unwrap(), None);
    assert!(
        ["-X1.3_1", "-X1.1_2"]
            .iter()
            .all(|t| built.devices.contains_key(&DeviceTag::parse(t).unwrap())),
        "one level: {:?}",
        built.devices.keys()
    );
    let ends = report
        .iter()
        .find(|c| c.what == "wire ends (from/to terminals)")
        .unwrap();
    assert!(ends.note.contains("1 more name only one terminal"), "{}", ends.note);

    let mut project = wc_core::Project::new("t");
    project.pages = built.pages.clone();
    project.devices = built.devices.clone();
    let symbols: BTreeMap<String, Symbol> = built.symbols.iter().map(|s| (s.id.clone(), s.clone())).collect();
    let parts = BTreeMap::new();
    let plan = wc_report::terminal_plan(&wc_report::Inputs {
        project: &project,
        symbols: &symbols,
        parts: &parts,
    });
    let rows: Vec<(&str, &str)> = plan.rows.iter().map(|r| (r[0].as_str(), r[1].as_str())).collect();
    assert_eq!(rows, [("-X1", "1"), ("-X1", "2")], "strip -X1, by number");
    // X1.1's lower side is wired to K1:A2.
    assert!(plan.rows[0][3].contains("A2"), "{:?}", plan.rows[0]);
}

#[test]
fn a_wire_end_on_a_repeated_pin_name_is_found_on_the_right_pin() {
    let c = sample();
    // A strip terminal whose two points are both called `1`: the lower one becomes `1#2`.
    // The line reaches only the lower one, and the wire record names that one.
    c.execute_batch(
        "INSERT INTO tew_component VALUES (200, 'X1', '-X1', 3, NULL, 0), (201, '1', '-X1-1', 4, 200, 1);
         INSERT INTO tew_symbol VALUES (20, 10, 201, 'Terminal', 150, 200, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (20, 0, 0, 0, 5), (20, 0, 1, 0, -5);
         INSERT INTO tew_componentelement VALUES (60, 20, 0, 201, 0);
         INSERT INTO tew_componentterminal VALUES (60, 0, '1'), (60, 1, '1');
         INSERT INTO tew_line VALUES (10, 10, 150, 195, 100, 195, NULL);
         INSERT INTO tew_wire VALUES (80, NULL, -1, 'N1', 60, 1, 50, 1);",
    )
    .unwrap();
    let (_, warnings, report) = build(&read(&c).unwrap(), None);
    let ends = report
        .iter()
        .find(|c| c.what == "wire ends (from/to terminals)")
        .unwrap();
    assert_eq!((ends.source, ends.imported), (1, 1), "{} {warnings:?}", ends.note);
}

#[test]
fn terminal_numbers_that_sort_alike_count_the_strip_as_out_of_order() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_component VALUES (200, 'X1', '-X1', 3, NULL, 0), (201, '01', '-X1-01', 4, 200, 1),
           (202, '1', '-X1-1', 4, 200, 2), (210, 'X2', '-X2', 3, NULL, 0), (211, '1', '-X2-1', 4, 210, 1),
           (212, '2', '-X2-2', 4, 210, 2), (213, '10', '-X2-10', 4, 210, 3);",
    )
    .unwrap();
    let (_, warnings, report) = build(&read(&c).unwrap(), None);
    let strips = report.iter().find(|c| c.what == "terminal strips").unwrap();
    assert!(
        strips.note.starts_with("1 whose source order differs"),
        "{}",
        strips.note
    );
    assert!(
        warnings.iter().any(|w| w.starts_with("terminal strip(s) -X1 have")),
        "only -X1 (1, 2, 10 rise naturally): {warnings:?}"
    );
}

#[test]
fn a_line_diagram_does_not_join_what_the_schematics_keep_apart() {
    let c = sample_with_strip();
    // Wire N2's ends are apart in the schematic: X1.2's upper end (given potential JOIN here) and
    // K1:A1 (potential 101). A line diagram joins the two potentials at one pin; the check must
    // not let that count as a connection.
    c.execute_batch(
        "INSERT INTO tew_file (fil_id, fil_orderno, fil_filetype, fil_title, fil_fol_id) VALUES (12, 3, 1, '02', 7);
         INSERT INTO tew_equipotential VALUES (901, 'JOIN');
         INSERT INTO tew_symbol VALUES (30, 12, 202, 'Terminal', 50, 50, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (30, 0, 0, 0, 5);
         INSERT INTO tew_line VALUES (20, 12, 50, 55, 50, 70, 901), (22, 12, 50, 55, 30, 55, 900),
           (21, 10, 170, 205, 170, 230, 901);",
    )
    .unwrap();
    let (built, _, report) = build(&read(&c).unwrap(), None);
    assert!(built.pages.iter().any(|p| p.overview), "the line diagram is imported");
    // The line diagram draws X1.2 again; the terminal plan lists it once.
    let mut project = wc_core::Project::new("t");
    project.pages = built.pages.clone();
    let symbols: BTreeMap<String, Symbol> = built.symbols.iter().map(|s| (s.id.clone(), s.clone())).collect();
    let parts = BTreeMap::new();
    let plan = wc_report::terminal_plan(&wc_report::Inputs {
        project: &project,
        symbols: &symbols,
        parts: &parts,
    });
    assert_eq!(plan.rows.len(), 2, "{:?}", plan.rows);
    let ends = report
        .iter()
        .find(|c| c.what == "wire ends (from/to terminals)")
        .unwrap();
    assert!(
        ends.note.contains("1 join terminals that are not connected"),
        "{}",
        ends.note
    );
}

/// An archive project list (`Database/tew_app_project.sqlite`) with two projects.
fn project_list() -> Vec<u8> {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE tew_project (pro_id INTEGER, pro_name TEXT, pro_directory TEXT, pro_customername TEXT,
           pro_custaddress1 TEXT, pro_custaddress2 TEXT, pro_custaddress3 TEXT, pro_drawingofficename TEXT,
           pro_doaddress1 TEXT, pro_doaddress2 TEXT, pro_doaddress3 TEXT, pro_contractnb TEXT,
           pro_createdby TEXT, pro_modifiedby TEXT, pro_creationdate TEXT, pro_modificationdate TEXT);
         CREATE TABLE tew_translatedtext (tra_objectid INTEGER, tra_objectno INTEGER, tra_lan_strid TEXT, tra_0 TEXT);
         INSERT INTO tew_project VALUES
           (1, 'Pump station', '3', 'Example Water Ltd', 'Harbour Road 1', '', 'Sample Town', 'Drawing Office A',
            NULL, NULL, NULL, 'P-1042', 'designer', 'checker', '20240131 08:15:00 ', '20250203 10:00:00 '),
           (2, 'Other', '12', '', '', '', '', '', '', '', '', '', '', '', '', '');
         INSERT INTO tew_translatedtext VALUES (1, -1, 'de', 'Pumpwerk'), (1, -1, 'en', 'Pump station control');",
    )
    .unwrap();
    c.serialize(rusqlite::MAIN_DB).unwrap().to_vec()
}

#[test]
fn the_project_list_gives_the_title_block_fields() {
    let list = project_list();
    let info = project_info(&open(&list).unwrap(), "3").unwrap().unwrap();
    assert_eq!(info.name, "Pump station");
    assert_eq!(
        info.customer_address,
        ["Harbour Road 1", "Sample Town"],
        "blank lines dropped"
    );
    assert_eq!(
        (info.created.as_str(), info.modified.as_str()),
        ("2024-01-31", "2025-02-03")
    );
    let tb = info.title_block();
    assert_eq!(tb["customer"], "Example Water Ltd");
    assert_eq!(tb["customer_address"], "Harbour Road 1, Sample Town");
    assert_eq!(tb["project_number"], "P-1042");
    assert_eq!(tb["drawn_by"], "designer");
    assert_eq!(tb["date"], "2024-01-31");
    assert_eq!(tb["description"], "Pump station control", "English wins");
    assert!(!tb.contains_key("drawing_office_address"), "empty fields are left out");
    let other = project_info(&open(&list).unwrap(), "12").unwrap().unwrap();
    assert!(other.title_block().is_empty());
    assert!(project_info(&open(&list).unwrap(), "99").unwrap().is_none());
}

#[test]
fn an_archive_import_carries_the_project_fields_into_the_title_block() {
    let db = sample().serialize(rusqlite::MAIN_DB).unwrap().to_vec();
    let list = project_list();
    let buf = archive(&[
        ("Projects/3/Database/tew_project_data.sqlite", &db),
        ("Database/tew_app_project.sqlite", &list),
    ]);
    let got = database_from_archive(buf, 1).unwrap();
    assert!(got.info_warning.is_none());
    let mut p = read(&open(&got.bytes).unwrap()).unwrap();
    p.info = got.info;
    let (built, _, _) = build(&p, None);
    assert_eq!(built.title_block["customer"], "Example Water Ltd");
    assert_eq!(built.title_block["project_number"], "P-1042");
    // Without the project list (a bare database, or an older archive) nothing is guessed.
    let bare = archive(&[("Projects/3/Database/tew_project_data.sqlite", &db)]);
    let got = database_from_archive(bare, 1).unwrap();
    assert!(got.info.is_none() && got.info_warning.is_none());
}

#[test]
fn the_project_fields_come_across_without_a_description_table_and_with_numbers() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE tew_project (pro_id INTEGER, pro_name TEXT, pro_directory TEXT, pro_customername TEXT,
           pro_custaddress1 TEXT, pro_custaddress2 TEXT, pro_custaddress3 TEXT, pro_drawingofficename TEXT,
           pro_doaddress1 TEXT, pro_doaddress2 TEXT, pro_doaddress3 TEXT, pro_contractnb INTEGER,
           pro_createdby TEXT, pro_modifiedby TEXT, pro_creationdate TEXT, pro_modificationdate TEXT);
         INSERT INTO tew_project VALUES (1, 'P', '1', 'Customer', '', '', '', '', '', '', '', 1042,
           'designer', '', '', '');",
    )
    .unwrap();
    let info = project_info(&c, "1").unwrap().unwrap();
    assert_eq!(
        info.contract_number, "1042",
        "a number stored as an integer is read as text"
    );
    assert_eq!(info.customer, "Customer");
    assert!(info.description.is_empty());
    assert!(
        info.warnings
            .iter()
            .any(|w| w.starts_with("the project description was not read")),
        "{:?}",
        info.warnings
    );
}

#[test]
fn an_unreadable_description_row_is_reported_with_its_reason() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE tew_project (pro_id INTEGER, pro_name TEXT, pro_directory TEXT, pro_customername TEXT,
           pro_custaddress1 TEXT, pro_custaddress2 TEXT, pro_custaddress3 TEXT, pro_drawingofficename TEXT,
           pro_doaddress1 TEXT, pro_doaddress2 TEXT, pro_doaddress3 TEXT, pro_contractnb TEXT,
           pro_createdby TEXT, pro_modifiedby TEXT, pro_creationdate TEXT, pro_modificationdate TEXT);
         INSERT INTO tew_project VALUES (1, 'P', '1', 'Customer', '', '', '', '', '', '', '', '7',
           '', '', '20240102 10:00:00', 'not a date');
         CREATE TABLE tew_translatedtext (tra_objectid INTEGER, tra_objectno INTEGER, tra_lan_strid TEXT, tra_0);
         INSERT INTO tew_translatedtext VALUES (1, -1, 'en', 42.5);",
    )
    .unwrap();
    let info = project_info(&c, "1").unwrap().unwrap();
    assert_eq!(info.customer, "Customer", "the other fields still come across");
    assert!(info.description.is_empty());
    assert!(
        info.warnings
            .iter()
            .any(|w| w.contains("text row(s) could not be read")),
        "{:?}",
        info.warnings
    );
    assert_eq!(info.created, "2024-01-02");
    assert_eq!(info.modified, "not a date", "a date in another form is kept as written");
}

#[test]
fn a_project_list_that_cannot_be_opened_is_a_warning_not_silence() {
    use std::io::Write;
    let db = sample().serialize(rusqlite::MAIN_DB).unwrap().to_vec();
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opt = zip::write::SimpleFileOptions::default();
        z.start_file("Projects/1/Database/tew_project_data.sqlite", opt)
            .unwrap();
        z.write_all(&db).unwrap();
        use zip::unstable::write::FileOptionsExt;
        let locked = opt.with_deprecated_encryption(b"secret");
        z.start_file("Database/tew_app_project.sqlite", locked).unwrap();
        z.write_all(&db).unwrap();
        z.finish().unwrap();
    }
    buf.set_position(0);
    let got = database_from_archive(buf, 1).unwrap();
    assert!(got.info.is_none());
    let w = got.info_warning.unwrap();
    assert!(w.contains("tew_app_project.sqlite"), "{w}");

    // Without a project list there are simply no fields, and no warning.
    let got = database_from_archive(archive(&[("Projects/1/Database/tew_project_data.sqlite", &db)]), 1).unwrap();
    assert!(got.info.is_none() && got.info_warning.is_none());
}

#[test]
fn a_blank_english_translation_does_not_hide_another_language() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE tew_project (pro_id INTEGER, pro_name TEXT, pro_directory TEXT, pro_customername TEXT,
           pro_custaddress1 TEXT, pro_custaddress2 TEXT, pro_custaddress3 TEXT, pro_drawingofficename TEXT,
           pro_doaddress1 TEXT, pro_doaddress2 TEXT, pro_doaddress3 TEXT, pro_contractnb TEXT,
           pro_createdby TEXT, pro_modifiedby TEXT, pro_creationdate TEXT, pro_modificationdate TEXT);
         INSERT INTO tew_project VALUES (1, 'P', '1', '', '', '', '', '', '', '', '', '', '', '', '', '');
         CREATE TABLE tew_translatedtext (tra_objectid INTEGER, tra_objectno INTEGER, tra_lan_strid TEXT, tra_0 TEXT);
         INSERT INTO tew_translatedtext VALUES (1, -1, 'de', ' Beschreibung '), (1, -1, 'en', char(9) || char(10));",
    )
    .unwrap();
    let info = project_info(&c, "1").unwrap().unwrap();
    assert_eq!(info.description, "Beschreibung");
    assert!(info.warnings.is_empty(), "{:?}", info.warnings);
}

#[test]
fn a_mirrored_symbol_turned_90_degrees_meets_its_line() {
    let c = sample();
    // Point (0, 5) turned by 90 degrees is (-5, 0); mirrored in x it is (5, 0), where the line
    // starts. Scaling first would put it at (-5, 0), away from the line.
    c.execute_batch(
        "INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type) VALUES (110, 'K9', '-K9', -1);
         INSERT INTO tew_symbol VALUES (40, 10, 110, 'Coil', 150, 100, 90, -1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (40, 0, 0, 0, 5);
         INSERT INTO tew_line VALUES (30, 10, 155, 100, 180, 100, NULL);",
    )
    .unwrap();
    let (built, _, _) = build(&read(&c).unwrap(), None);
    let pin = Endpoint::Pin {
        item: "s40".into(),
        pin: "1".into(),
    };
    assert!(
        page(&built, "01").wires.iter().any(|w| w.from == pin || w.to == pin),
        "the line joins the mirrored, turned pin"
    );
}

#[test]
fn a_mirrored_box_turned_90_degrees_has_its_corner_mirrored_too() {
    let c = sample();
    // Box 35 by -40 in the symbol's frame: turned (40, 35), mirrored in x (-40, 35).
    c.execute_batch(
        "INSERT INTO tew_symbol VALUES (41, 10, NULL, 'Box', 150, 100, 90, -1, 1, 35, -40);
         INSERT INTO tew_symbolpt VALUES (41, 0, 0, 0, 5);",
    )
    .unwrap();
    let p = read(&c).unwrap();
    let s = p.symbols.iter().find(|s| s.id == 41).unwrap();
    let corner = s.corner.unwrap();
    assert!(
        (corner.x - 110.0).abs() < 1e-9 && (corner.y - 135.0).abs() < 1e-9,
        "{corner:?}"
    );
}

#[test]
fn wire_records_without_a_drawn_line_are_counted_with_their_reason() {
    let c = sample_with_strip();
    // A jumper X1.1 upper to X1.2 lower, and a record X1.2 lower to K1:A2, neither on a line.
    c.execute_batch(
        "INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag, wir_cel_idfrom, wir_cte_nofrom, wir_cel_idto, wir_cte_noto)
           VALUES (90, NULL, -1, '', 60, 0, 61, 1), (91, NULL, -1, '', 61, 1, 50, 1);",
    )
    .unwrap();
    let (_, warnings, report) = build(&read(&c).unwrap(), None);
    let ends = report
        .iter()
        .find(|c| c.what == "wire ends (from/to terminals)")
        .unwrap();
    assert!(
        ends.note.contains("1 are jumpers between terminals of one strip"),
        "{}",
        ends.note
    );
    assert!(
        ends.note
            .contains("1 are in the source's wire list but on no drawn line"),
        "{}",
        ends.note
    );
    assert!(
        ends.note.contains("1 join terminals that are not connected"),
        "only N2: {}",
        ends.note
    );
    let apart = warnings
        .iter()
        .find(|w| w.contains("not connected on the imported drawing"))
        .unwrap();
    assert!(apart.starts_with("1 wire(s)"), "{apart}");
}

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

/// One book with a folder `Power`: a schematic page 01 with a relay coil K1 (pins A1 above, A2
/// below) and a fuse F1 rotated by 90 degrees, wired F1 -> K1:A1 with potential 101 through
/// core 1 (brown) of cable W1; a report page 00; a frame without connection points.
pub(super) fn sample() -> Connection {
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
    // K1's second part is kept now (see `extra_parts_of_a_component_are_kept`): no warnings.
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
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
fn extra_parts_of_a_component_are_kept() {
    let p = read(&sample()).unwrap();
    assert_eq!(p.components[&100].part_number, "RLY-24");
    assert_eq!(p.components[&100].extra_parts.len(), 1);
    assert!(!p.warnings.iter().any(|w| w.contains("left out")), "{:?}", p.warnings);
    // They reach the device as numbered properties, which `link_parts` turns into accessories.
    let (built, _, _) = build(&p, None);
    let (number, maker) = &p.components[&100].extra_parts[0];
    assert!(built.devices.values().any(|d| {
        d.properties.get("PartNumber") == Some(&"RLY-24".to_owned())
            && d.properties.get("PartNumber 2") == Some(number)
            && (maker.is_empty() || d.properties.get("Manufacturer 2") == Some(maker))
    }));
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
/// with a space), drawn at (150, 200) and (170, 200); line 10 joins X1.1's lower point to K1:A2
/// and carries wire record N1; line 11 runs up from X1.2's upper point and carries N2.
/// Wire records: N1 X1.1 lower to K1:A2 (drawn so), N2 X1.2 upper to K1:A1 (not drawn so), one to
/// a terminal that is not drawn, and one with no ends.
fn sample_with_strip() -> Connection {
    let c = sample();
    c.execute_batch(
        "
INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type, com_com_id, com_tagorderno) VALUES (200, 'X1', '-X1', 3, NULL, 0), (201, '1', '-X1 1', 4, 200, 2), (202, '2', '-X1 2', 4, 200, 1);
INSERT INTO tew_symbol VALUES (20, 10, 201, 'Terminal', 150, 200, 0, 1, 1, NULL, NULL),
  (21, 10, 202, 'Terminal', 170, 200, 0, 1, 1, NULL, NULL);
INSERT INTO tew_symbolpt VALUES (20, 0, 0, 0, 5), (20, 0, 1, 0, -5), (21, 0, 0, 0, 5), (21, 0, 1, 0, -5);
INSERT INTO tew_componentelement VALUES (60, 20, 0, 201, 0), (61, 21, 0, 202, 0);
INSERT INTO tew_componentterminal VALUES (60, 0, NULL), (60, 1, NULL), (61, 0, NULL), (61, 1, NULL);
INSERT INTO tew_line VALUES (10, 10, 150, 195, 100, 195, NULL), (11, 10, 170, 205, 170, 215, NULL);
INSERT INTO tew_linewirepair VALUES (10, 80, 1), (11, 81, 1);
INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag, wir_cel_idfrom, wir_cte_nofrom, wir_cel_idto, wir_cte_noto) VALUES (80, NULL, -1, 'N1', 60, 1, 50, 1), (81, NULL, -1, 'N2', 61, 0, 50, 0),
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
    c.execute_batch("INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type, com_com_id, com_tagorderno) VALUES (300, '5', '-X9-5', 4, NULL, 0);")
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
        "INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type, com_com_id, com_tagorderno) VALUES (203, '3.1', '-X1-3.1', 4, 200, 3), (204, '1+2', '-X1-1+2', 4, 200, 4);
         INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag, wir_cel_idfrom, wir_cte_nofrom, wir_cel_idto, wir_cte_noto) VALUES (84, NULL, -1, '', 60, 0, NULL, NULL);",
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
        "INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type, com_com_id, com_tagorderno) VALUES (200, 'X1', '-X1', 3, NULL, 0), (201, '1', '-X1-1', 4, 200, 1);
         INSERT INTO tew_symbol VALUES (20, 10, 201, 'Terminal', 150, 200, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (20, 0, 0, 0, 5), (20, 0, 1, 0, -5);
         INSERT INTO tew_componentelement VALUES (60, 20, 0, 201, 0);
         INSERT INTO tew_componentterminal VALUES (60, 0, '1'), (60, 1, '1');
         INSERT INTO tew_line VALUES (10, 10, 150, 195, 100, 195, NULL);
         INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag, wir_cel_idfrom, wir_cte_nofrom, wir_cel_idto, wir_cte_noto) VALUES (80, NULL, -1, 'N1', 60, 1, 50, 1);",
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
        "INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type, com_com_id, com_tagorderno) VALUES (200, 'X1', '-X1', 3, NULL, 0), (201, '01', '-X1-01', 4, 200, 1),
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

/// A database in the space form: function F1, locations L1 and L1 L11, and components whose
/// tag paths have no signs, as some SolidWorks Electrical projects store them.
fn space_form() -> Connection {
    let c = sample();
    c.execute_batch(
        "CREATE TABLE tew_function (fun_id INTEGER, fun_tagpath TEXT);
         CREATE TABLE tew_location (loc_id INTEGER, loc_tagpath TEXT);
         INSERT INTO tew_function VALUES (2, 'F1');
         INSERT INTO tew_location VALUES (3, 'L1'), (2450, 'L1 L11');
         INSERT INTO tew_component VALUES (500, 'CR1', 'F1 L1 L11 CR1', -1, NULL, 0, 2, 2450, 'CR1'),
           (501, 'TB1', 'F1 L1 L11 TB1', 3, NULL, 0, 2, 2450, 'TB1'),
           (502, '4', 'F1 L1 L11 TB1 4', 4, 501, 4, 2, 2450, 'TB1 4'),
           (503, 'M2', 'F1 L1 M2', -1, NULL, 0, 2, 3, 'M2');",
    )
    .unwrap();
    c
}

#[test]
fn tag_paths_stored_with_spaces_keep_function_and_location() {
    let p = read(&space_form()).unwrap();
    assert_eq!(p.components[&500].tag, "=F1+L1+L11-CR1");
    assert_eq!(p.components[&503].tag, "=F1+L1-M2");
    assert_eq!(p.components[&100].tag, "=F1+L1+L2-K1", "a signed path stays as it is");
    assert_eq!(
        p.components[&101].tag, "F1",
        "a bare tag without function or location stays"
    );
    assert!(
        p.warnings
            .iter()
            .any(|w| w.starts_with("4 component tag path(s) are stored with spaces")),
        "{:?}",
        p.warnings
    );
    let (built, _, _) = build(&p, None);
    // Not every device has =F1 (the sample's F1 has none), so the function stays in the tags.
    for t in ["=F1+L1.L11-CR1", "=F1+L1-M2", "=F1+L1.L11-TB1", "=F1+L1.L11-TB1.4"] {
        assert!(
            built.devices.contains_key(&DeviceTag::parse(t).unwrap()),
            "{t}: {:?}",
            built.devices.keys()
        );
    }
    let paths: Vec<&str> = built.locations.iter().map(|l| l.path.as_str()).collect();
    assert_eq!(paths, ["L1", "L1.L11"], "location paths in the space form");
}

#[test]
fn signed_paths_and_space_paths_compose_alike() {
    assert_eq!(signed("L1 L11", '+'), "+L1+L11");
    assert_eq!(signed("+L1+L11", '+'), "+L1+L11");
    assert_eq!(signed("  ", '+'), "");
    assert_eq!(composed_path(Some("F1"), Some("L1 L11"), "TB1 4"), "=F1+L1+L11-TB1-4");
    assert_eq!(composed_path(None, None, "K1"), "-K1");
    assert_eq!(location_path("L1 L11"), "L1.L11");
    assert_eq!(location_path("+L1+L11"), "L1.L11");
}

#[test]
fn space_form_edge_cases_keep_their_levels() {
    let c = space_form();
    c.execute_batch(
        "INSERT INTO tew_location VALUES (7, 'Deck-2');
         INSERT INTO tew_component VALUES (510, 'K-1', 'F1 Deck-2 K-1', -1, NULL, 0, 2, 7, 'K-1'),
           (511, 'CR9', 'F1 L1 L11 CR9', -1, NULL, 0, 2, 2450, '');",
    )
    .unwrap();
    let p = read(&c).unwrap();
    let (built, _, _) = build(&p, None);
    for t in ["=F1+Deck_2-K_1", "=F1+L1.L11-CR9"] {
        assert!(
            built.devices.contains_key(&DeviceTag::parse(t).unwrap()),
            "{t}: {:?}",
            built.devices.keys()
        );
    }
    assert!(
        built.locations.iter().any(|l| l.path == "Deck_2"),
        "{:?}",
        built.locations
    );
    assert!(has_signs(" -X1 1") && !has_signs("Deck-2"));
}

#[test]
fn a_bare_tag_with_spaces_and_no_function_or_location_stays_one_name() {
    let c = space_form();
    c.execute_batch("INSERT INTO tew_component VALUES (520, 'X 1/2', 'X 1/2', -1, NULL, 0, NULL, NULL, '');")
        .unwrap();
    let p = read(&c).unwrap();
    assert_eq!(p.components[&520].tag, "X 1/2", "as written");
    let (built, _, _) = build(&p, None);
    assert!(
        built.devices.contains_key(&DeviceTag::parse("-X_1_2").unwrap()),
        "one product level: {:?}",
        built.devices.keys()
    );
}

#[test]
fn wires_carry_their_section_and_colour() {
    assert_eq!(wire_section(Some(0), "1.5").as_deref(), Some("1.5 mm²"));
    assert_eq!(wire_section(Some(1), " 20 ").as_deref(), Some("20 AWG"));
    assert_eq!(wire_section(Some(0), "0"), None, "0 is no section");
    assert_eq!(wire_section(Some(-1), ""), None);
    assert_eq!(
        wire_section(None, "2,5").as_deref(),
        Some("2,5"),
        "unknown type: as written"
    );

    let c = sample();
    c.execute_batch(
        "UPDATE tew_wire SET wir_sectiontype = 0, wir_sectionorgauge = '0.75', wir_color = 'BN' WHERE wir_id = 70;",
    )
    .unwrap();
    let p = read(&c).unwrap();
    assert_eq!(p.wires[&70].spec.section.as_deref(), Some("0.75 mm²"));
    assert_eq!(p.wires[&71].spec, WireSpec::default());
    let (built, _, _) = build(&p, None);
    let drawn: Vec<&Wire> = page(&built, "01").wires.iter().filter(|w| w.cable.is_some()).collect();
    assert!(!drawn.is_empty());
    for w in drawn {
        assert_eq!(w.spec.section.as_deref(), Some("0.75 mm²"));
        assert_eq!(w.spec.colour.as_deref(), Some("BN"));
    }
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
fn an_empty_function_path_does_not_rebuild_the_stored_tag() {
    let c = space_form();
    c.execute_batch(
        "INSERT INTO tew_function (fun_id, fun_tagpath) VALUES (930, '');
         INSERT INTO tew_component VALUES (530, 'K2', 'K1', -1, NULL, 0, 930, NULL, '');",
    )
    .unwrap();
    let p = read(&c).unwrap();
    assert_eq!(
        p.components[&530].tag, "K1",
        "no function resolved, the stored path stays"
    );
}

#[test]
fn a_component_name_used_as_product_stays_one_level() {
    let c = space_form();
    c.execute_batch(
        "INSERT INTO tew_function (fun_id, fun_tagpath) VALUES (931, 'F1');
         INSERT INTO tew_component VALUES (531, 'X 1/2', 'F1 X 1/2', -1, NULL, 0, 931, NULL, '');",
    )
    .unwrap();
    let p = read(&c).unwrap();
    assert_eq!(p.components[&531].tag, "=F1-X_1/2");
    let (built, _, _) = build(&p, None);
    assert!(
        built.devices.contains_key(&DeviceTag::parse("=F1-X_1_2").unwrap()),
        "one product level: {:?}",
        built.devices.keys()
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
        ends.note.contains(
            "1 are jumpers between terminals of one strip that no drawn line carries, made into the strip's jumpers"
        ),
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

#[test]
fn a_wire_on_a_zero_length_line_still_counts_as_drawn() {
    let c = sample_with_strip();
    // A record X1.2 lower to K1:A2 whose only line has no length: the source draws it, so it is
    // a mismatch to report, not a record without a line.
    c.execute_batch(
        "INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag, wir_cel_idfrom, wir_cte_nofrom, wir_cel_idto, wir_cte_noto)
           VALUES (92, NULL, -1, 'N9', 61, 1, 50, 1);
         INSERT INTO tew_line VALUES (12, 10, 170, 195, 170, 195, NULL);
         INSERT INTO tew_linewirepair VALUES (12, 92, 1);",
    )
    .unwrap();
    let (_, warnings, report) = build(&read(&c).unwrap(), None);
    let ends = report
        .iter()
        .find(|c| c.what == "wire ends (from/to terminals)")
        .unwrap();
    assert!(!ends.note.contains("on no drawn line"), "{}", ends.note);
    assert!(
        ends.note.contains("2 join terminals that are not connected"),
        "{}",
        ends.note
    );
    let apart = warnings
        .iter()
        .find(|w| w.contains("not connected on the imported drawing"))
        .unwrap();
    assert!(apart.contains("wire N9"), "{apart}");
}

#[test]
fn a_wire_on_a_line_without_usable_end_points_still_counts_as_drawn() {
    let c = sample_with_strip();
    // Its only line has no end point, so the line is left out; the source still draws the wire.
    c.execute_batch(
        "INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag, wir_cel_idfrom, wir_cte_nofrom, wir_cel_idto, wir_cte_noto)
           VALUES (93, NULL, -1, 'N8', 61, 1, 50, 1);
         INSERT INTO tew_line VALUES (13, 10, NULL, 195, 170, 195, NULL);
         INSERT INTO tew_linewirepair VALUES (13, 93, 1);",
    )
    .unwrap();
    let p = read(&c).unwrap();
    assert!(p.wires_on_lines.contains(&93) && !p.lines.iter().any(|l| l.id == 13));
    let (_, warnings, report) = build(&p, None);
    let ends = report
        .iter()
        .find(|c| c.what == "wire ends (from/to terminals)")
        .unwrap();
    assert!(!ends.note.contains("on no drawn line"), "{}", ends.note);
    let apart = warnings
        .iter()
        .find(|w| w.contains("not connected on the imported drawing"))
        .unwrap();
    assert!(apart.contains("wire N8"), "{apart}");
}

#[test]
fn a_component_name_with_a_leading_sign_stays_the_product() {
    let c = space_form();
    c.execute_batch(
        "INSERT INTO tew_function (fun_id, fun_tagpath) VALUES (932, 'F1');
         INSERT INTO tew_component VALUES (532, '+24V', 'F1 +24V', -1, NULL, 0, 932, NULL, '');",
    )
    .unwrap();
    let p = read(&c).unwrap();
    assert_eq!(p.components[&532].tag, "=F1-_24V");
}

#[test]
fn a_spaced_location_path_counts_as_the_space_form() {
    let c = space_form();
    c.execute_batch(
        "INSERT INTO tew_location (loc_id, loc_tagpath) VALUES (940, 'L1 L11');
         INSERT INTO tew_component VALUES (540, 'K1', 'K1', -1, NULL, 0, NULL, 940, '');",
    )
    .unwrap();
    let p = read(&c).unwrap();
    assert_eq!(p.components[&540].tag, "+L1+L11-K1");
    assert!(
        p.warnings.iter().any(|w| w.contains("stored with spaces")),
        "{:?}",
        p.warnings
    );
}

#[test]
fn a_cable_core_keeps_its_own_wire_s_spec() {
    let c = sample();
    // Line 1 carries the cable wire 70 (no spec) and wire 71 (with one).
    c.execute_batch(
        "UPDATE tew_wire SET wir_sectiontype = 0, wir_sectionorgauge = '1.5', wir_color = 'RD' WHERE wir_id = 71;
         INSERT INTO tew_linewirepair VALUES (1, 71, 2);",
    )
    .unwrap();
    let p = read(&c).unwrap();
    let (built, _, report) = build(&p, None);
    let cabled: Vec<&Wire> = built
        .pages
        .iter()
        .flat_map(|pg| pg.wires.iter())
        .filter(|w| w.cable.is_some())
        .collect();
    assert!(!cabled.is_empty());
    for w in cabled {
        assert_eq!(w.spec, WireSpec::default(), "71's section is not put on 70's core");
    }
    let wires = report.iter().find(|c| c.what == "wires").unwrap();
    assert!(wires.note.contains("no section or colour"), "{}", wires.note);
}

/// A library coil whose pins sit where the sample's coil block has its points: A1 5 mm above
/// the origin, A2 5 mm below (page y runs down).
fn lib_coil(id: &str, a2_at: f64) -> Symbol {
    let pin = |name: &str, y: f64, dir| Pin {
        name: name.into(),
        at: Point::new(0.0, y),
        dir,
    };
    Symbol {
        id: id.into(),
        name: "Relay coil".into(),
        category: "relays".into(),
        letter_code: "K".into(),
        role: SymbolRole::Main,
        graphics: vec![Shape::Rect {
            min: Point::new(-4.0, -3.0),
            max: Point::new(4.0, 3.0),
        }],
        pins: vec![pin("A1", -5.0, PinDir::Up), pin("A2", a2_at, PinDir::Down)],
        tag_at: Point::new(5.0, 0.0),
        description: String::new(),
    }
}

fn choice(symbols: &[Symbol], map: &[(&str, &str)]) -> SymbolChoice {
    SymbolChoice {
        library: symbols.iter().map(|s| (s.id.clone(), s.clone())).collect(),
        by_block: map.iter().map(|(b, i)| ((*b).to_owned(), (*i).to_owned())).collect(),
    }
}

fn count<'a>(report: &'a [Count], what: &str) -> &'a Count {
    report.iter().find(|c| c.what == what).unwrap()
}

#[test]
fn a_mapped_block_is_drawn_as_the_library_symbol_and_keeps_its_wire() {
    let sw = read(&sample()).unwrap();
    let (built, _, report) = build_with(&sw, None, &choice(&[lib_coil("my.coil", 5.0)], &[("coil", "my.coil")]));
    let k1 = page(&built, "01").symbols.iter().find(|s| s.id == "s1").unwrap();
    assert_eq!(k1.symbol, "my.coil", "the map matches the block name ignoring case");
    assert!(
        built.symbols.iter().any(|s| s.id == "my.coil"),
        "used library symbols come along"
    );
    assert_eq!(
        built.symbols.len(),
        2,
        "the coil from the library, one box for the fuse"
    );
    // The wire still ends on K1:A1.
    let ends_on_a1 = page(&built, "01").wires.iter().any(|w| {
        [&w.from, &w.to]
            .iter()
            .any(|e| matches!(e, Endpoint::Pin { item, pin } if item == "s1" && pin == "A1"))
    });
    assert!(ends_on_a1);
    let c = count(&report, "library symbols");
    assert_eq!((c.source, c.imported), (2, 1));
    assert_eq!(
        c.note,
        "1 drawn from their DWG block or as boxes: no library symbol fits"
    );
}

#[test]
fn a_built_in_match_needs_the_pins_where_the_points_are() {
    let sw = read(&sample()).unwrap();
    // Pins named A1 and A2 match wc.coil without a map, when they sit where the points do.
    let (built, _, _) = build_with(&sw, None, &choice(&[lib_coil("wc.coil", 5.0)], &[]));
    let k1 = page(&built, "01").symbols.iter().find(|s| s.id == "s1").unwrap();
    assert_eq!(k1.symbol, "wc.coil");
    // A2 15 mm lower than the block's point: no fit, a box as before.
    let (built, _, report) = build_with(&sw, None, &choice(&[lib_coil("wc.coil", 15.0)], &[]));
    let k1 = page(&built, "01").symbols.iter().find(|s| s.id == "s1").unwrap();
    assert!(k1.symbol.starts_with("swe."), "{}", k1.symbol);
    assert_eq!(count(&report, "library symbols").imported, 0);
}

#[test]
fn a_mapped_block_that_does_not_fit_stays_a_box() {
    let sw = read(&sample()).unwrap();
    // Mapped to a symbol whose pins are elsewhere: a box, and no fallback to the built-in match.
    let lib = [lib_coil("my.coil", 15.0), lib_coil("wc.coil", 5.0)];
    let (built, _, _) = build_with(&sw, None, &choice(&lib, &[("Coil", "my.coil")]));
    let k1 = page(&built, "01").symbols.iter().find(|s| s.id == "s1").unwrap();
    assert!(k1.symbol.starts_with("swe."), "{}", k1.symbol);
    // Mapped to a symbol the library does not have: a box too.
    let (built, _, _) = build_with(&sw, None, &choice(&lib, &[("Coil", "gone.coil")]));
    let k1 = page(&built, "01").symbols.iter().find(|s| s.id == "s1").unwrap();
    assert!(k1.symbol.starts_with("swe."), "{}", k1.symbol);
}

#[test]
fn a_terminal_block_only_becomes_a_terminal_symbol() {
    let sw = read(&sample()).unwrap();
    let mut terminal = lib_coil("my.terminal", 5.0);
    terminal.role = SymbolRole::Terminal;
    // The coil block belongs to an ordinary component: a terminal symbol does not fit it.
    let (built, _, _) = build_with(&sw, None, &choice(&[terminal], &[("Coil", "my.terminal")]));
    let k1 = page(&built, "01").symbols.iter().find(|s| s.id == "s1").unwrap();
    assert!(k1.symbol.starts_with("swe."), "{}", k1.symbol);
}

#[test]
fn symbol_maps_are_read_line_by_line() {
    let map = parse_symbol_map("# blocks\n\nCoil = wc.coil\n  NO contact =wc.contact.no \n").unwrap();
    assert_eq!(map.get("Coil").map(String::as_str), Some("wc.coil"));
    assert_eq!(map.get("NO contact").map(String::as_str), Some("wc.contact.no"));
    assert_eq!(
        parse_symbol_map("Coil wc.coil").unwrap_err(),
        "line 1: expected `block name = symbol.id`"
    );
    assert!(parse_symbol_map("x\n = wc.coil").unwrap_err().starts_with("line 1"));
    assert!(parse_symbol_map("# c\n = wc.coil").unwrap_err().starts_with("line 2"));
}

#[test]
fn a_library_pin_a_little_off_keeps_the_net_the_source_drew_to_it() {
    let c = sample();
    // Net 202 ends 0.4 mm beside K1:A1, where net 101 ends exactly.
    c.execute_batch(
        "INSERT INTO tew_equipotential VALUES (901, '202');
         INSERT INTO tew_line VALUES (3, 10, 100.4, 205, 100.4, 240, 901);",
    )
    .unwrap();
    // The library coil's A1 sits at net 202's end, 0.4 mm from the block's point: still a fit.
    let mut coil = lib_coil("my.coil", 5.0);
    coil.pins[0].at = Point::new(0.4, -5.0);
    let (built, _, _) = build_with(&read(&c).unwrap(), None, &choice(&[coil], &[("Coil", "my.coil")]));
    let pg = page(&built, "01");
    assert_eq!(pg.symbols.iter().find(|s| s.id == "s1").unwrap().symbol, "my.coil");
    let on_a1 = |w: &Wire| {
        [&w.from, &w.to]
            .iter()
            .any(|e| matches!(e, Endpoint::Pin { item, pin } if item == "s1" && pin == "A1"))
    };
    let nets_on_a1: BTreeSet<Option<&str>> = pg
        .wires
        .iter()
        .filter(|w| on_a1(w))
        .map(|w| w.potential.as_deref())
        .collect();
    assert_eq!(nets_on_a1, BTreeSet::from([Some("101")]));
}

#[test]
fn a_box_never_takes_the_id_of_a_library_symbol() {
    let sw = read(&sample()).unwrap();
    // A library symbol called like the fuse's box would be, chosen for the coil.
    let mut taken = lib_coil("swe.fuse.1", 5.0);
    taken.name = "Coil from an earlier import".into();
    let (built, _, _) = build_with(&sw, None, &choice(&[taken], &[("Coil", "swe.fuse.1")]));
    let pg = page(&built, "01");
    let k1 = pg.symbols.iter().find(|s| s.id == "s1").unwrap();
    let f1 = pg.symbols.iter().find(|s| s.id == "s2").unwrap();
    assert_eq!(k1.symbol, "swe.fuse.1");
    assert_ne!(f1.symbol, "swe.fuse.1");
    let ids: Vec<&str> = built.symbols.iter().map(|s| s.id.as_str()).collect();
    let unique: BTreeSet<&str> = ids.iter().copied().collect();
    assert_eq!(ids.len(), unique.len(), "one definition per id: {ids:?}");
    let pins = |id: &str| -> Vec<String> {
        built
            .symbols
            .iter()
            .find(|s| s.id == id)
            .unwrap()
            .pins
            .iter()
            .map(|p| p.name.clone())
            .collect()
    };
    assert_eq!(pins(&k1.symbol), ["A1", "A2"]);
    assert_eq!(pins(&f1.symbol).len(), 2);
}

#[test]
fn each_page_takes_its_latest_revision_into_its_own_title_block() {
    let c = sample();
    c.execute_batch(
        "
CREATE TABLE tew_revision (rev_id INTEGER, rev_fil_id INTEGER, rev_orderno INTEGER, rev_tag TEXT,
  rev_createdby TEXT, rev_creationdate TEXT, rev_checkedby TEXT, rev_checkdate TEXT,
  rev_approvedby TEXT, rev_approvaldate TEXT);
-- Written out of order: order number 2 is the latest.
INSERT INTO tew_revision VALUES (2, 10, 2, 'B', 'Ann', '20240302 10:00:00', 'Bob', '20240303 09:00:00', '', NULL),
  (1, 10, 1, 'A', 'Ann', '20240101 10:00:00', 'Bob', '20240102 10:00:00', 'Cid', '20240103 10:00:00'),
  -- A page the project does not have.
  (3, 999, 1, 'Z', 'Nobody', '', '', '', '', '');
",
    )
    .unwrap();
    let p = read(&c).unwrap();
    let sch = p.pages.iter().find(|pg| pg.id == 10).unwrap();
    let want: BTreeMap<String, String> = [
        ("revision", "B"),
        ("drawn_by", "Ann"),
        ("date", "2024-03-02"),
        ("checked_by", "Bob"),
        ("checked_date", "2024-03-03"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    assert_eq!(sch.title_block, want, "the latest revision, blank fields left out");
    let report = p.pages.iter().find(|pg| pg.id == 11).unwrap();
    assert!(report.title_block.is_empty());
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);

    let (built, _, _) = build(&p, None);
    assert_eq!(page(&built, "01").title_block, want);
}

#[test]
fn revisions_by_object_id_keep_only_the_pages_ones() {
    let c = sample();
    c.execute_batch(
        "
CREATE TABLE tew_revision (rev_id INTEGER, rev_objectid INTEGER, rev_tag TEXT);
-- Object 1 is the book, 10 the schematic page.
INSERT INTO tew_revision VALUES (1, 1, 'R1'), (2, 10, 'P3');
",
    )
    .unwrap();
    let p = read(&c).unwrap();
    let sch = p.pages.iter().find(|pg| pg.id == 10).unwrap();
    assert_eq!(sch.title_block.get("revision").map(String::as_str), Some("P3"));
    assert_eq!(sch.title_block.len(), 1);
    // The book's row is the project's revision, in the project's title block.
    assert_eq!(p.revision.get("revision").map(String::as_str), Some("R1"));
    let (built, _, _) = build(&p, None);
    assert_eq!(built.title_block.get("revision").map(String::as_str), Some("R1"));
}

#[test]
fn the_projects_latest_revision_and_checker_fill_its_title_block() {
    let c = sample();
    c.execute_batch(
        "
CREATE TABLE tew_revision (rev_id INTEGER, rev_objectid INTEGER, rev_orderno INTEGER, rev_tag TEXT,
  rev_checkedby TEXT, rev_createdby TEXT);
-- Object 1 is the book: order number 2 is its latest revision. Object 77 is neither a page
-- nor a book (say a page row that could not be read): its later row is not the project's.
INSERT INTO tew_revision VALUES (2, 1, 2, 'B', 'Bob', 'Dan'), (1, 1, 1, 'A', 'Ann', 'Ann'),
  (3, 10, 1, 'P1', 'Cid', 'Cid'), (4, 77, 9, 'X', 'Xan', 'Xan');
",
    )
    .unwrap();
    let mut p = read(&c).unwrap();
    // The project's own fields win; the revision fills only what they leave.
    p.info = Some(SweProjectInfo {
        created_by: "Eve".into(),
        ..Default::default()
    });
    let (built, _, _) = build(&p, None);
    let field = |k: &str| built.title_block.get(k).map(String::as_str);
    assert_eq!(field("revision"), Some("B"));
    assert_eq!(field("checked_by"), Some("Bob"));
    assert_eq!(field("drawn_by"), Some("Eve"));
    assert_eq!(
        page(&built, "01").title_block.get("revision").map(String::as_str),
        Some("P1")
    );
    assert_eq!(p.revision.get("revision").map(String::as_str), Some("B"));
}

#[test]
fn with_several_books_the_first_books_revision_is_the_projects() {
    let c = sample();
    c.execute_batch(
        "
INSERT INTO tew_bundle VALUES (2, '2');
CREATE TABLE tew_revision (rev_id INTEGER, rev_objectid INTEGER, rev_orderno INTEGER, rev_tag TEXT);
-- Book 2's row comes last in the table's order, but book 1 is the first book.
INSERT INTO tew_revision VALUES (1, 1, 1, 'B1'), (2, 2, 5, 'B2');
",
    )
    .unwrap();
    let p = read(&c).unwrap();
    assert_eq!(p.revision.get("revision").map(String::as_str), Some("B1"));
    assert!(
        p.warnings.iter().any(|w| w.contains("different revisions")),
        "{:?}",
        p.warnings
    );
}

#[test]
fn a_revision_table_without_a_page_column_is_reported_not_guessed() {
    let c = sample();
    c.execute_batch(
        "CREATE TABLE tew_revision (rev_id INTEGER, rev_tag TEXT); INSERT INTO tew_revision VALUES (1, 'A');",
    )
    .unwrap();
    let p = read(&c).unwrap();
    assert!(p.pages.iter().all(|pg| pg.title_block.is_empty()));
    assert!(
        p.warnings.iter().any(|w| w.contains("page revisions were not read")),
        "{:?}",
        p.warnings
    );
}

#[test]
fn the_first_cover_page_becomes_the_projects_cover_page() {
    let c = sample();
    c.execute_batch(
        "INSERT INTO tew_file (fil_id, fil_orderno, fil_filetype, fil_title, fil_fol_id) VALUES (12, 0, 5, 'Cover', 7), (13, 3, 5, 'Cover 2', 7);
         -- A text on the cover is not drawn: WiringCraft draws the cover from the project's fields.
         INSERT INTO tew_symbol VALUES (9, 12, NULL, 'Logo', 10, 10, 0, 1, 1, NULL, NULL);",
    )
    .unwrap();
    let p = read(&c).unwrap();
    let (built, _, report) = build(&p, None);
    let covers: Vec<&Page> = built.pages.iter().filter(|pg| pg.cover).collect();
    assert_eq!(covers.len(), 1, "one cover page");
    assert_eq!(covers[0].id, "swe-page-12");
    assert!(covers[0].symbols.is_empty() && covers[0].wires.is_empty());
    assert_eq!(built.pages[0].id, "swe-page-12", "in source order, first");
    assert_eq!(built.pages.len(), 2);
    let pages = report.iter().find(|r| r.what == "pages").unwrap();
    assert_eq!((pages.source, pages.imported), (4, 2));
    assert!(pages.note.contains("1 further cover page"), "{}", pages.note);
    assert!(pages.note.contains("1 report"), "{}", pages.note);
}

/// The sample fuse's block as a library symbol drawn the way the block is defined (left to
/// right, before the page turns it by 90 degrees), with the import's names for its points.
fn lib_fuse_as_defined() -> Symbol {
    let sw = read(&sample()).unwrap();
    let (built, _, _) = build_with(&sw, None, &SymbolChoice::default());
    let f1 = page(&built, "01").symbols.iter().find(|s| s.id == "s2").unwrap();
    let boxed = built.symbols.iter().find(|s| s.id == f1.symbol).unwrap();
    let mut fuse = lib_coil("my.fuse", 5.0);
    fuse.name = "Fuse".into();
    fuse.letter_code = "F".into();
    // The block's points on the page, turned back a quarter (270 degrees clockwise).
    fuse.pins = boxed
        .pins
        .iter()
        .map(|p| Pin {
            at: p.at.transform(Rotation::R270, false),
            ..p.clone()
        })
        .collect();
    assert!(
        fuse.pins.iter().all(|p| p.at.y.abs() < 1e-6),
        "drawn left to right: {:?}",
        fuse.pins
    );
    fuse
}

#[test]
fn a_turned_block_fits_the_library_symbol_turned_the_same_way() {
    let sw = read(&sample()).unwrap();
    let fuse = lib_fuse_as_defined();
    let (built, _, report) = build_with(&sw, None, &choice(std::slice::from_ref(&fuse), &[("Fuse", "my.fuse")]));
    let pg = page(&built, "01");
    let f1 = pg.symbols.iter().find(|s| s.id == "s2").unwrap();
    assert_eq!(f1.symbol, "my.fuse", "the library fuse fits the block turned");
    assert_eq!((f1.rotation, f1.mirror), (Rotation::R90, false));
    assert_eq!(count(&report, "library symbols").imported, 1);
    // The wire from F1's upper point still ends on the pin the source drew it to.
    let top = fuse
        .pins
        .iter()
        .min_by(|a, b| {
            let (a, b) = (
                a.at.transform(Rotation::R90, false),
                b.at.transform(Rotation::R90, false),
            );
            a.y.total_cmp(&b.y)
        })
        .unwrap()
        .name
        .clone();
    let ends_on_top = pg.wires.iter().any(|w| {
        [&w.from, &w.to]
            .iter()
            .any(|e| matches!(e, Endpoint::Pin { item, pin } if item == "s2" && *pin == top))
    });
    assert!(ends_on_top, "{:?}", pg.wires);
}

#[test]
fn a_block_that_fits_unturned_stays_unturned() {
    let sw = read(&sample()).unwrap();
    let (built, _, _) = build_with(&sw, None, &choice(&[lib_coil("wc.coil", 5.0)], &[]));
    let k1 = page(&built, "01").symbols.iter().find(|s| s.id == "s1").unwrap();
    assert_eq!(k1.symbol, "wc.coil");
    assert_eq!((k1.rotation, k1.mirror), (Rotation::R0, false));
}

#[test]
fn a_turned_coil_fits_a_quarter_turned_library_coil() {
    let c = sample();
    // K1 drawn lying down: A1 5 mm to the left of the origin, A2 5 mm to the right.
    c.execute_batch(
        "UPDATE tew_symbolpt SET spt_dx = -5, spt_dy = 0 WHERE spt_sym_id = 1 AND spt_no = 0;
         UPDATE tew_symbolpt SET spt_dx = 5, spt_dy = 0 WHERE spt_sym_id = 1 AND spt_no = 1;",
    )
    .unwrap();
    let (built, _, _) = build_with(&read(&c).unwrap(), None, &choice(&[lib_coil("wc.coil", 5.0)], &[]));
    let k1 = page(&built, "01").symbols.iter().find(|s| s.id == "s1").unwrap();
    assert_eq!(k1.symbol, "wc.coil");
    // A1 at (0, -5) turned 270 degrees clockwise lands at (-5, 0).
    assert_eq!((k1.rotation, k1.mirror), (Rotation::R270, false));
}

#[test]
fn a_mirrored_block_fits_the_library_symbol_mirrored() {
    let c = sample();
    // A third point X on the coil, 3 mm right of A1: only the mirrored library coil, whose X is
    // 3 mm left of A1, fits.
    c.execute_batch(
        "INSERT INTO tew_symbolpt VALUES (1, 0, 2, 3, 5);
         INSERT INTO tew_componentterminal VALUES (50, 2, 'X');",
    )
    .unwrap();
    let mut coil = lib_coil("my.coil", 5.0);
    coil.pins.push(Pin {
        name: "X".into(),
        at: Point::new(-3.0, -5.0),
        dir: PinDir::Up,
    });
    let (built, _, _) = build_with(&read(&c).unwrap(), None, &choice(&[coil], &[("Coil", "my.coil")]));
    let k1 = page(&built, "01").symbols.iter().find(|s| s.id == "s1").unwrap();
    assert_eq!(k1.symbol, "my.coil");
    assert_eq!((k1.rotation, k1.mirror), (Rotation::R0, true));
}

// ---- Symbol and page graphics from the DWG files (#163) ----

use crate::dwg::objects::tests::{TestEntity as E, model_space_dwg};

/// The library symbol drawn for placed symbol `item` (`s1`, ...) on page `n`.
fn symbol_of<'a>(built: &'a Built, n: &str, item: &str) -> &'a Symbol {
    let placed = page(built, n).symbols.iter().find(|s| s.id == item).unwrap();
    built.symbols.iter().find(|s| s.id == placed.symbol).unwrap()
}

fn has_line(shapes: &[Shape], a: Point, b: Point) -> bool {
    shapes
        .iter()
        .any(|s| matches!(s, Shape::Line { a: p, b: q } if p.distance(a) < 1e-6 && q.distance(b) < 1e-6))
}

#[test]
fn symbols_are_drawn_from_their_dwg_block_and_pages_keep_their_texts() {
    let mut sw = read(&sample()).unwrap();
    // The block name differs in case from the database's `Coil`.
    let coil = model_space_dwg(&[E::Circle((0.0, 0.0), 4.0), E::Line((0.0, 5.0), (0.0, 4.0))]);
    sw.drawings.add_block("COIL", &coil).unwrap();
    // The page: a note, a frame line, one of the wire lines drawn again (reversed) and a block
    // reference (the title block), which WiringCraft draws itself.
    let page_dwg = model_space_dwg(&[
        E::Text((20.0, 30.0), "Check rotation"),
        E::Line((10.0, 10.0), (50.0, 10.0)),
        E::Line((100.0, 224.0), (60.0, 224.0)),
        E::Insert((0.0, 0.0)),
    ]);
    sw.drawings.add_page(10, &page_dwg).unwrap();
    let (built, _, report) = build(&sw, None);

    let k1 = symbol_of(&built, "01", "s1");
    assert!(
        k1.graphics.iter().any(
            |s| matches!(s, Shape::Circle { center, radius, .. } if *center == Point::new(0.0, 0.0) && *radius == 4.0)
        ),
        "{:?}",
        k1.graphics
    );
    assert!(
        has_line(&k1.graphics, Point::new(0.0, -5.0), Point::new(0.0, -4.0)),
        "y flipped: {:?}",
        k1.graphics
    );
    assert!(!k1.graphics.iter().any(|s| matches!(s, Shape::Rect { .. })), "no box");
    // The pins stay where the database puts them.
    let pins: Vec<(&str, Point)> = k1.pins.iter().map(|q| (q.name.as_str(), q.at)).collect();
    assert_eq!(pins, [("A1", Point::new(0.0, -5.0)), ("A2", Point::new(0.0, 5.0))]);
    // The fuse has no drawing: a box.
    let fuse = symbol_of(&built, "01", "s2");
    assert!(matches!(fuse.graphics[..], [Shape::Rect { .. }]), "{:?}", fuse.graphics);

    let pg = page(&built, "01");
    let h = pg.size.mm().1;
    assert!(
        pg.graphics
            .iter()
            .any(|s| matches!(s, Shape::Text { at, text, .. } if text == "Check rotation" && *at == Point::new(20.0, h - 30.0))),
        "{:?}",
        pg.graphics
    );
    assert!(has_line(
        &pg.graphics,
        Point::new(10.0, h - 10.0),
        Point::new(50.0, h - 10.0)
    ));
    assert_eq!(pg.graphics.len(), 2, "no wire line, no title block: {:?}", pg.graphics);

    let c = count(&report, "DWG symbol graphics");
    assert_eq!((c.source, c.imported), (2, 1));
    assert_eq!(
        c.note,
        "1 drawn as boxes: 1 block(s) without a readable drawing in the archive"
    );
    let c = count(&report, "page drawings");
    assert_eq!((c.source, c.imported), (1, 1));
}

#[test]
fn a_turned_and_mirrored_symbol_turns_its_drawing_like_its_points() {
    let mut sw = read(&sample()).unwrap();
    let block = model_space_dwg(&[E::Line((0.0, 0.0), (10.0, 5.0)), E::Arc((0.0, 0.0), 2.0, 0.0, 90.0)]);
    sw.drawings.add_block("Coil", &block).unwrap();
    let k1 = sw.symbols.iter_mut().find(|s| s.id == 1).unwrap();
    (k1.angle, k1.scale) = (90.0, (-1.0, 1.0));
    // The points placed the same way, so the drawing can be checked against `place`.
    let end = place(Point::new(0.0, 0.0), 90.0, (-1.0, 1.0), Point::new(10.0, 5.0));
    assert!(end.distance(Point::new(5.0, 10.0)) < 1e-9, "{end:?}");
    let (built, _, _) = build(&sw, None);
    let g = &symbol_of(&built, "01", "s1").graphics;
    assert!(has_line(g, Point::new(0.0, 0.0), Point::new(5.0, -10.0)), "{g:?}");

    // Mirrored only: the quarter arc from 0 to 90 degrees (y up) runs from 90 to 180, which
    // on the y-down page is -180 to -90.
    let k1 = sw.symbols.iter_mut().find(|s| s.id == 1).unwrap();
    k1.angle = 0.0;
    let (built, _, _) = build(&sw, None);
    let g = &symbol_of(&built, "01", "s1").graphics;
    assert!(
        g.iter().any(|s| matches!(s, Shape::Arc { start, end, radius, .. }
            if (*start + 180.0).abs() < 1e-6 && (*end + 90.0).abs() < 1e-6 && *radius == 2.0)),
        "{g:?}"
    );
}

#[test]
fn an_empty_or_missing_drawing_keeps_the_box() {
    let mut sw = read(&sample()).unwrap();
    sw.drawings.add_block("Coil", &model_space_dwg(&[])).unwrap();
    let (built, _, report) = build(&sw, None);
    assert!(matches!(
        symbol_of(&built, "01", "s1").graphics[..],
        [Shape::Rect { .. }]
    ));
    let c = count(&report, "DWG symbol graphics");
    assert_eq!((c.source, c.imported), (2, 0));
    assert_eq!(
        c.note,
        "2 drawn as boxes: 2 block(s) without a readable drawing in the archive"
    );
    assert!(page(&built, "01").graphics.is_empty());
    assert!(sw.drawings.add_block("Bad", b"AC1024 but nothing else").is_err());
}

#[test]
fn drawings_are_read_from_the_archive_and_the_project_copy_wins() {
    let db = sample().serialize(rusqlite::MAIN_DB).unwrap().to_vec();
    let library = model_space_dwg(&[E::Circle((0.0, 0.0), 3.0)]);
    let own = model_space_dwg(&[E::Line((0.0, 0.0), (0.0, 6.0))]);
    let page_dwg = model_space_dwg(&[E::Text((5.0, 5.0), "Note")]);
    let other_page = model_space_dwg(&[E::Text((5.0, 5.0), "Other project")]);
    let buf = archive(&[
        ("Projects/3/Database/tew_project_data.sqlite", &db),
        ("Projects/3/Block/coil.dwg", &own),
        ("Block/Coil.dwg", &library),
        ("Block/Broken.dwg", b"AC1024 cut short"),
        ("Projects/3/Drawings/10.ewg", &page_dwg),
        ("Projects/4/Drawings/10.ewg", &other_page),
    ]);
    let got = database_from_archive(buf.clone(), 1).unwrap();
    let drawings = drawings_from_archive(buf, &got.folder).unwrap();
    assert_eq!((drawings.block_count(), drawings.page_count()), (1, 1));
    assert_eq!(drawings.warnings.len(), 1, "{:?}", drawings.warnings);
    assert!(
        drawings.warnings[0].contains("Block/Broken.dwg"),
        "{:?}",
        drawings.warnings
    );

    let mut sw = read(&open(&got.bytes).unwrap()).unwrap();
    sw.drawings = drawings;
    let (built, _, _) = build(&sw, None);
    let g = &symbol_of(&built, "01", "s1").graphics;
    assert!(
        has_line(g, Point::new(0.0, 0.0), Point::new(0.0, -6.0)),
        "the project's copy: {g:?}"
    );
    let texts: Vec<&str> = page(&built, "01")
        .graphics
        .iter()
        .filter_map(|s| match s {
            Shape::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["Note"]);
    assert!(drawings_from_archive(std::io::Cursor::new(b"not a zip".to_vec()), "Projects/3").is_err());
}

#[test]
fn a_page_drawing_that_needs_a_bigger_sheet_gets_one() {
    let mut sw = read(&sample()).unwrap();
    // Page 01 has no frame name; its symbols and lines fit A4/A3, but the note at (500, 350)
    // only fits A2 (594 x 420).
    sw.drawings
        .add_page(10, &model_space_dwg(&[E::Text((500.0, 350.0), "Far note")]))
        .unwrap();
    let (built, _, _) = build(&sw, None);
    let pg = page(&built, "01");
    let (w, h) = pg.size.mm();
    assert!(w >= 500.0 && h >= 350.0, "{:?}", pg.size);
    assert!(
        pg.graphics
            .iter()
            .any(|s| matches!(s, Shape::Text { at, .. } if *at == Point::new(500.0, h - 350.0) && at.y >= 0.0)),
        "{:?}",
        pg.graphics
    );
}

#[test]
fn another_projects_block_never_replaces_the_library_one() {
    let db = sample().serialize(rusqlite::MAIN_DB).unwrap().to_vec();
    let library = model_space_dwg(&[E::Circle((0.0, 0.0), 3.0)]);
    let other = model_space_dwg(&[E::Line((0.0, 0.0), (0.0, 6.0))]);
    let buf = archive(&[
        ("Projects/3/Database/tew_project_data.sqlite", &db),
        ("Block/Coil.dwg", &library),
        ("Projects/5/Block/Coil.dwg", &other),
    ]);
    let drawings = drawings_from_archive(buf, "Projects/3").unwrap();
    assert_eq!(drawings.block_count(), 1);
    let mut sw = read(&sample()).unwrap();
    sw.drawings = drawings;
    let (built, _, _) = build(&sw, None);
    let g = &symbol_of(&built, "01", "s1").graphics;
    assert!(matches!(g[..], [Shape::Circle { .. }]), "the library's: {g:?}");
}

#[test]
fn a_symbol_scaled_unevenly_and_turned_is_drawn_like_its_points() {
    let mut sw = read(&sample()).unwrap();
    sw.drawings
        .add_block("Coil", &model_space_dwg(&[E::Line((0.0, 0.0), (10.0, 0.0))]))
        .unwrap();
    let k1 = sw.symbols.iter_mut().find(|s| s.id == 1).unwrap();
    (k1.angle, k1.scale) = (90.0, (2.0, 1.0));
    let end = place(Point::new(0.0, 0.0), 90.0, (2.0, 1.0), Point::new(10.0, 0.0));
    let (built, _, _) = build(&sw, None);
    let g = &symbol_of(&built, "01", "s1").graphics;
    // y flipped on the page.
    assert!(
        has_line(g, Point::new(0.0, 0.0), Point::new(round(end.x), -round(end.y))),
        "{end:?} {g:?}"
    );
}

#[test]
fn the_drawing_limit_is_reported_apart_from_missing_drawings() {
    // Two symbols drawn from blocks, with no budget left once the first page drawing is in:
    // checked through the parts the build uses.
    let mut d = SweDrawings::default();
    d.add_block("Coil", &model_space_dwg(&[E::Line((0.0, 0.0), (1.0, 0.0))]))
        .unwrap();
    let sw = read(&sample()).unwrap();
    let coil = sw.symbols.iter().find(|s| s.id == 1).unwrap();
    let mut skipped = BTreeMap::new();
    let mut budget = 0;
    assert_eq!(d.symbol_graphics(coil, &mut budget, &mut skipped), None);
    assert!(skipped.contains_key(crate::dxf::OVER_LIMIT), "{skipped:?}");
    assert!(d.has_block("COIL"));
}

#[test]
fn a_symbol_drawing_that_needs_a_bigger_sheet_gets_one() {
    let mut sw = read(&sample()).unwrap();
    // The coil's block reaches 250 mm right of and 150 mm above the symbol: past A3 (420 x 297)
    // although its points fit on it.
    sw.drawings
        .add_block("Coil", &model_space_dwg(&[E::Line((0.0, 0.0), (250.0, 150.0))]))
        .unwrap();
    let coil = sw.symbols.iter().find(|s| s.id == 1).unwrap();
    let reach = Point::new(coil.at.x + 250.0, coil.at.y + 150.0);
    assert!(reach.y > 297.0, "{reach:?}");
    let (built, _, _) = build(&sw, None);
    let (w, h) = page(&built, "01").size.mm();
    assert!(w >= reach.x && h >= reach.y, "{w} x {h} for {reach:?}");
}

#[test]
fn reaching_the_drawing_limit_is_reported_by_the_import() {
    let mut sw = read(&sample()).unwrap();
    sw.drawings
        .add_block("Coil", &model_space_dwg(&[E::Line((0.0, 0.0), (1.0, 0.0))]))
        .unwrap();
    sw.drawings.set_draw_limit(0);
    let (built, warnings, report) = build(&sw, None);
    // The coil is boxed, and counted apart from blocks without a drawing.
    let k1 = symbol_of(&built, "01", "s1");
    assert!(matches!(k1.graphics[..], [Shape::Rect { .. }]), "{:?}", k1.graphics);
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("the symbol and page drawings reached the limit of 0 entities")),
        "{warnings:?}"
    );
    let c = count(&report, "DWG symbol graphics");
    assert_eq!((c.source, c.imported), (2, 0));
    assert_eq!(
        c.note,
        "1 drawn as boxes: 1 block(s) without a readable drawing in the archive; 1 drawn as boxes: the drawings' entity limit was reached"
    );
}

#[test]
fn refused_drawing_files_count_against_the_archive_total() {
    let small = model_space_dwg(&[E::Line((0.0, 0.0), (1.0, 0.0))]);
    let file = small.len() as u64;
    let big = vec![0u8; 3 * small.len()];
    // Two files too big to keep, then one that fits: the bytes read of the two refused ones
    // (one past the file limit each) use up the total, so the third is not read.
    let buf = archive(&[("Block/A.dwg", &big), ("Block/B.dwg", &big), ("Block/D.dwg", &small)]);
    let lim = drawings::Limits {
        file,
        total: 2 * (file + 1),
    };
    let drawings = drawings::drawings_within(buf, "Projects/3", lim).unwrap();
    assert!(!drawings.has_block("D"), "{:?}", drawings.warnings);
    assert!(
        drawings
            .warnings
            .iter()
            .any(|w| w.starts_with("2 drawing file(s) could not be read")),
        "{:?}",
        drawings.warnings
    );
    assert!(
        drawings
            .warnings
            .iter()
            .any(|w| w.starts_with("1 drawing file(s) were not read")),
        "{:?}",
        drawings.warnings
    );
}

#[test]
fn local_pins_round_each_point_before_taking_off_the_origin() {
    let mut sw = read(&sample()).unwrap();
    let k1 = sw.symbols.iter_mut().find(|s| s.id == 1).unwrap();
    // Rounding the difference would give 0.500, right on JOIN; rounding each point gives 0.501.
    k1.at = Point::new(100.0004, 50.0);
    k1.pins.truncate(1);
    k1.pins[0].at = Point::new(100.5008, 40.0);
    let local = local_pins(k1);
    assert!((local[0].1.x - 0.501).abs() < 1e-9, "{local:?}");
    assert!((local[0].1.y - 10.0).abs() < 1e-9, "y down: {local:?}");
}

#[test]
fn an_empty_block_drawn_before_the_limit_is_not_counted_as_over_it() {
    let mut sw = read(&sample()).unwrap();
    // The coil's block has nothing to draw; the fuse's reaches the limit after it.
    sw.drawings.add_block("Coil", &model_space_dwg(&[])).unwrap();
    sw.drawings
        .add_block("Fuse", &model_space_dwg(&[E::Line((0.0, 0.0), (1.0, 0.0))]))
        .unwrap();
    sw.drawings.set_draw_limit(0);
    let (_, _, report) = build(&sw, None);
    let c = count(&report, "DWG symbol graphics");
    assert_eq!((c.source, c.imported), (2, 0));
    assert_eq!(
        c.note,
        "1 drawn as boxes: 1 block(s) without a readable drawing in the archive; 1 drawn as boxes: the drawings' entity limit was reached"
    );
}

#[test]
fn a_drawing_read_in_part_keeps_its_graphics_and_says_why() {
    let mut d = SweDrawings::default();
    d.add_block("Coil", &model_space_dwg(&[E::Line((0.0, 0.0), (0.0, 6.0)), E::Damaged]))
        .unwrap();
    assert_eq!(d.warnings.len(), 1, "{:?}", d.warnings);
    let w = &d.warnings[0];
    assert!(
        w.starts_with(
            "block Coil: 1 object(s) could not be read and are not drawn, the first because of: object 101 at"
        ),
        "{w}"
    );
    assert!(w.contains("past the end"), "{w}");
    // What could be read is drawn.
    let sw = read(&sample()).unwrap();
    let coil = sw.symbols.iter().find(|s| s.id == 1).unwrap();
    let g = d
        .symbol_graphics(coil, &mut { crate::dxf::MAX_DRAWN }, &mut BTreeMap::new())
        .unwrap();
    assert!(has_line(&g, Point::new(0.0, 0.0), Point::new(0.0, -6.0)), "{g:?}");
}

#[test]
fn a_symbol_at_the_join_boundary_is_drawn_and_sized_alike_by_both_checks() {
    let mut sw = read(&sample()).unwrap();
    // A2 sits 5.5002 below the origin: 5.500 when the difference is rounded (a library fit,
    // exactly JOIN from the library pin), 5.501 when each point is rounded (no fit).
    let k1 = sw.symbols.iter_mut().find(|s| s.id == 1).unwrap();
    k1.at = Point::new(100.0, 200.0006);
    k1.pins[0].at = Point::new(100.0, 205.0006);
    k1.pins[1].at = Point::new(100.0, 194.5004);
    // Its block reaches past A3 (420 x 297).
    sw.drawings
        .add_block("Coil", &model_space_dwg(&[E::Line((0.0, 0.0), (250.0, 150.0))]))
        .unwrap();
    let (built, _, _) = build_with(&sw, None, &choice(&[lib_coil("wc.coil", 5.0)], &[]));
    let k1 = page(&built, "01").symbols.iter().find(|s| s.id == "s1").unwrap();
    assert_ne!(k1.symbol, "wc.coil", "no library fit at 5.501");
    let drawn = built.symbols.iter().find(|s| s.id == k1.symbol).unwrap();
    assert!(
        !drawn.graphics.iter().any(|s| matches!(s, Shape::Rect { .. })),
        "drawn from its block: {:?}",
        drawn.graphics
    );
    let (w, h) = page(&built, "01").size.mm();
    assert!(w >= 350.0 && h >= 350.0, "the sheet has room for the block: {w} x {h}");
}

#[test]
fn jumpers_from_the_terminal_strip_editor_become_the_strips_jumper() {
    let c = sample_with_strip();
    // A third terminal, and two records no line carries: 1 to 2 and 2 to 3, chained.
    c.execute_batch(
        "INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type, com_com_id, com_tagorderno) VALUES (203, '3', '-X1 3', 4, 200, 3);
         INSERT INTO tew_symbol VALUES (22, 10, 203, 'Terminal', 190, 200, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (22, 0, 0, 0, 5), (22, 0, 1, 0, -5);
         INSERT INTO tew_componentelement VALUES (62, 22, 0, 203, 0);
         INSERT INTO tew_componentterminal VALUES (62, 0, NULL), (62, 1, NULL);
         INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag, wir_cel_idfrom, wir_cte_nofrom, wir_cel_idto, wir_cte_noto)
           VALUES (90, NULL, -1, '', 60, 0, 61, 1), (92, NULL, -1, '', 61, 0, 62, 1);",
    )
    .unwrap();
    let (built, _, report) = build(&read(&c).unwrap(), None);
    assert_eq!(built.jumpers.len(), 1, "{:?}", built.jumpers);
    let j = &built.jumpers[0];
    assert_eq!(j.strip, DeviceTag::parse("-X1").unwrap());
    let mut terminals = j.terminals.clone();
    terminals.sort();
    assert_eq!(terminals, ["1", "2", "3"]);
    let jumpers = report.iter().find(|c| c.what == "terminal jumpers").unwrap();
    assert_eq!((jumpers.source, jumpers.imported), (1, 1));
    assert!(jumpers.note.contains("2 wire record(s)"), "{}", jumpers.note);
    let ends = report
        .iter()
        .find(|c| c.what == "wire ends (from/to terminals)")
        .unwrap();
    assert!(!ends.note.contains("WiringCraft has no jumpers"), "{}", ends.note);
}

#[test]
fn a_jumper_counts_even_where_the_drawing_already_joins_its_terminals() {
    let c = sample_with_strip();
    // A drawn line from terminal 1's upper pin to terminal 2's lower pin, not linked to the
    // record 90 between the same two pins.
    c.execute_batch(
        "INSERT INTO tew_line VALUES (12, 10, 150, 205, 170, 195, NULL);
         INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag, wir_cel_idfrom, wir_cte_nofrom, wir_cel_idto, wir_cte_noto)
           VALUES (90, NULL, -1, '', 60, 0, 61, 1);",
    )
    .unwrap();
    let (built, _, _) = build(&read(&c).unwrap(), None);
    assert_eq!(built.jumpers.len(), 1, "{:?}", built.jumpers);
    let mut terminals = built.jumpers[0].terminals.clone();
    terminals.sort();
    assert_eq!(terminals, ["1", "2"]);
}

#[test]
fn a_jumper_to_a_terminal_whose_tag_was_taken_names_both_tags() {
    let c = sample_with_strip();
    // Another device already has -X1.2, so terminal 2 is renamed.
    c.execute_batch(
        "INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type) VALUES (205, 'X1.2', '-X1.2', -1);
         INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag, wir_cel_idfrom, wir_cte_nofrom, wir_cel_idto, wir_cte_noto)
           VALUES (90, NULL, -1, '', 60, 0, 61, 1);",
    )
    .unwrap();
    let (built, warnings, _) = build(&read(&c).unwrap(), None);
    assert!(built.jumpers.is_empty(), "{:?}", built.jumpers);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("the tag -X1.2 was taken by another device")
                && w.contains("so the terminal became -X1.2_2")),
        "{warnings:?}"
    );
}

#[test]
fn a_jumper_to_a_terminal_not_drawn_is_refused_with_its_reason() {
    let c = sample_with_strip();
    // Terminal 3 on the strip has no symbol: its circuit is in no drawing.
    c.execute_batch(
        "INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type, com_com_id, com_tagorderno) VALUES (203, '3', '-X1 3', 4, 200, 3);
         INSERT INTO tew_componentelement VALUES (62, NULL, 0, 203, 0);
         INSERT INTO tew_componentterminal VALUES (62, 0, NULL), (62, 1, NULL);
         INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag, wir_cel_idfrom, wir_cte_nofrom, wir_cel_idto, wir_cte_noto)
           VALUES (90, NULL, -1, '', 61, 0, 62, 1);",
    )
    .unwrap();
    let (built, warnings, _) = build(&read(&c).unwrap(), None);
    // Its number would join whichever terminal later gets it, and no plan would show it.
    assert!(built.jumpers.is_empty(), "{:?}", built.jumpers);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("1 jumper record(s)") && w.contains("a terminal is not drawn on any schematic page")),
        "{warnings:?}"
    );
}

#[test]
fn a_jumper_to_a_terminal_without_a_number_is_refused_with_its_reason() {
    let c = sample_with_strip();
    // A terminal on the strip with no name: its tag path `-X1.9` must not stand in for a number.
    c.execute_batch(
        "INSERT INTO tew_component (com_id, com_tag, com_tagpath, com_type, com_com_id, com_tagorderno) VALUES (204, '', '-X1.9', 4, 200, 4);
         INSERT INTO tew_symbol VALUES (23, 10, 204, 'Terminal', 210, 200, 0, 1, 1, NULL, NULL);
         INSERT INTO tew_symbolpt VALUES (23, 0, 0, 0, 5), (23, 0, 1, 0, -5);
         INSERT INTO tew_componentelement VALUES (63, 23, 0, 204, 0);
         INSERT INTO tew_componentterminal VALUES (63, 0, NULL), (63, 1, NULL);
         INSERT INTO tew_wire (wir_id, wir_cab_id, wir_caw_no, wir_tag, wir_cel_idfrom, wir_cte_nofrom, wir_cel_idto, wir_cte_noto)
           VALUES (93, NULL, -1, '', 60, 0, 63, 1);",
    )
    .unwrap();
    let (built, warnings, report) = build(&read(&c).unwrap(), None);
    assert!(built.jumpers.is_empty(), "{:?}", built.jumpers);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("1 jumper record(s)") && w.contains("a terminal has no number of its own")),
        "{warnings:?}"
    );
    let ends = report
        .iter()
        .find(|c| c.what == "wire ends (from/to terminals)")
        .unwrap();
    assert!(
        ends.note
            .contains("1 are jumpers between terminals of one strip that no drawn line carries, not imported"),
        "{}",
        ends.note
    );
}

#[test]
fn archive_names_with_dot_slash_are_read_like_plain_ones() {
    let library = model_space_dwg(&[E::Line((0.0, 0.0), (1.0, 0.0))]);
    let other = model_space_dwg(&[E::Line((0.0, 0.0), (9.0, 9.0))]);
    let page = model_space_dwg(&[E::Line((0.0, 0.0), (5.0, 0.0))]);
    // Another project's block comes after the library's: it must not replace it.
    let buf = archive(&[
        ("Block/Coil.dwg", &library),
        ("./Projects/2/Block/Coil.dwg", &other),
        ("./Projects/4/Drawings/17.ewg", &page),
    ]);
    let lim = drawings::Limits {
        file: 1 << 20,
        total: 1 << 24,
    };
    let drawings = drawings::drawings_within(buf, "Projects/4", lim).unwrap();
    assert!(drawings.warnings.is_empty(), "{:?}", drawings.warnings);
    let (mut budget, mut skipped) = (usize::MAX, BTreeMap::new());
    let coil = drawings.block_graphics("Coil", &mut budget, &mut skipped).unwrap();
    assert!(
        matches!(coil[..], [Shape::Line { b, .. }] if (b.x - 1.0).abs() < 1e-9 && b.y.abs() < 1e-9),
        "{coil:?}"
    );
    assert_eq!(drawings.page_count(), 1);
    assert!(!drawings.page_graphics(17, &[], &mut budget, &mut skipped).is_empty());
}

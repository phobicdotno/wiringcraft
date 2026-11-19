//! Tests on small library databases built here. The table and column names follow the
//! SolidWorks naming pattern the module docs describe; no real catalog files are used.
#![allow(clippy::unwrap_used)]

use super::*;
use wc_core::{Device, DeviceTag};

const CATALOG: &str = "
CREATE TABLE tew_catreference (ref_id INTEGER, ref_reference TEXT, ref_manufacturer TEXT, ref_description TEXT,
  ref_typedesignation TEXT, ref_width TEXT, ref_deleted INTEGER);
CREATE TABLE tew_catelement (ele_id INTEGER, ele_ref_id INTEGER, ele_orderno INTEGER, ele_name TEXT);
CREATE TABLE tew_catterminal (ter_ele_id INTEGER, ter_no INTEGER, ter_txt TEXT);
INSERT INTO tew_catreference VALUES
  (1, 'RLY-24', 'Maker A', 'Relay 24 V DC', 'R24', '18 mm', 0),
  (2, 'MCB C10', 'Maker B', NULL, NULL, NULL, 0),
  (3, '', 'Maker C', 'no reference', NULL, NULL, 0),
  (4, 'RLY-24', 'Maker A', 'same again', NULL, NULL, 0);
-- RLY-24: coil A1 A2, then a change-over contact 11 12 14 (listed out of order).
INSERT INTO tew_catelement VALUES (10, 1, 1, 'Coil'), (11, 1, 2, 'CO'), (20, 2, 1, '');
INSERT INTO tew_catterminal VALUES (10, 1, 'A1'), (10, 2, 'A2'), (11, 3, '14'), (11, 1, '11'), (11, 2, '12'),
  (20, 1, '1'), (20, 2, '2'), (99, 1, 'X');
";

const APP_DATA: &str = "
CREATE TABLE tew_cablereference (cbr_id INTEGER, cbr_reference TEXT, cbr_manufacturer TEXT, cbr_description TEXT);
CREATE TABLE tew_cablereferencewire (crw_cbr_id INTEGER, crw_no TEXT, crw_color TEXT, crw_section TEXT);
CREATE TABLE tew_block (blk_id INTEGER, blk_name TEXT, blk_description TEXT);
CREATE TABLE tew_blockpt (bpt_blk_id INTEGER, bpt_no INTEGER, bpt_txt TEXT, bpt_dx REAL, bpt_dy REAL);
INSERT INTO tew_cablereference VALUES (1, 'CBL-3G1', 'Maker C', 'Control cable 3G1'), (2, 'CBL-2', 'Maker C', NULL);
INSERT INTO tew_cablereferencewire VALUES (1, '1', 'BN', '1 mm2'), (1, '2', 'BU', '1 mm2'), (1, '3', 'GNYE', '1 mm2'),
  (2, '1', '', ''), (2, '2', '', '');
-- A coil block: A1 5 mm above the insertion point, A2 5 mm below (y up); a frame without points.
INSERT INTO tew_block VALUES (1, 'Coil', 'Relay coil'), (2, 'Frame A3', ''), (3, 'Coil', 'Another coil');
INSERT INTO tew_blockpt VALUES (1, 2, 'A2', 0, -5), (1, 1, 'A1', 0, 5), (3, 1, '', 0, 0);
";

fn db(schema: &str) -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(schema).unwrap();
    c
}

#[test]
fn catalog_parts_carry_their_terminals_and_extra_fields() {
    let (mut report, mut warnings) = (LibraryReport::default(), Vec::new());
    let parts = read_catalog(&db(CATALOG), &mut report, &mut warnings).unwrap();
    let ids: Vec<&str> = parts.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["maker-a.rly-24", "maker-b.mcb-c10"]);
    let relay = &parts[0];
    assert_eq!(
        (
            relay.manufacturer.as_str(),
            relay.order_number.as_str(),
            relay.description.as_str()
        ),
        ("Maker A", "RLY-24", "Relay 24 V DC")
    );
    assert_eq!(relay.properties[PROP_TERMINALS], "Coil: A1 A2; CO: 11 12 14");
    assert_eq!(relay.properties["Type designation"], "R24");
    assert_eq!(relay.properties["Width"], "18 mm");
    // A circuit without a name lists its marks only; empty fields are left out.
    assert_eq!(parts[1].properties[PROP_TERMINALS], "1 2");
    assert!(!parts[1].properties.contains_key("Width"));
    assert_eq!(
        report,
        LibraryReport {
            parts_in: 4,
            parts_out: 2,
            terminals: 7,
            ..Default::default()
        }
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("1 catalog part(s) without a reference"))
    );
    assert!(warnings.iter().any(|w| w.contains("1 catalog part(s) repeat")));
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("1 catalog terminal(s) belong to no circuit"))
    );
}

#[test]
fn cable_types_become_parts_with_their_cores() {
    let (mut report, mut warnings) = (LibraryReport::default(), Vec::new());
    let cables = read_cable_types(&db(APP_DATA), &mut report, &mut warnings).unwrap();
    assert_eq!(cables.len(), 2);
    let c = &cables[0];
    assert_eq!(c.id, "maker-c.cbl-2");
    assert_eq!(c.description, "Cable");
    assert_eq!(c.properties[PROP_CORES], "2");
    assert!(!c.properties.contains_key(PROP_CORE_COLOURS));
    let c = &cables[1];
    assert_eq!(c.id, "maker-c.cbl-3g1");
    assert_eq!(c.properties[PROP_KIND], "Cable");
    assert_eq!(c.properties[PROP_CORES], "3");
    assert_eq!(c.properties[PROP_CORE_COLOURS], "1 BN, 2 BU, 3 GNYE");
    assert_eq!(c.properties["Core cross-section"], "1 mm2");
    assert_eq!((report.cables_in, report.cables_out), (2, 2));
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn blocks_become_box_symbols_with_their_points_as_pins() {
    let (mut report, mut warnings) = (LibraryReport::default(), Vec::new());
    let symbols = read_blocks(&db(APP_DATA), None, &mut report, &mut warnings).unwrap();
    let ids: Vec<&str> = symbols.iter().map(|s| s.id.as_str()).collect();
    // Two blocks named `Coil` get distinct ids; the frame has no points and is left out.
    assert_eq!(ids, ["swe.lib.coil", "swe.lib.coil.2"]);
    let coil = &symbols[0];
    assert_eq!(coil.description, "Relay coil");
    // Points in their number order, y flipped to WiringCraft's y down.
    let pins: Vec<(&str, f64, f64)> = coil.pins.iter().map(|p| (p.name.as_str(), p.at.x, p.at.y)).collect();
    assert_eq!(pins, [("A1", 0.0, -5.0), ("A2", 0.0, 5.0)]);
    // A point without a name is named by its number.
    assert_eq!(symbols[1].pins[0].name, "1");
    assert_eq!((report.blocks_in, report.blocks_out), (3, 2));
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("1 symbol block(s) without connection points"))
    );
}

#[test]
fn other_prefixes_are_found_and_missing_fields_are_warnings() {
    // Different prefixes, `partnumber` instead of `reference`, no terminal tables.
    let c = db(
        "CREATE TABLE tew_catreference (cr_id INTEGER, cr_partnumber TEXT, cr_manufacturername TEXT);
         INSERT INTO tew_catreference VALUES (1, 'X-1', 'M');",
    );
    let (mut report, mut warnings) = (LibraryReport::default(), Vec::new());
    let parts = read_catalog(&c, &mut report, &mut warnings).unwrap();
    assert_eq!(parts[0].id, "m.x-1");
    assert!(warnings.iter().any(|w| w.contains("come without terminals")));

    // No manufacturer column: nothing read, said why, no error.
    let c = db("CREATE TABLE tew_catreference (cr_id INTEGER, cr_reference TEXT);");
    let mut warnings = Vec::new();
    let parts = read_catalog(&c, &mut report, &mut warnings).unwrap();
    assert!(parts.is_empty());
    assert!(
        warnings.iter().any(|w| w.contains("has no cr_manufacturer column")),
        "{warnings:?}"
    );

    // No tables at all.
    let c = db("CREATE TABLE other (x INTEGER);");
    let lib = read(Some(&c), Some(&c)).unwrap();
    assert!(lib.parts.is_empty() && lib.symbols.is_empty());
    assert_eq!(lib.warnings.len(), 3, "{:?}", lib.warnings);
}

#[test]
fn rows_of_the_wrong_type_are_counted_not_fatal() {
    let c = db(CATALOG);
    c.execute_batch("INSERT INTO tew_catreference VALUES ('not a number', 'Y', 'M', '', '', '', 0);")
        .unwrap();
    let (mut report, mut warnings) = (LibraryReport::default(), Vec::new());
    let parts = read_catalog(&c, &mut report, &mut warnings).unwrap();
    assert_eq!(parts.len(), 2);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("catalog part row(s) could not be read")),
        "{warnings:?}"
    );
    // The unreadable row still counts as a row of the source.
    assert_eq!(report.parts_in, 5);

    let c = db(APP_DATA);
    c.execute_batch("INSERT INTO tew_blockpt VALUES (1, 3, 'B', 'far', 0);")
        .unwrap();
    c.execute_batch("INSERT INTO tew_block VALUES ('bad id', 'Odd', '');")
        .unwrap();
    let mut warnings = Vec::new();
    let symbols = read_blocks(&c, None, &mut report, &mut warnings).unwrap();
    assert_eq!(report.blocks_in, 4);
    assert_eq!(symbols[0].pins.len(), 2);
    assert!(!warnings.is_empty());
}

fn archive(entries: &[(&str, &[u8])]) -> std::io::Cursor<Vec<u8>> {
    use std::io::Write;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opt = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            z.start_file(*name, opt).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }
    buf.set_position(0);
    buf
}

fn bytes(c: &Connection) -> Vec<u8> {
    c.serialize(rusqlite::MAIN_DB).unwrap().to_vec()
}

#[test]
fn an_archive_gives_parts_cables_and_symbols() {
    let (cat, app) = (bytes(&db(CATALOG)), bytes(&db(APP_DATA)));
    let empty = bytes(&db("CREATE TABLE tew_catreference (ref_id INTEGER);"));
    let buf = archive(&[
        // A project's own copy is used only when there is no shared one.
        ("Projects/2/Database/tew_catalog.sqlite", &empty),
        ("Database/tew_catalog.sqlite", &cat),
        ("Database/tew_app_data.sqlite", &app),
    ]);
    let lib = from_archive(buf).unwrap();
    let ids: Vec<&str> = lib.parts.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids,
        ["maker-a.rly-24", "maker-b.mcb-c10", "maker-c.cbl-2", "maker-c.cbl-3g1"]
    );
    assert_eq!(lib.symbols.len(), 2);
    assert_eq!(lib.report.lines().len(), 3);
    assert!(lib.report.lines()[0].contains("4 in the catalog, 2 imported"));

    // Only the catalog: parts, and a warning for the rest.
    let lib = from_archive(archive(&[("Database/tew_catalog.sqlite", &cat)])).unwrap();
    assert_eq!(lib.parts.len(), 2);
    assert!(lib.warnings.iter().any(|w| w.contains("no tew_app_data.sqlite")));

    // Neither database, or not a zip at all: errors.
    assert!(from_archive(archive(&[("Archive.ini", b"x")])).is_err());
    assert!(from_archive(std::io::Cursor::new(b"not a zip".to_vec())).is_err());
    // A database entry that is not SQLite is an error, not a panic.
    assert!(from_archive(archive(&[("Database/tew_catalog.sqlite", b"garbage")])).is_err());
}

#[test]
fn devices_and_cables_get_the_library_part_they_name() {
    let (mut report, mut warnings) = (LibraryReport::default(), Vec::new());
    let mut parts = read_catalog(&db(CATALOG), &mut report, &mut warnings).unwrap();
    parts.extend(read_cable_types(&db(APP_DATA), &mut report, &mut warnings).unwrap());
    let device = |pn: &str, maker: &str| Device {
        part: None,
        accessories: Vec::new(),
        description: String::new(),
        properties: BTreeMap::from([
            ("PartNumber".to_owned(), pn.to_owned()),
            ("Manufacturer".to_owned(), maker.to_owned()),
        ]),
    };
    let mut built = Built::default();
    built
        .devices
        .insert(DeviceTag::parse("-K1").unwrap(), device("RLY-24", "Maker A"));
    built
        .devices
        .insert(DeviceTag::parse("-K2").unwrap(), device("RLY-24", "Maker A"));
    built
        .devices
        .insert(DeviceTag::parse("-W1").unwrap(), device("CBL-3G1", "Maker C"));
    built
        .devices
        .insert(DeviceTag::parse("-F1").unwrap(), device("UNKNOWN", "Maker A"));
    built
        .devices
        .insert(DeviceTag::parse("-X1").unwrap(), Device::default());
    // `RLY/24` gives the same id as `RLY-24` but is another part; case and spaces do not count.
    built
        .devices
        .insert(DeviceTag::parse("-K3").unwrap(), device("RLY/24", "Maker A"));
    built
        .devices
        .insert(DeviceTag::parse("-K4").unwrap(), device(" rly-24", "MAKER A "));
    assert_eq!(link_parts(&mut built, &parts), 4);
    let part = |t: &str| built.devices[&DeviceTag::parse(t).unwrap()].part.clone();
    assert_eq!(part("-K1").as_deref(), Some("maker-a.rly-24"));
    assert_eq!(part("-K2").as_deref(), Some("maker-a.rly-24"));
    assert_eq!(part("-W1").as_deref(), Some("maker-c.cbl-3g1"));
    assert_eq!(part("-F1"), None);
    assert_eq!(part("-K3"), None);
    assert_eq!(part("-K4").as_deref(), Some("maker-a.rly-24"));
    // The text properties stay; each used part is added once.
    assert_eq!(
        built.devices[&DeviceTag::parse("-K1").unwrap()].properties["PartNumber"],
        "RLY-24"
    );
    let added: Vec<&str> = built.parts.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(added, ["maker-a.rly-24", "maker-c.cbl-3g1"]);

    // Further parts become accessories: the same part twice counts 2, an unknown one is skipped
    // (its text properties stay), and a gap ends the list.
    let mut d = device("RLY-24", "Maker A");
    for (n, pn, maker) in [
        (2, "CBL-3G1", "Maker C"),
        (3, "NOPE", "Maker A"),
        (4, "cbl-3g1", "maker c"),
    ] {
        d.properties.insert(format!("PartNumber {n}"), pn.into());
        d.properties.insert(format!("Manufacturer {n}"), maker.into());
    }
    d.properties.insert("PartNumber 6".into(), "RLY-24".into());
    let mut built = Built::default();
    built.devices.insert(DeviceTag::parse("-K9").unwrap(), d);
    assert_eq!(link_parts(&mut built, &parts), 1);
    let k9 = &built.devices[&DeviceTag::parse("-K9").unwrap()];
    assert_eq!(
        k9.accessories,
        [wc_core::DevicePart {
            part: "maker-c.cbl-3g1".into(),
            qty: 2
        }]
    );
    assert_eq!(k9.all_parts(), [("maker-a.rly-24", 1), ("maker-c.cbl-3g1", 2)]);
    let added: Vec<&str> = built.parts.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(added, ["maker-a.rly-24", "maker-c.cbl-3g1"]);
}

#[test]
fn an_imported_project_links_its_devices_to_the_archive_catalog() {
    // The project sample has relay K1 with part RLY-24 by Maker A, and cable W1 with CBL-3G1 by
    // Maker C: both are in the library databases above.
    let sw = super::super::read(&super::super::tests::sample()).unwrap();
    let (mut built, _, _) = super::super::build(&sw, None);
    let lib = read(Some(&db(CATALOG)), Some(&db(APP_DATA))).unwrap();
    assert_eq!(link_parts(&mut built, &lib.parts), 2);
    let k1 = built
        .devices
        .iter()
        .find(|(t, _)| t.to_string().ends_with("-K1"))
        .unwrap()
        .1;
    assert_eq!(k1.part.as_deref(), Some("maker-a.rly-24"));
    let w1 = built
        .devices
        .iter()
        .find(|(t, _)| t.to_string().ends_with("-W1"))
        .unwrap()
        .1;
    assert_eq!(w1.part.as_deref(), Some("maker-c.cbl-3g1"));
    assert!(built.parts.iter().any(|p| p.id == "maker-a.rly-24"));
}

#[test]
fn library_blocks_are_drawn_from_their_dwg_files_in_the_archive() {
    use crate::dwg::objects::tests::{TestEntity as E, model_space_dwg};
    use wc_core::geom::Shape;
    // A coil with a drawing, a lamp whose only drawing is another project's own copy, and a
    // block whose drawing is damaged.
    let app = bytes(&db("
CREATE TABLE tew_block (blk_id INTEGER, blk_name TEXT, blk_description TEXT);
CREATE TABLE tew_blockpt (bpt_blk_id INTEGER, bpt_no INTEGER, bpt_txt TEXT, bpt_dx REAL, bpt_dy REAL);
INSERT INTO tew_block VALUES (1, 'Coil', ''), (2, 'Lamp', ''), (3, 'Bad', '');
INSERT INTO tew_blockpt VALUES (1, 1, 'A1', 0, 5), (1, 2, 'A2', 0, -5), (2, 1, 'X1', 0, 5), (3, 1, '1', 0, 0);
"));
    // y up in the drawing: a circle and a line from the circle up to A1.
    let coil = model_space_dwg(&[E::Circle((0.0, 0.0), 4.0), E::Line((0.0, 4.0), (0.0, 5.0))]);
    let lamp = model_space_dwg(&[E::Circle((0.0, 0.0), 3.0)]);
    let buf = archive(&[
        ("Database/tew_app_data.sqlite", &app),
        ("Library/Block/coil.DWG", &coil),
        ("Projects/2/Block/Lamp.dwg", &lamp),
        ("Library/Block/Bad.dwg", b"AC1024 but nothing else"),
    ]);
    let lib = from_archive(buf).unwrap();
    let ids: Vec<&str> = lib.symbols.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["swe.lib.bad", "swe.lib.coil", "swe.lib.lamp"]);

    // The coil carries its drawing, y flipped, and the same pins as a box would.
    let coil = &lib.symbols[1];
    assert!(
        coil.graphics
            .iter()
            .any(|g| matches!(g, Shape::Circle { center, radius, .. } if *radius == 4.0 && center.x == 0.0 && center.y == 0.0)),
        "{:?}",
        coil.graphics
    );
    assert!(
        coil.graphics.iter().any(|g| matches!(g, Shape::Line { a, b }
            if (a.y, b.y) == (-4.0, -5.0) && a.x == 0.0 && b.x == 0.0)),
        "{:?}",
        coil.graphics
    );
    assert!(!coil.graphics.iter().any(|g| matches!(g, Shape::Rect { .. })));
    let pins: Vec<(&str, f64, f64)> = coil.pins.iter().map(|p| (p.name.as_str(), p.at.x, p.at.y)).collect();
    assert_eq!(pins, [("A1", 0.0, -5.0), ("A2", 0.0, 5.0)]);

    // Another project's copy is not the library's, and a damaged drawing leaves a box.
    for s in [&lib.symbols[0], &lib.symbols[2]] {
        assert!(
            matches!(s.graphics[..], [Shape::Rect { .. }]),
            "{}: {:?}",
            s.id,
            s.graphics
        );
    }
    assert_eq!((lib.report.blocks_out, lib.report.blocks_drawn), (3, 1));
    assert!(
        lib.report.lines()[2]
            .contains("3 imported with their connection points: 1 drawn from their DWG file, 2 as boxes"),
        "{:?}",
        lib.report.lines()
    );
    assert!(
        lib.warnings
            .iter()
            .any(|w| w.starts_with("block drawings:") && w.contains("Bad.dwg")),
        "{:?}",
        lib.warnings
    );

    // Without drawings every block is a box, and the archive is never read again for them.
    let rewinds = std::rc::Rc::new(std::cell::Cell::new(0));
    let lib = catalog_from_archive(Rewinds {
        inner: archive(&[
            ("readme.txt", b"first, so no entry starts at 0"),
            ("Database/tew_app_data.sqlite", &app),
            (
                "Library/Block/coil.DWG",
                &model_space_dwg(&[E::Circle((0.0, 0.0), 4.0)]),
            ),
        ]),
        count: rewinds.clone(),
        fail: false,
    })
    .unwrap();
    assert_eq!(rewinds.get(), 1, "the catalog pass went back for the drawings");
    assert!(
        !lib.warnings.iter().any(|w| w.contains("block drawings")),
        "{:?}",
        lib.warnings
    );
    let ids: Vec<&str> = lib.symbols.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["swe.lib.bad", "swe.lib.coil", "swe.lib.lamp"]);
    assert_eq!(lib.report.blocks_drawn, 0);
    assert!(
        lib.symbols
            .iter()
            .all(|s| matches!(s.graphics[..], [Shape::Rect { .. }]))
    );
}

#[test]
fn block_drawing_limits_and_unread_entities_are_reported() {
    use crate::dwg::objects::tests::{TestEntity as E, model_space_dwg};
    let app = db("
CREATE TABLE tew_block (blk_id INTEGER, blk_name TEXT, blk_description TEXT);
CREATE TABLE tew_blockpt (bpt_blk_id INTEGER, bpt_no INTEGER, bpt_txt TEXT, bpt_dx REAL, bpt_dy REAL);
INSERT INTO tew_block VALUES (1, 'Coil', ''), (2, 'Lamp', '');
INSERT INTO tew_blockpt VALUES (1, 1, 'A1', 0, 5), (2, 1, 'X1', 0, 5);
");
    let mut d = SweDrawings::default();
    d.add_block(
        "Coil",
        &model_space_dwg(&[E::Line((0.0, 0.0), (0.0, 5.0)), E::Line((1.0, 0.0), (1.0, 5.0))]),
    )
    .unwrap();
    d.add_block("Lamp", &model_space_dwg(&[E::Circle((0.0, 0.0), 3.0)]))
        .unwrap();
    // One entity left: the coil is drawn in part and the lamp is a box.
    d.set_draw_limit(1);
    let (mut report, mut warnings) = (LibraryReport::default(), Vec::new());
    let symbols = read_blocks(&app, Some(&d), &mut report, &mut warnings).unwrap();
    assert_eq!(symbols[0].graphics.len(), 1, "{:?}", symbols[0].graphics);
    assert_eq!((report.blocks_out, report.blocks_drawn), (2, 1));
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("1 symbol block(s) are drawn only in part")),
        "{warnings:?}"
    );
    assert!(
        warnings.iter().any(|w| w.starts_with("1 symbol block(s) are boxes")),
        "{warnings:?}"
    );

    // Everything fits: no limit warning.
    d.set_draw_limit(100);
    let mut warnings = Vec::new();
    read_blocks(&app, Some(&d), &mut report, &mut warnings).unwrap();
    assert_eq!(report.blocks_drawn, 2);
    assert!(!warnings.iter().any(|w| w.contains("entity limit")), "{warnings:?}");

    // A report used again for blocks that give nothing counts nothing drawn, and never underflows.
    let empty = db("
CREATE TABLE tew_block (blk_id INTEGER, blk_name TEXT);
CREATE TABLE tew_blockpt (bpt_blk_id INTEGER, bpt_no INTEGER, bpt_txt TEXT, bpt_dx REAL, bpt_dy REAL);
");
    read_blocks(&empty, Some(&d), &mut report, &mut Vec::new()).unwrap();
    assert_eq!((report.blocks_out, report.blocks_drawn), (0, 0));
    assert!(report.lines()[2].contains("0 drawn from their DWG file, 0 as boxes"));
    // Without the block tables, counts a report holds from an earlier import are cleared too.
    (report.blocks_in, report.blocks_out, report.blocks_drawn) = (2, 2, 2);
    read_blocks(
        &db("CREATE TABLE x (a INTEGER);"),
        Some(&d),
        &mut report,
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!((report.blocks_in, report.blocks_out, report.blocks_drawn), (0, 0, 0));

    // Entities the DWG reader skipped are named; damaged objects are not repeated.
    d.skipped.insert("SPLINE".into(), 3);
    d.skipped.insert(crate::dwg::objects::DAMAGED.into(), 1);
    let w = read_warnings(&d);
    assert!(
        w.iter().any(|w| w == "block drawings: entities not read (SPLINE: 3)"),
        "{w:?}"
    );
    assert!(!w.iter().any(|w| w.contains("damaged")), "{w:?}");

    // A file name with a line break or a terminal escape is printed escaped.
    d.warnings.push("Block/a\nwarning: fake\u{1b}[2J.dwg: bad".into());
    let w = read_warnings(&d);
    let named = w.iter().find(|w| w.contains("fake")).unwrap();
    assert!(!named.chars().any(char::is_control), "{named:?}");
    assert!(named.contains("a\\u{a}warning: fake\\u{1b}[2J.dwg"), "{named:?}");
}

/// An archive reader that counts seeks to its start and can fail every one after the first: the
/// zip reader goes to the start once when it opens the archive, the drawing pass again after.
struct Rewinds {
    inner: std::io::Cursor<Vec<u8>>,
    count: std::rc::Rc<std::cell::Cell<usize>>,
    fail: bool,
}

impl std::io::Read for Rewinds {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl std::io::Seek for Rewinds {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        if to == std::io::SeekFrom::Start(0) {
            self.count.set(self.count.get() + 1);
            if self.fail && self.count.get() > 1 {
                return Err(std::io::Error::other("rewind refused"));
            }
        }
        self.inner.seek(to)
    }
}

#[test]
fn a_failed_rewind_for_the_drawings_keeps_the_library_with_boxes() {
    use crate::dwg::objects::tests::{TestEntity as E, model_space_dwg};
    use wc_core::geom::Shape;
    let app = bytes(&db("
CREATE TABLE tew_block (blk_id INTEGER, blk_name TEXT, blk_description TEXT);
CREATE TABLE tew_blockpt (bpt_blk_id INTEGER, bpt_no INTEGER, bpt_txt TEXT, bpt_dx REAL, bpt_dy REAL);
INSERT INTO tew_block VALUES (1, 'Coil', '');
INSERT INTO tew_blockpt VALUES (1, 1, 'A1', 0, 5);
"));
    let entries = [
        ("readme.txt", &b"first, so no entry starts at 0"[..]),
        ("Database/tew_app_data.sqlite", &app),
        (
            "Library/Block/Coil.dwg",
            &model_space_dwg(&[E::Circle((0.0, 0.0), 4.0)]),
        ),
    ];
    let count = std::rc::Rc::new(std::cell::Cell::new(0));
    let reader = |fail| Rewinds {
        inner: archive(&entries),
        count: count.clone(),
        fail,
    };

    // The rewind works: the coil is drawn.
    let lib = from_archive(reader(false)).unwrap();
    assert!(count.get() > 1, "the drawing pass did not rewind");
    count.set(0);
    assert_eq!(lib.report.blocks_drawn, 1);

    // It fails: the database's blocks are still there, as boxes, and the warning says why.
    let lib = from_archive(reader(true)).unwrap();
    assert_eq!(lib.symbols.len(), 1);
    assert_eq!(lib.symbols[0].pins.len(), 1);
    assert!(matches!(lib.symbols[0].graphics[..], [Shape::Rect { .. }]));
    assert!(
        lib.warnings
            .iter()
            .any(|w| w.starts_with("block drawings were not read") && w.contains("rewind refused")),
        "{:?}",
        lib.warnings
    );
}

#[test]
fn a_drawing_whose_file_name_has_spaces_draws_its_block() {
    use crate::dwg::objects::tests::{TestEntity as E, model_space_dwg};
    let app = db("
CREATE TABLE tew_block (blk_id INTEGER, blk_name TEXT, blk_description TEXT);
CREATE TABLE tew_blockpt (bpt_blk_id INTEGER, bpt_no INTEGER, bpt_txt TEXT, bpt_dx REAL, bpt_dy REAL);
INSERT INTO tew_block VALUES (1, ' Coil ', '');
INSERT INTO tew_blockpt VALUES (1, 1, 'A1', 0, 5);
");
    let mut d = SweDrawings::default();
    d.add_block("Coil ", &model_space_dwg(&[E::Circle((0.0, 0.0), 4.0)]))
        .unwrap();
    let (mut report, mut warnings) = (LibraryReport::default(), Vec::new());
    let symbols = read_blocks(&app, Some(&d), &mut report, &mut warnings).unwrap();
    assert_eq!(report.blocks_drawn, 1, "{:?}", symbols[0].graphics);

    // A later file whose name differs only in spaces or case replaces the earlier one.
    d.add_block(
        " COIL",
        &model_space_dwg(&[E::Circle((0.0, 0.0), 2.0), E::Circle((0.0, 0.0), 1.0)]),
    )
    .unwrap();
    let symbols = read_blocks(&app, Some(&d), &mut report, &mut warnings).unwrap();
    assert_eq!(symbols[0].graphics.len(), 2, "{:?}", symbols[0].graphics);
}

#[test]
fn only_drawings_of_imported_blocks_count_against_the_limits() {
    use crate::dwg::objects::tests::{TestEntity as E, model_space_dwg};
    use wc_core::geom::Shape;
    // The coil has connection points; the frame has none and is not imported.
    let app = bytes(&db("
CREATE TABLE tew_block (blk_id INTEGER, blk_name TEXT, blk_description TEXT);
CREATE TABLE tew_blockpt (bpt_blk_id INTEGER, bpt_no INTEGER, bpt_txt TEXT, bpt_dx REAL, bpt_dy REAL);
INSERT INTO tew_block VALUES (1, 'Coil', ''), (2, 'Frame', '');
INSERT INTO tew_blockpt VALUES (1, 1, 'A1', 0, 5);
"));
    let drawing = model_space_dwg(&[E::Circle((0.0, 0.0), 4.0)]);
    let file = drawing.len() as u64;
    // Room for one drawing only, and many drawings of blocks the library does not import first.
    let lim = super::super::drawings::Limits { file, total: file + 1 };
    let names: Vec<String> = (0..20).map(|i| format!("Library/Block/Unused {i}.dwg")).collect();
    let mut entries: Vec<(&str, &[u8])> = vec![("Database/tew_app_data.sqlite", &app)];
    entries.extend(names.iter().map(|n| (n.as_str(), drawing.as_slice())));
    entries.push(("Library/Block/Frame.dwg", &drawing));
    entries.push(("Library/Block/ Coil .dwg", &drawing));
    let lib = from_archive_within(archive(&entries), true, lim).unwrap();
    assert_eq!(lib.report.blocks_drawn, 1, "{:?}", lib.warnings);
    assert!(
        lib.symbols[0]
            .graphics
            .iter()
            .any(|g| matches!(g, Shape::Circle { radius, .. } if *radius == 4.0)),
        "{:?}",
        lib.symbols[0].graphics
    );
    assert!(
        !lib.warnings.iter().any(|w| w.contains("not read")),
        "{:?}",
        lib.warnings
    );

    // Two copies of the coil's drawing and room for one: the second is left out, and no block
    // stays a box for it. A second imported block left out by the limit is named as a box.
    let app = bytes(&db("
CREATE TABLE tew_block (blk_id INTEGER, blk_name TEXT, blk_description TEXT);
CREATE TABLE tew_blockpt (bpt_blk_id INTEGER, bpt_no INTEGER, bpt_txt TEXT, bpt_dx REAL, bpt_dy REAL);
INSERT INTO tew_block VALUES (1, 'Coil', ''), (2, 'Lamp', '');
INSERT INTO tew_blockpt VALUES (1, 1, 'A1', 0, 5), (2, 1, 'X1', 0, 5);
"));
    let buf = archive(&[
        ("Database/tew_app_data.sqlite", &app),
        ("Library/Block/Coil.dwg", &drawing),
        ("Other/Block/coil.dwg", &drawing),
        ("Library/Block/Lamp.dwg", &drawing),
    ]);
    let lib = from_archive_within(buf, true, lim).unwrap();
    assert_eq!(lib.report.blocks_drawn, 1, "{:?}", lib.warnings);
    assert!(
        lib.warnings
            .iter()
            .any(|w| w.contains("drawing file(s) were not read") && w.ends_with("; 1 block(s) stay boxes")),
        "{:?}",
        lib.warnings
    );
}

#[test]
fn a_block_the_total_refuses_is_counted_as_a_box() {
    use crate::dwg::objects::tests::{TestEntity as E, model_space_dwg};
    let app = bytes(&db("
CREATE TABLE tew_block (blk_id INTEGER, blk_name TEXT, blk_description TEXT);
CREATE TABLE tew_blockpt (bpt_blk_id INTEGER, bpt_no INTEGER, bpt_txt TEXT, bpt_dx REAL, bpt_dy REAL);
INSERT INTO tew_block VALUES (1, 'Coil', ''), (2, 'Lamp', '');
INSERT INTO tew_blockpt VALUES (1, 1, 'A1', 0, 5), (2, 1, 'X1', 0, 5);
"));
    let drawing = model_space_dwg(&[E::Circle((0.0, 0.0), 4.0)]);
    let file = drawing.len() as u64;
    // The coil is read; one byte is left, so the lamp's drawing is refused for the total.
    let lim = super::super::drawings::Limits { file, total: file + 1 };
    let buf = archive(&[
        ("Database/tew_app_data.sqlite", &app),
        ("Library/Block/Coil.dwg", &drawing),
        ("Library/Block/Lamp.dwg", &drawing),
    ]);
    let lib = from_archive_within(buf, true, lim).unwrap();
    assert_eq!(lib.report.blocks_drawn, 1, "{:?}", lib.warnings);
    assert!(
        lib.warnings.iter().any(|w| w.contains("1 block(s) stay boxes")),
        "{:?}",
        lib.warnings
    );
}

#[test]
fn a_block_without_a_name_finds_its_drawing() {
    use crate::dwg::objects::tests::{TestEntity as E, model_space_dwg};
    use wc_core::geom::Shape;
    // The block's name is empty in the source; its drawing's file name is only a space.
    let app = bytes(&db("
CREATE TABLE tew_block (blk_id INTEGER, blk_name TEXT, blk_description TEXT);
CREATE TABLE tew_blockpt (bpt_blk_id INTEGER, bpt_no INTEGER, bpt_txt TEXT, bpt_dx REAL, bpt_dy REAL);
INSERT INTO tew_block VALUES (1, '', '');
INSERT INTO tew_blockpt VALUES (1, 1, 'A1', 0, 5);
"));
    let drawing = model_space_dwg(&[E::Circle((0.0, 0.0), 4.0)]);
    let buf = archive(&[
        ("Database/tew_app_data.sqlite", &app),
        ("Library/Block/ .dwg", &drawing),
    ]);
    let lib = from_archive(buf).unwrap();
    assert_eq!(lib.report.blocks_drawn, 1, "{:?}", lib.warnings);
    assert!(
        lib.symbols[0]
            .graphics
            .iter()
            .any(|g| matches!(g, Shape::Circle { radius, .. } if *radius == 4.0)),
        "{:?}",
        lib.symbols[0].graphics
    );
}

#[test]
fn blocks_sharing_a_name_each_count_as_a_box() {
    use crate::dwg::objects::tests::{TestEntity as E, model_space_dwg};
    // Two imported blocks named Coil share one drawing, which the total refuses after the lamp's.
    let app = bytes(&db("
CREATE TABLE tew_block (blk_id INTEGER, blk_name TEXT, blk_description TEXT);
CREATE TABLE tew_blockpt (bpt_blk_id INTEGER, bpt_no INTEGER, bpt_txt TEXT, bpt_dx REAL, bpt_dy REAL);
INSERT INTO tew_block VALUES (1, 'Lamp', ''), (2, 'Coil', ''), (3, 'Coil', '');
INSERT INTO tew_blockpt VALUES (1, 1, 'X1', 0, 5), (2, 1, 'A1', 0, 5), (3, 1, 'A1', 0, 5);
"));
    let drawing = model_space_dwg(&[E::Circle((0.0, 0.0), 4.0)]);
    let file = drawing.len() as u64;
    let lim = super::super::drawings::Limits { file, total: file + 1 };
    let buf = archive(&[
        ("Database/tew_app_data.sqlite", &app),
        ("Library/Block/Lamp.dwg", &drawing),
        ("Library/Block/Coil.dwg", &drawing),
    ]);
    let lib = from_archive_within(buf, true, lim).unwrap();
    assert_eq!(
        (lib.report.blocks_out, lib.report.blocks_drawn),
        (3, 1),
        "{:?}",
        lib.warnings
    );
    assert!(
        lib.warnings.iter().any(|w| w.contains("2 block(s) stay boxes")),
        "{:?}",
        lib.warnings
    );
}

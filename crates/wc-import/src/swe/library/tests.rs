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
    let symbols = read_blocks(&db(APP_DATA), &mut report, &mut warnings).unwrap();
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
    let symbols = read_blocks(&c, &mut report, &mut warnings).unwrap();
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

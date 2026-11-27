//! Edge cases of the library readers: the report's lines, block identities, archive entry
//! choice, missing databases and tables, and cable types that clash with catalog parts.
#![allow(clippy::unwrap_used)]

use super::*;

fn db(schema: &str) -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(schema).unwrap();
    c
}

fn archive(entries: &[&str]) -> zip::ZipArchive<std::io::Cursor<Vec<u8>>> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        for name in entries {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        }
        z.finish().unwrap();
    }
    buf.set_position(0);
    zip::ZipArchive::new(buf).unwrap()
}

#[test]
fn the_report_lines_never_count_negative_boxes() {
    let report = LibraryReport {
        parts_in: 5,
        parts_out: 4,
        terminals: 12,
        cables_in: 3,
        cables_out: 2,
        blocks_in: 2,
        blocks_out: 1,
        blocks_drawn: 3,
    };
    assert_eq!(
        report.lines(),
        [
            "parts: 5 in the catalog, 4 imported, with 12 terminal(s)",
            "cable types: 3 in the source, 2 imported",
            "symbol blocks: 2 in the source, 1 imported with their connection points: 3 drawn from their DWG file, 0 as boxes",
        ]
    );
}

#[test]
fn a_block_is_named_by_its_guid_then_its_path_then_its_name() {
    let id = |name, guid, path| block_identity(name, guid, path);
    // The GUID loses its braces and case.
    assert_eq!(
        id("Coil", " {AB-Cd} ", "lib/coil"),
        "solidworks-electrical:block:guid:ab-cd"
    );
    // A blank GUID falls back to the path, trimmed but with its case kept.
    assert_eq!(
        id("Coil", "  ", " Lib/Coil "),
        "solidworks-electrical:block:path:Lib/Coil"
    );
    assert_eq!(id(" Coil ", "", "\t"), "solidworks-electrical:block:name:Coil");
    // Nothing at all still gives a name identity.
    assert_eq!(id("", "", ""), "solidworks-electrical:block:name:");
}

#[test]
fn quoted_columns_double_their_quotes() {
    assert_eq!(quoted("ref_id"), "\"ref_id\"");
    assert_eq!(quoted("a\"b"), "\"a\"\"b\"");
    assert_eq!(text("x"), "TRIM(COALESCE(CAST(\"x\" AS TEXT), ''))");
}

#[test]
fn an_archive_entry_is_found_by_its_file_name_in_any_case() {
    let zip = archive(&[
        "Projects/1/tew_catalog.sqlite",
        "Libs/Deep/TEW_CATALOG.SQLITE",
        "Libs/tew_catalog.sqlite.bak",
        "Other/tew_Catalog.sqlite",
    ]);
    // Outside Projects/ first, then the fewest folders, then by name.
    assert_eq!(
        find_entry(&zip, CATALOG_DB).as_deref(),
        Some("Other/tew_Catalog.sqlite")
    );
    // Only a project's copy: that one is used.
    let zip = archive(&[
        "Projects/2/Database/tew_app_data.sqlite",
        "Projects/10/tew_app_data.sqlite",
    ]);
    assert_eq!(
        find_entry(&zip, APP_DATA_DB).as_deref(),
        Some("Projects/10/tew_app_data.sqlite")
    );
    // A name that only ends like the file is not it.
    let zip = archive(&["Database/old_tew_catalog.sqlite"]);
    assert_eq!(find_entry(&zip, CATALOG_DB), None);
}

#[test]
fn no_databases_give_an_empty_library_with_a_warning_for_each() {
    let lib = read(None, None).unwrap();
    assert!(lib.parts.is_empty() && lib.symbols.is_empty());
    assert_eq!(lib.report, LibraryReport::default());
    assert_eq!(
        lib.warnings,
        [
            "no tew_catalog.sqlite; no manufacturer parts were read",
            "no tew_app_data.sqlite; no cable types or symbol blocks were read",
        ]
    );
}

#[test]
fn databases_without_the_tables_warn_for_each_library() {
    let empty = db("CREATE TABLE unrelated (x INTEGER);");
    let lib = read(Some(&empty), Some(&empty)).unwrap();
    assert!(lib.parts.is_empty() && lib.symbols.is_empty());
    assert_eq!(
        lib.warnings,
        [
            "the catalog has no tew_catreference table; no parts were read",
            "no tew_cablereference table; no cable types were read",
            "no tew_block table; no symbol blocks were read",
        ]
    );
}

#[test]
fn a_cable_type_with_a_catalog_parts_id_is_left_out() {
    let catalog = db("
        CREATE TABLE tew_catreference (ref_id INTEGER, ref_reference TEXT, ref_manufacturer TEXT, ref_description TEXT);
        INSERT INTO tew_catreference VALUES (1, 'CBL-2', 'Maker C', 'Catalog cable'), (2, 'ZZ-1', 'Maker Z', 'Last');
    ");
    let app = db("
        CREATE TABLE tew_cablereference (cbr_id INTEGER, cbr_reference TEXT, cbr_manufacturer TEXT, cbr_description TEXT);
        INSERT INTO tew_cablereference VALUES (1, 'cbl-2', 'MAKER C', 'Cable type'), (2, 'CBL-5', 'Maker C', NULL);
    ");
    let lib = read(Some(&catalog), Some(&app)).unwrap();
    let ids: Vec<&str> = lib.parts.iter().map(|p| p.id.as_str()).collect();
    // Sorted by id across parts and cables, the catalog part kept over the cable type.
    assert_eq!(ids, ["maker-c.cbl-2", "maker-c.cbl-5", "maker-z.zz-1"]);
    assert_eq!(lib.parts[0].description, "Catalog cable");
    assert_eq!((lib.report.cables_in, lib.report.cables_out), (2, 1));
    assert!(
        lib.warnings
            .iter()
            .any(|w| w == "1 cable type(s) have the id of a catalog part and were left out"),
        "{:?}",
        lib.warnings
    );
}

#[test]
fn an_archive_with_only_app_data_reads_cables_and_warns_for_the_catalog() {
    use std::io::Write;
    let app = db("
        CREATE TABLE tew_cablereference (cbr_id INTEGER, cbr_reference TEXT, cbr_manufacturer TEXT);
        INSERT INTO tew_cablereference VALUES (1, 'CBL-5', 'Maker C');
    ");
    let bytes = app.serialize(rusqlite::MAIN_DB).unwrap().to_vec();
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        z.start_file("Database/TEW_APP_DATA.sqlite", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(&bytes).unwrap();
        z.finish().unwrap();
    }
    buf.set_position(0);
    let lib = catalog_from_archive(buf).unwrap();
    let ids: Vec<&str> = lib.parts.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["maker-c.cbl-5"]);
    assert!(lib.warnings.iter().any(|w| w.contains("no tew_catalog.sqlite")));
    assert!(lib.warnings.iter().any(|w| w.contains("no tew_block table")));
}

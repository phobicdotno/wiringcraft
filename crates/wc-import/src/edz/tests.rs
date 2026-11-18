use std::io::{Cursor, Write};

use super::*;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

const PARTS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<partsmanagement>
  <part P_ARTICLE_PARTNR="SIE.3RT2015-1BB41" P_ARTICLE_MANUFACTURER="SIE"
        P_ARTICLE_ORDERNR="3RT2015-1BB41" P_ARTICLE_TYPENR="3RT2015-1BB41"
        P_ARTICLE_DESCR1="de_DE@Leistungsschütz, AC-3, 3 kW;en_US@Power contactor, AC-3, 3 kW;"
        P_ARTICLE_VOLTAGE="24 V DC" P_ARTICLE_WIDTH="45" P_ARTICLE_NOTE="">
    <variant P_ARTICLE_DEPTH="73" P_ARTICLE_WIDTH="99"/>
    <freeproperty P_ARTICLE_FREE_DATA_DESCRIPTION="ignored"/>
  </part>
  <part P_ARTICLE_PARTNR="RIT.8601.000" P_ARTICLE_MANUFACTURER="RIT"
        P_ARTICLE_DESCR1="Mounting plate &amp; rail"/>
</partsmanagement>
"#;

/// A 7-Zip archive with these entries, as an EDZ is.
fn seven_zip(entries: &[(&str, &[u8])]) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut w = sevenz_rust2::ArchiveWriter::new(Cursor::new(Vec::new()))?;
    for (name, data) in entries {
        w.push_archive_entry(sevenz_rust2::ArchiveEntry::new_file(name), Some(*data))?;
    }
    Ok(w.finish()?.into_inner())
}

fn zip(entries: &[(&str, &[u8])]) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        z.start_file(*name, zip::write::SimpleFileOptions::default())?;
        z.write_all(data)?;
    }
    Ok(z.finish()?.into_inner())
}

#[test]
fn parts_come_with_number_maker_description_and_properties() -> TestResult {
    let edz = seven_zip(&[
        ("items/parts.xml", PARTS.as_bytes()),
        ("items/macros/SIE_3RT2015.ema", b"binary macro"),
        ("items/pictures/3RT2015.jpg", b"\xFF\xD8\xFF"),
        ("items/readme.txt", b"hello"),
    ])?;
    let (parts, report, warnings) = read(&edz)?;
    assert_eq!(
        warnings,
        ["macro items/macros/SIE_3RT2015.ema not imported: it is not an EPLAN macro (no EplanPxfRoot)"]
    );
    assert_eq!(parts.len(), 2);
    let c = parts
        .iter()
        .find(|p| p.manufacturer == "SIE")
        .ok_or("no Siemens part")?;
    assert_eq!(c.order_number, "3RT2015-1BB41");
    assert_eq!(c.id, Part::make_id("SIE", "3RT2015-1BB41"));
    assert_eq!(c.description, "Power contactor, AC-3, 3 kW");
    assert_eq!(c.properties[PROP_EPLAN_PART], "SIE.3RT2015-1BB41");
    assert_eq!(c.properties["P_ARTICLE_VOLTAGE"], "24 V DC");
    // The part's own value wins over the variant's; the variant fills the gaps.
    assert_eq!(c.properties["P_ARTICLE_WIDTH"], "45");
    assert_eq!(c.properties["P_ARTICLE_DEPTH"], "73");
    assert!(
        !c.properties.contains_key(PROP_TYPE),
        "the type number equals the order number"
    );
    assert!(!c.properties.contains_key("P_ARTICLE_FREE_DATA_DESCRIPTION"));

    // Without an order number, the EPLAN part number without the maker is used.
    let r = parts.iter().find(|p| p.manufacturer == "RIT").ok_or("no Rittal part")?;
    assert_eq!(r.order_number, "8601.000");
    assert_eq!(r.description, "Mounting plate & rail");

    assert_eq!(
        report,
        Report {
            parts: 2,
            part_files: 1,
            macros: 1,
            macro_symbols: 0,
            macros_unread: 1,
            left_out: BTreeMap::new(),
            pictures: 1,
            other: 1,
            too_large: 0,
            macro_files: vec!["items/macros/SIE_3RT2015.ema".into()],
            part_macros: Vec::new(),
        }
    );
    assert_eq!(
        report.lines(),
        [
            "2 part(s) from 1 part file(s)",
            "1 macro(s) (.ema): 0 read as symbols, 1 not readable as symbols",
            "1 picture(s): not imported",
            "1 other file(s): not imported",
            "macro file: items/macros/SIE_3RT2015.ema",
        ]
    );
    Ok(())
}

#[test]
fn a_zip_edz_is_read_too_and_other_xml_is_only_counted() -> TestResult {
    let edz = zip(&[
        ("parts.xml", PARTS.as_bytes()),
        ("settings.xml", b"<settings><a/></settings>"),
    ])?;
    let (parts, report, _) = read(&edz)?;
    assert_eq!(parts.len(), 2);
    assert_eq!(report.part_files, 1);
    assert_eq!(report.other, 1);
    Ok(())
}

#[test]
fn damaged_archives_and_files_are_errors_not_panics() -> TestResult {
    assert!(read(b"not an archive at all").is_err());
    let good = seven_zip(&[("parts.xml", PARTS.as_bytes())])?;
    for cut in [8, 32, good.len() / 2, good.len() - 4] {
        assert!(read(&good[..cut]).is_err(), "cut at {cut} was taken");
    }
    let mut flipped = good.clone();
    let mid = flipped.len() / 2;
    flipped[mid] ^= 0xFF;
    let _ = read(&flipped);

    let broken = seven_zip(&[("parts.xml", b"<partsmanagement><part P_ARTICLE_PARTNR=\"x\">")])?;
    let err = read(&broken).err().ok_or("a part file that ends early was taken")?;
    assert!(err.to_string().starts_with("parts.xml"), "{err}");
    Ok(())
}

#[test]
fn an_archive_without_parts_says_so() -> TestResult {
    let edz = seven_zip(&[("macro.ema", b"m")])?;
    let (parts, report, warnings) = read(&edz)?;
    assert!(parts.is_empty());
    assert_eq!(report.macros, 1);
    assert_eq!(
        warnings.last().map(String::as_str),
        Some("the archive holds no part data")
    );
    Ok(())
}

#[test]
fn texts_in_several_languages_prefer_english() {
    assert_eq!(pick_language("de_DE@Schütz;en_US@Contactor;"), "Contactor");
    assert_eq!(pick_language("de_DE@Schütz;fr_FR@Contacteur;"), "Schütz");
    assert_eq!(pick_language("en_GB@Contactor;"), "Contactor");
    assert_eq!(pick_language("Plain text"), "Plain text");
    assert_eq!(pick_language("de_DE@;"), "");
}

#[test]
fn plain_text_with_an_at_sign_is_kept_whole() {
    assert_eq!(
        pick_language("Contact support@example.com"),
        "Contact support@example.com"
    );
    assert_eq!(pick_language("ORD@12;34"), "ORD@12;34");
    assert_eq!(pick_language("de-DE@Schütz;"), "Schütz");
}

#[test]
fn a_variant_fills_a_text_that_is_empty_in_every_language() -> TestResult {
    let xml = r#"<partsmanagement><part P_ARTICLE_ORDERNR="K1" P_ARTICLE_MANUFACTURER="ACME" P_ARTICLE_DESCR1="de_DE@;"><variant P_ARTICLE_DESCR1="en_US@Contactor;"/></part></partsmanagement>"#;
    let parts = part_xml(xml)?.ok_or("not part data")?;
    assert_eq!(parts[0].description, "Contactor");
    Ok(())
}

const SMALL: Limits = Limits {
    entry: 1024,
    entries: 3,
    total: 2048,
    unpacked: 8 * 1024,
};

#[test]
fn an_xml_file_over_the_entry_limit_is_skipped_and_reported() -> TestResult {
    let mut big = PARTS.to_owned();
    big.push_str(&" ".repeat(2000));
    for edz in [
        seven_zip(&[("big.xml", big.as_bytes()), ("parts.xml", PARTS.as_bytes())])?,
        zip(&[("big.xml", big.as_bytes()), ("parts.xml", PARTS.as_bytes())])?,
    ] {
        let (parts, report, _) = read_with(&edz, SMALL)?;
        assert_eq!(report.too_large, 1);
        assert_eq!(report.part_files, 1);
        assert_eq!(parts.len(), 2);
        assert!(
            report
                .lines()
                .contains(&"1 XML or macro file(s) over the size limit: skipped".to_owned())
        );
    }
    Ok(())
}

#[test]
fn part_xml_past_the_total_limit_is_skipped() -> TestResult {
    let edz = zip(&[
        ("a.xml", PARTS.as_bytes()),
        ("b.xml", PARTS.as_bytes()),
        ("c.xml", PARTS.as_bytes()),
    ])?;
    let size = PARTS.len() as u64;
    let limits = Limits {
        entry: size,
        total: size * 2 + size / 2,
        ..SMALL
    };
    let (_, report, _) = read_with(&edz, limits)?;
    assert_eq!((report.part_files, report.too_large), (2, 1));
    Ok(())
}

#[test]
fn an_archive_with_too_many_files_is_refused_not_cut_short() -> TestResult {
    let files: Vec<(&str, &[u8])> = vec![("a.ema", b"m"), ("b.ema", b"m"), ("c.ema", b"m"), ("d.ema", b"m")];
    for edz in [seven_zip(&files)?, zip(&files)?] {
        let err = read_with(&edz, SMALL).err().ok_or("four files were taken")?;
        assert!(err.to_string().contains("more than 3 files"), "{err}");
        assert!(read_with(&edz, Limits { entries: 4, ..SMALL }).is_ok());
    }
    Ok(())
}

#[test]
fn a_seven_zip_archive_that_unpacks_too_large_is_refused() -> TestResult {
    let filler = vec![0u8; 6 * 1024];
    let edz = seven_zip(&[("one.bin", &filler), ("two.bin", &filler)])?;
    let err = read_with(&edz, SMALL)
        .err()
        .ok_or("the archive was unpacked past the budget")?;
    assert!(err.to_string().contains("unpacks to more than"), "{err}");
    assert!(
        read_with(
            &edz,
            Limits {
                unpacked: 16 * 1024,
                ..SMALL
            }
        )
        .is_ok()
    );
    Ok(())
}

#[test]
fn xml_nested_too_deeply_is_an_error() {
    let xml = format!(
        "<partsmanagement>{}{}</partsmanagement>",
        "<a>".repeat(70),
        "</a>".repeat(70)
    );
    assert!(part_xml(&xml).is_err());
    let ok = format!(
        "<partsmanagement>{}{}</partsmanagement>",
        "<a>".repeat(10),
        "</a>".repeat(10)
    );
    assert!(part_xml(&ok).is_ok());
}

#[test]
fn the_largest_limits_do_not_overflow() -> TestResult {
    let edz = seven_zip(&[("parts.xml", PARTS.as_bytes())])?;
    let all = Limits {
        entry: u64::MAX,
        entries: usize::MAX,
        total: u64::MAX,
        unpacked: u64::MAX,
    };
    let (parts, _, _) = read_with(&edz, all)?;
    assert_eq!(parts.len(), 2);
    Ok(())
}

const PARTS_WITH_MACROS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<partsmanagement>
  <part P_ARTICLE_PARTNR="SIE.3RT2015-1BB41" P_ARTICLE_MANUFACTURER="SIE"
        P_ARTICLE_ORDERNR="3RT2015-1BB41" P_ARTICLE_MACRO="$(MD_MACROS)\SIE\SIE_3RT2015.ema"/>
  <part P_ARTICLE_PARTNR="SIE.3SU1100" P_ARTICLE_MANUFACTURER="SIE"
        P_ARTICLE_ORDERNR="3SU1100" P_ARTICLE_GROUPSYMBOLMACRO="SIE\SIE_3SU1.ema"/>
  <part P_ARTICLE_PARTNR="RIT.8601.000" P_ARTICLE_MANUFACTURER="RIT" P_ARTICLE_ORDERNR="8601.000"/>
</partsmanagement>
"#;

#[test]
fn the_report_lists_each_parts_macro_and_whether_the_archive_has_it() -> TestResult {
    let edz = seven_zip(&[
        ("items/parts.xml", PARTS_WITH_MACROS.as_bytes()),
        ("items/macros/sie_3rt2015.EMA", b"binary macro"),
        ("items/macros/OTHER.ema", b"binary macro"),
    ])?;
    let (parts, report, _) = read(&edz)?;
    assert_eq!(parts.len(), 3);
    assert_eq!(
        report.macro_files,
        ["items/macros/OTHER.ema", "items/macros/sie_3rt2015.EMA"],
        "sorted"
    );
    let contactor = Part::make_id("SIE", "3RT2015-1BB41");
    let button = Part::make_id("SIE", "3SU1100");
    assert_eq!(
        report.part_macros,
        [
            PartMacro {
                part: contactor.clone(),
                name: "$(MD_MACROS)\\SIE\\SIE_3RT2015.ema".into(),
                in_archive: true,
                symbol: None,
            },
            PartMacro {
                part: button.clone(),
                name: "SIE\\SIE_3SU1.ema".into(),
                in_archive: false,
                symbol: None,
            },
        ]
    );
    let lines = report.lines();
    assert!(
        lines.contains(&"macro file: items/macros/OTHER.ema".to_owned()),
        "{lines:?}"
    );
    assert!(
        lines.contains(&format!(
            "part {contactor}: macro $(MD_MACROS)\\SIE\\SIE_3RT2015.ema (in the archive), pick a symbol by hand"
        )),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains(&button) && l.contains("not in the archive")),
        "{lines:?}"
    );
    Ok(())
}

#[test]
fn a_symbol_map_gives_parts_with_a_mapped_macro_their_symbol() -> TestResult {
    let edz = zip(&[("parts.xml", PARTS_WITH_MACROS.as_bytes())])?;
    let (mut parts, mut report, _) = read(&edz)?;
    // By file name without `.ema`, any case; the other macro is not in the map.
    let map = crate::swe::parse_symbol_map("# EPLAN macros\nsie_3rt2015 = wc.coil\nNOPE.ema = wc.lamp\n")?;
    assert_eq!(apply_symbol_map(&mut parts, &map, &mut report), 1);
    let symbol = |id: &str| parts.iter().find(|p| p.id == id).and_then(|p| p.symbol.clone());
    assert_eq!(
        symbol(&Part::make_id("SIE", "3RT2015-1BB41")).as_deref(),
        Some("wc.coil")
    );
    assert_eq!(symbol(&Part::make_id("SIE", "3SU1100")), None);
    assert_eq!(symbol(&Part::make_id("RIT", "8601.000")), None, "no macro");
    assert!(
        report.lines().iter().any(|l| l.ends_with("drawn with symbol wc.coil")),
        "{:?}",
        report.lines()
    );
    // The full name and the file name match too.
    for key in ["$(MD_MACROS)\\SIE\\SIE_3SU1.ema", "SIE_3SU1.EMA", "SIE\\SIE_3SU1.ema"] {
        let (mut parts, mut report, _) = read(&edz)?;
        let map = BTreeMap::from([(key.to_owned(), "wc.lamp".to_owned())]);
        let n = apply_symbol_map(&mut parts, &map, &mut report);
        assert_eq!(n, usize::from(key != "$(MD_MACROS)\\SIE\\SIE_3SU1.ema"), "{key}");
    }
    Ok(())
}

#[test]
fn control_characters_in_macro_names_print_as_plain_text() -> TestResult {
    let edz = zip(&[
        ("parts.xml", PARTS_WITH_MACROS.as_bytes()),
        ("macros/evil\u{1b}[2J.ema", b"m"),
        ("macros/two\nlines.ema", b"m"),
    ])?;
    let (_, report, _) = read(&edz)?;
    let lines = report.lines();
    assert!(lines.iter().all(|l| !l.chars().any(char::is_control)), "{lines:?}");
    assert!(
        lines.contains(&"macro file: macros/evil\\u{1b}[2J.ema".to_owned()),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"macro file: macros/two\\u{a}lines.ema".to_owned()),
        "{lines:?}"
    );
    // EPLAN's backslash paths are left as they are.
    assert!(
        lines.iter().any(|l| l.contains("macro SIE\\SIE_3SU1.ema (")),
        "{lines:?}"
    );
    Ok(())
}

/// A macro as EPLAN writes one, cut down: a black box with two connection points.
const CONTACTOR_MACRO: &str = r#"<EplanPxfRoot Name="SIE_3RT2015">
 <MacroVariant VariantId="0" ReferencePoint="0/40" MacroFuncType="1" DocumentType="1">
  <ESymLib Name="SPECIAL" ID="0"/>
  <O17 A1261="0" A1262="1" A1381="301"><S89x5 A1651="0/-20" A1652="20/0"/><S40x1201 A762="0/40"/><P144 P20013="Q"/></O17>
  <O17 A1261="0" A1262="40" A1263="0" A1381="300" A185="1"><S40x1201 A762="4/40"/></O17>
  <O17 A1261="0" A1262="40" A1263="2" A1381="300" A185="2"><S40x1201 A762="4/20"/></O17>
 </MacroVariant>
</EplanPxfRoot>"#;

#[test]
fn macros_become_symbols_and_parts_get_the_symbol_of_the_macro_they_name() -> TestResult {
    let edz = seven_zip(&[
        ("items/parts.xml", PARTS_WITH_MACROS.as_bytes()),
        ("items/macros/sie_3rt2015.EMA", CONTACTOR_MACRO.as_bytes()),
        ("items/macros/OTHER.ema", b"binary macro"),
    ])?;
    let got = read_all(&edz)?;
    assert_eq!(got.symbols.len(), 1);
    let s = &got.symbols[0];
    assert_eq!(s.id, "eplan.sie_3rt2015");
    assert_eq!(s.letter_code, "Q");
    assert_eq!(s.pins.len(), 2);
    assert_eq!(
        (got.report.macros, got.report.macro_symbols, got.report.macros_unread),
        (2, 1, 1)
    );
    assert!(
        got.warnings
            .iter()
            .any(|w| w.starts_with("macro items/macros/OTHER.ema not imported: it is not an EPLAN macro")),
        "{:?}",
        got.warnings
    );
    let contactor = got
        .parts
        .iter()
        .find(|p| p.id == Part::make_id("SIE", "3RT2015-1BB41"))
        .ok_or("no contactor")?;
    assert_eq!(contactor.symbol.as_deref(), Some("eplan.sie_3rt2015"));
    let lines = got.report.lines();
    assert!(
        lines.contains(&"2 macro(s) (.ema): 1 read as symbols, 1 not readable as symbols".to_owned()),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("drawn with symbol eplan.sie_3rt2015")),
        "{lines:?}"
    );
    // A symbol map still decides over the macro's own symbol.
    let mut parts = got.parts.clone();
    let mut report = got.report.clone();
    let map: BTreeMap<String, String> = [("sie_3rt2015".to_owned(), "wc.coil".to_owned())].into();
    apply_symbol_map(&mut parts, &map, &mut report);
    assert!(parts.iter().any(|p| p.symbol.as_deref() == Some("wc.coil")));
    Ok(())
}

#[test]
fn a_macro_counts_towards_the_total_limit() -> TestResult {
    let edz = zip(&[("m.ema", CONTACTOR_MACRO.as_bytes())])?;
    let limits = Limits { total: 10, ..LIMITS };
    let got = read_all_with(&edz, limits)?;
    assert!(got.symbols.is_empty());
    assert_eq!(got.report.too_large, 1);
    Ok(())
}

#[test]
fn a_part_naming_its_macro_without_the_extension_gets_its_symbol() -> TestResult {
    let xml = r#"<partsmanagement><part P_ARTICLE_PARTNR="SIE.3RT2015" P_ARTICLE_MANUFACTURER="SIE" P_ARTICLE_MACRO="$(MD_MACROS)\SIE\SIE_3RT2015"/></partsmanagement>"#;
    let edz = zip(&[
        ("parts.xml", xml.as_bytes()),
        ("SIE_3RT2015.ema", CONTACTOR_MACRO.as_bytes()),
    ])?;
    let got = read_all(&edz)?;
    let part = got.parts.first().ok_or("no part")?;
    assert_eq!(part.symbol.as_deref(), Some("eplan.sie_3rt2015"));
    let m = got.report.part_macros.first().ok_or("no part macro")?;
    assert!(m.in_archive, "{m:?}");
    Ok(())
}

#[test]
fn macros_do_not_use_up_the_part_xml_budget() -> TestResult {
    let edz = zip(&[("a.ema", CONTACTOR_MACRO.as_bytes()), ("parts.xml", PARTS.as_bytes())])?;
    let total = PARTS.len().max(CONTACTOR_MACRO.len()) as u64 + 1;
    let got = read_all_with(&edz, Limits { total, ..LIMITS })?;
    assert_eq!(got.report.too_large, 0);
    assert!(got.report.parts > 0);
    assert_eq!(got.symbols.len(), 1);
    Ok(())
}

#[test]
fn the_parts_only_reader_links_no_part_to_a_symbol_it_does_not_return() -> TestResult {
    let xml = r#"<partsmanagement><part P_ARTICLE_PARTNR="SIE.3RT2015" P_ARTICLE_MANUFACTURER="SIE" P_ARTICLE_MACRO="SIE_3RT2015.ema"/></partsmanagement>"#;
    let edz = zip(&[
        ("parts.xml", xml.as_bytes()),
        ("SIE_3RT2015.ema", CONTACTOR_MACRO.as_bytes()),
    ])?;
    let (parts, report, _) = read(&edz)?;
    assert_eq!(parts.first().and_then(|p| p.symbol.clone()), None);
    assert!(report.part_macros.iter().all(|m| m.symbol.is_none()));
    Ok(())
}

#[test]
fn macros_of_the_same_file_name_are_told_apart_by_their_path() -> TestResult {
    let other = CONTACTOR_MACRO.replace("P20013=\"Q\"", "P20013=\"K\"");
    let xml = r#"<partsmanagement><part P_ARTICLE_PARTNR="B.1" P_ARTICLE_MANUFACTURER="B" P_ARTICLE_MACRO="$(MD_MACROS)\B\relay.ema"/></partsmanagement>"#;
    let edz = zip(&[
        ("parts.xml", xml.as_bytes()),
        ("macros/A/relay.ema", CONTACTOR_MACRO.as_bytes()),
        ("macros/B/relay.ema", other.as_bytes()),
    ])?;
    let got = read_all(&edz)?;
    let part = got.parts.first().ok_or("no part")?;
    let id = part.symbol.clone().ok_or("no symbol")?;
    let s = got.symbols.iter().find(|s| s.id == id).ok_or("no such symbol")?;
    assert_eq!(s.letter_code, "K");
    Ok(())
}

#[test]
fn only_the_macro_whose_symbol_a_part_got_reports_it() -> TestResult {
    let xml = r#"<partsmanagement><part P_ARTICLE_PARTNR="B.1" P_ARTICLE_MANUFACTURER="B" P_ARTICLE_MACRO="one.ema" P_ARTICLE_GROUPSYMBOLMACRO="two.ema"/></partsmanagement>"#;
    let edz = zip(&[
        ("parts.xml", xml.as_bytes()),
        ("one.ema", CONTACTOR_MACRO.as_bytes()),
        ("two.ema", CONTACTOR_MACRO.as_bytes()),
    ])?;
    let got = read_all(&edz)?;
    let symbols: Vec<Option<&str>> = got.report.part_macros.iter().map(|m| m.symbol.as_deref()).collect();
    assert_eq!(symbols, [Some("eplan.one"), None]);
    assert!(got.report.part_macros.iter().all(|m| m.in_archive));
    // A map choosing the group symbol macro takes the report row with it.
    let (mut parts, mut report) = (got.parts.clone(), got.report.clone());
    let map: BTreeMap<String, String> = [("two".to_owned(), "wc.coil".to_owned())].into();
    apply_symbol_map(&mut parts, &map, &mut report);
    let symbols: Vec<Option<&str>> = report.part_macros.iter().map(|m| m.symbol.as_deref()).collect();
    assert_eq!(symbols, [None, Some("wc.coil")]);
    Ok(())
}

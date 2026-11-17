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
    assert!(warnings.is_empty(), "{warnings:?}");
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
            pictures: 1,
            other: 1,
            too_large: 0,
        }
    );
    assert_eq!(
        report.lines(),
        [
            "2 part(s) from 1 part file(s)",
            "1 macro(s) (.ema): not imported",
            "1 picture(s): not imported",
            "1 other file(s): not imported"
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
    assert_eq!(warnings, ["the archive holds no part data"]);
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

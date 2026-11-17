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
                .contains(&"1 XML file(s) over the size limit: skipped".to_owned())
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

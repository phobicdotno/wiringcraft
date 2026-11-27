//! DXF import through the public API: errors on unreadable input, pin naming and the
//! matching of block references to library symbols.

use std::collections::BTreeSet;

use wc_import::dxf::{footprint, page, symbol};
use wc_import::{Error, Result};

fn dxf(entities: &str, blocks: &str) -> String {
    format!(
        "0\nSECTION\n2\nHEADER\n0\nENDSEC\n0\nSECTION\n2\nBLOCKS\n{blocks}0\nENDSEC\n0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n"
    )
}

const LINE: &str = "0\nLINE\n8\n0\n10\n0\n20\n0\n11\n10\n21\n0\n";

fn format_error<T>(r: Result<T>) -> String {
    match r {
        Err(Error::Format(why)) => why,
        Err(other) => format!("not a format error: {other}"),
        Ok(_) => "no error".to_owned(),
    }
}

#[test]
fn binary_and_non_dxf_text_are_refused_with_a_reason() {
    let why = format_error(symbol("AutoCAD Binary DXF\r\n\u{1a}\0", "my.part", "Part"));
    assert!(why.contains("binary DXF is not supported"), "{why}");

    let why = format_error(page("hello\nworld\n", 1.0, &BTreeSet::new()));
    assert!(why.contains("not an ASCII DXF file"), "{why}");
    assert!(why.contains("\"hello\""), "the bad group code is named: {why}");
}

#[test]
fn a_page_scale_that_is_not_a_positive_number_is_refused() {
    let text = dxf(LINE, "");
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let why = format_error(page(&text, scale, &BTreeSet::new()));
        assert_eq!(why, "scale must be a positive number", "scale {scale}");
    }
    assert!(page(&text, 0.5, &BTreeSet::new()).is_ok());
}

#[test]
fn an_empty_drawing_is_neither_a_symbol_nor_a_footprint() {
    let text = dxf("", "");
    assert_eq!(format_error(symbol(&text, "my.part", "Part")), "the drawing is empty");
    let why = format_error(footprint(&text));
    assert!(why.contains("no outline WiringCraft can read"), "{why}");

    // Only entities that are not read: the footprint error says which.
    let why = format_error(footprint(&dxf("0\n3DFACE\n8\n0\n", "")));
    assert!(why.contains("3DFACE"), "{why}");
}

#[test]
fn crlf_line_endings_read_like_plain_ones() -> Result<()> {
    let text = dxf(&format!("{LINE}0\nPOINT\n8\nPIN\n10\n0\n20\n0\n"), "");
    let plain = symbol(&text, "my.part", "Part")?;
    let crlf = symbol(&text.replace('\n', "\r\n"), "my.part", "Part")?;
    assert_eq!(plain.pins, crlf.pins);
    assert_eq!(plain.graphics, crlf.graphics);
    Ok(())
}

#[test]
fn a_pin_label_too_far_away_leaves_the_pin_numbered() -> Result<()> {
    // The label is 10 mm from the pin: past the 6 mm reach, so the pin keeps its number.
    let ents = format!("{LINE}0\nPOINT\n8\nPIN\n10\n0\n20\n0\n0\nTEXT\n8\nPIN\n10\n10\n20\n0\n40\n2\n1\nA1\n");
    let s = symbol(&dxf(&ents, ""), "my.part", "Part")?;
    assert_eq!(s.pins.len(), 1);
    assert_eq!(s.pins[0].name, "1");
    // Pin-layer text is never drawn, even when it names no pin.
    assert_eq!(s.graphics.len(), 1, "{:?}", s.graphics);
    Ok(())
}

#[test]
fn pins_on_any_pin_layer_are_ordered_and_repeated_names_made_unique() -> Result<()> {
    // Pin circles on CONNECTION and WC_PIN, a point on PINS; two labelled A1 and one blank.
    let ents = format!(
        "{LINE}\
         0\nCIRCLE\n8\nCONNECTION\n10\n5\n20\n0\n40\n0.5\n\
         0\nCIRCLE\n8\nWC_PIN\n10\n0\n20\n0\n40\n0.5\n\
         0\nPOINT\n8\nPINS\n10\n0\n20\n10\n\
         0\nTEXT\n8\nPINS\n10\n0\n20\n0\n40\n2\n1\nA1\n\
         0\nTEXT\n8\nPINS\n10\n5\n20\n0\n40\n2\n1\nA1\n\
         0\nTEXT\n8\nPINS\n10\n0\n20\n10\n40\n2\n1\n  \n"
    );
    let s = symbol(&dxf(&ents, ""), "my.part", "Part")?;
    let names: Vec<&str> = s.pins.iter().map(|p| p.name.as_str()).collect();
    // The top pin (y = 10 in DXF, up) first, then left to right; the blank label falls back to
    // the pin's number and the second A1 is primed.
    assert_eq!(names, ["1", "A1", "A1'"], "{:?}", s.pins);
    assert_eq!(s.pins[0].at.x, 0.0);
    assert_eq!(s.pins[0].at.y, 0.0, "the first pin is the origin");
    // The pin circles are not drawn: only the line is.
    assert_eq!(s.graphics.len(), 1, "{:?}", s.graphics);
    Ok(())
}

#[test]
fn a_bad_symbol_id_is_refused() {
    let text = dxf(&format!("{LINE}0\nPOINT\n8\nPIN\n10\n0\n20\n0\n"), "");
    assert!(symbol(&text, "", "Part").is_err());
    assert!(symbol(&text, "../escape", "Part").is_err());
}

#[test]
fn block_names_match_symbols_ignoring_case_spaces_and_slashes() -> Result<()> {
    let blocks = "0\nBLOCK\n2\nMy Coil/1\n0\nLINE\n10\n0\n20\n0\n11\n1\n21\n0\n0\nENDBLK\n";
    let ents = "0\nINSERT\n2\nMy Coil/1\n10\n10\n20\n10\n";
    let known = BTreeSet::from(["my-coil-1".to_owned()]);
    let p = page(&dxf(ents, blocks), 1.0, &known)?;
    assert_eq!(p.symbols.len(), 1, "{:?}", p.graphics);
    assert_eq!(p.symbols[0].symbol, "my-coil-1", "the library id, not the block name");
    assert!(p.graphics.is_empty(), "{:?}", p.graphics);

    // Not a known symbol: the block is drawn as graphics.
    let p = page(&dxf(ents, blocks), 1.0, &BTreeSet::new())?;
    assert!(p.symbols.is_empty());
    assert_eq!(p.graphics.len(), 1);
    Ok(())
}

#[test]
fn the_device_tag_comes_from_tag_device_bmk_or_ref_attributes() -> Result<()> {
    let blocks = "0\nBLOCK\n2\nK\n0\nLINE\n10\n0\n20\n0\n11\n1\n21\n0\n0\nENDBLK\n";
    let known = BTreeSet::from(["k".to_owned()]);
    for name in ["TAG", "device", "Bmk", "REF"] {
        let ents = format!("0\nINSERT\n2\nK\n10\n0\n20\n0\n0\nATTRIB\n2\n{name}\n1\n-K7\n10\n0\n20\n0\n40\n2\n");
        let p = page(&dxf(&ents, blocks), 1.0, &known)?;
        let tags: Vec<String> = p
            .symbols
            .iter()
            .map(|s| s.tag.as_ref().map(ToString::to_string).unwrap_or_default())
            .collect();
        assert_eq!(tags, ["-K7"], "attribute {name}");
    }
    // Another attribute name, or a blank value: placed without a tag and nothing said.
    for (name, value) in [("NOTE", "-K7"), ("TAG", "  ")] {
        let ents = format!("0\nINSERT\n2\nK\n10\n0\n20\n0\n0\nATTRIB\n2\n{name}\n1\n{value}\n10\n0\n20\n0\n40\n2\n");
        let p = page(&dxf(&ents, blocks), 1.0, &known)?;
        assert_eq!(p.symbols.len(), 1);
        assert!(p.symbols[0].tag.is_none(), "{name} {value:?}");
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    }
    Ok(())
}

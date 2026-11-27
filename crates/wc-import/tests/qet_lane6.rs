//! Checks of the public QElectroTech element import (`wc_import::qet::element`): roles from
//! `link_type`, the name it picks, the scale limits, the cap on listed reasons, the default
//! label place and moving the first terminal to the origin.

use wc_core::geom::Point;
use wc_core::symbol::{PinDir, SymbolRole};
use wc_import::Result;
use wc_import::qet::{DEFAULT_SCALE, element};

/// An element with the given `link_type` attribute text, `<names>` content and description body.
fn el(attrs: &str, names: &str, body: &str) -> String {
    format!(
        r#"<definition type="element" {attrs}><names>{names}</names><description>{body}</description></definition>"#
    )
}

const LINE: &str = r#"<line x1="0" y1="0" x2="10" y2="0"/>"#;

#[test]
fn link_type_sets_the_symbol_role() -> Result<()> {
    for (link, role) in [
        ("master", SymbolRole::Main),
        ("slave", SymbolRole::Auxiliary),
        ("terminal", SymbolRole::Terminal),
        ("next_report", SymbolRole::Interruption),
        ("previous_report", SymbolRole::Interruption),
        ("simple", SymbolRole::Standalone),
        ("", SymbolRole::Standalone),
    ] {
        let (imp, warnings) = element(&el(&format!(r#"link_type="{link}""#), "", LINE), "qet.x", 1.0)?;
        assert_eq!(imp.symbol.role, role, "{link}");
        assert!(warnings.is_empty(), "{link}: {warnings:?}");
    }
    Ok(())
}

#[test]
fn the_english_name_wins_then_the_first_then_the_id() -> Result<()> {
    let both = r#"<name lang="fr">Contact</name><name lang="en">Contact NO</name>"#;
    let (imp, _) = element(&el("", both, LINE), "qet.contact", 1.0)?;
    assert_eq!(imp.qet_name, "Contact NO");
    assert_eq!(imp.symbol.name, "Contact NO");

    let no_en = r#"<name lang="fr">Contact</name><name lang="de">Kontakt</name>"#;
    let (imp, _) = element(&el("", no_en, LINE), "qet.contact", 1.0)?;
    assert_eq!(imp.qet_name, "Contact");

    // No usable name (blank text only): the last part of the id is used.
    let blank = r#"<name lang="en">   </name>"#;
    let (imp, _) = element(&el("", blank, LINE), "qet.lib.relay_coil", 1.0)?;
    assert_eq!(imp.qet_name, "relay_coil");
    assert_eq!(imp.symbol.id, "qet.lib.relay_coil");
    Ok(())
}

#[test]
fn scale_must_be_above_zero_and_at_most_one_hundred() -> Result<()> {
    let text = el("", "", LINE);
    for bad in [0.0, -1.0, 100.5, f64::INFINITY] {
        let r = element(&text, "qet.x", bad);
        assert!(r.is_err_and(|e| e.to_string().contains("scale")), "{bad}");
    }
    let (imp, _) = element(&text, "qet.x", 100.0)?;
    assert!(matches!(
        imp.symbol.graphics.as_slice(),
        [wc_core::geom::Shape::Line { a, b }] if *a == Point::new(0.0, 0.0) && *b == Point::new(1000.0, 0.0)
    ));
    let (imp, _) = element(&text, "qet.x", DEFAULT_SCALE)?;
    assert!(matches!(
        imp.symbol.graphics.as_slice(),
        [wc_core::geom::Shape::Line { b, .. }] if *b == Point::new(5.0, 0.0)
    ));
    Ok(())
}

#[test]
fn many_bad_values_list_twenty_reasons_and_count_the_rest() -> Result<()> {
    let bad = r#"<line x1="oops" y1="0" x2="1" y2="1"/>"#.repeat(25);
    let (_, warnings) = element(&el("", "", &format!("{bad}{LINE}")), "qet.x", 1.0)?;
    let reasons = warnings.iter().filter(|w| w.contains("is not a number")).count();
    assert_eq!(reasons, 20, "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("x1 = \"oops\"")), "{warnings:?}");
    assert_eq!(warnings.last().map(String::as_str), Some("5 more value(s) not read"));
    Ok(())
}

#[test]
fn without_a_label_text_the_tag_goes_left_of_the_drawing() -> Result<()> {
    let body = r#"<rect x="-20" y="-5" width="10" height="10"/><line x1="0" y1="0" x2="10" y2="0"/>"#;
    let (imp, warnings) = element(&el("", "", body), "qet.x", 1.0)?;
    assert!(warnings.is_empty(), "{warnings:?}");
    // No terminals: nothing moves, and the tag sits 3 mm left of the leftmost shape.
    assert_eq!(imp.symbol.tag_at, Point::new(-23.0, 0.0));
    Ok(())
}

#[test]
fn the_first_terminal_becomes_the_origin_and_unnamed_ones_are_numbered() -> Result<()> {
    let body = r#"<line x1="10" y1="-20" x2="10" y2="20"/>
<terminal x="10" y="-20" orientation="n"/>
<terminal x="10" y="20" orientation="s"/>
<terminal x="30" y="0" orientation="e" name="A1"/>"#;
    let (imp, warnings) = element(&el("", "", body), "qet.x", DEFAULT_SCALE)?;
    assert!(warnings.is_empty(), "{warnings:?}");
    let pins: Vec<(&str, Point, PinDir)> = imp.symbol.pins.iter().map(|p| (p.name.as_str(), p.at, p.dir)).collect();
    assert_eq!(
        pins,
        vec![
            ("1", Point::new(0.0, 0.0), PinDir::Up),
            ("2", Point::new(0.0, 20.0), PinDir::Down),
            ("A1", Point::new(10.0, 10.0), PinDir::Right),
        ]
    );
    assert!(matches!(
        imp.symbol.graphics.as_slice(),
        [wc_core::geom::Shape::Line { a, b }] if *a == Point::new(0.0, 0.0) && *b == Point::new(0.0, 20.0)
    ));
    Ok(())
}

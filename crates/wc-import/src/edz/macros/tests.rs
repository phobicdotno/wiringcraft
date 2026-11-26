use super::*;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

/// A small I/O module macro as EPLAN writes one: an overview first, then the multi-line
/// representation (variant A) with a black box, four connection points facing up, down, left
/// and right, and drawing objects nested in groups. Reference point (100, 200).
const MODULE: &str = r#"<?xml version="1.0" encoding="utf-8" ?>
<EplanPxfRoot Name="ACME.IO4" Type="WindowMacro" Version="2.1">
 <MacroVariant MacroFuncType="4" VariantId="0" ReferencePoint="10/20/0" DocumentType="3">
  <ESymLib Name="SPECIAL" ID="0"/>
  <O4 A1="4/1">
   <O17 A1="17/9" A1261="0" A1262="40" A1263="1" A1381="300" A1382="1" A1383="2" A185="1">
    <S40x1201 A762="10/20"/>
   </O17>
  </O4>
 </MacroVariant>
 <MacroVariant MacroFuncType="1" VariantId="0" ReferencePoint="100/200/0" DocumentType="1">
  <ESymLib Name="SPECIAL" ID="0"/>
  <ESymLib Name="IEC_symbol" ID="1"/>
  <ESymLib/>
  <O4 A1="4/2">
   <O37 A1="37/3" A1631="0/-50" A1632="60/0">
    <S40x1201 A762="100/200"/>
    <S89x5 A1651="0/-50" A1652="60/0"/>
   </O37>
   <O17 A1="17/4" A1261="0" A1262="1" A1263="0" A1381="301" A1382="1" A1383="1">
    <S89x5 A1651="0/-20" A1652="40/0"/>
    <S40x1201 A762="100/200"/>
    <P144 P20013="A" P20014="3"/>
   </O17>
   <O17 A1="17/5" A1261="0" A1262="40" A1263="2" A1381="300" A1382="1" A1383="2" A185="1">
    <S40x1201 A762="108/180"/>
    <P150 P20013="A" P20014="3"/>
   </O17>
   <O17 A1="17/6" A1261="0" A1262="385" A1263="0" A1381="300" A1382="1" A1383="2" A185="2">
    <S40x1201 A762="116/200"/>
   </O17>
   <O17 A1="17/7" A1261="0" A1262="40" A1263="1" A1381="300" A1382="1" A1383="2" A185="1">
    <S40x1201 A762="100/190"/>
   </O17>
   <O17 A1="17/8" A1261="0" A1262="40" A1263="7" A1381="300" A1382="1" A1383="2" A185="">
    <S40x1201 A762="140/190"/>
   </O17>
   <O26 A1="26/10">
    <O26 A1="26/11">
     <O31 A1="31/12" A531="100/200" A532="140/180"/>
     <O34 A1="34/13" A621="101/199;102/199;102/198"/>
     <O89 A1="89/14" A1651="104/196" A1652="110/190"/>
     <O32 A1="32/15" A555="120/190" A556="2/0" A553="0" A554="6.28318530717959"/>
     <O32 A1="32/16" A555="130/190" A556="3/0" A553="0" A554="1.5707963267949"/>
     <O30 A1="30/17" A501="105/185" A511="de_DE@Eingang;en_US@Input;">
      <S54x505 A961="1.8" A966="5"/>
     </O30>
     <O30 A1="30/18" A501="105/183" A511="??_??@24 V;"/>
     <O42 A1="42/19"/>
    </O26>
   </O26>
  </O4>
 </MacroVariant>
 <MacroVariant MacroFuncType="1" VariantId="1" ReferencePoint="0/0/0" DocumentType="1">
  <O4 A1="4/20"/>
 </MacroVariant>
</EplanPxfRoot>
"#;

fn near(a: Point, x: f64, y: f64) -> bool {
    a.distance(Point::new(x, y)) < 1e-9
}

fn pin<'a>(s: &'a Symbol, name: &str) -> std::result::Result<&'a Pin, String> {
    s.pin(name).ok_or_else(|| format!("no pin {name} in {:?}", s.pins))
}

#[test]
fn the_multi_line_variant_a_becomes_a_symbol_with_its_connection_points_as_pins() -> TestResult {
    let m = read("macros/ACME.IO4.ema", MODULE)?;
    let s = &m.symbol;
    assert_eq!(s.id, "eplan.acme_io4");
    assert_eq!(s.name, "ACME.IO4");
    assert_eq!(s.letter_code, "A");
    assert_eq!(s.role, SymbolRole::Standalone);
    assert_eq!(s.description, "EPLAN macro ACME.IO4.ema, multi-line, variant A");
    // Pins relative to the reference point, y flipped; variant 0 faces up, 1 left, 2 down, 3
    // right. A repeated designation is marked, a blank one numbered by its place.
    let names: Vec<&str> = s.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["1", "2", "1'", "4"]);
    let p = pin(s, "1")?;
    assert!(near(p.at, 8.0, 20.0) && p.dir == PinDir::Down, "{p:?}");
    let p = pin(s, "2")?;
    assert!(near(p.at, 16.0, 0.0) && p.dir == PinDir::Up, "{p:?}");
    let p = pin(s, "1'")?;
    assert!(near(p.at, 0.0, 10.0) && p.dir == PinDir::Left, "{p:?}");
    let p = pin(s, "4")?;
    // Variant 7 is variant 3 mirrored; a mirror leaves a pin facing up as it is.
    assert!(near(p.at, 40.0, 10.0) && p.dir == PinDir::Right, "{p:?}");
    Ok(())
}

#[test]
fn graphics_are_flipped_to_a_y_down_page_and_the_macro_frame_is_not_drawn() -> TestResult {
    let s = read("ACME.IO4.ema", MODULE)?.symbol;
    let g = &s.graphics;
    assert!(g.contains(&Shape::Line {
        a: Point::new(0.0, 0.0),
        b: Point::new(40.0, 20.0),
    }));
    assert!(g.contains(&Shape::Polyline {
        points: vec![Point::new(1.0, 1.0), Point::new(2.0, 1.0), Point::new(2.0, 2.0)],
        closed: false,
        filled: false,
    }));
    assert!(g.contains(&Shape::Rect {
        min: Point::new(4.0, 4.0),
        max: Point::new(10.0, 10.0),
    }));
    assert!(g.contains(&Shape::Circle {
        center: Point::new(20.0, 10.0),
        radius: 2.0,
        filled: false,
    }));
    // A quarter arc from 0 to 90 degrees, counter-clockwise with y up, runs from 270 to 360
    // (0) on a y-down page.
    let arc = g
        .iter()
        .find_map(|x| match x {
            Shape::Arc {
                center,
                radius,
                start,
                end,
            } => Some((*center, *radius, *start, *end)),
            _ => None,
        })
        .ok_or("no arc")?;
    assert!(near(arc.0, 30.0, 10.0) && arc.1 == 3.0, "{arc:?}");
    assert!((arc.2 - 270.0).abs() < 1e-6 && arc.3.abs() < 1e-6, "{arc:?}");
    // The black box outline: 40 wide, 20 high, below the reference point in EPLAN.
    assert!(g.contains(&Shape::Rect {
        min: Point::new(0.0, 0.0),
        max: Point::new(40.0, 20.0),
    }));
    // Not the macro's own 60 by 50 frame.
    assert!(
        !g.iter().any(|x| matches!(x, Shape::Rect { max, .. } if max.x > 50.0)),
        "{g:?}"
    );
    // Texts in English, with their height and alignment; a text in no language is kept.
    assert!(g.contains(&Shape::Text {
        at: Point::new(5.0, 15.0),
        text: "Input".into(),
        height: 1.8,
        align: Align::Center,
        angle: 0.0,
    }));
    assert!(
        g.iter()
            .any(|x| matches!(x, Shape::Text { text, .. } if text == "24 V")),
        "{g:?}"
    );
    assert_eq!(s.tag_at, Point::new(0.0, -2.0));
    Ok(())
}

#[test]
fn what_is_left_out_is_counted() -> TestResult {
    let m = read("ACME.IO4.ema", MODULE)?;
    let want: BTreeMap<String, usize> = [
        ("overview representations", 1),
        ("other variants of the multi-line representation", 1),
        ("objects of other kinds (connections, potentials, references)", 1),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    assert_eq!(m.left_out, want);
    Ok(())
}

const COIL: &str = r#"<EplanPxfRoot Name="K">
 <MacroVariant VariantId="0" ReferencePoint="50/100" MacroFuncType="1" DocumentType="1">
  <ESymLib Name="IEC_symbol" ID="1"/>
  <O4>
   <O17 A1261="1" A1262="20" A1263="1" A1381="200" A1382="1" A1383="1" A185="A1;A2">
    <S40x1201 A762="60/90"/>
    <P144 P20013="K" P20014="1"/>
   </O17>
  </O4>
 </MacroVariant>
</EplanPxfRoot>"#;

#[test]
fn a_library_symbol_is_drawn_as_the_symbol_of_its_kind_turned_by_its_variant() -> TestResult {
    let m = read("BES_contactor.ema", COIL)?;
    let s = &m.symbol;
    assert_eq!(s.letter_code, "K");
    assert_eq!(s.role, SymbolRole::Main, "a coil drives contacts");
    // The coil's first connection point is at the placement (10, 10); variant 1 turns the
    // symbol a quarter, so the second one, 8 below it in variant 0, is 8 to the right.
    let a1 = pin(s, "A1")?;
    let a2 = pin(s, "A2")?;
    assert!(near(a1.at, 10.0, 10.0) && a1.dir == PinDir::Left, "{a1:?}");
    assert!(near(a2.at, 18.0, 10.0) && a2.dir == PinDir::Right, "{a2:?}");
    assert!(!s.graphics.is_empty());
    assert_eq!(
        m.left_out
            .get("library symbols drawn as the WiringCraft symbol of the same kind"),
        Some(&1)
    );
    Ok(())
}

/// A coil whose macro shows its device tag and the description of its first connection point,
/// a connection point with its description shown, and one whose property placements run in a
/// loop. Reference point (50, 100).
const SHOWN_TEXTS: &str = r#"<EplanPxfRoot Name="K">
 <MacroVariant VariantId="0" ReferencePoint="50/100" MacroFuncType="1" DocumentType="1">
  <ESymLib Name="IEC_symbol" ID="1"/>
  <ESymLib Name="SPECIAL" ID="0"/>
  <O4>
   <O17 A1="17/50" A1261="1" A1262="20" A1263="0" A1381="200" A1382="1" A1383="1" A185="A1;A2" A186="Coil;Return">
    <S53x5 A4="1" R7="17/50" A771="20010" A953="0" A501="-6/3">
     <S54x505 A961="L" A966="7"/>
    </S53x5>
    <S53x5 A4="2" R7="17/50" A771="20028" A953="1" A501="1/0"/>
    <S53x5 A4="3" R7="17/50" A771="20029" A953="0" A502="17/50/2" A501="0/-2">
     <S54x505 A961="1.5" A962="1.5707963267949" A966="1"/>
    </S53x5>
    <P11 P20051="0"/>
    <S40x1201 A762="60/90"/>
    <P144 P20013="K" P20014="1"/>
   </O17>
   <O17 A1="17/51" A1261="0" A1262="40" A1263="0" A1381="300" A1382="1" A1383="2" A185="1" A186="IN1">
    <S53x5 A4="1" R7="17/51" A771="20029" A953="0" A501="2/1"/>
    <S53x5 A4="2" R7="17/51" A771="20010" A953="0" A501="-50/-50"/>
    <S40x1201 A762="80/90"/>
   </O17>
   <O17 A1="17/52" A1261="0" A1262="40" A1263="0" A1381="300" A1382="1" A1383="2" A185="2" A186="Loop">
    <S53x5 A4="1" R7="17/52" A771="20029" A953="0" A502="17/52/2"/>
    <S53x5 A4="2" R7="17/52" A771="20028" A953="0" A502="17/52/1"/>
    <S40x1201 A762="90/90"/>
   </O17>
  </O4>
 </MacroVariant>
</EplanPxfRoot>"#;

fn text_at(s: &Symbol, text: &str) -> std::result::Result<(Point, f64, f64, Align), String> {
    s.graphics
        .iter()
        .find_map(|g| match g {
            Shape::Text {
                at,
                text: t,
                height,
                align,
                angle,
            } if t == text => Some((*at, *height, *angle, *align)),
            _ => None,
        })
        .ok_or_else(|| format!("no text {text} in {:?}", s.graphics))
}

#[test]
fn the_device_tag_and_connection_point_descriptions_go_where_the_macro_shows_them() -> TestResult {
    let m = read("coil.ema", SHOWN_TEXTS)?;
    let s = &m.symbol;
    // The coil's displayed device tag: 6 left of and 3 above its point (60, 90), on the page
    // (4, 7); not the tag shown by the connection point, which has no device tag.
    assert!(near(s.tag_at, 4.0, 7.0), "{:?}", s.tag_at);
    // "Coil" is anchored to the first point's designation (1 right of the point) and 2 below
    // it, reads bottom to top, left aligned, 1.5 high.
    let (at, height, angle, align) = text_at(s, "Coil")?;
    assert!(near(at, 11.0, 12.0), "{at:?}");
    assert!((height - 1.5).abs() < 1e-9 && (angle - 90.0).abs() < 1e-9 && align == Align::Left);
    // "Return" has no placement of its own: it is printed beside its pin, A2 8 below A1.
    let a2 = pin(s, "A2")?;
    let (at, ..) = text_at(s, "Return")?;
    assert!(near(at, a2.at.x + 1.0, a2.at.y - 1.0), "{at:?} beside {a2:?}");
    // A connection point of its own shows its description 2 right of and 1 above it.
    let (at, ..) = text_at(s, "IN1")?;
    assert!(near(at, 32.0, 9.0), "{at:?}");
    // Placements anchored in a loop are not followed: beside the pin (40, 10).
    let (at, ..) = text_at(s, "Loop")?;
    assert!(near(at, 41.0, 9.0), "{at:?}");
    Ok(())
}

#[test]
fn without_a_shown_device_tag_the_tag_goes_above_the_drawing() -> TestResult {
    let s = read("BES_contactor.ema", COIL)?.symbol;
    let mut pts: Vec<Point> = s.pins.iter().map(|p| p.at).collect();
    for g in &s.graphics {
        let (a, b) = g.bounds();
        pts.extend([a, b]);
    }
    let (min, _) = bounds_of(&pts);
    assert!(near(s.tag_at, min.x, min.y - 2.0), "{:?}", s.tag_at);
    Ok(())
}

#[test]
fn a_description_of_a_point_without_a_pin_is_counted() -> TestResult {
    // A coil has two designations and pins; a third description belongs to no point.
    let xml = SHOWN_TEXTS.replace(
        r#"A185="A1;A2" A186="Coil;Return""#,
        r#"A185="A1;A2" A186="Coil;Return;Spare""#,
    );
    let m = read("coil.ema", &xml)?;
    assert!(text_at(&m.symbol, "Spare").is_err());
    assert_eq!(
        m.left_out.get("connection point descriptions of points without a pin"),
        Some(&1),
        "{:?}",
        m.left_out
    );
    Ok(())
}

#[test]
fn many_descriptions_are_cut_and_too_many_shown_properties_refused() -> TestResult {
    let long = vec!["x"; 2 * MAX_DESIGNATIONS].join(";");
    let xml = SHOWN_TEXTS.replace(r#"A186="IN1""#, &format!(r#"A186="IN1;{long}""#));
    let m = read("coil.ema", &xml)?;
    assert!(text_at(&m.symbol, "IN1").is_ok());
    let kept = variants(&xml)?
        .iter()
        .flat_map(|v| v.placements.iter())
        .find(|p| p.id == "17/51")
        .map(|p| p.descriptions.len());
    assert_eq!(kept, Some(MAX_DESIGNATIONS));

    let many: String = (0..=MAX_PROPS)
        .map(|k| format!(r#"<S53x5 A4="{}" A771="20029" A953="0" A502="17/52/{k}"/>"#, k + 10))
        .collect();
    let xml = SHOWN_TEXTS.replace(
        r#"<S40x1201 A762="90/90"/>"#,
        &format!(r#"{many}<S40x1201 A762="90/90"/>"#),
    );
    let Err(e) = read("coil.ema", &xml) else {
        unreachable!("too many shown properties are refused")
    };
    assert!(e.to_string().contains("properties"), "{e}");
    Ok(())
}

#[test]
fn a_macro_without_a_multi_line_representation_says_what_it_has() {
    let layout = r#"<EplanPxfRoot><MacroVariant MacroFuncType="6" DocumentType="7" ReferencePoint="0/0"/>
        <MacroVariant MacroFuncType="10"/></EplanPxfRoot>"#;
    let e = read("x.ema", layout).map(|m| m.symbol.id).err().map(|e| e.to_string());
    assert_eq!(
        e.as_deref(),
        Some("has no multi-line representation (only panel layout, 3D)")
    );
}

#[test]
fn damaged_macros_are_errors_not_panics() {
    let cases: [(&str, &str); 7] = [
        ("binary macro", "is not an EPLAN macro"),
        ("<other/>", "is not an EPLAN macro (its root is other)"),
        (
            "<EplanPxfRoot><MacroVariant MacroFuncType=\"1\">",
            "ends inside an element",
        ),
        (
            "<EplanPxfRoot><MacroVariant MacroFuncType=\"1\"><O31 A531=\"1/2\"/>",
            "ends inside",
        ),
        ("<EplanPxfRoot></EplanPxfRoot>", "holds no representation"),
        (
            "<EplanPxfRoot><MacroVariant MacroFuncType=\"1\"><O4/></MacroVariant></EplanPxfRoot>",
            "draws nothing",
        ),
        (
            "<EplanPxfRoot><MacroVariant MacroFuncType=\"1\"><O31 A531=\"&bad;\"/></MacroVariant></EplanPxfRoot>",
            "bad value",
        ),
    ];
    for (text, want) in cases {
        let e = read("x.ema", text).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(e.contains(want), "{text}: {e:?}");
    }
}

#[test]
fn objects_whose_numbers_cannot_be_read_are_counted_not_drawn() -> TestResult {
    let text = r#"<EplanPxfRoot><MacroVariant MacroFuncType="1" ReferencePoint="0/0">
        <O31 A531="1/x" A532="2/2"/><O31 A531="NaN/1" A532="2/2"/><O32 A555="1/1" A556="-1/0" A553="0" A554="1"/>
        <O34 A621="1/1"/><O17 A1381="300" A185="1"/>
        <O17 A1381="300" A185="1"><S40x1201/></O17>
        <O31 A531="0/0" A532="1/1"/></MacroVariant></EplanPxfRoot>"#;
    let m = read("x.ema", text)?;
    assert_eq!(m.symbol.graphics.len(), 1);
    assert!(m.symbol.pins.is_empty());
    assert_eq!(m.left_out.get("objects whose numbers could not be read"), Some(&6));
    Ok(())
}

#[test]
fn deep_nesting_is_refused() {
    let mut text = String::from("<EplanPxfRoot><MacroVariant MacroFuncType=\"1\">");
    for _ in 0..200 {
        text.push_str("<O26>");
    }
    let e = read("x.ema", &text).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(e.contains("nested too deeply"), "{e:?}");
}

#[test]
fn text_alignment_runs_left_centre_right_in_rows_of_three() {
    assert_eq!(alignment(1), Align::Left);
    assert_eq!(alignment(5), Align::Center);
    assert_eq!(alignment(12), Align::Right);
    assert_eq!(alignment(0), Align::Left);
}

#[test]
fn pins_of_a_macro_with_several_devices_are_led_by_their_device_tag() -> TestResult {
    let text = r#"<EplanPxfRoot><MacroVariant MacroFuncType="1" ReferencePoint="0/0">
        <O17 A1381="200" A1382="1" A185="A1;A2"><S40x1201 A762="0/0"/><P144 P20013="K" P20014="1"/></O17>
        <O17 A1381="600" A1382="1" A185="13;14"><S40x1201 A762="20/0"/><P144 P20013="S" P20014="2"/></O17>
        <O17 A1381="300" A185="1"><S40x1201 A762="40/0"/></O17>
        </MacroVariant></EplanPxfRoot>"#;
    let s = read("circuit.ema", text)?.symbol;
    let names: Vec<&str> = s.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["K1:A1", "K1:A2", "S2:13", "S2:14", "1"]);
    assert_eq!(s.letter_code, "K");
    assert_eq!(s.role, SymbolRole::Standalone);
    Ok(())
}

/// A macro of `n` device connection points, all named `1`.
fn many_points(n: usize) -> String {
    let mut text = String::from(r#"<EplanPxfRoot><MacroVariant MacroFuncType="1" ReferencePoint="0/0">"#);
    for k in 0..n {
        text.push_str(&format!(r#"<O17 A1381="300" A185="1"><S40x1201 A762="{k}/0"/></O17>"#));
    }
    text.push_str("</MacroVariant></EplanPxfRoot>");
    text
}

#[test]
fn a_large_macro_with_repeated_names_is_read_and_a_larger_one_refused() -> TestResult {
    let s = read("big.ema", &many_points(MAX_PINS))?.symbol;
    let names: std::collections::BTreeSet<&str> = s.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names.len(), MAX_PINS, "every pin name is unique");
    let first: Vec<&str> = s.pins.iter().take(5).map(|p| p.name.as_str()).collect();
    assert_eq!(first, ["1", "1'", "1''", "1'''", "1#4"]);
    let e = read("big.ema", &many_points(2 * MAX_PINS))
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert_eq!(e, format!("holds more than {MAX_PINS} connection points"));
    Ok(())
}

#[test]
fn a_placed_symbol_with_too_many_designations_is_refused() {
    let many = vec!["1"; 20_000].join(";");
    let text = format!(
        r#"<EplanPxfRoot><MacroVariant MacroFuncType="1"><O17 A1381="200" A185="{many}"><S40x1201 A762="0/0"/></O17></MacroVariant></EplanPxfRoot>"#
    );
    let e = read("x.ema", &text).err().map(|e| e.to_string()).unwrap_or_default();
    assert_eq!(
        e,
        format!("places a symbol with more than {MAX_DESIGNATIONS} connection points")
    );
}

#[test]
fn unique_names_mark_then_number_repeats_and_skip_names_already_given() {
    let mut u = UniqueNames::default();
    let got: Vec<String> = ["1", "1'", "1", "1", "1", "1", "1#4", "1"]
        .into_iter()
        .map(|n| u.unique(n.to_owned()))
        .collect();
    assert_eq!(got, ["1", "1'", "1''", "1'''", "1#4", "1#5", "1#4'", "1#6"]);
    // Many repeats: each gets the next number, never a longer run of marks.
    let mut u = UniqueNames::default();
    let names: std::collections::BTreeSet<String> = (0..20_000).map(|_| u.unique("1".into())).collect();
    assert_eq!(names.len(), 20_000);
    assert!(names.iter().all(|n| n.len() <= "1#19999".len()), "names stay short");
}

#[test]
fn a_symbol_id_from_a_long_file_name_is_shortened() -> TestResult {
    let name = format!("{}.ema", "x".repeat(300));
    let text =
        r#"<EplanPxfRoot><MacroVariant MacroFuncType="1"><O31 A531="0/0" A532="1/1"/></MacroVariant></EplanPxfRoot>"#;
    let s = read(&name, text)?.symbol;
    assert_eq!(s.id, format!("eplan.{}", "x".repeat(MAX_STEM)));
    wc_core::store::check_id(&format!("{}_99", s.id))?;
    Ok(())
}

#[test]
fn errors_read_as_a_sentence_and_name_each_representation_once() {
    let bad = r#"<EplanPxfRoot><MacroVariant MacroFuncType="1"><O31 A531="&bad;"/></MacroVariant></EplanPxfRoot>"#;
    let e = read("x.ema", bad).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(e.starts_with("has a bad value of A531: "), "{e:?}");
    let layouts = r#"<EplanPxfRoot><MacroVariant MacroFuncType="6"/><MacroVariant MacroFuncType="10"/>
        <MacroVariant MacroFuncType="6" VariantId="1"/></EplanPxfRoot>"#;
    let e = read("x.ema", layouts).err().map(|e| e.to_string()).unwrap_or_default();
    assert_eq!(e, "has no multi-line representation (only panel layout, 3D)");
}

#[test]
fn texts_of_the_second_kind_are_drawn_location_boxes_counted_and_groups_opened() -> TestResult {
    let text = r#"<EplanPxfRoot><MacroVariant MacroFuncType="1" ReferencePoint="0/0">
        <O132><O83 A501="10/-5" A511="de_DE@Not-Aus;en_US@Emergency stop;"><S54x505 A961="3" A966="10"/></O83></O132>
        <O88 A1631="0/-48" A1632="72/0"/>
        <O17 A1381="301"><S40x1201 A762="0/0"/><P144 P20013="A"/></O17>
        </MacroVariant></EplanPxfRoot>"#;
    let m = read("x.ema", text)?;
    assert!(m.symbol.graphics.contains(&Shape::Text {
        at: Point::new(10.0, 5.0),
        text: "Emergency stop".into(),
        height: 3.0,
        align: Align::Left,
        angle: 0.0,
    }));
    let want: BTreeMap<String, usize> = [("location boxes", 1), ("black boxes without an outline", 1)]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
    assert_eq!(m.left_out, want);
    assert_eq!(m.symbol.letter_code, "A");
    Ok(())
}

#[test]
fn the_letter_is_that_of_the_device_with_the_most_pins() -> TestResult {
    let text = r#"<EplanPxfRoot><MacroVariant MacroFuncType="1" ReferencePoint="0/0">
        <O17 A1381="300" A185="PE"><S40x1201 A762="0/0"/><P144 P20013="PE" P20014="1"/></O17>
        <O17 A1381="200" A1382="1" A185="A1;A2"><S40x1201 A762="20/0"/><P144 P20013="K" P20014="1"/></O17>
        </MacroVariant></EplanPxfRoot>"#;
    assert_eq!(read("circuit.ema", text)?.symbol.letter_code, "K");
    Ok(())
}

#[test]
fn a_macro_without_a_multi_line_variant_a_is_not_read_from_another_variant() {
    let text = r#"<EplanPxfRoot><MacroVariant MacroFuncType="1" VariantId="1"><O31 A531="0/0" A532="1/1"/></MacroVariant></EplanPxfRoot>"#;
    let e = read("x.ema", text).err().map(|e| e.to_string()).unwrap_or_default();
    assert_eq!(e, "has no variant A of its multi-line representation");
}

#[test]
fn coordinates_that_overflow_on_the_way_are_refused() {
    let text = r#"<EplanPxfRoot><MacroVariant MacroFuncType="1" ReferencePoint="-1e308/0"><O31 A531="1e308/0" A532="1/1"/></MacroVariant></EplanPxfRoot>"#;
    let e = read("x.ema", text).err().map(|e| e.to_string()).unwrap_or_default();
    assert_eq!(e, "has coordinates too large to draw");
}

#[test]
fn a_terminal_keeps_the_macros_own_designations() -> TestResult {
    let text = r#"<EplanPxfRoot><MacroVariant MacroFuncType="1" ReferencePoint="0/0">
        <O17 A1381="100" A185="L;R"><S40x1201 A762="0/0"/><P144 P20013="X" P20014="1"/></O17>
        </MacroVariant></EplanPxfRoot>"#;
    let s = read("x.ema", text)?.symbol;
    let names: Vec<&str> = s.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["L", "R"]);
    Ok(())
}

#[test]
fn many_representation_kinds_are_named_once_and_summed_up_past_a_few() {
    let mut text = String::from("<EplanPxfRoot>");
    for k in 0..20 {
        text.push_str(&format!(
            r#"<MacroVariant MacroFuncType="{}"/><MacroVariant MacroFuncType="{}"/>"#,
            100 + k,
            100 + k
        ));
    }
    text.push_str("</EplanPxfRoot>");
    let e = read("x.ema", &text).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(e.ends_with("type 107, 12 more)"), "{e:?}");
    assert_eq!(e.matches("type 100").count(), 1, "{e:?}");
}

#[test]
fn an_overlong_device_tag_is_not_taken_as_one() -> TestResult {
    let long = "K".repeat(MAX_TAG + 1);
    let text = format!(
        r#"<EplanPxfRoot><MacroVariant MacroFuncType="1"><O17 A1381="300" A185="1"><S40x1201 A762="0/0"/><P144 P20013="{long}" P20014="1"/></O17></MacroVariant></EplanPxfRoot>"#
    );
    let s = read("x.ema", &text)?.symbol;
    assert_eq!(s.letter_code, "");
    assert_eq!(s.pins.first().map(|p| p.name.as_str()), Some("1"));
    Ok(())
}

/// A PLC module whose multi-line variant A holds terminals 1 and 2 and variant B terminals 2
/// to 4 (or only 2), and whose overview lists 1 to 5 on one placement.
fn split_module(with_b: bool) -> String {
    split_module_with(if with_b { &["2", "3", "4"] } else { &[] })
}

fn split_module_with(b_names: &[&str]) -> String {
    let point = |x: u32, name: &str| {
        format!(r#"<O17 A1381="300" A185="{name}"><S40x1201 A762="{x}/0"/><P144 P20013="A" P20014="1"/></O17>"#)
    };
    let b = if b_names.is_empty() {
        String::new()
    } else {
        let points: String = b_names.iter().zip(0u32..).map(|(n, k)| point(8 * k, n)).collect();
        format!(r#"<MacroVariant MacroFuncType="1" VariantId="1" ReferencePoint="0/0">{points}</MacroVariant>"#)
    };
    format!(
        r#"<EplanPxfRoot>
        <MacroVariant MacroFuncType="4" VariantId="0" ReferencePoint="0/0">
         <O17 A1381="200" A1382="1" A185="1;2;3;4;5"><S40x1201 A762="0/0"/><P144 P20013="A" P20014="1"/></O17>
        </MacroVariant>
        <MacroVariant MacroFuncType="1" VariantId="0" ReferencePoint="0/0">{}{}</MacroVariant>
        {b}
        </EplanPxfRoot>"#,
        point(0, "1"),
        point(8, "2")
    )
}

#[test]
fn terminals_in_other_multi_line_variants_complete_the_symbol() -> TestResult {
    let m = read("plc.ema", &split_module(true))?;
    let s = &m.symbol;
    let names: Vec<&str> = s.pins.iter().map(|p| p.name.as_str()).collect();
    // Variant B's 2 is variant A's and is not repeated; the overview is not needed.
    assert_eq!(names, ["1", "2", "3", "4"]);
    // B's points go in a column a grid step right of the drawing, facing right.
    let p = pin(s, "3")?;
    assert!(near(p.at, 12.0, 0.0) && p.dir == PinDir::Right, "{p:?}");
    let p = pin(s, "4")?;
    assert!(near(p.at, 12.0, 4.0) && p.dir == PinDir::Right, "{p:?}");
    // Variant B gave pins and is not left out; the overview gave none and is.
    let want: BTreeMap<String, usize> = [
        ("overview representations", 1),
        ("library graphics of connection points (pins kept)", 2),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    assert_eq!(m.left_out, want);
    Ok(())
}

#[test]
fn without_other_variants_the_overview_completes_the_symbol() -> TestResult {
    let m = read("plc.ema", &split_module(false))?;
    let names: Vec<&str> = m.symbol.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["1", "2", "3", "4", "5"]);
    let want: BTreeMap<String, usize> = [("library graphics of connection points (pins kept)".to_owned(), 2)]
        .into_iter()
        .collect();
    assert_eq!(m.left_out, want, "the overview is not left out");
    Ok(())
}

#[test]
fn a_variant_with_only_variant_as_points_leaves_the_overview_to_complete_the_symbol() -> TestResult {
    let m = read("plc.ema", &split_module_with(&["2"]))?;
    let names: Vec<&str> = m.symbol.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["1", "2", "3", "4", "5"]);
    // Variant B gave nothing new and is left out; the overview gave pins.
    assert_eq!(
        m.left_out.get("other variants of the multi-line representation"),
        Some(&1)
    );
    assert_eq!(m.left_out.get("overview representations"), None);
    Ok(())
}

#[test]
fn points_from_other_variants_count_toward_the_most_a_macro_may_hold() {
    let mut text = many_points(MAX_PINS).replace("</EplanPxfRoot>", "");
    text.push_str(r#"<MacroVariant MacroFuncType="1" VariantId="1" ReferencePoint="0/0">"#);
    text.push_str(r#"<O17 A1381="300" A185="new"><S40x1201 A762="0/0"/></O17>"#);
    text.push_str("</MacroVariant></EplanPxfRoot>");
    let e = read("big.ema", &text).err().map(|e| e.to_string()).unwrap_or_default();
    assert_eq!(e, format!("holds more than {MAX_PINS} connection points"));
}

#[test]
fn a_terminal_designation_without_a_pin_in_variant_a_comes_from_another_variant() -> TestResult {
    // Variant A draws terminal X1 with designations 1;2;3, but the terminal symbol has two
    // pins; variant B holds 3 as a connection point of its own.
    let text = r#"<EplanPxfRoot>
        <MacroVariant MacroFuncType="1" VariantId="0" ReferencePoint="0/0">
         <O17 A1381="100" A185="1;2;3"><S40x1201 A762="0/0"/><P144 P20013="X" P20014="1"/></O17>
        </MacroVariant>
        <MacroVariant MacroFuncType="1" VariantId="1" ReferencePoint="0/0">
         <O17 A1381="300" A185="3"><S40x1201 A762="0/0"/><P144 P20013="X" P20014="1"/></O17>
        </MacroVariant></EplanPxfRoot>"#;
    let s = read("x.ema", text)?.symbol;
    let names: Vec<&str> = s.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["1", "2", "3"]);
    Ok(())
}

#[test]
fn a_terminal_gets_a_pin_per_designation_and_a_fuse_is_drawn_as_fuses() -> TestResult {
    let terminal = r#"<EplanPxfRoot Name="X">
 <MacroVariant VariantId="0" ReferencePoint="50/100" MacroFuncType="1" DocumentType="1">
  <ESymLib Name="IEC_symbol" ID="1"/>
  <O17 A1261="1" A1262="30" A1263="0" A1381="100" A1382="1" A185="1;2;3;4">
   <S40x1201 A762="60/90"/>
   <P144 P20013="X" P20014="1"/>
  </O17>
 </MacroVariant>
</EplanPxfRoot>"#;
    let m = read("terminal4.ema", terminal)?;
    let s = &m.symbol;
    assert_eq!(s.pins.len(), 4, "{:?}", s.pins);
    // In pairs, top and bottom, a grid step apart; the macro's designations kept.
    for (name, x, y, dir) in [
        ("1", 10.0, 9.0, PinDir::Up),
        ("2", 10.0, 11.0, PinDir::Down),
        ("3", 14.0, 9.0, PinDir::Up),
        ("4", 14.0, 11.0, PinDir::Down),
    ] {
        let p = pin(s, name)?;
        assert!(near(p.at, x, y) && p.dir == dir, "{name}: {p:?}");
    }
    assert_eq!(s.role, SymbolRole::Terminal);

    let fuse = r#"<EplanPxfRoot Name="F">
 <MacroVariant VariantId="0" ReferencePoint="50/100" MacroFuncType="1" DocumentType="1">
  <ESymLib Name="IEC_symbol" ID="1"/>
  <O17 A1261="1" A1262="50" A1263="0" A1381="502" A1382="1" A185="1;2;3;4">
   <S40x1201 A762="60/90"/>
   <P144 P20013="F" P20014="1"/>
  </O17>
 </MacroVariant>
</EplanPxfRoot>"#;
    let m = read("fuse2.ema", fuse)?;
    let s = &m.symbol;
    assert_eq!(s.pins.len(), 4);
    for (name, x, y, dir) in [
        ("1", 10.0, 10.0, PinDir::Up),
        ("2", 10.0, 18.0, PinDir::Down),
        ("3", 14.0, 10.0, PinDir::Up),
        ("4", 14.0, 18.0, PinDir::Down),
    ] {
        let p = pin(s, name)?;
        assert!(near(p.at, x, y) && p.dir == dir, "{name}: {p:?}");
    }
    // One fuse body per pole, not a box round all of them.
    let bodies = s
        .graphics
        .iter()
        .filter(|g| matches!(g, Shape::Rect { min, max } if (max.x - min.x - 2.0).abs() < 1e-9))
        .count();
    assert_eq!(bodies, 2, "{:?}", s.graphics);

    // A one-pole fuse whose points are named like a coil's is still a fuse.
    let m = read("fuse1.ema", &fuse.replace("1;2;3;4", "A1;A2"))?;
    assert_eq!(m.symbol.role, SymbolRole::Standalone, "not a coil");
    assert!(near(pin(&m.symbol, "A2")?.at, 10.0, 18.0));
    Ok(())
}

#[test]
fn a_single_points_shown_description_is_its_first_and_formatting_stays_with_its_own_property() -> TestResult {
    // A second description of a single point has no point of its own: counted, not drawn over
    // the first. A property placement that is not read keeps its formatting to itself.
    let xml = SHOWN_TEXTS.replace(r#"A186="IN1""#, r#"A186="IN1;Spare""#).replace(
        r#"<S53x5 A4="2" R7="17/51" A771="20010" A953="0" A501="-50/-50"/>"#,
        r#"<S53x5 A4="2" R7="17/51" A771="x" A953="0"><S54x505 A961="9" A962="1"/></S53x5>"#,
    );
    let m = read("coil.ema", &xml)?;
    let (at, height, angle, _) = text_at(&m.symbol, "IN1")?;
    assert!(near(at, 32.0, 9.0), "{at:?}");
    assert!(
        (height - TEXT_HEIGHT).abs() < 1e-9 && angle.abs() < 1e-9,
        "{height} {angle}"
    );
    assert!(text_at(&m.symbol, "Spare").is_err());
    assert_eq!(
        m.left_out.get("connection point descriptions of points without a pin"),
        Some(&1),
        "{:?}",
        m.left_out
    );
    Ok(())
}

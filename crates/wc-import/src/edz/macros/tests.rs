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

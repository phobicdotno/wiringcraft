//! Tests on synthetic tables written to the layout described in the module docs. No real
//! project files are used.
#![allow(clippy::unwrap_used)]

use super::*;

fn u16le(v: u16) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

fn reference(class: u16, id: u32) -> Vec<u8> {
    let mut v = u16le(class);
    v.extend(id.to_le_bytes());
    v
}

/// A record: padding as found between records, then `ff ff`, the own reference and the body.
fn record(class: u16, id: u32, body: &[u8]) -> Vec<u8> {
    let mut v = vec![0u8; 24];
    v.extend([0x0a, 0, 0, 0, 0x38, 0, 0xff, 0xff]);
    v.extend(u16le(60001));
    v.extend(reference(class, id));
    v.extend(body);
    v
}

fn block(marker: u16, entries: &[(u16, u16, &str)]) -> Vec<u8> {
    let mut v = u16le(marker);
    for (id, idx, s) in entries {
        v.extend(u16le(*id));
        v.extend(u16le(*idx));
        v.extend(s.as_bytes());
        v.push(0);
    }
    v.extend([0xff, 0xff]);
    v
}

fn text(field: u16, s: &str) -> Vec<u8> {
    let mut v = u16le(field);
    v.extend(s.as_bytes());
    v.push(0);
    v
}

fn ref_field_bytes(field: u16, class: u16, id: u32) -> Vec<u8> {
    let mut v = u16le(field);
    v.extend(reference(class, id));
    v
}

fn point(field: u16, x: f64, y: f64) -> Vec<u8> {
    let mut v = u16le(field);
    v.extend(x.to_le_bytes());
    v.extend(y.to_le_bytes());
    v
}

fn function(id: u32, page: u32, tag: &[(u16, u16, &str)], props: &[(u16, u16, &str)], at: (f64, f64)) -> Vec<u8> {
    let mut body = ref_field_bytes(F_OWNER, CLASS_PAGE, page);
    body.extend(point(F_POSITION, at.0, at.1));
    body.extend(block(F_PROPERTIES, props));
    body.extend(block(F_TAG_PARTS, tag));
    record(CLASS_FUNCTION, id, &body)
}

fn connection(page: u32, from: &str, to: &str, props: &[(u16, u16, &str)], cable: Option<(&str, &str)>) -> Vec<u8> {
    let mut body = ref_field_bytes(F_CONNECTION_PAGE, CLASS_PAGE, page);
    if let Some((a, b)) = cable {
        body.extend(text(F_TAG_ID, a));
        body.extend(text(F_TAG_COUNTER, b));
    }
    body.extend(text(F_CONNECTION_FROM, from));
    body.extend(text(F_CONNECTION_TO, to));
    body.extend(block(F_PROPERTIES, props));
    record(CLASS_CONNECTION, 900, &body)
}

pub(crate) fn sample() -> Tables {
    let mut t = Tables::new();
    let mut pages = Vec::new();
    for (id, name, desc) in [
        (20, "2", "??_??@CONTROL;"),
        (10, "1", "de_DE@Hauptstrom;en_US@MAIN CIRCUIT;"),
    ] {
        let mut body = text(F_PAGE_NAME, name);
        body.extend(block(
            F_PROPERTIES,
            &[(P_PAGE_DESCRIPTION, 0, desc), (P_PAGE_SCALE, 0, "1")],
        ));
        pages.extend(record(CLASS_PAGE, id, &body));
    }
    t.insert("page.eod".into(), pages);
    let mut f = Vec::new();
    f.extend(function(
        1,
        10,
        &[(P_TAG_ID, 0, "K"), (P_TAG_COUNTER, 0, "1")],
        &[
            (P_FUNCTION_TEXT, 0, "??_??@PUMP;"),
            (P_TECHNICAL, 0, "230 V"),
            (P_PART, 1, "ACME.R1"),
            (P_PART, 2, "ACME.S1"),
        ],
        (100.0, 50.0),
    ));
    f.extend(function(
        2,
        20,
        &[(P_TAG_ID, 0, "K"), (P_TAG_COUNTER, 0, "1")],
        &[],
        (40.0, 60.0),
    ));
    f.extend(function(
        3,
        20,
        &[
            (P_TAG_ID, 0, "PUMP"),
            (P_TAG_COUNTER, 0, "1"),
            (P_TAG_SUB_ID, 0, "X"),
            (P_TAG_SUB_COUNTER, 0, "2"),
        ],
        &[],
        (60.0, 60.0),
    ));
    // A connection point record of another class is ignored.
    f.extend(record(59, 4, &block(F_PROPERTIES, &[(31005, 0, "-16002")])));
    t.insert("function.eod".into(), f);
    let mut c = Vec::new();
    c.extend(connection(
        10,
        "K1:A1",
        "PUMP1-X2:3:a",
        &[(P_WIRE_CROSS_SECTION, 0, "1,5"), (P_WIRE_COLOUR, 0, "BK")],
        Some(("W", "1")),
    ));
    c.extend(connection(20, "K1:13", "S9:14", &[], None));
    c.extend(connection(20, "L", "PE", &[], None));
    c.extend(record(CLASS_CONNECTION, 901, &[0, 0, 0]));
    t.insert("connection.eod".into(), c);
    let mut a = Vec::new();
    a.extend(record(
        CLASS_ARTICLE,
        5,
        &block(
            F_PROPERTIES,
            &[
                (P_ARTICLE_NUMBER, 0, "ACME.R1"),
                (P_ARTICLE_TYPE, 0, "R-1"),
                (P_ARTICLE_DESCRIPTION, 0, "??_??@Relay;"),
                (P_ARTICLE_MANUFACTURER, 0, "Acme"),
            ],
        ),
    ));
    t.insert("article.eod".into(), a);
    let mut ar = text(F_TAG_ID, "S");
    ar.extend(text(F_TAG_COUNTER, "9"));
    ar.extend(text(F_PART_NUMBER, "OTHER.B2"));
    t.insert("articleref.eod".into(), record(CLASS_ARTICLE_REF, 6, &ar));
    t.insert(
        "projectinfo.xml".into(),
        br#"<?xml version="1.0"?><Properties><Property name="Company name" id="10015" index="0" type="7">Customer Ltd</Property><Property name="Project description" id="10011" index="0" type="15">??_??@Tank;</Property></Properties>"#.to_vec(),
    );
    t
}

fn f64_bytes(field: u16, v: f64) -> Vec<u8> {
    let mut b = u16le(field);
    b.extend(v.to_le_bytes());
    b
}

fn u16_bytes(field: u16, v: u16) -> Vec<u8> {
    let mut b = u16le(field);
    b.extend(u16le(v));
    b
}

/// The symbol number, then the variant byte when it is not 0, as Eplan writes them.
fn symbol_bytes(number: u16, variant: u8) -> Vec<u8> {
    let mut b = u16_bytes(F_SYMBOL_NUMBER, number);
    if variant > 0 {
        b.extend(u16le(F_VARIANT));
        b.push(variant);
    }
    b
}

struct Drawn<'a> {
    id: u32,
    tag: (&'a str, &'a str),
    pins: &'a [&'a str],
    kind: (u16, u16),
    variant: u8,
    at: (f64, f64),
    terminal: &'a str,
}

fn drawn_function(page: u32, d: &Drawn) -> Vec<u8> {
    let mut body = ref_field_bytes(F_OWNER, CLASS_PAGE, page);
    for p in d.pins {
        body.extend(text(F_PINS, p));
    }
    body.extend(point(F_POSITION, d.at.0, d.at.1));
    body.extend(symbol_bytes(1000 + d.id as u16, d.variant));
    body.extend(u16_bytes(F_FUNCTION_KIND, d.kind.0));
    body.extend(u16_bytes(F_FUNCTION_DETAIL, d.kind.1));
    let props: Vec<(u16, u16, &str)> = if d.terminal.is_empty() {
        vec![]
    } else {
        vec![(P_TERMINAL, 0, d.terminal)]
    };
    body.extend(block(F_PROPERTIES, &props));
    body.extend(block(
        F_TAG_PARTS,
        &[(P_TAG_ID, 0, d.tag.0), (P_TAG_COUNTER, 0, d.tag.1)],
    ));
    record(CLASS_FUNCTION, d.id, &body)
}

/// The sample plus a schematic page 3 drawn with devices, a wire corner and free graphics.
fn drawn_sample() -> Tables {
    let mut t = sample();
    let mut body = text(F_PAGE_NAME, "3");
    body.extend(block(
        F_PROPERTIES,
        &[(P_PAGE_DESCRIPTION, 0, "??_??@MOTOR CONTROL;"), (P_PAGE_SCALE, 0, "1")],
    ));
    if let Some(pages) = t.get_mut("page.eod") {
        pages.extend(record(CLASS_PAGE, 30, &body));
    }
    let drawn = [
        // A make contact above a coil: the contact's bottom pin faces the coil's top pin.
        Drawn {
            id: 11,
            tag: ("S", "1"),
            pins: &["13", "14"],
            kind: (drawing::KIND_BUTTON_CONTACT, 1),
            variant: 0,
            at: (100.0, 220.0),
            terminal: "",
        },
        Drawn {
            id: 12,
            tag: ("K", "5"),
            pins: &["A1", "A2"],
            kind: (0, 0),
            variant: 0,
            at: (100.0, 200.0),
            terminal: "",
        },
        // A changeover contact turned a quarter (variant 1), and a terminal.
        Drawn {
            id: 13,
            tag: ("K", "5"),
            pins: &["22", "21", "24"],
            kind: (203, 1),
            variant: 1,
            at: (200.0, 200.0),
            terminal: "",
        },
        Drawn {
            id: 14,
            tag: ("X", "1"),
            pins: &["", "", "", ""],
            kind: (drawing::KIND_TERMINAL, 6),
            variant: 0,
            at: (150.0, 100.0),
            terminal: "7",
        },
    ];
    if let Some(f) = t.get_mut("function.eod") {
        for d in &drawn {
            f.extend(drawn_function(30, d));
        }
    }
    let on_page = |page: u32| ref_field_bytes(F_GRAPHIC_PAGE, CLASS_PAGE, page);
    let mut inst = Vec::new();
    let mut line = on_page(30);
    line.extend(point(F_LINE_FROM, 10.0, 20.0));
    line.extend(point(F_LINE_TO, 50.0, 20.0));
    inst.extend(record(CLASS_LINE, 101, &line));
    // A line of the page frame (owned by a form, class 51) is not the page's own.
    let mut frame = ref_field_bytes(F_GRAPHIC_PAGE, 51, 2);
    frame.extend(point(F_LINE_FROM, 0.0, 0.0));
    frame.extend(point(F_LINE_TO, 400.0, 0.0));
    inst.extend(record(CLASS_LINE, 102, &frame));
    let mut label = on_page(30);
    label.extend(point(F_TEXT_AT, 20.0, 40.0));
    label.extend(text(F_TEXT, "??_??@FIRST\nSECOND;"));
    label.extend(f64_bytes(F_TEXT_HEIGHT, 2.0));
    inst.extend(record(CLASS_TEXT, 103, &label));
    // A text turned a quarter (radians in the table), on its own page so the counts above
    // stay as they are.
    let mut turned = on_page(31);
    turned.extend(point(F_TEXT_AT, 30.0, 50.0));
    turned.extend(text(F_TEXT, "??_??@UP;"));
    turned.extend(f64_bytes(F_TEXT_ANGLE, std::f64::consts::FRAC_PI_2));
    inst.extend(record(CLASS_TEXT, 107, &turned));
    let mut circle = on_page(30);
    circle.extend(point(F_CIRCLE_CENTER, 60.0, 60.0));
    circle.extend(f64_bytes(F_CIRCLE_RADIUS, 3.0));
    inst.extend(record(CLASS_CIRCLE, 104, &circle));
    // A corner above the contact, joining right and down, and a node of an unknown kind to its right.
    let mut corner = on_page(30);
    corner.extend(point(F_POSITION, 100.0, 240.0));
    corner.extend(symbol_bytes(70, 0));
    inst.extend(record(CLASS_NODE, 105, &corner));
    let mut unknown = on_page(30);
    unknown.extend(point(F_POSITION, 300.0, 240.0));
    unknown.extend(symbol_bytes(999, 0));
    inst.extend(record(CLASS_NODE, 106, &unknown));
    t.insert("instance.eod".into(), inst);
    t
}

#[test]
fn reads_symbols_variants_and_page_graphics() {
    let p = read(&drawn_sample()).unwrap();
    let co = p.functions.iter().find(|f| f.pins.len() == 3).unwrap();
    assert_eq!((co.tag.as_str(), co.variant, co.kind, co.detail), ("K5", 1, 203, 1));
    assert_eq!(co.pins, ["22", "21", "24"]);
    let x = p.functions.iter().find(|f| f.tag == "X1").unwrap();
    assert_eq!((x.terminal.as_str(), x.variant, x.pins.len()), ("7", 0, 4));
    // The frame line is left out; the text is split into its lines.
    assert_eq!(p.graphics.iter().filter(|g| g.page == 30).count(), 4);
    let up = p.graphics.iter().find(|g| g.page == 31).unwrap();
    assert!(matches!(&up.shape, wc_core::geom::Shape::Text { angle, .. } if *angle == 90.0));
    let texts: Vec<(&str, f64)> = p
        .graphics
        .iter()
        .filter(|g| g.page == 30)
        .filter_map(|g| match &g.shape {
            wc_core::geom::Shape::Text { text, at, .. } => Some((text.as_str(), at.y)),
            _ => None,
        })
        .collect();
    assert_eq!(texts, [("FIRST", 40.0), ("SECOND", 37.0)]);
    assert_eq!(p.nodes.len(), 2);
    assert_eq!((p.nodes[0].symbol, p.nodes[0].at), (70, Point::new(100.0, 240.0)));
}

#[test]
fn rebuilds_drawn_pages_at_original_positions() {
    use wc_core::geom::Rotation;
    let p = read(&drawn_sample()).unwrap();
    let (b, warnings) = build(&p, 100);
    let titles: Vec<&str> = b.pages.iter().map(|pg| pg.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Eplan page 3: MOTOR CONTROL",
            "Connections, Eplan page 1: MAIN CIRCUIT",
            "Connections, Eplan page 2: CONTROL"
        ]
    );
    assert_eq!((b.pages[0].number.as_str(), b.pages[1].number.as_str()), ("100", "101"));
    let page = &b.pages[0];
    let placed = |tag: &str| -> Vec<&wc_core::PlacedSymbol> {
        page.symbols
            .iter()
            .filter(|s| s.tag.as_ref().is_some_and(|t| t.to_string() == tag))
            .collect()
    };
    // Eplan's origin is bottom left with y up; WiringCraft's is top left with y down.
    let coil = placed("-K5").into_iter().find(|s| s.rotation == Rotation::R0).unwrap();
    assert_eq!(coil.at, Point::new(110.0, 93.0));
    let changeover = placed("-K5").into_iter().find(|s| s.rotation != Rotation::R0).unwrap();
    assert_eq!(changeover.rotation, Rotation::R270);
    assert_eq!(placed("-S1").len(), 1);
    assert_eq!(placed("-X1.7").len(), 1);
    let sym = |placed: &wc_core::PlacedSymbol| b.symbols.iter().find(|s| s.id == placed.symbol).unwrap();
    assert_eq!(sym(coil).role, wc_core::symbol::SymbolRole::Main);
    assert_eq!(sym(changeover).role, wc_core::symbol::SymbolRole::Auxiliary);
    let pins: Vec<&str> = sym(changeover).pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(pins, ["22", "21", "24"]);
    // Wires: corner down to the contact, contact down to the coil, and the corner right to the
    // node of an unknown kind, which joins whatever lines up with it.
    assert_eq!(page.wires.len(), 3);
    let s1 = &placed("-S1")[0].id;
    let joins = |a: &wc_core::Endpoint, b: &wc_core::Endpoint| {
        page.wires
            .iter()
            .any(|w| (&w.from, &w.to) == (a, b) || (&w.from, &w.to) == (b, a))
    };
    let pin = |item: &str, pin: &str| wc_core::Endpoint::Pin {
        item: item.to_owned(),
        pin: pin.to_owned(),
    };
    assert!(joins(&pin(s1, "14"), &pin(&coil.id, "A1")));
    assert!(joins(
        &wc_core::Endpoint::Point {
            at: Point::new(110.0, 53.0)
        },
        &pin(s1, "13")
    ));
    assert!(joins(
        &wc_core::Endpoint::Point {
            at: Point::new(110.0, 53.0)
        },
        &wc_core::Endpoint::Point {
            at: Point::new(310.0, 53.0)
        }
    ));
    // Free graphics, moved the same way.
    assert!(page.graphics.contains(&wc_core::geom::Shape::Line {
        a: Point::new(20.0, 273.0),
        b: Point::new(60.0, 273.0)
    }));
    // Drawn devices keep their device data and stay off the connection overview.
    assert!(b.devices.contains_key(&DeviceTag::parse("-K5").unwrap()));
    assert!(b.pages[1..].iter().all(|pg| placed_tags(pg).iter().all(|t| t != "-K5")));
    assert!(warnings.iter().any(|w| w.contains("1 wire corner")));
}

fn placed_tags(pg: &wc_core::Page) -> Vec<String> {
    pg.symbols
        .iter()
        .filter_map(|s| s.tag.as_ref().map(ToString::to_string))
        .collect()
}

#[test]
fn wire_nodes_turn_with_their_variant() {
    use drawing::node_directions;
    assert_eq!(node_directions(70, 0), Some([false, true, true, false]));
    assert_eq!(node_directions(70, 1), Some([true, true, false, false]));
    assert_eq!(node_directions(70, 3), Some([false, false, true, true]));
    assert_eq!(node_directions(66, 1), Some([true, true, true, false]));
    assert_eq!(node_directions(5, 0), None);
}

#[test]
fn reads_pages_devices_articles_and_connections() {
    let p = read(&sample()).unwrap();
    assert_eq!(p.properties[0], (10015, "Company name".into(), "Customer Ltd".into()));
    assert_eq!(p.properties[1].2, "Tank");
    let pages: Vec<(&str, &str)> = p
        .pages
        .iter()
        .map(|g| (g.name.as_str(), g.description.as_str()))
        .collect();
    assert_eq!(pages, [("1", "MAIN CIRCUIT"), ("2", "CONTROL")]);
    assert_eq!(p.functions.len(), 3);
    let k1 = &p.functions[0];
    assert_eq!(
        (k1.tag.as_str(), k1.page, k1.text.as_str(), k1.technical.as_str()),
        ("K1", Some(10), "PUMP", "230 V")
    );
    assert_eq!(k1.at, Some(Point::new(100.0, 50.0)));
    assert_eq!(k1.parts, ["ACME.R1", "ACME.S1"]);
    assert_eq!(p.functions[2].tag, "PUMP1-X2");
    assert_eq!(p.article_refs, [("S9".to_owned(), "OTHER.B2".to_owned())]);
    assert_eq!(p.articles[0].manufacturer, "Acme");
    assert_eq!(p.connections.len(), 3);
    let c = &p.connections[0];
    assert_eq!(
        (
            c.from.as_str(),
            c.to.as_str(),
            c.cross_section.as_str(),
            c.colour.as_str(),
            c.cable.as_str()
        ),
        ("K1:A1", "PUMP1-X2:3:a", "1.5", "BK", "W1")
    );
    assert!(p.warnings.iter().any(|w| w.contains("1 connection")));
}

#[test]
fn builds_overview_pages_per_eplan_page() {
    let p = read(&sample()).unwrap();
    let (b, warnings) = build(&p, 100);
    let titles: Vec<&str> = b.pages.iter().map(|pg| pg.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Connections, Eplan page 1: MAIN CIRCUIT",
            "Connections, Eplan page 2: CONTROL"
        ]
    );
    assert_eq!(b.pages[0].number, "100");
    // K1 is first drawn on page 1; PUMP1-X2 and S9 on page 2.
    let on = |i: usize| -> Vec<String> {
        b.pages[i]
            .symbols
            .iter()
            .filter_map(|s| s.tag.as_ref().map(ToString::to_string))
            .collect()
    };
    assert_eq!(on(0), ["-K1"]);
    assert_eq!(on(1), ["-PUMP1.X2", "-S9"]);
    let k1 = b.devices.get(&DeviceTag::parse("-K1").unwrap()).unwrap();
    assert_eq!(k1.part.as_deref(), Some("acme.acme-r1"));
    assert_eq!(
        k1.properties.get("Additional parts").map(String::as_str),
        Some("ACME.S1")
    );
    assert_eq!(k1.properties.get("Function text").map(String::as_str), Some("PUMP"));
    let s9 = b.devices.get(&DeviceTag::parse("-S9").unwrap()).unwrap();
    assert_eq!(s9.part.as_deref(), Some("other.other-b2"));
    // K1's symbol has both pins used by connections.
    let sym = b.symbols.iter().find(|s| s.name.starts_with("K1")).unwrap();
    let pins: Vec<&str> = sym.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(pins, ["13", "A1"]);
    // Both wires cross pages, so each becomes two stubs; the pin-less one is reported.
    let wires: usize = b.pages.iter().map(|pg| pg.wires.len()).sum();
    assert_eq!(wires, 4);
    assert!(warnings.iter().any(|w| w.contains("1 connection")));
    let specced: Vec<&wc_core::Wire> = b
        .pages
        .iter()
        .flat_map(|pg| &pg.wires)
        .filter(|w| w.spec.colour.as_deref() == Some("BK"))
        .collect();
    assert_eq!(specced.len(), 2, "both stubs of the connection");
    for w in specced {
        assert_eq!(w.spec.section.as_deref(), Some("1.5 mm²"), "a plain number is mm²");
        assert_eq!(w.number, None, "colour and section are not a wire number");
    }
}

#[test]
fn texts_tags_and_elk() {
    assert_eq!(pick_language("de_DE@Titelblatt;en_US@Front page;"), "Front page");
    assert_eq!(pick_language("??_??@A;B;"), "A;B");
    assert_eq!(pick_language("plain"), "plain");
    assert_eq!(device_tag("PS1-24VDC-NO").unwrap().to_string(), "-PS1.24VDC.NO");
    assert_eq!(device_tag("A/B").unwrap().to_string(), "-A_B");
    assert!(device_tag("-").is_none());
    assert_eq!(split_end("X2:3:b"), ("X2", Some("3:b")));
    assert_eq!(split_end("PE"), ("PE", None));
    let mut elk = vec![0xff, 0xfe];
    for u in "My project.edb\r\n12.0\r\n".encode_utf16() {
        elk.extend(u.to_le_bytes());
    }
    assert_eq!(elk_target(&elk).as_deref(), Some("My project.edb"));
    assert_eq!(elk_target(b"../evil\n"), None);
}

#[test]
fn hostile_tables_never_panic() {
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let bases = [sample(), drawn_sample()];
    for round in 0..600 {
        let mut t = bases[round % 2].clone();
        for data in t.values_mut() {
            for _ in 0..(round % 7 + 1) {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                if data.is_empty() {
                    break;
                }
                let at = (seed as usize) % data.len();
                data[at] = (seed >> 32) as u8;
                if round % 5 == 0 {
                    data.truncate(at);
                }
            }
        }
        if let Ok(p) = read(&t) {
            let _ = build(&p, 1);
        }
    }
    assert!(read(&Tables::new()).is_err());
    assert!(tables_from_zip(b"not a zip").is_err());
}

#[test]
fn timed_contacts_read_their_pin_numbers() {
    let contact = |pins: &[&str]| {
        let f = EplanFunction {
            tag: "-K10".into(),
            kind: drawing::KIND_CONTACTS.start,
            pins: pins.iter().map(|p| (*p).to_owned()).collect(),
            ..Default::default()
        };
        drawing::symbol_for(&f).unwrap()
    };
    // A timed changeover lists 16, 18, 15 as often as 16, 15, 18; either way 16 is the break, 15
    // the common and 18 the make.
    for order in [["16", "18", "15"], ["16", "15", "18"], ["15", "16", "18"]] {
        let s = contact(&order);
        let at = |name: &str| s.pins.iter().find(|p| p.name == name).map(|p| p.at).unwrap();
        assert_eq!(s.id, "eplan.changeover.16-15-18", "{order:?}");
        assert_eq!(at("16"), Point::new(0.0, 0.0));
        assert_eq!(at("15").y, 8.0);
        assert_eq!(at("18"), Point::new(4.0, 0.0));
    }
    assert!(contact(&["15", "16"]).id.starts_with("eplan.break."));
    assert!(contact(&["17", "18"]).id.starts_with("eplan.make."));
    assert!(contact(&["11", "12"]).id.starts_with("eplan.break."));
}

#[test]
fn drawn_wires_get_the_section_and_colour_of_their_connection() {
    use wc_core::geom::Point;
    use wc_core::{Endpoint, Page, Wire};
    let pt = |x: f64, y: f64| Endpoint::Point { at: Point::new(x, y) };
    let pin = |item: &str, pin: &str| Endpoint::Pin {
        item: item.into(),
        pin: pin.into(),
    };
    let wire = |id: &str, from: Endpoint, to: Endpoint| Wire {
        id: id.into(),
        from,
        to,
        via: vec![],
        number: None,
        potential: None,
        cable: None,
        route: Default::default(),
        spec: Default::default(),
    };
    let mut page = Page::new("p1", "1", "");
    // K1:21 to terminal X2.3 through two junctions, and an unrelated wire from K1:22.
    page.wires = vec![
        wire("w1", pin("s1", "21"), pt(10.0, 0.0)),
        wire("w2", pt(10.0, 0.0), pt(20.0, 0.0)),
        wire("w3", pt(20.0, 0.0), pin("s2", "1")),
        wire("w4", pin("s1", "22"), pt(50.0, 50.0)),
    ];
    let owners = vec![
        ("s1".to_owned(), "K1".to_owned(), String::new()),
        ("s2".to_owned(), "X2".to_owned(), "3".to_owned()),
    ];
    let conn = |from: &str, to: &str, section: &str, colour: &str| EplanConnection {
        from: from.into(),
        to: to.into(),
        cross_section: section.into(),
        colour: colour.into(),
        ..Default::default()
    };
    let connections = vec![
        conn("K1:21", "X2:3:b", "1.5", "BU"),
        // A second connection over the same wires does not overwrite the first.
        conn("X2:3", "K1:21", "2.5", "RD"),
    ];
    assert_eq!(drawing::spec_from_connections(&mut page, &owners, &connections), 3);
    for w in &page.wires[..3] {
        assert_eq!(w.spec.section.as_deref(), Some("1.5 mm²"), "{}", w.id);
        assert_eq!(w.spec.colour.as_deref(), Some("BU"), "{}", w.id);
    }
    assert!(page.wires[3].spec.is_empty(), "w4 is not on the connection");
    assert_eq!(conn("a:1", "b:1", "0", "").spec(), wc_core::WireSpec::default());
    assert_eq!(
        conn("a:1", "b:1", "18 AWG", "").spec().section.as_deref(),
        Some("18 AWG")
    );
}

fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write;
    let mut buf = Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opt = zip::write::SimpleFileOptions::default();
        for (name, data) in files {
            z.start_file(*name, opt).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }
    buf.into_inner()
}

#[test]
fn zip_tables_are_keyed_by_lower_case_file_name_at_any_depth() {
    let bytes = zip_of(&[
        ("Projects/Demo.edb/Page.EOD", b"page"),
        ("Projects\\Demo.edb\\function.eod", b"func"),
        ("Demo.edb/ProjectInfo.xml", b"<x/>"),
        ("Demo.edb/readme.txt", b"skip"),
        ("Demo.edb/page.eod.bak", b"skip"),
    ]);
    let (tables, warnings) = tables_from_zip(&bytes).unwrap();
    assert!(warnings.is_empty());
    let keys: Vec<&str> = tables.keys().map(String::as_str).collect();
    assert_eq!(keys, ["function.eod", "page.eod", "projectinfo.xml"]);
    assert_eq!(tables["page.eod"], b"page");
    assert_eq!(tables["function.eod"], b"func");
}

#[test]
fn zip_without_tables_is_an_error() {
    let only_info = zip_of(&[("Demo.edb/ProjectInfo.xml", b"<x/>"), ("notes.txt", b"hi")]);
    // ProjectInfo.xml alone is kept, but reading needs at least one table.
    let (t, _) = tables_from_zip(&only_info).unwrap();
    assert_eq!(t.keys().collect::<Vec<_>>(), ["projectinfo.xml"]);
    assert!(read(&t).is_err());
    let nothing = zip_of(&[("notes.txt", b"hi")]);
    assert!(matches!(tables_from_zip(&nothing), Err(Error::Format(m)) if m.contains("no Eplan project tables")));
}

#[test]
fn wanted_and_optional_files() {
    assert!(wanted("page.eod"));
    assert!(wanted("projectinfo.xml"));
    assert!(!wanted("page.eod.bak"));
    assert!(!wanted("project.elk"));
    assert!(optional("projectinfo.xml"));
    assert!(!optional("page.eod"));
}

#[test]
fn project_info_reads_properties_and_picks_a_language() {
    let xml = br#"<?xml version="1.0"?>
<EplanPxfRoot>
  <Property id="10011" name="Project name">  Pump station  </Property>
  <Property id="10013" name="Customer">de_DE@Kunde &amp; Co;en_US@Customer &amp; Co;</Property>
  <Property id="oops" name="Bad id">kept</Property>
  <Other>ignored</Other>
</EplanPxfRoot>"#;
    let props = project_info(xml);
    assert_eq!(
        props,
        [
            (10011, "Project name".to_owned(), "Pump station".to_owned()),
            (10013, "Customer".to_owned(), "Customer & Co".to_owned()),
            (0, "Bad id".to_owned(), "kept".to_owned()),
        ]
    );
    // Broken XML stops reading but keeps what came before.
    let broken = br#"<R><Property id="1" name="A">a</Property><Property id="2" name="B">b</Wrong></R>"#;
    let props = project_info(broken);
    assert_eq!(props.first(), Some(&(1, "A".to_owned(), "a".to_owned())));
    assert!(project_info(b"").is_empty());
}

#[test]
fn project_info_from_tables_reaches_the_project() {
    let mut t = sample();
    t.insert(
        "projectinfo.xml".into(),
        br#"<R><Property id="10011" name="Project name">Demo</Property></R>"#.to_vec(),
    );
    let p = read(&t).unwrap();
    assert_eq!(p.properties, [(10011, "Project name".to_owned(), "Demo".to_owned())]);
}

#[test]
fn pick_language_fallbacks() {
    // en_GB wins over neutral and other languages; neutral wins over the rest.
    assert_eq!(pick_language("de_DE@Pumpe;??_??@P;en_GB@Pump;"), "Pump");
    assert_eq!(pick_language("de_DE@Pumpe;??_??@P;"), "P");
    // No preferred language: the first one.
    assert_eq!(pick_language("fr_FR@Pompe;de_DE@Pumpe;"), "Pompe");
    // An '@' without a language code is plain text.
    assert_eq!(pick_language("user@example"), "user@example");
}

#[test]
fn tag_text_joins_the_parts_that_are_present() {
    assert_eq!(tag_text("PUMPE", "1", "M", "1"), "PUMPE1-M1");
    assert_eq!(tag_text("K", "2", "", ""), "K2");
    assert_eq!(tag_text("", "", "X", "3"), "X3");
    assert_eq!(tag_text("", "", "", ""), "");
}

#[test]
fn crate_natural_orders_numbers_by_value() {
    assert_eq!(crate_natural("X10:2"), [("X".to_owned(), 10), (":".to_owned(), 2)]);
    assert_eq!(crate_natural("PE"), [("PE".to_owned(), 0)]);
    assert!(crate_natural("X2") < crate_natural("X10"));
    assert!(crate_natural("X10:2") < crate_natural("X10:11"));
    // Too many digits saturate rather than panic.
    assert_eq!(crate_natural("A99999999999999999999999"), [("A".to_owned(), u64::MAX)]);
}

#[test]
fn user_block_keeps_entries_before_damage() {
    let mut body = block(F_PROPERTIES, &[(20001, 0, "first"), (20002, 1, "second")]);
    // Corrupt the end marker into an out-of-range id followed by junk.
    let len = body.len();
    body[len - 2..].copy_from_slice(&u16le(5));
    body.extend([0, 0, b'x', 0]);
    let r = record(CLASS_PAGE, 1, &body);
    let recs = records(&r);
    let [rec] = recs.as_slice() else { unreachable!() };
    let b = user_block(rec, F_PROPERTIES);
    assert_eq!(b.len(), 2);
    assert_eq!(prop(&b, 20001), "first");
    assert_eq!(b.get(&(20002, 1)).map(String::as_str), Some("second"));
    assert_eq!(prop(&b, 20002), "");
    assert!(user_block(rec, F_TAG_PARTS).is_empty());
}

#[test]
fn elk_target_encodings_and_rejections() {
    assert_eq!(
        elk_target("\u{feff}Plant.edb\n12\n".as_bytes()).as_deref(),
        Some("Plant.edb")
    );
    // UTF-16 without a byte order mark is detected by its zero high bytes.
    let units: Vec<u8> = "Plant.edb\r\n".encode_utf16().flat_map(u16::to_le_bytes).collect();
    assert_eq!(elk_target(&units).as_deref(), Some("Plant.edb"));
    assert_eq!(elk_target(b""), None);
    assert_eq!(elk_target(b"   \nPlant.edb\n"), None);
    assert_eq!(elk_target(b"..\\evil.edb\n"), None);
}

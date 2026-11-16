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

fn sample() -> Tables {
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
    assert_eq!(titles, ["Eplan page 1: MAIN CIRCUIT", "Eplan page 2: CONTROL"]);
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
    let labelled = b
        .pages
        .iter()
        .flat_map(|pg| &pg.wires)
        .any(|w| w.number.as_deref() == Some("BK 1.5"));
    assert!(labelled);
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
    let base = sample();
    for round in 0..300 {
        let mut t = base.clone();
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

#![allow(clippy::unwrap_used)]

use super::*;
use wc_core::geom::{Point, Rotation};
use wc_core::symbol::{Pin, PinDir};
use wc_core::{Device, Page, PlacedSymbol, Wire};

fn sym(id: &str, role: SymbolRole, pins: &[&str]) -> Symbol {
    Symbol {
        source: String::new(),
        id: id.into(),
        name: id.into(),
        category: String::new(),
        letter_code: String::new(),
        role,
        graphics: vec![],
        pins: pins
            .iter()
            .enumerate()
            .map(|(n, p)| Pin {
                name: (*p).into(),
                at: Point::new(0.0, 10.0 * n as f64),
                dir: PinDir::Down,
            })
            .collect(),
        tag_at: Point::default(),
        description: String::new(),
    }
}

fn place(id: &str, symbol: &str, x: f64, tag: Option<&str>) -> PlacedSymbol {
    PlacedSymbol {
        id: id.into(),
        symbol: symbol.into(),
        at: Point::new(x, 50.0),
        rotation: Rotation::R0,
        mirror: false,
        tag: tag.map(|t| DeviceTag::parse(t).unwrap()),
    }
}

fn pin(item: &str, pin: &str) -> Endpoint {
    Endpoint::Pin {
        item: item.into(),
        pin: pin.into(),
    }
}

fn wire(id: &str, from: Endpoint, to: Endpoint, number: Option<&str>, potential: Option<&str>) -> Wire {
    Wire {
        id: id.into(),
        from,
        to,
        via: vec![],
        number: number.map(Into::into),
        potential: potential.map(Into::into),
        cable: None,
        route: Default::default(),
        spec: Default::default(),
    }
}

fn fixture() -> (Project, BTreeMap<String, Symbol>, BTreeMap<String, Part>) {
    let mut p = Project::new("Test");
    let mut page = Page::new("p2", "2", "Control");
    page.symbols = vec![
        place("s1", "coil", 50.0, Some("=A+C1-K1")),
        place("s2", "no", 80.0, Some("=A+C1-K1")),
        place("s3", "term", 120.0, Some("-X1.2")),
        place("s4", "term", 110.0, Some("-X1.1")),
        place("s5", "term", 130.0, Some("-X2")),
        place("s6", "lamp", 150.0, Some("-P1")),
    ];
    page.wires = vec![
        wire(
            "w1",
            Endpoint::Point {
                at: Point::new(50.0, 20.0),
            },
            pin("s1", "A1"),
            Some("1"),
            Some("24V"),
        ),
        wire("w2", pin("s2", "14"), pin("s4", "1"), Some("2"), None),
        wire("w3", pin("s4", "2"), pin("s6", "X1"), Some("3"), None),
        wire("w4", pin("s1", "A2"), pin("s5", "1"), None, Some("0V")),
    ];
    p.pages.push(page);
    let part = Part {
        id: "acme.r24".into(),
        manufacturer: "Acme".into(),
        order_number: "R-24, DC".into(),
        description: "Relay 24 V".into(),
        ..Default::default()
    };
    p.devices.insert(
        DeviceTag::parse("=A+C1-K1").unwrap(),
        Device {
            part: Some(part.id.clone()),
            description: "Pump relay".into(),
            ..Default::default()
        },
    );
    let symbols = [
        sym("coil", SymbolRole::Main, &["A1", "A2"]),
        sym("no", SymbolRole::Auxiliary, &["13", "14"]),
        sym("term", SymbolRole::Terminal, &["1", "2"]),
        sym("lamp", SymbolRole::Standalone, &["X1", "X2"]),
    ]
    .into_iter()
    .map(|s| (s.id.clone(), s))
    .collect();
    let parts = [(part.id.clone(), part)].into_iter().collect();
    (p, symbols, parts)
}

#[test]
fn parts_list_groups_by_part_and_lists_gaps() {
    let (p, s, parts) = fixture();
    let t = parts_list(&Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    });
    assert_eq!(t.rows[0], ["1", "1", "Acme", "R-24, DC", "Relay 24 V", "=A+C1-K1"]);
    // -P1, -X1.1, -X1.2, -X2 have no part.
    assert_eq!(t.rows.len(), 5);
    assert!(t.rows[1..].iter().all(|r| r[4] == "no part assigned"));
    // The comma in the order number is quoted in CSV.
    assert!(t.to_csv().contains("\"R-24, DC\""));
    assert!(t.to_markdown().starts_with("## Parts list\n\n| Pos | Qty |"));
}

#[test]
fn parts_list_counts_accessories_with_their_quantity() {
    let (mut p, s, mut parts) = fixture();
    let marker = wc_core::Part {
        id: "acme.marker".into(),
        manufacturer: "Acme".into(),
        order_number: "MK-1".into(),
        description: "Marker".into(),
        ..Default::default()
    };
    parts.insert(marker.id.clone(), marker);
    let relay = parts.keys().find(|k| *k != "acme.marker").cloned().unwrap();
    let k1 = DeviceTag::parse("=A+C1-K1").unwrap();
    let d = p.devices.get_mut(&k1).unwrap();
    // Two markers, and the relay again as a spare: 1 + 1 relays.
    d.accessories = vec![
        wc_core::DevicePart {
            part: "acme.marker".into(),
            qty: 2,
        },
        wc_core::DevicePart {
            part: relay.clone(),
            qty: 1,
        },
    ];
    // -P1 has only accessories: it is no longer a gap.
    p.devices.insert(
        DeviceTag::parse("-P1").unwrap(),
        Device {
            accessories: vec![wc_core::DevicePart {
                part: "acme.marker".into(),
                qty: 3,
            }],
            ..Default::default()
        },
    );
    let t = parts_list(&Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    });
    let row = |order: &str| t.rows.iter().find(|r| r[3] == order).unwrap().clone();
    assert_eq!(row("MK-1")[1], "5");
    assert_eq!(row("MK-1")[5], "-P1, =A+C1-K1");
    assert_eq!(row("R-24, DC")[1], "2");
    assert_eq!(row("R-24, DC")[5], "=A+C1-K1");
    assert_eq!(t.rows.iter().filter(|r| r[4] == "no part assigned").count(), 3);
}

#[test]
fn interruption_points_are_not_devices() {
    let (mut p, mut s, parts) = fixture();
    let arrow = sym("arrow", SymbolRole::Interruption, &["1"]);
    s.insert(arrow.id.clone(), arrow);
    p.pages[0].symbols.push(place("s9", "arrow", 200.0, Some("-L1")));
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    assert!(!parts_list(&i).rows.iter().any(|r| r[5] == "-L1"));
    assert!(!device_list(&i).rows.iter().any(|r| r[0] == "-L1"));
}

#[test]
fn device_list_merges_symbols_of_one_device() {
    let (p, s, parts) = fixture();
    let t = device_list(&Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    });
    let k1 = t.rows.iter().find(|r| r[0] == "=A+C1-K1").unwrap();
    assert_eq!(k1, &["=A+C1-K1", "Pump relay", "acme.r24", "coil, no", "2"]);
}

#[test]
fn wire_list_names_both_ends_and_inherits_potentials() {
    let (p, s, parts) = fixture();
    let t = wire_list(&Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    });
    assert_eq!(
        t.rows[0],
        ["2", "1", "24V", "", "junction 50.0,20.0", "=A+C1-K1:A1", "", "", ""]
    );
    assert_eq!(t.rows[1][4..6], ["=A+C1-K1:14", "-X1.1:1"]);
    // w4 carries 0V itself.
    assert_eq!(t.rows[3][2], "0V");
}

#[test]
fn wire_list_shows_section_colour_and_type() {
    let (mut p, s, parts) = fixture();
    let w = p
        .pages
        .iter_mut()
        .flat_map(|pg| pg.wires.iter_mut())
        .find(|w| w.id == "w2")
        .unwrap();
    w.spec.section = Some("1.5 mm²".into());
    w.spec.colour = Some("BU".into());
    w.spec.kind = Some("H07V-K".into());
    let t = wire_list(&Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    });
    assert_eq!(&t.columns[6..], ["Section", "Colour", "Type"]);
    let row = t.rows.iter().find(|r| r[1] == "2").unwrap();
    assert_eq!(&row[6..], ["1.5 mm²", "BU", "H07V-K"], "{row:?}");
    assert!(
        t.rows
            .iter()
            .filter(|r| r[1] != "2")
            .all(|r| r[6..].iter().all(String::is_empty))
    );
}

#[test]
fn terminal_plan_sorts_numbers_and_shows_both_sides() {
    let (p, s, parts) = fixture();
    let t = terminal_plan(&Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    });
    let x1: Vec<&Vec<String>> = t.rows.iter().filter(|r| r[0] == "-X1").collect();
    assert_eq!(x1.len(), 2);
    assert_eq!(x1[0][1], "1");
    assert_eq!(x1[0][2], "=A+C1-K1:14");
    assert_eq!(x1[0][3], "-P1:X1");
    assert_eq!(x1[1][1], "2");
    assert_eq!(x1[1][2], "");
    // -X2 has no number in its tag: numbered in drawing order, and it is on the 0V net.
    let x2 = t.rows.iter().find(|r| r[0] == "-X2").unwrap();
    assert_eq!(
        (x2[1].as_str(), x2[2].as_str(), x2[4].as_str()),
        ("1", "=A+C1-K1:A2", "0V")
    );
}

#[test]
fn terminal_plan_shows_jumpers() {
    let (mut p, s, parts) = fixture();
    let x1 = wc_core::DeviceTag::parse("-X1").unwrap();
    p.meta.add_jumper(&x1, &["1".to_owned(), "2".to_owned()]).unwrap();
    // Wire the upper side of -X1.2 too: through the jumper it is the same potential as the
    // upper side of -X1.1, but -X1.1's Upper column must still list only its own wiring.
    let pg = p.pages.iter_mut().find(|pg| pg.symbol("s3").is_some()).unwrap();
    pg.wires.push(wire("w8", pin("s3", "1"), pin("s1", "A2"), None, None));
    let t = terminal_plan(&Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    });
    assert_eq!(t.columns[6], "Jumpered with");
    let row = |strip: &str, n: &str| t.rows.iter().find(|r| r[0] == strip && r[1] == n).unwrap().clone();
    assert_eq!(row("-X1", "1")[6], "2");
    assert_eq!(row("-X1", "2")[6], "1");
    assert_eq!(row("-X2", "1")[6], "", "not jumpered");
    // Upper and Lower still show what is wired to that side of the terminal itself.
    assert_eq!(row("-X1", "1")[2], "=A+C1-K1:14");
    assert_eq!(row("-X1", "1")[3], "-P1:X1");
    assert!(row("-X1", "2")[2].contains("K1:A2"), "{:?}", row("-X1", "2"));
    assert!(!row("-X1", "2")[2].contains("K1:14"), "not -X1.1's wiring");
    // The potential is shared through the jumper.
    assert_eq!(row("-X1", "1")[4], row("-X1", "2")[4]);
}

#[test]
fn net_list_joins_across_wires() {
    let (p, s, parts) = fixture();
    let t = net_list(&Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    });
    let zero = t.rows.iter().find(|r| r[0] == "0V").unwrap();
    assert_eq!(zero[2], "-X2:1, =A+C1-K1:A2");
    assert!(Kind::ALL.iter().all(|k| Kind::parse(k.name()) == Some(*k)));
}

#[test]
fn natural_order() {
    let mut v = vec!["X10", "X2", "X1"];
    v.sort_by_key(|s| natural_key(s));
    assert_eq!(v, ["X1", "X2", "X10"]);
}

#[test]
fn cable_list_groups_cores_by_cable() {
    let (mut p, s, parts) = fixture();
    let w1 = DeviceTag::parse("-W1").unwrap();
    p.devices.insert(
        w1.clone(),
        Device {
            description: "Lamp cable".into(),
            ..Default::default()
        },
    );
    for (id, core) in [("w3", "2"), ("w2", "1")] {
        if let Some(w) = p.pages[0].wires.iter_mut().find(|w| w.id == id) {
            w.cable = Some(wc_core::CableCore {
                cable: w1.clone(),
                core: core.into(),
            });
        }
    }
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let t = cable_list(&i);
    assert_eq!(t.rows.len(), 2);
    assert_eq!(t.rows[0], ["-W1", "", "Lamp cable", "1", "2", "=A+C1-K1:14", "-X1.1:1"]);
    assert_eq!(t.rows[1][..4], ["", "", "", "2"]);
    assert_eq!(wire_list(&i).rows[1][3], "-W1:1");
}

#[test]
fn check_names_a_cable_core_on_two_connections() {
    let (mut p, s, parts) = fixture();
    let found = |p: &Project| {
        let i = Inputs {
            project: p,
            symbols: &s,
            parts: &parts,
        };
        check(&i)
            .into_iter()
            .filter(|f| f.message.contains("cable core"))
            .map(|f| (f.page_id, f.item, f.message))
            .collect::<Vec<_>>()
    };
    let set_core = |p: &mut Project, page: usize, id: &str, core: &str| {
        if let Some(w) = p.pages[page].wires.iter_mut().find(|w| w.id == id) {
            w.cable = Some(wc_core::CableCore {
                cable: DeviceTag::parse("-W1").unwrap(),
                core: core.into(),
            });
        }
    };
    // w2 (-K1:14 to -X1.1:1) and w4 (-K1:A2 to -X2:1) are two connections.
    set_core(&mut p, 0, "w2", "1");
    set_core(&mut p, 0, "w4", "2");
    assert!(found(&p).is_empty());
    // A core drawn in two pieces of one connection is one conductor.
    p.pages[0].wires.push(wire(
        "w5",
        pin("s4", "1"),
        Endpoint::Point {
            at: Point::new(110.0, 60.0),
        },
        None,
        None,
    ));
    set_core(&mut p, 0, "w5", "1");
    assert!(found(&p).is_empty());
    // A wire naming only the cable names no core.
    set_core(&mut p, 0, "w3", "");
    set_core(&mut p, 0, "w1", " ");
    assert!(found(&p).is_empty());
    // A repeat on an overview page does not count.
    let mut overview = p.pages[0].clone();
    overview.id = "ov".into();
    overview.number = "1".into();
    overview.overview = true;
    if let Some(c) = overview
        .wires
        .iter_mut()
        .find(|w| w.id == "w4")
        .and_then(|w| w.cable.as_mut())
    {
        c.core = "1".into();
    }
    p.pages.insert(0, overview);
    assert!(found(&p).is_empty());
    // The same core on a second connection is named on the second one's wire.
    set_core(&mut p, 1, "w4", "1");
    assert_eq!(
        found(&p),
        [(
            Some("p2".to_owned()),
            Some("w4".to_owned()),
            "cable core -W1:1 is on 2 connections".to_owned()
        )]
    );
    // Wire ids need not be unique on a page (a replaced page can repeat them): the second
    // connection's wire still counts when it shares the first one's id.
    let w4 = p.pages[1].wires.iter_mut().find(|w| w.id == "w4");
    let Some(w4) = w4 else {
        unreachable!("the fixture has w4")
    };
    w4.id = "w2".into();
    assert_eq!(
        found(&p),
        [(
            Some("p2".to_owned()),
            Some("w2".to_owned()),
            "cable core -W1:1 is on 2 connections".to_owned()
        )]
    );
}

#[test]
fn check_cable_cores_ignore_overview_wiring_that_joins_connections() {
    let (mut p, s, parts) = fixture();
    let found = |p: &Project| {
        let i = Inputs {
            project: p,
            symbols: &s,
            parts: &parts,
        };
        check(&i)
            .into_iter()
            .filter(|f| f.message.contains("cable core"))
            .map(|f| (f.page_id, f.item, f.message))
            .collect::<Vec<_>>()
    };
    // w1 (24V, at -K1:A1) and w4 (0V, at -K1:A2) are two connections sharing core -W1:1.
    for w in p.pages[0].wires.iter_mut().filter(|w| w.id == "w1" || w.id == "w4") {
        w.cable = Some(wc_core::CableCore {
            cable: DeviceTag::parse("-W1").unwrap(),
            core: "1".into(),
        });
    }
    let expected = [(
        Some("p2".to_owned()),
        Some("w4".to_owned()),
        "cable core -W1:1 is on 2 connections".to_owned(),
    )];
    assert_eq!(found(&p), expected);
    // An overview page whose wiring joins the 24V and 0V potentials into one net.
    let mut overview = Page::new("ov", "1", "Overview");
    overview.overview = true;
    let mid = Point::new(20.0, 20.0);
    overview.wires = vec![
        wire(
            "o1",
            Endpoint::Point {
                at: Point::new(10.0, 20.0),
            },
            Endpoint::Point { at: mid },
            None,
            Some("24V"),
        ),
        wire(
            "o2",
            Endpoint::Point { at: mid },
            Endpoint::Point {
                at: Point::new(30.0, 20.0),
            },
            None,
            Some("0V"),
        ),
    ];
    p.pages.insert(0, overview);
    assert_eq!(found(&p), expected);
}

#[test]
fn project_check_finds_the_usual_mistakes() {
    let (mut p, s, parts) = fixture();
    {
        let page = &mut p.pages[0];
        // A second coil with the same tag, and a contact whose coil does not exist.
        page.symbols.push(place("s10", "coil", 300.0, Some("=A+C1-K1")));
        page.symbols.push(place("s11", "no", 320.0, Some("-K9")));
        // 24V and 0V meet on the lamp's X2 pin.
        page.wires
            .push(wire("w5", pin("s6", "X2"), pin("s5", "1"), None, Some("24V")));
        // A wire that ends in the air, and one on a pin that does not exist.
        page.wires.push(wire(
            "w6",
            pin("s6", "X2"),
            Endpoint::Point {
                at: Point::new(400.0, 400.0),
            },
            Some("9"),
            None,
        ));
        page.wires
            .push(wire("w7", pin("s1", "A9"), pin("s6", "X2"), None, None));
    }
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let found = check(&i);
    let has = |sev: Severity, text: &str| found.iter().any(|f| f.severity == sev && f.message.contains(text));
    assert!(has(Severity::Error, "=A+C1-K1 is drawn 2 times"));
    assert!(has(Severity::Warning, "contact -K9 has no coil"));
    assert!(has(Severity::Error, "potentials 0V and 24V are connected"));
    assert!(has(Severity::Warning, "wire 9 ends in the air at 400.0, 400.0"));
    assert!(has(Severity::Error, "wire ends on s1:A9, which does not exist"));
    assert!(has(Severity::Note, "pin 13 not connected"));
    assert!(has(Severity::Note, "-P1 has no part"));
    // Errors come first, and every finding on a page names the item to select.
    assert_eq!(found[0].severity, Severity::Error);
    assert!(found.iter().filter(|f| f.page_id.is_some()).all(|f| f.item.is_some()));
    assert_eq!(check_table(&i).rows.len(), found.len());
}

#[test]
fn a_device_repeated_on_an_overview_page_is_not_drawn_twice() {
    let (mut p, s, parts) = fixture();
    // The coil -K1 again, on a line diagram.
    let mut overview = Page::new("p9", "9", "Line diagram");
    overview.symbols.push(place("s20", "coil", 100.0, Some("=A+C1-K1")));
    p.pages.push(overview);
    let drawn_twice = |p: &Project| {
        let i = Inputs {
            project: p,
            symbols: &s,
            parts: &parts,
        };
        check(&i).iter().any(|f| f.message.contains("is drawn 2 times"))
    };
    assert!(drawn_twice(&p), "on a normal page the repeat is an error");
    p.pages[1].overview = true;
    assert!(!drawn_twice(&p), "on an overview page it is not");
}

#[test]
fn a_coil_drawn_only_on_an_overview_page_still_exists_for_its_contacts() {
    // Decided on purpose: the contact's cross-reference shows that coil, so the contact is not
    // reported as having none. Only the drawn-twice count leaves overview pages out.
    let (mut p, s, parts) = fixture();
    p.pages[0].symbols.push(place("s11", "no", 320.0, Some("-K9")));
    let mut overview = Page::new("p9", "9", "Line diagram");
    overview.overview = true;
    overview.symbols.push(place("s20", "coil", 100.0, Some("-K9")));
    p.pages.push(overview);
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    assert!(!check(&i).iter().any(|f| f.message.contains("contact -K9 has no coil")));
}

#[test]
fn automationml_has_devices_pins_and_one_link_per_join() {
    let (mut p, mut s, parts) = fixture();
    let arrow = sym("arrow", SymbolRole::Interruption, &["1"]);
    s.insert(arrow.id.clone(), arrow);
    p.pages[0].symbols.push(place("s9", "arrow", 200.0, Some("-L1")));
    p.pages[0].symbols.push(place("s10", "lamp", 220.0, None));
    p.pages[0]
        .wires
        .push(wire("w5", pin("s6", "X2"), pin("s10", "X1"), None, None));
    p.devices
        .get_mut(&DeviceTag::parse("=A+C1-K1").unwrap())
        .unwrap()
        .properties = [("Coil <voltage>".to_string(), "24 V & more".to_string())]
        .into_iter()
        .collect();
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let (xml, counts) = automationml(&i, "2026-11-16T22:00:00Z");
    // K1, -P1, -X1.1, -X1.2, -X2; not the interruption point.
    assert_eq!(counts.devices, 5, "{xml}");
    assert!(!xml.contains("-L1"));
    // w2, w3, w4 each join two device pins; w1 has one, w5 ends on an untagged lamp.
    assert_eq!(counts.links, 3);
    assert_eq!(counts.untagged_pins, 1);
    assert!(xml.contains("<InternalLink Name=\"0V\""));
    assert!(xml.contains("<Value>R-24, DC</Value>") && xml.contains("<Value>Acme</Value>"));
    assert!(xml.contains("Name=\"Coil &lt;voltage&gt;\"") && xml.contains("<Value>24 V &amp; more</Value>"));
    assert!(xml.contains("LastWritingDateTime=\"2026-11-16T22:00:00Z\""));
    // The coil and the contact are one device with all four connection points.
    let k1 = xml.split("<InternalElement Name=\"=A+C1-K1\"").nth(1).unwrap();
    let k1 = k1.split("</InternalElement>").next().unwrap();
    assert_eq!(k1.matches("<ExternalInterface").count(), 4);
    // The same project gives the same file.
    assert_eq!(automationml(&i, "2026-11-16T22:00:00Z").0, xml);
}

#[test]
fn plc_io_list_and_contents() {
    let (mut p, mut s, parts) = fixture();
    let mut input = sym("wc.plc.input", SymbolRole::Standalone, &["I"]);
    input.category = "plc".into();
    input.name = "PLC digital input".into();
    let mut output = sym("wc.plc.output", SymbolRole::Standalone, &["Q"]);
    output.category = "plc".into();
    for y in [input, output] {
        s.insert(y.id.clone(), y);
    }
    let page = &mut p.pages[0];
    page.symbols.push(place("i2", "wc.plc.input", 300.0, Some("-A1.I2")));
    page.symbols.push(place("i1", "wc.plc.input", 310.0, Some("-A1.I1")));
    page.symbols.push(place("q1", "wc.plc.output", 320.0, Some("-A1.Q1")));
    page.symbols.push(place("u1", "wc.plc.input", 330.0, None));
    page.wires.push(wire("w9", pin("s6", "X2"), pin("q1", "Q"), None, None));
    for (tag, addr) in [("-A1.I2", "%I0.10"), ("-A1.I1", "%I0.2"), ("-A1.Q1", "%Q0.0")] {
        p.devices.insert(
            DeviceTag::parse(tag).unwrap(),
            Device {
                description: format!("point {tag}"),
                properties: [("Address".to_string(), addr.to_string())].into_iter().collect(),
                ..Default::default()
            },
        );
    }
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let t = build(Kind::Plc, &i);
    let col = |n: usize| t.rows.iter().map(|r| r[n].as_str()).collect::<Vec<_>>();
    // Natural address order (0.2 before 0.10), points without an address last.
    assert_eq!(col(0), ["%I0.2", "%I0.10", "%Q0.0", ""]);
    assert_eq!(col(1), ["Input", "Input", "Output", "Input"]);
    assert_eq!(t.rows[2][2], "-A1.Q1");
    assert_eq!(t.rows[2][4], "-P1:X2");
    assert_eq!(t.rows[3][2], "u1 (no tag)");

    let c = build(Kind::Contents, &i);
    assert_eq!(c.columns, ["Page", "Title", "Devices", "Wires"]);
    // K1 counts once for its coil and contact; the untagged input has no device.
    assert_eq!(c.rows, [["2", "Control", "8", "5"]]);

    let v = plc_variables(&i);
    assert_eq!((v.variables, v.unlocated), (4, 1));
    let lines: Vec<&str> = v.text.lines().collect();
    assert_eq!(lines[1], "VAR_GLOBAL");
    assert!(lines[2].starts_with("    A1_I1 AT %IX0.2 : BOOL; (* -A1.I1, point -A1.I1, address %I0.2"));
    assert!(lines[3].starts_with("    A1_I2 AT %IX0.10 : BOOL;"));
    assert!(lines[4].starts_with("    A1_Q1 AT %QX0.0 : BOOL; (* -A1.Q1, point -A1.Q1, address %Q0.0, to -P1:X2"));
    assert!(lines[5].starts_with("    u1 AT %I* : BOOL;"));
    assert_eq!(lines[6], "END_VAR");
}

#[test]
fn the_location_list_counts_devices_and_names_unlisted_locations() {
    let mut p = Project::new("t");
    for t in ["+L1-K1", "+L1-K2", "+L2.A-K3", "-K4"] {
        p.devices.insert(DeviceTag::parse(t).unwrap(), Default::default());
    }
    p.meta.set_location("L1", "Main cabinet").unwrap();
    p.meta.set_location("L9", "").unwrap();
    let (symbols, parts) = (BTreeMap::new(), BTreeMap::new());
    let i = Inputs {
        project: &p,
        symbols: &symbols,
        parts: &parts,
    };
    let t = build(Kind::Locations, &i);
    assert_eq!(
        t.rows,
        [
            ["+L1", "Main cabinet", "2", ""],
            ["+L9", "", "0", ""],
            ["+L2.A", "", "1", "not in the project's locations"],
        ]
    );
}

#[test]
fn the_network_list_shows_each_device_with_its_address_ports_and_cables() {
    let mut p = Project::new("t");
    let mut page = Page::new("p3", "3", "PLC");
    page.symbols.push(place("s1", "plc", 20.0, Some("-A1")));
    // An overview drawn before it shows -A1 again; the list keeps to schematic pages.
    let mut overview = Page::new("p1", "1", "Overview");
    overview.overview = true;
    overview.symbols.push(place("s1", "plc", 20.0, Some("-A1")));
    p.pages.push(overview);
    p.pages.push(page);
    for (t, d) in [
        ("-K10", "Drive pump 1"),
        ("-K11", "Drive pump 2"),
        ("-K12", "Spare drive"),
    ] {
        p.devices.insert(
            DeviceTag::parse(t).unwrap(),
            Device {
                description: d.into(),
                ..Default::default()
            },
        );
    }
    use wc_core::network::{Link, Network, Node};
    let node = |device: &str, address: &str, station: &str| Node {
        device: device.into(),
        address: address.into(),
        station: station.into(),
        ..Default::default()
    };
    let link = |from: &str, to: &str, cable: &str| Link {
        from: from.into(),
        to: to.into(),
        cable: cable.into(),
        ..Default::default()
    };
    p.meta.networks = vec![Network {
        name: "PN1".into(),
        kind: "profinet".into(),
        nodes: vec![
            node("-A1", "192.168.0.1", "plc"),
            node("-K10", "192.168.0.10", ""),
            node("-K11", "192.168.0.11", ""),
            node("-K12", "", ""),
            node("-K99", "192.168.0.11", ""),
        ],
        links: vec![link("-A1:X1P1", "-K10:P1", "-W20"), link("-K10:P2", "-K11:P1", "-W21")],
        ..Default::default()
    }];
    let (symbols, parts) = (BTreeMap::new(), BTreeMap::new());
    let i = Inputs {
        project: &p,
        symbols: &symbols,
        parts: &parts,
    };
    let t = build(Kind::Networks, &i);
    let row = |c: &[&str]| c.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        t.rows,
        [
            row(&[
                "PN1",
                "PROFINET",
                "-A1",
                "",
                "192.168.0.1",
                "plc",
                "X1P1",
                "-K10:P1",
                "-W20",
                "3"
            ]),
            row(&[
                "PN1",
                "PROFINET",
                "-K10",
                "Drive pump 1",
                "192.168.0.10",
                "",
                "P1",
                "-A1:X1P1",
                "-W20",
                ""
            ]),
            row(&[
                "PN1",
                "PROFINET",
                "-K10",
                "Drive pump 1",
                "192.168.0.10",
                "",
                "P2",
                "-K11:P1",
                "-W21",
                ""
            ]),
            row(&[
                "PN1",
                "PROFINET",
                "-K11",
                "Drive pump 2",
                "192.168.0.11",
                "",
                "P1",
                "-K10:P2",
                "-W21",
                ""
            ]),
            row(&["PN1", "PROFINET", "-K12", "Spare drive", "", "", "", "", "", ""]),
            row(&["PN1", "PROFINET", "-K99", "", "192.168.0.11", "", "", "", "", ""]),
        ]
    );
    // Only the drawn device links to its page.
    assert_eq!(t.row_pages, ["p3"]);
    assert_eq!(
        network_warnings(&i),
        [
            "network PN1: -K11 and -K99 have the same address 192.168.0.11",
            "network PN1: -K99 is not a device of the project",
        ]
    );
    p.meta.networks.clear();
    let i = Inputs {
        project: &p,
        symbols: &symbols,
        parts: &parts,
    };
    assert!(build(Kind::Networks, &i).rows.is_empty());
    assert!(network_warnings(&i).is_empty());
}

#[test]
fn marking_list_has_every_marker_in_natural_order() {
    let (mut p, s, parts) = fixture();
    let w1 = DeviceTag::parse("-W1").unwrap();
    if let Some(w) = p.pages[0].wires.iter_mut().find(|w| w.id == "w3") {
        w.cable = Some(wc_core::CableCore {
            cable: w1.clone(),
            core: "1".into(),
        });
    }
    // A cable device without cores yet, and a strip numbered past 9.
    p.devices.insert(DeviceTag::parse("-W10").unwrap(), Device::default());
    p.pages[0].symbols.push(place("s7", "term", 140.0, Some("-X1.10")));
    p.pages[0]
        .wires
        .push(wire("w5", pin("s6", "X2"), pin("s7", "1"), Some("10"), None));
    p.pages[0]
        .wires
        .push(wire("w6", pin("s7", "2"), pin("s5", "2"), Some("2"), None));
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let t = marking_list(&i, &[]);
    let rows: Vec<String> = t.rows.iter().map(|r| r.join("|")).collect();
    assert_eq!(
        rows,
        [
            "Terminal|-X1|1|1",
            "Terminal|-X1|2|1",
            "Terminal|-X1|10|1",
            "Terminal|-X2|1|1",
            "Wire||1|2",
            "Wire||2|4",
            "Wire||3|2",
            "Wire||10|2",
            "Device||-P1|1",
            "Device|+C1|=A+C1-K1|1",
            "Cable||-W1|2",
            "Cable||-W10|2",
        ]
    );

    let only = marking_list(&i, &[MarkerKind::Cables, MarkerKind::Terminals]);
    assert!(only.rows.iter().all(|r| r[0] == "Cable" || r[0] == "Terminal"));
    assert_eq!(only.rows.len(), 6);
    assert_eq!(MarkerKind::parse("Wire"), Some(MarkerKind::Wires));
    assert_eq!(MarkerKind::parse("cables"), Some(MarkerKind::Cables));
    assert_eq!(MarkerKind::parse("rails"), None);
}

#[test]
fn smartscript_csv_is_semicolon_separated_utf8_with_bom() {
    let t = Table {
        title: "Marking list".into(),
        columns: vec!["Type".into(), "Group".into(), "Marker".into(), "Count".into()],
        rows: vec![
            vec!["Device".into(), "+Kjøkken".into(), "-K1;A".into(), "1".into()],
            vec!["Wire".into(), String::new(), "2,5".into(), "2".into()],
        ],
        ..Default::default()
    };
    let csv = to_smartscript_csv(&t);
    assert_eq!(
        csv,
        "\u{feff}Type;Group;Marker;Count\r\nDevice;+Kjøkken;\"-K1;A\";1\r\nWire;;2,5;2\r\n"
    );
}

#[test]
fn wire_numbers_that_look_like_formulas_are_warned_about_and_kept() {
    let t = Table {
        title: "Marking list".into(),
        columns: vec!["Type".into(), "Group".into(), "Marker".into(), "Count".into()],
        rows: vec![
            vec!["Terminal".into(), "-X1".into(), "-1".into(), "1".into()],
            vec!["Wire".into(), String::new(), "=1".into(), "2".into()],
            vec!["Wire".into(), String::new(), "@L1".into(), "2".into()],
            vec!["Wire".into(), String::new(), "101".into(), "2".into()],
            vec!["Device".into(), String::new(), "-K1".into(), "1".into()],
            vec!["Cable".into(), "+C1".into(), "+C1-W1".into(), "2".into()],
        ],
        ..Default::default()
    };
    let w = formula_warnings(&t);
    assert_eq!(w.len(), 2, "{w:?}");
    assert!(w[0].contains("\"=1\"") && w[0].contains("formula"), "{}", w[0]);
    assert!(w[1].contains("\"@L1\""));
    assert!(to_smartscript_csv(&t).contains("Wire;;=1;2"), "the marker text is kept");
}

#[test]
fn a_cable_drawn_only_as_a_symbol_gets_its_markers() {
    let (mut p, s, parts) = fixture();
    p.pages[0].symbols.push(place("s9", "lamp", 200.0, Some("-W2")));
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let t = marking_list(&i, &[]);
    let cables: Vec<String> = t.rows.iter().filter(|r| r[0] == "Cable").map(|r| r.join("|")).collect();
    assert_eq!(cables, ["Cable||-W2|2"]);
    assert!(!t.rows.iter().any(|r| r[0] == "Device" && r[2] == "-W2"));
    assert_eq!(
        parse_kinds(&["wire".into(), "Cables".into()]),
        Ok(vec![MarkerKind::Wires, MarkerKind::Cables])
    );
    assert!(parse_kinds(&["rails".into()]).is_err());
}

#[test]
fn a_terminal_drawn_only_on_an_overview_does_not_take_a_cables_markers() {
    let (mut p, s, parts) = fixture();
    // -X1.W1 is drawn as a terminal on an overview only; -X1.W2 on a schematic page.
    let mut overview = Page::new("ov", "9", "Overview");
    overview.overview = true;
    overview.symbols.push(place("o1", "term", 0.0, Some("-X1.W1")));
    p.pages.push(overview);
    p.pages[0].symbols.push(place("t9", "term", 150.0, Some("-X1.W2")));
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let t = marking_list(&i, &[MarkerKind::Cables, MarkerKind::Devices]);
    let cables: Vec<&str> = t
        .rows
        .iter()
        .filter(|r| r[0] == "Cable")
        .map(|r| r[2].as_str())
        .collect();
    assert!(cables.contains(&"-X1.W1"), "{cables:?}");
    assert!(!cables.contains(&"-X1.W2"), "{cables:?}");
    assert!(!t.rows.iter().any(|r| r[0] == "Device" && r[2].starts_with("-X1.W")));
}

#[test]
fn marking_list_drops_repeated_terminals_even_between_equal_sorting_numbers() {
    let (mut p, s, parts) = fixture();
    // `-X1.1`, `-X1.01`, `-X1.1`: `1` and `01` sort alike, but only the exact repeat goes.
    for (id, tag) in [("t1", "-X1.1"), ("t2", "-X1.01"), ("t3", "-X1.1")] {
        p.pages[0].symbols.push(place(id, "term", 150.0, Some(tag)));
    }
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let t = marking_list(&i, &[MarkerKind::Terminals]);
    let x1: Vec<&str> = t.rows.iter().filter(|r| r[1] == "-X1").map(|r| r[2].as_str()).collect();
    let ones = x1.iter().filter(|n| **n == "1").count();
    assert_eq!(ones, 1, "{x1:?}");
    assert_eq!(x1.iter().filter(|n| **n == "01").count(), 1, "{x1:?}");
}

#[test]
fn marker_lines_put_the_engraving_text_and_the_description_on_lines_of_their_own() {
    let (mut p, s, parts) = fixture();
    let k1 = DeviceTag::parse("=A+C1-K1").unwrap();
    let dev = p.devices.entry(k1).or_default();
    dev.description = "Pump contactor".into();
    dev.properties.insert(MARKER_PROPERTY.into(), "PUMP 1".into());
    let p1 = DeviceTag::parse("-P1").unwrap();
    p.devices
        .entry(p1)
        .or_default()
        .properties
        .insert(MARKER_PROPERTY.into(), "  ".into());
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    // One line: the engraving text replaces the tag; an empty one falls back to the tag.
    let one = marking_list(&i, &[MarkerKind::Devices]);
    let rows: Vec<String> = one.rows.iter().map(|r| r.join("|")).collect();
    assert_eq!(rows, ["Device||-P1|1", "Device|+C1|PUMP 1|1"]);
    assert_eq!(one.columns, ["Type", "Group", "Marker", "Count"]);

    let o = MarkingOptions {
        kinds: vec![MarkerKind::Devices, MarkerKind::Wires],
        lines: vec![LineField::Tag, LineField::Description, LineField::Location],
        ..MarkingOptions::default()
    };
    let t = marking_list_with(&i, &o);
    assert_eq!(t.columns, ["Type", "Group", "Line 1", "Line 2", "Line 3", "Count"]);
    let k1 = t.rows.iter().find(|r| r[2] == "=A+C1-K1").unwrap();
    assert_eq!(k1.join("|"), "Device|+C1|=A+C1-K1|Pump contactor|+C1|1");
    // A wire has no description or location: those lines stay empty.
    let wire = t.rows.iter().find(|r| r[0] == "Wire").unwrap();
    assert_eq!((wire[3].as_str(), wire[4].as_str()), ("", ""));
    assert_eq!(
        parse_lines(&["Tag".into(), "description".into()]),
        Ok(vec![LineField::Tag, LineField::Description])
    );
    assert!(parse_lines(&["colour".into()]).is_err());
}

#[test]
fn marker_list_headers_and_types_follow_the_language_and_warnings_still_find_wires() {
    let (mut p, s, parts) = fixture();
    p.pages[0].wires[0].number = Some("=1".into());
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let de = marking_list_with(
        &i,
        &MarkingOptions {
            language: Language::De,
            ..MarkingOptions::default()
        },
    );
    assert_eq!(de.columns, ["Typ", "Gruppe", "Beschriftung", "Anzahl"]);
    assert!(de.rows.iter().any(|r| r[0] == "Klemme"));
    assert!(de.rows.iter().any(|r| r[0] == "Leiter"));
    assert_eq!(formula_warnings(&de).len(), 1);
    let nb = marking_list_with(
        &i,
        &MarkingOptions {
            language: Language::Nb,
            lines: vec![LineField::Marker, LineField::Description],
            ..MarkingOptions::default()
        },
    );
    assert_eq!(nb.columns, ["Type", "Gruppe", "Linje 1", "Linje 2", "Antall"]);
    assert!(nb.rows.iter().any(|r| r[0] == "Komponent"));
    assert_eq!(Language::parse("NO"), Some(Language::Nb));
    assert_eq!(Language::parse("fr"), None);
}

#[test]
fn formula_warnings_check_every_marker_line_and_free_marker_texts() {
    let (mut p, s, parts) = fixture();
    p.pages[0].wires[0].number = Some("=1".into());
    let k1 = DeviceTag::parse("=A+C1-K1").unwrap();
    let dev = p.devices.entry(k1).or_default();
    dev.properties
        .insert(MARKER_PROPERTY.into(), "=HYPERLINK(\"x\")".into());
    dev.description = "@pump".into();
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    // The wire number is in the second line column here.
    let t = marking_list_with(
        &i,
        &MarkingOptions {
            kinds: vec![MarkerKind::Wires],
            lines: vec![LineField::Description, LineField::Marker],
            ..MarkingOptions::default()
        },
    );
    let w = formula_warnings(&t);
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("\"=1\""));
    // Free texts warn; tags such as `=A+C1-K1` and `-P1` do not.
    let t = marking_list_with(
        &i,
        &MarkingOptions {
            kinds: vec![MarkerKind::Devices],
            lines: vec![LineField::Marker, LineField::Tag, LineField::Description],
            ..MarkingOptions::default()
        },
    );
    let w = formula_warnings(&t);
    assert_eq!(w.len(), 2, "{w:?}");
    assert!(w[0].contains("HYPERLINK") && w[1].contains("@pump"), "{w:?}");
    assert!(to_smartscript_csv(&t).contains("=HYPERLINK"), "the text is kept");
}

#[test]
fn one_strips_markers_are_only_that_strips_terminals() {
    let (mut p, s, parts) = fixture();
    p.pages[0].symbols.push(place("s7", "term", 140.0, Some("-X1.10")));
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    assert_eq!(terminal_strips(&i), ["-X1", "-X2"]);
    // Strips that sort alike are each listed once.
    let mut q = p.clone();
    for (id, tag) in [("z1", "-X01.1"), ("z2", "-X01.3"), ("z3", "-X1.2")] {
        q.pages[0].symbols.push(place(id, "term", 160.0, Some(tag)));
    }
    let strips = terminal_strips(&Inputs {
        project: &q,
        symbols: &s,
        parts: &parts,
    });
    assert_eq!(strips.iter().filter(|s| *s == "-X1").count(), 1, "{strips:?}");
    assert_eq!(strips.iter().filter(|s| *s == "-X01").count(), 1, "{strips:?}");
    let o = MarkingOptions {
        strip: Some("-X1".into()),
        ..MarkingOptions::default()
    };
    let t = marking_list_with(&i, &o);
    let rows: Vec<String> = t.rows.iter().map(|r| r.join("|")).collect();
    assert_eq!(rows, ["Terminal|-X1|1|1", "Terminal|-X1|2|1", "Terminal|-X1|10|1"]);
    // Asking for other kinds as well keeps them; the strip limits only the terminals.
    let both = marking_list_with(
        &i,
        &MarkingOptions {
            kinds: vec![MarkerKind::Terminals, MarkerKind::Wires],
            ..o
        },
    );
    assert!(both.rows.iter().any(|r| r[0] == "Wire"));
    assert!(!both.rows.iter().any(|r| r[1] == "-X2"));
}

#[test]
fn check_names_a_jumper_terminal_drawn_nowhere() {
    let (mut p, s, parts) = fixture();
    let x1 = DeviceTag::parse("-X1").unwrap();
    p.meta.add_jumper(&x1, &["1".into(), "2".into(), "9".into()]).unwrap();
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let found: Vec<String> = check(&i).into_iter().map(|f| f.message).collect();
    assert!(
        found.iter().any(|m| m == "jumper of -X1: terminal 9 is drawn nowhere"),
        "{found:?}"
    );
    assert!(!found.iter().any(|m| m.contains("terminal 1 is drawn nowhere")));
    // A spare terminal is on the rail, not drawn: no warning.
    p.meta.add_spare(&x1, "9", None, &[]).unwrap();
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    assert!(!check(&i).iter().any(|f| f.message.contains("drawn nowhere")));
}

#[test]
fn only_a_terminal_on_an_overview_page_counts_as_drawn_for_a_jumper() {
    let (mut p, s, parts) = fixture();
    let x1 = DeviceTag::parse("-X1").unwrap();
    p.meta.add_jumper(&x1, &["1".into(), "9".into()]).unwrap();
    let mut overview = Page::new("p9", "9", "Overview");
    overview.overview = true;
    // A coil tagged like terminal 9 is no terminal 9.
    overview.symbols.push(place("s30", "coil", 100.0, Some("-X1.9")));
    p.pages.push(overview);
    let warned = |p: &wc_core::Project| {
        let i = Inputs {
            project: p,
            symbols: &s,
            parts: &parts,
        };
        check(&i)
            .into_iter()
            .any(|f| f.message == "jumper of -X1: terminal 9 is drawn nowhere")
    };
    assert!(warned(&p), "a coil does not draw the terminal");
    // A real terminal there does.
    p.pages.last_mut().unwrap().symbols[0].symbol = "term".into();
    assert!(!warned(&p), "an overview terminal counts");
}

#[test]
fn a_two_level_terminal_shows_its_levels_and_shares_one_marker() {
    let (mut p, s, parts) = fixture();
    // -X1.1 and -X1.2 are the two levels of one terminal; -X1.3 and -X1.4 another one whose
    // bottom level says so itself; -X1.5 stands alone.
    for (id, tag, x) in [("t3", "-X1.3", 130.0), ("t4", "-X1.4", 135.0), ("t5", "-X1.5", 140.0)] {
        p.pages[0].symbols.push(place(id, "term", x, Some(tag)));
    }
    for (tag, level) in [("-X1.2", "2"), ("-X1.3", "1"), ("-X1.4", " 2 "), ("-X1.5", "x")] {
        let tag = DeviceTag::parse(tag).unwrap();
        p.devices
            .entry(tag)
            .or_default()
            .properties
            .insert(LEVEL_PROPERTY.into(), level.into());
    }
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let plan = terminal_plan(&i);
    assert_eq!(plan.columns.last().map(String::as_str), Some("Level"));
    let levels: Vec<(&str, &str)> = plan
        .rows
        .iter()
        .filter(|r| r[0] == "-X1")
        .map(|r| (r[1].as_str(), r[7].as_str()))
        .collect();
    assert_eq!(
        levels,
        [("1", "1"), ("2", "2"), ("3", "1"), ("4", "2"), ("5", "")],
        "the bottom level is 1 without a property; a bad level is none"
    );
    let marked = |sep: &str| -> Vec<String> {
        let o = MarkingOptions {
            strip: Some("-X1".into()),
            level_separator: sep.into(),
            ..MarkingOptions::default()
        };
        marking_list_with(&i, &o).rows.iter().map(|r| r[2].clone()).collect()
    };
    assert_eq!(marked(LEVEL_SEPARATOR), ["1 / 2", "3 / 4", "5"]);
    assert_eq!(marked("+"), ["1+2", "3+4", "5"]);
    // Other strips are untouched.
    assert_eq!(plan.rows.iter().find(|r| r[0] == "-X2").unwrap()[7], "");
}

#[test]
fn terminal_markers_of_an_arranged_strip_come_in_its_rail_order() {
    let (mut p, s, parts) = fixture();
    let x1 = DeviceTag::parse("-X1").unwrap();
    // -X1.1 and -X1.2 are one two-level terminal.
    p.devices
        .entry(DeviceTag::parse("-X1.2").unwrap())
        .or_default()
        .properties
        .insert(LEVEL_PROPERTY.into(), "2".into());
    let markers = |p: &wc_core::Project| -> Vec<String> {
        let i = Inputs {
            project: p,
            symbols: &s,
            parts: &parts,
        };
        let o = MarkingOptions {
            kinds: vec![MarkerKind::Terminals],
            level_separator: LEVEL_SEPARATOR.into(),
            ..MarkingOptions::default()
        };
        marking_list_with(&i, &o)
            .rows
            .iter()
            .map(|r| format!("{} {}", r[1], r[2]))
            .collect()
    };
    // Nobody arranged a strip: today's order.
    assert_eq!(markers(&p), ["-X1 1 / 2", "-X2 1"]);
    // A spare S1 goes on the rail before the two-level terminal, and 2 is moved before 1.
    p.meta
        .add_spare(&x1, "S1", None, &[("1".to_owned(), false), ("2".to_owned(), false)])
        .unwrap();
    let current = vec![
        ("1".to_owned(), false),
        ("2".to_owned(), false),
        ("S1".to_owned(), true),
    ];
    p.meta.move_terminal(&x1, "S1", 0, &current).unwrap();
    let current = vec![
        ("S1".to_owned(), true),
        ("1".to_owned(), false),
        ("2".to_owned(), false),
    ];
    p.meta.move_terminal(&x1, "2", 1, &current).unwrap();
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let rail: Vec<String> = terminal_plan(&i)
        .rows
        .iter()
        .filter(|r| r[0] == "-X1")
        .map(|r| r[1].clone())
        .collect();
    assert_eq!(rail, ["S1", "2", "1"]);
    // The spare's marker goes first, as on the rail; the block keeps one marker, numbered
    // upwards; the strip that nobody arranged is as before, and strips stay in tag order.
    assert_eq!(markers(&p), ["-X1 S1", "-X1 1 / 2", "-X2 1"]);
}

#[test]
fn levels_join_only_upwards_within_one_strip() {
    let rows = |v: &[(&str, &str, Option<u32>)]| -> Vec<(String, String, Option<u32>)> {
        v.iter().map(|(a, b, l)| ((*a).into(), (*b).into(), *l)).collect()
    };
    let got = marking::terminal_markers(
        rows(&[
            ("-X1", "1", Some(1)),
            ("-X1", "2", Some(2)),
            ("-X1", "3", Some(3)),
            ("-X1", "4", Some(2)),
            ("-X2", "1", Some(2)),
            ("-X2", "2", None),
        ]),
        "/",
    );
    let got: Vec<String> = got.into_iter().map(|(s, m)| format!("{s} {m}")).collect();
    assert_eq!(got, ["-X1 1/2/3", "-X1 4", "-X2 1", "-X2 2"]);
}

#[test]
fn a_bad_level_is_warned_about_with_its_device_value_and_reason() {
    let (mut p, _, _) = fixture();
    for (tag, level) in [("-X1.1", "1"), ("-X1.2", "two"), ("-X1.3", "0")] {
        p.devices
            .entry(DeviceTag::parse(tag).unwrap())
            .or_default()
            .properties
            .insert(LEVEL_PROPERTY.into(), level.into());
    }
    let w = level_warnings(&p);
    assert_eq!(w.len(), 2, "{w:?}");
    assert!(w[0].contains("-X1.2") && w[0].contains("\"two\"") && w[0].contains("not a whole number"));
    assert!(w[1].contains("-X1.3") && w[1].contains("\"0\"") && w[1].contains("start at 1"));
}

#[test]
fn a_bottom_level_drawn_twice_gets_its_level_on_every_occurrence() {
    let (mut p, s, parts) = fixture();
    // -X1.1 is drawn a second time; -X1.2 says it is level 2.
    p.pages[0].symbols.push(place("t9", "term", 150.0, Some("-X1.1")));
    p.devices
        .entry(DeviceTag::parse("-X1.2").unwrap())
        .or_default()
        .properties
        .insert(LEVEL_PROPERTY.into(), "2".into());
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let levels: Vec<(String, String)> = terminal_plan(&i)
        .rows
        .iter()
        .filter(|r| r[0] == "-X1")
        .map(|r| (r[1].clone(), r[7].clone()))
        .collect();
    assert!(
        levels.iter().filter(|(n, _)| n == "1").all(|(_, l)| l == "1"),
        "{levels:?}"
    );
    let o = MarkingOptions {
        strip: Some("-X1".into()),
        ..MarkingOptions::default()
    };
    let marked: Vec<String> = marking_list_with(&i, &o).rows.iter().map(|r| r[2].clone()).collect();
    assert_eq!(marked, ["1 / 2"]);
}

#[test]
fn strips_with_equal_natural_keys_keep_their_levels_together() {
    let (mut p, s, parts) = fixture();
    for (id, tag, x) in [
        ("a1", "-X01.1", 160.0),
        ("a2", "-X01.2", 165.0),
        ("b1", "-X9.1", 170.0),
        ("b2", "-X9.2", 175.0),
        ("c1", "-X09.1", 180.0),
        ("c2", "-X09.2", 185.0),
    ] {
        p.pages[0].symbols.push(place(id, "term", x, Some(tag)));
    }
    for tag in ["-X01.2", "-X9.2", "-X09.2"] {
        p.devices
            .entry(DeviceTag::parse(tag).unwrap())
            .or_default()
            .properties
            .insert(LEVEL_PROPERTY.into(), "2".into());
    }
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let marked: Vec<String> = marking_list_with(&i, &MarkingOptions::default())
        .rows
        .iter()
        .filter(|r| r[2].contains(" / "))
        .map(|r| r[2].clone())
        .collect();
    assert_eq!(
        marked,
        ["1 / 2", "1 / 2", "1 / 2"],
        "-X01, -X09 and -X9 each give one block"
    );
}

#[test]
fn a_bottom_level_is_inferred_in_the_marking_order_and_never_over_a_bad_value() {
    let set = |p: &mut wc_core::Project, tag: &str, level: &str| {
        p.devices
            .entry(DeviceTag::parse(tag).unwrap())
            .or_default()
            .properties
            .insert(LEVEL_PROPERTY.into(), level.into());
    };
    let marked = |p: &wc_core::Project, s, parts| -> Vec<String> {
        let i = Inputs {
            project: p,
            symbols: s,
            parts,
        };
        let o = MarkingOptions {
            strip: Some("-X1".into()),
            ..MarkingOptions::default()
        };
        marking_list_with(&i, &o).rows.iter().map(|r| r[2].clone()).collect()
    };
    // `1`, `01` and `2` in drawing order, only `2` at level 2: `1` is its bottom level.
    let (mut p, s, parts) = fixture();
    p.pages[0].symbols.push(place("t01", "term", 105.0, Some("-X1.01")));
    set(&mut p, "-X1.2", "2");
    assert_eq!(marked(&p, &s, &parts), ["01", "1 / 2"]);
    // A bad value below level 2 is not taken as level 1.
    let (mut p, s, parts) = fixture();
    set(&mut p, "-X1.1", "two");
    set(&mut p, "-X1.2", "2");
    assert_eq!(marked(&p, &s, &parts), ["1", "2"]);
    assert_eq!(level_warnings(&p).len(), 1);
}

#[test]
fn configurator_list_has_plates_rails_terminals_jumpers_and_accessories_per_strip() {
    use wc_core::DevicePart;
    use wc_core::panel::{Panel, PanelItem};
    let (mut p, s, mut parts) = fixture();
    let term = Part {
        id: "wago.2002-1201".into(),
        manufacturer: "WAGO".into(),
        order_number: "2002-1201".into(),
        description: "2-conductor through terminal block".into(),
        ..Default::default()
    };
    let end_plate = Part {
        id: "wago.2002-1291".into(),
        manufacturer: "WAGO".into(),
        order_number: "2002-1291".into(),
        description: "End and intermediate plate".into(),
        ..Default::default()
    };
    for part in [&term, &end_plate] {
        parts.insert(part.id.clone(), part.clone());
    }
    let tag = |t: &str| DeviceTag::parse(t).unwrap();
    for t in ["-X1.1", "-X1.2"] {
        p.devices.insert(
            tag(t),
            Device {
                part: Some(term.id.clone()),
                ..Default::default()
            },
        );
    }
    p.devices.get_mut(&tag("-X1.2")).unwrap().accessories = vec![DevicePart {
        part: "acme.marker".into(),
        qty: 2,
    }];
    p.devices.insert(
        tag("-X1"),
        Device {
            accessories: vec![DevicePart {
                part: end_plate.id.clone(),
                qty: 1,
            }],
            ..Default::default()
        },
    );
    p.meta
        .add_jumper(&tag("-X1"), &["1".to_owned(), "2".to_owned()])
        .unwrap();
    // w2 ends on -X1.1's upper side, w3 on its lower side.
    for w in p.pages.iter_mut().flat_map(|pg| pg.wires.iter_mut()) {
        match w.id.as_str() {
            "w2" => {
                w.spec.section = Some("1.5 mm²".into());
                w.spec.colour = Some("BU".into());
            }
            "w3" => w.spec.section = Some("2.5 mm²".into()),
            _ => {}
        }
    }
    let mut plate = Panel::with_rails("main", "Main plate", 600.0, 800.0);
    let at = plate
        .items
        .iter()
        .find_map(|i| match i {
            PanelItem::Rail { at, .. } => Some(*at),
            _ => None,
        })
        .unwrap();
    plate.items.push(PanelItem::Device {
        tag: tag("-X1.1"),
        at: Point::new(at.x + 10.0, at.y - 20.0),
        width: 5.2,
        height: 40.0,
        rotation: wc_core::geom::Rotation::R0,
    });
    p.panels.push(plate);

    let out = configurator_list(&Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    });
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let t = out.table;
    assert_eq!(t.columns, CONFIGURATOR_COLUMNS);
    let x1: Vec<(&str, &str)> = t
        .rows
        .iter()
        .filter(|r| r[2] == "-X1")
        .map(|r| (r[0].as_str(), r[1].as_str()))
        .collect();
    assert_eq!(
        x1,
        [
            ("-X1/P1", "Plate"),
            ("-X1/R1", "Rail"),
            ("-X1/T:1", "Terminal"),
            ("-X1/T:2", "Terminal"),
            ("-X1/J1", "Jumper"),
            ("-X1/A1", "Accessory"),
            ("-X1/T:2/A1", "Accessory"),
        ]
    );
    let row = |index: &str| t.rows.iter().find(|r| r[0] == index).unwrap().clone();
    assert_eq!(row("-X1/P1")[8], "Mounting plate 600 x 800 mm");
    assert_eq!(
        (row("-X1/R1")[15].as_str(), row("-X1/R1")[16].as_str()),
        ("Main plate", "1")
    );
    assert_eq!(row("-X1/R1")[18], "0");
    let t1 = row("-X1/T:1");
    assert_eq!((t1[3].as_str(), t1[4].as_str()), ("1", "1"));
    assert_eq!((t1[6].as_str(), t1[7].as_str()), ("2002-1201", "WAGO"));
    assert_eq!(&t1[10..14], ["1.5 mm²", "BU", "2.5 mm²", ""]);
    assert_eq!(row("-X1/J1")[14], "1, 2");
    // The strip's own end plate first, then -X1.2's markers with their quantity.
    assert_eq!(
        (row("-X1/A1")[6].as_str(), row("-X1/A1")[9].as_str()),
        ("2002-1291", "1")
    );
    let a2 = row("-X1/T:2/A1");
    assert_eq!(
        (a2[6].as_str(), a2[9].as_str(), a2[14].as_str()),
        ("acme.marker", "2", "2")
    );
    // -X2 is on no plate and has no part: one bare terminal row.
    let x2: Vec<&Vec<String>> = t.rows.iter().filter(|r| r[2] == "-X2").collect();
    assert_eq!(x2.len(), 1);
    assert_eq!((x2[0][0].as_str(), x2[0][6].as_str()), ("-X2/T:1", ""));
    assert!(to_smartscript_csv(&t).starts_with("\u{feff}Index;Type;Strip;"));
}

#[test]
fn a_terminal_tagged_with_only_its_strip_takes_the_strip_devices_part() {
    use wc_core::DevicePart;
    // -X2 in the fixture is drawn with the tag of the strip itself.
    let (mut p, s, mut parts) = fixture();
    let term = Part {
        id: "wago.2002-1201".into(),
        order_number: "2002-1201".into(),
        ..Default::default()
    };
    parts.insert(term.id.clone(), term.clone());
    p.devices.insert(
        DeviceTag::parse("-X2").unwrap(),
        Device {
            part: Some(term.id.clone()),
            accessories: vec![DevicePart {
                part: "acme.marker".into(),
                qty: 3,
            }],
            ..Default::default()
        },
    );
    let t = configurator_list(&Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    })
    .table;
    let x2: Vec<&Vec<String>> = t.rows.iter().filter(|r| r[2] == "-X2").collect();
    assert_eq!(x2.len(), 2, "{x2:?}");
    assert_eq!((x2[0][0].as_str(), x2[0][6].as_str()), ("-X2/T:1", "2002-1201"));
    // Its accessory belongs to terminal 1, and the part is not listed again as an accessory.
    assert_eq!(
        (
            x2[1][0].as_str(),
            x2[1][6].as_str(),
            x2[1][9].as_str(),
            x2[1][14].as_str()
        ),
        ("-X2/T:1/A1", "acme.marker", "3", "1")
    );
}

#[test]
fn an_accessory_of_terminals_drawn_as_the_strip_keeps_its_index_when_the_strip_is_reordered() {
    use wc_core::DevicePart;
    // Two terminals drawn with the tag of the strip itself: one device, -X2, for both.
    let (mut p, s, parts) = fixture();
    p.pages[0].symbols.push(place("s7", "term", 140.0, Some("-X2")));
    let x2 = DeviceTag::parse("-X2").unwrap();
    p.devices.insert(
        x2.clone(),
        Device {
            accessories: vec![DevicePart {
                part: "acme.marker".into(),
                qty: 3,
            }],
            ..Default::default()
        },
    );
    let accessory = |p: &Project| -> Vec<(String, String)> {
        configurator_list(&Inputs {
            project: p,
            symbols: &s,
            parts: &parts,
        })
        .table
        .rows
        .iter()
        .filter(|r| r[2] == "-X2" && r[1] == "Accessory")
        .map(|r| (r[0].clone(), r[14].clone()))
        .collect()
    };
    let before = accessory(&p);
    assert_eq!(before, [("-X2/T:1/A1".to_owned(), "1".to_owned())]);
    assert_eq!(p.meta.set_terminal_order(&x2, &["2".into(), "1".into()]), Ok(true));
    assert_eq!(
        accessory(&p),
        before,
        "named by the lowest terminal, not the first on the rail"
    );
}

#[test]
fn configurator_tags_with_a_function_or_location_aspect_are_written_as_text() {
    let (mut p, s, parts) = fixture();
    p.pages[0].symbols.push(place("s9", "term", 140.0, Some("=A+C1-X9.1")));
    let out = configurator_list(&Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    });
    let row = out.table.rows.iter().find(|r| r[2].contains("X9")).unwrap();
    assert_eq!(row[0], "'=A+C1-X9/T:1");
    assert_eq!(row[2], "'=A+C1-X9");
    assert!(
        out.warnings.iter().any(|w| w.contains("Strip \"=A+C1-X9\"")),
        "{:?}",
        out.warnings
    );
    assert!(out.warnings.iter().any(|w| w.contains("Index \"=A+C1-X9/T:1\"")));
    let plain = out.table.rows.iter().find(|r| r[0] == "-X1/T:1").unwrap();
    assert_eq!(plain[2], "-X1", "a plain tag stays as it is");
}

#[test]
fn configurator_free_texts_that_look_like_formulas_are_written_as_text() {
    let (mut p, s, mut parts) = fixture();
    let odd = Part {
        id: "odd".into(),
        order_number: "=1+1".into(),
        manufacturer: "@cme".into(),
        description: "plain".into(),
        ..Default::default()
    };
    parts.insert(odd.id.clone(), odd.clone());
    p.devices.insert(
        DeviceTag::parse("-X1.1").unwrap(),
        Device {
            part: Some(odd.id.clone()),
            ..Default::default()
        },
    );
    let out = configurator_list(&Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    });
    let row = out.table.rows.iter().find(|r| r[0] == "-X1/T:1").unwrap();
    assert_eq!(&row[6..9], ["'=1+1", "'@cme", "plain"]);
    assert_eq!(row[2], "-X1", "tags stay as they are");
    assert_eq!(out.warnings.len(), 2, "{:?}", out.warnings);
    assert!(out.warnings[0].contains("-X1/T:1") && out.warnings[0].contains("=1+1"));
}

#[test]
fn a_jumper_part_is_counted_in_the_parts_list_and_written_for_the_configurator() {
    let (mut p, s, mut parts) = fixture();
    let jumper = Part {
        id: "wago.2002-402".into(),
        manufacturer: "WAGO".into(),
        order_number: "2002-402".into(),
        description: "Push-in jumper, 2-way".into(),
        ..Default::default()
    };
    parts.insert(jumper.id.clone(), jumper.clone());
    let x1 = DeviceTag::parse("-X1").unwrap();
    p.meta.add_jumper(&x1, &["1".to_owned(), "2".to_owned()]).unwrap();
    p.meta.add_jumper(&x1, &["3".to_owned(), "4".to_owned()]).unwrap();
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    // Without parts, jumpers are not in the parts list and the configurator rows name no article.
    assert!(!parts_list(&i).rows.iter().any(|r| r[3] == "2002-402"));
    let rows = configurator_list(&i).table.rows;
    let j1 = rows.iter().find(|r| r[0] == "-X1/J1").unwrap();
    assert_eq!((j1[6].as_str(), j1[8].as_str()), ("", "2-way jumper"));

    for t in ["1", "3"] {
        p.meta.set_jumper_part(&x1, t, Some(&jumper.id)).unwrap();
    }
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let t = parts_list(&i);
    let row = t.rows.iter().find(|r| r[3] == "2002-402").unwrap();
    assert_eq!(
        (row[1].as_str(), row[2].as_str(), row[5].as_str()),
        ("2", "WAGO", "-X1")
    );
    let rows = configurator_list(&i).table.rows;
    let j1 = rows.iter().find(|r| r[0] == "-X1/J1").unwrap();
    assert_eq!(
        (j1[6].as_str(), j1[7].as_str(), j1[8].as_str()),
        ("2002-402", "WAGO", "Push-in jumper, 2-way")
    );
    // A part the project has no copy of is written by its id, with the jumper's own description.
    p.meta.set_jumper_part(&x1, "1", Some("acme.bridge")).unwrap();
    let rows = configurator_list(&Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    })
    .table
    .rows;
    let j1 = rows.iter().find(|r| r[0] == "-X1/J1").unwrap();
    assert_eq!((j1[6].as_str(), j1[8].as_str()), ("acme.bridge", "2-way jumper"));
}

#[test]
fn the_terminal_strip_editors_order_and_spares_show_in_the_plan_and_the_configurator() {
    let (mut p, s, mut parts) = fixture();
    let x1 = DeviceTag::parse("-X1").unwrap();
    let term = Part {
        id: "wago.2002-1201".into(),
        manufacturer: "WAGO".into(),
        order_number: "2002-1201".into(),
        description: "2-conductor through terminal block".into(),
        ..Default::default()
    };
    parts.insert(term.id.clone(), term.clone());
    // Terminal 2 first, a spare S1 between 2 and 1, jumpered with 1; the spare has a part.
    let current = vec![("1".to_owned(), false), ("2".to_owned(), false)];
    p.meta.move_terminal(&x1, "2", 0, &current).unwrap();
    let current = vec![("2".to_owned(), false), ("1".to_owned(), false)];
    p.meta.add_spare(&x1, "S1", Some("2"), &current).unwrap();
    p.meta.add_jumper(&x1, &["S1".to_owned(), "1".to_owned()]).unwrap();
    p.devices.insert(
        DeviceTag::parse("-X1.S1").unwrap(),
        Device {
            part: Some(term.id.clone()),
            ..Default::default()
        },
    );
    // A strip that is only spares.
    p.meta
        .add_spare(&DeviceTag::parse("-X9").unwrap(), "1", None, &[])
        .unwrap();
    let inputs = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let t = terminal_plan(&inputs);
    let x1_rows: Vec<(&str, &str, &str)> = t
        .rows
        .iter()
        .filter(|r| r[0] == "-X1")
        .map(|r| (r[1].as_str(), r[5].as_str(), r[6].as_str()))
        .collect();
    assert_eq!(x1_rows.len(), 3, "{x1_rows:?}");
    assert_eq!(
        (x1_rows[0].0, x1_rows[1], x1_rows[2].0),
        ("2", ("S1", SPARE_PAGE, "1"), "1")
    );
    // The drawn terminals keep their wiring.
    let one = t.rows.iter().find(|r| r[0] == "-X1" && r[1] == "1").unwrap();
    assert_eq!(one[2], "=A+C1-K1:14");
    assert_eq!(one[6], "S1");
    let x9 = t.rows.iter().find(|r| r[0] == "-X9").unwrap();
    assert_eq!((x9[1].as_str(), x9[5].as_str()), ("1", SPARE_PAGE));

    let out = configurator_list(&inputs);
    let terminals: Vec<(&str, &str, &str, &str)> = out
        .table
        .rows
        .iter()
        .filter(|r| r[1] == "Terminal" && r[2] == "-X1")
        .map(|r| (r[3].as_str(), r[4].as_str(), r[6].as_str(), r[0].as_str()))
        .collect();
    assert_eq!(
        terminals,
        [
            ("1", "2", "", "-X1/T:2"),
            ("2", "S1", "2002-1201", "-X1/T:S1"),
            ("3", "1", "", "-X1/T:1"),
        ]
    );
    // The spare's row imports back onto its own device; the index names the terminal, the
    // position its place on the rail.
    assert!(matches!(
        out.targets.get("-X1/T:S1"),
        Some(configurator::Target::Part(t)) if t.to_string() == "-X1.S1"
    ));
    // A spare without a device entry still names its own device for the import back.
    assert!(matches!(
        out.targets.get("-X9/T:1"),
        Some(configurator::Target::Part(t)) if t.to_string() == "-X9.1"
    ));
}

#[test]
fn a_rail_order_or_a_spare_keeps_levels_and_markers_and_a_jumpered_spare_takes_the_potential() {
    let (mut p, s, parts) = fixture();
    let x1 = DeviceTag::parse("-X1").unwrap();
    // -X1.1 and -X1.2 are one two-level terminal (the level above says so).
    p.devices
        .entry(DeviceTag::parse("-X1.2").unwrap())
        .or_default()
        .properties
        .insert(LEVEL_PROPERTY.into(), "2".into());
    // On the rail 2 comes first, and a spare 1a (between 1 and 2 by number) is added.
    let current = vec![("1".to_owned(), false), ("2".to_owned(), false)];
    p.meta.move_terminal(&x1, "2", 0, &current).unwrap();
    let current = vec![("2".to_owned(), false), ("1".to_owned(), false)];
    p.meta.add_spare(&x1, "1a", None, &current).unwrap();
    // -X2.1 is on 0V; a spare jumpered to it is too.
    let x2 = DeviceTag::parse("-X2").unwrap();
    p.meta.add_spare(&x2, "S1", None, &[("1".to_owned(), false)]).unwrap();
    p.meta.add_jumper(&x2, &["1".to_owned(), "S1".to_owned()]).unwrap();
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let plan = terminal_plan(&i);
    let levels: Vec<(&str, &str)> = plan
        .rows
        .iter()
        .filter(|r| r[0] == "-X1")
        .map(|r| (r[1].as_str(), r[7].as_str()))
        .collect();
    assert_eq!(levels, [("2", "2"), ("1", "1"), ("1a", "")]);
    let o = MarkingOptions {
        strip: Some("-X1".into()),
        level_separator: LEVEL_SEPARATOR.into(),
        ..MarkingOptions::default()
    };
    let marked: Vec<String> = marking_list_with(&i, &o).rows.iter().map(|r| r[2].clone()).collect();
    assert_eq!(marked, ["1 / 2", "1a"]);
    let spare = plan.rows.iter().find(|r| r[0] == "-X2" && r[1] == "S1").unwrap();
    assert_eq!((spare[2].as_str(), spare[4].as_str()), ("", "0V"));

    // A page numbered "spare" is not taken for a spare: the potential still comes through, and
    // the drawn terminals keep their marker order.
    for page in p.pages.iter_mut() {
        page.number = SPARE_PAGE.into();
    }
    // A drawn terminal 0 without a level, on that page, is not taken for a spare either. -X1 is
    // arranged, so its marker goes where the rail has it: a newly drawn terminal at the end.
    let mut zero = p
        .pages
        .iter()
        .flat_map(|pg| &pg.symbols)
        .find(|s| s.tag.as_ref().is_some_and(|t| t.to_string() == "-X1.1"))
        .cloned()
        .unwrap();
    zero.id = "zero".into();
    zero.at.x += 200.0;
    zero.tag = Some(DeviceTag::parse("-X1.0").unwrap());
    p.pages[0].symbols.push(zero);
    // A spare with a bad level does not join the inference: 1 keeps level 1.
    p.devices
        .entry(DeviceTag::parse("-X1.1a").unwrap())
        .or_default()
        .properties
        .insert(LEVEL_PROPERTY.into(), "x".into());
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let plan = terminal_plan(&i);
    let level_of = |n: &str| plan.rows.iter().find(|r| r[0] == "-X1" && r[1] == n).unwrap()[7].clone();
    assert_eq!((level_of("1"), level_of("1a")), ("1".to_owned(), String::new()));
    let spare = plan.rows.iter().find(|r| r[0] == "-X2" && r[1] == "S1").unwrap();
    assert_eq!(spare[4], "0V");
    let marked: Vec<String> = marking_list_with(&i, &o).rows.iter().map(|r| r[2].clone()).collect();
    let rail: Vec<&str> = plan
        .rows
        .iter()
        .filter(|r| r[0] == "-X1")
        .map(|r| r[1].as_str())
        .collect();
    assert_eq!(rail, ["2", "1", "1a", "0"]);
    assert_eq!(marked, ["1 / 2", "1a", "0"]);
    // The spare -X1.1a has a device entry, but it is marked once, as a terminal.
    let all = marking_list_with(&i, &MarkingOptions::default());
    let named: Vec<&Vec<String>> = all.rows.iter().filter(|r| r.iter().any(|c| c.contains("1a"))).collect();
    assert_eq!(named.len(), 1, "{named:?}");

    // A spare named like a cable (W1) with a level of its own is a terminal, not a cable end.
    p.meta.add_spare(&x1, "W1", None, &[]).unwrap();
    p.devices
        .entry(DeviceTag::parse("-X1.W1").unwrap())
        .or_default()
        .properties
        .insert(LEVEL_PROPERTY.into(), "1".into());
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let all = marking_list_with(&i, &MarkingOptions::default());
    let named: Vec<&Vec<String>> = all.rows.iter().filter(|r| r.iter().any(|c| c.contains("W1"))).collect();
    assert_eq!(named.len(), 1, "{named:?}");
    let terminal = MarkerKind::Terminals.label(MarkingOptions::default().language);
    assert_eq!(named[0][..3], [terminal.to_owned(), "-X1".to_owned(), "W1".to_owned()]);
}

#[test]
fn a_terminal_whose_symbol_is_missing_is_counted_the_same_everywhere() {
    let (mut p, s, parts) = fixture();
    // -X1.3 drawn with a symbol the project has no copy of; -K9 likewise, but no terminal.
    p.pages[0].symbols.push(place("s8", "gone.term", 150.0, Some("-X1.3")));
    p.pages[0].symbols.push(place("s9", "gone.relay", 160.0, Some("-K9")));
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    // The strip editor and jumpers count it ...
    let numbers = wc_core::project::terminal_numbers(&p, |id| s.get(id).map(|s| s.role));
    let x1 = DeviceTag::parse("-X1").unwrap();
    assert_eq!(
        numbers.get(&(p.pages[0].id.clone(), "s8".to_owned())),
        Some(&(x1, "3".to_owned()))
    );
    assert!(!numbers.contains_key(&(p.pages[0].id.clone(), "s9".to_owned())));
    // ... and so do the terminal plan (marked), the markers and the configurator (warned).
    let plan = terminal_plan(&i);
    let row = plan.rows.iter().find(|r| r[0] == "-X1" && r[1] == "3").unwrap();
    assert_eq!(row[2], "(symbol gone.term missing)");
    assert!(!plan.rows.iter().any(|r| r.iter().any(|c| c.contains("gone.relay"))));
    let markers = marking_list(&i, &[MarkerKind::Terminals]);
    assert!(
        markers.rows.iter().any(|r| r[1] == "-X1" && r[2] == "3"),
        "{:?}",
        markers.rows
    );
    // With devices asked for too, -X1.3 has its terminal marker only, not a device marker.
    let both = marking_list(&i, &[MarkerKind::Devices, MarkerKind::Terminals]);
    let device = MarkerKind::Devices.label(MarkingOptions::default().language);
    assert!(
        !both
            .rows
            .iter()
            .any(|r| r[0] == device && r.iter().any(|c| c.contains("X1.3"))),
        "{:?}",
        both.rows
    );
    assert!(
        both.rows.iter().any(|r| r[1] == "-X1" && r[2] == "3"),
        "{:?}",
        both.rows
    );
    let out = configurator_list(&i);
    assert!(
        out.table.rows.iter().any(|r| r.iter().any(|c| c == "-X1/T:3")),
        "{:?}",
        out.table.rows
    );
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("-X1.3") && w.contains("gone.term")),
        "{:?}",
        out.warnings
    );
}

#[test]
fn check_warns_about_a_cloud_of_an_unlisted_revision() {
    let (mut p, s, parts) = fixture();
    p.add_revision("A", "", "", "").unwrap();
    p.add_cloud("p2", Point::new(10.0, 10.0), Point::new(30.0, 30.0), None)
        .unwrap();
    let found = |p: &Project| -> Vec<Finding> {
        let i = Inputs {
            project: p,
            symbols: &s,
            parts: &parts,
        };
        check(&i)
            .into_iter()
            .filter(|f| f.message.contains("does not list"))
            .collect()
    };
    assert!(found(&p).is_empty());
    p.meta.revisions.clear();
    let f = found(&p);
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].severity, Severity::Warning);
    assert_eq!(f[0].page_id.as_deref(), Some("p2"));
    assert_eq!(f[0].item.as_deref(), Some("cloud 0"));
    assert_eq!(
        f[0].message,
        "a cloud carries revision A, which the project does not list"
    );
}

#[test]
fn check_warns_about_a_revision_letter_the_pdf_cannot_print() {
    let (mut p, s, parts) = fixture();
    p.add_revision("A", "", "", "").unwrap();
    let found = |p: &Project| -> Vec<Finding> {
        let i = Inputs {
            project: p,
            symbols: &s,
            parts: &parts,
        };
        check(&i)
            .into_iter()
            .filter(|f| f.message.contains("cannot print apart"))
            .collect()
    };
    assert!(found(&p).is_empty());
    p.meta.revisions[0].letter = "Ж".into();
    let f = found(&p);
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].severity, Severity::Warning);
    assert!(f[0].message.starts_with("revision Ж "), "{f:?}");
}

#[test]
fn check_finds_a_device_that_draws_more_contacts_than_its_parts_have() {
    let (mut p, mut s, mut parts) = fixture();
    s.insert("nc".into(), sym("nc", SymbolRole::Auxiliary, &["21", "22"]));
    let k1 = DeviceTag::parse("=A+C1-K1").unwrap();
    let overflow = |p: &Project, s: &BTreeMap<String, Symbol>, parts: &BTreeMap<String, Part>| -> Vec<Finding> {
        let i = Inputs {
            project: p,
            symbols: s,
            parts,
        };
        check(&i)
            .into_iter()
            .filter(|f| f.message.contains("too many"))
            .collect()
    };
    // The relay's part does not list its contacts: nothing to compare with.
    p.pages[0].symbols.push(place("s7", "no", 90.0, Some("=A+C1-K1")));
    assert!(overflow(&p, &s, &parts).is_empty());

    // One NO listed, two drawn: both drawings are named.
    parts.get_mut("acme.r24").unwrap().contacts = Some(wc_core::Contacts {
        no: 1,
        ..Default::default()
    });
    let found = overflow(&p, &s, &parts);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(
        found[0].message,
        "=A+C1-K1 draws 2 NO contacts, its parts have 1 NO (1 NO too many)"
    );
    assert_eq!(found[0].severity, Severity::Warning);

    // An auxiliary block fitted twice adds a NO and an NC each.
    let block = Part {
        id: "acme.aux11".into(),
        contacts: Some(wc_core::Contacts {
            no: 1,
            nc: 1,
            ..Default::default()
        }),
        ..Default::default()
    };
    parts.insert(block.id.clone(), block);
    p.devices.get_mut(&k1).unwrap().accessories.push(wc_core::DevicePart {
        part: "acme.aux11".into(),
        qty: 2,
    });
    p.pages[0].symbols.push(place("s8", "nc", 100.0, Some("=A+C1-K1")));
    p.pages[0].symbols.push(place("s9", "nc", 105.0, Some("=A+C1-K1")));
    assert!(overflow(&p, &s, &parts).is_empty());
    p.pages[0].symbols.push(place("s10", "nc", 106.0, Some("=A+C1-K1")));
    let found = overflow(&p, &s, &parts);
    assert!(found[0].message.ends_with("(1 NC too many)"), "{found:?}");

    // A contact repeated on an overview page does not count.
    p.pages[0].symbols.pop();
    let mut overview = Page::new("p9", "9", "Overview");
    overview.overview = true;
    overview.symbols.push(place("s30", "nc", 10.0, Some("=A+C1-K1")));
    p.pages.push(overview);
    assert!(overflow(&p, &s, &parts).is_empty());
}

#[test]
fn csv_cells_a_spreadsheet_would_run_are_written_as_text() {
    for (cell, out) in [
        ("=1+2", "'=1+2"),
        ("+C1", "'+C1"),
        ("@SUM(A1)", "'@SUM(A1)"),
        ("\tx", "'\tx"),
        ("\rx", "'\rx"),
        ("-cmd|' /C calc'!A0", "'-cmd|' /C calc'!A0"),
        ("=A+C1-K1", "'=A+C1-K1"),
        ("+C1-K1", "'+C1-K1"),
        // Excel reads -K1 as =-K1: product-aspect tags are guarded too.
        ("-K1", "'-K1"),
        ("-X12", "'-X12"),
        // A typed `'` before such text gets a second one, so the import keeps the first.
        ("'-K1", "''-K1"),
        ("'=1", "''=1"),
        ("''-K1", "'''-K1"),
        ("'quoted'", "'quoted'"),
        // Kept: ordinary text and plain numbers.
        ("Motor 3", "Motor 3"),
        ("", ""),
        ("-5", "-5"),
        ("-0.75", "-0.75"),
        ("-2,5", "-2,5"),
        ("a=b", "a=b"),
    ] {
        assert_eq!(formula_safe(cell), out, "{cell:?}");
    }
}

#[test]
fn every_csv_report_goes_through_the_guard_but_the_marker_list_keeps_its_text() {
    let mut t = Table::new("T", &["Tag", "Text"]);
    t.rows.push(vec!["-K1".into(), "=HYPERLINK(\"x\")".into()]);
    t.rows.push(vec!["-5".into(), "@x".into()]);
    let csv = t.to_csv();
    assert_eq!(csv, "Tag,Text\r\n'-K1,\"'=HYPERLINK(\"\"x\"\")\"\r\n-5,'@x\r\n");
    assert_eq!(t.to_csv_with(';'), csv.replace(',', ";"));
    // smartScript prints the text on the marker: it is written as it is.
    let raw = t.to_csv_raw_with(';');
    assert!(
        raw.contains(";\"=HYPERLINK(\"\"x\"\")\"\r\n") && raw.contains(";@x\r\n"),
        "{raw}"
    );
    assert!(to_smartscript_csv(&t).contains(";@x\r\n"));
}

#[test]
fn a_csv_header_that_looks_like_a_formula_is_written_as_text_too() {
    let mut t = Table::new("T", &["=1+2", "Tag"]);
    t.rows.push(vec!["a".into(), "-K1".into()]);
    assert!(t.to_csv().starts_with("'=1+2,Tag\r\n"), "{}", t.to_csv());
    assert!(t.to_csv_with(';').starts_with("'=1+2;Tag\r\n"));
    assert!(t.to_csv_raw_with(';').starts_with("=1+2;Tag\r\n"));
}

#[test]
fn a_report_layout_picks_orders_renames_and_sorts_columns() {
    use wc_core::report_layout::{ReportColumn, ReportLayout};
    let (mut p, s, mut parts) = fixture();
    // A second part, so the rows have something to sort.
    let part = Part {
        id: "zeta.lamp".into(),
        manufacturer: "Zeta".into(),
        order_number: "L-1".into(),
        description: "Lamp".into(),
        ..Default::default()
    };
    p.devices.insert(
        DeviceTag::parse("-P1").unwrap(),
        Device {
            part: Some(part.id.clone()),
            ..Default::default()
        },
    );
    parts.insert(part.id.clone(), part);
    let column = |from: &str, heading: Option<&str>| ReportColumn {
        from: from.into(),
        heading: heading.map(Into::into),
    };
    p.meta.reports.push(ReportLayout {
        kind: "parts".into(),
        title: Some("Bill of materials".into()),
        columns: vec![
            column("Order number", Some("Art. no.")),
            column("Manufacturer", None),
            column("Qty", None),
        ],
        sort: vec!["-Manufacturer".into()],
        ..Default::default()
    });
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let t = build(Kind::Parts, &i);
    assert_eq!(t.title, "Bill of materials");
    assert_eq!(t.columns, ["Art. no.", "Manufacturer", "Qty"]);
    // Descending by maker: Zeta, Acme, then the devices without a part (no maker).
    assert_eq!(t.rows[0], ["L-1", "Zeta", "1"]);
    assert_eq!(t.rows[1], ["R-24, DC", "Acme", "1"]);
    assert_eq!(t.rows.len(), 5);
    assert_eq!(
        t.to_csv().lines().take(3).collect::<Vec<_>>(),
        ["Art. no.,Manufacturer,Qty", "L-1,Zeta,1", "\"R-24, DC\",Acme,1"]
    );
    assert!(layout_warnings(&i).is_empty());
    // The built-in layout is still there for whoever asks for it.
    assert_eq!(build_builtin(Kind::Parts, &i).columns[0], "Pos");
    // Other reports keep theirs.
    assert_eq!(build(Kind::Devices, &i), device_list(&i));
}

#[test]
fn a_report_layout_keeps_page_links_with_their_rows_and_sorts_numbers_by_value() {
    use wc_core::report_layout::ReportLayout;
    let t = Table {
        title: "T".into(),
        columns: vec!["Page".into(), "Title".into()],
        rows: vec![
            vec!["2".into(), "b".into()],
            vec!["10".into(), "c".into()],
            vec!["1".into(), "a".into()],
        ],
        row_pages: vec!["p2".into(), "p10".into()],
        front: true,
        ..Default::default()
    };
    let sorted = t.with_layout(&ReportLayout {
        kind: "contents".into(),
        sort: vec!["Page".into()],
        ..Default::default()
    });
    assert_eq!(
        sorted.rows.iter().map(|r| r[0].as_str()).collect::<Vec<_>>(),
        ["1", "2", "10"]
    );
    assert_eq!(sorted.row_pages, ["", "p2", "p10"]);
    assert!(sorted.front);
}

#[test]
fn a_sorted_cable_list_keeps_each_core_with_its_cable() {
    use wc_core::report_layout::ReportLayout;
    let (mut p, s, parts) = fixture();
    let w1 = DeviceTag::parse("-W1").unwrap();
    let w2 = DeviceTag::parse("-W2").unwrap();
    for (id, cable, core) in [("w1", &w1, "1"), ("w2", &w1, "2"), ("w3", &w2, "1"), ("w4", &w2, "2")] {
        if let Some(w) = p.pages[0].wires.iter_mut().find(|w| w.id == id) {
            w.cable = Some(wc_core::CableCore {
                cable: cable.clone(),
                core: core.into(),
            });
        }
    }
    // Without a sort, the built-in grouping is kept.
    p.meta.reports.push(ReportLayout {
        kind: "cables".into(),
        ..Default::default()
    });
    let i2 = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let grouped = build(Kind::Cables, &i2);
    let firsts: Vec<&str> = grouped.rows.iter().map(|r| r[0].as_str()).collect();
    assert_eq!(firsts, ["-W1", "", "-W2", ""]);
    // Sorted by core, every row names its own cable.
    p.meta.reports[0].sort = vec!["Core".into(), "-Cable".into()];
    let i3 = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let sorted = build(Kind::Cables, &i3);
    let pairs: Vec<(&str, &str)> = sorted.rows.iter().map(|r| (r[0].as_str(), r[3].as_str())).collect();
    assert_eq!(pairs, [("-W2", "1"), ("-W1", "1"), ("-W2", "2"), ("-W1", "2")]);
    // A draft layout (the layout dialog's preview) gets the same fill-down without being
    // kept; no layout given is the built-in one, whatever the project keeps.
    let saved = p.meta.reports.clone();
    let draft = ReportLayout {
        kind: "cables".into(),
        sort: vec!["Core".into()],
        ..Default::default()
    };
    let i4 = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let preview = build_with(Kind::Cables, &i4, Some(&draft));
    let pairs: Vec<(&str, &str)> = preview.rows.iter().map(|r| (r[0].as_str(), r[3].as_str())).collect();
    assert_eq!(pairs, [("-W1", "1"), ("-W2", "1"), ("-W1", "2"), ("-W2", "2")]);
    let builtin = build_with(Kind::Cables, &i4, None);
    let firsts: Vec<&str> = builtin.rows.iter().map(|r| r[0].as_str()).collect();
    assert_eq!(firsts, ["-W1", "", "-W2", ""]);
    assert_eq!(p.meta.reports, saved);
}

#[test]
fn a_cable_list_grouped_by_cable_without_a_sort_keeps_each_core_in_its_cable_group() {
    use wc_core::report_layout::ReportLayout;
    let (mut p, s, parts) = fixture();
    let w1 = DeviceTag::parse("-W1").unwrap();
    let w2 = DeviceTag::parse("-W2").unwrap();
    for (id, cable, core) in [("w1", &w1, "1"), ("w2", &w1, "2"), ("w3", &w2, "1"), ("w4", &w2, "2")] {
        if let Some(w) = p.pages[0].wires.iter_mut().find(|w| w.id == id) {
            w.cable = Some(wc_core::CableCore {
                cable: cable.clone(),
                core: core.into(),
            });
        }
    }
    p.meta.reports.push(ReportLayout {
        kind: "cables".into(),
        group: Some("Cable".into()),
        ..Default::default()
    });
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let t = build(Kind::Cables, &i);
    let g = t.group.unwrap();
    let pairs: Vec<(&str, &str)> = t.rows.iter().map(|r| (r[g].as_str(), r[3].as_str())).collect();
    assert_eq!(pairs, [("-W1", "1"), ("-W1", "2"), ("-W2", "1"), ("-W2", "2")]);
}

#[test]
fn a_layout_sorts_by_every_key_in_turn_and_keeps_the_built_in_order_on_ties() {
    use wc_core::report_layout::ReportLayout;
    let row = |a: &str, b: &str, n: &str| vec![a.to_string(), b.to_string(), n.to_string()];
    let t = Table {
        title: "T".into(),
        columns: vec!["Group".into(), "Item".into(), "Seen".into()],
        rows: vec![
            row("B", "1", "first"),
            row("A", "2", "second"),
            row("B", "2", "third"),
            row("A", "1", "fourth"),
            row("B", "2", "fifth"),
        ],
        ..Default::default()
    };
    let sorted = t.with_layout(&ReportLayout {
        kind: "contents".into(),
        sort: vec!["Group".into(), "-Item".into()],
        ..Default::default()
    });
    assert_eq!(
        sorted.rows.iter().map(|r| r[2].as_str()).collect::<Vec<_>>(),
        ["second", "fourth", "third", "fifth", "first"]
    );
}

#[test]
fn a_layout_groups_rows_by_a_column_and_the_csv_keeps_one_row_per_record() {
    use wc_core::report_layout::{ReportColumn, ReportLayout};
    let row = |loc: &str, wire: &str| vec![loc.to_string(), wire.to_string(), format!("page of {wire}")];
    let t = Table {
        title: "Wires".into(),
        columns: vec!["Location".into(), "Wire".into(), "Page".into()],
        rows: vec![row("B1", "W1"), row("A1", "W3"), row("B1", "W2"), row("A1", "W4")],
        row_pages: vec!["p1".into(), "p3".into(), "p2".into(), "p4".into()],
        ..Default::default()
    };
    // The group column is left out of the picked columns: it comes first all the same.
    let grouped = t.clone().with_layout(&ReportLayout {
        kind: "wires".into(),
        columns: vec![ReportColumn {
            from: "Wire".into(),
            heading: None,
        }],
        sort: vec!["-Wire".into()],
        group: Some("Location".into()),
        header: vec!["project.name".into(), "revision".into()],
        ..Default::default()
    });
    assert_eq!(grouped.columns, ["Location", "Wire"]);
    assert_eq!(grouped.group, Some(0));
    assert_eq!(grouped.header, ["project.name", "revision"]);
    // Groups in the order of their first row after sorting (W4 of A1 first), each sorted.
    assert_eq!(
        grouped.to_csv(),
        "Location,Wire\r\nA1,W4\r\nA1,W3\r\nB1,W2\r\nB1,W1\r\n"
    );
    assert_eq!(
        grouped.row_pages,
        ["p4", "p3", "p2", "p1"],
        "links stay with their rows"
    );
    // A group column the table does not have groups nothing.
    let none = t.with_layout(&ReportLayout {
        kind: "wires".into(),
        group: Some("Colour".into()),
        ..Default::default()
    });
    assert_eq!(none.group, None);
    assert_eq!(none.columns.len(), 3);
}

#[test]
fn layout_headings_with_a_pipe_or_a_line_break_keep_the_markdown_table_whole() {
    use wc_core::report_layout::{ReportColumn, ReportLayout};
    let mut t = Table::new("T", &["Tag", "Text"]);
    t.rows.push(vec!["-K1".into(), "Relay".into()]);
    let t = t.with_layout(&ReportLayout {
        kind: "devices".into(),
        title: Some("Two\nlines".into()),
        columns: vec![
            ReportColumn {
                from: "Tag".into(),
                heading: Some("Part | number".into()),
            },
            ReportColumn {
                from: "Text".into(),
                heading: Some("Long\ntext".into()),
            },
        ],
        ..Default::default()
    });
    assert_eq!(
        t.to_markdown(),
        "## Two lines\n\n| Part \\| number | Long text |\n|---|---|\n| -K1 | Relay |\n"
    );
    // Carriage returns end a line too, and angle brackets are not read as HTML.
    let mut t = Table::new("A\r\nB\rC", &["<b>Tag</b>"]);
    t.rows.push(vec!["x\r\ny <script>".into()]);
    assert_eq!(
        t.to_markdown(),
        "## A B C\n\n| &lt;b&gt;Tag&lt;/b&gt; |\n|---|\n| x y &lt;script&gt; |\n"
    );
}

#[test]
fn report_layouts_that_name_nothing_are_reported() {
    use wc_core::report_layout::{ReportColumn, ReportLayout};
    let (mut p, s, parts) = fixture();
    p.meta.reports = vec![
        ReportLayout {
            kind: "bom".into(),
            ..Default::default()
        },
        ReportLayout {
            kind: "wires".into(),
            columns: vec![ReportColumn {
                from: "Shade".into(),
                heading: None,
            }],
            sort: vec!["-Length".into()],
            ..Default::default()
        },
        ReportLayout {
            kind: "wires".into(),
            ..Default::default()
        },
    ];
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let w = layout_warnings(&i);
    assert_eq!(w.len(), 4, "{w:?}");
    assert!(w[0].contains("\"bom\": no such report"));
    assert!(w[1].contains("no column \"Shade\""));
    assert!(w[2].contains("cannot sort by \"Length\""));
    assert!(w[3].contains("a second one"));
    // The unknown column is left out; the report still builds.
    assert!(build(Kind::Wires, &i).columns.is_empty());
}

#[test]
fn a_report_layouts_unknown_group_column_and_header_keys_are_reported() {
    use wc_core::report_layout::ReportLayout;
    let (mut p, s, parts) = fixture();
    p.meta.title_block.insert("contract".into(), "K-7".into());
    p.pages[0].title_block.insert("approved_by".into(), "AB".into());
    p.meta.reports = vec![ReportLayout {
        kind: "wires".into(),
        group: Some(" Maker ".into()),
        header: [
            "project.name",
            "revision",
            "contract",
            "approved_by",
            "page.sheet",
            "revison",
        ]
        .map(String::from)
        .to_vec(),
        ..Default::default()
    }];
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let w = layout_warnings(&i);
    assert_eq!(w.len(), 2, "{w:?}");
    assert!(w[0].contains("cannot group by \"Maker\", no such column"), "{w:?}");
    assert!(w[1].contains("header \"revison\" is not a title-block field"), "{w:?}");
    assert!(w[1].contains("contract") && w[1].contains("approved_by"), "{w:?}");

    // A group column the report has, and known header keys only: nothing to say.
    let column = build(Kind::Wires, &i).columns[0].clone();
    p.meta.reports[0].group = Some(column);
    p.meta.reports[0].header.pop();
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    assert!(layout_warnings(&i).is_empty(), "{:?}", layout_warnings(&i));
}

#[test]
fn check_runs_the_projects_own_rules() {
    let (mut p, s, parts) = fixture();
    use wc_core::check_rule::CheckRule;
    let rule = |kind: &str| CheckRule {
        kind: kind.into(),
        ..CheckRule::default()
    };
    p.meta.rules = vec![
        CheckRule {
            devices: Some("-P*".into()),
            ..rule("cable")
        },
        CheckRule {
            role: Some("terminal".into()),
            pattern: Some("-X#.#".into()),
            severity: Some("error".into()),
            ..rule("tag")
        },
        CheckRule {
            max: Some(1),
            message: Some("{tag} is overcrowded".into()),
            ..rule("max-wires")
        },
        rule("colour"),
        CheckRule {
            role: Some("coil".into()),
            pattern: Some("-K#".into()),
            ..rule("tag")
        },
        CheckRule {
            devices: Some("-K*".into()),
            ..rule("tag")
        },
        CheckRule {
            devices: Some("-M*".into()),
            ..rule("")
        },
        CheckRule {
            pattern: Some("-K#".into()),
            unknown: [("severty".to_owned(), "error".into())].into_iter().collect(),
            ..rule("tag")
        },
    ];
    // A second wire on terminal -X1.1 pin 1.
    p.pages[0]
        .wires
        .push(wire("w5", pin("s4", "1"), pin("s6", "X2"), Some("4"), None));
    let run = |p: &Project| {
        check(&Inputs {
            project: p,
            symbols: &s,
            parts: &parts,
        })
    };
    let found = run(&p);
    let has = |m: &str| found.iter().find(|f| f.message == m);
    let cable = has("-P1 has no cable (check rule 1)").unwrap();
    assert_eq!((cable.severity, cable.item.as_deref()), (Severity::Warning, Some("s6")));
    let bad_tag = has("-X2 does not match -X#.# (check rule 2)").unwrap();
    assert_eq!(
        (bad_tag.severity, bad_tag.item.as_deref()),
        (Severity::Error, Some("s5"))
    );
    assert!(!found.iter().any(|f| f.message.contains("-X1.1 does not match")));
    // Only terminals count for max-wires without a role: the lamp's pin X2 is not checked.
    assert!(has("-X1.1 is overcrowded").is_some(), "{found:?}");
    assert_eq!(found.iter().filter(|f| f.message.contains("overcrowded")).count(), 1);
    assert!(has("check rule 4 cannot run: kind \"colour\" is not one of cable, tag, max-wires").is_some());
    assert!(
        found
            .iter()
            .any(|f| f.message.starts_with("check rule 5 cannot run: role \"coil\""))
    );
    assert!(has("check rule 6 cannot run: a tag rule needs a pattern").is_some());
    assert!(has("check rule 7 cannot run: it has no kind (cable, tag, max-wires)").is_some());
    assert!(
        found.iter().any(|f| f.severity == Severity::Error
            && f.message
                .starts_with("check rule 8 cannot run: it has a key \"severty\"")),
        "{found:?}"
    );

    // A cable on the lamp's wire satisfies the cable rule.
    p.pages[0].wires[2].cable = Some(wc_core::CableCore {
        cable: DeviceTag::parse("-W1").unwrap(),
        core: "1".into(),
    });
    assert!(!run(&p).iter().any(|f| f.message.contains("has no cable")));
    // Overview pages do not count.
    let mut overview = Page::new("p9", "9", "Overview");
    overview.overview = true;
    overview.symbols.push(place("s40", "lamp", 10.0, Some("-P7")));
    p.pages.push(overview);
    assert!(!run(&p).iter().any(|f| f.message.contains("has no cable")));
}

#[test]
fn library_rules_run_before_the_projects_name_their_source_and_can_be_turned_off() {
    use wc_core::check_rule::{CheckRule, RuleSet};
    let (mut p, s, parts) = fixture();
    let cable = CheckRule {
        name: Some("lamp-cable".into()),
        kind: "cable".into(),
        devices: Some("-P*".into()),
        ..CheckRule::default()
    };
    let tag = CheckRule {
        kind: "tag".into(),
        role: Some("terminal".into()),
        pattern: Some("-X#.#".into()),
        severity: Some("error".into()),
        ..CheckRule::default()
    };
    let library = vec![
        RuleSet {
            source: "house".into(),
            rules: Ok(vec![cable, tag]),
        },
        RuleSet {
            source: "broken".into(),
            rules: Err("expected `]`".into()),
        },
    ];
    // The project's own rule is numbered on its own.
    p.meta.rules = vec![CheckRule {
        kind: "cable".into(),
        devices: Some("-P*".into()),
        message: Some("{tag} wants a cable".into()),
        ..CheckRule::default()
    }];
    let run = |p: &Project| {
        check_with(
            &Inputs {
                project: p,
                symbols: &s,
                parts: &parts,
            },
            &library,
        )
    };
    let messages = |p: &Project| run(p).into_iter().map(|f| f.message).collect::<Vec<_>>();
    let found = messages(&p);
    let has = |m: &str| found.iter().any(|f| f == m);
    assert!(
        has("-P1 has no cable (check rule 1 \"lamp-cable\" of library house)"),
        "{found:?}"
    );
    assert!(
        has("-X2 does not match -X#.# (check rule 2 of library house)"),
        "{found:?}"
    );
    assert!(has("-P1 wants a cable"), "{found:?}");
    assert!(
        has("the check rules of library broken cannot be read: expected `]`"),
        "{found:?}"
    );
    // Without the library, only the project's rule.
    let alone = check(&Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    });
    assert!(!alone.iter().any(|f| f.message.contains("of library")));

    // Turned off by name and by number; an entry that names nothing is reported, one for
    // the source that could not be read is not.
    p.meta.rules_off = vec![
        "house:lamp-cable".into(),
        "house:2".into(),
        "house:9".into(),
        "broken:1".into(),
    ];
    let found = messages(&p);
    assert!(!found.iter().any(|f| f.contains("of library house")), "{found:?}");
    assert!(found.iter().any(|f| f == "-P1 wants a cable"));
    let unknown: Vec<_> = found.iter().filter(|f| f.starts_with("rules_off names")).collect();
    assert_eq!(unknown.len(), 1, "{found:?}");
    assert!(unknown[0].contains("\"house:9\""));
}

#[test]
fn a_library_rules_own_message_still_names_the_rule_and_its_source() {
    use wc_core::check_rule::{CheckRule, RuleSet};
    let (p, s, parts) = fixture();
    let library = vec![RuleSet {
        source: "house".into(),
        rules: Ok(vec![
            CheckRule {
                kind: "cable".into(),
                devices: Some("-P*".into()),
                message: Some("{tag} wants a house cable".into()),
                ..CheckRule::default()
            },
            CheckRule {
                kind: "tag".into(),
                role: Some("terminal".into()),
                pattern: Some("-X#.#".into()),
                message: Some("{tag} breaks the house tags".into()),
                ..CheckRule::default()
            },
        ]),
    }];
    let found: Vec<String> = check_with(
        &Inputs {
            project: &p,
            symbols: &s,
            parts: &parts,
        },
        &library,
    )
    .into_iter()
    .map(|f| f.message)
    .collect();
    let has = |m: &str| found.iter().any(|f| f == m);
    assert!(
        has("-P1 wants a house cable (check rule 1 of library house)"),
        "{found:?}"
    );
    assert!(
        has("-X2 breaks the house tags (check rule 2 of library house)"),
        "{found:?}"
    );
}

#[test]
fn plc_card_overview_lays_out_three_cards_like_the_io_list() {
    use wc_core::plc_rack::{PlcCard, PlcRack};
    let (mut p, mut s, parts) = fixture();
    let mut input = sym("wc.plc.input", SymbolRole::Standalone, &["I"]);
    input.category = "plc".into();
    input.name = "PLC digital input".into();
    let mut output = sym("wc.plc.output", SymbolRole::Standalone, &["Q"]);
    output.category = "plc".into();
    for y in [input, output] {
        s.insert(y.id.clone(), y);
    }
    let page = &mut p.pages[0];
    page.symbols.push(place("i1", "wc.plc.input", 300.0, Some("-A1.I1")));
    page.symbols.push(place("i2", "wc.plc.input", 310.0, Some("-A1.I2")));
    page.symbols.push(place("q1", "wc.plc.output", 320.0, Some("-A1.Q1")));
    page.symbols.push(place("a1", "wc.plc.input", 330.0, Some("-B1")));
    page.symbols.push(place("x1", "wc.plc.input", 340.0, Some("-B9")));
    page.wires.push(wire("w9", pin("s6", "X2"), pin("q1", "Q"), None, None));
    for (tag, addr) in [
        ("-A1.I1", "I0.1"),
        ("-A1.I2", "%IX1.0"),
        ("-A1.Q1", "A4.1"),
        ("-B1", "IW66"),
        ("-B9", "%I9.0"),
    ] {
        p.devices.insert(
            DeviceTag::parse(tag).unwrap(),
            Device {
                description: format!("point {tag}"),
                properties: [("Address".to_string(), addr.to_string())].into_iter().collect(),
                ..Default::default()
            },
        );
    }
    let card = |slot: u32, tag: &str, kind: &str, channels: u32, start: &str| PlcCard {
        slot,
        tag: tag.into(),
        kind: kind.into(),
        channels,
        start: start.into(),
        ..Default::default()
    };
    p.meta.racks.push(PlcRack {
        name: "-A1".into(),
        description: "Main PLC".into(),
        // Out of slot order on purpose; the overview sorts them.
        cards: vec![
            card(3, "", "AI 2x 4-20 mA", 2, "%IW64"),
            card(1, "-A1.DI1", "DI 16", 16, "%IX0.0"),
            card(2, "-A1.DO1", "DO 8", 8, "%QX4.0"),
        ],
        ..Default::default()
    });
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let t = build(Kind::PlcCards, &i);
    assert_eq!(t.columns[0..6], ["Rack", "Slot", "Card", "Type", "Channel", "Address"]);
    assert_eq!(t.rows.len(), 16 + 8 + 2);
    let row = |addr: &str| t.rows.iter().position(|r| r[5] == addr).unwrap();
    // Every drawn point on a card shows the same device, description, connection and page as
    // in the PLC I/O list.
    let io = build(Kind::Plc, &i);
    for (addr, tag) in [
        ("%IX0.1", "-A1.I1"),
        ("%IX1.0", "-A1.I2"),
        ("%QX4.1", "-A1.Q1"),
        ("%IW66", "-B1"),
    ] {
        let r = &t.rows[row(addr)];
        let io_row = io.rows.iter().find(|x| x[2] == tag).unwrap();
        assert_eq!(r[6..], io_row[2..], "{addr}");
        assert_eq!(t.row_pages[row(addr)], p.pages[0].id, "{addr}");
    }
    assert_eq!(t.rows[row("%QX4.1")][8], "-P1:X2");
    // Slot order, channel numbers from 0, and an untagged card named by its type.
    assert_eq!(t.rows[0][..6], ["-A1", "1", "-A1.DI1", "DI 16", "0", "%IX0.0"]);
    assert_eq!(t.rows[16][..6], ["-A1", "2", "-A1.DO1", "DO 8", "0", "%QX4.0"]);
    assert_eq!(
        t.rows[25][..6],
        ["-A1", "3", "AI 2x 4-20 mA", "AI 2x 4-20 mA", "1", "%IW66"]
    );
    // A spare channel is listed empty and links nowhere.
    assert_eq!(t.rows[0][6..], ["", "", "", ""]);
    assert_eq!(t.row_pages.first().map(String::as_str).unwrap_or(""), "");
    // The point at %IX9.0 is on no card.
    let w = plc_rack_warnings(&i);
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("-B9") && w[0].contains("%IX9.0"), "{w:?}");

    // Broken racks are reported, not laid out.
    p.meta.racks[0].cards.push(card(2, "", "", 4, "%QX4.4"));
    p.meta.racks[0].cards.push(card(5, "", "", 4, "Slot 5"));
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let w = plc_rack_warnings(&i);
    assert!(w.iter().any(|w| w.contains("slot 2: a second card")), "{w:?}");
    assert!(
        w.iter()
            .any(|w| w.contains("address %QX4.4 is also on PLC rack \"-A1\" slot 2")),
        "{w:?}"
    );
    assert!(
        w.iter().any(|w| w.contains("slot 5: start address \"Slot 5\"")),
        "{w:?}"
    );
    assert_eq!(build(Kind::PlcCards, &i).rows.len(), 16 + 8 + 2 + 4);

    // No racks: an empty overview and nothing to warn about.
    p.meta.racks.clear();
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    assert!(build(Kind::PlcCards, &i).rows.is_empty());
    assert!(plc_rack_warnings(&i).is_empty());
}

#[test]
fn plc_card_overview_links_by_page_id_and_warns_on_points_and_shared_bytes() {
    use wc_core::plc_rack::{PlcCard, PlcRack};
    let (mut p, mut s, parts) = fixture();
    let mut input = sym("wc.plc.input", SymbolRole::Standalone, &["I"]);
    input.category = "plc".into();
    input.name = "PLC digital input".into();
    let mut output = sym("wc.plc.output", SymbolRole::Standalone, &["Q"]);
    output.category = "plc".into();
    for y in [input, output] {
        s.insert(y.id.clone(), y);
    }
    // A second page with the same number as the first, as a hand-edited project file may have.
    let mut twin = Page::new("p9", "2", "Twin");
    twin.symbols.push(place("t1", "wc.plc.input", 10.0, Some("-T1")));
    p.pages[0]
        .symbols
        .push(place("i1", "wc.plc.input", 300.0, Some("-A1.I1")));
    p.pages[0]
        .symbols
        .push(place("q1", "wc.plc.output", 310.0, Some("-Q9")));
    p.pages[0].symbols.push(place("i2", "wc.plc.input", 320.0, Some("-I9")));
    p.pages[0]
        .symbols
        .push(place("i3", "wc.plc.input", 330.0, Some("-I10")));
    p.pages[0]
        .symbols
        .push(place("i4", "wc.plc.input", 340.0, Some("-I11")));
    p.pages.push(twin);
    for (tag, addr) in [
        ("-A1.I1", "%IX0.0"),
        ("-T1", "%IX0.1"),
        ("-Q9", "%IX0.2"),
        ("-I9", "Slot 1"),
        ("-I10", "%I0.10"),
        ("-I11", ""),
    ] {
        p.devices.insert(
            DeviceTag::parse(tag).unwrap(),
            Device {
                properties: [("Address".to_string(), addr.to_string())].into_iter().collect(),
                ..Default::default()
            },
        );
    }
    let card = |slot: u32, channels: u32, start: &str| PlcCard {
        slot,
        channels,
        start: start.into(),
        ..Default::default()
    };
    p.meta.racks.push(PlcRack {
        name: "R".into(),
        cards: vec![
            card(1, 8, "%IX0.0"),
            card(2, 1, "%IW64"),
            card(3, 1, "%IB65"),
            card(4, 1, "%IB66"),
        ],
        ..Default::default()
    });
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let t = build(Kind::PlcCards, &i);
    // Each point links to the page it is drawn on, though both pages are numbered 2.
    let link = |dev: &str| {
        let n = t.rows.iter().position(|r| r[6] == dev).unwrap();
        t.row_pages.get(n).cloned().unwrap_or_default()
    };
    assert_eq!(link("-A1.I1"), "p2");
    assert_eq!(link("-T1"), "p9");
    let w = plc_rack_warnings(&i);
    let has = |s: &str| w.iter().any(|w| w.contains(s));
    // An output at an input address, an address that is no PLC address and bit 10 are on no
    // card; a point without an address is left to the I/O list.
    assert!(has("-Q9 (page 2): address %IX0.2 is an input"), "{w:?}");
    assert!(has("-I9 (page 2): address \"Slot 1\" is not a PLC address"), "{w:?}");
    assert!(has("-I10 (page 2): address %IX0.10 names no bit"), "{w:?}");
    assert!(!has("-I11"), "{w:?}");
    // %IW64 covers bytes 64 and 65: the byte card at %IB65 shares one, the one at %IB66 none.
    assert!(has("slot 3: address %IB65 is also on PLC rack \"R\" slot 2"), "{w:?}");
    assert!(!has("slot 4"), "{w:?}");
    assert_eq!(w.len(), 4, "{w:?}");
    // A card with its own step is compared by address only.
    p.meta.racks[0].cards[2].step = Some(1);
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    assert!(!plc_rack_warnings(&i).iter().any(|w| w.contains("slot 3")));
}

#[test]
fn rows_of_the_wire_cable_terminal_and_plc_lists_link_to_their_page() {
    let (mut p, mut s, parts) = fixture();
    let mut input = sym("wc.plc.input", SymbolRole::Standalone, &["I"]);
    input.category = "plc".into();
    input.name = "PLC digital input".into();
    s.insert(input.id.clone(), input);
    // A second page, numbered 7 with the id p7, with a terminal, a PLC input and a cable core.
    let mut second = Page::new("p7", "7", "Field");
    second.symbols = vec![
        place("t1", "term", 10.0, Some("-X1.3")),
        place("in1", "wc.plc.input", 40.0, Some("-A1.I1")),
    ];
    let mut core = wire("w20", pin("t1", "2"), pin("in1", "I"), Some("20"), None);
    core.cable = Some(wc_core::CableCore {
        cable: DeviceTag::parse("-W1").unwrap(),
        core: "2".into(),
    });
    second.wires.push(core);
    p.pages.push(second);
    if let Some(w) = p.pages[0].wires.iter_mut().find(|w| w.id == "w2") {
        w.cable = Some(wc_core::CableCore {
            cable: DeviceTag::parse("-W1").unwrap(),
            core: "1".into(),
        });
    }
    p.meta.strips.push(wc_core::project::StripLayout {
        strip: DeviceTag::parse("-X1").unwrap(),
        spares: vec!["9".into()],
        order: vec![],
    });
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let id_of = |number: &str| p.pages.iter().find(|pg| pg.number == number).map(|pg| pg.id.clone());
    for (kind, page_column) in [
        (Kind::Wires, 0),
        (Kind::Cables, 4),
        (Kind::Terminals, 5),
        (Kind::Plc, 5),
    ] {
        let t = build(kind, &i);
        assert!(!t.rows.is_empty(), "{kind:?}");
        for (n, row) in t.rows.iter().enumerate() {
            let link = t.row_pages.get(n).cloned().unwrap_or_default();
            let want = id_of(&row[page_column]).unwrap_or_default();
            assert_eq!(link, want, "{kind:?} row {row:?}");
        }
        assert!(t.row_pages.iter().any(|l| l == "p7"), "{kind:?}");
    }
    // The spare terminal is on no page and links nowhere.
    let t = build(Kind::Terminals, &i);
    let spare = t.rows.iter().position(|r| r[5] == SPARE_PAGE).unwrap();
    assert_eq!(t.row_pages.get(spare).map(String::as_str).unwrap_or(""), "");
    // A sorted layout keeps each row's link with it.
    let layout = ReportLayout {
        sort: vec!["-Page".into()],
        ..Default::default()
    };
    let sorted = build_with(Kind::Wires, &i, Some(&layout));
    assert_eq!(sorted.rows[0][0], "7");
    assert_eq!(sorted.row_pages[0], "p7");
    assert_eq!(sorted.row_pages[1], "p2");
}

#[test]
fn check_warns_about_panel_devices_that_overlap_stick_out_or_left_the_project() {
    use wc_core::panel::Panel;
    let (mut p, s, parts) = fixture();
    let tag = |t: &str| {
        let Ok(tag) = DeviceTag::parse(t) else { unreachable!() };
        tag
    };
    let mut plate = Panel::new("p1", "Main", 400.0, 300.0);
    let place = |plate: &mut Panel, t: &str, x: f64, y: f64, r: Rotation| {
        let Ok(()) = plate.place_device(tag(t), Point::new(x, y), (40.0, 80.0), r) else {
            unreachable!()
        };
    };
    place(&mut plate, "=A+C1-K1", 100.0, 0.0, Rotation::R0);
    // Turned, -X1.1 covers 80 x 40 from (60, 50): it overlaps -K1 (100..140 x 0..80).
    place(&mut plate, "-X1.1", 60.0, 50.0, Rotation::R90);
    // -P1 touches -K1's and -X1.1's right edges: a gap of 0 is no overlap.
    place(&mut plate, "-P1", 140.0, 0.0, Rotation::R0);
    // -X2 lies partly off the plate (as a hand-edited file can have it).
    place(&mut plate, "-X2", 300.0, 200.0, Rotation::R0);
    if let Some(wc_core::panel::PanelItem::Device { at, .. }) = plate.items.last_mut() {
        *at = Point::new(380.0, 250.0);
    }
    // -K9 is drawn nowhere and has no device entry.
    place(&mut plate, "-K9", 200.0, 100.0, Rotation::R0);
    p.panels.push(plate);
    let i = Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    };
    let found: Vec<String> = check(&i)
        .into_iter()
        .filter(|f| f.message.starts_with("panel "))
        .map(|f| f.message)
        .collect();
    let has = |m: &str| found.iter().any(|f| f == m);
    assert!(has("panel Main: =A+C1-K1 and -X1.1 overlap"), "{found:?}");
    assert!(has("panel Main: -X2 sticks out of the plate"), "{found:?}");
    assert!(
        has("panel Main: -K9 is on the plate but no longer in the project"),
        "{found:?}"
    );
    assert_eq!(found.len(), 3, "{found:?}");
}

#[test]
fn check_keeps_panel_devices_whose_tag_is_only_drawn_as_interruption_points() {
    use wc_core::panel::Panel;
    let (mut p, mut s, parts) = fixture();
    let arrow = sym("arrow", SymbolRole::Interruption, &["1"]);
    s.insert(arrow.id.clone(), arrow);
    p.pages[0].symbols.push(place("s9", "arrow", 200.0, Some("-L1")));
    let Ok(l1) = DeviceTag::parse("-L1") else {
        unreachable!()
    };
    p.devices.entry(l1.clone()).or_default();
    let mut plate = Panel::new("p1", "Main", 400.0, 300.0);
    let Ok(()) = plate.place_device(l1, Point::new(10.0, 10.0), (40.0, 80.0), Rotation::R0) else {
        unreachable!()
    };
    p.panels.push(plate);
    let found: Vec<String> = check(&Inputs {
        project: &p,
        symbols: &s,
        parts: &parts,
    })
    .into_iter()
    .map(|f| f.message)
    .collect();
    assert!(!found.iter().any(|m| m.starts_with("panel ")), "{found:?}");
    // The tag still only names a connection, so it gets no "has no part" note.
    assert!(!found.iter().any(|m| m == "-L1 has no part"), "{found:?}");
}

#[test]
fn markdown_cells_with_a_backslash_keep_their_columns() {
    let mut t = Table::new("Paths", &["Path", "Text", "Next"]);
    t.rows.push(vec!["C:\\".into(), "a\\|b".into(), "next".into()]);
    assert_eq!(
        t.to_markdown(),
        "## Paths\n\n| Path | Text | Next |\n|---|---|---|\n| C:\\\\ | a\\\\\\|b | next |\n"
    );
}

#[test]
fn markdown_cells_show_markdown_characters_as_typed() {
    let mut t = Table::new("Notes", &["*Tag*", "Text"]);
    t.rows
        .push(vec!["*a*".into(), "b_c_d `x` <tag> [l](u) ~s~ & ![i](p)".into()]);
    t.rows.push(vec!["#1".into(), "-K1 2.5 mm² plain".into()]);
    assert_eq!(
        t.to_markdown(),
        "## Notes\n\n| \\*Tag\\* | Text |\n|---|---|\n\
         | \\*a\\* | b\\_c\\_d \\`x\\` &lt;tag&gt; \\[l\\](u) \\~s\\~ \\& \\!\\[i\\](p) |\n\
         | \\#1 | -K1 2.5 mm² plain |\n"
    );
}

#[test]
fn markdown_cells_keep_links_as_typed() {
    let mut t = Table::new("Links", &["Text"]);
    t.rows.push(vec!["see https://example.com/a_b *x*".into()]);
    t.rows.push(vec!["(www.x.no/a_b|c), mail a_b@x.no or a_b@c".into()]);
    assert_eq!(
        t.to_markdown(),
        "## Links\n\n| Text |\n|---|\n\
         | see https://example.com/a_b \\*x\\* |\n\
         | (www.x.no/a_b\\|c), mail a_b@x.no or a\\_b@c |\n"
    );
}

#[test]
fn markdown_cells_escape_words_that_only_look_like_links() {
    let mut t = Table::new("Links", &["Text"]);
    t.rows.push(vec!["https://*spare*/path".into()]);
    t.rows.push(vec!["https://example.com/&copy;".into()]);
    t.rows.push(vec!["https://example.com/a_b. www.a_b.no".into()]);
    assert_eq!(
        t.to_markdown(),
        "## Links\n\n| Text |\n|---|\n\
         | https://\\*spare\\*/path |\n\
         | https://example.com/\\&copy; |\n\
         | https://example.com/a_b. www.a\\_b.no |\n"
    );
}

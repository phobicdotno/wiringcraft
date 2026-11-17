#![allow(clippy::unwrap_used)]

use super::*;
use wc_core::geom::{Point, Rotation};
use wc_core::symbol::{Pin, PinDir};
use wc_core::{Device, Page, PlacedSymbol, Wire};

fn sym(id: &str, role: SymbolRole, pins: &[&str]) -> Symbol {
    Symbol {
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
    assert_eq!(t.rows[0], ["2", "1", "24V", "", "junction 50.0,20.0", "=A+C1-K1:A1"]);
    assert_eq!(t.rows[1][4..], ["=A+C1-K1:14", "-X1.1:1"]);
    // w4 carries 0V itself.
    assert_eq!(t.rows[3][2], "0V");
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

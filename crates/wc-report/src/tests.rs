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
            ("-X1/T1", "Terminal"),
            ("-X1/T2", "Terminal"),
            ("-X1/J1", "Jumper"),
            ("-X1/A1", "Accessory"),
            ("-X1/T2/A1", "Accessory"),
        ]
    );
    let row = |index: &str| t.rows.iter().find(|r| r[0] == index).unwrap().clone();
    assert_eq!(row("-X1/P1")[8], "Mounting plate 600 x 800 mm");
    assert_eq!(
        (row("-X1/R1")[15].as_str(), row("-X1/R1")[16].as_str()),
        ("Main plate", "1")
    );
    assert_eq!(row("-X1/R1")[18], "0");
    let t1 = row("-X1/T1");
    assert_eq!((t1[3].as_str(), t1[4].as_str()), ("1", "1"));
    assert_eq!((t1[6].as_str(), t1[7].as_str()), ("2002-1201", "WAGO"));
    assert_eq!(&t1[10..14], ["1.5 mm²", "BU", "2.5 mm²", ""]);
    assert_eq!(row("-X1/J1")[14], "1, 2");
    // The strip's own end plate first, then -X1.2's markers with their quantity.
    assert_eq!(
        (row("-X1/A1")[6].as_str(), row("-X1/A1")[9].as_str()),
        ("2002-1291", "1")
    );
    let a2 = row("-X1/T2/A1");
    assert_eq!(
        (a2[6].as_str(), a2[9].as_str(), a2[14].as_str()),
        ("acme.marker", "2", "2")
    );
    // -X2 is on no plate and has no part: one bare terminal row.
    let x2: Vec<&Vec<String>> = t.rows.iter().filter(|r| r[2] == "-X2").collect();
    assert_eq!(x2.len(), 1);
    assert_eq!((x2[0][0].as_str(), x2[0][6].as_str()), ("-X2/T1", ""));
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
    assert_eq!((x2[0][0].as_str(), x2[0][6].as_str()), ("-X2/T1", "2002-1201"));
    // Its accessory belongs to terminal 1, and the part is not listed again as an accessory.
    assert_eq!(
        (
            x2[1][0].as_str(),
            x2[1][6].as_str(),
            x2[1][9].as_str(),
            x2[1][14].as_str()
        ),
        ("-X2/T1/A1", "acme.marker", "3", "1")
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
    assert_eq!(row[0], "'=A+C1-X9/T1");
    assert_eq!(row[2], "'=A+C1-X9");
    assert!(
        out.warnings.iter().any(|w| w.contains("Strip \"=A+C1-X9\"")),
        "{:?}",
        out.warnings
    );
    assert!(out.warnings.iter().any(|w| w.contains("Index \"=A+C1-X9/T1\"")));
    let plain = out.table.rows.iter().find(|r| r[0] == "-X1/T1").unwrap();
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
    let row = out.table.rows.iter().find(|r| r[0] == "-X1/T1").unwrap();
    assert_eq!(&row[6..9], ["'=1+1", "'@cme", "plain"]);
    assert_eq!(row[2], "-X1", "tags stay as they are");
    assert_eq!(out.warnings.len(), 2, "{:?}", out.warnings);
    assert!(out.warnings[0].contains("-X1/T1") && out.warnings[0].contains("=1+1"));
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
            ("1", "2", "", "-X1/T1"),
            ("2", "S1", "2002-1201", "-X1/T2"),
            ("3", "1", "", "-X1/T3"),
        ]
    );
    // The spare's row imports back onto its own device.
    assert!(matches!(
        out.targets.get("-X1/T2"),
        Some(configurator::Target::Part(t)) if t.to_string() == "-X1.S1"
    ));
    // A spare without a device entry still names its own device for the import back.
    assert!(matches!(
        out.targets.get("-X9/T1"),
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
    // A drawn terminal 0 without a level, on that page, is not taken for a spare either: its
    // marker comes first, by number, not after the block.
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
    assert_eq!(marked, ["0", "1 / 2", "1a"]);
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
        out.table.rows.iter().any(|r| r.iter().any(|c| c == "-X1/T3")),
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

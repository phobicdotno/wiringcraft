#![allow(clippy::unwrap_used)]

use super::*;
use wc_core::geom::{Point, Rotation};
use wc_core::panel::Panel;
use wc_core::symbol::{Pin, PinDir};
use wc_core::{Device, DevicePart, Page, Part, PlacedSymbol, Project, Symbol, Wire};

fn tag(t: &str) -> DeviceTag {
    DeviceTag::parse(t).unwrap()
}

fn term_symbol() -> BTreeMap<String, Symbol> {
    let s = Symbol {
        source: String::new(),
        id: "term".into(),
        name: "term".into(),
        category: String::new(),
        letter_code: String::new(),
        role: SymbolRole::Terminal,
        graphics: vec![],
        pins: ["1", "2"]
            .iter()
            .enumerate()
            .map(|(n, p)| Pin {
                name: (*p).into(),
                at: Point::new(0.0, 10.0 * n as f64),
                dir: PinDir::Down,
                ..Default::default()
            })
            .collect(),
        tag_at: Point::default(),
        description: String::new(),
    };
    [(s.id.clone(), s)].into_iter().collect()
}

fn place(id: &str, symbol: &str, x: f64, t: &str) -> PlacedSymbol {
    PlacedSymbol {
        id: id.into(),
        symbol: symbol.into(),
        at: Point::new(x, 50.0),
        rotation: Rotation::R0,
        mirror: false,
        tag: Some(tag(t)),
    }
}

fn wire_to(id: &str, item: &str, pin: &str, section: &str, colour: &str) -> Wire {
    let mut w = Wire {
        id: id.into(),
        from: Endpoint::Point {
            at: Point::new(0.0, 0.0),
        },
        to: Endpoint::Pin {
            item: item.into(),
            pin: pin.into(),
        },
        via: vec![],
        number: None,
        potential: None,
        cable: None,
        route: Default::default(),
        spec: Default::default(),
    };
    w.spec.section = Some(section.into());
    w.spec.colour = Some(colour.into());
    w
}

/// A project with one schematic page holding the given terminals.
fn project(terminals: &[(&str, &str)]) -> Project {
    let mut p = Project::new("Test");
    let mut page = Page::new("p1", "1", "Terminals");
    page.symbols = terminals
        .iter()
        .enumerate()
        .map(|(n, (id, t))| place(id, "term", 10.0 * n as f64, t))
        .collect();
    p.pages.push(page);
    p
}

fn list(p: &Project, s: &BTreeMap<String, Symbol>, parts: &BTreeMap<String, Part>) -> Configurator {
    configurator_list(&Inputs {
        project: p,
        symbols: s,
        parts,
    })
}

fn indexes(c: &Configurator) -> Vec<&str> {
    c.table.rows.iter().map(|r| r[0].as_str()).collect()
}

#[test]
fn millimetres_have_at_most_one_decimal_and_no_trailing_zero() {
    assert_eq!(mm(600.0), "600");
    assert_eq!(mm(35.5), "35.5");
    assert_eq!(mm(487.54), "487.5");
    assert_eq!(mm(599.96), "600");
    assert_eq!(mm(0.0), "0");
    assert_eq!(join(&BTreeSet::new()), "");
    let set: BTreeSet<String> = ["RD", "BU"].iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(join(&set), "BU, RD");
}

#[test]
fn strips_come_in_natural_tag_order() {
    let p = project(&[("a", "-X10.1"), ("b", "-X2.1"), ("c", "-X1.1")]);
    let out = list(&p, &term_symbol(), &BTreeMap::new());
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(indexes(&out), ["-X1/T:1", "-X2/T:1", "-X10/T:1"]);
}

#[test]
fn several_wires_on_one_side_list_each_section_and_colour_once_sorted() {
    let mut p = project(&[("t", "-X1.1")]);
    p.pages[0].wires = vec![
        wire_to("w1", "t", "1", "2.5 mm²", "RD"),
        wire_to("w2", "t", "1", "1.5 mm²", "BU"),
        wire_to("w3", "t", "1", "1.5 mm²", "RD"),
        wire_to("w4", "t", "2", "0.75 mm²", "GNYE"),
    ];
    let out = list(&p, &term_symbol(), &BTreeMap::new());
    let row = &out.table.rows[0];
    assert_eq!(&row[10..14], ["1.5 mm², 2.5 mm²", "BU, RD", "0.75 mm²", "GNYE"]);
}

#[test]
fn terminals_on_overview_pages_are_not_listed() {
    let mut p = project(&[("t", "-X1.1")]);
    let mut overview = Page::new("p9", "9", "Overview");
    overview.overview = true;
    overview.symbols.push(place("o", "term", 0.0, "-X7.1"));
    overview.wires.push(wire_to("w", "o", "1", "1.5 mm²", "BU"));
    p.pages.push(overview);
    let out = list(&p, &term_symbol(), &BTreeMap::new());
    assert_eq!(indexes(&out), ["-X1/T:1"]);
    assert!(out.table.rows[0][10..14].iter().all(String::is_empty));
}

#[test]
fn a_terminal_whose_symbol_is_missing_is_listed_with_a_warning() {
    let mut p = project(&[("t", "-X1.1")]);
    p.pages[0].symbols.push(place("g", "gone", 20.0, "-X1.2"));
    p.pages[0].wires.push(wire_to("w", "g", "1", "1.5 mm²", "BU"));
    let out = list(&p, &term_symbol(), &BTreeMap::new());
    assert_eq!(indexes(&out), ["-X1/T:1", "-X1/T:2"]);
    assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
    assert!(
        out.warnings[0].contains("-X1.2") && out.warnings[0].contains("gone"),
        "{:?}",
        out.warnings
    );
    // Nothing tells its sides apart, so its wire's spec is not written.
    let two = &out.table.rows[1];
    assert!(two[10..14].iter().all(String::is_empty), "{two:?}");
}

#[test]
fn only_plates_holding_the_strip_and_rails_crossing_it_are_listed() {
    let mut p = project(&[("t", "-X1.1")]);
    let mut main = Panel::new("main", "Main", 599.96, 800.0);
    main.items = vec![
        // Rail 1 misses the terminal, rail 2 crosses it.
        PanelItem::Rail {
            at: Point::new(0.0, 100.0),
            length: 500.0,
        },
        PanelItem::Rail {
            at: Point::new(0.0, 300.0),
            length: 487.54,
        },
        PanelItem::Device {
            tag: tag("-X1.1"),
            at: Point::new(50.0, 280.0),
            width: 5.2,
            height: 40.0,
            rotation: Rotation::R0,
        },
    ];
    let mut other = Panel::new("other", "Other", 300.0, 300.0);
    other.items = vec![
        PanelItem::Rail {
            at: Point::new(0.0, 150.0),
            length: 200.0,
        },
        PanelItem::Device {
            tag: tag("-K1"),
            at: Point::new(10.0, 130.0),
            width: 20.0,
            height: 40.0,
            rotation: Rotation::R0,
        },
    ];
    p.panels = vec![other, main];
    let out = list(&p, &term_symbol(), &BTreeMap::new());
    assert_eq!(indexes(&out), ["-X1/P1", "-X1/R1", "-X1/T:1"]);
    let plate = &out.table.rows[0];
    assert_eq!(
        (plate[8].as_str(), plate[15].as_str()),
        ("Mounting plate 600 x 800 mm", "Main")
    );
    // The rail keeps its number on the plate (2), its own index counts per strip (R1).
    let rail = &out.table.rows[1];
    assert_eq!(&rail[15..19], ["Main", "2", "487.5", "0"]);
    // Plates and rails stand for no device part.
    assert_eq!(out.targets.len(), 1);
    assert!(matches!(out.targets.get("-X1/T:1"), Some(Target::Part(t)) if *t == tag("-X1.1")));
}

#[test]
fn a_device_beside_a_rail_is_not_on_it() {
    let mut p = project(&[("t", "-X1.1")]);
    let mut main = Panel::new("main", "Main", 600.0, 400.0);
    main.items = vec![
        PanelItem::Rail {
            at: Point::new(0.0, 100.0),
            length: 100.0,
        },
        // Level with the rail, but past its right end.
        PanelItem::Device {
            tag: tag("-X1.1"),
            at: Point::new(150.0, 80.0),
            width: 5.2,
            height: 40.0,
            rotation: Rotation::R0,
        },
    ];
    p.panels.push(main);
    let out = list(&p, &term_symbol(), &BTreeMap::new());
    assert_eq!(indexes(&out), ["-X1/P1", "-X1/T:1"]);
}

#[test]
fn jumper_and_accessory_rows_name_their_targets() {
    let mut p = project(&[("a", "-X1.1"), ("b", "-X1.2"), ("c", "-X1.3"), ("d", "-X1.4")]);
    let x1 = tag("-X1");
    p.meta.add_jumper(&x1, &["1".to_owned(), "2".to_owned()]).unwrap();
    p.meta.add_jumper(&x1, &["3".to_owned(), "4".to_owned()]).unwrap();
    p.devices.insert(
        x1.clone(),
        Device {
            part: Some("acme.strip".into()),
            accessories: vec![
                DevicePart {
                    part: "acme.end".into(),
                    qty: 2,
                },
                DevicePart {
                    part: "acme.stop".into(),
                    qty: 1,
                },
            ],
            ..Default::default()
        },
    );
    p.devices.insert(
        tag("-X1.3"),
        Device {
            accessories: vec![DevicePart {
                part: "acme.marker".into(),
                qty: 4,
            }],
            ..Default::default()
        },
    );
    let out = list(&p, &term_symbol(), &BTreeMap::new());
    assert_eq!(
        indexes(&out),
        [
            "-X1/T:1",
            "-X1/T:2",
            "-X1/T:3",
            "-X1/T:4",
            "-X1/J1",
            "-X1/J2",
            "-X1/S",
            "-X1/A1",
            "-X1/A2",
            "-X1/T:3/A1"
        ]
    );
    let t = &out.targets;
    assert!(matches!(t.get("-X1/J2"), Some(Target::Jumper(s, 1)) if *s == x1));
    assert!(matches!(t.get("-X1/S"), Some(Target::Part(s)) if *s == x1));
    assert!(matches!(t.get("-X1/A2"), Some(Target::Accessory(s, 1)) if *s == x1));
    assert!(matches!(t.get("-X1/T:3/A1"), Some(Target::Accessory(s, 0)) if *s == tag("-X1.3")));
    // Parts without a vendored copy are written by their id, with their quantity.
    let a1 = out.table.rows.iter().find(|r| r[0] == "-X1/A1").unwrap();
    assert_eq!((a1[6].as_str(), a1[9].as_str()), ("acme.end", "2"));
    let j2 = out.table.rows.iter().find(|r| r[0] == "-X1/J2").unwrap();
    assert_eq!((j2[8].as_str(), j2[14].as_str()), ("2-way jumper", "3, 4"));
}

#[test]
fn a_free_text_starting_with_a_minus_is_guarded_but_a_tag_is_not() {
    let mut p = project(&[("t", "-X1.1")]);
    p.pages[0].wires.push(wire_to("w", "t", "2", "-1.5", "\tBU"));
    let out = list(&p, &term_symbol(), &BTreeMap::new());
    let row = &out.table.rows[0];
    assert_eq!((row[0].as_str(), row[2].as_str()), ("-X1/T:1", "-X1"));
    assert_eq!((row[12].as_str(), row[13].as_str()), ("'-1.5", "'\tBU"));
    assert_eq!(out.warnings.len(), 2, "{:?}", out.warnings);
    assert!(out.warnings[0].contains("Lower section"));
    assert!(out.warnings[1].contains("Lower colour"));
}

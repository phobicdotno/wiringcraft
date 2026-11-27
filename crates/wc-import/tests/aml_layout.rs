//! AutomationML import: refusing files that are not CAEX, device tags and part data, and laying
//! devices out in sections across overview pages.

use std::collections::BTreeMap;

use wc_core::Endpoint;
use wc_import::Result;
use wc_import::aml::{AmlDevice, AmlImport, AmlLink, Section, build, device_entry, layout, parse};

fn device(name: &str, interfaces: &[&str]) -> AmlDevice {
    AmlDevice {
        name: name.into(),
        tag: wc_core::DeviceTag::parse(&format!("-{name}")).ok(),
        interfaces: interfaces.iter().map(|s| (*s).to_owned()).collect(),
        ..Default::default()
    }
}

fn link(name: &str, a: (usize, &str), b: (usize, &str)) -> AmlLink {
    AmlLink {
        name: name.into(),
        a: (a.0, a.1.into()),
        b: (b.0, b.1.into()),
        spec: Default::default(),
    }
}

#[test]
fn a_file_that_is_not_caex_or_not_xml_is_refused() {
    assert!(parse("<root><InternalElement Name=\"K1\"/></root>").is_err());
    assert!(parse("<CAEXFile><InstanceHierarchy></CAEXFile>").is_err());
    assert!(parse("<CAEXFile").is_err());
}

#[test]
fn tag_attribute_names_the_device_and_is_not_kept_as_a_property() -> Result<()> {
    let aml = r#"<CAEXFile><InstanceHierarchy Name="P">
  <InternalElement Name="Relay" ID="e1">
    <Attribute Name="BMK"><Value>-K7</Value></Attribute>
    <Attribute Name="Colour"><Value>grey</Value></Attribute>
    <ExternalInterface Name="A1" ID="i1"/>
  </InternalElement>
  <InternalElement Name="K..1" ID="e2">
    <ExternalInterface Name="1" ID="i2"/>
  </InternalElement>
  <InternalLink Name="w" RefPartnerSideA="i1" RefPartnerSideB="i2"/>
</InstanceHierarchy></CAEXFile>"#;
    let imp = parse(aml)?;
    assert_eq!(imp.devices.len(), 2);
    let relay = &imp.devices[0];
    assert_eq!(relay.tag.as_ref().map(ToString::to_string).as_deref(), Some("-K7"));
    assert!(!relay.properties.contains_key("BMK"));
    assert_eq!(relay.properties.get("Colour").map(String::as_str), Some("grey"));
    assert!(
        relay.path.is_empty(),
        "the instance hierarchy is not an enclosing element"
    );

    // A name that is no tag, even with a `-` in front, leaves the device untagged and says so.
    assert!(imp.devices[1].tag.is_none());
    assert_eq!(imp.warnings.len(), 1, "{:?}", imp.warnings);
    assert!(imp.warnings[0].contains("K..1"));
    // The link by interface ids still stands.
    assert_eq!(imp.links.len(), 1);
    assert_eq!(imp.links[0].a, (0, "A1".to_string()));
    assert_eq!(imp.links[0].b, (1, "1".to_string()));
    Ok(())
}

#[test]
fn device_without_order_number_has_no_part_and_takes_its_function_text() {
    let mut parts = Vec::new();
    let mut d = device("K1", &["A1"]);
    d.properties = BTreeMap::from([
        ("Function text".to_string(), "Pump contactor".to_string()),
        ("Manufacturer".to_string(), "Acme".to_string()),
    ]);
    let Some((tag, dev)) = device_entry(&d, &mut parts) else {
        unreachable!("a tagged device gives an entry")
    };
    assert_eq!(tag.to_string(), "-K1");
    assert!(dev.part.is_none());
    assert!(parts.is_empty());
    assert_eq!(dev.description, "Pump contactor");

    // Without a tag there is no project device at all.
    let untagged = AmlDevice { tag: None, ..d };
    assert!(device_entry(&untagged, &mut parts).is_none());
}

#[test]
fn devices_in_no_section_are_left_out_with_their_links() {
    let imp = AmlImport {
        devices: vec![device("K1", &["A1"]), device("K2", &["A1"]), device("X1", &["1"])],
        links: vec![link("in", (0, "A1"), (2, "1")), link("out", (1, "A1"), (2, "1"))],
        ..Default::default()
    };
    let sections = [Section {
        title: "Main".into(),
        devices: vec![0, 2, 0, 99],
    }];
    let b = layout(&imp, &sections, "s", 1);
    assert_eq!(b.pages.len(), 1);
    assert_eq!(b.pages[0].id, "s-1");
    assert_eq!(b.pages[0].title, "Main");
    let items: Vec<&str> = b.pages[0].symbols.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(items, ["d1", "d3"], "K1 is placed once and K2 not at all");
    assert_eq!(b.symbols.len(), 2);
    assert_eq!(b.devices.len(), 2);
    assert_eq!(b.pages[0].wires.len(), 1);
    assert_eq!(b.pages[0].wires[0].number.as_deref(), Some("in"));
}

#[test]
fn a_full_section_continues_on_a_new_page() {
    let imp = AmlImport {
        devices: (1..=30).map(|n| device(&format!("K{n}"), &["A1"])).collect(),
        ..Default::default()
    };
    let b = build(&imp, 5);
    assert_eq!(b.pages.len(), 2);
    assert_eq!(b.pages[1].title, "AutomationML connections (continued)");
    assert_eq!(b.pages[1].number, "6");
    assert_eq!(b.pages[1].id, "aml-2");
    let placed: usize = b.pages.iter().map(|p| p.symbols.len()).sum();
    assert_eq!(placed, 30);
    assert!(b.pages.iter().all(|p| !p.symbols.is_empty()));
}

#[test]
fn a_link_across_sections_ends_on_a_named_potential_on_each_page() {
    let imp = AmlImport {
        devices: vec![device("K1", &["A1", "A2"]), device("X1", &["1"])],
        links: vec![link("", (0, "A1"), (1, "1")), link("7", (0, "A2"), (1, "1"))],
        ..Default::default()
    };
    let sections = [
        Section {
            title: "One".into(),
            devices: vec![0],
        },
        Section {
            title: "Two".into(),
            devices: vec![1],
        },
    ];
    let b = layout(&imp, &sections, "eplan", 1);
    assert_eq!(b.pages.len(), 2);
    for (page, item) in [(&b.pages[0], "d1"), (&b.pages[1], "d2")] {
        let potentials: Vec<Option<&str>> = page.wires.iter().map(|w| w.potential.as_deref()).collect();
        assert_eq!(potentials, [Some("EPLAN1"), Some("EPLAN2")]);
        for w in &page.wires {
            assert!(matches!(&w.from, Endpoint::Pin { item: i, .. } if i == item));
            assert!(matches!(w.to, Endpoint::Point { .. }));
        }
        // An unnamed link carries no wire number; a named one carries its name.
        assert_eq!(page.wires[0].number, None);
        assert_eq!(page.wires[1].number.as_deref(), Some("7"));
    }
}

#[test]
fn symbol_ids_are_made_safe_and_a_link_to_a_missing_pin_draws_no_wire() {
    let imp = AmlImport {
        devices: vec![device("Motor M1!", &["U", "U"]), device("X1", &["1"])],
        links: vec![link("ok", (0, "U'"), (1, "1")), link("no pin", (0, "W"), (1, "1"))],
        ..Default::default()
    };
    let b = build(&imp, 1);
    assert_eq!(b.symbols[0].id, "aml.1.motor-m1");
    assert_eq!(b.symbols[0].name, "Motor M1! (imported)");
    let pins: Vec<&str> = b.symbols[0].pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(pins, ["U", "U'"]);
    assert_eq!(b.pages[0].wires.len(), 1);
    assert!(matches!(&b.pages[0].wires[0].from, Endpoint::Pin { pin, .. } if pin == "U'"));
}

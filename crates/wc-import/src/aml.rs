//! AutomationML (IEC 62714, CAEX XML) import: devices, their properties, connection points and
//! the links between them.
//!
//! AutomationML carries no drawing, so [`build`] lays the result out as connection overview
//! pages: one box per device with its connection points along the bottom edge, wired as the file
//! says. That makes an import visible and checkable immediately, and every wire is real
//! connectivity that later redrawing keeps.

use std::collections::BTreeMap;

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use wc_core::geom::{Align, Point, Shape};
use wc_core::symbol::{Pin, PinDir, SymbolRole};
use wc_core::{Device, DeviceTag, Endpoint, Page, PlacedSymbol, Symbol, Wire};

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AmlDevice {
    /// The element's own name.
    pub name: String,
    /// Names of the enclosing elements, outermost first.
    pub path: Vec<String>,
    pub tag: Option<DeviceTag>,
    pub properties: BTreeMap<String, String>,
    pub interfaces: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AmlLink {
    pub name: String,
    pub a: (usize, String),
    pub b: (usize, String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AmlImport {
    pub devices: Vec<AmlDevice>,
    pub links: Vec<AmlLink>,
    pub warnings: Vec<String>,
    /// Structure elements that are locations (`+L1`), with their description.
    pub locations: Vec<wc_core::Location>,
}

/// The location levels of a structure element named `own` inside elements named `outer`: every
/// `+` name on the way adds its levels, unless it already repeats the levels above it.
fn location_levels(outer: &[String], own: &str) -> Vec<String> {
    let mut acc: Vec<String> = Vec::new();
    for name in outer.iter().map(String::as_str).chain([own]) {
        let name = name.trim();
        if !name.starts_with('+') {
            continue;
        }
        let Ok(path) = wc_core::project::location_path(name) else {
            continue;
        };
        let levels: Vec<String> = path.split('.').map(str::to_owned).collect();
        if levels.starts_with(&acc) {
            acc = levels;
        } else {
            acc.extend(levels);
        }
    }
    acc
}

fn attr(e: &BytesStart, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == name.as_bytes())
        .and_then(|a| a.unescape_value().ok())
        .map(|v| v.into_owned())
}

const TAG_ATTRIBUTES: [&str; 6] = [
    "ReferenceDesignation",
    "ReferenceDesignator",
    "RefDes",
    "Tag",
    "DeviceTag",
    "BMK",
];

#[derive(Default)]
struct Elem {
    name: String,
    id: String,
    attributes: BTreeMap<String, String>,
    interfaces: Vec<(String, String)>, // (id, name)
}

/// Parse a CAEX file.
pub fn parse(text: &str) -> Result<AmlImport> {
    let mut reader = Reader::from_str(text);
    let mut out = AmlImport::default();
    let mut stack: Vec<Elem> = Vec::new();
    // Attribute nesting: names of open <Attribute> elements and whether we are in <Value>.
    let mut attr_stack: Vec<String> = Vec::new();
    let mut in_value = false;
    let mut raw_links: Vec<(String, String, String)> = Vec::new();
    let mut iface_index: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut elem_ids: BTreeMap<String, usize> = BTreeMap::new();
    // Each finished element with the names of its parents and their designations (the tag
    // attribute when they have one, else the name).
    let mut pending: Vec<(Vec<String>, Vec<String>, Elem)> = Vec::new();
    // An element's designation for locations: a reference designation that is a location, else
    // its name when that is one, else whatever it is designated by.
    let designation = |e: &Elem| {
        let attr = TAG_ATTRIBUTES.iter().find_map(|k| e.attributes.get(*k));
        let is_loc = |s: &str| s.trim_start().starts_with('+');
        match attr {
            Some(a) if is_loc(a) => a.clone(),
            _ if is_loc(&e.name) => e.name.clone(),
            Some(a) => a.clone(),
            None => e.name.clone(),
        }
    };
    let mut seen_caex = false;
    let mut events = 0usize;
    loop {
        events += 1;
        if events > 5_000_000 {
            return Err(Error::Format("AutomationML file too large".into()));
        }
        let ev = reader
            .read_event()
            .map_err(|e| Error::Format(format!("AutomationML: {e}")))?;
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(ev, Event::Empty(_));
                let tag = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                match tag.as_str() {
                    "CAEXFile" => seen_caex = true,
                    "InternalElement" => {
                        let el = Elem {
                            name: attr(e, "Name").unwrap_or_default(),
                            id: attr(e, "ID").unwrap_or_default(),
                            ..Default::default()
                        };
                        if empty {
                            let path = stack.iter().map(|s| s.name.clone()).collect();
                            let outer = stack.iter().map(designation).collect();
                            pending.push((path, outer, el));
                        } else {
                            stack.push(el);
                        }
                    }
                    "ExternalInterface" => {
                        if let Some(top) = stack.last_mut() {
                            top.interfaces
                                .push((attr(e, "ID").unwrap_or_default(), attr(e, "Name").unwrap_or_default()));
                        }
                        if !empty {
                            // Attributes inside an interface describe the interface, not the device.
                            attr_stack.push("\u{0}interface".into());
                        }
                    }
                    "Attribute" if !empty => attr_stack.push(attr(e, "Name").unwrap_or_default()),
                    "Value" if !empty => in_value = true,
                    "InternalLink" => raw_links.push((
                        attr(e, "Name").unwrap_or_default(),
                        attr(e, "RefPartnerSideA").unwrap_or_default(),
                        attr(e, "RefPartnerSideB").unwrap_or_default(),
                    )),
                    _ => {}
                }
            }
            Event::Text(t) if in_value => {
                let v = t.unescape().map(|s| s.trim().to_owned()).unwrap_or_default();
                let interface = attr_stack.iter().any(|a| a == "\u{0}interface");
                if let (Some(name), Some(top), false) = (attr_stack.last(), stack.last_mut(), interface)
                    && !v.is_empty()
                {
                    // Nested attributes become dotted names.
                    let full: Vec<&str> = attr_stack.iter().map(String::as_str).collect();
                    let key = if full.len() > 1 { full.join(".") } else { name.clone() };
                    top.attributes.insert(key, v);
                }
            }
            Event::End(e) => {
                let tag = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                match tag.as_str() {
                    "InternalElement" => {
                        if let Some(el) = stack.pop() {
                            let path = stack.iter().map(|s| s.name.clone()).collect();
                            let outer = stack.iter().map(designation).collect();
                            pending.push((path, outer, el));
                        }
                    }
                    "ExternalInterface" | "Attribute" => {
                        attr_stack.pop();
                    }
                    "Value" => in_value = false,
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !seen_caex {
        return Err(Error::Format("not an AutomationML (CAEX) file".into()));
    }
    for (path, outer, el) in pending {
        if el.interfaces.is_empty() {
            // Structure (function, location), not a device. A location goes to the project.
            let own = designation(&el);
            // Only an element that is itself a location (`+L1`, not `+L1-K1`) names one.
            let valid = wc_core::project::location_path(&own);
            if own.trim_start().starts_with('+')
                && let Err(why) = &valid
            {
                out.warnings
                    .push(format!("{}: {why}; not taken as a location", el.name));
            }
            if own.trim_start().starts_with('+') && valid.is_ok() {
                let levels = location_levels(&outer, &own);
                if !levels.is_empty() {
                    let description = ["Description", "description", "Beschreibung"]
                        .iter()
                        .find_map(|k| el.attributes.get(*k))
                        .cloned()
                        .unwrap_or_default();
                    let path = levels.join(".");
                    match out.locations.iter_mut().find(|l| l.path == path) {
                        // Children come before their parents: a description found later fills
                        // an entry that has none.
                        Some(l) if l.description.is_empty() => l.description = description,
                        Some(_) => {}
                        None => out.locations.push(wc_core::Location { path, description }),
                    }
                }
            }
            continue;
        }
        let idx = out.devices.len();
        elem_ids.insert(el.id.clone(), idx);
        let mut names = Vec::new();
        for (id, name) in &el.interfaces {
            if !id.is_empty() {
                iface_index.insert(id.clone(), (idx, name.clone()));
            }
            names.push(name.clone());
        }
        let tag_text = TAG_ATTRIBUTES
            .iter()
            .find_map(|k| el.attributes.get(*k))
            .cloned()
            .unwrap_or_else(|| el.name.clone());
        let tag = DeviceTag::parse(&tag_text)
            .or_else(|_| DeviceTag::parse(&format!("-{tag_text}")))
            .ok();
        if tag.is_none() {
            out.warnings
                .push(format!("{}: {tag_text:?} is not a usable device tag", el.name));
        }
        let properties = el
            .attributes
            .into_iter()
            .filter(|(k, _)| !TAG_ATTRIBUTES.contains(&k.as_str()))
            .collect();
        out.devices.push(AmlDevice {
            name: el.name,
            path,
            tag,
            properties,
            interfaces: names,
        });
    }
    let resolve = |r: &str| -> Option<(usize, String)> {
        let r = r.trim();
        if let Some(hit) = iface_index.get(r) {
            return Some(hit.clone());
        }
        // `elementID:interfaceName`, element ids often in braces.
        let (el, iface) = r.rsplit_once(':')?;
        let idx = *elem_ids
            .get(el)
            .or_else(|| elem_ids.get(&format!("{{{}}}", el.trim_matches(|c| c == '{' || c == '}'))))?;
        Some((idx, iface.to_owned()))
    };
    for (name, a, b) in raw_links {
        match (resolve(&a), resolve(&b)) {
            (Some(a), Some(b)) => out.links.push(AmlLink { name, a, b }),
            _ => out.warnings.push(format!(
                "link {name:?} refers to an unknown connection point ({a} / {b})"
            )),
        }
    }
    Ok(out)
}

/// Common part attributes, matched case-insensitively.
fn part_fields(props: &BTreeMap<String, String>) -> (Option<String>, Option<String>) {
    let find = |keys: &[&str]| {
        props
            .iter()
            .find(|(k, _)| keys.iter().any(|w| k.eq_ignore_ascii_case(w)))
            .map(|(_, v)| v.clone())
    };
    (
        find(&["Manufacturer", "ManufacturerName", "Vendor"]),
        find(&[
            "OrderNumber",
            "OrderCode",
            "ProductCode",
            "ArticleNumber",
            "PartNumber",
            "TypeNumber",
        ]),
    )
}

/// Everything an AutomationML import adds to a project.
#[derive(Debug, Default)]
pub struct Built {
    pub symbols: Vec<Symbol>,
    pub parts: Vec<wc_core::Part>,
    pub pages: Vec<Page>,
    pub devices: BTreeMap<DeviceTag, Device>,
    /// Locations with their descriptions, for the project file (importers that have them).
    pub locations: Vec<wc_core::Location>,
    /// Title-block fields for the project (`ProjectMeta.title_block` keys), from importers
    /// that carry project data.
    pub title_block: BTreeMap<String, String>,
}

const PIN_STEP: f64 = 7.5;
const BOX_H: f64 = 14.0;
const ROW_H: f64 = 70.0;
const LEFT: f64 = 30.0;
const TOP: f64 = 40.0;
const RIGHT: f64 = 390.0;
const ROWS_PER_PAGE: usize = 3;

fn safe(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    s.trim_matches('-').chars().take(60).collect()
}

/// Lay an import out as connection overview pages. `first_page_number` is where page numbering
/// starts; page ids are `aml-<n>`.
pub fn build(imp: &AmlImport, first_page_number: usize) -> Built {
    let all = Section {
        title: "AutomationML connections".into(),
        devices: (0..imp.devices.len()).collect(),
    };
    let mut built = layout(imp, &[all], "aml", first_page_number);
    built.locations = imp.locations.clone();
    built.locations.sort_by(|a, b| a.path.cmp(&b.path));
    built
}

/// A group of devices that starts on a new overview page, such as the devices of one page of
/// the source project.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Section {
    pub title: String,
    /// Indexes into the import's devices. A device listed twice is placed once, in its first
    /// section.
    pub devices: Vec<usize>,
}

/// The project device for an imported device, adding its part (if it names one) to `parts`.
pub fn device_entry(d: &AmlDevice, parts: &mut Vec<wc_core::Part>) -> Option<(DeviceTag, Device)> {
    let tag = d.tag.clone()?;
    let (manufacturer, order) = part_fields(&d.properties);
    let part = order.map(|o| {
        let p = wc_core::Part {
            id: wc_core::Part::make_id(manufacturer.as_deref().unwrap_or(""), &o),
            manufacturer: manufacturer.unwrap_or_default(),
            order_number: o,
            description: d.name.clone(),
            symbol: None,
            properties: d.properties.clone(),
        };
        let id = p.id.clone();
        if !parts.iter().any(|q| q.id == id) {
            parts.push(p);
        }
        id
    });
    let device = Device {
        part,
        description: d
            .properties
            .get("Function text")
            .cloned()
            .unwrap_or_else(|| d.name.clone()),
        properties: d.properties.clone(),
    };
    Some((tag, device))
}

/// Lay devices and links out as connection overview pages, one or more pages per section.
/// Symbol ids are `<prefix>.<n>.<name>`, page ids `<prefix>-<n>`, numbered from
/// `first_page_number`. Devices in no section are left out, with their links.
pub fn layout(imp: &AmlImport, sections: &[Section], prefix: &str, first_page_number: usize) -> Built {
    let mut built = Built::default();
    // Device symbols: a box with the device name, pins along the bottom.
    let mut placements: Vec<Option<(usize, Point)>> = vec![None; imp.devices.len()];
    let mut symbol_of: Vec<Option<usize>> = vec![None; imp.devices.len()];
    let mut page_titles: Vec<String> = Vec::new();
    for section in sections {
        let (mut x, mut row) = (LEFT, 0usize);
        page_titles.push(section.title.clone());
        let mut page = page_titles.len() - 1;
        for &i in &section.devices {
            let Some(d) = imp.devices.get(i) else { continue };
            if placements[i].is_some() {
                continue;
            }
            let n = d.interfaces.len().max(1);
            let w = (n as f64 + 1.0) * PIN_STEP;
            let id = format!("{prefix}.{}.{}", i + 1, safe(&d.name));
            let mut graphics = vec![
                Shape::Rect {
                    min: Point::new(0.0, -BOX_H),
                    max: Point::new(w, 0.0),
                },
                Shape::Text {
                    at: Point::new(w / 2.0, -BOX_H / 2.0 + 1.2),
                    text: d.name.clone(),
                    height: 2.5,
                    align: Align::Center,
                    angle: 0.0,
                },
            ];
            let mut pins = Vec::new();
            for (k, iface) in d.interfaces.iter().enumerate() {
                let px = PIN_STEP * (k as f64 + 1.0);
                graphics.push(Shape::Line {
                    a: Point::new(px, 0.0),
                    b: Point::new(px, 3.0),
                });
                graphics.push(Shape::Text {
                    at: Point::new(px + 0.8, -1.0),
                    text: iface.clone(),
                    height: 1.8,
                    align: Align::Left,
                    angle: 0.0,
                });
                // Duplicate interface names would make pins ambiguous: suffix them.
                let mut name = iface.clone();
                while pins.iter().any(|p: &Pin| p.name == name) {
                    name.push('\'');
                }
                pins.push(Pin {
                    name,
                    at: Point::new(px, 3.0),
                    dir: PinDir::Down,
                });
            }
            let description = d.path.join(" / ");
            built.symbols.push(Symbol {
                id: id.clone(),
                name: format!("{} (imported)", d.name),
                category: "imported".into(),
                letter_code: d.tag.as_ref().map(|t| t.letter_code()).unwrap_or_default(),
                role: SymbolRole::Standalone,
                graphics,
                pins,
                tag_at: Point::new(-2.0, -BOX_H / 2.0),
                description: description.clone(),
            });
            symbol_of[i] = Some(built.symbols.len() - 1);
            if x + w > RIGHT && x > LEFT {
                x = LEFT;
                row += 1;
                if row == ROWS_PER_PAGE {
                    row = 0;
                    page_titles.push(format!("{} (continued)", section.title));
                    page = page_titles.len() - 1;
                }
            }
            let at = Point::new(x + 18.0, TOP + BOX_H + ROW_H * row as f64);
            placements[i] = Some((page, at));
            x += w + 30.0;
            if let Some((tag, device)) = device_entry(d, &mut built.parts) {
                built.devices.insert(tag, device);
            }
        }
    }
    for (n, title) in page_titles.iter().enumerate() {
        let mut p = Page::new(
            format!("{prefix}-{}", n + 1),
            (first_page_number + n).to_string(),
            title.clone(),
        );
        p.size = wc_core::PageSize::A3;
        built.pages.push(p);
    }
    for (i, d) in imp.devices.iter().enumerate() {
        let (Some((page, at)), Some(sym)) = (placements[i], symbol_of[i]) else {
            continue;
        };
        let item = format!("d{}", i + 1);
        if let Some(pg) = built.pages.get_mut(page) {
            pg.symbols.push(PlacedSymbol {
                id: item,
                symbol: built.symbols[sym].id.clone(),
                at,
                rotation: Default::default(),
                mirror: false,
                tag: d.tag.clone(),
            });
        }
    }
    // Wires: down from each pin into a routing channel under the row, across, and up.
    for (k, l) in imp.links.iter().enumerate() {
        let (Some(Some((pa, ata))), Some(Some((pb, atb)))) = (placements.get(l.a.0), placements.get(l.b.0)) else {
            continue;
        };
        let (pa, ata, pb, atb) = (*pa, *ata, *pb, *atb);
        let pin_name = |dev: usize, iface: &str| {
            let sym = symbol_of.get(dev).copied().flatten()?;
            built.symbols[sym]
                .pins
                .iter()
                .find(|p| p.name == iface)
                .map(|p| (p.name.clone(), p.at))
        };
        let (Some((na, la)), Some((nb, lb))) = (pin_name(l.a.0, &l.a.1), pin_name(l.b.0, &l.b.1)) else {
            continue;
        };
        let a = Endpoint::Pin {
            item: format!("d{}", l.a.0 + 1),
            pin: na,
        };
        let b = Endpoint::Pin {
            item: format!("d{}", l.b.0 + 1),
            pin: nb,
        };
        let lane = 6.0 + 2.5 * (k % 12) as f64;
        let number = (!l.name.is_empty()).then(|| l.name.clone());
        if pa == pb {
            let (wa, wb) = (ata + la, atb + lb);
            let y = wa.y.max(wb.y) + lane;
            let via = vec![Point::new(wa.x, y), Point::new(wb.x, y)];
            if let Some(pg) = built.pages.get_mut(pa) {
                let id = format!("w{}", k + 1);
                pg.wires.push(Wire {
                    id,
                    from: a,
                    to: b,
                    via,
                    number,
                    potential: None,
                    cable: None,
                    route: Default::default(),
                });
            }
        } else {
            // Across pages: each end gets a stub to a named potential.
            let potential = Some(format!("{}{}", prefix.to_uppercase(), k + 1));
            for (page, end, at, local) in [(pa, a, ata, la), (pb, b, atb, lb)] {
                let p = at + local;
                if let Some(pg) = built.pages.get_mut(page) {
                    let id = pg.next_id("w");
                    pg.wires.push(Wire {
                        id,
                        from: end,
                        to: Endpoint::Point {
                            at: Point::new(p.x, p.y + lane),
                        },
                        via: vec![],
                        number: number.clone(),
                        potential: potential.clone(),
                        cable: None,
                        route: Default::default(),
                    });
                }
            }
        }
    }
    built
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<CAEXFile FileName="t.aml" SchemaVersion="3.0" xmlns="http://www.dke.de/CAEX">
  <InstanceHierarchy Name="Plant">
    <InternalElement Name="=PUMP" ID="f1">
      <InternalElement Name="K1" ID="{e1}">
        <Attribute Name="ReferenceDesignation"><Value>=PUMP+CAB1-K1</Value></Attribute>
        <Attribute Name="OrderNumber"><Value>R-24</Value></Attribute>
        <Attribute Name="Manufacturer"><Value>Acme</Value></Attribute>
        <Attribute Name="Coil"><Attribute Name="Voltage"><Value>24 V DC</Value></Attribute></Attribute>
        <ExternalInterface Name="A1" ID="i1"><Attribute Name="Ignored"><Value>x</Value></Attribute></ExternalInterface>
        <ExternalInterface Name="A2" ID="i2"/>
      </InternalElement>
      <InternalElement Name="X1" ID="{e2}">
        <ExternalInterface Name="1" ID="i3"/>
        <ExternalInterface Name="2" ID="i4"/>
      </InternalElement>
      <InternalLink Name="101" RefPartnerSideA="i1" RefPartnerSideB="i3"/>
      <InternalLink Name="102" RefPartnerSideA="{e1}:A2" RefPartnerSideB="e2:2"/>
      <InternalLink Name="bad" RefPartnerSideA="nope" RefPartnerSideB="i3"/>
    </InternalElement>
  </InstanceHierarchy>
</CAEXFile>"#;

    #[test]
    fn parses_devices_links_and_properties() -> Result<()> {
        let imp = parse(SAMPLE)?;
        assert_eq!(imp.devices.len(), 2, "the function element is structure, not a device");
        let k1 = &imp.devices[0];
        assert_eq!(k1.tag.as_ref().map(|t| t.to_string()).as_deref(), Some("=PUMP+CAB1-K1"));
        assert_eq!(k1.properties.get("Coil.Voltage").map(String::as_str), Some("24 V DC"));
        assert!(!k1.properties.contains_key("Ignored"));
        assert_eq!(
            imp.devices[1].tag.as_ref().map(|t| t.to_string()).as_deref(),
            Some("-X1")
        );
        assert_eq!(imp.links.len(), 2);
        assert_eq!(imp.links[1].b, (1, "2".to_string()));
        assert_eq!(imp.warnings.len(), 1);
        Ok(())
    }

    #[test]
    fn builds_connected_pages() -> Result<()> {
        let mut project = wc_core::Project::new("t");
        let b = build(&parse(SAMPLE)?, 10);
        assert_eq!(b.pages.len(), 1);
        assert_eq!(b.pages[0].number, "10");
        assert_eq!(b.parts.len(), 1);
        assert_eq!(b.parts[0].id, "acme.r-24");
        project.pages = b.pages;
        let nets = wc_core::connectivity::nets(&project);
        assert_eq!(nets.len(), 2);
        assert!(nets.iter().all(|n| n.pins.len() == 2));
        Ok(())
    }

    #[test]
    fn hostile() {
        assert!(parse("").is_err());
        assert!(parse("<html/>").is_err());
        assert!(parse("<CAEXFile><InternalElement>").is_ok());
    }

    #[test]
    fn reads_back_its_own_export() -> Result<()> {
        // Export a small project with the report crate's AutomationML writer and import it:
        // devices, part data and connections survive the trip.
        let sym = |id: &str, pins: &[&str]| Symbol {
            id: id.into(),
            name: id.into(),
            category: String::new(),
            letter_code: String::new(),
            role: SymbolRole::Standalone,
            graphics: vec![],
            pins: pins
                .iter()
                .map(|p| Pin {
                    name: (*p).into(),
                    at: Point::default(),
                    dir: PinDir::Down,
                })
                .collect(),
            tag_at: Point::default(),
            description: String::new(),
        };
        let tag = |t: &str| DeviceTag::parse(t).map_err(|_| Error::Format(t.into()));
        let mut project = wc_core::Project::new("Round & trip");
        let mut page = Page::new("p1", "1", "Control");
        for (id, s, t) in [("s1", "relay", "=A-K1"), ("s2", "lamp", "=A-P1")] {
            page.symbols.push(PlacedSymbol {
                id: id.into(),
                symbol: s.into(),
                at: Point::default(),
                rotation: Default::default(),
                mirror: false,
                tag: Some(tag(t)?),
            });
        }
        let end = |item: &str, pin: &str| Endpoint::Pin {
            item: item.into(),
            pin: pin.into(),
        };
        page.wires.push(Wire {
            id: "w1".into(),
            from: end("s1", "14"),
            to: end("s2", "X1"),
            via: vec![],
            number: None,
            potential: Some("L+".into()),
            cable: None,
            route: Default::default(),
        });
        project.pages.push(page);
        let part = wc_core::Part {
            id: "acme.r24".into(),
            manufacturer: "Acme".into(),
            order_number: "R-24".into(),
            ..Default::default()
        };
        project.devices.insert(
            tag("=A-K1")?,
            Device {
                part: Some(part.id.clone()),
                description: "Pump relay".into(),
                properties: BTreeMap::new(),
            },
        );
        let symbols: BTreeMap<String, Symbol> = [sym("relay", &["13", "14"]), sym("lamp", &["X1", "X2"])]
            .into_iter()
            .map(|s| (s.id.clone(), s))
            .collect();
        let parts = [(part.id.clone(), part)].into_iter().collect();
        let (xml, _) = wc_report::automationml(
            &wc_report::Inputs {
                project: &project,
                symbols: &symbols,
                parts: &parts,
            },
            "",
        );
        let imp = parse(&xml)?;
        let k1_tag = tag("=A-K1")?;
        let k1 = imp.devices.iter().find(|d| d.tag.as_ref() == Some(&k1_tag));
        let k1 = k1.ok_or_else(|| Error::Format(xml.clone()))?;
        assert_eq!(k1.interfaces, ["13", "14"]);
        let mut parts_out = Vec::new();
        let (_, dev) = device_entry(k1, &mut parts_out).ok_or_else(|| Error::Format("no device".into()))?;
        assert_eq!(dev.description, "Pump relay");
        assert_eq!(
            (parts_out[0].manufacturer.as_str(), parts_out[0].order_number.as_str()),
            ("Acme", "R-24")
        );
        assert_eq!(imp.links.len(), 1);
        let l = &imp.links[0];
        assert_eq!(l.name, "L+");
        let ends = [
            (imp.devices[l.a.0].name.as_str(), l.a.1.as_str()),
            (imp.devices[l.b.0].name.as_str(), l.b.1.as_str()),
        ];
        assert_eq!(ends, [("=A-K1", "14"), ("=A-P1", "X1")]);
        Ok(())
    }

    #[test]
    fn location_structure_elements_become_project_locations() -> Result<()> {
        let text = r#"<CAEXFile><InstanceHierarchy Name="H">
    <InternalElement Name="+L1" ID="l1">
      <Attribute Name="Description"><Value>Main cabinet</Value></Attribute>
      <InternalElement Name="+L3" ID="l3">
        <InternalElement Name="K1" ID="{e1}">
          <ExternalInterface Name="A1" ID="i1"/>
        </InternalElement>
      </InternalElement>
      <InternalElement Name="+L1.L4" ID="l4"/>
    </InternalElement>
    <InternalElement Name="+L9" ID="l9">
      <Attribute Name="Description"><Value>Engine room</Value></Attribute>
      <InternalElement Name="+L9-K2" ID="k2"/>
      <InternalElement Name="+L9" ID="l9b"/>
    </InternalElement>
    <InternalElement Name="+L7" ID="l7">
      <Attribute Name="ReferenceDesignation"><Value>F1</Value></Attribute>
    </InternalElement>
    <InternalElement Name="Cabinet" ID="c2">
      <Attribute Name="RefDes"><Value>+L2</Value></Attribute>
      <InternalElement Name="+L5" ID="l5"/>
    </InternalElement>
    <InternalElement Name="=PUMP" ID="f1"/>
  </InstanceHierarchy></CAEXFile>"#;
        let imp = parse(text)?;
        let built = build(&imp, 1);
        let got: Vec<(&str, &str)> = built
            .locations
            .iter()
            .map(|l| (l.path.as_str(), l.description.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("L1", "Main cabinet"),
                ("L1.L3", ""),
                ("L1.L4", ""),
                ("L2", ""),
                ("L2.L5", ""),
                ("L7", ""),
                ("L9", "Engine room")
            ]
        );
        assert_eq!(imp.devices.len(), 1, "locations are not devices");
        assert!(
            imp.warnings
                .iter()
                .any(|w| w.contains("+L9-K2") && w.contains("not a location")),
            "{:?}",
            imp.warnings
        );
        Ok(())
    }
}

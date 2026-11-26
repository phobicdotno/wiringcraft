//! AutomationML (IEC 62714, CAEX 3.0) export of a project's devices and connections, for tools
//! that take engineering data rather than drawings.
//!
//! Each device becomes an `InternalElement` named by its tag, with its description, part data
//! and properties as attributes and one `ExternalInterface` per connection point drawn on any of
//! its symbols. Each potential (net) becomes a chain of `InternalLink`s between the connection
//! points it joins, named after the potential. Interruption points are not devices: they only
//! join potentials. Symbols without a tag have no device, so their pins are left out.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use wc_core::DeviceTag;
use wc_core::symbol::SymbolRole;

use crate::{Inputs, natural_key, net_names};

/// `s` escaped for XML text and attribute values.
///
/// Markup characters become entities. Tab, line feed and carriage return become `&#9;`, `&#10;`
/// and `&#13;`, so a reader does not turn them into spaces when it normalises an attribute value.
/// Every other character is kept only when XML 1.0 allows it (the `Char` production:
/// `#x20-#xD7FF`, `#xE000-#xFFFD` and `#x10000-#x10FFFF`); the rest, such as other C0 control
/// characters, `U+FFFE` and `U+FFFF`, are dropped.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}' => out.push(ch),
            _ => {}
        }
    }
    out
}

fn attribute(out: &mut String, indent: &str, name: &str, value: &str) {
    let _ = writeln!(
        out,
        "{indent}<Attribute Name=\"{}\" AttributeDataType=\"xs:string\"><Value>{}</Value></Attribute>",
        esc(name),
        esc(value)
    );
}

/// What the export wrote, for a summary line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AmlCounts {
    pub devices: usize,
    pub links: usize,
    /// Pins on symbols without a tag, which have no device to belong to.
    pub untagged_pins: usize,
}

/// The project as an AutomationML document. `written` is the time stamp CAEX asks for
/// (`2026-11-16T22:00:00Z`); the caller passes it so the same project always gives the same file.
/// `file_name` is the name of the file being written (`plant-v2.aml`), which goes in the CAEX
/// `FileName` attribute; when it is empty, the project name plus `.aml` is used instead.
pub fn automationml(i: &Inputs<'_>, written: &str, file_name: &str) -> (String, AmlCounts) {
    let mut counts = AmlCounts::default();
    let is_device = |page: &str, item: &str| -> Option<DeviceTag> {
        let s = i.project.page(page)?.symbol(item)?;
        if i.symbols
            .get(&s.symbol)
            .is_some_and(|y| y.role == SymbolRole::Interruption)
        {
            return None;
        }
        s.tag.clone()
    };
    // Every device with the connection points drawn for it, on any page.
    let mut pins: BTreeMap<DeviceTag, BTreeSet<String>> = BTreeMap::new();
    // Tags drawn only as interruption points name a potential, not a device.
    let index = i.project.tag_index();
    for tag in i.project.devices.keys() {
        let uses = index.get(tag).map(Vec::as_slice).unwrap_or(&[]);
        let only_interruptions = !uses.is_empty() && uses.iter().all(|(page, item)| is_device(page, item).is_none());
        if !only_interruptions {
            pins.entry(tag.clone()).or_default();
        }
    }
    for page in &i.project.pages {
        for s in &page.symbols {
            let Some(tag) = is_device(&page.id, &s.id) else {
                continue;
            };
            let entry = pins.entry(tag).or_default();
            if let Some(sym) = i.symbols.get(&s.symbol) {
                entry.extend(sym.pins.iter().map(|p| p.name.clone()));
            }
        }
    }
    let mut tags: Vec<&DeviceTag> = pins.keys().collect();
    tags.sort_by_key(|t| natural_key(&t.to_string()));
    let ids: BTreeMap<&DeviceTag, String> = tags
        .iter()
        .enumerate()
        .map(|(n, t)| (*t, format!("d{}", n + 1)))
        .collect();
    let interface_id = |tag: &DeviceTag, pin: &str| -> Option<String> {
        let id = ids.get(tag)?;
        let k = pins.get(tag)?.iter().position(|p| p == pin)?;
        Some(format!("{id}.c{}", k + 1))
    };

    let mut out = String::new();
    let name = if i.project.meta.name.trim().is_empty() {
        "Project"
    } else {
        i.project.meta.name.trim()
    };
    let file_name = if file_name.is_empty() {
        format!("{name}.aml")
    } else {
        file_name.to_string()
    };
    let _ = writeln!(
        out,
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<CAEXFile FileName=\"{}\" SchemaVersion=\"3.0\" \
         xmlns=\"http://www.dke.de/CAEX\" xmlns:xs=\"http://www.w3.org/2001/XMLSchema\">\n  \
         <SourceDocumentInformation OriginName=\"WiringCraft\" OriginID=\"wiringcraft\" OriginVersion=\"{}\" \
         LastWritingDateTime=\"{}\"/>\n  <InstanceHierarchy Name=\"{}\" Version=\"1\">\n    \
         <InternalElement Name=\"{}\" ID=\"project\">",
        esc(&file_name),
        env!("CARGO_PKG_VERSION"),
        esc(if written.is_empty() {
            "1970-01-01T00:00:00Z"
        } else {
            written
        }),
        esc(name),
        esc(name)
    );
    for tag in &tags {
        let Some(id) = ids.get(*tag) else { continue };
        counts.devices += 1;
        let tag_text = tag.to_string();
        let _ = writeln!(out, "      <InternalElement Name=\"{}\" ID=\"{id}\">", esc(&tag_text));
        let ind = "        ";
        attribute(&mut out, ind, "ReferenceDesignation", &tag_text);
        if let Some(dev) = i.project.devices.get(*tag) {
            if !dev.description.is_empty() {
                attribute(&mut out, ind, "Function text", &dev.description);
            }
            if let Some(part) = dev.part.as_ref().and_then(|p| i.parts.get(p)) {
                if !part.manufacturer.is_empty() {
                    attribute(&mut out, ind, "Manufacturer", &part.manufacturer);
                }
                attribute(&mut out, ind, "OrderNumber", &part.order_number);
                if !part.description.is_empty() {
                    attribute(&mut out, ind, "PartDescription", &part.description);
                }
            } else if let Some(p) = &dev.part {
                attribute(&mut out, ind, "OrderNumber", p);
            }
            let reserved = [
                "ReferenceDesignation",
                "Function text",
                "Manufacturer",
                "OrderNumber",
                "PartDescription",
            ];
            for (k, v) in dev.properties.iter().filter(|(k, _)| !reserved.contains(&k.as_str())) {
                attribute(&mut out, ind, k, v);
            }
        }
        for pin in pins.get(*tag).into_iter().flatten() {
            if let Some(iid) = interface_id(tag, pin) {
                let _ = writeln!(out, "{ind}<ExternalInterface Name=\"{}\" ID=\"{iid}\"/>", esc(pin));
            }
        }
        out.push_str("      </InternalElement>\n");
    }
    // Connections: each net's device pins, chained in a stable order.
    let (nets, _) = net_names(i);
    let mut used_names: BTreeMap<String, usize> = BTreeMap::new();
    for (n, net) in nets.iter().enumerate() {
        let mut ends: Vec<(String, String)> = Vec::new();
        for (page, item, pin) in &net.pins {
            match is_device(page, item) {
                Some(tag) => {
                    if let Some(iid) = interface_id(&tag, pin) {
                        ends.push((format!("{tag}:{pin}"), iid));
                    }
                }
                None => {
                    let untagged = i
                        .project
                        .page(page)
                        .and_then(|p| p.symbol(item))
                        .is_some_and(|s| s.tag.is_none());
                    if untagged {
                        counts.untagged_pins += 1;
                    }
                }
            }
        }
        ends.sort_by_key(|(label, _)| natural_key(label));
        ends.dedup_by(|a, b| a.1 == b.1);
        let base = net.names.first().cloned().unwrap_or_else(|| format!("N{}", n + 1));
        for pair in ends.windows(2) {
            let count = used_names.entry(base.clone()).or_insert(0);
            *count += 1;
            let link = if *count == 1 {
                base.clone()
            } else {
                format!("{base}#{count}")
            };
            let _ = writeln!(
                out,
                "      <InternalLink Name=\"{}\" RefPartnerSideA=\"{}\" RefPartnerSideB=\"{}\"/>",
                esc(&link),
                pair[0].1,
                pair[1].1
            );
            counts.links += 1;
        }
    }
    out.push_str("    </InternalElement>\n  </InstanceHierarchy>\n</CAEXFile>\n");
    (out, counts)
}

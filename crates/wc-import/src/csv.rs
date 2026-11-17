//! Parts lists and PLC address lists from CSV, as written by spreadsheet programs and most
//! CAD/ERP/PLC exports.

use std::collections::{BTreeMap, BTreeSet};

use wc_core::{DeviceTag, Part, Project};

use crate::{Error, Result};

/// Decode bytes as UTF-8 (with or without BOM), falling back to Windows-1252/Latin-1, which is
/// what Excel writes on most European Windows machines.
pub fn decode(bytes: &[u8]) -> String {
    let b = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_owned(),
        Err(_) => b
            .iter()
            .map(|&c| match c {
                0x80 => '€',
                0x96 => '–',
                0x97 => '—',
                c => c as char,
            })
            .collect(),
    }
}

/// Split CSV text into records. Handles quotes, doubled quotes and newlines inside quotes.
pub fn records(text: &str, delim: char) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' && field.is_empty() {
            quoted = true;
        } else if c == delim {
            row.push(std::mem::take(&mut field));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut field));
            if row.iter().any(|f| !f.trim().is_empty()) {
                out.push(std::mem::take(&mut row));
            } else {
                row.clear();
            }
        } else {
            field.push(c);
        }
    }
    row.push(field);
    if row.iter().any(|f| !f.trim().is_empty()) {
        out.push(row);
    }
    out
}

fn guess_delimiter(header: &str) -> char {
    [';', '\t', ',', '|']
        .into_iter()
        .max_by_key(|d| header.matches(*d).count())
        .unwrap_or(',')
}

#[derive(Clone, Copy, PartialEq)]
enum Col {
    Manufacturer,
    OrderNumber,
    Description,
    Symbol,
    Other,
}

fn classify(h: &str) -> Col {
    let k: String = h.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
    match k.as_str() {
        "manufacturer" | "maker" | "brand" | "vendor" | "hersteller" | "produsent" | "leverandor" | "leverandør"
        | "fabrikant" => Col::Manufacturer,
        "ordernumber" | "orderno" | "ordernr" | "partnumber" | "partno" | "articlenumber" | "article" | "articleno"
        | "artikelnummer" | "bestellnummer" | "bestillingsnummer" | "varenummer" | "typenumber" | "type"
        | "catalognumber" | "catno" | "mpn" => Col::OrderNumber,
        "description" | "designation" | "name" | "text" | "beschreibung" | "bezeichnung" | "beskrivelse"
        | "betegnelse" => Col::Description,
        "symbol" | "symbolid" => Col::Symbol,
        _ => Col::Other,
    }
}

/// Parse a parts list. The first row is the header; columns are matched by name (several
/// languages), anything unrecognised becomes a part property. Returns parts and warnings.
pub fn parts(text: &str) -> Result<(Vec<Part>, Vec<String>)> {
    let first_line = text.lines().next().unwrap_or("");
    let rows = records(text, guess_delimiter(first_line));
    let Some((header, body)) = rows.split_first() else {
        return Err(Error::Format("the file is empty".into()));
    };
    let cols: Vec<Col> = header.iter().map(|h| classify(h)).collect();
    if !cols.contains(&Col::OrderNumber) {
        return Err(Error::Format(format!("no order/part number column among {header:?}")));
    }
    let mut out: BTreeMap<String, Part> = BTreeMap::new();
    let mut warnings = Vec::new();
    for (i, row) in body.iter().enumerate() {
        let mut p = Part::default();
        for ((col, h), v) in cols.iter().zip(header).zip(row) {
            let v = v.trim();
            if v.is_empty() {
                continue;
            }
            match col {
                Col::Manufacturer => p.manufacturer = v.into(),
                Col::OrderNumber => p.order_number = v.into(),
                Col::Description => p.description = v.into(),
                Col::Symbol => p.symbol = Some(v.into()),
                Col::Other => {
                    p.properties.insert(h.trim().into(), v.into());
                }
            }
        }
        if p.order_number.is_empty() {
            warnings.push(format!("row {}: no order number, skipped", i + 2));
            continue;
        }
        crate::add_part(&mut out, p, &format!("row {}", i + 2), &mut warnings);
    }
    Ok((out.into_values().collect(), warnings))
}

/// One row of a PLC address list: which device gets which address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlcAddress {
    pub tag: DeviceTag,
    pub address: String,
    /// Empty when the file has no description for the row.
    pub description: String,
}

/// What applying a PLC address list changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlcApplied {
    /// Devices whose address or description changed.
    pub changed: usize,
    /// Rows whose address and description were already set.
    pub unchanged: usize,
    /// Tags in the file that no symbol in the project carries.
    pub unknown: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum PlcCol {
    Tag,
    Address,
    Description,
    Other,
}

fn classify_plc(h: &str) -> PlcCol {
    let k: String = h.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
    match k.as_str() {
        "device" | "tag" | "devicetag" | "reference" | "designation" | "bmk" | "betriebsmittel" | "komponent"
        | "utstyr" => PlcCol::Tag,
        "address" | "plcaddress" | "ioaddress" | "io" | "operand" | "adresse" | "plcadresse" | "symboladdress" => {
            PlcCol::Address
        }
        "description" | "comment" | "text" | "symboltext" | "functiontext" | "beschreibung" | "kommentar"
        | "beskrivelse" | "kommentar2" => PlcCol::Description,
        _ => PlcCol::Other,
    }
}

/// Parse a PLC address list: a header row with a device column and an address column (names
/// in several languages; a description column is optional). The `plc` report written as CSV
/// reads back unchanged. Returns the rows and warnings for rows that were skipped.
pub fn plc_addresses(text: &str) -> Result<(Vec<PlcAddress>, Vec<String>)> {
    let first_line = text.lines().next().unwrap_or("");
    let rows = records(text, guess_delimiter(first_line));
    let Some((header, body)) = rows.split_first() else {
        return Err(Error::Format("the file is empty".into()));
    };
    let cols: Vec<PlcCol> = header.iter().map(|h| classify_plc(h)).collect();
    let find = |c: PlcCol| cols.iter().position(|x| *x == c);
    let (Some(tag_col), Some(addr_col)) = (find(PlcCol::Tag), find(PlcCol::Address)) else {
        return Err(Error::Format(format!(
            "need a device column and an address column among {header:?}"
        )));
    };
    let desc_col = find(PlcCol::Description);
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for (i, row) in body.iter().enumerate() {
        let cell = |c: usize| row.get(c).map(|v| v.trim()).unwrap_or("");
        let (tag, address) = (cell(tag_col), cell(addr_col));
        if tag.is_empty() || tag.ends_with("(no tag)") {
            warnings.push(format!("row {}: no device, skipped", i + 2));
            continue;
        }
        let Ok(tag) = DeviceTag::parse(tag) else {
            warnings.push(format!("row {}: {tag:?} is not a device tag, skipped", i + 2));
            continue;
        };
        out.push(PlcAddress {
            tag,
            address: address.to_owned(),
            description: desc_col.map(cell).unwrap_or("").to_owned(),
        });
    }
    Ok((out, warnings))
}

/// Write PLC addresses into the project's devices: the device property `Address` (or the
/// existing `PLC address` key when the device has one), and the description when the row has
/// one. An empty address in the file clears it. Tags no symbol carries are reported, not added.
pub fn apply_plc_addresses(project: &mut Project, rows: &[PlcAddress]) -> PlcApplied {
    let drawn: BTreeSet<DeviceTag> = project
        .pages
        .iter()
        .flat_map(|p| p.symbols.iter().filter_map(|s| s.tag.clone()))
        .collect();
    let mut applied = PlcApplied::default();
    for row in rows {
        if !drawn.contains(&row.tag) && !project.devices.contains_key(&row.tag) {
            applied.unknown.push(row.tag.to_string());
            continue;
        }
        let dev = project.devices.entry(row.tag.clone()).or_default();
        let before = dev.clone();
        let key = dev
            .properties
            .keys()
            .find(|k| k.eq_ignore_ascii_case("address") || k.eq_ignore_ascii_case("plc address"))
            .cloned()
            .unwrap_or_else(|| "Address".to_owned());
        if row.address.is_empty() {
            dev.properties.remove(&key);
        } else {
            dev.properties.insert(key, row.address.clone());
        }
        if !row.description.is_empty() {
            dev.description = row.description.clone();
        }
        if *dev == before {
            applied.unchanged += 1;
        } else {
            applied.changed += 1;
        }
    }
    applied
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semicolon_latin1_with_quotes() -> Result<()> {
        let bytes = b"Hersteller;Bestellnummer;Beschreibung;Spannung\r\nAcme;\"R-24;2\";Rel\xe6 24 V;24 V DC\r\n;;\r\nAcme;X1;\"Klemme \"\"grau\"\"\";\r\n";
        let (parts, warnings) = parts(&decode(bytes))?;
        assert_eq!(parts.len(), 2, "{warnings:?}");
        let r = &parts[0];
        assert_eq!(r.order_number, "R-24;2");
        assert_eq!(r.description, "Relæ 24 V");
        assert_eq!(r.properties.get("Spannung").map(String::as_str), Some("24 V DC"));
        assert_eq!(parts[1].description, "Klemme \"grau\"");
        Ok(())
    }

    #[test]
    fn needs_an_order_number_column() {
        assert!(parts("a,b\n1,2\n").is_err());
        assert!(parts("").is_err());
    }

    #[test]
    fn rows_without_number_warn() -> Result<()> {
        let (p, w) = parts("Part number,Description\n,orphan\nA1,ok\n")?;
        assert_eq!(p.len(), 1);
        assert_eq!(w.len(), 1);
        Ok(())
    }

    #[test]
    fn plc_addresses_read_back_the_plc_report_and_apply_to_devices() -> Result<()> {
        use wc_core::{Page, PlacedSymbol};
        let text = "Address;Type;Device;Description;Connected to;Page\n\
                    I0.1;Input;-A1.DI1;Start button;-S1:13;3\n\
                    Q0.0;Output;-A1.DQ1;;-K1:A1;3\n\
                    ;Input;-A1.DI2;;;3\n\
                    I0.3;Input;di3 (no tag);;;3\n\
                    I0.4;Input;-Z9;Not drawn;;3\n";
        let (rows, warnings) = plc_addresses(text)?;
        assert_eq!(rows.len(), 4, "{warnings:?}");
        assert_eq!(warnings.len(), 1, "the untagged row");
        let mut project = Project::new("x");
        let mut page = Page::new("p3", "3", "PLC");
        for (id, tag) in [("a", "-A1.DI1"), ("b", "-A1.DQ1"), ("c", "-A1.DI2")] {
            page.symbols.push(PlacedSymbol {
                id: id.into(),
                symbol: "plc".into(),
                at: Default::default(),
                rotation: Default::default(),
                mirror: false,
                tag: DeviceTag::parse(tag).ok(),
            });
        }
        project.pages.push(page);
        let dq1 = DeviceTag::parse("-A1.DQ1")?;
        let di2 = DeviceTag::parse("-A1.DI2")?;
        project
            .devices
            .entry(dq1.clone())
            .or_default()
            .properties
            .insert("PLC address".into(), "Q9.9".into());
        project
            .devices
            .entry(di2.clone())
            .or_default()
            .properties
            .insert("Address".into(), "I7.7".into());
        let applied = apply_plc_addresses(&mut project, &rows);
        assert_eq!(applied.unknown, vec!["-Z9".to_owned()]);
        assert_eq!(applied.changed, 3);
        let di1 = project.devices.get(&DeviceTag::parse("-A1.DI1")?);
        assert_eq!(
            di1.and_then(|d| d.properties.get("Address")).map(String::as_str),
            Some("I0.1")
        );
        assert_eq!(di1.map(|d| d.description.as_str()), Some("Start button"));
        let dq1 = project.devices.get(&dq1);
        assert_eq!(
            dq1.and_then(|d| d.properties.get("PLC address")).map(String::as_str),
            Some("Q0.0")
        );
        assert_eq!(dq1.map(|d| d.properties.len()), Some(1), "the existing key is reused");
        assert!(
            project.devices.get(&di2).is_some_and(|d| d.properties.is_empty()),
            "empty address clears"
        );
        // Applying again changes nothing.
        let again = apply_plc_addresses(&mut project, &rows);
        assert_eq!((again.changed, again.unchanged), (0, 3));
        assert!(plc_addresses("Address,Type\nI0.0,Input\n").is_err());
        Ok(())
    }
}

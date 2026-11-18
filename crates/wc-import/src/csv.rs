//! Parts lists and PLC address lists from CSV, as written by spreadsheet programs and most
//! CAD/ERP/PLC exports, or from Excel workbooks through [`file_rows`].

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

/// A field with the `'` a CSV export puts before text a spreadsheet would run as a formula
/// (`'=A+C1-K1`, `'-K1`, see `wc_report::formula_safe`) taken off again. A `'` before anything
/// else is part of the text and kept.
fn unguard(field: String) -> String {
    match field.strip_prefix('\'') {
        Some(rest) if rest.starts_with(['=', '+', '-', '@', '\t', '\r']) => rest.to_owned(),
        _ => field,
    }
}

/// Split CSV text into records. Handles quotes, doubled quotes and newlines inside quotes, and
/// takes off the `'` WiringCraft's CSV reports put before cells such as `-K1` (`'-K1` reads `-K1`).
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
            row.push(unguard(std::mem::take(&mut field)));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(unguard(std::mem::take(&mut field)));
            if row.iter().any(|f| !f.trim().is_empty()) {
                out.push(std::mem::take(&mut row));
            } else {
                row.clear();
            }
        } else {
            field.push(c);
        }
    }
    row.push(unguard(field));
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

/// What a CSV list holds, told from its header row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsvKind {
    /// A device column and an address column: read with [`plc_addresses`].
    PlcAddresses,
    /// An order/part number column: read with [`parts`].
    Parts,
}

/// Tell a PLC address list from a parts list by its header, the same column names the two
/// readers accept. A header with both a device and an address column is a PLC list, even when
/// it also names a type or article; `None` when neither reader would take the file.
pub fn kind(text: &str) -> Option<CsvKind> {
    // The same header the readers take: the first record that is not blank, quoted line
    // breaks and all.
    let first_line = text.lines().next().unwrap_or("");
    kind_of_rows(&records(text, guess_delimiter(first_line)))
}

/// [`kind`] for rows already split into cells, such as an Excel worksheet's.
pub fn kind_of_rows(rows: &[Vec<String>]) -> Option<CsvKind> {
    let header = rows.first()?;
    let plc: Vec<PlcCol> = header.iter().map(|h| classify_plc(h)).collect();
    if plc.contains(&PlcCol::Tag) && plc.contains(&PlcCol::Address) {
        return Some(CsvKind::PlcAddresses);
    }
    header
        .iter()
        .any(|h| classify(h) == Col::OrderNumber)
        .then_some(CsvKind::Parts)
}

/// The rows of a list file: an Excel workbook (`.xlsx`, its first visible sheet) or CSV text in
/// any of the encodings [`decode`] reads. An old binary `.xls` workbook is refused with a hint.
pub fn file_rows(bytes: &[u8]) -> Result<Vec<Vec<String>>> {
    if crate::xlsx::is_xlsx(bytes) {
        return crate::xlsx::rows(bytes);
    }
    if crate::xlsx::is_legacy_xls(bytes) {
        return Err(Error::Format(
            "this is an old Excel .xls workbook; save it as .xlsx or CSV and import that".into(),
        ));
    }
    let text = decode(bytes);
    let first_line = text.lines().next().unwrap_or("");
    Ok(records(&text, guess_delimiter(first_line)))
}

/// Parse a parts list. The first row is the header; columns are matched by name (several
/// languages), anything unrecognised becomes a part property. Returns parts and warnings.
pub fn parts(text: &str) -> Result<(Vec<Part>, Vec<String>)> {
    let first_line = text.lines().next().unwrap_or("");
    parts_from_rows(&records(text, guess_delimiter(first_line)))
}

/// [`parts`] for rows already split into cells: an Excel worksheet's, or [`file_rows`]'.
pub fn parts_from_rows(rows: &[Vec<String>]) -> Result<(Vec<Part>, Vec<String>)> {
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
    plc_addresses_from_rows(&records(text, guess_delimiter(first_line)))
}

/// [`plc_addresses`] for rows already split into cells: an Excel worksheet's, or [`file_rows`]'.
pub fn plc_addresses_from_rows(rows: &[Vec<String>]) -> Result<(Vec<PlcAddress>, Vec<String>)> {
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

    #[test]
    fn kind_tells_plc_lists_from_parts_lists_by_header() {
        assert_eq!(kind("Device;Address;Comment\n-K1;%I0.0;x"), Some(CsvKind::PlcAddresses));
        // A PLC list that also names the type is still a PLC list.
        assert_eq!(kind("Tag,Type,PLC address\n-K1,X,%Q0.1"), Some(CsvKind::PlcAddresses));
        assert_eq!(
            kind(&decode(b"\xEF\xBB\xBFHersteller;Artikelnummer\nSIE;3RT2015")),
            Some(CsvKind::Parts)
        );
        assert_eq!(kind("Manufacturer,Order number\nABB,1SBL"), Some(CsvKind::Parts));
        assert_eq!(kind("Name,Value\na,1"), None);
        // A blank first line and a quoted line break in a header name, as the readers take them.
        assert_eq!(kind("\nOrder number\nA1\n"), Some(CsvKind::Parts));
        let text = "\"Device\";\"PLC\naddress\"\n-K1;%I0.0";
        assert_eq!(kind(text), Some(CsvKind::PlcAddresses));
        assert_eq!(
            plc_addresses(text).map(|(rows, _)| rows.len()).ok(),
            Some(1),
            "the reader agrees"
        );
        assert_eq!(kind(""), None);
    }
    use super::*;

    #[test]
    fn a_csv_report_reads_back_with_the_tags_it_guarded() -> Result<()> {
        let t = wc_report::Table {
            title: "PLC".into(),
            columns: vec!["Device".into(), "Address".into(), "Description".into()],
            rows: vec![
                vec!["-K1".into(), "%I0.0".into(), "-5".into()],
                vec!["-X12".into(), "%I0.1".into(), "=A+C1-K1".into()],
                vec!["+C1-K2".into(), "%I0.2".into(), "'quoted' text".into()],
            ],
            ..wc_report::Table::default()
        };
        let csv = t.to_csv_with(';');
        // A spreadsheet sees text, not a formula, in each tag cell...
        assert!(csv.contains("'-K1;"), "{csv}");
        assert!(csv.contains("'-X12;"), "{csv}");
        assert!(csv.contains(";-5\r\n"), "{csv}");
        // ...and the import reads every cell back as it was.
        let rows = records(&csv, ';');
        assert_eq!(rows[1..], t.rows[..]);
        let (addresses, warnings) = plc_addresses(&csv)?;
        assert!(warnings.is_empty(), "{warnings:?}");
        let tags: Vec<String> = addresses.iter().map(|a| a.tag.to_string()).collect();
        assert_eq!(tags, ["-K1", "-X12", "+C1-K2"]);
        Ok(())
    }

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

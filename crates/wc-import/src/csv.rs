//! Parts lists from CSV, as written by spreadsheet programs and most CAD/ERP exports.

use std::collections::BTreeMap;

use wc_core::Part;

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
        p.id = Part::make_id(&p.manufacturer, &p.order_number);
        if wc_core::store::check_id(&p.id).is_err() {
            warnings.push(format!(
                "row {}: cannot make an id from {:?}, skipped",
                i + 2,
                p.order_number
            ));
            continue;
        }
        if out.insert(p.id.clone(), p).is_some() {
            warnings.push(format!("row {}: duplicate part, the later row wins", i + 2));
        }
    }
    Ok((out.into_values().collect(), warnings))
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
}

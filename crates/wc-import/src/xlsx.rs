//! Lists from Excel workbooks (`.xlsx`, `.xlsm`): the first visible worksheet read as rows of
//! text, the same shape [`crate::csv::records`] gives for a CSV file, so parts lists and PLC
//! address lists are read with the same column names either way.
//!
//! Cells keep the text the workbook stores: shared and inline strings as written, numbers in
//! their shortest form rounded to the 15 digits a spreadsheet program shows (`1492`, `0.3`),
//! booleans as `TRUE`/`FALSE`, and a formula as its last calculated value. Number formats are not
//! applied, so a date reads as its serial number. The old binary `.xls` format is not read.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

use crate::{Error, Result};

/// Largest workbook part read into memory.
const MAX_PART: u64 = 64 * 1024 * 1024;
/// Excel's own limits: columns `A` to `XFD`, rows 1 to 1,048,576.
const MAX_COLUMNS: usize = 16_384;
const MAX_ROWS: usize = 1_048_576;

/// Whether `bytes` look like an Office Open XML package (a zip file).
pub fn is_xlsx(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

/// Whether `bytes` are an old binary Excel workbook (`.xls`, an OLE compound file).
pub fn is_legacy_xls(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1")
}

/// The first visible worksheet as rows of cell text. Rows with no text are left out; a row is
/// as long as its last cell, and cells a row skips are empty strings.
pub fn rows(bytes: &[u8]) -> Result<Vec<Vec<String>>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| Error::Format(format!("xlsx: {e}")))?;
    let workbook = part(&mut zip, "xl/workbook.xml")?
        .ok_or_else(|| Error::Format("xlsx: no xl/workbook.xml, not an Excel workbook".into()))?;
    let rels = part(&mut zip, "xl/_rels/workbook.xml.rels")?.unwrap_or_default();
    let rels = relationships(&rels)?;
    let sheet_path = match first_sheet(&workbook)? {
        Some(id) => rels
            .iter()
            .find(|r| r.id == id)
            .map(|r| target_path(&r.target))
            .ok_or_else(|| Error::Format(format!("xlsx: the first sheet's relationship {id:?} is missing")))?,
        None => "xl/worksheets/sheet1.xml".into(),
    };
    let strings_path = rels
        .iter()
        .find(|r| r.kind.ends_with("/sharedStrings"))
        .map(|r| target_path(&r.target))
        .unwrap_or_else(|| "xl/sharedStrings.xml".into());
    let strings = match part(&mut zip, &strings_path)? {
        Some(xml) => shared_strings(&xml)?,
        None => Vec::new(),
    };
    let sheet = part(&mut zip, &sheet_path)?
        .ok_or_else(|| Error::Format(format!("xlsx: the worksheet {sheet_path} is missing")))?;
    sheet_rows(&sheet, &strings)
}

/// A part of the package as text, `None` when the package has no such part.
fn part(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Option<String>> {
    let f = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(Error::Format(format!("xlsx: {name}: {e}"))),
    };
    let mut data = Vec::new();
    f.take(MAX_PART + 1)
        .read_to_end(&mut data)
        .map_err(|e| Error::Format(format!("xlsx: {name}: {e}")))?;
    if data.len() as u64 > MAX_PART {
        return Err(Error::Format(format!(
            "xlsx: {name} is larger than {} MB",
            MAX_PART / 1024 / 1024
        )));
    }
    Ok(Some(String::from_utf8_lossy(&data).into_owned()))
}

/// A relationship target as a package path: relative to `xl/` unless it starts with `/`.
fn target_path(target: &str) -> String {
    match target.strip_prefix('/') {
        Some(abs) => abs.to_owned(),
        None => format!("xl/{target}"),
    }
}

fn xml_error(e: impl std::fmt::Display) -> Error {
    Error::Format(format!("xlsx: {e}"))
}

fn attr(e: &BytesStart, name: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == name)
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

struct Relationship {
    id: String,
    kind: String,
    target: String,
}

fn relationships(xml: &str) -> Result<Vec<Relationship>> {
    let mut r = Reader::from_str(xml);
    let mut out = Vec::new();
    loop {
        match r.read_event().map_err(xml_error)? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"Relationship" => {
                out.push(Relationship {
                    id: attr(&e, b"Id").unwrap_or_default(),
                    kind: attr(&e, b"Type").unwrap_or_default(),
                    target: attr(&e, b"Target").unwrap_or_default(),
                });
            }
            Event::Eof => return Ok(out),
            _ => {}
        }
    }
}

/// The relationship id of the first sheet that is not hidden (or of the first sheet when all
/// are hidden); `None` when the workbook lists no sheets.
fn first_sheet(xml: &str) -> Result<Option<String>> {
    let mut r = Reader::from_str(xml);
    let mut first = None;
    loop {
        match r.read_event().map_err(xml_error)? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"sheet" => {
                let id = attr(&e, b"id");
                let hidden = attr(&e, b"state").is_some_and(|s| s != "visible");
                if !hidden && id.is_some() {
                    return Ok(id);
                }
                if first.is_none() {
                    first = id;
                }
            }
            Event::Eof => return Ok(first),
            _ => {}
        }
    }
}

/// The text of each `<si>` in order: plain or rich text runs joined, phonetic hints left out.
fn shared_strings(xml: &str) -> Result<Vec<String>> {
    let mut r = Reader::from_str(xml);
    let mut out = Vec::new();
    let mut current = String::new();
    let (mut in_t, mut in_phonetic) = (false, false);
    loop {
        match r.read_event().map_err(xml_error)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"si" => current.clear(),
                b"t" => in_t = true,
                b"rPh" => in_phonetic = true,
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                b"si" => out.push(std::mem::take(&mut current)),
                b"t" => in_t = false,
                b"rPh" => in_phonetic = false,
                _ => {}
            },
            Event::Empty(e) if e.local_name().as_ref() == b"si" => out.push(String::new()),
            Event::Text(t) if in_t && !in_phonetic => current.push_str(&t.unescape().map_err(xml_error)?),
            Event::CData(t) if in_t && !in_phonetic => current.push_str(&String::from_utf8_lossy(&t)),
            Event::Eof => return Ok(out),
            _ => {}
        }
    }
}

/// The zero-based column of a cell reference such as `B7` (`None` when it has no letters or
/// lies past column `XFD`).
fn column_of(reference: &str) -> Option<usize> {
    let letters: Vec<u8> = reference
        .bytes()
        .take_while(u8::is_ascii_alphabetic)
        .map(|b| b.to_ascii_uppercase())
        .collect();
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    let n = letters.iter().fold(0usize, |n, b| n * 26 + usize::from(b - b'A') + 1);
    (n <= MAX_COLUMNS).then(|| n - 1)
}

/// A stored number as a spreadsheet shows it: rounded to 15 significant digits, without a
/// trailing `.0`. Text that is not a number is kept as it is.
fn number(v: &str) -> String {
    match v.trim().parse::<f64>() {
        Ok(x) if x.is_finite() => {
            let rounded: f64 = format!("{x:.14e}").parse().unwrap_or(x);
            if rounded == 0.0 {
                "0".into()
            } else {
                format!("{rounded}")
            }
        }
        _ => v.to_owned(),
    }
}

#[derive(Default)]
struct Cell {
    column: usize,
    kind: String,
    value: String,
    inline: String,
}

impl Cell {
    fn text(self, strings: &[String]) -> String {
        match self.kind.as_str() {
            "s" => self
                .value
                .trim()
                .parse::<usize>()
                .ok()
                .and_then(|i| strings.get(i).cloned())
                .unwrap_or_default(),
            "inlineStr" => self.inline,
            "str" | "e" => self.value,
            "b" => match self.value.trim() {
                "1" => "TRUE".into(),
                "0" => "FALSE".into(),
                v => v.to_owned(),
            },
            _ => number(&self.value),
        }
    }
}

fn sheet_rows(xml: &str, strings: &[String]) -> Result<Vec<Vec<String>>> {
    let mut r = Reader::from_str(xml);
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut row: BTreeMap<usize, String> = BTreeMap::new();
    let mut cell: Option<Cell> = None;
    let mut next_column = 0;
    // Inside `<v>`, inside an inline string's `<t>`, inside a phonetic run.
    let (mut in_v, mut in_t, mut in_phonetic) = (false, false, false);
    let finish_row = |row: &mut BTreeMap<usize, String>, out: &mut Vec<Vec<String>>| -> Result<()> {
        if row.values().any(|v| !v.trim().is_empty()) {
            if out.len() >= MAX_ROWS {
                return Err(Error::Format(format!("xlsx: more than {MAX_ROWS} rows")));
            }
            let width = row.keys().next_back().map_or(0, |c| c + 1);
            let mut cells = vec![String::new(); width];
            for (c, v) in std::mem::take(row) {
                cells[c] = v;
            }
            out.push(cells);
        }
        row.clear();
        Ok(())
    };
    loop {
        match r.read_event().map_err(xml_error)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"row" => {
                    finish_row(&mut row, &mut out)?;
                    next_column = 0;
                }
                b"c" => {
                    cell = Some(start_cell(&e, &mut next_column)?);
                }
                b"v" => in_v = true,
                b"t" => in_t = true,
                b"rPh" => in_phonetic = true,
                _ => {}
            },
            Event::Empty(e) if e.local_name().as_ref() == b"c" => {
                start_cell(&e, &mut next_column)?;
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"row" => finish_row(&mut row, &mut out)?,
                b"c" => {
                    if let Some(c) = cell.take() {
                        let column = c.column;
                        let text = c.text(strings);
                        if !text.is_empty() {
                            row.insert(column, text);
                        }
                    }
                }
                b"v" => in_v = false,
                b"t" => in_t = false,
                b"rPh" => in_phonetic = false,
                _ => {}
            },
            Event::Text(t) => {
                if let Some(c) = cell.as_mut() {
                    if in_v {
                        c.value.push_str(&t.unescape().map_err(xml_error)?);
                    } else if in_t && !in_phonetic {
                        c.inline.push_str(&t.unescape().map_err(xml_error)?);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    finish_row(&mut row, &mut out)?;
    Ok(out)
}

fn start_cell(e: &BytesStart, next_column: &mut usize) -> Result<Cell> {
    let column = match attr(e, b"r") {
        Some(reference) => column_of(&reference)
            .ok_or_else(|| Error::Format(format!("xlsx: cell reference {reference:?} is not a column Excel has")))?,
        None => *next_column,
    };
    if column >= MAX_COLUMNS {
        return Err(Error::Format(format!("xlsx: more than {MAX_COLUMNS} columns")));
    }
    *next_column = column + 1;
    Ok(Cell {
        column,
        kind: attr(e, b"t").unwrap_or_default(),
        ..Cell::default()
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// A workbook built by hand the way spreadsheet programs write one: shared strings, a
    /// hidden first sheet, a rich text run and every cell type.
    fn workbook(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in parts {
            let _ = z.start_file(*name, zip::write::SimpleFileOptions::default());
            let _ = z.write_all(body.as_bytes());
        }
        z.finish().map(Cursor::into_inner).unwrap_or_default()
    }

    const RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
        <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
        <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="/xl/worksheets/data.xml"/>
        <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>
    </Relationships>"#;

    #[test]
    fn reads_the_first_visible_sheet_with_every_cell_type() -> Result<()> {
        let bytes = workbook(&[
            (
                "xl/workbook.xml",
                r#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>
                    <sheet name="Notes" sheetId="1" state="hidden" r:id="rId1"/>
                    <sheet name="Parts" sheetId="2" r:id="rId2"/></sheets></workbook>"#,
            ),
            ("xl/_rels/workbook.xml.rels", RELS),
            (
                "xl/sharedStrings.xml",
                r#"<sst><si><t>Order number</t></si><si><r><t>Manu</t></r><r><t xml:space="preserve">facturer</t></r><rPh><t>x</t></rPh></si><si><t>R&amp;S</t></si></sst>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>hidden</t></is></c></row></sheetData></worksheet>"#,
            ),
            (
                "xl/worksheets/data.xml",
                r#"<worksheet><sheetData>
                    <row r="1"><c r="A1" t="s"><v>0</v></c><c r="C1" t="s"><v>1</v></c></row>
                    <row r="2"><c r="A2"/></row>
                    <row r="4"><c r="A4"><v>1492</v></c><c r="B4" t="b"><v>1</v></c><c r="C4" t="s"><v>2</v></c></row>
                    <row r="5"><c r="A5"><v>0.30000000000000004</v></c><c r="B5" t="str"><f>A1&amp;"x"</f><v>calc</v></c><c t="inlineStr"><is><t> a b </t></is></c></row>
                </sheetData></worksheet>"#,
            ),
        ]);
        assert_eq!(
            rows(&bytes)?,
            vec![
                vec!["Order number".to_owned(), String::new(), "Manufacturer".into()],
                vec!["1492".into(), "TRUE".into(), "R&S".into()],
                vec!["0.3".into(), "calc".into(), " a b ".into()],
            ]
        );
        Ok(())
    }

    #[test]
    fn a_workbook_without_relationships_reads_sheet1() -> Result<()> {
        let bytes = workbook(&[
            ("xl/workbook.xml", "<workbook/>"),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><sheetData><row><c t="inlineStr"><is><t>a</t></is></c><c><v>2</v></c></row></sheetData></worksheet>"#,
            ),
        ]);
        assert_eq!(rows(&bytes)?, vec![vec!["a".to_owned(), "2".into()]]);
        Ok(())
    }

    #[test]
    fn refuses_what_is_not_a_workbook_and_columns_past_xfd() {
        assert!(rows(b"not a zip").is_err());
        assert!(rows(&workbook(&[("word/document.xml", "<w/>")])).is_err());
        let wide = workbook(&[
            ("xl/workbook.xml", "<workbook/>"),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><sheetData><row><c r="XFE1"><v>1</v></c></row></sheetData></worksheet>"#,
            ),
        ]);
        assert!(rows(&wide).is_err());
    }

    #[test]
    fn numbers_read_as_a_spreadsheet_shows_them() {
        assert_eq!(number("1492"), "1492");
        assert_eq!(number("1.0E-3"), "0.001");
        assert_eq!(number("4011209123456"), "4011209123456");
        assert_eq!(number("-0"), "0");
        assert_eq!(number("n/a"), "n/a");
        assert_eq!(column_of("A1"), Some(0));
        assert_eq!(column_of("XFD1048576"), Some(16_383));
        assert_eq!(column_of("12"), None);
    }

    #[test]
    fn a_report_workbook_reads_back_cell_for_cell() -> Result<()> {
        let t = wc_report::Table {
            title: "Parts".into(),
            columns: vec!["Order number".into(), "Description".into()],
            rows: vec![
                vec!["0042".into(), "=SUM(A1) & <b>".into()],
                vec!["1e3".into(), String::new()],
            ],
            ..wc_report::Table::default()
        };
        let bytes = t.to_xlsx().map_err(|e| Error::Format(e.to_string()))?;
        let mut expected = vec![t.columns.clone()];
        expected.push(t.rows[0].clone());
        expected.push(vec!["1e3".into()]);
        assert_eq!(rows(&bytes)?, expected);
        Ok(())
    }

    #[test]
    fn a_parts_list_workbook_imports_like_the_same_list_as_csv() -> Result<()> {
        let t = wc_report::Table {
            title: "Teileliste".into(),
            columns: vec![
                "Hersteller".into(),
                "Bestellnummer".into(),
                "Beschreibung".into(),
                "Colour".into(),
            ],
            rows: vec![
                vec![
                    "WAGO".into(),
                    "2002-1201".into(),
                    "Terminal, 2.5 mm²".into(),
                    "grey".into(),
                ],
                vec!["Phoenix".into(), String::new(), "no number".into(), String::new()],
            ],
            ..wc_report::Table::default()
        };
        let bytes = t.to_xlsx().map_err(|e| Error::Format(e.to_string()))?;
        let from_xlsx = crate::csv::parts_from_rows(&crate::csv::file_rows(&bytes)?)?;
        let from_csv = crate::csv::parts(&t.to_csv())?;
        assert_eq!(from_xlsx, from_csv);
        assert_eq!(from_xlsx.0.len(), 1);
        assert_eq!(from_xlsx.0[0].order_number, "2002-1201");
        assert_eq!(
            from_xlsx.0[0].properties.get("Colour").map(String::as_str),
            Some("grey")
        );
        assert_eq!(from_xlsx.1, ["row 3: no order number, skipped"]);
        assert_eq!(
            crate::csv::kind_of_rows(&crate::csv::file_rows(&bytes)?),
            Some(crate::csv::CsvKind::Parts)
        );
        Ok(())
    }

    #[test]
    fn a_plc_report_workbook_reads_back_as_addresses() -> Result<()> {
        let t = wc_report::Table {
            title: "PLC".into(),
            columns: vec!["Device".into(), "Address".into(), "Description".into()],
            rows: vec![vec!["-K1".into(), "%I0.0".into(), "Start".into()]],
            ..wc_report::Table::default()
        };
        let bytes = t.to_xlsx().map_err(|e| Error::Format(e.to_string()))?;
        let rows = crate::csv::file_rows(&bytes)?;
        assert_eq!(crate::csv::kind_of_rows(&rows), Some(crate::csv::CsvKind::PlcAddresses));
        let (addresses, warnings) = crate::csv::plc_addresses_from_rows(&rows)?;
        assert!(warnings.is_empty());
        assert_eq!(addresses.len(), 1);
        assert_eq!(addresses[0].address, "%I0.0");
        assert_eq!(addresses[0].description, "Start");
        Ok(())
    }

    #[test]
    fn an_old_xls_workbook_is_refused_with_a_hint() {
        let mut bytes = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1".to_vec();
        bytes.extend([0; 64]);
        let err = crate::csv::file_rows(&bytes)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(err.contains(".xlsx"), "{err}");
    }
}

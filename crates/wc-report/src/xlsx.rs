//! Reports as Excel workbooks (`.xlsx`, Office Open XML), one worksheet per report.
//!
//! Every cell is written as an inline text string, never as a number or a formula, so a value
//! such as `=SUM(A1)` or `0042` reads back exactly as the report holds it and a spreadsheet
//! program cannot run anything from it. The header row is bold and frozen.

use std::collections::BTreeSet;
use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;

use crate::Table;

/// Longest sheet name Excel accepts.
const MAX_SHEET_NAME: usize = 31;
/// Widest column written, in characters; longer cells wrap or overflow in the program.
const MAX_WIDTH: usize = 60;

/// A workbook with one worksheet per table, in order, named after each table's title.
pub fn workbook(tables: &[&Table]) -> std::io::Result<Vec<u8>> {
    let mut names = Vec::new();
    let mut taken = BTreeSet::new();
    for (i, t) in tables.iter().enumerate() {
        names.push(sheet_name(&t.title, i + 1, &mut taken));
    }
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut put = |path: &str, body: String| -> std::io::Result<()> {
        zip.start_file(path, opts).map_err(std::io::Error::other)?;
        zip.write_all(body.as_bytes())
    };
    put("[Content_Types].xml", content_types(tables.len()))?;
    put("_rels/.rels", ROOT_RELS.into())?;
    put("xl/workbook.xml", workbook_xml(&names))?;
    put("xl/_rels/workbook.xml.rels", workbook_rels(tables.len()))?;
    put("xl/styles.xml", STYLES.into())?;
    for (i, t) in tables.iter().enumerate() {
        put(&format!("xl/worksheets/sheet{}.xml", i + 1), sheet_xml(t))?;
    }
    let out = zip.finish().map_err(std::io::Error::other)?;
    Ok(out.into_inner())
}

impl Table {
    /// This table as a workbook with a single worksheet.
    pub fn to_xlsx(&self) -> std::io::Result<Vec<u8>> {
        workbook(&[self])
    }
}

/// A sheet name Excel accepts: no `[]:*?/\`, no leading or trailing apostrophe, at most 31
/// characters, not empty and not used by an earlier sheet (names compare without case).
fn sheet_name(title: &str, number: usize, taken: &mut BTreeSet<String>) -> String {
    let clean: String = title
        .chars()
        .map(|c| {
            if "[]:*?/\\".contains(c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim().trim_matches('\'').trim();
    let base = if clean.is_empty() {
        format!("Sheet{number}")
    } else {
        clean.chars().take(MAX_SHEET_NAME).collect()
    };
    let mut name = base.clone();
    let mut n = 2;
    while taken.contains(&name.to_lowercase()) {
        let suffix = format!(" ({n})");
        let keep = MAX_SHEET_NAME - suffix.chars().count();
        name = format!("{}{suffix}", base.chars().take(keep).collect::<String>().trim_end());
        n += 1;
    }
    taken.insert(name.to_lowercase());
    name
}

/// Text made safe for XML 1.0: markup characters escaped, characters XML cannot hold dropped.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// The column letters of a zero-based index: 0 is `A`, 25 is `Z`, 26 is `AA`.
fn column_letters(mut i: usize) -> String {
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (i % 26) as u8);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    out.reverse();
    String::from_utf8_lossy(&out).into_owned()
}

fn sheet_xml(t: &Table) -> String {
    let columns = t.rows.iter().map(Vec::len).chain([t.columns.len()]).max().unwrap_or(0);
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">",
    );
    if !t.columns.is_empty() {
        out.push_str(
            "<sheetViews><sheetView workbookViewId=\"0\"><pane ySplit=\"1\" topLeftCell=\"A2\" activePane=\"bottomLeft\" state=\"frozen\"/></sheetView></sheetViews>",
        );
    }
    if columns > 0 {
        out.push_str("<cols>");
        for c in 0..columns {
            let widest = std::iter::once(&t.columns)
                .chain(&t.rows)
                .filter_map(|r| r.get(c))
                .map(|s| s.lines().map(|l| l.chars().count()).max().unwrap_or(0))
                .max()
                .unwrap_or(0);
            let width = widest.clamp(4, MAX_WIDTH) + 2;
            out.push_str(&format!(
                "<col min=\"{n}\" max=\"{n}\" width=\"{width}\" customWidth=\"1\"/>",
                n = c + 1
            ));
        }
        out.push_str("</cols>");
    }
    out.push_str("<sheetData>");
    let header = (!t.columns.is_empty()).then_some((&t.columns, true));
    let body = t.rows.iter().map(|r| (r, false));
    for (r, (cells, bold)) in header.into_iter().chain(body).enumerate() {
        out.push_str(&format!("<row r=\"{}\">", r + 1));
        for (c, v) in cells.iter().enumerate() {
            if v.is_empty() {
                continue;
            }
            let style = if bold { " s=\"1\"" } else { "" };
            out.push_str(&format!(
                "<c r=\"{}{}\"{style} t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                column_letters(c),
                r + 1,
                esc(v)
            ));
        }
        out.push_str("</row>");
    }
    out.push_str("</sheetData></worksheet>");
    out
}

fn content_types(sheets: usize) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>",
    );
    for i in 1..=sheets {
        out.push_str(&format!(
            "<Override PartName=\"/xl/worksheets/sheet{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"
        ));
    }
    out.push_str("</Types>");
    out
}

const ROOT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>";

fn workbook_xml(names: &[String]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>",
    );
    for (i, n) in names.iter().enumerate() {
        out.push_str(&format!(
            "<sheet name=\"{}\" sheetId=\"{id}\" r:id=\"rId{id}\"/>",
            esc(n),
            id = i + 1
        ));
    }
    out.push_str("</sheets></workbook>");
    out
}

fn workbook_rels(sheets: usize) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );
    for i in 1..=sheets {
        out.push_str(&format!(
            "<Relationship Id=\"rId{i}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{i}.xml\"/>"
        ));
    }
    out.push_str(&format!(
        "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>",
        sheets + 1
    ));
    out
}

/// Two cell formats: 0 plain, 1 bold (the header row).
const STYLES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><fonts count=\"2\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font><font><b/><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts><fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill></fills><borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs><cellXfs count=\"2\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/><xf numFmtId=\"0\" fontId=\"1\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyFont=\"1\"/></cellXfs><cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles></styleSheet>";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_letters_run_past_z() {
        assert_eq!(column_letters(0), "A");
        assert_eq!(column_letters(25), "Z");
        assert_eq!(column_letters(26), "AA");
        assert_eq!(column_letters(51), "AZ");
        assert_eq!(column_letters(52), "BA");
        assert_eq!(column_letters(701), "ZZ");
        assert_eq!(column_letters(702), "AAA");
    }

    #[test]
    fn sheet_names_are_cleaned_shortened_and_unique() {
        let mut taken = BTreeSet::new();
        assert_eq!(sheet_name("Parts / BOM: [main]", 1, &mut taken), "Parts   BOM   main");
        assert_eq!(sheet_name("", 2, &mut taken), "Sheet2");
        assert_eq!(sheet_name("'quoted'", 3, &mut taken), "quoted");
        let long = "A very long report title that goes on";
        let first = sheet_name(long, 4, &mut taken);
        assert_eq!(first.chars().count(), 31);
        let second = sheet_name(long, 5, &mut taken);
        assert_eq!(second.chars().count(), 31);
        assert!(second.ends_with(" (2)"), "{second}");
        assert_eq!(sheet_name("SHEET2", 6, &mut taken), "SHEET2 (2)");
    }

    #[test]
    fn cells_are_inline_text_escaped_and_without_invalid_characters() {
        let t = Table {
            title: "Parts".into(),
            columns: vec!["Order number".into(), "Text".into()],
            rows: vec![vec!["=SUM(A1)".into(), "a<b & \"c\"\u{1}\u{7}".into()]],
            ..Table::default()
        };
        let xml = sheet_xml(&t);
        assert!(xml.contains("<c r=\"A1\" s=\"1\" t=\"inlineStr\"><is><t xml:space=\"preserve\">Order number</t>"));
        assert!(xml.contains("<c r=\"A2\" t=\"inlineStr\"><is><t xml:space=\"preserve\">=SUM(A1)</t>"));
        assert!(xml.contains("a&lt;b &amp; &quot;c&quot;</t>"), "{xml}");
        assert!(!xml.contains("<f>"));
        assert!(!xml.contains('\u{1}'));
    }

    #[test]
    fn a_workbook_holds_every_part_a_reader_needs() -> std::io::Result<()> {
        let t = Table {
            title: "Wires".into(),
            columns: vec!["From".into()],
            rows: vec![vec!["-K1:13".into()]],
            ..Table::default()
        };
        let bytes = workbook(&[&t, &t])?;
        let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(std::io::Error::other)?;
        let mut names: Vec<String> = z.file_names().map(str::to_owned).collect();
        names.sort();
        assert_eq!(
            names,
            [
                "[Content_Types].xml",
                "_rels/.rels",
                "xl/_rels/workbook.xml.rels",
                "xl/styles.xml",
                "xl/workbook.xml",
                "xl/worksheets/sheet1.xml",
                "xl/worksheets/sheet2.xml",
            ]
        );
        let mut wb = String::new();
        std::io::Read::read_to_string(
            &mut z.by_name("xl/workbook.xml").map_err(std::io::Error::other)?,
            &mut wb,
        )?;
        assert!(wb.contains("name=\"Wires\""));
        assert!(wb.contains("name=\"Wires (2)\""));
        Ok(())
    }
}

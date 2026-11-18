//! Lists from Excel workbooks (`.xlsx`, `.xlsm`): the first visible worksheet read as rows of
//! text, the same shape [`crate::csv::records`] gives for a CSV file, so parts lists and PLC
//! address lists are read with the same column names either way.
//!
//! Cells keep the text the workbook stores: shared and inline strings as written, numbers in
//! their shortest form rounded to the 15 digits a spreadsheet program shows (`1492`, `0.3`),
//! booleans as `TRUE`/`FALSE`, and a formula as its last calculated value. Two kinds of number
//! format are applied: dates and times read as ISO text (`2024-11-18`,
//! `2024-11-18 14:30`, `14:30:05`), and a format of only zeros (`00000`) pads the number with
//! leading zeros the way the workbook shows it (`00042`). Other formats (currency, thousands
//! separators, percentages, fractions) are not applied. The old binary `.xls` format is not read.

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
/// Most cells read in all, the empty ones a row skips included: a row with one cell in column
/// `XFD` holds 16,384, so this bounds memory, not only the row count.
const MAX_CELLS: usize = 20_000_000;
/// Most text kept in all: one long shared string referenced from every row is copied each time.
const MAX_TEXT_BYTES: usize = 256 * 1024 * 1024;

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
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(xml_error)?;
    let workbook = part(&mut zip, "xl/workbook.xml")?
        .ok_or_else(|| Error::Format("xlsx: no xl/workbook.xml, not an Excel workbook".into()))?;
    let rels = part(&mut zip, "xl/_rels/workbook.xml.rels")?.unwrap_or_default();
    let rels = relationships(&rels)?;
    let sheet_path = match first_worksheet(&sheets(&workbook)?, &rels)? {
        Some(r) => target_path(&r.target),
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
    let styles_path = rels
        .iter()
        .find(|r| r.kind.ends_with("/styles"))
        .map(|r| target_path(&r.target))
        .unwrap_or_else(|| "xl/styles.xml".into());
    let mut styles = match part(&mut zip, &styles_path)? {
        Some(xml) => styles(&xml)?,
        None => Styles::default(),
    };
    styles.date1904 = date1904(&workbook)?;
    let sheet = part(&mut zip, &sheet_path)?
        .ok_or_else(|| Error::Format(format!("xlsx: the worksheet {sheet_path:?} is missing")))?;
    sheet_rows_styled(&sheet, &strings, &styles, MAX_TEXT_BYTES)
}

/// A part of the package as text, `None` when the package has no such part.
fn part(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Option<String>> {
    let f = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => {
            return Err(Error::Format(format!(
                "xlsx: {name:?}: {}",
                crate::printable(&e.to_string())
            )));
        }
    };
    let mut data = Vec::new();
    f.take(MAX_PART + 1)
        .read_to_end(&mut data)
        .map_err(|e| Error::Format(format!("xlsx: {name:?}: {}", crate::printable(&e.to_string()))))?;
    if data.len() as u64 > MAX_PART {
        return Err(Error::Format(format!(
            "xlsx: {name:?} is larger than {} MB",
            MAX_PART / 1024 / 1024
        )));
    }
    decode(&data)
        .map(Some)
        .map_err(|e| Error::Format(format!("xlsx: {name:?}: {e}")))
}

/// An XML part as text: UTF-16 when it starts with a byte order mark (or with `<` as UTF-16),
/// otherwise UTF-8 with any byte order mark left out. Bytes that are not valid in that encoding
/// are an error, never replaced, so no cell text changes silently.
fn decode(data: &[u8]) -> std::result::Result<String, String> {
    let utf16 = |bytes: &[u8], big: bool| {
        let (units, rest) = bytes.as_chunks::<2>();
        if !rest.is_empty() {
            return Err("UTF-16 text ends in half a code unit".to_owned());
        }
        let units = units.iter().map(|&p| {
            if big {
                u16::from_be_bytes(p)
            } else {
                u16::from_le_bytes(p)
            }
        });
        char::decode_utf16(units)
            .collect::<std::result::Result<String, _>>()
            .map_err(|e| format!("invalid UTF-16: {e}"))
    };
    let utf8 = |bytes: &[u8]| {
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|e| format!("invalid UTF-8: {e}"))
    };
    match data {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, false),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, true),
        [b'<', 0, ..] => utf16(data, false),
        [0, b'<', ..] => utf16(data, true),
        [0xEF, 0xBB, 0xBF, rest @ ..] => utf8(rest),
        _ => utf8(data),
    }
}

/// A relationship target as a package path: relative to `xl/` unless it starts with `/`, with
/// `.` and `..` segments resolved (`./worksheets/a.xml` is `xl/worksheets/a.xml`).
fn target_path(target: &str) -> String {
    let joined = match target.strip_prefix('/') {
        Some(abs) => abs.to_owned(),
        None => format!("xl/{target}"),
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn xml_error(e: impl std::fmt::Display) -> Error {
    // Parser and zip messages can quote the file's own bytes: printed without control characters.
    Error::Format(format!("xlsx: {}", crate::printable(&e.to_string())))
}

/// An attribute's value, `None` when the element has no such attribute. A malformed attribute
/// list or value is an error with the parser's reason.
fn attr(e: &BytesStart, name: &[u8]) -> Result<Option<String>> {
    for a in e.attributes() {
        let a = a.map_err(xml_error)?;
        if a.key.local_name().as_ref() == name {
            return a.unescape_value().map(|v| Some(v.into_owned())).map_err(xml_error);
        }
    }
    Ok(None)
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
                    id: attr(&e, b"Id")?.unwrap_or_default(),
                    kind: attr(&e, b"Type")?.unwrap_or_default(),
                    target: attr(&e, b"Target")?.unwrap_or_default(),
                });
            }
            Event::Eof => return Ok(out),
            _ => {}
        }
    }
}

/// The workbook's sheets in order: each one's relationship id and whether it is hidden.
fn sheets(xml: &str) -> Result<Vec<(String, bool)>> {
    let mut r = Reader::from_str(xml);
    let mut out = Vec::new();
    loop {
        match r.read_event().map_err(xml_error)? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"sheet" => {
                if let Some(id) = attr(&e, b"id")? {
                    let hidden = attr(&e, b"state")?.is_some_and(|s| s != "visible");
                    out.push((id, hidden));
                }
            }
            Event::Eof => return Ok(out),
            _ => {}
        }
    }
}

/// The relationship of the first worksheet that is not hidden (or of the first worksheet when
/// all are hidden). Chartsheets and dialog sheets hold no cells and are passed over; a sheet
/// whose relationship is missing is an error. `None` when the workbook lists no sheets.
fn first_worksheet<'a>(sheets: &[(String, bool)], rels: &'a [Relationship]) -> Result<Option<&'a Relationship>> {
    if sheets.is_empty() {
        return Ok(None);
    }
    // Indexed once: a workbook can list very many sheets.
    let by_id: std::collections::HashMap<&str, &Relationship> = rels.iter().map(|r| (r.id.as_str(), r)).collect();
    let in_order = sheets
        .iter()
        .filter(|(_, hidden)| !hidden)
        .chain(sheets.iter().filter(|(_, hidden)| *hidden));
    for (id, _) in in_order {
        let r = by_id.get(id.as_str()).ok_or_else(|| {
            Error::Format(format!(
                "xlsx: the sheet relationship {:?} is missing",
                crate::printable(id)
            ))
        })?;
        if r.kind.ends_with("/worksheet") {
            return Ok(Some(r));
        }
    }
    Err(Error::Format(
        "xlsx: the workbook has no worksheet, only chart or dialog sheets".into(),
    ))
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
                b"si" => out.push(unescape_xstring(&std::mem::take(&mut current))),
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

/// Text with SpreadsheetML's `_xHHHH_` escapes turned back into their characters: `_x000D_` is
/// a carriage return and `_x005F_` a literal underscore, so `_x005F_x0041_` reads `_x0041_`.
/// An escape for a code that is no character is left as written.
fn unescape_xstring(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("_x") {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let decoded = tail
            .get(2..6)
            .filter(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .filter(|_| tail.as_bytes().get(6) == Some(&b'_'))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .and_then(char::from_u32);
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[7..];
            }
            None => {
                out.push_str("_x");
                rest = &tail[2..];
            }
        }
    }
    out.push_str(rest);
    out
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

/// How a number cell is shown, from the number format of its style. Formats this reader does
/// not apply are [`Shown::Plain`]: the number in its shortest form.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Shown {
    #[default]
    Plain,
    /// A date without a time of day, as `YYYY-MM-DD`.
    Date,
    /// A date and a time of day, as `YYYY-MM-DD HH:MM`, with `:SS` when the format shows seconds.
    DateTime { seconds: bool },
    /// A time of day, as `HH:MM`, with `:SS` when the format shows seconds.
    Time { seconds: bool },
    /// A whole number padded with leading zeros to this many digits (format `00000`).
    Padded(usize),
}

/// The number formats of a workbook's cell styles (`cellXfs`, by the index a cell's `s` names)
/// and whether its dates count from 1904 instead of 1900.
#[derive(Default)]
struct Styles {
    by_xf: Vec<Shown>,
    date1904: bool,
}

impl Styles {
    /// The number format of the cell style `xf`; a cell without a style has style 0.
    fn shown(&self, xf: Option<usize>) -> Shown {
        self.by_xf.get(xf.unwrap_or(0)).copied().unwrap_or_default()
    }
}

/// The built-in number formats that are dates or times. The ones that depend on the reader's
/// language (27 to 36, 50 to 58) and the elapsed and minute-second times (45 to 47) are left
/// as numbers.
fn builtin_format(id: u32) -> Shown {
    match id {
        14..=17 => Shown::Date,
        18 | 20 => Shown::Time { seconds: false },
        19 | 21 => Shown::Time { seconds: true },
        22 => Shown::DateTime { seconds: false },
        _ => Shown::Plain,
    }
}

/// Longest zero-only format padded: Excel's own limit on a format code's length.
const MAX_PADDING: usize = 255;

/// What a custom format code shows, from its first section (the one for positive numbers).
/// Quoted text, escaped characters, spacing and fill characters and bracketed colours and
/// locales are passed over. A format with a condition (`[>=1000]…`) picks its section by the
/// value, which is not followed here: it stays a number, like an elapsed time such as `[h]:mm`.
/// `m` is a month unless the section also shows hours or seconds, as Excel reads it.
fn custom_format(code: &str) -> Shown {
    let mut section = String::new();
    // The section's own characters, lower case, without quoted or escaped text.
    let mut bare = String::new();
    let mut sections = 1;
    let mut chars = code.chars();
    while let Some(c) = chars.next() {
        match c {
            ';' => {
                sections += 1;
                // Later sections only matter for a condition or a zero-only format, both
                // decided below from the count and the first section.
                for c in chars.by_ref() {
                    if c == ';' {
                        sections += 1;
                    }
                }
                break;
            }
            '"' => {
                section.push(c);
                for q in chars.by_ref() {
                    section.push(q);
                    if q == '"' {
                        break;
                    }
                }
            }
            '\\' | '_' | '*' => {
                section.push(c);
                section.extend(chars.next());
            }
            '[' => {
                let inner: String = chars.by_ref().take_while(|&b| b != ']').collect();
                let lower = inner.to_ascii_lowercase();
                let elapsed = !lower.is_empty() && lower.chars().all(|b| matches!(b, 'h' | 'm' | 's'));
                let condition = lower.starts_with(['<', '>', '=']);
                if elapsed || condition {
                    return Shown::Plain;
                }
                section.push('[');
                section.push_str(&inner);
                section.push(']');
            }
            c => {
                section.push(c);
                bare.push(c.to_ascii_lowercase());
            }
        }
    }
    let bare = bare.replace("am/pm", "").replace("a/p", "");
    let time = bare.contains(['h', 's']);
    let date = bare.contains(['y', 'd']) || (!time && bare.contains('m'));
    let seconds = bare.contains('s');
    match (date, time) {
        (true, true) => Shown::DateTime { seconds },
        (true, false) => Shown::Date,
        (false, true) => Shown::Time { seconds },
        _ if sections == 1 && (1..=MAX_PADDING).contains(&section.len()) && section.chars().all(|c| c == '0') => {
            Shown::Padded(section.len())
        }
        _ => Shown::Plain,
    }
}

/// A whole-number attribute such as a style or format id: `None` when the element has none,
/// an error naming the attribute and the parser's reason when it is not a number.
fn index_attr<T: std::str::FromStr<Err = std::num::ParseIntError>>(e: &BytesStart, name: &[u8]) -> Result<Option<T>> {
    let Some(v) = attr(e, name)? else { return Ok(None) };
    v.trim().parse::<T>().map(Some).map_err(|err| {
        Error::Format(format!(
            "xlsx: {} {:?} is not a number: {err}",
            String::from_utf8_lossy(name),
            crate::printable(&v)
        ))
    })
}

/// The number format of each cell style in `xl/styles.xml`. Custom codes are read from the
/// style sheet's `numFmts` only: differential formats (`dxfs`, for conditional formatting)
/// can reuse an id for another code that cell styles never mean.
fn styles(xml: &str) -> Result<Styles> {
    let mut r = Reader::from_str(xml);
    let mut custom: std::collections::HashMap<u32, Shown> = std::collections::HashMap::new();
    let mut xf_formats: Vec<Option<u32>> = Vec::new();
    let (mut in_num_fmts, mut in_cell_xfs) = (false, false);
    loop {
        match r.read_event().map_err(xml_error)? {
            Event::Start(e) if e.local_name().as_ref() == b"numFmts" => in_num_fmts = true,
            Event::End(e) if e.local_name().as_ref() == b"numFmts" => in_num_fmts = false,
            Event::Start(e) | Event::Empty(e) if in_num_fmts && e.local_name().as_ref() == b"numFmt" => {
                if let (Some(id), Some(code)) = (index_attr::<u32>(&e, b"numFmtId")?, attr(&e, b"formatCode")?) {
                    custom.insert(id, custom_format(&code));
                }
            }
            Event::Start(e) if e.local_name().as_ref() == b"cellXfs" => in_cell_xfs = true,
            Event::End(e) if e.local_name().as_ref() == b"cellXfs" => in_cell_xfs = false,
            Event::Start(e) | Event::Empty(e) if in_cell_xfs && e.local_name().as_ref() == b"xf" => {
                if xf_formats.len() >= MAX_COLUMNS * 4 {
                    // Excel allows about 64,000 cell styles; the rest are read as plain numbers.
                    continue;
                }
                xf_formats.push(index_attr::<u32>(&e, b"numFmtId")?);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let by_xf = xf_formats
        .into_iter()
        .map(|id| match id {
            // A custom format can reuse a built-in id; the workbook's own code wins.
            Some(id) => custom.get(&id).copied().unwrap_or_else(|| builtin_format(id)),
            None => Shown::Plain,
        })
        .collect();
    Ok(Styles { by_xf, date1904: false })
}

/// Whether the workbook counts dates from 1904 (`<workbookPr date1904="1"/>`).
fn date1904(workbook: &str) -> Result<bool> {
    let mut r = Reader::from_str(workbook);
    loop {
        match r.read_event().map_err(xml_error)? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"workbookPr" => {
                return Ok(attr(&e, b"date1904")?.is_some_and(|v| matches!(v.trim(), "1" | "true")));
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
    }
}

/// The year, month and day of a count of days since 1970-01-01 (proleptic Gregorian).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The largest serial number Excel shows as a date: 9999-12-31.
const MAX_SERIAL: f64 = 2_958_465.0;

/// A stored number as its number format shows it; formats this reader does not apply, and
/// numbers the format cannot show (a negative date, a fraction with zero padding), read as
/// [`number`] gives them.
fn formatted(v: &str, shown: Shown, date1904: bool) -> String {
    if shown == Shown::Plain {
        return number(v);
    }
    let x = match v.trim().parse::<f64>() {
        Ok(x) if x.is_finite() => x,
        _ => return number(v),
    };
    let date_and_time = |seconds: bool, with_date: bool, with_time: bool| -> Option<String> {
        if !(0.0..=MAX_SERIAL + 1.0).contains(&x) {
            return None;
        }
        let mut days = x.floor() as i64;
        // Excel rounds a time to the nearest second before showing it; a date without a time
        // shows its own day even when the time would round up to midnight.
        let mut secs = ((x - x.floor()) * 86_400.0).round() as i64;
        if secs >= 86_400 && with_time {
            days += 1;
            secs -= 86_400;
        }
        let mut text = String::new();
        if with_date {
            let ymd = if date1904 {
                Some(civil_from_days(days - 24_107))
            } else if days < 1 {
                // Serial 0 is "1900-01-00", which is no date.
                None
            } else if days == 60 {
                // Excel keeps Lotus 1-2-3's leap day that 1900 did not have.
                Some((1900, 2, 29))
            } else if days < 60 {
                Some(civil_from_days(days - 25_568))
            } else {
                Some(civil_from_days(days - 25_569))
            };
            let (y, m, d) = ymd?;
            if y > 9999 {
                return None;
            }
            text.push_str(&format!("{y:04}-{m:02}-{d:02}"));
        }
        if with_time {
            if with_date {
                text.push(' ');
            }
            text.push_str(&format!("{:02}:{:02}", secs / 3600, secs / 60 % 60));
            if seconds {
                text.push_str(&format!(":{:02}", secs % 60));
            }
        }
        Some(text)
    };
    let shown = match shown {
        Shown::Plain => None,
        Shown::Date => date_and_time(false, true, false),
        Shown::DateTime { seconds } => date_and_time(seconds, true, true),
        Shown::Time { seconds } => date_and_time(seconds, false, true),
        // Whole numbers up to 2^53 convert to an integer without losing a digit.
        Shown::Padded(width) => (x.fract() == 0.0 && x.abs() <= 9_007_199_254_740_992.0).then(|| {
            let sign = if x < 0.0 { "-" } else { "" };
            format!("{sign}{:0width$}", x.abs() as u64)
        }),
    };
    shown.unwrap_or_else(|| number(v))
}

#[derive(Default)]
struct Cell {
    /// The cell's place for messages, such as `B7` or `column C`.
    reference: String,
    column: usize,
    kind: String,
    /// The index of the cell's style in `cellXfs`, when it names one.
    style: Option<usize>,
    value: String,
    inline: String,
}

impl Cell {
    fn text(self, strings: &[String], styles: &Styles) -> Result<String> {
        Ok(match self.kind.as_str() {
            "s" => {
                let index = self.value.trim();
                index
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| strings.get(i).cloned())
                    .ok_or_else(|| {
                        Error::Format(format!(
                            "xlsx: cell {} refers to shared string {:?}, but the workbook has {} shared strings",
                            self.reference,
                            crate::printable(index),
                            strings.len()
                        ))
                    })?
            }
            "inlineStr" => unescape_xstring(&self.inline),
            // A formula's cached text is escaped like any other string.
            "str" => unescape_xstring(&self.value),
            "e" => self.value,
            "b" => match self.value.trim() {
                "1" => "TRUE".into(),
                "0" => "FALSE".into(),
                v => v.to_owned(),
            },
            "" | "n" => formatted(&self.value, styles.shown(self.style), styles.date1904),
            _ => number(&self.value),
        })
    }
}

#[cfg(test)]
fn sheet_rows(xml: &str, strings: &[String]) -> Result<Vec<Vec<String>>> {
    sheet_rows_styled(xml, strings, &Styles::default(), MAX_TEXT_BYTES)
}

#[cfg(test)]
fn sheet_rows_within(xml: &str, strings: &[String], max_text: usize) -> Result<Vec<Vec<String>>> {
    sheet_rows_styled(xml, strings, &Styles::default(), max_text)
}

/// A worksheet's rows with its numbers shown through `styles`, keeping at most `max_text` bytes
/// of cell text.
fn sheet_rows_styled(xml: &str, strings: &[String], styles: &Styles, max_text: usize) -> Result<Vec<Vec<String>>> {
    let mut text_bytes = 0usize;
    let mut r = Reader::from_str(xml);
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut row: BTreeMap<usize, String> = BTreeMap::new();
    let mut cell: Option<Cell> = None;
    let mut next_column = 0;
    // Inside `<v>`, inside an inline string's `<t>`, inside a phonetic run.
    let (mut in_v, mut in_t, mut in_phonetic) = (false, false, false);
    let mut cells_read = 0usize;
    let mut finish_row = |row: &mut BTreeMap<usize, String>, out: &mut Vec<Vec<String>>| -> Result<()> {
        if row.values().any(|v| !v.trim().is_empty()) {
            if out.len() >= MAX_ROWS {
                return Err(Error::Format(format!("xlsx: more than {MAX_ROWS} rows")));
            }
            let width = row.keys().next_back().map_or(0, |c| c + 1);
            cells_read += width;
            if cells_read > MAX_CELLS {
                return Err(Error::Format(format!("xlsx: more than {MAX_CELLS} cells")));
            }
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
                        let text = c.text(strings, styles)?;
                        text_bytes = text_bytes.saturating_add(text.len());
                        if text_bytes > max_text {
                            return Err(Error::Format(format!(
                                "xlsx: more than {} MB of cell text",
                                max_text / 1024 / 1024
                            )));
                        }
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
            // CDATA is text taken as written, without entity unescaping.
            Event::CData(t) => {
                if let Some(c) = cell.as_mut() {
                    if in_v {
                        c.value.push_str(&String::from_utf8_lossy(&t));
                    } else if in_t && !in_phonetic {
                        c.inline.push_str(&String::from_utf8_lossy(&t));
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
    let given = attr(e, b"r")?;
    let column = match &given {
        Some(reference) => column_of(reference).ok_or_else(|| {
            Error::Format(format!(
                "xlsx: cell reference {:?} is not a column Excel has",
                crate::printable(reference)
            ))
        })?,
        None => *next_column,
    };
    if column >= MAX_COLUMNS {
        return Err(Error::Format(format!("xlsx: more than {MAX_COLUMNS} columns")));
    }
    *next_column = column + 1;
    let reference = match given {
        Some(r) => format!("{:?}", crate::printable(&r)),
        None => format!("in column {}", column + 1),
    };
    Ok(Cell {
        reference,
        column,
        kind: attr(e, b"t")?.unwrap_or_default(),
        style: index_attr::<usize>(e, b"s")?,
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
        let parts: Vec<(&str, Vec<u8>)> = parts.iter().map(|(n, b)| (*n, b.as_bytes().to_vec())).collect();
        workbook_bytes(&parts)
    }

    fn workbook_bytes(parts: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in parts {
            let _ = z.start_file(*name, zip::write::SimpleFileOptions::default());
            let _ = z.write_all(body);
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
                vec!["0042".into(), "=SUM(A1) & <b>\r\n_x000D_\u{c}\u{1}".into()],
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

    #[test]
    fn spreadsheet_escapes_read_back_as_their_characters() {
        assert_eq!(unescape_xstring("a_x000D_b"), "a\rb");
        assert_eq!(unescape_xstring("part_x005F_x0041_"), "part_x0041_");
        assert_eq!(unescape_xstring("_x00_ _xZZZZ_ _x0041"), "_x00_ _xZZZZ_ _x0041");
        assert_eq!(unescape_xstring("plain_name"), "plain_name");
    }

    #[test]
    fn a_wide_sheet_stops_at_the_cell_budget() {
        let mut xml = String::from("<worksheet><sheetData>");
        for r in 1..=(MAX_CELLS / MAX_COLUMNS + 2) {
            xml.push_str(&format!(
                "<row><c r=\"XFD{r}\" t=\"inlineStr\"><is><t>x</t></is></c></row>"
            ));
        }
        xml.push_str("</sheetData></worksheet>");
        let err = sheet_rows(&xml, &[]).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("cells"), "{err}");
    }

    #[test]
    fn a_path_from_the_file_is_quoted_in_errors() {
        let bytes = workbook(&[
            (
                "xl/workbook.xml",
                r#"<workbook xmlns:r="r"><sheets><sheet name="a" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                "<Relationships><Relationship Id=\"rId1\" Type=\"x/worksheet\" Target=\"a&#27;]0;title&#7;.xml\"/></Relationships>",
            ),
        ]);
        let err = rows(&bytes).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(!err.contains('\u{1b}') && !err.contains('\u{7}'), "{err:?}");
        assert!(err.contains("\\u{1b}"), "{err:?}");
    }

    #[test]
    fn a_shared_string_repeated_on_every_row_stops_at_the_text_budget() {
        let long = "x".repeat(1024 * 1024);
        let mut xml = String::from("<worksheet><sheetData>");
        for _ in 0..3 {
            xml.push_str(r#"<row><c t="s"><v>0</v></c></row>"#);
        }
        xml.push_str("</sheetData></worksheet>");
        let strings = [long];
        assert!(sheet_rows_within(&xml, &strings, 3 * 1024 * 1024).is_ok());
        let err = sheet_rows_within(&xml, &strings, 2 * 1024 * 1024 + 1)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(err.contains("cell text"), "{err}");
    }

    #[test]
    fn a_cached_formula_string_is_unescaped() -> Result<()> {
        let xml = r#"<worksheet><sheetData><row><c t="str"><f>A1</f><v>part_x005F_x0041_</v></c><c t="str"><v>plain</v></c></row></sheetData></worksheet>"#;
        assert_eq!(
            sheet_rows(xml, &[])?,
            vec![vec!["part_x0041_".to_owned(), "plain".into()]]
        );
        Ok(())
    }

    /// A workbook whose one sheet, `rId1`, has the given relationship target and worksheet.
    fn one_sheet(target: &str, sheet_path: &str, sheet: &str) -> Vec<u8> {
        let rels = format!(
            r#"<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="{target}"/></Relationships>"#
        );
        workbook(&[
            (
                "xl/workbook.xml",
                r#"<workbook><sheets><sheet name="A" r:id="rId1"/></sheets></workbook>"#,
            ),
            ("xl/_rels/workbook.xml.rels", &rels),
            (sheet_path, sheet),
        ])
    }

    const ONE_CELL: &str =
        r#"<worksheet><sheetData><row><c t="inlineStr"><is><t>x</t></is></c></row></sheetData></worksheet>"#;

    #[test]
    fn relationship_targets_with_dot_segments_resolve() -> Result<()> {
        let plain = rows(&one_sheet("worksheets/data.xml", "xl/worksheets/data.xml", ONE_CELL))?;
        assert_eq!(plain, vec![vec!["x".to_owned()]]);
        assert_eq!(
            rows(&one_sheet("./worksheets/data.xml", "xl/worksheets/data.xml", ONE_CELL))?,
            plain
        );
        assert_eq!(
            rows(&one_sheet(
                "../xl/worksheets/./data.xml",
                "xl/worksheets/data.xml",
                ONE_CELL
            ))?,
            plain
        );
        assert_eq!(target_path("/xl/a/../b.xml"), "xl/b.xml");
        Ok(())
    }

    #[test]
    fn utf16_parts_read_like_utf8() -> Result<()> {
        let xml = r#"<?xml version="1.0" encoding="UTF-16"?><worksheet><sheetData><row><c t="inlineStr"><is><t>Ø x</t></is></c></row></sheetData></worksheet>"#;
        let utf8 = sheet_rows(&decode(xml.as_bytes()).map_err(Error::Format)?, &[])?;
        let mut le = vec![0xFF, 0xFE];
        le.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
        let mut be = vec![0xFE, 0xFF];
        be.extend(xml.encode_utf16().flat_map(u16::to_be_bytes));
        let no_bom: Vec<u8> = xml.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut bom8 = vec![0xEF, 0xBB, 0xBF];
        bom8.extend(xml.as_bytes());
        assert_eq!(utf8, vec![vec!["Ø x".to_owned()]]);
        for bytes in [le, be, no_bom, bom8] {
            assert_eq!(sheet_rows(&decode(&bytes).map_err(Error::Format)?, &[])?, utf8);
        }
        Ok(())
    }

    #[test]
    fn a_utf16_workbook_imports_like_the_same_workbook_in_utf8() -> Result<()> {
        let parts = [
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0" encoding="UTF-16"?><workbook><sheets><sheet name="Parts" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0" encoding="UTF-16"?><Relationships>
                    <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
                    <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>
                </Relationships>"#,
            ),
            (
                "xl/sharedStrings.xml",
                r#"<?xml version="1.0" encoding="UTF-16"?><sst><si><t>Order number</t></si><si><t>Kontaktør Ø</t></si></sst>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0" encoding="UTF-16"?><worksheet><sheetData><row><c t="s"><v>0</v></c></row><row><c t="s"><v>1</v></c><c t="inlineStr"><is><t>µ</t></is></c></row></sheetData></worksheet>"#,
            ),
        ];
        // The UTF-8 copy declares its own encoding.
        let utf8: Vec<(&str, Vec<u8>)> = parts
            .iter()
            .map(|(n, b)| {
                (
                    *n,
                    b.replace(r#"encoding="UTF-16""#, r#"encoding="UTF-8""#).into_bytes(),
                )
            })
            .collect();
        let utf16: Vec<(&str, Vec<u8>)> = parts
            .iter()
            .map(|(n, b)| {
                let mut bytes = vec![0xFF, 0xFE];
                bytes.extend(b.encode_utf16().flat_map(u16::to_le_bytes));
                (*n, bytes)
            })
            .collect();
        let expected = vec![
            vec!["Order number".to_owned()],
            vec!["Kontaktør Ø".to_owned(), "µ".into()],
        ];
        assert!(utf8.iter().all(|(_, b)| !String::from_utf8_lossy(b).contains("UTF-16")));
        assert_eq!(rows(&workbook_bytes(&utf8))?, expected);
        assert_eq!(rows(&workbook_bytes(&utf16))?, expected);
        Ok(())
    }

    #[test]
    fn malformed_text_encodings_are_errors() {
        // An unpaired surrogate, half a code unit and a broken UTF-8 sequence.
        let mut lone = vec![0xFF, 0xFE, b'<', 0];
        lone.extend(0xD800u16.to_le_bytes());
        let half = vec![0xFF, 0xFE, b'<', 0, b'a'];
        let bad8 = b"<a>\xC3</a>".to_vec();
        for bytes in [lone, half, bad8] {
            assert!(decode(&bytes).is_err(), "{bytes:?}");
        }
    }

    #[test]
    fn a_malformed_cell_reference_is_refused_with_its_reason() {
        let xml = r#"<worksheet><sheetData><row><c r="&missing;"><v>1</v></c></row></sheetData></worksheet>"#;
        let err = sheet_rows(xml, &[]).map(|_| ()).map_err(|e| e.to_string());
        assert!(err.as_ref().is_err_and(|e| e.contains("missing")), "{err:?}");
        // A cell without `r` still takes the next column.
        let implicit = r#"<worksheet><sheetData><row><c><v>1</v></c><c><v>2</v></c></row></sheetData></worksheet>"#;
        assert_eq!(
            sheet_rows(implicit, &[]).ok(),
            Some(vec![vec!["1".to_owned(), "2".into()]])
        );
    }

    #[test]
    fn a_chartsheet_before_the_data_is_passed_over() -> Result<()> {
        let bytes = workbook(&[
            (
                "xl/workbook.xml",
                r#"<workbook><sheets><sheet name="Chart" r:id="rId1"/><sheet name="Parts" r:id="rId2"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships>
                    <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chartsheet" Target="chartsheets/sheet1.xml"/>
                    <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
                </Relationships>"#,
            ),
            ("xl/chartsheets/sheet1.xml", "<chartsheet/>"),
            ("xl/worksheets/sheet1.xml", ONE_CELL),
        ]);
        assert_eq!(rows(&bytes)?, vec![vec!["x".to_owned()]]);
        Ok(())
    }

    #[test]
    fn a_sheet_with_a_missing_relationship_is_an_error() {
        let bytes = workbook(&[
            (
                "xl/workbook.xml",
                r#"<workbook><sheets><sheet name="A" r:id="rId9"/><sheet name="B" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            ("xl/worksheets/sheet1.xml", ONE_CELL),
        ]);
        let err = rows(&bytes).map(|_| ()).map_err(|e| e.to_string());
        assert!(err.as_ref().is_err_and(|e| e.contains("rId9")), "{err:?}");
    }

    #[test]
    fn an_unresolved_shared_string_is_an_error() {
        let strings = vec!["Order number".to_owned(), String::new()];
        let ok = r#"<worksheet><sheetData><row><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row></sheetData></worksheet>"#;
        assert_eq!(
            sheet_rows(ok, &strings).ok(),
            Some(vec![vec!["Order number".to_owned()]])
        );
        let bad = r#"<worksheet><sheetData><row><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>99</v></c></row></sheetData></worksheet>"#;
        let err = sheet_rows(bad, &strings).map(|_| ()).map_err(|e| e.to_string());
        assert!(
            err.as_ref().is_err_and(|e| e.contains("B1") && e.contains("99")),
            "{err:?}"
        );
    }

    #[test]
    fn cdata_in_cells_is_kept_as_written() -> Result<()> {
        let xml = r#"<worksheet><sheetData><row><c t="inlineStr"><is><t>a<![CDATA[<b>&amp;]]>c</t></is></c><c t="str"><f>A1</f><v><![CDATA[x&y]]></v></c></row></sheetData></worksheet>"#;
        assert_eq!(sheet_rows(xml, &[])?, vec![vec!["a<b>&amp;c".to_owned(), "x&y".into()]]);
        Ok(())
    }

    const STYLE_RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
        <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
        <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
    </Relationships>"#;

    /// Cell styles: 0 general, 1 date (built-in 14), 2 custom `00000`, 3 custom date and time,
    /// 4 built-in time with seconds, 5 elapsed hours, 6 custom `#,##0.00`.
    const STYLES: &str = r##"<styleSheet>
        <numFmts count="4">
            <numFmt numFmtId="164" formatCode="00000"/>
            <numFmt numFmtId="165" formatCode="dd/mm/yyyy\ hh:mm;@"/>
            <numFmt numFmtId="166" formatCode="[h]:mm:ss"/>
            <numFmt numFmtId="167" formatCode="#,##0.00"/>
        </numFmts>
        <cellStyleXfs count="1"><xf numFmtId="14"/></cellStyleXfs>
        <cellXfs count="7">
            <xf numFmtId="0"/><xf numFmtId="14" applyNumberFormat="1"/><xf numFmtId="164"/>
            <xf numFmtId="165"/><xf numFmtId="21"/><xf numFmtId="166"/><xf numFmtId="167"/>
        </cellXfs></styleSheet>"##;

    fn styled_workbook(workbook_xml: &str, sheet: &str) -> Vec<u8> {
        workbook(&[
            ("xl/workbook.xml", workbook_xml),
            ("xl/_rels/workbook.xml.rels", STYLE_RELS),
            ("xl/styles.xml", STYLES),
            ("xl/worksheets/sheet1.xml", sheet),
        ])
    }

    const SHEET_WORKBOOK: &str = r#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="A" sheetId="1" r:id="rId1"/></sheets></workbook>"#;

    #[test]
    fn dates_and_zero_padded_numbers_read_as_the_workbook_shows_them() -> Result<()> {
        let bytes = styled_workbook(
            SHEET_WORKBOOK,
            r#"<worksheet><sheetData><row>
                <c r="A1" s="1"><v>45614</v></c>
                <c r="B1" s="2"><v>42</v></c>
                <c r="C1"><v>42</v></c>
                <c r="D1" s="3"><v>45614.604166666664</v></c>
                <c r="E1" s="4"><v>0.5000578703703704</v></c>
                <c r="F1" s="5"><v>1.5</v></c>
                <c r="G1" s="6"><v>1234.5</v></c>
                <c r="H1" s="2" t="str"><v>abc</v></c>
                <c r="I1" s="9"><v>7</v></c>
            </row></sheetData></worksheet>"#,
        );
        assert_eq!(
            rows(&bytes)?,
            vec![vec![
                "2024-11-18".to_owned(),
                "00042".into(),
                "42".into(),
                "2024-11-18 14:30".into(),
                "12:00:05".into(),
                "1.5".into(),
                "1234.5".into(),
                "abc".into(),
                "7".into(),
            ]]
        );
        Ok(())
    }

    #[test]
    fn a_1904_workbook_counts_dates_from_1904() -> Result<()> {
        let workbook_xml = r#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><workbookPr date1904="1"/><sheets><sheet name="A" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
        let bytes = styled_workbook(
            workbook_xml,
            r#"<worksheet><sheetData><row><c r="A1" s="1"><v>44152</v></c><c r="B1" s="1"><v>0</v></c></row></sheetData></worksheet>"#,
        );
        assert_eq!(rows(&bytes)?, vec![vec!["2024-11-18".to_owned(), "1904-01-01".into()]]);
        Ok(())
    }

    #[test]
    fn date_serials_follow_excels_1900_calendar() {
        let date = |v: &str| formatted(v, Shown::Date, false);
        assert_eq!(date("1"), "1900-01-01");
        assert_eq!(date("59"), "1900-02-28");
        assert_eq!(date("60"), "1900-02-29");
        assert_eq!(date("61"), "1900-03-01");
        assert_eq!(date("45614"), "2024-11-18");
        assert_eq!(date("2958465"), "9999-12-31");
        // No date to show: kept as the number.
        assert_eq!(date("0"), "0");
        assert_eq!(date("-3"), "-3");
        assert_eq!(date("2958466"), "2958466");
        assert_eq!(date("n/a"), "n/a");
        // A time that rounds up to midnight moves to the next day.
        assert_eq!(
            formatted("45614.999999", Shown::DateTime { seconds: true }, false),
            "2024-11-19 00:00:00"
        );
        let padded = |v: &str| formatted(v, Shown::Padded(5), false);
        assert_eq!(padded("42"), "00042");
        assert_eq!(padded("-42"), "-00042");
        assert_eq!(padded("1234567"), "1234567");
        assert_eq!(
            formatted("1000000000000000", Shown::Padded(17), false),
            "01000000000000000"
        );
        // A date without a time keeps its day when the time would round to midnight.
        assert_eq!(date("45614.999999"), "2024-11-18");
        assert_eq!(padded("4.5"), "4.5");
    }

    #[test]
    fn format_codes_are_told_apart() {
        assert_eq!(custom_format("00000"), Shown::Padded(5));
        assert_eq!(custom_format("yyyy-mm-dd"), Shown::Date);
        assert_eq!(custom_format("[$-409]d-mmm-yy;@"), Shown::Date);
        assert_eq!(custom_format("hh:mm AM/PM"), Shown::Time { seconds: false });
        assert_eq!(custom_format("[h]:mm"), Shown::Plain);
        assert_eq!(custom_format("\"days\" 0"), Shown::Plain);
        assert_eq!(custom_format("\"y\"0"), Shown::Plain);
        assert_eq!(custom_format("#,##0.00;[Red]-#,##0.00"), Shown::Plain);
        assert_eq!(custom_format("General"), Shown::Plain);
        assert_eq!(custom_format("mmm yyyy"), Shown::Date);
        assert_eq!(custom_format("mmm"), Shown::Date);
        assert_eq!(custom_format("mm:ss"), Shown::Time { seconds: true });
        assert_eq!(custom_format("h:mm AM/PM"), Shown::Time { seconds: false });
        assert_eq!(custom_format("[>=1000]yyyy-mm-dd;0.00"), Shown::Plain);
        assert_eq!(custom_format("00000;-00000"), Shown::Plain);
        assert_eq!(custom_format(&"0".repeat(MAX_PADDING)), Shown::Padded(MAX_PADDING));
        assert_eq!(custom_format(&"0".repeat(MAX_PADDING + 1)), Shown::Plain);
        assert_eq!(custom_format("0.00E+00"), Shown::Plain);
        assert_eq!(builtin_format(14), Shown::Date);
        assert_eq!(builtin_format(22), Shown::DateTime { seconds: false });
        assert_eq!(builtin_format(46), Shown::Plain);
    }

    #[test]
    fn cell_styles_use_only_numfmts_and_style_0_by_default() -> Result<()> {
        let styles_xml = r#"<styleSheet>
            <numFmts count="1"><numFmt numFmtId="164" formatCode="00000"/></numFmts>
            <cellXfs count="1"><xf numFmtId="164"/></cellXfs>
            <dxfs count="1"><dxf><numFmt numFmtId="164" formatCode="yyyy-mm-dd"/></dxf></dxfs>
        </styleSheet>"#;
        let bytes = workbook(&[
            ("xl/workbook.xml", SHEET_WORKBOOK),
            ("xl/_rels/workbook.xml.rels", STYLE_RELS),
            ("xl/styles.xml", styles_xml),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><sheetData><row><c r="A1"><v>42</v></c><c r="B1" s="0"><v>42</v></c></row></sheetData></worksheet>"#,
            ),
        ]);
        assert_eq!(rows(&bytes)?, vec![vec!["00042".to_owned(), "00042".into()]]);
        Ok(())
    }

    #[test]
    fn a_malformed_style_id_is_an_error_with_its_reason() {
        let bad_cell = styled_workbook(
            SHEET_WORKBOOK,
            r#"<worksheet><sheetData><row><c r="A1" s="abc"><v>42</v></c></row></sheetData></worksheet>"#,
        );
        let err = rows(&bad_cell).map(|_| ()).map_err(|e| e.to_string());
        assert!(
            matches!(&err, Err(m) if m.contains("s \"abc\" is not a number")),
            "{err:?}"
        );
        let bad_format = styles(r#"<styleSheet><cellXfs><xf numFmtId="x1"/></cellXfs></styleSheet>"#)
            .map(|_| ())
            .map_err(|e| e.to_string());
        assert!(
            matches!(&bad_format, Err(m) if m.contains("numFmtId \"x1\" is not a number")),
            "{bad_format:?}"
        );
    }
}

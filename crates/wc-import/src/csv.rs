//! Parts lists and PLC address lists from CSV, as written by spreadsheet programs and most
//! CAD/ERP/PLC exports, or from Excel workbooks through [`file_rows`].

use std::collections::{BTreeMap, BTreeSet};

use wc_core::{Contacts, DeviceTag, Part, Project};

use crate::{Error, Result};

/// Decode bytes as UTF-8 (with or without BOM), falling back to Windows-1252, which is what
/// Excel writes on most European Windows machines (its curly quotes `“ ”`, `…`, `Š` and the
/// rest; the five bytes Windows-1252 has no character for keep their own value).
pub fn decode(bytes: &[u8]) -> String {
    let b = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_owned(),
        Err(_) => crate::dwg::codepage::Decoding::Page(30).decode(b),
    }
}

/// A field with the `'` a CSV export puts before text a spreadsheet would run as a formula
/// (`'=A+C1-K1`, `'-K1`, see `wc_report::formula_safe`) taken off again. A `'` before anything
/// else is part of the text and kept. A file cannot tell a guard from a `'` typed as text, so a
/// field `'-K1` reads `-K1` from any CSV, as a spreadsheet shows it; WiringCraft's reports write
/// a typed `'-K1` as `''-K1` (one `'` more however many it has), which reads back as `'-K1`.
fn unguard(field: String) -> String {
    match field.strip_prefix('\'') {
        Some(rest)
            if rest
                .trim_start_matches('\'')
                .starts_with(['=', '+', '-', '@', '\t', '\r']) =>
        {
            rest.to_owned()
        }
        _ => field,
    }
}

/// The rows of a list that are not blank, each with the number of the line (CSV, counting line
/// breaks inside quotes) or worksheet row (Excel) it starts on, from 1. Reads as its rows'
/// cells; warnings name a row by [`Rows::number`], so a blank row before it does not throw the
/// count off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rows {
    pub cells: Vec<Vec<String>>,
    /// The line or worksheet row of each of `cells`.
    pub numbers: Vec<usize>,
}

impl Rows {
    /// The line or worksheet row that row `i` starts on (rows given without numbers count from 1).
    pub fn number(&self, i: usize) -> usize {
        self.numbers.get(i).copied().unwrap_or(i + 1)
    }
}

/// Rows given without their numbers, such as a table's: numbered 1, 2, 3 and on.
impl From<Vec<Vec<String>>> for Rows {
    fn from(cells: Vec<Vec<String>>) -> Self {
        let numbers = (1..=cells.len()).collect();
        Rows { cells, numbers }
    }
}

impl std::ops::Deref for Rows {
    type Target = [Vec<String>];
    fn deref(&self) -> &Self::Target {
        &self.cells
    }
}

/// Split CSV text into records, leaving out blank ones. Handles quotes, doubled quotes and
/// newlines inside quotes, and takes off the `'` WiringCraft's CSV reports put before cells
/// such as `-K1` (`'-K1` reads `-K1`). Each record keeps the line it starts on.
pub fn records(text: &str, delim: char) -> Rows {
    let mut out = Rows::default();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    // The line being read, and the one the record being read starts on.
    let (mut line, mut start) = (1usize, 1usize);
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
                // A line break inside quotes is part of the field, and still a line of the file.
                if c == '\n' || (c == '\r' && chars.peek() != Some(&'\n')) {
                    line += 1;
                }
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
                out.cells.push(std::mem::take(&mut row));
                out.numbers.push(start);
            } else {
                row.clear();
            }
            line += 1;
            start = line;
        } else {
            field.push(c);
        }
    }
    row.push(unguard(field));
    if row.iter().any(|f| !f.trim().is_empty()) {
        out.cells.push(row);
        out.numbers.push(start);
    }
    out
}

/// The separator a CSV text uses: the one that splits the header [`records`] reads into the most
/// cells. Reading the header with [`records`] itself keeps the guess to the same header line, the
/// same line endings and the same quoting rules, also when blank lines come before it.
fn guess_delimiter(text: &str) -> char {
    [';', '\t', ',', '|']
        .into_iter()
        .max_by_key(|d| records(text, *d).cells.first().map_or(0, Vec::len))
        .unwrap_or(',')
}

#[derive(Clone, Copy, PartialEq)]
enum Col {
    Manufacturer,
    OrderNumber,
    Description,
    Symbol,
    /// A count of one kind of contact.
    Count(ContactKind),
    /// Contacts written out, such as `3NO+1NC`.
    Contacts,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ContactKind {
    Main,
    No,
    Nc,
    Co,
}

fn classify(h: &str) -> Col {
    // `NO`, `NC` and `CO` only in capitals: `No.` is a row number in many lists.
    match h.trim() {
        "NO" => return Col::Count(ContactKind::No),
        "NC" => return Col::Count(ContactKind::Nc),
        "CO" => return Col::Count(ContactKind::Co),
        _ => {}
    }
    let k: String = h.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
    match k.as_str() {
        "maincontacts" | "main" | "poles" | "hauptkontakte" | "pole" | "polzahl" | "hovedkontakter" | "poler" => {
            Col::Count(ContactKind::Main)
        }
        "nocontacts" | "normallyopen" | "schließer" | "schliesser" | "sluttekontakter" | "sluttekontakt" => {
            Col::Count(ContactKind::No)
        }
        "nccontacts" | "normallyclosed" | "öffner" | "oeffner" | "brytekontakter" | "brytekontakt" => {
            Col::Count(ContactKind::Nc)
        }
        "cocontacts" | "changeover" | "changeovercontacts" | "wechsler" | "vekselkontakter" | "vekselkontakt" => {
            Col::Count(ContactKind::Co)
        }
        "contacts" | "contactconfiguration" | "kontakte" | "kontaktbestückung" | "kontaktbestueckung" | "kontakter" => {
            Col::Contacts
        }
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
    kind_of_rows(&records(text, guess_delimiter(text)))
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
pub fn file_rows(bytes: &[u8]) -> Result<Rows> {
    if crate::xlsx::is_xlsx(bytes) {
        return crate::xlsx::rows(bytes);
    }
    if crate::xlsx::is_legacy_xls(bytes) {
        return Err(Error::Format(
            "this is an old Excel .xls workbook; save it as .xlsx or CSV and import that".into(),
        ));
    }
    let text = decode(bytes);
    Ok(records(&text, guess_delimiter(&text)))
}

/// Parse a parts list. The first row is the header; columns are matched by name (several
/// languages), anything unrecognised becomes a part property. Returns parts and warnings.
pub fn parts(text: &str) -> Result<(Vec<Part>, Vec<String>)> {
    parts_from_rows(&records(text, guess_delimiter(text)))
}

/// The most contacts of one kind a part may have.
const MAX_CONTACTS: u32 = 1000;

/// A count of contacts: a whole number from 0 to [`MAX_CONTACTS`], also as a spreadsheet
/// writes it (`3.0`). Fails with the reason.
fn contact_count(v: &str) -> std::result::Result<u32, String> {
    let n: f64 = v
        .trim()
        .replace(',', ".")
        .parse()
        .map_err(|_| "not a number".to_owned())?;
    if n.fract() != 0.0 {
        return Err("not a whole number".into());
    }
    if !(0.0..=f64::from(MAX_CONTACTS)).contains(&n) {
        return Err(format!("not between 0 and {MAX_CONTACTS}"));
    }
    Ok(n as u32)
}

/// Contacts written out, such as `3NO+1NC`, `1 NO, 1 NC`, `3 main + 1 CO` or German `1S+1Ö`:
/// pieces split at `+`, `,`, `;` or `/`, each a count (1 when left out) and a kind; the same
/// kind may come more than once (`1NO+2NO`). Fails with the reason for a piece that cannot be
/// read, a total over [`MAX_CONTACTS`] for a kind, or text that names no contact at all.
fn contacts_text(v: &str) -> std::result::Result<Contacts, String> {
    let mut c = Contacts::default();
    let mut named = false;
    for piece in v.split(['+', ',', ';', '/']) {
        let piece: String = piece
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>()
            .to_uppercase();
        if piece.is_empty() {
            continue;
        }
        let digits = piece.chars().take_while(char::is_ascii_digit).count();
        let (count, kind) = piece.split_at(digits);
        let n = if count.is_empty() {
            1
        } else {
            contact_count(count).map_err(|why| format!("{piece:?}: {why}"))?
        };
        let field = match kind {
            "MAIN" | "P" | "POLE" | "POLES" | "HK" => &mut c.main,
            "NO" | "S" => &mut c.no,
            "NC" | "Ö" | "OE" => &mut c.nc,
            "CO" | "W" => &mut c.co,
            _ => return Err(format!("{piece:?}: not a kind of contact (main, NO, NC or CO)")),
        };
        *field = field
            .checked_add(n)
            .filter(|t| *t <= MAX_CONTACTS)
            .ok_or_else(|| format!("more than {MAX_CONTACTS} contacts of one kind"))?;
        named = true;
    }
    if named { Ok(c) } else { Err("names no contact".into()) }
}

/// [`parts`] for rows already split into cells: an Excel worksheet's, or [`file_rows`]'.
///
/// A part's contacts come from count columns (`Main contacts`, `NO`, `NC`, `CO`, and their
/// German and Norwegian names) and from a `Contacts` column written out (`3NO+1NC`); a count
/// column wins over the same kind in the written-out one. A row whose contacts cannot be read
/// is imported without them, with a warning naming the row.
pub fn parts_from_rows(rows: &Rows) -> Result<(Vec<Part>, Vec<String>)> {
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
        // The header is row 0 of `rows`.
        let number = rows.number(i + 1);
        let mut p = Part::default();
        let mut contacts: Option<Contacts> = None;
        let mut counts: Vec<(ContactKind, u32)> = Vec::new();
        let mut unread: Vec<String> = Vec::new();
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
                Col::Count(kind) => match contact_count(v) {
                    Ok(n) => counts.push((*kind, n)),
                    Err(why) => unread.push(format!("{} {v:?}: {why}", h.trim())),
                },
                Col::Contacts => match contacts_text(v) {
                    Ok(c) => contacts = Some(c),
                    Err(why) => unread.push(format!("{} {v:?}: {why}", h.trim())),
                },
                Col::Other => {
                    p.properties.insert(h.trim().into(), v.into());
                }
            }
        }
        if p.order_number.is_empty() {
            warnings.push(format!("row {number}: no order number, skipped"));
            continue;
        }
        if unread.is_empty() {
            if !counts.is_empty() {
                let c = contacts.get_or_insert_default();
                for (kind, n) in counts {
                    *match kind {
                        ContactKind::Main => &mut c.main,
                        ContactKind::No => &mut c.no,
                        ContactKind::Nc => &mut c.nc,
                        ContactKind::Co => &mut c.co,
                    } = n;
                }
            }
            p.contacts = contacts;
        } else {
            warnings.push(format!(
                "row {number}: contacts not read ({}), the part is imported without them",
                unread.join(", ")
            ));
        }
        crate::add_part(&mut out, p, &format!("row {number}"), &mut warnings);
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
    plc_addresses_from_rows(&records(text, guess_delimiter(text)))
}

/// [`plc_addresses`] for rows already split into cells: an Excel worksheet's, or [`file_rows`]'.
pub fn plc_addresses_from_rows(rows: &Rows) -> Result<(Vec<PlcAddress>, Vec<String>)> {
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
        // The header is row 0 of `rows`.
        let number = rows.number(i + 1);
        let cell = |c: usize| row.get(c).map(|v| v.trim()).unwrap_or("");
        let (tag, address) = (cell(tag_col), cell(addr_col));
        if tag.is_empty() || tag.ends_with("(no tag)") {
            warnings.push(format!("row {number}: no device, skipped"));
            continue;
        }
        let Ok(tag) = DeviceTag::parse(tag) else {
            warnings.push(format!("row {number}: {tag:?} is not a device tag, skipped"));
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
                vec!["-K3".into(), "%I0.3".into(), "'-K1".into()],
                vec!["-K4".into(), "%I0.4".into(), "''=1".into()],
            ],
            ..wc_report::Table::default()
        };
        let csv = t.to_csv_with(';');
        // A spreadsheet sees text, not a formula, in each tag cell...
        assert!(csv.contains("'-K1;"), "{csv}");
        assert!(csv.contains("'-X12;"), "{csv}");
        assert!(csv.contains(";-5\r\n"), "{csv}");
        assert!(csv.contains(";''-K1\r\n"), "{csv}");
        // ...and the import reads every cell back as it was.
        let rows = records(&csv, ';');
        assert_eq!(rows[1..], t.rows[..]);
        let (addresses, warnings) = plc_addresses(&csv)?;
        assert!(warnings.is_empty(), "{warnings:?}");
        let tags: Vec<String> = addresses.iter().map(|a| a.tag.to_string()).collect();
        assert_eq!(tags, ["-K1", "-X12", "+C1-K2", "-K3", "-K4"]);
        Ok(())
    }

    #[test]
    fn row_warnings_name_the_line_past_blank_lines_and_quoted_line_breaks() -> Result<()> {
        // Line 2 is blank, line 3 only has separators; the record on line 4 runs on to line 6
        // through quoted line breaks (one CRLF, one lone CR).
        let text = "Order number;Description\r\n\r\n;\r\nA;\"two\r\nline\rtext\"\r\n;no order\r\n\nA;again\n";
        let rows = records(text, ';');
        assert_eq!(rows.numbers, [1, 4, 7, 9]);
        assert_eq!(rows[1], ["A", "two\r\nline\rtext"]);
        let (_, warnings) = parts(text)?;
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert_eq!(warnings[0], "row 7: no order number, skipped");
        assert!(warnings[1].starts_with("row 9: "), "{warnings:?}");
        // A PLC list counts the same way.
        let (_, warnings) = plc_addresses("Device;Address\n\n\n;%I0.0\n-K1;%I0.1\n\nK 1;%I0.2\n")?;
        assert_eq!(
            warnings,
            [
                "row 4: no device, skipped",
                "row 7: \"K 1\" is not a device tag, skipped"
            ]
        );
        // Rows given without numbers count from 1.
        let given = Rows::from(vec![vec!["Order number".to_owned()], vec![String::new()]]);
        assert_eq!(parts_from_rows(&given)?.1, ["row 2: no order number, skipped"]);
        Ok(())
    }

    #[test]
    fn separators_inside_quotes_or_after_a_literal_quote_do_not_count() {
        assert_eq!(guess_delimiter("Order number;\"Description\n,,,,\"\nA1;example\n"), ';');
        assert_eq!(guess_delimiter("Order number;Size (\")\nA1;,,,,\n"), ';');
    }

    #[test]
    fn a_line_break_inside_a_quoted_header_does_not_end_it() {
        assert_eq!(guess_delimiter("\"Order\rnumber\";Description\r\nA1;example\r\n"), ';');
        assert_eq!(guess_delimiter("\"Order\nnumber\";Description\nA1;example\n"), ';');
    }

    #[test]
    fn a_header_ended_by_a_lone_carriage_return_sets_the_separator() {
        // Old Mac line endings: the header is only the text up to the first `\r`.
        assert_eq!(guess_delimiter("\nManufacturer;Order number\rABB;1SBL,,,\r"), ';');
    }

    #[test]
    fn blank_lines_before_the_header_do_not_change_the_separator() -> Result<()> {
        let list = "Manufacturer;Order number;Description\r\nABB;1SBL;Contactor, 3 pole\r\n";
        for lead in ["\r\n", "\n\n", "  \t\n"] {
            let text = format!("{lead}{list}");
            assert_eq!(kind(&text), Some(CsvKind::Parts), "{lead:?}");
            assert_eq!(parts(&text)?, parts(list)?, "{lead:?}");
            let Ok(rows) = file_rows(text.as_bytes()) else {
                unreachable!("{lead:?} reads");
            };
            assert_eq!(rows[0], ["Manufacturer", "Order number", "Description"]);
        }
        let (rows, _) = plc_addresses("\nDevice;Address\n-K1;%I0.0\n")?;
        assert_eq!(rows.len(), 1);
        Ok(())
    }

    #[test]
    fn a_leading_apostrophe_is_a_guard_only_before_formula_characters() {
        // Any CSV: `'-K1` is taken as a guarded `-K1`, as a spreadsheet shows it.
        let rows = records(
            "Order;Text
'-K1;'-5
''-K1;'quoted'
",
            ';',
        );
        assert_eq!(rows[1], ["-K1", "-5"]);
        assert_eq!(rows[2], ["'-K1", "'quoted'"]);
    }

    #[test]
    fn a_windows_1252_file_reads_its_curly_quotes_and_letters() -> Result<()> {
        // Excel's "CSV (semicolon)" on a Western European Windows: “ ” … • ™ Š ž and the euro.
        let bytes = b"Bestellnummer;Beschreibung\r\nX1;\x93grau\x94 \x85 \x95 \x99 \x8a\x9e \x80\r\n";
        let (parts, warnings) = parts(&decode(bytes))?;
        assert!(warnings.is_empty(), "{warnings:?}");
        let [p] = parts.as_slice() else {
            unreachable!("one part: {parts:?}");
        };
        assert_eq!(
            p.description,
            "\u{201c}grau\u{201d} \u{2026} \u{2022} \u{2122} \u{160}\u{17e} \u{20ac}"
        );
        // A byte Windows-1252 has no character for keeps its value; Latin-1 letters stay as they are.
        assert_eq!(decode(b"\x81\xe6\xf8\xe5"), "\u{81}\u{e6}\u{f8}\u{e5}");
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
    fn contacts_are_read_from_count_columns_and_written_out() -> Result<()> {
        let c = |main, no, nc, co| Some(Contacts { main, no, nc, co });
        let (p, w) = parts(
            "Order number;Main contacts;NO;NC;CO;No.\n\
             LC1D09;3;1;1;;7\n\
             CA2;;2.0;2;0;8\n\
             PLAIN;;;;;9\n",
        )?;
        assert!(w.is_empty(), "{w:?}");
        let by = |n: &str| p.iter().find(|p| p.order_number == n).and_then(|p| p.contacts);
        assert_eq!(by("LC1D09"), c(3, 1, 1, 0));
        assert_eq!(by("CA2"), c(0, 2, 2, 0));
        // No contact cells: not known, not zero.
        assert_eq!(by("PLAIN"), None);
        // `No.` is a row number, not normally open contacts.
        assert_eq!(
            p.iter()
                .find(|p| p.order_number == "LC1D09")
                .and_then(|p| p.properties.get("No."))
                .map(String::as_str),
            Some("7")
        );

        let (p, w) = parts(
            "Bestellnummer;Kontakte;Öffner\n\
             K1;3 main + 1 NO;\n\
             K2;1S+1Ö;2\n\
             K3;2CO;\n\
             K4;3NO+lots;\n\
             K5;;two\n",
        )?;
        let by = |n: &str| p.iter().find(|p| p.order_number == n).and_then(|p| p.contacts);
        assert_eq!(by("K1"), c(3, 1, 0, 0));
        // The count column wins over the same kind written out.
        assert_eq!(by("K2"), c(0, 1, 2, 0));
        assert_eq!(by("K3"), c(0, 0, 0, 2));
        // Unreadable contacts: the part comes in without them, and the row is named.
        assert_eq!(p.len(), 5);
        assert_eq!((by("K4"), by("K5")), (None, None));
        assert_eq!(
            w,
            [
                "row 5: contacts not read (Kontakte \"3NO+lots\": \"LOTS\": not a kind of contact (main, NO, NC or CO)), \
                 the part is imported without them",
                "row 6: contacts not read (Öffner \"two\": not a number), the part is imported without them",
            ]
        );
        Ok(())
    }

    #[test]
    fn written_out_contacts_add_up_and_say_why_they_cannot_be_read() {
        let c = |main, no, nc, co| Ok(Contacts { main, no, nc, co });
        assert_eq!(contacts_text("1NO+2NO"), c(0, 3, 0, 0));
        assert_eq!(contacts_text("0NO"), c(0, 0, 0, 0));
        // Separators alone name nothing: unknown, not zero.
        assert_eq!(contacts_text("+"), Err("names no contact".into()));
        assert_eq!(contacts_text(" , "), Err("names no contact".into()));
        // A total over the limit is refused, also when it would overflow.
        assert_eq!(
            contacts_text("1000NO+1NO"),
            Err("more than 1000 contacts of one kind".into())
        );
        let many = ["1000NO"; 5].join("+");
        assert_eq!(contacts_text(&many), Err("more than 1000 contacts of one kind".into()));
        assert_eq!(
            contacts_text("1001NC"),
            Err("\"1001NC\": not between 0 and 1000".into())
        );
        assert_eq!(contact_count("2.5"), Err("not a whole number".into()));
        assert_eq!(contact_count("-1"), Err("not between 0 and 1000".into()));
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

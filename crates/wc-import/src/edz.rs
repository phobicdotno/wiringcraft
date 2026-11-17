//! Part data from EDZ files (EPLAN Data Exchange archives, as the EPLAN Data Portal and makers'
//! download pages hand them out).
//!
//! An EDZ is a 7-Zip archive (some tools write a plain zip). Its part data is EPLAN's part XML:
//! a `partsmanagement` root with one `part` element per part, whose attributes are EPLAN part
//! properties (`P_ARTICLE_PARTNR`, `P_ARTICLE_MANUFACTURER`, `P_ARTICLE_ORDERNR`,
//! `P_ARTICLE_DESCR1`, ...). A `variant` inside a part fills in what the part leaves empty.
//! Texts in several languages are written `de_DE@Schütz;en_US@Contactor;`; English is taken
//! when there is one. Macros (`.ema`), pictures and other entries are counted, not imported.
//!
//! Written from public descriptions of the format and checked with synthetic archives only.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use wc_core::Part;

use crate::{Error, Result};

/// The largest entry read, in bytes; a larger one is skipped and counted.
pub const MAX_ENTRY: u64 = 64 * 1024 * 1024;
/// The most entries looked at.
pub const MAX_ENTRIES: usize = 100_000;
/// The most part XML read in all, in bytes, so an archive cannot fill the memory.
pub const MAX_TOTAL: u64 = 512 * 1024 * 1024;
/// Deepest XML nesting accepted.
const MAX_DEPTH: usize = 64;

/// Property holding EPLAN's own part number (`SIE.3RT2015-1BB41`).
pub const PROP_EPLAN_PART: &str = "EplanPartNumber";
/// Property holding the type number.
pub const PROP_TYPE: &str = "TypeNumber";

const SEVEN_ZIP: &[u8] = b"7z\xBC\xAF\x27\x1C";
const ZIP: &[u8] = b"PK\x03\x04";

/// What was in the archive.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Parts read.
    pub parts: usize,
    /// XML files that held part data.
    pub part_files: usize,
    /// EPLAN macros (`.ema`), not imported.
    pub macros: usize,
    /// Pictures, not imported.
    pub pictures: usize,
    /// Everything else (other XML, documents, folders' contents), not imported.
    pub other: usize,
    /// Entries larger than [`MAX_ENTRY`], skipped.
    pub too_large: usize,
}

impl Report {
    /// The report as lines for the import's output.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!("{} part(s) from {} part file(s)", self.parts, self.part_files)];
        if self.macros > 0 {
            out.push(format!("{} macro(s) (.ema): not imported", self.macros));
        }
        if self.pictures > 0 {
            out.push(format!("{} picture(s): not imported", self.pictures));
        }
        if self.other > 0 {
            out.push(format!("{} other file(s): not imported", self.other));
        }
        if self.too_large > 0 {
            out.push(format!(
                "{} file(s) larger than {} MiB: skipped",
                self.too_large,
                MAX_ENTRY / 1024 / 1024
            ));
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    Xml,
    Macro,
    Picture,
    Other,
}

fn entry_kind(name: &str) -> Entry {
    let lower = name.to_lowercase();
    match lower.rsplit_once('.').map_or("", |(_, ext)| ext) {
        "xml" => Entry::Xml,
        "ema" => Entry::Macro,
        "png" | "jpg" | "jpeg" | "bmp" | "gif" | "tif" | "tiff" | "wmf" | "emf" | "svg" => Entry::Picture,
        _ => Entry::Other,
    }
}

/// Read an EDZ archive's parts. Returns the parts (sorted by id), what the archive held, and
/// warnings. A damaged archive or part file is an error, never a panic.
pub fn read(bytes: &[u8]) -> Result<(Vec<Part>, Report, Vec<String>)> {
    let mut report = Report::default();
    let mut xml: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total = 0u64;
    let mut take = |name: &str, size: u64, data: &mut dyn Read, report: &mut Report| -> Result<bool> {
        match entry_kind(name) {
            Entry::Macro => report.macros += 1,
            Entry::Picture => report.pictures += 1,
            Entry::Other => report.other += 1,
            Entry::Xml if size > MAX_ENTRY || total.saturating_add(size) > MAX_TOTAL => report.too_large += 1,
            Entry::Xml => {
                let mut buf = Vec::new();
                data.take(MAX_ENTRY + 1)
                    .read_to_end(&mut buf)
                    .map_err(|e| Error::Format(format!("{name} cannot be read from the archive: {e}")))?;
                if buf.len() as u64 > MAX_ENTRY {
                    report.too_large += 1;
                } else {
                    total += buf.len() as u64;
                    xml.push((name.to_owned(), buf));
                }
            }
        }
        Ok(true)
    };

    if bytes.starts_with(SEVEN_ZIP) {
        let mut reader = sevenz_rust2::ArchiveReader::new(Cursor::new(bytes), sevenz_rust2::Password::empty())
            .map_err(|e| Error::Format(format!("not a readable EDZ (7-Zip) archive: {e}")))?;
        let mut seen = 0usize;
        let mut failed: Option<Error> = None;
        reader
            .for_each_entries(|entry, data| {
                if entry.is_directory {
                    return Ok(true);
                }
                seen += 1;
                if seen > MAX_ENTRIES {
                    return Ok(false);
                }
                match take(&entry.name, entry.size, data, &mut report) {
                    Ok(go) => Ok(go),
                    Err(e) => {
                        failed = Some(e);
                        Ok(false)
                    }
                }
            })
            .map_err(|e| Error::Format(format!("the EDZ archive is damaged: {e}")))?;
        if let Some(e) = failed {
            return Err(e);
        }
    } else if bytes.starts_with(ZIP) {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| Error::Format(format!("not a readable EDZ (zip) archive: {e}")))?;
        for i in 0..z.len().min(MAX_ENTRIES) {
            let mut f = z
                .by_index(i)
                .map_err(|e| Error::Format(format!("the EDZ archive is damaged: {e}")))?;
            if f.is_dir() {
                continue;
            }
            let (name, size) = (f.name().to_owned(), f.size());
            take(&name, size, &mut f, &mut report)?;
        }
    } else {
        return Err(Error::Format(
            "not an EDZ archive (neither a 7-Zip nor a zip file)".into(),
        ));
    }

    let mut parts: BTreeMap<String, Part> = BTreeMap::new();
    let mut warnings = Vec::new();
    for (name, data) in xml {
        let text = crate::csv::decode(&data);
        match part_xml(&text) {
            Ok(None) => report.other += 1,
            Ok(Some(found)) => {
                report.part_files += 1;
                for p in found {
                    if parts.contains_key(&p.id) {
                        warnings.push(format!("part {} is in the archive twice; the later one is kept", p.id));
                    }
                    parts.insert(p.id.clone(), p);
                }
            }
            Err(e) => return Err(Error::Format(format!("{name}: {e}"))),
        }
    }
    report.parts = parts.len();
    if report.parts == 0 {
        warnings.push("the archive holds no part data".into());
    }
    Ok((parts.into_values().collect(), report, warnings))
}

/// One part's EPLAN properties, by name, and the first variant's for what the part leaves empty.
#[derive(Default)]
struct Raw {
    props: BTreeMap<String, String>,
    variant: Option<BTreeMap<String, String>>,
}

fn attributes(e: &BytesStart<'_>) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for a in e.attributes() {
        let a = a.map_err(|err| Error::Format(format!("bad attribute: {err}")))?;
        let key = String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned();
        let value = a
            .unescape_value()
            .map_err(|err| Error::Format(format!("bad value of {key}: {err}")))?;
        let value = value.trim();
        if !value.is_empty() {
            out.insert(key, value.to_owned());
        }
    }
    Ok(out)
}

/// The parts in one EPLAN part XML file; `None` when the file is not part data.
pub fn part_xml(text: &str) -> Result<Option<Vec<Part>>> {
    let mut reader = Reader::from_str(text);
    let mut depth = 0usize;
    let mut raws: Vec<Raw> = Vec::new();
    // The part being read, when inside one.
    let mut current: Option<Raw> = None;
    let mut part_depth = 0usize;
    loop {
        let ev = reader
            .read_event()
            .map_err(|e| Error::Format(format!("not readable XML: {e}")))?;
        let (start, empty) = match &ev {
            Event::Start(e) => (Some(e), false),
            Event::Empty(e) => (Some(e), true),
            _ => (None, false),
        };
        if let Some(e) = start {
            if depth >= MAX_DEPTH {
                return Err(Error::Format("the file is nested too deeply".into()));
            }
            let tag = String::from_utf8_lossy(e.local_name().as_ref()).to_lowercase();
            if depth == 0 && tag != "partsmanagement" {
                return Ok(None);
            }
            if tag == "part" && current.is_none() {
                let raw = Raw {
                    props: attributes(e)?,
                    variant: None,
                };
                if empty {
                    raws.push(raw);
                } else {
                    current = Some(raw);
                    part_depth = depth + 1;
                }
            } else if tag == "variant"
                && let Some(raw) = current.as_mut()
                && raw.variant.is_none()
            {
                raw.variant = Some(attributes(e)?);
            }
            if !empty {
                depth += 1;
            }
            continue;
        }
        match ev {
            Event::End(_) => {
                if depth == part_depth
                    && let Some(raw) = current.take()
                {
                    raws.push(raw);
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 {
        return Err(Error::Format("the file ends inside an element".into()));
    }
    Ok(Some(raws.into_iter().filter_map(part_of).collect()))
}

/// The text for one language out of `de_DE@Schütz;en_US@Contactor;`: English first, then the
/// first one given. Plain text is returned as it is.
pub fn pick_language(value: &str) -> String {
    if !value.contains('@') {
        return value.trim().to_owned();
    }
    let texts: Vec<(&str, &str)> = value
        .split(';')
        .filter_map(|t| t.split_once('@'))
        .map(|(lang, text)| (lang.trim(), text.trim()))
        .filter(|(_, text)| !text.is_empty())
        .collect();
    let english = |l: &str| l.eq_ignore_ascii_case("en_US") || l.to_lowercase().starts_with("en");
    texts
        .iter()
        .find(|(l, _)| english(l))
        .or_else(|| texts.first())
        .map(|(_, t)| (*t).to_owned())
        .unwrap_or_default()
}

fn part_of(raw: Raw) -> Option<Part> {
    let mut props = raw.props;
    if let Some(variant) = raw.variant {
        for (k, v) in variant {
            props.entry(k).or_insert(v);
        }
    }
    let mut get = |k: &str| props.remove(k).map(|v| pick_language(&v)).unwrap_or_default();
    let partnr = get("P_ARTICLE_PARTNR");
    let manufacturer = get("P_ARTICLE_MANUFACTURER");
    let ordernr = get("P_ARTICLE_ORDERNR");
    let typenr = get("P_ARTICLE_TYPENR");
    let mut description = get("P_ARTICLE_DESCR1");
    if description.is_empty() {
        description = get("P_ARTICLE_NOTE");
    }
    // EPLAN part numbers start with the maker's short name: `SIE.3RT2015-1BB41`.
    let bare = partnr
        .strip_prefix(&format!("{manufacturer}."))
        .unwrap_or(&partnr)
        .to_owned();
    let order_number = [&ordernr, &typenr, &bare].into_iter().find(|s| !s.is_empty())?.clone();
    let mut properties = BTreeMap::new();
    if !partnr.is_empty() {
        properties.insert(PROP_EPLAN_PART.to_owned(), partnr);
    }
    if !typenr.is_empty() && typenr != order_number {
        properties.insert(PROP_TYPE.to_owned(), typenr);
    }
    for (k, v) in props {
        let v = pick_language(&v);
        if !v.is_empty() {
            properties.insert(k, v);
        }
    }
    Some(Part {
        id: Part::make_id(&manufacturer, &order_number),
        manufacturer,
        order_number,
        description,
        symbol: None,
        properties,
    })
}

#[cfg(test)]
mod tests;

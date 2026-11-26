//! Eplan Electric P8 project import (`.elk` plus its `.edb` folder).
//!
//! The format was worked out from sample projects and their PDF exports; no Eplan software was
//! decompiled or run. What is read:
//!
//! - `ProjectInfo.xml`: project properties (plain XML).
//! - `*.eod` tables: records of one class each. A record's tagged part starts with the field
//!   `60001` (its own class and id) after an `ff ff` marker. Fields are a 16-bit id followed by
//!   a value whose size depends on the field. Two kinds of fields are read:
//!   - user properties: blocks of `(u16 id, u16 index, UTF-8 C string)` entries ended by
//!     `ff ff`, after the marker field `60011` (properties) or `60150` (device tag parts);
//!   - a handful of fixed fields: references (`u16 class, u32 id`), points (two `f64`), and
//!     C strings such as connection ends (`64003`, `64004`).
//!
//! From those: pages (name, description, scale), devices (tag, function text, technical data,
//! part numbers, page, position, connection point names, symbol variant), cached article data,
//! connections (both ends as `device:pin`, page, colour, cross-section, cable), and from
//! `instance.eod` the pages' own graphics (lines, rectangles, circles, texts) and wire corners.
//!
//! [`build`] rebuilds each Eplan page as drawn (see [`drawing`]): Eplan keeps symbol graphics in
//! its libraries, not in the project, so devices get WiringCraft symbols of the same kind at
//! their original positions, and wires are traced between facing connection points as Eplan
//! does. Devices that are not on a drawn page are laid out as connection overview pages (one
//! section per Eplan page), and a PDF export of the project can be added as trace pages with
//! the PDF importer.
//!
//! Input is hostile: every read is bounds-checked and malformed records are skipped.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use wc_core::DeviceTag;
use wc_core::geom::Point;

use crate::aml::{self, AmlDevice, AmlImport, AmlLink, Section};
use crate::{Error, Result};

/// Classes of the records this importer reads.
const CLASS_PAGE: u16 = 4;
const CLASS_FUNCTION: u16 = 17;
const CLASS_CONNECTION: u16 = 18;
const CLASS_ARTICLE_REF: u16 = 68;
const CLASS_ARTICLE: u16 = 117;
const CLASS_TEXT: u16 = 30;
const CLASS_LINE: u16 = 31;
const CLASS_CIRCLE: u16 = 32;
const CLASS_NODE: u16 = 42;
const CLASS_RECT: u16 = 89;

/// Field ids.
const F_PROPERTIES: u16 = 60011;
const F_TAG_PARTS: u16 = 60150;
const F_OWNER: u16 = 60401;
const F_CONNECTION_PAGE: u16 = 60202;
const F_POSITION: u16 = 60762;
const F_PAGE_NAME: u16 = 61101;
const F_TAG_ID: u16 = 60972;
const F_TAG_COUNTER: u16 = 60973;
const F_TAG_SUB_ID: u16 = 60976;
const F_TAG_SUB_COUNTER: u16 = 60977;
const F_PART_NUMBER: u16 = 61273;
const F_CONNECTION_FROM: u16 = 64003;
const F_CONNECTION_TO: u16 = 64004;
const F_PINS: u16 = 60185;
const F_GRAPHIC_PAGE: u16 = 60402;
const F_TEXT_AT: u16 = 60501;
const F_TEXT: u16 = 60511;
const F_LINE_FROM: u16 = 60531;
const F_LINE_TO: u16 = 60532;
const F_CIRCLE_CENTER: u16 = 60555;
const F_CIRCLE_RADIUS: u16 = 60556;
const F_TEXT_HEIGHT: u16 = 60961;
const F_TEXT_ANGLE: u16 = 60962;
const F_SYMBOL_NUMBER: u16 = 61262;
const F_VARIANT: u16 = 61263;
const F_FUNCTION_KIND: u16 = 61381;
const F_FUNCTION_DETAIL: u16 = 61382;
const F_RECT_FROM: u16 = 61651;
const F_RECT_TO: u16 = 61652;

/// Property ids (Eplan's own numbering, also used in `ProjectInfo.xml`).
const P_PAGE_DESCRIPTION: u16 = 11011;
const P_PAGE_SCALE: u16 = 11048;
const P_TAG_ID: u16 = 20013;
const P_TAG_COUNTER: u16 = 20014;
const P_TAG_SUB_ID: u16 = 20017;
const P_TAG_SUB_COUNTER: u16 = 20018;
const P_FUNCTION_TEXT: u16 = 20025;
const P_TECHNICAL: u16 = 20027;
const P_TERMINAL: u16 = 20030;
const P_CABLE_TYPE: u16 = 20040;
const P_CABLE_CONDUCTORS: u16 = 20042;
const P_CABLE_CROSS_SECTION: u16 = 20043;
const P_CABLE_LENGTH: u16 = 20046;
const P_PART: u16 = 20100;
const P_ARTICLE_TYPE: u16 = 22001;
const P_ARTICLE_NUMBER: u16 = 22003;
const P_ARTICLE_DESCRIPTION: u16 = 22004;
const P_ARTICLE_MANUFACTURER: u16 = 22007;
const P_WIRE_CROSS_SECTION: u16 = 31002;
const P_WIRE_COLOUR: u16 = 31004;

/// Largest table file read (the biggest seen are a few MB).
const MAX_TABLE: usize = 64 << 20;
const MAX_STRING: usize = 8192;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EplanPage {
    pub id: u32,
    /// Page name as shown in the page navigator (`1`, `2`, ...).
    pub name: String,
    pub description: String,
    /// Drawing scale (layout pages are often 1:5 or 1:6; schematics 1:1).
    pub scale: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EplanFunction {
    pub id: u32,
    /// Device tag as Eplan shows it without structure prefixes, e.g. `K1` or `PUMPE1-M1`.
    pub tag: String,
    pub page: Option<u32>,
    pub at: Option<Point>,
    pub text: String,
    pub technical: String,
    /// Part numbers in order.
    pub parts: Vec<String>,
    /// Other readable properties, named.
    pub properties: BTreeMap<String, String>,
    /// Connection point names in symbol order (may be empty strings, as on terminals).
    pub pins: Vec<String>,
    /// Function category (coil, contact, terminal, ...), see [`drawing`].
    pub kind: u16,
    /// Refinement of the category, such as make (1) or break (2) for a contact.
    pub detail: u16,
    /// Symbol variant: 0 to 3 turn the symbol in quarter steps, 4 to 7 also mirror it.
    pub variant: u8,
    /// Terminal designation, for terminals.
    pub terminal: String,
}

/// A free graphic on a page, in Eplan coordinates (millimetres, y up).
#[derive(Debug, Clone, PartialEq)]
pub struct EplanGraphic {
    pub page: u32,
    pub shape: wc_core::geom::Shape,
}

/// A wire corner or junction (Eplan draws wires between these and symbol connection points).
#[derive(Debug, Clone, PartialEq)]
pub struct EplanNode {
    pub page: u32,
    pub at: Point,
    pub symbol: u16,
    pub variant: u8,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EplanArticle {
    pub part_number: String,
    pub type_number: String,
    pub description: String,
    pub manufacturer: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EplanConnection {
    pub page: Option<u32>,
    /// Ends as Eplan prints them: `K1:21`, `X2:3:b` (terminal 3, level b), or a bare name.
    pub from: String,
    pub to: String,
    pub cross_section: String,
    pub colour: String,
    /// Cable the connection runs in, e.g. `WSC2`.
    pub cable: String,
}

impl EplanConnection {
    /// Cross-section and colour as a wire spec. A plain number is in mm², as EPLAN's default
    /// unit; anything else (`18 AWG`) is kept as written, and 0 is no section.
    pub fn spec(&self) -> wc_core::WireSpec {
        let v = self.cross_section.trim();
        let section = match v.parse::<f64>() {
            Ok(0.0) => None,
            Ok(_) => Some(format!("{v} mm²")),
            Err(_) => wc_core::WireSpec::value(v),
        };
        wc_core::WireSpec {
            section,
            colour: wc_core::WireSpec::value(&self.colour),
            kind: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EplanProject {
    /// Project properties from `ProjectInfo.xml`: (id, name, value).
    pub properties: Vec<(u32, String, String)>,
    pub pages: Vec<EplanPage>,
    pub functions: Vec<EplanFunction>,
    /// Part numbers referenced per device tag (from the article references table).
    pub article_refs: Vec<(String, String)>,
    /// Article data cached in the project (often only part of what is used).
    pub articles: Vec<EplanArticle>,
    pub connections: Vec<EplanConnection>,
    pub graphics: Vec<EplanGraphic>,
    pub nodes: Vec<EplanNode>,
    pub warnings: Vec<String>,
}

/// The table files of an `.edb` folder, keyed by lower-case file name (`page.eod`).
pub type Tables = BTreeMap<String, Vec<u8>>;

/// Read the tables from a zip of an `.edb` folder (any folder depth), with warnings. A table
/// that is in the zip but cannot be read is an error naming it, not an empty table; an
/// unreadable `ProjectInfo.xml` (optional title block data) is a warning and left out.
pub fn tables_from_zip(bytes: &[u8]) -> Result<(Tables, Vec<String>)> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| Error::Format(format!("zip: {e}")))?;
    let mut out = Tables::new();
    let mut warnings = Vec::new();
    for i in 0..z.len() {
        let Ok(mut f) = z.by_index(i) else { continue };
        let name = f.name().rsplit(['/', '\\']).next().unwrap_or("").to_lowercase();
        if !wanted(&name) || f.size() as usize > MAX_TABLE {
            continue;
        }
        let mut data = Vec::new();
        match f.by_ref().take(MAX_TABLE as u64).read_to_end(&mut data) {
            Ok(_) => {
                out.insert(name, data);
            }
            Err(e) if optional(&name) => warnings.push(format!("{}: {e}; read without it", f.name())),
            Err(e) => return Err(Error::Format(format!("{}: {e}", f.name()))),
        }
    }
    if out.is_empty() {
        return Err(Error::Format("the zip holds no Eplan project tables (*.eod)".into()));
    }
    Ok((out, warnings))
}

/// Whether a wanted file of an `.edb` folder may be left out when it cannot be read: the project
/// properties only fill title block fields, the tables carry the project itself.
pub fn optional(lower_name: &str) -> bool {
    lower_name == "projectinfo.xml"
}

/// Whether a file of an `.edb` folder is one this importer reads.
pub fn wanted(lower_name: &str) -> bool {
    lower_name.ends_with(".eod") || lower_name == "projectinfo.xml"
}

/// The `.edb` folder name an `.elk` file points to (its first line, UTF-16 or UTF-8).
pub fn elk_target(bytes: &[u8]) -> Option<String> {
    let text = if bytes.starts_with(&[0xff, 0xfe]) || (bytes.len() > 1 && bytes.get(1) == Some(&0)) {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    let first = text.trim_start_matches('\u{feff}').lines().next()?.trim().to_owned();
    (!first.is_empty() && !first.contains(['/', '\\'])).then_some(first)
}

/// Pick one language from a multi-language Eplan text (`en_US@Front page;de_DE@Titelblatt;`):
/// English, then language-neutral (`??_??`), then the first one.
pub fn pick_language(s: &str) -> String {
    if !s.contains('@') {
        return s.to_owned();
    }
    let mut parts: Vec<(&str, &str)> = Vec::new();
    for entry in s.split(';') {
        if entry.is_empty() {
            continue;
        }
        if let Some((lang, text)) = entry.split_once('@')
            && lang.len() == 5
        {
            parts.push((lang, text));
        } else if let Some(last) = parts.last_mut() {
            // A ';' inside the text: glue it back on.
            let start = last.1.as_ptr() as usize - s.as_ptr() as usize;
            let end = entry.as_ptr() as usize - s.as_ptr() as usize + entry.len();
            if let Some(joined) = s.get(start..end) {
                last.1 = joined;
            }
        }
    }
    for want in ["en_US", "en_GB", "??_??"] {
        if let Some((_, t)) = parts.iter().find(|(l, _)| *l == want) {
            return (*t).to_owned();
        }
    }
    parts
        .first()
        .map(|(_, t)| (*t).to_owned())
        .unwrap_or_else(|| s.to_owned())
}

/// The records of a table: each slice starts at its `60001` field.
fn records(data: &[u8]) -> Vec<&[u8]> {
    const ANCHOR: [u8; 4] = [0xff, 0xff, 0x61, 0xea];
    let starts: Vec<usize> = data
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == ANCHOR)
        .map(|(i, _)| i + 2)
        .collect();
    starts
        .iter()
        .enumerate()
        .filter_map(|(k, &s)| {
            let e = starts.get(k + 1).map(|n| n - 2).unwrap_or(data.len());
            data.get(s..e)
        })
        .collect()
}

fn u16_at(r: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*r.get(at)?, *r.get(at + 1)?]))
}

fn u32_at(r: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *r.get(at)?,
        *r.get(at + 1)?,
        *r.get(at + 2)?,
        *r.get(at + 3)?,
    ]))
}

fn f64_at(r: &[u8], at: usize) -> Option<f64> {
    let b: [u8; 8] = r.get(at..at + 8)?.try_into().ok()?;
    let v = f64::from_le_bytes(b);
    v.is_finite().then_some(v)
}

/// A record's own (class, id).
fn self_ref(r: &[u8]) -> Option<(u16, u32)> {
    (u16_at(r, 0)? == 60001).then_some(())?;
    Some((u16_at(r, 2)?, u32_at(r, 4)?))
}

/// Offsets just after each occurrence of field id `id` (past the record's own reference).
fn field_offsets(r: &[u8], id: u16) -> impl Iterator<Item = usize> + '_ {
    let b = id.to_le_bytes();
    r.windows(2)
        .enumerate()
        .skip(8)
        .filter(move |(_, w)| *w == b)
        .map(|(i, _)| i + 2)
}

fn c_string(r: &[u8], at: usize) -> Option<String> {
    let rest = r.get(at..)?;
    let end = rest.iter().take(MAX_STRING).position(|&b| b == 0)?;
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// A printable C string field.
fn text_field(r: &[u8], id: u16) -> Option<String> {
    field_offsets(r, id)
        .find_map(|at| c_string(r, at).filter(|s| !s.is_empty() && s.chars().all(|c| !c.is_control() || c == '\n')))
}

/// A reference field to a record of a class below 1000.
fn ref_field(r: &[u8], id: u16) -> Option<(u16, u32)> {
    field_offsets(r, id).find_map(|at| {
        let class = u16_at(r, at)?;
        let rid = u32_at(r, at + 2)?;
        (class > 0 && class < 1000 && rid > 0).then_some((class, rid))
    })
}

/// A point field: two doubles in a plausible range.
fn point_field(r: &[u8], id: u16) -> Option<Point> {
    field_offsets(r, id).find_map(|at| {
        let (x, y) = (f64_at(r, at)?, f64_at(r, at + 8)?);
        (x.abs() < 1e6 && y.abs() < 1e6).then(|| Point::new(x, y))
    })
}

/// User properties after a marker field: (id, index) -> value.
fn user_block(r: &[u8], marker: u16) -> BTreeMap<(u16, u16), String> {
    for start in field_offsets(r, marker) {
        let mut p = start;
        let mut block = BTreeMap::new();
        // Entries run until `ff ff`; an entry that does not look like one ends the block early
        // (keeping what was read) so a damaged record still yields its first properties.
        while let (Some(id), Some(idx)) = (u16_at(r, p), u16_at(r, p + 2)) {
            if !(1000..60000).contains(&id) {
                break;
            }
            let Some(v) = c_string(r, p + 4) else { break };
            p += 4 + v.len() + 1;
            block.insert((id, idx), v);
        }
        if !block.is_empty() {
            return block;
        }
    }
    BTreeMap::new()
}

fn prop(block: &BTreeMap<(u16, u16), String>, id: u16) -> String {
    block.get(&(id, 0)).cloned().unwrap_or_default()
}

/// Device tag text from its parts: `PUMPE` `1` `M` `1` -> `PUMPE1-M1`.
fn tag_text(id: &str, counter: &str, sub_id: &str, sub_counter: &str) -> String {
    let main = format!("{id}{counter}");
    let sub = format!("{sub_id}{sub_counter}");
    match (main.is_empty(), sub.is_empty()) {
        (false, false) => format!("{main}-{sub}"),
        (false, true) => main,
        (true, false) => sub,
        (true, true) => String::new(),
    }
}

fn project_info(xml: &[u8]) -> Vec<(u32, String, String)> {
    let mut reader = Reader::from_reader(xml);
    let mut out = Vec::new();
    let mut current: Option<(u32, String)> = None;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name().as_ref() == b"Property" => {
                let mut id = 0;
                let mut name = String::new();
                for a in e.attributes().flatten() {
                    let v = a.unescape_value().map(|v| v.into_owned()).unwrap_or_default();
                    match a.key.as_ref() {
                        b"id" => id = v.parse().unwrap_or(0),
                        b"name" => name = v,
                        _ => {}
                    }
                }
                current = Some((id, name));
            }
            Ok(Event::Text(t)) => {
                if let Some((id, name)) = current.take() {
                    let v = t.unescape().map(|v| v.into_owned()).unwrap_or_default();
                    out.push((id, name, pick_language(v.trim())));
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// Read an Eplan project from its tables.
pub fn read(tables: &Tables) -> Result<EplanProject> {
    if !tables.keys().any(|k| k.ends_with(".eod")) {
        return Err(Error::Format("no Eplan project tables (*.eod) found".into()));
    }
    let mut p = EplanProject::default();
    let table = |name: &str| tables.get(name).map(Vec::as_slice).unwrap_or(&[]);
    if let Some(xml) = tables.get("projectinfo.xml") {
        p.properties = project_info(xml);
    }

    for r in records(table("page.eod")) {
        let Some((CLASS_PAGE, id)) = self_ref(r) else { continue };
        let props = user_block(r, F_PROPERTIES);
        p.pages.push(EplanPage {
            id,
            name: text_field(r, F_PAGE_NAME).unwrap_or_else(|| id.to_string()),
            description: pick_language(&prop(&props, P_PAGE_DESCRIPTION))
                .replace(['\r', '\n'], " ")
                .replace("  ", " "),
            scale: prop(&props, P_PAGE_SCALE).replace(',', ".").parse().unwrap_or(1.0),
        });
    }
    p.pages.sort_by_key(|pg| crate_natural(&pg.name));

    for r in records(table("function.eod")) {
        let Some((CLASS_FUNCTION, id)) = self_ref(r) else {
            continue;
        };
        let parts = user_block(r, F_TAG_PARTS);
        let tag = tag_text(
            &prop(&parts, P_TAG_ID),
            &prop(&parts, P_TAG_COUNTER),
            &prop(&parts, P_TAG_SUB_ID),
            &prop(&parts, P_TAG_SUB_COUNTER),
        );
        if tag.is_empty() {
            continue;
        }
        let props = user_block(r, F_PROPERTIES);
        let mut named = BTreeMap::new();
        for (pid, name) in [
            (P_CABLE_TYPE, "Cable type"),
            (P_CABLE_CONDUCTORS, "Conductors"),
            (P_CABLE_CROSS_SECTION, "Cross-section"),
            (P_CABLE_LENGTH, "Length"),
        ] {
            let v = prop(&props, pid);
            if !v.is_empty() {
                named.insert(name.to_owned(), v);
            }
        }
        p.functions.push(EplanFunction {
            id,
            tag,
            // Owned by the page, or by a group (a macro or black box) that sits on the page.
            page: ref_field(r, F_OWNER)
                .filter(|(c, _)| *c == CLASS_PAGE)
                .or_else(|| ref_field(r, F_GRAPHIC_PAGE).filter(|(c, _)| *c == CLASS_PAGE))
                .map(|(_, i)| i),
            at: point_field(r, F_POSITION),
            text: pick_language(&prop(&props, P_FUNCTION_TEXT)),
            technical: prop(&props, P_TECHNICAL),
            parts: props
                .iter()
                .filter(|((k, _), v)| *k == P_PART && !v.is_empty())
                .map(|(_, v)| v.clone())
                .collect(),
            properties: named,
            pins: pins(r),
            kind: u16_field(r, F_FUNCTION_KIND).unwrap_or(0),
            detail: u16_field(r, F_FUNCTION_DETAIL).unwrap_or(0),
            variant: variant(r),
            terminal: prop(&props, P_TERMINAL),
        });
    }

    for r in records(table("articleref.eod")) {
        let Some((CLASS_ARTICLE_REF, _)) = self_ref(r) else {
            continue;
        };
        let tag = tag_text(
            &text_field(r, F_TAG_ID).unwrap_or_default(),
            &text_field(r, F_TAG_COUNTER).unwrap_or_default(),
            &text_field(r, F_TAG_SUB_ID).unwrap_or_default(),
            &text_field(r, F_TAG_SUB_COUNTER).unwrap_or_default(),
        );
        if let Some(part) = text_field(r, F_PART_NUMBER)
            && !tag.is_empty()
        {
            p.article_refs.push((tag, part));
        }
    }

    for r in records(table("article.eod")) {
        let Some((CLASS_ARTICLE, _)) = self_ref(r) else {
            continue;
        };
        let props = user_block(r, F_PROPERTIES);
        let part_number = prop(&props, P_ARTICLE_NUMBER);
        if part_number.is_empty() {
            continue;
        }
        p.articles.push(EplanArticle {
            part_number,
            type_number: prop(&props, P_ARTICLE_TYPE),
            description: pick_language(&prop(&props, P_ARTICLE_DESCRIPTION)),
            manufacturer: prop(&props, P_ARTICLE_MANUFACTURER),
        });
    }

    let mut empty = 0;
    for r in records(table("connection.eod")) {
        let Some((CLASS_CONNECTION, _)) = self_ref(r) else {
            continue;
        };
        let (Some(from), Some(to)) = (text_field(r, F_CONNECTION_FROM), text_field(r, F_CONNECTION_TO)) else {
            empty += 1;
            continue;
        };
        let props = user_block(r, F_PROPERTIES);
        let cable = format!(
            "{}{}",
            text_field(r, F_TAG_ID).unwrap_or_default(),
            text_field(r, F_TAG_COUNTER).unwrap_or_default()
        );
        p.connections.push(EplanConnection {
            page: ref_field(r, F_CONNECTION_PAGE)
                .filter(|(c, _)| *c == CLASS_PAGE)
                .map(|(_, i)| i),
            from,
            to,
            cross_section: prop(&props, P_WIRE_CROSS_SECTION).replace(',', "."),
            colour: prop(&props, P_WIRE_COLOUR),
            cable,
        });
    }
    read_graphics(table("instance.eod"), &mut p);
    if empty > 0 {
        p.warnings
            .push(format!("{empty} connection(s) without named ends were skipped"));
    }
    if p.pages.is_empty() && p.functions.is_empty() {
        p.warnings
            .push("no pages or devices were recognised; the project may use a newer format".into());
    }
    Ok(p)
}

fn u16_field(r: &[u8], id: u16) -> Option<u16> {
    field_offsets(r, id).find_map(|at| u16_at(r, at))
}

fn f64_field(r: &[u8], id: u16, range: std::ops::Range<f64>) -> Option<f64> {
    field_offsets(r, id).find_map(|at| f64_at(r, at).filter(|v| range.contains(v)))
}

/// The symbol variant: a byte field right after the symbol number, absent for variant 0.
fn variant(r: &[u8]) -> u8 {
    let next = F_VARIANT.to_le_bytes();
    field_offsets(r, F_SYMBOL_NUMBER)
        .find_map(|at| {
            (r.get(at + 2..at + 4)? == next.as_slice())
                .then(|| r.get(at + 4).copied())
                .flatten()
        })
        .filter(|v| *v < 8)
        .unwrap_or(0)
}

/// Connection point names: a run of `F_PINS` C strings.
fn pins(r: &[u8]) -> Vec<String> {
    let mark = F_PINS.to_le_bytes();
    let Some(mut at) = field_offsets(r, F_PINS).next() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Some(s) = c_string(r, at) {
        if s.chars().any(char::is_control) || out.len() >= 64 {
            break;
        }
        at += s.len() + 1;
        out.push(s);
        if r.get(at..at + 2) != Some(mark.as_slice()) {
            break;
        }
        at += 2;
    }
    out
}

/// Free graphics and wire nodes of pages (not of forms) from the instance table.
fn read_graphics(data: &[u8], p: &mut EplanProject) {
    use wc_core::geom::{Align, Shape};
    for r in records(data) {
        let Some((class, _)) = self_ref(r) else { continue };
        let Some((CLASS_PAGE, page)) = ref_field(r, F_GRAPHIC_PAGE) else {
            continue;
        };
        let shape = match class {
            CLASS_LINE => match (point_field(r, F_LINE_FROM), point_field(r, F_LINE_TO)) {
                (Some(a), Some(b)) => Shape::Line { a, b },
                _ => continue,
            },
            CLASS_RECT => match (point_field(r, F_RECT_FROM), point_field(r, F_RECT_TO)) {
                (Some(a), Some(b)) => Shape::Rect {
                    min: Point::new(a.x.min(b.x), a.y.min(b.y)),
                    max: Point::new(a.x.max(b.x), a.y.max(b.y)),
                },
                _ => continue,
            },
            CLASS_CIRCLE => match (
                point_field(r, F_CIRCLE_CENTER),
                f64_field(r, F_CIRCLE_RADIUS, 0.01..1e5),
            ) {
                (Some(center), Some(radius)) => Shape::Circle {
                    center,
                    radius,
                    filled: false,
                },
                _ => continue,
            },
            CLASS_TEXT => {
                let (Some(at), Some(text)) = (point_field(r, F_TEXT_AT), text_field(r, F_TEXT)) else {
                    continue;
                };
                let text = pick_language(&text);
                let height = f64_field(r, F_TEXT_HEIGHT, 0.5..50.0).unwrap_or(2.5);
                // Radians, counter-clockwise; whole degrees are all a drawing needs.
                let angle = f64_field(r, F_TEXT_ANGLE, -7.0..7.0)
                    .map(|a| a.to_degrees().round())
                    .unwrap_or(0.0);
                let (sin, cos) = angle.to_radians().sin_cos();
                // One text per line, each line below the last (Eplan's y runs up).
                for (k, line) in text.lines().enumerate() {
                    let line = line.trim_end_matches('\r');
                    if line.trim().is_empty() {
                        continue;
                    }
                    p.graphics.push(EplanGraphic {
                        page,
                        shape: Shape::Text {
                            // Each line one line-height further, square to the reading direction.
                            at: Point::new(
                                at.x + 1.5 * height * k as f64 * sin,
                                at.y - 1.5 * height * k as f64 * cos,
                            ),
                            text: line.to_owned(),
                            height,
                            align: Align::Left,
                            angle,
                        },
                    });
                }
                continue;
            }
            CLASS_NODE => {
                if let (Some(at), Some(symbol)) = (point_field(r, F_POSITION), u16_field(r, F_SYMBOL_NUMBER)) {
                    p.nodes.push(EplanNode {
                        page,
                        at,
                        symbol,
                        variant: variant(r),
                    });
                }
                continue;
            }
            _ => continue,
        };
        p.graphics.push(EplanGraphic { page, shape });
    }
}

fn crate_natural(s: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let (mut text, mut num) = (String::new(), String::new());
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            if !num.is_empty() {
                out.push((std::mem::take(&mut text), num.parse().unwrap_or(u64::MAX)));
                num.clear();
            }
            text.push(c);
        }
    }
    out.push((
        text,
        if num.is_empty() {
            0
        } else {
            num.parse().unwrap_or(u64::MAX)
        },
    ));
    out
}

/// Split a connection end into device and pin: `X2:3:b` -> (`X2`, `3:b`). A bare name has no pin.
pub fn split_end(end: &str) -> (&str, Option<&str>) {
    match end.split_once(':') {
        Some((d, pin)) if !d.is_empty() && !pin.is_empty() => (d, Some(pin)),
        _ => (end, None),
    }
}

/// A WiringCraft tag for an Eplan device name: `PUMPE1-M1` -> `-PUMPE1.M1`. Characters a tag
/// cannot hold become `_`.
pub fn device_tag(name: &str) -> Option<DeviceTag> {
    let levels: Vec<String> = name
        .split('-')
        .filter(|l| !l.is_empty())
        .map(|l| {
            l.chars()
                .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
                .collect()
        })
        .collect();
    if levels.is_empty() {
        return None;
    }
    DeviceTag::parse(&format!("-{}", levels.join("."))).ok()
}

/// Lay the project out as connection overview pages: one section per Eplan page holding the
/// devices first drawn there, devices with their part data, and every connection between two
/// device pins as a wire (labelled with colour and cross-section).
pub fn build(p: &EplanProject, first_page_number: usize) -> (aml::Built, Vec<String>) {
    let mut warnings = Vec::new();
    // Devices by name, in first-seen order.
    let mut names: Vec<String> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    let mut add = |n: &str, names: &mut Vec<String>| -> usize {
        *index.entry(n.to_owned()).or_insert_with(|| {
            names.push(n.to_owned());
            names.len() - 1
        })
    };
    let mut page_of: BTreeMap<usize, u32> = BTreeMap::new();
    for f in &p.functions {
        let i = add(&f.tag, &mut names);
        if let Some(pg) = f.page {
            page_of.entry(i).or_insert(pg);
        }
    }
    let mut pins: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    let mut links = Vec::new();
    let mut skipped = 0;
    for c in &p.connections {
        let (da, pa) = split_end(&c.from);
        let (db, pb) = split_end(&c.to);
        let (Some(pa), Some(pb)) = (pa, pb) else {
            skipped += 1;
            continue;
        };
        let (ia, ib) = (add(da, &mut names), add(db, &mut names));
        if let Some(pg) = c.page {
            page_of.entry(ia).or_insert(pg);
            page_of.entry(ib).or_insert(pg);
        }
        pins.entry(ia).or_default().insert(pa.to_owned());
        pins.entry(ib).or_default().insert(pb.to_owned());
        // Colour and cross-section go to the wire's own fields, not into its number.
        links.push(AmlLink {
            name: String::new(),
            a: (ia, pa.to_owned()),
            b: (ib, pb.to_owned()),
            spec: c.spec(),
        });
    }
    if skipped > 0 {
        warnings.push(format!(
            "{skipped} connection(s) to a name without a pin (potentials, cable ends) are listed in the device \
             properties only"
        ));
    }

    // Article data by part number, from the project's cache.
    let article: BTreeMap<&str, &EplanArticle> = p.articles.iter().map(|a| (a.part_number.as_str(), a)).collect();
    let mut parts_of: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for f in &p.functions {
        parts_of
            .entry(f.tag.as_str())
            .or_default()
            .extend(f.parts.iter().cloned());
    }
    for (tag, part) in &p.article_refs {
        parts_of.entry(tag.as_str()).or_default().push(part.clone());
    }

    let mut devices = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let mut properties = BTreeMap::new();
        let funcs: Vec<&EplanFunction> = p.functions.iter().filter(|f| &f.tag == name).collect();
        let texts: BTreeSet<&str> = funcs
            .iter()
            .map(|f| f.text.as_str())
            .filter(|t| !t.is_empty())
            .collect();
        if !texts.is_empty() {
            properties.insert(
                "Function text".to_owned(),
                texts.into_iter().collect::<Vec<_>>().join(" / "),
            );
        }
        let tech: BTreeSet<&str> = funcs
            .iter()
            .map(|f| f.technical.as_str())
            .filter(|t| !t.is_empty())
            .collect();
        if !tech.is_empty() {
            properties.insert(
                "Technical data".to_owned(),
                tech.into_iter().collect::<Vec<_>>().join(" / "),
            );
        }
        for f in &funcs {
            properties.extend(f.properties.clone());
        }
        let mut parts: Vec<String> = Vec::new();
        for part in parts_of.get(name.as_str()).into_iter().flatten() {
            if !parts.contains(part) {
                parts.push(part.clone());
            }
        }
        if let Some(first) = parts.first() {
            properties.insert("PartNumber".to_owned(), first.clone());
            let manufacturer = article
                .get(first.as_str())
                .map(|a| a.manufacturer.clone())
                .filter(|m| !m.is_empty())
                .or_else(|| first.split_once('.').map(|(m, _)| m.to_owned()));
            if let Some(m) = manufacturer {
                properties.insert("Manufacturer".to_owned(), m);
            }
            if let Some(a) = article.get(first.as_str()) {
                if !a.description.is_empty() {
                    properties.insert("Part description".to_owned(), a.description.clone());
                }
                if !a.type_number.is_empty() {
                    properties.insert("Type number".to_owned(), a.type_number.clone());
                }
            }
        }
        if parts.len() > 1 {
            properties.insert("Additional parts".to_owned(), parts[1..].join(", "));
        }
        let mut interfaces: Vec<String> = pins.get(&i).map(|s| s.iter().cloned().collect()).unwrap_or_default();
        interfaces.sort_by_key(|s| crate_natural(s));
        let tag = device_tag(name);
        if tag.is_none() {
            warnings.push(format!("device {name:?} has no usable tag"));
        }
        devices.push(AmlDevice {
            name: name.clone(),
            path: Vec::new(),
            tag,
            properties,
            interfaces,
        });
    }

    // Pages as drawn first; a connection overview for devices not placed on any of them.
    let drawing = drawing::pages(p, first_page_number);
    warnings.extend(drawing.warnings.iter().cloned());
    let overview: Vec<usize> = (0..names.len())
        .filter(|i| !drawing.drawn.contains(&names[*i]))
        .collect();
    let mut sections: Vec<Section> = Vec::new();
    for pg in &p.pages {
        let devs: Vec<usize> = overview
            .iter()
            .copied()
            .filter(|i| page_of.get(i) == Some(&pg.id))
            .collect();
        if devs.is_empty() {
            continue;
        }
        let title = if pg.description.is_empty() {
            format!("Connections, Eplan page {}", pg.name)
        } else {
            format!("Connections, Eplan page {}: {}", pg.name, pg.description)
        };
        sections.push(Section { title, devices: devs });
    }
    let known: BTreeSet<u32> = p.pages.iter().map(|pg| pg.id).collect();
    let rest: Vec<usize> = overview
        .iter()
        .copied()
        .filter(|i| page_of.get(i).is_none_or(|pg| !known.contains(pg)))
        .collect();
    if !rest.is_empty() {
        sections.push(Section {
            title: "Eplan devices on layout and report pages".into(),
            devices: rest,
        });
    }
    let dropped = links
        .iter()
        .filter(|l| drawing.drawn.contains(&names[l.a.0]) || drawing.drawn.contains(&names[l.b.0]))
        .count();
    if dropped > 0 {
        warnings.push(format!(
            "{dropped} connection(s) of the connection list to devices on drawn pages were left out; their wires are \
             traced from the drawing"
        ));
    }
    let imp = AmlImport {
        devices,
        links,
        warnings: Vec::new(),
        locations: Vec::new(),
    };
    let mut built = aml::layout(&imp, &sections, "eplan", first_page_number + drawing.pages.len());
    for d in imp.devices.iter().filter(|d| drawing.drawn.contains(&d.name)) {
        if let Some((tag, device)) = aml::device_entry(d, &mut built.parts) {
            built.devices.entry(tag).or_insert(device);
        }
    }
    let mut pages = drawing.pages;
    pages.append(&mut built.pages);
    built.pages = pages;
    built.symbols.extend(drawing.symbols);
    (built, warnings)
}

pub mod drawing;

#[cfg(test)]
mod tests;

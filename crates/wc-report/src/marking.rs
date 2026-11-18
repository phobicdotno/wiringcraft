//! Marker lists for printing marking strips, written for WAGO smartScript's spreadsheet import.
//!
//! smartScript imports marker texts from an Excel or CSV file: each row is one marker, and the
//! columns are mapped to marker fields in the import dialog (WAGO, "smartScript" marking
//! software, import of Excel/CSV data). This writes such a file: one row per marker, with
//! the marker type, the group it belongs to (terminal strip, page or location), the marker
//! text and how many copies to print (a wire or a cable gets one marker at each end).
//!
//! The file is semicolon-separated UTF-8 with a byte-order mark and CRLF line ends, which is
//! how Excel and smartScript read CSV files in European locales without garbling `æøå` or
//! splitting decimal commas. Rows are sorted in natural tag order (`-X2` before `-X10`).
//!
//! WAGO's add-in for EPLAN also offers template files for smartScript's MultiImport; those are
//! not written here (their layout is not publicly documented).

use std::collections::{BTreeMap, BTreeSet};

use wc_core::DeviceTag;
use wc_core::symbol::SymbolRole;

use crate::{Inputs, Table, natural_key};

/// What kind of marker a row is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MarkerKind {
    Terminals,
    Wires,
    Devices,
    Cables,
}

impl MarkerKind {
    pub const ALL: [MarkerKind; 4] = [
        MarkerKind::Terminals,
        MarkerKind::Wires,
        MarkerKind::Devices,
        MarkerKind::Cables,
    ];

    pub fn name(self) -> &'static str {
        match self {
            MarkerKind::Terminals => "terminals",
            MarkerKind::Wires => "wires",
            MarkerKind::Devices => "devices",
            MarkerKind::Cables => "cables",
        }
    }

    /// The marker type as written in the file's first column.
    pub(crate) fn label(self, lang: Language) -> &'static str {
        use Language::*;
        match (self, lang) {
            (MarkerKind::Terminals, En) => "Terminal",
            (MarkerKind::Wires, En) => "Wire",
            (MarkerKind::Devices, En) => "Device",
            (MarkerKind::Cables, En) => "Cable",
            (MarkerKind::Terminals, De) => "Klemme",
            (MarkerKind::Wires, De) => "Leiter",
            (MarkerKind::Devices, De) => "Betriebsmittel",
            (MarkerKind::Cables, De) => "Kabel",
            (MarkerKind::Terminals, Nb) => "Klemme",
            (MarkerKind::Wires, Nb) => "Leder",
            (MarkerKind::Devices, Nb) => "Komponent",
            (MarkerKind::Cables, Nb) => "Kabel",
        }
    }

    pub fn parse(s: &str) -> Option<MarkerKind> {
        let s = s.trim().to_ascii_lowercase();
        MarkerKind::ALL
            .into_iter()
            .find(|k| k.name() == s || k.name().strip_suffix('s') == Some(s.as_str()))
    }
}

/// The language of the header row and of the marker type column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    En,
    De,
    Nb,
}

impl Language {
    pub const ALL: [Language; 3] = [Language::En, Language::De, Language::Nb];

    /// The code used on the command line (`en`, `de`, `nb`).
    pub fn code(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::De => "de",
            Language::Nb => "nb",
        }
    }

    /// The language's own name, for a menu.
    pub fn name(self) -> &'static str {
        match self {
            Language::En => "English",
            Language::De => "Deutsch",
            Language::Nb => "Norsk",
        }
    }

    /// A language by its code; `no` is taken as Norwegian Bokmål too.
    pub fn parse(s: &str) -> Option<Language> {
        match s.trim().to_ascii_lowercase().as_str() {
            "en" => Some(Language::En),
            "de" => Some(Language::De),
            "nb" | "no" => Some(Language::Nb),
            _ => None,
        }
    }

    /// Header words: type, group, marker, line (followed by its number), count.
    fn words(self) -> [&'static str; 5] {
        match self {
            Language::En => ["Type", "Group", "Marker", "Line", "Count"],
            Language::De => ["Typ", "Gruppe", "Beschriftung", "Zeile", "Anzahl"],
            Language::Nb => ["Type", "Gruppe", "Merking", "Linje", "Antall"],
        }
    }
}

/// What one line of a marker shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineField {
    /// The marker text: a device's or cable's engraving text (its [`MARKER_PROPERTY`]) when it
    /// has one, otherwise its tag; a terminal's number; a wire's number.
    Marker,
    /// The tag itself, ignoring any engraving text (a terminal shows its strip).
    Tag,
    /// The device's description (a terminal: its strip's description); empty for wires.
    Description,
    /// The location part of the tag (`+C1`); empty for wires.
    Location,
}

impl LineField {
    pub const ALL: [LineField; 4] = [
        LineField::Marker,
        LineField::Tag,
        LineField::Description,
        LineField::Location,
    ];

    pub fn name(self) -> &'static str {
        match self {
            LineField::Marker => "marker",
            LineField::Tag => "tag",
            LineField::Description => "description",
            LineField::Location => "location",
        }
    }

    pub fn parse(s: &str) -> Option<LineField> {
        let s = s.trim().to_ascii_lowercase();
        LineField::ALL.into_iter().find(|f| f.name() == s)
    }
}

/// The device property holding a marker (engraving) text that differs from the tag, such as a
/// customer's own name for a device. Devices and cables without it are marked with their tag.
pub const MARKER_PROPERTY: &str = "Marker";

/// What a marker list holds and how it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkingOptions {
    /// The kinds of marker; empty means all of them (or only terminals with `strip`).
    pub kinds: Vec<MarkerKind>,
    /// The lines of each marker, one column each; empty means one line with the marker text.
    pub lines: Vec<LineField>,
    pub language: Language,
    /// Only this terminal strip's terminals (as the terminal plan names it, e.g. `-X1`).
    pub strip: Option<String>,
    /// Written between the numbers of a multi-level terminal's levels on its one marker
    /// (see [`crate::LEVEL_PROPERTY`]).
    pub level_separator: String,
}

/// The default [`MarkingOptions::level_separator`].
pub const LEVEL_SEPARATOR: &str = " / ";

impl Default for MarkingOptions {
    fn default() -> Self {
        Self {
            kinds: Vec::new(),
            lines: vec![LineField::Marker],
            language: Language::En,
            strip: None,
            level_separator: LEVEL_SEPARATOR.to_owned(),
        }
    }
}

/// The marker list for the chosen kinds (all of them when `kinds` is empty), in the order
/// terminals, wires, devices, cables, each sorted in natural order.
pub fn marking_list(i: &Inputs<'_>, kinds: &[MarkerKind]) -> Table {
    marking_list_with(
        i,
        &MarkingOptions {
            kinds: kinds.to_vec(),
            ..MarkingOptions::default()
        },
    )
}

/// The terminal strips that have numbered terminals, in natural order: what
/// [`MarkingOptions::strip`] can name.
pub fn terminal_strips(i: &Inputs<'_>) -> Vec<String> {
    let mut strips: Vec<String> = terminal_rows(i).into_iter().map(|(s, _, _)| s).collect();
    // `-X1` and `-X01` sort alike, so their rows can interleave: order them exactly first.
    strips.sort_by(|a, b| natural_key(a).cmp(&natural_key(b)).then_with(|| a.cmp(b)));
    strips.dedup();
    strips
}

/// (strip, number, level) of every numbered terminal, sorted and without repeats. The level is
/// the terminal plan's: set on the levels of a multi-level terminal.
fn terminal_rows(i: &Inputs<'_>) -> Vec<(String, String, Option<u32>)> {
    // The terminal plan already numbers terminals and sorts strips and numbers. A spare
    // without a level of its own (terminal strip editor) is in no multi-level block, so it
    // goes after its strip's other terminals and never splits a block's marker.
    let (plan, spares) = crate::terminal_plan_with_spares(i);
    let mut rows: Vec<(String, bool, String, Option<u32>)> = plan
        .rows
        .into_iter()
        .enumerate()
        .filter_map(|(k, r)| match (r.first(), r.get(1)) {
            (Some(strip), Some(number)) if strip != "(untagged)" => {
                let level = r.get(7).and_then(|l| l.parse().ok());
                let loose = level.is_none() && spares.contains(&k);
                Some((strip.clone(), loose, number.clone(), level))
            }
            _ => None,
        })
        .collect();
    rows.sort_by(|a, b| {
        // `-X1` and `-X01` have equal keys: settle the exact strip before the numbers, so one
        // strip's terminals stay together.
        natural_key(&a.0)
            .cmp(&natural_key(&b.0))
            .then_with(|| a.0.cmp(&b.0))
            .then(a.1.cmp(&b.1))
            .then(natural_key(&a.2).cmp(&natural_key(&b.2)))
            // `1` and `01` have equal keys: order them exactly too, so `dedup` sees
            // repeats side by side.
            .then_with(|| a.2.cmp(&b.2))
    });
    rows.dedup_by(|a, b| a.0 == b.0 && a.2 == b.2);
    rows.into_iter().map(|(s, _, n, l)| (s, n, l)).collect()
}

/// The terminal markers of `rows` (from [`terminal_rows`]) as (strip, marker text): one per
/// terminal block, so the levels of a multi-level terminal share one marker with their numbers
/// joined by `separator` (`1 / 2`). A level of 2 or more joins the terminal before it in the
/// same strip when that one is a lower level.
pub(crate) fn terminal_markers(rows: Vec<(String, String, Option<u32>)>, separator: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut last: Option<(String, u32)> = None;
    for (strip, number, level) in rows {
        let joins = match (&last, level) {
            (Some((s, below)), Some(l)) => *s == strip && l >= 2 && l > *below,
            _ => false,
        };
        match out.last_mut() {
            Some((_, marker)) if joins => {
                marker.push_str(separator);
                marker.push_str(&number);
            }
            _ => out.push((strip.clone(), number)),
        }
        last = level.map(|l| (strip, l));
    }
    out
}

/// The marker list as `o` asks for it: columns type, group, one per line, count.
pub fn marking_list_with(i: &Inputs<'_>, o: &MarkingOptions) -> Table {
    let [w_type, w_group, w_marker, w_line, w_count] = o.language.words();
    let lines: Vec<LineField> = if o.lines.is_empty() {
        vec![LineField::Marker]
    } else {
        o.lines.clone()
    };
    let mut columns: Vec<String> = vec![w_type.into(), w_group.into()];
    if lines.len() == 1 {
        columns.push(w_marker.into());
    } else {
        columns.extend((1..=lines.len()).map(|n| format!("{w_line} {n}")));
    }
    columns.push(w_count.into());
    let mut t = Table::new("Marking list", &[]);
    t.columns = columns;
    let wanted = |k: MarkerKind| {
        if o.kinds.is_empty() {
            o.strip.is_none() || k == MarkerKind::Terminals
        } else {
            o.kinds.contains(&k)
        }
    };
    let description = |tag: &str| -> String {
        tag.parse::<DeviceTag>()
            .ok()
            .and_then(|t| i.project.devices.get(&t))
            .map(|d| d.description.clone())
            .unwrap_or_default()
    };
    let engraving = |tag: &DeviceTag| -> Option<String> {
        i.project
            .devices
            .get(tag)
            .and_then(|d| d.properties.get(MARKER_PROPERTY))
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty())
    };
    // One marker row: `marker` is the marker text, `tag` the tag (or strip, or number).
    let mut push = |k: MarkerKind, group: &str, marker: &str, tag: &str, location: &str, count: usize| {
        let mut r = vec![k.label(o.language).to_owned(), group.to_owned()];
        for f in &lines {
            r.push(match (f, k) {
                (LineField::Marker, _) => marker.to_owned(),
                (LineField::Tag, _) => tag.to_owned(),
                (LineField::Description, MarkerKind::Wires) => String::new(),
                (LineField::Description, _) => description(tag),
                (LineField::Location, _) => location.to_owned(),
            });
        }
        r.push(count.to_string());
        t.rows.push(r);
    };

    if wanted(MarkerKind::Terminals) {
        for (strip, number) in terminal_markers(terminal_rows(i), &o.level_separator) {
            if o.strip.as_ref().is_some_and(|s| s.trim() != strip) {
                continue;
            }
            let location = strip.parse::<DeviceTag>().map(|t| location_of(&t)).unwrap_or_default();
            push(MarkerKind::Terminals, &strip, &number, &strip, &location, 1);
        }
    }

    let cables: BTreeSet<DeviceTag> = i
        .project
        .pages
        .iter()
        .flat_map(|p| p.wires.iter())
        .filter_map(|w| w.cable.as_ref().map(|c| c.cable.clone()))
        .collect();

    if wanted(MarkerKind::Wires) {
        // One marker at each end of every numbered wire; wires of one connection share a
        // number, so a number's count is the number of wire ends that carry it.
        let mut ends: BTreeMap<String, usize> = BTreeMap::new();
        for page in &i.project.pages {
            for w in &page.wires {
                if let Some(n) = w.number.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                    *ends.entry(n.to_owned()).or_default() += 2;
                }
            }
        }
        let mut list: Vec<(String, usize)> = ends.into_iter().collect();
        list.sort_by_key(|(n, _)| natural_key(n));
        for (n, count) in list {
            push(MarkerKind::Wires, "", &n, &n, "", count);
        }
    }

    // Terminals have terminal markers: the drawn ones and the spares the terminal strip editor
    // keeps (a spare that is drawn too is a drawn terminal already). Only the device and cable
    // markers need them.
    let mut terminals: BTreeSet<DeviceTag> = BTreeSet::new();
    if wanted(MarkerKind::Devices) || wanted(MarkerKind::Cables) {
        for l in &i.project.meta.strips {
            for n in &l.spares {
                if let Ok(tag) = DeviceTag::parse(&format!("{}.{n}", l.strip)) {
                    terminals.insert(tag);
                }
            }
        }
        for page in &i.project.pages {
            for s in &page.symbols {
                let role = i.symbols.get(&s.symbol).map(|y| y.role);
                if matches!(role, Some(SymbolRole::Terminal))
                    && let Some(tag) = &s.tag
                {
                    terminals.insert(tag.clone());
                }
            }
        }
    }

    if wanted(MarkerKind::Devices) {
        // Devices are marked by their tag; terminals and cables have markers of their own,
        // and interruption points (links between pages) are not devices.
        let mut skip: BTreeSet<DeviceTag> = cables.clone();
        skip.extend(terminals.iter().cloned());
        let mut list: Vec<DeviceTag> = crate::all_tags(i)
            .into_iter()
            .filter(|t| !skip.contains(t) && !is_cable_tag(t))
            .collect();
        list.sort_by_key(|t| natural_key(&t.to_string()));
        for tag in list {
            let text = tag.to_string();
            let location = location_of(&tag);
            let marker = engraving(&tag).unwrap_or_else(|| text.clone());
            push(MarkerKind::Devices, &location, &marker, &text, &location, 1);
        }
    }

    if wanted(MarkerKind::Cables) {
        // Cables drawn as cores of wires, and cable devices listed without cores yet; a
        // terminal named like a cable (-X1.W1) is a terminal.
        let mut list: BTreeSet<DeviceTag> = cables;
        list.extend(
            crate::all_tags(i)
                .into_iter()
                .filter(|t| is_cable_tag(t) && !terminals.contains(t)),
        );
        list.extend(
            i.project
                .devices
                .keys()
                .filter(|t| is_cable_tag(t) && !terminals.contains(*t))
                .cloned(),
        );
        let mut list: Vec<DeviceTag> = list.into_iter().collect();
        list.sort_by_key(|t| natural_key(&t.to_string()));
        for tag in list {
            let text = tag.to_string();
            let location = location_of(&tag);
            let marker = engraving(&tag).unwrap_or_else(|| text.clone());
            push(MarkerKind::Cables, &location, &marker, &text, &location, 2);
        }
    }
    t
}

/// The location part of a tag (`+C1` of `=A+C1-K1`), or nothing.
fn location_of(tag: &DeviceTag) -> String {
    if tag.location.is_empty() {
        String::new()
    } else {
        format!("+{}", tag.location.join("."))
    }
}

/// Whether a device tag names a cable by its IEC 81346-2 letter code (`-W1`, `-WD3`).
fn is_cable_tag(tag: &DeviceTag) -> bool {
    tag.product.last().is_some_and(|p| {
        p.strip_prefix('W').is_some_and(|rest| {
            rest.chars()
                .next()
                .is_none_or(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        })
    })
}

/// Warnings for marker texts that a spreadsheet could run as a formula: those starting with `=`,
/// `+`, `-` or `@` (CWE-1236). The file keeps them as they are, since smartScript prints the text
/// on the marker; the warning lets the user change the text before opening the file in a
/// spreadsheet. Every text column is checked (all marker lines, not the type, group or count).
/// A wire number is always warned about. On other rows a text that is exactly a device tag
/// (`-K1`, `=A+C1-K1`) or a terminal number starts with these by design and gives none; free text
/// such as a `Marker` property or a description does.
pub fn formula_warnings(t: &Table) -> Vec<String> {
    let is_wire = |r: &Vec<String>| {
        Language::ALL
            .iter()
            .any(|l| r.first().map(String::as_str) == Some(MarkerKind::Wires.label(*l)))
    };
    let mut out = Vec::new();
    for r in &t.rows {
        let wire = is_wire(r);
        let texts = r.get(2..r.len().saturating_sub(1)).unwrap_or_default();
        let mut seen: Vec<&str> = Vec::new();
        for text in texts {
            let Some(c) = text.chars().next().filter(|c| ['=', '+', '-', '@'].contains(c)) else {
                continue;
            };
            if seen.contains(&text.as_str()) {
                continue;
            }
            seen.push(text);
            if wire {
                out.push(format!(
                    "wire number {text:?} starts with {c:?}: a spreadsheet opening the file may run it as a \
                     formula (smartScript prints it as it is)"
                ));
            } else if !is_tag(text) {
                out.push(format!(
                    "marker text {text:?} starts with {c:?}: a spreadsheet opening the file may run it as a \
                     formula (smartScript prints it as it is)"
                ));
            }
        }
    }
    out
}

/// Whether `s` is written exactly as a device tag is (`-K1`, `=A+C1-K1`, a number such as `-1`).
fn is_tag(s: &str) -> bool {
    DeviceTag::parse(s).is_ok_and(|t| t.to_string() == s)
}

/// The marker list as smartScript reads it: semicolon-separated UTF-8 with a byte-order mark,
/// a header row and CRLF line ends. Fields are quoted when they contain `;`, `"` or a line end.
pub fn to_smartscript_csv(t: &Table) -> String {
    let mut out = String::from('\u{feff}');
    out.push_str(&t.to_csv_with(';'));
    out
}

/// Parse marker kind names (`terminals`, `wires`, `devices`, `cables`, or singular); an
/// unknown name is an error that lists the known ones.
pub fn parse_kinds(names: &[String]) -> Result<Vec<MarkerKind>, String> {
    names
        .iter()
        .map(|k| {
            MarkerKind::parse(k).ok_or_else(|| format!("unknown marker kind {k:?} (terminals, wires, devices, cables)"))
        })
        .collect()
}

/// Parse line field names (`marker`, `tag`, `description`, `location`).
pub fn parse_lines(names: &[String]) -> Result<Vec<LineField>, String> {
    names
        .iter()
        .map(|f| {
            LineField::parse(f).ok_or_else(|| format!("unknown marker line {f:?} (marker, tag, description, location)"))
        })
        .collect()
}

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

use crate::{Inputs, Table, natural_key, terminal_plan};

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
    fn label(self) -> &'static str {
        match self {
            MarkerKind::Terminals => "Terminal",
            MarkerKind::Wires => "Wire",
            MarkerKind::Devices => "Device",
            MarkerKind::Cables => "Cable",
        }
    }

    pub fn parse(s: &str) -> Option<MarkerKind> {
        let s = s.trim().to_ascii_lowercase();
        MarkerKind::ALL
            .into_iter()
            .find(|k| k.name() == s || k.name().strip_suffix('s') == Some(s.as_str()))
    }
}

/// The marker list for the chosen kinds (all of them when `kinds` is empty), in the order
/// terminals, wires, devices, cables, each sorted in natural order.
pub fn marking_list(i: &Inputs<'_>, kinds: &[MarkerKind]) -> Table {
    let mut t = Table::new("Marking list", &["Type", "Group", "Marker", "Count"]);
    let wanted = |k: MarkerKind| kinds.is_empty() || kinds.contains(&k);
    let row = |k: MarkerKind, group: &str, text: &str, count: usize| {
        vec![
            k.label().to_owned(),
            group.to_owned(),
            text.to_owned(),
            count.to_string(),
        ]
    };

    if wanted(MarkerKind::Terminals) {
        // The terminal plan already numbers terminals and sorts strips and numbers.
        let mut rows: Vec<(String, String)> = terminal_plan(i)
            .rows
            .into_iter()
            .filter_map(|r| match (r.first(), r.get(1)) {
                (Some(strip), Some(number)) if strip != "(untagged)" => Some((strip.clone(), number.clone())),
                _ => None,
            })
            .collect();
        rows.sort_by(|a, b| {
            natural_key(&a.0)
                .cmp(&natural_key(&b.0))
                .then(natural_key(&a.1).cmp(&natural_key(&b.1)))
                // `1` and `01` have equal keys: order them exactly too, so `dedup` sees
                // repeats side by side.
                .then_with(|| a.cmp(b))
        });
        rows.dedup();
        for (strip, number) in rows {
            t.rows.push(row(MarkerKind::Terminals, &strip, &number, 1));
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
            t.rows.push(row(MarkerKind::Wires, "", &n, count));
        }
    }

    if wanted(MarkerKind::Devices) {
        // Devices are marked by their tag; terminals and cables have markers of their own,
        // and interruption points (links between pages) are not devices.
        let mut skip: BTreeSet<DeviceTag> = cables.clone();
        for page in &i.project.pages {
            for s in &page.symbols {
                let role = i.symbols.get(&s.symbol).map(|y| y.role);
                if matches!(role, Some(SymbolRole::Terminal))
                    && let Some(tag) = &s.tag
                {
                    skip.insert(tag.clone());
                }
            }
        }
        let mut list: Vec<DeviceTag> = crate::all_tags(i)
            .into_iter()
            .filter(|t| !skip.contains(t) && !is_cable_tag(t))
            .collect();
        list.sort_by_key(|t| natural_key(&t.to_string()));
        for tag in list {
            t.rows
                .push(row(MarkerKind::Devices, &location_of(&tag), &tag.to_string(), 1));
        }
    }

    if wanted(MarkerKind::Cables) {
        // Cables drawn as cores of wires, and cable devices listed without cores yet.
        let mut list: BTreeSet<DeviceTag> = cables;
        list.extend(crate::all_tags(i).into_iter().filter(is_cable_tag));
        list.extend(i.project.devices.keys().filter(|t| is_cable_tag(t)).cloned());
        let mut list: Vec<DeviceTag> = list.into_iter().collect();
        list.sort_by_key(|t| natural_key(&t.to_string()));
        for tag in list {
            t.rows
                .push(row(MarkerKind::Cables, &location_of(&tag), &tag.to_string(), 2));
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

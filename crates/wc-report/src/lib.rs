//! Reports generated from a project: parts list, device list, wire list, net list and terminal
//! plan. Each report is a [`Table`] that can be written as CSV or Markdown, or laid out on PDF
//! report pages.
//!
//! Reports are always derived from the schematic; they are never stored, so they can never be
//! out of date.

use std::collections::{BTreeMap, BTreeSet};

use wc_core::connectivity::{self, PinRef};
use wc_core::symbol::SymbolRole;
use wc_core::{DeviceTag, Endpoint, Part, Project, Symbol};

pub mod aml;
pub mod check;
pub mod marking;
pub mod plc;
pub mod step;
#[cfg(test)]
mod tests;

pub use aml::{AmlCounts, automationml};
pub use check::{Finding, Severity, check, check_table};
pub use marking::{MarkerKind, marking_list, parse_kinds, to_smartscript_csv};
pub use plc::{PlcVariables, plc_variables};

/// A finished report: a title, column headings and rows of text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Table {
    pub title: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// For each row, the id of the page it describes (empty when it describes none); a PDF
    /// export makes such rows clickable. Shorter than `rows` when the last rows have none.
    pub row_pages: Vec<String>,
    /// Printed before the schematic pages in a PDF, like the table of contents of a drawing set.
    pub front: bool,
}

impl Table {
    fn new(title: &str, columns: &[&str]) -> Self {
        Self {
            title: title.into(),
            columns: columns.iter().map(|c| (*c).to_owned()).collect(),
            ..Self::default()
        }
    }

    /// RFC 4180 CSV with a header row; fields are quoted when they need it.
    pub fn to_csv(&self) -> String {
        self.to_csv_with(',')
    }

    /// CSV separated by `sep` (`,` or `;`), with a header row and CRLF line ends; fields are
    /// quoted when they contain the separator, a quote or a line end.
    pub fn to_csv_with(&self, sep: char) -> String {
        let field = |s: &str| {
            if s.contains([sep, '"', '\n', '\r']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_owned()
            }
        };
        let line = |cells: &[String]| {
            cells
                .iter()
                .map(|c| field(c))
                .collect::<Vec<_>>()
                .join(&sep.to_string())
        };
        let mut out = line(&self.columns);
        out.push_str("\r\n");
        for r in &self.rows {
            out.push_str(&line(r));
            out.push_str("\r\n");
        }
        out
    }

    /// A Markdown table under a level-2 heading.
    pub fn to_markdown(&self) -> String {
        let cell = |s: &str| s.replace('|', "\\|").replace('\n', " ");
        let mut out = format!(
            "## {}\n\n| {} |\n|{}|\n",
            self.title,
            self.columns.join(" | "),
            "---|".repeat(self.columns.len())
        );
        for r in &self.rows {
            out.push_str(&format!(
                "| {} |\n",
                r.iter().map(|c| cell(c)).collect::<Vec<_>>().join(" | ")
            ));
        }
        out
    }
}

/// Which report to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Parts,
    Devices,
    Wires,
    Nets,
    Terminals,
    Cables,
    Plc,
    Contents,
}

impl Kind {
    pub const ALL: [Kind; 8] = [
        Kind::Contents,
        Kind::Parts,
        Kind::Devices,
        Kind::Wires,
        Kind::Nets,
        Kind::Terminals,
        Kind::Cables,
        Kind::Plc,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Parts => "parts",
            Kind::Devices => "devices",
            Kind::Wires => "wires",
            Kind::Nets => "nets",
            Kind::Terminals => "terminals",
            Kind::Cables => "cables",
            Kind::Plc => "plc",
            Kind::Contents => "contents",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == s)
    }
}

/// Everything a report may need besides the project.
pub struct Inputs<'a> {
    pub project: &'a Project,
    /// The project's vendored symbols by id.
    pub symbols: &'a BTreeMap<String, Symbol>,
    /// The project's vendored parts by id.
    pub parts: &'a BTreeMap<String, Part>,
}

pub fn build(kind: Kind, i: &Inputs<'_>) -> Table {
    match kind {
        Kind::Parts => parts_list(i),
        Kind::Devices => device_list(i),
        Kind::Wires => wire_list(i),
        Kind::Nets => net_list(i),
        Kind::Terminals => terminal_plan(i),
        Kind::Cables => cable_list(i),
        Kind::Plc => plc_io_list(i),
        Kind::Contents => contents(i),
    }
}

pub use wc_core::project::natural_key;

/// Every device tag in the project: drawn ones and ones only listed in `devices.json`.
/// Every real device: tags drawn on pages or listed in the devices file. Interruption points
/// carry tags too, but they only name a connection, so a tag used by nothing else is left out.
fn all_tags(i: &Inputs<'_>) -> BTreeSet<DeviceTag> {
    let p = i.project;
    let index = p.tag_index();
    let only_links = |tag: &DeviceTag| {
        index.get(tag).is_some_and(|uses| {
            !uses.is_empty()
                && uses.iter().all(|(page, item)| {
                    p.page(page)
                        .and_then(|pg| pg.symbol(item))
                        .and_then(|s| i.symbols.get(&s.symbol))
                        .is_some_and(|sym| sym.role == SymbolRole::Interruption)
                })
        })
    };
    index
        .keys()
        .cloned()
        .chain(p.devices.keys().cloned())
        .filter(|t| !only_links(t))
        .collect()
}

/// Page numbers where a tag is drawn, as `2, 3`.
fn pages_of(p: &Project, uses: &[(String, String)]) -> String {
    let mut nums: Vec<String> = uses
        .iter()
        .filter_map(|(pid, _)| p.page(pid).map(|pg| pg.number.clone()))
        .collect();
    nums.sort_by_key(|n| natural_key(n));
    nums.dedup();
    nums.join(", ")
}

/// Parts list (bill of materials): one row per part, with quantity and the devices using it.
/// Devices without a part come last so the gaps are visible.
pub fn parts_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new(
        "Parts list",
        &["Pos", "Qty", "Manufacturer", "Order number", "Description", "Devices"],
    );
    let mut by_part: BTreeMap<String, Vec<DeviceTag>> = BTreeMap::new();
    let mut without: Vec<DeviceTag> = Vec::new();
    for tag in all_tags(i) {
        match i.project.devices.get(&tag).and_then(|d| d.part.clone()) {
            Some(part) => by_part.entry(part).or_default().push(tag),
            None => without.push(tag),
        }
    }
    let mut pos = 0;
    for (part_id, tags) in &by_part {
        pos += 1;
        let part = i.parts.get(part_id);
        t.rows.push(vec![
            pos.to_string(),
            tags.len().to_string(),
            part.map(|p| p.manufacturer.clone()).unwrap_or_default(),
            part.map(|p| p.order_number.clone()).unwrap_or_else(|| part_id.clone()),
            part.map(|p| p.description.clone())
                .unwrap_or_else(|| "part not in the project's library copy".into()),
            tags.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
        ]);
    }
    for tag in without {
        t.rows.push(vec![
            String::new(),
            "1".into(),
            String::new(),
            String::new(),
            "no part assigned".into(),
            tag.to_string(),
        ]);
    }
    t
}

/// Device list: every tag with its description, part and the pages it appears on.
pub fn device_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new("Device list", &["Device", "Description", "Part", "Symbols", "Pages"]);
    let index = i.project.tag_index();
    for tag in all_tags(i) {
        let dev = i.project.devices.get(&tag);
        let uses = index.get(&tag).map(Vec::as_slice).unwrap_or(&[]);
        let symbols: BTreeSet<String> = uses
            .iter()
            .filter_map(|(pid, item)| i.project.page(pid)?.symbol(item))
            .map(|s| {
                i.symbols
                    .get(&s.symbol)
                    .map(|y| y.name.clone())
                    .unwrap_or_else(|| s.symbol.clone())
            })
            .collect();
        t.rows.push(vec![
            tag.to_string(),
            dev.map(|d| d.description.clone()).unwrap_or_default(),
            dev.and_then(|d| d.part.clone()).unwrap_or_default(),
            symbols.into_iter().collect::<Vec<_>>().join(", "),
            pages_of(i.project, uses),
        ]);
    }
    t
}

/// How a pin is named in reports: `=A+B-K1:13`, or `page 3 s4:13` for an untagged symbol.
fn pin_label(p: &Project, page_id: &str, item: &str, pin: &str) -> String {
    let page = p.page(page_id);
    match page.and_then(|pg| pg.symbol(item)).and_then(|s| s.tag.as_ref()) {
        Some(tag) => format!("{tag}:{pin}"),
        None => format!(
            "page {} {item}:{pin}",
            page.map(|pg| pg.number.as_str()).unwrap_or(page_id)
        ),
    }
}

/// The net each pin belongs to, with its potential name (or a generated one).
fn net_names(i: &Inputs) -> (Vec<connectivity::Net>, BTreeMap<PinRef, String>) {
    let nets = connectivity::nets_linked(i.project, |id| i.symbols.get(id).map(|s| s.role));
    let mut by_pin = BTreeMap::new();
    for (n, net) in nets.iter().enumerate() {
        let name = net.names.first().cloned().unwrap_or_else(|| format!("N{}", n + 1));
        for pin in &net.pins {
            by_pin.insert(pin.clone(), name.clone());
        }
    }
    (nets, by_pin)
}

/// Wire list: every drawn wire with both ends, then its cross-section, colour and type. A free
/// end is reported as a junction point.
pub fn wire_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new(
        "Wire list",
        &[
            "Page",
            "Wire",
            "Potential",
            "Cable",
            "From",
            "To",
            "Section",
            "Colour",
            "Type",
        ],
    );
    let (_, net_of) = net_names(i);
    for page in &i.project.pages {
        for w in &page.wires {
            let end = |e: &Endpoint| match e {
                Endpoint::Pin { item, pin } => pin_label(i.project, &page.id, item, pin),
                Endpoint::Point { at } => format!("junction {:.1},{:.1}", at.x, at.y),
            };
            let potential = w.potential.clone().or_else(|| {
                [&w.from, &w.to].into_iter().find_map(|e| match e {
                    Endpoint::Pin { item, pin } => net_of.get(&(page.id.clone(), item.clone(), pin.clone())).cloned(),
                    Endpoint::Point { .. } => None,
                })
            });
            t.rows.push(vec![
                page.number.clone(),
                w.number.clone().unwrap_or_default(),
                potential.unwrap_or_default(),
                w.cable.as_ref().map(cable_core_label).unwrap_or_default(),
                end(&w.from),
                end(&w.to),
                w.spec.section.clone().unwrap_or_default(),
                w.spec.colour.clone().unwrap_or_default(),
                w.spec.kind.clone().unwrap_or_default(),
            ]);
        }
    }
    t
}

fn cable_core_label(c: &wc_core::CableCore) -> String {
    if c.core.is_empty() {
        c.cable.to_string()
    } else {
        format!("{}:{}", c.cable, c.core)
    }
}

/// Cable list: every cable with its type, then one row per core with the wire's two ends.
pub fn cable_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new(
        "Cable list",
        &["Cable", "Type", "Description", "Core", "Page", "From", "To"],
    );
    let mut cores: BTreeMap<DeviceTag, Vec<(String, String, String, String)>> = BTreeMap::new();
    for page in &i.project.pages {
        for w in &page.wires {
            let Some(c) = &w.cable else { continue };
            let end = |e: &Endpoint| match e {
                Endpoint::Pin { item, pin } => pin_label(i.project, &page.id, item, pin),
                Endpoint::Point { at } => format!("junction {:.1},{:.1}", at.x, at.y),
            };
            cores.entry(c.cable.clone()).or_default().push((
                c.core.clone(),
                page.number.clone(),
                end(&w.from),
                end(&w.to),
            ));
        }
    }
    for (cable, mut list) in cores {
        list.sort_by_key(|a| natural_key(&a.0));
        let device = i.project.devices.get(&cable);
        let part = device.and_then(|d| d.part.as_ref()).and_then(|id| i.parts.get(id));
        let kind = part
            .map(|p| {
                [p.manufacturer.as_str(), p.order_number.as_str()]
                    .join(" ")
                    .trim()
                    .to_owned()
            })
            .unwrap_or_default();
        let description = device
            .map(|d| d.description.clone())
            .filter(|d| !d.is_empty())
            .or_else(|| part.map(|p| p.description.clone()))
            .unwrap_or_default();
        for (n, (core, page, from, to)) in list.into_iter().enumerate() {
            let first = n == 0;
            t.rows.push(vec![
                if first { cable.to_string() } else { String::new() },
                if first { kind.clone() } else { String::new() },
                if first { description.clone() } else { String::new() },
                core,
                page,
                from,
                to,
            ]);
        }
    }
    t
}

/// Net list: each electrically connected group of pins, across pages.
pub fn net_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new("Net list", &["Net", "Pins", "Connections"]);
    let (nets, _) = net_names(i);
    let mut rows: Vec<(String, Vec<String>)> = nets
        .iter()
        .enumerate()
        .map(|(n, net)| {
            let name = if net.names.is_empty() {
                format!("N{}", n + 1)
            } else {
                net.names.join(" / ")
            };
            let mut pins: Vec<String> = net
                .pins
                .iter()
                .map(|(pg, item, pin)| pin_label(i.project, pg, item, pin))
                .collect();
            pins.sort_by_key(|s| natural_key(s));
            (name, pins)
        })
        .collect();
    rows.sort_by_key(|(name, _)| natural_key(name));
    for (name, pins) in rows {
        t.rows.push(vec![name, pins.len().to_string(), pins.join(", ")]);
    }
    t
}

/// Terminal plan: for every terminal strip, each terminal with what connects to its two sides.
///
/// A terminal is a symbol with the terminal role. Its strip and number come from the tag:
/// `-X1.3` is terminal 3 of strip `-X1`. Terminals tagged with just the strip (`-X1`) are
/// numbered in drawing order (page, then left to right).
pub fn terminal_plan(i: &Inputs<'_>) -> Table {
    let mut t = Table::new(
        "Terminal plan",
        &[
            "Strip",
            "Terminal",
            "Upper (pin 1)",
            "Lower (pin 2)",
            "Potential",
            "Page",
            "Jumpered with",
        ],
    );
    // The potential column sees jumpers; the Upper and Lower columns show what is wired to that
    // side of this terminal itself, so they come from the nets without jumpers.
    let (_, net_of) = net_names(i);
    let mut unjumpered = i.project.clone();
    unjumpered.meta.jumpers.clear();
    let (nets, _) = net_names(&Inputs {
        project: &unjumpered,
        symbols: i.symbols,
        parts: i.parts,
    });
    // Pins connected to a pin, excluding the pin itself.
    let others = |pin: &PinRef| -> Vec<String> {
        nets.iter()
            .find(|n| n.pins.contains(pin))
            .map(|n| {
                let mut v: Vec<String> = n
                    .pins
                    .iter()
                    .filter(|p| *p != pin)
                    .map(|(pg, it, pn)| pin_label(i.project, pg, it, pn))
                    .collect();
                v.sort_by_key(|s| natural_key(s));
                v
            })
            .unwrap_or_default()
    };
    struct Term {
        strip: String,
        number: Option<String>,
        page: String,
        page_id: String,
        item: String,
        pins: Vec<String>,
        x: f64,
        /// The other terminals of its jumper, e.g. `3, 5`.
        jumpered: String,
    }
    let mut terms: Vec<Term> = Vec::new();
    // The numbers jumpers refer to: a terminal tagged with only its strip gets the next one.
    let numbers = wc_core::project::terminal_numbers(i.project, |id| i.symbols.get(id).map(|s| s.role));
    // An overview (line diagram) draws terminals again; the plan lists each one once.
    for page in i.project.pages.iter().filter(|pg| !pg.overview) {
        for s in &page.symbols {
            let Some(sym) = i.symbols.get(&s.symbol) else { continue };
            if sym.role != SymbolRole::Terminal {
                continue;
            }
            let (strip, number) = match &s.tag {
                Some(tag) if tag.product.len() > 1 => {
                    let mut strip = tag.clone();
                    let number = strip.product.pop();
                    (strip.to_string(), number)
                }
                Some(tag) => (tag.to_string(), None),
                None => ("(untagged)".to_owned(), None),
            };
            let assigned = numbers.get(&(page.id.clone(), s.id.clone()));
            let number = assigned.map(|(_, n)| n.clone()).or(number);
            let jumpered = assigned
                .and_then(|(strip, n)| {
                    let j = i
                        .project
                        .meta
                        .jumpers
                        .iter()
                        .find(|j| j.strip == *strip && j.terminals.contains(n))?;
                    Some(
                        j.terminals
                            .iter()
                            .filter(|t| *t != n)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", "),
                    )
                })
                .unwrap_or_default();
            terms.push(Term {
                strip,
                number,
                jumpered,
                page: page.number.clone(),
                page_id: page.id.clone(),
                item: s.id.clone(),
                pins: sym.pins.iter().map(|p| p.name.clone()).collect(),
                x: s.at.x,
            });
        }
    }
    let mut strips: BTreeMap<String, Vec<Term>> = BTreeMap::new();
    for term in terms {
        strips.entry(term.strip.clone()).or_default().push(term);
    }
    for (strip, mut list) in strips {
        // Numbered ones by number; unnumbered ones keep drawing order and are numbered after.
        let page_order = |id: &str| i.project.pages.iter().position(|p| p.id == id).unwrap_or(usize::MAX);
        list.sort_by(|a, b| {
            let ka = a.number.as_deref().map(natural_key);
            let kb = b.number.as_deref().map(natural_key);
            match (ka, kb) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => page_order(&a.page_id)
                    .cmp(&page_order(&b.page_id))
                    .then(a.x.total_cmp(&b.x)),
            }
        });
        let mut next = list
            .iter()
            .filter_map(|t| t.number.as_deref()?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        for term in list {
            let number = term
                .number
                .clone()
                .or_else(|| wc_core::project::next_number(&mut next))
                .unwrap_or_default();
            let side = |n: usize| -> (String, Option<String>) {
                let Some(pin) = term.pins.get(n) else {
                    return (String::new(), None);
                };
                let r: PinRef = (term.page_id.clone(), term.item.clone(), pin.clone());
                (others(&r).join(", "), net_of.get(&r).cloned())
            };
            let (upper, pa) = side(0);
            let (lower, pb) = side(1);
            t.rows.push(vec![
                strip.clone(),
                number,
                upper,
                lower,
                pa.or(pb).unwrap_or_default(),
                term.page.clone(),
                term.jumpered.clone(),
            ]);
        }
    }
    t
}

/// Table of contents: every schematic page with its title and what is drawn on it.
pub fn contents(i: &Inputs<'_>) -> Table {
    let mut t = Table::new("Table of contents", &["Page", "Title", "Devices", "Wires"]);
    t.front = true;
    for page in &i.project.pages {
        t.row_pages.push(page.id.clone());
        let tags: BTreeSet<String> = page
            .symbols
            .iter()
            .filter_map(|s| s.tag.as_ref().map(ToString::to_string))
            .collect();
        t.rows.push(vec![
            page.number.clone(),
            page.title.clone(),
            tags.len().to_string(),
            page.wires.len().to_string(),
        ]);
    }
    t
}

/// Whether a symbol is a PLC input or output, from its library category and name or pin.
fn plc_kind(sym: &Symbol) -> Option<&'static str> {
    let category = sym.category.to_ascii_lowercase();
    let id = sym.id.to_ascii_lowercase();
    if category != "plc" && !id.contains(".plc.") {
        return None;
    }
    let name = format!("{} {}", sym.name, sym.id).to_ascii_lowercase();
    let pins: Vec<&str> = sym.pins.iter().map(|p| p.name.as_str()).collect();
    if name.contains("input") || pins == ["I"] {
        Some("Input")
    } else if name.contains("output") || pins == ["Q"] {
        Some("Output")
    } else {
        None
    }
}

/// PLC I/O list: every PLC input and output drawn, with its address, the device it belongs to
/// and what it is wired to. The address is the device property `Address` (or `PLC address`);
/// points without one sort last so the gaps are visible.
pub fn plc_io_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new(
        "PLC I/O list",
        &["Address", "Type", "Device", "Description", "Connected to", "Page"],
    );
    let (nets, _) = net_names(i);
    let mut net_of: BTreeMap<PinRef, usize> = BTreeMap::new();
    for (n, net) in nets.iter().enumerate() {
        for pin in &net.pins {
            net_of.insert(pin.clone(), n);
        }
    }
    // (no address, address sort key, row)
    type Row = (bool, Vec<(String, u64)>, Vec<String>);
    let mut rows: Vec<Row> = Vec::new();
    for page in &i.project.pages {
        for s in &page.symbols {
            let Some(sym) = i.symbols.get(&s.symbol) else { continue };
            let Some(kind) = plc_kind(sym) else { continue };
            let dev = s.tag.as_ref().and_then(|tag| i.project.devices.get(tag));
            let address = dev
                .and_then(|d| {
                    d.properties
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("address") || k.eq_ignore_ascii_case("plc address"))
                        .map(|(_, v)| v.trim().to_owned())
                })
                .unwrap_or_default();
            let mut others: BTreeSet<String> = BTreeSet::new();
            for p in &sym.pins {
                let key = (page.id.clone(), s.id.clone(), p.name.clone());
                let Some(n) = net_of.get(&key) else { continue };
                for other in nets.get(*n).map(|net| net.pins.as_slice()).unwrap_or(&[]) {
                    if *other != key {
                        let (pg, item, pin) = other;
                        others.insert(pin_label(i.project, pg, item, pin));
                    }
                }
            }
            let mut others: Vec<String> = others.into_iter().collect();
            others.sort_by_key(|o| natural_key(o));
            let row = vec![
                address.clone(),
                kind.to_owned(),
                s.tag
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| format!("{} (no tag)", s.id)),
                dev.map(|d| d.description.clone()).unwrap_or_default(),
                others.join(", "),
                page.number.clone(),
            ];
            rows.push((address.is_empty(), natural_key(&address), row));
        }
    }
    rows.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    t.rows = rows.into_iter().map(|(_, _, r)| r).collect();
    t
}

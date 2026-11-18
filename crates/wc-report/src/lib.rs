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
pub mod configurator;
pub mod marking;
pub mod plc;
pub mod step;
#[cfg(test)]
mod tests;

pub use aml::{AmlCounts, automationml};
pub use check::{Finding, Severity, check, check_table};
pub use configurator::{CONFIGURATOR_COLUMNS, Configurator, Target, configurator_list};
pub use marking::{
    LEVEL_SEPARATOR, Language, LineField, MARKER_PROPERTY, MarkerKind, MarkingOptions, formula_warnings, marking_list,
    marking_list_with, parse_kinds, parse_lines, terminal_strips, to_smartscript_csv,
};
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
    Locations,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Contents,
        Kind::Parts,
        Kind::Devices,
        Kind::Wires,
        Kind::Nets,
        Kind::Terminals,
        Kind::Cables,
        Kind::Plc,
        Kind::Locations,
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
            Kind::Locations => "locations",
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
        Kind::Locations => location_list(i),
    }
}

/// Locations: every location of the project with its description and how many devices are in
/// it (by the `+` part of their tags; a device in `+L1.L3` counts for `L1.L3`, not for `L1`), then
/// locations device tags use that the project does not list.
pub fn location_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new("Locations", &["Location", "Description", "Devices", "Note"]);
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for tag in all_tags(i).iter().filter(|t| !t.location.is_empty()) {
        *count.entry(tag.location.join(".")).or_default() += 1;
    }
    for l in &i.project.meta.locations {
        let n = count.remove(&l.path).unwrap_or(0);
        t.rows.push(vec![
            format!("+{}", l.path),
            l.description.clone(),
            n.to_string(),
            String::new(),
        ]);
    }
    for (path, n) in count {
        t.rows.push(vec![
            format!("+{path}"),
            String::new(),
            n.to_string(),
            "not in the project's locations".into(),
        ]);
    }
    t
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
/// A device's accessories count with their quantity. Devices without any part come last so the
/// gaps are visible.
pub fn parts_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new(
        "Parts list",
        &["Pos", "Qty", "Manufacturer", "Order number", "Description", "Devices"],
    );
    let mut by_part: BTreeMap<String, (u64, Vec<DeviceTag>)> = BTreeMap::new();
    let mut without: Vec<DeviceTag> = Vec::new();
    for tag in all_tags(i) {
        let parts = i.project.devices.get(&tag).map(|d| d.all_parts()).unwrap_or_default();
        if parts.is_empty() {
            without.push(tag);
            continue;
        }
        for (part, qty) in parts {
            let (n, tags) = by_part.entry(part.to_owned()).or_default();
            *n += u64::from(qty);
            if tags.last() != Some(&tag) {
                tags.push(tag.clone());
            }
        }
    }
    // Jumpers with a part count once each, under their strip.
    for j in &i.project.meta.jumpers {
        if let Some(part) = &j.part {
            let (n, tags) = by_part.entry(part.clone()).or_default();
            *n += 1;
            if !tags.contains(&j.strip) {
                tags.push(j.strip.clone());
            }
        }
    }
    let mut pos = 0;
    for (part_id, (qty, tags)) in &by_part {
        pos += 1;
        let part = i.parts.get(part_id);
        t.rows.push(vec![
            pos.to_string(),
            qty.to_string(),
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

/// The device property that makes a terminal one level of a multi-level (double- or triple-deck)
/// terminal: `1` is the bottom level, `2` the next one up. A terminal with level 2 or more sits on
/// the same terminal block as the terminal before it in the strip, so `-X1.1` (level 1) and
/// `-X1.2` (level 2) are one two-level terminal.
pub const LEVEL_PROPERTY: &str = "Level";

/// A [`LEVEL_PROPERTY`] value as a level, or why it is not one.
fn parse_level(value: &str) -> Result<u32, String> {
    match value.trim().parse::<u32>() {
        Ok(0) => Err("levels start at 1".into()),
        Ok(l) => Ok(l),
        Err(e) => Err(format!("not a whole number ({e})")),
    }
}

/// A terminal's [`LEVEL_PROPERTY`] value, `None` when it has none.
fn level_value<'p>(project: &'p wc_core::Project, strip: &str, number: &str) -> Option<&'p str> {
    let tag = DeviceTag::parse(&format!("{strip}.{number}")).ok()?;
    project
        .devices
        .get(&tag)?
        .properties
        .get(LEVEL_PROPERTY)
        .map(String::as_str)
}

/// A terminal's level from its [`LEVEL_PROPERTY`]; `None` when it has none or the value is not a
/// level ([`level_warnings`] says which and why).
fn terminal_level(project: &wc_core::Project, strip: &str, number: &str) -> Option<u32> {
    parse_level(level_value(project, strip, number)?).ok()
}

/// One warning for each device whose [`LEVEL_PROPERTY`] is set but is not a level, naming the
/// device, the value and the reason. Such a terminal counts as single-level in the terminal plan
/// and the marking export.
pub fn level_warnings(project: &wc_core::Project) -> Vec<String> {
    project
        .devices
        .iter()
        .filter_map(|(tag, dev)| {
            let value = dev.properties.get(LEVEL_PROPERTY)?;
            let reason = parse_level(value).err()?;
            Some(format!(
                "{tag}: {LEVEL_PROPERTY} {value:?} is ignored: {reason}; the terminal counts as single-level"
            ))
        })
        .collect()
}

/// What the terminal plan's Page column says for a spare terminal, which no page draws.
pub const SPARE_PAGE: &str = "spare";

/// Terminal plan: for every terminal strip, each terminal with what connects to its two sides.
///
/// The last column is the terminal's level on a multi-level terminal (see [`LEVEL_PROPERTY`]):
/// empty for a single-level terminal, and `1` for the bottom level of a block even when only the
/// levels above it say so.
///
/// A terminal is a symbol with the terminal role. Its strip and number come from the tag:
/// `-X1.3` is terminal 3 of strip `-X1`. Terminals tagged with just the strip (`-X1`) are
/// numbered in drawing order (page, then left to right). Terminals come by number, unless the
/// terminal strip editor put the strip in another order ([`wc_core::project::StripLayout`]);
/// its spare terminals are listed with [`SPARE_PAGE`] in the Page column.
pub fn terminal_plan(i: &Inputs<'_>) -> Table {
    terminal_plan_with_spares(i).0
}

/// [`terminal_plan`], with which of its rows are spare terminals (by row index): a spare is
/// known by where it comes from, not by what its Page cell says.
pub(crate) fn terminal_plan_with_spares(i: &Inputs<'_>) -> (Table, BTreeSet<usize>) {
    let mut spare_rows: BTreeSet<usize> = BTreeSet::new();
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
            "Level",
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
                Some(tag) => {
                    let (strip, number) = tag.terminal_parts();
                    (strip.to_string(), number)
                }
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
    // A strip arranged in the terminal strip editor is listed even when only spares are on it.
    for l in &i.project.meta.strips {
        strips.entry(l.strip.to_string()).or_default();
    }
    for (strip, mut list) in strips {
        // Numbered ones by number; unnumbered ones keep drawing order and are numbered after.
        let page_order = |id: &str| i.project.pages.iter().position(|p| p.id == id).unwrap_or(usize::MAX);
        list.sort_by(|a, b| {
            let ka = a.number.as_deref().map(natural_key);
            let kb = b.number.as_deref().map(natural_key);
            match (ka, kb) {
                // `1` and `01` have equal keys: order them exactly too, as the marking export does.
                (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.number.cmp(&b.number)),
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
        let numbered: Vec<(Term, String)> = list
            .into_iter()
            .map(|term| {
                let number = term
                    .number
                    .clone()
                    .or_else(|| wc_core::project::next_number(&mut next))
                    .unwrap_or_default();
                (term, number)
            })
            .collect();
        // The rail order and spares the terminal strip editor set: a terminal drawn twice
        // stays together, a spare gets a row of its own with nothing connected.
        let layout = i.project.meta.strips.iter().find(|l| l.strip.to_string() == strip);
        // Levels are inferred in the usual order (by number), so moving terminals on the rail
        // never changes a terminal's level or which block it is in. Spares only count when they
        // have a level of their own, so adding one never splits a block.
        let mut usual: Vec<String> = numbered.iter().map(|(_, n)| n.clone()).collect();
        usual.dedup();
        for spare in layout.iter().flat_map(|l| &l.spares) {
            // Only a valid level of its own counts; a bad value is reported by level_warnings.
            if usual.contains(spare) || terminal_level(i.project, &strip, spare).is_none() {
                continue;
            }
            let key = (natural_key(spare), spare.clone());
            let at = usual.partition_point(|n| (natural_key(n), n.clone()) < key);
            usual.insert(at, spare.clone());
        }
        let numbered: Vec<(Option<Term>, String)> = match layout {
            None => numbered.into_iter().map(|(t, n)| (Some(t), n)).collect(),
            Some(layout) => {
                let mut drawn: Vec<String> = numbered.iter().map(|(_, n)| n.clone()).collect();
                drawn.dedup();
                let mut rest: Vec<Option<(Term, String)>> = numbered.into_iter().map(Some).collect();
                let mut out = Vec::new();
                for (n, spare) in layout.arrange(&drawn) {
                    if spare {
                        out.push((None, n));
                        continue;
                    }
                    for slot in rest.iter_mut() {
                        if slot.as_ref().is_some_and(|(_, m)| *m == n)
                            && let Some((t, m)) = slot.take()
                        {
                            out.push((Some(t), m));
                        }
                    }
                }
                out
            }
        };
        // Levels are inferred over distinct terminals: a terminal drawn twice has one level.
        let distinct: Vec<&str> = usual.iter().map(String::as_str).collect();
        let mut by_number: BTreeMap<&str, u32> = BTreeMap::new();
        for (k, n) in distinct.iter().enumerate() {
            let own = terminal_level(i.project, &strip, n);
            // The bottom level of a block needs no property of its own: the level above says so.
            // A bad value is not missing: that terminal stays single-level, as its warning says.
            let missing = level_value(i.project, &strip, n).is_none();
            let below = || {
                distinct
                    .get(k + 1)
                    .and_then(|up| terminal_level(i.project, &strip, up))
                    .filter(|l| missing && *l >= 2)
                    .map(|_| 1)
            };
            if let Some(l) = own.or_else(below) {
                by_number.insert(n, l);
            }
        }
        let levels: Vec<Option<u32>> = numbered
            .iter()
            .map(|(_, n)| by_number.get(n.as_str()).copied())
            .collect();
        for ((term, number), level) in numbered.into_iter().zip(levels) {
            let level = level.map(|l| l.to_string()).unwrap_or_default();
            let Some(term) = term else {
                // A spare: on the rail, not drawn. Its jumper can still be set.
                let jumpered = DeviceTag::parse(&strip)
                    .ok()
                    .and_then(|st| {
                        i.project
                            .meta
                            .jumpers
                            .iter()
                            .find(|j| j.strip == st && j.terminals.contains(&number))
                    })
                    .map(|j| {
                        j.terminals
                            .iter()
                            .filter(|t| **t != number)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                spare_rows.insert(t.rows.len());
                t.rows.push(vec![
                    strip.clone(),
                    number,
                    String::new(),
                    String::new(),
                    String::new(),
                    SPARE_PAGE.to_owned(),
                    jumpered,
                    level,
                ]);
                continue;
            };
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
                level,
            ]);
        }
    }
    // A spare has no wiring of its own, but a jumper to a drawn terminal puts it on that
    // terminal's potential.
    for &k in &spare_rows {
        let Some(row) = t.rows.get(k) else { continue };
        if !row[4].is_empty() {
            continue;
        }
        let (strip, number) = (row[0].clone(), row[1].clone());
        let Some(jumper) = DeviceTag::parse(&strip).ok().and_then(|st| {
            i.project
                .meta
                .jumpers
                .iter()
                .find(|j| j.strip == st && j.terminals.contains(&number))
        }) else {
            continue;
        };
        let potential = t
            .rows
            .iter()
            .enumerate()
            .find(|(n, r)| {
                !spare_rows.contains(n) && r[0] == strip && jumper.terminals.contains(&r[1]) && !r[4].is_empty()
            })
            .map(|(_, r)| r[4].clone());
        if let (Some(p), Some(row)) = (potential, t.rows.get_mut(k)) {
            row[4] = p;
        }
    }
    (t, spare_rows)
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

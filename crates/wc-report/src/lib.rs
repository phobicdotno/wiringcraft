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

#[cfg(test)]
mod tests;

/// A finished report: a title, column headings and rows of text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Table {
    pub title: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl Table {
    fn new(title: &str, columns: &[&str]) -> Self {
        Self {
            title: title.into(),
            columns: columns.iter().map(|c| (*c).to_owned()).collect(),
            rows: Vec::new(),
        }
    }

    /// RFC 4180 CSV with a header row; fields are quoted when they need it.
    pub fn to_csv(&self) -> String {
        let field = |s: &str| {
            if s.contains([',', '"', '\n', '\r']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_owned()
            }
        };
        let line = |cells: &[String]| cells.iter().map(|c| field(c)).collect::<Vec<_>>().join(",");
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
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Parts, Kind::Devices, Kind::Wires, Kind::Nets, Kind::Terminals];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Parts => "parts",
            Kind::Devices => "devices",
            Kind::Wires => "wires",
            Kind::Nets => "nets",
            Kind::Terminals => "terminals",
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
    }
}

/// Split into text and number chunks so `X2` sorts before `X10`.
pub fn natural_key(s: &str) -> Vec<(String, u64)> {
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

/// Every device tag in the project: drawn ones and ones only listed in `devices.json`.
fn all_tags(p: &Project) -> BTreeSet<DeviceTag> {
    p.tag_index().into_keys().chain(p.devices.keys().cloned()).collect()
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
    for tag in all_tags(i.project) {
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
    for tag in all_tags(i.project) {
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
fn net_names(p: &Project) -> (Vec<connectivity::Net>, BTreeMap<PinRef, String>) {
    let nets = connectivity::nets(p);
    let mut by_pin = BTreeMap::new();
    for (n, net) in nets.iter().enumerate() {
        let name = net.names.first().cloned().unwrap_or_else(|| format!("N{}", n + 1));
        for pin in &net.pins {
            by_pin.insert(pin.clone(), name.clone());
        }
    }
    (nets, by_pin)
}

/// Wire list: every drawn wire with both ends. A free end is reported as a junction point.
pub fn wire_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new("Wire list", &["Page", "Wire", "Potential", "From", "To"]);
    let (_, net_of) = net_names(i.project);
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
                end(&w.from),
                end(&w.to),
            ]);
        }
    }
    t
}

/// Net list: each electrically connected group of pins, across pages.
pub fn net_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new("Net list", &["Net", "Pins", "Connections"]);
    let (nets, _) = net_names(i.project);
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
        ],
    );
    let (nets, net_of) = net_names(i.project);
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
    }
    let mut terms: Vec<Term> = Vec::new();
    for page in &i.project.pages {
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
            terms.push(Term {
                strip,
                number,
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
            let number = term.number.clone().unwrap_or_else(|| {
                next += 1;
                next.to_string()
            });
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
            ]);
        }
    }
    t
}

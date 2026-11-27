//! Project check: problems in a schematic that are easy to miss on screen, such as a device
//! drawn twice, a contact whose coil is missing, a pin left open or two potentials shorted.

use std::collections::{BTreeMap, BTreeSet};

use wc_core::geom::Point;
use wc_core::symbol::SymbolRole;
use wc_core::{DeviceTag, Endpoint, Page};

use crate::{Inputs, Table, natural_key};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Wrong: the drawing or its reports cannot be trusted until it is fixed.
    Error,
    /// Probably a mistake.
    Warning,
    /// Worth knowing, often fine (for example a device without a part yet).
    Note,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

/// One problem, with where to find it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    /// The page it is on, when it is on one.
    pub page_id: Option<String>,
    /// The placed symbol or wire id on that page, for selecting it.
    pub item: Option<String>,
    pub message: String,
}

fn at(page: &Page, item: &str, severity: Severity, message: String) -> Finding {
    Finding {
        severity,
        page_id: Some(page.id.clone()),
        item: Some(item.to_owned()),
        message,
    }
}

/// Run every check. Findings come sorted: errors first, then by page.
pub fn check(i: &Inputs<'_>) -> Vec<Finding> {
    check_with(i, &[])
}

/// The item a finding on a page's revision cloud names: `cloud <index>`, the cloud's index in
/// the page's clouds.
pub fn cloud_item(n: usize) -> String {
    format!("cloud {n}")
}

/// The index in the page's clouds of the revision cloud a finding's item names, see
/// [`cloud_item`].
pub fn cloud_index(item: &str) -> Option<usize> {
    item.strip_prefix("cloud ")?.parse().ok()
}

/// [`check`], with the check rules of the project's library sources (read by
/// `wc_library::library_rules`): they run before the project's own, less the ones the project
/// turns off.
pub fn check_with(i: &Inputs<'_>, library: &[wc_core::check_rule::RuleSet]) -> Vec<Finding> {
    check_with_unreadable(i, library, &BTreeMap::new())
}

/// [`check_with`], told which used parts have a copy in the project's library that cannot be
/// read, by id with the reason. Such a part is reported with its reason, not as missing from
/// the library copy.
pub fn check_with_unreadable(
    i: &Inputs<'_>,
    library: &[wc_core::check_rule::RuleSet],
    unreadable: &BTreeMap<String, String>,
) -> Vec<Finding> {
    let p = i.project;
    let mut out = Vec::new();
    let role = |symbol: &str| i.symbols.get(symbol).map(|s| s.role);
    // Connections outside overview pages, by wire place; cable cores and wire numbers share them.
    let wire_net = wire_nets_off_overview(i);

    // Revision clouds whose revision the project does not list.
    for page in &p.pages {
        for (n, c) in page.clouds.iter().enumerate() {
            if p.cloud_revision(&c.revision).is_none() {
                out.push(at(
                    page,
                    &cloud_item(n),
                    Severity::Warning,
                    format!(
                        "a cloud carries revision {}, which the project does not list",
                        c.revision
                    ),
                ));
            }
        }
    }

    // Revision letters the PDF cannot print: they come out as `?` and read alike.
    for letter in p.unprintable_revisions() {
        out.push(Finding {
            severity: Severity::Warning,
            page_id: None,
            item: None,
            message: format!(
                "revision {letter} has characters the PDF cannot print apart from others; give it plain Latin-1 letters"
            ),
        });
    }

    // Symbols missing from the project's library copy, and wires on missing symbols or pins.
    for page in &p.pages {
        for s in &page.symbols {
            if !i.symbols.contains_key(&s.symbol) {
                out.push(at(
                    page,
                    &s.id,
                    Severity::Error,
                    format!("symbol {} is missing from the project's library copy", s.symbol),
                ));
            }
        }
        for w in &page.wires {
            for e in [&w.from, &w.to] {
                if let Endpoint::Pin { item, pin } = e {
                    let ok = page
                        .symbol(item)
                        .and_then(|s| i.symbols.get(&s.symbol))
                        .is_some_and(|sym| sym.pin(pin).is_some());
                    let known = page.symbol(item).is_some_and(|s| i.symbols.contains_key(&s.symbol));
                    if !ok && (known || page.symbol(item).is_none()) {
                        out.push(at(
                            page,
                            &w.id,
                            Severity::Error,
                            format!("wire ends on {item}:{pin}, which does not exist"),
                        ));
                    }
                }
            }
        }
    }

    // Devices drawn twice: one tag on two coils, or on two copies of the same standalone symbol,
    // or one terminal number of a strip on two terminals of any symbol. Terminals tagged with
    // only their strip (`-X1`) are numbered one by one, so each is its own terminal. Contacts
    // may repeat (a relay has several). An overview page (line diagram) shows devices again on
    // purpose, so its symbols do not count here.
    let terminals = wc_core::project::terminal_numbers(p, role);
    // What a tag is drawn as: its coil, a numbered terminal (the tag is the strip with the
    // number), or a symbol, by id. Kept apart so that no symbol id can pass for the others.
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    enum DrawnAs {
        Main,
        NumberedTerminal,
        Symbol(String),
    }
    let mut seen: BTreeMap<(DeviceTag, DrawnAs), Vec<(&Page, &str)>> = BTreeMap::new();
    let mut mains: BTreeSet<DeviceTag> = BTreeSet::new();
    for page in &p.pages {
        for s in &page.symbols {
            let Some(tag) = &s.tag else { continue };
            match role(&s.symbol) {
                Some(SymbolRole::Main) => {
                    mains.insert(tag.clone());
                    if page.overview {
                        continue;
                    }
                    seen.entry((tag.clone(), DrawnAs::Main))
                        .or_default()
                        .push((page, &s.id));
                }
                Some(SymbolRole::Terminal) if !page.overview => {
                    let key = match terminals.get(&(page.id.clone(), s.id.clone())) {
                        Some((strip, number)) => {
                            let mut terminal = strip.clone();
                            terminal.product.push(number.clone());
                            (terminal, DrawnAs::NumberedTerminal)
                        }
                        // Not numbered (no strip in its tag, or a strip already at the largest
                        // number): by its tag and symbol.
                        None => (tag.clone(), DrawnAs::Symbol(s.symbol.clone())),
                    };
                    seen.entry(key).or_default().push((page, &s.id));
                }
                Some(SymbolRole::Standalone) if !page.overview => {
                    seen.entry((tag.clone(), DrawnAs::Symbol(s.symbol.clone())))
                        .or_default()
                        .push((page, &s.id));
                }
                _ => {}
            }
        }
    }
    for ((tag, _), uses) in &seen {
        if uses.len() > 1 {
            // In page order (uses come in it), each page once.
            let mut pages: Vec<&str> = Vec::new();
            for (pg, _) in uses {
                if !pages.contains(&pg.number.as_str()) {
                    pages.push(&pg.number);
                }
            }
            let word = if pages.len() == 1 { "page" } else { "pages" };
            for (page, item) in uses {
                out.push(at(
                    page,
                    item,
                    Severity::Error,
                    format!("{tag} is drawn {} times ({word} {})", uses.len(), pages.join(", ")),
                ));
            }
        }
    }

    // Contact overflow: a device draws more contacts than its parts have. Only devices whose
    // main part or accessories list their contacts are checked; overview pages do not count.
    type Drawn<'p> = (wc_core::Contacts, Vec<(&'p Page, &'p str)>);
    let mut drawn: BTreeMap<&DeviceTag, Drawn> = BTreeMap::new();
    for page in p.pages.iter().filter(|pg| !pg.overview) {
        for s in &page.symbols {
            let (Some(tag), Some(sym)) = (&s.tag, i.symbols.get(&s.symbol)) else {
                continue;
            };
            if sym.role != SymbolRole::Auxiliary {
                continue;
            }
            let kinds = wc_core::part::contact_kinds(sym.pins.iter().map(|p| p.name.as_str()));
            if kinds.is_empty() {
                continue;
            }
            let (count, at) = drawn.entry(tag).or_default();
            for k in kinds {
                count.add(k);
            }
            at.push((page, &s.id));
        }
    }
    for (tag, (count, uses)) in &drawn {
        let Some(device) = p.devices.get(*tag) else { continue };
        let fitted = device
            .part
            .iter()
            .map(|id| (id.as_str(), 1))
            .chain(device.accessories.iter().map(|a| (a.part.as_str(), a.qty)));
        let mut has = wc_core::Contacts::default();
        let mut listed = false;
        for (id, qty) in fitted {
            if let Some(c) = i.parts.get(id).and_then(|part| part.contacts.as_ref()) {
                has.add_all(c, qty);
                listed = true;
            }
        }
        if !listed {
            continue;
        }
        if let Some(over) = has.overflow(count) {
            for (page, item) in uses {
                out.push(at(
                    page,
                    item,
                    Severity::Warning,
                    format!(
                        "{tag} draws {} contacts, its parts have {} ({} too many)",
                        count.describe(),
                        has.describe(),
                        over.describe()
                    ),
                ));
            }
        }
    }

    // Contacts without a coil, interruption points without a partner. A copy on an overview
    // page links nothing in the schematic, so it is no partner of an interruption point on a
    // schematic page; one on an overview page may have its partner anywhere.
    let mut links: BTreeMap<DeviceTag, usize> = BTreeMap::new();
    let mut overview_links: BTreeMap<DeviceTag, usize> = BTreeMap::new();
    for page in &p.pages {
        for s in &page.symbols {
            if let (Some(tag), Some(SymbolRole::Interruption)) = (&s.tag, role(&s.symbol)) {
                let count = if page.overview { &mut overview_links } else { &mut links };
                *count.entry(tag.clone()).or_default() += 1;
            }
        }
    }
    let partnered = |page: &Page, tag: &DeviceTag| {
        let schematic = links.get(tag).copied().unwrap_or(0);
        if page.overview {
            schematic + overview_links.get(tag).copied().unwrap_or(0) >= 2
        } else {
            schematic >= 2
        }
    };
    for page in &p.pages {
        for s in &page.symbols {
            match (role(&s.symbol), &s.tag) {
                (Some(SymbolRole::Auxiliary), Some(tag)) if !mains.contains(tag) => {
                    out.push(at(page, &s.id, Severity::Warning, format!("contact {tag} has no coil")))
                }
                (Some(SymbolRole::Interruption), Some(tag)) if !partnered(page, tag) => {
                    out.push(at(
                        page,
                        &s.id,
                        Severity::Warning,
                        format!("interruption point {tag} has no partner"),
                    ));
                }
                (Some(SymbolRole::Interruption), None) => out.push(at(
                    page,
                    &s.id,
                    Severity::Warning,
                    "interruption point has no tag, so it joins nothing".into(),
                )),
                _ => {}
            }
        }
    }

    // Open pins: a pin no wire reaches.
    for page in &p.pages {
        let wired: BTreeSet<(&str, &str)> = page
            .wires
            .iter()
            .flat_map(|w| [&w.from, &w.to])
            .filter_map(|e| match e {
                Endpoint::Pin { item, pin } => Some((item.as_str(), pin.as_str())),
                Endpoint::Point { .. } => None,
            })
            .collect();
        for s in &page.symbols {
            let Some(sym) = i.symbols.get(&s.symbol) else { continue };
            let open: Vec<&str> = sym
                .pins
                .iter()
                .map(|pin| pin.name.as_str())
                .filter(|pin| !wired.contains(&(s.id.as_str(), *pin)))
                .collect();
            if !open.is_empty() {
                let who = s.tag.as_ref().map_or_else(|| sym.name.clone(), ToString::to_string);
                out.push(at(
                    page,
                    &s.id,
                    Severity::Note,
                    format!("{who}: pin {} not connected", open.join(", ")),
                ));
            }
        }
    }

    // Wire ends in the air: a free end that meets no other wire and carries no potential name.
    for page in &p.pages {
        let paths: Vec<(&str, Vec<Point>)> = page
            .wires
            .iter()
            .filter_map(|w| {
                let mut pts = vec![end_pos(i, page, &w.from)?];
                pts.extend(w.via.iter().copied());
                pts.push(end_pos(i, page, &w.to)?);
                Some((w.id.as_str(), pts))
            })
            .collect();
        for w in &page.wires {
            if w.potential.is_some() {
                continue;
            }
            for e in [&w.from, &w.to] {
                let Endpoint::Point { at: q } = e else { continue };
                let touches = paths
                    .iter()
                    .filter(|(id, _)| *id != w.id)
                    .any(|(_, pts)| pts.windows(2).any(|ab| on_segment(*q, ab[0], ab[1])));
                if !touches {
                    out.push(at(
                        page,
                        &w.id,
                        Severity::Warning,
                        format!(
                            "wire{} ends in the air at {:.1}, {:.1}",
                            w.number.as_ref().map(|n| format!(" {n}")).unwrap_or_default(),
                            q.x,
                            q.y
                        ),
                    ));
                }
            }
        }
    }

    same_pin_wires(i, &mut out);

    // Jumpers naming a terminal that is drawn nowhere join nothing there; a spare terminal of
    // the strip (terminal strip editor) is on the rail without a drawing.
    if !p.meta.jumpers.is_empty() {
        let numbers = wc_core::project::terminal_numbers(p, role);
        for j in &p.meta.jumpers {
            for t in &j.terminals {
                let on_overview = || {
                    p.pages.iter().filter(|pg| pg.overview).any(|pg| {
                        pg.symbols.iter().any(|s| {
                            s.tag.as_ref().is_some_and(|tag| {
                                wc_core::project::is_terminal(tag, role(&s.symbol))
                                    && j.terminal_of(tag) == Some(t.as_str())
                            })
                        })
                    })
                };
                let spare = || {
                    p.meta
                        .strip_layout(&j.strip)
                        .is_some_and(|l| l.spares.iter().any(|s| s == t))
                };
                if !numbers.values().any(|(s, n)| *s == j.strip && n == t) && !spare() && !on_overview() {
                    out.push(Finding {
                        severity: Severity::Warning,
                        page_id: None,
                        item: None,
                        message: format!("jumper of {}: terminal {t} is drawn nowhere", j.strip),
                    });
                }
            }
        }
    }

    cable_cores(i, &wire_net, &mut out);

    // Shorts: one net carrying two potential names (for example L1 and L2).
    let nets = wc_core::connectivity::nets_linked(p, |id| i.symbols.get(id).map(|s| s.role));
    for n in nets {
        let names: BTreeSet<&str> = n.names.iter().map(String::as_str).collect();
        if names.len() > 1 {
            let (page_id, item) = n
                .pins
                .first()
                .map(|(pg, it, _)| (Some(pg.clone()), Some(it.clone())))
                .unwrap_or((None, None));
            out.push(Finding {
                severity: Severity::Error,
                page_id,
                item,
                message: format!(
                    "potentials {} are connected to each other",
                    names.into_iter().collect::<Vec<_>>().join(" and ")
                ),
            });
        }
    }

    wire_numbers(i, &wire_net, &mut out);

    // Panel layouts: rails and ducts sticking out of the plate, and devices covering each
    // other, sticking out of the plate, or whose tag is no longer in the project. Outlines that
    // only touch do not overlap. A tag is in the project when it is drawn on a page or listed in
    // the devices file; unlike `all_tags`, a tag drawn only as interruption points still counts,
    // so its plate entry is not called stale.
    let index = p.tag_index();
    for panel in &p.panels {
        let warn = |message: String| Finding {
            severity: Severity::Warning,
            page_id: None,
            item: None,
            message: format!("panel {}: {message}", panel.name),
        };
        let eps = 1e-6;
        let devices: Vec<(&DeviceTag, Point, Point)> = panel
            .items
            .iter()
            .filter_map(|it| match it {
                wc_core::panel::PanelItem::Device { tag, .. } => it.device_bounds().map(|(min, max)| (tag, min, max)),
                _ => None,
            })
            .collect();
        let on_plate = |min: Point, max: Point| wc_core::panel::outline_on_plate(panel.width, panel.height, min, max);
        // Rails and ducts too, named as a plate resize names them.
        for it in &panel.items {
            let (min, max) = it.bounds();
            let what = match it {
                wc_core::panel::PanelItem::Rail { at, .. } => format!("rail at {},{}", at.x, at.y),
                wc_core::panel::PanelItem::Duct { min, max } => {
                    format!("duct {},{} to {},{}", min.x, min.y, max.x, max.y)
                }
                wc_core::panel::PanelItem::Device { .. } => continue,
            };
            // `bounds` sorts a duct's corners with `min`/`max`, which drop a NaN, so the
            // corners as written must be numbers too.
            let written_finite = match it {
                wc_core::panel::PanelItem::Duct { min, max } => {
                    [min.x, min.y, max.x, max.y].iter().all(|v| v.is_finite())
                }
                _ => true,
            };
            if !written_finite || !on_plate(min, max) {
                out.push(warn(format!("{what} sticks out of the plate")));
            }
        }
        for (n, (tag, min, max)) in devices.iter().enumerate() {
            if !index.contains_key(*tag) && !p.devices.contains_key(*tag) {
                out.push(warn(format!("{tag} is on the plate but no longer in the project")));
            }
            if !on_plate(*min, *max) {
                out.push(warn(format!("{tag} sticks out of the plate")));
            }
            for (other, omin, omax) in &devices[n + 1..] {
                if min.x < omax.x - eps && omin.x < max.x - eps && min.y < omax.y - eps && omin.y < max.y - eps {
                    out.push(warn(format!("{tag} and {other} overlap")));
                }
            }
        }
    }

    // Devices without a part, and parts (main, accessory or a jumper's) that the project's
    // library copy does not have: the parts list shows those without a maker, and their
    // contacts cannot be checked.
    // Every device in the devices file counts, even one drawn only as interruption points:
    // its parts are named explicitly.
    let missing = |part: &str| match unreadable.get(part) {
        _ if i.parts.contains_key(part) => None,
        Some(why) => Some(format!(
            "part {part} in the project's library copy cannot be read: {why}"
        )),
        None => Some(format!("part {part} is missing from the project's library copy")),
    };
    for (tag, device) in &p.devices {
        // A part named twice gives one warning: equal findings are merged below.
        for (part, _) in device.all_parts() {
            if let Some(what) = missing(part) {
                out.push(Finding {
                    severity: Severity::Warning,
                    page_id: None,
                    item: None,
                    message: format!("{tag}: {what}"),
                });
            }
        }
    }
    for j in &p.meta.jumpers {
        if let Some(what) = j.part.as_deref().and_then(missing) {
            out.push(Finding {
                severity: Severity::Warning,
                page_id: None,
                item: None,
                message: format!("jumper of {}: {what}", j.strip),
            });
        }
    }
    for tag in crate::all_tags(i) {
        if p.devices.get(&tag).and_then(|d| d.part.as_ref()).is_none() {
            out.push(Finding {
                severity: Severity::Note,
                page_id: None,
                item: None,
                message: format!("{tag} has no part"),
            });
        }
    }

    rules(i, library, &mut out);

    let page_order: BTreeMap<&str, usize> = p.pages.iter().enumerate().map(|(n, pg)| (pg.id.as_str(), n)).collect();
    out.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| {
                let pa = a
                    .page_id
                    .as_deref()
                    .and_then(|x| page_order.get(x))
                    .copied()
                    .unwrap_or(usize::MAX);
                let pb = b
                    .page_id
                    .as_deref()
                    .and_then(|x| page_order.get(x))
                    .copied()
                    .unwrap_or(usize::MAX);
                pa.cmp(&pb)
            })
            .then_with(|| natural_key(&a.message).cmp(&natural_key(&b.message)))
    });
    out.dedup();
    out
}

/// Each wire's connection (net), keyed by its place (page index, wire index): ids need not be
/// unique on a page.
type WireNets = BTreeMap<(usize, usize), usize>;

/// The connections of the wiring outside overview pages, by wire place. An overview wire can
/// carry potentials or link interruption points and so join two connections into one net, which
/// would hide a reused wire number or cable core, or make up two numbers on one connection.
/// Pages keep their places, so page indices stay the same; overview wires get no entry.
fn wire_nets_off_overview(i: &Inputs<'_>) -> WireNets {
    let mut wiring = i.project.clone();
    for page in wiring.pages.iter_mut().filter(|pg| pg.overview) {
        page.wires.clear();
    }
    wc_core::connectivity::wire_nets_by_place(&wiring, |id| i.symbols.get(id).map(|s| s.role)).1
}

/// Wire numbers that do not name one connection each: a connection (net) whose wires carry
/// two different numbers, and one number on wires of two connections. Wire numbering never
/// makes either; numbers typed or imported can. Wires on overview pages repeat connections on
/// purpose and do not count; wires that touch no pin are no connection, as for numbering.
fn wire_numbers(i: &Inputs<'_>, wire_net: &WireNets, out: &mut Vec<Finding>) {
    let p = i.project;
    // Per net: its numbers.
    let mut by_net: BTreeMap<usize, BTreeSet<&str>> = BTreeMap::new();
    // Per number: the nets it is on, each with the first wire carrying it.
    let mut by_number: BTreeMap<&str, Vec<(usize, &Page, &str)>> = BTreeMap::new();
    // Where each net is first drawn, by any of its wires, numbered or not: its place in
    // drawing order and that first wire (page, wire id).
    let mut first_drawn: BTreeMap<usize, (usize, &Page, &str)> = BTreeMap::new();
    for (pi, page) in p.pages.iter().enumerate().filter(|(_, pg)| !pg.overview) {
        for (wi, w) in page.wires.iter().enumerate() {
            if let Some(&net) = wire_net.get(&(pi, wi)) {
                let next = first_drawn.len();
                first_drawn.entry(net).or_insert((next, page, w.id.as_str()));
            }
        }
    }
    for (pi, page) in p.pages.iter().enumerate().filter(|(_, pg)| !pg.overview) {
        for (wi, w) in page.wires.iter().enumerate() {
            let Some(number) = w.number.as_deref().map(str::trim).filter(|n| !n.is_empty()) else {
                continue;
            };
            let Some(&net) = wire_net.get(&(pi, wi)) else {
                continue;
            };
            by_net.entry(net).or_default().insert(number);
            let nets = by_number.entry(number).or_default();
            if !nets.iter().any(|(n, ..)| *n == net) {
                nets.push((net, page, w.id.as_str()));
            }
        }
    }
    // Named on the net's first wire, numbered or not.
    for (net, numbers) in by_net.iter().filter(|(_, n)| n.len() > 1) {
        let Some(&(_, page, wire)) = first_drawn.get(net) else {
            continue;
        };
        let mut list: Vec<&str> = numbers.iter().copied().collect();
        list.sort_by_key(|n| natural_key(n));
        let list = match list.split_last() {
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
            None => continue,
        };
        out.push(at(
            page,
            wire,
            Severity::Warning,
            format!("one connection has wire numbers {list}"),
        ));
    }
    for (number, nets) in by_number.iter_mut().filter(|(_, n)| n.len() > 1) {
        nets.sort_by_key(|(net, ..)| first_drawn.get(net).map_or(usize::MAX, |&(order, ..)| order));
        // Named on the second connection in drawing order (by any of its wires): the first may
        // well be the right one.
        let Some((_, page, wire)) = nets.get(1) else { continue };
        out.push(at(
            page,
            wire,
            Severity::Warning,
            format!("wire number {number} is on {} connections", nets.len()),
        ));
    }
}

/// The library sources' check rules, less the ones the project turns off, then the
/// project's own ([`wc_core::check_rule`]).
fn rules(i: &Inputs<'_>, library: &[wc_core::check_rule::RuleSet], out: &mut Vec<Finding>) {
    use wc_core::check_rule::{CheckRule, KINDS, turned_off};

    let p = i.project;
    let role = |symbol: &str| i.symbols.get(symbol).map(|s| s.role);
    let broken = |label: &str, why: String| Finding {
        severity: Severity::Error,
        page_id: None,
        item: None,
        message: format!("{label} cannot run: {why}"),
    };
    let named = |n: usize, rule: &CheckRule| match rule.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => format!("check rule {n} \"{name}\""),
        None => format!("check rule {n}"),
    };
    // Each rule to run with how its findings name it, and whether a library holds it.
    let mut all: Vec<(String, &CheckRule, bool)> = Vec::new();
    let mut off_used = vec![false; p.meta.rules_off.len()];
    for set in library {
        let rules = match &set.rules {
            Ok(r) => r,
            Err(e) => {
                out.push(Finding {
                    severity: Severity::Error,
                    page_id: None,
                    item: None,
                    message: format!("the check rules of library {} cannot be read: {e}", set.source),
                });
                continue;
            }
        };
        for (k, rule) in rules.iter().enumerate() {
            let mut off = false;
            for (o, used) in p.meta.rules_off.iter().zip(off_used.iter_mut()) {
                if turned_off(std::slice::from_ref(o), &set.source, k + 1, rule) {
                    *used = true;
                    off = true;
                }
            }
            if !off {
                all.push((format!("{} of library {}", named(k + 1, rule), set.source), rule, true));
            }
        }
    }
    for (o, used) in p.meta.rules_off.iter().zip(&off_used) {
        // A source whose rules could not be read may hold it: that is reported above.
        let unread = o
            .split_once(':')
            .is_some_and(|(s, _)| library.iter().any(|set| set.source == s.trim() && set.rules.is_err()));
        if !used && !unread {
            out.push(Finding {
                severity: Severity::Warning,
                page_id: None,
                item: None,
                message: format!(
                    "rules_off names \"{o}\", which is no library check rule here (write source:number or source:name)"
                ),
            });
        }
    }
    all.extend(
        p.meta
            .rules
            .iter()
            .enumerate()
            .map(|(k, r)| (named(k + 1, r), r, false)),
    );
    for (label, rule, library) in all {
        let label = label.as_str();
        // The finding's text: a library rule's own message still names the rule and its
        // source, since the project does not show where it comes from.
        let text = |tag: &DeviceTag, default: String| say(rule, tag, default, label, library);
        if let Some(key) = rule.unknown.keys().next() {
            out.push(broken(
                label,
                format!(
                    "it has a key \"{key}\" it does not know (name, kind, devices, role, pattern, max, severity and message are known)"
                ),
            ));
            continue;
        }
        let kind = rule.kind.trim().to_ascii_lowercase();
        if kind.is_empty() {
            out.push(broken(label, format!("it has no kind ({})", KINDS.join(", "))));
            continue;
        }
        if !KINDS.contains(&kind.as_str()) {
            out.push(broken(
                label,
                format!("kind \"{}\" is not one of {}", rule.kind, KINDS.join(", ")),
            ));
            continue;
        }
        let rule_role = match rule.role() {
            Ok(r) => r,
            Err(bad) => {
                out.push(broken(
                    label,
                    format!("role \"{bad}\" is not one of main, auxiliary, standalone, terminal, interruption"),
                ));
                continue;
            }
        };
        let severity = match rule.severity.as_deref().map(|s| s.trim().to_ascii_lowercase()) {
            None => Severity::Warning,
            Some(s) if s == "error" => Severity::Error,
            Some(s) if s == "warning" => Severity::Warning,
            Some(s) if s == "note" => Severity::Note,
            Some(s) => {
                out.push(broken(label, format!("severity \"{s}\" is not error, warning or note")));
                continue;
            }
        };
        let default_role = (kind == "max-wires").then_some(SymbolRole::Terminal);
        // The placed symbols the rule looks at, in drawing order.
        let picked: Vec<(&Page, &wc_core::project::PlacedSymbol, &DeviceTag)> = p
            .pages
            .iter()
            .filter(|pg| !pg.overview)
            .flat_map(|pg| pg.symbols.iter().map(move |s| (pg, s)))
            .filter_map(|(pg, s)| {
                let tag = s.tag.as_ref()?;
                rule.picks(tag, role(&s.symbol), rule_role, default_role)
                    .then_some((pg, s, tag))
            })
            .collect();
        match kind.as_str() {
            "cable" => cable_rule(label, library, rule, severity, &picked, p, out),
            "tag" => {
                let Some(pattern) = rule.pattern.as_deref().filter(|x| !x.trim().is_empty()) else {
                    out.push(broken(label, "a tag rule needs a pattern".into()));
                    continue;
                };
                let mut done: BTreeSet<&DeviceTag> = BTreeSet::new();
                for (page, s, tag) in &picked {
                    if done.insert(tag) && !wc_core::check_rule::tag_matches(pattern, tag) {
                        out.push(at(
                            page,
                            &s.id,
                            severity,
                            text(tag, format!("{tag} does not match {pattern} ({label})")),
                        ));
                    }
                }
            }
            _ => {
                let Some(max) = rule.max else {
                    out.push(broken(label, "a max-wires rule needs max".into()));
                    continue;
                };
                for (page, s, tag) in &picked {
                    let mut ends: BTreeMap<&str, u32> = BTreeMap::new();
                    for w in &page.wires {
                        for e in [&w.from, &w.to] {
                            if let Endpoint::Pin { item, pin } = e
                                && *item == s.id
                            {
                                *ends.entry(pin.as_str()).or_default() += 1;
                            }
                        }
                    }
                    for (pin, count) in ends.into_iter().filter(|(_, c)| *c > max) {
                        out.push(at(
                            page,
                            &s.id,
                            severity,
                            text(
                                tag,
                                format!("{tag}:{pin} has {count} wires, at most {max} allowed ({label})"),
                            ),
                        ));
                    }
                }
            }
        }
    }

    fn say(rule: &CheckRule, tag: &DeviceTag, default: String, label: &str, library: bool) -> String {
        let t = rule.text(tag, default);
        if library && rule.message.is_some() {
            format!("{t} ({label})")
        } else {
            t
        }
    }

    fn cable_rule(
        label: &str,
        library: bool,
        rule: &CheckRule,
        severity: Severity,
        picked: &[(&Page, &wc_core::project::PlacedSymbol, &DeviceTag)],
        p: &wc_core::Project,
        out: &mut Vec<Finding>,
    ) {
        // Every (page, symbol) a cabled wire ends on.
        let cabled: BTreeSet<(&str, &str)> = p
            .pages
            .iter()
            .flat_map(|pg| {
                pg.wires
                    .iter()
                    .filter(|w| w.cable.is_some())
                    .flat_map(|w| [&w.from, &w.to])
                    .filter_map(move |e| match e {
                        Endpoint::Pin { item, .. } => Some((pg.id.as_str(), item.as_str())),
                        Endpoint::Point { .. } => None,
                    })
            })
            .collect();
        let mut devices: BTreeMap<&DeviceTag, (bool, &Page, &str)> = BTreeMap::new();
        for (page, s, tag) in picked {
            let has = cabled.contains(&(page.id.as_str(), s.id.as_str()));
            let entry = devices.entry(tag).or_insert((false, page, &s.id));
            entry.0 |= has;
        }
        for (tag, (has, page, item)) in devices {
            if !has {
                out.push(at(
                    page,
                    item,
                    severity,
                    say(rule, tag, format!("{tag} has no cable ({label})"), label, library),
                ));
            }
        }
    }
}

fn end_pos(i: &Inputs<'_>, page: &Page, e: &Endpoint) -> Option<Point> {
    match e {
        Endpoint::Point { at } => Some(*at),
        Endpoint::Pin { item, pin } => {
            let s = page.symbol(item)?;
            let p = i.symbols.get(&s.symbol)?.pin(pin)?;
            Some(p.at.transform(s.rotation, s.mirror) + s.at)
        }
    }
}

fn on_segment(p: Point, a: Point, b: Point) -> bool {
    const EPS: f64 = 0.05;
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 < EPS * EPS {
        return p.distance(a) < EPS;
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.distance(Point::new(a.x + t * dx, a.y + t * dy)) < EPS
}

/// Cable cores on two connections: a core is one conductor, so all wires that name it must be
/// one connection (net). Wires on overview pages repeat connections on purpose and do not count;
/// wires that touch no pin are no connection, and a wire naming only its cable (no core) names
/// no conductor.
fn cable_cores(i: &Inputs<'_>, wire_net: &WireNets, out: &mut Vec<Finding>) {
    let p = i.project;
    // Per core: the nets it is on, in drawing order, each with the first wire naming it there.
    type Uses<'p> = Vec<(usize, &'p Page, &'p str)>;
    let mut by_core: BTreeMap<(&DeviceTag, &str), Uses> = BTreeMap::new();
    for (pi, page) in p.pages.iter().enumerate().filter(|(_, pg)| !pg.overview) {
        for (wi, w) in page.wires.iter().enumerate() {
            let Some(c) = &w.cable else { continue };
            let core = c.core.trim();
            if core.is_empty() {
                continue;
            }
            let Some(&net) = wire_net.get(&(pi, wi)) else {
                continue;
            };
            let nets = by_core.entry((&c.cable, core)).or_default();
            if !nets.iter().any(|(n, ..)| *n == net) {
                nets.push((net, page, w.id.as_str()));
            }
        }
    }
    for ((cable, core), nets) in by_core.iter().filter(|(_, n)| n.len() > 1) {
        // Named on the wire of the second connection: the first may well be the right one.
        let Some((_, page, wire)) = nets.get(1) else { continue };
        out.push(at(
            page,
            wire,
            Severity::Warning,
            format!("cable core {cable}:{core} is on {} connections", nets.len()),
        ));
    }
}

/// The findings as a report table.
pub fn check_table(i: &Inputs<'_>) -> Table {
    let mut t = Table::new("Project check", &["Severity", "Page", "Problem"]);
    for f in check(i) {
        let page = f
            .page_id
            .as_deref()
            .and_then(|id| i.project.page(id))
            .map(|pg| pg.number.clone())
            .unwrap_or_default();
        t.rows.push(vec![f.severity.label().into(), page, f.message]);
    }
    t
}

/// Wires whose two ends are on the same pin of the same symbol: they connect nothing, yet show
/// in the wire list and silence the pin's open-pin note. Wire ids need not be unique, so a wire
/// that shares its id with others on the page says which of them it is: otherwise two such wires
/// would give the same finding and the check would show only one.
fn same_pin_wires(i: &Inputs<'_>, out: &mut Vec<Finding>) {
    for page in &i.project.pages {
        for (n, w) in page.wires.iter().enumerate() {
            if let (Endpoint::Pin { item, pin }, Endpoint::Pin { item: item2, pin: pin2 }) = (&w.from, &w.to)
                && item == item2
                && pin == pin2
            {
                let same_id = page.wires.iter().filter(|x| x.id == w.id).count();
                let which = if same_id > 1 {
                    let k = page.wires.iter().take(n).filter(|x| x.id == w.id).count() + 1;
                    format!(" (wire {k} of {same_id} with id {} on this page)", w.id)
                } else {
                    String::new()
                };
                out.push(at(
                    page,
                    &w.id,
                    Severity::Warning,
                    format!(
                        "wire{} starts and ends on {}{which}",
                        w.number.as_ref().map(|n| format!(" {n}")).unwrap_or_default(),
                        crate::pin_label(i.project, &page.id, item, pin)
                    ),
                ));
            }
        }
    }
}

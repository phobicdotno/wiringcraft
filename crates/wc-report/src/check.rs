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
    let p = i.project;
    let mut out = Vec::new();
    let role = |symbol: &str| i.symbols.get(symbol).map(|s| s.role);

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

    // Devices drawn twice: one tag on two coils, or on two copies of the same standalone symbol
    // or terminal. Contacts may repeat (a relay has several). An overview page (line diagram)
    // shows devices again on purpose, so its symbols do not count here.
    let mut seen: BTreeMap<(DeviceTag, String), Vec<(&Page, &str)>> = BTreeMap::new();
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
                    seen.entry((tag.clone(), "main".into()))
                        .or_default()
                        .push((page, &s.id));
                }
                Some(SymbolRole::Standalone | SymbolRole::Terminal) if !page.overview => {
                    seen.entry((tag.clone(), s.symbol.clone()))
                        .or_default()
                        .push((page, &s.id));
                }
                _ => {}
            }
        }
    }
    for ((tag, _), uses) in &seen {
        if uses.len() > 1 {
            let pages: BTreeSet<&str> = uses.iter().map(|(pg, _)| pg.number.as_str()).collect();
            for (page, item) in uses {
                out.push(at(
                    page,
                    item,
                    Severity::Error,
                    format!(
                        "{tag} is drawn {} times (pages {})",
                        uses.len(),
                        pages.iter().copied().collect::<Vec<_>>().join(", ")
                    ),
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

    // Contacts without a coil, interruption points without a partner.
    let mut links: BTreeMap<DeviceTag, usize> = BTreeMap::new();
    for page in &p.pages {
        for s in &page.symbols {
            if let (Some(tag), Some(SymbolRole::Interruption)) = (&s.tag, role(&s.symbol)) {
                *links.entry(tag.clone()).or_default() += 1;
            }
        }
    }
    for page in &p.pages {
        for s in &page.symbols {
            match (role(&s.symbol), &s.tag) {
                (Some(SymbolRole::Auxiliary), Some(tag)) if !mains.contains(tag) => {
                    out.push(at(page, &s.id, Severity::Warning, format!("contact {tag} has no coil")))
                }
                (Some(SymbolRole::Interruption), Some(tag)) if links.get(tag).copied().unwrap_or(0) < 2 => {
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

    // Devices without a part.
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

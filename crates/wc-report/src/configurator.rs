//! Terminal strips for the WAGO configurator (smartDESIGNER): every strip with its terminals,
//! jumpers, accessories, the mounting rails and plates it sits on, and the cross-section and
//! colour of the wires on each side of each terminal.
//!
//! WAGO's configurator takes terminal strip data from E-CAD tools through its EPLAN interface,
//! whose exchange files are not publicly documented, and through spreadsheet (Excel/CSV)
//! article lists. This writes such a list in the same form as the smartScript marker export
//! ([`crate::to_smartscript_csv`]): semicolon-separated UTF-8 with a byte-order mark and CRLF
//! line ends, one row per item. The column layout is WiringCraft's own; the configurator's
//! import dialog maps the Article and Quantity columns, and the other columns describe where
//! each item goes. See `docs/wago-configurator.md`.
//!
//! Every row has an `Index` that stays the same as long as the strip does not change, so a list
//! that comes back from the configurator with articles filled in can be matched to the project's
//! devices row by row (the import back, #123).

use std::collections::{BTreeMap, BTreeSet};

use wc_core::panel::PanelItem;
use wc_core::symbol::SymbolRole;
use wc_core::{DeviceTag, Endpoint};

use crate::{Inputs, Table, natural_key, terminal_plan};

/// The column headings of [`configurator_list`], in order.
pub const CONFIGURATOR_COLUMNS: [&str; 19] = [
    "Index",
    "Type",
    "Strip",
    "Position",
    "Terminal",
    "Level",
    "Article",
    "Manufacturer",
    "Description",
    "Quantity",
    "Upper section",
    "Upper colour",
    "Lower section",
    "Lower colour",
    "Connects",
    "Plate",
    "Rail",
    "Rail length (mm)",
    "Rail rotation",
];

/// The terminal strips of the project for the WAGO configurator, strip by strip in natural tag
/// order. For each strip:
///
/// - `Plate` and `Rail` rows: each panel layout that holds the strip or one of its terminals,
///   with the plate's size, and each DIN rail they sit on with its length and rotation (rails
///   are horizontal, `0`).
/// - `Terminal` rows in strip order (natural order of the terminal numbers), with the
///   terminal's part, its level on a multi-level terminal, and the cross-section and colour of
///   the wires on its upper (pin 1) and lower (pin 2) side.
/// - `Jumper` rows: the terminals each jumper joins.
/// - `Accessory` rows: the strip device's own part and accessories (end plates, end stops,
///   marker carriers) and each terminal's accessories, with their quantities.
///
/// The `Index` column is `<strip>/T<position>` for a terminal, `/J<n>` for a jumper, `/A<n>` for
/// an accessory, `/P<n>` for a plate and `/R<n>` for a rail (`-X1/T3`).
pub fn configurator_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new("WAGO configurator", &CONFIGURATOR_COLUMNS);
    let sides = wire_sides(i);
    // (strip, number, level) in strip order, each terminal once.
    let mut strips: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for r in terminal_plan(i).rows {
        let (Some(strip), Some(number)) = (r.first(), r.get(1)) else {
            continue;
        };
        if strip == "(untagged)" || number.is_empty() {
            continue;
        }
        let list = strips.entry(strip.clone()).or_default();
        if !list.iter().any(|(n, _)| n == number) {
            list.push((number.clone(), r.get(7).cloned().unwrap_or_default()));
        }
    }
    let mut order: Vec<&String> = strips.keys().collect();
    order.sort_by(|a, b| natural_key(a).cmp(&natural_key(b)).then_with(|| a.cmp(b)));
    for strip in order {
        let terminals = &strips[strip];
        let Ok(strip_tag) = DeviceTag::parse(strip) else {
            continue;
        };
        let row = |index: String, kind: &str| {
            let mut r = vec![String::new(); CONFIGURATOR_COLUMNS.len()];
            r[0] = index;
            r[1] = kind.to_owned();
            r[2] = strip.clone();
            r
        };
        let part_cells = |r: &mut Vec<String>, part: &str| {
            match i.parts.get(part) {
                Some(p) => {
                    r[6] = p.order_number.clone();
                    r[7] = p.manufacturer.clone();
                    r[8] = p.description.clone();
                }
                // A part without a vendored copy: its id is all there is.
                None => r[6] = part.to_owned(),
            }
        };

        let (mut plates, mut rails) = (0, 0);
        for (plate, rail) in mounting(i, &strip_tag) {
            if let Some((name, w, h)) = plate {
                plates += 1;
                let mut r = row(format!("{strip}/P{plates}"), "Plate");
                r[8] = format!("Mounting plate {} x {} mm", mm(w), mm(h));
                r[9] = "1".into();
                r[15] = name;
                t.rows.push(r);
            }
            if let Some((name, number, length)) = rail {
                rails += 1;
                let mut r = row(format!("{strip}/R{rails}"), "Rail");
                r[8] = "DIN rail 35 mm".into();
                r[9] = "1".into();
                r[15] = name;
                r[16] = number.to_string();
                r[17] = mm(length);
                r[18] = "0".into();
                t.rows.push(r);
            }
        }

        for (pos, (number, level)) in terminals.iter().enumerate() {
            let mut r = row(format!("{strip}/T{}", pos + 1), "Terminal");
            r[3] = (pos + 1).to_string();
            r[4] = number.clone();
            r[5] = level.clone();
            r[9] = "1".into();
            if let Some(part) = terminal_device(i, strip, number).and_then(|d| d.part.as_deref()) {
                part_cells(&mut r, part);
            }
            if let Some(s) = sides.get(&(strip.clone(), number.clone())) {
                r[10] = join(&s[0].0);
                r[11] = join(&s[0].1);
                r[12] = join(&s[1].0);
                r[13] = join(&s[1].1);
            }
            t.rows.push(r);
        }

        for (n, j) in i
            .project
            .meta
            .jumpers
            .iter()
            .filter(|j| j.strip == strip_tag)
            .enumerate()
        {
            let mut r = row(format!("{strip}/J{}", n + 1), "Jumper");
            r[8] = format!("{}-way jumper", j.terminals.len());
            r[9] = "1".into();
            r[14] = j.terminals.join(", ");
            t.rows.push(r);
        }

        // The strip's own parts first, then each terminal's accessories in strip order.
        let mut accessories: Vec<(String, u32, String)> = Vec::new();
        if let Some(d) = i.project.devices.get(&strip_tag) {
            accessories.extend(d.part.iter().map(|p| (p.clone(), 1, String::new())));
            accessories.extend(d.accessories.iter().map(|a| (a.part.clone(), a.qty, String::new())));
        }
        for (number, _) in terminals {
            if let Some(d) = terminal_device(i, strip, number) {
                accessories.extend(d.accessories.iter().map(|a| (a.part.clone(), a.qty, number.clone())));
            }
        }
        for (n, (part, qty, number)) in accessories.into_iter().enumerate() {
            let mut r = row(format!("{strip}/A{}", n + 1), "Accessory");
            part_cells(&mut r, &part);
            r[9] = qty.to_string();
            r[14] = number;
            t.rows.push(r);
        }
    }
    t
}

/// The device of terminal `number` of `strip` (`-X1.3`), if the project lists it.
fn terminal_device<'p>(i: &Inputs<'p>, strip: &str, number: &str) -> Option<&'p wc_core::Device> {
    let tag = DeviceTag::parse(&format!("{strip}.{number}")).ok()?;
    i.project.devices.get(&tag)
}

/// Cross-sections and colours of the wires on each side of a terminal.
type Sides = [(BTreeSet<String>, BTreeSet<String>); 2];

/// For each terminal (strip, number): the sections and colours of the wires ending on its first
/// pin (upper side) and second pin (lower side), on every schematic page it is drawn on.
fn wire_sides(i: &Inputs<'_>) -> BTreeMap<(String, String), Sides> {
    let numbers = wc_core::project::terminal_numbers(i.project, |id| i.symbols.get(id).map(|s| s.role));
    let mut out: BTreeMap<(String, String), Sides> = BTreeMap::new();
    for page in i.project.pages.iter().filter(|pg| !pg.overview) {
        for w in &page.wires {
            for end in [&w.from, &w.to] {
                let Endpoint::Pin { item, pin } = end else { continue };
                let Some((strip, number)) = numbers.get(&(page.id.clone(), item.clone())) else {
                    continue;
                };
                let Some(sym) = page
                    .symbol(item)
                    .and_then(|s| i.symbols.get(&s.symbol))
                    .filter(|s| s.role == SymbolRole::Terminal)
                else {
                    continue;
                };
                let Some(side) = sym.pins.iter().position(|p| p.name == *pin).filter(|s| *s < 2) else {
                    continue;
                };
                let entry = out.entry((strip.to_string(), number.clone())).or_default();
                entry[side].0.extend(w.spec.section.clone());
                entry[side].1.extend(w.spec.colour.clone());
            }
        }
    }
    out
}

/// A plate (name, width, height) and a rail on it (plate name, rail number from 1, length).
type Mounting = (Option<(String, f64, f64)>, Option<(String, usize, f64)>);

/// Where `strip` is mounted: for each panel layout that holds the strip device or one of its
/// terminals, the plate (once) and each rail they sit on, in layout order. A device sits on a
/// rail when the rail's centre line crosses its footprint.
fn mounting(i: &Inputs<'_>, strip: &DeviceTag) -> Vec<Mounting> {
    let mut out: Vec<Mounting> = Vec::new();
    for panel in &i.project.panels {
        let footprints: Vec<(f64, f64, f64, f64)> = panel
            .items
            .iter()
            .filter_map(|it| match it {
                PanelItem::Device { tag, at, width, height } if tag == strip || tag.terminal_parts().0 == *strip => {
                    Some((at.x, at.y, at.x + width, at.y + height))
                }
                _ => None,
            })
            .collect();
        if footprints.is_empty() {
            continue;
        }
        out.push((Some((panel.name.clone(), panel.width, panel.height)), None));
        let rails = panel.items.iter().filter_map(|it| match it {
            PanelItem::Rail { at, length } => Some((at, *length)),
            _ => None,
        });
        for (n, (at, length)) in rails.enumerate() {
            let on = footprints
                .iter()
                .any(|&(x0, y0, x1, y1)| y0 <= at.y && at.y <= y1 && x1 >= at.x && x0 <= at.x + length);
            if on {
                out.push((None, Some((panel.name.clone(), n + 1, length))));
            }
        }
    }
    out
}

fn join(set: &BTreeSet<String>) -> String {
    set.iter().cloned().collect::<Vec<_>>().join(", ")
}

/// Millimetres with at most one decimal, without a trailing `.0`.
fn mm(v: f64) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map(str::to_owned).unwrap_or(s)
}

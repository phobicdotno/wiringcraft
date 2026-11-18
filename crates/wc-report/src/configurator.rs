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
/// The `Index` column is `<strip>/T<position>` for a terminal, `/J<n>` for a jumper, `/P<n>` for
/// a plate and `/R<n>` for a rail (`-X1/T3`). Accessory rows name the device slot they are:
/// `<strip>/S` for the strip device's own part, `<strip>/A<n>` for its n-th accessory, and
/// `<strip>/T<position>/A<n>` for the n-th accessory of the terminal device first at that
/// position (`-X1/T2/A1`).
pub fn configurator_list(i: &Inputs<'_>) -> Configurator {
    let mut out = Configurator {
        table: Table::new("WAGO configurator", &CONFIGURATOR_COLUMNS),
        warnings: Vec::new(),
        targets: BTreeMap::new(),
    };
    let info = terminal_info(i);
    // (number, level) of each terminal in strip order, each terminal once, by strip.
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
    let mut order: Vec<(String, Vec<(String, String)>)> = strips.into_iter().collect();
    order.sort_by(|(a, _), (b, _)| natural_key(a).cmp(&natural_key(b)).then_with(|| a.cmp(b)));
    let mut rows: Vec<Row> = Vec::new();
    for (strip, terminals) in &order {
        // The terminal plan writes strips from parsed tags, so this only fails for a tag that
        // does not survive being written and read back; such a strip is reported, not dropped.
        let strip_tag = match DeviceTag::parse(strip) {
            Ok(t) => t,
            Err(e) => {
                out.warnings.push(format!(
                    "terminal strip {strip:?} is left out: its tag cannot be read ({e})"
                ));
                continue;
            }
        };
        let row = |index: String, kind: &'static str| Row {
            index,
            kind,
            strip: strip.clone(),
            ..Row::default()
        };
        // The device each terminal is drawn as: `-X1.3`, or the strip itself for a terminal
        // tagged only with its strip (numbered in drawing order).
        let device_of = |number: &str| -> Option<(&DeviceTag, &wc_core::Device)> {
            let tag = info
                .get(&(strip.clone(), number.to_owned()))
                .and_then(|t| t.tag.as_ref())?;
            i.project.devices.get_key_value(tag)
        };

        let (mut plates, mut rails) = (0, 0);
        for (plate, rail) in mounting(i, &strip_tag) {
            if let Some((name, w, h)) = plate {
                plates += 1;
                rows.push(Row {
                    description: format!("Mounting plate {} x {} mm", mm(w), mm(h)),
                    quantity: "1".into(),
                    plate: name,
                    ..row(format!("{strip}/P{plates}"), "Plate")
                });
            }
            if let Some((name, number, length)) = rail {
                rails += 1;
                rows.push(Row {
                    description: "DIN rail 35 mm".into(),
                    quantity: "1".into(),
                    plate: name,
                    rail: number.to_string(),
                    rail_length: mm(length),
                    rail_rotation: "0".into(),
                    ..row(format!("{strip}/R{rails}"), "Rail")
                });
            }
        }

        for (pos, (number, level)) in terminals.iter().enumerate() {
            let mut r = Row {
                position: (pos + 1).to_string(),
                terminal: number.clone(),
                level: level.clone(),
                quantity: "1".into(),
                ..row(format!("{strip}/T{}", pos + 1), "Terminal")
            };
            if let Some((tag, d)) = device_of(number) {
                if let Some(part) = d.part.as_deref() {
                    r.set_part(i, part);
                }
                r.target = Some(Target::Part(tag.clone()));
            } else if let Some(tag) = info.get(&(strip.clone(), number.clone())).and_then(|t| t.tag.as_ref()) {
                // Drawn but without a device entry yet: the import back makes one.
                r.target = Some(Target::Part(tag.clone()));
            }
            if let Some([(us, uc), (ls, lc)]) = info.get(&(strip.clone(), number.clone())).map(|t| &t.sides) {
                r.upper_section = join(us);
                r.upper_colour = join(uc);
                r.lower_section = join(ls);
                r.lower_colour = join(lc);
            }
            rows.push(r);
        }

        for (n, j) in i
            .project
            .meta
            .jumpers
            .iter()
            .filter(|j| j.strip == strip_tag)
            .enumerate()
        {
            rows.push(Row {
                description: format!("{}-way jumper", j.terminals.len()),
                quantity: "1".into(),
                connects: j.terminals.join(", "),
                ..row(format!("{strip}/J{}", n + 1), "Jumper")
            });
        }

        // The strip device's own parts first (unless terminals are drawn as the strip device
        // itself: then its part is theirs), then each terminal device's accessories in strip
        // order, once per device. Each index names the device's own slot, so adding an
        // accessory to one device leaves the others' indexes as they are.
        let mut accessories: Vec<(String, String, u32, String, Target)> = Vec::new();
        let strip_is_a_terminal = terminals
            .iter()
            .any(|(n, _)| device_of(n).is_some_and(|(t, _)| *t == strip_tag));
        if !strip_is_a_terminal && let Some(d) = i.project.devices.get(&strip_tag) {
            accessories.extend(d.part.iter().map(|p| {
                (
                    format!("{strip}/S"),
                    p.clone(),
                    1,
                    String::new(),
                    Target::Part(strip_tag.clone()),
                )
            }));
            accessories.extend(d.accessories.iter().enumerate().map(|(k, a)| {
                (
                    format!("{strip}/A{}", k + 1),
                    a.part.clone(),
                    a.qty,
                    String::new(),
                    Target::Accessory(strip_tag.clone(), k),
                )
            }));
        }
        let mut seen: BTreeSet<&DeviceTag> = BTreeSet::new();
        for (pos, (number, _)) in terminals.iter().enumerate() {
            if let Some((tag, d)) = device_of(number)
                && seen.insert(tag)
            {
                accessories.extend(d.accessories.iter().enumerate().map(|(k, a)| {
                    (
                        format!("{strip}/T{}/A{}", pos + 1, k + 1),
                        a.part.clone(),
                        a.qty,
                        number.clone(),
                        Target::Accessory(tag.clone(), k),
                    )
                }));
            }
        }
        for (index, part, qty, number, target) in accessories {
            let mut r = Row {
                quantity: qty.to_string(),
                connects: number,
                target: Some(target),
                ..row(index, "Accessory")
            };
            r.set_part(i, &part);
            rows.push(r);
        }
    }
    for mut r in rows {
        if let Some(t) = r.target.take() {
            out.targets.insert(r.index.clone(), t);
        }
        let cells = r.into_cells(&mut out.warnings);
        out.table.rows.push(cells);
    }
    out
}

/// What [`configurator_list`] made: the table, and what was changed or left out on the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configurator {
    pub table: Table,
    pub warnings: Vec<String>,
    /// Which device part each row's article is, by `Index` (as written, before any leading `'`):
    /// terminal and accessory rows. Plates, rails and jumpers are not device parts and have none.
    pub targets: BTreeMap<String, Target>,
}

/// The device part a row of [`configurator_list`] stands for, so a list that comes back from the
/// configurator can be written onto the project (#123).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The device's main part: a terminal's, or the strip device's own part.
    Part(DeviceTag),
    /// The device's accessory at this position (from 0) of its accessory list.
    Accessory(DeviceTag, usize),
}

/// One row of the list, by column (see [`CONFIGURATOR_COLUMNS`]).
#[derive(Debug, Default)]
struct Row {
    index: String,
    kind: &'static str,
    strip: String,
    position: String,
    terminal: String,
    level: String,
    article: String,
    manufacturer: String,
    description: String,
    quantity: String,
    upper_section: String,
    upper_colour: String,
    lower_section: String,
    lower_colour: String,
    connects: String,
    plate: String,
    rail: String,
    rail_length: String,
    rail_rotation: String,
    /// Not a cell: the device part the row stands for.
    target: Option<Target>,
}

impl Row {
    /// Article, manufacturer and description from `part`; a part without a vendored copy is
    /// written by its id.
    fn set_part(&mut self, i: &Inputs<'_>, part: &str) {
        match i.parts.get(part) {
            Some(p) => {
                self.article = p.order_number.clone();
                self.manufacturer = p.manufacturer.clone();
                self.description = p.description.clone();
            }
            None => self.article = part.to_owned(),
        }
    }

    /// The cells in column order. Free texts (part data, wire specs, plate names) that a
    /// spreadsheet would take for a formula get a leading `'`, with a warning. The tag-derived
    /// columns (index, strip) do the same, except for a leading `-`: a product aspect starts a
    /// plain tag (`-X1`), while a function or location aspect (`=A-X1`, `+C1-X1`) is guarded.
    fn into_cells(self, warnings: &mut Vec<String>) -> Vec<String> {
        let index = self.index.clone();
        // `tag`: a leading `-` is a product aspect and stays as it is.
        let mut guard = |column: &str, text: String, tag: bool| -> String {
            let c = match text.chars().next() {
                Some(c @ ('=' | '+' | '@' | '\t' | '\r')) => c,
                Some('-') if !tag => '-',
                _ => return text,
            };
            warnings.push(format!(
                "{index}: {column} {text:?} starts with {c:?}, which a spreadsheet may run as a formula; \
                 it is written with a leading '"
            ));
            format!("'{text}")
        };
        vec![
            guard("Index", self.index, true),
            self.kind.to_owned(),
            guard("Strip", self.strip, true),
            self.position,
            self.terminal,
            self.level,
            guard("Article", self.article, false),
            guard("Manufacturer", self.manufacturer, false),
            guard("Description", self.description, false),
            self.quantity,
            guard("Upper section", self.upper_section, false),
            guard("Upper colour", self.upper_colour, false),
            guard("Lower section", self.lower_section, false),
            guard("Lower colour", self.lower_colour, false),
            self.connects,
            guard("Plate", self.plate, false),
            self.rail,
            self.rail_length,
            self.rail_rotation,
        ]
    }
}

/// Cross-sections and colours of the wires on each side of a terminal.
type Sides = [(BTreeSet<String>, BTreeSet<String>); 2];

/// What the drawing says about one terminal.
#[derive(Default)]
struct Terminal {
    /// The tag it is drawn with: `-X1.3`, or just the strip (`-X1`).
    tag: Option<DeviceTag>,
    /// The sections and colours of the wires on its first pin (upper side) and second pin
    /// (lower side), on every schematic page it is drawn on.
    sides: Sides,
}

/// Every terminal on the schematic pages by (strip, number), numbered as the terminal plan
/// numbers them.
fn terminal_info(i: &Inputs<'_>) -> BTreeMap<(String, String), Terminal> {
    let numbers = wc_core::project::terminal_numbers(i.project, |id| i.symbols.get(id).map(|s| s.role));
    let mut out: BTreeMap<(String, String), Terminal> = BTreeMap::new();
    for page in i.project.pages.iter().filter(|pg| !pg.overview) {
        for s in &page.symbols {
            if let Some((strip, number)) = numbers.get(&(page.id.clone(), s.id.clone())) {
                let t = out.entry((strip.to_string(), number.clone())).or_default();
                if t.tag.is_none() {
                    t.tag = s.tag.clone();
                }
            }
        }
        for w in &page.wires {
            for end in [&w.from, &w.to] {
                let Endpoint::Pin { item, pin } = end else {
                    continue;
                };
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
                let side = sym.pins.iter().position(|p| p.name == *pin);
                let entry = out.entry((strip.to_string(), number.clone())).or_default();
                if let Some((sections, colours)) = side.and_then(|k| entry.sides.get_mut(k)) {
                    sections.extend(w.spec.section.clone());
                    colours.extend(w.spec.colour.clone());
                }
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

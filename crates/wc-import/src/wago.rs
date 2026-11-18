//! The WAGO configurator list back: the articles and accessories chosen in the configurator
//! (smartDESIGNER), written onto the project's terminal and strip devices (#123).
//!
//! `wiringcraft export wago-configurator` writes one row per item with an `Index` that stays the
//! same as long as the strip does not change (`-X1/T3`, `-X1/A2`). The list comes back with the
//! Article (and Quantity) columns filled in; [`read_configured`] reads it, [`plan`] matches each
//! row to the device part it stands for by that index and the article to a part of the project
//! or its libraries, and [`apply`] writes the result. Everything that cannot be matched is listed
//! with the reason, never guessed. See `docs/wago-configurator.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use wc_core::lock::ItemKind;
use wc_core::{Device, DevicePart, DeviceTag, Part, Project, store};
use wc_report::{Inputs, Target};

use crate::csv::{decode, records};
use crate::{Error, Result};

/// One row of a configured list, as far as the import reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfiguredRow {
    /// The row's line in the file, from 1 (the header is line 1).
    pub line: usize,
    pub index: String,
    /// `Terminal`, `Accessory`, ... or empty when the file has no Type column.
    pub kind: String,
    pub strip: String,
    pub terminal: String,
    /// The Connects column (the terminal an accessory belongs to); `None` when the file has
    /// no such column.
    pub connects: Option<String>,
    pub article: String,
    pub manufacturer: String,
    /// `None` when the file has no Quantity column or the cell is empty.
    pub quantity: Option<u32>,
}

/// Read a configured list (bytes as saved: UTF-8 with or without BOM, or Latin-1). The header
/// must name an Index and an Article column; Type, Strip, Terminal, Manufacturer and Quantity
/// are read when present. Rows whose quantity is not a whole number are read without one, with
/// a warning.
pub fn read_configured(bytes: &[u8]) -> Result<(Vec<ConfiguredRow>, Vec<String>)> {
    let text = decode(bytes);
    let first_line = text.lines().next().unwrap_or("");
    let delim = [';', '\t', ',']
        .into_iter()
        .max_by_key(|d| first_line.matches(*d).count())
        .unwrap_or(';');
    let rows = records(&text, delim);
    let Some((header, body)) = rows.split_first() else {
        return Err(Error::Format("the file is empty".into()));
    };
    let key = |h: &str| -> String { h.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect() };
    let find = |names: &[&str]| header.iter().position(|h| names.contains(&key(h).as_str()));
    let (Some(index), Some(article)) = (
        find(&["index"]),
        find(&[
            "article",
            "articleno",
            "articlenumber",
            "itemnumber",
            "ordernumber",
            "artikelnummer",
            "bestellnummer",
            "artikel",
        ]),
    ) else {
        return Err(Error::Format(format!(
            "need an Index and an Article column (as `export wago-configurator` writes them) among {header:?}"
        )));
    };
    let kind = find(&["type", "typ"]);
    let strip = find(&["strip", "terminalstrip", "leiste", "klemmenleiste"]);
    let terminal = find(&["terminal", "klemme"]);
    let connects = find(&["connects"]);
    let manufacturer = find(&["manufacturer", "hersteller"]);
    let quantity = find(&["quantity", "qty", "menge", "anzahl"]);
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for (n, row) in body.iter().enumerate() {
        let line = n + 2;
        let cell = |c: Option<usize>| -> String {
            let v = c.and_then(|c| row.get(c)).map(|v| v.trim()).unwrap_or("");
            // The export guards cells a spreadsheet would run as a formula with a leading `'`.
            v.strip_prefix('\'').unwrap_or(v).trim().to_owned()
        };
        let q = cell(quantity);
        let quantity = if q.is_empty() {
            None
        } else {
            match q.parse::<u32>() {
                Ok(v) => Some(v),
                Err(why) => {
                    warnings.push(format!(
                        "line {line}: quantity {q:?} cannot be read ({why}); the quantity is left as it is"
                    ));
                    None
                }
            }
        };
        out.push(ConfiguredRow {
            line,
            index: cell(Some(index)),
            kind: cell(kind),
            strip: cell(strip),
            terminal: cell(terminal),
            connects: connects.map(|c| cell(Some(c))),
            article: cell(Some(article)),
            manufacturer: cell(manufacturer),
            quantity,
        });
    }
    Ok((out, warnings))
}

/// The parts an article may be, by id: the project's own (read from its copies) before its
/// libraries'.
#[derive(Debug, Clone, Default)]
pub struct Candidates {
    pub project: BTreeMap<String, Part>,
    pub library: BTreeMap<String, Part>,
    /// Parts the project locks whose copy cannot be read, with the reason: never assigned, so
    /// a device does not get a part the project cannot show.
    pub unreadable: BTreeMap<String, String>,
    /// For each library part, the position of the library it was read from in the project's
    /// list, so it is taken from that library alone (another may not open).
    pub sources: BTreeMap<String, usize>,
}

impl Candidates {
    /// The parts of the project in `dir` (its locked copies) and of its libraries, and a warning
    /// for each library or part that cannot be read, with the reason. A library that cannot be
    /// opened only means fewer candidates; the others are still read.
    pub fn load(dir: &Path, project: &Project) -> (Self, Vec<String>) {
        let mut out = Self::default();
        let mut warnings = Vec::new();
        for (n, source) in project.meta.libraries.iter().enumerate() {
            let mut one = project.clone();
            one.meta.libraries = vec![source.clone()];
            let lib = match wc_library::Library::open(dir, &one) {
                Ok(lib) => lib,
                Err(why) => {
                    warnings.push(format!("library {} cannot be read: {why}", source.name));
                    continue;
                }
            };
            // A library without a parts folder has no parts; one whose folder cannot be listed
            // is reported, or its parts would look missing.
            for s in &lib.sources {
                let folder = s.root.join(ItemKind::Part.dir());
                if let Err(why) = std::fs::read_dir(&folder)
                    && why.kind() != std::io::ErrorKind::NotFound
                {
                    warnings.push(format!(
                        "the parts of library {} cannot be listed ({}): {why}",
                        source.name,
                        folder.display()
                    ));
                }
            }
            for id in lib.index(ItemKind::Part).into_keys() {
                // The first library that has a part wins, as everywhere else.
                if out.library.contains_key(&id) {
                    continue;
                }
                match lib.part(&id) {
                    Ok((part, _)) => {
                        out.sources.insert(id.clone(), n);
                        out.library.insert(id, part);
                    }
                    Err(why) => warnings.push(format!("library part {id} cannot be read: {why}")),
                }
            }
        }
        for id in project.lock.parts.keys() {
            match store::read_vendored_part(dir, id) {
                Ok(part) => {
                    out.project.insert(id.clone(), part);
                }
                Err(why) => {
                    warnings.push(format!("the project's copy of part {id} cannot be read: {why}"));
                    out.unreadable.insert(id.clone(), why.to_string());
                }
            }
        }
        (out, warnings)
    }

    /// Make sure the project in `dir` holds part `id`: a library part is copied into the
    /// project and locked as it was read here, the copy its article was matched on (not a
    /// fresh read, which may have changed since).
    pub fn take(&self, dir: &Path, project: &mut Project, id: &str) -> std::result::Result<(), String> {
        if self.project.contains_key(id) || project.lock.parts.contains_key(id) {
            return Ok(());
        }
        let (Some(part), Some(source)) = (
            self.library.get(id),
            self.sources.get(id).and_then(|&n| project.meta.libraries.get(n)),
        ) else {
            return Err(format!("no library has part {id}"));
        };
        store::write_vendored_part(dir, part).map_err(|e| e.to_string())?;
        project.lock.parts.insert(
            id.to_owned(),
            wc_core::lock::LockEntry {
                source: source.name.clone(),
                hash: part.hash(),
            },
        );
        Ok(())
    }

    /// Every part, the project's copy first for an id both have.
    pub fn all(&self) -> BTreeMap<String, Part> {
        let mut all = self.library.clone();
        all.extend(self.project.clone());
        all
    }
}

/// Where an article goes on a device.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Slot {
    /// The device's main part.
    Part,
    /// Its accessory at this position (from 0).
    Accessory(usize),
    /// A new accessory at the end of its list (a row the configurator added, without an index).
    NewAccessory,
}

/// One change [`plan`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    /// The row's line in the file.
    pub line: usize,
    pub tag: DeviceTag,
    pub slot: Slot,
    /// The part id.
    pub part: String,
    pub qty: u32,
}

/// What a configured list would change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub assignments: Vec<Assignment>,
    /// Rows with an article that already is the device's part.
    pub unchanged: usize,
    /// Rows with an article that could not be matched, each with the line and the reason.
    pub unmatched: Vec<String>,
}

impl Plan {
    /// The part ids the changes use, each once.
    pub fn parts(&self) -> BTreeSet<&str> {
        self.assignments.iter().map(|a| a.part.as_str()).collect()
    }
}

/// Match the rows of a configured list to the project in `i`. `candidates` are the parts an
/// article may be: a part matches when its order number is the article (ignoring case) or its
/// id is (the export writes a part without a copy by its id), the project's own parts before
/// the libraries'; when several match, the row's manufacturer chooses. Rows without an article
/// are passed over. A row without an index that names a strip of the project becomes a new
/// accessory of the strip device, unless the device has that part already.
pub fn plan(i: &Inputs<'_>, candidates: &Candidates, rows: &[ConfiguredRow]) -> Plan {
    let now = wc_report::configurator_list(i);
    // Index (unguarded) → (type, terminal, connects) of the list the project gives now.
    let unguard = |s: &str| s.strip_prefix('\'').unwrap_or(s).to_owned();
    let current: BTreeMap<String, (String, String, String)> = now
        .table
        .rows
        .iter()
        .map(|r| {
            let cell = |k: usize| r.get(k).cloned().unwrap_or_default();
            (unguard(&cell(0)), (cell(1), cell(4), cell(14)))
        })
        .collect();
    let strips: BTreeSet<String> = now
        .table
        .rows
        .iter()
        .filter_map(|r| r.get(2))
        .map(|s| unguard(s))
        .collect();

    let mut out = Plan::default();
    // (tag, slot) → (part, quantity, line) already planned, to catch two rows that disagree.
    let mut taken: BTreeMap<(DeviceTag, Slot), Taken> = BTreeMap::new();
    let mut conflicted: BTreeSet<(DeviceTag, Slot)> = BTreeSet::new();
    let mut new_accessories: Vec<Assignment> = Vec::new();
    for row in rows {
        if row.article.is_empty() {
            continue;
        }
        let at = if row.index.is_empty() {
            format!("line {}", row.line)
        } else {
            format!("line {} ({})", row.line, row.index)
        };
        let mut miss = |why: String| out.unmatched.push(format!("{at}: {} — {why}", row.article));
        let (tag, slot) = if row.index.is_empty() {
            match DeviceTag::parse(&row.strip) {
                Ok(t) if strips.contains(&row.strip) => (t, Slot::NewAccessory),
                _ if row.strip.is_empty() => {
                    miss("the row has no index and no strip; export again and add it to a strip's rows".into());
                    continue;
                }
                _ => {
                    miss(format!(
                        "the row has no index and the project has no strip {:?}",
                        row.strip
                    ));
                    continue;
                }
            }
        } else {
            let Some((kind, terminal, connects)) = current.get(&row.index) else {
                miss("the project has no such row now (the strip changed?); export the list again".into());
                continue;
            };
            if !row.kind.is_empty() && !row.kind.eq_ignore_ascii_case(kind) {
                miss(format!(
                    "the row's type is {} in the file but {kind} in the project now; export the list again",
                    row.kind
                ));
                continue;
            }
            if kind == "Terminal" && !row.terminal.is_empty() && row.terminal != *terminal {
                miss(format!(
                    "the row is terminal {} in the file but terminal {terminal} in the project now; export the list again",
                    row.terminal
                ));
                continue;
            }
            // An accessory's index holds its terminal's position, which moves when a terminal
            // is added before it: the terminal it belongs to must still be the same, and a file
            // without the Connects column cannot show that.
            if kind == "Accessory" && !connects.is_empty() && row.connects.is_none() {
                miss(
                    "the file has no Connects column, so the terminal this accessory belongs to cannot be checked; export the list again"
                        .into(),
                );
                continue;
            }
            if kind == "Accessory"
                && let Some(file) = &row.connects
                && file != connects
            {
                let name = |c: &str| {
                    if c.is_empty() {
                        "the strip".to_owned()
                    } else {
                        format!("terminal {c}")
                    }
                };
                miss(format!(
                    "the accessory belongs to {} in the file but to {} in the project now; export the list again",
                    name(file),
                    name(connects)
                ));
                continue;
            }
            match now.targets.get(&row.index) {
                Some(Target::Part(t)) => (t.clone(), Slot::Part),
                Some(Target::Accessory(t, k)) => (t.clone(), Slot::Accessory(*k)),
                None => {
                    miss(match &**kind {
                        "Jumper" => "jumpers have no part of their own yet".into(),
                        "Plate" | "Rail" => format!("a {} is not a device of the project", kind.to_lowercase()),
                        _ => "the terminal has no device tag".into(),
                    });
                    continue;
                }
            }
        };
        let part = match find_part(candidates, &row.article, &row.manufacturer) {
            Ok(p) => p,
            Err(why) => {
                miss(why);
                continue;
            }
        };
        let device = i.project.devices.get(&tag);
        let (now_part, now_qty) = match &slot {
            Slot::Part => (device.and_then(|d| d.part.clone()), 1),
            Slot::Accessory(k) => device
                .and_then(|d| d.accessories.get(*k))
                .map_or((None, 1), |a| (Some(a.part.clone()), a.qty)),
            Slot::NewAccessory => (None, 1),
        };
        let qty = match slot {
            Slot::Part => 1,
            _ => row.quantity.unwrap_or(now_qty).max(1),
        };
        if slot == Slot::NewAccessory {
            // Decided below, against the device as the other rows leave it.
            new_accessories.push(Assignment {
                line: row.line,
                tag,
                slot,
                part,
                qty,
            });
            continue;
        }
        {
            let key = (tag.clone(), slot.clone());
            if let Some(first) = taken.get_mut(&key) {
                let (why, choose) = if first.part != part {
                    (format!("line {} gives {tag} part {}", first.line, first.part), "one")
                } else if first.qty != qty {
                    (
                        format!("line {} gives {tag} {} {} times", first.line, first.part, first.qty),
                        "one quantity",
                    )
                } else if conflicted.contains(&key) {
                    (format!("other rows give {tag} something else here"), "one")
                } else {
                    // The same again: taken with the first row, unless a later row disagrees.
                    first.same.push(format!("{at}: {}", row.article));
                    continue;
                };
                miss(format!("{why}; neither is taken, choose {choose} in the configurator"));
                // The rows before are not taken either: each listed once, with the row it
                // clashes with.
                if conflicted.insert(key) {
                    if first.unchanged {
                        out.unchanged -= 1;
                    }
                    let earlier =
                        std::iter::once(format!("{}: {}", first.at, first.article)).chain(first.same.drain(..));
                    for e in earlier {
                        out.unmatched.push(format!(
                            "{e} — line {} disagrees; neither is taken, choose {choose} in the configurator",
                            row.line
                        ));
                    }
                }
                continue;
            }
            let unchanged = now_part.as_deref() == Some(part.as_str()) && qty == now_qty;
            taken.insert(
                key,
                Taken {
                    part: part.clone(),
                    qty,
                    line: row.line,
                    at: at.clone(),
                    article: row.article.clone(),
                    unchanged,
                    same: Vec::new(),
                },
            );
        }
        if now_part.as_deref() == Some(part.as_str()) && qty == now_qty {
            out.unchanged += 1;
            continue;
        }
        out.assignments.push(Assignment {
            line: row.line,
            tag,
            slot,
            part,
            qty,
        });
    }
    // A device part two rows disagree on gets neither article.
    out.assignments
        .retain(|a| !conflicted.contains(&(a.tag.clone(), a.slot.clone())));
    // A new accessory the device has after the rows above (or an earlier new one) is there
    // already.
    let mut after: BTreeMap<DeviceTag, Device> = BTreeMap::new();
    for a in &out.assignments {
        put(after.entry(a.tag.clone()).or_insert_with(|| device_now(i, &a.tag)), a);
    }
    for a in new_accessories {
        let d = after.entry(a.tag.clone()).or_insert_with(|| device_now(i, &a.tag));
        if put(d, &a) {
            out.assignments.push(a);
        } else {
            out.unchanged += 1;
        }
    }
    out
}

fn device_now(i: &Inputs<'_>, tag: &DeviceTag) -> Device {
    i.project.devices.get(tag).cloned().unwrap_or_default()
}

/// Write one assignment onto a device; false for a new accessory the device has already.
fn put(d: &mut Device, a: &Assignment) -> bool {
    let part = DevicePart {
        part: a.part.clone(),
        qty: a.qty,
    };
    match a.slot {
        Slot::Part => d.part = Some(a.part.clone()),
        Slot::Accessory(k) => match d.accessories.get_mut(k) {
            Some(slot) => *slot = part,
            None => d.accessories.push(part),
        },
        Slot::NewAccessory if d.uses_part(&a.part) => return false,
        Slot::NewAccessory => d.accessories.push(part),
    }
    true
}

/// The first row [`plan`] found for a device slot.
struct Taken {
    part: String,
    qty: u32,
    line: usize,
    /// Where the row is, as the reasons name it.
    at: String,
    article: String,
    /// It already was the device's part.
    unchanged: bool,
    /// Later rows that give the same, as the reasons name them.
    same: Vec<String>,
}

/// The part `article` names among `candidates`: by id, or by order number (ignoring case and
/// spaces at the ends), the project's own parts first and the libraries' only when none of the
/// project's match; when several match, the one by `manufacturer`. The reason when none or more
/// than one does, or when the part is one the project locks but cannot read.
fn find_part(candidates: &Candidates, article: &str, manufacturer: &str) -> std::result::Result<String, String> {
    let unreadable = |id: &str| {
        candidates
            .unreadable
            .get(id)
            .map(|why| format!("the project has part {id}, but its copy cannot be read ({why})"))
    };
    if let Some(why) = unreadable(article) {
        return Err(why);
    }
    if candidates.project.contains_key(article) || candidates.library.contains_key(article) {
        return Ok(article.to_owned());
    }
    let matches = |p: &&Part| p.order_number.trim().eq_ignore_ascii_case(article);
    // The project's own parts (not from a library) first. A part the project took from a
    // library is looked up together with the library's other parts, so a list imported again
    // after a part was taken gets the same answer as the first time.
    let mut hits: Vec<&Part> = candidates
        .project
        .values()
        .filter(|p| !candidates.library.contains_key(&p.id))
        .filter(matches)
        .collect();
    if hits.is_empty() {
        hits = candidates
            .library
            .iter()
            .map(|(id, part)| candidates.project.get(id).unwrap_or(part))
            .filter(matches)
            .collect();
    }
    let chosen: Vec<&Part> = if hits.len() > 1 && !manufacturer.is_empty() {
        hits.iter()
            .copied()
            .filter(|p| p.manufacturer.trim().eq_ignore_ascii_case(manufacturer))
            .collect()
    } else {
        hits.clone()
    };
    match chosen.as_slice() {
        [one] => match unreadable(&one.id) {
            Some(why) => Err(why),
            None => Ok(one.id.clone()),
        },
        [] if hits.is_empty() => Err(
            "no part of the project or its libraries has this article number; import the maker's catalogue (BMEcat, EDZ or CSV) into a library first"
                .into(),
        ),
        _ => {
            let ids: Vec<&str> = if chosen.is_empty() {
                hits.iter().map(|p| p.id.as_str()).collect()
            } else {
                chosen.iter().map(|p| p.id.as_str()).collect()
            };
            Err(format!("several parts have this article number ({})", ids.join(", ")))
        }
    }
}

/// Write `plan` onto the project's devices (a drawn device without an entry gets one). Returns
/// how many devices changed. `plan` must come from this project as it is: accessory positions
/// are those it had then.
pub fn apply(project: &mut Project, plan: &Plan) -> usize {
    let mut changed: BTreeSet<&DeviceTag> = BTreeSet::new();
    // New accessories come last in a plan, so a device keeps one only when a replacement
    // that was planned (and not dropped since) does not keep it already.
    for a in &plan.assignments {
        if put(project.devices.entry(a.tag.clone()).or_default(), a) {
            changed.insert(&a.tag);
        }
    }
    changed.len()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use wc_core::geom::{Point, Rotation};
    use wc_core::symbol::{Pin, PinDir, SymbolRole};
    use wc_core::{Page, PlacedSymbol, Symbol};

    fn tag(t: &str) -> DeviceTag {
        DeviceTag::parse(t).unwrap()
    }

    fn part(id: &str, maker: &str, number: &str) -> (String, Part) {
        (
            id.to_owned(),
            Part {
                id: id.into(),
                manufacturer: maker.into(),
                order_number: number.into(),
                ..Default::default()
            },
        )
    }

    /// Terminals -X1.1 to -X1.3 and two terminals tagged only -X2; -X1.1 has a part, -X1.2 a
    /// marker (2), the strip -X1 an end plate, and -X1.1 and -X1.2 are jumpered.
    fn project() -> (Project, BTreeMap<String, Symbol>) {
        let mut p = Project::new("x");
        let mut page = Page::new("p1", "1", "Terminals");
        for (n, (id, t)) in [
            ("a", "-X1.1"),
            ("b", "-X1.2"),
            ("c", "-X1.3"),
            ("d", "-X2"),
            ("e", "-X2"),
        ]
        .into_iter()
        .enumerate()
        {
            page.symbols.push(PlacedSymbol {
                id: id.into(),
                symbol: "term".into(),
                at: Point::new(20.0 + 10.0 * n as f64, 50.0),
                rotation: Rotation::R0,
                mirror: false,
                tag: Some(tag(t)),
            });
        }
        p.pages.push(page);
        p.devices.insert(
            tag("-X1.1"),
            Device {
                part: Some("wago.2002-1201".into()),
                ..Default::default()
            },
        );
        p.devices.insert(
            tag("-X1.2"),
            Device {
                accessories: vec![DevicePart {
                    part: "wago.marker".into(),
                    qty: 2,
                }],
                ..Default::default()
            },
        );
        p.devices.insert(
            tag("-X1"),
            Device {
                accessories: vec![DevicePart {
                    part: "wago.2002-1291".into(),
                    qty: 1,
                }],
                ..Default::default()
            },
        );
        p.meta
            .add_jumper(&tag("-X1"), &["1".to_owned(), "2".to_owned()])
            .unwrap();
        let term = Symbol {
            id: "term".into(),
            name: "Terminal".into(),
            category: String::new(),
            letter_code: "X".into(),
            role: SymbolRole::Terminal,
            graphics: vec![],
            pins: ["1", "2"]
                .iter()
                .enumerate()
                .map(|(n, name)| Pin {
                    name: (*name).into(),
                    at: Point::new(0.0, 10.0 * n as f64),
                    dir: PinDir::Down,
                })
                .collect(),
            tag_at: Point::default(),
            description: String::new(),
        };
        (p, [("term".to_owned(), term)].into_iter().collect())
    }

    fn candidates() -> BTreeMap<String, Part> {
        [
            part("wago.2002-1201", "WAGO", "2002-1201"),
            part("wago.2002-1301", "WAGO", "2002-1301"),
            part("wago.2002-1291", "WAGO", "2002-1291"),
            part("wago.249-116", "WAGO", "249-116"),
            part("wago.marker", "WAGO", "793-501"),
            part("acme.dup", "Acme", "DUP-1"),
            part("phoenix.dup", "Phoenix", "dup-1 "),
        ]
        .into_iter()
        .collect()
    }

    /// `parts` as the libraries' parts, the project having none of its own.
    fn library(parts: &BTreeMap<String, Part>) -> Candidates {
        Candidates {
            library: parts.clone(),
            ..Default::default()
        }
    }

    #[test]
    fn a_configured_list_puts_its_articles_on_the_devices_and_lists_the_rest() {
        let (mut p, symbols) = project();
        let parts = candidates();
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        // The export, with articles chosen as the configurator would.
        let mut t = wc_report::configurator_list(&inputs).table;
        let indexes: Vec<&str> = t.rows.iter().map(|r| r[0].as_str()).collect();
        assert_eq!(
            indexes,
            [
                "-X1/T1",
                "-X1/T2",
                "-X1/T3",
                "-X1/J1",
                "-X1/A1",
                "-X1/T2/A1",
                "-X2/T1",
                "-X2/T2"
            ]
        );
        let choose = |t: &mut wc_report::Table, index: &str, article: &str, maker: &str, qty: &str| {
            let r = t.rows.iter_mut().find(|r| r[0] == index).unwrap();
            r[6] = article.into();
            r[7] = maker.into();
            if !qty.is_empty() {
                r[9] = qty.into();
            }
        };
        choose(&mut t, "-X1/T1", "2002-1301", "WAGO", "");
        choose(&mut t, "-X1/T2", "2002-1201", "WAGO", "");
        choose(&mut t, "-X1/T3", "DUP-1", "Phoenix", "");
        choose(&mut t, "-X1/J1", "2002-402", "WAGO", "");
        choose(&mut t, "-X1/A1", "2002-1291", "WAGO", "2");
        choose(&mut t, "-X2/T1", "2002-1201", "WAGO", "");
        choose(&mut t, "-X2/T2", "2002-1301", "WAGO", "");
        // Rows the configurator added, and one whose strip is gone.
        let extra = |index: &str, strip: &str, article: &str, qty: &str| {
            let mut r = vec![String::new(); wc_report::CONFIGURATOR_COLUMNS.len()];
            (r[0], r[1], r[2], r[6], r[9]) = (
                index.into(),
                "Accessory".into(),
                strip.into(),
                article.into(),
                qty.into(),
            );
            r
        };
        t.rows.push(extra("", "-X1", "249-116", "2"));
        t.rows.push(extra("", "-X1", "999-999", "1"));
        t.rows.push(extra("-X9/A1", "-X9", "249-116", "x"));
        let bytes = wc_report::to_smartscript_csv(&t);

        let (rows, warnings) = read_configured(bytes.as_bytes()).unwrap();
        assert_eq!(rows.len(), 11);
        assert_eq!(warnings.len(), 1, "the quantity x: {warnings:?}");
        assert!(warnings[0].contains("invalid digit"), "the reason: {warnings:?}");
        let plan = plan(&inputs, &library(&parts), &rows);
        assert_eq!(plan.unchanged, 1, "the marker: {plan:?}");
        assert_eq!(plan.unmatched.len(), 5, "{:#?}", plan.unmatched);
        for (n, why) in [
            "jumpers have no part",
            "neither is taken",
            "no part of the project or its libraries",
            "the project has no such row",
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                plan.unmatched.iter().any(|u| u.contains(why)),
                "{n}: {why} in {:#?}",
                plan.unmatched
            );
        }
        assert_eq!(apply(&mut p, &plan), 4);
        let device = |t: &str| p.devices.get(&tag(t)).cloned().unwrap_or_default();
        assert_eq!(device("-X1.1").part.as_deref(), Some("wago.2002-1301"));
        assert_eq!(device("-X1.2").part.as_deref(), Some("wago.2002-1201"));
        assert_eq!(device("-X1.2").accessories.len(), 1, "the marker stays");
        assert_eq!(device("-X1.3").part.as_deref(), Some("phoenix.dup"), "by maker");
        let acc: Vec<(String, u32)> = device("-X1").accessories.into_iter().map(|a| (a.part, a.qty)).collect();
        assert_eq!(acc, [("wago.2002-1291".to_owned(), 2), ("wago.249-116".to_owned(), 2)]);
        assert!(!p.devices.contains_key(&tag("-X2")), "its rows disagree");

        // The same file again changes nothing.
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        let again = super::plan(&inputs, &library(&parts), &rows);
        assert!(again.assignments.is_empty(), "{again:#?}");
    }

    #[test]
    fn a_row_that_no_longer_matches_the_project_is_not_taken() {
        let (p, symbols) = project();
        let parts = candidates();
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        let text = "Index;Type;Terminal;Article\n\
                    -X1/T2;Terminal;7;2002-1201\n\
                    -X1/A1;Terminal;;2002-1201\n\
                    -X1/T1;;;DUP-1\n\
                    '-X1/T3;Terminal;3;wago.2002-1301\n";
        let (rows, _) = read_configured(text.as_bytes()).unwrap();
        let plan = plan(&inputs, &library(&parts), &rows);
        assert_eq!(plan.unmatched.len(), 3, "{:#?}", plan.unmatched);
        assert!(plan.unmatched[0].contains("terminal 7 in the file but terminal 2"));
        assert!(plan.unmatched[1].contains("type is Terminal in the file but Accessory"));
        assert!(plan.unmatched[2].contains("several parts") && plan.unmatched[2].contains("acme.dup"));
        // A part id is taken as it is, and the guard quote is dropped.
        assert_eq!(plan.assignments.len(), 1);
        assert_eq!(plan.assignments[0].tag, tag("-X1.3"));
        assert_eq!(plan.assignments[0].part, "wago.2002-1301");
        assert!(read_configured(b"Article;Quantity\n1;2\n").is_err());
        assert!(read_configured(b"").is_err());
    }

    #[test]
    fn the_projects_own_part_wins_over_a_library_part_with_the_same_article() {
        let (p, symbols) = project();
        let parts = candidates();
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        let mut c = library(&parts);
        let (id, mine) = part("own.terminal", "WAGO", "2002-1201");
        c.project.insert(id, mine);
        let rows = read_configured(b"Index;Article\n-X1/T3;2002-1201\n").unwrap().0;
        let plan = plan(&inputs, &c, &rows);
        assert_eq!(plan.assignments.len(), 1, "{plan:#?}");
        assert_eq!(plan.assignments[0].part, "own.terminal");

        // A part the project locks but cannot read is not given to a device, also when a
        // library has it.
        c.unreadable.insert("wago.2002-1301".into(), "bad json".into());
        let rows = read_configured(b"Index;Article\n-X1/T3;2002-1301\n-X1/T2;wago.2002-1301\n")
            .unwrap()
            .0;
        let plan = super::plan(&inputs, &c, &rows);
        assert!(plan.assignments.is_empty(), "{plan:#?}");
        assert_eq!(plan.unmatched.len(), 2);
        assert!(plan.unmatched.iter().all(|u| u.contains("cannot be read (bad json)")));
    }

    #[test]
    fn an_ambiguous_article_stays_ambiguous_when_the_list_is_imported_again() {
        let (p, symbols) = project();
        let parts = candidates();
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        let mut c = library(&parts);
        for (id, part) in [part("dup.a", "", "DUP-1"), part("dup.b", "", "DUP-1")] {
            c.library.insert(id, part);
        }
        let rows = read_configured(b"Index;Article\n-X1/T3;dup.a\n-X1/T2;DUP-1\n")
            .unwrap()
            .0;
        let first = plan(&inputs, &c, &rows);
        assert_eq!(first.assignments.len(), 1, "{first:#?}");
        assert!(first.unmatched[0].contains("several parts"), "{first:#?}");
        // The first import took dup.a into the project; the same list gives the same answer.
        c.project.insert("dup.a".into(), c.library["dup.a"].clone());
        let again = plan(&inputs, &c, &rows);
        assert_eq!(again.unmatched, first.unmatched);
    }

    #[test]
    fn a_new_accessory_is_checked_against_the_device_after_the_other_rows() {
        let (mut p, symbols) = project();
        let parts = candidates();
        // -X1's accessory 2002-1291 is replaced, and a new row asks for 2002-1291 again.
        let rows = read_configured(b"Index;Article;Strip\n-X1/A1;249-116;-X1\n;2002-1291;-X1\n")
            .unwrap()
            .0;
        let first = {
            let inputs = Inputs {
                project: &p,
                symbols: &symbols,
                parts: &parts,
            };
            plan(&inputs, &library(&parts), &rows)
        };
        assert_eq!(first.assignments.len(), 2, "{first:#?}");
        assert_eq!(apply(&mut p, &first), 1);
        let had: Vec<&str> = p.devices[&tag("-X1")]
            .accessories
            .iter()
            .map(|a| a.part.as_str())
            .collect();
        assert_eq!(had, ["wago.249-116", "wago.2002-1291"]);
        // Again: nothing to change.
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        let again = plan(&inputs, &library(&parts), &rows);
        assert!(again.assignments.is_empty(), "{again:#?}");
        assert_eq!(again.unchanged, 2);
        // When the replacement is dropped (its part could not be taken), the new row adds nothing.
        let (mut p, _) = project();
        let mut dropped = first.clone();
        dropped.assignments.retain(|a| a.part != "wago.249-116");
        assert_eq!(apply(&mut p, &dropped), 0);
        assert_eq!(p.devices[&tag("-X1")].accessories.len(), 1);
    }

    #[test]
    fn a_library_whose_parts_cannot_be_listed_is_reported_and_the_others_still_count() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        let mut p = store::create(&dir, "Test").unwrap();
        let good = tmp.path().join("good");
        let (_, terminal) = part("wago.2002-1201", "WAGO", "2002-1201");
        wc_library::write_part(&good, &terminal).unwrap();
        // A file where the parts folder should be cannot be listed.
        let bad = tmp.path().join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join(ItemKind::Part.dir()), "not a folder").unwrap();
        for (name, root) in [("bad", &bad), ("good", &good)] {
            p.meta.libraries.push(wc_core::project::LibrarySource {
                name: name.into(),
                kind: "folder".into(),
                location: root.to_string_lossy().into_owned(),
                reference: None,
            });
        }
        let (c, warnings) = Candidates::load(&dir, &p);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("the parts of library bad cannot be listed"),
            "{warnings:?}"
        );
        assert!(c.library.contains_key("wago.2002-1201"), "{c:?}");
    }

    #[test]
    fn a_part_is_taken_as_it_was_matched_even_when_the_library_changed_since() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        let mut p = store::create(&dir, "Test").unwrap();
        let lib = tmp.path().join("lib");
        let (_, matched) = part("wago.2002-1201", "WAGO", "2002-1201");
        wc_library::write_part(&lib, &matched).unwrap();
        p.meta.libraries.push(wc_core::project::LibrarySource {
            name: "wago".into(),
            kind: "folder".into(),
            location: lib.to_string_lossy().into_owned(),
            reference: None,
        });
        let (c, warnings) = Candidates::load(&dir, &p);
        assert!(warnings.is_empty(), "{warnings:?}");
        // The library file changes after the article was matched.
        let (_, changed) = part("wago.2002-1201", "WAGO", "9999-0000");
        wc_library::write_part(&lib, &changed).unwrap();
        c.take(&dir, &mut p, "wago.2002-1201").unwrap();
        let taken = store::read_vendored_part(&dir, "wago.2002-1201").unwrap();
        assert_eq!(taken.order_number, "2002-1201");
        let entry = &p.lock.parts["wago.2002-1201"];
        assert_eq!(
            (entry.source.as_str(), entry.hash.as_str()),
            ("wago", matched.hash().as_str())
        );
        assert!(c.take(&dir, &mut p, "no.such").is_err());
    }

    #[test]
    fn two_rows_with_different_quantities_for_one_accessory_take_neither() {
        let (p, symbols) = project();
        let parts = candidates();
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        for text in [
            "Index;Article;Quantity\n-X1/A1;2002-1291;3\n-X1/A1;2002-1291;4\n",
            "Index;Article;Quantity\n-X1/A1;2002-1291;4\n-X1/A1;2002-1291;3\n",
        ] {
            let rows = read_configured(text.as_bytes()).unwrap().0;
            let plan = plan(&inputs, &library(&parts), &rows);
            assert!(plan.assignments.is_empty(), "{text}: {plan:#?}");
            // Both rows are listed.
            assert_eq!(plan.unmatched.len(), 2, "{plan:#?}");
            assert!(
                plan.unmatched.iter().all(|u| u.contains("choose one quantity")),
                "{plan:#?}"
            );
            assert!(plan.unmatched.iter().any(|u| u.starts_with("line 2 ")), "{plan:#?}");
            assert!(plan.unmatched.iter().any(|u| u.starts_with("line 3 ")), "{plan:#?}");
        }
        // Every row of a slot rows disagree on is listed once, whatever the order.
        for text in [
            "Index;Article;Quantity\n-X1/A1;2002-1291;3\n-X1/A1;2002-1291;4\n-X1/A1;2002-1291;3\n",
            "Index;Article;Quantity\n-X1/A1;2002-1291;3\n-X1/A1;2002-1291;3\n-X1/A1;2002-1291;4\n",
        ] {
            let rows = read_configured(text.as_bytes()).unwrap().0;
            let plan = plan(&inputs, &library(&parts), &rows);
            assert!(plan.assignments.is_empty(), "{text}: {plan:#?}");
            for line in ["line 2 ", "line 3 ", "line 4 "] {
                assert_eq!(
                    plan.unmatched.iter().filter(|u| u.starts_with(line)).count(),
                    1,
                    "{line} in {text}: {plan:#?}"
                );
            }
            assert_eq!(plan.unmatched.len(), 3, "{plan:#?}");
        }
        // A third row that disagrees too is listed, and the first is still listed only once.
        let text = "Index;Article;Quantity\n-X1/A1;2002-1291;3\n-X1/A1;2002-1291;4\n-X1/A1;2002-1291;5\n";
        let rows = read_configured(text.as_bytes()).unwrap().0;
        let plan = plan(&inputs, &library(&parts), &rows);
        assert!(plan.assignments.is_empty(), "{plan:#?}");
        assert_eq!(plan.unmatched.len(), 3, "{plan:#?}");
        assert_eq!(plan.unmatched.iter().filter(|u| u.starts_with("line 2 ")).count(), 1);
        // A quantity too large for a count is reported with the reason.
        let (_, warnings) = read_configured(b"Index;Article;Quantity\n-X1/A1;2002-1291;4294967296\n").unwrap();
        assert!(warnings[0].contains("too large"), "{warnings:?}");
    }

    #[test]
    fn an_accessory_row_whose_terminal_moved_is_not_taken() {
        let (p, symbols) = project();
        let parts = candidates();
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        // Exported when -X1.2 was at position 3 (a terminal before it was removed since): the
        // index now names another terminal's accessory.
        let rows = read_configured(
            b"Index;Type;Article;Quantity;Connects\n-X1/T2/A1;Accessory;793-501;1;3\n-X1/A1;Accessory;2002-1291;1;2\n",
        )
        .unwrap()
        .0;
        let plan = plan(&inputs, &library(&parts), &rows);
        assert!(plan.assignments.is_empty(), "{plan:#?}");
        assert!(plan.unmatched[0].contains("belongs to terminal 3 in the file but to terminal 2"));
        assert!(plan.unmatched[1].contains("terminal 2 in the file but to the strip"));
        // The same with the right terminal is taken.
        let rows = read_configured(b"Index;Article;Quantity;Connects\n-X1/T2/A1;793-501;5;2\n")
            .unwrap()
            .0;
        let plan = super::plan(&inputs, &library(&parts), &rows);
        assert_eq!(plan.assignments.len(), 1, "{plan:#?}");
        assert_eq!(plan.assignments[0].qty, 5);
        // Without the Connects column a terminal's accessory is not taken; the strip's is.
        let rows = read_configured(b"Index;Article;Quantity\n-X1/T2/A1;793-501;5\n-X1/A1;2002-1291;2\n")
            .unwrap()
            .0;
        let plan = super::plan(&inputs, &library(&parts), &rows);
        assert_eq!(plan.unmatched.len(), 1, "{plan:#?}");
        assert!(plan.unmatched[0].contains("no Connects column"), "{plan:#?}");
        assert_eq!(plan.assignments.len(), 1, "{plan:#?}");
        assert_eq!(plan.assignments[0].slot, Slot::Accessory(0));
    }
}

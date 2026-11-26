//! The WAGO configurator list back: the articles and accessories chosen in the configurator
//! (smartDESIGNER), written onto the project's terminal and strip devices (#123) and its jumpers
//! (#237).
//!
//! `wiringcraft export wago-configurator` writes one row per item with an `Index` that stays the
//! same as long as the item does (`-X1/T:3` for terminal 3 wherever it is on the rail,
//! `-X1/A2`). The list comes back with the
//! Article (and Quantity) columns filled in; [`read_configured`] reads it, [`plan`] matches each
//! row to the device part it stands for by that index and the article to a part of the project
//! or its libraries, and [`apply`] writes the result. Everything that cannot be matched is listed
//! with the reason, never guessed. See `docs/wago-configurator.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use wc_core::lock::ItemKind;
use wc_core::project::Jumper;
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
            // A library without a parts folder has no parts; one that is not there, or whose
            // parts folder cannot be listed, is reported, or its parts would look missing.
            for s in &lib.sources {
                if let Err(why) = std::fs::read_dir(&s.root) {
                    warnings.push(format!(
                        "library {} cannot be read ({}): {why}",
                        source.name,
                        s.root.display()
                    ));
                    continue;
                }
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
                    // A file that holds another part is no candidate: matched by article, it
                    // would stand for a part it is not.
                    Ok((part, _)) if part.id != id => warnings.push(format!(
                        "library part {id} cannot be used: its file declares id {:?}",
                        part.id
                    )),
                    Ok((part, _)) => {
                        out.sources.insert(id.clone(), n);
                        out.library.insert(id, part);
                    }
                    Err(why) => warnings.push(format!("library part {id} cannot be read: {why}")),
                }
            }
        }
        // The project's copies: every pinned one, and one left without its pin (an undone change
        // took the pin back) or used without one, which is matched and pinned as it is, never
        // replaced by a newer library version.
        let mut ids: std::collections::BTreeSet<String> = project.lock.parts.keys().cloned().collect();
        ids.extend(project.used_parts());
        let folder = dir.join(store::VENDOR_DIR).join(ItemKind::Part.dir());
        match std::fs::read_dir(&folder) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(e) => {
                            if let Some(id) = e.file_name().to_str().and_then(|n| n.strip_suffix(".json")) {
                                ids.insert(id.to_owned());
                            }
                        }
                        Err(why) => warnings.push(format!(
                            "the project's part copies cannot all be listed ({}): {why}",
                            folder.display()
                        )),
                    }
                }
            }
            Err(why) if why.kind() == std::io::ErrorKind::NotFound => {}
            Err(why) => warnings.push(format!(
                "the project's part copies cannot be listed ({}): {why}",
                folder.display()
            )),
        }
        let used = project.used_parts();
        for id in ids {
            let pinned = project.lock.parts.contains_key(&id);
            // A copy left without its pin and used nowhere can be pinned only to a library that
            // has the part: one none has would stand in for a library part it cannot replace.
            if !pinned && !used.contains(&id) && !out.library.contains_key(&id) {
                continue;
            }
            let path = match store::vendor_path(dir, ItemKind::Part, &id) {
                Ok(path) => path,
                Err(_) if !pinned => continue,
                Err(why) => {
                    out.unreadable.insert(id, why.to_string());
                    continue;
                }
            };
            // Only a plain file is read: anything else (a folder, a pipe) could block or fail. An
            // entry that is there but cannot be followed (a link to nothing) is unreadable, not
            // absent, so a library part never replaces it.
            let unreadable = match std::fs::symlink_metadata(&path) {
                Err(why) if why.kind() == std::io::ErrorKind::NotFound && !pinned => continue,
                Err(why) if why.kind() == std::io::ErrorKind::NotFound => None,
                Err(why) => Some(why.to_string()),
                Ok(_) => match std::fs::metadata(&path) {
                    Ok(m) if !m.is_file() => Some("it is not a file".to_owned()),
                    Ok(_) => None,
                    Err(why) => Some(why.to_string()),
                },
            };
            if let Some(why) = unreadable {
                warnings.push(format!("the project's copy of part {id} cannot be read: {why}"));
                out.unreadable.insert(id, why);
                continue;
            }
            match store::read_vendored_part(dir, &id) {
                Ok(part) if part.id == id => {
                    out.project.insert(id, part);
                }
                Ok(part) => {
                    let why = format!("the file declares id {:?}", part.id);
                    warnings.push(format!("the project's copy of part {id} cannot be used: {why}"));
                    out.unreadable.insert(id, why);
                }
                Err(why) => {
                    warnings.push(format!("the project's copy of part {id} cannot be read: {why}"));
                    out.unreadable.insert(id, why.to_string());
                }
            }
        }
        (out, warnings)
    }

    /// Make sure the project in `dir` holds part `id`, pinned: a copy the project has without
    /// its pin is pinned as it is, to the library that has the part; a library part is copied
    /// into the project and pinned as it was read here, the copy its article was matched on
    /// (not a fresh read, which may have changed since). A copy that cannot be read is never
    /// replaced.
    pub fn take(&self, dir: &Path, project: &mut Project, id: &str) -> std::result::Result<(), String> {
        if project.lock.parts.contains_key(id) {
            return Ok(());
        }
        if let Some(why) = self.unreadable.get(id) {
            return Err(format!("the project's copy of part {id} cannot be read: {why}"));
        }
        let source = self
            .sources
            .get(id)
            .and_then(|&n| project.meta.libraries.get(n))
            .map(|l| l.name.clone());
        if let Some(copy) = self.project.get(id) {
            let source = source.ok_or_else(|| {
                format!("part {id} has a copy in the project but no pin, and no library has it to pin it to")
            })?;
            project.lock.parts.insert(
                id.to_owned(),
                wc_core::lock::LockEntry {
                    source,
                    hash: copy.hash(),
                },
            );
            return Ok(());
        }
        let (Some(part), Some(source)) = (self.library.get(id), source) else {
            return Err(format!("no library has part {id}"));
        };
        if part.id != id {
            return Err(format!("the library's file for part {id} declares id {:?}", part.id));
        }
        store::write_vendored_part(dir, part).map_err(|e| e.to_string())?;
        project.lock.parts.insert(
            id.to_owned(),
            wc_core::lock::LockEntry {
                source,
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
    /// The part of the strip's jumper at this position (from 0) among the strip's jumpers; the
    /// assignment's tag is the strip.
    Jumper(usize),
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
    // An index written before #340 held the terminal's place on the rail (`-X1/T3`): the row
    // there now, by old index → (its index now, its terminal number).
    let by_position: BTreeMap<String, (String, String)> = now
        .table
        .rows
        .iter()
        .filter(|r| r.get(1).is_some_and(|k| k == "Terminal"))
        .filter_map(|r| {
            let cell = |k: usize| r.get(k).map(|c| unguard(c));
            Some((format!("{}/T{}", cell(2)?, cell(3)?), (cell(0)?, cell(4)?)))
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
            // Only accessories can be new: a jumper row needs its index to say which jumper.
            if row.kind.eq_ignore_ascii_case("Jumper") {
                miss("a jumper row without an index cannot be matched to a jumper; export the list again".into());
                continue;
            }
            // Without a Type the row could be a jumper that lost its index: its Connects says so.
            if row.kind.is_empty() && row.connects.as_deref().is_some_and(|c| !c.trim().is_empty()) {
                miss("a row without an index or a type but with terminals in Connects may be a jumper; export the list again".into());
                continue;
            }
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
            // A row from a list written before #340: its terminal is the one at that place on
            // the rail now only when the row's terminal number says so.
            // An old accessory row checked by its Connects above: the terminal that names its
            // device now may be another of the device's terminals.
            let mut legacy_accessory = false;
            let index = match old_index(&row.index) {
                None => row.index.clone(),
                Some((terminal_at, accessory)) => {
                    let Some((index, number)) = by_position.get(terminal_at) else {
                        miss("the project has no such row now (the strip changed?); export the list again".into());
                        continue;
                    };
                    // A terminal says which it is by its number, an accessory by its Connects
                    // only: its Terminal cell is not the terminal it belongs to.
                    let empty = String::new();
                    let file = match accessory {
                        None => &row.terminal,
                        Some(_) => row.connects.as_ref().unwrap_or(&empty),
                    };
                    if file.is_empty() {
                        miss(format!(
                            "the list was written by an older version whose index is the terminal's place on the rail, and the row has no {} to tell which terminal it is; export the list again",
                            if accessory.is_some() {
                                "Connects"
                            } else {
                                "Terminal number"
                            }
                        ));
                        continue;
                    }
                    if file != number {
                        miss(format!(
                            "the row is terminal {file} in the file but terminal {number} is at that place on the rail now; export the list again"
                        ));
                        continue;
                    }
                    match accessory {
                        None => index.clone(),
                        // The accessory slot of that terminal's device, whichever terminal
                        // names it now (a device several terminals are drawn as is named by
                        // its lowest number).
                        Some(a) => {
                            let slot = a
                                .strip_prefix("/A")
                                .and_then(|n| n.parse::<usize>().ok())
                                .and_then(|n| n.checked_sub(1));
                            let found = match (now.targets.get(index), slot) {
                                (Some(Target::Part(tag)), Some(k)) => now
                                    .targets
                                    .iter()
                                    .find(|(_, t)| matches!(t, Target::Accessory(d, j) if d == tag && *j == k))
                                    .map(|(i, _)| i.clone()),
                                _ => None,
                            };
                            let Some(found) = found else {
                                miss(
                                    "the project has no such row now (the strip changed?); export the list again"
                                        .into(),
                                );
                                continue;
                            };
                            legacy_accessory = true;
                            found
                        }
                    }
                }
            };
            let Some((kind, terminal, connects)) = current.get(&index) else {
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
            // An accessory's index names its terminal; a Connects that names another one (a
            // row moved by hand) is not guessed at.
            if kind == "Accessory"
                && !legacy_accessory
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
            // A jumper's index is its position among the strip's jumpers, which moves when a
            // jumper before it is added or taken apart: it must still join the same terminals.
            if kind == "Jumper" {
                match &row.connects {
                    None => {
                        miss(
                            "the file has no Connects column, so the terminals this jumper joins cannot be checked; export the list again"
                                .into(),
                        );
                        continue;
                    }
                    Some(file) if !same_terminals(file, connects) => {
                        miss(format!(
                            "the jumper joins terminals {file} in the file but {connects} in the project now; export the list again"
                        ));
                        continue;
                    }
                    Some(_) => {}
                }
            }
            match now.targets.get(&index) {
                Some(Target::Part(t)) => (t.clone(), Slot::Part),
                Some(Target::Accessory(t, k)) => (t.clone(), Slot::Accessory(*k)),
                Some(Target::Jumper(t, n)) => (t.clone(), Slot::Jumper(*n)),
                None => {
                    miss(match &**kind {
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
            Slot::Jumper(n) => (jumper(i.project, &tag, *n).and_then(|j| j.part.clone()), 1),
            Slot::Part => (device.and_then(|d| d.part.clone()), 1),
            Slot::Accessory(k) => device
                .and_then(|d| d.accessories.get(*k))
                .map_or((None, 1), |a| (Some(a.part.clone()), a.qty)),
            Slot::NewAccessory => (None, 1),
        };
        let qty = match slot {
            Slot::Part | Slot::Jumper(_) => 1,
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
    for a in out.assignments.iter().filter(|a| !matches!(a.slot, Slot::Jumper(_))) {
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
    // The reasons quote cells of the file as they are: a control character in one (a terminal
    // escape sequence) is shown escaped, so printing the list cannot change the terminal.
    for u in &mut out.unmatched {
        *u = escape_controls(u);
    }
    out
}

/// `text` with every control character written as its escape (`\u{1b}`), the rest as it is.
fn escape_controls(text: &str) -> String {
    if !text.chars().any(char::is_control) {
        return text.to_owned();
    }
    text.chars()
        .map(|c| {
            if c.is_control() {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// The `n`-th jumper (from 0) of `strip`, in the project's order.
fn jumper<'a>(project: &'a Project, strip: &DeviceTag, n: usize) -> Option<&'a Jumper> {
    project.meta.jumpers.iter().filter(|j| &j.strip == strip).nth(n)
}

/// An index as lists written before #340 had it, tying a row to the terminal's place on the
/// rail: `-X1/T3` or `-X1/T3/A1`. Returns the terminal's part (`-X1/T3`) and the accessory's
/// (`/A1`), if any.
fn old_index(index: &str) -> Option<(&str, Option<&str>)> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let (terminal, accessory) = match index.rsplit_once("/A") {
        Some((head, n)) if digits(n) => (head, Some(index.get(head.len()..)?)),
        _ => (index, None),
    };
    let (strip, position) = terminal.rsplit_once("/T")?;
    (!strip.is_empty() && digits(position)).then_some((terminal, accessory))
}

/// Whether two Connects cells name the same terminals (`1, 2` and `1,2` do).
fn same_terminals(a: &str, b: &str) -> bool {
    let list = |s: &str| -> Vec<String> {
        s.split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .collect()
    };
    list(a) == list(b)
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
        // Jumpers are not devices: see [`apply`].
        Slot::Jumper(_) => return false,
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

/// Write `plan` onto the project's devices (a drawn device without an entry gets one) and
/// jumpers. Returns how many devices and jumpers changed. `plan` must come from this project as
/// it is: accessory and jumper positions are those it had then.
pub fn apply(project: &mut Project, plan: &Plan) -> usize {
    let mut changed: BTreeSet<&DeviceTag> = BTreeSet::new();
    let mut jumpers = 0;
    // New accessories come last in a plan, so a device keeps one only when a replacement
    // that was planned (and not dropped since) does not keep it already.
    for a in &plan.assignments {
        if let Slot::Jumper(n) = a.slot {
            let j = project.meta.jumpers.iter_mut().filter(|j| j.strip == a.tag).nth(n);
            if let Some(j) = j
                && j.part.as_deref() != Some(a.part.as_str())
            {
                j.part = Some(a.part.clone());
                jumpers += 1;
            }
            continue;
        }
        if put(project.devices.entry(a.tag.clone()).or_default(), a) {
            changed.insert(&a.tag);
        }
    }
    changed.len() + jumpers
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
            source: String::new(),
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
                "-X1/T:1",
                "-X1/T:2",
                "-X1/T:3",
                "-X1/J1",
                "-X1/A1",
                "-X1/T:2/A1",
                "-X2/T:1",
                "-X2/T:2"
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
        choose(&mut t, "-X1/T:1", "2002-1301", "WAGO", "");
        choose(&mut t, "-X1/T:2", "2002-1201", "WAGO", "");
        choose(&mut t, "-X1/T:3", "DUP-1", "Phoenix", "");
        choose(&mut t, "-X1/J1", "2002-402", "WAGO", "");
        choose(&mut t, "-X1/A1", "2002-1291", "WAGO", "2");
        choose(&mut t, "-X2/T:1", "2002-1201", "WAGO", "");
        choose(&mut t, "-X2/T:2", "2002-1301", "WAGO", "");
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
    fn a_jumper_row_gives_the_strips_jumper_its_article() {
        let (mut p, symbols) = project();
        // A second jumper of -X1 and one of -X2, which must keep their (lack of) part.
        p.meta.jumpers.push(Jumper {
            strip: tag("-X1"),
            terminals: vec!["3".into(), "4".into()],
            part: None,
        });
        p.meta.jumpers.push(Jumper {
            strip: tag("-X2"),
            terminals: vec!["1".into(), "2".into()],
            part: None,
        });
        let mut parts = candidates();
        parts.extend([part("wago.2002-402", "WAGO", "2002-402")]);
        let lib = library(&parts);
        let list = |p: &Project| {
            let inputs = Inputs {
                project: p,
                symbols: &symbols,
                parts: &parts,
            };
            let mut t = wc_report::configurator_list(&inputs).table;
            let r = t.rows.iter_mut().find(|r| r[0] == "-X1/J1").unwrap();
            assert_eq!(r[1], "Jumper");
            assert_eq!(r[14], "1, 2");
            r[6] = "2002-402".into();
            wc_report::to_smartscript_csv(&t)
        };
        let bytes = list(&p);
        let (rows, _) = read_configured(bytes.as_bytes()).unwrap();
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        let planned = plan(&inputs, &lib, &rows);
        assert!(planned.unmatched.is_empty(), "{:#?}", planned.unmatched);
        assert_eq!(planned.assignments.len(), 1, "{planned:#?}");
        assert_eq!(planned.assignments[0].slot, Slot::Jumper(0));
        assert_eq!(planned.parts().into_iter().collect::<Vec<_>>(), ["wago.2002-402"]);
        assert_eq!(apply(&mut p, &planned), 1);
        assert_eq!(p.meta.jumpers[0].part.as_deref(), Some("wago.2002-402"));
        assert!(p.meta.jumpers[1..].iter().all(|j| j.part.is_none()), "only the first");
        assert!(
            p.devices.get(&tag("-X1")).is_some_and(|d| d.part.is_none()),
            "not the strip"
        );

        // The same list again changes nothing.
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        let again = plan(&inputs, &lib, &rows);
        assert!(again.assignments.is_empty(), "{again:#?}");
        assert_eq!(again.unchanged, planned.unchanged + 1, "the jumper is up to date now");
        assert_eq!(apply(&mut p, &again), 0);
    }

    #[test]
    fn a_jumper_row_whose_terminals_changed_is_not_taken() {
        let (mut p, symbols) = project();
        let mut parts = candidates();
        parts.extend([part("wago.2002-402", "WAGO", "2002-402")]);
        let text = "Index;Type;Article;Connects\n\
                    -X1/J1;Jumper;2002-402;1,2\n";
        let (rows, _) = read_configured(text.as_bytes()).unwrap();
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        let ok = plan(&inputs, &library(&parts), &rows);
        assert_eq!(ok.assignments.len(), 1, "spaces in Connects do not matter: {ok:#?}");

        // The jumper now joins 1 to 3: the row stood for 1 and 2.
        p.meta
            .add_jumper(&tag("-X1"), &["2".to_owned(), "3".to_owned()])
            .unwrap();
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        let moved = plan(&inputs, &library(&parts), &rows);
        assert!(moved.assignments.is_empty(), "{moved:#?}");
        assert_eq!(moved.unmatched.len(), 1);
        assert!(
            moved.unmatched[0].contains("joins terminals 1,2 in the file but 1, 2, 3 in the project now"),
            "{:#?}",
            moved.unmatched
        );

        // A control character in a cell is shown escaped in the reason.
        let text = "Index;Type;Article;Connects\n-X1/J1;Jumper;2002-402;1\u{1b}[2J,2\n";
        let (rows, _) = read_configured(text.as_bytes()).unwrap();
        let escaped = plan(&inputs, &library(&parts), &rows);
        assert_eq!(escaped.unmatched.len(), 1);
        assert!(!escaped.unmatched[0].contains('\u{1b}'), "{:?}", escaped.unmatched);
        assert!(
            escaped.unmatched[0].contains("1\\u{1b}[2J,2"),
            "{:?}",
            escaped.unmatched
        );

        // A jumper row without an index is not taken as a new accessory of the strip.
        let text = "Index;Type;Strip;Article;Connects\n;Jumper;-X1;2002-402;1, 2\n";
        let (rows, _) = read_configured(text.as_bytes()).unwrap();
        let blank = plan(&inputs, &library(&parts), &rows);
        assert!(blank.assignments.is_empty(), "{blank:#?}");
        assert!(blank.unmatched[0].contains("without an index"), "{:?}", blank.unmatched);

        // Nor is one that has lost its type too but still names terminals.
        let text = "Index;Strip;Article;Connects\n;-X1;2002-402;1, 2\n";
        let (rows, _) = read_configured(text.as_bytes()).unwrap();
        let untyped = plan(&inputs, &library(&parts), &rows);
        assert!(untyped.assignments.is_empty(), "{untyped:#?}");
        assert_eq!(untyped.unmatched.len(), 1, "{:?}", untyped.unmatched);
        assert!(
            untyped.unmatched[0].contains("may be a jumper"),
            "{:?}",
            untyped.unmatched
        );

        // Without a Connects column the jumper cannot be checked.
        let text = "Index;Type;Article\n-X1/J1;Jumper;2002-402\n";
        let (rows, _) = read_configured(text.as_bytes()).unwrap();
        let blind = plan(&inputs, &library(&parts), &rows);
        assert!(blind.assignments.is_empty());
        assert!(
            blind.unmatched[0].contains("no Connects column"),
            "{:#?}",
            blind.unmatched
        );
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
                    -X1/T:2;Terminal;7;2002-1201\n\
                    -X1/A1;Terminal;;2002-1201\n\
                    -X1/T:1;;;DUP-1\n\
                    '-X1/T:3;Terminal;3;wago.2002-1301\n";
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
        let rows = read_configured(b"Index;Article\n-X1/T:3;2002-1201\n").unwrap().0;
        let plan = plan(&inputs, &c, &rows);
        assert_eq!(plan.assignments.len(), 1, "{plan:#?}");
        assert_eq!(plan.assignments[0].part, "own.terminal");

        // A part the project locks but cannot read is not given to a device, also when a
        // library has it.
        c.unreadable.insert("wago.2002-1301".into(), "bad json".into());
        let rows = read_configured(b"Index;Article\n-X1/T:3;2002-1301\n-X1/T:2;wago.2002-1301\n")
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
        let rows = read_configured(b"Index;Article\n-X1/T:3;dup.a\n-X1/T:2;DUP-1\n")
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
        // A library that is not there at all, and one that has no parts (which is fine).
        let gone = tmp.path().join("gone");
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        for (name, root) in [("bad", &bad), ("gone", &gone), ("empty", &empty), ("good", &good)] {
            p.meta.libraries.push(wc_core::project::LibrarySource {
                name: name.into(),
                kind: "folder".into(),
                location: root.to_string_lossy().into_owned(),
                reference: None,
            });
        }
        let (c, warnings) = Candidates::load(&dir, &p);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[0].contains("the parts of library bad cannot be listed"),
            "{warnings:?}"
        );
        assert!(warnings[1].contains("library gone cannot be read"), "{warnings:?}");
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
    fn a_copy_left_without_its_pin_is_matched_and_pinned_as_it_is() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        let mut p = store::create(&dir, "Test").unwrap();
        let lib = tmp.path().join("lib");
        p.meta.libraries.push(wc_core::project::LibrarySource {
            name: "wago".into(),
            kind: "folder".into(),
            location: lib.to_string_lossy().into_owned(),
            reference: None,
        });
        // The project keeps v1 without its pin (an undone import); the library has v2 now.
        let (_, v1) = part("wago.2002-1201", "WAGO", "2002-1201");
        store::write_vendored_part(&dir, &v1).unwrap();
        let mut v2 = v1.clone();
        v2.description = "Changed".into();
        wc_library::write_part(&lib, &v2).unwrap();
        // And a library file named after one part that declares another.
        let (_, other) = part("wago.other", "WAGO", "2002-9999");
        let path = wc_library::write_part(&lib, &other).unwrap();
        std::fs::rename(&path, lib.join("parts").join("wago.renamed.json")).unwrap();
        let (c, warnings) = Candidates::load(&dir, &p);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("wago.renamed") && warnings[0].contains("declares id"));
        assert!(
            !c.library.contains_key("wago.renamed"),
            "no candidate under a name it is not"
        );
        assert_eq!(
            c.project.get("wago.2002-1201"),
            Some(&v1),
            "the project's copy is a candidate"
        );
        c.take(&dir, &mut p, "wago.2002-1201").unwrap();
        assert_eq!(store::read_vendored_part(&dir, "wago.2002-1201").unwrap(), v1, "kept");
        assert_eq!(p.lock.parts["wago.2002-1201"].hash, v1.hash());
        assert!(c.take(&dir, &mut p, "wago.renamed").is_err());
        assert!(
            store::read_vendored_part(&dir, "wago.other").is_err(),
            "nothing written"
        );
        assert!(!p.lock.parts.contains_key("wago.renamed"));
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
    fn a_list_exported_before_the_strip_was_reordered_still_puts_each_article_on_its_terminal() {
        let (mut p, symbols) = project();
        let parts = candidates();
        let x1 = tag("-X1");
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        // Exported, and the articles chosen: -X1.1 gets 2002-1301, -X1.3 gets 2002-1201.
        let mut t = wc_report::configurator_list(&inputs).table;
        for (index, article) in [("-X1/T:1", "2002-1301"), ("-X1/T:3", "2002-1201")] {
            t.rows.iter_mut().find(|r| r[0] == index).unwrap()[6] = article.into();
        }
        let text: String = std::iter::once("Index;Article".to_owned())
            .chain(t.rows.iter().map(|r| format!("{};{}", r[0], r[6])))
            .collect::<Vec<_>>()
            .join("\n");
        // Then the strip is put in another order on the rail.
        assert_eq!(
            p.meta.set_terminal_order(&x1, &["3".into(), "1".into(), "2".into()]),
            Ok(true)
        );
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        let rows = read_configured(text.as_bytes()).unwrap().0;
        let plan = plan(&inputs, &library(&parts), &rows);
        assert!(plan.unmatched.is_empty(), "{plan:#?}");
        let mut got: Vec<(String, &str)> = plan
            .assignments
            .iter()
            .map(|a| (a.tag.to_string(), a.part.as_str()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            [
                ("-X1.1".to_owned(), "wago.2002-1301"),
                ("-X1.3".to_owned(), "wago.2002-1201")
            ]
        );

        // A list written before #340 indexed the rows by place on the rail. With the Terminal
        // column, a row whose terminal is still at its place is taken, one whose place now holds
        // another terminal is not.
        let old = "Index;Terminal;Article\n-X1/T1;1;2002-1301\n-X1/T2;2;2002-1201\n";
        let rows = read_configured(old.as_bytes()).unwrap().0;
        let plan = super::plan(&inputs, &library(&parts), &rows);
        assert_eq!(plan.assignments.len(), 0, "{plan:#?}");
        assert_eq!(plan.unmatched.len(), 2, "{plan:#?}");
        assert!(
            plan.unmatched[0].contains("terminal 1 in the file but terminal 3 is at that place"),
            "{plan:#?}"
        );
        let old = "Index;Terminal;Article\n-X1/T2;1;2002-1301\n";
        let rows = read_configured(old.as_bytes()).unwrap().0;
        let plan = super::plan(&inputs, &library(&parts), &rows);
        assert!(plan.unmatched.is_empty(), "{plan:#?}");
        assert_eq!(plan.assignments.len(), 1, "{plan:#?}");
        assert_eq!(plan.assignments[0].tag, tag("-X1.1"));
        // Without it, nothing tells which terminal the row was: refused, not put on whatever is
        // at that place now.
        let old = "Index;Article\n-X1/T1;2002-1301\n";
        let rows = read_configured(old.as_bytes()).unwrap().0;
        let plan = super::plan(&inputs, &library(&parts), &rows);
        assert!(plan.assignments.is_empty(), "{plan:#?}");
        assert!(plan.unmatched[0].contains("older version"), "{plan:#?}");
    }

    #[test]
    fn an_old_accessory_row_of_terminals_drawn_as_the_strip_is_found_through_their_device() {
        use wc_core::DevicePart;
        let (mut p, symbols) = project();
        let parts = candidates();
        let x2 = tag("-X2");
        p.devices.entry(x2.clone()).or_default().accessories = vec![DevicePart {
            part: "wago.2002-1301".into(),
            qty: 1,
        }];
        // Exported before #340 with the rail [2, 1]: the accessory was named by terminal 2,
        // first on the rail, as -X2/T1/A1. Now it is named by terminal 1.
        assert_eq!(p.meta.set_terminal_order(&x2, &["2".into(), "1".into()]), Ok(true));
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        let rows = read_configured(b"Index;Article;Quantity;Connects\n-X2/T1/A1;793-501;4;2\n")
            .unwrap()
            .0;
        let plan = plan(&inputs, &library(&parts), &rows);
        assert!(plan.unmatched.is_empty(), "{plan:#?}");
        assert_eq!(plan.assignments.len(), 1, "{plan:#?}");
        assert_eq!(
            (&plan.assignments[0].tag, &plan.assignments[0].slot),
            (&x2, &Slot::Accessory(0))
        );
        assert_eq!(plan.assignments[0].qty, 4);
        // A Connects that is not the terminal at that place is still refused.
        let rows = read_configured(b"Index;Article;Quantity;Connects\n-X2/T1/A1;793-501;4;1\n")
            .unwrap()
            .0;
        let plan = super::plan(&inputs, &library(&parts), &rows);
        assert!(plan.assignments.is_empty(), "{plan:#?}");
        assert!(
            plan.unmatched[0].contains("terminal 1 in the file but terminal 2 is at that place"),
            "{plan:#?}"
        );
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
        // A list written before #340, when -X1.3 was at position 2 (a terminal before it was
        // removed since): the place on the rail now holds another terminal's accessory.
        let rows = read_configured(
            b"Index;Type;Article;Quantity;Connects\n-X1/T2/A1;Accessory;793-501;1;3\n-X1/A1;Accessory;2002-1291;1;2\n",
        )
        .unwrap()
        .0;
        let plan = plan(&inputs, &library(&parts), &rows);
        assert!(plan.assignments.is_empty(), "{plan:#?}");
        assert!(
            plan.unmatched[0].contains("terminal 3 in the file but terminal 2 is at that place"),
            "{plan:#?}"
        );
        assert!(plan.unmatched[1].contains("terminal 2 in the file but to the strip"));
        // The same with the right terminal is taken, under the old index and the new one.
        for text in [
            &b"Index;Article;Quantity;Connects\n-X1/T2/A1;793-501;5;2\n"[..],
            b"Index;Article;Quantity;Connects\n-X1/T:2/A1;793-501;5;2\n",
        ] {
            let rows = read_configured(text).unwrap().0;
            let plan = super::plan(&inputs, &library(&parts), &rows);
            assert_eq!(plan.assignments.len(), 1, "{plan:#?}");
            assert_eq!(plan.assignments[0].qty, 5);
            assert_eq!(plan.assignments[0].tag, tag("-X1.2"));
        }
        // A new index names the terminal, so no Connects column is needed. An old one without
        // it cannot say which terminal it was, and is not taken.
        let rows =
            read_configured(b"Index;Article;Quantity\n-X1/T:2/A1;793-501;5\n-X1/T2/A1;793-501;5\n-X1/A1;2002-1291;2\n")
                .unwrap()
                .0;
        let plan = super::plan(&inputs, &library(&parts), &rows);
        assert_eq!(plan.unmatched.len(), 1, "{plan:#?}");
        assert!(plan.unmatched[0].starts_with("line 3 "), "{plan:#?}");
        assert!(plan.unmatched[0].contains("older version"), "{plan:#?}");
        assert_eq!(plan.assignments.len(), 2, "{plan:#?}");
        // An old one's Terminal cell does not stand in for Connects.
        let rows = read_configured(b"Index;Terminal;Article;Quantity\n-X1/T2/A1;2;793-501;5\n")
            .unwrap()
            .0;
        let plan = super::plan(&inputs, &library(&parts), &rows);
        assert!(plan.assignments.is_empty(), "{plan:#?}");
        assert!(plan.unmatched[0].contains("no Connects"), "{plan:#?}");
    }

    #[test]
    fn an_article_for_a_spare_terminal_survives_the_import_and_a_reload() {
        let (mut p, symbols) = project();
        let x1 = tag("-X1");
        let current: Vec<(String, bool)> = ["1", "2", "3"].iter().map(|n| ((*n).to_owned(), false)).collect();
        p.meta.add_spare(&x1, "S1", None, &current).unwrap();
        let parts = candidates();
        let inputs = Inputs {
            project: &p,
            symbols: &symbols,
            parts: &parts,
        };
        let out = wc_report::configurator_list(&inputs);
        let index = out
            .targets
            .iter()
            .find(|(_, t)| matches!(t, Target::Part(d) if *d == tag("-X1.S1")))
            .map(|(k, _)| k.clone())
            .unwrap();
        let mut t = out.table;
        let r = t.rows.iter_mut().find(|r| r[0] == index).unwrap();
        r[6] = "2002-1201".into();
        r[7] = "WAGO".into();
        let bytes = wc_report::to_smartscript_csv(&t);
        let (rows, _) = read_configured(bytes.as_bytes()).unwrap();
        let plan = plan(&inputs, &library(&parts), &rows);
        assert!(apply(&mut p, &plan) >= 1, "{plan:?}");
        assert_eq!(
            p.devices.get(&tag("-X1.S1")).and_then(|d| d.part.as_deref()),
            Some("wago.2002-1201")
        );
        // The device key of the spare loads again, with the spare still on its strip.
        let dir = tempfile::tempdir().unwrap();
        wc_core::store::save(dir.path(), &p).unwrap();
        let back = wc_core::store::load(dir.path()).unwrap();
        assert_eq!(
            back.devices.get(&tag("-X1.S1")).and_then(|d| d.part.as_deref()),
            Some("wago.2002-1201")
        );
        assert_eq!(
            back.meta.strip_layout(&x1).map(|l| l.spares.clone()),
            Some(vec!["S1".to_owned()])
        );
    }
}

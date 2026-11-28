//! Macros: reusable groups of symbols, wires and drawings, saved as one JSON file each in a
//! `macros/` folder of a project or a library source. Inserting one works like a paste: symbols
//! get fresh ids and the next free tags, and wires follow their symbols.
//!
//! A macro can hold placeholders, written `{{name}}` in its device descriptions, parts,
//! accessories and properties, its wires' cross-section, colour and type, and its drawn texts,
//! plus named value sets that fill them in. One macro then covers several variants (a motor
//! starter for 0.75, 1.5 and 3 kW); placing it asks which value set to use, see [`Macro::apply`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::geom::{Point, Shape};
use crate::project::{Device, Endpoint, PlacedSymbol, Project, Wire, WireSpec};
use crate::store::{check_id, read_json, to_json, write_if_changed};
use crate::{DeviceTag, Error, Result};

/// Folder holding macros, in a project or a library source.
pub const MACROS_DIR: &str = "macros";

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Macro {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<PlacedSymbol>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wires: Vec<Wire>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graphics: Vec<Shape>,
    /// Device data (description, part, properties) of the macro's tags.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub devices: BTreeMap<DeviceTag, Device>,
    /// Top-left corner of the group; inserting puts this corner where the user points.
    #[serde(default)]
    pub anchor: Point,
    /// The placeholders its texts may use as `{{name}}`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub placeholders: Vec<Placeholder>,
    /// Named sets of placeholder values, one per variant.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub value_sets: Vec<ValueSet>,
}

/// A value a macro leaves open, used in its texts as `{{name}}`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Placeholder {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Used when the chosen value set does not give it (or there are no value sets).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default: String,
}

/// One variant of a macro: a value for each of its placeholders.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ValueSet {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, String>,
}

impl Macro {
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty() && self.wires.is_empty() && self.graphics.is_empty()
    }

    /// The value set named `name`, matched without regard to case.
    pub fn value_set(&self, name: &str) -> Option<&ValueSet> {
        self.value_sets.iter().find(|v| key(&v.name) == key(name))
    }

    /// The names of the placeholders its texts use, in order of first use, whether declared
    /// or not.
    pub fn used_placeholders(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        self.for_each_text(|t| {
            for name in placeholder_names(t) {
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        });
        out
    }

    /// Check the placeholders and value sets: names are unique and not empty, value sets only
    /// fill declared placeholders, every placeholder the texts use is declared, symbol ids are
    /// unique and wires only end on the macro's own symbols.
    pub fn check(&self) -> Result<()> {
        let bad = |why: String| Err(Error::Invalid(format!("macro {:?}: {why}", self.name)));
        let mut names: Vec<&str> = Vec::new();
        for p in &self.placeholders {
            let n = p.name.trim();
            if n.is_empty() || n.contains("{{") || n.contains("}}") {
                return bad(format!("placeholder name {:?} is not usable", p.name));
            }
            if names.iter().any(|m| key(m) == key(n)) {
                return bad(format!("placeholder {n} is declared twice"));
            }
            names.push(n);
        }
        let mut symbols: BTreeSet<&str> = BTreeSet::new();
        for s in &self.symbols {
            if !symbols.insert(s.id.as_str()) {
                return bad(format!(
                    "two symbols share the id {}, so wires to it would join the wrong one",
                    s.id
                ));
            }
        }
        for w in &self.wires {
            for end in [&w.from, &w.to] {
                if let Endpoint::Pin { item, .. } = end
                    && !symbols.contains(item.as_str())
                {
                    return bad(format!("wire {} ends on {item}, which is not in the macro", w.id));
                }
            }
        }
        let mut open = None;
        self.for_each_text(|t| {
            if open.is_none() && unterminated(t) {
                open = Some(t.to_owned());
            }
        });
        if let Some(t) = open {
            return bad(format!(
                "{t:?} opens a placeholder with {{{{ but does not close it with }}}}"
            ));
        }
        let declared = |n: &str| names.iter().any(|m| key(m) == key(n));
        for used in self.used_placeholders() {
            if !declared(&used) {
                return bad(format!("{{{{{used}}}}} is used but not declared as a placeholder"));
            }
        }
        let mut sets: Vec<&str> = Vec::new();
        for v in &self.value_sets {
            let n = v.name.trim();
            if n.is_empty() {
                return bad("a value set has no name".into());
            }
            if sets.iter().any(|m| key(m) == key(n)) {
                return bad(format!("value set {n} is there twice"));
            }
            sets.push(n);
            if let Some(k) = v.values.keys().find(|k| !declared(k)) {
                return bad(format!("value set {n} fills {k}, which is not a placeholder"));
            }
        }
        Ok(())
    }

    /// The macro as placed with the value set named `set` (or with the placeholders' defaults
    /// when it has no value sets), then with `overrides` on top: every `{{name}}` in its texts
    /// is replaced and the placeholders and value sets are dropped. A macro with value sets
    /// needs one named; an unknown name, an undeclared placeholder or a placeholder left
    /// without a value is an error.
    pub fn apply(&self, set: Option<&str>, overrides: &BTreeMap<String, String>) -> Result<Macro> {
        self.check()?;
        let bad = |why: String| Err(Error::Invalid(format!("macro {:?}: {why}", self.name)));
        let chosen = match set.map(str::trim).filter(|s| !s.is_empty()) {
            Some(name) => match self.value_set(name) {
                Some(v) => Some(v),
                None => {
                    // Quoted, so a control character in a declared name is shown escaped.
                    let known: Vec<String> = self.value_sets.iter().map(|v| format!("{:?}", v.name)).collect();
                    return bad(if known.is_empty() {
                        format!("it has no value sets, so {name:?} cannot be chosen")
                    } else {
                        format!("no value set {name:?}; it has {}", known.join(", "))
                    });
                }
            },
            None if !self.value_sets.is_empty() => {
                let known: Vec<String> = self.value_sets.iter().map(|v| format!("{:?}", v.name)).collect();
                return bad(format!("choose a value set: {}", known.join(", ")));
            }
            None => None,
        };
        // A placeholder with neither a value in the set nor a default has no value; an empty
        // value given on purpose (in the set) is a value.
        let mut values: BTreeMap<String, String> = BTreeMap::new();
        for p in &self.placeholders {
            let given = chosen.and_then(|v| {
                v.values
                    .iter()
                    .find(|(k, _)| key(k) == key(&p.name))
                    .map(|(_, v)| v.clone())
            });
            if let Some(v) = given.or_else(|| Some(p.default.clone()).filter(|d| !d.is_empty())) {
                values.insert(key(&p.name), v);
            }
        }
        for (k, v) in overrides {
            if !self.placeholders.iter().any(|p| key(&p.name) == key(k)) {
                return bad(format!("{k:?} is not a placeholder"));
            }
            values.insert(key(k), v.clone());
        }
        let mut out = self.clone();
        out.placeholders.clear();
        out.value_sets.clear();
        let mut missing: Vec<String> = Vec::new();
        out.for_each_text_mut(|t| {
            if t.contains("{{") {
                *t = fill(t, &values, &mut missing);
            }
        });
        if !missing.is_empty() {
            let mut seen = BTreeSet::new();
            missing.retain(|n| seen.insert(key(n)));
            return bad(format!("no value for {}", missing.join(", ")));
        }
        // A wire value is stored as typed in anywhere else: trimmed, and no value when empty;
        // a device value that came out empty is no value.
        for w in &mut out.wires {
            for v in [&mut w.spec.section, &mut w.spec.colour, &mut w.spec.kind] {
                *v = v.as_deref().and_then(WireSpec::value);
            }
        }
        for d in out.devices.values_mut() {
            // A part id is kept trimmed, as `device --part` stores it, so it matches its pin.
            d.part = d.part.take().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
            for a in &mut d.accessories {
                a.part = a.part.trim().to_owned();
            }
            d.accessories.retain(|a| !a.part.is_empty());
            // An empty property is no property, as when typed empty in the device's form.
            d.properties.retain(|_, v| !v.trim().is_empty());
        }
        Ok(out)
    }

    /// Write `{{new}}` for each `{{old}}` in its texts (names matched without regard to case),
    /// so a renamed placeholder keeps its uses. The placeholder and value sets are left to
    /// the caller.
    pub fn rename_placeholder(&mut self, old: &str, new: &str) {
        self.rename_placeholders(&[(old, new)]);
    }

    /// [`Macro::rename_placeholder`] for several at once: each `(old, new)` is applied to the
    /// texts as they were, so a swap (`a` to `b`, `b` to `a`) or a chain renames each use once.
    pub fn rename_placeholders(&mut self, renames: &[(&str, &str)]) {
        let map: BTreeMap<String, String> = renames
            .iter()
            .map(|(old, new)| (key(old), new.trim().to_owned()))
            .collect();
        if map.is_empty() {
            return;
        }
        self.for_each_text_mut(|t| {
            if !placeholder_names(t).iter().any(|n| map.contains_key(&key(n))) {
                return;
            }
            let mut out = String::with_capacity(t.len());
            let mut rest = t.as_str();
            while let Some(start) = rest.find("{{") {
                let Some(after) = rest.get(start + 2..) else { break };
                let Some(end) = after.find("}}") else { break };
                let (Some(before), Some(name), Some(token), Some(next)) = (
                    rest.get(..start),
                    after.get(..end),
                    rest.get(start..start + 2 + end + 2),
                    after.get(end + 2..),
                ) else {
                    break;
                };
                out.push_str(before);
                match map.get(&key(name)) {
                    Some(new) => out.push_str(&format!("{{{{{new}}}}}")),
                    None => out.push_str(token),
                }
                rest = next;
            }
            out.push_str(rest);
            *t = out;
        });
    }

    /// Rename declared placeholder `old` to `new` (names matched without regard to case): its
    /// declaration, its values in the value sets and its uses in the texts.
    pub fn rename_declared(&mut self, old: &str, new: &str) -> Result<()> {
        let bad = |why: String| Err(Error::Invalid(format!("macro {:?}: {why}", self.name)));
        let new = new.trim();
        if new.is_empty() {
            return bad(format!("placeholder {} needs a new name", old.trim()));
        }
        let Some(i) = self.placeholders.iter().position(|p| key(&p.name) == key(old)) else {
            return bad(format!("no placeholder {}", old.trim()));
        };
        if self
            .placeholders
            .iter()
            .enumerate()
            .any(|(j, p)| j != i && key(&p.name) == key(new))
        {
            return bad(format!("placeholder {new} is there already"));
        }
        if let Some(p) = self.placeholders.get_mut(i) {
            p.name = new.to_owned();
        }
        // A set may give the placeholder under several spellings (`KW` and `kw`): all are
        // renamed into one, keeping the value placing uses, the first in key order.
        for v in &mut self.value_sets {
            let spellings: Vec<String> = v.values.keys().filter(|k| key(k) == key(old)).cloned().collect();
            let mut kept = None;
            for k in spellings {
                let value = v.values.remove(&k);
                kept = kept.or(value);
            }
            if let Some(value) = kept {
                v.values.insert(new.to_owned(), value);
            }
        }
        self.rename_placeholder(old, new);
        Ok(())
    }

    fn for_each_text(&self, mut f: impl FnMut(&str)) {
        let mut copy = self.clone();
        copy.for_each_text_mut(|t| f(t));
    }

    fn for_each_text_mut(&mut self, mut f: impl FnMut(&mut String)) {
        for d in self.devices.values_mut() {
            f(&mut d.description);
            if let Some(p) = &mut d.part {
                f(p);
            }
            for a in &mut d.accessories {
                f(&mut a.part);
            }
            for v in d.properties.values_mut() {
                f(v);
            }
        }
        for w in &mut self.wires {
            for v in [&mut w.spec.section, &mut w.spec.colour, &mut w.spec.kind]
                .into_iter()
                .flatten()
            {
                f(v);
            }
        }
        for g in &mut self.graphics {
            if let Shape::Text { text, .. } = g {
                f(text);
            }
        }
    }
}

/// Place `m` (already [applied](Macro::apply)) on page `page` with its anchor at `at`, as the
/// app's insert does: symbols get fresh ids and the next free tags like theirs, with the macro's
/// device data; wires follow their symbols and lose their numbers; drawings move along.
/// Returns the new symbol and wire ids. The symbols must be in the project already.
pub fn place(project: &mut Project, page: &str, m: &Macro, at: Point) -> Result<Vec<String>> {
    if !m.placeholders.is_empty() || !m.value_sets.is_empty() {
        return Err(Error::Invalid(format!(
            "macro {}: apply a value set before placing it",
            m.name
        )));
    }
    let Some(index) = project.pages.iter().position(|p| p.id == page) else {
        return Err(Error::Invalid(format!("no page {page}")));
    };
    let group = crate::placement::Group {
        symbols: &m.symbols,
        wires: &m.wires,
        graphics: &m.graphics,
        devices: &m.devices,
        anchor: m.anchor,
    };
    use crate::placement::{DeviceData, OutOfTags};
    let placed = crate::placement::place(project, index, group, at, DeviceData::GroupOnly, OutOfTags::Fail)?;
    Ok(placed.symbols.into_iter().chain(placed.wires).collect())
}

/// A macro of everything on page `page`: its symbols, its wires (their numbers cleared, as a
/// placed wire is numbered afresh), its drawings and the device data of the tags its symbols
/// carry and of the cables its wires are cores of, anchored at the page's top-left corner, so
/// [`place`] at `(0, 0)` draws each item where it was. The `{{name}}` placeholders its texts
/// use are declared, without defaults, so a placement fills them. A page with nothing on it is
/// refused, as no macro can be empty, and so is one whose macro [`Macro::check`] refuses.
pub fn from_page(project: &Project, page: &str, name: &str, description: &str) -> Result<Macro> {
    let Some(p) = project.page(page) else {
        return Err(Error::Invalid(format!("no page {page}")));
    };
    if p.symbols.is_empty() && p.wires.is_empty() && p.graphics.is_empty() {
        return Err(Error::Invalid(format!(
            "page {page} has no symbols, wires or drawings to make a macro of"
        )));
    }
    let devices = p
        .symbols
        .iter()
        .filter_map(|s| s.tag.as_ref())
        .chain(p.wires.iter().filter_map(|w| w.cable.as_ref().map(|c| &c.cable)))
        .filter_map(|t| project.devices.get(t).map(|d| (t.clone(), d.clone())))
        .collect();
    let mut m = Macro {
        name: name.trim().to_owned(),
        description: description.trim().to_owned(),
        symbols: p.symbols.clone(),
        wires: p
            .wires
            .iter()
            .cloned()
            .map(|mut w| {
                w.number = None;
                w
            })
            .collect(),
        graphics: p.graphics.clone(),
        devices,
        anchor: Point::new(0.0, 0.0),
        placeholders: Vec::new(),
        value_sets: Vec::new(),
    };
    // One placeholder for names alike but for case, as `apply` fills them; the first spelling.
    let mut seen = BTreeSet::new();
    m.placeholders = m
        .used_placeholders()
        .into_iter()
        .filter(|name| seen.insert(key(name)))
        .map(|name| Placeholder {
            name,
            ..Default::default()
        })
        .collect();
    m.check()?;
    Ok(m)
}

/// How placeholder and value set names are compared: trimmed, without regard to case.
pub fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Whether `text` has a `{{` that no `}}` closes.
fn unterminated(text: &str) -> bool {
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { return true };
        rest = &after[end + 2..];
    }
    false
}

/// The names written as `{{name}}` in `text`, trimmed, in order.
fn placeholder_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        out.push(after[..end].trim().to_owned());
        rest = &after[end + 2..];
    }
    out
}

/// `text` with each `{{name}}` replaced by its value (names matched without regard to case).
/// Names without a value are added to `missing` and left as written.
fn fill(text: &str, values: &BTreeMap<String, String>, missing: &mut Vec<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        out.push_str(&rest[..start]);
        let name = after[..end].trim();
        match values.get(&key(name)) {
            Some(v) => out.push_str(v),
            None => {
                missing.push(name.to_owned());
                out.push_str(&rest[start..start + 2 + end + 2]);
            }
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// A file id for a macro name: lower case letters, digits and dashes (Nordic and other
/// accented letters spelled plainly).
pub fn macro_id(name: &str) -> String {
    let mut id = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        let plain = match c {
            'æ' => "ae",
            'ø' | 'ö' | 'ó' | 'ò' | 'ô' => "o",
            'å' | 'ä' | 'á' | 'à' | 'â' => "a",
            'é' | 'è' | 'ê' | 'ë' => "e",
            'ü' | 'ú' | 'ù' | 'û' => "u",
            'í' | 'ì' | 'î' | 'ï' => "i",
            'ß' => "ss",
            c if c.is_ascii_alphanumeric() => {
                id.push(c);
                continue;
            }
            _ => "",
        };
        if !plain.is_empty() {
            id.push_str(plain);
        } else if !id.ends_with('-') && !id.is_empty() {
            id.push('-');
        }
    }
    let id: String = id.chars().take(100).collect();
    let id = id.trim_end_matches('-');
    if id.is_empty() {
        "macro".into()
    } else if check_id(id).is_err() {
        // A name Windows keeps for a device (`CON`, `AUX`, `COM1`) cannot be a file name.
        format!("{id}-macro")
    } else {
        id.to_owned()
    }
}

/// Save a macro as `<root>/macros/<id>.json`. Returns the path.
pub fn write_macro(root: &Path, id: &str, m: &Macro) -> Result<PathBuf> {
    check_id(id)?;
    if m.is_empty() {
        return Err(Error::Invalid(
            "a macro needs at least one symbol, wire or drawing".into(),
        ));
    }
    let path = root.join(MACROS_DIR).join(format!("{id}.json"));
    write_if_changed(&path, &to_json(m)?)?;
    Ok(path)
}

/// Check `m` with [`Macro::check`] and write it to `path`, a macro file in a `macros` folder.
/// Returns the path.
pub fn write_macro_file(path: &Path, m: &Macro) -> Result<PathBuf> {
    m.check()?;
    let not_macro = || Error::Invalid(format!("{} is not a macro file in a macros folder", path.display()));
    let id = path.file_stem().and_then(|s| s.to_str()).ok_or_else(not_macro)?;
    let root = path
        .parent()
        .filter(|d| d.file_name().is_some_and(|n| n == MACROS_DIR))
        .and_then(Path::parent)
        .ok_or_else(not_macro)?;
    write_macro(root, id, m)
}

pub fn read_macro(path: &Path) -> Result<Macro> {
    read_json(path)
}

/// The macros in `<root>/macros`, by id, sorted. Files that are not valid ids are skipped.
pub fn list_macros(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(root.join(MACROS_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let id = name.to_str()?.strip_suffix(".json")?.to_owned();
            check_id(&id).ok()?;
            Some((id, e.path()))
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::DevicePart;

    fn err_of<T>(r: Result<T>) -> String {
        r.err().map(|e| e.to_string()).unwrap_or_default()
    }

    #[test]
    fn ids_saving_and_listing() -> Result<()> {
        assert_eq!(macro_id("  Motor starter, DOL 3~ "), "motor-starter-dol-3");
        assert_eq!(macro_id("Ølkjøler Ærø"), "olkjoler-aero");
        assert_eq!(macro_id("!!!"), "macro");
        let tmp = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let empty = Macro {
            name: "Empty".into(),
            ..Default::default()
        };
        assert!(write_macro(tmp.path(), "empty", &empty).is_err());
        let tag = DeviceTag::parse("-K1")?;
        let m = Macro {
            name: "Relay".into(),
            symbols: vec![PlacedSymbol {
                id: "s1".into(),
                symbol: "wc.coil".into(),
                at: Point::new(20.0, 30.0),
                rotation: Default::default(),
                mirror: false,
                tag: Some(tag.clone()),
            }],
            wires: vec![Wire {
                id: "w1".into(),
                from: Endpoint::Pin {
                    item: "s1".into(),
                    pin: "A1".into(),
                },
                to: Endpoint::Point {
                    at: Point::new(20.0, 10.0),
                },
                via: vec![],
                number: None,
                potential: None,
                cable: None,
                route: Default::default(),
                spec: Default::default(),
            }],
            devices: [(tag, Device::default())].into_iter().collect(),
            anchor: Point::new(10.0, 10.0),
            ..Default::default()
        };
        let path = write_macro(tmp.path(), "relay", &m)?;
        assert_eq!(read_macro(&path)?, m);
        assert!(write_macro(tmp.path(), "../escape", &m).is_err());
        std::fs::write(tmp.path().join(MACROS_DIR).join("notes.txt"), "x").map_err(|e| Error::io("w", e))?;
        let ids: Vec<String> = list_macros(tmp.path()).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, ["relay"]);
        assert!(list_macros(&tmp.path().join("missing")).is_empty());
        Ok(())
    }

    /// A motor starter: contactor -Q1 with a wire and a text, sized by `kw`.
    fn starter() -> Result<Macro> {
        let tag = DeviceTag::parse("-Q1")?;
        let device = Device {
            part: Some("{{part}}".into()),
            description: "Motor starter {{kW}} kW".into(),
            properties: [("Setting".to_owned(), "{{ current }} A".to_owned())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        Ok(Macro {
            name: "Starter".into(),
            description: String::new(),
            symbols: vec![PlacedSymbol {
                id: "s1".into(),
                symbol: "wc.contactor".into(),
                at: Point::new(20.0, 30.0),
                rotation: Default::default(),
                mirror: false,
                tag: Some(tag.clone()),
            }],
            wires: vec![Wire {
                id: "w1".into(),
                from: Endpoint::Pin {
                    item: "s1".into(),
                    pin: "1".into(),
                },
                to: Endpoint::Point {
                    at: Point::new(20.0, 10.0),
                },
                via: vec![],
                number: Some("7".into()),
                potential: None,
                cable: None,
                route: Default::default(),
                spec: WireSpec {
                    section: Some("{{section}}".into()),
                    colour: Some("BK".into()),
                    kind: None,
                },
            }],
            graphics: vec![Shape::Text {
                at: Point::new(30.0, 30.0),
                text: "{{kW}} kW".into(),
                height: 2.5,
                align: Default::default(),
                angle: 0.0,
            }],
            devices: [(tag, device)].into_iter().collect(),
            anchor: Point::new(10.0, 10.0),
            placeholders: ["kW", "part", "current", "section"]
                .into_iter()
                .map(|n| Placeholder {
                    name: n.into(),
                    default: if n == "section" {
                        "1.5 mm²".into()
                    } else {
                        String::new()
                    },
                    ..Default::default()
                })
                .collect(),
            value_sets: vec![
                ValueSet {
                    name: "0.75 kW".into(),
                    values: [("kW", "0.75"), ("part", "SIE.3RV2011-1AA10"), ("current", "2")]
                        .into_iter()
                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                        .collect(),
                    ..Default::default()
                },
                ValueSet {
                    name: "3 kW".into(),
                    values: [
                        ("KW", "3"),
                        ("part", "SIE.3RV2011-1GA10"),
                        ("current", "6.3"),
                        ("section", "2.5 mm²"),
                    ]
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect(),
                    ..Default::default()
                },
            ],
        })
    }

    #[test]
    fn value_sets_fill_the_placeholders() -> Result<()> {
        let m = starter()?;
        m.check()?;
        assert_eq!(m.used_placeholders(), ["kW", "part", "current", "section"]);
        let tmp = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let path = write_macro(tmp.path(), "starter", &m)?;
        assert_eq!(read_macro(&path)?, m);

        let none = BTreeMap::new();
        let err = err_of(m.apply(None, &none));
        assert!(err.contains("choose a value set: \"0.75 kW\", \"3 kW\""), "{err}");
        assert!(m.apply(Some("5 kW"), &none).is_err());

        let small = m.apply(Some("0.75 KW"), &none)?;
        assert!(small.placeholders.is_empty() && small.value_sets.is_empty());
        let d = &small.devices[&DeviceTag::parse("-Q1")?];
        assert_eq!(d.part.as_deref(), Some("SIE.3RV2011-1AA10"));
        assert_eq!(d.description, "Motor starter 0.75 kW");
        assert_eq!(d.properties["Setting"], "2 A");
        assert_eq!(small.wires[0].spec.section.as_deref(), Some("1.5 mm²"));
        assert!(matches!(&small.graphics[0], Shape::Text { text, .. } if text == "0.75 kW"));

        let big = m.apply(Some("3 kW"), &none)?;
        let d = &big.devices[&DeviceTag::parse("-Q1")?];
        assert_eq!(d.part.as_deref(), Some("SIE.3RV2011-1GA10"));
        assert_eq!(d.description, "Motor starter 3 kW");
        assert_eq!(big.wires[0].spec.section.as_deref(), Some("2.5 mm²"));

        // A typed value wins over the set; one for a name that is not a placeholder is refused.
        let typed: BTreeMap<String, String> = [("current".to_owned(), "5.5".to_owned())].into_iter().collect();
        let d = m.apply(Some("3 kW"), &typed)?.devices[&DeviceTag::parse("-Q1")?].clone();
        assert_eq!(d.properties["Setting"], "5.5 A");
        let wrong: BTreeMap<String, String> = [("voltage".to_owned(), "400".to_owned())].into_iter().collect();
        assert!(m.apply(Some("3 kW"), &wrong).is_err());
        Ok(())
    }

    #[test]
    fn a_wire_value_from_a_placeholder_is_stored_trimmed() -> Result<()> {
        let m = starter()?;
        let spaced: BTreeMap<String, String> = [("section".to_owned(), " 2.5 mm² ".to_owned())].into_iter().collect();
        let w = &m.apply(Some("3 kW"), &spaced)?.wires[0];
        assert_eq!(w.spec.section.as_deref(), Some("2.5 mm²"));
        assert_eq!(w.spec.colour.as_deref(), Some("BK"));
        let blank: BTreeMap<String, String> = [("section".to_owned(), "  ".to_owned())].into_iter().collect();
        assert_eq!(m.apply(Some("3 kW"), &blank)?.wires[0].spec.section, None);

        // Colour and type are trimmed the same way.
        let mut m = m;
        m.wires[0].spec.colour = Some("{{colour}}".into());
        m.wires[0].spec.kind = Some("{{kind}}".into());
        m.placeholders.extend(["colour", "kind"].map(|name| Placeholder {
            name: name.into(),
            ..Default::default()
        }));
        let padded: BTreeMap<String, String> = [
            ("section".to_owned(), "1.5".to_owned()),
            ("colour".to_owned(), " BU ".to_owned()),
            ("kind".to_owned(), " H07V-K ".to_owned()),
        ]
        .into_iter()
        .collect();
        let w = &m.apply(Some("3 kW"), &padded)?.wires[0];
        assert_eq!(w.spec.colour.as_deref(), Some("BU"));
        assert_eq!(w.spec.kind.as_deref(), Some("H07V-K"));
        Ok(())
    }

    #[test]
    fn a_renamed_placeholder_keeps_its_uses() -> Result<()> {
        let mut m = starter()?;
        m.rename_placeholder(" KW ", "power");
        assert_eq!(m.used_placeholders(), ["power", "part", "current", "section"]);
        let d = &m.devices[&DeviceTag::parse("-Q1")?];
        assert_eq!(d.description, "Motor starter {{power}} kW");
        // Other placeholders and plain braces are left as written.
        assert_eq!(d.properties["Setting"], "{{ current }} A");
        Ok(())
    }

    #[test]
    fn placeholders_renamed_together_are_each_renamed_once() -> Result<()> {
        // A swap: each use takes the other's name, not both the same one.
        let mut m = starter()?;
        m.rename_placeholders(&[("kW", "part"), ("part", "kW")]);
        assert_eq!(m.used_placeholders(), ["part", "kW", "current", "section"]);
        // A chain: kW becomes current and current becomes amps, each use once.
        let mut m = starter()?;
        m.rename_placeholders(&[("kW", "current"), ("current", "amps")]);
        assert_eq!(m.used_placeholders(), ["current", "part", "amps", "section"]);
        Ok(())
    }

    #[test]
    fn placeholders_are_checked() -> Result<()> {
        let none = BTreeMap::new();
        // Without value sets the defaults are used, and a placeholder without one has no value.
        let mut m = starter()?;
        m.value_sets.clear();
        let err = err_of(m.apply(None, &none));
        assert!(err.contains("no value for kW, part, current"), "{err}");
        let all: BTreeMap<String, String> = [("kw", "1.5"), ("part", ""), ("current", "3.2")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let placed = m.apply(None, &all)?;
        // An empty part is no part.
        assert_eq!(placed.devices[&DeviceTag::parse("-Q1")?].part, None);
        // A part typed with spaces around it is kept trimmed.
        let mut spaced = all.clone();
        spaced.insert("part".into(), " SIE.3RV2011-1AA10 ".into());
        let placed = m.apply(None, &spaced)?;
        assert_eq!(
            placed.devices[&DeviceTag::parse("-Q1")?].part.as_deref(),
            Some("SIE.3RV2011-1AA10")
        );
        // Accessory parts are trimmed too, and one that comes out blank is dropped.
        let mut extras = m.clone();
        if let Some(d) = extras.devices.values_mut().next() {
            d.accessories = vec![
                DevicePart {
                    part: " nowhere.cap ".into(),
                    qty: 2,
                },
                DevicePart {
                    part: "  ".into(),
                    qty: 1,
                },
            ];
        }
        let placed = extras.apply(None, &all)?;
        assert_eq!(
            placed.devices[&DeviceTag::parse("-Q1")?].accessories,
            [DevicePart {
                part: "nowhere.cap".into(),
                qty: 2
            }]
        );
        // A property that comes out empty is no property; one with text is kept.
        let mut blank = m.clone();
        if let Some(d) = blank.devices.values_mut().next() {
            d.properties.insert("Address".into(), " {{part}} ".into());
        }
        let d = blank.apply(None, &all)?.devices[&DeviceTag::parse("-Q1")?].clone();
        assert!(!d.properties.contains_key("Address"), "{:?}", d.properties);
        assert_eq!(d.properties["Setting"], "3.2 A");
        assert!(m.apply(Some("3 kW"), &none).is_err());

        let mut undeclared = starter()?;
        undeclared.placeholders.retain(|p| p.name != "current");
        undeclared.value_sets.iter_mut().for_each(|v| {
            v.values.remove("current");
        });
        let err = err_of(undeclared.check());
        // The name is quoted (escaped), so odd characters in it cannot reach a terminal raw.
        assert!(
            err.contains("{{current}} is used") && err.contains(&format!("macro {:?}:", undeclared.name)),
            "{err}"
        );
        assert!(err.contains("macro \""), "{err}");

        let mut twice = starter()?;
        twice.placeholders.push(Placeholder {
            name: "KW".into(),
            ..Default::default()
        });
        assert!(twice.check().is_err());

        // A placeholder left open is refused, and a closed one still works.
        let mut open = starter()?;
        if let Some(d) = open.devices.values_mut().next() {
            d.part = Some("{{part".into());
        }
        assert!(err_of(open.check()).contains("does not close"));
        assert!(open.apply(Some("3 kW"), &none).is_err());
        assert!(starter()?.apply(Some("3 kW"), &none).is_ok());

        // A wire ending on a symbol the macro does not hold is refused.
        let mut dangling = starter()?;
        dangling.wires[0].from = Endpoint::Pin {
            item: "s9".into(),
            pin: "1".into(),
        };
        assert!(err_of(dangling.check()).contains("s9"));

        // Two symbols with one id are refused: placing would hang the wires on the wrong one.
        let mut shared = starter()?;
        let first = shared.symbols[0].id.clone();
        let mut twin = shared.symbols[0].clone();
        twin.tag = None;
        shared.symbols.push(twin);
        let err = err_of(shared.check());
        assert!(err.contains(&format!("two symbols share the id {first}")), "{err}");
        assert!(shared.apply(Some("3 kW"), &none).is_err());

        let mut stray = starter()?;
        stray.value_sets[0].values.insert("voltage".into(), "400".into());
        assert!(err_of(stray.check()).contains("voltage"));

        // Placing needs the values applied.
        let mut p = Project::new("x");
        p.pages.push(crate::project::Page::new("p1", "1", "Motors"));
        assert!(place(&mut p, "p1", &starter()?, Point::new(0.0, 0.0)).is_err());
        Ok(())
    }

    #[test]
    fn a_declared_placeholder_is_renamed_everywhere_and_written_checked() -> Result<()> {
        let mut m = starter()?;
        m.rename_declared("kw", "power")?;
        assert_eq!(m.placeholders[0].name, "power");
        assert_eq!(m.used_placeholders(), ["power", "part", "current", "section"]);
        assert!(m.value_sets.iter().all(|v| v.values.contains_key("power")));
        assert!(m.value_sets.iter().all(|v| !v.values.keys().any(|k| key(k) == "kw")));
        m.check()?;
        assert!(err_of(m.rename_declared("power", "Part")).contains("there already"));
        let err = err_of(m.rename_declared("volts", "v"));
        assert!(
            err.contains("no placeholder volts") && err.contains(&format!("macro {:?}:", m.name)),
            "{err}"
        );
        assert!(err.contains("macro \""), "{err}");

        // A set giving the placeholder under two spellings keeps the value placing used.
        let mut twice = starter()?;
        if let Some(v) = twice.value_sets.get_mut(1) {
            v.values.insert("kw".into(), "4".into());
        }
        twice.check()?;
        let before = twice.apply(Some("3 kW"), &BTreeMap::new())?;
        twice.rename_declared("kW", "power")?;
        twice.check()?;
        let values = &twice.value_sets[1].values;
        assert_eq!(values.get("power").map(String::as_str), Some("3"));
        assert!(!values.keys().any(|k| key(k) == "kw"), "{values:?}");
        assert_eq!(twice.apply(Some("3 kW"), &BTreeMap::new())?, before);

        let tmp = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let path = write_macro(tmp.path(), "starter", &m)?;
        assert_eq!(write_macro_file(&path, &m)?, path);
        let mut broken = m.clone();
        broken.placeholders.remove(0);
        assert!(err_of(write_macro_file(&path, &broken)).contains("{{power}} is used"));
        assert_eq!(read_macro(&path)?, m, "an unchecked macro is not written");
        assert!(write_macro_file(&tmp.path().join("starter.json"), &m).is_err());
        Ok(())
    }

    #[test]
    fn a_page_becomes_a_macro_that_places_its_items_where_they_were() -> Result<()> {
        let m = starter()?.apply(Some("3 kW"), &BTreeMap::new())?;
        let mut p = Project::new("x");
        p.pages.push(crate::project::Page::new("p1", "1", "Motors"));
        place(&mut p, "p1", &m, Point::new(50.0, 60.0))?;
        p.pages[0].wires[0].number = Some("12".into());
        let saved = from_page(&p, "p1", " Starter page ", " A whole page ")?;
        assert_eq!(
            (saved.name.as_str(), saved.description.as_str()),
            ("Starter page", "A whole page")
        );
        assert_eq!(saved.anchor, Point::new(0.0, 0.0));
        assert_eq!(saved.wires[0].number, None, "a placed wire is numbered afresh");
        let tag = DeviceTag::parse("-Q1")?;
        assert_eq!(saved.devices[&tag].description, "Motor starter 3 kW");
        // Placed at the corner of another page, each item is where it was, with a new tag.
        p.pages.push(crate::project::Page::new("p2", "2", "Copy"));
        place(&mut p, "p2", &saved, Point::new(0.0, 0.0))?;
        let at = |i: usize| -> Vec<Point> { p.pages[i].symbols.iter().map(|s| s.at).collect() };
        assert_eq!(at(0), at(1));
        let graphics = |i: usize| p.pages[i].graphics.clone();
        assert_eq!(graphics(0), graphics(1));
        assert_eq!(
            p.pages[1].symbols[0].tag.as_ref().map(ToString::to_string).as_deref(),
            Some("-Q2")
        );
        // The device data of a cable only a wire names is kept; a placeholder a text uses is
        // declared, so the macro places with a value for it.
        let w1 = DeviceTag::parse("-W1")?;
        p.devices.insert(
            w1.clone(),
            Device {
                description: "Feed cable".into(),
                ..Default::default()
            },
        );
        p.pages[0].wires[0].cable = Some(crate::project::CableCore {
            cable: w1.clone(),
            core: "1".into(),
        });
        p.pages[0].graphics.push(Shape::Text {
            at: Point::new(10.0, 10.0),
            text: "{{voltage}} V, {{Voltage }}".into(),
            height: 3.5,
            align: Default::default(),
            angle: 0.0,
        });
        let saved = from_page(&p, "p1", "Fed", "")?;
        assert_eq!(saved.devices[&w1].description, "Feed cable");
        let names: Vec<&str> = saved.placeholders.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["voltage"]);
        let values = BTreeMap::from([("voltage".to_owned(), "24".to_owned())]);
        assert!(saved.apply(None, &values).is_ok());
        // A text whose braces are not closed makes no macro.
        p.pages[0].graphics.push(Shape::Text {
            at: Point::new(10.0, 20.0),
            text: "{{open".into(),
            height: 3.5,
            align: Default::default(),
            angle: 0.0,
        });
        assert!(from_page(&p, "p1", "Open", "").is_err());
        // An empty page and a missing one are refused.
        p.pages.push(crate::project::Page::new("p3", "3", "Empty"));
        assert_eq!(
            err_of(from_page(&p, "p3", "x", "")),
            "page p3 has no symbols, wires or drawings to make a macro of"
        );
        assert!(err_of(from_page(&p, "p9", "x", "")).contains("no page p9"));
        Ok(())
    }

    #[test]
    fn placing_takes_free_tags_and_the_macro_device_data() -> Result<()> {
        let m = starter()?.apply(Some("3 kW"), &BTreeMap::new())?;
        let mut p = Project::new("x");
        let mut page = crate::project::Page::new("p1", "1", "Motors");
        let q1 = DeviceTag::parse("-Q1")?;
        page.symbols.push(PlacedSymbol {
            id: "s1".into(),
            symbol: "wc.contactor".into(),
            at: Point::new(100.0, 100.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(q1.clone()),
        });
        p.pages.push(page);
        p.devices.insert(
            q1.clone(),
            Device {
                description: "Fan".into(),
                ..Default::default()
            },
        );
        let ids = place(&mut p, "p1", &m, Point::new(50.0, 60.0))?;
        assert_eq!(ids, ["s2", "w1"]);
        let page = &p.pages[0];
        let s = page.symbol("s2").ok_or_else(|| Error::Invalid("not placed".into()))?;
        assert_eq!(s.tag.as_ref().map(ToString::to_string).as_deref(), Some("-Q2"));
        assert_eq!(s.at, Point::new(60.0, 80.0));
        assert_eq!(p.devices[&q1].description, "Fan");
        assert_eq!(p.devices[&DeviceTag::parse("-Q2")?].description, "Motor starter 3 kW");
        let w = &page.wires[0];
        assert_eq!(w.number, None);
        assert!(matches!(&w.from, Endpoint::Pin { item, .. } if item == "s2"));
        assert!(matches!(&w.to, Endpoint::Point { at } if *at == Point::new(60.0, 60.0)));
        assert!(matches!(&page.graphics[0], Shape::Text { at, .. } if *at == Point::new(70.0, 80.0)));
        assert!(place(&mut p, "p9", &m, Point::new(0.0, 0.0)).is_err());

        // A wire that is a core of the macro's cable follows the cable to its new tag.
        let w1 = DeviceTag::parse("-W1")?;
        let mut cabled = m.clone();
        cabled.symbols.push(PlacedSymbol {
            id: "s2".into(),
            symbol: "wc.cable".into(),
            at: Point::new(40.0, 30.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(w1.clone()),
        });
        cabled.wires[0].cable = Some(crate::project::CableCore {
            cable: w1.clone(),
            core: "BN".into(),
        });
        p.devices.insert(w1.clone(), Device::default());
        let ids = place(&mut p, "p1", &cabled, Point::new(0.0, 0.0))?;
        let page = &p.pages[0];
        let cable = page.symbol(&ids[1]).and_then(|s| s.tag.clone());
        assert_eq!(cable.as_ref().map(ToString::to_string).as_deref(), Some("-W2"));
        let wire = page.wires.iter().find(|w| w.id == ids[2]).and_then(|w| w.cable.clone());
        assert_eq!(wire.map(|c| c.cable), cable, "the core runs in the placed cable");
        Ok(())
    }

    #[test]
    fn placing_fails_untouched_when_no_tag_like_a_device_is_left() -> Result<()> {
        let mut m = starter()?.apply(Some("3 kW"), &BTreeMap::new())?;
        // A device with a free tag comes first: it is not placed either.
        let x1 = DeviceTag::parse("-X1")?;
        m.symbols.insert(
            0,
            PlacedSymbol {
                id: "s0".into(),
                symbol: "wc.terminal".into(),
                at: Point::new(10.0, 10.0),
                rotation: Default::default(),
                mirror: false,
                tag: Some(x1.clone()),
            },
        );
        m.devices.insert(x1, Device::default());
        let mut p = Project::new("x");
        p.pages.push(crate::project::Page::new("p1", "1", "Motors"));
        for n in 1..10_000 {
            p.devices
                .insert(DeviceTag::parse(&format!("-Q{n}"))?, Device::default());
        }
        let before = p.clone();
        let err = place(&mut p, "p1", &m, Point::new(0.0, 0.0))
            .err()
            .map(|e| e.to_string());
        assert!(
            err.as_deref().is_some_and(|e| e.contains("no free tag like -Q1")),
            "{err:?}"
        );
        assert_eq!(p, before, "nothing is placed");
        Ok(())
    }

    #[test]
    fn placed_terminals_follow_their_strip_to_its_new_tag() -> Result<()> {
        let sym = |id: &str, tag: &str| -> Result<PlacedSymbol> {
            Ok(PlacedSymbol {
                id: id.into(),
                symbol: "wc.terminal".into(),
                at: Point::new(10.0, 10.0),
                rotation: Default::default(),
                mirror: false,
                tag: Some(DeviceTag::parse(tag)?),
            })
        };
        let m = Macro {
            name: "Strip".into(),
            symbols: vec![sym("t1", "-X1.1")?, sym("t2", "-X1.2")?, sym("x", "-X1")?],
            devices: [(
                DeviceTag::parse("-X1.2")?,
                Device {
                    part: Some("PT 2.5".into()),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        // The project has its own strip -X1 with terminal 1.
        let mut p = Project::new("x");
        let mut page = crate::project::Page::new("p1", "1", "Terminals");
        page.symbols.push(sym("s1", "-X1")?);
        page.symbols.push(sym("s2", "-X1.1")?);
        p.pages.push(page);
        let ids = place(&mut p, "p1", &m, Point::new(0.0, 0.0))?;
        let tags: Vec<String> = ids
            .iter()
            .filter_map(|id| {
                p.pages[0]
                    .symbol(id)
                    .and_then(|s| s.tag.as_ref())
                    .map(ToString::to_string)
            })
            .collect();
        assert_eq!(
            tags,
            ["-X2.1", "-X2.2", "-X2"],
            "the terminals stay on the macro's strip"
        );
        assert_eq!(p.devices[&DeviceTag::parse("-X2.2")?].part.as_deref(), Some("PT 2.5"));

        // Without the strip in the macro, a terminal takes a free number as before.
        let lone = Macro {
            symbols: vec![sym("t1", "-X1.1")?],
            ..m.clone()
        };
        let ids = place(&mut p, "p1", &lone, Point::new(0.0, 0.0))?;
        let tag = p.pages[0].symbol(&ids[0]).and_then(|s| s.tag.clone());
        assert_eq!(tag.map(|t| t.to_string()).as_deref(), Some("-X1.2"));

        // A strip whose terminal is already used (device data for -X2.1) is taken, so the
        // macro's strip skips it and its terminals keep their numbers on the next free strip.
        let mut p = Project::new("x");
        let mut page = crate::project::Page::new("p1", "1", "Terminals");
        page.symbols.push(sym("s1", "-X1")?);
        page.symbols.push(sym("s2", "-X1.1")?);
        p.pages.push(page);
        p.devices.insert(DeviceTag::parse("-X2.1")?, Device::default());
        let ids = place(&mut p, "p1", &m, Point::new(0.0, 0.0))?;
        let tags: Vec<String> = ids
            .iter()
            .filter_map(|id| {
                p.pages[0]
                    .symbol(id)
                    .and_then(|s| s.tag.as_ref())
                    .map(ToString::to_string)
            })
            .collect();
        assert_eq!(
            tags,
            ["-X3.1", "-X3.2", "-X3"],
            "-X2 is held by its terminal -X2.1, so the strip moves to -X3"
        );
        assert_eq!(p.devices[&DeviceTag::parse("-X3.2")?].part.as_deref(), Some("PT 2.5"));
        Ok(())
    }

    #[test]
    fn placed_terminals_keep_their_numbers_when_the_strip_keeps_its_tag() -> Result<()> {
        let sym = |id: &str, tag: &str| -> Result<PlacedSymbol> {
            Ok(PlacedSymbol {
                id: id.into(),
                symbol: "wc.terminal".into(),
                at: Point::new(10.0, 10.0),
                rotation: Default::default(),
                mirror: false,
                tag: Some(DeviceTag::parse(tag)?),
            })
        };
        let m = Macro {
            name: "Strip".into(),
            symbols: vec![sym("t7", "-X1.7")?, sym("x", "-X1")?],
            ..Default::default()
        };
        // -X1 is free in this project, so the strip keeps its tag and terminal 7 its number.
        let mut p = Project::new("x");
        p.pages.push(crate::project::Page::new("p1", "1", "Terminals"));
        let ids = place(&mut p, "p1", &m, Point::new(0.0, 0.0))?;
        let tags: Vec<String> = ids
            .iter()
            .filter_map(|id| {
                p.pages[0]
                    .symbol(id)
                    .and_then(|s| s.tag.as_ref())
                    .map(ToString::to_string)
            })
            .collect();
        assert_eq!(tags, ["-X1.7", "-X1"]);
        Ok(())
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;
    use crate::project::{DevicePart, WireSpec};

    fn err_of<T>(r: Result<T>) -> String {
        r.err().map(|e| e.to_string()).unwrap_or_default()
    }

    fn placeholder(name: &str, default: &str) -> Placeholder {
        Placeholder {
            name: name.into(),
            default: default.into(),
            ..Default::default()
        }
    }

    /// A terminal -X1 whose wire section and accessory are placeholders.
    fn terminal() -> Result<Macro> {
        let tag = DeviceTag::parse("-X1")?;
        let device = Device {
            description: "Terminal {{size}}".into(),
            accessories: vec![
                DevicePart {
                    part: "{{bridge}}".into(),
                    qty: 2,
                },
                DevicePart {
                    part: "PXC.3030336".into(),
                    qty: 1,
                },
            ],
            ..Default::default()
        };
        Ok(Macro {
            name: "Terminal".into(),
            symbols: vec![PlacedSymbol {
                id: "s1".into(),
                symbol: "wc.terminal".into(),
                at: Point::new(0.0, 0.0),
                rotation: Default::default(),
                mirror: false,
                tag: Some(tag.clone()),
            }],
            wires: vec![Wire {
                id: "w1".into(),
                from: Endpoint::Pin {
                    item: "s1".into(),
                    pin: "1".into(),
                },
                to: Endpoint::Point {
                    at: Point::new(0.0, 10.0),
                },
                via: vec![],
                number: None,
                potential: None,
                cable: None,
                route: Default::default(),
                spec: WireSpec {
                    section: Some("{{size}}".into()),
                    colour: Some(" {{colour}} ".into()),
                    kind: None,
                },
            }],
            devices: [(tag, device)].into_iter().collect(),
            placeholders: vec![
                placeholder("size", "2.5"),
                placeholder("bridge", ""),
                placeholder("colour", "BU"),
            ],
            ..Default::default()
        })
    }

    #[test]
    fn macro_ids_are_always_usable_file_ids() -> Result<()> {
        // Cut at 100 characters, the id does not end on a dash.
        let long = format!("{} b", "a".repeat(99));
        let id = macro_id(&long);
        assert_eq!(id, "a".repeat(99));
        // Names Windows keeps for a device get a suffix, so the macro can be saved.
        assert_eq!(macro_id("Aux"), "aux-macro");
        assert_eq!(macro_id(" COM1 "), "com1-macro");
        assert_eq!(macro_id("Aux contacts"), "aux-contacts");
        assert_eq!(macro_id("--Straße--"), "strasse");
        for name in [long.as_str(), "Aux", "con", "NUL", "lpt9", "Ærø", "", "  "] {
            let id = macro_id(name);
            assert!(check_id(&id).is_ok(), "{name:?} gave {id:?}");
        }
        let tmp = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let m = terminal()?;
        let path = write_macro(tmp.path(), &macro_id("Con"), &m)?;
        assert_eq!(read_macro(&path)?, m);
        Ok(())
    }

    #[test]
    fn unusable_names_are_refused_by_check() -> Result<()> {
        for name in ["", "   ", "{{x}}", "a}}b"] {
            let mut m = terminal()?;
            m.placeholders.push(placeholder(name, ""));
            assert!(err_of(m.check()).contains("is not usable"), "{name:?}");
        }
        // An empty `{{}}` is a use of a placeholder that cannot be declared.
        let mut m = terminal()?;
        if let Some(d) = m.devices.values_mut().next() {
            d.description = "Terminal {{ }}".into();
        }
        assert!(err_of(m.check()).contains("{{}} is used"));

        let mut unnamed = terminal()?;
        unnamed.value_sets.push(ValueSet {
            name: "  ".into(),
            ..Default::default()
        });
        assert!(err_of(unnamed.check()).contains("has no name"));

        let mut twice = terminal()?;
        for name in ["Small", " small "] {
            twice.value_sets.push(ValueSet {
                name: name.into(),
                ..Default::default()
            });
        }
        assert!(err_of(twice.check()).contains("value set small is there twice"));
        Ok(())
    }

    #[test]
    fn a_set_cannot_be_chosen_on_a_macro_without_sets() -> Result<()> {
        let m = terminal()?;
        let none = BTreeMap::new();
        let err = err_of(m.apply(Some("Big"), &none));
        assert!(
            err.contains("it has no value sets, so \"Big\" cannot be chosen"),
            "{err}"
        );
        // A blank choice is no choice: the defaults are used.
        let bridge: BTreeMap<String, String> = [("BRIDGE".to_owned(), "PXC.3030161".to_owned())].into_iter().collect();
        let blank = m.apply(Some("   "), &bridge)?;
        assert_eq!(blank, m.apply(None, &bridge)?);
        assert_eq!(blank.wires[0].spec.section.as_deref(), Some("2.5"));
        assert_eq!(blank.wires[0].spec.colour.as_deref(), Some("BU"));
        // With sets, a blank choice still asks for one.
        let mut with_sets = m.clone();
        with_sets.value_sets.push(ValueSet {
            name: "Big".into(),
            ..Default::default()
        });
        assert!(err_of(with_sets.apply(Some(" "), &bridge)).contains("choose a value set: \"Big\""));
        Ok(())
    }

    #[test]
    fn empty_values_drop_wire_values_and_accessories() -> Result<()> {
        let mut m = terminal()?;
        m.value_sets.push(ValueSet {
            name: "Bare".into(),
            values: [("size", ""), ("bridge", "  "), ("colour", "")]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            ..Default::default()
        });
        // An empty value given on purpose is a value, even over a default.
        let bare = m.apply(Some("bare"), &BTreeMap::new())?;
        assert_eq!(bare.wires[0].spec.section, None);
        assert_eq!(bare.wires[0].spec.colour, None, "only spaces left is no value");
        let d = &bare.devices[&DeviceTag::parse("-X1")?];
        assert_eq!(d.description, "Terminal ");
        let parts: Vec<&str> = d.accessories.iter().map(|a| a.part.as_str()).collect();
        assert_eq!(parts, ["PXC.3030336"], "the empty accessory is dropped, the other kept");
        // Leaving `bridge` without any value is an error, not a dropped accessory.
        m.value_sets.clear();
        assert!(err_of(m.apply(None, &BTreeMap::new())).contains("no value for bridge"));
        Ok(())
    }

    #[test]
    fn renaming_leaves_other_texts_alone() -> Result<()> {
        let mut m = terminal()?;
        let before = m.clone();
        m.rename_placeholders(&[]);
        m.rename_placeholder("volts", "v");
        assert_eq!(m, before, "nothing to rename changes nothing");
        // The new name is trimmed, and an unclosed tail is kept as written.
        if let Some(d) = m.devices.values_mut().next() {
            d.description = "{{ SIZE }} and {{size".into();
        }
        m.rename_placeholder("size", "  width ");
        let d = &m.devices[&DeviceTag::parse("-X1")?];
        assert_eq!(d.description, "{{width}} and {{size");
        assert_eq!(m.wires[0].spec.section.as_deref(), Some("{{width}}"));
        assert_eq!(m.wires[0].spec.colour.as_deref(), Some(" {{colour}} "));
        Ok(())
    }

    #[test]
    fn a_declared_placeholder_can_change_case_but_needs_a_name() -> Result<()> {
        let mut m = terminal()?;
        assert!(err_of(m.rename_declared("size", "  ")).contains("placeholder size needs a new name"));
        m.rename_declared("SIZE", "Size")?;
        assert_eq!(m.placeholders[0].name, "Size");
        assert_eq!(m.wires[0].spec.section.as_deref(), Some("{{Size}}"));
        m.check()?;
        Ok(())
    }

    #[test]
    fn listing_is_sorted_and_skips_unusable_ids() -> Result<()> {
        let tmp = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let m = terminal()?;
        for id in ["zeta", "alpha", "mid-1"] {
            write_macro(tmp.path(), id, &m)?;
        }
        let dir = tmp.path().join(MACROS_DIR);
        for name in [".hidden.json", "bad name.json", "alpha.json.bak"] {
            std::fs::write(dir.join(name), "{}").map_err(|e| Error::io("w", e))?;
        }
        // A name Windows keeps for a device is not a usable id; its file cannot be made there.
        assert!(check_id("con").is_err());
        #[cfg(not(windows))]
        std::fs::write(dir.join("con.json"), "{}").map_err(|e| Error::io("w", e))?;
        let listed = list_macros(tmp.path());
        let ids: Vec<&str> = listed.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["alpha", "mid-1", "zeta"]);
        assert!(listed.iter().all(|(id, p)| *p == dir.join(format!("{id}.json"))));
        Ok(())
    }

    #[test]
    fn placing_on_a_missing_page_is_refused_untouched() -> Result<()> {
        // `bridge` has no default: give it a value so the macro applies.
        let bridge: BTreeMap<String, String> = [("bridge".to_owned(), "PXC.3030161".to_owned())].into_iter().collect();
        let m = terminal()?.apply(None, &bridge)?;
        let mut p = Project::new("x");
        p.pages.push(crate::project::Page::new("p1", "1", "Terminals"));
        let before = p.clone();
        assert!(err_of(place(&mut p, "p2", &m, Point::new(0.0, 0.0))).contains("no page p2"));
        assert_eq!(p, before);
        Ok(())
    }
}

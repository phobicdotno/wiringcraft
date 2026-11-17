//! Macros: reusable groups of symbols, wires and drawings, saved as one JSON file each in a
//! `macros/` folder of a project or a library source. Inserting one works like a paste: symbols
//! get fresh ids and the next free tags, and wires follow their symbols.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::geom::{Point, Shape};
use crate::project::{Device, PlacedSymbol, Wire};
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
}

impl Macro {
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty() && self.wires.is_empty() && self.graphics.is_empty()
    }
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
    let id = id.trim_end_matches('-');
    let id: String = id.chars().take(100).collect();
    if id.is_empty() { "macro".into() } else { id }
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
    use crate::project::Endpoint;

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
}

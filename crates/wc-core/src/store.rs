//! The project folder on disk.
//!
//! ```text
//! my-project/
//!   project.toml            name, title block, library sources
//!   devices.json            device tag -> part and properties
//!   library.lock            exact version of every library item in use
//!   pages/<id>.page.json    one file per page
//!   vendor/symbols/<id>.json  copies of the library items in use
//!   vendor/parts/<id>.json
//! ```
//!
//! Every file is written deterministically (sorted keys, fixed indentation, trailing newline),
//! atomically (temporary file, then rename) and only when its content changed, so version
//! control shows real changes only and a crash never leaves a half-written file.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::lock::ItemKind;
use crate::project::{Device, Page, Project, ProjectMeta};
use crate::tag::DeviceTag;
use crate::{Error, Lockfile, Part, Result, Symbol};

pub const PROJECT_FILE: &str = "project.toml";
pub const DEVICES_FILE: &str = "devices.json";
pub const LOCK_FILE: &str = "library.lock";
pub const PAGES_DIR: &str = "pages";
pub const VENDOR_DIR: &str = "vendor";
const PAGE_SUFFIX: &str = ".page.json";

/// Ids become file names, so they are restricted to a portable set.
pub fn check_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "id {id:?} must be 1-128 of A-Z a-z 0-9 . _ - and not start with '.'"
        )))
    }
}

pub fn to_json<T: Serialize>(value: &T) -> Result<String> {
    // Through Value so object keys come out sorted whatever the struct order is.
    let v = serde_json::to_value(value).map_err(|e| Error::Json {
        path: PathBuf::new(),
        source: e,
    })?;
    let mut s = serde_json::to_string_pretty(&v).map_err(|e| Error::Json {
        path: PathBuf::new(),
        source: e,
    })?;
    s.push('\n');
    Ok(s)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    serde_json::from_str(&text).map_err(|e| Error::Json {
        path: path.into(),
        source: e,
    })
}

fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    toml::from_str(&text).map_err(|e| Error::Toml {
        path: path.into(),
        message: e.to_string(),
    })
}

fn to_toml<T: Serialize>(value: &T, path: &Path) -> Result<String> {
    toml::to_string_pretty(value).map_err(|e| Error::Toml {
        path: path.into(),
        message: e.to_string(),
    })
}

/// Write `content` to `path` atomically, skipping the write when the file already holds it.
/// Returns whether the file changed.
pub fn write_if_changed(path: &Path, content: &str) -> Result<bool> {
    if fs::read_to_string(path).is_ok_and(|old| old == content) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, content).map_err(|e| Error::io(&tmp, e))?;
    fs::rename(&tmp, path).map_err(|e| Error::io(path, e))?;
    Ok(true)
}

/// Load a whole project folder.
pub fn load(dir: &Path) -> Result<Project> {
    let meta: ProjectMeta = read_toml(&dir.join(PROJECT_FILE))?;
    if meta.format_version > ProjectMeta::FORMAT_VERSION {
        return Err(Error::Invalid(format!(
            "project format {} is newer than this WiringCraft understands ({})",
            meta.format_version,
            ProjectMeta::FORMAT_VERSION
        )));
    }
    let devices_path = dir.join(DEVICES_FILE);
    let devices: BTreeMap<DeviceTag, Device> = if devices_path.exists() {
        read_json(&devices_path)?
    } else {
        BTreeMap::new()
    };
    let lock_path = dir.join(LOCK_FILE);
    let lock: Lockfile = if lock_path.exists() {
        read_toml(&lock_path)?
    } else {
        Lockfile::default()
    };

    let mut pages = Vec::new();
    let pages_dir = dir.join(PAGES_DIR);
    if pages_dir.exists() {
        let entries = fs::read_dir(&pages_dir).map_err(|e| Error::io(&pages_dir, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| Error::io(&pages_dir, e))?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(stem) = name.strip_suffix(PAGE_SUFFIX) else {
                continue;
            };
            let page: Page = read_json(&path)?;
            if page.id != stem {
                return Err(Error::Invalid(format!(
                    "{} holds page id {:?}; the file name and id must match",
                    path.display(),
                    page.id
                )));
            }
            pages.push(page);
        }
    }
    let mut project = Project {
        meta,
        pages,
        devices,
        lock,
    };
    project.sort_pages();
    Ok(project)
}

/// Save a project folder. Page files of deleted pages are removed. Returns the files written.
pub fn save(dir: &Path, project: &Project) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    let mut put = |path: PathBuf, content: String| -> Result<()> {
        if write_if_changed(&path, &content)? {
            written.push(path);
        }
        Ok(())
    };
    put(dir.join(PROJECT_FILE), to_toml(&project.meta, &dir.join(PROJECT_FILE))?)?;
    put(dir.join(DEVICES_FILE), to_json(&project.devices)?)?;
    put(dir.join(LOCK_FILE), to_toml(&project.lock, &dir.join(LOCK_FILE))?)?;

    let pages_dir = dir.join(PAGES_DIR);
    let mut keep = std::collections::BTreeSet::new();
    for page in &project.pages {
        check_id(&page.id)?;
        let name = format!("{}{PAGE_SUFFIX}", page.id);
        put(pages_dir.join(&name), to_json(page)?)?;
        keep.insert(name);
    }
    if pages_dir.exists() {
        let entries = fs::read_dir(&pages_dir).map_err(|e| Error::io(&pages_dir, e))?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(PAGE_SUFFIX) && !keep.contains(&name) {
                fs::remove_file(entry.path()).map_err(|e| Error::io(entry.path(), e))?;
            }
        }
    }
    Ok(written)
}

/// Create a new, empty project folder. Refuses a folder that already holds a project.
pub fn create(dir: &Path, name: &str) -> Result<Project> {
    if dir.join(PROJECT_FILE).exists() {
        return Err(Error::Invalid(format!("{} already holds a project", dir.display())));
    }
    let mut project = Project::new(name);
    project.pages.push(Page::new("p0001", "1", "Cover"));
    save(dir, &project)?;
    Ok(project)
}

fn vendor_path(dir: &Path, kind: ItemKind, id: &str) -> Result<PathBuf> {
    check_id(id)?;
    Ok(dir.join(VENDOR_DIR).join(kind.dir()).join(format!("{id}.json")))
}

pub fn write_vendored_symbol(dir: &Path, s: &Symbol) -> Result<bool> {
    write_if_changed(&vendor_path(dir, ItemKind::Symbol, &s.id)?, &to_json(s)?)
}

pub fn write_vendored_part(dir: &Path, p: &Part) -> Result<bool> {
    write_if_changed(&vendor_path(dir, ItemKind::Part, &p.id)?, &to_json(p)?)
}

pub fn read_vendored_symbol(dir: &Path, id: &str) -> Result<Symbol> {
    read_json(&vendor_path(dir, ItemKind::Symbol, id)?)
}

pub fn read_vendored_part(dir: &Path, id: &str) -> Result<Part> {
    read_json(&vendor_path(dir, ItemKind::Part, id)?)
}

/// All vendored symbols the project's pages use, keyed by id. Missing ones are reported.
pub fn vendored_symbols(dir: &Path, project: &Project) -> (BTreeMap<String, Symbol>, Vec<String>) {
    let mut found = BTreeMap::new();
    let mut missing = Vec::new();
    for id in project.used_symbols() {
        match read_vendored_symbol(dir, &id) {
            Ok(s) => {
                found.insert(id, s);
            }
            Err(_) => missing.push(id),
        }
    }
    (found, missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Point;
    use crate::project::{Endpoint, PlacedSymbol, Wire};

    fn sample() -> Result<Project> {
        let mut p = Project::new("Demo");
        p.meta.title_block.insert("customer".into(), "ACME".into());
        let mut page = Page::new("p0002", "2", "Motor circuit");
        page.symbols.push(PlacedSymbol {
            id: "s1".into(),
            symbol: "wc.coil".into(),
            at: Point::new(40.0, 60.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(DeviceTag::parse("+CAB1-K1")?),
        });
        page.wires.push(Wire {
            id: "w1".into(),
            from: Endpoint::Pin {
                item: "s1".into(),
                pin: "A1".into(),
            },
            to: Endpoint::Point {
                at: Point::new(40.0, 20.0),
            },
            via: vec![],
            number: None,
            potential: Some("24V".into()),
        });
        p.pages.push(page);
        p.devices.insert(
            DeviceTag::parse("+CAB1-K1")?,
            Device {
                part: Some("acme.r1".into()),
                ..Default::default()
            },
        );
        Ok(p)
    }

    #[test]
    fn round_trip_and_idempotent_save() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let p = sample()?;
        let first = save(dir.path(), &p)?;
        assert!(first.len() >= 4);
        let again = save(dir.path(), &p)?;
        assert!(again.is_empty(), "unchanged project rewrote {again:?}");
        let back = load(dir.path())?;
        assert_eq!(back, p);
        Ok(())
    }

    #[test]
    fn deleted_pages_lose_their_file() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let mut p = sample()?;
        save(dir.path(), &p)?;
        p.pages.clear();
        save(dir.path(), &p)?;
        assert!(!dir.path().join("pages/p0002.page.json").exists());
        Ok(())
    }

    #[test]
    fn create_refuses_existing_project() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        create(dir.path(), "A")?;
        assert!(create(dir.path(), "B").is_err());
        Ok(())
    }

    #[test]
    fn hostile_ids_are_refused() {
        for bad in ["", "../x", ".hidden", "a/b", "a b"] {
            assert!(check_id(bad).is_err(), "{bad}");
        }
        assert!(check_id("wc.contact-no_1").is_ok());
    }
}

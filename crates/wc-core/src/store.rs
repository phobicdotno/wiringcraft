//! The project folder on disk.
//!
//! ```text
//! my-project/
//!   project.toml            name, format version, title block, library sources and settings
//!   devices.json            device tag -> part, description and properties
//!   library.lock            exact version of every library item in use
//!   pages/<id>.page.json    one file per page
//!   panels/<id>.panel.json  cabinet panel layouts
//!   comments/<page id>.comments.json  review comments on a page
//!   vendor/symbols/<id>.json  copies of the library symbols in use
//!   vendor/parts/<id>.json    copies of the library parts in use
//!   macros/<id>.json        the project's own macros ([`crate::macros`])
//!   translations.toml       translation dictionary ([`crate::translate`])
//!   pictures/               the logo and other pictures ([`crate::Logo::DIR`])
//!   underlays/              pictures drawn under a page ([`crate::Underlay::DIR`])
//! ```
//!
//! `project.toml` holds `name`, `format_version`, `[title_block]` and the `[[library]]`
//! sources, then one table or key per kind of project setting: `[[location]]`, `[[jumper]]`,
//! `[[strip]]`, `[logo]`, `[template]`, `[[revision]]`, `[[report]]`, `[[rule]]`, `rules_off`,
//! `[[rack]]` and `[[network]]` (see [`ProjectMeta`]). A setting that would be empty, or a
//! `[template]` that is the built-in one, is left out.
//!
//! This module reads and writes `project.toml`, `devices.json`, `library.lock`, pages, panels,
//! comments and `vendor/`. Macros and the translation dictionary are read and written by their
//! own modules, with the same write helpers. Only `project.toml` is needed: a missing
//! `devices.json` or `library.lock` reads as empty, and a missing folder as one with nothing in
//! it. [`docs/project-format.md`](https://github.com/phobicdotno/wiringcraft/blob/main/docs/project-format.md)
//! in the repository is the full reference for the format.
//!
//! Every file is written deterministically (sorted keys, fixed indentation, trailing newline),
//! atomically (temporary file, then rename) and only when its content changed, so version
//! control shows real changes only and a crash never leaves a half-written file. A write counts
//! as done only once it would last through a power loss: the new bytes are synced before the
//! rename, and the folder is synced after it (see [`replace_whole`]). A folder whose sync failed
//! stays owed until a later write or save in the same place gets it synced.

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
pub const PAGE_SUFFIX: &str = ".page.json";
pub const PANELS_DIR: &str = "panels";
pub const PANEL_SUFFIX: &str = ".panel.json";
pub const COMMENTS_DIR: &str = "comments";
pub const COMMENTS_SUFFIX: &str = ".comments.json";
/// The file a `box-edit` run keeps in the project folder while it writes: the bytes each file
/// it may change had before. One left behind means a run stopped half way; the next
/// `wiringcraft` command in the project puts the files back from it.
pub const BOX_EDIT_JOURNAL: &str = ".box-edit.journal";

/// Ids become file names, so they are restricted to a portable set. A name Windows keeps for a
/// device (`con`, `nul`, `aux`, `com1`, ...) is refused too, whatever follows it after a dot:
/// Windows opens the device instead of the file.
pub fn check_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !ok {
        return Err(Error::Invalid(format!(
            "id {id:?} must be 1-128 of A-Z a-z 0-9 . _ - and not start with '.'"
        )));
    }
    let stem = id.split('.').next().unwrap_or(id);
    if is_windows_device_name(stem) {
        return Err(Error::Invalid(format!(
            "id {id:?} starts with {stem:?}, a name Windows keeps for a device: its file could not be opened there"
        )));
    }
    Ok(())
}

/// Whether `name` is one Windows keeps for a device: `CON`, `PRN`, `AUX`, `NUL`, `COM0`-`COM9`
/// or `LPT0`-`LPT9`, in any case.
fn is_windows_device_name(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    match n.as_bytes() {
        [_, _, _] => matches!(n.as_str(), "CON" | "PRN" | "AUX" | "NUL"),
        [a, b, c, d] => matches!(&[*a, *b, *c], b"COM" | b"LPT") && d.is_ascii_digit(),
        _ => false,
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
/// Returns whether the file changed. A changed file is on disk when this returns (see
/// [`replace_whole`]), and so is every folder this call had to make for it.
///
/// A folder sync that failed in an earlier write to this place is tried again here, also when
/// the file already holds `content`: `Ok` means nothing is owed any more.
pub fn write_if_changed(path: &Path, content: &str) -> Result<bool> {
    let mut folders = Folders::new();
    let changed = stage_if_changed(path, content, &mut folders);
    add_owed(&folder_of(path), &mut folders);
    synced(changed, &folders)
}

/// The folders whose entries changed (a file renamed into place or removed, a folder made) and
/// that are not synced yet. A save that writes many files into one folder syncs it once, at the
/// end, instead of once per file.
type Folders = std::collections::BTreeSet<PathBuf>;

/// [`write_if_changed`] without the folder sync: the folders to sync are added to `folders`,
/// for the caller to pass to [`synced`] once all its files are in place.
fn stage_if_changed(path: &Path, content: &str, folders: &mut Folders) -> Result<bool> {
    if fs::read_to_string(path).is_ok_and(|old| old == content) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        // A folder made here is a new entry in the folder above it.
        let made: Vec<&Path> = dir
            .ancestors()
            .take_while(|a| !a.as_os_str().is_empty() && !a.exists())
            .collect();
        let created = fs::create_dir_all(dir).map_err(|e| Error::io(dir, e));
        folders.extend(made.into_iter().filter(|m| m.exists()).map(folder_of));
        created?;
    }
    rename_staged(path, content.as_bytes())?;
    folders.insert(folder_of(path));
    Ok(true)
}

/// The folder that holds the entry for `path`.
fn folder_of(path: &Path) -> PathBuf {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_owned(),
        _ => PathBuf::from("."),
    }
}

/// Sync `folders`, then hand back `result`. The folders are synced after a failure too, so the
/// files that did get into place before it are on disk; the failure is what is reported then.
/// A folder whose sync failed is owed from then on (see [`OWED`]), one that got synced is not.
fn synced<T>(result: Result<T>, folders: &Folders) -> Result<T> {
    let mut outcome = Ok(());
    for dir in folders {
        // A debt another thread notes for this folder while it is being synced is from a rename
        // this sync may not cover: only debts noted before the sync are paid by it.
        let before = OWED_NOTED.load(std::sync::atomic::Ordering::SeqCst);
        let raw = sync_folder(dir);
        // A folder that is not there has no entries left to keep.
        let gone = raw.as_ref().is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
        let one = folder_synced(dir, raw);
        settle(dir, one.is_err() && !gone, before);
        if outcome.is_ok() {
            outcome = one;
        }
    }
    let value = result?;
    outcome?;
    Ok(value)
}

/// The folders whose sync failed and was reported, by resolved path, each with the path it was
/// synced under. The write that reported the failure left its files in place, so a second try
/// finds nothing to write and would sync nothing: this list is what makes it sync the folder
/// after all. An entry leaves the list when its folder gets synced, or is gone.
///
/// It is empty unless a folder sync failed, and then a write costs one look at it and no sync.
/// It lasts as long as the process: a failure the process did not get to try again was reported
/// as a failed write, and is not known to the next process.
static OWED: std::sync::Mutex<BTreeMap<PathBuf, Debt>> = std::sync::Mutex::new(BTreeMap::new());

/// How many debts were noted so far; each debt keeps the count it was noted at.
static OWED_NOTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A folder owed a sync: the path it was synced under, and when it was noted ([`OWED_NOTED`]).
struct Debt {
    spelled: PathBuf,
    noted: u64,
}

fn owed() -> std::sync::MutexGuard<'static, BTreeMap<PathBuf, Debt>> {
    OWED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `dir` as a full path, as it is spelled (links not followed).
fn spelled(dir: &Path) -> PathBuf {
    std::path::absolute(dir).unwrap_or_else(|_| dir.to_owned())
}

/// `dir` as a full path with its links followed, so one folder has one name in [`OWED`].
fn resolved(dir: &Path) -> PathBuf {
    dir.canonicalize().unwrap_or_else(|_| spelled(dir))
}

/// Note whether the folder `dir` is owed a sync after an attempt that began when `before` debts
/// had been noted. A sync that went through pays only a debt noted before it began.
fn settle(dir: &Path, failed: bool, before: u64) {
    let mut owed = owed();
    if failed {
        let noted = OWED_NOTED.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        owed.insert(
            resolved(dir),
            Debt {
                spelled: spelled(dir),
                noted,
            },
        );
    } else if !owed.is_empty() {
        let key = resolved(dir);
        if owed.get(&key).is_some_and(|d| d.noted <= before) {
            owed.remove(&key);
        }
    }
}

/// Add to `folders` the folders owed a sync around `at`: `at` itself, the folders it lies in (a
/// folder made for a write is an entry in the one above it) and the folders inside it (a save of
/// a project folder covers its page folders).
fn add_owed(at: &Path, folders: &mut Folders) {
    let mut owed = owed();
    if owed.is_empty() {
        return;
    }
    owed.retain(|dir, _| dir.exists());
    let (real, given) = (resolved(at), spelled(at));
    let listed: Vec<PathBuf> = folders.iter().map(|f| resolved(f)).collect();
    let near = |a: &Path, b: &Path| a.starts_with(b) || b.starts_with(a);
    for (dir, debt) in owed.iter() {
        if (near(dir, &real) || near(&debt.spelled, &given)) && !listed.contains(dir) {
            folders.insert(dir.clone());
        }
    }
}

/// What a sync is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Synced {
    /// A staging file, before it gets its name.
    File,
    /// A folder, after its entries changed.
    Folder,
}

/// Sync the staging `file` at `path`.
fn sync_file(file: &fs::File, path: &Path) -> std::io::Result<()> {
    watched(Synced::File, path)?;
    file.sync_all()
}

/// [`sync_dir`] for the write path.
fn sync_folder(dir: &Path) -> std::io::Result<()> {
    watched(Synced::Folder, dir)?;
    sync_dir(dir)
}

/// Every sync of the write path passes here first. Outside tests this does nothing.
#[cfg(not(test))]
fn watched(_what: Synced, _path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// What a test put in the way of the syncs its thread makes (see [`watched`]).
#[cfg(test)]
type SyncWatch = Box<dyn FnMut(Synced, &Path) -> std::io::Result<()>>;

#[cfg(test)]
thread_local! {
    static SYNC_WATCH: std::cell::RefCell<Option<SyncWatch>> = const { std::cell::RefCell::new(None) };
}

/// Every sync of the write path passes here first. A test sees each one through
/// [`SYNC_WATCH`], and can fail it: the error it gives is the outcome of the sync.
#[cfg(test)]
fn watched(what: Synced, path: &Path) -> std::io::Result<()> {
    SYNC_WATCH.with(|watch| match watch.borrow_mut().as_mut() {
        Some(tell) => tell(what, path),
        None => Ok(()),
    })
}

/// Make the renames and removals in `dir` last on disk where the system allows it (Unix);
/// elsewhere the rename itself is what the system keeps.
pub fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// What the `outcome` of syncing the folder `dir` means for a write. A folder that cannot be
/// synced here (some file systems answer that the call is not supported or not valid for a
/// folder, and a folder that may be written to but not read cannot be opened for it) does not
/// fail the write: the file itself is synced and in place. Any other failure does.
fn folder_synced(dir: &Path, outcome: std::io::Result<()>) -> Result<()> {
    use std::io::ErrorKind::{InvalidInput, PermissionDenied, Unsupported};
    match outcome {
        Err(e) if !matches!(e.kind(), InvalidInput | PermissionDenied | Unsupported) => Err(Error::io(dir, e)),
        _ => Ok(()),
    }
}

/// Replace the file at `path` with `bytes` as a whole: staged next to it in `<file>.tmp`, then
/// renamed over it, so a failed write leaves the file as it was rather than cut short. The
/// staging file is one this call creates itself (opened with `create_new`): a link left at that
/// path is never written through. The folder must exist.
///
/// The write is on disk when this returns: the staged bytes are synced before the rename, so the
/// new name never points at bytes a power loss can drop, and the folder is synced after it
/// (Unix; elsewhere the rename is what the system keeps). A failed sync of the bytes fails the
/// write and leaves the file as it was. A folder that cannot be synced does not fail it.
pub fn replace_whole(path: &Path, bytes: &[u8]) -> Result<()> {
    rename_staged(path, bytes)?;
    synced(Ok(()), &Folders::from([folder_of(path)]))
}

/// [`replace_whole`] without the folder sync.
fn rename_staged(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    // A staging file left by a failed write or rename is removed again (a folder in its way is
    // not ours to remove).
    let drop_tmp = || {
        if fs::symlink_metadata(&tmp).is_ok_and(|m| m.is_file()) {
            let _ = fs::remove_file(&tmp);
        }
    };
    if let Err(e) = write_staging(&tmp, bytes) {
        drop_tmp();
        return Err(Error::io(&tmp, e));
    }
    if let Err(e) = fs::rename(&tmp, path) {
        drop_tmp();
        return Err(Error::io(path, e));
    }
    Ok(())
}

/// Write `content` to a staging file at `tmp` that this call creates itself, and sync it, so the
/// bytes are on disk before the file gets its real name.
///
/// The file is opened with `create_new`, which never follows a link: a symbolic link left at
/// `tmp` by something else in the project folder is removed (the link, not its target), and a
/// link that shows up again before the file is created makes the write fail instead of writing
/// through it. A regular file left by an earlier failed write is replaced the same way. Anything
/// else in the way, such as a folder, is not ours to remove and fails the write.
fn write_staging(tmp: &Path, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Ok(meta) = fs::symlink_metadata(tmp)
        && (meta.is_symlink() || meta.is_file())
    {
        fs::remove_file(tmp)?;
    }
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(tmp)?;
    file.write_all(content)?;
    sync_file(&file, tmp)
}

/// The devices of `devices.json`, refusing two entries for one device: a map would keep only
/// the last of them, and the next save would drop the other without a word. Two spellings of
/// one tag (`-K1+X1` and `+X1-K1`) are one device too.
struct UniqueDevices(BTreeMap<DeviceTag, Device>);

impl<'de> serde::Deserialize<'de> for UniqueDevices {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Entries;
        impl<'de> serde::de::Visitor<'de> for Entries {
            type Value = UniqueDevices;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a map of device tags to devices")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> std::result::Result<Self::Value, A::Error> {
                let mut devices = BTreeMap::new();
                let mut spelled: BTreeMap<DeviceTag, String> = BTreeMap::new();
                while let Some((key, device)) = map.next_entry::<String, Device>()? {
                    let tag = DeviceTag::parse(&key).map_err(serde::de::Error::custom)?;
                    if let Some(first) = spelled.insert(tag.clone(), key.clone()) {
                        return Err(serde::de::Error::custom(format!(
                            "devices {first:?} and {key:?} are the same device; keep one of them"
                        )));
                    }
                    devices.insert(tag, device);
                }
                Ok(UniqueDevices(devices))
            }
        }
        d.deserialize_map(Entries)
    }
}

/// Load a whole project folder.
pub fn load(dir: &Path) -> Result<Project> {
    let meta_path = dir.join(PROJECT_FILE);
    let meta: ProjectMeta = read_toml(&meta_path)?;
    if meta.format_version > ProjectMeta::FORMAT_VERSION {
        return Err(Error::Invalid(format!(
            "{} is project format {}, newer than this WiringCraft reads (format {}): update \
             WiringCraft to open it",
            meta_path.display(),
            meta.format_version,
            ProjectMeta::FORMAT_VERSION
        )));
    }
    let devices_path = dir.join(DEVICES_FILE);
    let devices: BTreeMap<DeviceTag, Device> = if devices_path.exists() {
        read_json::<UniqueDevices>(&devices_path)?.0
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
    let mut panels: Vec<crate::panel::Panel> = Vec::new();
    let panels_dir = dir.join(PANELS_DIR);
    if panels_dir.exists() {
        let entries = fs::read_dir(&panels_dir).map_err(|e| Error::io(&panels_dir, e))?;
        for entry in entries {
            let path = entry.map_err(|e| Error::io(&panels_dir, e))?.path();
            let Some(stem) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(PANEL_SUFFIX))
            else {
                continue;
            };
            let panel: crate::panel::Panel = read_json(&path)?;
            if panel.id != stem {
                return Err(Error::Invalid(format!(
                    "{} holds panel id {:?}; the file name and id must match",
                    path.display(),
                    panel.id
                )));
            }
            panels.push(panel);
        }
    }
    panels.sort_by(|a, b| a.id.cmp(&b.id));
    let mut comments: Vec<crate::review::Comment> = Vec::new();
    let comments_dir = dir.join(COMMENTS_DIR);
    if comments_dir.exists() {
        let entries = fs::read_dir(&comments_dir).map_err(|e| Error::io(&comments_dir, e))?;
        for entry in entries {
            let path = entry.map_err(|e| Error::io(&comments_dir, e))?.path();
            let Some(stem) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(COMMENTS_SUFFIX))
            else {
                continue;
            };
            let mut list: Vec<crate::review::Comment> = read_json(&path)?;
            // The file says which page; a comment that claims another is moved back to it.
            for c in &mut list {
                c.page = stem.to_owned();
            }
            comments.extend(list);
        }
    }
    let mut project = Project {
        meta,
        pages,
        devices,
        lock,
        panels,
        comments,
    };
    project.sort_pages();
    project.sort_comments();
    Ok(project)
}

/// Write only the project's `library.lock`; true when it changed. For a change that pins new
/// items: written before the files that use them, a failed save leaves a pin nothing uses yet
/// (harmless, and the next try finds it), never a use without its pin. A changed lock is on disk
/// when this returns.
pub fn save_lock(dir: &Path, project: &Project) -> Result<bool> {
    let path = dir.join(LOCK_FILE);
    write_if_changed(&path, &to_toml(&project.lock, &path)?)
}

/// Save a project folder. Page files of deleted pages are removed. Returns the files written.
///
/// Ids that differ only in case are refused before anything is written: on a folder that ignores
/// case (macOS and Windows by default) they would be one file. A page or panel id that cannot be
/// a file name ([`check_id`]) is refused before anything is written too.
///
/// Every file written is synced, and each folder the save changed (a file written or removed
/// in it) is synced once at the end, so the whole save is on disk when this returns. Files that
/// did not change cost no sync. A folder sync that failed in an earlier save of this folder is
/// tried again here, also when no file changed: `Ok` means nothing is owed any more.
pub fn save(dir: &Path, project: &Project) -> Result<Vec<PathBuf>> {
    for id in project
        .pages
        .iter()
        .map(|p| &p.id)
        .chain(project.panels.iter().map(|p| &p.id))
    {
        check_id(id)?;
    }
    unique_ignoring_case("page", project.pages.iter().map(|p| p.id.as_str()))?;
    unique_ignoring_case("panel", project.panels.iter().map(|p| p.id.as_str()))?;
    let mut folders = Folders::new();
    let written = save_files(dir, project, &mut folders);
    add_owed(dir, &mut folders);
    synced(written, &folders)
}

/// [`save`] without the folder syncs: the folders it changed are added to `folders`.
fn save_files(dir: &Path, project: &Project, folders: &mut Folders) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    let mut put = |path: PathBuf, content: String, folders: &mut Folders| -> Result<()> {
        if stage_if_changed(&path, &content, folders)? {
            written.push(path);
        }
        Ok(())
    };
    put(
        dir.join(PROJECT_FILE),
        to_toml(&project.meta, &dir.join(PROJECT_FILE))?,
        folders,
    )?;
    put(dir.join(DEVICES_FILE), to_json(&project.devices)?, folders)?;
    put(
        dir.join(LOCK_FILE),
        to_toml(&project.lock, &dir.join(LOCK_FILE))?,
        folders,
    )?;

    let pages_dir = dir.join(PAGES_DIR);
    let mut keep = std::collections::BTreeSet::new();
    for page in &project.pages {
        check_id(&page.id)?;
        let name = format!("{}{PAGE_SUFFIX}", page.id);
        put(pages_dir.join(&name), to_json(page)?, folders)?;
        keep.insert(name);
    }
    clean_up(&pages_dir, &keep, PAGE_SUFFIX, folders)?;
    let panels_dir = dir.join(PANELS_DIR);
    let mut keep = std::collections::BTreeSet::new();
    for panel in &project.panels {
        check_id(&panel.id)?;
        let name = format!("{}{PANEL_SUFFIX}", panel.id);
        put(panels_dir.join(&name), to_json(panel)?, folders)?;
        keep.insert(name);
    }
    clean_up(&panels_dir, &keep, PANEL_SUFFIX, folders)?;
    let comments_dir = dir.join(COMMENTS_DIR);
    let mut by_page: BTreeMap<&str, Vec<&crate::review::Comment>> = BTreeMap::new();
    // Comments on a page that was deleted go with it.
    for c in project.comments.iter().filter(|c| project.page(&c.page).is_some()) {
        by_page.entry(c.page.as_str()).or_default().push(c);
    }
    let mut keep = std::collections::BTreeSet::new();
    for (page, list) in &by_page {
        check_id(page)?;
        let name = format!("{page}{COMMENTS_SUFFIX}");
        put(comments_dir.join(&name), to_json(list)?, folders)?;
        keep.insert(name);
    }
    clean_up(&comments_dir, &keep, COMMENTS_SUFFIX, folders)?;
    remove_unused_box_symbols(dir, project, folders)?;
    Ok(written)
}

/// Remove the files of black box symbols (`box.<page>.<id>`, the project's own symbols for the
/// boxes drawn on its pages) that no page places and the lock does not pin: left behind by a box
/// that was undone, or drawn and never saved. Other vendored symbols are left alone.
fn remove_unused_box_symbols(dir: &Path, project: &Project, folders: &mut Folders) -> Result<()> {
    let folder = dir.join(VENDOR_DIR).join(ItemKind::Symbol.dir());
    let Some(resolved) = box_symbol_folder(dir)? else {
        return Ok(());
    };
    let used: Vec<String> = project
        .lock
        .symbols
        .keys()
        .chain(project.pages.iter().flat_map(|p| p.symbols.iter().map(|s| &s.symbol)))
        .filter(|id| is_box_symbol(id))
        .map(|id| format!("{id}.json"))
        .collect();
    let listed = fs::read_dir(&resolved)
        .map_err(|e| Error::io(&folder, e))?
        .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<Vec<String>>>()
        .map_err(|e| Error::io(&folder, e))?;
    // A used name that is a link keeps the file it points to.
    let mut targets: Vec<std::ffi::OsString> = Vec::new();
    for u in &used {
        let path = resolved.join(u);
        let target = match path.canonicalize() {
            Ok(target) => target,
            // A used name with no file (or a dangling link) protects nothing.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(Error::io(&path, e)),
        };
        if target.parent() == Some(resolved.as_path())
            && let Some(name) = target.file_name()
        {
            targets.push(name.to_owned());
        }
    }
    for name in &listed {
        // A case variant of a used name is kept only when the used name itself is not
        // listed: then the folder ignores case and the variant is the used file. When both are
        // listed, the variant is a separate, unused file.
        if !is_box_symbol(name)
            || !name.ends_with(".json")
            || used
                .iter()
                .any(|u| u.eq_ignore_ascii_case(name) && (u == name || !listed.contains(u)))
            || targets.iter().any(|t| t.as_os_str() == std::ffi::OsStr::new(name))
        {
            continue;
        }
        let path = resolved.join(name);
        // Only plain files: never a folder or a link someone put there.
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_file() => {}
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(Error::io(&path, e)),
        }
        match fs::remove_file(&path) {
            Ok(()) => {
                folders.insert(resolved.clone());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(&path, e)),
        }
    }
    Ok(())
}

/// The project's vendored symbol folder, resolved, when a save may remove black box symbol
/// files from it: `None` when it does not exist or is a link to outside the project folder
/// (the files there are not this project's to remove).
fn box_symbol_folder(dir: &Path) -> Result<Option<PathBuf>> {
    let folder = dir.join(VENDOR_DIR).join(ItemKind::Symbol.dir());
    let resolved = match folder.canonicalize() {
        Ok(resolved) => resolved,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io(&folder, e)),
    };
    let root = dir.canonicalize().map_err(|e| Error::io(dir, e))?;
    Ok(resolved.starts_with(&root).then_some(resolved))
}

/// Every file that [`save`] may remove as an unused black box symbol: the plain
/// `box.<page>.<id>.json` files in the project's vendored symbol folder. A command that puts the
/// project back after a failed save keeps a copy of these too, so a cleanup that removed some
/// of them before failing can be undone.
pub fn removable_box_symbol_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let Some(resolved) = box_symbol_folder(dir)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in fs::read_dir(&resolved).map_err(|e| Error::io(&resolved, e))? {
        let entry = entry.map_err(|e| Error::io(&resolved, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_box_symbol(&name) || !name.ends_with(".json") {
            continue;
        }
        let path = entry.path();
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_file() => out.push(path),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(&path, e)),
        }
    }
    out.sort();
    Ok(out)
}

/// Whether a symbol id (or its file name) is a black box's own symbol, `box.<page>.<id>`.
fn is_box_symbol(id: &str) -> bool {
    id.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("box."))
}

/// Refuse two ids that differ only in case, naming both.
fn unique_ignoring_case<'a>(kind: &str, ids: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    for id in ids {
        if let Some(other) = seen.insert(id.to_ascii_lowercase(), id)
            && other != id
        {
            return Err(Error::Invalid(format!(
                "{kind} ids {other:?} and {id:?} differ only in case, so they would be one file on \
                 macOS and Windows; rename one of them"
            )));
        }
    }
    Ok(())
}

/// What to do with a file left in a folder after a save.
#[derive(Debug, PartialEq, Eq)]
enum Tidy {
    /// Nothing uses it any more.
    Remove(String),
    /// It is a file just written, listed under the old case of its name: the folder ignores case.
    Rename { from: String, to: String },
}

/// Which of the `listed` file names ending in `suffix` are not among the `keep` names written.
///
/// A name that differs from a kept one only in case is the same file on a folder that ignores
/// case, which still lists it under its old name: it is renamed to the new name, never removed.
/// When the folder lists both names, they are two files and the old one goes.
fn tidy(listed: &[String], keep: &std::collections::BTreeSet<String>, suffix: &str) -> Vec<Tidy> {
    let mut out = Vec::new();
    for name in listed {
        if !name.ends_with(suffix) || keep.contains(name) {
            continue;
        }
        match keep.iter().find(|k| k.eq_ignore_ascii_case(name)) {
            Some(k) if !listed.contains(k) => out.push(Tidy::Rename {
                from: name.clone(),
                to: k.clone(),
            }),
            _ => out.push(Tidy::Remove(name.clone())),
        }
    }
    out
}

/// Remove the files in `dir` ending in `suffix` that the save did not write (see [`tidy`]).
fn clean_up(dir: &Path, keep: &std::collections::BTreeSet<String>, suffix: &str, folders: &mut Folders) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|e| Error::io(dir, e))?;
    let names = entries.map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()));
    clean_up_listed(dir, names, keep, suffix, folders)
}

/// [`clean_up`] on the names a listing of `dir` gave.
fn clean_up_listed(
    dir: &Path,
    names: impl Iterator<Item = std::io::Result<String>>,
    keep: &std::collections::BTreeSet<String>,
    suffix: &str,
    folders: &mut Folders,
) -> Result<()> {
    // A listing cut short would look like a folder without the file just written: stop instead.
    let listed = names
        .collect::<std::io::Result<Vec<String>>>()
        .map_err(|e| Error::io(dir, e))?;
    for step in tidy(&listed, keep, suffix) {
        // Each step changes the folder's entries, so the folder is synced with the save.
        folders.insert(dir.to_owned());
        match step {
            Tidy::Remove(name) => {
                let path = dir.join(name);
                fs::remove_file(&path).map_err(|e| Error::io(&path, e))?;
            }
            Tidy::Rename { from, to } => {
                let to = dir.join(to);
                fs::rename(dir.join(from), &to).map_err(|e| Error::io(&to, e))?;
            }
        }
    }
    Ok(())
}

/// Create a new, empty project folder. Refuses a folder that already holds a project.
pub fn create(dir: &Path, name: &str) -> Result<Project> {
    if dir.join(PROJECT_FILE).exists() {
        return Err(Error::Invalid(format!("{} already holds a project", dir.display())));
    }
    let mut project = Project::new(name);
    let mut cover = Page::new("p0001", "1", "Cover");
    cover.cover = true;
    project.pages.push(cover);
    save(dir, &project)?;
    Ok(project)
}

/// Where a vendored item of this kind and id lives in the project folder.
pub fn vendor_path(dir: &Path, kind: ItemKind, id: &str) -> Result<PathBuf> {
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
            cable: None,
            route: Default::default(),
            spec: Default::default(),
        });
        p.pages.push(page);
        p.devices.insert(
            DeviceTag::parse("+CAB1-K1")?,
            Device {
                part: Some("acme.r1".into()),
                ..Default::default()
            },
        );
        let mut panel = crate::panel::Panel::with_rails("main", "Cabinet", 600.0, 800.0);
        panel.items.push(crate::panel::PanelItem::Device {
            tag: DeviceTag::parse("+CAB1-K1")?,
            at: Point::new(50.0, 100.0),
            width: 45.0,
            height: 85.0,
            rotation: crate::geom::Rotation::R0,
        });
        p.panels.push(panel);
        p.add_comment(
            "p0002",
            Some(Point::new(40.0, 60.0)),
            "Kari",
            1_700_000_000,
            "Is K1 rated for 24 V?",
        )?;
        p.reply_to_comment("c1", "Ola", 1_700_000_100, "Yes, coil is 24 V DC")?;
        p.add_comment("p0002", None, "Ola", 1_700_000_200, "Page title should say control")?;
        p.resolve_comment("c2", true)?;
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
    fn a_load_refuses_two_devices_json_entries_for_one_device() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        save(dir.path(), &Project::new("Demo"))?;
        let path = dir.path().join(DEVICES_FILE);
        // The same tag twice, and one tag in two spellings: either would keep only the last.
        for (first, second) in [("-K1", "-K1"), ("+X1-K1", "-K1+X1")] {
            let text = format!("{{\n  {first:?}: {{ \"part\": \"a\" }},\n  {second:?}: {{ \"part\": \"b\" }}\n}}\n");
            fs::write(&path, text).map_err(|e| Error::io(&path, e))?;
            let Err(e) = load(dir.path()) else {
                unreachable!("{first} and {second} loaded as one device");
            };
            let message = format!("{e} {:?}", std::error::Error::source(&e).map(ToString::to_string));
            assert!(
                message.contains(&format!("{first:?}")) && message.contains(&format!("{second:?}")),
                "{message}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_save_removes_only_box_symbols_no_page_places_and_the_lock_does_not_pin() -> Result<()> {
        let tmp = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let dir = tmp.path();
        let mut p = sample()?;
        p.pages[0].symbols[0].symbol = "box.p0002.s1".into();
        p.lock.symbols.insert(
            "box.p0002.s7".into(),
            crate::LockEntry {
                source: "project".into(),
                hash: "h".into(),
            },
        );
        let folder = dir.join(VENDOR_DIR).join(ItemKind::Symbol.dir());
        fs::create_dir_all(folder.join("box.p0002.s9.json")).map_err(|e| Error::io(&folder, e))?;
        for name in [
            "box.p0002.s1.json",
            "box.p0002.s7.json",
            "box.p0002.s2.json",
            "BOX.P0002.S3.json",
            "box.p0002.s4.txt",
            "wc.coil.json",
        ] {
            fs::write(folder.join(name), "{}").map_err(|e| Error::io(name, e))?;
        }
        save(dir, &p)?;
        let mut left: Vec<String> = fs::read_dir(&folder)
            .map_err(|e| Error::io(&folder, e))?
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "box.p0002.s1.json",
                "box.p0002.s4.txt",
                "box.p0002.s7.json",
                "box.p0002.s9.json",
                "wc.coil.json"
            ],
            "placed, pinned, not a symbol file, a folder and other symbols stay"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_save_never_removes_box_symbols_outside_the_project_folder() -> Result<()> {
        let tmp = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let other = tmp.path().join("other");
        let elsewhere = other.join("symbols");
        fs::create_dir_all(&elsewhere).map_err(|e| Error::io(&elsewhere, e))?;
        fs::write(elsewhere.join("box.p1.s1.json"), "{}").map_err(|e| Error::io("write", e))?;
        let dir = tmp.path().join("proj");
        fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        // `vendor` itself is a link to another project's folder.
        std::os::unix::fs::symlink(&other, dir.join(VENDOR_DIR)).map_err(|e| Error::io("link", e))?;
        save(&dir, &Project::new("Empty"))?;
        assert!(elsewhere.join("box.p1.s1.json").exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_save_keeps_the_file_a_used_box_symbol_links_to() -> Result<()> {
        let tmp = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let dir = tmp.path();
        let mut p = Project::new("Linked");
        let mut page = Page::new("p1", "1", "One");
        page.symbols.push(PlacedSymbol {
            id: "s1".into(),
            symbol: "box.p1.s1".into(),
            at: Point::new(0.0, 0.0),
            rotation: Default::default(),
            mirror: false,
            tag: None,
        });
        p.pages.push(page);
        let folder = dir.join(VENDOR_DIR).join(ItemKind::Symbol.dir());
        fs::create_dir_all(&folder).map_err(|e| Error::io(&folder, e))?;
        fs::write(folder.join("box.p1.s9.json"), "{}").map_err(|e| Error::io("write", e))?;
        std::os::unix::fs::symlink("box.p1.s9.json", folder.join("box.p1.s1.json"))
            .map_err(|e| Error::io("link", e))?;
        save(dir, &p)?;
        assert!(folder.join("box.p1.s9.json").exists(), "the link's target stays");
        assert!(
            fs::read(folder.join("box.p1.s1.json")).is_ok(),
            "the placed symbol reads"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_save_reports_a_used_box_symbol_it_cannot_resolve_and_removes_nothing() -> Result<()> {
        let tmp = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let dir = tmp.path();
        let mut p = Project::new("Loop");
        let mut page = Page::new("p1", "1", "One");
        page.symbols.push(PlacedSymbol {
            id: "s1".into(),
            symbol: "box.p1.s1".into(),
            at: Point::new(0.0, 0.0),
            rotation: Default::default(),
            mirror: false,
            tag: None,
        });
        p.pages.push(page);
        let folder = dir.join(VENDOR_DIR).join(ItemKind::Symbol.dir());
        fs::create_dir_all(&folder).map_err(|e| Error::io(&folder, e))?;
        fs::write(folder.join("box.p1.s2.json"), "{}").map_err(|e| Error::io("write", e))?;
        // The used name is a link to itself: resolving it fails with something other than
        // NotFound.
        std::os::unix::fs::symlink("box.p1.s1.json", folder.join("box.p1.s1.json"))
            .map_err(|e| Error::io("link", e))?;
        assert!(save(dir, &p).is_err(), "the resolution error is reported");
        assert!(
            folder.join("box.p1.s2.json").exists(),
            "nothing is removed when a used name cannot be resolved"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_save_removes_an_unused_case_duplicate_but_keeps_a_link_target() -> Result<()> {
        let tmp = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let dir = tmp.path();
        let mut p = Project::new("Cases");
        let mut page = Page::new("p1", "1", "One");
        for (id, symbol) in [("s1", "box.p1.s1"), ("s2", "box.p1.s2")] {
            page.symbols.push(PlacedSymbol {
                id: id.into(),
                symbol: symbol.into(),
                at: Point::new(0.0, 0.0),
                rotation: Default::default(),
                mirror: false,
                tag: None,
            });
        }
        p.pages.push(page);
        let folder = dir.join(VENDOR_DIR).join(ItemKind::Symbol.dir());
        fs::create_dir_all(&folder).map_err(|e| Error::io(&folder, e))?;
        // Only on a folder that tells case apart; one that ignores case has no duplicates.
        fs::write(folder.join("probe.txt"), "").map_err(|e| Error::io("write", e))?;
        let ignores_case = folder.join("PROBE.txt").exists();
        fs::remove_file(folder.join("probe.txt")).map_err(|e| Error::io("remove", e))?;
        if ignores_case {
            return Ok(());
        }
        // A folder that tells case apart: the duplicate is its own, unused file.
        for name in ["box.p1.s1.json", "BOX.P1.S1.json", "Box.P1.S2.json"] {
            fs::write(folder.join(name), "{}").map_err(|e| Error::io("write", e))?;
        }
        // The used name is a link to a case variant, which must stay.
        std::os::unix::fs::symlink("Box.P1.S2.json", folder.join("box.p1.s2.json"))
            .map_err(|e| Error::io("link", e))?;
        save(dir, &p)?;
        assert!(folder.join("box.p1.s1.json").exists(), "the used file stays");
        assert!(!folder.join("BOX.P1.S1.json").exists(), "the unused duplicate goes");
        assert!(folder.join("Box.P1.S2.json").exists(), "the link's target stays");
        assert!(
            fs::read(folder.join("box.p1.s2.json")).is_ok(),
            "the linked symbol reads"
        );
        Ok(())
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_save_never_removes_a_file_it_wrote_under_another_case() {
        let keep = names(&["DXF-300.page.json", "p1.page.json"]).into_iter().collect();
        // A folder that ignores case lists the file just written under its old name.
        let listed = names(&["dxf-300.page.json", "p1.page.json", "gone.page.json", "notes.txt"]);
        assert_eq!(
            tidy(&listed, &keep, PAGE_SUFFIX),
            vec![
                Tidy::Rename {
                    from: "dxf-300.page.json".into(),
                    to: "DXF-300.page.json".into()
                },
                Tidy::Remove("gone.page.json".into()),
            ]
        );
        // A folder that keeps case lists both: two files, and the old one goes.
        let listed = names(&["dxf-300.page.json", "DXF-300.page.json", "p1.page.json"]);
        assert_eq!(
            tidy(&listed, &keep, PAGE_SUFFIX),
            vec![Tidy::Remove("dxf-300.page.json".into())]
        );
    }

    #[test]
    fn a_listing_cut_short_changes_nothing() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        // Names that differ by more than case, so this holds on every folder: a complete listing
        // would rename the first and remove the second.
        let files = [("dxf-300.page.json", "old"), ("gone.page.json", "stale")];
        for (name, body) in files {
            fs::write(dir.path().join(name), body).map_err(|e| Error::io(name, e))?;
        }
        let keep = names(&["DXF-300.page.json"]).into_iter().collect();
        let listing = vec![
            Ok("dxf-300.page.json".to_string()),
            Ok("gone.page.json".to_string()),
            Err(std::io::Error::other("listing cut short")),
        ];
        let out = clean_up_listed(dir.path(), listing.into_iter(), &keep, PAGE_SUFFIX, &mut Folders::new());
        assert!(out.is_err_and(|e| e.to_string().contains("listing cut short")));
        let mut left = names(&[]);
        for entry in fs::read_dir(dir.path()).map_err(|e| Error::io("tmp", e))? {
            left.push(
                entry
                    .map_err(|e| Error::io("tmp", e))?
                    .file_name()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        left.sort();
        assert_eq!(left, names(&["dxf-300.page.json", "gone.page.json"]));
        for (name, body) in files {
            let path = dir.path().join(name);
            assert_eq!(fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?, body);
        }
        Ok(())
    }

    #[test]
    fn a_page_renamed_only_in_case_is_saved_and_loads() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let mut p = sample()?;
        save(dir.path(), &p)?;
        let old = p.pages[0].id.clone();
        p.pages[0].id = old.to_ascii_uppercase();
        assert_ne!(p.pages[0].id, old);
        for c in p.comments.iter_mut().filter(|c| c.page == old) {
            c.page = p.pages[0].id.clone();
        }
        save(dir.path(), &p)?;
        let back = load(dir.path())?;
        assert_eq!(back, p);
        Ok(())
    }

    /// The message of the error `load` gave, or an error when it loaded.
    fn load_refused(dir: &Path) -> Result<String> {
        match load(dir) {
            Ok(_) => Err(Error::Invalid(format!("{} loaded", dir.display()))),
            Err(e) => Ok(e.to_string()),
        }
    }

    #[test]
    fn a_project_from_a_newer_format_is_refused_naming_its_file_and_the_remedy() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let mut p = sample()?;
        p.meta.format_version = ProjectMeta::FORMAT_VERSION + 1;
        save(dir.path(), &p)?;
        let msg = load_refused(dir.path())?;
        let file = dir.path().join(PROJECT_FILE);
        assert!(msg.contains(&file.display().to_string()), "{msg}");
        assert!(
            msg.contains(&format!(
                "project format {}, newer than this WiringCraft reads (format {})",
                ProjectMeta::FORMAT_VERSION + 1,
                ProjectMeta::FORMAT_VERSION
            )),
            "{msg}"
        );
        assert!(msg.contains("update WiringCraft to open it"), "{msg}");
        // The format this build writes loads.
        p.meta.format_version = ProjectMeta::FORMAT_VERSION;
        save(dir.path(), &p)?;
        assert_eq!(load(dir.path())?, p);
        Ok(())
    }

    #[test]
    fn a_page_or_panel_file_whose_id_differs_from_its_name_is_refused() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let mut p = sample()?;
        p.panels.push(crate::panel::Panel::new("main", "Main", 600.0, 800.0));
        save(dir.path(), &p)?;
        let page = p.pages[0].id.clone();
        let pages = dir.path().join(PAGES_DIR);
        let copy = pages.join(format!("copy{PAGE_SUFFIX}"));
        fs::copy(pages.join(format!("{page}{PAGE_SUFFIX}")), &copy).map_err(|e| Error::io(&copy, e))?;
        let msg = load_refused(dir.path())?;
        assert!(msg.contains(&copy.display().to_string()), "{msg}");
        assert!(msg.contains(&format!("holds page id {page:?}")), "{msg}");
        fs::remove_file(&copy).map_err(|e| Error::io(&copy, e))?;

        let panels = dir.path().join(PANELS_DIR);
        let moved = panels.join(format!("other{PANEL_SUFFIX}"));
        fs::rename(panels.join(format!("main{PANEL_SUFFIX}")), &moved).map_err(|e| Error::io(&moved, e))?;
        let msg = load_refused(dir.path())?;
        assert!(msg.contains(&moved.display().to_string()), "{msg}");
        assert!(msg.contains("holds panel id \"main\""), "{msg}");
        Ok(())
    }

    fn refused<T>(r: Result<T>) -> Result<String> {
        match r {
            Ok(_) => Err(Error::Invalid("saved ids that differ only in case".into())),
            Err(e) => Ok(e.to_string()),
        }
    }

    #[test]
    fn ids_that_differ_only_in_case_are_refused_before_anything_is_written() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let mut p = sample()?;
        let mut twin = p.pages[0].clone();
        twin.id = twin.id.to_ascii_uppercase();
        let (a, b) = (p.pages[0].id.clone(), twin.id.clone());
        p.pages.push(twin);
        let err = refused(save(dir.path(), &p))?;
        assert!(
            err.contains(&format!("{a:?}")) && err.contains(&format!("{b:?}")),
            "{err}"
        );
        assert!(!dir.path().join(PROJECT_FILE).exists());
        p.pages.pop();
        let mut twin = p.panels[0].clone();
        twin.id = twin.id.to_ascii_uppercase();
        p.panels.push(twin);
        let err = refused(save(dir.path(), &p))?;
        assert!(err.contains("panel ids"), "{err}");
        Ok(())
    }

    #[test]
    fn a_bad_page_or_panel_id_is_refused_before_anything_is_written() -> Result<()> {
        for bad_page in [true, false] {
            let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
            let mut p = sample()?;
            if bad_page {
                p.pages[0].id = "nul".into();
            } else {
                p.panels[0].id = "../x".into();
            }
            refused(save(dir.path(), &p))?;
            let left = fs::read_dir(dir.path()).map_err(|e| Error::io(dir.path(), e))?.count();
            assert_eq!(left, 0, "the folder stays empty (bad page id: {bad_page})");
        }
        Ok(())
    }

    #[test]
    fn deleted_pages_lose_their_file() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let mut p = sample()?;
        save(dir.path(), &p)?;
        assert!(dir.path().join("comments/p0002.comments.json").exists());
        p.pages.clear();
        save(dir.path(), &p)?;
        assert!(!dir.path().join("pages/p0002.page.json").exists());
        assert!(dir.path().join("panels/main.panel.json").exists());
        assert!(!dir.path().join("comments/p0002.comments.json").exists());
        p.panels.clear();
        save(dir.path(), &p)?;
        assert!(!dir.path().join("panels/main.panel.json").exists());
        Ok(())
    }

    #[test]
    fn a_new_project_starts_with_its_cover_page_and_older_pages_load_as_normal_pages() -> Result<()> {
        let dir = tempfile::tempdir().map_err(|e| Error::io("tmp", e))?;
        let p = create(dir.path(), "A")?;
        assert_eq!(p.cover_page(), Some(0));
        assert_eq!(load(dir.path())?.cover_page(), Some(0), "saved with the project");
        // A page written before cover pages existed has no `cover` key and is a normal page.
        let old: Page = serde_json::from_str(r#"{"id":"p0001","number":"1","title":"Cover"}"#)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        assert!(!old.cover);
        let normal = serde_json::to_string(&Page::new("p2", "2", "x")).map_err(|e| Error::Invalid(e.to_string()))?;
        assert!(
            !normal.contains("cover"),
            "normal pages keep their files as before: {normal}"
        );
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

    #[test]
    fn windows_device_names_are_refused() {
        for bad in ["con", "NUL", "Aux.page", "prn", "com1", "COM0", "lpt9.x.y"] {
            assert!(check_id(bad).is_err(), "{bad}");
        }
        for good in ["console", "com10", "aux1", "acme.aux", "nul_", "com", "lptx"] {
            assert!(check_id(good).is_ok(), "{good}");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod staging_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_link_at_the_staging_path_is_not_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("outside.txt");
        fs::write(&target, "keep me").unwrap();
        let path = dir.path().join("devices.json");
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        std::os::unix::fs::symlink(&target, PathBuf::from(&tmp)).unwrap();

        assert!(write_if_changed(&path, "new content\n").unwrap());
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep me");
        assert_eq!(fs::read_to_string(&path).unwrap(), "new content\n");
        assert!(fs::symlink_metadata(PathBuf::from(&tmp)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_at_the_staging_path_creates_nothing_outside() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere").join("created.txt");
        fs::create_dir(dir.path().join("elsewhere")).unwrap();
        let path = dir.path().join("pages").join("p1.page.json");
        fs::create_dir(dir.path().join("pages")).unwrap();
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        std::os::unix::fs::symlink(&target, PathBuf::from(&tmp)).unwrap();

        assert!(write_if_changed(&path, "{}\n").unwrap());
        assert!(!target.exists());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}\n");
    }

    /// Every file under `dir`, named from it, in order.
    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut todo = vec![dir.to_owned()];
        while let Some(folder) = todo.pop() {
            for entry in fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    todo.push(path);
                } else {
                    out.push(path.strip_prefix(dir).unwrap().to_owned());
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn a_save_leaves_no_staging_file_and_replaces_each_file_whole() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::new("Demo");
        for i in 0..5 {
            p.pages.push(Page::new(
                format!("p{i:04}"),
                i.to_string(),
                "A long page title that the second save cuts short",
            ));
        }
        save(dir.path(), &p).unwrap();
        for page in &mut p.pages {
            page.title = "T".into();
        }
        p.pages.pop();
        let written = save(dir.path(), &p).unwrap();

        assert_eq!(written.len(), 4, "{written:?}");
        let files = files_under(dir.path());
        assert!(
            files.iter().all(|f| f.extension().is_none_or(|x| x != "tmp")),
            "a staging file is left: {files:?}"
        );
        assert_eq!(files.iter().filter(|f| f.starts_with(PAGES_DIR)).count(), 4);
        for page in &p.pages {
            let file = dir.path().join(PAGES_DIR).join(format!("{}{PAGE_SUFFIX}", page.id));
            assert_eq!(fs::read_to_string(file).unwrap(), to_json(page).unwrap());
        }
        assert_eq!(load(dir.path()).unwrap(), p);
    }

    #[test]
    fn replace_whole_leaves_the_new_bytes_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.bin");
        fs::write(&path, "old and much longer than what replaces it").unwrap();

        replace_whole(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(files_under(dir.path()), [PathBuf::from("notes.bin")]);
    }

    #[test]
    fn a_write_that_cannot_take_its_name_leaves_no_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        // A folder that is not empty is in the way of the name, so the rename fails.
        let path = dir.path().join("taken");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("inside.txt"), "keep me").unwrap();

        assert!(replace_whole(&path, b"new").is_err());
        assert_eq!(files_under(dir.path()), [Path::new("taken").join("inside.txt")]);
        assert_eq!(fs::read_to_string(path.join("inside.txt")).unwrap(), "keep me");
    }

    #[test]
    fn a_folder_that_cannot_be_synced_does_not_fail_the_write() {
        use std::io::ErrorKind::{InvalidInput, PermissionDenied, Unsupported};
        let dir = Path::new("pages");
        assert!(folder_synced(dir, Ok(())).is_ok());
        for kind in [InvalidInput, PermissionDenied, Unsupported] {
            assert!(folder_synced(dir, Err(kind.into())).is_ok(), "{kind:?}");
        }
        // Anything else is a real failure, and names the folder.
        let failed = folder_synced(dir, Err(std::io::Error::other("the disk went away")));
        assert!(failed.unwrap_err().to_string().contains("pages"));
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_sync_the_system_calls_not_valid_is_tolerated_and_a_missing_folder_is_not() {
        // EINVAL: what a file system that cannot sync a folder answers.
        let not_valid = std::io::Error::from_raw_os_error(22);
        assert!(folder_synced(Path::new("pages"), Err(not_valid)).is_ok());

        let dir = tempfile::tempdir().unwrap();
        assert!(sync_dir(dir.path()).is_ok());
        let gone: Folders = [dir.path().join("gone")].into();
        assert!(sync_dir(&dir.path().join("gone")).is_err());
        assert!(synced(Ok(()), &gone).is_err());
    }

    #[test]
    fn a_failure_is_still_the_one_reported_once_the_folders_are_synced() {
        let dir = tempfile::tempdir().unwrap();
        let folders: Folders = [dir.path().to_owned()].into();
        assert_eq!(synced(Ok(7), &folders).unwrap(), 7);
        let failed: Result<u8> = synced(Err(Error::Invalid("first".into())), &folders);
        assert!(matches!(failed, Err(Error::Invalid(m)) if m == "first"));
    }

    #[test]
    fn a_write_names_each_folder_whose_entries_it_changed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let symbols = root.join("vendor").join("symbols");
        let path = symbols.join("a.json");

        // Two folders are made on the way: each is a new entry in the one above it.
        let mut folders = Folders::new();
        assert!(stage_if_changed(&path, "{}\n", &mut folders).unwrap());
        let expected: Folders = [root.to_owned(), root.join("vendor"), symbols.clone()].into();
        assert_eq!(folders, expected);

        let mut folders = Folders::new();
        assert!(!stage_if_changed(&path, "{}\n", &mut folders).unwrap());
        assert!(folders.is_empty(), "an unchanged file asked for {folders:?}");

        assert!(stage_if_changed(&path, "[]\n", &mut folders).unwrap());
        assert_eq!(folders, Folders::from([symbols]));
    }

    #[test]
    fn a_save_syncs_only_the_folders_it_changed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let pages = root.join(PAGES_DIR);
        let mut p = Project::new("Demo");
        for i in 0..3 {
            p.pages.push(Page::new(format!("p{i:04}"), i.to_string(), "Page"));
        }

        // However many pages are written, their folder is in the list once.
        let mut folders = Folders::new();
        assert_eq!(save_files(root, &p, &mut folders).unwrap().len(), 6);
        assert_eq!(folders, Folders::from([root.to_owned(), pages.clone()]));

        let mut folders = Folders::new();
        assert!(save_files(root, &p, &mut folders).unwrap().is_empty());
        assert!(folders.is_empty(), "an unchanged project asked for {folders:?}");

        // A removed page file changes its folder too.
        p.pages.pop();
        assert!(save_files(root, &p, &mut folders).unwrap().is_empty());
        assert_eq!(folders, Folders::from([pages]));
    }

    #[test]
    fn a_staging_file_left_by_a_failed_write_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project.toml");
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        fs::write(PathBuf::from(&tmp), "stale and much longer than the new content").unwrap();

        assert!(write_if_changed(&path, "x\n").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "x\n");
        assert!(!PathBuf::from(&tmp).exists());
    }

    /// Each sync this thread makes, in order, while the watch lives.
    type Seen = std::rc::Rc<std::cell::RefCell<Vec<(Synced, PathBuf)>>>;

    /// Takes the watch off the syncs again when it goes.
    struct Watch;

    impl Drop for Watch {
        fn drop(&mut self) {
            SYNC_WATCH.with(|watch| *watch.borrow_mut() = None);
        }
    }

    /// Record every sync this thread makes, and fail the ones `fail` picks.
    fn watch(fail: impl Fn(Synced, &Path) -> bool + 'static) -> (Watch, Seen) {
        let seen = Seen::default();
        let log = seen.clone();
        let tell = move |what: Synced, path: &Path| {
            log.borrow_mut().push((what, path.to_owned()));
            if fail(what, path) {
                return Err(std::io::Error::other("the disk went away"));
            }
            Ok(())
        };
        SYNC_WATCH.with(|watch| *watch.borrow_mut() = Some(Box::new(tell)));
        (Watch, seen)
    }

    /// The syncs seen so far, and none of them after this.
    fn taken(seen: &Seen) -> Vec<(Synced, PathBuf)> {
        std::mem::take(&mut *seen.borrow_mut())
    }

    fn folders_of(seen: &Seen) -> Vec<PathBuf> {
        let all = taken(seen);
        all.into_iter()
            .filter(|(what, _)| *what == Synced::Folder)
            .map(|(_, p)| p)
            .collect()
    }

    fn staging_of(path: &Path) -> PathBuf {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        PathBuf::from(tmp)
    }

    #[test]
    fn the_staged_file_is_synced_before_it_gets_its_name_and_the_folder_after() {
        let dir = tempfile::tempdir().unwrap();
        let root = resolved(dir.path());
        let path = root.join("devices.json");
        fs::write(&path, "old\n").unwrap();

        // What the file under the real name held at each sync.
        let held = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let (at, log) = (path.clone(), held.clone());
        let tell = move |what: Synced, synced: &Path| {
            let holds = fs::read_to_string(&at)?;
            log.borrow_mut().push((what, synced.to_owned(), holds));
            Ok(())
        };
        SYNC_WATCH.with(|watch| *watch.borrow_mut() = Some(Box::new(tell)));
        let _watch = Watch;

        assert!(write_if_changed(&path, "new\n").unwrap());
        assert_eq!(
            *held.borrow(),
            [
                (Synced::File, staging_of(&path), "old\n".to_string()),
                (Synced::Folder, root.clone(), "new\n".to_string()),
            ]
        );

        held.borrow_mut().clear();
        replace_whole(&path, b"newer\n").unwrap();
        assert_eq!(
            *held.borrow(),
            [
                (Synced::File, staging_of(&path), "new\n".to_string()),
                (Synced::Folder, root, "newer\n".to_string()),
            ]
        );
    }

    #[test]
    fn a_staged_file_that_cannot_be_synced_never_gets_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = resolved(dir.path());
        let path = root.join("devices.json");
        fs::write(&path, "old\n").unwrap();
        let (_watch, seen) = watch(|what, _| what == Synced::File);

        let failed = write_if_changed(&path, "new\n").unwrap_err().to_string();
        assert!(
            failed.contains("devices.json.tmp") && failed.contains("the disk went away"),
            "{failed}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "old\n");
        assert_eq!(files_under(&root), [PathBuf::from("devices.json")]);
        // No entry changed, so no folder is synced.
        assert_eq!(taken(&seen), [(Synced::File, staging_of(&path))]);

        assert!(replace_whole(&path, b"new\n").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "old\n");
        assert_eq!(files_under(&root), [PathBuf::from("devices.json")]);
        assert_eq!(taken(&seen), [(Synced::File, staging_of(&path))]);
    }

    fn three_pages() -> Project {
        let mut p = Project::new("Demo");
        for i in 0..3 {
            p.pages.push(Page::new(format!("p{i:04}"), i.to_string(), "Page"));
        }
        p
    }

    #[test]
    fn a_save_syncs_each_file_it_writes_and_each_folder_it_changed_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = resolved(dir.path());
        let pages = root.join(PAGES_DIR);
        let mut p = three_pages();
        let (_watch, seen) = watch(|_, _| false);

        // Six files, two folders: the three pages share one folder sync, after all the files.
        let written = save(&root, &p).unwrap();
        assert_eq!(written.len(), 6);
        let mut expected: Vec<(Synced, PathBuf)> = written.iter().map(|w| (Synced::File, staging_of(w))).collect();
        expected.push((Synced::Folder, root.clone()));
        expected.push((Synced::Folder, pages.clone()));
        assert_eq!(taken(&seen), expected);

        // Nothing changed: no sync at all.
        assert!(save(&root, &p).unwrap().is_empty());
        assert_eq!(taken(&seen), []);

        // One page changed: its file and its folder, nothing else.
        p.pages[1].title = "Changed".into();
        let page = pages.join(format!("p0001{PAGE_SUFFIX}"));
        assert_eq!(save(&root, &p).unwrap(), std::slice::from_ref(&page));
        assert_eq!(
            taken(&seen),
            [(Synced::File, staging_of(&page)), (Synced::Folder, pages.clone())]
        );

        // A page removed: no file to sync, and the folder it left.
        p.pages.pop();
        assert!(save(&root, &p).unwrap().is_empty());
        assert_eq!(taken(&seen), [(Synced::Folder, pages)]);
    }

    #[test]
    fn a_folder_sync_that_fails_after_a_failed_write_leaves_the_first_failure_reported() {
        let dir = tempfile::tempdir().unwrap();
        let root = resolved(dir.path());
        let pages = root.join(PAGES_DIR);
        let p = three_pages();
        // The second page cannot be written, after the first is in place: a folder that is not
        // empty stands where its file goes. (An id that is no file name is refused before any
        // write, so it cannot make a write fail half way.)
        let blocker = pages.join(format!("p0001{PAGE_SUFFIX}"));
        fs::create_dir_all(blocker.join("in-the-way")).unwrap();
        let (_watch, seen) = watch(|what, _| what == Synced::Folder);

        let failed = save(&root, &p);
        // The page write failure is the one reported, not the folder sync failure after it.
        assert!(
            matches!(&failed, Err(Error::Io { path, .. }) if *path == blocker),
            "{failed:?}"
        );
        // Both folders with a file in place were still synced, failing or not.
        assert_eq!(folders_of(&seen), [root.clone(), pages.clone()]);
        assert!(pages.join(format!("p0000{PAGE_SUFFIX}")).exists());

        // With no write failure, the folder sync failure is the one reported.
        fs::remove_dir_all(&blocker).unwrap();
        let failed = save(&root, &p).unwrap_err().to_string();
        assert!(failed.contains("the disk went away"), "{failed}");
    }

    #[test]
    fn a_folder_whose_sync_failed_is_synced_by_the_next_write_of_the_same_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let root = resolved(dir.path());
        let symbols = root.join("vendor").join("symbols");
        let path = symbols.join("a.json");
        let broken = std::rc::Rc::new(std::cell::Cell::new(true));
        let now = broken.clone();
        let (_watch, seen) = watch(move |what, _| what == Synced::Folder && now.get());
        let owed = [root.clone(), root.join("vendor"), symbols.clone()];

        // The file gets into place, with the two folders made for it; no folder sync holds.
        assert!(write_if_changed(&path, "{}\n").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}\n");
        assert_eq!(folders_of(&seen), owed);

        // A second try has nothing to write. It still fails while the folders cannot be synced.
        assert!(write_if_changed(&path, "{}\n").is_err());
        assert_eq!(taken(&seen), owed.clone().map(|dir| (Synced::Folder, dir)));

        // Once they can, it syncs all three and says the file did not change.
        broken.set(false);
        assert!(!write_if_changed(&path, "{}\n").unwrap());
        assert_eq!(taken(&seen), owed.map(|dir| (Synced::Folder, dir)));

        // Nothing is owed after that: an unchanged file costs no sync again.
        assert!(!write_if_changed(&path, "{}\n").unwrap());
        assert_eq!(taken(&seen), []);

        // A changed file whose folder is owed syncs that folder once, not twice.
        broken.set(true);
        assert!(write_if_changed(&path, "[]\n").is_err());
        broken.set(false);
        taken(&seen);
        assert!(write_if_changed(&path, "{}\n").unwrap());
        assert_eq!(folders_of(&seen), [symbols]);
    }

    #[test]
    fn a_save_that_only_removed_a_file_still_owes_the_folder_sync_that_failed() {
        let dir = tempfile::tempdir().unwrap();
        let root = resolved(dir.path());
        let pages = root.join(PAGES_DIR);
        let mut p = three_pages();
        save(&root, &p).unwrap();

        let broken = std::rc::Rc::new(std::cell::Cell::new(true));
        let now = broken.clone();
        let (_watch, seen) = watch(move |what, _| what == Synced::Folder && now.get());
        p.pages.pop();
        assert!(save(&root, &p).is_err());
        assert!(!pages.join(format!("p0002{PAGE_SUFFIX}")).exists());
        assert_eq!(taken(&seen), [(Synced::Folder, pages.clone())]);

        // The file is gone, so the next save finds nothing to do but the sync it owes.
        broken.set(false);
        assert!(save(&root, &p).unwrap().is_empty());
        assert_eq!(taken(&seen), [(Synced::Folder, pages.clone())]);
        assert!(save(&root, &p).unwrap().is_empty());
        assert_eq!(taken(&seen), []);

        // A write elsewhere in the project folder gets an owed page folder synced too.
        broken.set(true);
        p.pages.pop();
        assert!(save(&root, &p).is_err());
        broken.set(false);
        taken(&seen);
        let meta = fs::read_to_string(root.join(PROJECT_FILE)).unwrap();
        assert!(!write_if_changed(&root.join(PROJECT_FILE), &meta).unwrap());
        assert_eq!(taken(&seen), [(Synced::Folder, pages)]);
    }

    #[test]
    fn an_owed_folder_that_is_gone_is_owed_no_more() {
        let dir = tempfile::tempdir().unwrap();
        let root = resolved(dir.path());
        let pages = root.join(PAGES_DIR);
        let path = pages.join("a.json");
        fs::create_dir(&pages).unwrap();
        let (_watch, seen) = watch(|what, path| what == Synced::Folder && path.ends_with(PAGES_DIR));

        assert!(write_if_changed(&path, "{}\n").is_err());
        fs::remove_dir_all(&pages).unwrap();
        taken(&seen);
        assert!(write_if_changed(&root.join("b.json"), "{}\n").unwrap());
        assert_eq!(folders_of(&seen), [root]);
    }

    #[test]
    fn a_sync_does_not_pay_a_debt_noted_while_it_ran() {
        let dir = tempfile::tempdir().unwrap();
        let root = resolved(dir.path());
        let is_owed = || owed().contains_key(&root);
        // A sync begins; while it runs, another write's sync of the folder fails and is noted.
        let before = OWED_NOTED.load(std::sync::atomic::Ordering::SeqCst);
        settle(&root, true, before);
        // The first sync went through, but may not have covered the other write's rename.
        settle(&root, false, before);
        assert!(is_owed());
        // A sync that began after the debt was noted pays it.
        settle(&root, false, OWED_NOTED.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!is_owed());
    }
}

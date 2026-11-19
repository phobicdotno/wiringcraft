//! Keeping written files off a project's files while someone else holds one of its pages in
//! a shared session (#282, #458).
//!
//! The app looks at a command's output paths before it starts the command line
//! ([`ProjectFiles::holds`]). That look cannot see a link swapped in afterwards, so the
//! command line opens each output file through [`ProjectFiles::create`] too: the open itself
//! does not follow a link at the file, and the file it got is checked before anything in it
//! changes.

use std::collections::HashSet;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

/// The environment variable the app sets when it starts the command line while someone else
/// holds a page: the project folder whose files an output file must not land on.
pub const GUARD_ENV: &str = "WIRINGCRAFT_GUARD_PROJECT";

/// The project's files, to tell whether an output file could land on one.
#[derive(Debug, Clone)]
pub struct ProjectFiles {
    /// Where the project's files really are: the project folder, and where each link in it
    /// points (a `pages` folder linked to elsewhere is the project's too).
    pub roots: Vec<PathBuf>,
    /// The identity of each project file ([`file_id`]), so a hard link to one is known too.
    pub files: HashSet<(u64, u64)>,
    /// Why a project file's identity could not be read, if one could not: a hard link to that
    /// file would go unseen, so no output file is taken as safe then.
    pub unknown: Option<String>,
}

impl ProjectFiles {
    /// The files of the project in `project`, `.git` left out.
    pub fn new(project: &Path) -> Self {
        let (roots, mut unknown) = project_roots(project);
        let mut files = HashSet::new();
        let mut todo = roots.clone();
        // A folder that cannot be listed, or an entry that cannot be looked at, hides files
        // too, so it is recorded like a file whose identity cannot be read.
        let mut note = |at: &Path, e: io::Error| {
            unknown.get_or_insert_with(|| format!("{}: {e}", at.display()));
        };
        while let Some(at) = todo.pop() {
            let m = match std::fs::symlink_metadata(&at) {
                Ok(m) => m,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => {
                    note(&at, e);
                    continue;
                }
            };
            if m.is_dir() {
                let entries = match std::fs::read_dir(&at) {
                    Ok(entries) => entries,
                    Err(e) => {
                        note(&at, e);
                        continue;
                    }
                };
                for entry in entries {
                    match entry {
                        Ok(e) if e.file_name() == ".git" => {}
                        Ok(e) => todo.push(e.path()),
                        Err(e) => note(&at, e),
                    }
                }
            } else if m.is_file() {
                match file_id(&at) {
                    Ok(Some(id)) => {
                        files.insert(id);
                    }
                    Ok(None) => {}
                    Err(e) => note(&at, e),
                }
            }
        }
        Self { roots, files, unknown }
    }

    /// The project named by [`GUARD_ENV`], when it is set.
    pub fn from_env() -> Option<Self> {
        let dir = std::env::var_os(GUARD_ENV)?;
        (!dir.is_empty()).then(|| Self::new(Path::new(&dir)))
    }

    /// Whether `path`, as the file system resolves it, is in the project or is one of its
    /// files under another name. An error when that cannot be told.
    pub fn holds(&self, path: &Path) -> io::Result<bool> {
        Ok(inside(&self.roots, path) || self.same_file(path)?)
    }

    /// Whether `path` is a project file under another name (a hard link to it). Nothing there,
    /// or a folder, is not; a file whose identity, or a project file's, cannot be read is an
    /// error, with the reason.
    pub fn same_file(&self, path: &Path) -> io::Result<bool> {
        match std::fs::metadata(path) {
            Ok(m) if m.is_file() => {}
            Ok(_) => return Ok(false),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        }
        self.known(file_id(path)?)
    }

    /// Whether the file with identity `id` is a project file; an error when the project's own
    /// identities are not all known.
    fn known(&self, id: Option<(u64, u64)>) -> io::Result<bool> {
        if let Some(why) = &self.unknown {
            return Err(io::Error::other(format!(
                "the identity of a project file could not be read ({why})"
            )));
        }
        Ok(id.is_some_and(|id| self.files.contains(&id)))
    }

    /// Open `path` to write an output file into, empty, refusing a project file (#458).
    ///
    /// The open does not follow a link at `path` itself (a link there is refused), and does
    /// not empty the file. Only once the file it got is known not to be a project file (by its
    /// identity, so also a hard link to one, or a folder above swapped for a link) is it
    /// emptied. A file this call made and then refused is removed again.
    pub fn create(&self, path: &Path) -> io::Result<File> {
        let refuse = |why: &str| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} {why}, and someone else has a page of the project open in the shared session",
                    path.display()
                ),
            ))
        };
        match self.holds(path) {
            Ok(false) => {}
            Ok(true) => return refuse("is in the project folder or is a project file"),
            Err(e) => return refuse(&format!("could not be told apart from the project's files ({e})")),
        }
        let (file, made) = open_no_follow(path)?;
        let why = match self.check_opened(path, &file) {
            Ok(()) => {
                file.set_len(0)?;
                return Ok(file);
            }
            Err(why) => why,
        };
        let id = handle_id(&file).ok().flatten();
        drop(file);
        if made && id.is_some() {
            // The name may lead elsewhere by now; only the file this call made goes.
            let at = real(path);
            if file_id(&at).ok().flatten() == id {
                let _ = std::fs::remove_file(&at);
            }
        }
        refuse(&why)
    }

    /// Why the file `file`, just opened at `path`, must not be written, if it must not.
    fn check_opened(&self, path: &Path, file: &File) -> Result<(), String> {
        let meta = file.metadata().map_err(|e| format!("could not be looked at ({e})"))?;
        if !meta.is_file() || is_link(&meta) {
            return Err("is not a plain file".into());
        }
        let id = handle_id(file).map_err(|e| format!("could not be looked at ({e})"))?;
        match self.known(id) {
            Ok(false) => {}
            Ok(true) => return Err("is a project file".into()),
            Err(e) => return Err(format!("could not be told apart from the project's files ({e})")),
        }
        // Where the name leads now: a folder above swapped for a link into the project shows
        // there, and a name that no longer leads to the opened file means it changed meanwhile.
        let at = real(path);
        if inside(&self.roots, &at) {
            return Err("is in the project folder".into());
        }
        match (id, file_id(&at)) {
            (Some(id), Ok(Some(now))) if id != now => Err("changed while it was opened".into()),
            (Some(_), Err(e)) => Err(format!("could not be looked at after opening ({e})")),
            _ => Ok(()),
        }
    }
}

/// Write `bytes` to the output file `path`, through `guard` when there is one
/// ([`ProjectFiles::create`]), else as [`std::fs::write`] does.
pub fn write_output(guard: Option<&ProjectFiles>, path: &Path, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    match guard {
        None => std::fs::write(path, bytes),
        Some(g) => {
            use std::io::Write;
            let mut f = g.create(path)?;
            f.write_all(bytes.as_ref())?;
            f.sync_all()
        }
    }
}

/// Open `path` to write without following a link at it and without emptying it, made when
/// nothing is there; with whether this call made it.
fn open_no_follow(path: &Path) -> io::Result<(File, bool)> {
    let opts = |new: bool| {
        let mut o = std::fs::OpenOptions::new();
        o.write(true);
        if new {
            // A new file is never made through a link: an existing name, a link too, fails.
            o.create_new(true);
        }
        no_follow(&mut o);
        o
    };
    match opts(true).open(path) {
        Ok(f) => Ok((f, true)),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => opts(false).open(path).map(|f| (f, false)),
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn no_follow(o: &mut std::fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    o.custom_flags(libc::O_NOFOLLOW);
}

/// On Windows the link itself is opened, and then refused as not a plain file.
#[cfg(windows)]
fn no_follow(o: &mut std::fs::OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    o.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
}

#[cfg(not(any(unix, windows)))]
fn no_follow(_o: &mut std::fs::OpenOptions) {}

/// Whether an opened file is a link itself (Windows opens one as itself, see [`no_follow`]).
#[cfg(windows)]
fn is_link(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

/// What names one file on this machine whatever path reaches it, links followed: device and
/// file number on unix.
#[cfg(unix)]
pub fn file_id(path: &Path) -> io::Result<Option<(u64, u64)>> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).map(|m| Some((m.dev(), m.ino())))
}

#[cfg(unix)]
fn handle_id(file: &File) -> io::Result<Option<(u64, u64)>> {
    use std::os::unix::fs::MetadataExt;
    file.metadata().map(|m| Some((m.dev(), m.ino())))
}

/// What names one file on this machine whatever path reaches it, links followed: the volume
/// serial number and the file index on Windows (#459). The file is opened without read or
/// write access, which only lets its attributes be read.
#[cfg(windows)]
pub fn file_id(path: &Path) -> io::Result<Option<(u64, u64)>> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE};
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(path)?;
    handle_id(&file)
}

#[cfg(windows)]
fn handle_id(file: &File) -> io::Result<Option<(u64, u64)>> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
    // SAFETY: an all-zero BY_HANDLE_FILE_INFORMATION is a valid value (plain integers).
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open for the whole call and `info` is a valid place to write to.
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Some((
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    )))
}

/// Elsewhere hard links are not told apart: no identity, which is not an error.
#[cfg(not(any(unix, windows)))]
pub fn file_id(_path: &Path) -> io::Result<Option<(u64, u64)>> {
    Ok(None)
}

#[cfg(not(any(unix, windows)))]
fn handle_id(_file: &File) -> io::Result<Option<(u64, u64)>> {
    Ok(None)
}

/// Whether `path`, as the file system resolves it, is in one of `roots`.
pub fn inside(roots: &[PathBuf], path: &Path) -> bool {
    let path = normalize(&real(path));
    roots.iter().any(|r| path.starts_with(r))
}

/// Where the project's files really are: the project folder, and where each link in it
/// points (a `pages` folder linked to elsewhere is the project's too), followed through
/// further links. `.git` is left out. With them, why a folder or link could not be looked at,
/// if one could not: a root behind it would go unseen.
fn project_roots(project: &Path) -> (Vec<PathBuf>, Option<String>) {
    let mut roots = vec![real(project)];
    let mut unknown = None;
    // A folder or link that cannot be looked at may hide a root, so why is kept (a link to
    // nothing hides nothing).
    let mut note = |at: &Path, e: io::Error| {
        unknown.get_or_insert_with(|| format!("{}: {e}", at.display()));
    };
    let mut todo = vec![project.to_path_buf()];
    while let Some(d) = todo.pop() {
        let entries = match std::fs::read_dir(&d) {
            Ok(entries) => entries,
            Err(e) => {
                note(&d, e);
                continue;
            }
        };
        for e in entries {
            let e = match e {
                Ok(e) => e,
                Err(err) => {
                    note(&d, err);
                    continue;
                }
            };
            if e.file_name() == ".git" {
                continue;
            }
            let kind = match e.file_type() {
                Ok(kind) => kind,
                Err(err) => {
                    note(&e.path(), err);
                    continue;
                }
            };
            if kind.is_symlink() {
                let to = match e.path().canonicalize() {
                    Ok(to) => to,
                    Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                    Err(err) => {
                        note(&e.path(), err);
                        continue;
                    }
                };
                if roots.iter().any(|r| to.starts_with(r)) {
                    continue;
                }
                if to.is_dir() {
                    todo.push(to.clone());
                }
                roots.push(to);
            } else if kind.is_dir() {
                todo.push(e.path());
            }
        }
    }
    (roots, unknown)
}

/// `path` with links and `..` worked out as the file system does, as far as it exists (a link
/// to the project folder is the project folder, and `link/..` is the folder above where the
/// link points); the part that does not exist yet is kept by name.
pub fn real(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut at = path;
    loop {
        if let Ok(found) = at.canonicalize() {
            return rest.iter().rev().fold(found, |p, name| p.join(name));
        }
        match (at.parent(), at.components().next_back()) {
            (Some(up), Some(name)) => {
                rest.push(name.as_os_str().to_owned());
                at = up;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// `path` with `.` and `..` worked out by name (the folder need not exist).
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn project() -> io::Result<tempfile::TempDir> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir_all(dir.path().join("pages"))?;
        std::fs::write(dir.path().join("pages").join("p1.page.json"), "page one")?;
        std::fs::write(dir.path().join("project.toml"), "name = \"x\"")?;
        Ok(dir)
    }

    #[test]
    fn writes_a_new_file_and_empties_an_old_one() -> TestResult {
        let p = project()?;
        let out = tempfile::tempdir()?;
        let g = ProjectFiles::new(p.path());
        let file = out.path().join("list.csv");
        write_output(Some(&g), &file, "long first text")?;
        write_output(Some(&g), &file, "short")?;
        assert_eq!(std::fs::read_to_string(&file)?, "short");
        Ok(())
    }

    #[test]
    fn refuses_a_path_into_the_project() -> TestResult {
        let p = project()?;
        let g = ProjectFiles::new(p.path());
        let page = p.path().join("pages").join("p1.page.json");
        assert!(write_output(Some(&g), &page, "x").is_err());
        assert!(write_output(Some(&g), &p.path().join("new.csv"), "x").is_err());
        assert_eq!(std::fs::read_to_string(&page)?, "page one");
        assert!(!p.path().join("new.csv").exists());
        Ok(())
    }

    /// The app's look found nothing at the output path; a link to a page is swapped in after
    /// it, before the command line opens the file (#458).
    #[cfg(unix)]
    #[test]
    fn refuses_a_link_swapped_in_after_the_check() -> TestResult {
        let p = project()?;
        let out = tempfile::tempdir()?;
        let g = ProjectFiles::new(p.path());
        let page = p.path().join("pages").join("p1.page.json");
        let file = out.path().join("vars.csv");
        assert!(!g.holds(&file)?);
        std::os::unix::fs::symlink(&page, &file)?;
        let e = g.create(&file).err().ok_or("a link to a page was opened")?;
        assert!(e.to_string().contains("project"), "{e}");
        // Even without the look before the open, the open does not follow the link.
        let blind = ProjectFiles {
            roots: vec![],
            ..g.clone()
        };
        assert!(blind.create(&file).is_err());
        assert_eq!(std::fs::read_to_string(&page)?, "page one");
        Ok(())
    }

    /// A hard link to a page made after the look is the page under another name: refused
    /// before it is emptied.
    #[cfg(unix)]
    #[test]
    fn refuses_a_hard_link_to_a_page() -> TestResult {
        let p = project()?;
        let out = tempfile::tempdir()?;
        let g = ProjectFiles::new(p.path());
        let page = p.path().join("pages").join("p1.page.json");
        let file = out.path().join("vars.csv");
        std::fs::hard_link(&page, &file)?;
        // `create` looks first; the opened file's identity is checked as well.
        assert!(g.create(&file).is_err());
        let blind = ProjectFiles {
            roots: vec![],
            ..g.clone()
        };
        let opened = std::fs::OpenOptions::new().write(true).open(&file)?;
        assert!(blind.check_opened(&file, &opened).is_err());
        assert_eq!(std::fs::read_to_string(&page)?, "page one");
        Ok(())
    }

    /// A folder above the output file swapped for a link into the project after the look:
    /// the new file it made there is refused and removed.
    #[cfg(unix)]
    #[test]
    fn refuses_a_folder_swapped_for_a_link_into_the_project() -> TestResult {
        let p = project()?;
        let out = tempfile::tempdir()?;
        let g = ProjectFiles::new(p.path());
        let sub = out.path().join("exports");
        let file = sub.join("list.csv");
        assert!(!g.holds(&file)?);
        std::os::unix::fs::symlink(p.path().join("pages"), &sub)?;
        // Skip the look before the open, as if the swap came right after it.
        let (f, made) = open_no_follow(&file)?;
        assert!(made);
        assert!(g.check_opened(&file, &f).is_err());
        drop(f);
        std::fs::remove_file(&file)?;
        // Through `create` as a whole: refused and nothing left in the project.
        assert!(g.create(&file).is_err());
        assert!(!p.path().join("pages").join("list.csv").exists());
        Ok(())
    }

    #[test]
    fn without_a_guard_writes_as_usual() -> TestResult {
        let out = tempfile::tempdir()?;
        let file = out.path().join("a.txt");
        write_output(None, &file, "a")?;
        assert_eq!(std::fs::read_to_string(&file)?, "a");
        Ok(())
    }
}

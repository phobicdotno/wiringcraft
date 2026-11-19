//! Keeping written files off a project's files while someone else holds one of its pages in
//! a shared session (#282, #458).
//!
//! The app looks at a command's output paths before it starts the command line
//! ([`ProjectFiles::holds`]). That look cannot see a link swapped in afterwards, so the
//! command line writes each output file through [`ProjectFiles::write`] too: it never opens
//! what is already at the output name, but writes a new file next to it, checks it, and gives
//! it the name.

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

    /// Write `bytes` to the output file `path`, refusing to land on a project file (#458).
    ///
    /// Nothing already at `path` is opened or changed: the bytes go to a new file of their
    /// own next to it, which then takes the name. A link or a hard link at `path`, also one
    /// made after the look before, loses only its name there; the file behind it stays as it
    /// was. The new file is checked before it takes the name: it must not be in the project
    /// (a folder above swapped for a link into it shows here) and must still be the file that
    /// was made. A refused file is removed again.
    pub fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let refuse = |why: &str| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} {why}, and someone else has a page of the project open in the shared session",
                    path.display()
                ),
            )
        };
        match self.holds(path) {
            Ok(false) => {}
            Ok(true) => return Err(refuse("is in the project folder or is a project file")),
            Err(e) => {
                return Err(refuse(&format!(
                    "could not be told apart from the project's files ({e})"
                )));
            }
        }
        let (staged, file) = stage_next_to(path)?;
        let id = handle_id(&file).ok().flatten();
        // A refusal says why the file must not be written; any other failure (a full disk, a
        // rename across devices) is the plain I/O error with its path.
        let done = match self.check_staged(&staged, &file, id) {
            Err(why) => {
                // Closed first, so Windows can remove it.
                drop(file);
                Err(refuse(&why))
            }
            Ok(()) => {
                use std::io::Write;
                let mut file = file;
                file.write_all(bytes)
                    .and_then(|()| file.sync_all())
                    .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))
                    .and_then(|()| {
                        std::fs::rename(&staged, path)
                            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))
                    })
            }
        };
        if done.is_err() {
            // The name may lead elsewhere by now; only the file this call made goes.
            let at = real(&staged);
            if id.is_some() && file_id(&at).ok().flatten() == id {
                let _ = std::fs::remove_file(&at);
            }
        }
        done
    }

    /// Why the new file `file` (identity `id`), just made at `staged`, must not be written, if
    /// it must not.
    fn check_staged(&self, staged: &Path, file: &File, id: Option<(u64, u64)>) -> Result<(), String> {
        let meta = file.metadata().map_err(|e| format!("could not be looked at ({e})"))?;
        if !meta.is_file() {
            return Err("is not a plain file".into());
        }
        // Where the name leads now: a folder above swapped for a link into the project shows
        // there, and a name that no longer leads to the made file means it changed meanwhile.
        let at = real(staged);
        if inside(&self.roots, &at) {
            return Err("is in the project folder".into());
        }
        match (id, file_id(&at)) {
            (Some(id), Ok(Some(now))) if id != now => Err("changed while it was written".into()),
            (Some(_), Err(e)) => Err(format!("could not be looked at after it was made ({e})")),
            _ => Ok(()),
        }
    }
}

/// Write `bytes` to the output file `path`, through `guard` when there is one
/// ([`ProjectFiles::write`]), else as [`std::fs::write`] does.
pub fn write_output(guard: Option<&ProjectFiles>, path: &Path, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    match guard {
        None => std::fs::write(path, bytes),
        Some(g) => g.write(path, bytes.as_ref()),
    }
}

/// A new, empty file in the folder of `path`, with a name nothing had: a name already there,
/// a link too, is never opened.
fn stage_next_to(path: &Path) -> io::Result<(PathBuf, File)> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let folder = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut last = None;
    for _ in 0..16 {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let staged = folder.join(format!(".{name}.{}-{n}-{nanos}.wc-tmp", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&staged) {
            Ok(f) => return Ok((staged, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no free name for the output file")))
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

    /// No look before the write, as if the change came right after the app's look: a guard
    /// that knows nothing of the project's files.
    #[cfg(unix)]
    fn blind() -> ProjectFiles {
        ProjectFiles {
            roots: vec![],
            files: HashSet::new(),
            unknown: None,
        }
    }

    #[cfg(unix)]
    fn no_temp_left(folder: &Path) -> io::Result<bool> {
        Ok(std::fs::read_dir(folder)?
            .filter_map(Result::ok)
            .all(|e| !e.file_name().to_string_lossy().ends_with(".wc-tmp")))
    }

    /// The app's look found nothing at the output path; a link to a page is swapped in after
    /// it, before the command line writes (#458).
    #[cfg(unix)]
    #[test]
    fn a_link_swapped_in_after_the_check_is_not_written_through() -> TestResult {
        let p = project()?;
        let out = tempfile::tempdir()?;
        let g = ProjectFiles::new(p.path());
        let page = p.path().join("pages").join("p1.page.json");
        let file = out.path().join("vars.csv");
        assert!(!g.holds(&file)?);
        std::os::unix::fs::symlink(&page, &file)?;
        // With the look: refused.
        let e = g.write(&file, b"x").err().ok_or("a link to a page was written")?;
        assert!(e.to_string().contains("project"), "{e}");
        // Without it, the write still never goes through the link: the link loses its name.
        blind().write(&file, b"new list")?;
        assert_eq!(std::fs::read_to_string(&page)?, "page one");
        assert!(!std::fs::symlink_metadata(&file)?.file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&file)?, "new list");
        assert!(no_temp_left(out.path())?);
        Ok(())
    }

    /// A hard link to a page is the page under another name: refused by the look, and without
    /// the look (a page replaced after the project's files were listed) it loses only its name.
    #[cfg(unix)]
    #[test]
    fn a_hard_link_to_a_page_is_never_emptied() -> TestResult {
        let p = project()?;
        let out = tempfile::tempdir()?;
        let g = ProjectFiles::new(p.path());
        let page = p.path().join("pages").join("p1.page.json");
        let file = out.path().join("vars.csv");
        std::fs::hard_link(&page, &file)?;
        assert!(g.write(&file, b"x").is_err());
        blind().write(&file, b"new list")?;
        assert_eq!(std::fs::read_to_string(&page)?, "page one");
        assert_eq!(std::fs::read_to_string(&file)?, "new list");
        Ok(())
    }

    /// A folder above the output file swapped for a link into the project after the look:
    /// the file made there is refused and removed.
    #[cfg(unix)]
    #[test]
    fn a_folder_swapped_for_a_link_into_the_project_is_refused() -> TestResult {
        let p = project()?;
        let out = tempfile::tempdir()?;
        let g = ProjectFiles::new(p.path());
        let sub = out.path().join("exports");
        let file = sub.join("list.csv");
        assert!(!g.holds(&file)?);
        std::os::unix::fs::symlink(p.path().join("pages"), &sub)?;
        // Past the look, as if the swap came right after it.
        let (staged, f) = stage_next_to(&file)?;
        let id = handle_id(&f)?;
        assert!(g.check_staged(&staged, &f, id).is_err());
        drop(f);
        std::fs::remove_file(&staged)?;
        // Through `write` as a whole: refused and nothing left in the project.
        assert!(g.write(&file, b"x").is_err());
        assert!(!p.path().join("pages").join("list.csv").exists());
        assert!(no_temp_left(&p.path().join("pages"))?);
        Ok(())
    }

    /// A FIFO at the output name is not opened, so the write does not wait for a reader.
    #[cfg(unix)]
    #[test]
    fn a_fifo_at_the_output_name_is_not_opened() -> TestResult {
        let out = tempfile::tempdir()?;
        let file = out.path().join("vars.csv");
        let made = std::process::Command::new("mkfifo").arg(&file).status();
        if !made.is_ok_and(|s| s.success()) {
            return Ok(()); // no mkfifo here
        }
        blind().write(&file, b"list")?;
        assert_eq!(std::fs::read_to_string(&file)?, "list");
        Ok(())
    }

    /// A write or rename that fails for its own reasons is that I/O error, with the output
    /// path, not a shared-session refusal; the staging file is gone afterwards.
    #[test]
    fn a_failed_rename_is_a_plain_io_error() -> TestResult {
        let p = project()?;
        let out = tempfile::tempdir()?;
        let g = ProjectFiles::new(p.path());
        // A folder with something in it at the output name: no file can take that name.
        let file = out.path().join("list.csv");
        std::fs::create_dir(&file)?;
        std::fs::write(file.join("keep.txt"), "x")?;
        let e = g.write(&file, b"list").err().ok_or("a file took a folder's name")?;
        assert_ne!(e.kind(), io::ErrorKind::PermissionDenied, "{e}");
        assert!(e.to_string().contains(&file.display().to_string()), "{e}");
        assert!(!e.to_string().contains("shared session"), "{e}");
        let left = std::fs::read_dir(out.path())?
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().ends_with(".wc-tmp"));
        assert!(!left);
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

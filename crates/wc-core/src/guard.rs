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
    /// The identity of each project folder, so an output folder opened through a link is
    /// known to be the project's whatever name reached it (unix and Windows; empty elsewhere).
    pub dirs: HashSet<(u64, u64)>,
    /// Why a project file's identity could not be read, if one could not: a hard link to that
    /// file would go unseen, so no output file is taken as safe then.
    pub unknown: Option<String>,
}

impl ProjectFiles {
    /// The files of the project in `project`, `.git` left out.
    pub fn new(project: &Path) -> Self {
        let (roots, mut unknown) = project_roots(project);
        let mut files = HashSet::new();
        #[cfg_attr(not(any(unix, windows)), allow(unused_mut))]
        let mut dirs = HashSet::new();
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
                // Unix and Windows open the output folder and compare it by identity.
                #[cfg(any(unix, windows))]
                match file_id(&at) {
                    Ok(Some(id)) => {
                        dirs.insert(id);
                    }
                    Ok(None) => {}
                    Err(e) => note(&at, e),
                }
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
        Self {
            roots,
            files,
            dirs,
            unknown,
        }
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
    /// was. The output folder must not be the project's: on unix and Windows it is opened once
    /// and known by its identity, and the new file is made and renamed relative to it, so a
    /// folder above swapped for a link into the project after the checks changes nothing
    /// (#471, #476). Elsewhere the new file is checked by where its name leads before it takes
    /// the name. A refused file is removed again.
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
        self.put(path, bytes).map_err(|fail| match fail {
            Fail::Refused(why) => refuse(&why),
            // Any other failure (a full disk, a rename across devices) is the plain I/O error
            // with its path.
            Fail::Io(e) => io::Error::new(e.kind(), format!("{}: {e}", path.display())),
        })
    }

    /// Write `bytes` as a new file in the folder of `path` and give it the name (#471). The
    /// folder is opened once, checked by its identity, and the file is made, checked and
    /// renamed relative to that open folder: a folder above swapped for a link afterwards
    /// changes nothing, since no name is looked up again.
    #[cfg(unix)]
    fn put(&self, path: &Path, bytes: &[u8]) -> Result<(), Fail> {
        let (folder, name) = folder_and_name(path)?;
        #[cfg(test)]
        tests::hook("open", folder, std::ffi::OsStr::new(""));
        let dir = at::Dir::open(folder)?;
        #[cfg(test)]
        tests::hook("opened", folder, std::ffi::OsStr::new(""));
        // The folder that was opened, not whatever its name leads to now: by its identity,
        // and by where the open folder is (a project folder made after the look has no
        // identity on record).
        let opened = dir
            .path()
            .map_err(|e| Fail::Refused(format!("could not be told apart from the project's files ({e})")))?;
        if self.dirs.contains(&dir.id) || inside(&self.roots, &opened) {
            return Err(Fail::Refused("is in the project folder".into()));
        }
        let (staged, mut file) = dir.create_new(&name)?;
        let id = handle_id(&file).ok().flatten();
        let done = (|| {
            use std::io::Write;
            if !file.metadata()?.is_file() {
                return Err(Fail::Refused("is not a plain file".into()));
            }
            file.write_all(bytes)?;
            file.sync_all()?;
            #[cfg(test)]
            tests::hook("rename", folder, &staged);
            dir.rename(&staged, &name)?;
            Ok(())
        })();
        if done.is_err() && id.is_some() && dir.id_of(&staged) == id {
            // Only the file this call made goes: the name is looked up in the opened folder.
            let _ = dir.remove(&staged);
        }
        done
    }

    /// Write `bytes` as a new file in the folder of `path` and give it the name (#476), as on
    /// unix: the folder is opened once and checked by its identity, and the file is made
    /// (`NtCreateFile`) and renamed (`FileRenameInformation`) relative to that open folder, so
    /// a folder above swapped for a junction or link afterwards changes nothing.
    #[cfg(windows)]
    fn put(&self, path: &Path, bytes: &[u8]) -> Result<(), Fail> {
        let (folder, name) = folder_and_name(path)?;
        #[cfg(test)]
        tests::hook("open", folder, std::ffi::OsStr::new(""));
        let dir = win::Dir::open(folder)?;
        #[cfg(test)]
        tests::hook("opened", folder, std::ffi::OsStr::new(""));
        let opened = dir
            .path()
            .map_err(|e| Fail::Refused(format!("could not be told apart from the project's files ({e})")))?;
        if self.dirs.contains(&dir.id) || inside(&self.roots, &opened) {
            return Err(Fail::Refused("is in the project folder".into()));
        }
        let (staged, mut file) = dir.create_new(&name)?;
        let done = (|| {
            use std::io::Write;
            if !file.metadata()?.is_file() {
                return Err(Fail::Refused("is not a plain file".into()));
            }
            file.write_all(bytes)?;
            file.sync_all()?;
            #[cfg(test)]
            tests::hook("rename", folder, &staged);
            dir.rename(&file, &name)?;
            Ok(())
        })();
        // Only the file this call made goes: it is removed through its own handle, by no name
        // at all. A removal that fails too is named after the first failure.
        match done {
            Err(fail) => Err(match (fail, win::remove(&file)) {
                (fail, Ok(())) => fail,
                (Fail::Refused(why), Err(e)) => Fail::Refused(format!(
                    "{why} (its new file {} is left: {e})",
                    staged.to_string_lossy()
                )),
                (Fail::Io(first), Err(e)) => Fail::Io(io::Error::new(
                    first.kind(),
                    format!("{first} (the new file {} is left: {e})", staged.to_string_lossy()),
                )),
            }),
            Ok(()) => Ok(()),
        }
    }

    /// Write `bytes` as a new file next to `path`, checked by where its name leads, and give it
    /// the name.
    #[cfg(not(any(unix, windows)))]
    fn put(&self, path: &Path, bytes: &[u8]) -> Result<(), Fail> {
        let (staged, file) = stage_next_to(path)?;
        let id = handle_id(&file).ok().flatten();
        let done = match self.check_staged(&staged, &file, id) {
            Err(why) => {
                // Closed first, so Windows can remove it.
                drop(file);
                Err(Fail::Refused(why))
            }
            Ok(()) => {
                use std::io::Write;
                let mut file = file;
                file.write_all(bytes)
                    .and_then(|()| file.sync_all())
                    .and_then(|()| std::fs::rename(&staged, path))
                    .map_err(Fail::Io)
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
    #[cfg(not(any(unix, windows)))]
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

/// Why [`ProjectFiles::put`] did not write: refused, or an I/O failure of its own.
#[derive(Debug)]
enum Fail {
    Refused(String),
    Io(io::Error),
}

impl From<io::Error> for Fail {
    fn from(e: io::Error) -> Self {
        Fail::Io(e)
    }
}

/// The folder `path` is in (`.` for a bare name) and its file name.
#[cfg(any(unix, windows))]
fn folder_and_name(path: &Path) -> io::Result<(&Path, std::ffi::OsString)> {
    let folder = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the output path names no file"))?;
    Ok((folder, name.to_owned()))
}

/// A name for a staging file next to the file named `name`, unused so far by this process.
fn staging_name(name: &str) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!(".{name}.{}-{n}-{nanos}.wc-tmp", std::process::id())
}

/// Files made, looked at, renamed and removed relative to an opened folder (`openat` and
/// friends), so that no folder name is looked up again after the folder was checked.
#[cfg(unix)]
mod at {
    use std::ffi::{CString, OsStr, OsString};
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Path;

    /// An opened folder and its identity (device and inode).
    pub(super) struct Dir {
        file: File,
        pub(super) id: (u64, u64),
    }

    fn c_name(name: &OsStr) -> io::Result<CString> {
        CString::new(name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a name holds a NUL byte"))
    }

    impl Dir {
        /// Open the folder at `path` (links in it followed: what counts is the folder reached).
        /// On Linux it is opened as a path only (`O_PATH`), so a folder that may be written
        /// and searched but not listed still takes output files.
        pub(super) fn open(path: &Path) -> io::Result<Dir> {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            let flags = libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC;
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            let flags = libc::O_DIRECTORY | libc::O_CLOEXEC;
            let file = std::fs::OpenOptions::new().read(true).custom_flags(flags).open(path)?;
            let m = file.metadata()?;
            if !m.is_dir() {
                return Err(io::Error::new(io::ErrorKind::NotADirectory, "not a folder"));
            }
            Ok(Dir {
                id: (m.dev(), m.ino()),
                file,
            })
        }

        /// A new, empty file in this folder with an unused name made from `name`: a name
        /// already there, a link too, is never opened.
        pub(super) fn create_new(&self, name: &OsStr) -> io::Result<(OsString, File)> {
            let mut last = None;
            for _ in 0..16 {
                let staged = OsString::from(super::staging_name(&name.to_string_lossy()));
                let c = c_name(&staged)?;
                // SAFETY: the folder's descriptor is open and `c` is a NUL-terminated name.
                let fd = unsafe {
                    libc::openat(
                        self.file.as_raw_fd(),
                        c.as_ptr(),
                        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                        0o666 as libc::c_uint,
                    )
                };
                if fd >= 0 {
                    // SAFETY: `fd` was just opened and is owned by nothing else.
                    return Ok((staged, unsafe { File::from_raw_fd(fd) }));
                }
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::AlreadyExists {
                    return Err(e);
                }
                last = Some(e);
            }
            Err(last.unwrap_or_else(|| io::Error::other("no free name for the output file")))
        }

        /// Where this open folder is now, links worked out, found from the open folder itself
        /// (not by looking its old name up again).
        #[cfg(any(target_os = "linux", target_os = "android"))]
        pub(super) fn path(&self) -> io::Result<std::path::PathBuf> {
            std::fs::read_link(format!("/proc/self/fd/{}", self.file.as_raw_fd()))
        }

        /// Where this open folder is now, links worked out, found from the open folder itself
        /// (not by looking its old name up again).
        #[cfg(target_vendor = "apple")]
        pub(super) fn path(&self) -> io::Result<std::path::PathBuf> {
            use std::os::unix::ffi::OsStringExt;
            let mut buf = vec![0u8; libc::PATH_MAX as usize];
            // SAFETY: the descriptor is open and `buf` holds PATH_MAX bytes, as F_GETPATH needs.
            if unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) } == -1 {
                return Err(io::Error::last_os_error());
            }
            let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            buf.truncate(len);
            Ok(std::path::PathBuf::from(OsString::from_vec(buf)))
        }

        /// Elsewhere an open folder cannot say where it is: the write is refused.
        #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
        pub(super) fn path(&self) -> io::Result<std::path::PathBuf> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "where an open folder is cannot be read here",
            ))
        }

        /// Give the file named `from` in this folder the name `to` in this folder.
        pub(super) fn rename(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
            let (from, to) = (c_name(from)?, c_name(to)?);
            let fd = self.file.as_raw_fd();
            // SAFETY: the folder's descriptor is open and both names are NUL-terminated.
            if unsafe { libc::renameat(fd, from.as_ptr(), fd, to.as_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        /// The identity of the entry `name` in this folder, a link not followed.
        pub(super) fn id_of(&self, name: &OsStr) -> Option<(u64, u64)> {
            let c = c_name(name).ok()?;
            // SAFETY: an all-zero `stat` is a valid value (plain integers).
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            // SAFETY: the folder's descriptor is open, `c` is NUL-terminated and `st` is a
            // valid place to write to.
            let r = unsafe { libc::fstatat(self.file.as_raw_fd(), c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) };
            #[allow(clippy::unnecessary_cast)] // the field types differ between systems
            (r == 0).then_some((st.st_dev as u64, st.st_ino as u64))
        }

        /// Remove the file `name` from this folder.
        pub(super) fn remove(&self, name: &OsStr) -> io::Result<()> {
            let c = c_name(name)?;
            // SAFETY: the folder's descriptor is open and `c` is NUL-terminated.
            if unsafe { libc::unlinkat(self.file.as_raw_fd(), c.as_ptr(), 0) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }
}

/// Files made, renamed and removed relative to an opened folder on Windows (#476): the file is
/// made with `NtCreateFile` and renamed with `FileRenameInformation`, both with the open folder
/// as their root, so that no folder name is looked up again after the folder was checked.
#[cfg(windows)]
mod win {
    use std::ffi::{OsStr, OsString};
    use std::fs::File;
    use std::io;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::{Path, PathBuf};
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION,
        FILE_SYNCHRONOUS_IO_NONALERT, FileRenameInformation, NtCreateFile, NtSetInformationFile,
    };
    use windows_sys::Win32::Foundation::{
        HANDLE, NTSTATUS, OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError, UNICODE_STRING,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_ATTRIBUTE_NORMAL, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_WRITE,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, FileDispositionInfo,
        GetFinalPathNameByHandleW, SYNCHRONIZE, SetFileInformationByHandle,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    /// An opened folder and its identity (volume serial and file index).
    pub(super) struct Dir {
        file: File,
        pub(super) id: (u64, u64),
    }

    /// The I/O error for an `NTSTATUS` that is not a success.
    fn nt_error(status: NTSTATUS) -> io::Error {
        // SAFETY: a plain conversion of a status code, with no pointers.
        let code = unsafe { RtlNtStatusToDosError(status) };
        io::Error::from_raw_os_error(code as i32)
    }

    /// `name` as UTF-16, refused when it is too long for a `UNICODE_STRING` or names more
    /// than one entry.
    fn wide(name: &OsStr) -> io::Result<Vec<u16>> {
        let w: Vec<u16> = name.encode_wide().collect();
        if w.is_empty()
            || w.len() > usize::from(u16::MAX / 2)
            || w.iter()
                .any(|&c| c == u16::from(b'\\') || c == u16::from(b'/') || c == 0)
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a plain file name"));
        }
        Ok(w)
    }

    impl Dir {
        /// Open the folder at `path` (junctions and links in it followed: what counts is the
        /// folder reached), with access only to pass through it and read its attributes.
        pub(super) fn open(path: &Path) -> io::Result<Dir> {
            let file = std::fs::OpenOptions::new()
                .access_mode(FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(path)?;
            if !file.metadata()?.is_dir() {
                return Err(io::Error::new(io::ErrorKind::NotADirectory, "not a folder"));
            }
            let id =
                super::handle_id(&file)?.ok_or_else(|| io::Error::other("the folder's identity could not be read"))?;
            Ok(Dir { file, id })
        }

        /// Where this open folder is now, links worked out, found from the open folder itself
        /// (not by looking its old name up again).
        pub(super) fn path(&self) -> io::Result<PathBuf> {
            let handle = self.file.as_raw_handle() as HANDLE;
            let mut buf = vec![0u16; 512];
            loop {
                let cap = u32::try_from(buf.len()).map_err(io::Error::other)?;
                // SAFETY: the handle is open and `buf` holds `cap` UTF-16 units.
                let n = unsafe { GetFinalPathNameByHandleW(handle, buf.as_mut_ptr(), cap, 0) } as usize;
                if n == 0 {
                    return Err(io::Error::last_os_error());
                }
                if n < buf.len() {
                    buf.truncate(n);
                    return Ok(PathBuf::from(OsString::from_wide(&buf)));
                }
                // Too small: `n` is the size needed, with the closing NUL.
                buf.resize(n + 1, 0);
            }
        }

        /// A new, empty file in this folder with an unused name made from `name`: a name
        /// already there, a link or junction too, is never opened. The file may be written,
        /// renamed and removed through its handle.
        pub(super) fn create_new(&self, name: &OsStr) -> io::Result<(OsString, File)> {
            let mut last = None;
            for _ in 0..16 {
                let staged = OsString::from(super::staging_name(&name.to_string_lossy()));
                let w = wide(&staged)?;
                let bytes = (w.len() * 2) as u16;
                let object = UNICODE_STRING {
                    Length: bytes,
                    MaximumLength: bytes,
                    Buffer: w.as_ptr().cast_mut(),
                };
                let attributes = OBJECT_ATTRIBUTES {
                    Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
                    RootDirectory: self.file.as_raw_handle() as HANDLE,
                    ObjectName: &object,
                    Attributes: OBJ_CASE_INSENSITIVE,
                    SecurityDescriptor: std::ptr::null(),
                    SecurityQualityOfService: std::ptr::null(),
                };
                let mut handle: HANDLE = std::ptr::null_mut();
                // SAFETY: an all-zero IO_STATUS_BLOCK is a valid value.
                let mut io_status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
                // SAFETY: every pointer is to a live value of the right type for the whole call:
                // `object` and `w` outlive it, and the folder's handle is open.
                let status = unsafe {
                    NtCreateFile(
                        &mut handle,
                        FILE_GENERIC_WRITE | DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                        &attributes,
                        &mut io_status,
                        std::ptr::null(),
                        FILE_ATTRIBUTE_NORMAL,
                        FILE_SHARE_READ,
                        FILE_CREATE,
                        FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
                        std::ptr::null(),
                        0,
                    )
                };
                if status >= 0 {
                    // SAFETY: `handle` was just opened (for synchronous I/O, as `File` needs) and
                    // is owned by nothing else.
                    return Ok((staged, unsafe { File::from_raw_handle(handle as _) }));
                }
                let e = nt_error(status);
                if e.kind() != io::ErrorKind::AlreadyExists {
                    return Err(e);
                }
                last = Some(e);
            }
            Err(last.unwrap_or_else(|| io::Error::other("no free name for the output file")))
        }

        /// Give the open file `file`, made in this folder, the name `to` in this folder,
        /// replacing what has that name.
        pub(super) fn rename(&self, file: &File, to: &OsStr) -> io::Result<()> {
            let w = wide(to)?;
            let head = std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName);
            let size = head + w.len() * 2;
            // u64 words, so the structure (which holds a handle) is aligned.
            let mut buf = vec![0u64; size.div_ceil(8)];
            let info = buf.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
            // SAFETY: `buf` is zeroed, aligned for the structure and holds `size` bytes: the
            // structure's fixed part and the name right after it.
            unsafe {
                (*info).Anonymous.ReplaceIfExists = true;
                (*info).RootDirectory = self.file.as_raw_handle() as HANDLE;
                (*info).FileNameLength = (w.len() * 2) as u32;
                std::ptr::copy_nonoverlapping(
                    w.as_ptr(),
                    std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                    w.len(),
                );
            }
            // SAFETY: an all-zero IO_STATUS_BLOCK is a valid value.
            let mut io_status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
            // SAFETY: both handles are open, `info` points to `size` initialised bytes and
            // `io_status` is a valid place to write to.
            let status = unsafe {
                NtSetInformationFile(
                    file.as_raw_handle() as HANDLE,
                    &mut io_status,
                    info.cast(),
                    size as u32,
                    FileRenameInformation,
                )
            };
            if status < 0 {
                return Err(nt_error(status));
            }
            Ok(())
        }
    }

    /// Remove the open file `file` (made with delete access) through its handle: whatever its
    /// name or folder by now, only this file goes.
    pub(super) fn remove(file: &File) -> io::Result<()> {
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: the handle is open and `info` is a valid FILE_DISPOSITION_INFO of the size given.
        let ok = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle() as HANDLE,
                FileDispositionInfo,
                std::ptr::addr_of!(info).cast(),
                std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// A new, empty file in the folder of `path`, with a name nothing had: a name already there,
/// a link too, is never opened.
#[cfg(not(any(unix, windows)))]
fn stage_next_to(path: &Path) -> io::Result<(PathBuf, File)> {
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
        let staged = folder.join(staging_name(&name));
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
/// write access, which only lets its attributes be read; a folder can be opened too (#476).
#[cfg(windows)]
pub fn file_id(path: &Path) -> io::Result<Option<(u64, u64)>> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
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
            dirs: HashSet::new(),
            unknown: None,
        }
    }

    #[cfg(any(unix, windows))]
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
        // Past the look, as if the swap came right after it: the opened folder is the
        // project's by its identity, refused and nothing left in the project.
        let e = g.write(&file, b"x").err().ok_or("written into the project")?;
        assert!(e.to_string().contains("project folder"), "{e}");
        assert!(!p.path().join("pages").join("list.csv").exists());
        assert!(no_temp_left(&p.path().join("pages"))?);
        Ok(())
    }

    /// What to do in the next write at one of its steps, on this thread.
    #[cfg(any(unix, windows))]
    type Swap = Box<dyn FnOnce(&Path, &std::ffi::OsStr) -> io::Result<()>>;

    #[cfg(any(unix, windows))]
    thread_local! {
        static SWAPS: std::cell::RefCell<Vec<(&'static str, Swap)>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Run `swap` when the next write on this thread reaches `step`: `open` (before the output
    /// folder is opened), `opened` (right after) or `rename` (after every check, before the
    /// rename; it is given the staging file's name).
    #[cfg(any(unix, windows))]
    fn on(step: &'static str, swap: impl FnOnce(&Path, &std::ffi::OsStr) -> io::Result<()> + 'static) {
        SWAPS.with(|s| s.borrow_mut().push((step, Box::new(swap))));
    }

    /// Called by [`ProjectFiles::put`] at each `step`.
    #[cfg(any(unix, windows))]
    pub(super) fn hook(step: &str, folder: &Path, staged: &std::ffi::OsStr) {
        let swap = SWAPS.with(|s| {
            let mut s = s.borrow_mut();
            let at = s.iter().position(|(k, _)| *k == step)?;
            Some(s.remove(at).1)
        });
        if let Some(swap) = swap {
            let done = swap(folder, staged);
            assert!(done.is_ok(), "the swap at {step} failed: {done:?}");
        }
    }

    /// The output folder's parent pointed into a project folder made after the look while the
    /// output folder is opened, and put back right after: the write is refused by where the
    /// opened folder is, and the project's new file is unchanged (#477 review).
    #[cfg(unix)]
    #[test]
    fn a_new_project_folder_opened_through_a_swap_is_refused() -> TestResult {
        let p = project()?;
        let out = tempfile::tempdir()?;
        let g = ProjectFiles::new(p.path());
        let new = p.path().join("new");
        std::fs::create_dir(&new)?;
        std::fs::write(new.join("page.json"), "new page")?;
        let parent = out.path().join("parent");
        std::fs::create_dir_all(parent.join("sub"))?;
        let file = parent.join("sub").join("page.json");
        assert!(!g.holds(&file)?);
        let aside = out.path().join("parent-aside");
        let (parent2, aside2, project2) = (parent.clone(), aside.clone(), p.path().to_path_buf());
        on("open", move |_, _| {
            std::fs::rename(&parent2, &aside2)?;
            std::os::unix::fs::symlink(&project2, &parent2)?;
            std::fs::rename(project2.join("new"), project2.join("sub"))
        });
        let (parent3, aside3, project3) = (parent.clone(), aside.clone(), p.path().to_path_buf());
        on("opened", move |_, _| {
            std::fs::remove_file(&parent3)?;
            std::fs::rename(&aside3, &parent3)?;
            std::fs::rename(project3.join("sub"), project3.join("new"))
        });
        let e = g
            .write(&file, b"exported")
            .err()
            .ok_or("written into a project folder")?;
        assert!(e.to_string().contains("project folder"), "{e}");
        assert_eq!(std::fs::read_to_string(new.join("page.json"))?, "new page");
        assert!(no_temp_left(&new)?);
        assert!(!file.exists());
        Ok(())
    }

    /// The output folder swapped for a link into the project after every check, with an entry
    /// of the staging file's name planted there: the rename still happens in the folder that
    /// was opened and checked, and the project's files stay as they were (#471).
    #[cfg(unix)]
    #[test]
    fn a_folder_swapped_after_the_checks_does_not_redirect_the_rename() -> TestResult {
        let p = project()?;
        let out = tempfile::tempdir()?;
        let g = ProjectFiles::new(p.path());
        let sub = out.path().join("exports");
        std::fs::create_dir(&sub)?;
        let file = sub.join("p1.page.json");
        let pages = p.path().join("pages");
        let moved = out.path().join("exports-old");
        let (pages2, moved2, sub2) = (pages.clone(), moved.clone(), sub.clone());
        on("rename", move |_folder: &Path, staged: &std::ffi::OsStr| {
            std::fs::rename(&sub2, &moved2)?;
            std::os::unix::fs::symlink(&pages2, &sub2)?;
            std::fs::write(pages2.join(staged), "planted")?;
            Ok(())
        });
        g.write(&file, b"exported")?;
        assert_eq!(std::fs::read_to_string(pages.join("p1.page.json"))?, "page one");
        assert_eq!(std::fs::read_to_string(moved.join("p1.page.json"))?, "exported");
        assert!(no_temp_left(&moved)?);
        Ok(())
    }

    /// A junction at `link` to the folder `to` (no special rights needed, unlike a symbolic
    /// link).
    #[cfg(windows)]
    fn junction(link: &Path, to: &Path) -> io::Result<()> {
        use std::os::windows::process::CommandExt;
        // The paths reach cmd through variables, quoted: cmd expands a variable once and does
        // not parse its value again, so `&` and `%` in a path are taken literally (a path
        // cannot hold a `"`).
        let done = std::process::Command::new("cmd")
            .arg("/C")
            .raw_arg(r#"mklink /J "%WC_JUNCTION_LINK%" "%WC_JUNCTION_TO%""#)
            .env("WC_JUNCTION_LINK", link)
            .env("WC_JUNCTION_TO", to)
            .output()?;
        if done.status.success() {
            Ok(())
        } else {
            Err(io::Error::other(String::from_utf8_lossy(&done.stderr).into_owned()))
        }
    }

    /// The junction helper takes paths with `&` and `%` in them literally.
    #[cfg(windows)]
    #[test]
    fn a_junction_is_made_for_paths_cmd_would_parse() -> TestResult {
        let out = tempfile::tempdir()?;
        let to = out.path().join("a&b %TEMP% c");
        std::fs::create_dir(&to)?;
        std::fs::write(to.join("x.txt"), "x")?;
        let link = out.path().join("l&k %TEMP%");
        junction(&link, &to)?;
        assert_eq!(std::fs::read_to_string(link.join("x.txt"))?, "x");
        std::fs::remove_dir(&link)?;
        Ok(())
    }

    /// The output folder's parent swapped for a junction into the project after the look,
    /// right before the output folder is opened: the opened folder is the project's, so the
    /// write is refused and the project's files are unchanged (#476).
    #[cfg(windows)]
    #[test]
    fn a_folder_opened_through_a_junction_into_the_project_is_refused() -> TestResult {
        let p = project()?;
        let out = tempfile::tempdir()?;
        let g = ProjectFiles::new(p.path());
        let parent = out.path().join("parent");
        std::fs::create_dir_all(parent.join("pages"))?;
        let file = parent.join("pages").join("p1.page.json");
        assert!(!g.holds(&file)?);
        let (parent2, aside, project2) = (parent.clone(), out.path().join("parent-aside"), p.path().to_path_buf());
        on("open", move |_, _| {
            std::fs::rename(&parent2, &aside)?;
            junction(&parent2, &project2)
        });
        let e = g
            .write(&file, b"exported")
            .err()
            .ok_or("written into a project folder")?;
        assert!(e.to_string().contains("project folder"), "{e}");
        let pages = p.path().join("pages");
        assert_eq!(std::fs::read_to_string(pages.join("p1.page.json"))?, "page one");
        assert!(no_temp_left(&pages)?);
        // The junction goes before the temporary folder does.
        std::fs::remove_dir(&parent)?;
        Ok(())
    }

    /// The output folder swapped for a junction into the project after every check, with an
    /// entry of the staging file's name planted there: the rename still happens in the folder
    /// that was opened and checked, and the project's files stay as they were (#476). Windows
    /// may refuse to move a folder that holds an open file; then the folder stays and takes
    /// the output, and the planted file is still left alone.
    #[cfg(windows)]
    #[test]
    fn a_folder_swapped_for_a_junction_after_the_checks_does_not_redirect_the_rename() -> TestResult {
        let p = project()?;
        let out = tempfile::tempdir()?;
        let g = ProjectFiles::new(p.path());
        let sub = out.path().join("exports");
        std::fs::create_dir(&sub)?;
        let file = sub.join("p1.page.json");
        let pages = p.path().join("pages");
        let moved = out.path().join("exports-old");
        let swapped = std::rc::Rc::new(std::cell::Cell::new(false));
        let (pages2, moved2, sub2, swapped2) = (pages.clone(), moved.clone(), sub.clone(), swapped.clone());
        let planted = std::rc::Rc::new(std::cell::RefCell::new(std::ffi::OsString::new()));
        let planted2 = planted.clone();
        on("rename", move |_folder: &Path, staged: &std::ffi::OsStr| {
            match std::fs::rename(&sub2, &moved2) {
                Ok(()) => {
                    junction(&sub2, &pages2)?;
                    swapped2.set(true);
                }
                // Only the refusal to move a folder that holds an open file (access denied or
                // a sharing violation) lets the test go on without the swap.
                Err(e) if matches!(e.raw_os_error(), Some(5 | 32)) => {}
                Err(e) => return Err(e),
            }
            std::fs::write(pages2.join(staged), "planted")?;
            *planted2.borrow_mut() = staged.to_owned();
            Ok(())
        });
        g.write(&file, b"exported")?;
        assert_eq!(std::fs::read_to_string(pages.join("p1.page.json"))?, "page one");
        let staged = planted.borrow().clone();
        assert_eq!(std::fs::read_to_string(pages.join(&staged))?, "planted");
        let landed = if swapped.get() { &moved } else { &sub };
        assert_eq!(std::fs::read_to_string(landed.join("p1.page.json"))?, "exported");
        assert!(no_temp_left(landed)?);
        if swapped.get() {
            // The junction goes before the temporary folder does, so the project is not emptied.
            std::fs::remove_dir(&sub)?;
        }
        Ok(())
    }

    /// A folder that may be written and searched but not listed (mode 0300) still takes the
    /// output file. Root reads any folder, so the check only means something for others.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn a_folder_that_cannot_be_listed_still_takes_the_output() -> TestResult {
        use std::os::unix::fs::PermissionsExt;
        let p = project()?;
        let out = tempfile::tempdir()?;
        let g = ProjectFiles::new(p.path());
        let sub = out.path().join("drop");
        std::fs::create_dir(&sub)?;
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o300))?;
        let file = sub.join("list.csv");
        let done = g.write(&file, b"list");
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o700))?;
        done?;
        assert_eq!(std::fs::read_to_string(&file)?, "list");
        assert!(no_temp_left(&sub)?);
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
        // Windows itself answers "Access is denied" here, so only unix can tell by the kind.
        #[cfg(unix)]
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

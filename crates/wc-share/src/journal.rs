//! Recording every change of a shared session: one commit each, with its author.

use std::path::PathBuf;

use wc_core::{Part, Project, Symbol};
use wc_vcs::Git;

use crate::{PeerId, Picture, Result};

/// One applied change, as recorded in the history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub by: PeerId,
    /// `Name <email>` of the participant who made it.
    pub author: String,
    /// The page it changed, when it is about one page.
    pub page: Option<String>,
    /// The commit message.
    pub message: String,
}

/// Where the host records changes. Called once per applied change, right after it was applied.
pub trait Journal {
    fn record(&mut self, project: &Project, change: &Change) -> Result<()>;

    /// Called before `record` for a change that adds or updates symbols and parts
    /// ([`crate::ProjectEdit::AddItems`], [`crate::ProjectEdit::UpdateItems`], and an undo that
    /// locks an earlier version again): store their files with the project, so the commit holds
    /// them next to their lock entries.
    fn vendor(&mut self, _symbols: &[Symbol], _parts: &[Part]) -> Result<()> {
        Ok(())
    }

    /// Store a picture file a participant added ([`crate::ProjectEdit::AddPicture`]) with the
    /// project, so the commit of the change that names it holds it. A file already there with
    /// the same content is fine; one with other content must not be overwritten (an undo may
    /// still show it) and is an error.
    fn picture(&mut self, _picture: &Picture) -> Result<()> {
        Ok(())
    }

    /// Called when `record` failed: put back what it stored, so it holds `project` again.
    fn restore(&mut self, _project: &Project) -> Result<()> {
        Ok(())
    }
}

/// Keeps changes in memory; for tests and for sessions without a project folder.
#[derive(Debug, Default)]
pub struct MemoryJournal {
    pub changes: Vec<Change>,
    /// Ids of the symbols and parts handed to [`Journal::vendor`].
    pub vendored: Vec<String>,
    /// Paths of the pictures handed to [`Journal::picture`].
    pub pictures: Vec<String>,
}

impl Journal for MemoryJournal {
    fn record(&mut self, _project: &Project, change: &Change) -> Result<()> {
        self.changes.push(change.clone());
        Ok(())
    }

    fn vendor(&mut self, symbols: &[Symbol], parts: &[Part]) -> Result<()> {
        self.vendored.extend(
            symbols
                .iter()
                .map(|s| s.id.clone())
                .chain(parts.iter().map(|p| p.id.clone())),
        );
        Ok(())
    }

    fn picture(&mut self, picture: &Picture) -> Result<()> {
        self.pictures.push(picture.file.clone());
        Ok(())
    }
}

/// Saves the project folder and commits each change as its author.
#[derive(Debug)]
pub struct GitJournal {
    dir: PathBuf,
    git: Git,
}

impl GitJournal {
    /// The folder must hold the project; it is made a repository if it is not one yet.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        let git = Git::new(&dir);
        git.init()?;
        Ok(Self { dir, git })
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    pub fn git(&self) -> &Git {
        &self.git
    }
}

impl Journal for GitJournal {
    fn record(&mut self, project: &Project, change: &Change) -> Result<()> {
        wc_core::store::save(&self.dir, project)?;
        self.git.commit_as(&change.message, &change.author)?;
        Ok(())
    }

    /// Write the vendored files into the project folder (`vendor/symbols`, `vendor/parts`).
    fn vendor(&mut self, symbols: &[Symbol], parts: &[Part]) -> Result<()> {
        for s in symbols {
            wc_core::store::write_vendored_symbol(&self.dir, s)?;
        }
        for p in parts {
            wc_core::store::write_vendored_part(&self.dir, p)?;
        }
        Ok(())
    }

    /// Write the picture into the project folder, never outside it and never over a file with
    /// other content.
    fn picture(&mut self, picture: &Picture) -> Result<()> {
        write_picture(&self.dir, picture)
    }

    /// Save the project as it was before the change, so the folder does not keep a change
    /// whose commit failed.
    fn restore(&mut self, project: &Project) -> Result<()> {
        wc_core::store::save(&self.dir, project)?;
        Ok(())
    }
}

/// Write `picture` into the project folder `dir`, making its folder. A file there with the same
/// content is left as it is; one with other content is never written over (an undo may still
/// show it, or the person picked it here) and is an error. The file always lands inside the
/// project, even when a folder on the way is a link.
pub fn write_picture(dir: &std::path::Path, picture: &Picture) -> Result<()> {
    use std::io::{Error, ErrorKind};
    if let Some(why) = picture.problem() {
        return Err(Error::new(ErrorKind::InvalidInput, why).into());
    }
    let outside = || -> crate::Error {
        Error::new(
            ErrorKind::InvalidInput,
            format!("{} leads out of the project folder", picture.file),
        )
        .into()
    };
    let path = dir.join(&picture.file);
    let folder = path.parent().unwrap_or(dir);
    let root = dir.canonicalize()?;
    // The nearest folder that exists already must be inside the project before any is made,
    // so a link on the way cannot make folders elsewhere.
    let mut anchor = folder;
    while !anchor.exists() {
        match anchor.parent() {
            Some(parent) => anchor = parent,
            None => break,
        }
    }
    if !anchor.canonicalize()?.starts_with(&root) {
        return Err(outside());
    }
    std::fs::create_dir_all(folder)?;
    if !folder.canonicalize()?.starts_with(&root) {
        return Err(outside());
    }
    let existing = || -> Result<()> {
        match std::fs::read(&path) {
            Ok(have) if have == picture.data => Ok(()),
            Ok(_) => Err(Error::new(
                ErrorKind::AlreadyExists,
                format!("the project already has another picture named {}", picture.file),
            )
            .into()),
            Err(e) => Err(e.into()),
        }
    };
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(outside());
    }
    match std::fs::symlink_metadata(&path) {
        Ok(_) => return existing(),
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    // The whole file is written beside it first and then put in place without replacing
    // anything, so a failed write never leaves a cut-off picture under the name.
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("picture");
    let part = stage(folder, name, &picture.data)?;
    let published = publish(&part, &path);
    let _ = std::fs::remove_file(&part);
    match published {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => existing(),
        Err(e) => Err(e.into()),
    }
}

/// Write `data` to a part file in `folder` that this call makes new, under a name no other
/// file has (any file already there is left alone), and return its path.
fn stage(folder: &std::path::Path, name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    for _ in 0..16 {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        // Short and apart from the picture's own name, so a long name that fits the folder
        // still has a part file name that does.
        let part = folder.join(format!(".wc-{}-{nanos}-{n}.part", std::process::id()));
        match write_new(&part, data) {
            Ok(()) => return Ok(part),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("no free name for a part file of {name}"),
    ))
}

/// Write `data` to a new file at `path`, removing it again when that fails.
fn write_new(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    let written = file.write_all(data).and_then(|()| file.sync_all());
    drop(file);
    if written.is_err() {
        let _ = std::fs::remove_file(path);
    }
    written
}

/// Put the finished file `part` at `path`, failing with `AlreadyExists` when something is
/// there. A hard link does that in one step.
fn publish(part: &std::path::Path, path: &std::path::Path) -> std::io::Result<()> {
    publish_with(part, path, |a, b| std::fs::hard_link(a, b))
}

/// [`publish`] with the hard link made by `link`. Where a folder cannot hold hard links
/// (FAT, some network drives), the whole part file is renamed into place once nothing is
/// there, so no cut-off picture ever shows under the name. That check and the rename are
/// two steps: a file made in between by someone else would be replaced, which only another
/// writer of the same picture name at the same moment could cause.
fn publish_with(
    part: &std::path::Path,
    path: &std::path::Path,
    link: impl Fn(&std::path::Path, &std::path::Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::io::ErrorKind;
    match link(part, path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(e),
        Err(_) => match std::fs::symlink_metadata(path) {
            Ok(_) => Err(std::io::Error::new(
                ErrorKind::AlreadyExists,
                "the picture is there already",
            )),
            Err(e) if e.kind() == ErrorKind::NotFound => std::fs::rename(part, path),
            Err(e) => Err(e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

    fn no_links(_: &std::path::Path, _: &std::path::Path) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no hard links here",
        ))
    }

    #[test]
    fn without_hard_links_a_picture_is_renamed_into_place_whole() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let (part, path) = (tmp.path().join(".logo.png.part"), tmp.path().join("logo.png"));
        std::fs::write(&part, b"whole picture")?;
        publish_with(&part, &path, no_links)?;
        assert_eq!(std::fs::read(&path)?, b"whole picture");
        assert!(!part.exists(), "the part file became the picture");
        Ok(())
    }

    #[test]
    fn without_hard_links_a_file_already_there_is_kept() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let (part, path) = (tmp.path().join(".logo.png.part"), tmp.path().join("logo.png"));
        std::fs::write(&part, b"new")?;
        std::fs::write(&path, b"mine")?;
        let r = publish_with(&part, &path, no_links);
        assert!(r.is_err_and(|e| e.kind() == std::io::ErrorKind::AlreadyExists));
        assert_eq!(std::fs::read(&path)?, b"mine");
        Ok(())
    }
}

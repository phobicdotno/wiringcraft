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

    /// Called before `record` for a change that adds symbols and parts
    /// ([`crate::ProjectEdit::AddItems`]): store their files with the project, so the commit
    /// holds them next to their lock entries.
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
    match std::fs::read(&path) {
        Ok(have) if have == picture.data => return Ok(()),
        Ok(_) => {
            return Err(Error::new(
                ErrorKind::AlreadyExists,
                format!("the project already has another picture named {}", picture.file),
            )
            .into());
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(outside());
    }
    std::fs::write(&path, &picture.data)?;
    Ok(())
}

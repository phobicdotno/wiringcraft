//! Recording every change of a shared session: one commit each, with its author.

use std::path::PathBuf;

use wc_core::Project;
use wc_vcs::Git;

use crate::{PeerId, Result};

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
}

/// Keeps changes in memory; for tests and for sessions without a project folder.
#[derive(Debug, Default)]
pub struct MemoryJournal {
    pub changes: Vec<Change>,
}

impl Journal for MemoryJournal {
    fn record(&mut self, _project: &Project, change: &Change) -> Result<()> {
        self.changes.push(change.clone());
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
}

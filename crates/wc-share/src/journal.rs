//! Recording every change of a shared session: one commit each, with its author.

use std::path::PathBuf;

use wc_core::{Part, Project, Symbol};
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

    /// Called before `record` for a change that adds symbols and parts
    /// ([`crate::ProjectEdit::AddItems`]): store their files with the project, so the commit
    /// holds them next to their lock entries.
    fn vendor(&mut self, _symbols: &[Symbol], _parts: &[Part]) -> Result<()> {
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

    /// Save the project as it was before the change, so the folder does not keep a change
    /// whose commit failed.
    fn restore(&mut self, project: &Project) -> Result<()> {
        wc_core::store::save(&self.dir, project)?;
        Ok(())
    }
}

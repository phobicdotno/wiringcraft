use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Error, Result};

/// Lines written to `.gitattributes` in a new project: line endings fixed so Windows and Linux
/// users produce identical files.
const GITATTRIBUTES: &str = "\
# WiringCraft project files are text with LF line endings everywhere.
*.json text eol=lf
*.toml text eol=lf
library.lock text eol=lf
*.pdf binary
";

const GITIGNORE: &str = "\
# WiringCraft temporary and generated files
*.tmp
/export/
";

/// A project folder's git repository.
#[derive(Debug, Clone)]
pub struct Git {
    pub dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// Nothing to do: already contains the other branch.
    UpToDate,
    /// Merged and committed.
    Merged,
    /// Stopped with conflicts in these files; resolve, then [`Git::commit`], or [`Git::abort_merge`].
    Conflicts(Vec<String>),
}

impl Git {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub(crate) fn run(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(Error::Spawn)?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let mut message = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            if message.is_empty() {
                message = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            }
            Err(Error::Git {
                args: args.join(" "),
                message,
            })
        }
    }

    pub fn is_repo(&self) -> bool {
        self.dir.join(".git").exists()
    }

    /// Make the project folder a repository (no-op when it is one) and record a first version.
    pub fn init(&self) -> Result<()> {
        if !self.is_repo() {
            self.run(&["init", "-q", "-b", "main"])?;
        }
        for (name, content) in [(".gitattributes", GITATTRIBUTES), (".gitignore", GITIGNORE)] {
            let p: &Path = &self.dir.join(name);
            if !p.exists() {
                wc_core::store::write_if_changed(p, content)?;
            }
        }
        if self.head().is_none() {
            self.commit("New project")?;
        }
        Ok(())
    }

    /// The current commit, if any.
    pub fn head(&self) -> Option<String> {
        self.run(&["rev-parse", "--verify", "-q", "HEAD"])
            .ok()
            .map(|s| s.trim().to_owned())
    }

    /// Record every change in the folder. Returns the new commit, or `None` when nothing changed.
    pub fn commit(&self, message: &str) -> Result<Option<String>> {
        self.run(&["add", "-A"])?;
        let staged = self.run(&["status", "--porcelain"])?;
        let merging = self.dir.join(".git").join("MERGE_HEAD").exists();
        if staged.trim().is_empty() && !merging && self.head().is_some() {
            return Ok(None);
        }
        let mut args = vec!["commit", "-q", "--allow-empty", "-m", message];
        // Commit as the configured user; fall back to a neutral identity so a fresh machine works.
        let has_identity = self.run(&["config", "user.email"]).is_ok();
        if !has_identity {
            args = vec![
                "-c",
                "user.name=WiringCraft",
                "-c",
                "user.email=wiringcraft@localhost",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                message,
            ];
        }
        self.run(&args)?;
        Ok(self.head())
    }

    pub fn current_branch(&self) -> Option<String> {
        self.run(&["symbolic-ref", "--short", "-q", "HEAD"])
            .ok()
            .map(|s| s.trim().to_owned())
    }

    pub fn branches(&self) -> Result<Vec<String>> {
        Ok(self
            .run(&["branch", "--format=%(refname:short)"])?
            .lines()
            .map(str::to_owned)
            .collect())
    }

    /// Create a branch at the current version and switch to it.
    pub fn create_branch(&self, name: &str) -> Result<()> {
        self.run(&["check-ref-format", "--branch", name])?;
        self.run(&["switch", "-q", "-c", name])?;
        Ok(())
    }

    /// Switch branches. Refuses when there are unrecorded changes, so nothing is lost.
    pub fn switch(&self, name: &str) -> Result<()> {
        if !self.run(&["status", "--porcelain"])?.trim().is_empty() {
            return Err(Error::Git {
                args: format!("switch {name}"),
                message: "record or discard your changes first (commit)".into(),
            });
        }
        self.run(&["switch", "-q", name])?;
        Ok(())
    }

    /// Merge another branch into the current one.
    pub fn merge(&self, other: &str) -> Result<MergeOutcome> {
        let before = self.head();
        let msg = format!("Merge {other}");
        let mut args = vec!["merge", "--no-ff", "-m", msg.as_str(), other];
        if self.run(&["config", "user.email"]).is_err() {
            args.splice(
                0..0,
                ["-c", "user.name=WiringCraft", "-c", "user.email=wiringcraft@localhost"],
            );
        }
        match self.run(&args) {
            Ok(_) => Ok(if self.head() == before {
                MergeOutcome::UpToDate
            } else {
                MergeOutcome::Merged
            }),
            Err(e) => {
                let conflicts: Vec<String> = self
                    .run(&["diff", "--name-only", "--diff-filter=U"])
                    .unwrap_or_default()
                    .lines()
                    .map(str::to_owned)
                    .collect();
                if conflicts.is_empty() {
                    Err(e)
                } else {
                    Ok(MergeOutcome::Conflicts(conflicts))
                }
            }
        }
    }

    pub fn abort_merge(&self) -> Result<()> {
        self.run(&["merge", "--abort"])?;
        Ok(())
    }

    /// A file's content at a revision, or `None` when it did not exist there.
    pub fn show(&self, rev: &str, path: &str) -> Option<String> {
        self.run(&["show", &format!("{rev}:{path}")]).ok()
    }
}

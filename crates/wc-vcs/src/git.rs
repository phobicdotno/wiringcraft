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

    /// [`Git::run`] that stops git and returns [`Error::Cancelled`] once `cancel` is set,
    /// checked a few times a second while it runs (a clone from a slow server, say).
    pub(crate) fn run_until(&self, args: &[&str], cancel: &std::sync::atomic::AtomicBool) -> Result<String> {
        use std::io::Read;
        use std::process::Stdio;
        use std::sync::atomic::Ordering;
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let mut child = Command::new("git")
            .args(args)
            .current_dir(&self.dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(Error::Spawn)?;
        // Read both streams as they come, so a full pipe never holds git up.
        // Bytes, read to the end even when they are not UTF-8, as [`Git::run`] does.
        let read = |s: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || -> std::io::Result<String> {
                let mut bytes = Vec::new();
                if let Some(mut s) = s {
                    s.read_to_end(&mut bytes)?;
                }
                Ok(String::from_utf8_lossy(&bytes).into_owned())
            })
        };
        let out = read(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
        let err = read(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
        let status = loop {
            if let Some(status) = child.try_wait().map_err(Error::Spawn)? {
                break status;
            }
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Cancelled);
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        };
        let joined = |t: std::thread::JoinHandle<std::io::Result<String>>| {
            t.join()
                .unwrap_or_else(|_| Err(std::io::Error::other("the reader stopped")))
                .map_err(Error::Spawn)
        };
        let out = joined(out)?;
        let err = joined(err)?;
        if status.success() {
            Ok(out)
        } else {
            let mut message = err.trim().to_owned();
            if message.is_empty() {
                message = out.trim().to_owned();
            }
            Err(Error::Git {
                args: args.join(" "),
                message,
            })
        }
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
        self.commit_inner(message, None, None)
    }

    /// Like [`Git::commit`], but recorded as written by `author` (`Name <email>`), for example
    /// another participant of a shared session. The committer stays the local user.
    pub fn commit_as(&self, message: &str, author: &str) -> Result<Option<String>> {
        self.commit_inner(message, Some(author), None)
    }

    /// Like [`Git::commit_as`], but records only the changes under `paths` (files or folders,
    /// relative to the project folder; ones that do not exist are fine). Every other change in
    /// the folder, staged or not, stays as it is for a later commit. Returns `None` when nothing
    /// under `paths` changed.
    ///
    /// While a merge is being finished, a commit cannot leave anything out, so it records every
    /// change like [`Git::commit_as`].
    pub fn commit_paths_as(&self, message: &str, author: &str, paths: &[&str]) -> Result<Option<String>> {
        if self.git_path("MERGE_HEAD")?.exists() {
            return self.commit_as(message, author);
        }
        let changed = self.changed_under(paths)?;
        if changed.is_empty() && self.head().is_some() {
            return Ok(None);
        }
        self.commit_inner(message, Some(author), Some(&changed))
    }

    /// The paths with a change (staged, unstaged or untracked) under `paths`, as git names them.
    /// Renames are not paired up, so a moved file is its removal and its addition.
    fn changed_under(&self, paths: &[&str]) -> Result<Vec<String>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut args = vec![
            "--literal-pathspecs",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
            "--",
        ];
        args.extend(paths);
        let out = self.run(&args)?;
        let mut changed: Vec<String> = out
            .split('\0')
            .filter_map(|entry| entry.get(3..))
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect();
        changed.sort();
        changed.dedup();
        Ok(changed)
    }

    /// Commit every change (`only` is `None`), or only the changes of the paths in `only`, which
    /// must each have one ([`Git::changed_under`]).
    fn commit_inner(&self, message: &str, author: Option<&str>, only: Option<&[String]>) -> Result<Option<String>> {
        // What was staged before, so a failed commit can put the index back as it was. This can
        // fail (an unmerged index while resolving a merge has no tree); the commit still goes on,
        // and the reason is kept for the error if the commit then fails.
        let index_before = self.run(&["write-tree"]).map(|t| t.trim().to_owned());
        match only {
            None => {
                self.run(&["add", "-A"])?;
                let staged = self.run(&["status", "--porcelain"])?;
                let merging = self.git_path("MERGE_HEAD")?.exists();
                if staged.trim().is_empty() && !merging && self.head().is_some() {
                    return Ok(None);
                }
            }
            Some(paths) if !paths.is_empty() => {
                let mut add = vec!["--literal-pathspecs", "add", "-A", "--"];
                add.extend(paths.iter().map(String::as_str));
                self.run(&add)?;
            }
            Some(_) => {}
        }
        let author_arg = author.map(|a| format!("--author={a}"));
        let mut args: Vec<&str> = Vec::new();
        // Commit as the configured user; fall back to a neutral identity so a fresh machine works.
        if self.run(&["config", "user.email"]).is_err() {
            args.extend(["-c", "user.name=WiringCraft", "-c", "user.email=wiringcraft@localhost"]);
        }
        if only.is_some() {
            args.push("--literal-pathspecs");
        }
        args.extend(["commit", "-q", "--allow-empty", "-m", message]);
        if let Some(a) = &author_arg {
            args.push(a);
        }
        // Only these paths' changes, whatever else is staged; no paths (a first commit with
        // nothing under them) records an empty commit.
        if let Some(paths) = only.filter(|p| !p.is_empty()) {
            args.push("--only");
            args.push("--");
            args.extend(paths.iter().map(String::as_str));
        }
        if let Err(e) = self.run(&args) {
            let undo = match &index_before {
                Ok(tree) => self.run(&["read-tree", tree]).err(),
                Err(snapshot) => Some(Error::Git {
                    args: "write-tree".into(),
                    message: format!("no copy of the staged files was taken: {snapshot}"),
                }),
            };
            return Err(match undo {
                None => e,
                Some(undo) => Error::Git {
                    args: "commit".into(),
                    message: format!("{e}; the staged files could not be put back: {undo}"),
                },
            });
        }
        Ok(self.head())
    }

    /// Undo one earlier commit with a new commit that reverses it (the history keeps both).
    /// Returns the new commit. Fails, leaving nothing half done, when later commits changed the
    /// same lines.
    pub fn revert(&self, commit: &str) -> Result<Option<String>> {
        let mut args: Vec<&str> = Vec::new();
        if self.run(&["config", "user.email"]).is_err() {
            args.extend(["-c", "user.name=WiringCraft", "-c", "user.email=wiringcraft@localhost"]);
        }
        let mut busy = None;
        for f in ["REVERT_HEAD", "CHERRY_PICK_HEAD", "MERGE_HEAD", "sequencer"] {
            if self.git_path(f)?.exists() {
                busy = Some(f);
                break;
            }
        }
        if let Some(f) = busy {
            return Err(Error::Git {
                args: format!("revert {commit}"),
                message: format!("another revert, cherry-pick or merge is in progress ({f}); finish or abort it first"),
            });
        }
        args.extend(["revert", "--no-edit", commit]);
        if let Err(e) = self.run(&args) {
            // Only this call's revert can be in progress here (checked above), so aborting it
            // cannot throw away earlier work.
            // When the lookup itself fails, try the abort anyway (no other operation was running)
            // and keep the lookup's reason with the error.
            let (mid_revert, lookup) = match self.git_path("REVERT_HEAD") {
                Ok(p) => (p.exists(), None),
                Err(l) => (true, Some(l)),
            };
            let abort = if mid_revert {
                self.run(&["revert", "--abort"]).err()
            } else {
                None
            };
            if abort.is_none() && lookup.is_none() {
                return Err(e);
            }
            let mut message = e.to_string();
            if let Some(l) = lookup {
                message.push_str(&format!("; could not tell whether the revert was left half done: {l}"));
            }
            if let Some(a) = abort {
                message.push_str(&format!(
                    "; undoing the failed revert also failed, so the folder is mid-revert: {a}"
                ));
            }
            return Err(Error::Git {
                args: format!("revert {commit}"),
                message,
            });
        }
        Ok(self.head())
    }

    /// Where git keeps an operation file such as `REVERT_HEAD`. In a linked worktree `.git` is a
    /// file pointing elsewhere, so git is asked.
    fn git_path(&self, name: &str) -> Result<PathBuf> {
        let out = self.run(&["rev-parse", "--git-path", name])?;
        Ok(self.dir.join(out.trim()))
    }

    /// The author (`Name <email>`) and subject of a commit.
    pub fn author_and_subject(&self, rev: &str) -> Result<(String, String)> {
        let out = self.run(&["log", "-1", "--format=%an <%ae>%n%s", rev])?;
        let mut lines = out.lines();
        let author = lines.next().unwrap_or_default().to_owned();
        let subject = lines.next().unwrap_or_default().to_owned();
        Ok((author, subject))
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

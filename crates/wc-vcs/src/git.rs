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
/.wiringcraft.lock
";

/// The variable every git command this crate runs gets, naming the project folder it runs
/// for. A wiringcraft command that git starts in turn (from a hook) on the same project sees
/// that the command which started git already holds the project, and does not wait for it.
pub const PARENT_PROJECT_ENV: &str = "WIRINGCRAFT_GIT_PARENT_PROJECT";

/// The file in a project folder that wiringcraft commands lock so they take turns on the
/// project. Never part of the project's history: new projects' `.gitignore` lists it, and
/// [`Git::exclude_locally`] keeps it out of repositories whose `.gitignore` does not.
pub const LOCK_FILE: &str = ".wiringcraft.lock";

/// Put before a branch name or revision the user gave, so git takes one that starts with `-`
/// (`--detach`, `--output=file`) as a name it then refuses, never as an option.
const END_OF_OPTIONS: &str = "--end-of-options";

/// Where [`Git::exclude_locally`] writes the new exclude list before renaming it over `at`:
/// beside it, named after this process so two processes never write the same file.
pub(crate) fn exclude_scratch(at: &Path) -> PathBuf {
    let mut name = at.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".{}.new", std::process::id()));
    at.with_file_name(name)
}

/// A path from git's `-z` output as text: as it is when it is UTF-8. A name that is not (a
/// Unix file name may hold any bytes) comes back in git's own quoted form, `"`, the name with
/// `"` and `\` escaped and each byte that is not UTF-8 as a `\` and three octal digits, then
/// `"` (`"pages/\377.page.json"`). Two names that differ in such a byte so stay two entries,
/// where reading them lossily would give both the same text. A UTF-8 name that starts with `"`
/// is quoted the same way, so no name is listed as another name's quoted form. The quoted form
/// names no file: such an entry can be listed, not opened.
pub(crate) fn path_text(bytes: &[u8]) -> String {
    use std::fmt::Write;
    // A UTF-8 name that itself starts with `"` is quoted too, so it never reads like the quoted
    // form of another name (`"a\377.txt"` the file vs. `a\xff.txt` quoted).
    if let Ok(text) = std::str::from_utf8(bytes)
        && !text.starts_with('"')
    {
        return text.to_owned();
    }
    let mut out = String::from("\"");
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c => out.push(c),
            }
        }
        for b in chunk.invalid() {
            let _ = write!(out, "\\{b:03o}");
        }
    }
    out.push('"');
    out
}

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
            .env(PARENT_PROJECT_ENV, self.full_dir())
            // This project's repository, whatever a caller (a git hook) points git at.
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
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
        self.run_bytes(args)
            .map(|out| String::from_utf8_lossy(&out).into_owned())
    }

    /// [`Git::run`] for `-z` output that lists paths: its NUL-ended fields, each read with
    /// [`path_text`], so a name that is not UTF-8 never comes back as another name's text.
    pub(crate) fn run_paths(&self, args: &[&str]) -> Result<Vec<String>> {
        Ok(self
            .run_bytes(args)?
            .split(|&b| b == 0)
            .filter(|field| !field.is_empty())
            .map(path_text)
            .collect())
    }

    /// [`Git::run`] with git's output as the bytes it printed.
    fn run_bytes(&self, args: &[&str]) -> Result<Vec<u8>> {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env(PARENT_PROJECT_ENV, self.full_dir())
            // This project's repository, whatever a caller (a git hook) points git at.
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .map_err(Error::Spawn)?;
        if out.status.success() {
            Ok(out.stdout)
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

    /// The project folder as an absolute path, for [`PARENT_PROJECT_ENV`]: git runs hooks from
    /// the work tree, where a relative path would point somewhere else. As given when it
    /// cannot be resolved (git then fails on the folder itself and says why).
    fn full_dir(&self) -> PathBuf {
        std::fs::canonicalize(&self.dir).unwrap_or_else(|_| self.dir.clone())
    }

    /// Keep `name` (a file at the top of the work tree) out of this repository's history
    /// without changing a tracked file: listed in git's own exclude list (`info/exclude`, as
    /// `git rev-parse --git-path` names it, so a linked work tree uses the shared one). Nothing
    /// changes when it is listed already; the list's other lines are kept byte for byte.
    pub fn exclude_locally(&self, name: &str) -> Result<()> {
        let at = self.run(&["rev-parse", "--git-path", "info/exclude"])?;
        let at = self.dir.join(at.trim_end_matches(['\r', '\n']));
        let io = |source: std::io::Error| Error::Io {
            path: at.display().to_string(),
            source,
        };
        let mut bytes = match std::fs::read(&at) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(io(e)),
        };
        let line = format!("/{name}");
        if bytes.split(|b| *b == b'\n').any(|l| l.trim_ascii() == line.as_bytes()) {
            return Ok(());
        }
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        if let Some(parent) = at.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        // Written whole to a file beside it, then renamed over it: a failed write leaves the
        // list as it was, and a command reading it meanwhile never sees it cut short.
        let new = exclude_scratch(&at);
        if let Err(err) = std::fs::write(&new, bytes) {
            let _ = std::fs::remove_file(&new);
            return Err(io(err));
        }
        std::fs::rename(&new, &at).map_err(|err| {
            let _ = std::fs::remove_file(&new);
            io(err)
        })
    }

    pub fn is_repo(&self) -> bool {
        self.dir.join(".git").exists()
    }

    /// Make the project folder a repository (no-op when it is one) and record a first version.
    pub fn init(&self) -> Result<()> {
        if !self.is_repo() {
            self.run(&["init", "-q", "-b", "main"])?;
        }
        // Before the first commit, so a lock file there already is not recorded in it.
        self.exclude_locally(LOCK_FILE)?;
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
        if changed.commit.is_empty() {
            return Ok(None);
        }
        self.commit_inner(message, Some(author), Some(&changed))
    }

    /// The paths with a change (staged, unstaged or untracked) under `paths`, as git names them.
    /// Renames are not paired up, so a moved file is its removal and its addition.
    fn changed_under(&self, paths: &[&str]) -> Result<Changed> {
        if paths.is_empty() {
            return Ok(Changed::default());
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
        let mut changed = Changed::default();
        for entry in out.split('\0') {
            let (Some(code), Some(path)) = (entry.get(..2), entry.get(3..)) else {
                continue;
            };
            if path.is_empty() {
                continue;
            }
            changed.commit.push(path.to_owned());
            // A removal staged already (`git rm`) is in neither the index nor the folder, so
            // `git add` would not find it; the commit takes it as it is staged.
            if code != "D " {
                changed.add.push(path.to_owned());
            }
        }
        changed.commit.sort();
        changed.commit.dedup();
        changed.add.sort();
        changed.add.dedup();
        Ok(changed)
    }

    /// Commit every change (`only` is `None`), or only the changes in `only`
    /// ([`Git::changed_under`], with at least one path to commit).
    fn commit_inner(&self, message: &str, author: Option<&str>, only: Option<&Changed>) -> Result<Option<String>> {
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
            Some(changed) => {
                if !changed.add.is_empty() {
                    let mut add = vec!["--literal-pathspecs", "add", "-A", "--"];
                    add.extend(changed.add.iter().map(String::as_str));
                    self.run(&add)?;
                }
            }
        }
        // What staging left to record under the paths, against the last commit: a file added
        // and removed again is gone, and one changed back to its committed content has nothing
        // left. Nothing left is no commit.
        let selected = match only {
            None => None,
            Some(changed) => {
                let head = self.head();
                let mut diff = vec![
                    "--literal-pathspecs",
                    "diff",
                    "--cached",
                    "--name-only",
                    "--no-renames",
                    "-z",
                ];
                if let Some(h) = &head {
                    diff.push(h);
                }
                diff.push("--");
                diff.extend(changed.commit.iter().map(String::as_str));
                let out = self.run(&diff)?;
                let paths: Vec<String> = out.split('\0').filter(|p| !p.is_empty()).map(str::to_owned).collect();
                if paths.is_empty() {
                    return Ok(None);
                }
                Some(paths)
            }
        };
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
        // Only these paths' changes, whatever else is staged.
        if let Some(paths) = &selected {
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
        // Resolved to one commit first: `git revert` hands what follows `--end-of-options` to
        // its revision walk, which still reads `--all` there as every branch.
        let resolved = self.run(&[
            "rev-parse",
            "--verify",
            "--quiet",
            END_OF_OPTIONS,
            &format!("{commit}^{{commit}}"),
        ]);
        let commit = match resolved {
            Ok(id) if !id.trim().is_empty() => id.trim().to_owned(),
            // `--quiet`: an unknown version fails with nothing said. Any other failure (git
            // could not run, a broken repository) keeps its own reason.
            Ok(_) => return Err(no_such_version(commit)),
            Err(Error::Git { message, .. }) if message.is_empty() => return Err(no_such_version(commit)),
            Err(e) => return Err(e),
        };
        args.extend(["revert", "--no-edit", END_OF_OPTIONS, &commit]);
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
        let out = self.run(&["log", "-1", "--format=%an <%ae>%n%s", END_OF_OPTIONS, rev])?;
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

    /// Create a branch at the current version and switch to it. Changes not yet recorded come
    /// along unchanged, so there is no need (and, with such changes, no way) to [`Git::switch`]
    /// to it afterwards.
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
        self.run(&["switch", "-q", END_OF_OPTIONS, name])?;
        Ok(())
    }

    /// Merge another branch into the current one. Refuses while an earlier merge is still
    /// being finished, so that merge's conflicts are never reported as this one's. Refuses too
    /// when there are unrecorded changes, like [`Git::switch`]: committing a merge that stopped
    /// on conflicts records every change, so they would end up inside the merge, and aborting
    /// it cannot always bring them back.
    pub fn merge(&self, other: &str) -> Result<MergeOutcome> {
        if self.git_path("MERGE_HEAD")?.exists() {
            return Err(Error::Git {
                args: format!("merge {other}"),
                message: "another merge is in progress; resolve its conflicts and commit, or abort it first".into(),
            });
        }
        if !self.run(&["status", "--porcelain"])?.trim().is_empty() {
            return Err(Error::Git {
                args: format!("merge {other}"),
                message: "record or discard your changes first (commit)".into(),
            });
        }
        let before = self.head();
        let msg = format!("Merge {other}");
        let mut args = vec!["merge", "--no-ff", "-m", msg.as_str(), END_OF_OPTIONS, other];
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
                // `-z`: paths as they are (no quoting of accents, tabs or quotes), NUL-ended.
                let conflicts = self
                    .run_paths(&["diff", "--name-only", "--diff-filter=U", "-z"])
                    .unwrap_or_default();
                if conflicts.is_empty() {
                    Err(e)
                } else {
                    Ok(MergeOutcome::Conflicts(conflicts))
                }
            }
        }
    }

    /// Undo a merge that stopped on conflicts, bringing the files back to how they were before
    /// it. Says so plainly when no merge is in progress.
    pub fn abort_merge(&self) -> Result<()> {
        if !self.git_path("MERGE_HEAD")?.exists() {
            return Err(Error::Git {
                args: "merge --abort".into(),
                message: "no merge in progress".into(),
            });
        }
        self.run(&["merge", "--abort"])?;
        Ok(())
    }

    /// A file's content at a revision, or `None` when it did not exist there.
    pub fn show(&self, rev: &str, path: &str) -> Option<String> {
        self.run(&["show", END_OF_OPTIONS, &format!("{rev}:{path}")]).ok()
    }
}

/// [`Git::revert`]'s refusal of a name that is no version.
fn no_such_version(commit: &str) -> Error {
    Error::Git {
        args: format!("revert {commit}"),
        message: format!("no such version: {commit}"),
    }
}

/// What [`Git::commit_paths_as`] records: every path with a change, and the ones of them
/// `git add` can take (all but removals staged already).
#[derive(Debug, Default)]
struct Changed {
    commit: Vec<String>,
    add: Vec<String>,
}

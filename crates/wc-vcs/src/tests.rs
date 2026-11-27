use super::*;
use wc_core::geom::Point;
use wc_core::{DeviceTag, Page, PlacedSymbol, store};

fn place(page: &mut Page, id: &str, tag: &str, x: f64) -> std::result::Result<(), wc_core::Error> {
    page.symbols.push(PlacedSymbol {
        id: id.into(),
        symbol: "coil".into(),
        at: Point::new(x, 50.0),
        rotation: Default::default(),
        mirror: false,
        tag: Some(DeviceTag::parse(tag)?),
    });
    Ok(())
}

#[test]
fn branch_edit_merge_and_map() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    let mut p = store::create(dir, "Demo")?;
    let git = Git::new(dir);
    git.init()?;
    assert_eq!(git.current_branch().as_deref(), Some("main"));
    assert!(git.commit("nothing")?.is_none(), "no empty commits");

    git.create_branch("motor-circuit")?;
    let mut page = Page::new("p0002", "2", "Motor");
    place(&mut page, "s1", "-K1", 40.0)?;
    p.pages.push(page);
    store::save(dir, &p)?;
    let ch = changes(&git, "HEAD", None)?;
    assert!(ch.iter().any(|c| matches!(c, Change::PageAdded { .. })), "{ch:?}");
    git.commit("Add motor page")?;

    git.switch("main")?;
    let mut p_main = store::load(dir)?;
    if let Some(cover) = p_main.page_mut("p0001") {
        cover.title = "Cover sheet".into();
    }
    store::save(dir, &p_main)?;
    git.commit("Rename cover")?;

    // Different pages: merges cleanly.
    assert_eq!(git.merge("motor-circuit")?, MergeOutcome::Merged);
    let merged = store::load(dir)?;
    assert_eq!(merged.pages.len(), 2);
    assert_eq!(merged.page("p0001").map(|p| p.title.as_str()), Some("Cover sheet"));

    let rows = history(&git, 50)?;
    assert_eq!(rows.len(), 4);
    assert!(rows[0].commit.head && rows[0].commit.parents.len() == 2);
    assert!(rows.iter().any(|r| r.lane == 1), "the branch gets its own line");
    assert!(subway_svg(&rows).contains("motor-circuit"));

    // Semantic change list between two commits.
    let ch = changes(&git, "HEAD~1", Some("HEAD"))?;
    assert!(ch.iter().any(|c| c.to_string().contains("page 2 added")), "{ch:?}");

    // One page's own history: only the commits that touched it, newest first.
    let mut p = store::load(dir)?;
    if let Some(page) = p.page_mut("p0002") {
        place(page, "s2", "-K2", 80.0)?;
    }
    store::save(dir, &p)?;
    git.commit("Add K2")?;
    let h = page_history(&git, "p0002", 10)?;
    let subjects: Vec<&str> = h.iter().map(|v| v.subject.as_str()).collect();
    assert_eq!(subjects, ["Add K2", "Add motor page"]);
    assert!(
        h[0].changes.iter().any(|c| c.to_string().contains("-K2")),
        "{:?}",
        h[0].changes
    );
    assert!(
        matches!(h[1].changes.first(), Some(Change::PageAdded { .. })),
        "{:?}",
        h[1].changes
    );
    assert_eq!(page_history(&git, "p0001", 10)?.len(), 2);
    assert!(page_history(&git, "../x", 10).is_err());
    Ok(())
}

#[test]
fn symbol_level_changes() -> std::result::Result<(), wc_core::Error> {
    let mut a = Page::new("p", "3", "X");
    place(&mut a, "s1", "-K1", 10.0)?;
    place(&mut a, "s2", "-K2", 20.0)?;
    let mut b = a.clone();
    b.symbols[0].at.x = 15.0;
    b.symbols[1].tag = Some(DeviceTag::parse("-K5")?);
    b.symbols.push(b.symbols[0].clone());
    b.symbols[2].id = "s3".into();
    let ch: Vec<String> = changes::page_changes(Some(&a), Some(&b))
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(ch.contains(&"page 3: coil -K1 moved".to_string()), "{ch:?}");
    assert!(ch.contains(&"page 3: s2 tag -K2 -> -K5".to_string()), "{ch:?}");
    assert!(ch.iter().any(|c| c.ends_with("added")), "{ch:?}");
    Ok(())
}

#[test]
fn a_symbol_swapped_for_another_is_named() -> std::result::Result<(), wc_core::Error> {
    let mut a = Page::new("p", "3", "X");
    place(&mut a, "s1", "-K1", 10.0)?;
    let shown = |b: &Page| -> Vec<String> {
        changes::page_changes(Some(&a), Some(b))
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    // Alone: named, not the catch-all "page changed".
    let mut b = a.clone();
    b.symbols[0].symbol = "contact.nc".into();
    assert_eq!(shown(&b), ["page 3: -K1 symbol coil -> contact.nc"]);
    // Together with a move of the same symbol: still listed.
    b.symbols[0].at.x = 30.0;
    assert_eq!(
        shown(&b),
        ["page 3: -K1 symbol coil -> contact.nc", "page 3: contact.nc -K1 moved"]
    );
    Ok(())
}

#[test]
fn page_setting_changes_are_reported() -> std::result::Result<(), wc_core::Error> {
    let mut a = Page::new("p", "3", "X");
    place(&mut a, "s1", "-K1", 10.0)?;
    assert!(
        changes::page_changes(Some(&a), Some(&a.clone())).is_empty(),
        "same page, no change"
    );
    let shown = |b: &Page| -> Vec<String> {
        changes::page_changes(Some(&a), Some(b))
            .iter()
            .map(ToString::to_string)
            .collect()
    };

    let mut b = a.clone();
    b.number = "4".into();
    assert_eq!(shown(&b), ["page 3 renumbered -> 4"]);

    let mut b = a.clone();
    b.title_block.insert("revision".into(), "B".into());
    assert_eq!(shown(&b), ["page 3: title block changed"]);

    let mut b = a.clone();
    b.size = wc_core::project::PageSize::A4;
    assert_eq!(shown(&b), ["page 3: sheet size changed"]);

    let mut b = a.clone();
    b.cover = true;
    b.revision_table = Some(Point::new(10.0, 10.0));
    assert_eq!(
        shown(&b),
        ["page 3: revision table changed", "page 3: cover setting changed"]
    );

    let mut b = a.clone();
    b.clouds.push(wc_core::revision::RevisionCloud {
        min: Point::new(0.0, 0.0),
        max: Point::new(20.0, 20.0),
        revision: "B".into(),
    });
    assert_eq!(shown(&b), ["page 3: revision clouds changed"]);

    let mut b = a.clone();
    b.underlay = Some(wc_core::project::Underlay {
        file: "underlays/p.jpg".into(),
        min: Point::new(0.0, 0.0),
        max: Point::new(420.0, 297.0),
    });
    assert_eq!(shown(&b), ["page 3: underlay changed"]);

    let mut b = a.clone();
    b.overview = true;
    assert_eq!(shown(&b), ["page 3: overview setting changed"]);

    // A difference no check names (here the stored id) is still reported.
    let mut b = a.clone();
    b.id = "q".into();
    assert_eq!(shown(&b), ["page 3: page changed"]);
    Ok(())
}

#[test]
fn revert_refuses_while_another_operation_is_in_progress() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    store::create(dir, "Demo")?;
    let git = Git::new(dir);
    git.init()?;
    let head = git.head().unwrap_or_default();
    std::fs::write(dir.join(".git").join("CHERRY_PICK_HEAD"), &head).map_err(|e| wc_core::Error::io("write", e))?;
    let err = git.revert(&head).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(err.contains("in progress"), "{err}");
    assert!(
        dir.join(".git").join("CHERRY_PICK_HEAD").exists(),
        "the other operation is left alone"
    );
    Ok(())
}

#[test]
fn revert_keeps_the_reason_of_a_lookup_that_fails_loudly() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    store::create(dir, "Demo")?;
    let git = Git::new(dir);
    git.init()?;
    let head = git.head().unwrap_or_default();
    // A corrupt object under a made-up id: the lookup of that version fails and git says why,
    // while the checks before it (identity, operation files) still pass.
    let id = "1".repeat(head.len());
    let objects = dir.join(".git").join("objects").join(&id[..2]);
    std::fs::create_dir_all(&objects).map_err(|e| wc_core::Error::io("mkdir", e))?;
    std::fs::write(objects.join(&id[2..]), "not an object").map_err(|e| wc_core::Error::io("write", e))?;
    let err = git.revert(&id).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(!err.contains("no such version"), "{err}");
    assert!(err.contains("corrupt"), "{err}");
    assert_eq!(git.head(), Some(head), "nothing was reverted");
    Ok(())
}

#[test]
fn a_failed_revert_in_a_linked_worktree_is_undone() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path().join("main");
    std::fs::create_dir(&dir).map_err(|e| wc_core::Error::io("mkdir", e))?;
    store::create(&dir, "Demo")?;
    let git = Git::new(&dir);
    git.init()?;
    let write = |d: &std::path::Path, v: &str| {
        std::fs::write(d.join("note.txt"), v).map_err(|e| wc_core::Error::io("write", e))
    };
    write(&dir, "one\n")?;
    git.commit("one")?;
    write(&dir, "two\n")?;
    let two = git.commit("two")?.unwrap_or_default();
    write(&dir, "three\n")?;
    git.commit("three")?;
    let wt = tmp.path().join("wt");
    let ok = std::process::Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["worktree", "add", "-q", "-b", "side"])
        .arg(&wt)
        .status()
        .map_err(|e| wc_core::Error::io("git", e))?
        .success();
    assert!(ok, "worktree add");
    let side = Git::new(&wt);
    // Reverting "two" conflicts with "three": nothing may be left half done.
    assert!(side.revert(&two).is_err());
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&wt)
        .args(["status", "--porcelain"])
        .output()
        .map_err(|e| wc_core::Error::io("git", e))?;
    assert!(out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stdout));
    let text = std::fs::read_to_string(wt.join("note.txt")).map_err(|e| wc_core::Error::io("read", e))?;
    // Windows checkouts may turn the line end into CRLF.
    assert_eq!(text.trim_end(), "three");
    Ok(())
}

#[test]
fn a_resolved_merge_conflict_can_be_committed() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    let git = Git::new(dir);
    git.init()?;
    let write = |text: &str| std::fs::write(dir.join("notes.txt"), text).map_err(|e| wc_core::Error::io("notes", e));
    write("base\n")?;
    git.commit("base")?;
    git.create_branch("other")?;
    write("other\n")?;
    git.commit("other")?;
    git.switch("main")?;
    write("main\n")?;
    git.commit("main")?;
    assert_eq!(git.merge("other")?, MergeOutcome::Conflicts(vec!["notes.txt".into()]));
    // The index is unmerged here, so no copy of it can be taken; the commit must still work.
    assert!(git.run(&["write-tree"]).is_err());
    write("both\n")?;
    assert!(git.commit("Merge other")?.is_some());
    assert!(!dir.join(".git").join("MERGE_HEAD").exists());
    assert_eq!(
        git.run(&["show", "-s", "--format=%P", "HEAD"])?
            .split_whitespace()
            .count(),
        2
    );
    Ok(())
}

#[test]
fn a_merge_while_another_is_unfinished_is_refused() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    let git = Git::new(dir);
    git.init()?;
    let write = |text: &str| std::fs::write(dir.join("notes.txt"), text).map_err(|e| wc_core::Error::io("notes", e));
    write("base\n")?;
    git.commit("base")?;
    git.create_branch("other")?;
    write("other\n")?;
    git.commit("other")?;
    git.switch("main")?;
    write("main\n")?;
    git.commit("main")?;
    assert_eq!(git.merge("other")?, MergeOutcome::Conflicts(vec!["notes.txt".into()]));
    // Not the first merge's conflicts again, whatever branch is named.
    for name in ["other", "no-such-branch"] {
        let r = git.merge(name);
        let Err(Error::Git { message, .. }) = &r else {
            unreachable!("{r:?}")
        };
        assert!(message.contains("another merge is in progress"), "{message}");
    }
    // The first merge is untouched and can still be aborted.
    git.abort_merge()?;
    assert!(git.run(&["status", "--porcelain"])?.trim().is_empty());
    // Nothing left to abort: said plainly, not as git's fatal text.
    let r = git.abort_merge();
    let Err(Error::Git { message, .. }) = &r else {
        unreachable!("{r:?}")
    };
    assert_eq!(message, "no merge in progress");
    Ok(())
}

#[test]
fn a_merge_with_unrecorded_changes_is_refused() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    let git = Git::new(dir);
    git.init()?;
    let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).map_err(|e| wc_core::Error::io(name, e));
    write("notes.txt", "base\n")?;
    write("other.txt", "one\n")?;
    git.commit("base")?;
    git.create_branch("other")?;
    write("notes.txt", "other\n")?;
    git.commit("other")?;
    git.switch("main")?;
    write("notes.txt", "main\n")?;
    git.commit("main")?;
    // An edit not recorded yet: committing the conflicted merge would take it in too.
    write("other.txt", "edited\n")?;
    let head = git.head();
    let r = git.merge("other");
    let Err(Error::Git { message, .. }) = &r else {
        unreachable!("{r:?}")
    };
    assert!(message.contains("record or discard your changes first"), "{message}");
    assert_eq!(git.head(), head, "nothing was merged");
    assert!(!dir.join(".git").join("MERGE_HEAD").exists(), "no merge was started");
    let kept = std::fs::read_to_string(dir.join("other.txt")).map_err(|e| wc_core::Error::io("read", e))?;
    assert_eq!(kept, "edited\n", "the edit stays");
    // Recorded first, the merge goes ahead.
    git.commit("Edit other")?;
    assert_eq!(git.merge("other")?, MergeOutcome::Conflicts(vec!["notes.txt".into()]));
    Ok(())
}

#[test]
fn a_merge_conflict_names_a_file_with_an_accent_as_it_is() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    let git = Git::new(dir);
    git.init()?;
    std::fs::create_dir_all(dir.join("pages")).map_err(|e| wc_core::Error::io("pages", e))?;
    let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).map_err(|e| wc_core::Error::io(name, e));
    write("pages/øvre.page.json", "base\n")?;
    write("two words.txt", "base\n")?;
    git.commit("base")?;
    git.create_branch("other")?;
    write("pages/øvre.page.json", "other\n")?;
    write("two words.txt", "other\n")?;
    git.commit("other")?;
    git.switch("main")?;
    write("pages/øvre.page.json", "main\n")?;
    write("two words.txt", "main\n")?;
    git.commit("main")?;
    assert_eq!(
        git.merge("other")?,
        MergeOutcome::Conflicts(vec!["pages/øvre.page.json".into(), "two words.txt".into()])
    );
    Ok(())
}

#[test]
fn a_version_that_starts_with_a_dash_is_not_a_git_option() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    store::create(dir, "Demo")?;
    let git = Git::new(dir);
    git.init()?;
    let base = git.head().ok_or_else(|| wc_core::Error::Invalid("no head".into()))?;
    // As an option, `--output=<file>` makes git write that file and report no error.
    assert!(changes(&git, "--output=x", None).is_err());
    assert!(!dir.join("x").exists());
    assert!(changes(&git, &base, Some("--output=y")).is_err());
    assert!(!dir.join("y").exists());
    assert!(changes(&git, "--output=x", Some("--output=y")).is_err());
    assert!(!dir.join("x").exists() && !dir.join("y").exists());
    // A real version is still read.
    assert!(changes(&git, &base, None)?.is_empty());
    assert!(changes(&git, &base, Some(&base))?.is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_failed_commit_without_an_index_copy_says_so() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    let git = Git::new(dir);
    git.init()?;
    let io = |e| wc_core::Error::io("test", e);
    let write = |text: &str| std::fs::write(dir.join("notes.txt"), text).map_err(io);
    write("base\n")?;
    git.commit("base")?;
    git.create_branch("other")?;
    write("other\n")?;
    git.commit("other")?;
    git.switch("main")?;
    write("main\n")?;
    git.commit("main")?;
    assert!(matches!(git.merge("other")?, MergeOutcome::Conflicts(_)));
    write("both\n")?;
    // The unmerged index has no tree to copy, and a hook refuses the commit.
    let hook = dir.join(".git/hooks/pre-commit");
    std::fs::create_dir_all(dir.join(".git/hooks")).map_err(io)?;
    std::fs::write(&hook, "#!/bin/sh\necho refused by hook >&2\nexit 1\n").map_err(io)?;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).map_err(io)?;
    let e = git
        .commit("Merge other")
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(e.contains("refused by hook"), "the commit's reason: {e}");
    assert!(
        e.contains("could not be put back"),
        "says the index was not restored: {e}"
    );
    assert!(e.contains("no copy of the staged files was taken"), "and why: {e}");
    Ok(())
}

#[test]
fn a_commit_of_some_paths_leaves_every_other_change() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    let write = |name: &str, text: &str| -> Result<()> {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| wc_core::Error::io(parent, e))?;
        }
        std::fs::write(&path, text).map_err(|e| wc_core::Error::io(&path, e).into())
    };
    write("pages/p1.json", "1")?;
    write("pages/old.json", "old")?;
    write("vendor/symbols/a.json", "a1")?;
    let git = Git::new(dir);
    git.init()?;

    // A page added, one removed, a vendored file changed by someone else, and a staged stray file.
    write("pages/p2.json", "2")?;
    std::fs::remove_file(dir.join("pages/old.json")).map_err(|e| wc_core::Error::io("old", e))?;
    write("vendor/symbols/a.json", "a2")?;
    write("stray [x].txt", "s")?;
    git.run(&["add", "--", "stray [x].txt"])?;

    let head = git.commit_paths_as("Pages", "Guest <guest@example.com>", &["pages", "panels", "nothere"])?;
    assert!(head.is_some());
    let files = git.run(&["show", "--name-status", "--format=", "HEAD"])?;
    let mut lines: Vec<&str> = files.lines().collect();
    lines.sort();
    assert_eq!(lines, ["A\tpages/p2.json", "D\tpages/old.json"]);
    assert_eq!(git.author_and_subject("HEAD")?.0, "Guest <guest@example.com>");
    // The rest is still there to commit, the stray file still staged.
    let left = git.run(&["status", "--porcelain"])?;
    assert!(left.contains(" M vendor/symbols/a.json"), "{left}");
    assert!(
        left.contains("A  \"stray [x].txt\"") || left.contains("A  stray [x].txt"),
        "{left}"
    );

    // Nothing left under the paths: no commit.
    assert!(
        git.commit_paths_as("Again", "Guest <guest@example.com>", &["pages"])?
            .is_none()
    );
    // A removal staged with `git rm` is recorded too; a staged file elsewhere stays staged.
    git.run(&["rm", "-q", "--", "pages/p2.json"])?;
    git.commit_paths_as("Remove page 2", "Guest <guest@example.com>", &["pages"])?
        .ok_or("no commit")
        .map_err(|e| wc_core::Error::Invalid(e.into()))?;
    let files = git.run(&["show", "--name-status", "--format=", "HEAD"])?;
    assert_eq!(files.trim(), "D\tpages/p2.json");
    let left = git.run(&["status", "--porcelain"])?;
    assert!(left.contains("stray [x].txt"), "{left}");
    // A file added and removed again, beside a changed one: the changed one is recorded.
    write("pages/temp.json", "t")?;
    git.run(&["add", "--", "pages/temp.json"])?;
    std::fs::remove_file(dir.join("pages/temp.json")).map_err(|e| wc_core::Error::io("temp", e))?;
    write("pages/p1.json", "1b")?;
    assert!(
        git.commit_paths_as("Edit page 1", "Guest <guest@example.com>", &["pages"])?
            .is_some()
    );
    let files = git.run(&["show", "--name-status", "--format=", "HEAD"])?;
    assert_eq!(files.trim(), "M\tpages/p1.json");
    // A change staged and then undone in the folder leaves nothing to record: no commit.
    write("pages/p1.json", "1c")?;
    git.run(&["add", "--", "pages/p1.json"])?;
    write("pages/p1.json", "1b")?;
    let head = git.head();
    assert!(
        git.commit_paths_as("Nothing", "Guest <guest@example.com>", &["pages"])?
            .is_none()
    );
    assert_eq!(git.head(), head);
    // A name with glob characters is taken as it is written.
    assert!(
        git.commit_paths_as("Stray", "Guest <guest@example.com>", &["stray [x].txt"])?
            .is_some()
    );
    Ok(())
}

/// The names in a commit, byte for byte (`git ls-tree -r -z --name-only`).
#[cfg(all(unix, not(target_os = "macos")))]
fn names_in(dir: &std::path::Path, commit: &str) -> Result<Vec<Vec<u8>>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["ls-tree", "-r", "-z", "--name-only", commit])
        .output()
        .map_err(|e| wc_core::Error::io("git", e))?;
    if !out.status.success() {
        return Err(wc_core::Error::Invalid(format!(
            "git ls-tree {commit}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .into());
    }
    let mut names: Vec<Vec<u8>> = out
        .stdout
        .split(|&b| b == 0)
        .filter(|n| !n.is_empty())
        .map(<[u8]>::to_vec)
        .collect();
    names.sort();
    Ok(names)
}

// macOS file systems refuse names that are not UTF-8.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn names_that_are_not_utf8_are_recorded_as_they_are() -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    let git = Git::new(dir);
    git.init()?;
    let write = |name: &[u8]| -> Result<()> {
        let path = dir.join(std::ffi::OsStr::from_bytes(name));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| wc_core::Error::io(parent, e))?;
        }
        std::fs::write(&path, name).map_err(|e| wc_core::Error::io(&path, e).into())
    };
    // Two names that read alike when read lossily (both `a\u{fffd}.txt`).
    write(b"a\xff.txt")?;
    write(b"a\xfe.txt")?;
    let head = git
        .commit("Two odd names")?
        .ok_or_else(|| wc_core::Error::Invalid("no commit".into()))?;
    let names = names_in(dir, &head)?;
    assert!(names.contains(&b"a\xff.txt".to_vec()), "{names:?}");
    assert!(names.contains(&b"a\xfe.txt".to_vec()), "{names:?}");

    // Only some paths: the names git lists under them go back to it byte for byte.
    write(b"pages/p\xff.json")?;
    write(b"pages/p\xfe.json")?;
    write(b"other.txt")?;
    let head = git
        .commit_paths_as("Pages", "Guest <guest@example.com>", &["pages"])?
        .ok_or_else(|| wc_core::Error::Invalid("no commit".into()))?;
    let names = names_in(dir, &head)?;
    assert!(names.contains(&b"pages/p\xff.json".to_vec()), "{names:?}");
    assert!(names.contains(&b"pages/p\xfe.json".to_vec()), "{names:?}");
    assert!(!names.contains(&b"other.txt".to_vec()), "{names:?}");
    // Nothing under the paths is left to record.
    let left = git.run(&["status", "--porcelain"])?;
    assert_eq!(left.trim(), "?? other.txt");
    Ok(())
}

fn git_out(dir: &std::path::Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| wc_core::Error::io("git", e))?;
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn init_keeps_the_lock_out_of_a_project_whose_gitignore_predates_it() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    store::create(dir, "Demo")?;
    std::fs::write(dir.join(".gitignore"), "/target\n").map_err(|e| wc_core::Error::io("write", e))?;
    std::fs::write(dir.join(LOCK_FILE), "").map_err(|e| wc_core::Error::io("write", e))?;
    let git = Git::new(dir);
    git.init()?;
    let files = git_out(dir, &["ls-files"])?;
    assert!(!files.contains(LOCK_FILE), "{files}");
    assert!(git_out(dir, &["status", "--porcelain"])?.is_empty());
    Ok(())
}

#[test]
fn the_exclude_list_keeps_bytes_that_are_not_utf8() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    store::create(dir, "Demo")?;
    let git = Git::new(dir);
    git.init()?;
    let exclude = dir.join(".git/info/exclude");
    let before: &[u8] = b"# mine\n/caf\xe9";
    std::fs::write(&exclude, before).map_err(|e| wc_core::Error::io("write", e))?;
    git.exclude_locally(LOCK_FILE)?;
    git.exclude_locally(LOCK_FILE)?;
    let after = std::fs::read(&exclude).map_err(|e| wc_core::Error::io("read", e))?;
    assert_eq!(after, [before, b"\n/.wiringcraft.lock\n"].concat());
    Ok(())
}

#[test]
fn a_linked_worktree_leaves_the_lock_out_too() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path().join("main");
    std::fs::create_dir(&dir).map_err(|e| wc_core::Error::io("mkdir", e))?;
    store::create(&dir, "Demo")?;
    // A .gitignore from before the lock existed.
    std::fs::write(dir.join(".gitignore"), "/target\n").map_err(|e| wc_core::Error::io("write", e))?;
    Git::new(&dir).init()?;
    // As a repository made before the lock existed: the shared exclude list does not name it.
    let exclude = dir.join(".git/info/exclude");
    std::fs::write(&exclude, "# mine\n").map_err(|e| wc_core::Error::io("write", e))?;
    let wt = tmp.path().join("wt");
    git_out(&dir, &["worktree", "add", "-q", "-b", "side", &wt.to_string_lossy()])?;
    std::fs::write(wt.join(LOCK_FILE), "").map_err(|e| wc_core::Error::io("write", e))?;
    let status = git_out(&wt, &["status", "--porcelain"])?;
    assert!(status.contains(LOCK_FILE), "untracked at first: {status}");
    let side = Git::new(&wt);
    side.exclude_locally(LOCK_FILE)?;
    assert!(git_out(&wt, &["status", "--porcelain"])?.is_empty());
    let text = std::fs::read_to_string(&exclude).map_err(|e| wc_core::Error::io("read", e))?;
    assert_eq!(text, "# mine\n/.wiringcraft.lock\n");
    Ok(())
}

#[test]
fn an_exclude_list_that_cannot_be_read_is_reported_and_left_alone() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    store::create(dir, "Demo")?;
    let git = Git::new(dir);
    git.init()?;
    let exclude = dir.join(".git/info/exclude");
    std::fs::remove_file(&exclude).map_err(|e| wc_core::Error::io("rm", e))?;
    // A folder where the file belongs: reading it fails with a reason other than "missing".
    std::fs::create_dir(&exclude).map_err(|e| wc_core::Error::io("mkdir", e))?;
    let Err(Error::Io { path, .. }) = git.exclude_locally("other.lock") else {
        unreachable!("an unreadable exclude list is an error");
    };
    assert!(path.ends_with("exclude"), "{path}");
    assert!(exclude.is_dir(), "left as it was");
    Ok(())
}

#[test]
fn a_failed_write_leaves_the_exclude_list_as_it_was() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    store::create(dir, "Demo")?;
    let git = Git::new(dir);
    git.init()?;
    let exclude = dir.join(".git/info/exclude");
    let before = std::fs::read(&exclude).map_err(|e| wc_core::Error::io("read", e))?;
    // A folder where the new list would be written: the write fails.
    let scratch = crate::git::exclude_scratch(&exclude);
    std::fs::create_dir(&scratch).map_err(|e| wc_core::Error::io("mkdir", e))?;
    assert!(git.exclude_locally("other.lock").is_err());
    let after = std::fs::read(&exclude).map_err(|e| wc_core::Error::io("read", e))?;
    assert_eq!(after, before);
    Ok(())
}

#[test]
fn changes_name_a_file_with_an_accent_as_it_is() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    store::create(dir, "Demo")?;
    let git = Git::new(dir);
    git.init()?;
    let base = git.head().ok_or_else(|| wc_core::Error::Invalid("no head".into()))?;
    let path = dir.join("café.png");
    std::fs::write(&path, "1").map_err(|e| wc_core::Error::io(&path, e))?;

    // Not recorded yet.
    let found = changes(&git, &base, None)?;
    assert!(
        found.contains(&Change::File {
            path: "café.png".into(),
            status: "added".into()
        }),
        "{found:?}"
    );
    // Recorded, then changed again in the folder.
    git.commit("Picture")?;
    let first = git.head().ok_or_else(|| wc_core::Error::Invalid("no head".into()))?;
    let found = changes(&git, &base, Some(&first))?;
    assert!(
        found.contains(&Change::File {
            path: "café.png".into(),
            status: "added".into()
        }),
        "{found:?}"
    );
    std::fs::write(&path, "2").map_err(|e| wc_core::Error::io(&path, e))?;
    let found = changes(&git, &first, None)?;
    assert_eq!(
        found,
        [Change::File {
            path: "café.png".into(),
            status: "changed".into()
        }]
    );
    Ok(())
}

#[test]
fn a_name_that_starts_with_a_dash_is_never_a_git_option() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    let mut p = store::create(dir, "Demo")?;
    let git = Git::new(dir);
    git.init()?;
    // A second version on main, and a branch `side` that main follows: a bare `git merge`
    // would merge it, and `git revert --all` would undo every version.
    p.meta.title_block.insert("customer".into(), "ACME".into());
    store::save(dir, &p)?;
    git.commit("Customer")?;
    git.create_branch("side")?;
    p.meta.title_block.insert("customer".into(), "Side".into());
    store::save(dir, &p)?;
    git.commit("On side")?;
    git.switch("main")?;
    git.run(&["config", "branch.main.remote", "."])?;
    git.run(&["config", "branch.main.merge", "refs/heads/side"])?;
    let head = git.head();

    assert!(git.switch("--detach").is_err(), "no such branch");
    assert_eq!(git.current_branch().as_deref(), Some("main"), "HEAD is not detached");
    assert!(git.merge("--log").is_err(), "no such branch");
    // Refused before git runs: `git revert --all` would walk every branch, even after
    // `--end-of-options`, and fail (then be undone) only by chance.
    let refused = git.revert("--all").err().map(|e| e.to_string()).unwrap_or_default();
    assert!(refused.contains("no such version: --all"), "{refused}");
    assert!(git.author_and_subject("--output=log").is_err(), "no such version");
    assert!(git.create_branch("-x").is_err(), "not a branch name");
    assert_eq!(git.show("--output=shown", "project.json"), None, "no such version");
    assert_eq!(git.head(), head, "nothing was merged or reverted");
    assert_eq!(git.current_branch().as_deref(), Some("main"));

    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| wc_core::Error::io("read", e))? {
        let entry = entry.map_err(|e| wc_core::Error::io("read", e))?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    assert!(
        !names.iter().any(|n| n.starts_with("shown") || n.starts_with("log")),
        "git wrote no file: {names:?}"
    );
    Ok(())
}

#[test]
fn a_page_file_that_cannot_be_read_is_listed_as_a_file() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    store::create(dir, "Demo")?;
    let git = Git::new(dir);
    git.init()?;
    let base = git.head().ok_or_else(|| wc_core::Error::Invalid("no head".into()))?;
    let path = dir.join("pages/p0001.page.json");
    let changed = [Change::File {
        path: "pages/p0001.page.json".into(),
        status: "changed".into(),
    }];

    // Readable before, not now (left with merge conflict markers): changed, not removed.
    std::fs::write(&path, "<<<<<<< ours\n{}\n=======\n{}\n>>>>>>> theirs\n")
        .map_err(|e| wc_core::Error::io(&path, e))?;
    assert_eq!(changes(&git, &base, None)?, changed);
    git.commit("Broken")?;
    let broken = git.head().ok_or_else(|| wc_core::Error::Invalid("no head".into()))?;
    assert_eq!(changes(&git, &base, Some(&broken))?, changed);

    // Unreadable on both sides, and changed: still listed.
    std::fs::write(&path, "not a page").map_err(|e| wc_core::Error::io(&path, e))?;
    assert_eq!(changes(&git, &broken, None)?, changed);
    Ok(())
}

#[test]
fn a_page_file_that_is_not_text_is_changed_not_removed() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    store::create(dir, "Demo")?;
    let git = Git::new(dir);
    git.init()?;
    let base = git.head().ok_or_else(|| wc_core::Error::Invalid("no head".into()))?;
    let path = dir.join("pages/p0001.page.json");

    // Bytes that are not UTF-8: the file is still there, so it is listed, not removed.
    std::fs::write(&path, [0xff, 0xfe, b'{', 0x80, b'}']).map_err(|e| wc_core::Error::io(&path, e))?;
    assert_eq!(
        changes(&git, &base, None)?,
        [Change::File {
            path: "pages/p0001.page.json".into(),
            status: "changed".into(),
        }]
    );

    // Really gone: still reported as a removed page.
    std::fs::remove_file(&path).map_err(|e| wc_core::Error::io(&path, e))?;
    let ch = changes(&git, &base, None)?;
    assert!(
        matches!(ch.as_slice(), [Change::PageRemoved { page, .. }] if page == "1"),
        "{ch:?}"
    );
    Ok(())
}

#[test]
fn a_new_branch_is_current_and_keeps_changes_not_yet_recorded() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    let mut p = store::create(dir, "Demo")?;
    let git = Git::new(dir);
    git.init()?;
    p.meta.title_block.insert("customer".into(), "ACME".into());
    store::save(dir, &p)?;
    let pending = changes(&git, "HEAD", None)?;
    assert!(!pending.is_empty(), "a change not yet recorded");

    git.create_branch("draft")?;
    assert_eq!(git.current_branch().as_deref(), Some("draft"));
    assert_eq!(changes(&git, "HEAD", None)?, pending, "the change came along");
    assert_eq!(
        store::load(dir)?.meta.title_block.get("customer").map(String::as_str),
        Some("ACME")
    );
    Ok(())
}

#[test]
fn a_page_version_that_cannot_be_read_is_listed_as_a_file_in_its_history() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    store::create(dir, "Demo")?;
    let git = Git::new(dir);
    git.init()?;
    let path = dir.join("pages/p0001.page.json");
    let good = std::fs::read(&path).map_err(|e| wc_core::Error::io(&path, e))?;
    let write = |bytes: &[u8]| std::fs::write(&path, bytes).map_err(|e| wc_core::Error::io(&path, e));

    write(b"<<<<<<< ours\n{}\n=======\n{}\n>>>>>>> theirs\n")?;
    git.commit("Broken")?;
    write(b"not a page")?;
    git.commit("Still broken")?;
    write(&good)?;
    git.commit("Fixed")?;
    write(b"not a page")?;
    git.commit("Broken again")?;
    std::fs::remove_file(&path).map_err(|e| wc_core::Error::io(&path, e))?;
    git.commit("Removed")?;

    let file = |status: &str| {
        vec![Change::File {
            path: "pages/p0001.page.json".into(),
            status: status.into(),
        }]
    };
    let h = page_history(&git, "p0001", 10)?;
    let got: Vec<(&str, &[Change])> = h.iter().map(|v| (v.subject.as_str(), v.changes.as_slice())).collect();
    let (removed, changed) = (file("removed"), file("changed"));
    let expected: Vec<(&str, &[Change])> = vec![
        ("Removed", &removed),
        ("Broken again", &changed),
        ("Fixed", &changed),
        ("Still broken", &changed),
        ("Broken", &changed),
    ];
    assert_eq!(got[..5], expected[..], "{got:?}");
    // The first version, readable, is still an added page.
    assert!(matches!(got.get(5), Some((_, [Change::PageAdded { .. }]))), "{got:?}");
    Ok(())
}

#[test]
fn a_path_that_is_not_utf8_is_read_in_quoted_form() {
    assert_eq!(git::path_text(b"pages/\xc3\xb8vre.page.json"), "pages/øvre.page.json");
    assert_eq!(git::path_text(b"two words.txt"), "two words.txt");
    assert_eq!(git::path_text(b"a\xffb"), r#""a\377b""#);
    assert_eq!(git::path_text(b"a\xfeb"), r#""a\376b""#);
    assert_eq!(git::path_text(b"\"x\\\xff"), r#""\"x\\\377""#);
    // A UTF-8 name stays usable as it is, even one that starts with a quote.
    assert_eq!(git::path_text(br#""quoted".txt"#), r#""quoted".txt"#);
    assert_eq!(git::path_text(br#"a"b"#), r#"a"b"#);
    assert_eq!(
        git::path_text("ø\u{fffd}".as_bytes()),
        "ø\u{fffd}",
        "U+FFFD itself is UTF-8"
    );
}

/// Two file names that differ only in a byte that is not UTF-8 (both would read as `a\u{fffd}.txt`
/// lossily) stay two entries in the change list and in a merge's conflict list. Not on macOS:
/// its file systems refuse such names ("Illegal byte sequence"), so they never reach git there.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn names_that_differ_in_a_byte_that_is_not_utf8_stay_apart() -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    let git = Git::new(dir);
    git.init()?;
    let names = [&b"a\xff.txt"[..], &b"a\xfe.txt"[..]];
    let write = |text: &str| -> Result<()> {
        for name in names {
            let path = dir.join(std::ffi::OsStr::from_bytes(name));
            std::fs::write(&path, text).map_err(|e| wc_core::Error::io("write", e))?;
        }
        Ok(())
    };
    let listed = vec![r#""a\377.txt""#.to_owned(), r#""a\376.txt""#.to_owned()];
    std::fs::write(dir.join("base.txt"), "base\n").map_err(|e| wc_core::Error::io("write", e))?;
    git.commit("base")?;
    let base = git.head().ok_or_else(|| wc_core::Error::Invalid("no head".into()))?;
    // Not recorded yet: listed as new files.
    write("one\n")?;
    let mut new: Vec<String> = changes(&git, &base, None)?
        .into_iter()
        .filter_map(|c| match c {
            Change::File { path, status } if status == "added" => Some(path),
            _ => None,
        })
        .collect();
    new.sort();
    let mut want = listed.clone();
    want.sort();
    assert_eq!(new, want);
    // Recorded: listed between two versions.
    git.commit("one")?;
    let mut between: Vec<String> = changes(&git, &base, Some("HEAD"))?
        .into_iter()
        .filter_map(|c| match c {
            Change::File { path, .. } => Some(path),
            _ => None,
        })
        .collect();
    between.sort();
    assert_eq!(between, want);
    // Changed differently on two branches: two conflicts.
    git.create_branch("other")?;
    write("other\n")?;
    git.commit("other")?;
    git.switch("main")?;
    write("main\n")?;
    git.commit("main")?;
    let MergeOutcome::Conflicts(mut conflicts) = git.merge("other")? else {
        unreachable!("the merge conflicts")
    };
    conflicts.sort();
    assert_eq!(conflicts, want);
    Ok(())
}

#[test]
fn in_work_tree_sees_the_projects_own_and_a_larger_repository() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    store::create(dir, "Demo")?;
    let git = Git::new(dir);
    git.init()?;
    assert!(git.in_work_tree()?);
    // A project in a subfolder of a larger repository can keep versions there.
    let sub = dir.join("sub");
    std::fs::create_dir(&sub).map_err(|e| wc_core::Error::io("sub", e))?;
    assert!(Git::new(&sub).in_work_tree()?);
    // Git's own folder is no work tree.
    assert!(!Git::new(dir.join(".git")).in_work_tree()?);
    // A folder that does not exist is an error naming it, not "no repository".
    let gone = dir.join("gone");
    let missing = Git::new(&gone).in_work_tree();
    assert!(
        matches!(&missing, Err(Error::Io { path, .. }) if *path == gone.display().to_string()),
        "expected the missing folder as the error, got {missing:?}"
    );
    Ok(())
}

#[test]
fn in_work_tree_is_false_where_git_finds_no_repository() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let dir = tmp.path();
    // A `.git` file pointing nowhere: git stops here (with "not a git repository", or in newer
    // git "gitfile does not point to a valid repository") whatever
    // repository the temporary folder may sit in.
    let gitdir = format!("gitdir: {}\n", dir.join("none").display());
    std::fs::write(dir.join(".git"), gitdir).map_err(|e| wc_core::Error::io(".git", e))?;
    assert!(!Git::new(dir).in_work_tree()?);
    Ok(())
}

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
    // A name with glob characters is taken as it is written.
    assert!(
        git.commit_paths_as("Stray", "Guest <guest@example.com>", &["stray [x].txt"])?
            .is_some()
    );
    Ok(())
}

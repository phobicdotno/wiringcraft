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

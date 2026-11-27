//! Public behaviour of the library: opening sources, finding items by kind, taking and
//! proposing parts, and what `status` says when a pinned copy has gone.

use std::path::{Path, PathBuf};

use wc_core::Part;
use wc_core::lock::ItemKind;
use wc_core::project::LibrarySource;
use wc_core::store;
use wc_library::{Error, Library, State, library_rules, propose, status, take, write_part};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn part(id: &str, description: &str) -> Part {
    Part {
        id: id.into(),
        manufacturer: "ACME".into(),
        order_number: "  ".into(),
        description: description.into(),
        ..Part::default()
    }
}

fn folder(name: &str, location: &Path) -> LibrarySource {
    LibrarySource {
        name: name.into(),
        kind: "folder".into(),
        location: location.to_string_lossy().into(),
        reference: None,
    }
}

/// A project with two folder sources `a` and `b` (in that order), both empty.
fn project() -> std::io::Result<(tempfile::TempDir, PathBuf, wc_core::Project, PathBuf, PathBuf)> {
    let tmp = tempfile::tempdir()?;
    let proj = tmp.path().join("proj");
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    let mut project = store::create(&proj, "T").map_err(std::io::Error::other)?;
    project.meta.libraries = vec![folder("a", &a), folder("b", &b)];
    Ok((tmp, proj, project, a, b))
}

#[test]
fn opening_a_source_of_an_unknown_kind_names_the_source_and_the_kind() -> TestResult {
    let (_tmp, proj, mut project, _, _) = project()?;
    project.meta.libraries[1].kind = "svn".into();
    let Err(Error::Source { name, message }) = Library::open(&proj, &project) else {
        unreachable!("an unknown source kind was accepted")
    };
    assert_eq!(name, "b");
    assert!(message.contains("unknown kind \"svn\""), "{message}");
    Ok(())
}

#[test]
fn a_missing_item_is_not_found_under_its_own_kind() -> TestResult {
    let (_tmp, proj, project, a, _) = project()?;
    write_part(&a, &part("coil", "only a part"))?;
    let lib = Library::open(&proj, &project)?;
    // A part named `coil` is no symbol `coil`.
    assert!(lib.locate(ItemKind::Symbol, "coil").is_none());
    let Err(Error::NotFound { kind, id }) = lib.symbol("coil") else {
        unreachable!("the part was read as a symbol")
    };
    assert_eq!((kind, id.as_str()), (ItemKind::Symbol, "coil"));
    let Err(err) = lib.part("nope") else {
        unreachable!("a missing part was found")
    };
    assert!(matches!(
        err,
        Error::NotFound {
            kind: ItemKind::Part,
            ..
        }
    ));
    assert_eq!(err.to_string(), "Part \"nope\" is not in any library source");
    Ok(())
}

#[test]
fn locate_gives_the_winning_source_and_its_file() -> TestResult {
    let (_tmp, proj, project, a, b) = project()?;
    write_part(&b, &part("lamp", "from b"))?;
    let lib = Library::open(&proj, &project)?;
    let Some(found) = lib.locate(ItemKind::Part, "lamp") else {
        unreachable!("lamp not located")
    };
    assert_eq!(found.source, "b");
    assert_eq!(found.path, b.join("parts").join("lamp.json"));

    // Once the first source holds it too, that source wins.
    let written = write_part(&a, &part("lamp", "from a"))?;
    let Some(found) = lib.locate(ItemKind::Part, "lamp") else {
        unreachable!("lamp not located")
    };
    assert_eq!((found.source.as_str(), found.path), ("a", written));
    assert_eq!(lib.part("lamp")?.0.description, "from a");
    assert_eq!(lib.part_in("b", "lamp")?.description, "from b");
    Ok(())
}

#[test]
fn taking_a_part_vendors_and_pins_the_winning_version() -> TestResult {
    let (_tmp, proj, mut project, _, b) = project()?;
    let lamp = part("lamp", "signal lamp");
    write_part(&b, &lamp)?;
    let lib = Library::open(&proj, &project)?;
    let entry = take(&proj, &mut project, &lib, ItemKind::Part, "lamp")?;
    assert_eq!(entry.source, "b");
    assert_eq!(entry.hash, lamp.hash());
    assert_eq!(project.lock.get(ItemKind::Part, "lamp"), Some(&entry));
    assert_eq!(store::read_vendored_part(&proj, "lamp")?, lamp);

    let st = status(&proj, &project, &lib);
    assert_eq!(st.len(), 1, "{st:?}");
    let s = &st[0];
    assert_eq!(
        (s.kind, s.id.as_str(), s.state),
        (ItemKind::Part, "lamp", State::UpToDate)
    );
    assert_eq!(s.library_source.as_deref(), Some("b"));
    assert_eq!(s.library_hash.as_deref(), Some(entry.hash.as_str()));
    assert!(s.used_by.is_empty() && s.changes.is_empty(), "{s:?}");
    Ok(())
}

#[test]
fn a_changed_part_whose_vendored_copy_was_deleted_says_the_copy_is_missing() -> TestResult {
    let (_tmp, proj, mut project, a, _) = project()?;
    write_part(&a, &part("lamp", "old"))?;
    let lib = Library::open(&proj, &project)?;
    take(&proj, &mut project, &lib, ItemKind::Part, "lamp")?;
    write_part(&a, &part("lamp", "new"))?;
    std::fs::remove_file(store::vendor_path(&proj, ItemKind::Part, "lamp")?)?;
    let st = status(&proj, &project, &lib);
    assert_eq!(st.len(), 1, "{st:?}");
    assert_eq!(st[0].state, State::Changed);
    assert_eq!(st[0].changes, ["vendored copy missing"]);
    Ok(())
}

#[test]
fn proposing_a_part_copies_the_projects_version_for_review() -> TestResult {
    let (_tmp, proj, mut project, a, b) = project()?;
    write_part(&a, &part("lamp", "library"))?;
    let lib = Library::open(&proj, &project)?;
    take(&proj, &mut project, &lib, ItemKind::Part, "lamp")?;
    // The project edits its copy, then proposes it to the second source.
    store::write_vendored_part(&proj, &part("lamp", "edited"))?;
    let path = propose(&proj, &lib, "b", ItemKind::Part, "lamp")?;
    assert_eq!(path, b.join("proposed").join("parts").join("lamp.json"));
    let proposed: Part = store::read_json(&path)?;
    assert_eq!(proposed.description, "edited");
    // The real item is untouched and the proposal is no library item.
    assert_eq!(lib.part("lamp")?.0.description, "library");
    assert!(lib.locate(ItemKind::Part, "lamp").is_some_and(|l| l.source == "a"));
    // Nothing vendored under that id: nothing to propose.
    assert!(matches!(
        propose(&proj, &lib, "b", ItemKind::Part, "nope"),
        Err(Error::Core(_))
    ));
    Ok(())
}

#[test]
fn an_empty_search_lists_every_item_with_a_trimmed_part_title() -> TestResult {
    let (_tmp, proj, project, a, b) = project()?;
    write_part(&a, &part("lamp", ""))?;
    write_part(&b, &part("horn", "loud"))?;
    let lib = Library::open(&proj, &project)?;
    let hits: Vec<(String, String, String)> = lib
        .search("   ")
        .into_iter()
        .map(|h| (h.id, h.source, h.title))
        .collect();
    assert_eq!(
        hits,
        [
            ("horn".to_owned(), "b".to_owned(), "ACME    loud".to_owned()),
            ("lamp".to_owned(), "a".to_owned(), "ACME".to_owned()),
        ]
    );
    Ok(())
}

#[test]
fn library_rules_say_why_a_source_cannot_be_resolved() -> TestResult {
    let (_tmp, proj, mut project, _, _) = project()?;
    project.meta.libraries[0].kind = "svn".into();
    let sets = library_rules(&proj, &project);
    // `a` cannot be resolved; `b` has no folder, so it adds nothing.
    assert_eq!(sets.len(), 1, "{sets:?}");
    assert_eq!(sets[0].source, "a");
    let Err(why) = &sets[0].rules else {
        unreachable!("rules read from an unresolvable source")
    };
    assert!(why.contains("unknown kind \"svn\""), "{why}");
    Ok(())
}

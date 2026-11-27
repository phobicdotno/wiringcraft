//! New projects from a base project in a git repository, through the public
//! `wc_vcs::base` functions.
#![allow(clippy::unwrap_used)]

use std::path::Path;

use wc_core::project::LibrarySource;
use wc_core::store;
use wc_vcs::Git;
use wc_vcs::base::{create_from_base, is_url};

/// A `file://` URL for a local folder, also with a Windows drive letter.
fn file_url(p: &Path) -> String {
    let path = p.to_string_lossy().replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

fn library(name: &str, kind: &str, location: &str) -> LibrarySource {
    LibrarySource {
        name: name.into(),
        kind: kind.into(),
        location: location.into(),
        reference: None,
    }
}

#[test]
fn every_url_scheme_is_a_repository_and_lookalikes_are_folders() {
    for url in ["http://host/base.git", "git://host/base", "git@host:base"] {
        assert!(is_url(url), "{url}");
    }
    for folder in [
        "",
        "https:/host/base",
        "./https://host",
        "base/git@host:x",
        "Git@host:base",
    ] {
        assert!(!is_url(folder), "{folder}");
    }
}

#[test]
fn a_repository_fills_name_and_date_into_the_title_block() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    let mut p = store::create(&base, "ACME base").unwrap();
    p.meta.title_block.insert("project".into(), "{name} ({date})".into());
    p.meta.title_block.insert("date".into(), "2020-01-01".into());
    store::save(&base, &p).unwrap();
    Git::new(&base).init().unwrap();

    let new = tmp.path().join("new");
    let made = create_from_base(&file_url(&base), &new, "Pump station", "2026-11-27").unwrap();
    assert_eq!(made.meta.title_block["project"], "Pump station (2026-11-27)");
    let loaded = store::load(&new).unwrap();
    assert_eq!(loaded.meta.name, "Pump station");
    assert_eq!(loaded.meta.title_block["project"], "Pump station (2026-11-27)");
    assert_eq!(loaded.meta.title_block["date"], "2026-11-27");
    // The base repository itself is left as it was.
    assert_eq!(
        store::load(&base).unwrap().meta.title_block["project"],
        "{name} ({date})"
    );
}

#[test]
fn a_repository_with_a_library_folder_inside_it_brings_the_folder_along() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    let mut p = store::create(&base, "ACME base").unwrap();
    p.meta.libraries.push(library("own", "folder", "lib/./parts"));
    p.meta
        .libraries
        .push(library("abs", "folder", "/srv/libraries/company"));
    p.meta
        .libraries
        .push(library("shared", "git", "https://example.invalid/lib.git"));
    store::save(&base, &p).unwrap();
    std::fs::create_dir_all(base.join("lib/parts")).unwrap();
    std::fs::write(base.join("lib/parts/relay.txt"), "relay").unwrap();
    Git::new(&base).init().unwrap();

    let new = tmp.path().join("new");
    let made = create_from_base(&file_url(&base), &new, "New", "2026-11-27").unwrap();
    let locations: Vec<&str> = made.meta.libraries.iter().map(|l| l.location.as_str()).collect();
    assert_eq!(
        locations,
        [
            "lib/./parts",
            "/srv/libraries/company",
            "https://example.invalid/lib.git"
        ]
    );
    assert_eq!(
        std::fs::read_to_string(new.join("lib/parts/relay.txt")).unwrap(),
        "relay"
    );
}

#[test]
fn every_library_folder_outside_the_repository_is_named() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    let mut p = store::create(&base, "ACME base").unwrap();
    p.meta.libraries.push(library("a", "folder", "../one"));
    p.meta.libraries.push(library("b", "folder", "lib/../../two"));
    p.meta.libraries.push(library("c", "folder", "lib"));
    store::save(&base, &p).unwrap();
    Git::new(&base).init().unwrap();

    let new = tmp.path().join("new");
    let e = create_from_base(&file_url(&base), &new, "New", "2026-11-27").unwrap_err();
    let msg = e.to_string();
    assert!(msg.contains("(../one, lib/../../two)"), "{msg}");
    assert!(!new.exists());
}

#[test]
fn a_repository_is_not_put_into_a_folder_that_holds_files() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    store::create(&base, "ACME base").unwrap();
    Git::new(&base).init().unwrap();

    let new = tmp.path().join("new");
    std::fs::create_dir(&new).unwrap();
    std::fs::write(new.join("notes.txt"), "keep me").unwrap();
    let e = create_from_base(&file_url(&base), &new, "New", "2026-11-27").unwrap_err();
    assert!(e.to_string().contains("not empty"), "{e}");
    assert_eq!(std::fs::read_to_string(new.join("notes.txt")).unwrap(), "keep me");
    assert_eq!(std::fs::read_dir(&new).unwrap().count(), 1);

    // An empty folder is fine.
    let empty = tmp.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    create_from_base(&file_url(&base), &empty, "New", "2026-11-27").unwrap();
    assert_eq!(store::load(&empty).unwrap().meta.name, "New");
}

#[test]
fn a_repository_without_a_project_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join("README.txt"), "not a project").unwrap();
    Git::new(&repo).init().unwrap();

    let new = tmp.path().join("new");
    assert!(create_from_base(&file_url(&repo), &new, "New", "2026-11-27").is_err());
    assert!(!new.exists());
    // A folder with no project is refused the same way.
    let e = create_from_base(&repo.to_string_lossy(), &new, "New", "2026-11-27").unwrap_err();
    assert!(e.to_string().contains("holds no project"), "{e}");
    assert!(!new.exists());
}

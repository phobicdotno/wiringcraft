//! Tests of the library's lookups across sources, search, and the update error messages.

use super::*;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn symbol(id: &str, name: &str) -> Symbol {
    Symbol {
        source: String::new(),
        id: id.into(),
        name: name.into(),
        category: String::new(),
        letter_code: String::new(),
        role: wc_core::symbol::SymbolRole::Main,
        graphics: vec![],
        pins: vec![],
        tag_at: wc_core::geom::Point::default(),
        description: String::new(),
    }
}

/// Two folder sources, `a` first and `b` second, under a temporary folder.
fn two_sources() -> std::io::Result<(tempfile::TempDir, Library)> {
    let tmp = tempfile::tempdir()?;
    let source = |name: &str| Source {
        name: name.into(),
        root: tmp.path().join(name),
    };
    let lib = Library {
        sources: vec![source("a"), source("b")],
    };
    Ok((tmp, lib))
}

fn root(lib: &Library, n: usize) -> &Path {
    lib.sources.get(n).map_or(Path::new(""), |s| s.root.as_path())
}

#[test]
fn find_version_tries_the_named_source_first_then_the_others() -> TestResult {
    let (_tmp, lib) = two_sources()?;
    write_symbol(root(&lib, 0), &symbol("coil", "from a"))?;
    write_symbol(root(&lib, 1), &symbol("coil", "from b"))?;
    let any = |_: &Symbol| Ok(());
    let found = lib.find_version(ItemKind::Symbol, "coil", "b", any)?;
    assert_eq!(found.name, "from b");
    // A source that does not fit is passed over for the next one.
    let not_b = |s: &Symbol| {
        if s.name == "from b" {
            Err("wrong".into())
        } else {
            Ok(())
        }
    };
    let found = lib.find_version(ItemKind::Symbol, "coil", "b", not_b)?;
    assert_eq!(found.name, "from a");
    Ok(())
}

#[test]
fn find_version_names_why_each_source_was_refused() -> TestResult {
    let (_tmp, lib) = two_sources()?;
    let none = lib.find_version(ItemKind::Symbol, "coil", "a", |_: &Symbol| Ok(()));
    assert_eq!(none.err().as_deref(), Some("no library has it"));

    write_symbol(root(&lib, 0), &symbol("coil", "x"))?;
    // In `b`, `coil.json` is a folder: it is there but cannot be read as a file.
    std::fs::create_dir_all(root(&lib, 1).join("symbols").join("coil.json"))?;
    let refuse = |_: &Symbol| Err::<(), String>("too old".into());
    let Err(why) = lib.find_version(ItemKind::Symbol, "coil", "b", refuse) else {
        unreachable!("nothing fits")
    };
    // `b` is tried first, so its reason comes first.
    let Some((b, a)) = why.split_once("; ") else {
        unreachable!("{why}")
    };
    assert!(b.starts_with("b: ") && b.contains("it is not a file"), "{why}");
    assert_eq!(a, "a: too old");
    Ok(())
}

#[test]
fn symbol_in_reads_the_named_source_even_when_another_wins() -> TestResult {
    let (_tmp, lib) = two_sources()?;
    write_symbol(root(&lib, 0), &symbol("coil", "from a"))?;
    write_symbol(root(&lib, 1), &symbol("coil", "from b"))?;
    assert_eq!(lib.symbol("coil")?.1, "a");
    assert_eq!(lib.symbol_in("b", "coil")?.name, "from b");
    assert!(matches!(
        lib.symbol_in("nope", "coil"),
        Err(Error::NotFound {
            kind: ItemKind::Symbol,
            ..
        })
    ));
    assert!(matches!(
        lib.part_in("a", "coil"),
        Err(Error::NotFound {
            kind: ItemKind::Part,
            ..
        })
    ));
    // An id that is no plain file name is refused, not looked up.
    assert!(matches!(lib.symbol_in("a", "../a/symbols/coil"), Err(Error::Core(_))));
    Ok(())
}

#[test]
fn the_index_gives_each_id_the_source_that_wins_for_it() -> TestResult {
    let (_tmp, lib) = two_sources()?;
    write_symbol(root(&lib, 0), &symbol("coil", "x"))?;
    write_symbol(root(&lib, 1), &symbol("coil", "x"))?;
    write_symbol(root(&lib, 1), &symbol("lamp", "x"))?;
    let index = lib.index(ItemKind::Symbol);
    let pairs: Vec<(&str, &str)> = index.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(pairs, [("coil", "a"), ("lamp", "b")]);
    assert!(lib.index(ItemKind::Part).is_empty());
    Ok(())
}

#[test]
fn search_finds_a_part_by_a_property_and_titles_it_without_blanks() -> TestResult {
    let (_tmp, lib) = two_sources()?;
    let mut part = Part {
        id: "psu24".into(),
        description: "Power supply".into(),
        ..Part::default()
    };
    part.properties.insert("Voltage".into(), "24V".into());
    write_part(root(&lib, 1), &part)?;
    let hits = lib.search("voltage=24v");
    assert_eq!(
        hits,
        [SearchHit {
            kind: ItemKind::Part,
            id: "psu24".into(),
            source: "b".into(),
            title: "Power supply".into(),
        }]
    );
    assert!(lib.search("voltage=230v").is_empty());
    Ok(())
}

#[test]
fn writing_an_item_with_an_unsafe_id_is_refused() -> TestResult {
    let (_tmp, lib) = two_sources()?;
    assert!(write_symbol(root(&lib, 0), &symbol("../escape", "x")).is_err());
    let part = Part {
        id: "a/b".into(),
        ..Part::default()
    };
    assert!(write_part(root(&lib, 0), &part).is_err());
    assert!(!root(&lib, 0).exists());
    Ok(())
}

#[test]
fn an_unknown_id_error_lists_at_most_twenty_known_ids() {
    let known: Vec<String> = (0..25).map(|n| format!("id{n:02}")).collect();
    let msg = Error::NotInProject {
        ids: vec!["typo".into()],
        known,
    }
    .to_string();
    assert!(msg.starts_with("the project does not use or pin \"typo\"; "), "{msg}");
    assert!(msg.contains("\"id19\" and 5 more"), "{msg}");
    assert!(!msg.contains("id20"), "{msg}");

    let msg = Error::NotInProject {
        ids: vec!["x".into(), "y".into()],
        known: vec![],
    }
    .to_string();
    assert_eq!(
        msg,
        "the project does not use or pin \"x\", \"y\"; it uses or pins no symbols or parts (see `wiringcraft lib status`)"
    );
}

#[test]
fn changed_keeps_only_named_items_with_a_newer_version() {
    let item = |id: &str, state: State, hash: Option<&str>| ItemStatus {
        kind: ItemKind::Symbol,
        id: id.into(),
        state,
        locked: None,
        library_hash: hash.map(str::to_owned),
        library_source: None,
        used_by: vec![],
        changes: vec![],
    };
    let all = [
        item("a", State::Changed, Some("h1")),
        item("b", State::Changed, Some("h2")),
        item("c", State::UpToDate, Some("h3")),
        item("d", State::Changed, None),
    ];
    let ids = |v: Vec<(ItemKind, String, String)>| v.into_iter().map(|(_, id, h)| (id, h)).collect::<Vec<_>>();
    assert_eq!(
        ids(changed(&all, None)),
        [("a".to_owned(), "h1".to_owned()), ("b".to_owned(), "h2".to_owned())]
    );
    let only = ["b".to_owned(), "c".to_owned()];
    assert_eq!(ids(changed(&all, Some(&only))), [("b".to_owned(), "h2".to_owned())]);
    // Naming an item that is not wrong is no wrong-id error.
    assert!(wrong_named(&all, Some(&only)).is_none());
    assert!(unknown_named(&all, Some(&only)).is_none());
}

#[test]
fn restored_items_alone_leave_nothing_undone() {
    let mut left = Unpinned::default();
    assert!(left.is_empty());
    left.restored.push((ItemKind::Symbol, "coil".into()));
    assert!(left.is_empty());
    left.missing.push((ItemKind::Part, "psu".into()));
    assert!(!left.is_empty());
}

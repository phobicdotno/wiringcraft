use super::*;
use wc_core::geom::Point;
use wc_core::project::{LibrarySource, Page, PlacedSymbol};
use wc_core::symbol::{Pin, PinDir, SymbolRole};

fn sym(id: &str, pin_y: f64) -> Symbol {
    Symbol {
        source: String::new(),
        id: id.into(),
        name: "Coil".into(),
        category: "relays".into(),
        letter_code: "K".into(),
        role: SymbolRole::Main,
        graphics: vec![],
        pins: vec![
            Pin {
                name: "A1".into(),
                at: Point::new(0.0, 0.0),
                dir: PinDir::Up,
            },
            Pin {
                name: "A2".into(),
                at: Point::new(0.0, pin_y),
                dir: PinDir::Down,
            },
        ],
        tag_at: Point::default(),
        description: String::new(),
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    proj: PathBuf,
    company: PathBuf,
    project: Project,
}

fn fixture() -> Result<Fixture> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let proj = tmp.path().join("proj");
    let company = tmp.path().join("company");
    let personal = tmp.path().join("personal");
    write_symbol(&company, &sym("coil", 10.0))?;
    write_symbol(&personal, &sym("coil", 99.0))?;
    write_symbol(&company, &sym("other", 5.0))?;
    let mut project = wc_core::store::create(&proj, "T")?;
    project.meta.libraries = vec![
        LibrarySource {
            name: "company".into(),
            kind: "folder".into(),
            location: company.to_string_lossy().into(),
            reference: None,
        },
        LibrarySource {
            name: "personal".into(),
            kind: "folder".into(),
            location: "../personal".into(),
            reference: None,
        },
    ];
    let mut page = Page::new("p2", "2", "Control");
    page.symbols.push(PlacedSymbol {
        id: "s1".into(),
        symbol: "coil".into(),
        at: Point::new(10.0, 10.0),
        rotation: Default::default(),
        mirror: false,
        tag: None,
    });
    project.pages.push(page);
    Ok(Fixture {
        _tmp: tmp,
        proj,
        company,
        project,
    })
}

#[test]
fn first_source_wins_and_relative_paths_resolve() -> Result<()> {
    let f = fixture()?;
    let lib = Library::open(&f.proj, &f.project)?;
    let (s, src) = lib.symbol("coil")?;
    assert_eq!(src, "company");
    assert_eq!(s.pins[1].at.y, 10.0);
    assert_eq!(lib.index(ItemKind::Symbol).len(), 2);
    Ok(())
}

#[test]
fn lock_then_detect_change_then_update() -> Result<()> {
    let mut f = fixture()?;
    let lib = Library::open(&f.proj, &f.project)?;
    let st = status(&f.proj, &f.project, &lib);
    assert_eq!(st[0].state, State::NotLocked);

    assert!(lock_missing(&f.proj, &mut f.project, &lib)?.is_empty());
    assert_eq!(status(&f.proj, &f.project, &lib)[0].state, State::UpToDate);

    // Someone edits the company library.
    write_symbol(&f.company, &sym("coil", 12.5))?;
    let st = status(&f.proj, &f.project, &lib);
    assert_eq!(st[0].state, State::Changed);
    assert_eq!(st[0].used_by, vec!["2"]);
    assert!(
        st[0].changes.iter().any(|c| c.contains("pin A2 moved")),
        "{:?}",
        st[0].changes
    );

    // The project keeps drawing its pinned copy until it updates.
    assert_eq!(
        wc_core::store::read_vendored_symbol(&f.proj, "coil")?.pins[1].at.y,
        10.0
    );
    let updated = update(&f.proj, &mut f.project, &lib, None)?;
    assert_eq!(updated, vec![(ItemKind::Symbol, "coil".to_string())]);
    assert_eq!(
        wc_core::store::read_vendored_symbol(&f.proj, "coil")?.pins[1].at.y,
        12.5
    );
    assert_eq!(status(&f.proj, &f.project, &lib)[0].state, State::UpToDate);
    Ok(())
}

#[test]
fn an_update_naming_an_id_the_project_does_not_have_takes_nothing()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut f = fixture()?;
    let lib = Library::open(&f.proj, &f.project)?;
    lock_missing(&f.proj, &mut f.project, &lib)?;
    write_symbol(&f.company, &sym("coil", 12.5))?;
    let pinned = f.project.lock.clone();

    // A typo beside a real update stops the whole update, naming the typo and what is known.
    let only = ["coil".to_owned(), "coill".to_owned()];
    let err = update(&f.proj, &mut f.project, &lib, Some(&only))
        .err()
        .ok_or("updated")?;
    assert!(matches!(err, Error::NotInProject { .. }), "{err:?}");
    let msg = err.to_string();
    assert!(msg.contains("does not use or pin \"coill\""), "{msg}");
    assert!(msg.contains("are \"coil\""), "{msg}");
    assert_eq!(f.project.lock, pinned);
    assert_eq!(
        wc_core::store::read_vendored_symbol(&f.proj, "coil")?.pins[1].at.y,
        10.0
    );

    // A library item the project does not use is no more an update than a typo.
    let only = ["other".to_owned()];
    assert!(update(&f.proj, &mut f.project, &lib, Some(&only)).is_err());
    let only = ["coil".to_owned()];
    assert_eq!(update(&f.proj, &mut f.project, &lib, Some(&only))?.len(), 1);
    Ok(())
}

#[test]
fn removed_items_keep_working_from_vendor() -> Result<()> {
    let mut f = fixture()?;
    let lib = Library::open(&f.proj, &f.project)?;
    lock_missing(&f.proj, &mut f.project, &lib)?;
    f.project.meta.libraries.clear();
    let lib = Library::open(&f.proj, &f.project)?;
    assert_eq!(status(&f.proj, &f.project, &lib)[0].state, State::GoneFromLibrary);
    assert!(wc_core::store::read_vendored_symbol(&f.proj, "coil").is_ok());
    Ok(())
}

#[test]
fn propose_writes_to_review_folder() -> Result<()> {
    let mut f = fixture()?;
    let lib = Library::open(&f.proj, &f.project)?;
    lock_missing(&f.proj, &mut f.project, &lib)?;
    let path = propose(&f.proj, &lib, "company", ItemKind::Symbol, "coil")?;
    assert!(path.ends_with("proposed/symbols/coil.json"));
    assert!(propose(&f.proj, &lib, "nope", ItemKind::Symbol, "coil").is_err());
    Ok(())
}

#[test]
fn search_finds_by_category() -> Result<()> {
    let f = fixture()?;
    let lib = Library::open(&f.proj, &f.project)?;
    assert_eq!(lib.search("RELAYS").len(), 2);
    assert!(lib.search("../etc").is_empty());
    assert!(lib.locate(ItemKind::Symbol, "../coil").is_none());
    Ok(())
}

#[test]
fn search_matches_every_word_in_any_order_and_the_letter_code() -> Result<()> {
    let f = fixture()?;
    let mut breaker = sym("breaker3", 10.0);
    breaker.name = "Circuit breaker, 3-pole".into();
    breaker.category = "protection".into();
    breaker.letter_code = "Q".into();
    write_symbol(&f.company, &breaker)?;
    let lib = Library::open(&f.proj, &f.project)?;
    let ids = |q: &str| lib.search(q).into_iter().map(|h| h.id).collect::<Vec<_>>();
    assert_eq!(ids("breaker 3-pole"), ["breaker3"]);
    assert_eq!(ids("3-POLE   Circuit"), ["breaker3"]);
    assert!(ids("breaker 4-pole").is_empty());
    assert_eq!(ids("q protection"), ["breaker3"]);
    assert_eq!(ids("k relays").len(), 2);
    assert_eq!(ids("").len(), 3);
    Ok(())
}

#[test]
fn macros_are_listed_per_source_in_order() -> Result<()> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let m = wc_core::macros::Macro {
        name: "Lamp".into(),
        graphics: vec![wc_core::Shape::Circle {
            center: wc_core::Point::new(0.0, 0.0),
            radius: 2.0,
            filled: false,
        }],
        ..Default::default()
    };
    let (a, b) = (tmp.path().join("company"), tmp.path().join("personal"));
    wc_core::macros::write_macro(&a, "lamp", &m)?;
    wc_core::macros::write_macro(&b, "lamp", &m)?;
    wc_core::macros::write_macro(&b, "alarm", &m)?;
    let lib = Library {
        sources: vec![
            Source {
                name: "company".into(),
                root: a,
            },
            Source {
                name: "personal".into(),
                root: b,
            },
        ],
    };
    let found: Vec<(String, String)> = lib.macros().into_iter().map(|(s, id, _)| (s, id)).collect();
    assert_eq!(
        found,
        [
            ("company".to_string(), "lamp".to_string()),
            ("personal".to_string(), "alarm".to_string()),
            ("personal".to_string(), "lamp".to_string())
        ]
    );
    Ok(())
}

#[test]
fn a_jumper_part_is_listed_as_used_by_its_jumper() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut f = fixture()?;
    let x1 = wc_core::DeviceTag::parse("-X1")?;
    f.project.meta.add_jumper(&x1, &["1".into(), "2".into()])?;
    f.project.meta.set_jumper_part(&x1, "1", Some("wago.2002-402"))?;
    let lib = Library::open(&f.proj, &f.project)?;
    let st = status(&f.proj, &f.project, &lib);
    let jumper = st
        .iter()
        .find(|s| s.kind == ItemKind::Part && s.id == "wago.2002-402")
        .ok_or_else(|| format!("the jumper's part is not listed: {st:?}"))?;
    assert_eq!(jumper.used_by, vec!["-X1 jumper 1-2"]);
    Ok(())
}

#[test]
fn a_library_file_declaring_another_id_is_not_taken() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut f = fixture()?;
    // `fake.json` in the library holds a symbol declaring `other`, with other contents than
    // the library's `other`: taking it would write over the project's copy of `other`.
    let fake = write_symbol(&f.company, &sym("other", 77.0))?;
    std::fs::rename(&fake, f.company.join("symbols").join("fake.json"))?;
    write_symbol(&f.company, &sym("other", 5.0))?;
    let lib = Library::open(&f.proj, &f.project)?;
    take(&f.proj, &mut f.project, &lib, ItemKind::Symbol, "other")?;
    let kept = std::fs::read(store::vendor_path(&f.proj, ItemKind::Symbol, "other")?)?;
    let err = take(&f.proj, &mut f.project, &lib, ItemKind::Symbol, "fake")
        .err()
        .ok_or("taken")?;
    assert!(err.to_string().contains("declares id \"other\""), "{err}");
    assert!(!f.project.lock.symbols.contains_key("fake"), "not pinned");
    assert!(is_wrong_id(&err), "classified as a wrong-id refusal: {err:?}");
    let missing = take(&f.proj, &mut f.project, &lib, ItemKind::Symbol, "nope")
        .err()
        .ok_or("taken")?;
    assert!(!is_wrong_id(&missing), "{missing:?}");
    assert_eq!(
        std::fs::read(store::vendor_path(&f.proj, ItemKind::Symbol, "other")?)?,
        kept
    );
    Ok(())
}

#[test]
fn a_library_part_file_declaring_another_id_is_not_taken() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut f = fixture()?;
    let part = |description: &str| Part {
        id: "other".into(),
        description: description.into(),
        ..Part::default()
    };
    // `fake.json` holds a part declaring `other`, with other contents than the library's `other`.
    let fake = write_part(&f.company, &part("fake"))?;
    std::fs::rename(&fake, f.company.join("parts").join("fake.json"))?;
    write_part(&f.company, &part("the real one"))?;
    let lib = Library::open(&f.proj, &f.project)?;
    take(&f.proj, &mut f.project, &lib, ItemKind::Part, "other")?;
    let kept = std::fs::read(store::vendor_path(&f.proj, ItemKind::Part, "other")?)?;
    let err = take(&f.proj, &mut f.project, &lib, ItemKind::Part, "fake")
        .err()
        .ok_or("taken")?;
    assert!(matches!(err, Error::Source { .. }), "{err:?}");
    assert!(err.to_string().contains("declares id \"other\""), "{err}");
    assert!(!f.project.lock.parts.contains_key("fake"), "not pinned");
    assert_eq!(
        std::fs::read(store::vendor_path(&f.proj, ItemKind::Part, "other")?)?,
        kept
    );
    Ok(())
}

#[test]
fn a_library_file_declaring_another_id_is_no_update_and_nothing_to_pin()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut f = fixture()?;
    let personal = f.proj.parent().ok_or("no parent")?.join("personal");
    // `amp` sorts before `coil`, is pinned, and gets a valid newer version below.
    write_symbol(&f.company, &sym("amp", 3.0))?;
    let lib = Library::open(&f.proj, &f.project)?;
    lock_missing(&f.proj, &mut f.project, &lib)?;
    take(&f.proj, &mut f.project, &lib, ItemKind::Symbol, "amp")?;
    let pinned = f.project.lock.symbols.get("coil").cloned().ok_or("not pinned")?;
    let amp_pinned = f.project.lock.symbols.get("amp").cloned().ok_or("amp not pinned")?;
    let kept = std::fs::read(store::vendor_path(&f.proj, ItemKind::Symbol, "coil")?)?;
    let amp_kept = std::fs::read(store::vendor_path(&f.proj, ItemKind::Symbol, "amp")?)?;
    write_symbol(&f.company, &sym("amp", 4.0))?;

    // Both sources' `coil.json` now hold a symbol declaring `other`, with other contents.
    for root in [&f.company, &personal] {
        let wrong = write_symbol(root, &sym("other", 42.0))?;
        std::fs::rename(&wrong, root.join("symbols").join("coil.json"))?;
    }
    write_symbol(&f.company, &sym("other", 5.0))?;
    let lib = Library::open(&f.proj, &f.project)?;
    let st = status(&f.proj, &f.project, &lib);
    let coil = st.iter().find(|s| s.id == "coil").ok_or("coil not listed")?;
    assert_eq!(coil.state, State::WrongId, "{coil:?}");
    assert!(coil.changes[0].contains("declares id \"other\""), "{:?}", coil.changes);
    assert!(coil.library_hash.is_none());

    // Naming it beside a valid update is an error before anything is taken.
    let err = update(
        &f.proj,
        &mut f.project,
        &lib,
        Some(&["amp".to_owned(), "coil".to_owned()]),
    )
    .err()
    .ok_or("updated")?;
    assert!(err.to_string().contains("declares id \"other\""), "{err}");
    assert_eq!(f.project.lock.symbols.get("amp"), Some(&amp_pinned));
    assert_eq!(
        std::fs::read(store::vendor_path(&f.proj, ItemKind::Symbol, "amp")?)?,
        amp_kept
    );
    // Updating everything takes the valid one and skips it.
    assert_eq!(
        update(&f.proj, &mut f.project, &lib, None)?,
        vec![(ItemKind::Symbol, "amp".to_owned())]
    );
    assert_ne!(f.project.lock.symbols.get("amp"), Some(&amp_pinned));
    assert_eq!(f.project.lock.symbols.get("coil"), Some(&pinned));
    assert_eq!(
        std::fs::read(store::vendor_path(&f.proj, ItemKind::Symbol, "coil")?)?,
        kept
    );

    // Not pinned yet: it is not pinned from the wrong file either, and nothing fails.
    f.project.lock.symbols.remove("coil");
    let st = status(&f.proj, &f.project, &lib);
    assert_eq!(
        st.iter().find(|s| s.id == "coil").map(|s| s.state),
        Some(State::WrongId)
    );
    let left = lock_missing(&f.proj, &mut f.project, &lib)?;
    assert!(!f.project.lock.symbols.contains_key("coil"));
    assert!(left.missing.is_empty(), "{left:?}");
    assert_eq!(left.wrong_id.len(), 1, "{left:?}");
    assert_eq!(
        (left.wrong_id[0].0, left.wrong_id[0].1.as_str()),
        (ItemKind::Symbol, "coil")
    );
    assert!(left.wrong_id[0].2.contains("declares id \"other\""), "{left:?}");
    Ok(())
}

#[test]
fn lock_missing_reports_a_file_that_turns_wrong_after_the_scan_and_pins_the_rest() -> Result<()> {
    let mut f = fixture()?;
    for id in ["aa", "bb", "cc"] {
        write_symbol(&f.company, &sym(id, 5.0))?;
        f.project.pages[0].symbols.push(PlacedSymbol {
            id: format!("s-{id}"),
            symbol: id.into(),
            at: Point::new(30.0, 10.0),
            rotation: Default::default(),
            mirror: false,
            tag: None,
        });
    }
    let lib = Library::open(&f.proj, &f.project)?;
    let bb = f.company.join("symbols").join("bb.json");
    let company = f.company.clone();
    let mut changed = Ok(());
    let left = lock_missing_then(&f.proj, &mut f.project, &lib, || {
        // After the scan saw `bb` as not pinned, its file starts declaring another id.
        changed = write_symbol(&company, &sym("zz", 7.0))
            .and_then(|zz| std::fs::rename(&zz, &bb).map_err(|e| wc_core::Error::io("rename", e).into()));
    })?;
    changed?;
    assert_eq!(left.wrong_id.len(), 1, "{left:?}");
    assert_eq!(
        (left.wrong_id[0].0, left.wrong_id[0].1.as_str()),
        (ItemKind::Symbol, "bb")
    );
    assert!(left.wrong_id[0].2.contains("declares id \"zz\""), "{left:?}");
    assert!(left.missing.is_empty(), "{left:?}");
    // Pinned before the refusal and after it; the refused one is not.
    for id in ["aa", "cc", "coil"] {
        assert!(f.project.lock.symbols.contains_key(id), "{id} pinned");
    }
    assert!(!f.project.lock.symbols.contains_key("bb"));
    Ok(())
}

#[test]
fn an_update_checks_every_item_before_it_writes_any() -> Result<()> {
    let mut f = fixture()?;
    // Both placed, so both are pinned.
    f.project.pages[0].symbols.push(PlacedSymbol {
        id: "s2".into(),
        symbol: "other".into(),
        at: Point::new(30.0, 10.0),
        rotation: Default::default(),
        mirror: false,
        tag: None,
    });
    let lib = Library::open(&f.proj, &f.project)?;
    assert!(lock_missing(&f.proj, &mut f.project, &lib)?.is_empty());
    let lock = f.project.lock.clone();
    write_symbol(&f.company, &sym("coil", 12.5))?;
    write_symbol(&f.company, &sym("other", 6.0))?;
    let all = status(&f.proj, &f.project, &lib);
    let todo = changed(&all, None);
    assert_eq!(todo.len(), 2);
    // The second one changes again after it was looked at: nothing is taken, not even the
    // first, so the files and the lock still agree.
    write_symbol(&f.company, &sym("other", 7.0))?;
    let r = update_items(&f.proj, &mut f.project, &lib, &todo);
    assert!(r.is_err_and(|e| {
        e.to_string()
            .contains("changed in the library while it was being updated")
    }));
    assert_eq!(f.project.lock, lock);
    assert_eq!(
        wc_core::store::read_vendored_symbol(&f.proj, "coil")?.pins[1].at.y,
        10.0
    );
    assert_eq!(
        wc_core::store::read_vendored_symbol(&f.proj, "other")?.pins[1].at.y,
        5.0
    );

    // A file that cannot be read stops the update before anything is written: it could not
    // be put back after a later failure.
    let todo = changed(&status(&f.proj, &f.project, &lib), None);
    let other = wc_core::store::vendor_path(&f.proj, ItemKind::Symbol, "other")?;
    let kept = std::fs::read(&other).map_err(|e| wc_core::Error::io("other", e))?;
    std::fs::remove_file(&other).map_err(|e| wc_core::Error::io("other", e))?;
    std::fs::create_dir(&other).map_err(|e| wc_core::Error::io("other", e))?;
    assert!(update_items(&f.proj, &mut f.project, &lib, &todo).is_err());
    assert_eq!(f.project.lock, lock);
    assert_eq!(
        wc_core::store::read_vendored_symbol(&f.proj, "coil")?.pins[1].at.y,
        10.0
    );
    std::fs::remove_dir(&other).map_err(|e| wc_core::Error::io("other", e))?;
    std::fs::write(&other, kept).map_err(|e| wc_core::Error::io("other", e))?;

    // A write that fails puts back what was written before it and leaves the lock as it was.
    let todo = changed(&status(&f.proj, &f.project, &lib), None);
    let other = wc_core::store::vendor_path(&f.proj, ItemKind::Symbol, "other")?;
    let mut tmp = other.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::create_dir_all(PathBuf::from(&tmp)).map_err(|e| wc_core::Error::io("tmp", e))?;
    assert!(update_items(&f.proj, &mut f.project, &lib, &todo).is_err());
    assert_eq!(f.project.lock, lock);
    assert_eq!(
        wc_core::store::read_vendored_symbol(&f.proj, "coil")?.pins[1].at.y,
        10.0
    );
    std::fs::remove_dir(PathBuf::from(&tmp)).map_err(|e| wc_core::Error::io("tmp", e))?;
    assert_eq!(update_items(&f.proj, &mut f.project, &lib, &todo)?.len(), 2);
    assert_eq!(
        wc_core::store::read_vendored_symbol(&f.proj, "other")?.pins[1].at.y,
        7.0
    );
    Ok(())
}

#[test]
fn a_folder_source_with_a_rules_file_gives_its_rules_in_source_order() -> Result<()> {
    let f = fixture()?;
    let io = |e: std::io::Error| wc_core::Error::io("rules", e);
    // No rules files: nothing.
    assert!(library_rules(&f.proj, &f.project).is_empty());
    std::fs::write(
        f.company.join("rules.toml"),
        "[[rule]]\nname = \"motor-cable\"\nkind = \"cable\"\ndevices = \"-M*\"\n",
    )
    .map_err(io)?;
    let personal = f.proj.join("../personal");
    std::fs::write(personal.join("rules.toml"), "[[rule]\n").map_err(io)?;
    let sets = library_rules(&f.proj, &f.project);
    assert_eq!(sets.len(), 2);
    assert_eq!(sets[0].source, "company");
    let rules = sets[0].rules.as_ref().map_err(|e| wc_core::Error::Invalid(e.clone()))?;
    assert_eq!(rules[0].name.as_deref(), Some("motor-cable"));
    assert_eq!(sets[1].source, "personal");
    assert!(sets[1].rules.is_err(), "a file that does not parse says so");

    // A source that cannot be resolved says why, under its name, rather than vanishing.
    let mut project = f.project.clone();
    project.meta.libraries.push(wc_core::project::LibrarySource {
        name: "odd".into(),
        kind: "zip".into(),
        location: "x".into(),
        reference: None,
    });
    let sets = library_rules(&f.proj, &project);
    assert_eq!(sets.len(), 3);
    assert_eq!(sets[2].source, "odd");
    let why = sets[2].rules.as_ref().err().cloned().unwrap_or_default();
    assert!(why.contains("unknown kind"), "{why}");
    Ok(())
}

/// A temporary project whose only library source is the starter library in `library/`.
/// The temp dir is returned too, so it lives as long as the test holds it.
fn starter_library() -> Result<(tempfile::TempDir, Library)> {
    let tmp = tempfile::tempdir().map_err(|e| wc_core::Error::io("tmp", e))?;
    let starter = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library");
    let mut project = wc_core::store::create(&tmp.path().join("p"), "T")?;
    project.meta.libraries = vec![LibrarySource {
        name: "starter".into(),
        kind: "folder".into(),
        location: starter.to_string_lossy().into(),
        reference: None,
    }];
    let lib = Library::open(&tmp.path().join("p"), &project)?;
    Ok((tmp, lib))
}

#[test]
fn every_starter_symbol_loads_under_its_file_name_with_distinct_pins() -> Result<()> {
    let (_tmp, lib) = starter_library()?;
    let ids = lib.index(ItemKind::Symbol);
    for id in ids.keys() {
        let (s, _) = lib.symbol(id)?;
        assert_eq!(&s.id, id, "file name and id differ");
        assert!(!s.pins.is_empty(), "{id} has no pins");
        // Shown when browsing and matched by library search (#684).
        assert!(!s.description.trim().is_empty(), "{id} has no description");
        let names: std::collections::BTreeSet<_> = s.pins.iter().map(|p| &p.name).collect();
        assert_eq!(names.len(), s.pins.len(), "{id} repeats a pin name");
    }
    // The capacitor (#723): its own tag, letter code C (IEC 81346-2), pins 1 and 2.
    let (capacitor, _) = lib.symbol("wc.capacitor")?;
    assert_eq!(capacitor.role, SymbolRole::Standalone);
    assert_eq!(capacitor.letter_code, "C");
    let names: Vec<_> = capacitor.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["1", "2"]);
    // The potentiometer (#881): letter code R, ends 1 and 3, the wiper 2 off to the side.
    let (pot, _) = lib.symbol("wc.potentiometer")?;
    assert_eq!(pot.role, SymbolRole::Standalone);
    assert_eq!(pot.letter_code, "R");
    let names: Vec<_> = pot.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["1", "2", "3"]);
    let [end1, wiper, end3] = [0, 1, 2].map(|i| pot.pins[i].at);
    assert_eq!(end1.x, end3.x, "pins 1 and 3 share a vertical line");
    assert_ne!(wiper.x, end1.x, "the wiper sits off the line between 1 and 3");
    // Plug and socket (#880): one contact each, letter code X, the socket wired from the top
    // and the plug from the bottom so a plug drawn under a socket mates with it.
    for (id, dir) in [
        ("wc.connector.socket", wc_core::symbol::PinDir::Up),
        ("wc.connector.plug", wc_core::symbol::PinDir::Down),
    ] {
        let (s, _) = lib.symbol(id)?;
        assert_eq!(s.role, SymbolRole::Standalone, "{id}");
        assert_eq!(s.letter_code, "X", "{id}");
        assert_eq!(s.category, "terminals", "{id}");
        let names: Vec<_> = s.pins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["1"], "{id}");
        assert_eq!(s.pins[0].dir, dir, "{id}");
    }
    // The timer relay set (#531): coils drive, delayed contacts follow them.
    for (id, role) in [
        ("wc.coil.ondelay", SymbolRole::Main),
        ("wc.coil.offdelay", SymbolRole::Main),
        ("wc.contact.no.ondelay", SymbolRole::Auxiliary),
        ("wc.contact.nc.ondelay", SymbolRole::Auxiliary),
        ("wc.contact.no.offdelay", SymbolRole::Auxiliary),
        ("wc.contact.nc.offdelay", SymbolRole::Auxiliary),
    ] {
        assert_eq!(lib.symbol(id)?.0.role, role, "{id}");
    }
    // The main switch (#680): a 3-pole switch with its own tag, and an example part using it.
    let (main_switch, _) = lib.symbol("wc.switch.disconnector.3p")?;
    assert_eq!(main_switch.role, SymbolRole::Standalone);
    assert_eq!(main_switch.letter_code, "Q");
    let names: Vec<_> = main_switch.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["1", "2", "3", "4", "5", "6"]);
    assert_eq!(
        lib.part("example.ex-ms-3p-40")?.0.symbol.as_deref(),
        Some("wc.switch.disconnector.3p")
    );
    // The diode (#722): its own tag, anode on top and cathode below.
    let (diode, _) = lib.symbol("wc.diode")?;
    assert_eq!(diode.role, SymbolRole::Standalone);
    assert_eq!(diode.letter_code, "R");
    let names: Vec<_> = diode.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["A", "K"]);
    Ok(())
}

#[test]
fn every_starter_part_loads_under_its_file_name_and_names_a_starter_symbol() -> Result<()> {
    let (_tmp, lib) = starter_library()?;
    let symbols = lib.index(ItemKind::Symbol);
    let parts = lib.index(ItemKind::Part);
    assert!(!parts.is_empty(), "the starter library has no parts");
    for id in parts.keys() {
        let (p, _) = lib.part(id)?;
        assert_eq!(&p.id, id, "file name and id differ");
        assert!(!p.description.trim().is_empty(), "{id} has no description");
        if let Some(symbol) = &p.symbol {
            assert!(
                symbols.contains_key(symbol),
                "{id} names symbol {symbol}, which is not in the starter library"
            );
        }
    }
    Ok(())
}

#[test]
fn starter_solenoid_valve_coil_and_its_example_part_load() -> Result<()> {
    let (_tmp, lib) = starter_library()?;
    // The solenoid valve coil (#724): its own tag (no contacts follow it), letter code M for an
    // actuator in IEC 81346-2, and the coil pins of a relay coil.
    let (valve, _) = lib.symbol("wc.coil.valve")?;
    assert_eq!(valve.role, SymbolRole::Standalone);
    assert_eq!(valve.letter_code, "M");
    let names: Vec<_> = valve.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["A1", "A2"]);
    assert_eq!(
        lib.part("example.ex-sv-24dc")?.0.symbol.as_deref(),
        Some("wc.coil.valve")
    );
    Ok(())
}

#[test]
fn starter_photoelectric_sensor_and_its_example_part_load() -> Result<()> {
    let (_tmp, lib) = starter_library()?;
    // The photoelectric sensor (#756): letter code B for a sensor in IEC 81346-2, and the same
    // three wires as the 3-wire proximity sensors.
    let (photo, _) = lib.symbol("wc.sensor.photo3")?;
    assert_eq!(photo.role, SymbolRole::Standalone);
    assert_eq!(photo.letter_code, "B");
    let names: Vec<_> = photo.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["BN", "BK", "BU"]);
    assert_eq!(
        lib.part("example.ex-ps-m18-pnp")?.0.symbol.as_deref(),
        Some("wc.sensor.photo3")
    );
    Ok(())
}

#[test]
fn the_starter_key_switch_is_a_make_contact_tagged_s() -> Result<()> {
    // #767: a key-operated switch next to the push buttons and selector switch.
    let (_tmp, lib) = starter_library()?;
    let (s, _) = lib.symbol("wc.switch.key")?;
    assert_eq!(s.role, SymbolRole::Standalone);
    assert_eq!(s.letter_code, "S");
    let names: Vec<_> = s.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["13", "14"]);
    Ok(())
}

#[test]
fn the_starter_current_transformer_has_primary_and_secondary_terminals() -> Result<()> {
    // #769: a current transformer for panel ammeters and energy meters.
    let (_tmp, lib) = starter_library()?;
    let (ct, _) = lib.symbol("wc.transformer.current")?;
    assert_eq!(ct.role, SymbolRole::Standalone);
    assert_eq!(ct.letter_code, "T");
    let names: Vec<_> = ct.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["P1", "P2", "S1", "S2"]);
    Ok(())
}

#[test]
fn the_starter_functional_earth_is_not_the_protective_earth() -> Result<()> {
    // #897: a plain (functional) earth for signal grounds, 0 V and screens.
    let (_tmp, lib) = starter_library()?;
    let (fe, _) = lib.symbol("wc.fe")?;
    let (pe, _) = lib.symbol("wc.pe")?;
    assert_eq!(fe.id, "wc.fe");
    assert_eq!(fe.category, "power");
    assert_eq!(fe.role, SymbolRole::Standalone);
    let names: Vec<_> = fe.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["FE"]);
    assert!(!fe.description.is_empty());
    assert_ne!(fe.description, pe.description);
    Ok(())
}

#[test]
fn the_starter_three_phase_transformer_has_primary_and_secondary_terminals() -> Result<()> {
    // #763: a 3-phase transformer next to the 1-phase control transformer.
    let (_tmp, lib) = starter_library()?;
    let (t, _) = lib.symbol("wc.transformer.3ph")?;
    assert_eq!(t.role, SymbolRole::Standalone);
    assert_eq!(t.letter_code, "T");
    let names: Vec<_> = t.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["1U", "1V", "1W", "2U", "2V", "2W"]);
    Ok(())
}

#[test]
fn the_starter_fuse_switch_disconnector_has_three_poles_and_its_own_tag() -> Result<()> {
    // The 3-pole fuse switch disconnector (#725): Q in IEC 81346-2 (it switches and isolates;
    // the fuses ride on the switch), pins 1 to 6 like the 3-pole breaker, and an example part.
    let (_tmp, lib) = starter_library()?;
    let (fsd, _) = lib.symbol("wc.fuse.3p")?;
    assert_eq!(fsd.role, SymbolRole::Standalone);
    assert_eq!(fsd.letter_code, "Q");
    let names: Vec<_> = fsd.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["1", "2", "3", "4", "5", "6"]);
    assert_eq!(
        lib.part("example.ex-fsd-3p-32")?.0.symbol.as_deref(),
        Some("wc.fuse.3p")
    );
    Ok(())
}

#[test]
fn lock_restores_a_deleted_vendored_copy_at_the_pinned_version() -> Result<()> {
    let mut f = fixture()?;
    let lib = Library::open(&f.proj, &f.project)?;
    lock_missing(&f.proj, &mut f.project, &lib)?;
    let pinned = f.project.lock.get(ItemKind::Symbol, "coil").cloned();
    let path = wc_core::store::vendor_path(&f.proj, ItemKind::Symbol, "coil")?;
    std::fs::remove_file(&path).map_err(|e| wc_core::Error::io("rm", e))?;

    let left = lock_missing(&f.proj, &mut f.project, &lib)?;
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(left.restored, vec![(ItemKind::Symbol, "coil".to_string())]);
    assert_eq!(
        wc_core::store::read_vendored_symbol(&f.proj, "coil")?.pins[1].at.y,
        10.0
    );
    assert_eq!(f.project.lock.get(ItemKind::Symbol, "coil").cloned(), pinned);

    // A readable copy is left alone.
    assert!(lock_missing(&f.proj, &mut f.project, &lib)?.restored.is_empty());
    Ok(())
}

#[test]
fn lock_restores_the_pinned_version_from_another_source_and_keeps_the_pin() -> Result<()> {
    let mut f = fixture()?;
    let lib = Library::open(&f.proj, &f.project)?;
    lock_missing(&f.proj, &mut f.project, &lib)?;
    let pinned = f.project.lock.get(ItemKind::Symbol, "coil").cloned();
    let path = wc_core::store::vendor_path(&f.proj, ItemKind::Symbol, "coil")?;
    std::fs::write(&path, "not json").map_err(|e| wc_core::Error::io("write", e))?;

    // The pinned source moved on; another source holds the pinned version.
    write_symbol(&f.company, &sym("coil", 12.5))?;
    let personal = f.proj.join("../personal");
    write_symbol(&personal, &sym("coil", 10.0))?;
    let left = lock_missing(&f.proj, &mut f.project, &lib)?;
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(left.restored, vec![(ItemKind::Symbol, "coil".to_string())]);
    assert_eq!(
        wc_core::store::read_vendored_symbol(&f.proj, "coil")?.pins[1].at.y,
        10.0
    );
    assert_eq!(
        f.project.lock.get(ItemKind::Symbol, "coil").cloned(),
        pinned,
        "the pin is unchanged"
    );
    Ok(())
}

#[test]
fn lock_reports_a_failed_write_of_the_pinned_version_as_a_write_failure() -> Result<()> {
    let mut f = fixture()?;
    let lib = Library::open(&f.proj, &f.project)?;
    lock_missing(&f.proj, &mut f.project, &lib)?;
    let path = wc_core::store::vendor_path(&f.proj, ItemKind::Symbol, "coil")?;
    // A folder where the vendored file belongs: unreadable, and not replaced by a write.
    std::fs::remove_file(&path).map_err(|e| wc_core::Error::io("rm", e))?;
    std::fs::create_dir_all(path.join("x")).map_err(|e| wc_core::Error::io("mkdir", e))?;

    let left = lock_missing(&f.proj, &mut f.project, &lib)?;
    assert!(left.restored.is_empty(), "{left:?}");
    assert_eq!(left.unrestorable.len(), 1, "{left:?}");
    let why = &left.unrestorable[0].2;
    assert!(why.contains("writing it failed"), "{why}");
    assert!(why.contains("coil.json"), "the failed file is named: {why}");
    assert!(!why.contains("no library source"), "{why}");
    Ok(())
}

#[test]
fn lock_does_not_restore_another_version_or_an_item_gone_from_the_library() -> Result<()> {
    let mut f = fixture()?;
    let lib = Library::open(&f.proj, &f.project)?;
    lock_missing(&f.proj, &mut f.project, &lib)?;
    let pinned = f.project.lock.get(ItemKind::Symbol, "coil").cloned();
    let path = wc_core::store::vendor_path(&f.proj, ItemKind::Symbol, "coil")?;
    std::fs::write(&path, "not json").map_err(|e| wc_core::Error::io("write", e))?;

    // No source holds the pinned version any more: nothing is taken, and the reason and the
    // way out are given.
    write_symbol(&f.company, &sym("coil", 12.5))?;
    let left = lock_missing(&f.proj, &mut f.project, &lib)?;
    assert!(left.restored.is_empty());
    assert_eq!(left.unrestorable.len(), 1, "{left:?}");
    let why = &left.unrestorable[0].2;
    assert!(why.contains("another version"), "{why}");
    assert!(why.contains("project's history"), "{why}");
    assert!(
        wc_core::store::read_vendored_symbol(&f.proj, "coil").is_err(),
        "nothing taken"
    );
    assert_eq!(f.project.lock.get(ItemKind::Symbol, "coil").cloned(), pinned);

    f.project.meta.libraries.clear();
    let lib = Library::open(&f.proj, &f.project)?;
    let left = lock_missing(&f.proj, &mut f.project, &lib)?;
    assert_eq!(left.unrestorable.len(), 1, "{left:?}");
    assert!(left.unrestorable[0].2.contains("no library has it"), "{left:?}");
    Ok(())
}

#[test]
fn the_starter_ammeter_and_voltmeter_are_measuring_instruments() -> Result<()> {
    // #765: panel ammeters and voltmeters on supply and motor circuits.
    let (_tmp, lib) = starter_library()?;
    for id in ["wc.meter.ammeter", "wc.meter.voltmeter"] {
        let (m, _) = lib.symbol(id)?;
        assert_eq!(m.role, SymbolRole::Standalone, "{id}");
        assert_eq!(m.letter_code, "P", "{id}");
        assert_eq!(m.category, "measuring", "{id}");
        let names: Vec<_> = m.pins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["1", "2"], "{id}");
    }
    Ok(())
}

#[test]
fn the_starter_inductor_has_two_pins() -> Result<()> {
    // #770: an inductor for line and DC chokes on drives and in filters.
    let (_tmp, lib) = starter_library()?;
    let (l, _) = lib.symbol("wc.inductor")?;
    assert_eq!(l.role, SymbolRole::Standalone);
    assert_eq!(l.letter_code, "R");
    assert_eq!(l.category, "passive");
    let names: Vec<_> = l.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["1", "2"]);
    Ok(())
}

#[test]
fn starter_two_wire_proximity_sensor_and_its_example_part_load() -> Result<()> {
    let (_tmp, lib) = starter_library()?;
    // The 2-wire proximity sensor (#757): letter code B for a sensor in IEC 81346-2, one wire
    // in and one out, so it sits in series with the load like a switch.
    let (prox, _) = lib.symbol("wc.sensor.prox2")?;
    assert_eq!(prox.role, SymbolRole::Standalone);
    assert_eq!(prox.letter_code, "B");
    assert_eq!(prox.category, "sensors");
    let names: Vec<_> = prox.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["BN", "BU"]);
    assert_eq!(
        lib.part("example.ex-px-m12-2w")?.0.symbol.as_deref(),
        Some("wc.sensor.prox2")
    );
    Ok(())
}

#[test]
fn the_starter_energy_meter_is_a_measuring_instrument_on_the_grid() -> Result<()> {
    // #930: a single-phase kWh meter, L and N in at the top and out at the bottom.
    let (_tmp, lib) = starter_library()?;
    let (m, _) = lib.symbol("wc.meter.energy")?;
    assert_eq!(m.id, "wc.meter.energy");
    assert_eq!(m.role, SymbolRole::Standalone);
    assert_eq!(m.letter_code, "P");
    assert_eq!(m.category, "measuring");
    let names: Vec<_> = m.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["1", "3", "2", "4"]);
    for p in &m.pins {
        assert_eq!(p.at.x % 5.0, 0.0, "pin {} is off the 5 mm grid", p.name);
        assert_eq!(p.at.y % 5.0, 0.0, "pin {} is off the 5 mm grid", p.name);
    }
    Ok(())
}

#[test]
fn the_starter_dc_motor_has_plus_and_minus_pins() -> Result<()> {
    // #882: a DC motor next to the single- and three-phase ones.
    let (_tmp, lib) = starter_library()?;
    let (m, _) = lib.symbol("wc.motor.dc")?;
    assert_eq!(m.id, "wc.motor.dc");
    assert_eq!(m.role, SymbolRole::Standalone);
    assert_eq!(m.letter_code, "M");
    assert_eq!(m.category, "drives");
    let names: Vec<_> = m.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["+", "-"]);
    // Origin at the + pin, - one 5 mm grid step to its right (as wc.rcd.2p).
    assert_eq!((m.pins[0].at.x, m.pins[0].at.y), (0.0, 0.0));
    assert_eq!((m.pins[1].at.x, m.pins[1].at.y), (5.0, 0.0));
    Ok(())
}

#[test]
fn the_starter_flow_switch_is_a_make_contact_tagged_b() -> Result<()> {
    // #896: a flow switch next to the pressure, float and temperature switches.
    let (_tmp, lib) = starter_library()?;
    let (s, _) = lib.symbol("wc.switch.flow")?;
    assert_eq!(s.id, "wc.switch.flow");
    assert_eq!(s.role, SymbolRole::Standalone);
    assert_eq!(s.letter_code, "B");
    assert_eq!(s.category, "sensors");
    let names: Vec<_> = s.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["13", "14"]);
    // On the 5 mm grid: 13 at the origin, 14 three grid steps below it.
    assert_eq!((s.pins[0].at.x, s.pins[0].at.y), (0.0, 0.0));
    assert_eq!((s.pins[1].at.x, s.pins[1].at.y), (0.0, 15.0));
    Ok(())
}

#[test]
fn the_starter_battery_has_plus_and_minus_pins() -> Result<()> {
    // #764: a battery or cell for backup supplies and buffered 24 V DC circuits.
    let (_tmp, lib) = starter_library()?;
    let (b, _) = lib.symbol("wc.battery")?;
    assert_eq!(b.role, SymbolRole::Standalone);
    assert_eq!(b.letter_code, "G");
    assert_eq!(b.category, "power");
    let names: Vec<_> = b.pins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["+", "-"]);
    Ok(())
}

#[test]
fn the_starter_changeover_contact_has_its_pins_on_the_5_mm_grid() -> Result<()> {
    // #1148: pins 12 and 14 sat 4 mm to the sides, so a wire drawn on the grid missed them.
    let (_tmp, lib) = starter_library()?;
    let (s, _) = lib.symbol("wc.contact.co")?;
    let pins: Vec<_> = s.pins.iter().map(|p| (p.name.as_str(), p.at.x, p.at.y)).collect();
    assert_eq!(pins, [("11", 0.0, 0.0), ("12", -5.0, 15.0), ("14", 5.0, 15.0)]);
    Ok(())
}

#[test]
fn every_starter_pin_sits_on_the_5_mm_grid() -> Result<()> {
    // #1149: pins off the grid cannot be reached by a wire drawn on it.
    let on_grid = |v: f64| ((v / 5.0).round() * 5.0 - v).abs() < 1e-9;
    let (_tmp, lib) = starter_library()?;
    let mut off = Vec::new();
    for id in lib.index(ItemKind::Symbol).keys() {
        let (s, _) = lib.symbol(id)?;
        for p in &s.pins {
            if !on_grid(p.at.x) || !on_grid(p.at.y) {
                off.push(format!("{id} {} at ({}, {})", p.name, p.at.x, p.at.y));
            }
        }
    }
    assert!(off.is_empty(), "pins off the 5 mm grid: {off:?}");
    Ok(())
}

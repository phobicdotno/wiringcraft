use super::*;
use wc_core::geom::Point;
use wc_core::project::{LibrarySource, Page, PlacedSymbol};
use wc_core::symbol::{Pin, PinDir, SymbolRole};

fn sym(id: &str, pin_y: f64) -> Symbol {
    Symbol {
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
    assert!(is_wrong_id(&err), "lock_missing reports it and goes on: {err:?}");
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

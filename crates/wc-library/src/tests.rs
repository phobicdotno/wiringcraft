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

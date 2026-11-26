use std::cell::RefCell;
use std::rc::Rc;

use wc_core::{Comment, Device, DeviceTag};
use wc_share::transport::pair;
use wc_share::{Change, Journal};

use super::*;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A journal the test can look at.
#[derive(Clone, Default)]
pub(crate) struct Log(
    Rc<RefCell<Vec<Change>>>,
    Rc<std::cell::Cell<bool>>,
    Rc<std::cell::Cell<bool>>,
);

impl Journal for Log {
    /// Fails for a guest's change while [`Log::fail_guests`] is set.
    fn record(&mut self, _project: &Project, change: &Change) -> wc_share::Result<()> {
        if self.2.get() && change.by != wc_share::HOST {
            return Err(std::io::Error::other("the history is read-only").into());
        }
        self.0.borrow_mut().push(change.clone());
        Ok(())
    }

    /// Fails while [`Log::fail_vendor`] is set, as a host whose vendor folder is read-only.
    fn vendor(&mut self, _symbols: &[Symbol], _parts: &[Part]) -> wc_share::Result<()> {
        if self.1.get() {
            return Err(std::io::Error::other("vendor folder is read-only").into());
        }
        Ok(())
    }
}

impl Log {
    fn fail_vendor(&self, fail: bool) {
        self.1.set(fail);
    }

    fn fail_guests(&self, fail: bool) {
        self.2.set(fail);
    }

    fn messages(&self) -> Vec<String> {
        self.0.borrow().iter().map(|c| c.message.clone()).collect()
    }
}

/// A project folder with a cover and two pages and the starter library, opened as the app
/// opens it.
fn project(dir: &Path) -> std::result::Result<Open, Box<dyn std::error::Error>> {
    let mut p = store::create(dir, "Pump station")?;
    // The starter symbols, so tests can place symbols.
    let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library");
    p.meta.libraries.push(wc_core::project::LibrarySource {
        name: "starter".into(),
        kind: "folder".into(),
        location: lib.display().to_string(),
        reference: None,
    });
    p.pages.push(Page::new("p0002", "2", "Motor"));
    p.pages.push(Page::new("p0003", "3", "Valves"));
    store::save(dir, &p)?;
    Ok(Open::load(dir)?)
}

/// Share the open project in-process (no network), with a journal the test reads.
fn host(open: &Open) -> (Share, Log) {
    let log = Log::default();
    (host_with(open, Box::new(log.clone())), log)
}

/// A host of `open` that records into `journal`, set up as `Share::start_host` does.
fn host_with(open: &Open, journal: Box<dyn Journal>) -> Share {
    let mut host = Host::new(open.project.clone(), "Hanna", "hanna@example.com", journal);
    let files = vendored_files(&open.dir, &open.project);
    host.share_files(files.symbols, files.parts);
    host.share_pictures(wc_pdf::read_pictures(&open.dir, &open.project).files);
    Share {
        role: Some(Role::Host(Box::new(Hosting {
            host,
            hub: Hub::new(),
            listener: None,
            announcer: None,
            port: 0,
            dir: open.dir.clone(),
            took_updates: Vec::new(),
            names_taken: Vec::new(),
        }))),
        base: Some(open.project.clone()),
        ..Default::default()
    }
}

/// Connect a guest to the host in-process and ask to join; the copy goes under `folder`.
fn guest(host: &mut Share, name: &str, folder: &Path) -> Share {
    let (guest_end, host_end) = pair();
    match host.role.as_mut() {
        Some(Role::Host(h)) => {
            h.hub.add(&mut h.host, host_end);
        }
        _ => unreachable!("the test shares first"),
    }
    let mut g = Guest::new();
    assert!(guest_end.send(g.join(name, &format!("{name}@example.com"))).is_ok());
    Share {
        role: Some(Role::Guest(Box::new(Joining {
            guest: g,
            link: guest_end,
            addr: "in-process".into(),
            folder: folder.to_path_buf(),
            dir: None,
        }))),
        ..Default::default()
    }
}

fn pending(host: &Share) -> Vec<PeerId> {
    match &host.role {
        Some(Role::Host(h)) => h.host.pending().into_iter().map(|(id, ..)| id).collect(),
        _ => Vec::new(),
    }
}

fn approve(host: &mut Share, id: PeerId, yes: bool) {
    if let Some(Role::Host(h)) = host.role.as_mut() {
        if yes {
            assert!(h.host.approve(id));
        } else {
            assert!(h.host.decline(id));
        }
        h.hub.flush(&mut h.host);
    }
}

/// Run a few frames of both windows: each shows `page`, nothing is held down.
pub(crate) fn frames(a: (&mut Share, &mut Open, &str), b: (&mut Share, &mut Open, &str)) {
    for _ in 0..4 {
        a.0.frame(Some(a.1), Some(a.2.to_owned()), false);
        b.0.frame(Some(b.1), Some(b.2.to_owned()), false);
    }
}

/// The host's share, its journal and project, then the guest's share and project.
pub(crate) type Session = (Share, Log, Open, Share, Open);

/// Host and guest, joined, each in its own folder.
pub(crate) fn session(tmp: &Path) -> std::result::Result<Session, Box<dyn std::error::Error>> {
    session_with(tmp, |_| Ok(()))
}

/// Like [`session`], with `setup` run on the host's project before it is shared.
fn session_with(
    tmp: &Path,
    setup: impl FnOnce(&mut Open) -> std::result::Result<(), Box<dyn std::error::Error>>,
) -> std::result::Result<Session, Box<dyn std::error::Error>> {
    session_sharing(tmp, setup, BTreeMap::new())
}

/// Like [`session_with`], with `pictures` shared on top of the ones the host's project uses,
/// before the guest is let in.
fn session_sharing(
    tmp: &Path,
    setup: impl FnOnce(&mut Open) -> std::result::Result<(), Box<dyn std::error::Error>>,
    pictures: BTreeMap<String, Vec<u8>>,
) -> std::result::Result<Session, Box<dyn std::error::Error>> {
    let mut host_open = project(&tmp.join("host"))?;
    setup(&mut host_open)?;
    host_open.save()?;
    let (mut h, log) = host(&host_open);
    if let Some(Role::Host(hosting)) = h.role.as_mut()
        && !pictures.is_empty()
    {
        let mut all = hosting.host.pictures().clone();
        all.extend(pictures);
        hosting.host.share_pictures(all);
    }
    let mut g = guest(&mut h, "Ola", &tmp.join("guest"));
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    let ids = pending(&h);
    assert_eq!(ids.len(), 1, "the join request waits for the host");
    approve(&mut h, ids[0], true);
    let guest_open = g.frame(None, None, false).ok_or("the guest got no project")?;
    Ok((h, log, host_open, g, guest_open))
}

#[test]
fn an_edit_from_the_session_is_seen_by_checks_while_drawing() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // The host renames page 2; the guest's copy takes it in without any input there.
    let (guest_was, host_was) = (guest_open.revision(), host_open.revision());
    host_open.project.page_mut("p0002").ok_or("no page")?.title = "Renamed".into();
    host_open.touch();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(
        guest_open.project.page("p0002").map(|p| p.title.as_str()),
        Some("Renamed")
    );
    assert_ne!(
        guest_open.revision(),
        guest_was,
        "the guest's checks see the host's edit"
    );
    // And the other way round.
    let host_now = host_open.revision();
    assert_ne!(host_now, host_was);
    guest_open.project.page_mut("p0003").ok_or("no page")?.title = "Mine".into();
    guest_open.touch();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(host_open.project.page("p0003").map(|p| p.title.as_str()), Some("Mine"));
    assert_ne!(host_open.revision(), host_now, "the host's checks see the guest's edit");
    Ok(())
}

#[test]
fn a_joined_guest_gets_its_own_copy_on_disk() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (_h, _log, host_open, _g, guest_open) = session(tmp.path())?;
    assert_eq!(guest_open.dir, tmp.path().join("guest").join("Pump station"));
    assert_eq!(guest_open.project.pages, host_open.project.pages);
    assert_eq!(store::load(&guest_open.dir)?.pages, host_open.project.pages);
    Ok(())
}

#[test]
fn a_guest_on_another_version_is_told_both_versions_instead_of_a_dropped_connection() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut host_open = project(&tmp.path().join("host"))?;
    let (mut h, log) = host(&host_open);
    let (guest_end, host_end) = pair();
    if let Some(Role::Host(hosting)) = h.role.as_mut() {
        hosting.hub.add(&mut hosting.host, host_end);
    }
    let other = wc_share::PROTOCOL_VERSION + 1;
    // The status window is open after "Ask to join", with what the person typed.
    let typed = Form {
        name: "Kari".into(),
        email: "kari@example.com".into(),
        addr: "192.168.1.20:47810".into(),
        folder: crate::prefs::FolderField::typed(tmp.path().join("guest").display().to_string()),
        ..Form::default()
    };
    let mut g = Share {
        role: Some(Role::Guest(Box::new(Joining {
            guest: Guest::new(),
            link: guest_end,
            addr: "in-process".into(),
            folder: tmp.path().join("guest"),
            dir: None,
        }))),
        view: View::Status,
        form: Some(typed),
        ..Default::default()
    };
    if let Some(Role::Guest(j)) = g.role.as_mut() {
        let _ = j.guest.join("Kari", "kari@example.com");
        let join = wc_share::GuestMsg::Join {
            name: "Kari".into(),
            email: "kari@example.com".into(),
            protocol: other,
        };
        assert!(j.link.send(join).is_ok());
    }
    let before = host_open.project.clone();
    h.frame(Some(&mut host_open), None, false);
    assert!(pending(&h).is_empty(), "the host is not asked");
    assert!(g.frame(None, None, false).is_none());
    assert!(!g.active());
    let notes = g.take_notes();
    let told = notes.iter().find(|n| n.contains("cannot join")).ok_or("no note")?;
    // Each whole version number next to its side, so swapping them (or a longer number that
    // starts with the same digits) fails.
    let host_says = format!(
        "host's WiringCraft uses session protocol version {} and",
        wc_share::PROTOCOL_VERSION
    );
    let guest_says = format!("guest's uses version {other};");
    assert!(told.contains(&host_says) && told.contains(&guest_says), "{told}");
    // The Join dialog is shown again with the same reason, the typed values kept.
    assert!(g.view == View::Join);
    let form = g.form.as_ref().ok_or("no form")?;
    assert_eq!(form.error.as_deref(), Some(told.as_str()));
    assert_eq!(
        (form.name.as_str(), form.email.as_str(), form.addr.as_str()),
        ("Kari", "kari@example.com", "192.168.1.20:47810")
    );
    assert_eq!(form.folder.text, tmp.path().join("guest").display().to_string());
    assert!(!notes.iter().any(|n| n.contains("connection")), "{notes:?}");
    assert_eq!(host_open.project, before);
    assert!(log.messages().is_empty());
    Ok(())
}

#[test]
fn a_declined_guest_is_told_and_the_session_ends_for_them() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut host_open = project(&tmp.path().join("host"))?;
    let (mut h, _) = host(&host_open);
    let mut g = guest(&mut h, "Kari", &tmp.path().join("guest"));
    h.frame(Some(&mut host_open), None, false);
    let ids = pending(&h);
    approve(&mut h, ids[0], false);
    assert!(g.frame(None, None, false).is_none());
    assert!(!g.active());
    assert!(
        g.take_notes().iter().any(|n| n.contains("declined")),
        "the guest is told why"
    );
    Ok(())
}

#[test]
fn an_edit_reaches_the_other_and_the_page_is_read_only_there() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(h.held_by_others().get("p0003").map(String::as_str), Some("Ola"));
    assert_eq!(g.held_by_others().get("p0002").map(String::as_str), Some("Hanna"));

    // The guest changes its page; the host sees it.
    guest_open.project.page_mut("p0003").ok_or("no page")?.title = "Valves and pumps".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(
        host_open.project.page("p0003").map(|p| p.title.as_str()),
        Some("Valves and pumps")
    );

    // The host cannot change the guest's page: the change is put back and the host is told.
    h.take_notes();
    host_open.project.page_mut("p0003").ok_or("no page")?.title = "Mine now".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(
        host_open.project.page("p0003").map(|p| p.title.as_str()),
        Some("Valves and pumps")
    );
    assert_eq!(
        guest_open.project.page("p0003").map(|p| p.title.as_str()),
        Some("Valves and pumps")
    );
    assert!(h.take_notes().iter().any(|n| n.contains("page 3")));

    // The host's own page goes the other way.
    host_open.project.page_mut("p0002").ok_or("no page")?.title = "Motor starter".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(
        guest_open.project.page("p0002").map(|p| p.title.as_str()),
        Some("Motor starter")
    );
    assert_eq!(
        log.messages(),
        ["Edit page 3 Valves and pumps", "Edit page 2 Motor starter"]
    );
    Ok(())
}

#[test]
fn a_drag_over_many_frames_is_one_change() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    for i in 0..10 {
        guest_open.project.page_mut("p0003").ok_or("no page")?.title = format!("Moving {i}");
        g.frame(Some(&mut guest_open), Some("p0003".into()), true);
        h.frame(Some(&mut host_open), Some("p0002".into()), false);
    }
    assert!(log.messages().is_empty(), "nothing goes out while the button is down");
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(log.messages(), ["Edit page 3 Moving 9"]);
    assert_eq!(
        host_open.project.page("p0003").map(|p| p.title.as_str()),
        Some("Moving 9")
    );
    Ok(())
}

#[test]
fn project_level_changes_go_through_the_host_to_everyone() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));

    let tag = DeviceTag::parse("-K1")?;
    guest_open.project.devices.insert(
        tag.clone(),
        Device {
            description: "Main contactor".into(),
            ..Default::default()
        },
    );
    guest_open.project.comments.push(Comment {
        id: "c1".into(),
        page: "p0003".into(),
        at: None,
        author: "Ola".into(),
        time: 0,
        text: "Check the valve size".into(),
        resolved: false,
        replies: Vec::new(),
    });
    guest_open
        .project
        .meta
        .title_block
        .insert("customer".into(), "Kommune".into());
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(
        host_open.project.devices.get(&tag).map(|d| d.description.as_str()),
        Some("Main contactor")
    );
    assert_eq!(host_open.project.comments.len(), 1);
    assert_eq!(
        host_open.project.meta.title_block.get("customer").map(String::as_str),
        Some("Kommune")
    );

    // A page added by the host and renumbered reaches the guest.
    let mut extra = Page::new("p0004", "4", "Terminals");
    extra.title = "Terminals".into();
    host_open.project.pages.push(extra);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(guest_open.project.page("p0004").is_some());
    assert_eq!(
        log.messages(),
        [
            // One frame's project changes are one change.
            "Change project properties; Change device -K1; Comments on page p0003",
            "Add page 4 Terminals"
        ]
    );
    assert_eq!(guest_open.project.devices, host_open.project.devices);
    assert_eq!(guest_open.project.pages, host_open.project.pages);
    Ok(())
}

#[test]
fn removing_a_page_someone_else_has_open_is_not_asked_for() {
    let mut base = Project::new("Demo");
    base.pages.push(Page::new("p0001", "1", "One"));
    base.pages.push(Page::new("p0002", "2", "Two"));
    let mut local = base.clone();
    local.pages.retain(|p| p.id != "p0002");
    let held = BTreeMap::from([("p0002".to_owned(), 7)]);
    let out = outgoing(&local, &base, Some("p0001"), &held, HOST);
    assert!(out.edits.is_empty());
    assert_eq!(out.dropped, ["2"]);
    // Not held: the removal goes to the host.
    let out = outgoing(&local, &base, Some("p0001"), &BTreeMap::new(), HOST);
    assert_eq!(out.edits, [ProjectEdit::RemovePage { id: "p0002".into() }]);
}

#[test]
fn a_new_page_number_is_a_renumber_not_a_page_edit() {
    let mut base = Project::new("Demo");
    base.pages.push(Page::new("p0001", "1", "One"));
    let mut local = base.clone();
    local.pages[0].number = "10".into();
    let out = outgoing(&local, &base, None, &BTreeMap::new(), HOST);
    assert_eq!(out.page, None);
    assert!(out.dropped.is_empty());
    assert_eq!(
        out.edits,
        [ProjectEdit::Renumber {
            numbers: vec![("p0001".into(), "10".into())]
        }]
    );
}

#[test]
fn coming_in_changes_only_what_differs_and_keeps_the_local_symbol_lock() {
    let mut auth = Project::new("Demo");
    auth.pages.push(Page::new("p0001", "1", "One"));
    let mut local = auth.clone();
    assert!(!incoming(&mut local, &auth));
    local.lock.symbols.insert(
        "wc.coil".into(),
        wc_core::LockEntry {
            hash: "abc".into(),
            source: "starter".into(),
        },
    );
    auth.pages[0].title = "Changed".into();
    assert!(incoming(&mut local, &auth));
    assert_eq!(local.pages, auth.pages);
    assert!(local.lock.symbols.contains_key("wc.coil"));
}

#[test]
fn an_item_the_session_took_back_leaves_the_local_lock() -> TestResult {
    let entry = |hash: &str| wc_core::LockEntry {
        hash: hash.into(),
        source: "starter".into(),
    };
    let mut session = Project::new("Demo");
    session.lock.symbols.insert("wc.coil".into(), entry("abc"));
    session.lock.parts.insert("acme.relay".into(), entry("p1"));
    // Placed in the session at the last sync and taken back with the coil since: this copy
    // has not seen that yet, so the stale placement does not keep the coil.
    let mut stale = Page::new("p0002", "2", "Two");
    stale.symbols.push(wc_core::PlacedSymbol {
        id: "s9".into(),
        symbol: "wc.coil".into(),
        at: Default::default(),
        rotation: Default::default(),
        mirror: false,
        tag: None,
    });
    session.pages.push(stale);
    let base = session.clone();
    let mut local = session.clone();
    // Renumbering that page here is no new use of the coil on it.
    local.pages.iter_mut().find(|p| p.id == "p0002").ok_or("p0002")?.number = "20".into();
    // Used here and not sent yet: the session never had it, so it still has to go out.
    local.lock.symbols.insert("wc.lamp".into(), entry("def"));
    // Taken back in the session, but placed here since: it goes out again with that change.
    local.lock.symbols.insert("wc.placed".into(), entry("ghi"));
    session.lock.symbols.insert("wc.placed".into(), entry("ghi"));
    let was = session.lock.clone();
    let mut page = Page::new("p0001", "1", "One");
    page.symbols.push(wc_core::PlacedSymbol {
        id: "s1".into(),
        symbol: "wc.placed".into(),
        at: Default::default(),
        rotation: Default::default(),
        mirror: false,
        tag: None,
    });
    local.pages.push(page);
    // Same hash from another source counts as the same version.
    local.lock.parts.get_mut("acme.relay").ok_or("relay")?.source = "other".into();
    // The session undid the changes that added the coil, the relay and the placed symbol.
    let auth = Project::new("Demo");
    let dropped = taken_back(&mut local, &base, &was, &auth);
    assert_eq!(
        dropped,
        [
            (ItemKind::Symbol, "wc.coil".to_owned()),
            (ItemKind::Part, "acme.relay".to_owned())
        ]
    );
    assert!(!local.lock.symbols.contains_key("wc.coil"));
    assert!(!local.lock.parts.contains_key("acme.relay"));
    assert!(local.lock.symbols.contains_key("wc.lamp"));
    assert!(local.lock.symbols.contains_key("wc.placed"));
    Ok(())
}

#[test]
fn a_version_the_copy_has_not_taken_yet_still_goes_when_the_session_takes_back_the_next() {
    let entry = |hash: &str| wc_core::LockEntry {
        hash: hash.into(),
        source: "starter".into(),
    };
    let mut v1 = Project::new("Demo");
    v1.lock.symbols.insert("wc.coil".into(), entry("v1"));
    let local = v1.clone();
    // The session moved to v2 before this copy took it.
    let mut v2 = Project::new("Demo");
    v2.lock.symbols.insert("wc.coil".into(), entry("v2"));
    let seen = next_seen(&v1.lock, &local, &v2);
    assert_eq!(seen.symbols.get("wc.coil").map(|e| e.hash.as_str()), Some("v1"));
    // Then took v2 back as well: the v1 copy here goes with it.
    let mut local = local;
    let dropped = taken_back(&mut local, &v1, &seen, &Project::new("Demo"));
    assert_eq!(dropped, [(ItemKind::Symbol, "wc.coil".to_owned())]);
    // Once this copy holds v2, the session's version is what is seen.
    assert_eq!(next_seen(&seen, &v2, &v2), v2.lock);
}

#[test]
fn a_part_taken_back_leaves_the_part_list_unless_a_library_offers_it() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut open = project(&tmp.path().join("p"))?;
    let lib = wc_library::Library::open(&open.dir, &open.project)?;
    let (lamp, _) = lib.part("example.ex-lamp-24-gn")?;
    let mut own = lamp.clone();
    own.id = "own.part".into();
    store::write_vendored_part(&open.dir, &own)?;
    for p in [&lamp, &own] {
        let entry = wc_core::LockEntry {
            hash: p.hash(),
            source: "starter".into(),
        };
        open.project.lock.parts.insert(p.id.clone(), entry);
        open.parts.insert(p.id.clone(), p.clone());
        if !open.part_library.iter().any(|(i, _)| *i == p.id) {
            open.part_library.push((p.id.clone(), p.id.clone()));
        }
    }
    // A device here still names the project's own part from the last sync; the session took
    // that assignment back with the part.
    let device = Device {
        part: Some("own.part".into()),
        ..Default::default()
    };
    open.project.devices.insert(DeviceTag::parse("-K1")?, device);
    let mut seen = Some(open.project.lock.clone());
    let base = open.project.clone();
    let mut auth = open.project.clone();
    auth.lock = Default::default();
    auth.devices.clear();
    let (mut unload, mut delist) = Default::default();
    follow_take_backs(&mut seen, &mut unload, &mut delist, &mut open, &base, &auth);
    assert!(
        open.parts.contains_key("own.part"),
        "still drawn until the session's change is in"
    );
    assert!(incoming(&mut open.project, &auth));
    unload_taken_back(&mut unload, &mut open);
    assert_eq!(delist_taken_back(&mut delist, &mut open), None);
    assert!(delist.is_empty());
    // Reading the lists again does not bring the stale part back either.
    open.refresh_library();
    assert!(open.project.lock.parts.is_empty());
    assert!(
        open.part_library.iter().any(|(i, _)| i == "example.ex-lamp-24-gn"),
        "the library still offers it"
    );
    assert!(
        !open.part_library.iter().any(|(i, _)| i == "own.part"),
        "nothing offers it any more"
    );
    assert!(open.parts.is_empty() && unload.is_empty(), "neither copy is used here");
    assert!(
        store::read_vendored_part(&open.dir, "own.part").is_ok(),
        "the file stays"
    );
    Ok(())
}

/// Run frames of both windows until `done` or a few seconds have passed.
fn until(
    a: (&mut Share, &mut Open, &str),
    b: (&mut Share, &mut Open, &str),
    done: impl Fn(&Open, &Open) -> bool,
) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(10) {
        a.0.frame(Some(a.1), Some(a.2.to_owned()), false);
        b.0.frame(Some(b.1), Some(b.2.to_owned()), false);
        if done(a.1, b.1) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

#[test]
fn two_windows_share_over_the_network_and_every_change_is_a_commit() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut host_open = project(&tmp.path().join("host"))?;
    let mut h = Share::default();
    h.start_host(&mut host_open, "Hanna", "hanna@example.com")?;
    let port = match &h.role {
        Some(Role::Host(x)) => x.port,
        _ => 0,
    };
    let mut g = Share::default();
    g.join(
        &format!("127.0.0.1:{port}"),
        "Ola",
        "ola@example.com",
        tmp.path().join("guest"),
    )?;

    let start = std::time::Instant::now();
    let mut guest_open = None;
    while guest_open.is_none() && start.elapsed() < std::time::Duration::from_secs(10) {
        h.frame(Some(&mut host_open), Some("p0002".into()), false);
        for id in pending(&h) {
            approve(&mut h, id, true);
        }
        guest_open = g.frame(None, None, false);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let mut guest_open = guest_open.ok_or("the guest was not let in")?;

    // The guest's page is editable once the host has given it the page.
    let start = std::time::Instant::now();
    while !h.held_by_others().contains_key("p0003") && start.elapsed() < std::time::Duration::from_secs(10) {
        h.frame(Some(&mut host_open), Some("p0002".into()), false);
        g.frame(Some(&mut guest_open), Some("p0003".into()), false);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(h.held_by_others().get("p0003").map(String::as_str), Some("Ola"));
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);

    guest_open.project.page_mut("p0003").ok_or("no page")?.title = "Valves over TCP".into();
    let seen = until(
        (&mut h, &mut host_open, "p0002"),
        (&mut g, &mut guest_open, "p0003"),
        |host, _| host.project.page("p0003").map(|p| p.title.as_str()) == Some("Valves over TCP"),
    );
    assert!(seen, "the guest's change did not reach the host");
    let log = std::process::Command::new("git")
        .args(["log", "-1", "--format=%an <%ae>|%s"])
        .current_dir(&host_open.dir)
        .output()?;
    assert_eq!(
        String::from_utf8_lossy(&log.stdout).trim(),
        "Ola <ola@example.com>|Edit page 3 Valves over TCP"
    );

    // The host ends the session; the guest is told.
    h.stop();
    let start = std::time::Instant::now();
    while g.active() && start.elapsed() < std::time::Duration::from_secs(10) {
        g.frame(Some(&mut guest_open), Some("p0003".into()), false);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!g.active());
    assert!(g.take_notes().iter().any(|n| n.contains("ended")));
    Ok(())
}

#[test]
fn a_second_join_never_writes_over_an_earlier_copy() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (_h, _log, _host_open, _g, first) = session(tmp.path())?;
    let mut h2_open = Open::load(&tmp.path().join("host"))?;
    let (mut h2, _) = host(&h2_open);
    let mut g2 = guest(&mut h2, "Kari", &tmp.path().join("guest"));
    h2.frame(Some(&mut h2_open), None, false);
    let ids = pending(&h2);
    approve(&mut h2, ids[0], true);
    let second = g2.frame(None, None, false).ok_or("the second guest got no project")?;
    assert_eq!(first.dir, tmp.path().join("guest").join("Pump station"));
    assert_eq!(second.dir, tmp.path().join("guest").join("Pump station (2)"));
    Ok(())
}

#[test]
fn a_change_reaching_a_page_held_by_someone_else_is_not_sent_at_all() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    let k1 = DeviceTag::parse("-K1")?;
    host_open.project.devices.insert(k1.clone(), Device::default());
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(guest_open.project.devices.contains_key(&k1));
    let before = log.messages().len();

    // The guest renames -K1 to -K2 everywhere, including on the host's page.
    let k2 = DeviceTag::parse("-K2")?;
    let d = guest_open.project.devices.remove(&k1).ok_or("no -K1")?;
    guest_open.project.devices.insert(k2.clone(), d);
    guest_open.project.page_mut("p0002").ok_or("no page")?.title = "Motor -K2".into();
    guest_open.project.page_mut("p0003").ok_or("no page")?.title = "Valves -K2".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));

    assert_eq!(log.messages().len(), before, "nothing of the action was recorded");
    assert!(host_open.project.devices.contains_key(&k1));
    assert!(!host_open.project.devices.contains_key(&k2));
    assert!(
        guest_open.project.devices.contains_key(&k1),
        "the guest's copy is put back whole"
    );
    assert_eq!(
        guest_open.project.page("p0003").map(|p| p.title.as_str()),
        Some("Valves")
    );
    assert!(g.take_notes().iter().any(|n| n.contains("page 2")));
    Ok(())
}

#[test]
fn a_change_from_someone_else_marks_the_copy_unsaved() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    guest_open.save()?;
    host_open.project.page_mut("p0002").ok_or("no page")?.title = "Motor starter".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(guest_open.dirty);
    Ok(())
}

#[test]
fn a_joined_copy_waits_while_the_window_is_busy() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut host_open = project(&tmp.path().join("host"))?;
    let (mut h, _) = host(&host_open);
    let mut g = guest(&mut h, "Ola", &tmp.path().join("guest"));
    h.frame(Some(&mut host_open), None, false);
    let ids = pending(&h);
    approve(&mut h, ids[0], true);
    // An assistant command is running (or a button is held): the copy is not opened yet.
    assert!(g.frame(None, None, true).is_none());
    assert!(!tmp.path().join("guest").join("Pump station").exists());
    assert!(g.active());
    let joined = g.frame(None, None, false).ok_or("the copy never opened")?;
    assert_eq!(joined.project.pages, host_open.project.pages);
    Ok(())
}

#[test]
fn pages_nobody_has_open_go_out_together_as_one_change() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session_with(tmp.path(), |o| {
        o.project.pages.push(Page::new("p0004", "4", "Terminals"));
        Ok(())
    })?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let before = log.messages().len();

    // The guest renames -K1 on its own page, on the cover and on page 4, which nobody has open.
    let k1 = DeviceTag::parse("-K1")?;
    guest_open.project.devices.insert(k1.clone(), Device::default());
    guest_open.project.page_mut("p0001").ok_or("no page")?.title = "Cover -K1".into();
    guest_open.project.page_mut("p0004").ok_or("no page")?.title = "Terminals -K1".into();
    guest_open.project.page_mut("p0003").ok_or("no page")?.title = "Valves -K1".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));

    // The device, the two pages nobody holds and the guest's own page are one change.
    assert_eq!(log.messages()[before..], ["Change device -K1; Edit 3 pages"]);
    assert!(host_open.project.devices.contains_key(&k1));
    assert_eq!(host_open.project.pages, guest_open.project.pages);
    assert_eq!(
        host_open.project.page("p0004").map(|p| p.title.as_str()),
        Some("Terminals -K1")
    );
    Ok(())
}

/// A session whose guest holds page 3, which has a coil tagged -K1 (in the device list), with
/// the coil's id.
fn session_with_a_coil_on_the_guests_page(
    tmp: &Path,
) -> std::result::Result<(Session, String), Box<dyn std::error::Error>> {
    let mut coil = String::new();
    let (mut h, log, mut host_open, mut g, mut guest_open) = session_with(tmp, |o| {
        coil = o.place(2, "wc.coil", wc_core::Point::new(40.0, 120.0))?;
        let k1 = DeviceTag::parse("-K1")?;
        if let Some(s) = o.project.pages[2].symbols.iter_mut().find(|s| s.id == coil) {
            s.tag = Some(k1.clone());
        }
        o.project.devices.insert(k1, Device::default());
        Ok(())
    })?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    Ok(((h, log, host_open, g, guest_open), coil))
}

/// The coil's tag and place on page 3.
fn coil_on(o: &Open, coil: &str) -> std::result::Result<(Option<String>, wc_core::Point), Box<dyn std::error::Error>> {
    let s = o
        .project
        .page("p0003")
        .and_then(|p| p.symbols.iter().find(|s| s.id == coil))
        .ok_or("no coil")?;
    Ok((s.tag.as_ref().map(ToString::to_string), s.at))
}

#[test]
fn a_rename_on_your_own_page_and_in_the_device_list_is_one_change_and_a_move_meanwhile_follows_it() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, log, mut host_open, mut g, mut guest_open), coil) =
        session_with_a_coil_on_the_guests_page(tmp.path())?;
    let before = log.messages().len();
    let (k1, k2) = (DeviceTag::parse("-K1")?, DeviceTag::parse("-K2")?);
    let start = coil_on(&guest_open, &coil)?.1;

    // The guest renames -K1 (on its page and in the device list), then moves the coil before
    // the host has answered.
    guest_open.project.rename_device(&k1, &k2);
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    let moved = wc_core::Point::new(start.x + 10.0, start.y);
    for s in &mut guest_open.project.page_mut("p0003").ok_or("no page")?.symbols {
        if s.id == coil {
            s.at = moved;
        }
    }
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));

    // The rename is one change, the move the next; the host ends with both.
    let messages = log.messages()[before..].to_vec();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(
        messages[0].contains("-K2") && messages[0].contains("page"),
        "{messages:?}"
    );
    assert!(host_open.project.devices.contains_key(&k2) && !host_open.project.devices.contains_key(&k1));
    assert_eq!(coil_on(&host_open, &coil)?, (Some("-K2".into()), moved));
    assert_eq!(host_open.project.pages, guest_open.project.pages);
    assert_eq!(host_open.project.devices, guest_open.project.devices);
    Ok(())
}

#[test]
fn a_refused_rename_on_your_own_page_takes_back_the_page_and_a_move_made_meanwhile() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, log, mut host_open, mut g, mut guest_open), coil) =
        session_with_a_coil_on_the_guests_page(tmp.path())?;
    let before = log.messages().len();
    let (k1, k2) = (DeviceTag::parse("-K1")?, DeviceTag::parse("-K2")?);
    let start = coil_on(&guest_open, &coil)?;

    // The host describes -K1; before that reaches the guest, the guest renames it on its page
    // and in the device list, then moves the coil before the host has answered.
    host_open.project.devices.get_mut(&k1).ok_or("no -K1")?.description = "Pump contactor".into();
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    guest_open.project.rename_device(&k1, &k2);
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    for s in &mut guest_open.project.page_mut("p0003").ok_or("no page")?.symbols {
        if s.id == coil {
            s.at = wc_core::Point::new(start.1.x + 10.0, start.1.y);
        }
    }
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));

    // Neither the rename nor the move is kept, on the host or in the guest's window.
    assert_eq!(log.messages().len(), before + 1, "{:?}", log.messages());
    assert_eq!(coil_on(&host_open, &coil)?, start);
    assert_eq!(coil_on(&guest_open, &coil)?, start);
    assert!(!host_open.project.devices.contains_key(&k2));
    assert_eq!(
        host_open.project.devices.get(&k1).map(|d| d.description.as_str()),
        Some("Pump contactor")
    );
    assert_eq!(host_open.project.pages, guest_open.project.pages);
    assert_eq!(host_open.project.devices, guest_open.project.devices);
    assert!(g.take_notes().iter().any(|n| n.contains("nor what you did on it")));
    Ok(())
}

#[test]
fn a_refused_rename_on_your_own_page_puts_the_page_back_after_it_was_deleted_meanwhile() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), coil) =
        session_with_a_coil_on_the_guests_page(tmp.path())?;
    let (k1, k2) = (DeviceTag::parse("-K1")?, DeviceTag::parse("-K2")?);
    let start = coil_on(&guest_open, &coil)?;
    host_open.project.devices.get_mut(&k1).ok_or("no -K1")?.description = "Pump contactor".into();
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    guest_open.project.rename_device(&k1, &k2);
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    // The guest deletes its page before the answer comes.
    guest_open.project.pages.retain(|p| p.id != "p0003");
    g.frame(Some(&mut guest_open), Some("p0002".into()), false);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0002"));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0002"));

    // The refused action is not kept: the page is back, and the session never lost it.
    assert_eq!(coil_on(&host_open, &coil)?, start);
    assert_eq!(coil_on(&guest_open, &coil)?, start);
    assert_eq!(host_open.project.pages, guest_open.project.pages);
    assert_eq!(host_open.project.devices, guest_open.project.devices);
    Ok(())
}

#[test]
fn a_refused_rename_on_your_own_page_is_taken_back_also_when_the_session_ends_with_it() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), coil) =
        session_with_a_coil_on_the_guests_page(tmp.path())?;
    let (k1, k2) = (DeviceTag::parse("-K1")?, DeviceTag::parse("-K2")?);
    let start = coil_on(&guest_open, &coil)?;
    host_open.project.devices.get_mut(&k1).ok_or("no -K1")?.description = "Pump contactor".into();
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    guest_open.project.rename_device(&k1, &k2);
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    // The host refuses, changes -K1 again, then ends the session before the guest's next
    // frame: the guest gets all three at once.
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    host_open.project.devices.get_mut(&k1).ok_or("no -K1")?.description = "Pump contactor, 2".into();
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    h.stop();
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert!(!g.active());
    assert_eq!(coil_on(&guest_open, &coil)?, start);
    // The device records come back with the page: -K1 as the session has it last, no -K2.
    assert_eq!(
        guest_open.project.devices.get(&k1).map(|d| d.description.as_str()),
        Some("Pump contactor, 2")
    );
    assert!(!guest_open.project.devices.contains_key(&k2));
    assert!(g.take_notes().iter().any(|n| n.contains("nor what you did on it")));
    Ok(())
}

#[test]
fn a_page_renumbered_with_a_refused_change_of_your_own_page_gets_its_number_back() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), _coil) =
        session_with_a_coil_on_the_guests_page(tmp.path())?;
    let (k1, k2) = (DeviceTag::parse("-K1")?, DeviceTag::parse("-K2")?);
    let number = guest_open.project.page("p0001").ok_or("no page")?.number.clone();
    let title = guest_open.project.page("p0001").ok_or("no page")?.title.clone();
    host_open.project.devices.get_mut(&k1).ok_or("no -K1")?.description = "Pump contactor".into();
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    // One action: the guest renames -K1 on its page and renumbers the cover, all refused.
    guest_open.project.rename_device(&k1, &k2);
    guest_open.project.page_mut("p0001").ok_or("no page")?.number = "90".into();
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    // The session ends before the guest's next sync.
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    h.stop();
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert!(!g.active());
    // The cover has its number back, and keeps the rest as it was.
    let cover = guest_open.project.page("p0001").ok_or("no page")?;
    assert_eq!(cover.number, number);
    assert_eq!(cover.title, title);
    assert!(!guest_open.project.devices.contains_key(&k2));
    // And its place in the page order.
    let mut sorted = guest_open.project.clone();
    sorted.sort_pages();
    let ids = |p: &Project| p.pages.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&guest_open.project), ids(&sorted));
    assert_eq!(guest_open.project.pages[0].id, "p0001");
    Ok(())
}

#[test]
fn a_newer_number_set_while_a_refused_renumbering_was_on_its_way_is_kept_and_sent() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), _coil) =
        session_with_a_coil_on_the_guests_page(tmp.path())?;
    let (k1, k2) = (DeviceTag::parse("-K1")?, DeviceTag::parse("-K2")?);
    host_open.project.devices.get_mut(&k1).ok_or("no -K1")?.description = "Pump contactor".into();
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    // One action renames -K1 on the guest's page and renumbers the cover to 90; it is refused.
    guest_open.project.rename_device(&k1, &k2);
    guest_open.project.page_mut("p0001").ok_or("no page")?.number = "90".into();
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    // Before the refusal arrives, the guest renumbers the cover again.
    guest_open.project.page_mut("p0001").ok_or("no page")?.number = "91".into();
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert_eq!(guest_open.project.page("p0001").ok_or("no page")?.number, "91");
    // The newer number goes out on its own and the session takes it.
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(host_open.project.page("p0001").ok_or("no page")?.number, "91");
    assert_eq!(guest_open.project.page("p0001").ok_or("no page")?.number, "91");
    assert!(!guest_open.project.devices.contains_key(&k2));
    Ok(())
}

#[test]
fn a_page_the_host_removed_before_ending_goes_with_the_refused_renumbering() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), _coil) =
        session_with_a_coil_on_the_guests_page(tmp.path())?;
    let (k1, k2) = (DeviceTag::parse("-K1")?, DeviceTag::parse("-K2")?);
    host_open.project.devices.get_mut(&k1).ok_or("no -K1")?.description = "Pump contactor".into();
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    guest_open.project.rename_device(&k1, &k2);
    guest_open.project.page_mut("p0001").ok_or("no page")?.number = "90".into();
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    // The host refuses, removes the cover, and ends the session before the guest's next frame.
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    host_open.project.pages.retain(|p| p.id != "p0001");
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    h.stop();
    guest_open.project.comments.push(Comment {
        id: "c9".into(),
        page: "p0001".into(),
        at: None,
        author: "Ola".into(),
        time: 0,
        text: "Cover title?".into(),
        resolved: false,
        replies: Vec::new(),
    });
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert!(!g.active());
    // The cover is gone here too, not left with the refused number, and its comments with it.
    assert!(guest_open.project.page("p0001").is_none());
    assert!(guest_open.project.comments.iter().all(|c| c.page != "p0001"));
    assert!(!guest_open.project.devices.contains_key(&k2));
    Ok(())
}

#[test]
fn a_page_a_refused_rename_set_without_renumbering_keeps_a_newer_local_number() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = refused_rename_session(tmp.path())?;
    // Before the refusal arrives, the guest renumbers the cover.
    guest_open.project.page_mut("p0001").ok_or("no page")?.number = "90".into();
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert_eq!(guest_open.project.page("p0001").ok_or("no page")?.number, "90");
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(host_open.project.page("p0001").ok_or("no page")?.number, "90");
    Ok(())
}

/// A session where -K1 is drawn on the guest's page 3 and on the cover (a page nobody holds),
/// and the host has just changed -K1: a rename of it by the guest is refused.
fn refused_rename_session(tmp: &Path) -> std::result::Result<Session, Box<dyn std::error::Error>> {
    let (mut h, log, mut host_open, mut g, mut guest_open) = session_with(tmp, |o| {
        let k1 = DeviceTag::parse("-K1")?;
        let coil = o.place(2, "wc.coil", wc_core::Point::new(40.0, 120.0))?;
        let cover = o.place(0, "wc.coil", wc_core::Point::new(40.0, 120.0))?;
        for (page, id) in [(2, &coil), (0, &cover)] {
            if let Some(s) = o.project.pages[page].symbols.iter_mut().find(|s| s.id == *id) {
                s.tag = Some(k1.clone());
            }
        }
        o.project.devices.clear();
        o.project.devices.insert(k1, Device::default());
        Ok(())
    })?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let k1 = DeviceTag::parse("-K1")?;
    host_open.project.devices.get_mut(&k1).ok_or("no -K1")?.description = "Pump contactor".into();
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    // The rename sets the cover with the guest's page, as one change.
    guest_open.project.rename_device(&k1, &DeviceTag::parse("-K2")?);
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    Ok((h, log, host_open, g, guest_open))
}

/// The tags drawn on page `id` of `o`.
fn tags_on(o: &Open, id: &str) -> Vec<Option<String>> {
    o.project
        .page(id)
        .map(|p| {
            p.symbols
                .iter()
                .map(|s| s.tag.as_ref().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_free_page_a_refused_rename_set_comes_back_as_the_session_has_it() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = refused_rename_session(tmp.path())?;
    // Before the refusal arrives, the guest renumbers the cover: its content is still as sent.
    guest_open.project.page_mut("p0001").ok_or("no page")?.number = "90".into();
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert_eq!(
        tags_on(&guest_open, "p0001"),
        [Some("-K1".to_owned())],
        "the cover comes back"
    );
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let k2 = DeviceTag::parse("-K2")?;
    for (who, o) in [("host", &host_open), ("guest", &guest_open)] {
        for page in ["p0001", "p0003"] {
            assert_eq!(tags_on(o, page), [Some("-K1".to_owned())], "{who}: page {page}");
        }
        assert!(!o.project.devices.contains_key(&k2), "{who}: no -K2");
        // The newer number set meanwhile is kept, and went out on its own.
        assert_eq!(o.project.page("p0001").ok_or("no page")?.number, "90", "{who}");
    }
    Ok(())
}

#[test]
fn a_change_to_a_free_page_made_before_someone_elses_change_arrived_is_not_kept() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let before = log.messages().len();

    // The host retitles the cover; in the same moment, before that reaches the guest, the
    // guest changes the cover too, from the old version.
    host_open.project.page_mut("p0001").ok_or("no page")?.title = "Revised".into();
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    guest_open.project.page_mut("p0001").ok_or("no page")?.cover = false;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));

    assert_eq!(log.messages()[before..], ["Edit page 1 Revised"]);
    assert!(g.take_notes().iter().any(|n| n.contains("changed by someone else")));
    for (who, o) in [("host", &host_open), ("guest", &guest_open)] {
        let cover = o.project.page("p0001").ok_or("no page")?;
        assert_eq!(cover.title, "Revised", "{who}: the earlier change is kept");
        assert!(cover.cover, "the stale change is not");
    }
    Ok(())
}

#[test]
fn wire_numbering_in_a_session_leaves_pages_others_hold_and_says_so() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let pin = |item: &str, pin: &str| wc_core::Endpoint::Pin {
        item: item.into(),
        pin: pin.into(),
    };
    let (mut h, log, mut host_open, mut g, mut guest_open) = session_with(tmp.path(), |o| {
        // Two coils joined by a wire on every page.
        for page in 0..o.project.pages.len() {
            let a = o.place(page, "wc.coil", wc_core::Point::new(20.0, 120.0))?;
            let b = o.place(page, "wc.coil", wc_core::Point::new(60.0, 160.0))?;
            o.add_wire(page, pin(&a, "A2"), pin(&b, "A1"), Vec::new());
        }
        Ok(())
    })?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let before = log.messages().len();

    // The host numbers the wires while the guest has page 3 open.
    host_open.held_by_others = h.held_by_others();
    let done = host_open.number_wires(wc_core::numbering::Rule::Sequential);
    assert_eq!(done.skipped, ["3"]);
    assert_eq!(done.done.wires, 2, "the cover and the host's own page");
    assert!(done.report().contains("Page(s) 3 were left as they are"));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));

    let numbered = |o: &Open, page: &str| {
        o.project
            .page(page)
            .is_some_and(|p| p.wires.iter().all(|w| w.number.is_some()))
    };
    assert!(numbered(&guest_open, "p0001"), "the cover's numbers reached the guest");
    assert!(numbered(&guest_open, "p0002"));
    assert!(!numbered(&host_open, "p0003"), "the guest's page was not touched");
    assert_eq!(host_open.project.pages, guest_open.project.pages);
    // The host's own page and the cover are one change.
    assert_eq!(log.messages()[before..], ["Edit 2 pages"]);
    Ok(())
}

#[test]
fn a_page_changed_on_both_sides_takes_the_sessions_version() {
    let mut base = Project::new("Test");
    base.pages.push(Page::new("p1", "1", "One"));
    base.pages.push(Page::new("p2", "2", "Two"));
    let mut auth = base.clone();
    let mut local = base.clone();
    // Both changed page 1; only this window changed page 2.
    auth.pages[0].title = "Restored".into();
    local.pages[0].title = "Local".into();
    local.pages[1].title = "Mine".into();
    let lost = yield_to_session(&mut local, &mut base, &auth);
    assert_eq!(lost, ["page 1"]);
    assert_eq!(
        (local.pages[0].title.as_str(), base.pages[0].title.as_str()),
        ("Restored", "Restored")
    );
    assert_eq!(local.pages[1].title, "Mine", "kept");
}

fn symbol(id: &str, x: f64) -> wc_core::PlacedSymbol {
    wc_core::PlacedSymbol {
        id: id.into(),
        symbol: "coil".into(),
        at: wc_core::geom::Point::new(x, 0.0),
        rotation: Default::default(),
        mirror: false,
        tag: None,
    }
}

fn wire(id: &str, from: &str, to: &str) -> Result<wc_core::Wire, serde_json::Error> {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "from": { "item": from, "pin": "A1" },
        "to": { "item": to, "pin": "A2" },
    }))
}

/// A page with symbols s1 and s2 and a wire between them, as base, session and window copies.
fn three_copies() -> TestResult<(Project, Project, Project)> {
    let mut base = Project::new("Test");
    let mut page = Page::new("p1", "1", "One");
    page.symbols = vec![symbol("s1", 0.0), symbol("s2", 50.0)];
    page.wires = vec![wire("w1", "s1", "s2")?];
    base.pages.push(page);
    Ok((base.clone(), base.clone(), base))
}

#[test]
fn an_edit_on_a_page_the_undo_restored_is_kept_where_they_do_not_overlap() -> TestResult {
    let (mut base, mut auth, mut local) = three_copies()?;
    // The undo moved s1 back and took the title back.
    auth.pages[0].symbols[0].at.x = -10.0;
    auth.pages[0].title = "Restored".into();
    // Meanwhile this window added s3 with a wire to s2, moved s2, removed w1, added a text and
    // a title-block field.
    let page = &mut local.pages[0];
    page.symbols[1].at.x = 60.0;
    page.symbols.push(symbol("s3", 100.0));
    page.wires = vec![wire("w2", "s2", "s3")?];
    page.graphics.push(wc_core::geom::Shape::Line {
        a: wc_core::geom::Point::new(0.0, 0.0),
        b: wc_core::geom::Point::new(1.0, 1.0),
    });
    page.title_block.insert("revision".into(), "B".into());
    let lost = yield_to_session(&mut local, &mut base, &auth);
    assert!(lost.is_empty(), "{lost:?}");
    let page = &local.pages[0];
    assert_eq!(page.title, "Restored", "the undo's own parts are restored");
    let xs: Vec<(&str, f64)> = page.symbols.iter().map(|s| (s.id.as_str(), s.at.x)).collect();
    assert_eq!(xs, [("s1", -10.0), ("s2", 60.0), ("s3", 100.0)]);
    assert_eq!(page.wires.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["w2"]);
    assert_eq!(page.graphics.len(), 1);
    assert_eq!(page.title_block.get("revision").map(String::as_str), Some("B"));
    assert_eq!(base.pages[0], auth.pages[0], "the kept edit still goes out");
    Ok(())
}

#[test]
fn a_cloud_and_the_revision_table_drawn_here_are_kept_beside_the_sessions_edit() -> TestResult {
    use wc_core::geom::Point;
    let (mut base, mut auth, mut local) = three_copies()?;
    auth.pages[0].title = "Restored".into();
    local.pages[0].clouds.push(wc_core::RevisionCloud::new(
        Point::new(10.0, 10.0),
        Point::new(40.0, 30.0),
        "A",
    ));
    local.pages[0].revision_table = Some(Point::new(200.0, 150.0));
    let lost = yield_to_session(&mut local, &mut base, &auth);
    assert!(lost.is_empty(), "{lost:?}");
    let page = &local.pages[0];
    assert_eq!(page.title, "Restored");
    assert_eq!(page.clouds.len(), 1, "the cloud is not lost");
    assert_eq!(page.revision_table, Some(Point::new(200.0, 150.0)));
    // The table moved on both sides to different places is a clash.
    let (mut base, mut auth, mut local) = three_copies()?;
    auth.pages[0].revision_table = Some(Point::new(1.0, 1.0));
    local.pages[0].revision_table = Some(Point::new(2.0, 2.0));
    assert_eq!(yield_to_session(&mut local, &mut base, &auth), ["page 1"]);
    Ok(())
}

#[test]
fn a_cloud_drawn_here_for_a_revision_the_session_removed_is_kept_and_reported() -> TestResult {
    use wc_core::geom::Point;
    let revision = |letter: &str| wc_core::Revision {
        letter: letter.into(),
        date: String::new(),
        text: String::new(),
        author: String::new(),
    };
    let (mut base, mut auth, mut local) = three_copies()?;
    for p in [&mut base, &mut auth, &mut local] {
        p.meta.revisions = vec![revision("A"), revision("B")];
    }
    // The session removed revision A; this window drew a cloud tagged A, and one tagged B.
    auth.meta.revisions.retain(|r| r.letter != "A");
    for letter in ["A", "B"] {
        local.pages[0].clouds.push(wc_core::RevisionCloud::new(
            Point::new(10.0, 10.0),
            Point::new(40.0, 30.0),
            letter,
        ));
    }
    let (lost, clashes) = yield_and_check(&mut local, &mut base, &auth);
    assert!(lost.is_empty(), "{lost:?}");
    assert_eq!(
        clashes,
        ["a cloud on page 1 carries revision A, which the session removed"]
    );
    assert_eq!(local.pages[0].clouds.len(), 2, "the clouds are kept");

    // A cloud of a revision the project never listed is the project check's to report, not a
    // clash of the merge; nor is one this window had before the sync.
    let (mut base, auth, mut local) = three_copies()?;
    local.pages[0].clouds.push(wc_core::RevisionCloud::new(
        Point::new(10.0, 10.0),
        Point::new(40.0, 30.0),
        "Z",
    ));
    let (lost, clashes) = yield_and_check(&mut local, &mut base, &auth);
    assert!(lost.is_empty() && clashes.is_empty(), "{lost:?} {clashes:?}");

    // This window drew a cloud tagged A and then removed A itself; the session still lists A.
    let (mut base, mut auth, mut local) = three_copies()?;
    for p in [&mut base, &mut auth, &mut local] {
        p.meta.revisions = vec![revision("A")];
    }
    local.meta.revisions.clear();
    local.pages[0].clouds.push(wc_core::RevisionCloud::new(
        Point::new(10.0, 10.0),
        Point::new(40.0, 30.0),
        "A",
    ));
    let (_, clashes) = yield_and_check(&mut local, &mut base, &auth);
    assert!(clashes.is_empty(), "not the session's removal: {clashes:?}");

    // This window added revision C and a cloud for it; the session left the properties alone.
    let (mut base, auth, mut local) = three_copies()?;
    local.meta.revisions.push(revision("C"));
    local.pages[0].clouds.push(wc_core::RevisionCloud::new(
        Point::new(10.0, 10.0),
        Point::new(40.0, 30.0),
        "C",
    ));
    let (_, clashes) = yield_and_check(&mut local, &mut base, &auth);
    assert!(clashes.is_empty(), "{clashes:?}");
    Ok(())
}

#[test]
fn an_element_changed_on_both_sides_takes_the_sessions_page() -> TestResult {
    let (mut base, mut auth, mut local) = three_copies()?;
    auth.pages[0].symbols[0].at.x = -10.0;
    local.pages[0].symbols[0].at.x = 20.0;
    local.pages[0].symbols.push(symbol("s3", 100.0));
    let lost = yield_to_session(&mut local, &mut base, &auth);
    assert_eq!(lost, ["page 1"]);
    assert_eq!(local.pages[0], auth.pages[0]);
    assert_eq!(base.pages[0], auth.pages[0]);
    Ok(())
}

#[test]
fn removed_on_one_side_and_changed_on_the_other_is_a_clash() -> TestResult {
    // The session removed s2; this window moved it.
    let (mut base, mut auth, mut local) = three_copies()?;
    auth.pages[0].symbols.retain(|s| s.id != "s2");
    local.pages[0].symbols[1].at.x = 70.0;
    assert_eq!(yield_to_session(&mut local, &mut base, &auth), ["page 1"]);
    assert_eq!(local.pages[0], auth.pages[0]);
    // The session moved s2; this window removed it.
    let (mut base, mut auth, mut local) = three_copies()?;
    auth.pages[0].symbols[1].at.x = 70.0;
    local.pages[0].symbols.retain(|s| s.id != "s2");
    assert_eq!(yield_to_session(&mut local, &mut base, &auth), ["page 1"]);
    // Removed on both sides is no clash.
    let (mut base, mut auth, mut local) = three_copies()?;
    auth.pages[0].wires.clear();
    local.pages[0].wires.clear();
    local.pages[0].symbols.push(symbol("s3", 100.0));
    assert!(yield_to_session(&mut local, &mut base, &auth).is_empty());
    assert!(local.pages[0].wires.is_empty());
    assert_eq!(local.pages[0].symbols.len(), 3);
    Ok(())
}

#[test]
fn a_new_wire_to_a_symbol_the_session_removed_is_a_clash() -> TestResult {
    let (mut base, mut auth, mut local) = three_copies()?;
    // The undo took s2 and its wire away; this window drew a new wire to s2.
    auth.pages[0].symbols.retain(|s| s.id != "s2");
    auth.pages[0].wires.clear();
    local.pages[0].wires.push(wire("w2", "s1", "s2")?);
    assert_eq!(yield_to_session(&mut local, &mut base, &auth), ["page 1"]);
    assert_eq!(local.pages[0], auth.pages[0], "no wire left hanging");
    Ok(())
}

#[test]
fn a_session_wire_to_a_symbol_removed_here_is_a_clash() -> TestResult {
    let (mut base, mut auth, mut local) = three_copies()?;
    // This window removed s2 and its wire; the session drew a new wire to s2.
    local.pages[0].symbols.retain(|s| s.id != "s2");
    local.pages[0].wires.clear();
    auth.pages[0].wires.push(wire("w2", "s1", "s2")?);
    assert_eq!(yield_to_session(&mut local, &mut base, &auth), ["page 1"]);
    assert_eq!(local.pages[0], auth.pages[0], "no wire left hanging");
    Ok(())
}

#[test]
fn a_kept_edit_keeps_a_number_only_this_window_changed() -> TestResult {
    let (mut base, mut auth, mut local) = three_copies()?;
    auth.pages[0].title = "Restored".into();
    local.pages[0].number = "9".into();
    local.pages[0].symbols.push(symbol("s3", 100.0));
    assert!(yield_to_session(&mut local, &mut base, &auth).is_empty());
    assert_eq!(local.pages[0].number, "9", "the renumbering made here stays");
    assert_eq!(base.pages[0].number, "1", "and still goes out on its own");
    assert_eq!(local.pages[0].symbols.len(), 3);
    assert_eq!(local.pages[0].title, "Restored");
    Ok(())
}

#[test]
fn every_part_changed_on_both_sides_takes_the_sessions_version() -> TestResult {
    let k1 = wc_core::DeviceTag::parse("-K1")?;
    let k2 = wc_core::DeviceTag::parse("-K2")?;
    let mut base = Project::new("Test");
    base.pages.push(Page::new("p1", "1", "One"));
    base.pages.push(Page::new("p2", "2", "Two"));
    base.pages.push(Page::new("p3", "3", "Three"));
    base.meta.title_block.insert("customer".into(), "New".into());
    base.devices.insert(k1.clone(), wc_core::Device::default());
    let mut auth = base.clone();
    let mut local = base.clone();
    // The session (an undo) put back the customer, changed page 1 and -K1.
    auth.meta.title_block.insert("customer".into(), "Old".into());
    auth.pages[0].title = "Restored".into();
    auth.devices.remove(&k1);
    // Meanwhile this window changed another title-block field, removed page 1, changed -K1,
    // added -K2 and removed page 3: only what the session left alone is kept.
    local.meta.title_block.insert("drawn_by".into(), "Me".into());
    local.pages.retain(|p| p.id != "p1" && p.id != "p3");
    local.devices.get_mut(&k1).ok_or("no -K1")?.part = Some("changed".into());
    local.devices.insert(k2.clone(), wc_core::Device::default());
    let lost = yield_to_session(&mut local, &mut base, &auth);
    assert_eq!(lost, ["the project properties", "page 1", "device -K1"]);
    assert_eq!(local.meta, auth.meta, "the undone value is not sent back");
    assert_eq!(
        local.pages.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["p1", "p2"],
        "the restored page is back; page 3's removal still goes out"
    );
    assert_eq!(local.pages[0].title, "Restored");
    assert!(!local.devices.contains_key(&k1) && local.devices.contains_key(&k2));
    assert_eq!(base.pages[0].title, "Restored");
    assert!(!base.devices.contains_key(&k1));
    Ok(())
}

#[test]
fn a_page_removed_while_its_renumbering_is_undone_stays() {
    let mut base = Project::new("Test");
    base.pages.push(Page::new("p1", "1", "One"));
    base.pages.push(Page::new("p2", "2", "Two"));
    let mut auth = base.clone();
    let mut local = base.clone();
    // The session (an undo) gave page 1 its old number back; this window removed it.
    auth.pages[0].number = "7".into();
    local.pages.retain(|p| p.id != "p1");
    let lost = yield_to_session(&mut local, &mut base, &auth);
    assert_eq!(lost, ["page 1"]);
    assert_eq!(local.page("p1").map(|p| p.number.as_str()), Some("7"));
    assert_eq!(base.page("p1").map(|p| p.number.as_str()), Some("7"));
    // A renumbering alone does not clash with a content change made here.
    let mut base = Project::new("Test");
    base.pages.push(Page::new("p1", "1", "One"));
    let mut auth = base.clone();
    let mut local = base.clone();
    auth.pages[0].number = "7".into();
    local.pages[0].title = "Mine".into();
    assert!(yield_to_session(&mut local, &mut base, &auth).is_empty());
    assert_eq!(local.pages[0].title, "Mine");
}

#[test]
fn a_page_numbered_on_both_sides_keeps_the_sessions_number() {
    let mut base = Project::new("Test");
    base.pages.push(Page::new("p1", "1", "One"));
    let mut auth = base.clone();
    let mut local = base.clone();
    // The undo gave page 1 its old number back; this window gave it another one.
    auth.pages[0].number = "7".into();
    local.pages[0].number = "9".into();
    local.pages[0].title = "Mine".into();
    let lost = yield_to_session(&mut local, &mut base, &auth);
    assert_eq!(lost, ["the number of page 1"]);
    assert_eq!(
        (local.pages[0].number.as_str(), base.pages[0].number.as_str()),
        ("7", "7")
    );
    assert_eq!(
        local.pages[0].title, "Mine",
        "the content change is a part of its own and stays"
    );
    // The same new number on both sides is no clash.
    let mut base = Project::new("Test");
    base.pages.push(Page::new("p1", "1", "One"));
    let mut auth = base.clone();
    auth.pages[0].number = "7".into();
    let mut local = auth.clone();
    assert!(yield_to_session(&mut local, &mut base, &auth).is_empty());
}

#[test]
fn a_content_clash_keeps_a_number_only_this_window_changed() {
    let mut base = Project::new("Test");
    base.pages.push(Page::new("p1", "1", "A"));
    let mut auth = base.clone();
    let mut local = base.clone();
    auth.pages[0].title = "B".into();
    local.pages[0].number = "9".into();
    local.pages[0].title = "C".into();
    let lost = yield_to_session(&mut local, &mut base, &auth);
    assert_eq!(lost, ["page 1"]);
    assert_eq!(local.pages[0].title, "B", "the session's content wins");
    assert_eq!(local.pages[0].number, "9", "the renumbering made here stays");
    assert_eq!(base.pages[0].number, "1", "and still goes out on its own");
}

#[test]
fn a_page_removed_while_its_undo_is_on_its_way_stays_in_the_session() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    guest_open.project.page_mut("p0003").ok_or("no page")?.title = "First".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    g.undo()?;
    guest_open.project.pages.retain(|p| p.id != "p0003");
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    for open in [&host_open, &guest_open] {
        assert_eq!(open.project.page("p0003").map(|p| p.title.as_str()), Some("Valves"));
    }
    assert!(
        g.take_notes()
            .iter()
            .any(|n| n.contains("page 3") && n.contains("was not kept"))
    );
    Ok(())
}

#[test]
fn a_title_block_field_changed_while_an_undo_is_on_its_way_does_not_cancel_it() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let before = guest_open.project.meta.title_block.get("customer").cloned();
    guest_open
        .project
        .meta
        .title_block
        .insert("customer".into(), "New".into());
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    g.undo()?;
    guest_open
        .project
        .meta
        .title_block
        .insert("drawn_by".into(), "Guest".into());
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    for open in [&host_open, &guest_open] {
        assert_eq!(
            open.project.meta.title_block.get("customer"),
            before.as_ref(),
            "the undo is kept"
        );
    }
    assert!(
        g.take_notes()
            .iter()
            .any(|n| n.contains("project properties") && n.contains("was not kept"))
    );
    Ok(())
}

#[test]
fn a_second_guest_undo_waits_for_the_first_to_come_back() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    guest_open.project.page_mut("p0003").ok_or("no page")?.title = "First".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    guest_open.project.page_mut("p0003").ok_or("no page")?.title = "Second".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    g.undo()?;
    // Made while the first undo is on its way, so not sent yet: a second undo now would take
    // back "Second" ahead of it.
    guest_open.project.page_mut("p0003").ok_or("no page")?.title = "Third".into();
    let err = g.undo().err().ok_or("a second undo was sent")?;
    assert!(err.contains("still on its way"), "{err}");
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(
        log.messages().iter().filter(|m| m.starts_with("Undo")).count(),
        1,
        "only one undo reached the host"
    );
    Ok(())
}

#[test]
fn undo_this_change_waits_while_an_assistant_command_owns_the_files() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let original = host_open.project.page("p0002").ok_or("no page")?.title.clone();
    host_open.project.page_mut("p0002").ok_or("no page")?.title = "First".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let change = h
        .role
        .as_ref()
        .ok_or("no session")?
        .history()
        .last()
        .map(|c| c.id)
        .ok_or("no change")?;
    let before = log.messages().len();
    h.ask_revert(change, true);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(log.messages().len(), before, "nothing was taken back");
    assert!(h.take_notes().iter().any(|n| n.contains("assistant's command")));
    assert_eq!(host_open.project.page("p0002").map(|p| p.title.as_str()), Some("First"));
    h.ask_revert(change, false);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(
        host_open.project.page("p0002").map(|p| p.title.as_str()),
        Some(original.as_str()),
        "taken back once the command is done"
    );
    Ok(())
}

#[test]
fn a_guest_sends_nothing_until_its_undo_has_come_back() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    guest_open.project.page_mut("p0003").ok_or("no page")?.title = "First".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    g.undo()?;
    // Edited before the host answered: this page still holds "First" under the edit.
    guest_open.project.page_mut("p0003").ok_or("no page")?.title = "First, then more".into();
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert_eq!(
        guest_open.project.page("p0003").map(|p| p.title.as_str()),
        Some("First, then more"),
        "the change waits in the window"
    );
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    for open in [&host_open, &guest_open] {
        assert_eq!(open.project.page("p0003").map(|p| p.title.as_str()), Some("Valves"));
    }
    assert_eq!(
        log.messages(),
        ["Edit page 3 First", "Undo: Edit page 3 First"],
        "the stale page never went out"
    );
    assert!(g.take_notes().iter().any(|n| n.contains("was not kept")));
    Ok(())
}

#[test]
fn a_guest_edit_right_after_its_undo_is_kept_when_the_undo_did_not_touch_it() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    guest_open.project.page_mut("p0003").ok_or("no page")?.title = "First".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    g.undo()?;
    // Before the undo's result is in, a text is added to the same page.
    let text = wc_core::geom::Shape::Line {
        a: wc_core::geom::Point::new(5.0, 5.0),
        b: wc_core::geom::Point::new(9.0, 9.0),
    };
    guest_open
        .project
        .page_mut("p0003")
        .ok_or("no page")?
        .graphics
        .push(text.clone());
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    for open in [&host_open, &guest_open] {
        let page = open.project.page("p0003").ok_or("no page")?;
        assert_eq!(page.title, "Valves", "the undo is kept");
        assert!(page.graphics.contains(&text), "and so is the new line");
    }
    assert!(!g.take_notes().iter().any(|n| n.contains("was not kept")));
    Ok(())
}

fn line(x: f64) -> wc_core::geom::Shape {
    wc_core::geom::Shape::Line {
        a: wc_core::geom::Point::new(x, 5.0),
        b: wc_core::geom::Point::new(x, 9.0),
    }
}

#[test]
fn a_text_added_right_after_an_undo_that_moved_a_line_is_kept() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // A line on the page, then moved: the undo of the move puts it back.
    guest_open
        .project
        .page_mut("p0003")
        .ok_or("no page")?
        .graphics
        .push(line(5.0));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    guest_open.project.page_mut("p0003").ok_or("no page")?.graphics = vec![line(20.0)];
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    g.undo()?;
    // Before the undo's result is in, a text is added to the same page.
    let text = wc_core::geom::Shape::Text {
        at: wc_core::geom::Point::new(40.0, 40.0),
        text: "Note".into(),
        height: 3.5,
        align: Default::default(),
        angle: 0.0,
    };
    guest_open
        .project
        .page_mut("p0003")
        .ok_or("no page")?
        .graphics
        .push(text.clone());
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    for open in [&host_open, &guest_open] {
        let page = open.project.page("p0003").ok_or("no page")?;
        assert_eq!(
            page.graphics,
            [line(5.0), text.clone()],
            "the line is back and the text kept"
        );
    }
    assert!(!g.take_notes().iter().any(|n| n.contains("was not kept")));
    Ok(())
}

#[test]
fn a_line_moved_here_that_the_undo_removed_is_not_kept() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    guest_open
        .project
        .page_mut("p0003")
        .ok_or("no page")?
        .graphics
        .push(line(5.0));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    g.undo()?;
    // Moved before the undo, which takes the line away, is in.
    guest_open.project.page_mut("p0003").ok_or("no page")?.graphics = vec![line(20.0)];
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    for open in [&host_open, &guest_open] {
        assert!(open.project.page("p0003").ok_or("no page")?.graphics.is_empty());
    }
    assert!(g.take_notes().iter().any(|n| n.contains("was not kept")));
    Ok(())
}

#[test]
fn free_graphics_merge_item_by_item() {
    use wc_core::geom::{Point, Shape};
    let [a, b, c, d] = [1.0, 2.0, 3.0, 4.0].map(line);
    let text = Shape::Text {
        at: Point::new(40.0, 40.0),
        text: "Note".into(),
        height: 3.5,
        align: Default::default(),
        angle: 0.0,
    };
    let m = |base: &[&Shape], auth: &[&Shape], local: &[&Shape]| {
        let own = |v: &[&Shape]| v.iter().map(|x| (*x).clone()).collect::<Vec<_>>();
        merge_unordered(&own(base), &own(auth), &own(local), |x, y| {
            std::mem::discriminant(x) == std::mem::discriminant(y)
        })
    };
    // Added on each side: both kept, the session's order first.
    assert_eq!(
        m(&[&a], &[&a, &b], &[&a, &c]),
        Some(vec![a.clone(), b.clone(), c.clone()])
    );
    // Removed here while the session still has it; a repeat counts once per copy.
    assert_eq!(
        m(&[&a, &a, &b], &[&a, &a, &b, &d], &[&a, &b]),
        Some(vec![a.clone(), b.clone(), d.clone()])
    );
    // Removed on both sides, nothing added here: no clash.
    assert_eq!(m(&[&a, &b], &[&b], &[&b]), Some(vec![b.clone()]));
    // The same item added on both sides is there once.
    assert_eq!(m(&[&b], &[&b, &c], &[&b, &c]), Some(vec![b.clone(), c.clone()]));
    // Both sides made the same change (the undo put a line back where it was also moved here).
    assert_eq!(m(&[&a, &b], &[&c, &b], &[&c, &b]), Some(vec![c.clone(), b.clone()]));
    // Only this window changed the graphics: its list, in its order.
    assert_eq!(m(&[&a, &b], &[&a, &b], &[&c, &b]), Some(vec![c.clone(), b.clone()]));
    // Changed here (a to c) while the session changed it too (a to b): a clash.
    assert_eq!(m(&[&a], &[&b], &[&c]), None);
    // Deleted here while the session moved it (a to c): a clash, not a silent comeback.
    assert_eq!(m(&[&a, &b], &[&c, &b], &[&b]), None);
    // Deleted here while the session added only a text: no clash.
    assert_eq!(m(&[&a, &b], &[&b, &text], &[&b]), Some(vec![b.clone(), text.clone()]));
    // Deleted on both sides while a text was added here: the text cannot be the line's new
    // version, so it is kept.
    assert_eq!(m(&[&a, &b], &[&b], &[&b, &text]), Some(vec![b.clone(), text.clone()]));
    // The same, but the session put a new line in its place (a moved to d): a clash.
    assert_eq!(m(&[&a, &b], &[&b, &d], &[&b, &text]), None);
    // The same move made on both sides (a to c) and a text added here: agreed, text kept.
    assert_eq!(m(&[&a], &[&c], &[&c, &text]), Some(vec![c.clone(), text.clone()]));
    // One of two copies removed on both sides, a text added here: the other copy stays.
    assert_eq!(m(&[&a, &a], &[&a], &[&a, &text]), Some(vec![a.clone(), text.clone()]));
    // The session kept both copies and added one: the copy removed here still goes.
    assert_eq!(m(&[&a, &a], &[&a, &a, &b], &[&a]), Some(vec![a.clone(), b.clone()]));
    // The session put a line back as c while it was moved to d here and a line equal to c drawn
    // here too: the move here clashes with the session's.
    assert_eq!(m(&[&a], &[&c], &[&d, &c]), None);
    // The last of two copies around another line removed here while the session added a text:
    // which copy went decides what lies on top, and the session cannot tell, so a clash.
    assert_eq!(m(&[&a, &b, &a], &[&a, &b, &a, &text], &[&a, &b]), None);
    // Moved to different places on the two sides, with a text added here: still a clash.
    assert_eq!(m(&[&a], &[&c], &[&d, &text]), None);
    // Every copy around another line removed here: only one list results, so no clash.
    assert_eq!(
        m(&[&a, &b, &a], &[&a, &b, &a, &text], &[&b]),
        Some(vec![b.clone(), text.clone()])
    );
    // Some copies of one unbroken run removed here: which ones does not matter.
    assert_eq!(
        m(&[&b, &a, &a, &a], &[&b, &a, &a, &a, &text], &[&b, &a]),
        Some(vec![b.clone(), a.clone(), text.clone()])
    );
}

#[test]
fn a_guest_without_the_libraries_draws_every_symbol_of_the_project() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (_h, _log, host_open, _g, guest_open) = session_with(tmp.path(), |o| {
        o.place(1, "wc.coil", wc_core::Point::new(20.0, 120.0))?;
        // No library at all: the guest's copy can only have the symbol from the session.
        o.project.meta.libraries.clear();
        Ok(())
    })?;
    assert!(guest_open.project.meta.libraries.is_empty());
    assert_eq!(guest_open.symbols.get("wc.coil"), host_open.symbols.get("wc.coil"));
    assert!(guest_open.symbols.contains_key("wc.coil"));
    assert_eq!(
        store::read_vendored_symbol(&guest_open.dir, "wc.coil").ok().as_ref(),
        host_open.symbols.get("wc.coil")
    );
    Ok(())
}

/// Make every item `s` has sent due to be sent again, also one given up on: the next frame
/// tries it, however many tries slow frames have used up.
fn retry_due(s: &mut Share) {
    let past = std::time::Instant::now().checked_sub(ITEM_RETRY_LATER * 2);
    for out in s.items_out.values_mut() {
        if let Some(past) = past {
            out.at = past;
        }
    }
}

#[test]
fn a_pinned_copy_the_host_puts_back_during_a_session_reaches_the_guests() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session_with(tmp.path(), |o| {
        o.place(1, "wc.coil", wc_core::Point::new(20.0, 120.0))?;
        o.save()?;
        // Shared with the coil pinned and its copy gone.
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil")?)?;
        o.symbols.remove("wc.coil");
        Ok(())
    })?;
    // A guest that has no copy either (it took one from the library as it joined).
    std::fs::remove_file(store::vendor_path(&guest_open.dir, ItemKind::Symbol, "wc.coil")?)?;
    guest_open.symbols.remove("wc.coil");
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(store::read_vendored_symbol(&guest_open.dir, "wc.coil").is_err());
    let source = host_open.project.lock.symbols.get("wc.coil").map(|e| e.source.clone());
    let steps = match h.role.as_ref() {
        Some(Role::Host(hosting)) => hosting.host.history().len(),
        _ => return Err("still hosting".into()),
    };
    // Placed again while the host cannot store files: the host puts the pinned version back
    // from the library, the session refuses the file, and the placement waits for it.
    log.fail_vendor(true);
    let item = host_open.place(1, "wc.coil", wc_core::Point::new(60.0, 120.0))?;
    // The copy here names another library for the pin: the session's entry stays as it is.
    host_open
        .project
        .lock
        .symbols
        .get_mut("wc.coil")
        .ok_or("pinned")?
        .source = "elsewhere".into();
    let coil = host_open.symbols.get("wc.coil").cloned();
    assert!(coil.is_some());
    // The host's page as the session (and so the guest) has it.
    let drawn = |o: &Open| {
        o.project
            .page("p0002")
            .is_some_and(|p| p.symbols.iter().any(|s| s.id == item))
    };
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(store::read_vendored_symbol(&guest_open.dir, "wc.coil").is_err());
    assert!(!drawn(&guest_open), "the page waits for the file");
    // The host can store it again: the next try gets it in, and the page follows.
    log.fail_vendor(false);
    retry_due(&mut h);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(store::read_vendored_symbol(&guest_open.dir, "wc.coil").ok(), coil);
    assert_eq!(guest_open.symbols.get("wc.coil").cloned(), coil);
    assert!(drawn(&guest_open), "the placement reaches the guest");
    let messages = log.messages();
    let add = messages.iter().position(|m| m == "Add wc.coil to the project");
    let edit = messages.iter().position(|m| m.contains("Edit page 2"));
    assert!(add.is_some() && add < edit, "the file goes in first: {messages:?}");
    // The pin is the session's as it was, and the file alone is no step to undo.
    assert_eq!(
        guest_open.project.lock.symbols.get("wc.coil").map(|e| e.source.clone()),
        source
    );
    match h.role.as_ref() {
        Some(Role::Host(hosting)) => {
            assert_eq!(hosting.host.files().0.get("wc.coil").cloned(), coil);
            // The page edit, with the device the placement made (it waited with the page); the
            // file alone is no step to undo.
            assert_eq!(hosting.host.history().len(), steps + 1, "{:?}", log.messages());
            assert!(
                hosting
                    .host
                    .history()
                    .iter()
                    .all(|c| !c.message.starts_with("Add wc.coil")),
                "{:?}",
                log.messages()
            );
        }
        _ => return Err("still hosting".into()),
    }
    // Sent once.
    let n = log.messages().len();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(log.messages().len(), n);
    Ok(())
}

#[test]
fn a_new_placement_of_a_symbol_whose_file_the_session_lacks_waits_for_the_file() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session_with(tmp.path(), |o| {
        o.place(1, "wc.coil", wc_core::Point::new(20.0, 120.0))?;
        o.save()?;
        // Shared with the coil pinned and loaded, and its copy gone.
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil")?)?;
        Ok(())
    })?;
    let coil = host_open.symbols.get("wc.coil").cloned().ok_or("loaded")?;
    // A guest without a copy either (it took one from the library as it joined), so only the
    // host can bring the file in.
    std::fs::remove_file(store::vendor_path(&guest_open.dir, ItemKind::Symbol, "wc.coil")?)?;
    guest_open.symbols.remove("wc.coil");
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // Placed again from the copy in memory: the session still has no file of the coil.
    let item = host_open.place(1, "wc.coil", wc_core::Point::new(60.0, 120.0))?;
    let drawn = |o: &Open| {
        o.project
            .page("p0002")
            .is_some_and(|p| p.symbols.iter().any(|s| s.id == item))
    };
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(!drawn(&guest_open), "the placement waits for the file");
    assert_eq!(h.holding, ["symbol wc.coil"]);
    // The missing copy was looked for, and is not read again within the wait (made long here,
    // so a slow frame cannot end it).
    let later = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let (at, _) = h.repair_reads.get_mut("symbol wc.coil").ok_or("looked for")?;
    *at = later;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(h.repair_reads.get("symbol wc.coil").map(|(at, _)| *at), Some(later));
    // Put back, it is found at the next look, not before.
    store::write_vendored_symbol(&host_open.dir, &coil)?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(!log.messages().iter().any(|m| m == "Add wc.coil to the project"));
    assert!(!drawn(&guest_open));
    let past = std::time::Instant::now().checked_sub(ITEM_WAIT * 2).ok_or("clock")?;
    if let Some((at, _)) = h.repair_reads.get_mut("symbol wc.coil") {
        *at = past;
    }
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(store::read_vendored_symbol(&guest_open.dir, "wc.coil").ok(), Some(coil));
    assert!(drawn(&guest_open), "the placement follows the file");
    assert!(h.holding.is_empty());
    assert!(
        h.repair_reads.is_empty(),
        "the session has the file: nothing to look for"
    );
    let messages = log.messages();
    let add = messages.iter().position(|m| m == "Add wc.coil to the project");
    let edit = messages.iter().rposition(|m| m.contains("Edit page 2"));
    assert!(add.is_some() && add < edit, "the file goes in first: {messages:?}");
    Ok(())
}

#[test]
fn a_guests_new_placement_of_a_symbol_whose_file_the_session_lacks_reaches_other_guests_after_the_file() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session_with(tmp.path(), |o| {
        o.place(1, "wc.coil", wc_core::Point::new(20.0, 120.0))?;
        o.save()?;
        // Shared with the coil pinned and its copy gone: the session has no file of it.
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil")?)?;
        o.symbols.remove("wc.coil");
        Ok(())
    })?;
    // A second guest, without the library: it can draw the coil only from the session's file.
    let mut g2 = guest(&mut h, "Kari", &tmp.path().join("guest2"));
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    let ids = pending(&h);
    approve(&mut h, ids[0], true);
    let mut kari_open = g2.frame(None, None, false).ok_or("the second guest got no project")?;
    kari_open.project.meta.libraries.clear();
    let kari_copy = store::vendor_path(&kari_open.dir, ItemKind::Symbol, "wc.coil")?;
    match std::fs::remove_file(&kari_copy) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    assert!(!kari_copy.exists(), "the second guest has no copy");
    kari_open.symbols.remove("wc.coil");
    // A few frames of all three windows.
    let run = |h: &mut Share, ho: &mut Open, g: &mut Share, go: &mut Open, g2: &mut Share, ko: &mut Open| {
        for _ in 0..4 {
            h.frame(Some(ho), Some("p0002".into()), false);
            g.frame(Some(go), Some("p0003".into()), false);
            g2.frame(Some(ko), Some("p0002".into()), false);
        }
    };
    // The host cannot store files: the first guest's copy (taken from the library as it
    // joined) goes out as the missing file, and the session refuses it.
    log.fail_vendor(true);
    run(&mut h, &mut host_open, &mut g, &mut guest_open, &mut g2, &mut kari_open);
    assert!(!log.messages().iter().any(|m| m == "Add wc.coil to the project"));
    // The first guest places the coil meanwhile: the placement waits for the file.
    let item = guest_open.place(2, "wc.coil", wc_core::Point::new(60.0, 120.0))?;
    let coil = guest_open
        .symbols
        .get("wc.coil")
        .cloned()
        .ok_or("placed from the library")?;
    let drawn = |o: &Open| {
        o.project
            .page("p0003")
            .is_some_and(|p| p.symbols.iter().any(|s| s.id == item))
    };
    // An unrelated change of the guest's goes on meanwhile.
    let k9 = DeviceTag::parse("-K9")?;
    guest_open.project.devices.insert(k9.clone(), Device::default());
    run(&mut h, &mut host_open, &mut g, &mut guest_open, &mut g2, &mut kari_open);
    assert!(drawn(&guest_open), "the guest's own copy keeps it");
    assert!(!drawn(&kari_open), "the placement waits for the file");
    assert!(kari_open.project.devices.contains_key(&k9), "unrelated changes go on");
    // The session can store it again: the next try gets the file in, and the page follows.
    log.fail_vendor(false);
    retry_due(&mut g);
    run(&mut h, &mut host_open, &mut g, &mut guest_open, &mut g2, &mut kari_open);
    assert_eq!(kari_open.symbols.get("wc.coil"), Some(&coil));
    assert!(drawn(&kari_open), "the placement follows the file");
    assert!(g.holding.is_empty());
    let messages = log.messages();
    let add = messages.iter().position(|m| m == "Add wc.coil to the project");
    let edit = messages.iter().position(|m| m.contains("Edit page 3"));
    assert!(add.is_some() && add < edit, "the file goes in first: {messages:?}");
    Ok(())
}

#[test]
fn a_guest_with_the_pinned_copy_brings_in_the_file_the_session_lacks() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session_with(tmp.path(), |o| {
        o.place(1, "wc.coil", wc_core::Point::new(20.0, 120.0))?;
        o.save()?;
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil")?)?;
        o.symbols.remove("wc.coil");
        Ok(())
    })?;
    // The guest took the pinned version from the library as it joined.
    let coil = guest_open.symbols.get("wc.coil").cloned().ok_or("the guest's copy")?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    match h.role.as_ref() {
        Some(Role::Host(hosting)) => {
            assert_eq!(hosting.host.files().0.get("wc.coil"), Some(&coil));
            assert!(hosting.host.files_missing().is_empty());
        }
        _ => return Err("still hosting".into()),
    }
    match g.role.as_ref() {
        Some(Role::Guest(j)) => assert!(j.guest.files_missing().is_empty()),
        _ => return Err("still a guest".into()),
    }
    // Sent once.
    let n = log.messages().len();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(log.messages().len(), n);
    Ok(())
}

#[test]
fn a_guests_update_of_an_item_whose_file_the_session_lacks_is_kept_while_it_waits() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session_with(tmp.path(), |o| {
        o.place(1, "wc.coil", wc_core::Point::new(20.0, 120.0))?;
        o.save()?;
        std::fs::remove_file(store::vendor_path(&o.dir, ItemKind::Symbol, "wc.coil")?)?;
        o.symbols.remove("wc.coil");
        Ok(())
    })?;
    // The session cannot store files: the guest's copy goes out as the missing file and is
    // refused.
    log.fail_vendor(true);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // The guest updates the coil meanwhile: that update holds everything until it is answered,
    // or what comes in would put the session's entry back over it.
    let v1 = guest_open.symbols.get("wc.coil").cloned().ok_or("the guest's copy")?;
    let mut v2 = v1.clone();
    v2.name = "Coil, newer".into();
    store::write_vendored_symbol(&guest_open.dir, &v2)?;
    guest_open.project.lock.symbols.get_mut("wc.coil").ok_or("pin")?.hash = v2.hash();
    guest_open.symbols.insert("wc.coil".into(), v2.clone());
    let hash = |o: &Open| o.project.lock.symbols.get("wc.coil").map(|e| e.hash.clone());
    for _ in 0..3 {
        // Something of the host's comes in on every frame.
        let n = host_open.project.devices.len();
        host_open
            .project
            .devices
            .insert(DeviceTag::parse(&format!("-K{}", 50 + n))?, Device::default());
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
        assert_eq!(
            hash(&guest_open),
            Some(v2.hash()),
            "the guest keeps its update: {:?}",
            g.notes
        );
    }
    // Once the session can store it, the update goes in.
    log.fail_vendor(false);
    retry_due(&mut g);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(hash(&host_open), Some(v2.hash()), "{:?} {:?}", g.notes, h.notes);
    assert_eq!(hash(&guest_open), Some(v2.hash()));
    Ok(())
}

#[test]
fn a_symbol_used_for_the_first_time_reaches_the_host_with_its_file() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(!host_open.project.lock.symbols.contains_key("wc.contact.no"));

    let item = guest_open.place(2, "wc.contact.no", wc_core::Point::new(40.0, 100.0))?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));

    let ours = guest_open.symbols.get("wc.contact.no").cloned();
    assert!(ours.is_some());
    assert_eq!(
        host_open.symbols.get("wc.contact.no").cloned(),
        ours,
        "the host can draw it"
    );
    assert_eq!(
        host_open.project.lock.symbols.get("wc.contact.no"),
        guest_open.project.lock.symbols.get("wc.contact.no")
    );
    assert_eq!(store::read_vendored_symbol(&host_open.dir, "wc.contact.no").ok(), ours);
    assert!(
        host_open
            .project
            .page("p0003")
            .is_some_and(|p| p.symbols.iter().any(|s| s.id == item))
    );
    assert!(
        log.messages().iter().any(|m| m == "Add wc.contact.no to the project"),
        "{:?}",
        log.messages()
    );
    // Sent once: more frames send nothing new.
    let n = log.messages().len();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(log.messages().len(), n);
    Ok(())
}

#[test]
fn a_page_waits_until_the_session_has_the_symbols_it_draws() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    log.fail_vendor(true);
    let item = guest_open.place(2, "wc.contact.no", wc_core::Point::new(40.0, 100.0))?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let drawn = |o: &Open| {
        o.project
            .page("p0003")
            .is_some_and(|p| p.symbols.iter().any(|s| s.id == item))
    };
    assert!(!drawn(&host_open), "the page waits for its symbol");
    assert!(
        guest_open.project.lock.symbols.contains_key("wc.contact.no"),
        "the guest keeps its own entry"
    );
    match &g.role {
        Some(Role::Guest(j)) => assert!(
            !j.guest
                .project
                .as_ref()
                .is_some_and(|p| p.lock.symbols.contains_key("wc.contact.no")),
            "the session's copy has no entry the host refused"
        ),
        _ => return Err("not a guest".into()),
    }

    // The host can store it again: the next try gets it in, and the page follows.
    log.fail_vendor(false);
    retry_due(&mut g);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(drawn(&host_open));
    assert!(host_open.project.lock.symbols.contains_key("wc.contact.no"));
    let messages = log.messages();
    let add = messages.iter().position(|m| m == "Add wc.contact.no to the project");
    let edit = messages.iter().position(|m| m.contains("Edit page 3"));
    assert!(add.is_some() && add < edit, "{messages:?}");
    Ok(())
}

#[test]
fn a_page_waits_for_its_symbol_while_others_changes_come_in() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    log.fail_vendor(true);
    let item = guest_open.place(2, "wc.contact.no", wc_core::Point::new(40.0, 100.0))?;
    for _ in 0..=ITEM_TRIES {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
        std::thread::sleep(ITEM_WAIT);
    }
    // The host changes its own page while the guest's page waits.
    host_open.project.page_mut("p0002").ok_or("no page")?.title = "Motor, renamed".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let has_item = |o: &Open| {
        o.project
            .page("p0003")
            .is_some_and(|p| p.symbols.iter().any(|s| s.id == item))
    };
    assert!(!has_item(&host_open), "the page does not go without its symbol");
    assert!(has_item(&guest_open), "the guest keeps its waiting change");
    assert_eq!(
        guest_open.project.page("p0002").map(|p| p.title.as_str()),
        Some("Motor, renamed"),
        "others' changes come in meanwhile"
    );
    let notes = g.take_notes();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("did not take symbol wc.contact.no") && n.contains("wait until it does")),
        "{notes:?}"
    );

    // Once the host can store it, the symbol gets in and the page follows.
    log.fail_vendor(false);
    std::thread::sleep(ITEM_RETRY_LATER);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(host_open.project.lock.symbols.contains_key("wc.contact.no"));
    assert!(has_item(&host_open), "{:?} {:?}", g.notes, h.notes);
    assert_eq!(
        host_open.project.page("p0002").map(|p| p.title.as_str()),
        Some("Motor, renamed")
    );
    Ok(())
}

#[test]
fn a_symbol_someone_else_used_first_in_another_version_takes_the_sessions_version() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session_with(tmp.path(), |o| {
        o.place(1, "wc.coil", wc_core::Point::new(20.0, 120.0))?;
        Ok(())
    })?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let session_coil = host_open.symbols.get("wc.coil").cloned().ok_or("no coil")?;
    // The guest's copy holds another version, as when both used it at the same time.
    let mut other = session_coil.clone();
    other.name = "Coil, edited".into();
    store::write_vendored_symbol(&guest_open.dir, &other)?;
    guest_open.symbols.insert(other.id.clone(), other.clone());
    if let Some(e) = guest_open.project.lock.symbols.get_mut("wc.coil") {
        e.hash = other.hash();
    }
    // Taken up here before the session's version came in: not an update of it.
    if let Some(b) = g.base.as_mut() {
        b.lock.symbols.remove("wc.coil");
    }
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(guest_open.symbols.get("wc.coil"), Some(&session_coil));
    assert_eq!(
        guest_open.project.lock.symbols.get("wc.coil").map(|e| &e.hash),
        Some(&session_coil.hash())
    );
    assert_eq!(store::read_vendored_symbol(&guest_open.dir, "wc.coil")?, session_coil);
    assert!(
        g.take_notes()
            .iter()
            .any(|n| n.contains("symbol wc.coil first in another version"))
    );
    Ok(())
}

#[test]
fn an_unreadable_file_is_named_once_and_not_sent() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let item = guest_open.place(2, "wc.contact.no", wc_core::Point::new(40.0, 100.0))?;
    let sym = guest_open.symbols.get("wc.contact.no").cloned().ok_or("not loaded")?;
    let file = store::vendor_path(&guest_open.dir, ItemKind::Symbol, "wc.contact.no")?;
    std::fs::write(&file, "{ not json")?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let notes = g.take_notes();
    let named: Vec<_> = notes
        .iter()
        .filter(|n| n.contains("symbol wc.contact.no could not be read"))
        .collect();
    assert_eq!(named.len(), 1, "{notes:?}");
    assert!(!log.messages().iter().any(|m| m.starts_with("Add ")));
    let drawn = |o: &Open| {
        o.project
            .page("p0003")
            .is_some_and(|p| p.symbols.iter().any(|s| s.id == item))
    };
    assert!(!drawn(&host_open), "the page waits for the symbol it draws");

    // Readable again: the symbol goes out, then the page.
    store::write_vendored_symbol(&guest_open.dir, &sym)?;
    std::thread::sleep(ITEM_WAIT);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(host_open.symbols.get("wc.contact.no"), Some(&sym));
    assert!(drawn(&host_open));

    // The host's own unreadable files are named when it shares.
    std::fs::write(&file, "{ not json")?;
    let files = vendored_files(&guest_open.dir, &guest_open.project);
    assert!(files.unreadable.iter().any(|u| u.starts_with("symbol wc.contact.no (")));
    Ok(())
}

#[test]
fn a_file_in_another_version_than_its_lock_is_not_shared_or_taken() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut open = project(&tmp.path().join("p"))?;
    open.place(1, "wc.coil", wc_core::Point::new(20.0, 120.0))?;
    open.save()?;
    let mut edited = open.symbols.get("wc.coil").cloned().ok_or("no coil")?;
    edited.name = "Edited by hand".into();
    store::write_vendored_symbol(&open.dir, &edited)?;
    let files = vendored_files(&open.dir, &open.project);
    assert!(!files.symbols.contains_key("wc.coil"));
    assert!(
        files
            .unreadable
            .iter()
            .any(|u| u.starts_with("symbol wc.coil (") && u.contains("another version")),
        "{:?}",
        files.unreadable
    );

    // Offered by a session in another version than the lock names: not written, not loaded.
    open.symbols.remove("wc.coil");
    let offered = [("wc.coil".to_owned(), edited.clone())].into_iter().collect();
    let failed = take_files(&mut open, &offered, &BTreeMap::new());
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, "symbol wc.coil");
    assert!(!open.symbols.contains_key("wc.coil"));
    Ok(())
}

#[test]
fn a_symbol_taken_from_the_session_is_offered_in_the_symbol_list() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut open = project(&tmp.path().join("p"))?;
    open.place(1, "wc.coil", wc_core::Point::new(20.0, 120.0))?;
    let coil = open.symbols.remove("wc.coil").ok_or("no coil")?;
    open.library.retain(|(i, _)| i != "wc.coil");
    let offered = [("wc.coil".to_owned(), coil)].into_iter().collect();
    assert!(take_files(&mut open, &offered, &BTreeMap::new()).is_empty());
    assert!(open.symbols.contains_key("wc.coil"));
    assert!(open.library.iter().any(|(i, _)| i == "wc.coil"));
    // A newer definition under the same id is listed by its new name.
    let mut renamed = open.symbols["wc.coil"].clone();
    renamed.name = "Coil B".into();
    open.project.lock.symbols.get_mut("wc.coil").ok_or("pin")?.hash = renamed.hash();
    let offered = [("wc.coil".to_owned(), renamed)].into_iter().collect();
    assert!(take_files(&mut open, &offered, &BTreeMap::new()).is_empty());
    assert!(open.library.iter().any(|(i, n)| i == "wc.coil" && n == "Coil B"));
    Ok(())
}

#[test]
fn leaving_a_session_forgets_the_items_sent_to_it() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    log.fail_vendor(true);
    guest_open.place(2, "wc.contact.no", wc_core::Point::new(40.0, 100.0))?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(!g.items_out.is_empty(), "the refused symbol is still on its way");
    g.stop();
    assert!(g.items_out.is_empty() && g.unreadable.is_empty());
    Ok(())
}

#[test]
fn a_symbol_whose_file_becomes_unreadable_holds_only_the_change_that_uses_it() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    log.fail_vendor(true);
    let item = guest_open.place(2, "wc.contact.no", wc_core::Point::new(40.0, 100.0))?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let file = store::vendor_path(&guest_open.dir, ItemKind::Symbol, "wc.contact.no")?;
    std::fs::write(&file, "{ not json")?;
    // Meanwhile the host changes its own page.
    host_open.project.page_mut("p0002").ok_or("no page")?.title = "Motor, renamed".into();
    for _ in 0..=ITEM_TRIES {
        std::thread::sleep(ITEM_WAIT);
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(
        guest_open.project.page("p0002").map(|p| p.title.as_str()),
        Some("Motor, renamed"),
        "the session's changes come in again"
    );
    let has_item = |o: &Open| {
        o.project
            .page("p0003")
            .is_some_and(|p| p.symbols.iter().any(|s| s.id == item))
    };
    assert!(!has_item(&host_open), "the change that uses it still waits");
    let notes = g.take_notes();
    assert!(notes.iter().any(|n| n.contains("could not be read")), "{notes:?}");
    assert!(
        notes.iter().any(|n| n.contains("did not take symbol wc.contact.no")),
        "{notes:?}"
    );

    // Once the file can be read and the host can store it, the symbol still gets in.
    log.fail_vendor(false);
    let sym = guest_open.symbols.get("wc.contact.no").cloned().ok_or("not loaded")?;
    store::write_vendored_symbol(&guest_open.dir, &sym)?;
    std::thread::sleep(ITEM_RETRY_LATER);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(host_open.symbols.get("wc.contact.no"), Some(&sym), "sent again later");
    assert!(g.items_out.is_empty());
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(has_item(&host_open), "the waiting change follows it");
    Ok(())
}

#[test]
fn another_version_of_an_item_given_up_on_is_sent_without_waiting_for_its_retry() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    log.fail_vendor(true);
    guest_open.place(2, "wc.contact.no", wc_core::Point::new(40.0, 100.0))?;
    for _ in 0..=ITEM_TRIES {
        std::thread::sleep(ITEM_WAIT);
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert!(
        g.items_out.values().any(|o| o.gave_up),
        "the first version is given up on: {:?}",
        g.items_out
    );
    // The host can store files again, and the symbol is changed here: the new version goes
    // out at once, not after the first one's retry wait.
    log.fail_vendor(false);
    let mut sym = guest_open.symbols.get("wc.contact.no").cloned().ok_or("not loaded")?;
    sym.name = format!("{}, revised", sym.name);
    store::write_vendored_symbol(&guest_open.dir, &sym)?;
    guest_open
        .project
        .lock
        .symbols
        .get_mut("wc.contact.no")
        .ok_or("pinned")?
        .hash = sym.hash();
    guest_open.symbols.insert(sym.id.clone(), sym.clone());
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(host_open.symbols.get("wc.contact.no"), Some(&sym));
    Ok(())
}

#[test]
fn an_item_this_copy_no_longer_locks_stops_holding_changes() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    log.fail_vendor(true);
    guest_open.place(2, "wc.contact.no", wc_core::Point::new(40.0, 100.0))?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(!g.items_out.is_empty(), "the refused symbol is still on its way");
    // This copy stops using it, as when an assistant command rewrote the project.
    guest_open.project.lock.symbols.remove("wc.contact.no");
    for page in &mut guest_open.project.pages {
        page.symbols.retain(|s| s.symbol != "wc.contact.no");
    }
    guest_open.project.page_mut("p0003").ok_or("no page")?.title = "Valves, renamed".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(g.items_out.is_empty());
    assert_eq!(
        host_open.project.page("p0003").map(|p| p.title.as_str()),
        Some("Valves, renamed"),
        "changes go out again"
    );
    Ok(())
}

#[test]
fn a_session_version_that_cannot_be_written_is_not_drawn_with_the_old_one() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session_with(tmp.path(), |o| {
        o.place(1, "wc.coil", wc_core::Point::new(20.0, 120.0))?;
        Ok(())
    })?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let session_coil = host_open.symbols.get("wc.coil").cloned().ok_or("no coil")?;
    let mut other = session_coil.clone();
    other.name = "Coil, edited".into();
    guest_open.symbols.insert(other.id.clone(), other.clone());
    if let Some(e) = guest_open.project.lock.symbols.get_mut("wc.coil") {
        e.hash = other.hash();
    }
    // Taken up here before the session's version came in: not an update of it.
    if let Some(b) = g.base.as_mut() {
        b.lock.symbols.remove("wc.coil");
    }
    // The file cannot be written: a folder stands where it goes.
    let file = store::vendor_path(&guest_open.dir, ItemKind::Symbol, "wc.coil")?;
    std::fs::remove_file(&file)?;
    std::fs::create_dir_all(&file)?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(
        !guest_open.symbols.contains_key("wc.coil"),
        "the old version is not kept as if it were the session's"
    );
    let notes = g.take_notes();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("could not be taken") && n.contains("symbol wc.coil")),
        "{notes:?}"
    );

    // Writable again: the session's version is taken.
    std::fs::remove_dir(&file)?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(guest_open.symbols.get("wc.coil"), Some(&session_coil));
    Ok(())
}

#[test]
fn whole_record_edits_say_what_the_record_was_at_the_last_sync() -> TestResult {
    let mut base = Project::new("Demo");
    base.pages.push(Page::new("p0001", "1", "One"));
    let tag = wc_core::DeviceTag::parse("-K1")?;
    base.devices.insert(tag.clone(), wc_core::Device::default());
    let mut local = base.clone();
    local.devices.get_mut(&tag).ok_or("no device")?.description = "Relay".into();
    local.meta.name = "Renamed".into();
    let out = outgoing(&local, &base, None, &BTreeMap::new(), HOST);
    assert_eq!(out.edits.len(), 2, "{:?}", out.edits);
    for edit in &out.edits {
        let stamp = match edit {
            ProjectEdit::SetDevice { before, .. } | ProjectEdit::SetMeta { before, .. } => before.clone(),
            other => return Err(format!("unexpected {other:?}").into()),
        };
        assert!(stamp.is_some(), "{edit:?} carries what it was");
        assert_eq!(stamp, edit.record(&base), "made from the last sync, not from this copy");
        assert_ne!(stamp, edit.record(&local));
    }
    Ok(())
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really";

/// The bytes of the project file `file` in `dir`.
fn file_bytes(dir: &Path, file: &str) -> std::io::Result<Vec<u8>> {
    std::fs::read(dir.join(file))
}

#[test]
fn a_guest_gets_the_hosts_logo_and_underlays() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session_with(tmp.path(), |o| {
        std::fs::create_dir_all(o.dir.join("pictures"))?;
        std::fs::create_dir_all(o.dir.join("underlays"))?;
        std::fs::write(o.dir.join("pictures/logo.png"), PNG)?;
        std::fs::write(o.dir.join("underlays/p0002.jpg"), b"\xFF\xD8\xFFscan")?;
        o.project.meta.logo = Some(wc_core::Logo {
            file: "pictures/logo.png".into(),
            cover: None,
        });
        if let Some(p) = o.project.page_mut("p0002") {
            p.underlay = Some(wc_core::Underlay {
                file: "underlays/p0002.jpg".into(),
                min: wc_core::Point::new(0.0, 0.0),
                max: wc_core::Point::new(100.0, 100.0),
            });
        }
        Ok(())
    })?;
    // The window looked for the logo before it arrived; that is forgotten once it is there.
    guest_open
        .picture_problems
        .insert("pictures/logo.png".into(), "the file is missing".into());
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(file_bytes(&guest_open.dir, "pictures/logo.png")?, PNG);
    assert_eq!(
        file_bytes(&guest_open.dir, "underlays/p0002.jpg")?,
        file_bytes(&host_open.dir, "underlays/p0002.jpg")?
    );
    assert!(guest_open.picture_problems.is_empty());
    Ok(())
}

#[test]
fn a_logo_picked_by_a_guest_reaches_the_host_and_the_others() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    let mut k = guest(&mut h, "Kari", &tmp.path().join("kari"));
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    let ids = pending(&h);
    approve(&mut h, ids[0], true);
    let mut kari_open = k.frame(None, None, false).ok_or("Kari got no project")?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));

    let src = tmp.path().join("Acme logo.png");
    std::fs::write(&src, PNG)?;
    let file = guest_open.set_logo_from(&src)?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
        k.frame(Some(&mut kari_open), Some("p0001".into()), false);
    }
    assert_eq!(
        host_open.project.meta.logo.as_ref().map(|l| l.file.as_str()),
        Some(file.as_str())
    );
    assert_eq!(kari_open.project.meta.logo, host_open.project.meta.logo);
    for dir in [&host_open.dir, &kari_open.dir] {
        assert_eq!(file_bytes(dir, &file)?, PNG, "{}", dir.display());
    }
    assert!(
        log.messages().iter().any(|m| m == "Change project properties"),
        "{:?}",
        log.messages()
    );
    // Sent once: more frames send nothing new.
    let n = log.messages().len();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(log.messages().len(), n);
    Ok(())
}

#[test]
fn a_picture_replaced_here_under_its_name_reaches_the_session_under_a_name_of_its_own() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let src = tmp.path().join("logo.png");
    std::fs::write(&src, PNG)?;
    let file = guest_open.set_logo_from(&src)?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(file_bytes(&host_open.dir, &file)?, PNG);
    // Written again with the same picture: nothing goes out.
    let n = log.messages().len();
    std::fs::write(guest_open.dir.join(&file), PNG)?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(log.messages().len(), n, "{:?}", log.messages());
    // The file is replaced in the folder under the same name.
    let new = b"\x89PNG\r\n\x1a\nanother logo".to_vec();
    std::fs::write(guest_open.dir.join(&file), &new)?;
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    // The session keeps its file of that name; the new one goes in under a name of its own,
    // which the project names everywhere.
    let renamed = guest_open
        .project
        .meta
        .logo
        .as_ref()
        .map(|l| l.file.clone())
        .ok_or("no logo")?;
    assert_ne!(renamed, file);
    assert_eq!(file_bytes(&guest_open.dir, &renamed)?, new);
    assert_eq!(host_open.project.meta.logo, guest_open.project.meta.logo);
    assert_eq!(file_bytes(&host_open.dir, &renamed)?, new);
    assert_eq!(file_bytes(&host_open.dir, &file)?, PNG);
    // Once: more frames send nothing new.
    let n = log.messages().len();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(log.messages().len(), n);
    Ok(())
}

#[test]
fn the_session_picture_a_renamed_replacement_left_shows_here_and_is_not_sent_again() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let src = tmp.path().join("logo.png");
    std::fs::write(&src, PNG)?;
    let file = guest_open.set_logo_from(&src)?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    // The guest replaces the session's picture in its folder: refused, and renamed here.
    let new = b"\x89PNG\r\n\x1a\nanother logo".to_vec();
    std::fs::write(guest_open.dir.join(&file), &new)?;
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let renamed = guest_open
        .project
        .meta
        .logo
        .as_ref()
        .map(|l| l.file.clone())
        .ok_or("no logo")?;
    assert_ne!(renamed, file);
    assert_eq!(file_bytes(&guest_open.dir, &renamed)?, new);
    // What is left under the old name is the session's picture again.
    assert_eq!(file_bytes(&guest_open.dir, &file)?, PNG);
    g.take_notes();
    // The host then uses its picture of the old name as page 2's underlay.
    host_open.project.page_mut("p0002").ok_or("no page")?.underlay = Some(wc_core::Underlay {
        file: file.clone(),
        min: wc_core::Point::new(0.0, 0.0),
        max: wc_core::Point::new(10.0, 10.0),
    });
    let n = log.messages().len();
    for _ in 0..6 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let underlay = |o: &Open| {
        o.project
            .page("p0002")
            .and_then(|p| p.underlay.as_ref())
            .map(|u| u.file.clone())
    };
    assert_eq!(underlay(&host_open).as_deref(), Some(file.as_str()));
    assert_eq!(underlay(&guest_open).as_deref(), Some(file.as_str()));
    assert_eq!(file_bytes(&guest_open.dir, &file)?, PNG);
    assert_eq!(file_bytes(&host_open.dir, &file)?, PNG);
    // Nothing is sent again or renamed.
    let notes = g.take_notes();
    assert!(!notes.iter().any(|n| n.contains("renamed")), "{notes:?}");
    assert_eq!(log.messages().len(), n + 1, "{:?}", &log.messages()[n..]);
    Ok(())
}

#[test]
fn a_put_back_that_reports_an_error_shows_what_the_name_holds_now() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let src = tmp.path().join("logo.png");
    std::fs::write(&src, PNG)?;
    let file = guest_open.set_logo_from(&src)?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    // The guest replaces the session's picture in its folder; the window shows it. Putting the
    // session's file back under the name then goes through, but reports an error.
    std::fs::write(guest_open.dir.join(&file), b"\x89PNG\r\n\x1a\nanother logo")?;
    guest_open.underlays.insert(file.clone(), None);
    super::PUT_BACK_FAILS.with(|f| f.set(true));
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    super::PUT_BACK_FAILS.with(|f| f.set(false));
    let notes = g.take_notes();
    assert!(notes.iter().any(|n| n.contains("could not be put back")), "{notes:?}");
    assert_eq!(file_bytes(&guest_open.dir, &file)?, PNG);
    // What the window showed under the name is read again.
    assert!(!guest_open.underlays.contains_key(&file));
    Ok(())
}

#[test]
fn a_picture_the_host_replaced_in_its_folder_gets_a_name_of_its_own() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let src = tmp.path().join("logo.png");
    std::fs::write(&src, PNG)?;
    let file = guest_open.set_logo_from(&src)?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(file_bytes(&host_open.dir, &file)?, PNG);
    // The host replaces the file it took in its folder, under the same name.
    let new = b"\x89PNG\r\n\x1a\nthe host's logo".to_vec();
    std::fs::write(host_open.dir.join(&file), &new)?;
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let renamed = host_open
        .project
        .meta
        .logo
        .as_ref()
        .map(|l| l.file.clone())
        .ok_or("no logo")?;
    assert_ne!(renamed, file);
    assert_eq!(guest_open.project.meta.logo, host_open.project.meta.logo);
    assert_eq!(file_bytes(&guest_open.dir, &renamed)?, new);
    assert_eq!(file_bytes(&guest_open.dir, &file)?, PNG);
    assert!(!h.take_notes().iter().any(|n| n.contains("not kept")));
    Ok(())
}

/// A logo the guest picked, taken in by the host: the session, its journal and both copies, and
/// the logo's name.
fn session_with_a_guests_logo(tmp: &Path) -> TestResult<(Share, Log, Open, Share, Open, String)> {
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp)?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let src = tmp.join("logo.png");
    std::fs::write(&src, PNG)?;
    let file = guest_open.set_logo_from(&src)?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(file_bytes(&host_open.dir, &file)?, PNG);
    Ok((h, log, host_open, g, guest_open, file))
}

/// The files in the folder of picture `file` in `dir`.
fn picture_folder(dir: &Path, file: &str) -> std::io::Result<Vec<std::ffi::OsString>> {
    let folder = dir.join(file).parent().map(Path::to_path_buf).unwrap_or_default();
    let mut names: Vec<_> = std::fs::read_dir(folder)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<std::io::Result<_>>()?;
    names.sort();
    Ok(names)
}

#[test]
fn a_picture_a_guest_put_back_before_the_session_refused_its_replacement_is_not_renamed() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open, file) = session_with_a_guests_logo(tmp.path())?;
    let folder = picture_folder(&guest_open.dir, &file)?;
    let n = log.messages().len();
    // Replaced, and sent as a replacement in one frame of the guest's.
    std::fs::write(guest_open.dir.join(&file), b"\x89PNG\r\n\x1a\nanother logo")?;
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert!(g.pictures_replaced.contains(&file), "sent as a replacement");
    // The window found nothing to show meanwhile (the file was gone for a moment).
    guest_open.underlays.insert(file.clone(), None);
    guest_open.picture_problems.insert(file.clone(), "missing".into());
    // Put back before the refusal comes in.
    std::fs::write(guest_open.dir.join(&file), PNG)?;
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let named = |o: &Open| o.project.meta.logo.as_ref().map(|l| l.file.clone());
    assert_eq!(named(&guest_open), Some(file.clone()), "not renamed");
    assert_eq!(named(&host_open), Some(file.clone()));
    assert_eq!(
        picture_folder(&guest_open.dir, &file)?,
        folder,
        "no copy under a new name"
    );
    assert_eq!(file_bytes(&host_open.dir, &file)?, PNG);
    assert_eq!(log.messages().len(), n, "{:?}", log.messages());
    assert!(!g.take_notes().iter().any(|n| n.contains("renamed")));
    // Shown again: what the window found before is forgotten.
    assert!(!guest_open.underlays.contains_key(&file));
    assert!(!guest_open.picture_problems.contains_key(&file));
    // Taken as the session's again: more frames send nothing.
    assert!(g.pictures_replaced.is_empty());
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(named(&guest_open), Some(file));
    assert_eq!(log.messages().len(), n);
    Ok(())
}

#[test]
fn a_picture_the_host_put_back_before_its_replacement_was_refused_is_not_renamed() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open, file) = session_with_a_guests_logo(tmp.path())?;
    let folder = picture_folder(&host_open.dir, &file)?;
    let n = log.messages().len();
    // Replaced in the host's folder, and sent as a replacement in one frame of the host's.
    std::fs::write(host_open.dir.join(&file), b"\x89PNG\r\n\x1a\nthe host's logo")?;
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    assert!(h.pictures_replaced.contains(&file), "sent as a replacement");
    // Put back before the refusal is handled.
    std::fs::write(host_open.dir.join(&file), PNG)?;
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let named = |o: &Open| o.project.meta.logo.as_ref().map(|l| l.file.clone());
    assert_eq!(named(&host_open), Some(file.clone()), "not renamed");
    assert_eq!(named(&guest_open), Some(file.clone()));
    assert_eq!(
        picture_folder(&host_open.dir, &file)?,
        folder,
        "no copy under a new name"
    );
    assert_eq!(file_bytes(&guest_open.dir, &file)?, PNG);
    assert_eq!(log.messages().len(), n, "{:?}", log.messages());
    assert!(h.pictures_replaced.is_empty());
    Ok(())
}

#[test]
fn an_empty_file_put_in_place_of_a_picture_is_named_once_and_not_sent() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let src = tmp.path().join("logo.png");
    std::fs::write(&src, PNG)?;
    let file = guest_open.set_logo_from(&src)?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    g.take_notes();
    let n = log.messages().len();
    std::fs::write(guest_open.dir.join(&file), b"")?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(log.messages().len(), n, "{:?}", log.messages());
    let notes = g.take_notes();
    assert_eq!(
        notes.iter().filter(|n| n.contains("cannot be shared")).count(),
        1,
        "{notes:?}"
    );
    assert_eq!(file_bytes(&host_open.dir, &file)?, PNG);
    Ok(())
}

#[test]
fn a_logo_picked_by_the_host_reaches_the_guest() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let src = tmp.path().join("logo.png");
    std::fs::write(&src, PNG)?;
    let file = host_open.set_logo_from(&src)?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(guest_open.project.meta.logo, host_open.project.meta.logo);
    assert_eq!(file_bytes(&guest_open.dir, &file)?, PNG);
    Ok(())
}

#[test]
fn a_picture_outside_the_picture_folders_is_named_once_and_not_sent() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    std::fs::write(guest_open.dir.join("logo.png"), PNG)?;
    guest_open.project.meta.logo = Some(wc_core::Logo {
        file: "logo.png".into(),
        cover: None,
    });
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let notes: Vec<String> = g.take_notes().into_iter().filter(|n| n.contains("logo.png")).collect();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(h.role.as_ref().is_some_and(|r| r.pictures().is_empty()));
    // The change that names it still goes.
    assert_eq!(host_open.project.meta.logo, guest_open.project.meta.logo);
    Ok(())
}

#[test]
fn a_picture_picked_here_under_a_session_name_is_kept_and_shared_under_a_name_of_its_own() -> TestResult {
    let tmp = tempfile::tempdir()?;
    // The session has a picture under a name this copy does not use (yet).
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session_sharing(
        tmp.path(),
        |_| Ok(()),
        [("pictures/brand.png".to_owned(), PNG.to_vec())].into(),
    )?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(
        g.role.as_ref().and_then(|r| r.pictures().get("pictures/brand.png")),
        Some(&PNG.to_vec()),
        "the guest has the session's picture"
    );
    // The guest picks its own picture of that name.
    let mine = b"\x89PNG\r\n\x1a\nmine".to_vec();
    std::fs::create_dir_all(guest_open.dir.join("pictures"))?;
    std::fs::write(guest_open.dir.join("pictures/brand.png"), &mine)?;
    guest_open.project.meta.logo = Some(wc_core::Logo {
        file: "pictures/brand.png".into(),
        cover: None,
    });
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(file_bytes(&guest_open.dir, "pictures/brand.png")?, mine, "kept");
    // The project names it under a name of its own, and so does the session.
    let file = guest_open
        .project
        .meta
        .logo
        .as_ref()
        .map(|l| l.file.clone())
        .ok_or("no logo")?;
    assert_ne!(file, "pictures/brand.png");
    assert_eq!(file_bytes(&guest_open.dir, &file)?, mine);
    assert_eq!(host_open.project.meta.logo, guest_open.project.meta.logo);
    assert_eq!(file_bytes(&host_open.dir, &file)?, mine);
    let notes: Vec<String> = g
        .take_notes()
        .into_iter()
        .filter(|n| n.contains("pictures/brand.png"))
        .collect();
    assert_eq!(notes.len(), 1, "named once: {notes:?}");
    assert!(notes[0].contains(&format!("renamed {file} here")), "{}", notes[0]);
    Ok(())
}

#[test]
fn a_picked_picture_whose_change_the_session_refused_leaves_the_sessions_picture_alone() -> TestResult {
    let tmp = tempfile::tempdir()?;
    // The session has a picture brand.png that its drawings do not use.
    let (mut h, log, mut host_open, mut g, mut guest_open) = session_sharing(
        tmp.path(),
        |_| Ok(()),
        [("pictures/brand.png".to_owned(), PNG.to_vec())].into(),
    )?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // The guest picks its own picture of that name as the logo: it gets a name of its own here
    // first, and the change then goes out under that name; the session will refuse it.
    log.fail_guests(true);
    let mine = b"\x89PNG\r\n\x1a\nmine".to_vec();
    std::fs::create_dir_all(guest_open.dir.join("pictures"))?;
    std::fs::write(guest_open.dir.join("pictures/brand.png"), &mine)?;
    guest_open.project.meta.logo = Some(wc_core::Logo {
        file: "pictures/brand.png".into(),
        cover: None,
    });
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    // Meanwhile the host makes the session's picture of that name its logo: once the guest's
    // change is refused, the logo here is the session's.
    let theirs = Some(wc_core::Logo {
        file: "pictures/brand.png".into(),
        cover: None,
    });
    host_open.project.meta.logo = theirs.clone();
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(host_open.project.meta.logo, theirs, "{:?}", g.notes);
    assert_eq!(guest_open.project.meta.logo, theirs, "not renamed: {:?}", g.notes);
    assert_eq!(file_bytes(&guest_open.dir, "pictures/brand.png")?, mine, "kept");
    assert_eq!(
        h.role
            .as_ref()
            .and_then(|r| r.pictures().get("pictures/brand.png").cloned()),
        Some(PNG.to_vec()),
        "the session's picture is not replaced"
    );
    assert_eq!(file_bytes(&host_open.dir, "pictures/brand.png")?, PNG);
    Ok(())
}

#[test]
fn a_pick_is_withdrawn_by_the_refusal_of_the_edit_that_carried_it_only() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session_sharing(tmp.path(), |_| Ok(()), BTreeMap::new())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // The guest picks its own a.png as the logo; the change that carries it goes out.
    let file = "pictures/a.png".to_owned();
    std::fs::create_dir_all(guest_open.dir.join("pictures"))?;
    std::fs::write(guest_open.dir.join(&file), PNG)?;
    guest_open.project.meta.logo = Some(wc_core::Logo {
        file: file.clone(),
        cover: None,
    });
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    let key = (file.clone(), PicRef::Logo);
    let carrier = g
        .pick_carriers
        .get(&key)
        .copied()
        .flatten()
        .ok_or("no carrier recorded")?;
    let Some(Role::Guest(j)) = g.role.as_ref() else {
        return Err("not a guest".into());
    };
    assert_eq!(carrier, j.guest.last_edit(), "the logo went out last");
    let refuse = |g: &mut Share, seq: u64| -> TestResult {
        let Some(Role::Guest(j)) = g.role.as_mut() else {
            return Err("not a guest".into());
        };
        j.guest.refused_meta = true;
        j.guest.refused_edits.insert(seq);
        Ok(())
    };
    // A refused later change of the project settings (a rename, say) leaves the pick: the
    // session took the edit that carried it.
    refuse(&mut g, carrier + 1)?;
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert!(
        g.picks.get(&file).is_some_and(|refs| refs.contains(&PicRef::Logo)),
        "{:?}",
        g.notes
    );
    // The refusal of the carrying edit withdraws it.
    refuse(&mut g, carrier)?;
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert!(!g.picks.contains_key(&file), "{:?}", g.notes);
    assert!(!g.pick_carriers.contains_key(&key));
    Ok(())
}

#[test]
fn a_pick_sent_with_a_page_edit_keeps_the_target_rule() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session_sharing(tmp.path(), |_| Ok(()), BTreeMap::new())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // The guest picks its own a.png as the underlay of the page it has open: that goes out with
    // the page edit, which has no number.
    let file = "pictures/a.png".to_owned();
    std::fs::create_dir_all(guest_open.dir.join("pictures"))?;
    std::fs::write(guest_open.dir.join(&file), PNG)?;
    guest_open.project.page_mut("p0003").ok_or("no page")?.underlay = Some(wc_core::Underlay {
        file: file.clone(),
        min: wc_core::Point::new(0.0, 0.0),
        max: wc_core::Point::new(10.0, 10.0),
    });
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    let key = (file.clone(), PicRef::Underlay("p0003".into()));
    assert_eq!(g.pick_carriers.get(&key), Some(&None), "{:?}", g.pick_carriers);
    // A later project edit that names the picture there again does not take the pick over.
    let later = ProjectEdit::SetPages {
        pages: vec![guest_open.project.page("p0003").ok_or("no page")?.clone()],
        before: Vec::new(),
    };
    assert!(super::first_carried(&g.picks, &g.pick_carriers, |r, f| super::carries(&later, r, f)).is_empty());
    // So a refused change of the page withdraws it, whatever edit was refused.
    let Some(Role::Guest(j)) = g.role.as_mut() else {
        return Err("not a guest".into());
    };
    j.guest.refused_pages.insert("p0003".into());
    j.guest.refused_edits.insert(99);
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert!(
        !g.picks.get(&file).is_some_and(|refs| refs.contains(&key.1)),
        "{:?}",
        g.picks
    );
    Ok(())
}

#[test]
fn a_carried_pick_is_withdrawn_only_with_its_own_edit() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut open = project(tmp.path())?;
    let a = "pictures/a.png".to_owned();
    open.project.meta.logo = Some(wc_core::Logo {
        file: a.clone(),
        cover: None,
    });
    let mut picks: BTreeMap<String, BTreeSet<PicRef>> = [(a.clone(), [PicRef::Logo].into())].into();
    let mut carriers: BTreeMap<(String, PicRef), Option<u64>> = [((a.clone(), PicRef::Logo), Some(5))].into();
    let mut due = BTreeMap::new();
    let mut clashes = Vec::new();
    // A refused edit 7 touched the project settings: the pick edit 5 carried stays.
    let gone = withdraw_picks(
        &mut picks,
        &mut carriers,
        &mut due,
        &mut clashes,
        &open,
        ((BTreeSet::new(), true, [7].into()), &BTreeSet::new()),
    );
    assert!(gone.is_empty());
    assert_eq!(carriers.len(), 1);
    // Edit 5 itself refused: the pick goes, and so does its carrier.
    let gone = withdraw_picks(
        &mut picks,
        &mut carriers,
        &mut due,
        &mut clashes,
        &open,
        ((BTreeSet::new(), false, [5].into()), &BTreeSet::new()),
    );
    assert_eq!(gone, [a].into());
    assert!(picks.is_empty() && carriers.is_empty());
    Ok(())
}

#[test]
fn a_session_picture_replaced_here_after_a_withdrawn_pick_of_its_name_is_renamed_and_shared() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session_sharing(
        tmp.path(),
        |_| Ok(()),
        [("pictures/brand.png".to_owned(), PNG.to_vec())].into(),
    )?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // The guest's pick of its own brand.png as the logo is refused, and the host makes the
    // session's picture of that name its logo: the pick is withdrawn.
    log.fail_guests(true);
    let file = "pictures/brand.png";
    std::fs::create_dir_all(guest_open.dir.join("pictures"))?;
    std::fs::write(guest_open.dir.join(file), b"\x89PNG\r\n\x1a\nmine")?;
    guest_open.project.meta.logo = Some(wc_core::Logo {
        file: file.into(),
        cover: None,
    });
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    host_open.project.meta.logo = Some(wc_core::Logo {
        file: file.into(),
        cover: None,
    });
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    log.fail_guests(false);
    // The guest puts the session's picture in place, and later replaces it with another file
    // of that name: the replacement is renamed everywhere and shared under its new name.
    std::fs::write(guest_open.dir.join(file), PNG)?;
    std::thread::sleep(ITEM_RETRY_LATER + std::time::Duration::from_millis(50));
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let new = b"\x89PNG\r\n\x1a\nanother logo".to_vec();
    std::fs::write(guest_open.dir.join(file), &new)?;
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let renamed = guest_open
        .project
        .meta
        .logo
        .as_ref()
        .map(|l| l.file.clone())
        .ok_or("no logo")?;
    assert_ne!(renamed, file, "{:?}", g.notes);
    assert_eq!(file_bytes(&guest_open.dir, &renamed)?, new);
    assert_eq!(host_open.project.meta.logo, guest_open.project.meta.logo);
    assert_eq!(file_bytes(&host_open.dir, &renamed)?, new);
    assert_eq!(file_bytes(&host_open.dir, file)?, PNG);
    Ok(())
}

#[test]
fn a_pick_made_after_a_refused_one_was_sent_is_kept_through_its_refusal() -> TestResult {
    let tmp = tempfile::tempdir()?;
    // The session has a picture brand.png that its drawings do not use.
    let (mut h, log, mut host_open, mut g, mut guest_open) = session_sharing(
        tmp.path(),
        |_| Ok(()),
        [("pictures/brand.png".to_owned(), PNG.to_vec())].into(),
    )?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    std::fs::create_dir_all(guest_open.dir.join("pictures"))?;
    std::fs::write(guest_open.dir.join("pictures/a.png"), PNG)?;
    let mine = b"\x89PNG\r\n\x1a\nmine".to_vec();
    std::fs::write(guest_open.dir.join("pictures/brand.png"), &mine)?;
    // The guest's logo a.png goes out, and the session refuses it.
    log.fail_guests(true);
    guest_open.project.meta.logo = Some(wc_core::Logo {
        file: "pictures/a.png".into(),
        cover: None,
    });
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    log.fail_guests(false);
    // The refusal takes the logo back in the guest's next frame.
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert_eq!(guest_open.project.meta.logo, None, "{:?}", g.notes);
    // As when the refusal comes with the last answer of that frame, after the pictures were
    // looked at: what it refused is still to be gone through at the next frame.
    let Some(Role::Guest(j)) = g.role.as_mut() else {
        return Err("not a guest".into());
    };
    j.guest.refused_meta = true;
    // Before the next frame, the guest picks its own brand.png as the logo: that pick is
    // kept, and the picture gets a name of its own.
    guest_open.project.meta.logo = Some(wc_core::Logo {
        file: "pictures/brand.png".into(),
        cover: None,
    });
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(file_bytes(&guest_open.dir, "pictures/brand.png")?, mine, "kept");
    let file = guest_open
        .project
        .meta
        .logo
        .as_ref()
        .map(|l| l.file.clone())
        .ok_or("no logo")?;
    assert_ne!(file, "pictures/brand.png", "{:?}", g.notes);
    assert_eq!(file_bytes(&guest_open.dir, &file)?, mine);
    assert_eq!(host_open.project.meta.logo, guest_open.project.meta.logo);
    assert_eq!(file_bytes(&host_open.dir, &file)?, mine);
    Ok(())
}

#[test]
fn a_picture_refused_while_a_symbol_waits_is_renamed_only_where_it_was_picked() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), _item) = waiting_session(tmp.path())?;
    let file = "pictures/a.png";
    // The host's page 2 has a.png as its underlay, a picture nobody has yet.
    host_open.project.page_mut("p0002").ok_or("no page")?.underlay = Some(wc_core::Underlay {
        file: file.into(),
        min: wc_core::Point::new(0.0, 0.0),
        max: wc_core::Point::new(10.0, 10.0),
    });
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let underlay = |o: &Open| {
        o.project
            .page("p0002")
            .and_then(|p| p.underlay.as_ref())
            .map(|u| u.file.clone())
    };
    assert_eq!(underlay(&guest_open).as_deref(), Some(file));
    // Looked for here meanwhile, and missing: looked at again only after a while.
    std::thread::sleep(ITEM_RETRY_LATER + std::time::Duration::from_millis(50));
    // The guest picks its own a.png as the logo, while the host's a.png comes into the session:
    // the session refuses the guest's.
    let mine = b"\x89PNG\r\n\x1a\nmine".to_vec();
    std::fs::create_dir_all(guest_open.dir.join("pictures"))?;
    std::fs::write(guest_open.dir.join(file), &mine)?;
    guest_open.project.meta.logo = Some(wc_core::Logo {
        file: file.into(),
        cover: None,
    });
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    if let Some(Role::Host(hosting)) = h.role.as_mut() {
        let mut all = hosting.host.pictures().clone();
        all.insert(file.into(), PNG.to_vec());
        hosting.host.share_pictures(all);
    }
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert!(!g.items_out.is_empty(), "the symbol still waits");
    // The guest's logo gets a name of its own; the host's underlay keeps its picture.
    let logo = guest_open
        .project
        .meta
        .logo
        .as_ref()
        .map(|l| l.file.clone())
        .ok_or("no logo")?;
    assert_ne!(logo, file, "{:?}", g.notes);
    assert_eq!(file_bytes(&guest_open.dir, &logo)?, mine);
    assert_eq!(underlay(&guest_open).as_deref(), Some(file), "{:?}", g.notes);
    assert_eq!(underlay(&host_open).as_deref(), Some(file));
    Ok(())
}

#[test]
fn a_late_refusal_of_a_withdrawn_picks_picture_is_told_from_a_later_attempts() {
    let (a, b) = ("pictures/a.png".to_owned(), "pictures/b.png".to_owned());
    // a.png went out with edit 3, its pick was withdrawn, and a replacement went out with 5.
    let sent: BTreeMap<String, Option<u64>> = [(a.clone(), Some(5)), (b.clone(), Some(2))].into();
    let withdrawn: BTreeMap<String, Option<u64>> = [(a.clone(), Some(3))].into();
    // The refusal of edit 3 comes after the replacement went out: it renames nothing.
    assert!(names_to_rename(vec![(a.clone(), Some(3))], &sent, &withdrawn).is_empty());
    // The replacement's own refusal does; one of a picture not withdrawn does too.
    assert_eq!(
        names_to_rename(vec![(a.clone(), Some(5)), (b.clone(), Some(2))], &sent, &withdrawn),
        vec![a.clone(), b.clone()]
    );
    // Withdrawn before anything of it went out: any refusal is of a later attempt.
    let early: BTreeMap<String, Option<u64>> = [(a.clone(), None)].into();
    assert_eq!(
        names_to_rename(vec![(a.clone(), Some(5))], &sent, &early),
        vec![a.clone()]
    );
    // The host's refusals carry no edit: a withdrawn pick's picture is not renamed.
    assert!(names_to_rename(vec![(a.clone(), None)], &sent, &withdrawn).is_empty());
    // Renamed after the refusal of edit 5: another refusal of that edit (the picture went out
    // twice) renames nothing; one of the picture sent again (edit 7) does.
    let mut settled = BTreeMap::new();
    note_withdrawn(&mut settled, &sent, [a.clone()].into());
    assert!(names_to_rename(vec![(a.clone(), Some(5))], &sent, &settled).is_empty());
    let again: BTreeMap<String, Option<u64>> = [(a.clone(), Some(7))].into();
    assert_eq!(
        names_to_rename(vec![(a.clone(), Some(7))], &again, &settled),
        vec![a.clone()]
    );
    // Never sent from here: nothing.
    assert!(names_to_rename(vec![("pictures/c.png".into(), Some(9))], &sent, &BTreeMap::new()).is_empty());
}

#[test]
fn the_same_pick_made_again_after_its_refusal_put_the_logo_back_is_kept() -> TestResult {
    let tmp = tempfile::tempdir()?;
    // The session has a picture brand.png that its drawings do not use.
    let (mut h, log, mut host_open, mut g, mut guest_open) = session_sharing(
        tmp.path(),
        |_| Ok(()),
        [("pictures/brand.png".to_owned(), PNG.to_vec())].into(),
    )?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let file = "pictures/brand.png";
    let mine = b"\x89PNG\r\n\x1a\nmine".to_vec();
    std::fs::create_dir_all(guest_open.dir.join("pictures"))?;
    std::fs::write(guest_open.dir.join(file), &mine)?;
    let logo = Some(wc_core::Logo {
        file: file.into(),
        cover: None,
    });
    // The guest's pick of its own brand.png as the logo goes out, and the session refuses it.
    // (The session's picture of that name is compared with the guest's only after a while.)
    g.pictures_failed.insert(file.into(), std::time::Instant::now());
    log.fail_guests(true);
    guest_open.project.meta.logo = logo.clone();
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    log.fail_guests(false);
    // The refusal comes with the last answer of the guest's next frame, after the pictures
    // were looked at: that frame puts the logo back, and what was refused is gone through at
    // the frame after.
    let role = g.role.as_mut().ok_or("no role")?;
    role.pump(&mut g.notes);
    let Role::Guest(j) = role else {
        return Err("not a guest".into());
    };
    let refused = std::mem::take(&mut j.guest.refused_meta);
    assert!(refused, "the logo's change was refused");
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert_eq!(guest_open.project.meta.logo, None, "put back: {:?}", g.notes);
    if let Some(Role::Guest(j)) = g.role.as_mut() {
        j.guest.refused_meta = refused;
    }
    // Before that frame, the guest picks the same picture again: that pick is kept, and the
    // picture gets a name of its own once compared.
    guest_open.project.meta.logo = logo;
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    std::thread::sleep(ITEM_RETRY_LATER + std::time::Duration::from_millis(50));
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(file_bytes(&guest_open.dir, file)?, mine, "kept");
    let renamed = guest_open
        .project
        .meta
        .logo
        .as_ref()
        .map(|l| l.file.clone())
        .ok_or("no logo")?;
    assert_ne!(renamed, file, "{:?}", g.notes);
    assert_eq!(file_bytes(&guest_open.dir, &renamed)?, mine);
    assert_eq!(host_open.project.meta.logo, guest_open.project.meta.logo);
    assert_eq!(file_bytes(&host_open.dir, &renamed)?, mine);
    Ok(())
}

#[test]
fn a_refusal_coming_after_its_pictures_clash_rename_renames_nothing_more() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let file = "pictures/brand.png";
    // The session has brand.png, under the host's page 2.
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session_sharing(
        tmp.path(),
        |o| {
            o.project.page_mut("p0002").ok_or("no page")?.underlay = Some(wc_core::Underlay {
                file: file.into(),
                min: wc_core::Point::new(0.0, 0.0),
                max: wc_core::Point::new(10.0, 10.0),
            });
            Ok(())
        },
        [(file.to_owned(), PNG.to_vec())].into(),
    )?;
    // The guest has a file of its own under that name, and picks it as the logo.
    let mine = b"\x89PNG\r\n\x1a\nmine".to_vec();
    std::fs::create_dir_all(guest_open.dir.join("pictures"))?;
    std::fs::write(guest_open.dir.join(file), &mine)?;
    guest_open.project.meta.logo = Some(wc_core::Logo {
        file: file.into(),
        cover: None,
    });
    // As if its picture had gone out before the session's came in (edit `seq`).
    let seq = 1;
    g.pictures_sent.insert(file.into(), Some(seq));
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let logo = guest_open
        .project
        .meta
        .logo
        .as_ref()
        .map(|l| l.file.clone())
        .ok_or("no logo")?;
    assert_ne!(logo, file, "renamed on the clash: {:?}", g.notes);
    // The refusal of that picture comes only now: nothing more is renamed.
    if let Some(Role::Guest(j)) = g.role.as_mut() {
        j.guest.names_taken.push((file.into(), Some(seq)));
    }
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let notes = g.take_notes();
    let renames = notes.iter().filter(|n| n.contains("is renamed")).count();
    assert_eq!(renames, 1, "{notes:?}");
    assert!(!notes.iter().any(|n| n.contains("was not kept")), "{notes:?}");
    assert_eq!(
        guest_open.project.meta.logo.as_ref().map(|l| l.file.clone()),
        Some(logo)
    );
    let underlay = |o: &Open| {
        o.project
            .page("p0002")
            .and_then(|p| p.underlay.as_ref())
            .map(|u| u.file.clone())
    };
    assert_eq!(underlay(&guest_open).as_deref(), Some(file));
    assert_eq!(underlay(&host_open).as_deref(), Some(file));
    Ok(())
}

#[test]
fn a_picture_renamed_for_some_references_leaves_the_others() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut open = project(tmp.path())?;
    std::fs::create_dir_all(open.dir.join("pictures"))?;
    std::fs::write(open.dir.join("pictures/brand.png"), PNG)?;
    let file = "pictures/brand.png".to_owned();
    open.project.meta.logo = Some(wc_core::Logo {
        file: file.clone(),
        cover: None,
    });
    let id = open.project.pages[1].id.clone();
    open.project.pages[1].underlay = Some(wc_core::Underlay {
        file: file.clone(),
        min: wc_core::Point::new(0.0, 0.0),
        max: wc_core::Point::new(10.0, 10.0),
    });
    let (new, _) = rename_picture(&mut open, &file, Some(&[PicRef::Logo].into()))?;
    assert_eq!(
        open.project.meta.logo.as_ref().map(|l| l.file.as_str()),
        Some(new.as_str())
    );
    assert_eq!(
        open.project
            .page(&id)
            .and_then(|p| p.underlay.as_ref())
            .map(|u| u.file.as_str()),
        Some(file.as_str()),
        "the reference not picked keeps its file"
    );
    Ok(())
}

#[test]
fn a_pick_refused_or_pointed_elsewhere_is_withdrawn_with_its_waiting_rename() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut open = project(tmp.path())?;
    let id = open.project.pages[1].id.clone();
    let (a, b) = ("pictures/a.png".to_owned(), "pictures/b.png".to_owned());
    open.project.meta.logo = Some(wc_core::Logo {
        file: a.clone(),
        cover: None,
    });
    open.project.pages[1].underlay = Some(wc_core::Underlay {
        file: a.clone(),
        min: wc_core::Point::new(0.0, 0.0),
        max: wc_core::Point::new(10.0, 10.0),
    });
    let mut picks: BTreeMap<String, BTreeSet<PicRef>> = [
        (a.clone(), [PicRef::Logo, PicRef::Underlay(id.clone())].into()),
        (b.clone(), [PicRef::Logo].into()),
    ]
    .into();
    let now = std::time::Instant::now();
    let mut due: BTreeMap<String, std::time::Instant> = [(a.clone(), now), (b.clone(), now)].into();
    let mut clashes = vec![a.clone(), b.clone()];
    // The logo's change was refused: a.png keeps its underlay pick; b.png's logo names a.png.
    let gone = withdraw_picks(
        &mut picks,
        &mut BTreeMap::new(),
        &mut due,
        &mut clashes,
        &open,
        ((BTreeSet::new(), true, BTreeSet::new()), &BTreeSet::new()),
    );
    assert_eq!(gone, [b.clone()].into());
    assert_eq!(picks, [(a.clone(), [PicRef::Underlay(id.clone())].into())].into());
    assert_eq!(clashes, std::slice::from_ref(&a));
    assert!(due.contains_key(&a) && !due.contains_key(&b));
    // Then the underlay's page: nothing is left to rename.
    let gone = withdraw_picks(
        &mut picks,
        &mut BTreeMap::new(),
        &mut due,
        &mut clashes,
        &open,
        (([id].into(), false, BTreeSet::new()), &BTreeSet::new()),
    );
    assert_eq!(gone, [a].into());
    assert!(picks.is_empty() && clashes.is_empty() && due.is_empty());
    Ok(())
}

#[test]
fn a_file_here_under_the_name_of_a_picture_the_session_uses_is_kept_and_not_shared() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session_sharing(
        tmp.path(),
        |_| Ok(()),
        [("pictures/brand.png".to_owned(), PNG.to_vec())].into(),
    )?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // A file of that name is here already, which no change of this copy names.
    let mine = b"\x89PNG\r\n\x1a\nmine".to_vec();
    std::fs::create_dir_all(guest_open.dir.join("pictures"))?;
    std::fs::write(guest_open.dir.join("pictures/brand.png"), &mine)?;
    // Then the session's own drawing starts using its picture.
    let logo = Some(wc_core::Logo {
        file: "pictures/brand.png".into(),
        cover: None,
    });
    host_open.project.meta.logo = logo.clone();
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(file_bytes(&guest_open.dir, "pictures/brand.png")?, mine, "kept");
    assert_eq!(guest_open.project.meta.logo, logo, "not renamed");
    assert_eq!(host_open.project.meta.logo, logo);
    assert_eq!(
        h.role.as_ref().map(|r| r.pictures().len()),
        Some(1),
        "nothing of this copy's was shared"
    );
    let notes = g.take_notes();
    assert!(notes.iter().any(|n| n.contains("differs from your file")), "{notes:?}");
    Ok(())
}

#[test]
fn a_refusal_of_a_picture_this_copy_never_sent_shares_nothing() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session_sharing(
        tmp.path(),
        |_| Ok(()),
        [("pictures/brand.png".to_owned(), PNG.to_vec())].into(),
    )?;
    let logo = Some(wc_core::Logo {
        file: "pictures/brand.png".into(),
        cover: None,
    });
    host_open.project.meta.logo = logo.clone();
    let mine = b"\x89PNG\r\n\x1a\nmine".to_vec();
    std::fs::create_dir_all(guest_open.dir.join("pictures"))?;
    std::fs::write(guest_open.dir.join("pictures/brand.png"), &mine)?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // A refusal naming a path this copy never sent, though it looked at that path (a read
    // that failed is remembered the same way, so it is not tried again at once).
    g.pictures_out
        .insert("pictures/brand.png".into(), std::time::Instant::now());
    if let Some(Role::Guest(j)) = g.role.as_mut() {
        j.guest.names_taken.push(("pictures/brand.png".into(), Some(1)));
    }
    for _ in 0..4 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(guest_open.project.meta.logo, logo, "not renamed");
    assert_eq!(host_open.project.meta.logo, logo);
    assert_eq!(h.role.as_ref().map(|r| r.pictures().len()), Some(1), "nothing shared");
    Ok(())
}

#[test]
fn a_picture_that_could_not_be_written_is_tried_again_only_after_a_while() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session_with(tmp.path(), |o| {
        std::fs::create_dir_all(o.dir.join("pictures"))?;
        std::fs::write(o.dir.join("pictures/brand.png"), PNG)?;
        o.project.meta.logo = Some(wc_core::Logo {
            file: "pictures/brand.png".into(),
            cover: None,
        });
        Ok(())
    })?;
    // A folder stands where the picture goes, so writing it fails.
    let target = guest_open.dir.join("pictures/brand.png");
    assert!(!target.exists(), "not taken before the first frame");
    std::fs::create_dir_all(&target)?;
    let started = std::time::Instant::now();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(target.is_dir(), "not written");
    // The obstacle is gone, but the picture is not tried again right away.
    std::fs::remove_dir(&target)?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    if started.elapsed() < ITEM_RETRY_LATER {
        assert!(!target.exists(), "skipped while it failed a moment ago");
    }
    // After a while it is tried again, and written.
    std::thread::sleep(ITEM_RETRY_LATER + std::time::Duration::from_millis(50));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(file_bytes(&guest_open.dir, "pictures/brand.png")?, PNG, "written");
    let notes: Vec<String> = g
        .take_notes()
        .into_iter()
        .filter(|n| n.contains("pictures/brand.png"))
        .collect();
    assert_eq!(notes.len(), 1, "named once: {notes:?}");
    assert!(notes[0].contains("could not be written"), "{}", notes[0]);
    Ok(())
}

#[test]
fn a_picture_picked_while_the_sessions_of_that_name_waits_to_be_written_gets_a_name_of_its_own() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session_with(tmp.path(), |o| {
        std::fs::create_dir_all(o.dir.join("pictures"))?;
        std::fs::write(o.dir.join("pictures/brand.png"), PNG)?;
        o.project.meta.logo = Some(wc_core::Logo {
            file: "pictures/brand.png".into(),
            cover: None,
        });
        Ok(())
    })?;
    // A folder stands where the session's picture goes, so writing it fails.
    let target = guest_open.dir.join("pictures/brand.png");
    std::fs::create_dir_all(&target)?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // While it waits to be tried again, the guest puts a picture of its own there and picks it
    // under its own page: that change reaches the session.
    std::fs::remove_dir(&target)?;
    let mine = b"\x89PNG\r\n\x1a\nmine".to_vec();
    std::fs::write(&target, &mine)?;
    guest_open.project.page_mut("p0003").ok_or("no page")?.underlay = Some(wc_core::Underlay {
        file: "pictures/brand.png".into(),
        min: wc_core::Point::new(0.0, 0.0),
        max: wc_core::Point::new(100.0, 100.0),
    });
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // Once the session's picture is compared again, the pick gets a name of its own.
    std::thread::sleep(ITEM_RETRY_LATER + std::time::Duration::from_millis(50));
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let file = guest_open
        .project
        .page("p0003")
        .and_then(|p| p.underlay.as_ref())
        .map(|u| u.file.clone())
        .ok_or("no underlay")?;
    assert_ne!(file, "pictures/brand.png", "{:?}", g.notes);
    assert_eq!(file_bytes(&guest_open.dir, &file)?, mine);
    assert_eq!(
        host_open
            .project
            .page("p0003")
            .and_then(|p| p.underlay.as_ref())
            .map(|u| u.file.clone()),
        Some(file.clone())
    );
    assert_eq!(file_bytes(&host_open.dir, &file)?, mine);
    assert_eq!(
        guest_open.project.meta.logo.as_ref().map(|l| l.file.as_str()),
        Some("pictures/brand.png"),
        "the session's logo keeps its name"
    );
    Ok(())
}

#[test]
fn a_replaced_session_picture_also_picked_here_is_renamed_once_everywhere() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session_with(tmp.path(), |o| {
        std::fs::create_dir_all(o.dir.join("pictures"))?;
        std::fs::write(o.dir.join("pictures/brand.png"), PNG)?;
        o.project.meta.logo = Some(wc_core::Logo {
            file: "pictures/brand.png".into(),
            cover: None,
        });
        Ok(())
    })?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(file_bytes(&guest_open.dir, "pictures/brand.png")?, PNG, "taken");
    // The guest puts its own file in place of the session's and also picks it under its page.
    let mine = b"\x89PNG\r\n\x1a\nmine".to_vec();
    std::fs::write(guest_open.dir.join("pictures/brand.png"), &mine)?;
    guest_open.project.page_mut("p0003").ok_or("no page")?.underlay = Some(wc_core::Underlay {
        file: "pictures/brand.png".into(),
        min: wc_core::Point::new(0.0, 0.0),
        max: wc_core::Point::new(100.0, 100.0),
    });
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    // A replaced picture is renamed wherever it is used, once: the logo and the underlay
    // name the same new file.
    let file = guest_open
        .project
        .page("p0003")
        .and_then(|p| p.underlay.as_ref())
        .map(|u| u.file.clone())
        .ok_or("no underlay")?;
    assert_ne!(file, "pictures/brand.png", "{:?}", g.notes);
    assert_eq!(
        guest_open.project.meta.logo.as_ref().map(|l| l.file.as_str()),
        Some(file.as_str()),
        "{:?}",
        g.notes
    );
    let renames = g.take_notes().into_iter().filter(|n| n.contains("is renamed")).count();
    assert_eq!(renames, 1, "renamed once");
    assert_eq!(file_bytes(&host_open.dir, &file)?, mine);
    assert_eq!(file_bytes(&host_open.dir, "pictures/brand.png")?, PNG);
    Ok(())
}

/// A guest on its own end of a link, as after "Ask to join" with the status window open.
fn asking(tmp: &Path) -> (Share, wc_share::transport::HostLink) {
    let (guest_end, host_end) = pair();
    let mut guest = Guest::new();
    let _ = guest.join("Kari", "kari@example.com");
    let g = Share {
        role: Some(Role::Guest(Box::new(Joining {
            guest,
            link: guest_end,
            addr: "in-process".into(),
            folder: tmp.join("guest"),
            dir: None,
        }))),
        view: View::Status,
        form: Some(Form::default()),
        ..Default::default()
    };
    (g, host_end)
}

#[test]
fn a_close_right_behind_a_welcome_from_another_version_does_not_hide_the_version() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let open = project(&tmp.path().join("host"))?;
    let (mut g, host_end) = asking(tmp.path());
    let old = wc_share::PROTOCOL_VERSION + 1;
    // Both arrive before the guest's next frame.
    host_end.send(wc_share::HostMsg::Welcome {
        you: 1,
        project: Box::new(wc_share::Snapshot::from(&open.project)),
        locks: Default::default(),
        peers: Vec::new(),
        history: Vec::new(),
        protocol: old,
    })?;
    host_end.send(wc_share::HostMsg::Closed)?;
    assert!(g.frame(None, None, false).is_none());
    let notes = g.take_notes();
    assert!(notes.iter().any(|n| n.contains("cannot join")), "{notes:?}");
    assert!(!notes.iter().any(|n| n.contains("has ended")), "{notes:?}");
    assert!(g.view == View::Join);
    let error = g.form.as_ref().and_then(|f| f.error.clone()).ok_or("no form error")?;
    assert!(error.contains(&format!("version {old} and")), "{error}");
    Ok(())
}

#[test]
fn a_plain_decline_does_not_reopen_a_status_window_that_was_closed() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut g, host_end) = asking(tmp.path());
    g.view = View::Closed;
    host_end.send(wc_share::HostMsg::Declined { version: None })?;
    assert!(g.frame(None, None, false).is_none());
    assert!(g.view == View::Closed, "left closed");
    assert!(g.take_notes().iter().any(|n| n.contains("declined")));

    // With the status window open it goes back to the Join dialog, which says so.
    let (mut g, host_end) = asking(tmp.path());
    host_end.send(wc_share::HostMsg::Declined { version: None })?;
    assert!(g.frame(None, None, false).is_none());
    assert!(g.view == View::Join);
    assert_eq!(
        g.form.as_ref().and_then(|f| f.error.as_deref()),
        Some("The host declined your request to join.")
    );
    Ok(())
}

#[test]
fn the_join_dialog_forgets_sessions_heard_by_an_earlier_browser() -> TestResult {
    let mut s = Share {
        form: Some(Form {
            sessions: vec![wc_share::discovery::Session {
                project: "Gone".into(),
                host: "Ola".into(),
                addr: ([127, 0, 0, 1], 9).into(),
                protocol: wc_share::PROTOCOL_VERSION,
            }],
            ..Form::default()
        }),
        ..Default::default()
    };
    s.show_join();
    let form = s.form.as_ref().ok_or("no form")?;
    assert!(form.sessions.is_empty(), "{:?}", form.sessions);
    Ok(())
}

#[test]
fn another_version_goes_back_to_the_join_dialog_even_when_the_status_window_was_closed() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let open = project(&tmp.path().join("host"))?;
    let (mut g, host_end) = asking(tmp.path());
    g.view = View::Closed;
    let old = wc_share::PROTOCOL_VERSION + 1;
    host_end.send(wc_share::HostMsg::Welcome {
        you: 1,
        project: Box::new(wc_share::Snapshot::from(&open.project)),
        locks: Default::default(),
        peers: Vec::new(),
        history: Vec::new(),
        protocol: old,
    })?;
    assert!(g.frame(None, None, false).is_none());
    assert!(g.view == View::Join, "reopened: the person has to update one side");
    let error = g.form.as_ref().and_then(|f| f.error.clone()).ok_or("no form error")?;
    assert!(error.contains(&format!("version {old} and")), "{error}");
    assert!(
        error.contains(&format!("guest's uses version {};", wc_share::PROTOCOL_VERSION)),
        "{error}"
    );
    Ok(())
}

#[test]
fn a_session_on_another_version_is_listed_as_such_and_cannot_be_picked() {
    let row = |protocol| wc_share::discovery::Session {
        project: "Pump station".into(),
        host: "Ola".into(),
        addr: ([192, 168, 1, 20], 47812).into(),
        protocol,
    };
    let (label, joinable) = session_row(&row(wc_share::PROTOCOL_VERSION));
    assert!(joinable);
    assert_eq!(label, "Pump station  ·  Ola  (192.168.1.20:47812)");
    let (label, joinable) = session_row(&row(0));
    assert!(!joinable);
    assert!(
        label.contains(&format!("version 0, this one uses {};", wc_share::PROTOCOL_VERSION)),
        "{label}"
    );
    assert!(label.contains("cannot be joined"), "{label}");
}

#[test]
fn a_session_on_another_version_cannot_be_picked_in_the_join_dialog() {
    let row = |protocol, last| wc_share::discovery::Session {
        project: "Pump station".into(),
        host: "Ola".into(),
        addr: ([192, 168, 1, last], 47812).into(),
        protocol,
    };
    let mut s = Share {
        form: Some(Form {
            addr: String::new(),
            sessions: vec![row(0, 20), row(wc_share::PROTOCOL_VERSION, 21)],
            ..Form::default()
        }),
        view: View::Join,
        ..Default::default()
    };
    let ctx = egui::Context::default();
    let frame = |s: &mut Share, events: Vec<egui::Event>| {
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| s.join_window(ctx));
    };
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    };
    let addr = |s: &Share| s.form.as_ref().map(|f| f.addr.clone()).unwrap_or_default();
    frame(&mut s, Vec::new());
    // Step through the dialog with the keyboard and pick the first row that takes it: the
    // older session's row comes first but cannot be focused or picked.
    for _ in 0..8 {
        frame(&mut s, vec![key(egui::Key::Tab)]);
        frame(&mut s, vec![key(egui::Key::Space)]);
        if !addr(&s).is_empty() {
            break;
        }
    }
    assert_eq!(addr(&s), "192.168.1.21:47812");
}

#[test]
fn a_refusal_is_forgotten_once_another_address_is_chosen() {
    let mut form = Form {
        addr: "192.168.1.20:47812".into(),
        error: Some("The host declined your request to join.".into()),
        ..Form::default()
    };
    let folder = form.folder.text.clone();
    form.forget_error_unless("192.168.1.20:47812", &folder);
    assert!(form.error.is_some(), "same address: still shown");
    // Another folder does not make the host take the request.
    form.forget_error_unless("192.168.1.20:47812", "/somewhere/else");
    assert!(form.error.is_some(), "only the folder changed: still shown");
    form.addr = "192.168.1.21:47812".into();
    form.forget_error_unless("192.168.1.20:47812", &folder);
    assert!(form.error.is_none());
}

#[test]
fn a_folder_error_stays_until_the_folder_changes() -> TestResult {
    let mut s = Share {
        form: Some(Form {
            addr: "192.168.1.20:47812".into(),
            folder: crate::prefs::FolderField::typed("C:shared"),
            ..Form::default()
        }),
        view: View::Join,
        ..Default::default()
    };
    // The folder is refused (as `C:shared` is on Windows) before the host is tried.
    s.ask_to_join(|typed| Err(format!("{} is relative to the current folder", typed.text)));
    assert!(s.role.is_none(), "not joined");
    let form = s.form.as_mut().ok_or("no form")?;
    assert_eq!(
        form.error.as_deref(),
        Some("C:shared is relative to the current folder")
    );
    // Another address does not make the folder usable.
    form.addr = "192.168.1.21:47812".into();
    form.forget_error_unless("192.168.1.20:47812", "C:shared");
    assert!(form.error.is_some(), "the folder is still the same");
    form.folder = crate::prefs::FolderField::typed("C:\\shared");
    form.forget_error_unless("192.168.1.21:47812", "C:shared");
    assert!(form.error.is_none());
    Ok(())
}

#[test]
fn a_symbol_whose_adding_was_undone_is_not_sent_again_and_is_locked_when_placed_again() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let id = "wc.contact.no";
    assert!(!host_open.project.lock.symbols.contains_key(id), "new to the session");
    guest_open.place(2, id, wc_core::Point::new(40.0, 100.0))?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(host_open.project.lock.symbols.contains_key(id), "added");
    // Undo the placement and its new device, then the change that added the symbol.
    for _ in 0..3 {
        g.undo()?;
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    for open in [&host_open, &guest_open] {
        assert!(
            !open.project.lock.symbols.contains_key(id),
            "taken back, and not sent again"
        );
    }
    // Placed again: locked again, and the session gets it with the page.
    guest_open.place(2, id, wc_core::Point::new(60.0, 100.0))?;
    assert!(guest_open.project.lock.symbols.contains_key(id), "pinned when placed");
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(host_open.project.lock.symbols.contains_key(id));
    assert!(
        host_open
            .project
            .page("p0003")
            .is_some_and(|p| p.symbols.iter().any(|s| s.symbol == id))
    );
    Ok(())
}

#[test]
fn a_symbol_updated_in_a_session_reaches_the_host_and_an_undo_brings_the_old_one_back() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    guest_open.place(2, "wc.coil", wc_core::Point::new(40.0, 100.0))?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let v1 = guest_open.symbols.get("wc.coil").cloned().ok_or("coil")?;
    assert_eq!(
        host_open.project.lock.symbols.get("wc.coil").map(|e| e.hash.clone()),
        Some(v1.hash())
    );

    // The guest updates the coil from the library.
    let mut v2 = v1.clone();
    v2.name = "Coil, newer".into();
    store::write_vendored_symbol(&guest_open.dir, &v2)?;
    guest_open.project.lock.symbols.get_mut("wc.coil").ok_or("pin")?.hash = v2.hash();
    guest_open.symbols.insert("wc.coil".into(), v2.clone());
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let hash = |o: &Open| o.project.lock.symbols.get("wc.coil").map(|e| e.hash.clone());
    assert_eq!(hash(&host_open), Some(v2.hash()), "{:?}", h.notes);
    assert_eq!(host_open.symbols.get("wc.coil"), Some(&v2));
    assert_eq!(store::read_vendored_symbol(&host_open.dir, "wc.coil")?, v2);
    assert_eq!(hash(&guest_open), Some(v2.hash()), "the guest keeps its update");
    assert!(
        !g.notes.iter().any(|n| n.contains("first in another version")),
        "{:?}",
        g.notes
    );

    // Undone: both copies draw the old version again.
    g.undo()?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(hash(&host_open), Some(v1.hash()), "{:?} {:?}", g.notes, h.notes);
    assert_eq!(host_open.symbols.get("wc.coil"), Some(&v1));
    assert_eq!(hash(&guest_open), Some(v1.hash()), "{:?}", g.notes);
    assert_eq!(guest_open.symbols.get("wc.coil"), Some(&v1));
    assert_eq!(store::read_vendored_symbol(&guest_open.dir, "wc.coil")?, v1);
    Ok(())
}

#[test]
fn an_update_undone_before_its_sender_looked_is_not_sent_again_and_others_see_no_conflict() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    guest_open.place(2, "wc.coil", wc_core::Point::new(40.0, 100.0))?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let v1 = guest_open.symbols.get("wc.coil").cloned().ok_or("coil")?;
    let mut v2 = v1.clone();
    v2.name = "Coil, newer".into();
    store::write_vendored_symbol(&guest_open.dir, &v2)?;
    guest_open.project.lock.symbols.get_mut("wc.coil").ok_or("pin")?.hash = v2.hash();
    guest_open.symbols.insert("wc.coil".into(), v2.clone());
    // The guest sends the update; the host takes it and takes it back again at once, so the
    // guest gets both answers in one look.
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    let hash = |o: &Open| o.project.lock.symbols.get("wc.coil").map(|e| e.hash.clone());
    assert_eq!(hash(&host_open), Some(v2.hash()), "{:?}", h.notes);
    assert!(
        !h.notes.iter().any(|n| n.contains("first in another version")),
        "an update someone else made is no conflict here: {:?}",
        h.notes
    );
    let Some(Role::Host(hosting)) = h.role.as_mut() else {
        return Err("hosting".into());
    };
    let update = hosting.host.history().last().ok_or("recorded")?.id;
    h.revert(update)?;
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    assert_eq!(hash(&host_open), Some(v1.hash()), "{:?}", h.notes);
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    // Past the wait for an answer: an update still counted as unanswered would go again now.
    std::thread::sleep(ITEM_WAIT * 2);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(hash(&host_open), Some(v1.hash()), "the undo stays: {:?}", g.notes);
    assert_eq!(hash(&guest_open), Some(v1.hash()), "{:?}", g.notes);
    assert_eq!(
        guest_open.symbols.get("wc.coil"),
        Some(&v1),
        "the old version is loaded"
    );
    assert_eq!(
        store::read_vendored_symbol(&guest_open.dir, "wc.coil")?,
        v1,
        "and written back"
    );
    assert!(
        !g.notes.iter().any(|n| n.contains("first in another version")),
        "{:?}",
        g.notes
    );
    Ok(())
}

#[test]
fn a_joined_copy_takes_a_library_symbol_only_in_the_version_the_session_pinned() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let item = |id: &str, name: &str| Symbol {
        source: String::new(),
        id: id.into(),
        name: name.into(),
        category: String::new(),
        letter_code: String::new(),
        role: Default::default(),
        graphics: vec![],
        pins: vec![],
        tag_at: Default::default(),
        description: String::new(),
    };
    let (coil_v1, coil_v2, lamp) = (item("coil", "Coil"), item("coil", "Coil v2"), item("lamp", "Lamp"));
    // The library here has a newer coil than the session pinned, and the lamp it pinned.
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(lib.join("symbols"))?;
    for s in [&coil_v2, &lamp] {
        std::fs::write(
            lib.join("symbols").join(format!("{}.json", s.id)),
            serde_json::to_string(s)?,
        )?;
    }
    let mut project = Project::new("Demo");
    project.meta.libraries.push(wc_core::project::LibrarySource {
        name: "here".into(),
        kind: "folder".into(),
        location: lib.to_string_lossy().into_owned(),
        reference: None,
    });
    for s in [&coil_v1, &lamp] {
        project.lock.symbols.insert(
            s.id.clone(),
            wc_core::LockEntry {
                source: "here".into(),
                hash: s.hash(),
            },
        );
    }
    let open = write_copy(&tmp.path().join("copies"), &project, &BTreeMap::new(), &BTreeMap::new())?;
    assert_eq!(open.symbols.get("lamp"), Some(&lamp));
    assert!(!open.symbols.contains_key("coil"), "not the library's other version");
    assert_eq!(
        open.project.lock.symbols.get("coil").map(|e| e.hash.clone()),
        Some(coil_v1.hash())
    );
    assert_eq!(
        store::load(&open.dir)?.lock.symbols.get("coil").map(|e| e.hash.clone()),
        Some(coil_v1.hash())
    );
    assert!(
        store::read_vendored_symbol(&open.dir, "coil").is_err(),
        "no coil file written"
    );
    Ok(())
}

#[test]
fn an_update_of_an_item_the_session_took_back_is_not_sent_as_an_add() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let entry = |hash: &str| wc_core::LockEntry {
        source: "starter".into(),
        hash: hash.into(),
    };
    // At the last sync the session had coil v1; this copy updated it to v2; the session has
    // since taken the coil back (an undo of its add).
    let mut base = Project::new("Demo");
    base.lock.symbols.insert("coil".into(), entry("v1"));
    let mut local = base.clone();
    local.lock.symbols.insert("coil".into(), entry("v2"));
    let auth = Project::new("Demo");
    let out = new_items(
        tmp.path(),
        &local,
        &base,
        &auth,
        &Default::default(),
        &Default::default(),
        &Default::default(),
    );
    assert!(out.edits.is_empty(), "{:?}", out.edits);
    assert_eq!(out.stale, ["symbol coil"]);
    // An item the session never had is still sent as an add (here: unreadable, so tried).
    let mut fresh = local.clone();
    fresh.lock.symbols.insert("lamp".into(), entry("l1"));
    let out = new_items(
        tmp.path(),
        &fresh,
        &base,
        &auth,
        &Default::default(),
        &Default::default(),
        &Default::default(),
    );
    assert_eq!(out.unreadable.len(), 1, "{:?}", out.unreadable);
    assert_eq!(out.unreadable[0].0, "symbol lamp");
    Ok(())
}

/// A session where the guest placed `wc.contact.no` on page 3 and the host refuses to store
/// it, past the first tries: the placement waits. Returns the placement's id too.
fn waiting_session(tmp: &Path) -> std::result::Result<(Session, String), Box<dyn std::error::Error>> {
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp)?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    log.fail_vendor(true);
    let item = guest_open.place(2, "wc.contact.no", wc_core::Point::new(40.0, 100.0))?;
    for _ in 0..=ITEM_TRIES {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
        std::thread::sleep(ITEM_WAIT);
    }
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    Ok(((h, log, host_open, g, guest_open), item))
}

fn has_placement(o: &Open, page: &str, item: &str) -> bool {
    o.project
        .page(page)
        .is_some_and(|p| p.symbols.iter().any(|s| s.id == item))
}

#[test]
fn a_waiting_placement_survives_the_session_renumbering_its_page() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, log, mut host_open, mut g, mut guest_open), item) = waiting_session(tmp.path())?;
    host_open.project.page_mut("p0003").ok_or("no page")?.number = "30".into();
    host_open.project.sort_pages();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(guest_open.project.page("p0003").map(|p| p.number.as_str()), Some("30"));
    assert!(has_placement(&guest_open, "p0003", &item), "{:?}", g.notes);
    log.fail_vendor(false);
    std::thread::sleep(ITEM_RETRY_LATER);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(has_placement(&host_open, "p0003", &item));
    assert_eq!(host_open.project.page("p0003").map(|p| p.number.as_str()), Some("30"));
    Ok(())
}

#[test]
fn changes_that_do_not_use_the_waiting_symbol_still_go_out() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), item) = waiting_session(tmp.path())?;
    guest_open.project.meta.name = "Pump station 2".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(host_open.project.meta.name, "Pump station 2");
    assert!(!has_placement(&host_open, "p0003", &item));
    assert!(has_placement(&guest_open, "p0003", &item));
    Ok(())
}

#[test]
fn undo_of_a_waiting_change_takes_it_back_here_and_not_an_older_one_there() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), item) = waiting_session(tmp.path())?;
    let history = h.role.as_ref().map(|r| r.history().len());
    // Saved while it waited: the undo must leave the copy to be saved again.
    guest_open.dirty = false;
    g.undo()?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(guest_open.dirty, "the copy on disk still has the placement");
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(!has_placement(&guest_open, "p0003", &item), "taken back here");
    assert_eq!(
        h.role.as_ref().map(|r| r.history().len()),
        history,
        "nothing undone in the session"
    );
    let notes = g.take_notes();
    assert!(
        notes.iter().any(|n| n.contains("had not reached the session")),
        "{notes:?}"
    );
    Ok(())
}

#[test]
fn a_waiting_placement_does_not_take_a_device_the_session_made_under_its_tag() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, log, mut host_open, mut g, mut guest_open), item) = waiting_session(tmp.path())?;
    let tag = guest_open
        .project
        .page("p0003")
        .and_then(|p| p.symbols.iter().find(|s| s.id == item))
        .and_then(|s| s.tag.clone())
        .ok_or("the placement has no tag")?;
    assert!(!host_open.project.devices.contains_key(&tag));
    // The host makes a device of its own under the same tag while the placement waits.
    let theirs = wc_core::Device {
        description: "Theirs".into(),
        ..Default::default()
    };
    host_open.project.devices.insert(tag.clone(), theirs.clone());
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert_eq!(guest_open.project.devices.get(&tag), Some(&theirs));
    assert!(
        !has_placement(&guest_open, "p0003", &item),
        "the placement is not kept naming the session's device"
    );
    let notes = g.take_notes();
    assert!(notes.iter().any(|n| n.contains("was not kept")), "{notes:?}");
    // Once the item could go, nothing of the placement reaches the session.
    log.fail_vendor(false);
    std::thread::sleep(ITEM_RETRY_LATER);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(!has_placement(&host_open, "p0003", &item));
    assert_eq!(host_open.project.devices.get(&tag), Some(&theirs));
    Ok(())
}

#[test]
fn undo_of_a_second_waiting_placement_takes_back_only_that_one() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, log, mut host_open, mut g, mut guest_open), first) = waiting_session(tmp.path())?;
    let tag_of = |o: &Open, item: &str| -> Option<wc_core::DeviceTag> {
        o.project
            .page("p0003")?
            .symbols
            .iter()
            .find(|s| s.id == item)?
            .tag
            .clone()
    };
    // The same waiting symbol placed again: its new device record waits with the page.
    let second = guest_open.place(2, "wc.contact.no", wc_core::Point::new(80.0, 100.0))?;
    let second_tag = tag_of(&guest_open, &second).ok_or("no tag")?;
    let first_tag = tag_of(&guest_open, &first).ok_or("no tag")?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(has_placement(&guest_open, "p0003", &second));
    assert!(!has_placement(&host_open, "p0003", &second));
    assert!(!host_open.project.devices.contains_key(&second_tag), "{:?}", g.notes);

    let history = h.role.as_ref().map(|r| r.history().len());
    // One Undo takes back the second placement and its device, here only; the first placement
    // still waits.
    g.undo()?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert!(!has_placement(&guest_open, "p0003", &second), "{:?}", g.notes);
    assert!(has_placement(&guest_open, "p0003", &first), "{:?}", g.notes);
    assert!(!guest_open.project.devices.contains_key(&second_tag));
    assert!(!host_open.project.devices.contains_key(&second_tag));
    assert_eq!(
        h.role.as_ref().map(|r| r.history().len()),
        history,
        "nothing in the session"
    );
    assert!(g.can_undo(), "the first placement still waits");

    // Once the symbol gets in, the first placement goes out, and only that one.
    log.fail_vendor(false);
    std::thread::sleep(ITEM_RETRY_LATER);
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert!(has_placement(&host_open, "p0003", &first), "{:?}", g.notes);
    assert!(host_open.project.devices.contains_key(&first_tag));
    assert!(!has_placement(&host_open, "p0003", &second));
    assert!(!host_open.project.devices.contains_key(&second_tag));
    Ok(())
}

#[test]
fn undo_of_a_rename_of_a_waiting_placements_new_device_brings_the_device_back() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), first) = waiting_session(tmp.path())?;
    let tag = guest_open
        .project
        .page("p0003")
        .and_then(|p| p.symbols.iter().find(|s| s.id == first))
        .and_then(|s| s.tag.clone())
        .ok_or("no tag")?;
    let device = guest_open.project.devices.get(&tag).cloned().ok_or("no device")?;
    let renamed = DeviceTag::parse("-K77")?;
    // The new device the waiting placement made is renamed before the session has it.
    guest_open.checkpoint();
    guest_open.project.rename_device(&tag, &renamed);
    for _ in 0..2 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert!(!guest_open.project.devices.contains_key(&tag));
    g.undo()?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let on_page = guest_open
        .project
        .page("p0003")
        .and_then(|p| p.symbols.iter().find(|s| s.id == first))
        .and_then(|s| s.tag.clone());
    assert_eq!(on_page.as_ref(), Some(&tag), "{:?}", g.notes);
    assert_eq!(
        guest_open.project.devices.get(&tag),
        Some(&device),
        "the device comes back with it"
    );
    assert!(!guest_open.project.devices.contains_key(&renamed));
    assert!(!host_open.project.devices.contains_key(&tag) && !host_open.project.devices.contains_key(&renamed));
    Ok(())
}

#[test]
fn undo_of_a_waiting_placement_keeps_a_session_change_to_a_synced_device_on_its_page() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let k9 = DeviceTag::parse("-K9")?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session_with(tmp.path(), |o| {
        let coil = o.place(2, "wc.coil", wc_core::Point::new(40.0, 160.0))?;
        if let Some(s) = o.project.pages[2].symbols.iter_mut().find(|s| s.id == coil) {
            s.tag = Some(k9.clone());
        }
        o.project.devices.insert(k9.clone(), Device::default());
        Ok(())
    })?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    log.fail_vendor(true);
    let first = guest_open.place(2, "wc.contact.no", wc_core::Point::new(40.0, 100.0))?;
    for _ in 0..=ITEM_TRIES {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
        std::thread::sleep(ITEM_WAIT);
    }
    let second = guest_open.place(2, "wc.contact.no", wc_core::Point::new(80.0, 100.0))?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // After the second placement, the host changes -K9, which the waiting page names.
    host_open.project.devices.insert(
        k9.clone(),
        Device {
            description: "From the host".into(),
            ..Default::default()
        },
    );
    for _ in 0..2 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let description = |o: &Open| o.project.devices.get(&k9).map(|d| d.description.clone());
    assert_eq!(
        description(&guest_open).as_deref(),
        Some("From the host"),
        "{:?}",
        g.notes
    );
    g.undo()?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert!(!has_placement(&guest_open, "p0003", &second), "{:?}", g.notes);
    assert!(has_placement(&guest_open, "p0003", &first), "{:?}", g.notes);
    assert_eq!(
        description(&guest_open).as_deref(),
        Some("From the host"),
        "the session's -K9 stays"
    );
    assert_eq!(description(&host_open).as_deref(), Some("From the host"));
    Ok(())
}

#[test]
fn undo_of_a_waiting_placement_keeps_the_sessions_page_number() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), first) = waiting_session(tmp.path())?;
    let second = guest_open.place(2, "wc.contact.no", wc_core::Point::new(80.0, 100.0))?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // The host renumbers the waiting page meanwhile, and the guest takes that in.
    host_open.project.page_mut("p0003").ok_or("no page")?.number = "30".into();
    host_open.project.sort_pages();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    let number = |o: &Open| o.project.page("p0003").map(|p| p.number.clone());
    assert_eq!(number(&guest_open).as_deref(), Some("30"));
    g.undo()?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert!(!has_placement(&guest_open, "p0003", &second), "{:?}", g.notes);
    assert!(has_placement(&guest_open, "p0003", &first), "{:?}", g.notes);
    assert_eq!(number(&guest_open).as_deref(), Some("30"), "{:?}", g.notes);
    assert_eq!(number(&host_open).as_deref(), Some("30"), "{:?}", g.notes);
    Ok(())
}

#[test]
fn undo_of_a_renumbering_here_of_a_waiting_page_puts_its_number_back() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), first) = waiting_session(tmp.path())?;
    let number = |o: &Open| o.project.page("p0003").map(|p| p.number.clone());
    let original = number(&guest_open).ok_or("no page")?;
    // The guest renumbers the waiting page, then numbers it back: two actions here.
    for to in ["30", original.as_str()] {
        guest_open.checkpoint();
        guest_open.project.page_mut("p0003").ok_or("no page")?.number = to.into();
        guest_open.project.sort_pages();
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(number(&guest_open), Some(original.clone()));
    // Undo takes back the latest of them: the page is numbered 30 again, and still waits.
    g.undo()?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(number(&guest_open).as_deref(), Some("30"), "{:?}", g.notes);
    assert!(has_placement(&guest_open, "p0003", &first), "{:?}", g.notes);
    Ok(())
}

#[test]
fn undo_of_a_renumbering_here_after_the_sessions_takes_back_only_that_one() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), first) = waiting_session(tmp.path())?;
    let number = |o: &Open| o.project.page("p0003").map(|p| p.number.clone());
    let original = number(&guest_open).ok_or("no page")?;
    // The session renumbers the waiting page to 30, and the guest takes that in.
    host_open.project.page_mut("p0003").ok_or("no page")?.number = "30".into();
    host_open.project.sort_pages();
    for _ in 0..2 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(number(&guest_open).as_deref(), Some("30"));
    // The guest numbers it back to the number it had, then to 40: two actions here.
    for to in [original.as_str(), "40"] {
        guest_open.checkpoint();
        guest_open.project.page_mut("p0003").ok_or("no page")?.number = to.into();
        guest_open.project.sort_pages();
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    // Undo takes back the 40 only: the page has the guest's number again, and still waits.
    g.undo()?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(number(&guest_open), Some(original), "{:?}", g.notes);
    assert!(has_placement(&guest_open, "p0003", &first), "{:?}", g.notes);
    Ok(())
}

#[test]
fn undo_of_a_waiting_placement_keeps_the_sessions_second_renumbering() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), first) = waiting_session(tmp.path())?;
    let number = |o: &Open| o.project.page("p0003").map(|p| p.number.clone());
    let renumber = |o: &mut Open, to: &str| -> TestResult {
        o.project.page_mut("p0003").ok_or("no page")?.number = to.into();
        o.project.sort_pages();
        Ok(())
    };
    // The session renumbers the waiting page to 30; the guest places a second waiting symbol;
    // then the session renumbers the page again, to 50.
    renumber(&mut host_open, "30")?;
    for _ in 0..2 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let second = guest_open.place(2, "wc.contact.no", wc_core::Point::new(80.0, 100.0))?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    renumber(&mut host_open, "50")?;
    for _ in 0..2 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(number(&guest_open).as_deref(), Some("50"));
    // Undo takes back the second placement only; the page keeps the session's 50.
    g.undo()?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert!(!has_placement(&guest_open, "p0003", &second), "{:?}", g.notes);
    assert!(has_placement(&guest_open, "p0003", &first), "{:?}", g.notes);
    assert_eq!(number(&guest_open).as_deref(), Some("50"), "{:?}", g.notes);
    Ok(())
}

#[test]
fn undo_of_a_renumbering_here_before_the_sessions_gives_the_page_the_sessions_number() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), first) = waiting_session(tmp.path())?;
    let number = |o: &Open| o.project.page("p0003").map(|p| p.number.clone());
    // The guest renumbers the waiting page to 10 ...
    guest_open.checkpoint();
    guest_open.project.page_mut("p0003").ok_or("no page")?.number = "10".into();
    guest_open.project.sort_pages();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // ... and the session renumbers it to 30 meanwhile; the guest's 10 still waits.
    host_open.project.page_mut("p0003").ok_or("no page")?.number = "30".into();
    host_open.project.sort_pages();
    for _ in 0..2 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(number(&guest_open).as_deref(), Some("10"), "{:?}", g.notes);
    // Undo takes back the guest's 10: the page has the session's 30, and still waits.
    g.undo()?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(number(&guest_open).as_deref(), Some("30"), "{:?}", g.notes);
    assert!(has_placement(&guest_open, "p0003", &first), "{:?}", g.notes);
    Ok(())
}

#[test]
fn a_renumbering_here_the_session_repeats_leaves_no_empty_undo_step() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), first) = waiting_session(tmp.path())?;
    let number = |o: &Open| o.project.page("p0003").map(|p| p.number.clone());
    let original = number(&guest_open).ok_or("no page")?;
    let host_renumber = |o: &mut Open, to: &str| -> TestResult {
        o.project.page_mut("p0003").ok_or("no page")?.number = to.into();
        o.project.sort_pages();
        Ok(())
    };
    // The session renumbers the waiting page to 30.
    host_renumber(&mut host_open, "30")?;
    for _ in 0..2 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    // The guest places a second waiting symbol, then numbers the page back, then to 40.
    let second = guest_open.place(2, "wc.contact.no", wc_core::Point::new(80.0, 100.0))?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    for to in [original.as_str(), "40"] {
        guest_open.checkpoint();
        guest_open.project.page_mut("p0003").ok_or("no page")?.number = to.into();
        guest_open.project.sort_pages();
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    // The session numbers it back too.
    host_renumber(&mut host_open, &original)?;
    for _ in 0..2 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    // Undo takes back the 40; the next Undo has nothing of the guest's renumbering left to
    // take back, and takes back the second placement only.
    g.undo()?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(number(&guest_open), Some(original.clone()), "{:?}", g.notes);
    assert!(has_placement(&guest_open, "p0003", &second), "{:?}", g.notes);
    g.undo()?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert!(!has_placement(&guest_open, "p0003", &second), "{:?}", g.notes);
    assert!(
        has_placement(&guest_open, "p0003", &first),
        "the first still waits: {:?}",
        g.notes
    );
    assert_eq!(number(&guest_open), Some(original), "{:?}", g.notes);
    Ok(())
}

#[test]
fn a_renumbering_here_and_back_the_session_then_makes_leaves_no_empty_undo_step() -> TestResult {
    renumbering_back_then_session_renumbers(false)
}

#[test]
fn a_renumbering_the_session_makes_with_another_change_leaves_no_empty_undo_step() -> TestResult {
    renumbering_back_then_session_renumbers(true)
}

/// The guest numbers a page with two waiting placements 30 and back, then the session numbers
/// it 30, and (`rename`) also renames the project in the same sync.
fn renumbering_back_then_session_renumbers(rename: bool) -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), first) = waiting_session(tmp.path())?;
    let number = |o: &Open| o.project.page("p0003").map(|p| p.number.clone());
    let original = number(&guest_open).ok_or("no page")?;
    // A second waiting placement, then the guest numbers the page 30 and back.
    let second = guest_open.place(2, "wc.contact.no", wc_core::Point::new(80.0, 100.0))?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    for to in ["30", original.as_str()] {
        guest_open.checkpoint();
        guest_open.project.page_mut("p0003").ok_or("no page")?.number = to.into();
        guest_open.project.sort_pages();
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    // Then the session numbers the page 30: the step before the guest's renumbering back is
    // what the guest has now, and is no step to undo.
    host_open.project.page_mut("p0003").ok_or("no page")?.number = "30".into();
    host_open.project.sort_pages();
    if rename {
        host_open.project.meta.name = "Renamed in the session".into();
    }
    for _ in 0..2 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(number(&guest_open).as_deref(), Some("30"), "{:?}", g.notes);
    if rename {
        assert_eq!(guest_open.project.meta.name, "Renamed in the session");
    }
    // One Undo takes back the second placement only.
    g.undo()?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert!(!has_placement(&guest_open, "p0003", &second), "{:?}", g.notes);
    assert!(
        has_placement(&guest_open, "p0003", &first),
        "the first still waits: {:?}",
        g.notes
    );
    assert_eq!(number(&guest_open).as_deref(), Some("30"), "{:?}", g.notes);
    if rename {
        assert_eq!(guest_open.project.meta.name, "Renamed in the session");
    }
    Ok(())
}

#[test]
fn undo_of_an_action_here_on_nothing_that_waits_still_drops_what_waits_after_a_renumbering() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), first) = waiting_session(tmp.path())?;
    let second = guest_open.place(2, "wc.contact.no", wc_core::Point::new(80.0, 100.0))?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // The latest action here renames the project: nothing that waits.
    guest_open.checkpoint();
    guest_open.project.meta.name = "Renamed here".into();
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // The session renumbers the waiting page: the step before the rename has its waiting
    // targets as they are now, and still is the rename's.
    host_open.project.page_mut("p0003").ok_or("no page")?.number = "30".into();
    host_open.project.sort_pages();
    for _ in 0..2 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(guest_open.project.meta.name, "Renamed here", "{:?}", g.notes);
    // Undo drops what waits, as for any action that changed nothing that waits.
    g.undo()?;
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert!(!has_placement(&guest_open, "p0003", &second), "{:?}", g.notes);
    assert!(!has_placement(&guest_open, "p0003", &first), "{:?}", g.notes);
    Ok(())
}

#[test]
fn a_page_renumbered_in_the_undo_history_keeps_the_pages_in_order() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut open = project(tmp.path())?;
    let last = open.project.pages.last().ok_or("no page")?.clone();
    let order = |p: &Project| p.pages.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
    open.checkpoint();
    open.project.meta.name = "Renamed".into();
    // The session numbers the last page before all others: so does the step Undo brings back.
    open.renumber_in_history(&last.id, &last.number, "0");
    open.project.page_mut(&last.id).ok_or("no page")?.number = "0".into();
    open.project.sort_pages();
    open.undo();
    assert_eq!(open.project.pages.first().map(|p| p.id.clone()), Some(last.id.clone()));
    let mut sorted = open.project.clone();
    sorted.sort_pages();
    assert_eq!(order(&open.project), order(&sorted));
    Ok(())
}

#[test]
fn a_newer_version_taken_while_a_symbol_waits_still_goes_out() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, log, mut host_open, mut g, mut guest_open), item) = waiting_session(tmp.path())?;
    // The guest takes another version of the waiting symbol from its library.
    let mut v2 = guest_open.symbols.get("wc.contact.no").cloned().ok_or("not loaded")?;
    v2.name = "Contact, newer".into();
    store::write_vendored_symbol(&guest_open.dir, &v2)?;
    guest_open
        .project
        .lock
        .symbols
        .get_mut("wc.contact.no")
        .ok_or("pin")?
        .hash = v2.hash();
    guest_open.symbols.insert(v2.id.clone(), v2.clone());
    log.fail_vendor(false);
    std::thread::sleep(ITEM_RETRY_LATER);
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(host_open.symbols.get("wc.contact.no"), Some(&v2), "{:?}", g.notes);
    assert!(has_placement(&host_open, "p0003", &item));
    Ok(())
}

fn waiting(names: &[String]) -> std::collections::BTreeSet<String> {
    names.iter().cloned().collect()
}

#[test]
fn a_rename_to_a_tag_only_a_waiting_page_names_waits_as_a_pair() -> TestResult {
    let k1 = wc_core::DeviceTag::parse("-K1")?;
    let k2 = wc_core::DeviceTag::parse("-K2")?;
    let mut base = Project::new("Test");
    let mut page = Page::new("p1", "1", "One");
    let mut contact = symbol("s1", 10.0);
    contact.symbol = "contact".into();
    contact.tag = Some(k1.clone());
    page.symbols.push(contact);
    base.pages.push(page);
    base.devices.insert(k1.clone(), Default::default());
    // On the same page: -K1 renamed to -K2, and a symbol the session does not have placed.
    let mut local = base.clone();
    let p1 = local.page_mut("p1").ok_or("no page")?;
    p1.symbols[0].tag = Some(k2.clone());
    p1.symbols.push(symbol("s9", 40.0));
    local.devices.remove(&k1);
    local.devices.insert(k2.clone(), Default::default());
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Symbol, "coil")]));
    assert!(held.now.devices.contains_key(&k2), "the new tag waits with its page");
    assert_eq!(
        held.now.devices.get(&k1),
        Some(&None),
        "and the old one's removal with it"
    );
    assert_eq!(held.renames.len(), 1);
    assert_eq!(held.renames[0].devices, [k1, k2].into_iter().collect());
    Ok(())
}

#[test]
fn a_new_page_that_waits_for_its_symbol_is_kept_while_others_changes_come_in() -> TestResult {
    let mut base = Project::new("Test");
    base.pages.push(Page::new("p1", "1", "One"));
    let mut local = base.clone();
    let mut new_page = Page::new("p9", "9", "New");
    new_page.symbols.push(symbol("s1", 10.0));
    local.pages.push(new_page.clone());
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Symbol, "coil")]));
    assert_eq!(held.items, ["symbol coil"]);
    // The session renamed the project meanwhile and has no page p9.
    let mut session = base.clone();
    session.meta.name = "Renamed".into();
    let lost = held.put_back(&mut session);
    assert!(lost.is_empty(), "{lost:?}");
    assert_eq!(session.page("p9"), Some(&new_page));
    assert_eq!(session.meta.name, "Renamed");
    // A page p9 the session made meanwhile is its own: the waiting page is not kept, and said so.
    let mut session = base.clone();
    session.pages.push(Page::new("p9", "9", "Theirs"));
    assert_eq!(held.put_back(&mut session), ["page 9"]);
    assert_eq!(session.page("p9").map(|p| p.title.as_str()), Some("Theirs"));
    Ok(())
}

#[test]
fn a_waiting_page_and_its_new_device_go_back_together_or_not_at_all() -> TestResult {
    let k1 = wc_core::DeviceTag::parse("-K1")?;
    let mut base = Project::new("Test");
    base.pages.push(Page::new("p1", "1", "One"));
    // A symbol the session does not have placed here, with the new device it made.
    let mut local = base.clone();
    local
        .page_mut("p1")
        .ok_or("no page")?
        .symbols
        .push(wc_core::PlacedSymbol {
            tag: Some(k1.clone()),
            ..symbol("s9", 40.0)
        });
    local.devices.insert(k1.clone(), wc_core::Device::default());
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Symbol, "coil")]));
    assert!(held.now.devices.contains_key(&k1));
    // The session made a device of its own under the same tag meanwhile: the placement must not
    // come back naming it.
    let theirs = wc_core::Device {
        description: "Theirs".into(),
        ..Default::default()
    };
    let mut session = base.clone();
    session.devices.insert(k1.clone(), theirs.clone());
    assert_eq!(held.put_back(&mut session), ["device -K1", "page 1"]);
    assert_eq!(session.devices.get(&k1), Some(&theirs));
    assert_eq!(session.page("p1"), base.page("p1"));
    // The session changed the page meanwhile: the device does not stay without it.
    let mut session = base.clone();
    session.page_mut("p1").ok_or("no page")?.title = "Theirs".into();
    assert_eq!(held.put_back(&mut session), ["page 1", "device -K1"]);
    assert!(!session.devices.contains_key(&k1));
    // Neither: both come back.
    let mut session = base.clone();
    assert!(held.put_back(&mut session).is_empty());
    assert_eq!(session, local);
    Ok(())
}

#[test]
fn a_new_device_s_footprint_goes_back_with_it_or_not_at_all() -> TestResult {
    use wc_core::panel::{Panel, PanelItem};
    let k1 = wc_core::DeviceTag::parse("-K1")?;
    let mut base = Project::new("Test");
    base.pages.push(Page::new("p1", "1", "One"));
    base.panels.push(Panel::new("pn1", "Plate", 400.0, 300.0));
    // A waiting placement made -K1 here, and -K1 was placed on the plate too.
    let mut local = base.clone();
    local
        .page_mut("p1")
        .ok_or("no page")?
        .symbols
        .push(wc_core::PlacedSymbol {
            tag: Some(k1.clone()),
            ..symbol("s9", 40.0)
        });
    local.devices.insert(k1.clone(), wc_core::Device::default());
    local.panels[0].items.push(PanelItem::Device {
        tag: k1.clone(),
        at: wc_core::geom::Point::new(10.0, 10.0),
        width: 20.0,
        height: 40.0,
        rotation: wc_core::geom::Rotation::R0,
    });
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Symbol, "coil")]));
    assert!(held.now.panels.contains_key("pn1"));
    // The session changed the page meanwhile: neither the device nor its footprint stays.
    let mut session = base.clone();
    session.page_mut("p1").ok_or("no page")?.title = "Theirs".into();
    assert_eq!(held.put_back(&mut session), ["page 1", "device -K1", "panel pn1"]);
    assert!(!session.devices.contains_key(&k1));
    assert_eq!(session.panels, base.panels);
    Ok(())
}

#[test]
fn a_new_strip_s_jumpers_go_back_with_it_or_not_at_all() -> TestResult {
    let x1 = wc_core::DeviceTag::parse("-X1")?;
    let base = Project::new("Test");
    // A new terminal strip whose part waits, with jumpers on it; no page names it.
    let mut local = base.clone();
    local.devices.insert(
        x1.clone(),
        wc_core::Device {
            part: Some("acme.strip".into()),
            ..Default::default()
        },
    );
    local.meta.jumpers.push(wc_core::project::Jumper {
        strip: x1.clone(),
        terminals: vec!["1".into(), "2".into()],
        part: None,
    });
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Part, "acme.strip")]));
    assert!(held.now.devices.contains_key(&x1));
    assert!(held.now.meta.is_some(), "the jumpers wait with their strip");
    // The session made a device of its own under the same tag meanwhile: the jumpers must not
    // come back on it.
    let theirs = wc_core::Device {
        description: "Theirs".into(),
        ..Default::default()
    };
    let mut session = base.clone();
    session.devices.insert(x1.clone(), theirs.clone());
    assert_eq!(held.put_back(&mut session), ["device -X1", "the project properties"]);
    assert_eq!(session.devices.get(&x1), Some(&theirs));
    assert!(session.meta.jumpers.is_empty());
    // Neither changed: both come back.
    let mut session = base.clone();
    assert!(held.put_back(&mut session).is_empty());
    assert_eq!(session, local);
    Ok(())
}

#[test]
fn a_new_device_lost_with_a_waiting_rename_is_named() -> TestResult {
    let k1 = wc_core::DeviceTag::parse("-K1")?;
    let k2 = wc_core::DeviceTag::parse("-K2")?;
    let k5 = wc_core::DeviceTag::parse("-K5")?;
    let mut base = Project::new("Test");
    let mut page = Page::new("p1", "1", "One");
    page.symbols.push(wc_core::PlacedSymbol {
        tag: Some(k1.clone()),
        ..symbol("s1", 10.0)
    });
    base.pages.push(page);
    base.devices.insert(k1.clone(), wc_core::Device::default());
    // On the same page: -K1 renamed to -K2, and a waiting placement that made -K5.
    let mut local = base.clone();
    let p1 = local.page_mut("p1").ok_or("no page")?;
    p1.symbols[0].tag = Some(k2.clone());
    p1.symbols.push(wc_core::PlacedSymbol {
        tag: Some(k5.clone()),
        ..symbol("s9", 40.0)
    });
    local.devices.remove(&k1);
    local.devices.insert(k2.clone(), wc_core::Device::default());
    local.devices.insert(k5.clone(), wc_core::Device::default());
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Symbol, "coil")]));
    assert_eq!(held.renames.len(), 1, "one group: the page joins them");
    let mut session = base.clone();
    session.page_mut("p1").ok_or("no page")?.title = "Theirs".into();
    assert_eq!(
        held.put_back(&mut session),
        ["page 1", "the device rename that waited with it", "device -K5"]
    );
    assert!(!session.devices.contains_key(&k5));
    assert!(!session.devices.contains_key(&k2));
    Ok(())
}

#[test]
fn jumpers_of_a_new_device_lost_with_a_waiting_rename_are_named() -> TestResult {
    let k1 = wc_core::DeviceTag::parse("-K1")?;
    let k2 = wc_core::DeviceTag::parse("-K2")?;
    let x5 = wc_core::DeviceTag::parse("-X5")?;
    let mut base = Project::new("Test");
    let mut page = Page::new("p1", "1", "One");
    page.symbols.push(wc_core::PlacedSymbol {
        tag: Some(k1.clone()),
        ..symbol("s1", 10.0)
    });
    base.pages.push(page);
    base.devices.insert(k1.clone(), wc_core::Device::default());
    // On one waiting page: -K1 renamed to -K2, and a placement that made the strip -X5, which
    // has jumpers.
    let mut local = base.clone();
    let p1 = local.page_mut("p1").ok_or("no page")?;
    p1.symbols[0].tag = Some(k2.clone());
    p1.symbols.push(wc_core::PlacedSymbol {
        tag: Some(x5.clone()),
        ..symbol("s9", 40.0)
    });
    local.devices.remove(&k1);
    local.devices.insert(k2.clone(), wc_core::Device::default());
    local.devices.insert(x5.clone(), wc_core::Device::default());
    local.meta.jumpers.push(wc_core::project::Jumper {
        strip: x5.clone(),
        terminals: vec!["1".into(), "2".into()],
        part: None,
    });
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Symbol, "coil")]));
    assert_eq!(held.renames.len(), 1, "one group: the page joins them");
    let mut session = base.clone();
    session.page_mut("p1").ok_or("no page")?.title = "Theirs".into();
    assert_eq!(
        held.put_back(&mut session),
        [
            "page 1",
            "the device rename that waited with it",
            "device -X5",
            "the project properties"
        ]
    );
    assert!(session.meta.jumpers.is_empty());
    Ok(())
}

#[test]
fn pages_and_panels_of_a_new_device_lost_with_a_waiting_rename_are_named() -> TestResult {
    use wc_core::panel::{Panel, PanelItem};
    let k1 = wc_core::DeviceTag::parse("-K1")?;
    let k2 = wc_core::DeviceTag::parse("-K2")?;
    let k5 = wc_core::DeviceTag::parse("-K5")?;
    let mut base = Project::new("Test");
    let mut page = Page::new("p1", "1", "One");
    page.symbols.push(wc_core::PlacedSymbol {
        tag: Some(k1.clone()),
        ..symbol("s1", 10.0)
    });
    base.pages.push(page);
    base.pages.push(Page::new("p2", "2", "Two"));
    base.panels.push(Panel::new("pn1", "Plate", 400.0, 300.0));
    base.devices.insert(k1.clone(), wc_core::Device::default());
    // On one waiting page: -K1 renamed to -K2, and a placement that made -K5, which page 2 and
    // the plate name too.
    let mut local = base.clone();
    let p1 = local.page_mut("p1").ok_or("no page")?;
    p1.symbols[0].tag = Some(k2.clone());
    p1.symbols.push(wc_core::PlacedSymbol {
        tag: Some(k5.clone()),
        ..symbol("s9", 40.0)
    });
    local
        .page_mut("p2")
        .ok_or("no page")?
        .symbols
        .push(wc_core::PlacedSymbol {
            tag: Some(k5.clone()),
            ..symbol("s8", 60.0)
        });
    local.devices.remove(&k1);
    local.devices.insert(k2.clone(), wc_core::Device::default());
    local.devices.insert(k5.clone(), wc_core::Device::default());
    local.panels[0].items.push(PanelItem::Device {
        tag: k5.clone(),
        at: wc_core::geom::Point::new(10.0, 10.0),
        width: 20.0,
        height: 40.0,
        rotation: wc_core::geom::Rotation::R0,
    });
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Symbol, "coil")]));
    assert_eq!(held.renames.len(), 1, "one group: the pages join them");
    let mut session = base.clone();
    session.page_mut("p1").ok_or("no page")?.title = "Theirs".into();
    assert_eq!(
        held.put_back(&mut session),
        [
            "page 1",
            "the device rename that waited with it",
            "page 2",
            "device -K5",
            "panel pn1"
        ]
    );
    assert_eq!(session.pages[1], base.pages[1]);
    assert_eq!(session.panels, base.panels);
    Ok(())
}

#[test]
fn a_rename_to_a_device_with_a_waiting_part_waits_whole() -> TestResult {
    let k1 = wc_core::DeviceTag::parse("-K1")?;
    let k2 = wc_core::DeviceTag::parse("-K2")?;
    let mut base = Project::new("Test");
    let mut page = Page::new("p1", "1", "One");
    page.symbols.push(wc_core::PlacedSymbol {
        tag: Some(k1.clone()),
        ..symbol("s1", 10.0)
    });
    base.pages.push(page);
    base.pages.push(Page::new("p2", "2", "Two"));
    base.devices.insert(k1.clone(), wc_core::Device::default());
    let mut local = base.clone();
    local.rename_device(&k1, &k2);
    local.devices.get_mut(&k2).ok_or("no -K2")?.part = Some("acme.relay".into());
    local.pages[1].title = "Two, changed".into();
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Part, "acme.relay")]));
    assert_eq!(held.items, ["part acme.relay"]);
    // What goes out meanwhile: the old device and its page as they were, the other page changed.
    let mut sendable = local.clone();
    held.drop_from(&mut sendable);
    assert_eq!(sendable.devices, base.devices);
    assert_eq!(sendable.pages[0], base.pages[0]);
    assert_eq!(sendable.pages[1].title, "Two, changed");
    // Kept here while it waits.
    let mut session = sendable.clone();
    assert!(held.put_back(&mut session).is_empty());
    assert_eq!(session, local);
    Ok(())
}

#[test]
fn a_device_removed_beside_a_waiting_rename_goes_out_at_once() -> TestResult {
    let k1 = wc_core::DeviceTag::parse("-K1")?;
    let k2 = wc_core::DeviceTag::parse("-K2")?;
    let k9 = wc_core::DeviceTag::parse("-K9")?;
    let mut base = Project::new("Test");
    let mut page = Page::new("p1", "1", "One");
    page.symbols.push(wc_core::PlacedSymbol {
        tag: Some(k1.clone()),
        ..symbol("s1", 10.0)
    });
    // -K9 is drawn on the same page, and stays drawn there.
    page.symbols.push(wc_core::PlacedSymbol {
        tag: Some(k9.clone()),
        ..symbol("s9", 30.0)
    });
    base.pages.push(page);
    base.devices.insert(k1.clone(), wc_core::Device::default());
    base.devices.insert(k9.clone(), wc_core::Device::default());
    let mut local = base.clone();
    local.rename_device(&k1, &k2);
    local.devices.get_mut(&k2).ok_or("no -K2")?.part = Some("acme.relay".into());
    local.devices.remove(&k9);
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Part, "acme.relay")]));
    // The rename waits whole; -K9 is not part of it.
    assert!(held.now.devices.contains_key(&k1));
    assert!(held.now.devices.contains_key(&k2));
    assert!(!held.now.devices.contains_key(&k9));
    let mut sendable = local.clone();
    held.drop_from(&mut sendable);
    assert!(sendable.devices.contains_key(&k1));
    assert!(!sendable.devices.contains_key(&k9));
    Ok(())
}

#[test]
fn a_device_removed_beside_a_waiting_rename_on_a_panel_goes_out_at_once() -> TestResult {
    use wc_core::panel::{Panel, PanelItem};
    let k1 = wc_core::DeviceTag::parse("-K1")?;
    let k2 = wc_core::DeviceTag::parse("-K2")?;
    let k9 = wc_core::DeviceTag::parse("-K9")?;
    let mut base = Project::new("Test");
    let mut panel = Panel::new("pn1", "Plate", 400.0, 300.0);
    for (tag, x) in [(&k1, 10.0), (&k9, 60.0)] {
        panel.items.push(PanelItem::Device {
            tag: tag.clone(),
            at: wc_core::geom::Point::new(x, 10.0),
            width: 20.0,
            height: 40.0,
            rotation: wc_core::geom::Rotation::R0,
        });
    }
    base.panels.push(panel);
    base.devices.insert(k1.clone(), wc_core::Device::default());
    base.devices.insert(k9.clone(), wc_core::Device::default());
    let mut local = base.clone();
    local.rename_device(&k1, &k2);
    local.devices.get_mut(&k2).ok_or("no -K2")?.part = Some("acme.relay".into());
    // -K9 leaves the device list but keeps its footprint on the plate.
    local.devices.remove(&k9);
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Part, "acme.relay")]));
    assert!(held.now.devices.contains_key(&k1));
    assert!(held.now.devices.contains_key(&k2));
    assert!(!held.now.devices.contains_key(&k9));
    Ok(())
}

#[test]
fn a_device_removed_beside_a_waiting_rename_with_jumpers_goes_out_at_once() -> TestResult {
    let x1 = wc_core::DeviceTag::parse("-X1")?;
    let x2 = wc_core::DeviceTag::parse("-X2")?;
    let x9 = wc_core::DeviceTag::parse("-X9")?;
    let mut base = Project::new("Test");
    for strip in [&x1, &x9] {
        base.meta.jumpers.push(wc_core::project::Jumper {
            strip: strip.clone(),
            terminals: vec!["1".into(), "2".into()],
            part: None,
        });
        base.devices.insert(strip.clone(), wc_core::Device::default());
    }
    let mut local = base.clone();
    local.rename_device(&x1, &x2);
    local.devices.get_mut(&x2).ok_or("no -X2")?.part = Some("acme.strip".into());
    local.devices.remove(&x9);
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Part, "acme.strip")]));
    assert!(held.now.devices.contains_key(&x1));
    assert!(held.now.devices.contains_key(&x2));
    assert!(!held.now.devices.contains_key(&x9));
    Ok(())
}

#[test]
fn a_recorded_rename_of_a_device_nothing_draws_waits_as_a_pair() -> TestResult {
    let k1 = wc_core::DeviceTag::parse("-K1")?;
    let k2 = wc_core::DeviceTag::parse("-K2")?;
    let mut base = Project::new("Test");
    base.devices.insert(k1.clone(), wc_core::Device::default());
    let mut local = base.clone();
    local.rename_device(&k1, &k2);
    local.devices.get_mut(&k2).ok_or("no -K2")?.part = Some("acme.relay".into());
    let parts = waiting(&[item_name(ItemKind::Part, "acme.relay")]);
    let renamed: BTreeMap<_, _> = [(k1.clone(), k2.clone())].into_iter().collect();
    let held = Held::find_renamed(&local, &base, &parts, &renamed);
    // The old device's removal waits with the new one: the session keeps -K1 meanwhile.
    assert_eq!(held.now.devices.get(&k1), Some(&None));
    assert!(held.now.devices.contains_key(&k2));
    assert_eq!(held.renames.len(), 1);
    assert_eq!(held.renames[0].devices, [k1.clone(), k2.clone()].into_iter().collect());
    let mut sendable = local.clone();
    held.drop_from(&mut sendable);
    assert_eq!(sendable.devices, base.devices);
    // A rename taken back here is no pair: -K1 is here again.
    let held = Held::find_renamed(&base, &base, &parts, &renamed);
    assert!(held.is_empty());
    Ok(())
}

#[test]
fn a_recorded_strip_rename_waits_while_its_jumpers_wait_for_another_part() -> TestResult {
    let x1 = wc_core::DeviceTag::parse("-X1")?;
    let x2 = wc_core::DeviceTag::parse("-X2")?;
    let mut base = Project::new("Test");
    base.meta.jumpers.push(wc_core::project::Jumper {
        strip: x1.clone(),
        terminals: vec!["1".into(), "2".into()],
        part: None,
    });
    base.devices.insert(x1.clone(), wc_core::Device::default());
    let mut local = base.clone();
    local.rename_device(&x1, &x2);
    // A new jumper on the strip waits for its part; the strip's own part is in the session.
    local.meta.jumpers.push(wc_core::project::Jumper {
        strip: x2.clone(),
        terminals: vec!["3".into(), "4".into()],
        part: Some("acme.bridge".into()),
    });
    let parts = waiting(&[item_name(ItemKind::Part, "acme.bridge")]);
    let renamed: BTreeMap<_, _> = [(x1.clone(), x2.clone())].into_iter().collect();
    let held = Held::find_renamed(&local, &base, &parts, &renamed);
    assert!(held.now.meta.is_some(), "the jumpers wait");
    // The rename waits with them, or the session would have -X2 while its jumpers name -X1.
    assert!(held.now.devices.contains_key(&x2));
    assert_eq!(held.now.devices.get(&x1), Some(&None));
    assert_eq!(held.renames.len(), 1);
    assert!(held.renames[0].meta);
    Ok(())
}

#[test]
fn a_device_nothing_draws_renamed_to_a_waiting_placements_tag_stays_in_the_session_meanwhile() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), item) = waiting_session(tmp.path())?;
    let q5 = wc_core::DeviceTag::parse("-Q5")?;
    let q6 = wc_core::DeviceTag::parse("-Q6")?;
    host_open.project.devices.insert(q5.clone(), wc_core::Device::default());
    for _ in 0..2 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert!(guest_open.project.devices.contains_key(&q5));
    // The guest renames the undrawn -Q5 to -Q6 and gives the waiting placement that tag.
    let opts = wc_core::find::FindOptions::default();
    let hits = wc_core::find::find(&guest_open.project, "Q5", opts);
    assert_eq!(guest_open.replace_found(&hits, "Q5", "Q6", opts), Ok(1));
    guest_open.checkpoint();
    let placed = guest_open
        .project
        .page_mut("p0003")
        .and_then(|p| p.symbols.iter_mut().find(|s| s.id == item))
        .ok_or("no placement")?;
    placed.tag = Some(q6.clone());
    for _ in 0..2 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    // The rename waits with the placement: the session keeps -Q5 and has no -Q6 yet.
    assert!(host_open.project.devices.contains_key(&q5), "{:?}", g.notes);
    assert!(!host_open.project.devices.contains_key(&q6));
    assert!(guest_open.project.devices.contains_key(&q6));
    assert!(!guest_open.project.devices.contains_key(&q5));
    Ok(())
}

#[test]
fn renames_made_here_are_followed_to_their_last_tag_until_the_session_has_them() -> TestResult {
    let tag = |t: &str| wc_core::DeviceTag::parse(t);
    let (k1, k2, k3, k5) = (tag("-K1")?, tag("-K2")?, tag("-K3")?, tag("-K5")?);
    let one = |a: &wc_core::DeviceTag, b: &wc_core::DeviceTag| -> BTreeMap<_, _> {
        [(a.clone(), b.clone())].into_iter().collect()
    };
    let mut base = Project::new("Test");
    base.devices.insert(k1.clone(), wc_core::Device::default());
    base.devices.insert(k5.clone(), wc_core::Device::default());
    let mut renames = BTreeMap::new();
    let mut local = base.clone();
    local.rename_device(&k1, &k3);
    note_renames(&mut renames, vec![one(&k1, &k2), one(&k2, &k3)], &base, &local);
    assert_eq!(renames_now(&renames, &local), one(&k1, &k3));
    // Renamed back: no rename.
    note_renames(&mut renames, vec![one(&k3, &k1)], &base, &base);
    assert!(renames.is_empty());
    // One the session has is dropped.
    let mut local = base.clone();
    local.rename_device(&k5, &k2);
    note_renames(&mut renames, vec![one(&k5, &k2)], &base, &local);
    assert_eq!(renames.len(), 1);
    note_renames(&mut renames, Vec::new(), &local, &local);
    assert!(renames.is_empty());
    // So is one taken back here (Undo): a device that gets -K2 later is another device.
    note_renames(&mut renames, vec![one(&k5, &k2)], &base, &local);
    assert_eq!(renames.len(), 1);
    note_renames(&mut renames, Vec::new(), &base, &base);
    assert!(renames.is_empty());
    Ok(())
}

#[test]
fn a_rename_of_a_rename_the_session_has_is_a_rename_of_its_own() -> TestResult {
    let tag = |t: &str| wc_core::DeviceTag::parse(t);
    let (k1, k2, k3) = (tag("-K1")?, tag("-K2")?, tag("-K3")?);
    let mut base = Project::new("Test");
    base.devices.insert(k1.clone(), wc_core::Device::default());
    let mut renames = BTreeMap::new();
    let mut renamed = base.clone();
    renamed.rename_device(&k1, &k2);
    note_renames(
        &mut renames,
        vec![[(k1.clone(), k2.clone())].into_iter().collect()],
        &base,
        &renamed,
    );
    assert_eq!(
        renames_now(&renames, &renamed),
        [(k1.clone(), k2.clone())].into_iter().collect()
    );
    // The session takes -K1 -> -K2; then -K2 is renamed -K3 here.
    let synced = renamed;
    note_renames(
        &mut renames,
        vec![[(k2.clone(), k3.clone())].into_iter().collect()],
        &synced,
        &{
            let mut l = synced.clone();
            l.rename_device(&k2, &k3);
            l
        },
    );
    // And it waits as a pair while the undrawn -K3 waits for its part.
    let mut local = synced.clone();
    local.rename_device(&k2, &k3);
    local.devices.get_mut(&k3).ok_or("no -K3")?.part = Some("acme.relay".into());
    let now = renames_now(&renames, &local);
    assert_eq!(now, [(k2.clone(), k3.clone())].into_iter().collect());
    let parts = waiting(&[item_name(ItemKind::Part, "acme.relay")]);
    let held = Held::find_renamed(&local, &synced, &parts, &now);
    assert_eq!(held.now.devices.get(&k2), Some(&None));
    assert!(held.now.devices.contains_key(&k3));
    Ok(())
}

#[test]
fn undo_of_a_rename_of_a_waiting_rename_brings_the_device_between_back() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut open = project(tmp.path())?;
    let tag = |t: &str| wc_core::DeviceTag::parse(t);
    let (k1, k2, k3) = (tag("-K1")?, tag("-K2")?, tag("-K3")?);
    open.project.devices.insert(k1.clone(), wc_core::Device::default());
    let base = open.project.clone();
    // -K1 renamed -K2 and given a part the session lacks, then -K2 renamed -K3: nothing
    // draws them.
    open.checkpoint();
    open.project.rename_device(&k1, &k2);
    open.project.devices.get_mut(&k2).ok_or("no -K2")?.part = Some("acme.relay".into());
    open.checkpoint();
    open.project.rename_device(&k2, &k3);
    let mut renames = BTreeMap::new();
    let one = |a: &wc_core::DeviceTag, b: &wc_core::DeviceTag| -> BTreeMap<_, _> {
        [(a.clone(), b.clone())].into_iter().collect()
    };
    note_renames(&mut renames, vec![one(&k1, &k2), one(&k2, &k3)], &base, &open.project);
    let parts = waiting(&[item_name(ItemKind::Part, "acme.relay")]);
    let held = Held::find_renamed(&open.project, &base, &parts, &renames_now(&renames, &open.project));
    assert_eq!(held.renames.len(), 1);
    // Undo of the latest action brings -K2 back as it was before it.
    let step = latest_waiting_step(&open, &base, &held).ok_or("a step")?;
    step.apply(&mut open.project);
    assert!(!open.project.devices.contains_key(&k3));
    assert_eq!(
        open.project.devices.get(&k2).and_then(|d| d.part.as_deref()),
        Some("acme.relay")
    );
    assert!(!open.project.devices.contains_key(&k1));
    // At the next sync -K1's removal still waits with -K2: the rename is -K1 -> -K2 now.
    note_renames(&mut renames, Vec::new(), &base, &open.project);
    assert_eq!(renames_now(&renames, &open.project), one(&k1, &k2));
    let held = Held::find_renamed(&open.project, &base, &parts, &renames_now(&renames, &open.project));
    assert_eq!(held.now.devices.get(&k1), Some(&None));
    assert!(held.now.devices.contains_key(&k2));
    Ok(())
}

#[test]
fn a_tag_a_renamed_device_had_before_is_another_devices_once_reused() -> TestResult {
    let tag = |t: &str| wc_core::DeviceTag::parse(t);
    let (k1, k2, k3, k4, k9) = (tag("-K1")?, tag("-K2")?, tag("-K3")?, tag("-K4")?, tag("-K9")?);
    let one = |a: &wc_core::DeviceTag, b: &wc_core::DeviceTag| -> BTreeMap<_, _> {
        [(a.clone(), b.clone())].into_iter().collect()
    };
    let mut base = Project::new("Test");
    base.devices.insert(k1.clone(), wc_core::Device::default());
    base.devices.insert(k9.clone(), wc_core::Device::default());
    // -K1 renamed -K2, then -K3; then -K9 takes the free -K2 and goes on to -K4.
    let mut local = base.clone();
    local.rename_device(&k1, &k3);
    let mut renames = BTreeMap::new();
    note_renames(&mut renames, vec![one(&k1, &k2), one(&k2, &k3)], &base, &local);
    local.rename_device(&k9, &k2);
    note_renames(&mut renames, vec![one(&k9, &k2)], &base, &local);
    local.rename_device(&k2, &k4);
    note_renames(&mut renames, vec![one(&k2, &k4)], &base, &local);
    let pairs: BTreeMap<_, _> = [(k1.clone(), k3.clone()), (k9.clone(), k4.clone())]
        .into_iter()
        .collect();
    assert_eq!(renames_now(&renames, &local), pairs);
    Ok(())
}

#[test]
fn a_tag_an_undone_rename_left_is_another_devices_once_reused() -> TestResult {
    let tag = |t: &str| wc_core::DeviceTag::parse(t);
    let (k1, k2, k3, k9) = (tag("-K1")?, tag("-K2")?, tag("-K3")?, tag("-K9")?);
    let one = |a: &wc_core::DeviceTag, b: &wc_core::DeviceTag| -> BTreeMap<_, _> {
        [(a.clone(), b.clone())].into_iter().collect()
    };
    let mut base = Project::new("Test");
    base.devices.insert(k1.clone(), wc_core::Device::default());
    base.devices.insert(k9.clone(), wc_core::Device::default());
    let mut local = base.clone();
    local.rename_device(&k1, &k3);
    let mut renames = BTreeMap::new();
    note_renames(&mut renames, vec![one(&k1, &k2), one(&k2, &k3)], &base, &local);
    // Undo takes -K3 back to -K2; at the next sync -K9 takes the free -K3.
    local.rename_device(&k3, &k2);
    note_renames(&mut renames, Vec::new(), &base, &local);
    local.rename_device(&k9, &k3);
    note_renames(&mut renames, vec![one(&k9, &k3)], &base, &local);
    let pairs: BTreeMap<_, _> = [(k1.clone(), k2.clone()), (k9.clone(), k3.clone())]
        .into_iter()
        .collect();
    assert_eq!(renames_now(&renames, &local), pairs);
    // So -K1's removal waits with -K2 while only -K2 waits for its part.
    local.devices.get_mut(&k2).ok_or("no -K2")?.part = Some("acme.relay".into());
    let parts = waiting(&[item_name(ItemKind::Part, "acme.relay")]);
    let held = Held::find_renamed(&local, &base, &parts, &pairs);
    assert_eq!(held.now.devices.get(&k1), Some(&None));
    assert!(held.now.devices.contains_key(&k2));
    Ok(())
}

#[test]
fn a_tag_a_waiting_rename_left_on_undo_is_another_devices_at_once() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, _log, mut host_open, mut g, mut guest_open), item) = waiting_session(tmp.path())?;
    let tag = |t: &str| wc_core::DeviceTag::parse(t);
    let (q5, k9) = (tag("-Q5")?, tag("-K9")?);
    host_open.project.devices.insert(q5.clone(), wc_core::Device::default());
    host_open.project.devices.insert(k9.clone(), wc_core::Device::default());
    let sync = |h: &mut Share, ho: &mut Open, g: &mut Share, go: &mut Open| {
        for _ in 0..2 {
            frames((h, ho, "p0002"), (g, go, "p0003"));
        }
    };
    sync(&mut h, &mut host_open, &mut g, &mut guest_open);
    let opts = wc_core::find::FindOptions::default();
    let replace = |o: &mut Open, from: &str, to: &str| -> Result<usize, String> {
        let hits = wc_core::find::find(&o.project, from, opts);
        o.replace_found(&hits, from, to, opts)
    };
    // The undrawn -Q5 is renamed -K2 and the waiting placement given that tag; then -K2 is
    // renamed -K3. The rename waits with the placement.
    assert_eq!(replace(&mut guest_open, "Q5", "K2"), Ok(1));
    guest_open.checkpoint();
    guest_open
        .project
        .page_mut("p0003")
        .and_then(|p| p.symbols.iter_mut().find(|s| s.id == item))
        .ok_or("no placement")?
        .tag = Some(tag("-K2")?);
    sync(&mut h, &mut host_open, &mut g, &mut guest_open);
    assert_eq!(replace(&mut guest_open, "K2", "K3"), Ok(1));
    sync(&mut h, &mut host_open, &mut g, &mut guest_open);
    assert!(host_open.project.devices.contains_key(&q5), "{:?}", g.notes);
    // Undo takes -K3 back to -K2; before the next sync -K9 takes the free -K3.
    g.undo()?;
    g.frame(Some(&mut guest_open), Some("p0003".to_owned()), false);
    assert!(guest_open.project.devices.contains_key(&tag("-K2")?), "{:?}", g.notes);
    assert_eq!(replace(&mut guest_open, "K9", "K3"), Ok(1));
    sync(&mut h, &mut host_open, &mut g, &mut guest_open);
    // -Q5 stays in the session while -K2 waits with the placement.
    assert!(host_open.project.devices.contains_key(&q5), "{:?}", g.notes);
    assert!(!host_open.project.devices.contains_key(&tag("-K2")?));
    Ok(())
}

#[test]
fn devices_renamed_onto_each_other_at_once_wait_together() -> TestResult {
    renamed_onto_each_other("-K111")
}

#[test]
fn devices_renamed_onto_each_other_at_once_wait_together_when_the_first_ones_part_waits() -> TestResult {
    renamed_onto_each_other("-K11")
}

/// One replace renames -K1 (drawn on page 1) to -K11 and -K11 to -K111; device `waits`
/// waits for its part.
fn renamed_onto_each_other(waits: &str) -> TestResult {
    let tag = |t: &str| wc_core::DeviceTag::parse(t);
    let (k1, k11) = (tag("-K1")?, tag("-K11")?);
    let mut base = Project::new("Test");
    base.devices.insert(
        k1.clone(),
        wc_core::Device {
            description: "first".into(),
            ..Default::default()
        },
    );
    base.devices.insert(
        k11.clone(),
        wc_core::Device {
            description: "second".into(),
            ..Default::default()
        },
    );
    let mut page = Page::new("p1", "1", "One");
    page.symbols.push(wc_core::PlacedSymbol {
        tag: Some(k1.clone()),
        ..symbol("s1", 10.0)
    });
    base.pages.push(page);
    let opts = wc_core::find::FindOptions::default();
    let mut local = base.clone();
    let hits = wc_core::find::find(&local, "K1", opts);
    let (_, made) = wc_core::find::replace_renaming(&mut local, &hits, "K1", "K11", opts)?;
    assert_eq!(made.len(), 2, "{made:?}");
    assert_eq!(local.pages[0].symbols[0].tag.as_ref(), Some(&k11));
    local.devices.get_mut(&tag(waits)?).ok_or("no device")?.part = Some("acme.relay".into());
    let mut renames = BTreeMap::new();
    note_renames(&mut renames, vec![made], &base, &local);
    assert_eq!(renames.len(), 2, "two renames, not one chain: {renames:?}");
    // Still two at the next sync: the session has -K11, but as the old side of the second.
    note_renames(&mut renames, Vec::new(), &base, &local);
    assert_eq!(renames.len(), 2, "{renames:?}");
    let parts = waiting(&[item_name(ItemKind::Part, "acme.relay")]);
    let held = Held::find_renamed(&local, &base, &parts, &renames_now(&renames, &local));
    // What goes out meanwhile: both devices, and the page retagged to -K11, as the session
    // has them.
    let mut sendable = local.clone();
    held.drop_from(&mut sendable);
    assert_eq!(sendable.devices, base.devices);
    assert_eq!(sendable.pages, base.pages);
    assert_eq!(held.renames.len(), 1);
    // Kept here while they wait.
    let mut session = sendable.clone();
    assert!(held.put_back(&mut session).is_empty());
    assert_eq!(session, local);
    Ok(())
}

#[test]
fn a_waiting_rename_lost_to_the_session_leaves_another_rename_alone() -> TestResult {
    let tag = |t: &str| wc_core::DeviceTag::parse(t);
    let (k1, k2, k3, k4) = (tag("-K1")?, tag("-K2")?, tag("-K3")?, tag("-K4")?);
    let mut base = Project::new("Test");
    for (id, num, sym, t) in [("p1", "1", "s1", &k1), ("p2", "2", "s3", &k3)] {
        let mut page = Page::new(id, num, "Page");
        page.symbols.push(wc_core::PlacedSymbol {
            tag: Some(t.clone()),
            ..symbol(sym, 10.0)
        });
        base.pages.push(page);
        base.devices.insert(t.clone(), wc_core::Device::default());
    }
    let mut local = base.clone();
    local.rename_device(&k1, &k2);
    local.rename_device(&k3, &k4);
    local.devices.get_mut(&k2).ok_or("no -K2")?.part = Some("acme.relay".into());
    local.devices.get_mut(&k4).ok_or("no -K4")?.part = Some("acme.relay".into());
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Part, "acme.relay")]));
    let mut session = base.clone();
    session.pages[0].title = "Theirs".into();
    assert_eq!(
        held.put_back(&mut session),
        ["page 1", "the device rename that waited with it"]
    );
    // The first rename is lost whole; the second one, on its own page, is put back.
    assert_eq!(session.pages[0].title, "Theirs");
    assert!(session.devices.contains_key(&k1) && !session.devices.contains_key(&k2));
    assert!(!session.devices.contains_key(&k3) && session.devices.contains_key(&k4));
    assert_eq!(session.pages[1], local.pages[1]);
    Ok(())
}

#[test]
fn a_jumper_removed_beside_a_waiting_strip_rename_is_not_taken_for_its_old_side() -> TestResult {
    let x1 = wc_core::DeviceTag::parse("-X1")?;
    let x2 = wc_core::DeviceTag::parse("-X2")?;
    let x9 = wc_core::DeviceTag::parse("-X9")?;
    let mut base = Project::new("Test");
    for strip in [&x1, &x9] {
        base.meta.jumpers.push(wc_core::project::Jumper {
            strip: strip.clone(),
            terminals: vec!["1".into(), "2".into()],
            part: None,
        });
        base.devices.insert(strip.clone(), wc_core::Device::default());
    }
    let mut local = base.clone();
    // -X1 goes with its jumper, which shifts -X9's jumper to the front; -X9 becomes -X2.
    local.meta.jumpers.remove(0);
    local.devices.remove(&x1);
    local.rename_device(&x9, &x2);
    local.devices.get_mut(&x2).ok_or("no -X2")?.part = Some("acme.strip".into());
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Part, "acme.strip")]));
    assert!(!held.now.devices.contains_key(&x1));
    assert!(held.now.devices.contains_key(&x2));
    Ok(())
}

#[test]
fn a_waiting_rename_is_dropped_whole_when_the_session_changed_one_of_its_pages() -> TestResult {
    let k1 = wc_core::DeviceTag::parse("-K1")?;
    let k2 = wc_core::DeviceTag::parse("-K2")?;
    let mut base = Project::new("Test");
    let mut page = Page::new("p1", "1", "One");
    page.symbols.push(wc_core::PlacedSymbol {
        tag: Some(k1.clone()),
        ..symbol("s1", 10.0)
    });
    base.pages.push(page);
    base.devices.insert(k1.clone(), wc_core::Device::default());
    let mut local = base.clone();
    local.rename_device(&k1, &k2);
    local.devices.get_mut(&k2).ok_or("no -K2")?.part = Some("acme.relay".into());
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Part, "acme.relay")]));
    // Someone else changed the page meanwhile: the device keeps its old tag too.
    let mut session = base.clone();
    session.pages[0].title = "Theirs".into();
    let before = session.clone();
    assert_eq!(
        held.put_back(&mut session),
        ["page 1", "the device rename that waited with it"]
    );
    assert_eq!(session, before);
    Ok(())
}

#[test]
fn a_new_device_waiting_beside_a_lost_rename_is_kept() -> TestResult {
    let k1 = wc_core::DeviceTag::parse("-K1")?;
    let k2 = wc_core::DeviceTag::parse("-K2")?;
    let k5 = wc_core::DeviceTag::parse("-K5")?;
    let mut base = Project::new("Test");
    let mut page = Page::new("p1", "1", "One");
    page.symbols.push(wc_core::PlacedSymbol {
        tag: Some(k1.clone()),
        ..symbol("s1", 10.0)
    });
    base.pages.push(page);
    base.pages.push(Page::new("p2", "2", "Two"));
    base.devices.insert(k1.clone(), wc_core::Device::default());
    let mut local = base.clone();
    local.rename_device(&k1, &k2);
    local.devices.get_mut(&k2).ok_or("no -K2")?.part = Some("acme.relay".into());
    // An unrelated new device on another page, waiting for a part of its own.
    local.pages[1].symbols.push(wc_core::PlacedSymbol {
        tag: Some(k5.clone()),
        ..symbol("s5", 10.0)
    });
    local.devices.insert(
        k5.clone(),
        wc_core::Device {
            part: Some("acme.timer".into()),
            ..Default::default()
        },
    );
    let held = Held::find(
        &local,
        &base,
        &waiting(&[
            item_name(ItemKind::Part, "acme.relay"),
            item_name(ItemKind::Part, "acme.timer"),
        ]),
    );
    let mut session = base.clone();
    session.pages[0].title = "Theirs".into();
    assert_eq!(
        held.put_back(&mut session),
        ["page 1", "the device rename that waited with it"]
    );
    assert_eq!(session.pages[0].title, "Theirs");
    assert!(session.devices.contains_key(&k1));
    assert!(!session.devices.contains_key(&k2));
    assert_eq!(session.devices.get(&k5), local.devices.get(&k5));
    assert_eq!(session.pages[1], local.pages[1]);
    Ok(())
}

#[test]
fn a_waiting_page_the_session_left_alone_is_kept_when_another_one_is_lost() -> TestResult {
    let mut base = Project::new("Test");
    base.pages.push(Page::new("pa", "1", "A"));
    base.pages.push(Page::new("pb", "2", "B"));
    let mut local = base.clone();
    local.pages[0].symbols.push(symbol("s1", 10.0));
    local.pages[1].symbols.push(symbol("s2", 10.0));
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Symbol, "coil")]));
    let mut session = base.clone();
    session.pages[0].title = "Theirs".into();
    assert_eq!(held.put_back(&mut session), ["page 1"]);
    assert_eq!(session.pages[0].title, "Theirs");
    assert_eq!(session.pages[1], local.pages[1]);
    Ok(())
}

#[test]
fn comments_on_a_new_page_held_for_a_renamed_device_wait_with_it() -> TestResult {
    let k1 = wc_core::DeviceTag::parse("-K1")?;
    let k2 = wc_core::DeviceTag::parse("-K2")?;
    let mut base = Project::new("Test");
    let mut page = Page::new("p1", "1", "One");
    page.symbols.push(wc_core::PlacedSymbol {
        tag: Some(k1.clone()),
        ..symbol("s1", 10.0)
    });
    base.pages.push(page);
    base.devices.insert(k1.clone(), wc_core::Device::default());
    let mut local = base.clone();
    local.rename_device(&k1, &k2);
    local.devices.get_mut(&k2).ok_or("no -K2")?.part = Some("acme.relay".into());
    // A new page that waits only because it names the renamed device.
    let mut new_page = Page::new("p9", "9", "New");
    new_page.symbols.push(wc_core::PlacedSymbol {
        tag: Some(k2.clone()),
        ..symbol("s9", 10.0)
    });
    local.pages.push(new_page);
    let note = Comment {
        id: "c1".into(),
        page: "p9".into(),
        at: None,
        author: "Ola".into(),
        time: 0,
        text: "Check this".into(),
        resolved: false,
        replies: Vec::new(),
    };
    local.comments.push(note.clone());
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Part, "acme.relay")]));
    let mut session = base.clone();
    assert!(held.put_back(&mut session).is_empty());
    assert_eq!(session.comments, [note]);
    Ok(())
}

#[test]
fn comments_on_a_waiting_new_page_wait_with_it() -> TestResult {
    let base = Project::new("Test");
    let mut local = base.clone();
    let mut new_page = Page::new("p9", "9", "New");
    new_page.symbols.push(symbol("s1", 10.0));
    local.pages.push(new_page);
    let note = Comment {
        id: "c1".into(),
        page: "p9".into(),
        at: None,
        author: "Ola".into(),
        time: 0,
        text: "Check this".into(),
        resolved: false,
        replies: Vec::new(),
    };
    local.comments.push(note.clone());
    let held = Held::find(&local, &base, &waiting(&[item_name(ItemKind::Symbol, "coil")]));
    // Not sent while the page waits: the session has no page to put it on.
    let mut sendable = local.clone();
    held.drop_from(&mut sendable);
    assert!(sendable.comments.is_empty());
    // Back in the copy after a sync that brought the session's project in.
    let mut session = base.clone();
    assert!(held.put_back(&mut session).is_empty());
    assert_eq!(session.comments, [note]);
    Ok(())
}

#[test]
fn undo_of_a_waiting_change_whose_symbol_arrives_first_takes_it_back_in_the_session() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let ((mut h, log, mut host_open, mut g, mut guest_open), item) = waiting_session(tmp.path())?;
    g.undo()?;
    // The session gets the symbol from someone else before this copy looks at the undo: the
    // change goes out with the next sync, and the undo then takes it back there.
    log.fail_vendor(false);
    host_open.place(1, "wc.contact.no", wc_core::Point::new(60.0, 100.0))?;
    for _ in 0..4 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert!(!has_placement(&host_open, "p0003", &item), "{:?}", g.notes);
    assert!(!has_placement(&guest_open, "p0003", &item), "{:?}", g.notes);
    Ok(())
}

#[test]
fn items_sent_before_and_not_taken_go_each_in_an_edit_of_its_own() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut open = project(tmp.path())?;
    let before = open.project.clone();
    open.place(1, "wc.coil", wc_core::Point::new(40.0, 100.0))?;
    open.place(1, "wc.contact.no", wc_core::Point::new(80.0, 100.0))?;
    let added = |edits: &[ProjectEdit]| -> Vec<Vec<String>> {
        edits
            .iter()
            .filter_map(|e| match e {
                ProjectEdit::AddItems { symbols, .. } => Some(symbols.iter().map(|(s, _)| s.id.clone()).collect()),
                _ => None,
            })
            .collect()
    };
    let none = Default::default();
    let out = new_items(&open.dir, &open.project, &before, &before, &none, &none, &none);
    assert_eq!(
        added(&out.edits),
        [vec!["wc.coil", "wc.contact.no"]],
        "first time: one edit"
    );
    // The contact was sent before and the session did not take it: it goes alone, so the coil
    // gets in even when the session cannot store the contact.
    let apart = [item_name(ItemKind::Symbol, "wc.contact.no")].into();
    let out = new_items(&open.dir, &open.project, &before, &before, &none, &apart, &none);
    assert_eq!(added(&out.edits), [vec!["wc.contact.no"], vec!["wc.coil"]]);
    Ok(())
}

#[test]
fn undo_is_offered_for_a_waiting_change_when_the_session_has_nothing_of_this_persons() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (_h, _log, _host_open, mut g, _guest_open) = session(tmp.path())?;
    assert!(!g.can_undo(), "nothing of the guest's anywhere");
    g.holding = vec![item_name(ItemKind::Symbol, "wc.coil")];
    assert!(g.can_undo(), "the waiting change is undone here");
    Ok(())
}

#[test]
fn a_guest_logo_named_like_an_unused_file_of_the_host_gets_a_name_of_its_own() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut host_open = project(&tmp.path().join("host"))?;
    host_open.save()?;
    // An old logo the project no longer uses lies in the host's folder.
    let old_logo = b"\x89PNG\r\n\x1a\nthe old logo";
    std::fs::create_dir_all(host_open.dir.join(wc_core::Logo::DIR))?;
    std::fs::write(host_open.dir.join("pictures/logo.png"), old_logo)?;
    // The host records into git, as a real session does.
    let mut h = host_with(&host_open, Box::new(GitJournal::open(&host_open.dir)?));
    let mut join = |name: &str| -> std::result::Result<(Share, Open), Box<dyn std::error::Error>> {
        let mut g = guest(&mut h, name, &tmp.path().join(name));
        h.frame(Some(&mut host_open), Some("p0002".into()), false);
        let ids = pending(&h);
        approve(&mut h, ids[0], true);
        let open = g.frame(None, None, false).ok_or("no project")?;
        Ok((g, open))
    };
    let (mut g, mut guest_open) = join("Ola")?;
    let (mut k, mut kari_open) = join("Kari")?;

    // Ola picks a logo whose file name in Ola's own folder is free.
    let src = tmp.path().join("logo.png");
    std::fs::write(&src, PNG)?;
    assert_eq!(guest_open.set_logo_from(&src)?, "pictures/logo.png");
    for _ in 0..4 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
        k.frame(Some(&mut kari_open), Some("p0001".into()), false);
    }
    let file = host_open
        .project
        .meta
        .logo
        .as_ref()
        .map(|l| l.file.clone())
        .ok_or("no logo")?;
    assert_ne!(file, "pictures/logo.png", "a name of its own");
    assert_eq!(guest_open.project.meta.logo, host_open.project.meta.logo);
    assert_eq!(kari_open.project.meta.logo, host_open.project.meta.logo);
    for dir in [&host_open.dir, &guest_open.dir, &kari_open.dir] {
        assert_eq!(file_bytes(dir, &file)?, PNG, "{}", dir.display());
    }
    assert_eq!(
        file_bytes(&host_open.dir, "pictures/logo.png")?,
        old_logo,
        "the old file is untouched"
    );
    let notes = g.take_notes();
    assert!(
        notes.iter().any(|n| n.contains(&format!("renamed {file} here"))),
        "{notes:?}"
    );
    Ok(())
}

/// A host recording into git, as a real session does, whose session has a picture brand.png
/// its drawings do not use, and a guest Ola that joined: host, its copy, guest, its copy.
fn git_session_with_brand(tmp: &Path) -> std::result::Result<(Share, Open, Share, Open), Box<dyn std::error::Error>> {
    let mut host_open = project(&tmp.join("host"))?;
    host_open.save()?;
    let mut h = host_with(&host_open, Box::new(GitJournal::open(&host_open.dir)?));
    if let Some(Role::Host(hosting)) = h.role.as_mut() {
        let mut all = hosting.host.pictures().clone();
        all.insert("pictures/brand.png".into(), PNG.to_vec());
        hosting.host.share_pictures(all);
    }
    let mut g = guest(&mut h, "Ola", &tmp.join("Ola"));
    h.frame(Some(&mut host_open), Some("p0002".into()), false);
    let ids = pending(&h);
    approve(&mut h, ids[0], true);
    let mut guest_open = g.frame(None, None, false).ok_or("no project")?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    Ok((h, host_open, g, guest_open))
}

#[test]
fn an_undo_asked_for_while_a_clashing_pick_waits_takes_it_back_after_the_rename() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, mut host_open, mut g, mut guest_open) = git_session_with_brand(tmp.path())?;
    let mine = b"\x89PNG\r\n\x1a\nmine".to_vec();
    std::fs::create_dir_all(guest_open.dir.join("pictures"))?;
    std::fs::write(guest_open.dir.join("pictures/brand.png"), &mine)?;
    guest_open.checkpoint();
    guest_open.project.meta.logo = Some(wc_core::Logo {
        file: "pictures/brand.png".into(),
        cover: None,
    });
    // One frame: the pick waits under the clashing name, and is renamed at the next.
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert!(!g.holding.is_empty(), "the pick waits");
    // Undo before that next frame.
    g.undo()?;
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(guest_open.project.meta.logo, None, "taken back here: {:?}", g.notes);
    assert_eq!(host_open.project.meta.logo, None, "never sent");
    assert!(g.holding.is_empty());
    Ok(())
}

#[test]
fn a_clashing_pick_of_a_picture_that_cannot_be_shared_does_not_wait() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, mut host_open, mut g, mut guest_open) = git_session_with_brand(tmp.path())?;
    // The host's page 2 shows brand.png, so the guest takes the session's file.
    host_open.project.page_mut("p0002").ok_or("no page")?.underlay = Some(wc_core::Underlay {
        file: "pictures/brand.png".into(),
        min: wc_core::Point::new(0.0, 0.0),
        max: wc_core::Point::new(10.0, 10.0),
    });
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(file_bytes(&guest_open.dir, "pictures/brand.png")?, PNG);
    // The guest empties that file: it cannot be shared, so nothing would ever let a pick of it
    // go if it waited.
    std::fs::write(guest_open.dir.join("pictures/brand.png"), b"")?;
    guest_open.project.meta.logo = Some(wc_core::Logo {
        file: "pictures/brand.png".into(),
        cover: None,
    });
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert!(g.holding.is_empty(), "{:?}", g.holding);
    assert_eq!(host_open.project.meta.logo, guest_open.project.meta.logo);
    Ok(())
}

/// A GitJournal session whose pictures include a brand.png its drawings do not use; the guest
/// picks a file of its own of that name (`pick`), and once it has a name of its own, takes the
/// pick back with one undo. `used` reads the picked file from a copy.
fn renamed_pick_then_one_undo(
    pick: impl Fn(&mut Open, &str) -> TestResult,
    used: impl Fn(&Open) -> Option<String>,
) -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, mut host_open, mut g, mut guest_open) = git_session_with_brand(tmp.path())?;
    assert_eq!(used(&host_open), None);

    let mine = b"\x89PNG\r\n\x1a\nmine".to_vec();
    std::fs::create_dir_all(guest_open.dir.join("pictures"))?;
    std::fs::write(guest_open.dir.join("pictures/brand.png"), &mine)?;
    pick(&mut guest_open, "pictures/brand.png")?;
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let file = used(&host_open).ok_or("not picked")?;
    assert_ne!(file, "pictures/brand.png", "renamed for the session: {:?}", g.notes);
    assert_eq!(used(&guest_open), Some(file.clone()));
    assert_eq!(file_bytes(&host_open.dir, &file)?, mine);

    // The pick was one change, so one undo takes it back whole, not just the rename (which
    // would leave the session's brand.png in use).
    g.undo()?;
    for _ in 0..5 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(used(&host_open), None, "host: {:?}", g.notes);
    assert_eq!(used(&guest_open), None, "guest");
    Ok(())
}

#[test]
fn one_undo_takes_back_a_guest_logo_that_was_renamed_for_the_session() -> TestResult {
    renamed_pick_then_one_undo(
        |open, file| {
            open.project.meta.logo = Some(wc_core::Logo {
                file: file.into(),
                cover: None,
            });
            Ok(())
        },
        |open| open.project.meta.logo.as_ref().map(|l| l.file.clone()),
    )
}

#[test]
fn one_undo_takes_back_a_guest_underlay_that_was_renamed_for_the_session() -> TestResult {
    renamed_pick_then_one_undo(
        |open, file| {
            open.project.page_mut("p0003").ok_or("no page")?.underlay = Some(wc_core::Underlay {
                file: file.into(),
                min: wc_core::Point::new(0.0, 0.0),
                max: wc_core::Point::new(10.0, 10.0),
            });
            Ok(())
        },
        |open| {
            open.project
                .page("p0003")
                .and_then(|p| p.underlay.as_ref())
                .map(|u| u.file.clone())
        },
    )
}

#[test]
fn a_picture_renamed_for_the_session_keeps_its_name_within_255_bytes() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut open = project(&tmp.path().join("p"))?;
    // A name at the limit already: the mark has to take room from the stem.
    let stem = "æ".repeat(125);
    let file = format!("pictures/{stem}.png");
    assert_eq!(file.rsplit('/').next().map(str::len), Some(254));
    std::fs::create_dir_all(open.dir.join("pictures"))?;
    std::fs::write(open.dir.join(&file), PNG)?;
    open.project.meta.logo = Some(wc_core::Logo {
        file: file.clone(),
        cover: None,
    });
    let (new, _) = rename_picture(&mut open, &file, None)?;
    let name = new.rsplit('/').next().ok_or("no name")?;
    assert!(name.len() <= 255 && name.ends_with(".png"), "{name}");
    assert_eq!(std::fs::read(open.dir.join(&new))?, PNG);
    assert_eq!(
        open.project.meta.logo.as_ref().map(|l| l.file.as_str()),
        Some(new.as_str())
    );
    Ok(())
}

#[test]
fn a_picture_whose_rename_failed_gets_its_own_name_later_although_the_session_has_the_name() -> TestResult {
    let tmp = tempfile::tempdir()?;
    // The session has a picture logo.png that the host's project does not use.
    let theirs = b"\x89PNG\r\n\x1a\nthe host's other logo".to_vec();
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session_sharing(
        tmp.path(),
        |_| Ok(()),
        [("pictures/logo.png".to_owned(), theirs.clone())].into_iter().collect(),
    )?;
    // The guest's own logo.png cannot be renamed at first (an empty file is no picture).
    std::fs::create_dir_all(guest_open.dir.join("pictures"))?;
    std::fs::write(guest_open.dir.join("pictures/logo.png"), b"")?;
    guest_open.project.meta.logo = Some(wc_core::Logo {
        file: "pictures/logo.png".into(),
        cover: None,
    });
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let notes = g.take_notes();
    assert!(
        notes.iter().any(|n| n.contains("could not be given a new name yet")),
        "{notes:?}"
    );
    // Once it can be, it is: no refusal from the session is needed for that.
    std::fs::write(guest_open.dir.join("pictures/logo.png"), PNG)?;
    std::thread::sleep(ITEM_RETRY_LATER);
    for _ in 0..4 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    let file = guest_open
        .project
        .meta
        .logo
        .as_ref()
        .map(|l| l.file.clone())
        .ok_or("no logo")?;
    assert_ne!(file, "pictures/logo.png", "{:?}", g.notes);
    assert_eq!(host_open.project.meta.logo, guest_open.project.meta.logo);
    assert_eq!(file_bytes(&host_open.dir, &file)?, PNG);
    assert_eq!(
        file_bytes(&guest_open.dir, "pictures/logo.png")?,
        PNG,
        "the guest's file stays"
    );
    Ok(())
}

#[test]
fn a_page_too_large_to_go_twice_in_one_message_is_not_folded_into_the_batch() {
    let mut base = Project::new("Test");
    base.pages.push(Page::new("p1", "1", "One"));
    let device = ProjectEdit::SetDevice {
        tag: wc_core::DeviceTag::parse("-K1").unwrap_or_else(|_| unreachable!()),
        device: Some(Device::default()),
        before: None,
    };
    let fold = |base: &Project, title: String| {
        let mut page = base.pages[0].clone();
        page.title = title;
        let mut out = Outgoing {
            page: Some(page),
            edits: vec![device.clone()],
            dropped: Vec::new(),
        };
        fold_own_page(&mut out, base);
        out
    };
    // A small page joins the batch.
    let out = fold(&base, "Two".into());
    assert!(out.page.is_none() && out.edits.len() == 2);
    // One that would not fit a session message as it is and as it was stays a page edit of its
    // own, though either version alone fits.
    base.pages[0].title = "x".repeat(FOLD_LIMIT / 2);
    let out = fold(&base, "y".repeat(FOLD_LIMIT / 2));
    assert!(out.page.is_some() && out.edits.len() == 1);
    // One that fits, though it is more than half of what a message may carry, is folded.
    base.pages[0].title = "x".repeat(net::MAX_LINE * 3 / 8);
    let out = fold(&base, "y".repeat(net::MAX_LINE * 3 / 8));
    assert!(out.page.is_none() && out.edits.len() == 2);
    // Many small edits whose sizes add up to just under the limit: the commas between them take
    // it over, so the page stays apart.
    base.pages[0].title = "One".into();
    let mut page = base.pages[0].clone();
    page.title = "Two".into();
    let each = json_len(&device);
    let n = (FOLD_LIMIT - json_len(&page) - json_len(&base.pages[0])) / each;
    let mut out = Outgoing {
        page: Some(page),
        edits: vec![device.clone(); n],
        dropped: Vec::new(),
    };
    fold_own_page(&mut out, &base);
    assert!(out.page.is_some() && out.edits.len() == n);
}

#[test]
fn a_folder_picked_with_browse_is_kept_exactly_and_a_failure_is_about_the_folder() -> TestResult {
    let mut form = Form::default();
    let picked = std::env::temp_dir().join("Shared copy ");
    form.browsed(crate::Picked::Path(picked.clone()));
    // The trailing space survives: the picked folder is used as picked, not re-typed.
    assert_eq!(form.folder.path()?, picked);
    assert!(form.error.is_none());

    form.error_about = ErrorAbout::Address;
    form.browsed(crate::Picked::Failed("No picker.".into()));
    assert_eq!(form.error.as_deref(), Some("No picker."));
    assert_eq!(form.error_about, ErrorAbout::Folder);
    // Editing the address does not clear it; editing the folder does.
    let before = (form.addr.clone(), form.folder.text.clone());
    form.addr = "192.0.2.10:47812".into();
    form.forget_error_unless(&before.0, &before.1);
    assert!(form.error.is_some());
    form.folder.text.push('x');
    form.forget_error_unless(&before.0, &before.1);
    assert!(form.error.is_none());
    Ok(())
}

#[test]
fn a_picture_whose_write_time_is_unknown_is_read_again_even_at_the_same_size() {
    let at = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
    // Without a write time a same-size replacement looks the same: it must be read.
    assert!(!still_taken(Some(&(4, None)), &(4, None)));
    // With one, an unchanged size and time is the file taken in.
    assert!(still_taken(Some(&(4, Some(at))), &(4, Some(at))));
    assert!(!still_taken(Some(&(5, Some(at))), &(4, Some(at))));
    assert!(!still_taken(None, &(4, Some(at))));
}

/// A refusal that comes in after the frame looked (in the reply to what it sent) is seen by
/// the window in the same sync, and the picks still look at it in the next frame.
#[test]
fn a_refusal_that_comes_in_late_is_seen_at_once_and_kept_for_the_picks() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // As the late pump of a frame would leave it.
    let Some(Role::Guest(j)) = g.role.as_mut() else {
        return Err("not a guest".into());
    };
    j.guest.refused_pages.insert("p0003".into());
    assert!(g.take_refused_pages().contains("p0003"));
    assert!(g.take_refused_pages().is_empty(), "told once");
    assert!(g.refused_later.0.contains("p0003"), "kept for the picks");
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert!(g.refused_later.0.is_empty(), "the picks took it");
    Ok(())
}

#[test]
fn a_refusal_that_comes_with_the_end_of_the_session_is_still_told() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let (mut h, _log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    // The pump that sees the session end carries a refusal of the guest's.
    let Some(Role::Guest(j)) = g.role.as_mut() else {
        return Err("not a guest".into());
    };
    j.guest.refused_pages.insert("p0003".into());
    h.stop();
    g.frame(Some(&mut guest_open), Some("p0003".into()), false);
    assert!(g.role.is_none(), "the session ended");
    assert!(g.take_refused_pages().contains("p0003"));
    Ok(())
}

use std::cell::RefCell;
use std::rc::Rc;

use wc_core::{Comment, Device, DeviceTag};
use wc_share::transport::pair;
use wc_share::{Change, Journal};

use super::*;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A journal the test can look at.
#[derive(Clone, Default)]
struct Log(Rc<RefCell<Vec<Change>>>, Rc<std::cell::Cell<bool>>);

impl Journal for Log {
    fn record(&mut self, _project: &Project, change: &Change) -> wc_share::Result<()> {
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
    let mut host = Host::new(
        open.project.clone(),
        "Hanna",
        "hanna@example.com",
        Box::new(log.clone()),
    );
    // As `Share::start_host` does.
    let files = vendored_files(&open.dir, &open.project);
    host.share_files(files.symbols, files.parts);
    host.share_pictures(wc_pdf::read_pictures(&open.dir, &open.project).files);
    let share = Share {
        role: Some(Role::Host(Box::new(Hosting {
            host,
            hub: Hub::new(),
            listener: None,
            announcer: None,
            port: 0,
            dir: open.dir.clone(),
        }))),
        base: Some(open.project.clone()),
        ..Default::default()
    };
    (share, log)
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
fn frames(a: (&mut Share, &mut Open, &str), b: (&mut Share, &mut Open, &str)) {
    for _ in 0..4 {
        a.0.frame(Some(a.1), Some(a.2.to_owned()), false);
        b.0.frame(Some(b.1), Some(b.2.to_owned()), false);
    }
}

/// The host's share, its journal and project, then the guest's share and project.
type Session = (Share, Log, Open, Share, Open);

/// Host and guest, joined, each in its own folder.
fn session(tmp: &Path) -> std::result::Result<Session, Box<dyn std::error::Error>> {
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
fn a_joined_guest_gets_its_own_copy_on_disk() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (_h, _log, host_open, _g, guest_open) = session(tmp.path())?;
    assert_eq!(guest_open.dir, tmp.path().join("guest").join("Pump station"));
    assert_eq!(guest_open.project.pages, host_open.project.pages);
    assert_eq!(store::load(&guest_open.dir)?.pages, host_open.project.pages);
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

    // The device and the two pages nobody holds are one change; the guest's own page is its
    // page edit.
    assert_eq!(
        log.messages()[before..],
        ["Change device -K1; Edit 2 pages", "Edit page 3 Valves -K1"]
    );
    assert!(host_open.project.devices.contains_key(&k1));
    assert_eq!(host_open.project.pages, guest_open.project.pages);
    assert_eq!(
        host_open.project.page("p0004").map(|p| p.title.as_str()),
        Some("Terminals -K1")
    );
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
    assert_eq!(log.messages()[before..], ["Edit page 1 Cover", "Edit page 2 Motor"]);
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
    std::thread::sleep(ITEM_WAIT);
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(drawn(&host_open));
    assert!(host_open.project.lock.symbols.contains_key("wc.contact.no"));
    let messages = log.messages();
    let add = messages.iter().position(|m| m == "Add wc.contact.no to the project");
    let edit = messages.iter().position(|m| m.starts_with("Edit page 3"));
    assert!(add.is_some() && add < edit, "{messages:?}");
    Ok(())
}

#[test]
fn a_page_goes_without_a_symbol_the_session_keeps_refusing() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let (mut h, log, mut host_open, mut g, mut guest_open) = session(tmp.path())?;
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    log.fail_vendor(true);
    let item = guest_open.place(2, "wc.contact.no", wc_core::Point::new(40.0, 100.0))?;
    for _ in 0..=ITEM_TRIES {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
        std::thread::sleep(ITEM_WAIT);
    }
    frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    assert!(
        host_open
            .project
            .page("p0003")
            .is_some_and(|p| p.symbols.iter().any(|s| s.id == item)),
        "the change is not held forever"
    );
    let notes = g.take_notes();
    assert!(
        notes.iter().any(|n| n.contains("did not take symbol wc.contact.no")),
        "{notes:?}"
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
fn a_symbol_whose_file_becomes_unreadable_stops_holding_the_session() -> TestResult {
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
    assert!(
        host_open
            .project
            .page("p0003")
            .is_some_and(|p| p.symbols.iter().any(|s| s.id == item)),
        "the held change went out"
    );
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
fn a_picture_picked_here_is_not_written_over_by_the_sessions_picture_of_that_name() -> TestResult {
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
    for _ in 0..3 {
        frames((&mut h, &mut host_open, "p0002"), (&mut g, &mut guest_open, "p0003"));
    }
    assert_eq!(file_bytes(&guest_open.dir, "pictures/brand.png")?, mine, "kept");
    let notes: Vec<String> = g
        .take_notes()
        .into_iter()
        .filter(|n| n.contains("pictures/brand.png"))
        .collect();
    assert_eq!(notes.len(), 1, "named once: {notes:?}");
    assert!(notes[0].contains("another picture"), "{}", notes[0]);
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

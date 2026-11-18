use std::cell::RefCell;
use std::rc::Rc;

use super::transport::{GuestLink, Hub, decode, encode, pair};
use super::*;
use wc_core::geom::Point;
use wc_core::{Comment, Device, Page, PlacedSymbol, Project, store};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn demo() -> Project {
    let mut p = Project::new("Demo");
    p.pages.push(Page::new("p0001", "1", "Cover"));
    p.pages.push(Page::new("p0002", "2", "Motor"));
    p.pages.push(Page::new("p0003", "3", "Pump"));
    p
}

fn place(page: &mut Page, id: &str, x: f64) {
    page.symbols.push(PlacedSymbol {
        id: id.into(),
        symbol: "coil".into(),
        at: Point::new(x, 50.0),
        rotation: Default::default(),
        mirror: false,
        tag: None,
    });
}

/// A journal shared with the test so it can look at what was recorded.
#[derive(Clone, Default)]
struct Shared(Rc<RefCell<Vec<Change>>>, Rc<RefCell<Vec<String>>>);

impl Journal for Shared {
    fn record(&mut self, _project: &Project, change: &Change) -> Result<()> {
        self.0.borrow_mut().push(change.clone());
        Ok(())
    }

    fn picture(&mut self, picture: &Picture) -> Result<()> {
        self.1.borrow_mut().push(picture.file.clone());
        Ok(())
    }
}

struct Session {
    host: Host,
    hub: Hub,
    log: Shared,
}

impl Session {
    fn new() -> Self {
        let log = Shared::default();
        let host = Host::new(demo(), "Hanna Host", "hanna@example.com", Box::new(log.clone()));
        Self {
            host,
            hub: Hub::new(),
            log,
        }
    }

    fn connect(&mut self) -> (PeerId, GuestLink, Guest) {
        let (guest_end, host_end) = pair();
        let id = self.hub.add(&mut self.host, host_end);
        (id, guest_end, Guest::new())
    }

    /// Deliver everything both ways until it settles.
    fn pump(&mut self, guests: &mut [(&GuestLink, &mut Guest)]) {
        for _ in 0..4 {
            self.hub.pump(&mut self.host);
            for (link, guest) in guests.iter_mut() {
                for msg in link.drain() {
                    guest.handle(msg);
                }
            }
        }
    }

    /// Connect, ask to join and get approved.
    fn join(&mut self, name: &str) -> std::result::Result<(PeerId, GuestLink, Guest), Box<dyn std::error::Error>> {
        let (id, link, mut guest) = self.connect();
        link.send(guest.join(name, &format!("{}@example.com", name.to_lowercase())))?;
        self.pump(&mut [(&link, &mut guest)]);
        assert!(self.host.approve(id));
        self.pump(&mut [(&link, &mut guest)]);
        assert_eq!(guest.status, GuestStatus::Joined);
        Ok((id, link, guest))
    }
}

#[test]
fn a_guest_on_another_protocol_version_is_turned_away_with_both_versions() -> TestResult {
    let mut s = Session::new();
    let before = s.host.project().clone();
    let (id, link, mut guest) = s.connect();
    let other = crate::PROTOCOL_VERSION + 1;
    guest.status = GuestStatus::Waiting;
    link.send(GuestMsg::Join {
        name: "Ola".into(),
        email: "ola@example.com".into(),
        protocol: other,
    })?;
    s.pump(&mut [(&link, &mut guest)]);
    let want = crate::VersionMismatch {
        host: crate::PROTOCOL_VERSION,
        guest: other,
    };
    assert_eq!(guest.status, GuestStatus::WrongVersion(want));
    let said = want.to_string();
    assert!(
        said.contains(&format!("version {}", crate::PROTOCOL_VERSION)) && said.contains(&format!("version {other}")),
        "{said}"
    );
    assert!(s.hub.is_empty(), "the connection is dropped");
    assert!(s.host.pending().is_empty(), "nobody is asked to approve it");
    assert!(!s.host.approve(id));
    assert_eq!(s.host.project(), &before);
    assert!(s.log.0.borrow().is_empty());
    assert!(s.host.peers().iter().all(|p| p.id != id));

    // The same build still joins.
    s.join("Kari")?;
    Ok(())
}

#[test]
fn a_join_from_a_build_without_versions_is_declined_and_reads_as_declined_there() -> TestResult {
    // A build before versions sends no protocol: it counts as 0.
    let old = r#"{"type":"join","name":"Ola","email":"o@x"}"#;
    assert!(matches!(decode::<GuestMsg>(old)?, GuestMsg::Join { protocol: 0, .. }));
    let mut s = Session::new();
    let (_, link, mut guest) = s.connect();
    link.send(decode::<GuestMsg>(old)?)?;
    s.pump(&mut [(&link, &mut guest)]);
    assert_eq!(
        guest.status,
        GuestStatus::WrongVersion(crate::VersionMismatch {
            host: crate::PROTOCOL_VERSION,
            guest: 0
        })
    );
    // That build's own Declined has no fields: it still reads the refusal as a decline.
    #[derive(serde::Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum OldHostMsg {
        Declined,
    }
    let line = encode(&HostMsg::Declined {
        version: Some(crate::VersionMismatch { host: 1, guest: 0 }),
    })?;
    assert!(matches!(
        serde_json::from_str::<OldHostMsg>(&line)?,
        OldHostMsg::Declined
    ));
    // A plain decline is written as before.
    assert_eq!(
        encode(&HostMsg::Declined { version: None })?.trim(),
        r#"{"type":"declined"}"#
    );
    Ok(())
}

#[test]
fn a_guest_stops_at_the_welcome_of_a_host_on_another_version() {
    let mut guest = Guest::new();
    let _ = guest.join("Ola", "o@x");
    guest.handle(HostMsg::Welcome {
        you: 1,
        project: Box::new(Snapshot::from(&demo())),
        locks: std::collections::BTreeMap::new(),
        peers: Vec::new(),
        history: Vec::new(),
        protocol: 0,
    });
    assert_eq!(
        guest.status,
        GuestStatus::WrongVersion(crate::VersionMismatch {
            host: 0,
            guest: crate::PROTOCOL_VERSION
        })
    );
    assert!(guest.project.is_none(), "nothing of that session is taken");
}

#[test]
fn nobody_joins_without_approval() -> TestResult {
    let mut s = Session::new();
    let (id, link, mut guest) = s.connect();

    // Before asking to join, nothing is allowed.
    link.send(GuestMsg::Open {
        page: Some("p0001".into()),
    })?;
    s.pump(&mut [(&link, &mut guest)]);
    assert_eq!(guest.refusals, vec![Refusal::NotApproved]);
    assert!(s.host.locks().is_empty());

    link.send(guest.join("Ola", "ola@example.com"))?;
    s.pump(&mut [(&link, &mut guest)]);
    assert_eq!(guest.status, GuestStatus::Waiting);
    assert_eq!(
        s.host.pending(),
        vec![(id, "Ola".to_owned(), "ola@example.com".to_owned())]
    );
    assert!(guest.project.is_none(), "no project before approval");

    // Still waiting: edits are refused and change nothing.
    let mut page = Page::new("p0002", "2", "Hacked");
    place(&mut page, "s1", 10.0);
    link.send(GuestMsg::EditPage { page })?;
    link.send(GuestMsg::EditProject {
        edit: ProjectEdit::RemovePage { id: "p0003".into() },
        seq: 1,
    })?;
    s.pump(&mut [(&link, &mut guest)]);
    assert_eq!(guest.refusals.len(), 3);
    assert_eq!(s.host.project(), &demo());
    assert!(s.log.0.borrow().is_empty());

    assert!(s.host.decline(id));
    s.pump(&mut [(&link, &mut guest)]);
    assert_eq!(guest.status, GuestStatus::Declined);
    assert!(s.hub.is_empty(), "a declined connection is dropped");
    assert!(s.host.pending().is_empty());
    assert!(!s.host.approve(id), "a declined guest cannot be approved later");
    Ok(())
}

#[test]
fn approved_guest_gets_the_project_and_the_others() -> TestResult {
    let mut s = Session::new();
    s.host.open(Some("p0001"))?;
    let (id, _link, guest) = s.join("Ola")?;
    assert_eq!(guest.me, Some(id));
    assert_eq!(guest.project.as_ref(), Some(&demo()));
    assert_eq!(guest.locks.get("p0001"), Some(&HOST));
    assert_eq!(guest.locked_by("p0001").map(|p| p.name.as_str()), Some("Hanna Host"));
    assert_eq!(guest.peers.len(), 2);
    Ok(())
}

#[test]
fn an_open_page_is_read_only_for_the_others() -> TestResult {
    let mut s = Session::new();
    let (a_id, a_link, mut a) = s.join("Ola")?;
    let (b_id, b_link, mut b) = s.join("Kari")?;

    a_link.send(a.open(Some("p0002")))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(a.can_edit("p0002"));
    assert!(!b.can_edit("p0002"));
    assert_eq!(b.locked_by("p0002").map(|p| p.id), Some(a_id));

    // Kari cannot open it or edit it; the host cannot either.
    b_link.send(b.open(Some("p0002")))?;
    let mut page = Page::new("p0002", "2", "Motor");
    place(&mut page, "s9", 90.0);
    b_link.send(GuestMsg::EditPage { page: page.clone() })?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert_eq!(
        b.refusals,
        vec![
            Refusal::PageLocked {
                page: "p0002".into(),
                by: a_id
            },
            Refusal::NoLock { page: "p0002".into() },
        ]
    );
    assert_eq!(
        s.host.open(Some("p0002")),
        Err(Refusal::PageLocked {
            page: "p0002".into(),
            by: a_id
        })
    );
    assert_eq!(s.host.edit_page(page), Err(Refusal::NoLock { page: "p0002".into() }));
    assert!(
        b.edit_page(Page::new("p0002", "2", "x")).is_none(),
        "the guest side refuses too"
    );

    // Every other page is free.
    b_link.send(b.open(Some("p0003")))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert_eq!(s.host.locks().get("p0003"), Some(&b_id));

    // Switching releases the old page; Kari can take it now.
    a_link.send(a.open(Some("p0001")))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert_eq!(s.host.locks().get("p0002"), None);
    b_link.send(b.open(Some("p0002")))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert_eq!(s.host.locks().get("p0002"), Some(&b_id));
    assert_eq!(s.host.locks().get("p0003"), None, "one page at a time");
    Ok(())
}

#[test]
fn page_edits_reach_everyone_and_leaving_releases_locks() -> TestResult {
    let mut s = Session::new();
    let (a_id, a_link, mut a) = s.join("Ola")?;
    let (_, b_link, mut b) = s.join("Kari")?;
    a_link.send(a.open(Some("p0002")))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);

    let mut page = a
        .project
        .as_ref()
        .and_then(|p| p.page("p0002"))
        .cloned()
        .ok_or("page")?;
    place(&mut page, "s1", 40.0);
    page.number = "99".into(); // ignored: numbering is project-level
    let msg = a.edit_page(page).ok_or("lock")?;
    a_link.send(msg)?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);

    let on = |p: &Option<Project>| {
        p.as_ref()
            .and_then(|p| p.page("p0002"))
            .map(|p| (p.number.clone(), p.symbols.len()))
    };
    assert_eq!(on(&a.project), Some(("2".into(), 1)));
    assert_eq!(on(&b.project), Some(("2".into(), 1)));
    assert_eq!(s.host.project().page("p0002").map(|p| p.symbols.len()), Some(1));
    assert_eq!(a.project.as_ref(), Some(s.host.project()));
    assert_eq!(b.project.as_ref(), Some(s.host.project()));

    let log = s.log.0.borrow().clone();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].by, a_id);
    assert_eq!(log[0].author, "Ola <ola@example.com>");
    assert_eq!(log[0].page.as_deref(), Some("p0002"));

    // Dropping the connection is leaving.
    drop(a_link);
    s.pump(&mut [(&b_link, &mut b)]);
    assert!(s.host.locks().is_empty());
    assert!(b.locks.is_empty());
    assert_eq!(b.peers.len(), 2, "host and Kari");
    Ok(())
}

#[test]
fn project_changes_go_through_the_host_in_order() -> TestResult {
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    let (b_id, b_link, mut b) = s.join("Kari")?;
    let tag = wc_core::DeviceTag::parse("-K1")?;
    let device = |d: &str| Device {
        description: d.into(),
        ..Default::default()
    };

    // Both change the same device; the host applies them one after the other.
    a_link.send(a.edit_project(ProjectEdit::SetDevice {
        tag: tag.clone(),
        device: Some(device("Ola's")),
        before: None,
    }))?;
    b_link.send(b.edit_project(ProjectEdit::SetDevice {
        tag: tag.clone(),
        device: Some(device("Kari's")),
        before: None,
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert_eq!(
        s.host.project().devices.get(&tag).map(|d| d.description.as_str()),
        Some("Kari's")
    );
    assert_eq!(a.project.as_ref(), Some(s.host.project()));
    assert_eq!(b.project.as_ref(), Some(s.host.project()));

    // Renumbering and adding pages; a removal of a page someone has open is refused.
    b_link.send(b.open(Some("p0003")))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    a_link.send(a.edit_project(ProjectEdit::Renumber {
        numbers: vec![("p0003".into(), "1.5".into())],
    }))?;
    a_link.send(a.edit_project(ProjectEdit::AddPage {
        page: Page::new("p0004", "4", "Valves"),
    }))?;
    a_link.send(a.edit_project(ProjectEdit::AddPage {
        page: Page::new("p0004", "5", "Dup"),
    }))?;
    a_link.send(a.edit_project(ProjectEdit::RemovePage { id: "p0003".into() }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    let numbers: Vec<_> = s.host.project().pages.iter().map(|p| p.number.as_str()).collect();
    assert_eq!(numbers, ["1", "1.5", "2", "4"]);
    assert!(matches!(a.refusals[0], Refusal::Invalid { .. }));
    assert_eq!(
        a.refusals[1],
        Refusal::PageLocked {
            page: "p0003".into(),
            by: b_id
        }
    );
    assert_eq!(a.project.as_ref(), Some(s.host.project()));
    assert_eq!(b.project.as_ref(), Some(s.host.project()));

    // The holder may remove their own page; the lock goes with it.
    b_link.send(b.edit_project(ProjectEdit::RemovePage { id: "p0003".into() }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(s.host.project().page("p0003").is_none());
    assert!(b.locks.is_empty());
    assert_eq!(b.project.as_ref(), Some(s.host.project()));

    let messages: Vec<_> = s.log.0.borrow().iter().map(|c| c.message.clone()).collect();
    assert_eq!(
        messages,
        [
            "Change device -K1",
            "Change device -K1",
            "Renumber 1 pages",
            "Add page 4 Valves",
            "Remove page p0003"
        ]
    );
    Ok(())
}

#[test]
fn closing_the_session_tells_everyone() -> TestResult {
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    a_link.send(a.open(Some("p0001")))?;
    s.pump(&mut [(&a_link, &mut a)]);
    s.host.close();
    s.pump(&mut [(&a_link, &mut a)]);
    assert_eq!(a.status, GuestStatus::Closed);
    assert!(s.host.locks().is_empty());
    assert!(s.hub.is_empty());
    Ok(())
}

#[test]
fn names_cannot_break_the_author_line() -> TestResult {
    let mut s = Session::new();
    let (id, link, mut guest) = s.connect();
    link.send(guest.join("Eve <evil>\nX", ""))?;
    s.pump(&mut [(&link, &mut guest)]);
    assert_eq!(
        s.host.pending(),
        vec![(id, "Eve evilX".to_owned(), "guest@localhost".to_owned())]
    );
    Ok(())
}

#[test]
fn messages_survive_a_json_line() -> TestResult {
    let mut page = Page::new("p0002", "2", "Motor");
    place(&mut page, "s1", 40.0);
    let msgs = vec![
        GuestMsg::Join {
            name: "Ola".into(),
            email: "o@x".into(),
            protocol: crate::PROTOCOL_VERSION,
        },
        GuestMsg::Open { page: None },
        GuestMsg::EditPage { page },
        GuestMsg::EditProject {
            edit: ProjectEdit::Renumber {
                numbers: vec![("p1".into(), "2".into())],
            },
            seq: 7,
        },
        GuestMsg::Leave,
    ];
    for m in msgs {
        let line = encode(&m)?;
        assert!(line.ends_with('\n') && !line.trim_end().contains('\n'));
        assert_eq!(decode::<GuestMsg>(&line)?, m);
    }
    let welcome = HostMsg::Welcome {
        you: 3,
        project: Box::new(Snapshot::from(&demo())),
        locks: [("p0001".to_owned(), 0)].into(),
        peers: vec![Peer {
            id: 0,
            name: "H".into(),
            page: Some("p0001".into()),
        }],
        history: vec![ChangeInfo {
            id: 1,
            by: 0,
            name: "H".into(),
            message: "Edit page 1 Cover".into(),
            page: Some("p0001".into()),
            kind: ChangeKind::Edit,
            reverts: None,
            undone_by: None,
        }],
        protocol: crate::PROTOCOL_VERSION,
    };
    assert_eq!(decode::<HostMsg>(&encode(&welcome)?)?, welcome);
    // A welcome from a host without undo still reads.
    let old = r#"{"type":"welcome","you":1,"project":"#.to_owned()
        + &serde_json::to_string(&Snapshot::from(&demo()))?
        + r#","locks":{},"peers":[]}"#;
    assert!(matches!(decode::<HostMsg>(&old)?, HostMsg::Welcome { history, .. } if history.is_empty()));
    for m in [GuestMsg::Undo, GuestMsg::Redo, GuestMsg::Revert { change: 7 }] {
        assert_eq!(decode::<GuestMsg>(&encode(&m)?)?, m);
    }
    let mut parts = Parts::default();
    parts.pages.insert("p0002".into(), None);
    parts
        .devices
        .insert(wc_core::DeviceTag::parse("-K1")?, Some(Device::default()));
    let restore = HostMsg::ProjectChanged {
        edit: ProjectEdit::Restore { parts: Box::new(parts) },
        by: 2,
        seq: None,
    };
    assert_eq!(decode::<HostMsg>(&encode(&restore)?)?, restore);
    assert!(decode::<GuestMsg>("{\"type\":\"nope\"}").is_err());
    Ok(())
}

#[test]
fn every_change_is_a_commit_by_its_author_and_can_be_reverted() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    let mut project = store::create(dir, "Demo")?;
    project.pages.push(Page::new("p0002", "2", "Motor"));
    store::save(dir, &project)?;
    let journal = GitJournal::open(dir)?;
    let git = journal.git().clone();
    git.commit("Motor page")?;

    let mut host = Host::new(project, "Hanna Host", "hanna@example.com", Box::new(journal));
    let mut hub = Hub::new();
    let (link, host_end) = pair();
    let id = hub.add(&mut host, host_end);
    let mut guest = Guest::new();
    link.send(guest.join("Ola", "ola@example.com"))?;
    hub.pump(&mut host);
    host.approve(id);
    hub.pump(&mut host);
    link.drain().into_iter().for_each(|m| guest.handle(m));

    // Ola edits the motor page; the host edits the cover.
    link.send(guest.open(Some("p0002")))?;
    hub.pump(&mut host);
    link.drain().into_iter().for_each(|m| guest.handle(m));
    let mut motor = guest
        .project
        .as_ref()
        .and_then(|p| p.page("p0002"))
        .cloned()
        .ok_or("page")?;
    place(&mut motor, "s1", 40.0);
    link.send(guest.edit_page(motor).ok_or("lock")?)?;
    hub.pump(&mut host);
    let ola_commit = git.head().ok_or("head")?;

    host.open(Some("p0001"))?;
    let mut cover = host.project().page("p0001").cloned().ok_or("cover")?;
    cover.title = "Cover sheet".into();
    host.edit_page(cover)?;
    assert!(host.take_journal_errors().is_empty());

    assert_eq!(
        git.author_and_subject(&ola_commit)?,
        ("Ola <ola@example.com>".into(), "Edit page 2 Motor".into())
    );
    assert_eq!(
        git.author_and_subject("HEAD")?,
        (
            "Hanna Host <hanna@example.com>".into(),
            "Edit page 1 Cover sheet".into()
        )
    );

    // Undo Ola's change only: the motor page is empty again, the host's cover edit stays.
    git.revert(&ola_commit)?;
    let back = store::load(dir)?;
    assert_eq!(back.page("p0002").map(|p| p.symbols.len()), Some(0));
    assert_eq!(back.page("p0001").map(|p| p.title.as_str()), Some("Cover sheet"));
    Ok(())
}

#[test]
fn an_edit_sent_while_switching_pages_is_refused_and_put_right() -> TestResult {
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    a_link.send(a.open(Some("p0002")))?;
    s.pump(&mut [(&a_link, &mut a)]);
    assert!(a.can_edit("p0002"));

    // Switching gives the old page up at once: no edit under the old lock.
    a_link.send(a.open(Some("p0003")))?;
    let mut page = a
        .project
        .as_ref()
        .and_then(|p| p.page("p0002"))
        .cloned()
        .ok_or("page")?;
    place(&mut page, "s1", 10.0);
    assert!(!a.can_edit("p0002"));
    assert!(a.edit_page(page.clone()).is_none());

    // An edit that slips through anyway (sent raw, applied to the guest's copy) is refused,
    // and the host sends its own page back so both copies agree.
    if let Some(p) = a.project.as_mut().and_then(|p| p.page_mut("p0002")) {
        *p = page.clone();
    }
    a_link.send(GuestMsg::EditPage { page })?;
    s.pump(&mut [(&a_link, &mut a)]);
    assert_eq!(a.refusals, vec![Refusal::NoLock { page: "p0002".into() }]);
    assert_eq!(a.project.as_ref(), Some(s.host.project()));
    assert!(a.can_edit("p0003"));
    Ok(())
}

/// Fails the first `fail` records, then keeps the rest.
struct Flaky {
    fail: usize,
    log: Shared,
}

impl Journal for Flaky {
    fn record(&mut self, project: &Project, change: &Change) -> Result<()> {
        if self.fail > 0 {
            self.fail -= 1;
            return Err(Error::Closed);
        }
        self.log.record(project, change)
    }
}

#[test]
fn a_change_that_cannot_be_recorded_is_undone() -> TestResult {
    let log = Shared::default();
    let journal = Flaky {
        fail: 2,
        log: log.clone(),
    };
    let mut s = Session {
        host: Host::new(demo(), "Hanna Host", "h@x.no", Box::new(journal)),
        hub: Hub::new(),
        log,
    };
    let (_, a_link, mut a) = s.join("Ola")?;
    let (_, b_link, mut b) = s.join("Kari")?;
    a_link.send(a.open(Some("p0002")))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);

    // Ola's page edit and device change both fail to record: both are undone.
    let mut page = a
        .project
        .as_ref()
        .and_then(|p| p.page("p0002"))
        .cloned()
        .ok_or("page")?;
    place(&mut page, "s1", 10.0);
    a_link.send(a.edit_page(page).ok_or("lock")?)?;
    let tag = wc_core::DeviceTag::parse("-K1")?;
    a_link.send(a.edit_project(ProjectEdit::SetDevice {
        tag: tag.clone(),
        device: Some(Device::default()),
        before: None,
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert_eq!(s.host.project(), &demo());
    assert_eq!(a.project.as_ref(), Some(s.host.project()), "Ola's copy is put right");
    assert_eq!(b.project.as_ref(), Some(s.host.project()));
    assert!(matches!(
        a.refusals[..],
        [Refusal::NotRecorded { .. }, Refusal::NotRecorded { .. }]
    ));
    assert_eq!(s.host.take_journal_errors().len(), 2);

    // Kari's next change is recorded on its own, not together with Ola's.
    b_link.send(b.edit_project(ProjectEdit::AddPage {
        page: Page::new("p0009", "9", "Spare"),
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    let log = s.log.0.borrow().clone();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].author, "Kari <kari@example.com>");
    assert!(s.host.project().page("p0002").is_some_and(|p| p.symbols.is_empty()));
    assert!(!s.host.project().devices.contains_key(&tag));
    Ok(())
}

#[test]
fn a_refused_project_edit_is_taken_back_from_the_guests_copy() -> TestResult {
    let log = Shared::default();
    let mut s = Session {
        host: Host::new(
            demo(),
            "Hanna Host",
            "h@x.no",
            Box::new(Flaky {
                fail: 1,
                log: log.clone(),
            }),
        ),
        hub: Hub::new(),
        log,
    };
    let (_, a_link, mut a) = s.join("Ola")?;
    let (_, b_link, mut b) = s.join("Kari")?;
    b_link.send(b.open(Some("p0003")))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);

    // Applied to Ola's copy at once, then not recorded on the host.
    let tag = wc_core::DeviceTag::parse("-K1")?;
    a_link.send(a.edit_project(ProjectEdit::SetDevice {
        tag: tag.clone(),
        device: Some(Device::default()),
        before: None,
    }))?;
    assert!(a.project.as_ref().is_some_and(|p| p.devices.contains_key(&tag)));
    // Refused by the host's checks: a page someone else has open, a page that is not there.
    a_link.send(a.edit_project(ProjectEdit::RemovePage { id: "p0003".into() }))?;
    a_link.send(a.edit_project(ProjectEdit::Renumber {
        numbers: vec![("p0002".into(), "20".into()), ("p0099".into(), "99".into())],
    }))?;
    assert!(a.project.as_ref().is_some_and(|p| p.page("p0003").is_none()));
    assert_eq!(a.unanswered_edits(), 3);
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(matches!(
        a.refusals[..],
        [
            Refusal::NotRecorded { .. },
            Refusal::PageLocked { .. },
            Refusal::UnknownPage { .. }
        ]
    ));
    assert_eq!(s.host.project(), &demo());
    assert_eq!(a.project.as_ref(), Some(s.host.project()), "Ola's copy is put right");
    assert_eq!(b.project.as_ref(), Some(s.host.project()));
    assert_eq!(a.unanswered_edits(), 0);

    // The same device again is recorded now, and reaches everyone.
    a_link.send(a.edit_project(ProjectEdit::SetDevice {
        tag: tag.clone(),
        device: Some(Device::default()),
        before: None,
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(s.host.project().devices.contains_key(&tag));
    assert_eq!(a.project.as_ref(), Some(s.host.project()));
    assert_eq!(b.project.as_ref(), Some(s.host.project()));
    Ok(())
}

#[test]
fn a_refused_edit_does_not_take_back_work_on_the_page_the_guest_has_open() -> TestResult {
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    a_link.send(a.open(Some("p0002")))?;
    s.pump(&mut [(&a_link, &mut a)]);
    a_link.send(a.edit_project(ProjectEdit::Renumber {
        numbers: vec![("p0002".into(), "20".into()), ("p0099".into(), "99".into())],
    }))?;
    // Drawn before the refusal arrives: the put-back page must not undo it.
    let mut page = a
        .project
        .as_ref()
        .and_then(|p| p.page("p0002"))
        .cloned()
        .ok_or("page")?;
    place(&mut page, "s1", 10.0);
    a_link.send(a.edit_page(page).ok_or("lock")?)?;
    s.hub.pump(&mut s.host);
    let mut refused = false;
    for msg in a_link.drain() {
        refused |= matches!(msg, HostMsg::Refused { edit: Some(1), .. });
        a.handle(msg);
        if refused {
            // Also right after the refusal, before the page edit's answer.
            let mine = a.project.as_ref().and_then(|p| p.page("p0002")).ok_or("page")?;
            assert_eq!((mine.number.as_str(), mine.symbols.len()), ("2", 1));
        }
    }
    assert!(refused);
    assert_eq!(a.project.as_ref(), Some(s.host.project()));
    Ok(())
}

#[test]
fn a_refused_removal_and_new_page_bring_back_the_hosts_page_the_guest_had_open() -> TestResult {
    let log = Shared::default();
    let mut s = Session {
        host: Host::new(
            demo(),
            "Hanna Host",
            "h@x.no",
            Box::new(Flaky {
                fail: 1,
                log: log.clone(),
            }),
        ),
        hub: Hub::new(),
        log,
    };
    let (_, a_link, mut a) = s.join("Ola")?;
    a_link.send(a.open(Some("p0002")))?;
    s.pump(&mut [(&a_link, &mut a)]);
    let old = s.host.project().page("p0002").cloned().ok_or("page")?;
    // The removal is not recorded, so the host keeps the old page and refuses the new one.
    a_link.send(a.edit_project(ProjectEdit::RemovePage { id: "p0002".into() }))?;
    a_link.send(a.edit_project(ProjectEdit::AddPage {
        page: Page::new("p0002", "2", "New"),
    }))?;
    assert!(
        a.project
            .as_ref()
            .and_then(|p| p.page("p0002"))
            .is_some_and(|p| p.title == "New")
    );
    s.pump(&mut [(&a_link, &mut a)]);
    assert_eq!(a.refusals.len(), 2);
    assert_eq!(s.host.project().page("p0002"), Some(&old));
    assert_eq!(a.project.as_ref(), Some(s.host.project()), "the old page is back");
    assert_eq!(a.unanswered_edits(), 0);
    Ok(())
}

#[test]
fn adding_a_page_that_is_there_already_does_not_take_back_work_on_it() -> TestResult {
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    a_link.send(a.open(Some("p0002")))?;
    s.pump(&mut [(&a_link, &mut a)]);
    // The page is there already: nothing changes here, and the host refuses it.
    a_link.send(a.edit_project(ProjectEdit::AddPage {
        page: Page::new("p0002", "2", "Again"),
    }))?;
    let draw = |a: &mut Guest, id: &str, x: f64| -> std::result::Result<GuestMsg, Box<dyn std::error::Error>> {
        let mut page = a
            .project
            .as_ref()
            .and_then(|p| p.page("p0002"))
            .cloned()
            .ok_or("page")?;
        place(&mut page, id, x);
        Ok(a.edit_page(page).ok_or("lock")?)
    };
    a_link.send(draw(&mut a, "s1", 10.0)?)?;
    s.hub.pump(&mut s.host);
    let mut refused = false;
    for msg in a_link.drain() {
        let refusal = matches!(msg, HostMsg::Refused { edit: Some(1), .. });
        a.handle(msg);
        if refusal {
            refused = true;
            let mine = a.project.as_ref().and_then(|p| p.page("p0002")).ok_or("page")?;
            assert_eq!(mine.symbols.len(), 1, "the drawing is kept");
            // Drawn again before the first drawing's answer arrives.
            a_link.send(draw(&mut a, "s2", 20.0)?)?;
        }
    }
    assert!(refused);
    s.pump(&mut [(&a_link, &mut a)]);
    assert!(s.host.project().page("p0002").is_some_and(|p| p.symbols.len() == 2));
    assert_eq!(a.project.as_ref(), Some(s.host.project()));
    assert_eq!(a.unanswered_edits(), 0);
    Ok(())
}

#[test]
fn work_on_the_page_put_back_by_one_refusal_survives_the_next() -> TestResult {
    let log = Shared::default();
    let mut s = Session {
        host: Host::new(
            demo(),
            "Hanna Host",
            "h@x.no",
            Box::new(Flaky {
                fail: 1,
                log: log.clone(),
            }),
        ),
        hub: Hub::new(),
        log,
    };
    let (_, a_link, mut a) = s.join("Ola")?;
    a_link.send(a.open(Some("p0002")))?;
    s.pump(&mut [(&a_link, &mut a)]);
    a_link.send(a.edit_project(ProjectEdit::RemovePage { id: "p0002".into() }))?;
    a_link.send(a.edit_project(ProjectEdit::AddPage {
        page: Page::new("p0002", "2", "New"),
    }))?;
    s.hub.pump(&mut s.host);
    let mut refusals = 0;
    for msg in a_link.drain() {
        let refused = matches!(msg, HostMsg::Refused { edit: Some(_), .. });
        a.handle(msg);
        if !refused {
            continue;
        }
        refusals += 1;
        let mine = a
            .project
            .as_ref()
            .and_then(|p| p.page("p0002"))
            .cloned()
            .ok_or("page")?;
        if refusals == 1 {
            // The host's page is back; drawn on before the second refusal arrives.
            assert_ne!(mine.title, "New");
            let mut page = mine;
            place(&mut page, "s1", 10.0);
            a_link.send(a.edit_page(page).ok_or("lock")?)?;
        } else {
            assert_eq!(mine.symbols.len(), 1, "the drawing is kept");
        }
    }
    assert_eq!(refusals, 2);
    s.pump(&mut [(&a_link, &mut a)]);
    assert!(s.host.project().page("p0002").is_some_and(|p| p.symbols.len() == 1));
    assert_eq!(a.project.as_ref(), Some(s.host.project()));
    assert_eq!(a.unanswered_edits(), 0);
    Ok(())
}

#[test]
fn a_project_edit_before_approval_is_answered() -> TestResult {
    let mut s = Session::new();
    let (id, link, mut guest) = s.connect();
    link.send(guest.join("Ola", "ola@example.com"))?;
    s.pump(&mut [(&link, &mut guest)]);
    link.send(guest.edit_project(ProjectEdit::RemovePage { id: "p0002".into() }))?;
    s.pump(&mut [(&link, &mut guest)]);
    assert_eq!(guest.refusals, vec![Refusal::NotApproved]);
    assert_eq!(guest.unanswered_edits(), 0);
    assert!(s.host.approve(id));
    s.pump(&mut [(&link, &mut guest)]);
    assert_eq!(guest.unanswered_edits(), 0);
    Ok(())
}

#[test]
fn every_project_edit_of_a_guest_gets_exactly_one_answer() -> TestResult {
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    let (_, b_link, mut b) = s.join("Kari")?;
    let device = ProjectEdit::SetDevice {
        tag: wc_core::DeviceTag::parse("-K1")?,
        device: Some(Device::default()),
        before: None,
    };
    // Applied, no change (the same again), refused, and a whole-page edit applied.
    a_link.send(a.edit_project(device.clone()))?;
    a_link.send(a.edit_project(device))?;
    a_link.send(a.edit_project(ProjectEdit::RemovePage { id: "p0099".into() }))?;
    let mut motor = s.host.project().page("p0002").cloned().ok_or("page")?;
    let before = motor.clone();
    place(&mut motor, "s1", 10.0);
    a_link.send(a.edit_project(ProjectEdit::SetPages {
        pages: vec![motor],
        before: vec![before],
    }))?;
    s.hub.pump(&mut s.host);
    let mut answers: Vec<u64> = Vec::new();
    for msg in a_link.drain() {
        match &msg {
            HostMsg::ProjectChanged { seq: Some(n), .. } | HostMsg::Unchanged { seq: n } => answers.push(*n),
            HostMsg::Refused { edit: Some(n), .. } => answers.push(*n),
            _ => {}
        }
        a.handle(msg);
    }
    assert_eq!(answers, [1, 2, 3, 4]);
    assert_eq!(a.unanswered_edits(), 0);
    // The others' copies of the change carry no number.
    let others: Vec<HostMsg> = b_link.drain();
    assert!(
        others
            .iter()
            .all(|m| !matches!(m, HostMsg::ProjectChanged { seq: Some(_), .. })),
        "{others:?}"
    );
    for m in others {
        b.handle(m);
    }
    assert_eq!(a.project.as_ref(), Some(s.host.project()));
    assert_eq!(b.project.as_ref(), Some(s.host.project()));
    Ok(())
}

#[test]
fn comments_and_panels_go_through_the_host() -> TestResult {
    use wc_core::panel::Panel;
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    let (_, b_link, mut b) = s.join("Kari")?;
    let comment = |id: &str, page: &str| Comment {
        id: id.into(),
        page: page.into(),
        at: None,
        author: "Ola".into(),
        time: 0,
        text: "check this".into(),
        resolved: false,
        replies: Vec::new(),
    };
    let panel = |id: &str, name: &str| Panel {
        id: id.into(),
        name: name.into(),
        width: 600.0,
        height: 800.0,
        items: Vec::new(),
    };

    // Comments are put on the page named by the edit, whatever page they claim.
    a_link.send(a.edit_project(ProjectEdit::SetComments {
        page: "p0002".into(),
        comments: vec![comment("c1", "p0003")],
        before: None,
    }))?;
    // A comment id used on another page, and one used twice, are refused.
    b_link.send(b.edit_project(ProjectEdit::SetComments {
        page: "p0003".into(),
        comments: vec![comment("c1", "p0003")],
        before: None,
    }))?;
    b_link.send(b.edit_project(ProjectEdit::SetComments {
        page: "p0001".into(),
        comments: vec![comment("c7", "p0001"), comment("c7", "p0001")],
        before: None,
    }))?;
    a_link.send(a.edit_project(ProjectEdit::SetPanel {
        panel: panel("cab1", "Cabinet"),
        before: None,
    }))?;
    a_link.send(a.edit_project(ProjectEdit::SetPanel {
        panel: panel("cab1", "Main cabinet"),
        before: None,
    }))?;
    a_link.send(a.edit_project(ProjectEdit::SetPanel {
        panel: panel("cab2", "Field box"),
        before: None,
    }))?;
    a_link.send(a.edit_project(ProjectEdit::SetPanel {
        panel: panel("../evil", "Bad"),
        before: None,
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    b_link.send(b.edit_project(ProjectEdit::RemovePanel { id: "cab2".into() }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);

    let p = s.host.project();
    assert_eq!(p.comments.len(), 1);
    assert_eq!(
        (p.comments[0].id.as_str(), p.comments[0].page.as_str()),
        ("c1", "p0002")
    );
    let panels: Vec<_> = p.panels.iter().map(|x| (x.id.as_str(), x.name.as_str())).collect();
    assert_eq!(panels, [("cab1", "Main cabinet")]);
    assert!(matches!(
        b.refusals[..],
        [Refusal::Invalid { .. }, Refusal::Invalid { .. }]
    ));
    assert!(matches!(a.refusals[..], [Refusal::Invalid { .. }]));
    assert_eq!(a.project.as_ref(), Some(p));
    assert_eq!(b.project.as_ref(), Some(p));
    let messages: Vec<_> = s.log.0.borrow().iter().map(|c| c.message.clone()).collect();
    assert_eq!(
        messages,
        [
            "Comments on page p0002",
            "Change panel cab1",
            "Change panel cab1",
            "Change panel cab2",
            "Remove panel cab2"
        ]
    );
    Ok(())
}

#[test]
fn leaving_drops_the_connection() -> TestResult {
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    a_link.send(a.open(Some("p0001")))?;
    s.pump(&mut [(&a_link, &mut a)]);
    a_link.send(GuestMsg::Leave)?;
    s.pump(&mut [(&a_link, &mut a)]);
    assert!(s.hub.is_empty());
    assert!(s.host.locks().is_empty());
    assert_eq!(s.host.peers().len(), 1);
    Ok(())
}

#[test]
fn a_refused_switch_releases_the_old_page_everywhere() -> TestResult {
    let mut s = Session::new();
    let (a_id, a_link, mut a) = s.join("Ola")?;
    let (b_id, b_link, mut b) = s.join("Kari")?;
    a_link.send(a.open(Some("p0002")))?;
    b_link.send(b.open(Some("p0003")))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(a.can_edit("p0002"));

    // Ola asks for Kari's page: refused, and Ola's old page is free on every side.
    a_link.send(a.open(Some("p0003")))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert_eq!(
        a.refusals,
        vec![Refusal::PageLocked {
            page: "p0003".into(),
            by: b_id
        }]
    );
    assert_eq!(s.host.locks().get("p0002"), None);
    assert_eq!(a.locks, s.host.locks().clone());
    assert_eq!(b.locks, s.host.locks().clone());
    assert!(s.host.peers().iter().any(|p| p.id == a_id && p.page.is_none()));
    b_link.send(b.open(Some("p0002")))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(b.can_edit("p0002"), "the old page can be taken");
    Ok(())
}

#[test]
fn a_later_accepted_edit_wins_over_an_earlier_refused_one() -> TestResult {
    let log = Shared::default();
    let mut s = Session {
        host: Host::new(
            demo(),
            "Hanna Host",
            "h@x.no",
            Box::new(Flaky {
                fail: 1,
                log: log.clone(),
            }),
        ),
        hub: Hub::new(),
        log,
    };
    let (_, a_link, mut a) = s.join("Ola")?;
    a_link.send(a.open(Some("p0002")))?;
    s.pump(&mut [(&a_link, &mut a)]);

    // Two edits sent before any answer; only the first fails to record.
    let mut page = a
        .project
        .as_ref()
        .and_then(|p| p.page("p0002"))
        .cloned()
        .ok_or("page")?;
    place(&mut page, "s1", 10.0);
    a_link.send(a.edit_page(page.clone()).ok_or("lock")?)?;
    place(&mut page, "s2", 20.0);
    a_link.send(a.edit_page(page).ok_or("lock")?)?;
    s.pump(&mut [(&a_link, &mut a)]);
    let kept = s.host.project().page("p0002").ok_or("page")?;
    assert_eq!(kept.symbols.len(), 2, "the second edit holds both symbols");
    assert_eq!(a.project.as_ref().and_then(|p| p.page("p0002")), Some(kept));
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_failed_commit_leaves_the_folder_as_it_was() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    let project = store::create(dir, "Demo")?;
    let journal = GitJournal::open(dir)?;
    journal.git().commit("start")?;
    let hook = dir.join(".git/hooks/pre-commit");
    std::fs::create_dir_all(dir.join(".git/hooks"))?;
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n")?;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;

    let mut host = Host::new(project.clone(), "Hanna Host", "hanna@example.com", Box::new(journal));
    let added = host.edit_project(ProjectEdit::AddPage {
        page: Page::new("p0009", "9", "Spare"),
    });
    assert!(matches!(added, Err(Refusal::NotRecorded { .. })), "{added:?}");
    assert_eq!(host.project(), &project);
    assert_eq!(store::load(dir)?, project, "the saved files are put back too");
    let staged = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["diff", "--cached", "--name-only"])
        .output()?;
    assert!(
        staged.status.success(),
        "git diff --cached failed: {}",
        String::from_utf8_lossy(&staged.stderr)
    );
    assert!(
        staged.stdout.is_empty(),
        "nothing of the refused change stays staged: {}",
        String::from_utf8_lossy(&staged.stdout)
    );
    Ok(())
}

#[test]
fn an_old_lock_list_does_not_reopen_a_page_while_switching() -> TestResult {
    let mut s = Session::new();
    let (a_id, a_link, mut a) = s.join("Ola")?;
    a_link.send(a.open(Some("p0002")))?;
    s.pump(&mut [(&a_link, &mut a)]);
    assert!(a.can_edit("p0002"));

    a_link.send(a.open(Some("p0003")))?;
    // A lock list sent before the host saw the switch arrives now.
    a.handle(HostMsg::Locks {
        locks: [("p0002".to_owned(), a_id)].into_iter().collect(),
    });
    assert!(!a.can_edit("p0002"), "no edits while the switch is unanswered");
    let page = a
        .project
        .as_ref()
        .and_then(|p| p.page("p0002"))
        .cloned()
        .ok_or("page")?;
    assert!(a.edit_page(page).is_none());
    s.pump(&mut [(&a_link, &mut a)]);
    assert!(a.can_edit("p0003"));
    assert!(!a.can_edit("p0002"));
    Ok(())
}

#[test]
fn an_edit_made_after_an_old_answer_keeps_the_newer_work() -> TestResult {
    let log = Shared::default();
    let mut s = Session {
        host: Host::new(
            demo(),
            "Hanna Host",
            "h@x.no",
            Box::new(Flaky {
                fail: 1,
                log: log.clone(),
            }),
        ),
        hub: Hub::new(),
        log,
    };
    let (_, a_link, mut a) = s.join("Ola")?;
    a_link.send(a.open(Some("p0002")))?;
    s.pump(&mut [(&a_link, &mut a)]);

    // Edit A (will fail to record) and edit B go out; the host answers both.
    let mut page = a
        .project
        .as_ref()
        .and_then(|p| p.page("p0002"))
        .cloned()
        .ok_or("page")?;
    place(&mut page, "s1", 10.0);
    a_link.send(a.edit_page(page.clone()).ok_or("lock")?)?;
    place(&mut page, "s2", 20.0);
    a_link.send(a.edit_page(page).ok_or("lock")?)?;
    s.hub.pump(&mut s.host);
    let mut answers = a_link.drain().into_iter();
    // Only the answer to A is read before Ola makes edit C from the copy.
    for msg in answers.by_ref() {
        let is_page = matches!(msg, HostMsg::PageChanged { .. });
        a.handle(msg);
        if is_page {
            break;
        }
    }
    let mut c = a
        .project
        .as_ref()
        .and_then(|p| p.page("p0002"))
        .cloned()
        .ok_or("page")?;
    assert_eq!(c.symbols.len(), 2, "the answer to A did not wipe B from the copy");
    place(&mut c, "s3", 30.0);
    a_link.send(a.edit_page(c).ok_or("lock")?)?;
    answers.for_each(|m| a.handle(m));
    s.pump(&mut [(&a_link, &mut a)]);
    let kept = s.host.project().page("p0002").ok_or("page")?;
    assert_eq!(kept.symbols.len(), 3, "B and C are both kept");
    assert_eq!(a.project.as_ref().and_then(|p| p.page("p0002")), Some(kept));
    Ok(())
}

/// Whole sessions over real sockets on this computer.
mod network {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::net::{Listener, connect};

    const PATIENCE: Duration = Duration::from_secs(10);

    fn listen() -> std::result::Result<(Listener, std::net::SocketAddr), Box<dyn std::error::Error>> {
        let l = Listener::bind("127.0.0.1:0")?;
        let addr = l.local_addr()?;
        Ok((l, addr))
    }

    /// Accept, deliver everything both ways and give the socket threads a moment.
    fn step(s: &mut Session, l: &mut Listener, guests: &mut [(&GuestLink, &mut Guest)]) {
        l.accept(&mut s.hub, &mut s.host);
        s.pump(guests);
        std::thread::sleep(Duration::from_millis(5));
    }

    #[test]
    fn two_guests_over_tcp_one_approved_one_declined() -> TestResult {
        let mut s = Session::new();
        let (mut l, addr) = listen()?;

        let a_link = connect(addr)?;
        let mut a = Guest::new();
        let b_link = connect(addr)?;
        let mut b = Guest::new();
        a_link.send(a.join("Ola", "ola@example.com"))?;
        b_link.send(b.join("Kari", "kari@example.com"))?;

        let start = Instant::now();
        while s.host.pending().len() < 2 && start.elapsed() < PATIENCE {
            step(&mut s, &mut l, &mut [(&a_link, &mut a), (&b_link, &mut b)]);
        }
        let pending = s.host.pending();
        let id_of = |name: &str| pending.iter().find(|p| p.1 == name).map(|p| p.0).ok_or("pending");
        let (a_id, b_id) = (id_of("Ola")?, id_of("Kari")?);
        assert!(s.host.approve(a_id));
        assert!(s.host.decline(b_id));

        let start = Instant::now();
        while (a.status != GuestStatus::Joined || b.status != GuestStatus::Declined) && start.elapsed() < PATIENCE {
            step(&mut s, &mut l, &mut [(&a_link, &mut a), (&b_link, &mut b)]);
        }
        assert_eq!(a.status, GuestStatus::Joined);
        assert_eq!(a.project.as_ref(), Some(&demo()), "the project travels on joining");
        assert_eq!(b.status, GuestStatus::Declined);
        assert!(b.project.is_none());
        assert_eq!(s.hub.len(), 1, "the declined connection is dropped");

        // The declined guest's socket is closed by the host.
        let start = Instant::now();
        let mut closed = false;
        while !closed && start.elapsed() < PATIENCE {
            closed = matches!(b_link.try_recv(), Err(Error::Closed));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(closed);

        // Ola opens a page and edits it.
        a_link.send(a.open(Some("p0002")))?;
        let start = Instant::now();
        while !a.can_edit("p0002") && start.elapsed() < PATIENCE {
            step(&mut s, &mut l, &mut [(&a_link, &mut a)]);
        }
        let mut page = a
            .project
            .as_ref()
            .and_then(|p| p.page("p0002"))
            .cloned()
            .ok_or("page")?;
        place(&mut page, "s1", 40.0);
        a_link.send(a.edit_page(page).ok_or("lock")?)?;
        let start = Instant::now();
        while s.log.0.borrow().is_empty() && start.elapsed() < PATIENCE {
            step(&mut s, &mut l, &mut [(&a_link, &mut a)]);
        }
        step(&mut s, &mut l, &mut [(&a_link, &mut a)]);
        assert_eq!(s.host.project().page("p0002").map(|p| p.symbols.len()), Some(1));
        assert_eq!(a.project.as_ref(), Some(s.host.project()));
        assert_eq!(s.log.0.borrow()[0].author, "Ola <ola@example.com>");
        Ok(())
    }

    #[test]
    fn a_dropped_socket_releases_its_locks() -> TestResult {
        let mut s = Session::new();
        let (mut l, addr) = listen()?;
        let a_link = connect(addr)?;
        let mut a = Guest::new();
        a_link.send(a.join("Ola", "ola@example.com"))?;
        let start = Instant::now();
        while s.host.pending().is_empty() && start.elapsed() < PATIENCE {
            step(&mut s, &mut l, &mut [(&a_link, &mut a)]);
        }
        let id = s.host.pending().first().map(|p| p.0).ok_or("pending")?;
        assert!(s.host.approve(id));
        a_link.send(a.open(Some("p0003")))?;
        let start = Instant::now();
        while s.host.locks().get("p0003") != Some(&id) && start.elapsed() < PATIENCE {
            step(&mut s, &mut l, &mut [(&a_link, &mut a)]);
        }
        assert_eq!(s.host.locks().get("p0003"), Some(&id));

        drop(a_link);
        let start = Instant::now();
        while !s.host.locks().is_empty() && start.elapsed() < PATIENCE {
            step(&mut s, &mut l, &mut []);
        }
        assert!(s.host.locks().is_empty());
        assert!(s.hub.is_empty());
        assert!(s.host.open(Some("p0003")).is_ok(), "the page is free again");
        Ok(())
    }

    /// Read until the other side closes; true when it did within the patience.
    fn closed_by_peer(mut raw: TcpStream) -> std::io::Result<bool> {
        raw.set_read_timeout(Some(PATIENCE))?;
        let mut buf = [0u8; 4096];
        loop {
            match raw.read(&mut buf) {
                Ok(0) => return Ok(true),
                Ok(_) => continue,
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => return Ok(true),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    return Ok(false);
                }
                Err(e) => return Err(e),
            }
        }
    }

    #[test]
    fn a_bad_line_closes_only_that_connection() -> TestResult {
        let mut s = Session::new();
        let (mut l, addr) = listen()?;
        l.set_max_line(1024);

        let good = connect(addr)?;
        let mut g = Guest::new();
        good.send(g.join("Ola", "ola@example.com"))?;

        let mut long = TcpStream::connect(addr)?;
        let mut junk = TcpStream::connect(addr)?;
        let start = Instant::now();
        while (s.hub.len() < 3 || s.host.pending().is_empty()) && start.elapsed() < PATIENCE {
            step(&mut s, &mut l, &mut [(&good, &mut g)]);
        }
        assert_eq!(s.hub.len(), 3);
        let id = s.host.pending().first().map(|p| p.0).ok_or("pending")?;
        assert!(s.host.approve(id));

        // One sends a line with no end past the limit, the other a line that is not a message.
        long.write_all(&vec![b'x'; 4096])?;
        junk.write_all(b"{\"this is\": \"not a message\"}\n")?;
        let start = Instant::now();
        while s.hub.len() > 1 && start.elapsed() < PATIENCE {
            step(&mut s, &mut l, &mut [(&good, &mut g)]);
        }
        assert_eq!(s.hub.len(), 1, "both bad connections are dropped");
        assert!(closed_by_peer(long)?);
        assert!(closed_by_peer(junk)?);

        // The good guest carries on.
        good.send(g.open(Some("p0001")))?;
        let start = Instant::now();
        while !g.can_edit("p0001") && start.elapsed() < PATIENCE {
            step(&mut s, &mut l, &mut [(&good, &mut g)]);
        }
        assert_eq!(g.status, GuestStatus::Joined);
        assert!(g.can_edit("p0001"));
        Ok(())
    }
}

/// Finding sessions on the network, with beacons sent to a browser on this computer.
mod discovery {
    use std::net::SocketAddr;
    use std::time::{Duration, Instant};

    use super::TestResult;
    use crate::discovery::{Announcer, Beacon, Browser, MAX_BEACON, Session};
    use crate::net::{Listener, connect};

    const EVERY: Duration = Duration::from_millis(50);
    const FORGET: Duration = Duration::from_millis(400);
    const PATIENCE: Duration = Duration::from_secs(10);

    fn beacon(project: &str, host: &str, port: u16) -> Beacon {
        Beacon {
            project: project.into(),
            host: host.into(),
            port,
            protocol: crate::PROTOCOL_VERSION,
        }
    }

    /// Poll until `done` holds for the list, or fail after a while.
    fn poll_until(b: &mut Browser, done: impl Fn(&[Session]) -> bool) -> std::result::Result<Vec<Session>, String> {
        let start = Instant::now();
        loop {
            let list = b.poll().map_err(|e| e.to_string())?;
            if done(&list) {
                return Ok(list);
            }
            if start.elapsed() > PATIENCE {
                return Err(format!("gave up waiting; last list {list:?}"));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn two_announced_sessions_are_listed_and_a_stopped_one_is_forgotten() -> TestResult {
        let mut b = Browser::bind("127.0.0.1:0".parse()?, FORGET)?;
        let to = b.local_addr()?;
        let pumps = Announcer::start_to(beacon("Pump station", "Ola", 7001), to, EVERY)?;
        let mill = Announcer::start_to(beacon("Mill", "Kari", 7002), to, EVERY)?;
        let list = poll_until(&mut b, |l| l.len() == 2)?;
        assert_eq!(
            list.iter()
                .map(|s| (s.project.as_str(), s.host.as_str(), s.addr.port()))
                .collect::<Vec<_>>(),
            [("Mill", "Kari", 7002), ("Pump station", "Ola", 7001)]
        );
        assert!(
            list.iter().all(|s| s.addr.ip().is_loopback()),
            "joined at the sender's address"
        );

        mill.stop();
        let list = poll_until(&mut b, |l| l.len() == 1)?;
        assert_eq!(list[0].project, "Pump station", "the running one stays");
        drop(pumps);
        poll_until(&mut b, <[Session]>::is_empty)?;
        Ok(())
    }

    #[test]
    fn nothing_is_heard_unless_sharing_was_turned_on_and_other_traffic_is_ignored() -> TestResult {
        let mut b = Browser::bind("127.0.0.1:0".parse()?, FORGET)?;
        let to = b.local_addr()?;
        assert!(b.poll()?.is_empty(), "no announcer, no session");
        let other = std::net::UdpSocket::bind("127.0.0.1:0")?;
        other.send_to(b"hello", to)?;
        other.send_to(br#"{"magic":"something-else","project":"x","host":"y","port":1}"#, to)?;
        // A real beacon sent after them shows the browser has read the other two.
        other.send_to(&beacon("Control", "Ola", 7003).encode(), to)?;
        let list = poll_until(&mut b, |l| !l.is_empty())?;
        assert_eq!(list.len(), 1, "{list:?}");
        assert_eq!(list[0].project, "Control");
        Ok(())
    }

    #[test]
    fn an_announcement_longer_than_a_browser_reads_is_refused() -> TestResult {
        let to: SocketAddr = "127.0.0.1:9".parse()?;
        let fixed = beacon("", "Ola", 7001).encode().len();
        let at_limit = beacon(&"P".repeat(MAX_BEACON - fixed), "Ola", 7001);
        assert_eq!(at_limit.encode().len(), MAX_BEACON);
        Announcer::start_to(at_limit, to, EVERY)?.stop();
        let over = beacon(&"P".repeat(MAX_BEACON - fixed + 1), "Ola", 7001);
        let err = Announcer::start_to(over, to, EVERY).err().ok_or("accepted")?;
        assert!(err.to_string().contains(&MAX_BEACON.to_string()), "{err}");
        Ok(())
    }

    #[test]
    fn names_that_fit_before_the_version_was_announced_still_fit() -> TestResult {
        // The beacon as it was written before the version field: 2048 bytes at most.
        #[derive(serde::Serialize)]
        struct Old<'a> {
            magic: &'a str,
            project: &'a str,
            host: &'a str,
            port: u16,
        }
        let old_len = |project: &str| -> std::result::Result<usize, serde_json::Error> {
            Ok(serde_json::to_vec(&Old {
                magic: "wiringcraft-session",
                project,
                host: "Ola",
                port: 65535,
            })?
            .len())
        };
        let room = 2048 - old_len("")?;
        let project = "P".repeat(room);
        assert_eq!(old_len(&project)?, 2048);
        let b = Beacon {
            port: 65535,
            protocol: u32::MAX,
            ..beacon(&project, "Ola", 1)
        };
        assert!(b.encode().len() <= MAX_BEACON, "{}", b.encode().len());
        // It is sent, and a browser reads it whole.
        let mut browser = Browser::bind("127.0.0.1:0".parse()?, FORGET)?;
        let to = browser.local_addr()?;
        let announcer = Announcer::start_to(b, to, EVERY)?;
        let list = poll_until(&mut browser, |l| !l.is_empty())?;
        announcer.stop();
        assert_eq!(list.len(), 1, "{list:?}");
        assert_eq!(list[0].project, project);
        assert_eq!(list[0].host, "Ola");
        assert_eq!(list[0].addr.port(), 65535);
        assert_eq!(list[0].protocol, u32::MAX);
        Ok(())
    }

    #[test]
    fn a_flood_of_datagrams_does_not_hold_up_a_poll() -> TestResult {
        let mut b = Browser::bind("127.0.0.1:0".parse()?, FORGET)?;
        let to = b.local_addr()?;
        let other = std::net::UdpSocket::bind("127.0.0.1:0")?;
        for _ in 0..2_000 {
            other.send_to(b"noise", to)?;
        }
        let start = Instant::now();
        assert!(b.poll()?.is_empty());
        assert!(start.elapsed() < Duration::from_secs(1));
        Ok(())
    }

    #[test]
    fn one_poll_reads_at_most_its_budget_from_a_queue_that_never_empties() -> TestResult {
        let mut b = Browser::bind("127.0.0.1:0".parse()?, FORGET)?;
        let from: SocketAddr = "127.0.0.1:5000".parse()?;
        let mut calls = 0usize;
        let read = b.receive(|buf| {
            calls += 1;
            buf[..5].copy_from_slice(b"noise");
            Ok((5, from))
        })?;
        assert_eq!(read, crate::discovery::MAX_PER_POLL);
        assert_eq!(calls, crate::discovery::MAX_PER_POLL);

        // A beacon behind the budget is picked up by the next poll, not lost.
        let wire = beacon("Control", "Ola", 7003).encode();
        let mut left = 3;
        b.receive(|buf| {
            if left == 0 {
                return Err(std::io::ErrorKind::WouldBlock.into());
            }
            left -= 1;
            buf[..wire.len()].copy_from_slice(&wire);
            Ok((wire.len(), from))
        })?;
        let list = b.poll()?;
        assert_eq!(list.len(), 1, "{list:?}");
        assert_eq!(list[0].addr, SocketAddr::new(from.ip(), 7003));
        assert_eq!(list[0].protocol, crate::PROTOCOL_VERSION);
        Ok(())
    }

    #[test]
    fn a_beacon_from_an_older_build_reads_as_version_0() -> TestResult {
        let mut b = Browser::bind("127.0.0.1:0".parse()?, FORGET)?;
        let from: SocketAddr = "192.168.1.30:5000".parse()?;
        let wire = br#"{"magic":"wiringcraft-session","project":"Old","host":"Kari","port":7004}"#.to_vec();
        let mut sent = false;
        b.receive(|buf| {
            if sent {
                return Err(std::io::ErrorKind::WouldBlock.into());
            }
            sent = true;
            buf[..wire.len()].copy_from_slice(&wire);
            Ok((wire.len(), from))
        })?;
        let list = b.poll()?;
        assert_eq!(list.len(), 1, "{list:?}");
        assert_eq!(list[0].protocol, 0);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_failing_announcement_keeps_its_reason() -> TestResult {
        // Port 0 cannot be sent to.
        let a = Announcer::start_to(beacon("P", "Ola", 7001), "127.0.0.1:0".parse()?, EVERY)?;
        let start = Instant::now();
        while a.last_error().is_none() && start.elapsed() < PATIENCE {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(a.last_error().is_some());
        Ok(())
    }

    #[test]
    fn joining_by_a_typed_address_works_without_discovery() -> TestResult {
        let l = Listener::bind("127.0.0.1:0")?;
        let port = l.local_addr()?.port();
        // What a person types when the network passes no broadcasts.
        let typed = format!(" 127.0.0.1:{port} ");
        let addr: SocketAddr = typed.trim().parse()?;
        assert!(connect(addr).is_ok());
        Ok(())
    }
}

#[test]
fn project_properties_go_through_the_host() -> TestResult {
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    let (_, b_link, mut b) = s.join("Kari")?;
    let mut meta = s.host.project().meta.clone();
    meta.name = "Pump station".into();
    meta.title_block.insert("customer".into(), "Kommune".into());
    a_link.send(a.edit_project(ProjectEdit::SetMeta {
        meta: Box::new(meta.clone()),
        before: None,
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert_eq!(s.host.project().meta, meta);
    assert_eq!(b.project.as_ref(), Some(s.host.project()));
    let messages: Vec<_> = s.log.0.borrow().iter().map(|c| c.message.clone()).collect();
    assert_eq!(messages, ["Change project properties"]);
    Ok(())
}

#[test]
fn several_pages_change_in_one_commit_unless_someone_else_has_one_open() -> TestResult {
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    let (b_id, b_link, mut b) = s.join("Kari")?;
    b_link.send(b.open(Some("p0003")))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);

    let edited = |id: &str, number: &str, x: f64| -> std::result::Result<Page, String> {
        let mut p = s.host.project().page(id).ok_or("no page")?.clone();
        place(&mut p, "s1", x);
        // A stale display number never changes the page's number.
        p.number = number.into();
        Ok(p)
    };
    let cover = edited("p0001", "99", 10.0)?;
    let motor = edited("p0002", "98", 20.0)?;
    let pump = edited("p0003", "97", 30.0)?;
    let was = |p: &Project, ids: &[&str]| -> Vec<Page> { ids.iter().filter_map(|id| p.page(id).cloned()).collect() };

    // One of the pages is Kari's: nothing of the change is applied.
    a_link.send(a.edit_project(ProjectEdit::SetPages {
        pages: vec![cover.clone(), pump],
        before: was(s.host.project(), &["p0001", "p0003"]),
    }))?;
    // The same page twice is not a valid change.
    a_link.send(a.edit_project(ProjectEdit::SetPages {
        pages: vec![cover.clone(), cover.clone()],
        before: was(s.host.project(), &["p0001", "p0001"]),
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert_eq!(
        a.refusals[0],
        Refusal::PageLocked {
            page: "p0003".into(),
            by: b_id
        }
    );
    assert!(matches!(a.refusals[1], Refusal::Invalid { .. }));
    assert!(s.host.project().pages.iter().all(|p| p.symbols.is_empty()));
    assert!(s.log.0.borrow().is_empty());

    // Pages nobody else has open: one commit, everyone gets them.
    let before = was(s.host.project(), &["p0001", "p0002"]);
    a_link.send(a.edit_project(ProjectEdit::SetPages {
        pages: vec![cover.clone(), motor],
        before: before.clone(),
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    let numbers: Vec<_> = s.host.project().pages.iter().map(|p| p.number.as_str()).collect();
    assert_eq!(numbers, ["1", "2", "3"]);
    assert_eq!(s.host.project().page("p0002").map(|p| p.symbols.len()), Some(1));
    assert_eq!(a.project.as_ref(), Some(s.host.project()));
    assert_eq!(b.project.as_ref(), Some(s.host.project()));
    let messages: Vec<_> = s.log.0.borrow().iter().map(|c| c.message.clone()).collect();
    assert_eq!(messages, ["Edit 2 pages"]);

    // A copy that had not yet seen that change cannot undo it: Kari retitles the cover from
    // the old version, and the host refuses because the cover is no longer what Kari had.
    let mut stale = before[0].clone();
    stale.title = "Front".into();
    b_link.send(b.edit_project(ProjectEdit::SetPages {
        pages: vec![stale],
        before: vec![before[0].clone()],
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(
        matches!(&b.refusals[0], Refusal::Invalid { reason } if reason.contains("changed by someone else")),
        "{:?}",
        b.refusals
    );
    assert_eq!(
        s.host.project().page("p0001"),
        Some(&cover)
            .map(|c| {
                let mut c = c.clone();
                c.number = "1".into();
                c
            })
            .as_ref()
    );
    assert_eq!(s.log.0.borrow().len(), 1);
    Ok(())
}

#[test]
fn a_guest_undo_is_refused_while_the_host_holds_take_backs() -> TestResult {
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    a_link.send(a.open(Some("p0002")))?;
    s.pump(&mut [(&a_link, &mut a)]);
    let mut pa = a
        .project
        .as_ref()
        .and_then(|p| p.page("p0002"))
        .cloned()
        .ok_or("no page")?;
    place(&mut pa, "s1", 10.0);
    a_link.send(a.edit_page(pa.clone()).ok_or("not editable")?)?;
    s.pump(&mut [(&a_link, &mut a)]);
    let commits = s.log.0.borrow().len();

    s.host
        .hold_take_backs(Some("an assistant command is changing the project".into()));
    for m in [a.undo(), GuestMsg::Redo, GuestMsg::Revert { change: 1 }] {
        a_link.send(m)?;
        s.pump(&mut [(&a_link, &mut a)]);
        assert!(!a.taking_back(), "the refusal comes back as the answer");
    }
    assert_eq!(s.host.project().page("p0002"), Some(&pa), "nothing was taken back");
    assert_eq!(s.log.0.borrow().len(), commits, "nothing was written");
    assert!(a.refusals.iter().any(|r| r.to_string().contains("assistant command")));

    s.host.hold_take_backs(None);
    a_link.send(a.undo())?;
    s.pump(&mut [(&a_link, &mut a)]);
    assert!(s.host.project().page("p0002").is_some_and(|p| p.symbols.is_empty()));
    Ok(())
}

#[test]
fn undo_takes_back_only_your_own_change() -> TestResult {
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    let (_, b_link, mut b) = s.join("Kari")?;
    a_link.send(a.open(Some("p0002")))?;
    b_link.send(b.open(Some("p0003")))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);

    let mut pa = a
        .project
        .as_ref()
        .and_then(|p| p.page("p0002"))
        .cloned()
        .ok_or("no page")?;
    place(&mut pa, "s1", 10.0);
    a_link.send(a.edit_page(pa).ok_or("not editable")?)?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    let mut pb = b
        .project
        .as_ref()
        .and_then(|p| p.page("p0003"))
        .cloned()
        .ok_or("no page")?;
    place(&mut pb, "s2", 20.0);
    b_link.send(b.edit_page(pb.clone()).ok_or("not editable")?)?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(a.can_undo() && !a.can_redo());

    // Ola's undo takes back Ola's page only; Kari's later change stays.
    a_link.send(a.undo())?;
    assert!(a.taking_back(), "waiting for the host's restoration");
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(!a.taking_back() && !b.taking_back());
    let host = s.host.project();
    assert!(host.page("p0002").is_some_and(|p| p.symbols.is_empty()));
    assert_eq!(host.page("p0003"), Some(&pb));
    assert_eq!(a.project.as_ref(), Some(host));
    assert_eq!(b.project.as_ref(), Some(host));
    assert!(!a.can_undo() && a.can_redo());
    assert!(b.can_undo());
    assert_eq!(a.history, s.host.history());
    assert_eq!(b.history, s.host.history());

    // Redo puts it back; both are commits by Ola.
    a_link.send(a.redo())?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert_eq!(s.host.project().page("p0002").map(|p| p.symbols.len()), Some(1));
    assert_eq!(a.project.as_ref(), Some(s.host.project()));
    let log = s.log.0.borrow();
    let last: Vec<_> = log
        .iter()
        .rev()
        .take(2)
        .map(|c| (c.message.as_str(), c.author.as_str()))
        .collect();
    assert_eq!(
        last,
        [
            ("Redo: Edit page 2 Motor", "Ola <ola@example.com>"),
            ("Undo: Edit page 2 Motor", "Ola <ola@example.com>"),
        ]
    );
    Ok(())
}

#[test]
fn anyones_change_can_be_taken_back_from_the_list() -> TestResult {
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    let (_, b_link, mut b) = s.join("Kari")?;
    let tag = wc_core::DeviceTag::parse("-K1")?;
    let device = |d: &str| Device {
        description: d.into(),
        ..Default::default()
    };
    a_link.send(a.edit_project(ProjectEdit::SetDevice {
        tag: tag.clone(),
        device: Some(device("Ola's")),
        before: None,
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    let first = b.history.last().map(|c| c.id).ok_or("not in the list")?;

    // Kari takes back Ola's change: the device is gone for everyone, recorded as Kari's.
    b_link.send(b.revert(first))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(!s.host.project().devices.contains_key(&tag));
    assert_eq!(a.project.as_ref(), Some(s.host.project()));
    assert_eq!(
        s.log.0.borrow().last().map(|c| (c.message.clone(), c.author.clone())),
        Some(("Undo: Change device -K1".into(), "Kari <kari@example.com>".into()))
    );
    assert!(a.history.iter().any(|c| c.id == first && c.undone_by.is_some()));
    // Ola has nothing left to undo: the change was taken back already.
    assert!(!a.can_undo());

    // Taking back a change whose parts were changed again since is refused.
    a_link.send(a.edit_project(ProjectEdit::SetDevice {
        tag: tag.clone(),
        device: Some(device("v1")),
        before: None,
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    let v1 = a.history.last().map(|c| c.id).ok_or("not in the list")?;
    b_link.send(b.edit_project(ProjectEdit::SetDevice {
        tag: tag.clone(),
        device: Some(device("v2")),
        before: None,
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    a_link.send(a.revert(v1))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(matches!(a.refusals.last(), Some(Refusal::Invalid { .. })));
    assert_eq!(
        s.host.project().devices.get(&tag).map(|d| d.description.as_str()),
        Some("v2")
    );
    Ok(())
}

#[test]
fn undoing_a_removed_page_brings_it_back_with_its_comments_unless_someone_holds_a_page() -> TestResult {
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    let (_, b_link, mut b) = s.join("Kari")?;
    let comment = Comment {
        id: "c1".into(),
        page: "p0003".into(),
        at: Some(Point::new(1.0, 2.0)),
        author: "Ola".into(),
        time: 0,
        text: "check".into(),
        resolved: false,
        replies: Vec::new(),
    };
    a_link.send(a.edit_project(ProjectEdit::SetComments {
        page: "p0003".into(),
        comments: vec![comment],
        before: None,
    }))?;
    a_link.send(a.edit_project(ProjectEdit::RemovePage { id: "p0003".into() }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(s.host.project().page("p0003").is_none() && s.host.project().comments.is_empty());

    a_link.send(a.undo())?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(s.host.project().page("p0003").is_some());
    assert_eq!(s.host.project().comments.len(), 1);
    assert_eq!(b.project.as_ref(), Some(s.host.project()));

    // An undo that would change a page someone else has open is refused, and nothing changes.
    a_link.send(a.edit_project(ProjectEdit::Renumber {
        numbers: vec![("p0003".into(), "9".into())],
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    b_link.send(b.open(Some("p0003")))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    let before = s.host.project().clone();
    a_link.send(a.undo())?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(matches!(a.refusals.last(), Some(Refusal::PageLocked { .. })));
    assert!(!a.taking_back(), "a refusal answers the undo too");
    assert_eq!(s.host.project(), &before);

    // A restore sent as a plain edit is refused: only undo makes one, after its checks.
    let mut parts = Parts::default();
    parts.pages.insert("p0001".into(), None);
    a_link.send(a.edit_project(ProjectEdit::Restore { parts: Box::new(parts) }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(matches!(a.refusals.last(), Some(Refusal::Invalid { .. })));
    assert!(s.host.project().page("p0001").is_some());

    // The host's own undo works the same way.
    s.host.edit_project(ProjectEdit::AddPage {
        page: Page::new("p0009", "9.1", "Spare"),
    })?;
    s.host.undo()?;
    assert!(s.host.project().page("p0009").is_none());
    assert!(s.host.undo().is_err(), "the host has nothing more to undo");
    s.host.redo()?;
    assert!(s.host.project().page("p0009").is_some());
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert_eq!(a.project.as_ref(), Some(s.host.project()));

    // Taking back a new page is refused once someone has commented on it, so their comment is
    // not left on a page that is gone.
    s.host.edit_project(ProjectEdit::AddPage {
        page: Page::new("p0010", "10.1", "Spare"),
    })?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    let note = Comment {
        id: "c2".into(),
        page: "p0010".into(),
        at: None,
        author: "Kari".into(),
        time: 0,
        text: "why".into(),
        resolved: false,
        replies: Vec::new(),
    };
    b_link.send(b.edit_project(ProjectEdit::SetComments {
        page: "p0010".into(),
        comments: vec![note],
        before: None,
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(s.host.undo().is_err());
    assert!(s.host.project().page("p0010").is_some());
    assert!(s.host.project().comments.iter().any(|c| c.id == "c2"));
    Ok(())
}

#[test]
fn a_change_list_entry_is_kept_in_step_with_the_parts() {
    let before = demo();
    let mut after = before.clone();
    after.pages.retain(|p| p.id != "p0002");
    after.meta.name = "Other".into();
    let (was, now) = Parts::delta(&before, &after);
    assert_eq!(was.pages.len(), 1);
    assert!(now.pages.values().all(Option::is_none));
    assert_eq!(Parts::read(&after, &now), now);
    let mut back = after.clone();
    was.apply(&mut back);
    assert_eq!(back, before);
    assert!(Parts::delta(&before, &before).0.is_empty());
}

#[test]
fn only_the_answer_to_a_take_back_ends_the_wait_for_it() {
    let mut g = Guest::new();
    let _ = g.undo();
    // A refusal of an edit sent before the undo arrives first: still waiting.
    g.handle(HostMsg::Refused {
        refusal: Refusal::Invalid {
            reason: "earlier edit".into(),
        },
        take_back: false,
        edit: None,
        put_back: None,
    });
    assert!(g.taking_back());
    g.handle(HostMsg::Refused {
        refusal: Refusal::Invalid {
            reason: "nothing to undo".into(),
        },
        take_back: true,
        edit: None,
        put_back: None,
    });
    assert!(!g.taking_back());
    // Without the flag the message reads as before, as an older host writes it.
    let line = serde_json::to_string(&HostMsg::Refused {
        refusal: Refusal::NotApproved,
        take_back: false,
        edit: None,
        put_back: None,
    })
    .unwrap_or_default();
    assert!(!line.contains("take_back"), "{line}");
    let back: Option<HostMsg> = serde_json::from_str(&line).ok();
    assert!(
        matches!(back, Some(HostMsg::Refused { take_back: false, .. })),
        "{line}"
    );
}

#[test]
fn the_hub_reports_a_dropped_connection_but_not_a_guest_that_left() -> TestResult {
    let mut host = Host::new(
        demo(),
        "Hanna Host",
        "hanna@example.com",
        Box::new(MemoryJournal::default()),
    );
    let mut hub = Hub::new();
    let (gone, gone_end) = pair();
    let gone_id = hub.add(&mut host, gone_end);
    let (leaver, leaver_end) = pair();
    hub.add(&mut host, leaver_end);
    hub.pump(&mut host);
    assert!(hub.take_closed().is_empty(), "both still open");

    leaver.send(GuestMsg::Leave)?;
    drop(gone);
    hub.pump(&mut host);
    assert_eq!(hub.take_closed(), [(gone_id, transport::CloseReason::Ended)]);
    assert!(hub.take_closed().is_empty(), "reported once");
    assert!(hub.is_empty());
    Ok(())
}

#[test]
fn an_in_process_link_reports_an_orderly_end() {
    let (guest_end, host_end) = pair();
    assert_eq!(guest_end.close_reason(), None);
    drop(host_end);
    assert!(guest_end.close_reason().is_none(), "not known until a receive finds it");
    assert!(guest_end.try_recv().is_err());
    assert_eq!(guest_end.close_reason(), Some(transport::CloseReason::Ended));
}

fn symbol(id: &str, name: &str) -> wc_core::Symbol {
    wc_core::Symbol {
        id: id.into(),
        name: name.into(),
        category: String::new(),
        letter_code: String::new(),
        role: Default::default(),
        graphics: vec![],
        pins: vec![],
        tag_at: Point::default(),
        description: String::new(),
    }
}

#[test]
fn guests_get_the_symbol_files_and_symbols_added_during_the_session() -> TestResult {
    let mut s = Session::new();
    let coil = symbol("coil", "Coil");
    s.host
        .share_files([(coil.id.clone(), coil.clone())].into(), Default::default());
    // A guest that joins gets the host's files with the project.
    let (_a, link_a, mut a) = s.join("Ola")?;
    assert_eq!(a.symbols.get("coil"), Some(&coil));
    let (_b, link_b, mut b) = s.join("Kari")?;

    // Ola uses a symbol and a part the project did not have: everyone gets the files and the
    // lock entries, and the host records it as one change.
    let lamp = symbol("lamp", "Lamp");
    let part = wc_core::Part {
        id: "acme.lamp".into(),
        manufacturer: "Acme".into(),
        ..Default::default()
    };
    link_a.send(a.edit_project(ProjectEdit::AddItems {
        symbols: vec![(lamp.clone(), "starter".into())],
        parts: vec![(part.clone(), "acme".into())],
    }))?;
    s.pump(&mut [(&link_a, &mut a), (&link_b, &mut b)]);
    let entry = s.host.project().lock.symbols.get("lamp").cloned();
    assert_eq!(
        entry,
        Some(wc_core::lock::LockEntry {
            source: "starter".into(),
            hash: lamp.hash()
        })
    );
    assert_eq!(
        s.host.project().lock.parts.get("acme.lamp").map(|e| e.hash.clone()),
        Some(part.hash())
    );
    assert_eq!(s.host.files().0.get("lamp"), Some(&lamp));
    assert_eq!(
        s.log.0.borrow().last().map(|c| c.message.clone()),
        Some("Add lamp, acme.lamp to the project".into())
    );
    for g in [&a, &b] {
        assert_eq!(g.symbols.get("lamp"), Some(&lamp));
        assert_eq!(g.parts.get("acme.lamp"), Some(&part));
        assert_eq!(g.project.as_ref().map(|p| &p.lock), Some(&s.host.project().lock));
    }
    // Someone who joins later gets them in the welcome.
    let (_c, _link_c, c) = s.join("Per")?;
    assert_eq!(c.symbols.len(), 2);
    assert_eq!(c.parts.get("acme.lamp"), Some(&part));

    // Another version of a symbol the project has is refused, and so is an empty list.
    let changes = s.log.0.borrow().len();
    for edit in [
        ProjectEdit::AddItems {
            symbols: vec![(symbol("lamp", "Other lamp"), "starter".into())],
            parts: vec![],
        },
        ProjectEdit::AddItems {
            symbols: vec![],
            parts: vec![],
        },
    ] {
        link_b.send(b.edit_project(edit))?;
    }
    s.pump(&mut [(&link_a, &mut a), (&link_b, &mut b)]);
    assert_eq!(b.refusals.len(), 2, "{:?}", b.refusals);
    assert!(
        b.refusals[0].to_string().contains("another version"),
        "{}",
        b.refusals[0]
    );
    assert_eq!(s.log.0.borrow().len(), changes);
    assert_eq!(b.symbols.get("lamp"), Some(&lamp));
    Ok(())
}

#[test]
fn undoing_an_added_item_takes_back_its_lock_entry() -> TestResult {
    let mut s = Session::new();
    let (_a, link_a, mut a) = s.join("Ola")?;
    let (_b, link_b, mut b) = s.join("Kari")?;
    let coil = symbol("coil", "Coil");
    let part = wc_core::Part {
        id: "acme.coil".into(),
        ..Default::default()
    };
    link_a.send(a.edit_project(ProjectEdit::AddItems {
        symbols: vec![(coil.clone(), "starter".into())],
        parts: vec![(part.clone(), "acme".into())],
    }))?;
    s.pump(&mut [(&link_a, &mut a), (&link_b, &mut b)]);
    assert!(s.host.project().lock.symbols.contains_key("coil"));

    // Undo takes the lock entries back on the host and on every copy.
    link_a.send(GuestMsg::Undo)?;
    s.pump(&mut [(&link_a, &mut a), (&link_b, &mut b)]);
    assert!(a.refusals.is_empty(), "{:?}", a.refusals);
    for lock in [
        Some(&s.host.project().lock),
        a.project.as_ref().map(|p| &p.lock),
        b.project.as_ref().map(|p| &p.lock),
    ] {
        let lock = lock.ok_or("project")?;
        assert!(!lock.symbols.contains_key("coil"));
        assert!(!lock.parts.contains_key("acme.coil"));
    }
    // The file stays, so a redo can lock it again.
    assert_eq!(s.host.files().0.get("coil"), Some(&coil));

    // Redo brings them back everywhere.
    link_a.send(GuestMsg::Redo)?;
    s.pump(&mut [(&link_a, &mut a), (&link_b, &mut b)]);
    assert!(a.refusals.is_empty(), "{:?}", a.refusals);
    assert_eq!(
        s.host.project().lock.symbols.get("coil").map(|e| e.hash.clone()),
        Some(coil.hash())
    );
    assert!(s.host.project().lock.parts.contains_key("acme.coil"));
    for g in [&a, &b] {
        assert_eq!(g.project.as_ref().map(|p| &p.lock), Some(&s.host.project().lock));
    }

    // Once a page places the symbol, taking its lock entry back is refused: the undo of the
    // added item (reached through "Undo this change") would leave the page without it.
    let added = s
        .host
        .history()
        .iter()
        .rev()
        .find(|c| c.message.starts_with("Redo"))
        .map(|c| c.id)
        .ok_or("redo")?;
    link_b.send(b.open(Some("p0002")))?;
    s.pump(&mut [(&link_a, &mut a), (&link_b, &mut b)]);
    let mut page = b
        .project
        .as_ref()
        .and_then(|p| p.page("p0002"))
        .cloned()
        .ok_or("page")?;
    place(&mut page, "s1", 10.0);
    link_b.send(b.edit_page(page).ok_or("lock")?)?;
    s.pump(&mut [(&link_a, &mut a), (&link_b, &mut b)]);
    let changes = s.log.0.borrow().len();
    link_a.send(GuestMsg::Revert { change: added })?;
    s.pump(&mut [(&link_a, &mut a), (&link_b, &mut b)]);
    assert_eq!(s.log.0.borrow().len(), changes);
    let refused = a.refusals.last().map(ToString::to_string).unwrap_or_default();
    assert!(refused.contains("would use symbol coil"), "{refused}");
    assert!(s.host.project().lock.symbols.contains_key("coil"));
    Ok(())
}

#[test]
fn a_redo_is_refused_once_the_session_has_another_version_of_the_item() -> TestResult {
    let mut s = Session::new();
    let (_a, link_a, mut a) = s.join("Ola")?;
    let (_b, link_b, mut b) = s.join("Kari")?;
    let add = |name: &str| ProjectEdit::AddItems {
        symbols: vec![(symbol("coil", name), "starter".into())],
        parts: vec![],
    };
    // Ola adds coil and undoes it; Kari then uses coil in another version and undoes that.
    link_a.send(a.edit_project(add("Coil")))?;
    link_a.send(GuestMsg::Undo)?;
    s.pump(&mut [(&link_a, &mut a), (&link_b, &mut b)]);
    link_b.send(b.edit_project(add("Other coil")))?;
    link_b.send(GuestMsg::Undo)?;
    s.pump(&mut [(&link_a, &mut a), (&link_b, &mut b)]);
    assert!(b.refusals.is_empty(), "{:?}", b.refusals);
    assert_eq!(s.host.files().0.get("coil"), Some(&symbol("coil", "Other coil")));
    // Ola's redo would lock the first version, whose file the session no longer has.
    let changes = s.log.0.borrow().len();
    link_a.send(GuestMsg::Redo)?;
    s.pump(&mut [(&link_a, &mut a), (&link_b, &mut b)]);
    let refused = a.refusals.last().map(ToString::to_string).unwrap_or_default();
    assert!(
        refused.contains("no longer has that version of symbol coil"),
        "{refused}"
    );
    assert_eq!(s.log.0.borrow().len(), changes);
    assert!(!s.host.project().lock.symbols.contains_key("coil"));
    Ok(())
}

#[test]
fn bringing_back_a_page_that_uses_a_symbol_taken_back_since_is_refused() -> TestResult {
    let mut s = Session::new();
    s.host.edit_project(ProjectEdit::AddItems {
        symbols: vec![(symbol("coil", "Coil"), "starter".into())],
        parts: vec![],
    })?;
    let added = s.host.history().last().map(|c| c.id).ok_or("added")?;
    s.host.open(Some("p0003"))?;
    let mut page = s.host.project().page("p0003").cloned().ok_or("page")?;
    place(&mut page, "s1", 10.0);
    s.host.edit_page(page)?;
    s.host.open(None)?;
    s.host.edit_project(ProjectEdit::RemovePage { id: "p0003".into() })?;
    let removed = s.host.history().last().map(|c| c.id).ok_or("removed")?;
    // With the page gone nothing uses the coil, so its adding can be taken back.
    s.host.revert(added)?;
    assert!(!s.host.project().lock.symbols.contains_key("coil"));
    // Bringing the page back would place a symbol the project no longer locks.
    let changes = s.log.0.borrow().len();
    let refused = s.host.revert(removed);
    assert!(
        matches!(&refused, Err(Refusal::Invalid { reason }) if reason.contains("would use symbol coil")),
        "{refused:?}"
    );
    assert!(s.host.project().page("p0003").is_none());
    assert_eq!(s.log.0.borrow().len(), changes);
    Ok(())
}

#[test]
fn a_redo_writes_the_item_file_as_the_session_has_it() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    let project = store::create(dir, "Demo")?;
    let journal = GitJournal::open(dir)?;
    journal.git().commit("Start")?;
    let mut host = Host::new(project, "Hanna Host", "hanna@example.com", Box::new(journal));
    let lamp = symbol("lamp", "Lamp");
    host.edit_project(ProjectEdit::AddItems {
        symbols: vec![(lamp.clone(), "starter".into())],
        parts: vec![],
    })?;
    host.undo()?;
    // A change that was not recorded left another version of the file behind.
    store::write_vendored_symbol(dir, &symbol("lamp", "Other lamp"))?;
    host.redo()?;
    assert_eq!(store::read_vendored_symbol(dir, "lamp")?, lamp);
    assert_eq!(
        store::load(dir)?.lock.symbols.get("lamp").map(|e| e.hash.clone()),
        Some(lamp.hash())
    );
    Ok(())
}

#[test]
fn added_symbols_are_committed_with_their_files() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    let project = store::create(dir, "Demo")?;
    let journal = GitJournal::open(dir)?;
    let git = journal.git().clone();
    git.commit("Start")?;
    let mut host = Host::new(project, "Hanna Host", "hanna@example.com", Box::new(journal));
    let lamp = symbol("lamp", "Lamp");
    host.edit_project(ProjectEdit::AddItems {
        symbols: vec![(lamp.clone(), "starter".into())],
        parts: vec![],
    })?;
    assert_eq!(store::read_vendored_symbol(dir, "lamp")?, lamp);
    let on_disk = store::load(dir)?;
    assert_eq!(
        on_disk.lock.symbols.get("lamp").map(|e| e.hash.clone()),
        Some(lamp.hash())
    );
    let path = store::vendor_path(dir, wc_core::lock::ItemKind::Symbol, "lamp")?;
    let rel = path.strip_prefix(dir)?.to_string_lossy().replace('\\', "/");
    assert!(git.show("HEAD", &rel).is_some(), "the file is in the change's commit");
    assert_eq!(git.author_and_subject("HEAD")?.1, "Add lamp to the project");
    Ok(())
}

#[test]
fn a_whole_record_edit_from_a_copy_that_missed_a_change_is_refused() -> TestResult {
    use wc_core::panel::Panel;
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    let (_, b_link, mut b) = s.join("Kari")?;
    let tag = wc_core::DeviceTag::parse("-K1")?;
    let device = |text: &str| Device {
        description: text.into(),
        ..Device::default()
    };
    let comment = |text: &str| Comment {
        id: "c1".into(),
        page: "p0002".into(),
        at: None,
        author: "Ola".into(),
        time: 0,
        text: text.into(),
        resolved: false,
        replies: Vec::new(),
    };
    let panel = |name: &str| Panel {
        id: "cab1".into(),
        name: name.into(),
        width: 600.0,
        height: 800.0,
        items: Vec::new(),
    };
    let meta = |title: &str| {
        let mut m = s.host.project().meta.clone();
        m.title_block.insert("title".into(), title.into());
        Box::new(m)
    };
    // The same moment: both copies are as the host's project is now.
    let seen = s.host.project().clone();
    let kinds = [
        (
            ProjectEdit::SetDevice {
                tag: tag.clone(),
                device: Some(device("Ola's relay")),
                before: None,
            },
            ProjectEdit::SetDevice {
                tag: tag.clone(),
                device: Some(device("Kari's relay")),
                before: None,
            },
            "device -K1",
        ),
        (
            ProjectEdit::SetComments {
                page: "p0002".into(),
                comments: vec![comment("Ola's note")],
                before: None,
            },
            ProjectEdit::SetComments {
                page: "p0002".into(),
                comments: vec![comment("Kari's note")],
                before: None,
            },
            "the comments on page 2",
        ),
        (
            ProjectEdit::SetPanel {
                panel: panel("Ola's cabinet"),
                before: None,
            },
            ProjectEdit::SetPanel {
                panel: panel("Kari's cabinet"),
                before: None,
            },
            "panel cab1",
        ),
        (
            ProjectEdit::SetMeta {
                meta: meta("Ola's title"),
                before: None,
            },
            ProjectEdit::SetMeta {
                meta: meta("Kari's title"),
                before: None,
            },
            "the project properties",
        ),
    ];
    for (first, second, what) in kinds {
        // Both are sent before either hears of the other's.
        a_link.send(a.edit_project(first.clone().stamped(&seen)))?;
        b_link.send(b.edit_project(second.clone().stamped(&seen)))?;
        s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
        assert!(
            matches!(b.refusals.last(), Some(Refusal::Invalid { reason }) if reason.contains(what) && reason.contains("changed by someone else")),
            "{what}: {:?}",
            b.refusals.last()
        );
        let mut kept = seen.clone();
        apply_edit(&mut kept, &first);
        assert_eq!(
            first.record(s.host.project()),
            first.record(&kept),
            "{what}: the first change is kept"
        );
        assert_eq!(
            b.project.as_ref(),
            Some(s.host.project()),
            "{what}: Kari's copy is put right"
        );
        assert_eq!(a.project.as_ref(), Some(s.host.project()));
        assert!(a.refusals.is_empty(), "{what}: Ola's change went in");
    }
    let refused = b.refusals.len();

    // Made from the copy as it is now, Kari's change goes in.
    let now = b.project.clone().ok_or("no copy")?;
    b_link.send(
        b.edit_project(
            ProjectEdit::SetDevice {
                tag: tag.clone(),
                device: Some(device("Kari's relay")),
                before: None,
            }
            .stamped(&now),
        ),
    )?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert_eq!(b.refusals.len(), refused);
    assert_eq!(
        s.host.project().devices.get(&tag).map(|d| d.description.as_str()),
        Some("Kari's relay")
    );
    // An edit that does not say what it was made from is not checked.
    a_link.send(a.edit_project(ProjectEdit::SetDevice {
        tag: tag.clone(),
        device: Some(device("Ola again")),
        before: None,
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(a.refusals.is_empty());
    assert_eq!(b.project.as_ref(), Some(s.host.project()));
    Ok(())
}

#[test]
fn what_a_record_was_survives_the_wire_and_old_messages_still_read() -> TestResult {
    let mut p = demo();
    let edit = ProjectEdit::SetMeta {
        meta: Box::new(p.meta.clone()),
        before: None,
    }
    .stamped(&p);
    let line = serde_json::to_string(&edit)?;
    assert!(line.contains("\"before\""));
    assert_eq!(serde_json::from_str::<ProjectEdit>(&line)?, edit);
    // Without it (an older copy of the app): read as not checked.
    let old = r#"{"type":"set_panel","panel":{"id":"cab1","name":"A","width":1.0,"height":1.0,"items":[]}}"#;
    assert!(matches!(
        serde_json::from_str::<ProjectEdit>(old)?,
        ProjectEdit::SetPanel { before: None, .. }
    ));
    // The order comments are kept in does not count; their content does.
    let c = |id: &str| Comment {
        id: id.into(),
        page: "p0001".into(),
        at: None,
        author: "A".into(),
        time: 0,
        text: id.into(),
        resolved: false,
        replies: Vec::new(),
    };
    p.comments = vec![c("c1"), c("c2")];
    let edit = ProjectEdit::SetComments {
        page: "p0001".into(),
        comments: Vec::new(),
        before: None,
    };
    let was = edit.record(&p);
    p.comments.reverse();
    assert_eq!(edit.record(&p), was);
    p.comments[0].text = "changed".into();
    assert_ne!(edit.record(&p), was);
    Ok(())
}

#[test]
fn a_batch_is_one_change_applied_all_or_nothing() -> TestResult {
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    let (_, b_link, mut b) = s.join("Kari")?;
    let old = wc_core::DeviceTag::parse("-K1")?;
    let new = wc_core::DeviceTag::parse("-K2")?;
    // -K1 is used on page 2.
    let mut page = s.host.project().page("p0002").ok_or("no page")?.clone();
    place(&mut page, "s1", 10.0);
    page.symbols[0].tag = Some(old.clone());
    let seen = s.host.project().clone();
    a_link.send(a.edit_project(ProjectEdit::Batch {
        edits: vec![
            ProjectEdit::SetDevice {
                tag: old.clone(),
                device: Some(Device::default()),
                before: None,
            }
            .stamped(&seen),
            ProjectEdit::SetPages {
                pages: vec![page.clone()],
                before: vec![seen.page("p0002").ok_or("no page")?.clone()],
            },
        ],
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(a.refusals.is_empty(), "{:?}", a.refusals);
    let messages: Vec<String> = s.log.0.borrow().iter().map(|c| c.message.clone()).collect();
    assert_eq!(
        messages.last().map(String::as_str),
        Some("Change device -K1; Edit page 2 Motor")
    );
    let commits = messages.len();

    // Kari makes -K2 first; then Ola renames -K1 to -K2 from a copy that has not seen it: the
    // new device is refused, so the old one stays and so does the page.
    let renamed_from = s.host.project().clone();
    b_link.send(
        b.edit_project(
            ProjectEdit::SetDevice {
                tag: new.clone(),
                device: Some(Device {
                    description: "Kari's".into(),
                    ..Device::default()
                }),
                before: None,
            }
            .stamped(&renamed_from),
        ),
    )?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    let after_kari = s.host.project().clone();
    let mut renamed = page.clone();
    renamed.symbols[0].tag = Some(new.clone());
    a_link.send(a.edit_project(ProjectEdit::Batch {
        edits: vec![
            ProjectEdit::SetDevice {
                tag: old.clone(),
                device: None,
                before: None,
            }
            .stamped(&renamed_from),
            ProjectEdit::SetDevice {
                tag: new.clone(),
                device: Some(Device::default()),
                before: None,
            }
            .stamped(&renamed_from),
            ProjectEdit::SetPages {
                pages: vec![renamed],
                before: vec![page.clone()],
            },
        ],
    }))?;
    // Applied at once in Ola's copy: the device part (the pages wait for the host).
    assert!(a.project.as_ref().is_some_and(|p| !p.devices.contains_key(&old)));
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(
        matches!(a.refusals.last(), Some(Refusal::Invalid { reason }) if reason.contains("device -K2")),
        "{:?}",
        a.refusals
    );
    assert_eq!(s.host.project(), &after_kari, "nothing of the rename was applied");
    assert_eq!(s.log.0.borrow().len(), commits + 1, "only Kari's change was recorded");
    assert_eq!(a.project.as_ref(), Some(s.host.project()), "Ola's copy is put right");
    assert_eq!(b.project.as_ref(), Some(s.host.project()));

    // A batch holds two batchable edits or more. One that does not is refused whole and not
    // applied in Ola's copy either (an added page would stay there, as nothing puts it back).
    a_link.send(a.edit_project(ProjectEdit::Batch {
        edits: vec![
            ProjectEdit::AddPage {
                page: Page::new("p0009", "9", "New"),
            },
            ProjectEdit::RemovePanel { id: "cab1".into() },
        ],
    }))?;
    assert!(a.project.as_ref().is_some_and(|p| p.page("p0009").is_none()));
    a_link.send(a.edit_project(ProjectEdit::Batch {
        edits: vec![ProjectEdit::RemovePanel { id: "cab1".into() }],
    }))?;
    s.pump(&mut [(&a_link, &mut a), (&b_link, &mut b)]);
    assert!(matches!(
        &a.refusals[a.refusals.len() - 2..],
        [Refusal::Invalid { .. }, Refusal::Invalid { .. }]
    ));
    assert_eq!(s.host.project(), &after_kari);
    assert_eq!(a.project.as_ref(), Some(s.host.project()));
    Ok(())
}

#[test]
fn each_edit_of_a_batch_is_checked_against_what_the_ones_before_it_did() -> TestResult {
    let mut s = Session::new();
    let (_, a_link, mut a) = s.join("Ola")?;
    let tag = wc_core::DeviceTag::parse("-K1")?;
    let set = |description: &str, seen: &wc_core::Project| {
        ProjectEdit::SetDevice {
            tag: tag.clone(),
            device: Some(Device {
                description: description.into(),
                ..Device::default()
            }),
            before: None,
        }
        .stamped(seen)
    };
    // -K1 described twice in one action: the second edit is stamped from the project as the
    // first leaves it, so it only matches if the batch is checked in order.
    let seen = s.host.project().clone();
    let first = set("Pump", &seen);
    let mut after_first = seen.clone();
    crate::protocol::apply_edit(&mut after_first, &first);
    let second = set("Pump contactor", &after_first);
    a_link.send(a.edit_project(ProjectEdit::Batch {
        edits: vec![first, second],
    }))?;
    s.pump(&mut [(&a_link, &mut a)]);
    assert!(a.refusals.is_empty(), "{:?}", a.refusals);
    assert_eq!(
        s.host.project().devices.get(&tag).map(|d| d.description.as_str()),
        Some("Pump contactor")
    );
    Ok(())
}

#[test]
fn edits_that_can_go_together_are_batched_after_the_others() {
    let device = |t: &str| ProjectEdit::SetDevice {
        tag: wc_core::DeviceTag::parse(t).unwrap_or_default(),
        device: None,
        before: None,
    };
    let add = ProjectEdit::AddPage {
        page: Page::new("p0009", "9", "New"),
    };
    assert_eq!(ProjectEdit::batched(vec![device("-K1")]), [device("-K1")], "one alone");
    assert_eq!(
        ProjectEdit::batched(vec![device("-K1"), add.clone(), device("-K2")]),
        [
            add,
            ProjectEdit::Batch {
                edits: vec![device("-K1"), device("-K2")]
            }
        ]
    );
    assert!(ProjectEdit::batched(Vec::new()).is_empty());
}

fn picture(file: &str, data: &[u8]) -> Picture {
    Picture {
        file: file.into(),
        data: data.to_vec(),
    }
}

#[test]
fn guests_get_the_pictures_and_pictures_added_during_the_session() -> TestResult {
    let mut s = Session::new();
    let png = b"\x89PNG\r\n\x1a\nlogo".to_vec();
    let scan = b"\xFF\xD8\xFFscan".to_vec();
    s.host.share_pictures(
        [
            ("pictures/logo.png".to_owned(), png.clone()),
            ("underlays/p0002.jpg".to_owned(), scan.clone()),
            // Not in a picture folder, so not shared.
            ("project.toml".to_owned(), b"x".to_vec()),
        ]
        .into(),
    );
    assert_eq!(s.host.pictures().len(), 2);
    // A guest that joins gets the host's pictures right after the welcome.
    let (_a, link_a, mut a) = s.join("Ola")?;
    assert_eq!(a.pictures.get("pictures/logo.png"), Some(&png));
    assert_eq!(a.pictures.get("underlays/p0002.jpg"), Some(&scan));
    let (_b, link_b, mut b) = s.join("Kari")?;

    // Ola picks a new logo: everyone gets the file, and it is no change of its own.
    let new_logo = picture("pictures/acme.png", b"\x89PNG\r\n\x1a\nacme");
    let changes = s.log.0.borrow().len();
    link_a.send(a.edit_project(ProjectEdit::AddPicture {
        picture: new_logo.clone(),
    }))?;
    s.pump(&mut [(&link_a, &mut a), (&link_b, &mut b)]);
    assert!(a.refusals.is_empty(), "{:?}", a.refusals);
    assert_eq!(a.unanswered_edits(), 0, "the author gets its answer");
    assert_eq!(s.log.0.borrow().len(), changes);
    assert_eq!(s.host.pictures().get("pictures/acme.png"), Some(&new_logo.data));
    for g in [&a, &b] {
        assert_eq!(g.pictures.get("pictures/acme.png"), Some(&new_logo.data));
    }

    // Another picture under a name the session has, one outside the picture folders, one
    // leading out of the project and an empty one are refused.
    for p in [
        picture("pictures/logo.png", b"\x89PNG\r\n\x1a\nother"),
        picture("vendor/symbols/x.png", b"x"),
        picture("pictures/../../etc/passwd", b"x"),
        picture("underlays/empty.png", b""),
    ] {
        link_b.send(b.edit_project(ProjectEdit::AddPicture { picture: p }))?;
    }
    s.pump(&mut [(&link_a, &mut a), (&link_b, &mut b)]);
    assert_eq!(b.refusals.len(), 4, "{:?}", b.refusals);
    assert!(
        b.refusals[0].to_string().contains("another picture"),
        "{}",
        b.refusals[0]
    );
    assert_eq!(a.pictures.get("pictures/logo.png"), Some(&png));
    assert_eq!(s.host.pictures().len(), 3);

    // Someone who joins later gets them all.
    let (_c, _link_c, c) = s.join("Per")?;
    assert_eq!(c.pictures.len(), 3);
    Ok(())
}

#[test]
fn a_picture_survives_a_json_line() -> TestResult {
    let data: Vec<u8> = (0..=255).collect();
    let msg = HostMsg::Picture {
        picture: picture("underlays/p0001.png", &data),
    };
    let line = encode(&msg)?;
    assert!(line.contains("\"data\":\""), "bytes travel as text: {line}");
    assert_eq!(decode::<HostMsg>(&line)?, msg);
    Ok(())
}

#[test]
fn an_added_picture_is_committed_with_the_change_that_names_it() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    let project = store::create(dir, "Demo")?;
    let journal = GitJournal::open(dir)?;
    let git = journal.git().clone();
    git.commit("Start")?;
    let mut host = Host::new(project, "Hanna Host", "hanna@example.com", Box::new(journal));
    let logo = picture("pictures/logo.png", b"\x89PNG\r\n\x1a\nlogo");
    host.edit_project(ProjectEdit::AddPicture { picture: logo.clone() })?;
    assert_eq!(std::fs::read(dir.join("pictures/logo.png"))?, logo.data);
    let mut meta = host.project().meta.clone();
    meta.logo = Some(wc_core::Logo {
        file: logo.file.clone(),
        cover: None,
    });
    host.edit_project(ProjectEdit::SetMeta {
        meta: Box::new(meta),
        before: None,
    })?;
    assert!(
        git.show("HEAD", "pictures/logo.png").is_some(),
        "the file is in the change's commit"
    );

    // A file already in the folder under that name with other content is kept: the picture is
    // refused.
    std::fs::write(dir.join("pictures/old.png"), b"old")?;
    let refused = host.edit_project(ProjectEdit::AddPicture {
        picture: picture("pictures/old.png", b"new"),
    });
    assert!(
        matches!(&refused, Err(Refusal::NotRecorded { reason }) if reason.contains("another picture")),
        "{refused:?}"
    );
    assert_eq!(std::fs::read(dir.join("pictures/old.png"))?, b"old");
    assert!(!host.pictures().contains_key("pictures/old.png"));
    Ok(())
}

#[test]
fn a_session_takes_only_so_many_new_pictures() -> TestResult {
    let mut s = Session::new();
    for n in 0..MAX_ADDED_PICTURES {
        s.host.edit_project(ProjectEdit::AddPicture {
            picture: picture(&format!("pictures/p{n}.png"), b"x"),
        })?;
    }
    let refused = s.host.edit_project(ProjectEdit::AddPicture {
        picture: picture("pictures/one-more.png", b"x"),
    });
    assert!(
        matches!(&refused, Err(Refusal::Invalid { reason }) if reason.contains("at most")),
        "{refused:?}"
    );
    assert_eq!(s.host.pictures().len(), MAX_ADDED_PICTURES);
    // One the session has already is still fine.
    s.host.edit_project(ProjectEdit::AddPicture {
        picture: picture("pictures/p0.png", b"x"),
    })?;
    Ok(())
}

#[test]
fn a_session_takes_only_so_many_bytes_of_new_pictures() -> TestResult {
    let mut s = Session::new();
    let mb = vec![7u8; 1024 * 1024];
    let fit = MAX_ADDED_PICTURE_BYTES / mb.len();
    assert!(fit < MAX_ADDED_PICTURES, "the byte limit is reached first");
    for n in 0..fit {
        s.host.edit_project(ProjectEdit::AddPicture {
            picture: picture(&format!("pictures/p{n}.png"), &mb),
        })?;
    }
    let (pictures, recorded) = (s.host.pictures().clone(), s.log.1.borrow().clone());
    let refused = s.host.edit_project(ProjectEdit::AddPicture {
        picture: picture("pictures/one-more.png", b"x"),
    });
    assert!(
        matches!(&refused, Err(Refusal::Invalid { reason }) if reason.contains("at most")),
        "{refused:?}"
    );
    assert_eq!(s.host.pictures(), &pictures, "the session's pictures are unchanged");
    assert_eq!(*s.log.1.borrow(), recorded, "nothing more was written");
    assert_eq!(recorded.len(), fit);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_picture_folder_that_links_out_of_the_project_makes_nothing_there() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("project");
    let elsewhere = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&dir)?;
    std::fs::create_dir_all(&elsewhere)?;
    std::os::unix::fs::symlink(&elsewhere, dir.join("underlays"))?;
    let r = write_picture(&dir, &picture("underlays/deep/p1.png", b"x"));
    assert!(r.is_err());
    assert!(!elsewhere.join("deep").exists(), "no folder made outside the project");
    Ok(())
}

#[test]
fn a_picture_is_put_in_place_whole_and_leaves_no_part_file() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    let pictures = dir.join("pictures");
    std::fs::create_dir_all(&pictures)?;
    // A file named like a part file that this write did not make is neither in the way nor
    // touched.
    let other = pictures.join(format!(".wc-{}-0-0.part", std::process::id()));
    std::fs::write(&other, b"someone else's")?;
    let logo = picture("pictures/logo.png", b"\x89PNG\r\n\x1a\nlogo");
    write_picture(dir, &logo)?;
    assert_eq!(std::fs::read(pictures.join("logo.png"))?, logo.data);
    assert_eq!(std::fs::read(&other)?, b"someone else's");
    let mut names: Vec<_> = std::fs::read_dir(&pictures)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<std::io::Result<_>>()?;
    names.sort();
    assert_eq!(
        names,
        [other.file_name().ok_or("no name")?.to_owned(), "logo.png".into()],
        "no part file of its own is left"
    );
    // The same picture again is fine; another under that name is refused and changes nothing.
    write_picture(dir, &logo)?;
    assert!(write_picture(dir, &picture("pictures/logo.png", b"other")).is_err());
    assert_eq!(std::fs::read(pictures.join("logo.png"))?, logo.data);
    Ok(())
}

#[test]
fn a_picture_with_a_long_name_that_fits_the_folder_is_written() -> TestResult {
    let tmp = tempfile::tempdir()?;
    // 240 bytes: within the 255 most file systems allow, too long for a part file named after it.
    let file = format!("pictures/{}.png", "a".repeat(236));
    let logo = picture(&file, b"\x89PNG\r\n\x1a\nlogo");
    write_picture(tmp.path(), &logo)?;
    assert_eq!(std::fs::read(tmp.path().join(&file))?, logo.data);
    Ok(())
}

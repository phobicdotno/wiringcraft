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
struct Shared(Rc<RefCell<Vec<Change>>>);

impl Journal for Shared {
    fn record(&mut self, _project: &Project, change: &Change) -> Result<()> {
        self.0.borrow_mut().push(change.clone());
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
    }))?;
    b_link.send(b.edit_project(ProjectEdit::SetDevice {
        tag: tag.clone(),
        device: Some(device("Kari's")),
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
        },
        GuestMsg::Open { page: None },
        GuestMsg::EditPage { page },
        GuestMsg::EditProject {
            edit: ProjectEdit::Renumber {
                numbers: vec![("p1".into(), "2".into())],
            },
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
    };
    assert_eq!(decode::<HostMsg>(&encode(&welcome)?)?, welcome);
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
    }))?;
    // A comment id used on another page, and one used twice, are refused.
    b_link.send(b.edit_project(ProjectEdit::SetComments {
        page: "p0003".into(),
        comments: vec![comment("c1", "p0003")],
    }))?;
    b_link.send(b.edit_project(ProjectEdit::SetComments {
        page: "p0001".into(),
        comments: vec![comment("c7", "p0001"), comment("c7", "p0001")],
    }))?;
    a_link.send(a.edit_project(ProjectEdit::SetPanel {
        panel: panel("cab1", "Cabinet"),
    }))?;
    a_link.send(a.edit_project(ProjectEdit::SetPanel {
        panel: panel("cab1", "Main cabinet"),
    }))?;
    a_link.send(a.edit_project(ProjectEdit::SetPanel {
        panel: panel("cab2", "Field box"),
    }))?;
    a_link.send(a.edit_project(ProjectEdit::SetPanel {
        panel: panel("../evil", "Bad"),
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

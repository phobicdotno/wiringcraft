//! The host's side of a session: approval, page locks, applying changes and telling everyone.

use std::collections::BTreeMap;

use wc_core::{Page, Project};

use crate::protocol::check_edit;
use crate::{Change, GuestMsg, HostMsg, Journal, Peer, PeerId, ProjectEdit, Refusal, Snapshot, apply_edit};

/// The host's own participant id.
pub const HOST: PeerId = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Status {
    /// Connected, has not asked to join yet.
    Connected,
    /// Asked to join, waiting for the host.
    Pending,
    Joined,
}

#[derive(Debug, Clone)]
struct Participant {
    name: String,
    email: String,
    status: Status,
    page: Option<String>,
}

impl Participant {
    fn author(&self) -> String {
        format!("{} <{}>", self.name, self.email)
    }
}

/// The authoritative side of a shared session. Feed it what guests send ([`Host::receive`]),
/// what the host's own user does ([`Host::open`], [`Host::edit_page`], [`Host::edit_project`]),
/// and send out what [`Host::take_outbox`] returns.
pub struct Host {
    project: Project,
    peers: BTreeMap<PeerId, Participant>,
    /// Page id to the participant who has it open.
    locks: BTreeMap<String, PeerId>,
    next_id: PeerId,
    outbox: Vec<(PeerId, HostMsg)>,
    journal: Box<dyn Journal>,
    /// Changes that could not be recorded (and so were undone), with the reason.
    journal_errors: Vec<(Change, String)>,
}

/// Keep a name or email usable in a git author line (`Name <email>`).
fn clean(s: &str, fallback: &str, max: usize) -> String {
    let s: String = s
        .chars()
        .filter(|c| !c.is_control() && *c != '<' && *c != '>')
        .take(max)
        .collect();
    let s = s.trim();
    if s.is_empty() {
        fallback.to_owned()
    } else {
        s.to_owned()
    }
}

impl Host {
    pub fn new(project: Project, name: &str, email: &str, journal: Box<dyn Journal>) -> Self {
        let mut peers = BTreeMap::new();
        peers.insert(
            HOST,
            Participant {
                name: clean(name, "Host", 100),
                email: clean(email, "host@localhost", 200),
                status: Status::Joined,
                page: None,
            },
        );
        Self {
            project,
            peers,
            locks: BTreeMap::new(),
            next_id: HOST + 1,
            outbox: Vec::new(),
            journal,
            journal_errors: Vec::new(),
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Page id to the participant who has it open.
    pub fn locks(&self) -> &BTreeMap<String, PeerId> {
        &self.locks
    }

    /// Everyone who has joined, the host first.
    pub fn peers(&self) -> Vec<Peer> {
        self.peers
            .iter()
            .filter(|(_, p)| p.status == Status::Joined)
            .map(|(&id, p)| Peer {
                id,
                name: p.name.clone(),
                page: p.page.clone(),
            })
            .collect()
    }

    /// Join requests waiting for approval: `(id, name, email)`.
    pub fn pending(&self) -> Vec<(PeerId, String, String)> {
        self.peers
            .iter()
            .filter(|(_, p)| p.status == Status::Pending)
            .map(|(&id, p)| (id, p.name.clone(), p.email.clone()))
            .collect()
    }

    /// Messages to send, in order: `(to, message)`.
    pub fn take_outbox(&mut self) -> Vec<(PeerId, HostMsg)> {
        std::mem::take(&mut self.outbox)
    }

    /// Changes that could not be written to the history and were undone, with the reason.
    pub fn take_journal_errors(&mut self) -> Vec<(Change, String)> {
        std::mem::take(&mut self.journal_errors)
    }

    /// A new connection. Returns its id; it may ask to join next.
    pub fn connect(&mut self) -> PeerId {
        let id = self.next_id;
        self.next_id += 1;
        self.peers.insert(
            id,
            Participant {
                name: String::new(),
                email: String::new(),
                status: Status::Connected,
                page: None,
            },
        );
        id
    }

    /// A connection ended (or the guest left): release its locks and tell the others.
    pub fn disconnect(&mut self, id: PeerId) {
        if id == HOST {
            return;
        }
        let Some(p) = self.peers.remove(&id) else { return };
        if p.status == Status::Joined {
            self.locks.retain(|_, holder| *holder != id);
            self.broadcast_presence();
        }
    }

    /// Let a waiting guest in: they get the whole project and the current locks.
    pub fn approve(&mut self, id: PeerId) -> bool {
        match self.peers.get_mut(&id) {
            Some(p) if p.status == Status::Pending => p.status = Status::Joined,
            _ => return false,
        }
        let welcome = HostMsg::Welcome {
            you: id,
            project: Box::new(Snapshot::from(&self.project)),
            locks: self.locks.clone(),
            peers: self.peers(),
        };
        self.outbox.push((id, welcome));
        self.broadcast_presence_except(id);
        true
    }

    /// Turn a waiting guest away.
    pub fn decline(&mut self, id: PeerId) -> bool {
        match self.peers.get(&id) {
            Some(p) if p.status == Status::Pending => {
                self.peers.remove(&id);
                self.outbox.push((id, HostMsg::Declined));
                true
            }
            _ => false,
        }
    }

    /// End the session: every guest is told, and every lock is released.
    pub fn close(&mut self) {
        let ids: Vec<PeerId> = self.peers.keys().copied().filter(|&id| id != HOST).collect();
        for id in ids {
            self.outbox.push((id, HostMsg::Closed));
            self.peers.remove(&id);
        }
        self.locks.clear();
        if let Some(h) = self.peers.get_mut(&HOST) {
            h.page = None;
        }
    }

    /// The host's own user opens a page (or closes it with `None`).
    pub fn open(&mut self, page: Option<&str>) -> Result<(), Refusal> {
        self.do_open(HOST, page.map(str::to_owned))
    }

    /// The host's own user changed the page they have open.
    pub fn edit_page(&mut self, page: Page) -> Result<(), Refusal> {
        self.do_edit_page(HOST, page)
    }

    /// The host's own user changed project-level data.
    pub fn edit_project(&mut self, edit: ProjectEdit) -> Result<(), Refusal> {
        self.do_edit_project(HOST, edit)
    }

    /// Handle one message from a guest. A refusal is also sent back to that guest.
    pub fn receive(&mut self, from: PeerId, msg: GuestMsg) -> Result<(), Refusal> {
        if from == HOST {
            return Err(Refusal::Invalid {
                reason: "the host does not send guest messages".into(),
            });
        }
        let Some(status) = self.peers.get(&from).map(|p| p.status.clone()) else {
            return Err(Refusal::NotApproved);
        };
        let result = match (status, msg) {
            (Status::Connected, GuestMsg::Join { name, email }) => {
                if let Some(p) = self.peers.get_mut(&from) {
                    p.name = clean(&name, "Guest", 100);
                    p.email = clean(&email, "guest@localhost", 200);
                    p.status = Status::Pending;
                }
                self.outbox.push((from, HostMsg::Waiting));
                Ok(())
            }
            (_, GuestMsg::Leave) => {
                self.disconnect(from);
                Ok(())
            }
            (Status::Joined, GuestMsg::Join { .. }) => Ok(()),
            (Status::Joined, GuestMsg::Open { page }) => self.do_open(from, page),
            (Status::Joined, GuestMsg::EditPage { page }) => {
                let id = page.id.clone();
                let result = self.do_edit_page(from, page);
                // The guest already changed its own copy: send the host's page back so the two
                // copies agree again.
                if result.is_err()
                    && let Some(current) = self.project.page(&id)
                {
                    let by = self.locks.get(&id).copied().unwrap_or(HOST);
                    self.outbox.push((
                        from,
                        HostMsg::PageChanged {
                            page: current.clone(),
                            by,
                        },
                    ));
                }
                result
            }
            (Status::Joined, GuestMsg::EditProject { edit }) => self.do_edit_project(from, edit),
            _ => Err(Refusal::NotApproved),
        };
        if let Err(refusal) = &result {
            self.outbox.push((
                from,
                HostMsg::Refused {
                    refusal: refusal.clone(),
                },
            ));
        }
        result
    }

    fn do_open(&mut self, by: PeerId, page: Option<String>) -> Result<(), Refusal> {
        if let Some(id) = &page {
            if self.project.page(id).is_none() {
                return Err(Refusal::UnknownPage { page: id.clone() });
            }
            if let Some(&holder) = self.locks.get(id)
                && holder != by
            {
                return Err(Refusal::PageLocked {
                    page: id.clone(),
                    by: holder,
                });
            }
        }
        self.locks.retain(|_, holder| *holder != by);
        if let Some(id) = &page {
            self.locks.insert(id.clone(), by);
        }
        if let Some(p) = self.peers.get_mut(&by) {
            p.page = page;
        }
        self.broadcast_presence();
        Ok(())
    }

    fn do_edit_page(&mut self, by: PeerId, mut page: Page) -> Result<(), Refusal> {
        if self.locks.get(&page.id) != Some(&by) {
            return Err(match self.project.page(&page.id) {
                None => Refusal::UnknownPage { page: page.id },
                Some(_) => Refusal::NoLock { page: page.id },
            });
        }
        let Some(current) = self.project.page_mut(&page.id) else {
            return Err(Refusal::UnknownPage { page: page.id });
        };
        // The display number is project-level: only `Renumber` changes it.
        page.number = current.number.clone();
        if *current == page {
            return Ok(());
        }
        let before = std::mem::replace(current, page.clone());
        let message = format!("Edit page {} {}", page.number, page.title);
        if let Err(refusal) = self.record(by, Some(page.id.clone()), message) {
            if let Some(p) = self.project.page_mut(&page.id) {
                *p = before;
            }
            return Err(refusal);
        }
        self.send_others(by, HostMsg::PageChanged { page, by });
        Ok(())
    }

    fn do_edit_project(&mut self, by: PeerId, edit: ProjectEdit) -> Result<(), Refusal> {
        check_edit(&self.project, &self.locks, by, &edit)?;
        let before = self.project.clone();
        apply_edit(&mut self.project, &edit);
        if self.project == before {
            return Ok(());
        }
        if let Err(refusal) = self.record(by, edit.page().map(str::to_owned), edit.describe()) {
            self.project = before;
            return Err(refusal);
        }
        if let ProjectEdit::RemovePage { id } = &edit
            && self.locks.remove(id).is_some()
        {
            for p in self.peers.values_mut() {
                if p.page.as_deref() == Some(id.as_str()) {
                    p.page = None;
                }
            }
            self.broadcast_presence();
        }
        let ids: Vec<PeerId> = self.joined_guests().collect();
        for id in ids {
            self.outbox
                .push((id, HostMsg::ProjectChanged { edit: edit.clone(), by }));
        }
        Ok(())
    }

    /// Record an applied change. When that fails the caller undoes the change, so a later
    /// commit never carries it under someone else's name.
    fn record(&mut self, by: PeerId, page: Option<String>, message: String) -> Result<(), Refusal> {
        let author = self.peers.get(&by).map(Participant::author).unwrap_or_default();
        let change = Change {
            by,
            author,
            page,
            message,
        };
        if let Err(e) = self.journal.record(&self.project, &change) {
            let reason = e.to_string();
            self.journal_errors.push((change, reason.clone()));
            return Err(Refusal::NotRecorded { reason });
        }
        Ok(())
    }

    fn joined_guests(&self) -> impl Iterator<Item = PeerId> + '_ {
        self.peers
            .iter()
            .filter(|(id, p)| **id != HOST && p.status == Status::Joined)
            .map(|(&id, _)| id)
    }

    fn send_others(&mut self, except: PeerId, msg: HostMsg) {
        let ids: Vec<PeerId> = self.joined_guests().filter(|&id| id != except).collect();
        for id in ids {
            self.outbox.push((id, msg.clone()));
        }
    }

    fn broadcast_presence(&mut self) {
        self.broadcast_presence_except(HOST);
    }

    fn broadcast_presence_except(&mut self, except: PeerId) {
        let locks = HostMsg::Locks {
            locks: self.locks.clone(),
        };
        let peers = HostMsg::Peers { peers: self.peers() };
        self.send_others(except, locks);
        self.send_others(except, peers);
    }
}

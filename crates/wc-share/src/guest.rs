//! A guest's side of a session: its copy of the project, kept in step with the host.

use std::collections::BTreeMap;

use wc_core::{Page, Project};

use crate::{GuestMsg, HostMsg, Peer, PeerId, ProjectEdit, Refusal, apply_edit};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuestStatus {
    /// Not asked to join yet.
    Idle,
    /// Asked; the host has not decided.
    Waiting,
    Joined,
    Declined,
    /// The host ended the session.
    Closed,
}

/// A guest's view of a session. Turn its user's actions into messages with [`Guest::open`],
/// [`Guest::edit_page`] and [`Guest::edit_project`], send them to the host, and feed what the
/// host sends to [`Guest::handle`].
#[derive(Debug)]
pub struct Guest {
    pub status: GuestStatus,
    pub me: Option<PeerId>,
    pub project: Option<Project>,
    pub locks: BTreeMap<String, PeerId>,
    pub peers: Vec<Peer>,
    /// Refusals from the host not yet shown to the user.
    pub refusals: Vec<Refusal>,
}

impl Default for Guest {
    fn default() -> Self {
        Self::new()
    }
}

impl Guest {
    pub fn new() -> Self {
        Self {
            status: GuestStatus::Idle,
            me: None,
            project: None,
            locks: BTreeMap::new(),
            peers: Vec::new(),
            refusals: Vec::new(),
        }
    }

    pub fn join(&mut self, name: &str, email: &str) -> GuestMsg {
        self.status = GuestStatus::Waiting;
        GuestMsg::Join {
            name: name.to_owned(),
            email: email.to_owned(),
        }
    }

    /// Who holds a page, when it is someone else: the page is read-only for this guest.
    pub fn locked_by(&self, page: &str) -> Option<&Peer> {
        let holder = *self.locks.get(page)?;
        if Some(holder) == self.me {
            return None;
        }
        self.peers.iter().find(|p| p.id == holder)
    }

    /// Whether this guest may edit the page now (it has the page open).
    pub fn can_edit(&self, page: &str) -> bool {
        self.me.is_some() && self.locks.get(page) == self.me.as_ref()
    }

    /// Ask to open a page. The page held so far is given up at once, so nothing is edited
    /// under the old lock while the switch is on its way; the new lock is granted when the
    /// host's lock list says so.
    pub fn open(&mut self, page: Option<&str>) -> GuestMsg {
        if let Some(me) = self.me {
            self.locks.retain(|_, holder| *holder != me);
        }
        GuestMsg::Open {
            page: page.map(str::to_owned),
        }
    }

    /// Change the open page: applied to this copy right away, then sent to the host. `None`
    /// when this guest does not hold the page's lock.
    pub fn edit_page(&mut self, page: Page) -> Option<GuestMsg> {
        if !self.can_edit(&page.id) {
            return None;
        }
        let current = self.project.as_mut()?.page_mut(&page.id)?;
        let mut page = page;
        page.number = current.number.clone();
        *current = page.clone();
        Some(GuestMsg::EditPage { page })
    }

    /// Change project-level data. Applied when the host sends it back, so every copy applies
    /// project changes in the same order.
    pub fn edit_project(&self, edit: ProjectEdit) -> GuestMsg {
        GuestMsg::EditProject { edit }
    }

    pub fn handle(&mut self, msg: HostMsg) {
        match msg {
            HostMsg::Waiting => self.status = GuestStatus::Waiting,
            HostMsg::Welcome {
                you,
                project,
                locks,
                peers,
            } => {
                self.status = GuestStatus::Joined;
                self.me = Some(you);
                self.project = Some((*project).into());
                self.locks = locks;
                self.peers = peers;
            }
            HostMsg::Declined => self.status = GuestStatus::Declined,
            HostMsg::Closed => {
                self.status = GuestStatus::Closed;
                self.locks.clear();
            }
            HostMsg::PageChanged { page, .. } => {
                if let Some(p) = self.project.as_mut().and_then(|p| p.page_mut(&page.id)) {
                    *p = page;
                }
            }
            HostMsg::ProjectChanged { edit, .. } => {
                if let Some(p) = self.project.as_mut() {
                    apply_edit(p, &edit);
                }
            }
            HostMsg::Locks { locks } => self.locks = locks,
            HostMsg::Peers { peers } => self.peers = peers,
            HostMsg::Refused { refusal } => self.refusals.push(refusal),
        }
    }
}

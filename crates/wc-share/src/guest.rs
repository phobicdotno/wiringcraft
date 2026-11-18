//! A guest's side of a session: its copy of the project, kept in step with the host.

use std::collections::BTreeMap;

use wc_core::{Page, Project};

use crate::{ChangeInfo, note, redo_target, undo_target};
use crate::{GuestMsg, HostMsg, Peer, PeerId, ProjectEdit, Refusal, apply_edit};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuestStatus {
    /// Not asked to join yet.
    Idle,
    /// Asked; the host has not decided.
    Waiting,
    Joined,
    Declined,
    /// The host and this guest run session protocol versions that cannot work together.
    WrongVersion(crate::VersionMismatch),
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
    /// The session's recorded changes, oldest first.
    pub history: Vec<ChangeInfo>,
    /// Refusals from the host not yet shown to the user.
    pub refusals: Vec<Refusal>,
    /// A page switch the host has not answered yet: the page asked for.
    switching: Option<Option<String>>,
    /// Page edits sent and not answered yet, by page. The host answers each with the page as
    /// it kept it; only the answer to the last one is applied, so an answer to an older edit
    /// never replaces newer work.
    in_flight: BTreeMap<String, usize>,
    /// Undo, redo and revert requests sent and not answered yet. The host answers each with
    /// its restoration (a `Restore` edit by this guest) or a refusal.
    taking_back: usize,
    /// Symbol and part files the session sent (in the welcome, with each
    /// [`ProjectEdit::AddItems`] and [`ProjectEdit::UpdateItems`], and as [`HostMsg::Items`]), by id: what this guest's copy needs to draw the project.
    pub symbols: BTreeMap<String, wc_core::Symbol>,
    pub parts: BTreeMap<String, wc_core::Part>,
    /// Picture files the session sent ([`HostMsg::Picture`]), by project path: what this
    /// guest's copy needs to show the logo and underlays.
    pub pictures: BTreeMap<String, Vec<u8>>,
    /// The number of the last project edit sent.
    last_edit: u64,
    /// Project edits sent and not answered yet, by number, each with the page it adds or
    /// removes, if any.
    unanswered: BTreeMap<u64, Option<String>>,
    /// Pages whose content here comes from an unanswered edit that added or removed them, with
    /// that edit's number: a refusal of it brings the host's page back whole.
    replaced: BTreeMap<String, u64>,
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
            history: Vec::new(),
            refusals: Vec::new(),
            switching: None,
            in_flight: BTreeMap::new(),
            taking_back: 0,
            symbols: BTreeMap::new(),
            parts: BTreeMap::new(),
            pictures: BTreeMap::new(),
            last_edit: 0,
            unanswered: BTreeMap::new(),
            replaced: BTreeMap::new(),
        }
    }

    pub fn join(&mut self, name: &str, email: &str) -> GuestMsg {
        self.status = GuestStatus::Waiting;
        GuestMsg::Join {
            name: name.to_owned(),
            email: email.to_owned(),
            protocol: crate::PROTOCOL_VERSION,
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
        self.me.is_some() && self.switching.is_none() && self.locks.get(page) == self.me.as_ref()
    }

    /// Ask to open a page. The page held so far is given up at once, so nothing is edited
    /// under the old lock while the switch is on its way; the new lock is granted when the
    /// host's lock list says so.
    pub fn open(&mut self, page: Option<&str>) -> GuestMsg {
        if let Some(me) = self.me {
            self.locks.retain(|_, holder| *holder != me);
        }
        // Nothing is editable until the host answers: a lock list already on its way could
        // otherwise hand the old page back.
        self.switching = Some(page.map(str::to_owned));
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
        *self.in_flight.entry(page.id.clone()).or_default() += 1;
        Some(GuestMsg::EditPage { page })
    }

    /// Change project-level data. Most changes are applied to this copy at once, and again when
    /// the host sends them back in its order, which gives the same result, so every copy ends
    /// up the same; when the host refuses one, it sends back what the change touched as the
    /// host has it, and this copy takes that. Whole pages wait for the host (it refuses them
    /// when someone else changed one first, and applying them here would hide that change), as
    /// do symbols and parts (it refuses one the session has in another version, or cannot
    /// store, and this copy must not count it as taken). See [`ProjectEdit::applied_at_once`].
    pub fn edit_project(&mut self, edit: ProjectEdit) -> GuestMsg {
        let target = match &edit {
            ProjectEdit::AddPage { page } => Some(page.id.clone()),
            ProjectEdit::RemovePage { id } => Some(id.clone()),
            _ => None,
        };
        let has = |p: &Option<wc_core::Project>, id: &str| p.as_ref().is_some_and(|p| p.page(id).is_some());
        let before = target.as_deref().map(|id| has(&self.project, id));
        if let Some(p) = self.project.as_mut() {
            edit.apply_at_once(p);
        }
        self.last_edit += 1;
        // Only an edit that really added or removed the page here leaves content that is not
        // the host's page (adding a page that is there already changes nothing).
        let structural = target.filter(|id| before != Some(has(&self.project, id)));
        if let Some(page) = &structural {
            self.replaced.insert(page.clone(), self.last_edit);
        }
        self.unanswered.insert(self.last_edit, structural);
        GuestMsg::EditProject {
            edit,
            seq: self.last_edit,
        }
    }

    /// How many project edits the host has not answered yet.
    pub fn unanswered_edits(&self) -> usize {
        self.unanswered.len()
    }

    /// Whether Undo has a change of this guest's to take back.
    pub fn can_undo(&self) -> bool {
        self.me.is_some_and(|me| undo_target(&self.history, me).is_some())
    }

    /// Whether Redo has an undo of this guest's to take back.
    pub fn can_redo(&self) -> bool {
        self.me.is_some_and(|me| redo_target(&self.history, me).is_some())
    }

    /// Take back this guest's last change. The host puts back only what that change touched
    /// and sends the result to everyone.
    pub fn undo(&mut self) -> GuestMsg {
        self.taking_back += 1;
        GuestMsg::Undo
    }

    pub fn redo(&mut self) -> GuestMsg {
        self.taking_back += 1;
        GuestMsg::Redo
    }

    /// Take back one change from the session's list, also someone else's.
    pub fn revert(&mut self, change: u64) -> GuestMsg {
        self.taking_back += 1;
        GuestMsg::Revert { change }
    }

    /// Whether an undo, redo or revert is on its way and its result has not arrived: a page
    /// built from this copy now would still hold what the host is putting back.
    pub fn taking_back(&self) -> bool {
        self.taking_back > 0
    }

    pub fn handle(&mut self, msg: HostMsg) {
        match msg {
            HostMsg::Waiting => self.status = GuestStatus::Waiting,
            HostMsg::Welcome {
                you,
                project,
                locks,
                peers,
                history,
                protocol,
            } => {
                // A host on another version (one too old to check the guest's) may send
                // messages this guest cannot read: stop here.
                if protocol != crate::PROTOCOL_VERSION {
                    self.status = GuestStatus::WrongVersion(crate::VersionMismatch {
                        host: protocol,
                        guest: crate::PROTOCOL_VERSION,
                    });
                    return;
                }
                self.history = history;
                self.status = GuestStatus::Joined;
                self.me = Some(you);
                let mut project = *project;
                self.symbols = std::mem::take(&mut project.symbols);
                self.parts = std::mem::take(&mut project.parts);
                self.project = Some(project.into());
                self.locks = locks;
                self.peers = peers;
            }
            HostMsg::Declined { version: None } => self.status = GuestStatus::Declined,
            HostMsg::Declined { version: Some(v) } => self.status = GuestStatus::WrongVersion(v),
            HostMsg::Closed => {
                self.status = GuestStatus::Closed;
                self.locks.clear();
                self.taking_back = 0;
                self.unanswered.clear();
                self.replaced.clear();
            }
            HostMsg::PageChanged { page, .. } => {
                if self.answered(&page.id) {
                    return;
                }
                if let Some(p) = self.project.as_mut().and_then(|p| p.page_mut(&page.id)) {
                    *p = page;
                }
            }
            HostMsg::Unchanged { seq } => self.taken(seq),
            HostMsg::ProjectChanged { edit, by, seq } => {
                if let Some(seq) = seq {
                    self.taken(seq);
                }
                if matches!(edit, ProjectEdit::Restore { .. }) && Some(by) == self.me {
                    self.taking_back = self.taking_back.saturating_sub(1);
                }
                match &edit {
                    ProjectEdit::AddItems { symbols, parts } => {
                        self.symbols
                            .extend(symbols.iter().map(|(s, _)| (s.id.clone(), s.clone())));
                        self.parts.extend(parts.iter().map(|(p, _)| (p.id.clone(), p.clone())));
                    }
                    ProjectEdit::UpdateItems { symbols, parts } => {
                        self.symbols
                            .extend(symbols.iter().map(|(s, _, _)| (s.id.clone(), s.clone())));
                        self.parts
                            .extend(parts.iter().map(|(p, _, _)| (p.id.clone(), p.clone())));
                    }
                    _ => {}
                }
                if let Some(p) = self.project.as_mut() {
                    apply_edit(p, &edit);
                }
            }
            HostMsg::Picture { picture } => {
                self.pictures.insert(picture.file, picture.data);
            }
            HostMsg::Items { symbols, parts } => {
                self.symbols.extend(symbols.into_iter().map(|s| (s.id.clone(), s)));
                self.parts.extend(parts.into_iter().map(|p| (p.id.clone(), p)));
            }
            HostMsg::Recorded { change } => note(&mut self.history, change),
            HostMsg::Locks { locks } => {
                if let Some(asked) = &self.switching {
                    let done = match asked {
                        Some(page) => self.me.is_some() && locks.get(page) == self.me.as_ref(),
                        None => !locks.values().any(|h| Some(*h) == self.me),
                    };
                    if done {
                        self.switching = None;
                    }
                }
                self.locks = locks;
            }
            HostMsg::Peers { peers } => self.peers = peers,
            HostMsg::Refused {
                refusal,
                take_back,
                edit,
                put_back,
            } => {
                // The host's page comes back whole only while the content here is still the
                // refused page's replacement; once one refusal put the host's page back, later
                // edits of it are kept.
                let whole = edit
                    .and_then(|seq| self.unanswered.remove(&seq))
                    .flatten()
                    .filter(|page| self.replaced.remove(page).is_some());
                if let Some(parts) = put_back {
                    self.put_back(*parts, whole.as_deref());
                }
                // Only the refusal of an undo, redo or revert answers one: a refusal of an
                // earlier edit, arriving first, must not end the wait for the restoration.
                if take_back {
                    self.taking_back = self.taking_back.saturating_sub(1);
                }
                match &refusal {
                    Refusal::PageLocked { page, .. } if self.switching_to(page) => self.switching = None,
                    Refusal::UnknownPage { page } if self.switching_to(page) => self.switching = None,
                    // An edit of a page that is gone gets no page back.
                    Refusal::UnknownPage { page } => {
                        self.answered(page);
                    }
                    _ => {}
                }
                self.refusals.push(refusal);
            }
        }
    }

    /// Take the host's version of what a refused project edit touched. A page this guest has
    /// open keeps its content (page edits of it may still be on their way), and takes only the
    /// host's display number, unless the refused edit added or removed that page: then the
    /// content here is not the host's page at all, and the host's page comes back whole.
    fn put_back(&mut self, mut parts: crate::Parts, structural: Option<&str>) {
        let me = self.me;
        let Some(project) = self.project.as_mut() else {
            return;
        };
        for (id, host) in parts.pages.iter_mut() {
            let held = me.is_some() && self.locks.get(id) == me.as_ref() && structural != Some(id.as_str());
            if held && let (Some(host), Some(mine)) = (host.as_mut(), project.page(id)) {
                let number = std::mem::take(&mut host.number);
                *host = mine.clone();
                host.number = number;
            }
        }
        parts.apply(project);
    }

    /// The host took (or did not need) project edit `seq`.
    fn taken(&mut self, seq: u64) {
        if let Some(Some(page)) = self.unanswered.remove(&seq)
            && self.replaced.get(&page) == Some(&seq)
        {
            self.replaced.remove(&page);
        }
    }

    fn switching_to(&self, page: &str) -> bool {
        matches!(&self.switching, Some(Some(p)) if p == page)
    }

    /// Count one answer to an edit of `page`; true while newer edits of it are still unanswered,
    /// so this answer must not replace the guest's copy.
    fn answered(&mut self, page: &str) -> bool {
        let Some(n) = self.in_flight.get_mut(page) else {
            return false;
        };
        *n -= 1;
        if *n == 0 {
            self.in_flight.remove(page);
            return false;
        }
        true
    }
}

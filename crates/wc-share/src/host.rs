//! The host's side of a session: approval, page locks, applying changes and telling everyone.

use std::collections::{BTreeMap, BTreeSet};

use wc_core::{Page, Project};

use crate::protocol::check_edit;
use crate::{Change, GuestMsg, HostMsg, Journal, Peer, PeerId, ProjectEdit, Refusal, Snapshot, apply_edit};
use crate::{ChangeInfo, ChangeKind, Parts, redo_target, undo_target};

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
    /// The recorded changes, oldest first, with what each changed, for undo.
    history: Vec<Entry>,
    next_change: u64,
    /// Why guests' Undo, Redo and "Undo this change" are refused for now, if they are: the
    /// host's project folder belongs to something else meanwhile (an assistant command).
    take_backs_held: Option<String>,
    /// The vendored symbol and part files guests get when they join, by id: what the host's
    /// user shared ([`Host::share_files`]) and what participants added since.
    symbols: BTreeMap<String, wc_core::Symbol>,
    parts: BTreeMap<String, wc_core::Part>,
    /// The hash of each file in `symbols` and `parts`, kept as they change so a look at which
    /// files the session lacks does not hash them again.
    file_hashes: BTreeMap<(wc_core::lock::ItemKind, String), String>,
    /// Versions an update ([`ProjectEdit::UpdateItems`]) replaced, by id and hash, so an undo
    /// can lock and store them again.
    old_symbols: BTreeMap<(String, String), wc_core::Symbol>,
    old_parts: BTreeMap<(String, String), wc_core::Part>,
    /// The picture files guests get (the logo, page underlays), by project path: what the
    /// host's user shared ([`Host::share_pictures`]) and what participants added since.
    pictures: BTreeMap<String, Vec<u8>>,
    /// How many pictures participants added during the session, and their bytes, kept within
    /// [`crate::MAX_ADDED_PICTURES`] and [`crate::MAX_ADDED_PICTURE_BYTES`].
    added_pictures: (usize, usize),
}

/// A recorded change and the parts of the project it changed, before and after.
#[derive(Debug, Clone)]
struct Entry {
    info: ChangeInfo,
    before: Parts,
    after: Parts,
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
            history: Vec::new(),
            next_change: 1,
            take_backs_held: None,
            symbols: BTreeMap::new(),
            parts: BTreeMap::new(),
            file_hashes: BTreeMap::new(),
            old_symbols: BTreeMap::new(),
            old_parts: BTreeMap::new(),
            pictures: BTreeMap::new(),
            added_pictures: (0, 0),
        }
    }

    /// The project's vendored symbol and part files, sent to every guest that joins so they can
    /// draw the project without the host's libraries.
    pub fn share_files(&mut self, symbols: BTreeMap<String, wc_core::Symbol>, parts: BTreeMap<String, wc_core::Part>) {
        use wc_core::lock::ItemKind;
        self.file_hashes = symbols
            .values()
            .map(|s| ((ItemKind::Symbol, s.id.clone()), s.hash()))
            .chain(parts.values().map(|p| ((ItemKind::Part, p.id.clone()), p.hash())))
            .collect();
        self.symbols = symbols;
        self.parts = parts;
    }

    /// The items the session locks that it has no file of in that version (a host that shared
    /// the project with a copy missing): kind and id.
    pub fn files_missing(&self) -> Vec<(wc_core::lock::ItemKind, String)> {
        use wc_core::lock::ItemKind;
        [ItemKind::Symbol, ItemKind::Part]
            .into_iter()
            .flat_map(|kind| {
                self.project
                    .lock
                    .map(kind)
                    .iter()
                    .filter(move |(id, e)| self.file_hashes.get(&(kind, (*id).clone())) != Some(&e.hash))
                    .map(move |(id, _)| (kind, id.clone()))
            })
            .collect()
    }

    /// The project's picture files (the logo, page underlays) by project path, sent to every
    /// guest that joins. Files outside the picture folders or too large are left out.
    pub fn share_pictures(&mut self, pictures: BTreeMap<String, Vec<u8>>) {
        self.pictures = pictures
            .into_iter()
            .filter(|(file, data)| {
                crate::Picture::file_ok(file) && !data.is_empty() && data.len() <= crate::MAX_PICTURE
            })
            .collect();
    }

    /// The picture files guests get, by project path.
    pub fn pictures(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.pictures
    }

    /// The symbol and part files guests get, by id.
    pub fn files(&self) -> (&BTreeMap<String, wc_core::Symbol>, &BTreeMap<String, wc_core::Part>) {
        (&self.symbols, &self.parts)
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
        let mut snapshot = Snapshot::from(&self.project);
        snapshot.symbols = self.symbols.clone();
        snapshot.parts = self.parts.clone();
        let welcome = HostMsg::Welcome {
            you: id,
            project: Box::new(snapshot),
            locks: self.locks.clone(),
            peers: self.peers(),
            history: self.history(),
            protocol: crate::PROTOCOL_VERSION,
        };
        self.outbox.push((id, welcome));
        for (file, data) in &self.pictures {
            let picture = crate::Picture {
                file: file.clone(),
                data: data.clone(),
            };
            self.outbox.push((id, HostMsg::Picture { picture }));
        }
        self.broadcast_presence_except(id);
        true
    }

    /// Turn a waiting guest away.
    pub fn decline(&mut self, id: PeerId) -> bool {
        match self.peers.get(&id) {
            Some(p) if p.status == Status::Pending => {
                self.peers.remove(&id);
                self.outbox.push((id, HostMsg::Declined { version: None }));
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
        self.do_edit_project(HOST, edit, None).map(|_| ())
    }

    /// The session's recorded changes, oldest first (at most [`crate::MAX_HISTORY`]).
    pub fn history(&self) -> Vec<ChangeInfo> {
        self.history.iter().map(|e| e.info.clone()).collect()
    }

    /// The host's own user takes back their last change. Only the parts that change touched
    /// are put back, so other people's work stays.
    pub fn undo(&mut self) -> Result<(), Refusal> {
        self.do_undo(HOST)
    }

    /// The host's own user takes back their last undo.
    pub fn redo(&mut self) -> Result<(), Refusal> {
        self.do_redo(HOST)
    }

    /// The host's own user takes back one change from the list, whoever made it.
    pub fn revert(&mut self, change: u64) -> Result<(), Refusal> {
        self.do_revert(HOST, change, ChangeKind::Undo)
    }

    /// Refuse guests' Undo, Redo and "Undo this change" with `reason` while it is set; `None`
    /// takes them again.
    pub fn hold_take_backs(&mut self, reason: Option<String>) {
        self.take_backs_held = reason;
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
        let take_back = matches!(msg, GuestMsg::Undo | GuestMsg::Redo | GuestMsg::Revert { .. });
        // A project edit refused before the guest joined still answers that edit.
        let edit_seq = match &msg {
            GuestMsg::EditProject { seq, .. } => Some(*seq),
            _ => None,
        };
        // A project edit gets exactly one answer of its own.
        let msg = match (&status, msg) {
            (Status::Joined, GuestMsg::EditProject { edit, seq }) => return self.guest_edit_project(from, edit, seq),
            (_, msg) => msg,
        };
        let result = match (status, msg) {
            (Status::Connected, GuestMsg::Join { protocol, .. }) if protocol != crate::PROTOCOL_VERSION => {
                // Turned away before anyone sees the request: nothing of the session changes.
                self.peers.remove(&from);
                let version = crate::VersionMismatch {
                    host: crate::PROTOCOL_VERSION,
                    guest: protocol,
                };
                self.outbox.push((from, HostMsg::Declined { version: Some(version) }));
                Ok(())
            }
            (Status::Connected, GuestMsg::Join { name, email, .. }) => {
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
            (Status::Joined, GuestMsg::Undo | GuestMsg::Redo | GuestMsg::Revert { .. })
                if self.take_backs_held.is_some() =>
            {
                Err(Refusal::Invalid {
                    reason: self.take_backs_held.clone().unwrap_or_default(),
                })
            }
            (Status::Joined, GuestMsg::Undo) => self.do_undo(from),
            (Status::Joined, GuestMsg::Redo) => self.do_redo(from),
            (Status::Joined, GuestMsg::Revert { change }) => self.do_revert(from, change, ChangeKind::Undo),
            _ => Err(Refusal::NotApproved),
        };
        if let Err(refusal) = &result {
            self.outbox.push((
                from,
                HostMsg::Refused {
                    refusal: refusal.clone(),
                    take_back,
                    edit: edit_seq,
                    put_back: None,
                },
            ));
        }
        result
    }

    fn do_open(&mut self, by: PeerId, page: Option<String>) -> Result<(), Refusal> {
        // The page held so far is given up first, even when the new one is refused: the guest
        // gave it up on its side when it asked, and the lock list sent below says so to all.
        let held = self.locks.len();
        self.locks.retain(|_, holder| *holder != by);
        let refusal = match &page {
            Some(id) if self.project.page(id).is_none() => Some(Refusal::UnknownPage { page: id.clone() }),
            Some(id) => self.locks.get(id).map(|&holder| Refusal::PageLocked {
                page: id.clone(),
                by: holder,
            }),
            None => None,
        };
        if let Some(refusal) = refusal {
            if let Some(p) = self.peers.get_mut(&by) {
                p.page = None;
            }
            if self.locks.len() != held {
                self.broadcast_presence();
            }
            return Err(refusal);
        }
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
        let Some(current) = self.project.page(&page.id) else {
            return Err(Refusal::UnknownPage { page: page.id });
        };
        // The display number is project-level: only `Renumber` changes it.
        page.number = current.number.clone();
        if *current == page {
            // Nothing changed, but the author still gets its answer (see `Guest::edit_page`).
            if by != HOST {
                self.outbox.push((by, HostMsg::PageChanged { page, by }));
            }
            return Ok(());
        }
        unlocked_placements(&self.project, Some(current), &page)?;
        // A wire may not end on a pin the locked version of its symbol lacks: an update may have
        // dropped the pin after the author drew the wire. Wires already like that may stay.
        let on_page = |p: &Page| {
            let mut probe = Project::new("");
            probe.lock = self.project.lock.clone();
            probe.pages = vec![p.clone()];
            self.dangling_in(&probe)
        };
        let was = on_page(current);
        let dangling: Vec<String> = on_page(&page).difference(&was).cloned().collect();
        if !dangling.is_empty() {
            return Err(Refusal::Invalid {
                reason: format!(
                    "wires end on pins the project's version of their symbol does not have ({}); the symbol was updated meanwhile",
                    dangling.join(", ")
                ),
            });
        }
        let Some(current) = self.project.page_mut(&page.id) else {
            return Err(Refusal::UnknownPage { page: page.id });
        };
        let before = std::mem::replace(current, page.clone());
        let message = format!("Edit page {} {}", page.number, page.title);
        if let Err(refusal) = self.record(by, Some(page.id.clone()), message.clone()) {
            if let Some(p) = self.project.page_mut(&page.id) {
                *p = before;
            }
            self.restore_files(by);
            return Err(refusal);
        }
        // Everyone gets the page, the author too: a refusal of an earlier edit sent the author
        // the old page, and this puts the accepted one back after it, in order.
        let id = page.id.clone();
        let parts = |p: Page| Parts {
            pages: [(id.clone(), Some(p))].into(),
            ..Parts::default()
        };
        let (before, after) = (parts(before), parts(page.clone()));
        self.send_others(HOST, HostMsg::PageChanged { page, by });
        self.remember(by, Some(id.clone()), message, ChangeKind::Edit, None, before, after);
        Ok(())
    }

    /// A guest's project edit, answered: applied (its `seq` on the author's
    /// [`HostMsg::ProjectChanged`]), unchanged, or refused with what it touches as the host has
    /// it, since the guest applied it to its copy at once (pages it sets too: the guest writes
    /// a page it holds at once).
    fn guest_edit_project(&mut self, from: PeerId, edit: ProjectEdit, seq: u64) -> Result<(), Refusal> {
        let keys =
            (edit.partly_applied_at_once() || edit.set_pages().next().is_some()).then(|| edit.touches(&self.project));
        match self.do_edit_project(from, edit, Some(seq)) {
            Ok(true) => Ok(()),
            Ok(false) => {
                self.outbox.push((from, HostMsg::Unchanged { seq }));
                Ok(())
            }
            Err(refusal) => {
                // Read again: a refused change leaves the project as it was before it.
                let put_back = keys.map(|k| Box::new(Parts::read(&self.project, &k)));
                self.outbox.push((
                    from,
                    HostMsg::Refused {
                        refusal: refusal.clone(),
                        take_back: false,
                        edit: Some(seq),
                        put_back,
                    },
                ));
                Err(refusal)
            }
        }
    }

    /// Apply a project edit; true when it changed the project. `seq` is the author's number for
    /// it, sent back on the author's copy of the change.
    fn do_edit_project(&mut self, by: PeerId, edit: ProjectEdit, seq: Option<u64>) -> Result<bool, Refusal> {
        // A restore is made only by the host's own undo (`do_revert`), after it checked that
        // every part is still as the undone change left it.
        if matches!(edit, ProjectEdit::Restore { .. }) {
            return Err(Refusal::Invalid {
                reason: "a restore comes only from Undo; use Undo or \"Undo this change\"".into(),
            });
        }
        check_edit(&self.project, &self.locks, by, &edit)?;
        check_placements(&self.project, &edit)?;
        if let ProjectEdit::AddPicture { picture } = edit {
            self.add_picture(picture)?;
            return Ok(false);
        }
        // An item the session locks in that version but has no file of (the host shared with
        // the copy missing) still goes in: its file is what guests lack.
        let fills = match &edit {
            ProjectEdit::AddItems { symbols, parts } => {
                symbols
                    .iter()
                    .any(|(s, _)| self.symbols.get(&s.id).is_none_or(|have| have.hash() != s.hash()))
                    || parts
                        .iter()
                        .any(|(p, _)| self.parts.get(&p.id).is_none_or(|have| have.hash() != p.hash()))
            }
            _ => false,
        };
        let before = self.project.clone();
        apply_edit(&mut self.project, &edit);
        if self.project == before && !fills {
            return Ok(false);
        }
        // Pages set whole, as a page edit: a wire may not newly end on a pin the locked version of
        // its symbol lacks (an update may have dropped the pin after the author drew the wire).
        if edit.set_pages().next().is_some() {
            let was = self.dangling_in(&before);
            let dangling: Vec<String> = self.dangling_in(&self.project).difference(&was).cloned().collect();
            if !dangling.is_empty() {
                self.project = before;
                return Err(Refusal::Invalid {
                    reason: format!(
                        "wires end on pins the project's version of their symbol does not have ({}); the symbol was updated meanwhile",
                        dangling.join(", ")
                    ),
                });
            }
        }
        let message = edit.describe();
        let page = edit.page().map(str::to_owned);
        let added = match &edit {
            ProjectEdit::AddItems { symbols, parts } => Some((
                symbols.iter().map(|(s, _)| s.clone()).collect::<Vec<_>>(),
                parts.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(),
            )),
            ProjectEdit::UpdateItems { symbols, parts } => Some((
                symbols.iter().map(|(s, _, _)| s.clone()).collect::<Vec<_>>(),
                parts.iter().map(|(p, _, _)| p.clone()).collect::<Vec<_>>(),
            )),
            _ => None,
        };
        // The files go in first, so the change's commit holds them with their lock entries. A
        // file left behind by a change that is not recorded is harmless: nothing locks it.
        let stored = match &added {
            Some((symbols, parts)) => self.store_files(by, page.clone(), &message, symbols, parts),
            None => Ok(()),
        };
        if let Err(refusal) = stored.and_then(|()| self.record(by, page.clone(), message.clone())) {
            self.project = before;
            self.restore_files(by);
            return Err(refusal);
        }
        if let Some((symbols, parts)) = added {
            let update = matches!(edit, ProjectEdit::UpdateItems { .. });
            self.take_versions(symbols, parts, update);
        }
        let (was, now) = Parts::delta(&before, &self.project);
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
            let seq = if id == by { seq } else { None };
            self.outbox.push((
                id,
                HostMsg::ProjectChanged {
                    edit: edit.clone(),
                    by,
                    seq,
                },
            ));
        }
        // A file put back alone (the project as it was) is no step to undo.
        if self.project != before {
            self.remember(by, page, message, ChangeKind::Edit, None, was, now);
        }
        Ok(true)
    }

    /// Store a checked picture and send it to everyone. It changes no project data, so it has
    /// no change of its own: the change that names it commits it.
    fn add_picture(&mut self, picture: crate::Picture) -> Result<(), Refusal> {
        match self.pictures.get(&picture.file) {
            Some(have) if *have == picture.data => return Ok(()),
            Some(_) => return Err(Refusal::PictureNameTaken { file: picture.file }),
            None => {}
        }
        let (count, bytes) = self.added_pictures;
        if count >= crate::MAX_ADDED_PICTURES
            || bytes.saturating_add(picture.data.len()) > crate::MAX_ADDED_PICTURE_BYTES
        {
            return Err(Refusal::Invalid {
                reason: format!(
                    "the session takes at most {} new pictures of {} MB together",
                    crate::MAX_ADDED_PICTURES,
                    crate::MAX_ADDED_PICTURE_BYTES / (1024 * 1024)
                ),
            });
        }
        // A file of that name in the host's folder that the session does not use (an old logo) is
        // never written over: the sender renames its picture instead.
        self.journal.picture(&picture).map_err(|e| match e {
            crate::Error::PictureNameTaken(_) => Refusal::PictureNameTaken {
                file: picture.file.clone(),
            },
            e => Refusal::NotRecorded { reason: e.to_string() },
        })?;
        self.added_pictures = (count + 1, bytes + picture.data.len());
        self.pictures.insert(picture.file.clone(), picture.data.clone());
        let ids: Vec<PeerId> = self.joined_guests().collect();
        for id in ids {
            self.outbox.push((
                id,
                HostMsg::Picture {
                    picture: picture.clone(),
                },
            ));
        }
        Ok(())
    }

    fn do_undo(&mut self, by: PeerId) -> Result<(), Refusal> {
        let Some(id) = undo_target(&self.infos(), by) else {
            return Err(Refusal::Invalid {
                reason: "there is no change of yours to undo".into(),
            });
        };
        self.do_revert(by, id, ChangeKind::Undo)
    }

    fn do_redo(&mut self, by: PeerId) -> Result<(), Refusal> {
        let Some(id) = redo_target(&self.infos(), by) else {
            return Err(Refusal::Invalid {
                reason: "there is no undo of yours to redo".into(),
            });
        };
        self.do_revert(by, id, ChangeKind::Redo)
    }

    fn infos(&self) -> Vec<ChangeInfo> {
        self.history()
    }

    /// Take back recorded change `id`: put back the parts it changed, as long as nobody has
    /// changed them since. Recorded as a change of its own, by `by`.
    fn do_revert(&mut self, by: PeerId, id: u64, kind: ChangeKind) -> Result<(), Refusal> {
        let invalid = |reason: String| Err(Refusal::Invalid { reason });
        let Some(entry) = self.history.iter().find(|e| e.info.id == id).cloned() else {
            return invalid(format!("change {id} is no longer in the session's history"));
        };
        if entry.info.undone_by.is_some() {
            return invalid(format!("\"{}\" was already taken back", entry.info.message));
        }
        if Parts::read(&self.project, &entry.after) != entry.after {
            return invalid(format!(
                "\"{}\" cannot be taken back: what it changed was changed again since; take back the later change first",
                entry.info.message
            ));
        }
        let used = entry.before.unlocked_uses(&self.project);
        if !used.is_empty() {
            return invalid(format!(
                "\"{}\" cannot be taken back: the project would use {} without it being in the project's library; remove that use or add it again first",
                entry.info.message,
                used.join(", ")
            ));
        }
        // A lock entry is put back only for a version whose file the session still has: someone
        // may have used the id in another version meanwhile.
        let mut missing = Vec::new();
        for (id, e) in &entry.before.lock_symbols {
            if let Some(e) = e
                && self.symbol_version(id, &e.hash).is_none()
            {
                missing.push(format!("symbol {id}"));
            }
        }
        for (id, e) in &entry.before.lock_parts {
            if let Some(e) = e
                && self.part_version(id, &e.hash).is_none()
            {
                missing.push(format!("part {id}"));
            }
        }
        if !missing.is_empty() {
            return invalid(format!(
                "\"{}\" cannot be taken back: the session no longer has that version of {}",
                entry.info.message,
                missing.join(", ")
            ));
        }
        let edit = ProjectEdit::Restore {
            parts: Box::new(entry.before.clone()),
        };
        check_edit(&self.project, &self.locks, by, &edit)?;
        let before = self.project.clone();
        apply_edit(&mut self.project, &edit);
        // Every wire end the undo leaves must be on a pin of its symbol's locked version: a symbol
        // locked again in an older version, or a wire put back on a pin a later update dropped.
        let was = self.dangling_in(&before);
        let dangling: Vec<String> = self.dangling_in(&self.project).difference(&was).cloned().collect();
        if !dangling.is_empty() {
            self.project = before;
            return invalid(format!(
                "\"{}\" cannot be taken back: wires end on pins the earlier versions do not have ({}); move those wires first",
                entry.info.message,
                dangling.join(", ")
            ));
        }
        // The message names the edit itself: a redo of "Undo X" is "Redo X".
        let what = match kind {
            ChangeKind::Redo => entry
                .info
                .reverts
                .and_then(|r| self.history.iter().find(|e| e.info.id == r))
                .map_or_else(|| entry.info.message.clone(), |e| e.info.message.clone()),
            _ => entry.info.message.clone(),
        };
        let message = match kind {
            ChangeKind::Redo => format!("Redo: {what}"),
            _ => format!("Undo: {what}"),
        };
        // The files of the versions it locks again go in first, written as the session has them:
        // a change that was not recorded may have left another version of the file behind.
        let symbols: Vec<wc_core::Symbol> = entry
            .before
            .lock_symbols
            .iter()
            .filter_map(|(id, e)| self.symbol_version(id, &e.as_ref()?.hash))
            .collect();
        let parts: Vec<wc_core::Part> = entry
            .before
            .lock_parts
            .iter()
            .filter_map(|(id, e)| self.part_version(id, &e.as_ref()?.hash))
            .collect();
        let stored = if symbols.is_empty() && parts.is_empty() {
            Ok(())
        } else {
            self.store_files(by, entry.info.page.clone(), &message, &symbols, &parts)
        };
        if let Err(refusal) = stored.and_then(|()| self.record(by, entry.info.page.clone(), message.clone())) {
            self.project = before;
            self.restore_files(by);
            return Err(refusal);
        }
        // A page that is gone now cannot stay open.
        let gone: Vec<String> = self
            .locks
            .keys()
            .filter(|p| self.project.page(p).is_none())
            .cloned()
            .collect();
        if !gone.is_empty() {
            for p in &gone {
                self.locks.remove(p);
            }
            for p in self.peers.values_mut() {
                if p.page.as_ref().is_some_and(|id| gone.contains(id)) {
                    p.page = None;
                }
            }
            self.broadcast_presence();
        }
        // Versions the guests may no longer have go out before the change that locks them.
        let resent: (Vec<_>, Vec<_>) = (
            symbols
                .iter()
                .filter(|s| self.symbols.get(&s.id).is_none_or(|have| have.hash() != s.hash()))
                .cloned()
                .collect(),
            parts
                .iter()
                .filter(|p| self.parts.get(&p.id).is_none_or(|have| have.hash() != p.hash()))
                .cloned()
                .collect(),
        );
        self.take_versions(symbols, parts, true);
        let ids: Vec<PeerId> = self.joined_guests().collect();
        for g in ids {
            // One file a message: each came in one message, so each fits the line limit, while
            // several together may not.
            for s in &resent.0 {
                self.outbox.push((
                    g,
                    HostMsg::Items {
                        symbols: vec![s.clone()],
                        parts: Vec::new(),
                    },
                ));
            }
            for p in &resent.1 {
                self.outbox.push((
                    g,
                    HostMsg::Items {
                        symbols: Vec::new(),
                        parts: vec![p.clone()],
                    },
                ));
            }
            self.outbox.push((
                g,
                HostMsg::ProjectChanged {
                    edit: edit.clone(),
                    by,
                    seq: None,
                },
            ));
        }
        self.remember(by, entry.info.page, message, kind, Some(id), entry.after, entry.before);
        Ok(())
    }

    /// Keep a recorded change for undo and tell everyone it is in the list.
    #[allow(clippy::too_many_arguments)]
    fn remember(
        &mut self,
        by: PeerId,
        page: Option<String>,
        message: String,
        kind: ChangeKind,
        reverts: Option<u64>,
        before: Parts,
        after: Parts,
    ) {
        let info = ChangeInfo {
            id: self.next_change,
            by,
            name: self.peers.get(&by).map(|p| p.name.clone()).unwrap_or_default(),
            message,
            page,
            kind,
            reverts,
            undone_by: None,
        };
        self.next_change += 1;
        if let Some(r) = reverts
            && let Some(e) = self.history.iter_mut().find(|e| e.info.id == r)
        {
            e.info.undone_by = Some(info.id);
        }
        self.history.push(Entry {
            info: info.clone(),
            before,
            after,
        });
        if self.history.len() > crate::MAX_HISTORY {
            let extra = self.history.len() - crate::MAX_HISTORY;
            self.history.drain(..extra);
        }
        // Replaced versions only an undo or redo in the kept history can lock again stay.
        let wanted = |e: &Entry, symbols: bool| -> Vec<(String, String)> {
            [&e.before, &e.after]
                .into_iter()
                .flat_map(|p| if symbols { &p.lock_symbols } else { &p.lock_parts })
                .filter_map(|(id, l)| Some((id.clone(), l.as_ref()?.hash.clone())))
                .collect()
        };
        let symbols: BTreeSet<(String, String)> = self.history.iter().flat_map(|e| wanted(e, true)).collect();
        let parts: BTreeSet<(String, String)> = self.history.iter().flat_map(|e| wanted(e, false)).collect();
        self.old_symbols.retain(|k, _| symbols.contains(k));
        self.old_parts.retain(|k, _| parts.contains(k));
        let ids: Vec<PeerId> = self.joined_guests().collect();
        for g in ids {
            self.outbox.push((g, HostMsg::Recorded { change: info.clone() }));
        }
    }

    /// Symbol `id` in the version `hash`: the one guests get, or one an update replaced.
    /// Wire ends in `project` on pins the locked version of their symbol does not have, for the
    /// versions the session has.
    fn dangling_in(&self, project: &Project) -> BTreeSet<String> {
        project
            .lock
            .symbols
            .iter()
            .filter_map(|(id, e)| Some((id, self.symbol_version(id, &e.hash)?)))
            .flat_map(|(id, s)| project.dangling_pins(id, &s))
            .collect()
    }

    fn symbol_version(&self, id: &str, hash: &str) -> Option<wc_core::Symbol> {
        match self.symbols.get(id) {
            Some(s) if s.hash() == hash => Some(s.clone()),
            _ => self.old_symbols.get(&(id.to_owned(), hash.to_owned())).cloned(),
        }
    }

    /// Part `id` in the version `hash`, as [`Self::symbol_version`].
    fn part_version(&self, id: &str, hash: &str) -> Option<wc_core::Part> {
        match self.parts.get(id) {
            Some(p) if p.hash() == hash => Some(p.clone()),
            _ => self.old_parts.get(&(id.to_owned(), hash.to_owned())).cloned(),
        }
    }

    /// Make these the versions guests get. With `keep`, the versions they replace are kept for
    /// an undo ([`Self::symbol_version`]).
    fn take_versions(&mut self, symbols: Vec<wc_core::Symbol>, parts: Vec<wc_core::Part>, keep: bool) {
        for s in symbols {
            self.file_hashes
                .insert((wc_core::lock::ItemKind::Symbol, s.id.clone()), s.hash());
            if let Some(old) = self.symbols.insert(s.id.clone(), s)
                && keep
                && self.symbols.get(&old.id).is_some_and(|now| now.hash() != old.hash())
            {
                self.old_symbols.insert((old.id.clone(), old.hash()), old);
            }
        }
        for p in parts {
            self.file_hashes
                .insert((wc_core::lock::ItemKind::Part, p.id.clone()), p.hash());
            if let Some(old) = self.parts.insert(p.id.clone(), p)
                && keep
                && self.parts.get(&old.id).is_some_and(|now| now.hash() != old.hash())
            {
                self.old_parts.insert((old.id.clone(), old.hash()), old);
            }
        }
    }

    /// Store the files of the symbols and parts a change locks, before it is recorded. A
    /// failure is kept for the host like a failed record ([`Self::take_journal_errors`]).
    fn store_files(
        &mut self,
        by: PeerId,
        page: Option<String>,
        message: &str,
        symbols: &[wc_core::Symbol],
        parts: &[wc_core::Part],
    ) -> Result<(), Refusal> {
        if let Err(e) = self.journal.vendor(symbols, parts) {
            let reason = e.to_string();
            let change = Change {
                by,
                author: self.peers.get(&by).map(Participant::author).unwrap_or_default(),
                page,
                message: message.to_owned(),
            };
            self.journal_errors.push((change, reason.clone()));
            return Err(Refusal::NotRecorded { reason });
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

    /// After a change could not be recorded: write the project as it is again, so the files
    /// do not keep the change that was undone.
    fn restore_files(&mut self, by: PeerId) {
        if let Err(e) = self.journal.restore(&self.project) {
            let change = Change {
                by,
                author: self.peers.get(&by).map(Participant::author).unwrap_or_default(),
                page: None,
                message: "Put the project folder back after a change that was not recorded".into(),
            };
            self.journal_errors.push((change, e.to_string()));
        }
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

/// A page may not start using a symbol the project does not lock: the app sends the item first
/// (`AddItems`), so only a broken or older guest gets here. Each placement is judged on its own
/// (by placement and symbol), so placements the page already had stay editable, also of a
/// symbol the project never locked (older projects), while a new one of that symbol is refused.
fn unlocked_placements(project: &Project, old: Option<&Page>, new: &Page) -> Result<(), Refusal> {
    // Counted: one placement the page had keeps one placement editable, so copies of it under
    // the same id are new uses.
    let mut had: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for o in old.map(|p| p.symbols.as_slice()).unwrap_or_default() {
        *had.entry((o.id.as_str(), o.symbol.as_str())).or_default() += 1;
    }
    let unlocked: BTreeSet<&str> = new
        .symbols
        .iter()
        .filter(|o| match had.get_mut(&(o.id.as_str(), o.symbol.as_str())) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        })
        .map(|o| o.symbol.as_str())
        .filter(|id| !project.lock.symbols.contains_key(*id))
        .collect();
    if unlocked.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = unlocked.into_iter().map(|id| format!("symbol {id}")).collect();
    Err(Refusal::Invalid {
        reason: format!(
            "page {} uses {} that the project's library does not have; add it to the session first",
            new.number,
            names.join(", ")
        ),
    })
}

/// [`unlocked_placements`] for every page a project edit writes: a new page, the pages of a
/// `SetPages`, and those in a batch (which never adds items itself).
fn check_placements(project: &Project, edit: &ProjectEdit) -> Result<(), Refusal> {
    match edit {
        ProjectEdit::AddPage { page } => unlocked_placements(project, project.page(&page.id), page),
        ProjectEdit::SetPages { pages, .. } => pages
            .iter()
            .try_for_each(|p| unlocked_placements(project, project.page(&p.id), p)),
        ProjectEdit::Batch { edits } => edits.iter().try_for_each(|e| check_placements(project, e)),
        _ => Ok(()),
    }
}

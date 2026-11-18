//! The host's side of a session: approval, page locks, applying changes and telling everyone.

use std::collections::BTreeMap;

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
            pictures: BTreeMap::new(),
            added_pictures: (0, 0),
        }
    }

    /// The project's vendored symbol and part files, sent to every guest that joins so they can
    /// draw the project without the host's libraries.
    pub fn share_files(&mut self, symbols: BTreeMap<String, wc_core::Symbol>, parts: BTreeMap<String, wc_core::Part>) {
        self.symbols = symbols;
        self.parts = parts;
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
        let Some(current) = self.project.page_mut(&page.id) else {
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
    /// it, since the guest applied it to its copy at once.
    fn guest_edit_project(&mut self, from: PeerId, edit: ProjectEdit, seq: u64) -> Result<(), Refusal> {
        let keys = edit.partly_applied_at_once().then(|| edit.touches(&self.project));
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
        if let ProjectEdit::AddPicture { picture } = edit {
            self.add_picture(picture)?;
            return Ok(false);
        }
        let before = self.project.clone();
        apply_edit(&mut self.project, &edit);
        if self.project == before {
            return Ok(false);
        }
        let message = edit.describe();
        let page = edit.page().map(str::to_owned);
        let added = match &edit {
            ProjectEdit::AddItems { symbols, parts } => Some((
                symbols.iter().map(|(s, _)| s.clone()).collect::<Vec<_>>(),
                parts.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(),
            )),
            _ => None,
        };
        // The files go in first, so the change's commit holds them with their lock entries. A
        // file left behind by a change that is not recorded is harmless: nothing locks it.
        let stored = match &added {
            Some((symbols, parts)) => self
                .journal
                .vendor(symbols, parts)
                .map_err(|e| Refusal::NotRecorded { reason: e.to_string() }),
            None => Ok(()),
        };
        if let Err(refusal) = stored.and_then(|()| self.record(by, page.clone(), message.clone())) {
            self.project = before;
            self.restore_files(by);
            return Err(refusal);
        }
        if let Some((symbols, parts)) = added {
            self.symbols.extend(symbols.into_iter().map(|s| (s.id.clone(), s)));
            self.parts.extend(parts.into_iter().map(|p| (p.id.clone(), p)));
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
        self.remember(by, page, message, ChangeKind::Edit, None, was, now);
        Ok(true)
    }

    /// Store a checked picture and send it to everyone. It changes no project data, so it has
    /// no change of its own: the change that names it commits it.
    fn add_picture(&mut self, picture: crate::Picture) -> Result<(), Refusal> {
        match self.pictures.get(&picture.file) {
            Some(have) if *have == picture.data => return Ok(()),
            Some(_) => {
                return Err(Refusal::Invalid {
                    reason: format!(
                        "the session already has another picture named {}; pick the picture again to give it a new name",
                        picture.file
                    ),
                });
            }
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
        self.journal
            .picture(&picture)
            .map_err(|e| Refusal::NotRecorded { reason: e.to_string() })?;
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
                && self.symbols.get(id).map(wc_core::Symbol::hash).as_ref() != Some(&e.hash)
            {
                missing.push(format!("symbol {id}"));
            }
        }
        for (id, e) in &entry.before.lock_parts {
            if let Some(e) = e
                && self.parts.get(id).map(wc_core::Part::hash).as_ref() != Some(&e.hash)
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
            .filter(|(_, e)| e.is_some())
            .filter_map(|(id, _)| self.symbols.get(id).cloned())
            .collect();
        let parts: Vec<wc_core::Part> = entry
            .before
            .lock_parts
            .iter()
            .filter(|(_, e)| e.is_some())
            .filter_map(|(id, _)| self.parts.get(id).cloned())
            .collect();
        let stored = if symbols.is_empty() && parts.is_empty() {
            Ok(())
        } else {
            self.journal
                .vendor(&symbols, &parts)
                .map_err(|e| Refusal::NotRecorded { reason: e.to_string() })
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
        let ids: Vec<PeerId> = self.joined_guests().collect();
        for g in ids {
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
        let ids: Vec<PeerId> = self.joined_guests().collect();
        for g in ids {
            self.outbox.push((g, HostMsg::Recorded { change: info.clone() }));
        }
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

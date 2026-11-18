//! Shared work sessions in the window: share the open project, join someone else's, approve
//! join requests, and keep the open project in step with the session (`wc-share`).
//!
//! Each frame, [`Share::frame`] compares the open project with what it was after the last
//! frame's sync. Changes to the page this person holds go out as one page edit; changes to
//! project-level data (devices, comments, panels, pages added, removed or renumbered, project
//! properties) go out as project edits, and changes a tool made to pages nobody else holds
//! (wire numbering, a tag renamed on every page) go out together as one `SetPages`. Then the
//! session's project comes back in, so pages other people hold are read-only: a local change
//! to one of them is put back, and the whole action with it. Nothing goes out
//! while a mouse button is down or a live form is open, so a drag over many frames is one
//! change, and one commit on the host.
//!
//! Symbol and part files travel too: a guest gets the project's vendored files when it joins,
//! and a symbol or part someone uses for the first time goes out as an `AddItems` edit with its
//! file, so every copy can draw it and the host's folder and history keep it.

use std::collections::BTreeMap;
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use wc_core::lock::ItemKind;
use wc_core::{Page, Part, Project, Symbol, store};
use wc_share::discovery::{Announcer, Beacon, Browser};
use wc_share::net::{self, Listener};
use wc_share::transport::{GuestLink, Hub};
use wc_share::{ChangeInfo, ChangeKind, redo_target, undo_target};
use wc_share::{GitJournal, Guest, GuestMsg, GuestStatus, HOST, Host, Peer, PeerId, ProjectEdit};

use crate::state::Open;

/// The TCP port a shared session listens on when it is free; otherwise any free port is used
/// (a second session on the same computer, for example).
pub const SESSION_PORT: u16 = 47_812;

/// What one frame's sync wants to send: the edit of the held page, the project-level edits, and
/// the numbers of pages whose local changes could not be kept.
#[derive(Debug, Default, PartialEq)]
pub struct Outgoing {
    pub page: Option<Page>,
    pub edits: Vec<ProjectEdit>,
    pub dropped: Vec<String>,
}

/// A page without its display number, which only `Renumber` changes.
fn same_content(a: &Page, b: &Page) -> bool {
    wc_share::same_but_number(a, b)
}

/// What changed in `local` since `base` (the project after the last sync), as session messages.
/// `mine` is the page this person holds and goes out as a page edit; changes to pages nobody
/// else holds go out together as one `SetPages`; changes to pages someone else holds cannot be
/// sent and are listed in `dropped`. `held` tells who holds which page, so removing a page someone else has
/// open is not even asked for.
pub fn outgoing(
    local: &Project,
    base: &Project,
    mine: Option<&str>,
    held: &BTreeMap<String, PeerId>,
    me: PeerId,
) -> Outgoing {
    let mut out = Outgoing::default();
    if local.meta != base.meta {
        out.edits.push(
            ProjectEdit::SetMeta {
                meta: Box::new(local.meta.clone()),
                before: None,
            }
            .stamped(base),
        );
    }

    // Pages: removed, added, renumbered, edited.
    for b in &base.pages {
        if local.page(&b.id).is_none() {
            match held.get(&b.id) {
                Some(&holder) if holder != me => out.dropped.push(b.number.clone()),
                _ => out.edits.push(ProjectEdit::RemovePage { id: b.id.clone() }),
            }
        }
    }
    let mut numbers = Vec::new();
    let mut others = Vec::new();
    let mut others_before = Vec::new();
    for p in &local.pages {
        let Some(b) = base.page(&p.id) else {
            out.edits.push(ProjectEdit::AddPage { page: p.clone() });
            continue;
        };
        if p.number != b.number {
            numbers.push((p.id.clone(), p.number.clone()));
        }
        if !same_content(p, b) {
            if mine == Some(p.id.as_str()) {
                out.page = Some(p.clone());
            } else {
                match held.get(&p.id) {
                    Some(&holder) if holder != me => out.dropped.push(p.number.clone()),
                    // A page nobody has open, changed by a tool that works on several pages
                    // (wire numbering, a tag renamed everywhere): one edit for all of them.
                    _ => {
                        others.push(p.clone());
                        others_before.push(b.clone());
                    }
                }
            }
        }
    }
    if !numbers.is_empty() {
        out.edits.push(ProjectEdit::Renumber { numbers });
    }

    // Devices, one edit per changed tag.
    for (tag, d) in &local.devices {
        if base.devices.get(tag) != Some(d) {
            out.edits.push(
                ProjectEdit::SetDevice {
                    tag: tag.clone(),
                    device: Some(d.clone()),
                    before: None,
                }
                .stamped(base),
            );
        }
    }
    for tag in base.devices.keys() {
        if !local.devices.contains_key(tag) {
            out.edits.push(
                ProjectEdit::SetDevice {
                    tag: tag.clone(),
                    device: None,
                    before: None,
                }
                .stamped(base),
            );
        }
    }

    // Comments, per page; a removed page takes its comments with it.
    let pages: std::collections::BTreeSet<&str> = local
        .comments
        .iter()
        .chain(&base.comments)
        .map(|c| c.page.as_str())
        .collect();
    for page in pages {
        if local.page(page).is_none() {
            continue;
        }
        let of = |p: &Project| {
            p.comments
                .iter()
                .filter(|c| c.page == page)
                .cloned()
                .collect::<Vec<_>>()
        };
        let now = of(local);
        if now != of(base) {
            out.edits.push(
                ProjectEdit::SetComments {
                    page: page.to_owned(),
                    comments: now,
                    before: None,
                }
                .stamped(base),
            );
        }
    }

    // Panels.
    for p in &local.panels {
        if base.panels.iter().find(|b| b.id == p.id) != Some(p) {
            out.edits.push(
                ProjectEdit::SetPanel {
                    panel: p.clone(),
                    before: None,
                }
                .stamped(base),
            );
        }
    }
    for b in &base.panels {
        if !local.panels.iter().any(|p| p.id == b.id) {
            out.edits.push(ProjectEdit::RemovePanel { id: b.id.clone() });
        }
    }

    // Last, so the devices the pages name are there before them.
    if !others.is_empty() {
        out.edits.push(ProjectEdit::SetPages {
            pages: others,
            before: others_before,
        });
    }
    out
}

/// Parts changed both in this window and in the session since the last sync (`base`): the
/// session's version wins on them, in `local` and in `base`, so what is sent next is not built
/// on content an undo has just put back. A part is a page (also one removed on either side),
/// the project properties, one device, the comments of one page, or one panel; a change to a
/// part the session did not touch is kept and sent as usual. Returns the parts whose local
/// change was dropped, named for the person.
pub fn yield_to_session(local: &mut Project, base: &mut Project, auth: &Project) -> Vec<String> {
    let mut dropped = Vec::new();

    if local.meta != base.meta && auth.meta != base.meta {
        dropped.push("the project properties".to_owned());
        local.meta = auth.meta.clone();
        base.meta = auth.meta.clone();
    }

    // Pages, by id, including one this window removed or the session removed.
    let ids: Vec<String> = base.pages.iter().map(|p| p.id.clone()).collect();
    for id in ids {
        let Some(pos) = base.pages.iter().position(|p| p.id == id) else {
            continue;
        };
        // The number on its own: both gave the page another number, the session's stays.
        if let (Some(a), Some(l)) = (auth.page(&id), local.page_mut(&id)) {
            let was = &base.pages[pos].number;
            if a.number != *was && l.number != *was && l.number != a.number {
                dropped.push(format!("the number of page {was}"));
                l.number = a.number.clone();
                base.pages[pos].number = a.number.clone();
            }
        }
        let b = &base.pages[pos];
        let a = auth.page(&id);
        let l = local.page(&id);
        let differs = |x: Option<&Page>| x.is_none_or(|x| !same_content(x, b));
        // A renumbering goes out on its own, so a content change here does not clash with
        // one; a page removed here does, as removing it would take the number back too.
        let session_changed = match l {
            Some(_) => differs(a),
            None => a != Some(b),
        };
        if !session_changed || !differs(l) {
            continue;
        }
        dropped.push(format!("page {}", b.number));
        match a {
            Some(a) => {
                match local.page_mut(&id) {
                    // A number only this window changed goes out on its own, as a renumbering.
                    Some(p) => {
                        let number = if p.number != b.number {
                            p.number.clone()
                        } else {
                            a.number.clone()
                        };
                        *p = a.clone();
                        p.number = number;
                    }
                    None => {
                        let at = pos.min(local.pages.len());
                        local.pages.insert(at, a.clone());
                    }
                }
                base.pages[pos] = a.clone();
            }
            None => {
                local.pages.retain(|p| p.id != id);
                base.pages.remove(pos);
            }
        }
    }

    // Devices, by tag.
    let tags: std::collections::BTreeSet<_> = base.devices.keys().chain(auth.devices.keys()).cloned().collect();
    for tag in tags {
        let b = base.devices.get(&tag);
        let a = auth.devices.get(&tag);
        if a == b || local.devices.get(&tag) == b {
            continue;
        }
        dropped.push(format!("device {tag}"));
        match a.cloned() {
            Some(d) => {
                local.devices.insert(tag.clone(), d.clone());
                base.devices.insert(tag, d);
            }
            None => {
                local.devices.remove(&tag);
                base.devices.remove(&tag);
            }
        }
    }

    // Comments, by page.
    let pages: std::collections::BTreeSet<String> = base
        .comments
        .iter()
        .chain(&auth.comments)
        .map(|c| c.page.clone())
        .collect();
    for page in pages {
        let of = |p: &Project| {
            p.comments
                .iter()
                .filter(|c| c.page == page)
                .cloned()
                .collect::<Vec<_>>()
        };
        let (a, b) = (of(auth), of(base));
        if a == b || of(local) == b {
            continue;
        }
        dropped.push(format!("the comments on page {}", number_of(auth, &page)));
        for p in [&mut *local, &mut *base] {
            p.comments.retain(|c| c.page != page);
            p.comments.extend(a.iter().cloned());
        }
    }

    // Panels, by id.
    let ids: std::collections::BTreeSet<String> =
        base.panels.iter().chain(&auth.panels).map(|p| p.id.clone()).collect();
    for id in ids {
        let find = |p: &Project| p.panels.iter().find(|x| x.id == id).cloned();
        let (a, b) = (find(auth), find(base));
        if a == b || find(local) == b {
            continue;
        }
        let name = a.as_ref().or(b.as_ref()).map_or_else(|| id.clone(), |p| p.name.clone());
        dropped.push(format!("panel {name}"));
        for p in [&mut *local, &mut *base] {
            match (&a, p.panels.iter().position(|x| x.id == id)) {
                (Some(a), Some(i)) => p.panels[i] = a.clone(),
                (Some(a), None) => p.panels.push(a.clone()),
                (None, Some(i)) => {
                    p.panels.remove(i);
                }
                (None, None) => {}
            }
        }
    }
    dropped
}

/// A page's number for a message, or its id when the page is gone.
fn number_of(p: &Project, page: &str) -> String {
    p.page(page).map_or_else(|| page.to_owned(), |p| p.number.clone())
}

/// Make `local` the session's project `auth`, changing only what differs. The symbol lock stays
/// local, except for items the session has: this copy takes their entries, so an item it lacks
/// is added and one it has in another version (someone else used it first) takes the session's
/// version. Their files come with the session, see [`take_files`]. Returns whether anything
/// changed.
pub fn incoming(local: &mut Project, auth: &Project) -> bool {
    let mut changed = false;
    for kind in [ItemKind::Symbol, ItemKind::Part] {
        for (id, entry) in auth.lock.map(kind) {
            if local.lock.map(kind).get(id).is_none_or(|e| e.hash != entry.hash) {
                local.lock.map_mut(kind).insert(id.clone(), entry.clone());
                changed = true;
            }
        }
    }
    let set = |changed: &mut bool, differ: bool| {
        *changed |= differ;
        differ
    };
    if set(&mut changed, local.meta != auth.meta) {
        local.meta = auth.meta.clone();
    }
    let same_ids =
        local.pages.len() == auth.pages.len() && local.pages.iter().zip(&auth.pages).all(|(a, b)| a.id == b.id);
    if same_ids {
        for (l, a) in local.pages.iter_mut().zip(&auth.pages) {
            if set(&mut changed, l != a) {
                *l = a.clone();
            }
        }
    } else {
        changed = true;
        local.pages = auth.pages.clone();
    }
    if set(&mut changed, local.devices != auth.devices) {
        local.devices = auth.devices.clone();
    }
    if set(&mut changed, local.panels != auth.panels) {
        local.panels = auth.panels.clone();
    }
    if set(&mut changed, local.comments != auth.comments) {
        local.comments = auth.comments.clone();
    }
    changed
}

/// The items `local` locks in another version than the session `auth`: someone else used them
/// first, and [`incoming`] gives this copy the session's version. As `symbol id` or `part id`.
pub fn conflicts(local: &Project, auth: &Project) -> Vec<String> {
    [ItemKind::Symbol, ItemKind::Part]
        .into_iter()
        .flat_map(|kind| {
            local
                .lock
                .map(kind)
                .iter()
                .filter(move |(id, e)| auth.lock.map(kind).get(*id).is_some_and(|a| a.hash != e.hash))
                .map(move |(id, _)| item_name(kind, id))
        })
        .collect()
}

/// An item as messages name it: `symbol wc.coil`, `part acme.relay`.
fn item_name(kind: ItemKind, id: &str) -> String {
    match kind {
        ItemKind::Symbol => format!("symbol {id}"),
        ItemKind::Part => format!("part {id}"),
    }
}

/// Whether the session's project locks the item [`item_name`] names.
fn session_has(auth: &Project, name: &str) -> bool {
    match name.split_once(' ') {
        Some(("symbol", id)) => auth.lock.symbols.contains_key(id),
        Some(("part", id)) => auth.lock.parts.contains_key(id),
        _ => false,
    }
}

/// Why a file read for the session cannot be used: it could not be read, or it holds another
/// version than the lock names (edited by hand, or left by a merge).
fn check_locked<T>(read: wc_core::Result<T>, hash: impl Fn(&T) -> String, locked: &str) -> Result<T, String> {
    let item = read.map_err(|e| e.to_string())?;
    if hash(&item) != locked {
        return Err("its file holds another version than the project's lock".into());
    }
    Ok(item)
}

/// What [`new_items`] found.
#[derive(Debug, Default)]
pub struct NewItems {
    /// The edit that adds them, with their files.
    pub edit: Option<ProjectEdit>,
    /// The items it holds, as [`item_name`] names them.
    pub sent: Vec<String>,
    /// Items whose file could not be read, with the reason; they are not sent.
    pub unreadable: Vec<(String, String)>,
}

/// The symbols and parts locked in `local` and not in the session's project `auth`, with their
/// vendored files from `dir`, as one edit: what this person started using. Items named in
/// `skip` (sent and not answered yet) are left out. An item whose file cannot be read is not
/// sent; it is returned with the reason and tried again next time.
pub fn new_items(dir: &Path, local: &Project, auth: &Project, skip: &std::collections::BTreeSet<String>) -> NewItems {
    let mut out = NewItems::default();
    let fresh = |kind: ItemKind| {
        local
            .lock
            .map(kind)
            .iter()
            .filter(move |(id, _)| !auth.lock.map(kind).contains_key(*id))
            .filter(move |(id, _)| !skip.contains(&item_name(kind, id)))
            .map(|(id, e)| (id.clone(), e.source.clone(), e.hash.clone()))
            .collect::<Vec<_>>()
    };
    let mut symbols = Vec::new();
    for (id, source, hash) in fresh(ItemKind::Symbol) {
        match check_locked(store::read_vendored_symbol(dir, &id), Symbol::hash, &hash) {
            Ok(s) => {
                out.sent.push(item_name(ItemKind::Symbol, &id));
                symbols.push((s, source));
            }
            Err(e) => out.unreadable.push((item_name(ItemKind::Symbol, &id), e)),
        }
    }
    let mut parts = Vec::new();
    for (id, source, hash) in fresh(ItemKind::Part) {
        match check_locked(store::read_vendored_part(dir, &id), Part::hash, &hash) {
            Ok(p) => {
                out.sent.push(item_name(ItemKind::Part, &id));
                parts.push((p, source));
            }
            Err(e) => out.unreadable.push((item_name(ItemKind::Part, &id), e)),
        }
    }
    out.edit = (!symbols.is_empty() || !parts.is_empty()).then_some(ProjectEdit::AddItems { symbols, parts });
    out
}

/// Write the session's files for items `open` locks but has not loaded in the locked version
/// (symbols and parts someone else added, or used first in another version) into its vendor
/// folder, and load them. A file the session offers in another version than the lock names is
/// neither written nor loaded. Returns the items that could not be taken, by [`item_name`],
/// with the reason.
pub fn take_files(
    open: &mut Open,
    symbols: &BTreeMap<String, Symbol>,
    parts: &BTreeMap<String, Part>,
) -> Vec<(String, String)> {
    const OTHER: &str = "the session's file holds another version than its lock";
    let mut failed = Vec::new();
    let wanted: Vec<(String, String)> = open
        .project
        .lock
        .symbols
        .iter()
        .filter(|(id, e)| open.symbols.get(*id).is_none_or(|s| s.hash() != e.hash))
        .map(|(id, e)| (id.clone(), e.hash.clone()))
        .collect();
    for (id, hash) in wanted {
        let Some(s) = symbols.get(&id) else { continue };
        let name = item_name(ItemKind::Symbol, &id);
        if s.hash() != hash {
            failed.push((name, OTHER.into()));
            continue;
        }
        match store::write_vendored_symbol(&open.dir, s) {
            Ok(_) => {
                open.symbols.insert(id, s.clone());
            }
            Err(e) => {
                // A loaded version the lock no longer names is not drawn as if it were the
                // session's; the next frame tries the write again.
                open.symbols.remove(&id);
                failed.push((name, e.to_string()));
            }
        }
    }
    let wanted: Vec<(String, String)> = open
        .project
        .lock
        .parts
        .iter()
        .filter(|(id, e)| open.parts.get(*id).is_none_or(|p| p.hash() != e.hash))
        .map(|(id, e)| (id.clone(), e.hash.clone()))
        .collect();
    for (id, hash) in wanted {
        let Some(p) = parts.get(&id) else { continue };
        let name = item_name(ItemKind::Part, &id);
        if p.hash() != hash {
            failed.push((name, OTHER.into()));
            continue;
        }
        match store::write_vendored_part(&open.dir, p) {
            Ok(_) => {
                open.parts.insert(id, p.clone());
            }
            Err(e) => {
                open.parts.remove(&id);
                failed.push((name, e.to_string()));
            }
        }
    }
    failed
}

/// The project's vendored files in `dir`, as read for a session.
struct Files {
    symbols: BTreeMap<String, Symbol>,
    parts: BTreeMap<String, Part>,
    /// Locked items whose file cannot be shared, unreadable or not the locked version:
    /// `symbol id (reason)`.
    unreadable: Vec<String>,
}

/// The project's vendored symbol and part files in `dir`, for every item its lock lists; the
/// ones that cannot be read or hold another version than the lock are named with the reason.
fn vendored_files(dir: &Path, project: &Project) -> Files {
    let mut files = Files {
        symbols: BTreeMap::new(),
        parts: BTreeMap::new(),
        unreadable: Vec::new(),
    };
    for (id, e) in &project.lock.symbols {
        match check_locked(store::read_vendored_symbol(dir, id), Symbol::hash, &e.hash) {
            Ok(s) => {
                files.symbols.insert(id.clone(), s);
            }
            Err(e) => files
                .unreadable
                .push(format!("{} ({e})", item_name(ItemKind::Symbol, id))),
        }
    }
    for (id, e) in &project.lock.parts {
        match check_locked(store::read_vendored_part(dir, id), Part::hash, &e.hash) {
            Ok(p) => {
                files.parts.insert(id.clone(), p);
            }
            Err(e) => files
                .unreadable
                .push(format!("{} ({e})", item_name(ItemKind::Part, id))),
        }
    }
    files
}

/// The host's side: the session itself, the connections and the announcement.
pub struct Hosting {
    pub host: Host,
    hub: Hub,
    listener: Option<Listener>,
    announcer: Option<Announcer>,
    pub port: u16,
    dir: PathBuf,
}

/// A guest's side: its view of the session and the connection to the host.
pub struct Joining {
    pub guest: Guest,
    link: GuestLink,
    pub addr: String,
    /// Where this guest's copy of the project is kept (the folder made on joining).
    folder: PathBuf,
    /// The project folder once the welcome arrived and the copy was written.
    dir: Option<PathBuf>,
}

pub enum Role {
    Host(Box<Hosting>),
    Guest(Box<Joining>),
}

impl Role {
    fn me(&self) -> Option<PeerId> {
        match self {
            Role::Host(_) => Some(HOST),
            Role::Guest(j) => j.guest.me,
        }
    }

    fn locks(&self) -> BTreeMap<String, PeerId> {
        match self {
            Role::Host(h) => h.host.locks().clone(),
            Role::Guest(j) => j.guest.locks.clone(),
        }
    }

    pub fn peers(&self) -> Vec<Peer> {
        match self {
            Role::Host(h) => h.host.peers(),
            Role::Guest(j) => j.guest.peers.clone(),
        }
    }

    fn project(&self) -> Option<&Project> {
        match self {
            Role::Host(h) => Some(h.host.project()),
            Role::Guest(j) => j.guest.project.as_ref(),
        }
    }

    /// Whether an undo, redo or revert of this person's is still on its way to the host.
    fn taking_back(&self) -> bool {
        match self {
            Role::Host(_) => false,
            Role::Guest(j) => j.guest.taking_back(),
        }
    }

    /// The symbol and part files the session has, by id.
    fn files(&self) -> (&BTreeMap<String, Symbol>, &BTreeMap<String, Part>) {
        match self {
            Role::Host(h) => h.host.files(),
            Role::Guest(j) => (&j.guest.symbols, &j.guest.parts),
        }
    }

    fn dir(&self) -> Option<&Path> {
        match self {
            Role::Host(h) => Some(&h.dir),
            Role::Guest(j) => j.dir.as_deref(),
        }
    }

    /// Whether this person may edit `page` now: they hold its lock.
    fn holds(&self, page: &str) -> bool {
        match self {
            Role::Host(h) => h.host.locks().get(page) == Some(&HOST),
            Role::Guest(j) => j.guest.can_edit(page),
        }
    }

    fn open(&mut self, page: Option<&str>, notes: &mut Vec<String>) {
        match self {
            Role::Host(h) => {
                // A page someone else holds stays read-only here; that is no error.
                if let Err(e) = h.host.open(page)
                    && !matches!(e, wc_share::Refusal::PageLocked { .. })
                {
                    notes.push(format!("Could not open the page in the session: {e}"));
                }
            }
            Role::Guest(j) => {
                let msg = j.guest.open(page);
                send(&j.link, msg, notes);
            }
        }
    }

    fn edit_page(&mut self, page: Page, notes: &mut Vec<String>) {
        match self {
            Role::Host(h) => {
                if let Err(e) = h.host.edit_page(page) {
                    notes.push(format!("Your change was not kept: {e}"));
                }
            }
            Role::Guest(j) => match j.guest.edit_page(page) {
                Some(msg) => send(&j.link, msg, notes),
                None => notes.push("Your change was not kept: the page is not open by you yet.".into()),
            },
        }
    }

    fn edit_project(&mut self, edit: ProjectEdit, notes: &mut Vec<String>) {
        match self {
            Role::Host(h) => {
                if let Err(e) = h.host.edit_project(edit) {
                    notes.push(format!("Your change was not kept: {e}"));
                }
            }
            Role::Guest(j) => {
                // Applied to this copy at once (see `Guest::edit_project`); a refusal puts it
                // right again.
                let msg = j.guest.edit_project(edit);
                send(&j.link, msg, notes);
            }
        }
    }

    /// Move messages: accept connections, hand what arrived to the host or guest, send replies.
    /// Returns false once the session is over (the host closed it, or the connection dropped).
    fn pump(&mut self, notes: &mut Vec<String>) -> bool {
        match self {
            Role::Host(h) => {
                if let Some(l) = h.listener.as_mut() {
                    l.accept(&mut h.hub, &mut h.host);
                    for e in l.take_errors() {
                        crate::applog::warn(&format!("shared session: a connection failed: {e}"));
                    }
                }
                // Names before the pump, which forgets who disconnected.
                let names: std::collections::BTreeMap<_, _> =
                    h.host.peers().into_iter().map(|p| (p.id, p.name)).collect();
                h.hub.pump(&mut h.host);
                for (peer, reason) in h.hub.take_closed() {
                    let who = names
                        .get(&peer)
                        .cloned()
                        .unwrap_or_else(|| format!("participant {peer}"));
                    if reason.is_orderly() {
                        crate::applog::info(&format!("shared session: {who} disconnected: {reason}"));
                    } else {
                        crate::applog::warn(&format!("shared session: the connection to {who} failed: {reason}"));
                    }
                }
                for (change, reason) in h.host.take_journal_errors() {
                    notes.push(format!("\"{}\" could not be recorded: {reason}", change.message));
                }
                true
            }
            Role::Guest(j) => {
                loop {
                    match j.link.try_recv() {
                        Ok(Some(msg)) => j.guest.handle(msg),
                        Ok(None) => break,
                        Err(_) => {
                            // A host that declined or ended the session closes the connection
                            // after saying so; only an unexplained close is news.
                            if !matches!(j.guest.status, GuestStatus::Declined | GuestStatus::Closed) {
                                notes.push(closed_note(&j.link));
                                j.guest.status = GuestStatus::Closed;
                            }
                            break;
                        }
                    }
                }
                for r in std::mem::take(&mut j.guest.refusals) {
                    notes.push(format!("The host did not take a change: {r}"));
                }
                match j.guest.status {
                    GuestStatus::Declined => {
                        notes.push("The host declined your request to join.".into());
                        false
                    }
                    GuestStatus::Closed => {
                        notes.push("The shared session has ended.".into());
                        false
                    }
                    _ => true,
                }
            }
        }
    }

    fn history(&self) -> Vec<ChangeInfo> {
        match self {
            Role::Host(h) => h.host.history(),
            Role::Guest(j) => j.guest.history.clone(),
        }
    }

    /// Undo, redo, or take back one change from the list (`Some(id)` with `redo` false).
    /// Ask for a change to be taken back. The host answers at once, so its refusal comes back
    /// here; a guest's request is answered with a later sync.
    fn take_back(&mut self, redo: bool, change: Option<u64>, notes: &mut Vec<String>) -> Result<(), String> {
        match self {
            Role::Host(h) => {
                let result = match (redo, change) {
                    (_, Some(id)) => h.host.revert(id),
                    (true, None) => h.host.redo(),
                    (false, None) => h.host.undo(),
                };
                h.hub.flush(&mut h.host);
                result.map_err(|e| {
                    let e = format!("Nothing was taken back: {e}");
                    notes.push(e.clone());
                    e
                })
            }
            Role::Guest(j) => {
                let msg = match (redo, change) {
                    (_, Some(id)) => j.guest.revert(id),
                    (true, None) => j.guest.redo(),
                    (false, None) => j.guest.undo(),
                };
                j.link.send(msg).map_err(|_| {
                    let e = "The connection to the host closed.".to_owned();
                    notes.push(e.clone());
                    e
                })
            }
        }
    }

    /// Leave (a guest) or end (the host) the session.
    fn stop(&mut self) {
        match self {
            Role::Host(h) => {
                h.host.close();
                h.hub.flush(&mut h.host);
                if let Some(a) = h.announcer.take() {
                    a.stop();
                }
            }
            Role::Guest(j) => {
                let _ = j.link.send(GuestMsg::Leave);
            }
        }
    }
}

fn send(link: &GuestLink, msg: GuestMsg, notes: &mut Vec<String>) {
    if link.send(msg).is_err() {
        notes.push(closed_note(link));
    }
}

/// The note for a connection to the host that closed without the host saying so: why, when
/// the link knows, and the same in the log.
fn closed_note(link: &GuestLink) -> String {
    match link.close_reason() {
        Some(reason) if !reason.is_orderly() => {
            crate::applog::warn(&format!("shared session: the connection to the host failed: {reason}"));
            format!("The connection to the host failed: {reason}.")
        }
        _ => {
            crate::applog::info("shared session: the host closed the connection");
            "The connection to the host closed.".into()
        }
    }
}

/// Which session window is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum View {
    #[default]
    Closed,
    Share,
    Join,
    Status,
}

/// The fields of the share and join forms.
struct Form {
    name: String,
    email: String,
    addr: String,
    folder: String,
    browser: Option<Browser>,
    browse_error: Option<String>,
    sessions: Vec<wc_share::discovery::Session>,
    error: Option<String>,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            name: crate::state::author_name(),
            email: author_email(),
            addr: String::new(),
            folder: crate::prefs::projects_dir().join("Shared").display().to_string(),
            browser: None,
            browse_error: None,
            sessions: Vec::new(),
            error: None,
        }
    }
}

/// The shared session, if there is one, and its windows.
#[derive(Default)]
pub struct Share {
    pub role: Option<Role>,
    /// The open project as of the last sync.
    base: Option<Project>,
    /// The page last opened in the session, and when it was asked for.
    opened: Option<Option<String>>,
    asked_at: Option<std::time::Instant>,
    view: View,
    form: Option<Form>,
    /// Messages for the status line, oldest first.
    notes: Vec<String>,
    /// An undo, redo or revert was asked for since the last sync: its restoration wins over
    /// local changes made in the meantime to the same pages.
    took_back: bool,
    /// Symbols and parts sent to the session that it has not taken yet, by [`item_name`]. Other
    /// changes wait while one is on its way, so a page never reaches the session before what it
    /// draws.
    items_out: BTreeMap<String, ItemOut>,
    /// Items whose file could not be read, sent or taken, reported once each.
    unreadable: std::collections::BTreeSet<String>,
}

/// One symbol or part sent to the session and not taken yet.
#[derive(Debug, Clone, Copy)]
struct ItemOut {
    at: std::time::Instant,
    tries: u32,
    /// Not taken after [`ITEM_TRIES`] tries: changes no longer wait for it, and it is sent again
    /// only every [`ITEM_RETRY_LATER`].
    gave_up: bool,
}

/// How long an item sent to the session may take to be answered before it is sent again.
const ITEM_WAIT: std::time::Duration = if cfg!(test) {
    std::time::Duration::from_millis(100)
} else {
    std::time::Duration::from_secs(5)
};
/// How long a given-up item waits before it is sent again.
const ITEM_RETRY_LATER: std::time::Duration = if cfg!(test) {
    std::time::Duration::from_millis(300)
} else {
    std::time::Duration::from_secs(30)
};
/// How often an item is sent before changes stop waiting for it.
const ITEM_TRIES: u32 = 3;

impl Share {
    pub fn active(&self) -> bool {
        self.role.is_some()
    }

    /// While an assistant command owns the project folder, a host refuses guests' Undo, Redo
    /// and "Undo this change": each would write the folder through the session's history.
    pub fn set_command_running(&mut self, running: bool) {
        if let Some(Role::Host(h)) = self.role.as_mut() {
            h.host.hold_take_backs(running.then(|| {
                "the host's assistant is running a command on the project; try again once it has finished".to_owned()
            }));
        }
    }

    /// Show the share form, or the session window when a session is running.
    pub fn show_share(&mut self) {
        self.view = if self.active() { View::Status } else { View::Share };
        self.form.get_or_insert_with(Form::default).error = None;
    }

    /// Show the join form, or the session window when a session is running.
    pub fn show_join(&mut self) {
        self.view = if self.active() { View::Status } else { View::Join };
        let form = self.form.get_or_insert_with(Form::default);
        form.error = None;
        if self.view == View::Join && form.browser.is_none() {
            match Browser::open() {
                Ok(b) => {
                    form.browser = Some(b);
                    form.browse_error = None;
                }
                Err(e) => form.browse_error = Some(e.to_string()),
            }
        }
    }

    /// Whether Undo in the session has a change of this person's to take back.
    pub fn can_undo(&self) -> bool {
        self.role
            .as_ref()
            .and_then(|r| Some(undo_target(&r.history(), r.me()?)))
            .is_some_and(|t| t.is_some())
    }

    /// Whether this window hosts the session.
    pub fn is_host(&self) -> bool {
        matches!(self.role, Some(Role::Host(_)))
    }

    /// Whether Redo in the session has an undo of this person's to take back.
    pub fn can_redo(&self) -> bool {
        self.role
            .as_ref()
            .and_then(|r| Some(redo_target(&r.history(), r.me()?)))
            .is_some_and(|t| t.is_some())
    }

    /// Take back this person's own last change in the session. The host puts back only the
    /// parts that change touched, so other people's work stays; the result reaches this
    /// window with the next frame's sync.
    pub fn undo(&mut self) -> Result<(), String> {
        self.take_back(false, None)
    }

    /// Take back this person's own last undo in the session.
    pub fn redo(&mut self) -> Result<(), String> {
        self.take_back(true, None)
    }

    /// Take back one change from the session's list, also someone else's.
    pub fn revert(&mut self, change: u64) -> Result<(), String> {
        self.take_back(false, Some(change))
    }

    fn take_back(&mut self, redo: bool, change: Option<u64>) -> Result<(), String> {
        let role = self.role.as_mut().ok_or("Not in a shared session.")?;
        // Changes made since wait in the window until the first answer is back: a second undo
        // sent now would reach the host ahead of them and take back an older change instead.
        if role.taking_back() {
            return Err("Your last undo is still on its way to the session; try again once it has come back.".into());
        }
        role.take_back(redo, change, &mut self.notes)?;
        self.took_back = true;
        Ok(())
    }

    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// Who holds each page other than this person: page id to name.
    pub fn held_by_others(&self) -> BTreeMap<String, String> {
        let Some(role) = &self.role else {
            return BTreeMap::new();
        };
        let me = role.me();
        let peers = role.peers();
        role.locks()
            .into_iter()
            .filter(|(_, holder)| Some(*holder) != me)
            .map(|(page, holder)| {
                let name = peers
                    .iter()
                    .find(|p| p.id == holder)
                    .map_or_else(|| format!("participant {holder}"), |p| p.name.clone());
                (page, name)
            })
            .collect()
    }

    /// Start sharing the open project.
    pub fn start_host(&mut self, open: &mut Open, name: &str, email: &str) -> Result<(), String> {
        if open.dirty {
            open.save()?;
        }
        let journal = GitJournal::open(&open.dir).map_err(|e| format!("The project's history cannot be used: {e}"))?;
        let listener = Listener::bind(("0.0.0.0", SESSION_PORT))
            .or_else(|_| Listener::bind(("0.0.0.0", 0)))
            .map_err(|e| format!("No network port to share on: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let announcer = match Announcer::start(Beacon {
            project: open.project.meta.name.clone(),
            host: name.to_owned(),
            port,
        }) {
            Ok(a) => Some(a),
            Err(e) => {
                self.notes.push(format!(
                    "The session cannot be announced on the network ({e}); others can join by typing its address."
                ));
                None
            }
        };
        let mut host = Host::new(open.project.clone(), name, email, Box::new(journal));
        let files = vendored_files(&open.dir, &open.project);
        if !files.unreadable.is_empty() {
            self.notes.push(format!(
                "These files cannot be shared, so guests without the libraries cannot draw them: {}",
                files.unreadable.join(", ")
            ));
        }
        host.share_files(files.symbols, files.parts);
        self.role = Some(Role::Host(Box::new(Hosting {
            host,
            hub: Hub::new(),
            listener: Some(listener),
            announcer,
            port,
            dir: open.dir.clone(),
        })));
        self.base = Some(open.project.clone());
        self.forget_items();
        self.opened = None;
        crate::applog::info(&format!("shared session started on port {port}"));
        Ok(())
    }

    /// Ask to join the session at `addr`. The project arrives once the host approves.
    pub fn join(&mut self, addr: &str, name: &str, email: &str, folder: PathBuf) -> Result<(), String> {
        let addr = addr.trim();
        let target = if addr.contains(':') {
            addr.to_owned()
        } else {
            format!("{addr}:{SESSION_PORT}")
        };
        let link = net::connect(target.as_str()).map_err(|e| format!("Could not reach {target}: {e}"))?;
        let mut guest = Guest::new();
        let msg = guest.join(name, email);
        link.send(msg).map_err(|e| format!("Could not reach {target}: {e}"))?;
        self.role = Some(Role::Guest(Box::new(Joining {
            guest,
            link,
            addr: target,
            folder,
            dir: None,
        })));
        self.base = None;
        self.forget_items();
        self.opened = None;
        Ok(())
    }

    /// Items sent to a session, and problems reported about them, belong to that session.
    fn forget_items(&mut self) {
        self.items_out.clear();
        self.unreadable.clear();
    }

    /// Leave or end the session.
    pub fn stop(&mut self) {
        if let Some(mut role) = self.role.take() {
            role.stop();
            self.notes.push(match role {
                Role::Host(_) => "Stopped sharing.".into(),
                Role::Guest(_) => "Left the shared session.".into(),
            });
        }
        self.base = None;
        self.forget_items();
        self.opened = None;
        self.took_back = false;
        if self.view == View::Status {
            self.view = View::Closed;
        }
    }

    /// One frame: move messages, then sync the open project with the session. `page` is the
    /// page shown, and `busy` is true while an action is still going on (a drag, a live form).
    /// Returns a project to open instead of the current one: a guest's copy once it joined.
    pub fn frame(&mut self, open: Option<&mut Open>, page: Option<String>, busy: bool) -> Option<Open> {
        let role = self.role.as_mut()?;
        if !role.pump(&mut self.notes) {
            self.role = None;
            self.base = None;
            self.forget_items();
            return None;
        }

        // A guest that was just let in writes its copy and opens it, once the window is free:
        // never in the middle of an action or while an assistant command owns the files.
        if let Role::Guest(j) = role
            && !busy
            && j.dir.is_none()
            && j.guest.status == GuestStatus::Joined
            && let Some(project) = j.guest.project.as_ref()
        {
            return match write_copy(&j.folder, project, &j.guest.symbols, &j.guest.parts) {
                Ok(o) => {
                    j.dir = Some(o.dir.clone());
                    self.notes
                        .push(format!("Joined the session. Your copy is in {}.", o.dir.display()));
                    self.base = Some(o.project.clone());
                    self.opened = None;
                    Some(o)
                }
                Err(e) => {
                    self.notes.push(format!("Could not keep a copy of the project: {e}"));
                    self.stop();
                    None
                }
            };
        }

        let open = open?;
        // The session belongs to one project folder; opening another one leaves it.
        if role.dir().is_some_and(|d| d != open.dir) {
            self.stop();
            return None;
        }
        role.project()?;
        // Until the host's answer to an undo is here, a page built from this copy would still
        // hold what the undo puts back: changes wait in the window, unsent.
        if busy || role.taking_back() {
            return None;
        }
        // Open the shown page in the session; a page someone else held is asked for again once
        // they let it go.
        let free_again = page
            .as_deref()
            .is_some_and(|p| !role.holds(p) && !role.locks().contains_key(p))
            && self
                .asked_at
                .is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(1));
        if self.opened.as_ref() != Some(&page) || free_again {
            role.open(page.as_deref(), &mut self.notes);
            self.opened = Some(page.clone());
            self.asked_at = Some(std::time::Instant::now());
        }
        let base = self.base.get_or_insert_with(|| open.project.clone());
        if std::mem::take(&mut self.took_back)
            && let Some(auth) = role.project()
        {
            let lost = yield_to_session(&mut open.project, base, auth);
            if !lost.is_empty() {
                self.notes.push(format!(
                    "Your change to {} was not kept: the session changed it while your undo was on its way.",
                    lost.join(", ")
                ));
            }
        }
        let mine = page.as_deref().filter(|p| role.holds(p));
        let me = role.me().unwrap_or(HOST);
        // Symbols and parts used for the first time go out first. Until the session has them,
        // the changes that may draw them wait in the window: a page must not reach the session
        // without its symbols, and an item the session refused is sent again a few times.
        let auth = role.project()?;
        // An item the session took is done; one this copy no longer locks (an undo, or an
        // assistant command that rewrote the project) is not sent again, so nothing waits for it.
        self.items_out
            .retain(|name, _| !session_has(auth, name) && session_has(&open.project, name));
        for (name, out) in self.items_out.iter_mut() {
            if !out.gave_up && out.tries >= ITEM_TRIES && out.at.elapsed() >= ITEM_WAIT {
                out.gave_up = true;
                self.notes.push(format!(
                    "The session did not take {name}; your changes that use it are sent without it."
                ));
            }
        }
        let skip = self
            .items_out
            .iter()
            // One given up is still sent now and then, so it gets in once its file can be read
            // or the session can store it; changes no longer wait for it.
            .filter(|(_, o)| o.at.elapsed() < if o.gave_up { ITEM_RETRY_LATER } else { ITEM_WAIT })
            .map(|(n, _)| n.clone())
            .collect();
        let items = new_items(&open.dir, &open.project, auth, &skip);
        let now = std::time::Instant::now();
        for (name, why) in &items.unreadable {
            // A file that cannot be read counts as a try: the changes that may draw the item
            // wait for it as for a sent one, and go once the tries are used up.
            let out = self.items_out.entry(name.clone()).or_insert(ItemOut {
                at: now,
                tries: 0,
                gave_up: false,
            });
            out.at = now;
            out.tries += 1;
            if self.unreadable.insert(name.clone()) {
                self.notes.push(format!(
                    "The file of {name} could not be read ({why}); others cannot draw it until it can."
                ));
            }
        }
        for name in &items.sent {
            self.unreadable.remove(name);
            let out = self.items_out.entry(name.clone()).or_insert(ItemOut {
                at: now,
                tries: 0,
                gave_up: false,
            });
            out.at = now;
            out.tries += 1;
        }
        if let Some(edit) = items.edit {
            role.edit_project(edit, &mut self.notes);
            role.pump(&mut self.notes);
            let auth = role.project()?;
            self.items_out.retain(|name, _| !session_has(auth, name));
        }
        if self.items_out.values().any(|o| !o.gave_up) {
            return None;
        }
        let mut out = outgoing(&open.project, base, mine, &role.locks(), me);
        if !out.dropped.is_empty() {
            // One action is kept whole or not at all: a device renamed on two pages must not
            // change the device while the page someone else holds keeps the old tag.
            self.notes.push(format!(
                "Your change was not kept: it touched page {}, and in a shared session you can change only the page you have open.",
                out.dropped.join(", ")
            ));
            out.page = None;
            out.edits.clear();
        }
        let sent = out.page.is_some() || !out.edits.is_empty();
        for edit in out.edits {
            role.edit_project(edit, &mut self.notes);
        }
        if let Some(p) = out.page {
            role.edit_page(p, &mut self.notes);
        }
        if sent {
            // The host answers what was sent this frame at once; the reply goes out now.
            role.pump(&mut self.notes);
        }
        let auth = role.project()?;
        let conflicting = conflicts(&open.project, auth);
        if !conflicting.is_empty() {
            self.notes.push(format!(
                "Someone else used {} first in another version; your copy takes theirs.",
                conflicting.join(", ")
            ));
        }
        let changed = incoming(&mut open.project, auth);
        let (symbols, parts) = role.files();
        let failed: Vec<String> = take_files(open, symbols, parts)
            .into_iter()
            .filter(|(name, _)| self.unreadable.insert(name.clone()))
            .map(|(name, why)| format!("{name} ({why})"))
            .collect();
        if !failed.is_empty() {
            self.notes.push(format!(
                "Symbols and parts from the session could not be taken into your copy: {}",
                failed.join(", ")
            ));
        }
        if changed {
            // Others' changes are in the window, not yet in this copy's files.
            open.dirty = true;
        }
        if changed || sent || !out.dropped.is_empty() {
            open.forget_terminal_change();
            self.base = Some(open.project.clone());
        }
        None
    }

    /// The session windows: the share and join forms, the participants, and join requests.
    /// `busy`: an assistant command owns the project files, so nothing is taken back now.
    pub fn windows(&mut self, ctx: &egui::Context, open: Option<&mut Open>, busy: bool) {
        self.requests(ctx);
        match self.view {
            View::Closed => {}
            View::Share => self.share_window(ctx, open),
            View::Join => self.join_window(ctx),
            View::Status => self.status_window(ctx, busy),
        }
    }

    /// "Undo this change" from the session window. Refused while an assistant command owns the
    /// project files: the host's restore writes them. A refusal goes to the notes.
    pub(crate) fn ask_revert(&mut self, change: u64, busy: bool) {
        let r = if busy {
            Err("Undo waits until the assistant's command has finished.".to_owned())
        } else {
            self.revert(change)
        };
        if let Err(e) = r {
            self.notes.push(e);
        }
    }

    fn requests(&mut self, ctx: &egui::Context) {
        let Some(Role::Host(h)) = self.role.as_mut() else {
            return;
        };
        for (id, name, email) in h.host.pending() {
            egui::Window::new(format!("Join request {id}"))
                .title_bar(false)
                .collapsible(false)
                .resizable(false)
                .anchor(
                    egui::Align2::RIGHT_BOTTOM,
                    egui::vec2(-16.0, -40.0 - 90.0 * (id % 4) as f32),
                )
                .show(ctx, |ui| {
                    ui.label(RichText::new(format!("{name} wants to join")).strong());
                    ui.label(email.as_str());
                    ui.horizontal(|ui| {
                        if ui.button("Let in").clicked() {
                            h.host.approve(id);
                        }
                        if ui.button("Decline").clicked() {
                            h.host.decline(id);
                        }
                    });
                });
        }
        h.hub.flush(&mut h.host);
    }

    fn share_window(&mut self, ctx: &egui::Context, open: Option<&mut Open>) {
        let form = self.form.get_or_insert_with(Form::default);
        let mut keep = true;
        let mut start = false;
        egui::Window::new("Share session")
            .collapsible(false)
            .resizable(false)
            .open(&mut keep)
            .show(ctx, |ui| {
                ui.label("Colleagues on this network can ask to join. You let each one in.");
                ui.label("The page someone has open is read-only for the others.");
                egui::Grid::new("share_form").num_columns(2).show(ui, |ui| {
                    ui.label("Your name");
                    ui.text_edit_singleline(&mut form.name);
                    ui.end_row();
                    ui.label("Email");
                    ui.text_edit_singleline(&mut form.email);
                    ui.end_row();
                });
                if let Some(e) = &form.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                if open.is_none() {
                    ui.label("Open a project first.");
                }
                start = ui
                    .add_enabled(open.is_some(), egui::Button::new("Start sharing"))
                    .clicked();
            });
        if start && let Some(open) = open {
            let (name, email) = (form.name.clone(), form.email.clone());
            match self.start_host(open, &name, &email) {
                Ok(()) => self.view = View::Status,
                Err(e) => {
                    if let Some(f) = self.form.as_mut() {
                        f.error = Some(e);
                    }
                }
            }
        } else if !keep {
            self.view = View::Closed;
        }
    }

    fn join_window(&mut self, ctx: &egui::Context) {
        let form = self.form.get_or_insert_with(Form::default);
        if let Some(b) = form.browser.as_mut() {
            match b.poll() {
                Ok(list) => form.sessions = list,
                Err(e) => form.browse_error = Some(e.to_string()),
            }
            // Sessions come and go: look again shortly even when nothing moves.
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        let mut keep = true;
        let mut join = false;
        egui::Window::new("Join session")
            .collapsible(false)
            .resizable(false)
            .open(&mut keep)
            .show(ctx, |ui| {
                ui.label(RichText::new("Sessions on this network").strong());
                if form.sessions.is_empty() {
                    ui.label(match &form.browse_error {
                        Some(e) => format!("Sessions cannot be listed here ({e}). Type the host's address below."),
                        None => "None found yet. Over a VPN, type the host's address below.".into(),
                    });
                }
                for s in &form.sessions {
                    let label = format!("{}  ·  {}  ({})", s.project, s.host, s.addr);
                    if ui.selectable_label(form.addr == s.addr.to_string(), label).clicked() {
                        form.addr = s.addr.to_string();
                    }
                }
                ui.separator();
                egui::Grid::new("join_form").num_columns(2).show(ui, |ui| {
                    ui.label("Address");
                    ui.add(egui::TextEdit::singleline(&mut form.addr).hint_text("192.168.1.20:47812"));
                    ui.end_row();
                    ui.label("Your name");
                    ui.text_edit_singleline(&mut form.name);
                    ui.end_row();
                    ui.label("Email");
                    ui.text_edit_singleline(&mut form.email);
                    ui.end_row();
                    ui.label("Keep my copy in");
                    ui.text_edit_singleline(&mut form.folder);
                    ui.end_row();
                });
                if let Some(e) = &form.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                join = ui
                    .add_enabled(!form.addr.trim().is_empty(), egui::Button::new("Ask to join"))
                    .clicked();
            });
        if join {
            let (addr, name, email) = (form.addr.clone(), form.name.clone(), form.email.clone());
            let joined =
                crate::prefs::project_path(&form.folder).and_then(|folder| self.join(&addr, &name, &email, folder));
            match joined {
                Ok(()) => {
                    if let Some(f) = self.form.as_mut() {
                        f.browser = None;
                    }
                    self.view = View::Status;
                }
                Err(e) => {
                    if let Some(f) = self.form.as_mut() {
                        f.error = Some(e);
                    }
                }
            }
        } else if !keep {
            if let Some(f) = self.form.as_mut() {
                f.browser = None;
            }
            self.view = View::Closed;
        }
    }

    fn status_window(&mut self, ctx: &egui::Context, busy: bool) {
        let Some(role) = self.role.as_ref() else {
            self.view = View::Closed;
            return;
        };
        let mut keep = true;
        let mut stop = false;
        let mut revert = None;
        let history = role.history();
        let pages: BTreeMap<String, String> = role
            .project()
            .map(|p| p.pages.iter().map(|pg| (pg.id.clone(), pg.number.clone())).collect())
            .unwrap_or_default();
        egui::Window::new("Shared session")
            .collapsible(false)
            .resizable(false)
            .open(&mut keep)
            .show(ctx, |ui| {
                match role {
                    Role::Host(h) => {
                        let ip = local_ip().map_or_else(|| "this computer".to_owned(), |ip| ip.to_string());
                        ui.label(format!("Sharing. Others join at {ip}:{}.", h.port));
                        if h.announcer.is_none() {
                            ui.label("Not announced on the network: give them the address.");
                        }
                    }
                    Role::Guest(j) => {
                        ui.label(match j.guest.status {
                            GuestStatus::Waiting | GuestStatus::Idle => {
                                format!("Waiting for the host at {} to let you in.", j.addr)
                            }
                            _ => format!("Joined the session at {}.", j.addr),
                        });
                    }
                }
                ui.separator();
                ui.label(RichText::new("Participants").strong());
                let me = role.me();
                for p in role.peers() {
                    let page = p
                        .page
                        .as_ref()
                        .map(|id| format!(" · page {}", pages.get(id).unwrap_or(id)))
                        .unwrap_or_default();
                    let you = if Some(p.id) == me { " (you)" } else { "" };
                    ui.label(format!("{}{you}{page}", p.name));
                }
                ui.separator();
                ui.label(RichText::new("Changes").strong());
                if history.is_empty() {
                    ui.label("No changes yet.");
                }
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for c in history.iter().rev() {
                        ui.horizontal(|ui| {
                            let text = format!("{} · {}", c.name, c.message);
                            if c.undone_by.is_some() {
                                ui.label(RichText::new(text).strikethrough().weak());
                            } else {
                                ui.label(text);
                                let tip = match c.kind {
                                    ChangeKind::Edit => "Put back what this change touched, for everyone",
                                    _ => "Take this undo or redo back, for everyone",
                                };
                                let button = ui
                                    .add_enabled(!busy, egui::Button::new("Undo this change").small())
                                    .on_hover_text(tip)
                                    .on_disabled_hover_text("Waits until the assistant's command has finished");
                                if button.clicked() {
                                    revert = Some(c.id);
                                }
                            }
                        });
                    }
                });
                ui.separator();
                let label = match role {
                    Role::Host(_) => "Stop sharing",
                    Role::Guest(_) => "Leave",
                };
                stop = ui.button(label).clicked();
            });
        if let Some(id) = revert {
            self.ask_revert(id, busy);
        }
        if stop {
            self.stop();
        } else if !keep {
            self.view = View::Closed;
        }
    }
}

/// Write the session's project to a new folder under `folder` (named after the project) and
/// open it there, with the symbol and part files the session sent. Symbols it did not send are
/// taken from the libraries where they can be.
fn write_copy(
    folder: &Path,
    project: &Project,
    symbols: &BTreeMap<String, Symbol>,
    parts: &BTreeMap<String, Part>,
) -> Result<Open, String> {
    let leaf = crate::prefs::folder_name(&project.meta.name).unwrap_or_else(|| "shared-project".into());
    let dir = fresh_dir(folder, &leaf)?;
    store::save(&dir, project).map_err(|e| e.to_string())?;
    for s in symbols.values() {
        store::write_vendored_symbol(&dir, s).map_err(|e| e.to_string())?;
    }
    for p in parts.values() {
        store::write_vendored_part(&dir, p).map_err(|e| e.to_string())?;
    }
    let mut open = Open::load(&dir)?;
    let wanted: Vec<String> = open.project.lock.symbols.keys().cloned().collect();
    let mut missing = Vec::new();
    for id in wanted {
        if let Err(e) = open.ensure_symbol(&id) {
            missing.push(format!("{id} ({e})"));
        }
    }
    if !missing.is_empty() {
        crate::applog::warn(&format!(
            "shared session: symbols that could not be taken from the libraries here: {}",
            missing.join(", ")
        ));
    }
    Ok(open)
}

/// A new, empty folder `folder/leaf`, or `leaf (2)`, `leaf (3)`, ... when that name is taken, so
/// joining never writes over an earlier copy.
fn fresh_dir(folder: &Path, leaf: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    for n in 1..1000 {
        let name = if n == 1 {
            leaf.to_owned()
        } else {
            format!("{leaf} ({n})")
        };
        let dir = folder.join(name);
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        }
    }
    Err(format!("{}: no free folder name for {leaf}", folder.display()))
}

/// The address others reach this computer at on the local network, found by asking the system
/// which interface it would use (no packet is sent).
fn local_ip() -> Option<std::net::IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect(SocketAddr::from(([192, 0, 2, 1], 9))).ok()?;
    s.local_addr().ok().map(|a| a.ip()).filter(|ip| !ip.is_unspecified())
}

/// The email for the join request: `git config user.email`, or empty.
fn author_email() -> String {
    std::env::var("WIRINGCRAFT_EMAIL")
        .ok()
        .filter(|e| !e.trim().is_empty())
        .or_else(|| {
            std::process::Command::new("git")
                .args(["config", "user.email"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                .filter(|e| !e.is_empty())
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;

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

use std::collections::BTreeMap;
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use wc_core::{Page, Project, store};
use wc_share::discovery::{Announcer, Beacon, Browser};
use wc_share::net::{self, Listener};
use wc_share::transport::{GuestLink, Hub};
use wc_share::{ChangeInfo, ChangeKind, redo_target, undo_target};
use wc_share::{GitJournal, Guest, GuestMsg, GuestStatus, HOST, Host, Peer, PeerId, ProjectEdit, apply_edit};

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
        out.edits.push(ProjectEdit::SetMeta {
            meta: Box::new(local.meta.clone()),
        });
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
            out.edits.push(ProjectEdit::SetDevice {
                tag: tag.clone(),
                device: Some(d.clone()),
            });
        }
    }
    for tag in base.devices.keys() {
        if !local.devices.contains_key(tag) {
            out.edits.push(ProjectEdit::SetDevice {
                tag: tag.clone(),
                device: None,
            });
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
            out.edits.push(ProjectEdit::SetComments {
                page: page.to_owned(),
                comments: now,
            });
        }
    }

    // Panels.
    for p in &local.panels {
        if base.panels.iter().find(|b| b.id == p.id) != Some(p) {
            out.edits.push(ProjectEdit::SetPanel { panel: p.clone() });
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

/// Make `local` the session's project `auth`, changing only what differs. The symbol lock stays
/// local: each person's copy keeps its own symbol files. Returns whether anything changed.
pub fn incoming(local: &mut Project, auth: &Project) -> bool {
    let mut changed = false;
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
                // Applied to this copy at once; the host sends it back in its order, and applying
                // it again then gives the same result, so every copy ends up the same. Whole
                // pages wait for the host: it refuses them when someone else changed one first,
                // and applying them here would hide that change until the next join.
                if !matches!(edit, ProjectEdit::SetPages { .. })
                    && let Some(p) = j.guest.project.as_mut()
                {
                    apply_edit(p, &edit);
                }
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
                h.hub.pump(&mut h.host);
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
                                notes.push("The connection to the host closed.".into());
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
        notes.push("The connection to the host closed.".into());
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
}

impl Share {
    pub fn active(&self) -> bool {
        self.role.is_some()
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
        match self.role.as_mut() {
            Some(role) => role.take_back(false, None, &mut self.notes),
            None => Err("Not in a shared session.".into()),
        }
    }

    /// Take back this person's own last undo in the session.
    pub fn redo(&mut self) -> Result<(), String> {
        match self.role.as_mut() {
            Some(role) => role.take_back(true, None, &mut self.notes),
            None => Err("Not in a shared session.".into()),
        }
    }

    /// Take back one change from the session's list, also someone else's.
    pub fn revert(&mut self, change: u64) -> Result<(), String> {
        match self.role.as_mut() {
            Some(role) => role.take_back(false, Some(change), &mut self.notes),
            None => Err("Not in a shared session.".into()),
        }
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
        let host = Host::new(open.project.clone(), name, email, Box::new(journal));
        self.role = Some(Role::Host(Box::new(Hosting {
            host,
            hub: Hub::new(),
            listener: Some(listener),
            announcer,
            port,
            dir: open.dir.clone(),
        })));
        self.base = Some(open.project.clone());
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
        self.opened = None;
        Ok(())
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
        self.opened = None;
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
            return match write_copy(&j.folder, project) {
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
        if busy {
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
        let mine = page.as_deref().filter(|p| role.holds(p));
        let me = role.me().unwrap_or(HOST);
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
        let changed = incoming(&mut open.project, auth);
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
    pub fn windows(&mut self, ctx: &egui::Context, open: Option<&mut Open>) {
        self.requests(ctx);
        match self.view {
            View::Closed => {}
            View::Share => self.share_window(ctx, open),
            View::Join => self.join_window(ctx),
            View::Status => self.status_window(ctx),
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

    fn status_window(&mut self, ctx: &egui::Context) {
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
                                if ui.small_button("Undo this change").on_hover_text(tip).clicked() {
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
            // A refusal is in the notes.
            let _ = self.revert(id);
        }
        if stop {
            self.stop();
        } else if !keep {
            self.view = View::Closed;
        }
    }
}

/// Write the session's project to a new folder under `folder` (named after the project) and
/// open it there. Symbols the project uses are taken from the libraries where they can be.
fn write_copy(folder: &Path, project: &Project) -> Result<Open, String> {
    let leaf = crate::prefs::folder_name(&project.meta.name).unwrap_or_else(|| "shared-project".into());
    let dir = fresh_dir(folder, &leaf)?;
    store::save(&dir, project).map_err(|e| e.to_string())?;
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

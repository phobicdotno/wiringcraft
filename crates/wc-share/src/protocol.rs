//! The messages between a host and its guests, and how a project-level change is applied.
//! Everything is serde so a transport can send it as one JSON object per line.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use wc_core::lock::LockEntry;
use wc_core::panel::Panel;
use wc_core::{Comment, Device, DeviceTag, Page, Part, Project, ProjectMeta, Symbol};

use crate::{ChangeInfo, Parts};

/// A participant's number in a session. The host is always [`crate::HOST`].
pub type PeerId = u32;

/// The version of the messages below. A host lets in only guests on the same version (a build
/// before versions were sent counts as 0), since a message one side does not know cannot be
/// read by the other. Raise it whenever a message, edit or field is added or changed in a way
/// an older build cannot read.
pub const PROTOCOL_VERSION: u32 = 2;

/// The session protocol versions of a host and a guest that cannot work together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionMismatch {
    pub host: u32,
    pub guest: u32,
}

impl std::fmt::Display for VersionMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let newer = if self.host > self.guest { "host's" } else { "guest's" };
        write!(
            f,
            "the host's WiringCraft uses session protocol version {} and the guest's uses version {}; update the other side to the {newer} WiringCraft version",
            self.host, self.guest
        )
    }
}

/// The largest picture file a session carries (a logo or a page underlay), in bytes. It keeps
/// each message well under the connection's line limit ([`crate::net::MAX_LINE`]).
pub const MAX_PICTURE: usize = 16 * 1024 * 1024;
/// The most picture files participants can add to a session, beyond those the host shared.
pub const MAX_ADDED_PICTURES: usize = 200;
/// The most bytes all pictures participants add to a session can take together (4 MB in
/// tests, so the limit can be reached without writing hundreds of megabytes).
pub const MAX_ADDED_PICTURE_BYTES: usize = if cfg!(test) { 4 * 1024 * 1024 } else { 256 * 1024 * 1024 };

/// A picture file of the project (the logo in `pictures/`, a page underlay in `underlays/`), so
/// a participant without it can show it. Its bytes travel as base64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Picture {
    /// Path relative to the project folder, e.g. `pictures/logo.png`.
    pub file: String,
    #[serde(with = "base64_bytes")]
    pub data: Vec<u8>,
}

impl Picture {
    /// Whether `file` may be carried by a session: a plain relative path in the project's
    /// picture folders ([`wc_core::Logo::DIR`], [`wc_core::Underlay::DIR`]).
    pub fn file_ok(file: &str) -> bool {
        wc_core::project::relative_file_is_safe(file)
            && [wc_core::Logo::DIR, wc_core::Underlay::DIR]
                .iter()
                .any(|dir| file.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/')))
    }

    /// Why this picture cannot be carried, if it cannot.
    pub fn problem(&self) -> Option<String> {
        if !Self::file_ok(&self.file) {
            Some(format!(
                "picture {} is not in the project's {} or {} folder",
                self.file,
                wc_core::Logo::DIR,
                wc_core::Underlay::DIR
            ))
        } else if self.data.is_empty() {
            Some(format!("picture {} is empty", self.file))
        } else if self.data.len() > MAX_PICTURE {
            Some(format!(
                "picture {} is larger than {} MB",
                self.file,
                MAX_PICTURE / (1024 * 1024)
            ))
        } else {
            None
        }
    }
}

mod base64_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

/// A participant as the others see them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub id: PeerId,
    pub name: String,
    /// The page they have open, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
}

/// What a guest sends to the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GuestMsg {
    /// Ask to join. The host shows the name and email and approves or declines. A guest on
    /// another [`PROTOCOL_VERSION`] (0 from a build that did not send one) is declined at once.
    Join {
        name: String,
        email: String,
        #[serde(default)]
        protocol: u32,
    },
    /// Open a page (taking its lock), or `None` to close the open page (releasing it).
    Open { page: Option<String> },
    /// The new content of the page the guest holds the lock for.
    EditPage { page: Page },
    /// A change to project-level data. `seq` numbers this guest's project edits (from 1), so
    /// the host's answer can say which one it answers: every one gets exactly one answer, a
    /// [`HostMsg::ProjectChanged`] carrying it, a [`HostMsg::Unchanged`], or a
    /// [`HostMsg::Refused`] with the data to put back.
    EditProject {
        edit: ProjectEdit,
        #[serde(default)]
        seq: u64,
    },
    /// Take back this guest's own last change (see [`crate::undo_target`]).
    Undo,
    /// Take back this guest's own last undo (see [`crate::redo_target`]).
    Redo,
    /// Take back one change from the session's list, whoever made it.
    Revert { change: u64 },
    /// Leave the session; every lock is released.
    Leave,
}

/// A change to data shared by every page. Applied by the host one at a time, in arrival order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectEdit {
    /// Add or change a device, or remove it with `None`.
    SetDevice {
        tag: DeviceTag,
        device: Option<Device>,
        /// What the record was when the author last had it (see [`ProjectEdit::stamped`]): the
        /// host refuses the edit when the record has changed since. `None` is not checked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
    },
    /// Replace every review comment on one page.
    SetComments {
        page: String,
        comments: Vec<Comment>,
        /// What the record was when the author last had it (see [`ProjectEdit::stamped`]): the
        /// host refuses the edit when the record has changed since. `None` is not checked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
    },
    /// Add or replace a cabinet panel layout.
    SetPanel {
        panel: Panel,
        /// What the record was when the author last had it (see [`ProjectEdit::stamped`]): the
        /// host refuses the edit when the record has changed since. `None` is not checked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
    },
    /// Remove a cabinet panel layout.
    RemovePanel { id: String },
    /// Add a new page. Its id must be unused.
    AddPage { page: Page },
    /// Remove a page and its comments. Refused while someone else has it open.
    RemovePage { id: String },
    /// Give pages new display numbers: `(page id, number)`.
    Renumber { numbers: Vec<(String, String)> },
    /// Replace the project's properties (name, title block, locations, jumpers, settings).
    SetMeta {
        meta: Box<ProjectMeta>,
        /// What the record was when the author last had it (see [`ProjectEdit::stamped`]): the
        /// host refuses the edit when the record has changed since. `None` is not checked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
    },
    /// Replace the content of several existing pages at once, for a tool that changes pages
    /// other than the one the author has open (wire numbering, a device tag renamed on every
    /// page). Display numbers are kept: only `Renumber` changes them. `before` holds each page
    /// as the author last had it, in the same order: when one of them has changed on the host
    /// since, the edit is refused, so a copy that had not yet seen someone else's change cannot
    /// undo it. Also refused as a whole when someone else has one of the pages open.
    SetPages { pages: Vec<Page>, before: Vec<Page> },
    /// Put parts of the project back as they were: what an undo sends. Only the host makes it,
    /// after checking the parts are still as the undone change left them; sent as a plain edit
    /// (by a guest or the host's user) it is refused.
    Restore { parts: Box<Parts> },
    /// Add symbols and parts to the project: their vendored files, and lock entries naming the
    /// library source each came from (`(item, source)`). What a participant sends when they
    /// use a symbol or part the project did not have yet, so every copy can draw it. An item
    /// the project already has in another version is refused.
    AddItems {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        symbols: Vec<(Symbol, String)>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        parts: Vec<(Part, String)>,
    },
    /// Add a picture file to the project folder (a new logo, a page underlay), so every copy
    /// can show it: what a participant sends before the change that names it. It changes no
    /// project data; the host stores the file, which the next change's commit holds, and sends
    /// it to everyone as [`HostMsg::Picture`]. A picture the session has under the same name with
    /// other content is refused.
    AddPicture { picture: Picture },
    /// Several edits made by one user action (a device renamed on its pages and in the device
    /// list), checked and applied together in order: all of them or none, as one change in the
    /// history. Only [`ProjectEdit::batchable`] edits can be in it.
    Batch { edits: Vec<ProjectEdit> },
}

impl ProjectEdit {
    /// What the record a whole-record edit replaces (a device, one page's comments, a panel,
    /// the project properties) is in `project`, as a short content hash; `None` for other
    /// edits.
    pub fn record(&self, project: &Project) -> Option<String> {
        use wc_core::hash::content_hash;
        match self {
            Self::SetDevice { tag, .. } => Some(content_hash(&project.devices.get(tag))),
            Self::SetComments { page, .. } => {
                // By id, so the order the comments are kept in does not count.
                let mut of: Vec<&Comment> = project.comments.iter().filter(|c| &c.page == page).collect();
                of.sort_by(|a, b| a.id.cmp(&b.id));
                Some(content_hash(&of))
            }
            Self::SetPanel { panel, .. } => Some(content_hash(&project.panels.iter().find(|p| p.id == panel.id))),
            Self::SetMeta { .. } => Some(content_hash(&project.meta)),
            _ => None,
        }
    }

    /// This edit, carrying what its record was in `base`, the author's copy as it was before
    /// the change: the host then refuses it when someone else changed the record meanwhile,
    /// instead of letting a copy that had not seen that change overwrite it.
    pub fn stamped(mut self, base: &Project) -> Self {
        let was = self.record(base);
        match &mut self {
            Self::SetDevice { before, .. }
            | Self::SetComments { before, .. }
            | Self::SetPanel { before, .. }
            | Self::SetMeta { before, .. } => *before = was,
            _ => {}
        }
        self
    }

    /// The record as the author last had it, for an edit that carries it.
    fn before(&self) -> Option<&str> {
        match self {
            Self::SetDevice { before, .. }
            | Self::SetComments { before, .. }
            | Self::SetPanel { before, .. }
            | Self::SetMeta { before, .. } => before.as_deref(),
            _ => None,
        }
    }

    /// The record this edit replaces, in words, for a refusal.
    fn record_name(&self, project: &Project) -> String {
        match self {
            Self::SetDevice { tag, .. } => format!("device {tag}"),
            Self::SetComments { page, .. } => format!("the comments on page {}", now_number(project, page)),
            Self::SetPanel { panel, .. } => format!("panel {}", panel.id),
            Self::SetMeta { .. } => "the project properties".to_owned(),
            _ => String::new(),
        }
    }

    /// A one-line description for the commit message.
    pub fn describe(&self) -> String {
        match self {
            Self::SetDevice {
                tag, device: Some(_), ..
            } => format!("Change device {tag}"),
            Self::SetDevice { tag, device: None, .. } => format!("Remove device {tag}"),
            Self::SetComments { page, .. } => format!("Comments on page {page}"),
            Self::SetPanel { panel, .. } => format!("Change panel {}", panel.id),
            Self::RemovePanel { id } => format!("Remove panel {id}"),
            Self::AddPage { page } => format!("Add page {} {}", page.number, page.title),
            Self::RemovePage { id } => format!("Remove page {id}"),
            Self::Renumber { numbers } => format!("Renumber {} pages", numbers.len()),
            Self::SetMeta { .. } => "Change project properties".to_owned(),
            Self::SetPages { pages, .. } => match pages.as_slice() {
                [p] => format!("Edit page {} {}", p.number, p.title),
                _ => format!("Edit {} pages", pages.len()),
            },
            Self::Restore { .. } => "Put back an earlier state".to_owned(),
            Self::Batch { edits } => edits.iter().map(Self::describe).collect::<Vec<_>>().join("; "),
            Self::AddPicture { picture } => format!("Add picture {}", picture.file),
            Self::AddItems { symbols, parts } => {
                let names: Vec<&str> = symbols
                    .iter()
                    .map(|(s, _)| s.id.as_str())
                    .chain(parts.iter().map(|(p, _)| p.id.as_str()))
                    .collect();
                format!("Add {} to the project", names.join(", "))
            }
        }
    }

    /// Which parts of the project this change touches, as keys (the values are those of
    /// `project`): what a participant that applied it at once gets back when the host refuses
    /// it, to put its copy right. Whole-page changes and added symbols and parts are applied
    /// only when the host sends them, so they need nothing back.
    pub fn touches(&self, project: &Project) -> Parts {
        let mut keys = Parts::default();
        match self {
            Self::SetDevice { tag, .. } => {
                keys.devices.insert(tag.clone(), None);
            }
            Self::SetComments { page, .. } => {
                keys.comments.insert(page.clone(), Vec::new());
            }
            Self::SetPanel { panel, .. } => {
                keys.panels.insert(panel.id.clone(), None);
            }
            Self::RemovePanel { id } => {
                keys.panels.insert(id.clone(), None);
            }
            Self::AddPage { page } => {
                keys.pages.insert(page.id.clone(), None);
                keys.comments.insert(page.id.clone(), Vec::new());
            }
            Self::RemovePage { id } => {
                keys.pages.insert(id.clone(), None);
                keys.comments.insert(id.clone(), Vec::new());
            }
            Self::Renumber { numbers } => {
                for (id, _) in numbers {
                    keys.pages.insert(id.clone(), None);
                }
            }
            Self::SetMeta { .. } => keys.meta = Some(Box::new(project.meta.clone())),
            Self::Restore { parts } => keys = (**parts).clone(),
            Self::Batch { edits } => {
                // What the parts applied at once touch.
                for e in edits.iter().filter(|e| e.applied_at_once()) {
                    let t = e.touches(project);
                    keys.pages.extend(t.pages);
                    keys.devices.extend(t.devices);
                    keys.panels.extend(t.panels);
                    keys.comments.extend(t.comments);
                    if t.meta.is_some() {
                        keys.meta = t.meta;
                    }
                }
            }
            Self::SetPages { .. } | Self::AddItems { .. } | Self::AddPicture { .. } => {}
        }
        Parts::read(project, &keys)
    }

    /// Whether a participant applies this change to its own copy at once, before the host
    /// answers (see [`ProjectEdit::touches`]). Of a batch, the edits that are applied at once
    /// are (see [`ProjectEdit::apply_at_once`]); the batch itself counts as not.
    pub fn applied_at_once(&self) -> bool {
        !matches!(
            self,
            Self::SetPages { .. }
                | Self::AddItems { .. }
                | Self::AddPicture { .. }
                | Self::Restore { .. }
                | Self::Batch { .. }
        )
    }

    /// Whether any of this change is applied at once (for a batch: any of its edits).
    pub fn partly_applied_at_once(&self) -> bool {
        match self {
            Self::Batch { edits } => Self::well_formed(edits) && edits.iter().any(Self::applied_at_once),
            e => e.applied_at_once(),
        }
    }

    /// Whether these edits make a batch the host can accept: two or more, each batchable.
    fn well_formed(edits: &[ProjectEdit]) -> bool {
        edits.len() >= 2 && edits.iter().all(Self::batchable)
    }

    /// Apply to a participant's own copy what of this change is applied at once.
    pub fn apply_at_once(&self, project: &mut Project) {
        match self {
            // A batch the host will refuse whole (one that holds an edit that cannot be batched)
            // is not applied here either, so this copy stays as the host's.
            Self::Batch { edits } if Self::well_formed(edits) => {
                for e in edits.iter().filter(|e| e.applied_at_once()) {
                    apply_edit(project, e);
                }
            }
            e if e.applied_at_once() => apply_edit(project, e),
            _ => {}
        }
    }

    /// Whether this edit can be part of a [`ProjectEdit::Batch`]. Pages added or removed,
    /// symbols and parts added, and restores go on their own: they are tracked or stored apart.
    pub fn batchable(&self) -> bool {
        matches!(
            self,
            Self::SetDevice { .. }
                | Self::SetComments { .. }
                | Self::SetPanel { .. }
                | Self::RemovePanel { .. }
                | Self::Renumber { .. }
                | Self::SetMeta { .. }
                | Self::SetPages { .. }
        )
    }

    /// `edits` as they go to the host: those that cannot be batched first, on their own and in
    /// order, then the others together as one [`ProjectEdit::Batch`] (or alone when only one).
    pub fn batched(edits: Vec<ProjectEdit>) -> Vec<ProjectEdit> {
        let (mut batch, mut out): (Vec<_>, Vec<_>) = edits.into_iter().partition(Self::batchable);
        match batch.len() {
            0 => {}
            1 => out.append(&mut batch),
            _ => out.push(Self::Batch { edits: batch }),
        }
        out
    }

    /// The one page this change is about, if it is about one page.
    pub fn page(&self) -> Option<&str> {
        match self {
            Self::SetComments { page, .. } => Some(page),
            Self::AddPage { page } => Some(&page.id),
            Self::RemovePage { id } => Some(id),
            Self::SetPages { pages, .. } => match pages.as_slice() {
                [p] => Some(&p.id),
                _ => None,
            },
            _ => None,
        }
    }
}

/// Why the host did not do what a guest asked. Nothing changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Refusal {
    /// The guest has not been approved (yet).
    #[error("not approved by the host")]
    NotApproved,
    /// Someone else has the page open.
    #[error("page {page} is open by participant {by}")]
    PageLocked { page: String, by: PeerId },
    /// A page edit for a page the guest does not have open.
    #[error("page {page} is not open by you")]
    NoLock { page: String },
    #[error("no page {page}")]
    UnknownPage { page: String },
    /// The change is not valid (for example a duplicate page id).
    #[error("{reason}")]
    Invalid { reason: String },
    /// The change could not be written to the project's history, so it was undone.
    #[error("the change could not be recorded: {reason}")]
    NotRecorded { reason: String },
}

/// The whole project as sent to a guest when they join.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub meta: ProjectMeta,
    pub pages: Vec<Page>,
    pub devices: BTreeMap<DeviceTag, Device>,
    pub lock: wc_core::Lockfile,
    pub panels: Vec<Panel>,
    pub comments: Vec<Comment>,
    /// The vendored symbol files the host shares ([`crate::Host::share_files`]), by id, so a
    /// guest without the host's libraries can draw them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub symbols: BTreeMap<String, Symbol>,
    /// The vendored part files, by id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parts: BTreeMap<String, Part>,
}

impl From<&Project> for Snapshot {
    fn from(p: &Project) -> Self {
        Self {
            meta: p.meta.clone(),
            pages: p.pages.clone(),
            devices: p.devices.clone(),
            lock: p.lock.clone(),
            panels: p.panels.clone(),
            comments: p.comments.clone(),
            symbols: BTreeMap::new(),
            parts: BTreeMap::new(),
        }
    }
}

impl From<Snapshot> for Project {
    fn from(s: Snapshot) -> Self {
        Self {
            meta: s.meta,
            pages: s.pages,
            devices: s.devices,
            lock: s.lock,
            panels: s.panels,
            comments: s.comments,
        }
    }
}

/// What the host sends to a guest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMsg {
    /// The join request arrived and waits for the host's approval.
    Waiting,
    /// Approved: the guest's id, the project, who holds which page and who is in the session.
    Welcome {
        you: PeerId,
        project: Box<Snapshot>,
        locks: BTreeMap<String, PeerId>,
        peers: Vec<Peer>,
        /// The session's changes so far, for undo and the change list.
        #[serde(default)]
        history: Vec<ChangeInfo>,
        /// The host's [`PROTOCOL_VERSION`]; 0 from a build that did not send one.
        #[serde(default)]
        protocol: u32,
    },
    /// The host declined the join request, or (with `version`) the guest runs another
    /// [`PROTOCOL_VERSION`]. The connection should be closed. A build that does not know
    /// `version` still reads this as declined.
    Declined {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<VersionMismatch>,
    },
    /// A page changed (sent to everyone except the person who changed it).
    PageChanged { page: Page, by: PeerId },
    /// A project-level change was applied (sent to everyone, the author included, so all
    /// copies apply the same edits in the same order). The author's copy carries the `seq` of
    /// its [`GuestMsg::EditProject`].
    ProjectChanged {
        edit: ProjectEdit,
        by: PeerId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<u64>,
    },
    /// The guest's project edit `seq` was valid but changed nothing: the project already was
    /// that way.
    Unchanged { seq: u64 },
    /// A change was recorded in the history (sent to everyone after the change itself).
    Recorded { change: ChangeInfo },
    /// Who holds which page now: page id to participant.
    Locks { locks: BTreeMap<String, PeerId> },
    /// Who is in the session now.
    Peers { peers: Vec<Peer> },
    /// A request was not carried out. `take_back` is true when it was an undo, redo or revert,
    /// so the guest knows that request is answered and stops waiting for its result.
    Refused {
        refusal: Refusal,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        take_back: bool,
        /// For a refused project edit: its `seq`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edit: Option<u64>,
        /// For a refused project edit: what it touches, as the host has it, so the guest's
        /// copy (which applied the edit at once) matches the host's again.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        put_back: Option<Box<Parts>>,
    },
    /// A picture file of the project: one for each picture the project has, right after the
    /// [`HostMsg::Welcome`] (one message each, so a project with many underlays stays under the
    /// line limit), and each one a participant adds with [`ProjectEdit::AddPicture`].
    Picture { picture: Picture },
    /// The host ended the session.
    Closed,
}

/// Apply a project-level change. The host checks it first (`check_edit`); guests apply what
/// the host sends, so every copy ends up the same.
pub fn apply_edit(project: &mut Project, edit: &ProjectEdit) {
    match edit {
        ProjectEdit::SetDevice { tag, device, .. } => match device {
            Some(d) => {
                project.devices.insert(tag.clone(), d.clone());
            }
            None => {
                project.devices.remove(tag);
            }
        },
        ProjectEdit::SetComments { page, comments, .. } => {
            project.comments.retain(|c| &c.page != page);
            project.comments.extend(comments.iter().cloned().map(|mut c| {
                c.page = page.clone();
                c
            }));
            project.sort_comments();
        }
        ProjectEdit::SetPanel { panel, .. } => match project.panels.iter_mut().find(|p| p.id == panel.id) {
            Some(p) => *p = panel.clone(),
            None => {
                project.panels.push(panel.clone());
                project.panels.sort_by(|a, b| a.id.cmp(&b.id));
            }
        },
        ProjectEdit::RemovePanel { id } => project.panels.retain(|p| &p.id != id),
        ProjectEdit::AddPage { page } => {
            if project.page(&page.id).is_none() {
                project.pages.push(page.clone());
                project.sort_pages();
            }
        }
        ProjectEdit::RemovePage { id } => {
            project.pages.retain(|p| &p.id != id);
            project.comments.retain(|c| &c.page != id);
        }
        ProjectEdit::Renumber { numbers } => {
            for (id, number) in numbers {
                if let Some(p) = project.page_mut(id) {
                    p.number = number.clone();
                }
            }
            project.sort_pages();
        }
        ProjectEdit::SetMeta { meta, .. } => project.meta = (**meta).clone(),
        ProjectEdit::Restore { parts } => parts.apply(project),
        ProjectEdit::Batch { edits } => {
            for e in edits {
                apply_edit(project, e);
            }
        }
        ProjectEdit::AddItems { symbols, parts } => {
            for (s, source) in symbols {
                project.lock.symbols.insert(s.id.clone(), lock_entry(source, s.hash()));
            }
            for (p, source) in parts {
                project.lock.parts.insert(p.id.clone(), lock_entry(source, p.hash()));
            }
        }
        // The file is not project data: the host stores it, and each copy keeps it apart.
        ProjectEdit::AddPicture { .. } => {}
        ProjectEdit::SetPages { pages, .. } => {
            for page in pages {
                if let Some(p) = project.page_mut(&page.id) {
                    let number = std::mem::take(&mut p.number);
                    *p = page.clone();
                    p.number = number;
                }
            }
        }
    }
}

fn lock_entry(source: &str, hash: String) -> LockEntry {
    LockEntry {
        source: source.to_owned(),
        hash,
    }
}

/// Two versions of a page, ignoring the display number (only `Renumber` changes that). The
/// app uses the same rule to see what changed, so change detection and the host's stale check
/// agree.
pub fn same_but_number(a: &Page, b: &Page) -> bool {
    if a.number == b.number {
        return a == b;
    }
    let mut b = b.clone();
    b.number = a.number.clone();
    *a == b
}

fn now_number(project: &Project, id: &str) -> String {
    project.page(id).map_or_else(|| id.to_owned(), |p| p.number.clone())
}

/// Whether `edit` can be applied by `by`, given who holds which page.
pub(crate) fn check_edit(
    project: &Project,
    locks: &BTreeMap<String, PeerId>,
    by: PeerId,
    edit: &ProjectEdit,
) -> Result<(), Refusal> {
    let known = |page: &str| {
        if project.page(page).is_some() {
            Ok(())
        } else {
            Err(Refusal::UnknownPage { page: page.to_owned() })
        }
    };
    let invalid = |reason: String| Err(Refusal::Invalid { reason });
    if let Some(was) = edit.before()
        && edit.record(project).as_deref() != Some(was)
    {
        return invalid(format!(
            "{} was changed by someone else meanwhile; your change was not made, do it again on the new version",
            edit.record_name(project)
        ));
    }
    match edit {
        ProjectEdit::SetDevice { .. } => Ok(()),
        ProjectEdit::SetComments { page, comments, .. } => {
            known(page)?;
            let mut ids = std::collections::BTreeSet::new();
            for c in comments {
                let elsewhere = project.comments.iter().any(|o| o.id == c.id && &o.page != page);
                if elsewhere || !ids.insert(c.id.as_str()) {
                    return invalid(format!("comment id {} is used twice", c.id));
                }
            }
            Ok(())
        }
        ProjectEdit::SetPanel { panel, .. } => match wc_core::store::check_id(&panel.id) {
            Ok(()) => Ok(()),
            Err(e) => invalid(e.to_string()),
        },
        ProjectEdit::RemovePanel { .. } => Ok(()),
        ProjectEdit::AddPage { page } => {
            if let Err(e) = wc_core::store::check_id(&page.id) {
                return invalid(e.to_string());
            }
            if project.page(&page.id).is_some() {
                return invalid(format!("page id {} is already used", page.id));
            }
            Ok(())
        }
        ProjectEdit::RemovePage { id } => {
            known(id)?;
            match locks.get(id) {
                Some(&holder) if holder != by => Err(Refusal::PageLocked {
                    page: id.clone(),
                    by: holder,
                }),
                _ => Ok(()),
            }
        }
        ProjectEdit::Renumber { numbers } => {
            for (id, _) in numbers {
                known(id)?;
            }
            Ok(())
        }
        ProjectEdit::SetMeta { .. } => Ok(()),
        ProjectEdit::AddPicture { picture } => match picture.problem() {
            Some(why) => invalid(why),
            None => Ok(()),
        },
        ProjectEdit::AddItems { symbols, parts } => {
            if symbols.is_empty() && parts.is_empty() {
                return invalid("nothing to add".into());
            }
            let items = symbols
                .iter()
                .map(|(s, _)| ("symbol", &project.lock.symbols, s.id.as_str(), s.hash()))
                .chain(
                    parts
                        .iter()
                        .map(|(p, _)| ("part", &project.lock.parts, p.id.as_str(), p.hash())),
                );
            let mut seen = std::collections::BTreeSet::new();
            for (kind, lock, id, hash) in items {
                if let Err(e) = wc_core::store::check_id(id) {
                    return invalid(format!("{kind} {id}: {e}"));
                }
                if !seen.insert((kind, id)) {
                    return invalid(format!("{kind} {id} is listed twice"));
                }
                if lock.get(id).is_some_and(|e| e.hash != hash) {
                    return invalid(format!("the project already has {kind} {id} in another version"));
                }
            }
            Ok(())
        }
        ProjectEdit::Batch { edits } => {
            if edits.len() < 2 {
                return invalid("a batch holds two edits or more".into());
            }
            // Each is checked against the project as the ones before it leave it.
            let mut after = project.clone();
            for e in edits {
                if !e.batchable() {
                    return invalid(format!("{} cannot be part of a batch", e.describe()));
                }
                check_edit(&after, locks, by, e)?;
                apply_edit(&mut after, e);
            }
            Ok(())
        }
        ProjectEdit::Restore { parts } => {
            for (id, page) in &parts.pages {
                if let Some(page) = page
                    && (page.id != *id || wc_core::store::check_id(id).is_err())
                {
                    return invalid(format!("page id {id} is not valid"));
                }
                match locks.get(id) {
                    Some(&holder) if holder != by => {
                        return Err(Refusal::PageLocked {
                            page: id.clone(),
                            by: holder,
                        });
                    }
                    _ => {}
                }
            }
            for (id, panel) in &parts.panels {
                if let Some(panel) = panel
                    && (panel.id != *id || wc_core::store::check_id(id).is_err())
                {
                    return invalid(format!("panel id {id} is not valid"));
                }
            }
            Ok(())
        }
        ProjectEdit::SetPages { pages, before } => {
            if pages.len() != before.len() || pages.iter().zip(before).any(|(p, b)| p.id != b.id) {
                return invalid("the pages and what they were before do not match".into());
            }
            let mut ids = std::collections::BTreeSet::new();
            for (page, was) in pages.iter().zip(before) {
                known(&page.id)?;
                if !ids.insert(page.id.as_str()) {
                    return invalid(format!("page {} is listed twice", page.id));
                }
                if project.page(&page.id).is_some_and(|now| !same_but_number(now, was)) {
                    return invalid(format!(
                        "page {} was changed by someone else meanwhile; do it again on the new page",
                        now_number(project, &page.id)
                    ));
                }
                match locks.get(&page.id) {
                    Some(&holder) if holder != by => {
                        return Err(Refusal::PageLocked {
                            page: page.id.clone(),
                            by: holder,
                        });
                    }
                    _ => {}
                }
            }
            Ok(())
        }
    }
}

//! The messages between a host and its guests, and how a project-level change is applied.
//! Everything is serde so a transport can send it as one JSON object per line.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use wc_core::panel::Panel;
use wc_core::{Comment, Device, DeviceTag, Page, Project, ProjectMeta};

/// A participant's number in a session. The host is always [`crate::HOST`].
pub type PeerId = u32;

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
    /// Ask to join. The host shows the name and email and approves or declines.
    Join { name: String, email: String },
    /// Open a page (taking its lock), or `None` to close the open page (releasing it).
    Open { page: Option<String> },
    /// The new content of the page the guest holds the lock for.
    EditPage { page: Page },
    /// A change to project-level data.
    EditProject { edit: ProjectEdit },
    /// Leave the session; every lock is released.
    Leave,
}

/// A change to data shared by every page. Applied by the host one at a time, in arrival order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectEdit {
    /// Add or change a device, or remove it with `None`.
    SetDevice { tag: DeviceTag, device: Option<Device> },
    /// Replace every review comment on one page.
    SetComments { page: String, comments: Vec<Comment> },
    /// Add or replace a cabinet panel layout.
    SetPanel { panel: Panel },
    /// Remove a cabinet panel layout.
    RemovePanel { id: String },
    /// Add a new page. Its id must be unused.
    AddPage { page: Page },
    /// Remove a page and its comments. Refused while someone else has it open.
    RemovePage { id: String },
    /// Give pages new display numbers: `(page id, number)`.
    Renumber { numbers: Vec<(String, String)> },
}

impl ProjectEdit {
    /// A one-line description for the commit message.
    pub fn describe(&self) -> String {
        match self {
            Self::SetDevice { tag, device: Some(_) } => format!("Change device {tag}"),
            Self::SetDevice { tag, device: None } => format!("Remove device {tag}"),
            Self::SetComments { page, .. } => format!("Comments on page {page}"),
            Self::SetPanel { panel } => format!("Change panel {}", panel.id),
            Self::RemovePanel { id } => format!("Remove panel {id}"),
            Self::AddPage { page } => format!("Add page {} {}", page.number, page.title),
            Self::RemovePage { id } => format!("Remove page {id}"),
            Self::Renumber { numbers } => format!("Renumber {} pages", numbers.len()),
        }
    }

    /// The one page this change is about, if it is about one page.
    pub fn page(&self) -> Option<&str> {
        match self {
            Self::SetComments { page, .. } => Some(page),
            Self::AddPage { page } => Some(&page.id),
            Self::RemovePage { id } => Some(id),
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
    },
    /// The host declined the join request. The connection should be closed.
    Declined,
    /// A page changed (sent to everyone except the person who changed it).
    PageChanged { page: Page, by: PeerId },
    /// A project-level change was applied (sent to everyone, the author included, so all
    /// copies apply the same edits in the same order).
    ProjectChanged { edit: ProjectEdit, by: PeerId },
    /// Who holds which page now: page id to participant.
    Locks { locks: BTreeMap<String, PeerId> },
    /// Who is in the session now.
    Peers { peers: Vec<Peer> },
    /// A request was not carried out.
    Refused { refusal: Refusal },
    /// The host ended the session.
    Closed,
}

/// Apply a project-level change. The host checks it first ([`check_edit`]); guests apply what
/// the host sends, so every copy ends up the same.
pub fn apply_edit(project: &mut Project, edit: &ProjectEdit) {
    match edit {
        ProjectEdit::SetDevice { tag, device } => match device {
            Some(d) => {
                project.devices.insert(tag.clone(), d.clone());
            }
            None => {
                project.devices.remove(tag);
            }
        },
        ProjectEdit::SetComments { page, comments } => {
            project.comments.retain(|c| &c.page != page);
            project.comments.extend(comments.iter().cloned().map(|mut c| {
                c.page = page.clone();
                c
            }));
            project.sort_comments();
        }
        ProjectEdit::SetPanel { panel } => match project.panels.iter_mut().find(|p| p.id == panel.id) {
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
    }
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
    match edit {
        ProjectEdit::SetDevice { .. } => Ok(()),
        ProjectEdit::SetComments { page, comments } => {
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
        ProjectEdit::SetPanel { panel } => match wc_core::store::check_id(&panel.id) {
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
    }
}

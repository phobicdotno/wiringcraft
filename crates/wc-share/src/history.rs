//! Undo in a shared session: each change is kept as what it changed, so one person's change
//! can be taken back without touching anything someone else did after it.
//!
//! A change is stored as two [`Parts`] with the same keys: the parts of the project it
//! changed, as they were before and after. Undoing it puts the "before" parts back, but only
//! while every one of them is still as the change left it; when someone changed one of them
//! since, the undo is refused rather than throwing their work away.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use wc_core::panel::Panel;
use wc_core::{Comment, Device, DeviceTag, Page, Project, ProjectMeta};

use crate::PeerId;

/// Some parts of a project. A key with `None` means "not there" (a page or device that did
/// not exist). Comments are kept per page: every comment on that page.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Parts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Box<ProjectMeta>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pages: BTreeMap<String, Option<Page>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub devices: BTreeMap<DeviceTag, Option<Device>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub panels: BTreeMap<String, Option<Panel>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub comments: BTreeMap<String, Vec<Comment>>,
}

fn comments_on(project: &Project, page: &str) -> Vec<Comment> {
    project.comments.iter().filter(|c| c.page == page).cloned().collect()
}

impl Parts {
    pub fn is_empty(&self) -> bool {
        self.meta.is_none()
            && self.pages.is_empty()
            && self.devices.is_empty()
            && self.panels.is_empty()
            && self.comments.is_empty()
    }

    /// What differs between two versions of a project: `(before, after)`, with the same keys.
    /// The symbol lock is left out: each copy keeps its own.
    pub fn delta(before: &Project, after: &Project) -> (Parts, Parts) {
        let mut b = Parts::default();
        let mut a = Parts::default();
        if before.meta != after.meta {
            b.meta = Some(Box::new(before.meta.clone()));
            a.meta = Some(Box::new(after.meta.clone()));
        }
        let ids: BTreeSet<&str> = before.pages.iter().chain(&after.pages).map(|p| p.id.as_str()).collect();
        for id in ids {
            let (x, y) = (before.page(id), after.page(id));
            if x != y {
                b.pages.insert(id.to_owned(), x.cloned());
                a.pages.insert(id.to_owned(), y.cloned());
            }
        }
        let tags: BTreeSet<&DeviceTag> = before.devices.keys().chain(after.devices.keys()).collect();
        for tag in tags {
            let (x, y) = (before.devices.get(tag), after.devices.get(tag));
            if x != y {
                b.devices.insert(tag.clone(), x.cloned());
                a.devices.insert(tag.clone(), y.cloned());
            }
        }
        let panel_ids: BTreeSet<&str> = before
            .panels
            .iter()
            .chain(&after.panels)
            .map(|p| p.id.as_str())
            .collect();
        for id in panel_ids {
            let x = before.panels.iter().find(|p| p.id == id);
            let y = after.panels.iter().find(|p| p.id == id);
            if x != y {
                b.panels.insert(id.to_owned(), x.cloned());
                a.panels.insert(id.to_owned(), y.cloned());
            }
        }
        let pages: BTreeSet<&str> = before
            .comments
            .iter()
            .chain(&after.comments)
            .map(|c| c.page.as_str())
            .collect();
        for page in pages {
            let (x, y) = (comments_on(before, page), comments_on(after, page));
            if x != y {
                b.comments.insert(page.to_owned(), x);
                a.comments.insert(page.to_owned(), y);
            }
        }
        (b, a)
    }

    /// The same parts as `keys`, as they are in `project` now.
    pub fn read(project: &Project, keys: &Parts) -> Parts {
        Parts {
            meta: keys.meta.as_ref().map(|_| Box::new(project.meta.clone())),
            pages: keys
                .pages
                .keys()
                .map(|id| (id.clone(), project.page(id).cloned()))
                .collect(),
            devices: keys
                .devices
                .keys()
                .map(|tag| (tag.clone(), project.devices.get(tag).cloned()))
                .collect(),
            panels: keys
                .panels
                .keys()
                .map(|id| (id.clone(), project.panels.iter().find(|p| &p.id == id).cloned()))
                .collect(),
            comments: keys
                .comments
                .keys()
                .map(|page| (page.clone(), comments_on(project, page)))
                .collect(),
        }
    }

    /// Put these parts into `project`.
    pub fn apply(&self, project: &mut Project) {
        if let Some(meta) = &self.meta {
            project.meta = (**meta).clone();
        }
        if !self.pages.is_empty() {
            for (id, page) in &self.pages {
                match (project.page_mut(id), page) {
                    (Some(p), Some(page)) => *p = page.clone(),
                    (None, Some(page)) => project.pages.push(page.clone()),
                    (_, None) => project.pages.retain(|p| &p.id != id),
                }
            }
            project.sort_pages();
        }
        for (tag, device) in &self.devices {
            match device {
                Some(d) => {
                    project.devices.insert(tag.clone(), d.clone());
                }
                None => {
                    project.devices.remove(tag);
                }
            }
        }
        if !self.panels.is_empty() {
            for (id, panel) in &self.panels {
                project.panels.retain(|p| &p.id != id);
                if let Some(p) = panel {
                    project.panels.push(p.clone());
                }
            }
            project.panels.sort_by(|a, b| a.id.cmp(&b.id));
        }
        if !self.comments.is_empty() {
            for (page, comments) in &self.comments {
                project.comments.retain(|c| &c.page != page);
                project.comments.extend(comments.iter().cloned().map(|mut c| {
                    c.page = page.clone();
                    c
                }));
            }
            project.sort_comments();
        }
    }
}

/// What a recorded change was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// Someone's own edit.
    Edit,
    /// Took back an earlier change (Undo, or "Undo this change" in the list).
    Undo,
    /// Took back an undo (Redo).
    Redo,
}

/// One recorded change of a session, as everyone sees it in the session's change list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeInfo {
    /// Numbered from 1 in the order the host applied the changes.
    pub id: u64,
    pub by: PeerId,
    /// The name of who made it.
    pub name: String,
    /// The commit message.
    pub message: String,
    /// The page it is about, when it is about one page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    pub kind: ChangeKind,
    /// For an undo or redo: the change it took back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverts: Option<u64>,
    /// The change that took this one back, once it was undone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undone_by: Option<u64>,
}

/// The change `me` takes back with Undo: their latest edit (or redo) not undone yet.
pub fn undo_target(history: &[ChangeInfo], me: PeerId) -> Option<u64> {
    history
        .iter()
        .rev()
        .find(|c| c.by == me && c.kind != ChangeKind::Undo && c.undone_by.is_none())
        .map(|c| c.id)
}

/// The undo `me` takes back with Redo: their latest undo not taken back yet, as long as they
/// have made no new edit since (a new edit ends what can be redone, as outside a session).
pub fn redo_target(history: &[ChangeInfo], me: PeerId) -> Option<u64> {
    for c in history.iter().rev().filter(|c| c.by == me) {
        match c.kind {
            ChangeKind::Edit => return None,
            ChangeKind::Undo if c.undone_by.is_none() => return Some(c.id),
            _ => {}
        }
    }
    None
}

/// Note a newly recorded change in a list: it is added, and the change it took back is marked.
pub fn note(history: &mut Vec<ChangeInfo>, info: ChangeInfo) {
    if let Some(r) = info.reverts
        && let Some(c) = history.iter_mut().find(|c| c.id == r)
    {
        c.undone_by = Some(info.id);
    }
    history.push(info);
    if history.len() > MAX_HISTORY {
        history.drain(..history.len() - MAX_HISTORY);
    }
}

/// How many changes a session keeps for undo.
pub const MAX_HISTORY: usize = 500;

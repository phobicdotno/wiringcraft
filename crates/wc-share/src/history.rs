//! Undo in a shared session: each change is kept as what it changed, so one person's change
//! can be taken back without touching anything someone else did after it.
//!
//! A change is stored as two [`Parts`] with the same keys: the parts of the project it
//! changed, as they were before and after. Undoing it puts the "before" parts back, but only
//! while every one of them is still as the change left it; when someone changed one of them
//! since, the undo is refused rather than throwing their work away.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use wc_core::lock::{ItemKind, LockEntry, Lockfile};
use wc_core::panel::Panel;
use wc_core::{Comment, Device, DeviceTag, Page, Project, ProjectMeta};

use crate::PeerId;

/// Some parts of a project. A key with `None` means "not there" (a page or device that did
/// not exist). Comments are kept per page: every comment on that page. Lock entries of
/// symbols and parts are kept by id (`None`: the project did not lock it), so taking back a
/// change that added an item takes its lock entry back too; the vendored file stays.
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lock_symbols: BTreeMap<String, Option<LockEntry>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lock_parts: BTreeMap<String, Option<LockEntry>>,
}

/// The version (lock hash) of each symbol and part that some [`Parts`] use, by kind and id:
/// the versions their pages, devices and jumpers were drawn with.
pub type Uses = BTreeMap<(ItemKind, String), String>;

fn kind_name(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Symbol => "symbol",
        ItemKind::Part => "part",
    }
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
            && self.lock_symbols.is_empty()
            && self.lock_parts.is_empty()
    }

    /// The lock entries of one kind of item.
    pub fn lock(&self, kind: ItemKind) -> &BTreeMap<String, Option<LockEntry>> {
        match kind {
            ItemKind::Symbol => &self.lock_symbols,
            ItemKind::Part => &self.lock_parts,
        }
    }

    fn lock_mut(&mut self, kind: ItemKind) -> &mut BTreeMap<String, Option<LockEntry>> {
        match kind {
            ItemKind::Symbol => &mut self.lock_symbols,
            ItemKind::Part => &mut self.lock_parts,
        }
    }

    /// What differs between two versions of a project: `(before, after)`, with the same keys.
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
        // A page added or removed keeps its comments in the record even when they did not
        // change, so that taking it back is refused once someone has commented on it since,
        // instead of leaving that comment on a page that is gone.
        let added_or_removed: Vec<String> = b
            .pages
            .iter()
            .filter(|(id, x)| x.is_none() || a.pages.get(*id).is_some_and(Option::is_none))
            .map(|(id, _)| id.clone())
            .collect();
        let pages: BTreeSet<&str> = before
            .comments
            .iter()
            .chain(&after.comments)
            .map(|c| c.page.as_str())
            .chain(added_or_removed.iter().map(String::as_str))
            .collect();
        for page in pages {
            let (x, y) = (comments_on(before, page), comments_on(after, page));
            if x != y || added_or_removed.iter().any(|p| p == page) {
                b.comments.insert(page.to_owned(), x);
                a.comments.insert(page.to_owned(), y);
            }
        }
        for kind in [ItemKind::Symbol, ItemKind::Part] {
            let (x, y) = (before.lock.map(kind), after.lock.map(kind));
            let ids: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for id in ids {
                if x.get(id) != y.get(id) {
                    b.lock_mut(kind).insert(id.clone(), x.get(id).cloned());
                    a.lock_mut(kind).insert(id.clone(), y.get(id).cloned());
                }
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
            lock_symbols: read_lock(project, keys, ItemKind::Symbol),
            lock_parts: read_lock(project, keys, ItemKind::Part),
        }
    }

    /// Add `newer` to these parts: where both name a record, `newer`'s version stays.
    pub fn merge(&mut self, newer: Parts) {
        if newer.meta.is_some() {
            self.meta = newer.meta;
        }
        self.pages.extend(newer.pages);
        self.devices.extend(newer.devices);
        self.panels.extend(newer.panels);
        self.comments.extend(newer.comments);
        self.lock_symbols.extend(newer.lock_symbols);
        self.lock_parts.extend(newer.lock_parts);
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
        for kind in [ItemKind::Symbol, ItemKind::Part] {
            for (id, entry) in self.lock(kind) {
                match entry {
                    Some(e) => {
                        project.lock.map_mut(kind).insert(id.clone(), e.clone());
                    }
                    None => {
                        project.lock.map_mut(kind).remove(id);
                    }
                }
            }
        }
    }

    /// The versions in `lock` of the symbols and parts that these parts' pages, devices and
    /// jumpers use. One `lock` does not lock is left out.
    pub fn uses(&self, lock: &Lockfile) -> Uses {
        let mut probe = Project::new("");
        Parts {
            meta: self.meta.clone(),
            pages: self.pages.clone(),
            devices: self.devices.clone(),
            ..Parts::default()
        }
        .apply(&mut probe);
        let symbols = probe.used_symbols().into_iter().map(|id| (ItemKind::Symbol, id));
        let parts = probe.used_parts().into_iter().map(|id| (ItemKind::Part, id));
        symbols
            .chain(parts)
            .filter_map(|(kind, id)| {
                let hash = lock.map(kind).get(&id)?.hash.clone();
                Some(((kind, id), hash))
            })
            .collect()
    }

    /// Items of `recorded` that `lock` locks in another version (`symbol id` or `part id`):
    /// parts drawn with those versions are not to come back against these.
    pub fn other_versions(recorded: &Uses, lock: &Lockfile) -> Vec<String> {
        recorded
            .iter()
            .filter(|((kind, id), hash)| lock.map(*kind).get(id).is_some_and(|e| e.hash != **hash))
            .map(|((kind, id), _)| format!("{} {id}", kind_name(*kind)))
            .collect()
    }

    /// Items that putting these parts into `project` would leave used without a lock entry
    /// (`symbol id` or `part id`): a lock entry taken out while a page, device or jumper still
    /// uses the item, or a page or device brought back that uses an item no longer locked.
    /// Items already used without a lock before are not counted.
    pub fn unlocked_uses(&self, project: &Project) -> Vec<String> {
        let mut after = project.clone();
        self.apply(&mut after);
        let unlocked = |p: &Project| -> BTreeSet<String> {
            p.used_symbols()
                .into_iter()
                .filter(|id| !p.lock.symbols.contains_key(id))
                .map(|id| format!("symbol {id}"))
                .chain(
                    p.used_parts()
                        .into_iter()
                        .filter(|id| !p.lock.parts.contains_key(id))
                        .map(|id| format!("part {id}")),
                )
                .collect()
        };
        let had = unlocked(project);
        unlocked(&after).into_iter().filter(|i| !had.contains(i)).collect()
    }
}

fn read_lock(project: &Project, keys: &Parts, kind: ItemKind) -> BTreeMap<String, Option<LockEntry>> {
    keys.lock(kind)
        .keys()
        .map(|id| (id.clone(), project.lock.map(kind).get(id).cloned()))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use wc_core::PlacedSymbol;
    use wc_core::geom::Point;

    fn info(id: u64, by: PeerId, kind: ChangeKind, reverts: Option<u64>) -> ChangeInfo {
        ChangeInfo {
            id,
            by,
            name: format!("peer {by}"),
            message: format!("change {id}"),
            page: None,
            kind,
            reverts,
            undone_by: None,
        }
    }

    fn entry(hash: &str) -> LockEntry {
        LockEntry {
            source: "test".into(),
            hash: hash.into(),
        }
    }

    /// A page that places `symbol` once.
    fn page_using(id: &str, symbol: &str) -> Page {
        let mut page = Page::new(id, id, "Page");
        page.symbols.push(PlacedSymbol {
            id: "s1".into(),
            symbol: symbol.into(),
            at: Point::new(10.0, 10.0),
            rotation: Default::default(),
            mirror: false,
            tag: None,
        });
        page
    }

    #[test]
    fn undo_takes_back_my_latest_change_not_yet_undone() {
        let mut history = Vec::new();
        note(&mut history, info(1, 1, ChangeKind::Edit, None));
        note(&mut history, info(2, 1, ChangeKind::Edit, None));
        note(&mut history, info(3, 2, ChangeKind::Edit, None));
        // Someone else's later change is not mine to undo.
        assert_eq!(undo_target(&history, 1), Some(2));
        assert_eq!(undo_target(&history, 2), Some(3));
        assert_eq!(undo_target(&history, 9), None);

        // Once 2 is undone, the next undo goes to 1; the undo itself is never a target.
        note(&mut history, info(4, 1, ChangeKind::Undo, Some(2)));
        assert_eq!(undo_target(&history, 1), Some(1));
        // A redo is undone like an edit.
        note(&mut history, info(5, 1, ChangeKind::Redo, Some(4)));
        assert_eq!(undo_target(&history, 1), Some(5));
    }

    #[test]
    fn redo_takes_back_my_latest_undo_until_a_new_edit() {
        let mut history = Vec::new();
        note(&mut history, info(1, 1, ChangeKind::Edit, None));
        note(&mut history, info(2, 1, ChangeKind::Edit, None));
        assert_eq!(redo_target(&history, 1), None, "nothing undone yet");

        note(&mut history, info(3, 1, ChangeKind::Undo, Some(2)));
        note(&mut history, info(4, 1, ChangeKind::Undo, Some(1)));
        // Another peer's edit does not end my redo.
        note(&mut history, info(5, 2, ChangeKind::Edit, None));
        assert_eq!(redo_target(&history, 1), Some(4));

        // Redoing 4 leaves 3 to redo next.
        note(&mut history, info(6, 1, ChangeKind::Redo, Some(4)));
        assert_eq!(redo_target(&history, 1), Some(3));

        // My own new edit ends what can be redone.
        note(&mut history, info(7, 1, ChangeKind::Edit, None));
        assert_eq!(redo_target(&history, 1), None);
    }

    #[test]
    fn note_marks_what_it_took_back_and_keeps_the_latest_changes() {
        let mut history = Vec::new();
        note(&mut history, info(1, 1, ChangeKind::Edit, None));
        note(&mut history, info(2, 1, ChangeKind::Undo, Some(1)));
        assert_eq!(history.first().and_then(|c| c.undone_by), Some(2));
        // Taking back a change no longer kept is still noted.
        note(&mut history, info(3, 1, ChangeKind::Undo, Some(99)));
        assert_eq!(history.len(), 3);

        let mut long = Vec::new();
        for id in 1..=(MAX_HISTORY as u64 + 3) {
            note(&mut long, info(id, 1, ChangeKind::Edit, None));
        }
        assert_eq!(long.len(), MAX_HISTORY);
        assert_eq!(long.first().map(|c| c.id), Some(4), "the oldest are dropped");
        assert_eq!(long.last().map(|c| c.id), Some(MAX_HISTORY as u64 + 3));
    }

    #[test]
    fn merge_keeps_the_newer_record_and_the_older_meta_when_newer_has_none() {
        let mut older = Parts {
            meta: Some(Box::new(Project::new("Old").meta)),
            ..Parts::default()
        };
        older.pages.insert("p1".into(), None);
        older.pages.insert("p2".into(), Some(Page::new("p2", "2", "Old")));
        older.lock_symbols.insert("relay".into(), Some(entry("v1")));

        let mut newer = Parts::default();
        newer.pages.insert("p2".into(), Some(Page::new("p2", "2", "New")));
        newer.lock_symbols.insert("relay".into(), None);
        newer.lock_parts.insert("k1".into(), Some(entry("k")));
        older.merge(newer);

        assert_eq!(older.meta.as_ref().map(|m| m.name.as_str()), Some("Old"));
        assert_eq!(older.pages.get("p1"), Some(&None));
        assert!(matches!(older.pages.get("p2"), Some(Some(p)) if p.title == "New"));
        assert_eq!(older.lock_symbols.get("relay"), Some(&None));
        assert_eq!(older.lock_parts.get("k1"), Some(&Some(entry("k"))));
        assert!(!older.is_empty());
    }

    #[test]
    fn a_new_page_records_its_comments_even_when_there_are_none() {
        let before = Project::new("Demo");
        let mut after = before.clone();
        after.pages.push(Page::new("p1", "1", "Cover"));
        let (was, now) = Parts::delta(&before, &after);
        assert_eq!(was.comments.get("p1"), Some(&Vec::new()));
        assert_eq!(now.comments.get("p1"), Some(&Vec::new()));

        // Someone comments on it later: reading the same keys now differs from the record.
        let mut later = after.clone();
        later.comments.push(Comment {
            id: "c1".into(),
            page: "p1".into(),
            at: None,
            author: "Kari".into(),
            time: 0,
            text: "hm".into(),
            resolved: false,
            replies: Vec::new(),
        });
        assert_ne!(Parts::read(&later, &now), now);
        assert_eq!(Parts::read(&after, &now), now);
    }

    #[test]
    fn lock_entries_round_trip_through_delta_and_apply() {
        let mut before = Project::new("Demo");
        before.lock.symbols.insert("relay".into(), entry("v1"));
        before.lock.parts.insert("gone".into(), entry("g"));
        let mut after = before.clone();
        after.lock.symbols.insert("relay".into(), entry("v2"));
        after.lock.parts.remove("gone");
        after.lock.parts.insert("new".into(), entry("n"));

        let (was, now) = Parts::delta(&before, &after);
        assert_eq!(was.lock(ItemKind::Part).get("new"), Some(&None));
        assert_eq!(now.lock(ItemKind::Part).get("gone"), Some(&None));
        assert_eq!(was.lock(ItemKind::Symbol).len(), 1);

        let mut back = after.clone();
        was.apply(&mut back);
        assert_eq!(back, before);
        let mut forward = before.clone();
        now.apply(&mut forward);
        assert_eq!(forward, after);
    }

    #[test]
    fn other_versions_names_only_items_locked_in_another_version() {
        let mut lock = Lockfile::default();
        lock.symbols.insert("same".into(), entry("a"));
        lock.symbols.insert("moved".into(), entry("b2"));
        let mut recorded = Uses::new();
        recorded.insert((ItemKind::Symbol, "same".into()), "a".into());
        recorded.insert((ItemKind::Symbol, "moved".into()), "b1".into());
        recorded.insert((ItemKind::Part, "unlocked".into()), "c".into());
        assert_eq!(Parts::other_versions(&recorded, &lock), vec!["symbol moved".to_owned()]);
    }

    #[test]
    fn uses_leaves_out_what_the_lock_does_not_lock() {
        let mut parts = Parts::default();
        parts.pages.insert("p1".into(), Some(page_using("p1", "relay")));
        parts.pages.insert("p2".into(), Some(page_using("p2", "loose")));
        parts.pages.insert("p3".into(), None);
        let mut lock = Lockfile::default();
        lock.symbols.insert("relay".into(), entry("r1"));
        let uses = parts.uses(&lock);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses.get(&(ItemKind::Symbol, "relay".into())), Some(&"r1".to_owned()));
    }

    #[test]
    fn unlocked_uses_counts_only_items_newly_left_without_a_lock() {
        let mut project = Project::new("Demo");
        project.pages.push(page_using("p1", "relay"));
        project.pages.push(page_using("p2", "already"));
        project.lock.symbols.insert("relay".into(), entry("r1"));

        // Taking out the lock of a symbol still placed leaves it unlocked.
        let mut drop_lock = Parts::default();
        drop_lock.lock_symbols.insert("relay".into(), None);
        assert_eq!(drop_lock.unlocked_uses(&project), vec!["symbol relay".to_owned()]);

        // Bringing back a page that uses an unlocked symbol counts it, unless it was already so.
        let mut bring_back = Parts::default();
        bring_back.pages.insert("p3".into(), Some(page_using("p3", "fresh")));
        bring_back.pages.insert("p4".into(), Some(page_using("p4", "already")));
        assert_eq!(bring_back.unlocked_uses(&project), vec!["symbol fresh".to_owned()]);

        // Taking the page away with its lock leaves nothing unlocked.
        let mut both = Parts::default();
        both.pages.insert("p1".into(), None);
        both.lock_symbols.insert("relay".into(), None);
        assert!(both.unlocked_uses(&project).is_empty());
    }
}

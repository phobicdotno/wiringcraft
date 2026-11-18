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
//! file, so every copy can draw it and the host's folder and history keep it. Picture files
//! (the logo, page underlays) do the same: a guest gets the project's pictures when it joins,
//! and a picture someone starts using goes out as an `AddPicture` edit before the change that
//! names it, so every copy shows it and the host's folder keeps it with that change's commit.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use wc_core::lock::ItemKind;
use wc_core::{Page, Part, Project, Symbol, store};
use wc_share::discovery::{Announcer, Beacon, Browser};
use wc_share::net::{self, Listener};
use wc_share::transport::{GuestLink, Hub};
use wc_share::{ChangeInfo, ChangeKind, redo_target, undo_target};
use wc_share::{GitJournal, Guest, GuestMsg, GuestStatus, HOST, Host, Peer, PeerId, Picture, ProjectEdit};

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

/// Put back in the window (and in `base`, the last state synced) what a refused change of this
/// person's own page touched: the pages `ids` as the session has them (also one deleted here
/// meanwhile, or one the session no longer has), the project `records` it touched (as the
/// session has them now), and the display `numbers` it set on other pages (by id, with the
/// number sent), so the action is not left half kept.
fn take_back_pages(
    open: &mut Open,
    mut base: Option<&mut Project>,
    auth: &Project,
    ids: &[String],
    records: wc_share::Parts,
    numbers: &BTreeMap<String, String>,
    notes: &mut Vec<String>,
) {
    // The records it touched as the session has them now: a later change to one may have come
    // in with the refusal.
    let mut parts = wc_share::Parts::read(auth, &records);
    for id in ids {
        parts.pages.insert(id.clone(), auth.page(id).cloned());
    }
    parts.apply(&mut open.project);
    if let Some(base) = base.as_deref_mut() {
        parts.apply(base);
    }
    // Another page it renumbered takes back only its display number (its content is not what
    // was refused), and only where it still has the number sent: a newer one set here
    // meanwhile is kept, and goes out as its own renumbering. A page the session no longer
    // has (the host removed it) goes, as it went there.
    for (id, sent) in numbers.iter().filter(|(id, _)| !ids.contains(id)) {
        let session = auth.page(id).map(|p| p.number.clone());
        let back = |project: &mut Project| match &session {
            Some(number) => {
                if let Some(page) = project.page_mut(id)
                    && page.number == *sent
                {
                    page.number.clone_from(number);
                }
            }
            None => {
                project.pages.retain(|p| p.id != *id);
                project.comments.retain(|c| c.page != *id);
            }
        };
        back(&mut open.project);
        if let Some(base) = base.as_deref_mut() {
            back(base);
        }
    }
    // In page order by the numbers given back, also when the session has ended and no sync
    // follows.
    open.project.sort_pages();
    if let Some(base) = base {
        base.sort_pages();
    }
    open.dirty = true;
    notes.push(format!(
        "Your change to page {} was not kept, nor what you did on it after it.",
        ids.iter().map(|id| number_of(auth, id)).collect::<Vec<_>>().join(", ")
    ));
}

/// Take back in the window what the session refused since the last look ([`Role::retaken`]).
fn drain_retaken(role: &mut Role, open: &mut Open, base: Option<&mut Project>, notes: &mut Vec<String>) {
    let (ids, records, numbers) = role.retaken();
    if ids.is_empty() {
        return;
    }
    if let Some(auth) = role.project() {
        take_back_pages(open, base, auth, &ids, records, &numbers, notes);
    }
}

/// The most a folded batch may take as JSON: a session message's limit ([`net::MAX_LINE`])
/// less room for the message around it (its kind, number and author, a few hundred bytes; the
/// edits are carried as they are, not quoted again).
const FOLD_LIMIT: usize = net::MAX_LINE - 64 * 1024;

/// How long `v` is as JSON (what a session message carries).
fn json_len<T: serde::Serialize>(v: &T) -> usize {
    serde_json::to_vec(v).map_or(usize::MAX / 4, |b| b.len())
}

/// One action that changed this person's own page and project records goes out as one change:
/// the page joins the `SetPages` of the batch (made when there is none), so the host checks
/// and applies it with the rest, all or nothing. A page edit alone stays a page edit.
fn fold_own_page(out: &mut Outgoing, base: &Project) {
    if !out.edits.iter().any(ProjectEdit::batchable) {
        return;
    }
    let Some(page) = out.page.take() else { return };
    let Some(was) = base.page(&page.id).cloned() else {
        out.page = Some(page);
        return;
    };
    // The batch goes as one message, which carries the page twice (as it is and as it was): one
    // that would not fit a session message goes as before, the page edit apart. Measured as it
    // would go, separators and all.
    let mut edits = out.edits.clone();
    match edits.iter_mut().find_map(|e| match e {
        ProjectEdit::SetPages { pages, before } => Some((pages, before)),
        _ => None,
    }) {
        Some((pages, before)) => {
            pages.push(page.clone());
            before.push(was);
        }
        None => edits.push(ProjectEdit::SetPages {
            pages: vec![page.clone()],
            before: vec![was],
        }),
    }
    if json_len(&edits) > FOLD_LIMIT {
        out.page = Some(page);
        return;
    }
    out.edits = edits;
}

/// Page `base` as both this window (`local`) and the session (`auth`) changed it: the session's
/// page with the local changes on top, or `None` when the two touched the same thing. Symbols
/// and wires are matched by id, title-block fields by name, the free graphics and texts (which
/// carry no id) item by item ([`merge_unordered`]); the title, size, underlay, and the overview
/// and cover marks each count as one thing.
/// A wire that ends at a symbol the other side took away is a clash too. The page number is
/// left as the session's.
fn merge_page(base: &Page, auth: &Page, local: &Page) -> Option<Page> {
    let mut page = auth.clone();
    page.title = pick(&base.title, &auth.title, &local.title)?;
    page.size = pick(&base.size, &auth.size, &local.size)?;
    page.underlay = pick(&base.underlay, &auth.underlay, &local.underlay)?;
    page.overview = pick(&base.overview, &auth.overview, &local.overview)?;
    page.cover = pick(&base.cover, &auth.cover, &local.cover)?;
    page.graphics = merge_unordered(&base.graphics, &auth.graphics, &local.graphics, |a, b| {
        std::mem::discriminant(a) == std::mem::discriminant(b)
    })?;
    let names: std::collections::BTreeSet<&String> =
        [base, auth, local].iter().flat_map(|p| p.title_block.keys()).collect();
    let mut title_block = BTreeMap::new();
    for name in names {
        let field = |p: &Page| p.title_block.get(name).cloned();
        if let Some(value) = pick(&field(base), &field(auth), &field(local))? {
            title_block.insert(name.clone(), value);
        }
    }
    page.title_block = title_block;
    page.symbols = merge_by_id(&base.symbols, &auth.symbols, &local.symbols, |s| &s.id)?;
    page.wires = merge_by_id(&base.wires, &auth.wires, &local.wires, |w| &w.id)?;
    let symbols: std::collections::BTreeSet<&str> = page.symbols.iter().map(|s| s.id.as_str()).collect();
    let dangling = page
        .wires
        .iter()
        .flat_map(|w| [&w.from, &w.to])
        .any(|end| matches!(end, wc_core::Endpoint::Pin { item, .. } if !symbols.contains(item.as_str())));
    (!dangling).then_some(page)
}

/// One thing in a three-way merge: the side that changed it, or `None` when both changed it
/// differently.
fn pick<T: PartialEq + Clone>(base: &T, auth: &T, local: &T) -> Option<T> {
    if local == base || local == auth {
        Some(auth.clone())
    } else if auth == base {
        Some(local.clone())
    } else {
        None
    }
}

/// A three-way merge of items without ids (a page's free graphics and texts), as multisets.
/// When only one side changed the list, or both made it the same, that list is taken as it is
/// (its order kept). Otherwise: the session's items in its order, less those removed here that
/// the session still has, then the items added here (one the session added the same way is not
/// added twice). A change to an item is its removal and the new version's addition, so `None` (a
/// clash) when an item removed here is gone from the session too and this window added an item
/// that may be its new version (`same_kind`, such as another line for a line): it was most
/// likely changed here and removed or changed by the session. Likewise when the session added
/// such an item: it most likely changed the item this window deleted. Removed on both sides
/// with nothing of its kind added on either side is no clash.
fn merge_unordered<T: PartialEq + Clone>(
    base: &[T],
    auth: &[T],
    local: &[T],
    same_kind: impl Fn(&T, &T) -> bool,
) -> Option<Vec<T>> {
    if local == base || local == auth {
        return Some(auth.to_vec());
    }
    if auth == base {
        return Some(local.to_vec());
    }
    // `a` less `b`, counting repeats.
    let minus = |a: &[T], b: &[T]| -> Vec<T> {
        let mut left: Vec<&T> = b.iter().collect();
        a.iter()
            .filter(|x| match left.iter().position(|y| y == x) {
                Some(i) => {
                    left.swap_remove(i);
                    false
                }
                None => true,
            })
            .cloned()
            .collect()
    };
    let removed_here = minus(base, local);
    let mut added_here = minus(local, base);
    let mut added_there = minus(auth, base);
    // Additions made on both sides (the same move made here and there) are one change, agreed:
    // already in the session's list, and no sign of a clash.
    added_here.retain(|x| match added_there.iter().position(|y| y == x) {
        Some(i) => {
            added_there.swap_remove(i);
            false
        }
        None => true,
    });
    let mut removed_there = minus(base, auth);
    // Removed only here, grouped by value with a count.
    let mut only_here: Vec<(&T, usize)> = Vec::new();
    for r in &removed_here {
        match removed_there.iter().position(|x| x == r) {
            // Gone from the session too (that copy, not another one it still has): a clash if
            // it may have been changed here, or if the session may have changed it (deleted
            // here, moved there).
            Some(i) => {
                removed_there.swap_remove(i);
                if added_here.iter().chain(&added_there).any(|a| same_kind(a, r)) {
                    return None;
                }
            }
            None => match only_here.iter_mut().find(|(v, _)| *v == r) {
                Some((_, n)) => *n += 1,
                None => only_here.push((r, 1)),
            },
        }
    }
    let mut out = auth.to_vec();
    for (r, n) in only_here {
        // With several equal copies left in the session, which ones went matters when they sit
        // at different places among the others: removing every copy, or some copies of one
        // unbroken run, gives one list; otherwise it is not known which copies went.
        let copies = out.iter().filter(|x| *x == r).count();
        if n < copies {
            let mut runs = 0;
            let mut in_run = false;
            for x in &out {
                let is_copy = x == r;
                if is_copy && !in_run {
                    runs += 1;
                }
                in_run = is_copy;
            }
            if runs > 1 {
                return None;
            }
        }
        let mut left = n;
        out.retain(|x| {
            if left > 0 && x == r {
                left -= 1;
                false
            } else {
                true
            }
        });
    }
    out.extend(added_here);
    Some(out)
}

/// A three-way merge of items with ids: the session's items in its order, each with this
/// window's change when only this window changed it, then the items only this window added.
/// `None` when an item was changed on both sides differently, removed on one side and changed
/// on the other, or added on both with different content.
fn merge_by_id<T: PartialEq + Clone>(
    base: &[T],
    auth: &[T],
    local: &[T],
    id: impl Fn(&T) -> &String,
) -> Option<Vec<T>> {
    let find = |items: &[T], key: &String| items.iter().find(|x| id(x) == key).cloned();
    let mut out = Vec::new();
    for a in auth {
        match (find(base, id(a)), find(local, id(a))) {
            (Some(b), Some(l)) => out.push(pick(&b, a, &l)?),
            // Removed here: gone, unless the session changed it.
            (Some(b), None) if b == *a => {}
            (Some(_), None) => return None,
            // Added on both sides.
            (None, Some(l)) if l == *a => out.push(a.clone()),
            (None, Some(_)) => return None,
            (None, None) => out.push(a.clone()),
        }
    }
    for l in local {
        if find(auth, id(l)).is_some() {
            continue;
        }
        match find(base, id(l)) {
            // Added here.
            None => out.push(l.clone()),
            // The session removed it; changed here too is a clash.
            Some(b) if b != *l => return None,
            Some(_) => {}
        }
    }
    Some(out)
}

/// Parts changed both in this window and in the session since the last sync (`base`): the
/// session's version wins on them, in `local` and in `base`, so what is sent next is not built
/// on content an undo has just put back. A part is a page (also one removed on either side),
/// the project properties, one device, the comments of one page, or one panel; a change to a
/// part the session did not touch is kept and sent as usual. On a page both changed, the local
/// change is put on top of the session's page when the two touched different things
/// ([`merge_page`]). Returns the parts whose local change was dropped, named for the person.
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
        if let (Some(a), Some(l)) = (a, l)
            && let Some(mut merged) = merge_page(b, a, l)
        {
            // A number only this window changed goes out on its own, as a renumbering.
            merged.number = if l.number != b.number {
                l.number.clone()
            } else {
                a.number.clone()
            };
            let a = a.clone();
            if let Some(p) = local.page_mut(&id) {
                *p = merged;
            }
            base.pages[pos] = a;
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
/// version. Their files come with the session, see [`take_files`]; items the session took back
/// are dropped by [`taken_back`] first. Returns whether anything changed.
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

/// The symbols and parts `local` uses in what it changed since `base` (the last sync): a
/// placement the base's page does not have, a part a device names that the base's device does
/// not, a jumper the base does not have. A use the last sync brought in is not one, even on a
/// page or device changed otherwise since: the session may have taken it back.
fn fresh_uses(
    local: &Project,
    base: &Project,
) -> (std::collections::BTreeSet<String>, std::collections::BTreeSet<String>) {
    let fresh = FreshUses::find(local, base);
    let symbols = fresh
        .pages
        .into_iter()
        .flat_map(|(_, s)| s)
        .map(str::to_owned)
        .collect();
    let parts = fresh
        .devices
        .into_iter()
        .flat_map(|(_, p)| p)
        .chain(fresh.jumpers)
        .map(str::to_owned)
        .collect();
    (symbols, parts)
}

/// The fresh uses of [`fresh_uses`], by the target that has them (targets without any left out).
struct FreshUses<'a> {
    /// Page id, and the symbols of its placements the base's page does not have.
    pages: Vec<(&'a str, Vec<&'a str>)>,
    /// Device tag, and the parts it names that the base's device does not.
    devices: Vec<(&'a wc_core::DeviceTag, Vec<&'a str>)>,
    /// The parts of jumpers the base does not have.
    jumpers: Vec<&'a str>,
}

impl<'a> FreshUses<'a> {
    fn find(local: &'a Project, base: &Project) -> Self {
        let pages = local
            .pages
            .iter()
            .filter_map(|p| {
                let had: std::collections::BTreeSet<(&str, &str)> = base
                    .page(&p.id)
                    .map(|b| b.symbols.iter().map(|o| (o.id.as_str(), o.symbol.as_str())).collect())
                    .unwrap_or_default();
                let new: Vec<&str> = p
                    .symbols
                    .iter()
                    .filter(|s| !had.contains(&(s.id.as_str(), s.symbol.as_str())))
                    .map(|s| s.symbol.as_str())
                    .collect();
                (!new.is_empty()).then_some((p.id.as_str(), new))
            })
            .collect();
        let devices = local
            .devices
            .iter()
            .filter_map(|(tag, d)| {
                let had: std::collections::BTreeSet<String> = base
                    .devices
                    .get(tag)
                    .map(|b| b.all_parts().into_iter().map(|(p, _)| p.to_owned()).collect())
                    .unwrap_or_default();
                let new: Vec<&str> = d
                    .all_parts()
                    .into_iter()
                    .map(|(p, _)| p)
                    .filter(|p| !had.contains(*p))
                    .collect();
                (!new.is_empty()).then_some((tag, new))
            })
            .collect();
        let jumpers = local
            .meta
            .jumpers
            .iter()
            .filter(|j| !base.meta.jumpers.contains(j))
            .filter_map(|j| j.part.as_deref())
            .collect();
        FreshUses {
            pages,
            devices,
            jumpers,
        }
    }
}

/// Items the session locked at the last sync (`was`) and no longer locks were taken back (an
/// undo): drop them from `local`'s lock too, so they are not sent again as new items. Kept are
/// items this copy locks in another version, and items it uses in a change made since the last
/// sync (`base`) and not sent yet: those go out again with the change that uses them. A use
/// the last sync brought in does not keep an item: the session took that use back with it.
/// Returns the items dropped.
pub fn taken_back(
    local: &mut Project,
    base: &Project,
    was: &wc_core::Lockfile,
    auth: &Project,
) -> Vec<(ItemKind, String)> {
    let candidate = |kind: ItemKind| {
        was.map(kind).iter().any(|(id, e)| {
            !auth.lock.map(kind).contains_key(id) && local.lock.map(kind).get(id).is_some_and(|l| l.hash == e.hash)
        })
    };
    if !candidate(ItemKind::Symbol) && !candidate(ItemKind::Part) {
        return Vec::new();
    }
    let (symbols, parts) = fresh_uses(local, base);
    let mut dropped = Vec::new();
    for kind in [ItemKind::Symbol, ItemKind::Part] {
        let used = match kind {
            ItemKind::Symbol => &symbols,
            ItemKind::Part => &parts,
        };
        let gone: Vec<String> = was
            .map(kind)
            .iter()
            .filter(|(id, e)| {
                !auth.lock.map(kind).contains_key(*id)
                    && !used.contains(*id)
                    && local.lock.map(kind).get(*id).is_some_and(|l| l.hash == e.hash)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            local.lock.map_mut(kind).remove(&id);
            dropped.push((kind, id));
        }
    }
    dropped
}

/// The session's lock to compare the next look with: `auth`'s, except that an item this copy
/// still locks in the version the session had at the last look (`was`) keeps that version while
/// the session names another. This copy takes the new version only later ([`incoming`]); if
/// the session takes that one back first, the old version here is still known as the
/// session's and goes with it.
fn next_seen(was: &wc_core::Lockfile, local: &Project, auth: &Project) -> wc_core::Lockfile {
    let mut next = auth.lock.clone();
    for kind in [ItemKind::Symbol, ItemKind::Part] {
        for (id, e) in was.map(kind) {
            let here = local.lock.map(kind).get(id).is_some_and(|l| l.hash == e.hash);
            let moved = auth.lock.map(kind).get(id).is_some_and(|a| a.hash != e.hash);
            if here && moved {
                next.map_mut(kind).insert(id.clone(), e.clone());
            }
        }
    }
    next
}

/// Drop from `open` what the session took back since the last look (`seen`, the session's lock
/// then, updated by [`next_seen`]): the lock entries ([`taken_back`]) and the loaded copies, so
/// placing the item again takes it afresh and locks it, instead of drawing a loaded copy
/// without a pin. A copy a page or device here still uses stays loaded until the session's
/// change replaces it (`unload`, emptied by [`unload_taken_back`]). The entries in the symbol
/// and part lists wait in `delist` for [`delist_taken_back`], which looks at the libraries the
/// session's change names.
fn follow_take_backs(
    seen: &mut Option<wc_core::Lockfile>,
    unload: &mut std::collections::BTreeSet<(ItemKind, String)>,
    delist: &mut std::collections::BTreeSet<(ItemKind, String)>,
    open: &mut Open,
    base: &Project,
    auth: &Project,
) {
    let was = seen.take().unwrap_or_else(|| auth.lock.clone());
    let dropped = taken_back(&mut open.project, base, &was, auth);
    *seen = Some(next_seen(&was, &open.project, auth));
    if dropped.is_empty() {
        return;
    }
    // The lock changed, not yet in this copy's files.
    open.dirty = true;
    delist.extend(dropped.iter().cloned());
    unload.extend(dropped);
    unload_taken_back(unload, open);
}

/// Take the items the session took back (`delist`) out of the symbol and part lists unless a
/// library here offers them or this copy locks them again. Returns why the libraries could not
/// be opened, if they could not; the items then stay listed and waiting for the next try.
fn delist_taken_back(delist: &mut std::collections::BTreeSet<(ItemKind, String)>, open: &mut Open) -> Option<String> {
    if delist.is_empty() {
        return None;
    }
    let lib = match wc_library::Library::open(&open.dir, &open.project) {
        Ok(lib) => lib,
        Err(e) => {
            return Some(format!(
                "The library could not be opened to check the items the session took back ({e}); they stay in the lists."
            ));
        }
    };
    for (kind, id) in std::mem::take(delist) {
        if open.project.lock.map(kind).contains_key(&id) || lib.locate(kind, &id).is_some() {
            continue;
        }
        match kind {
            ItemKind::Symbol => open.library.retain(|(i, _)| *i != id),
            ItemKind::Part => open.part_library.retain(|(i, _)| *i != id),
        }
    }
    None
}

/// Unload the copies of items the session took back (`unload`) that this copy neither locks
/// again nor uses any more; the others wait for the session's change that removes their use.
fn unload_taken_back(unload: &mut std::collections::BTreeSet<(ItemKind, String)>, open: &mut Open) {
    if unload.is_empty() {
        return;
    }
    let (symbols, parts) = (open.project.used_symbols(), open.project.used_parts());
    unload.retain(|(kind, id)| {
        let (locked, used) = match kind {
            ItemKind::Symbol => (open.project.lock.symbols.contains_key(id), symbols.contains(id)),
            ItemKind::Part => (open.project.lock.parts.contains_key(id), parts.contains(id)),
        };
        if locked {
            return false;
        }
        if used {
            return true;
        }
        match kind {
            ItemKind::Symbol => {
                open.symbols.remove(id);
            }
            ItemKind::Part => {
                open.parts.remove(id);
            }
        }
        false
    });
}

/// The items `local` locks in another version than the session `auth`: someone else used them
/// first, and [`incoming`] gives this copy the session's version. As `symbol id` or `part id`.
/// An update made here since the last sync (`base`, see [`updated_here`]) is none.
pub fn conflicts(local: &Project, base: &Project, auth: &Project) -> Vec<String> {
    [ItemKind::Symbol, ItemKind::Part]
        .into_iter()
        .flat_map(|kind| {
            local
                .lock
                .map(kind)
                .iter()
                .filter(move |(id, e)| auth.lock.map(kind).get(*id).is_some_and(|a| a.hash != e.hash))
                .filter(move |(id, _)| !updated_here(local, base, auth, kind, id))
                // A copy still on the version the last sync brought did not change it here.
                .filter(move |(id, e)| base.lock.map(kind).get(*id).is_none_or(|b| b.hash != e.hash))
                .map(move |(id, _)| item_name(kind, id))
        })
        .collect()
}

/// Whether `local` holds item `id` in a version it updated to since the last sync (`base`):
/// the session `auth` still has the version the last sync brought, and this copy another.
fn updated_here(local: &Project, base: &Project, auth: &Project, kind: ItemKind, id: &str) -> bool {
    let (Some(l), Some(b), Some(a)) = (
        local.lock.map(kind).get(id),
        base.lock.map(kind).get(id),
        auth.lock.map(kind).get(id),
    ) else {
        return false;
    };
    b.hash == a.hash && l.hash != a.hash
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
    named(name).is_some_and(|(kind, id)| auth.lock.map(kind).contains_key(id))
}

/// Whether the session took what this copy sent of the item [`item_name`] names: it locks the
/// item, and not still in the version the last sync (`base`) brought while this copy updated
/// it ([`updated_here`]).
fn session_took(auth: &Project, local: &Project, base: &Project, name: &str) -> bool {
    named(name)
        .is_some_and(|(kind, id)| auth.lock.map(kind).contains_key(id) && !updated_here(local, base, auth, kind, id))
}

/// Take in the session's answers to this person's updates: the version it took is what this copy
/// synced to (`base`), so a later change here goes out as an update from it, and one the session
/// took is not waited for or sent again, even when a later change (an undo) put it back since.
fn take_updates(role: &mut Role, base: &mut Project, items_out: &mut BTreeMap<String, ItemOut>) {
    for (kind, id, hash) in role.took_updates() {
        if let Some(e) = base.lock.map_mut(kind).get_mut(&id) {
            e.hash = hash;
        }
        items_out.remove(&item_name(kind, &id));
    }
}

/// The kind and id of the item [`item_name`] names.
fn named(name: &str) -> Option<(ItemKind, &str)> {
    match name.split_once(' ')? {
        ("symbol", id) => Some((ItemKind::Symbol, id)),
        ("part", id) => Some((ItemKind::Part, id)),
        _ => None,
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
    /// The edits that add them and update them, with their files.
    pub edits: Vec<ProjectEdit>,
    /// The items it holds, as [`item_name`] names them.
    pub sent: Vec<String>,
    /// Items whose file could not be read, with the reason; they are not sent.
    pub unreadable: Vec<(String, String)>,
    /// Items this copy updated whose earlier version the session took back since the last sync
    /// (`base` has them, the session no longer does): not sent, as an add would undo that.
    pub stale: Vec<String>,
    /// Files the session lacks whose copy here is back but cannot be sent (another version than
    /// the pin, or unreadable), with the reason. Nothing waits for them.
    pub unsent_repairs: Vec<(String, String)>,
}

/// The items the session locks but has no file of, as [`item_name`] names them: for a host
/// [`wc_share::Host::files_missing`], for a guest the files the session did not send it
/// ([`wc_share::Guest::files_missing`]), as every guest gets them all.
fn repairs_due(role: &Role) -> std::collections::BTreeSet<String> {
    let missing = match role {
        Role::Host(h) => h.host.files_missing(),
        Role::Guest(j) => j.guest.files_missing(),
    };
    missing.into_iter().map(|(kind, id)| item_name(kind, &id)).collect()
}

/// The symbols and parts locked in `local` and not in the session's project `auth`, with their
/// vendored files from `dir`, as one edit: what this person started using. Then those this
/// copy updated since the last sync (`base`, see [`updated_here`]), as a second edit that
/// names the version the session has. Items named in `skip` (sent and not answered yet) are
/// left out. Items named in `apart` (sent before and not taken) go each in an edit of its own,
/// so one the session cannot store does not keep the others out. An item whose file cannot be
/// read is not sent; it is returned with the reason and tried again next time. Items named in
/// `repair` the session locks in the same version but has no file of (a host that started the
/// session with the copy missing): they are sent again as an add, so guests get the file.
pub fn new_items(
    dir: &Path,
    local: &Project,
    base: &Project,
    auth: &Project,
    skip: &std::collections::BTreeSet<String>,
    apart: &std::collections::BTreeSet<String>,
    repair: &std::collections::BTreeSet<String>,
) -> NewItems {
    let mut out = NewItems::default();
    // (id, source, hash here, the session's hash when it is an update)
    let wanted = |kind: ItemKind| {
        local
            .lock
            .map(kind)
            .iter()
            .filter(move |(id, _)| !skip.contains(&item_name(kind, id)))
            .filter_map(move |(id, e)| match auth.lock.map(kind).get(id) {
                // The session had it at the last sync in another version, and took it back since.
                None if base.lock.map(kind).get(id).is_some_and(|b| b.hash != e.hash) => None,
                None => Some((id.clone(), e.source.clone(), e.hash.clone(), None)),
                Some(a) if updated_here(local, base, auth, kind, id) => {
                    Some((id.clone(), e.source.clone(), e.hash.clone(), Some(a.hash.clone())))
                }
                // The session's entry as it is (its source too): only the file is missing.
                Some(a) if a.hash == e.hash && repair.contains(&item_name(kind, id)) => {
                    Some((id.clone(), a.source.clone(), e.hash.clone(), None))
                }
                Some(_) => None,
            })
            .collect::<Vec<_>>()
    };
    for kind in [ItemKind::Symbol, ItemKind::Part] {
        out.stale.extend(
            local
                .lock
                .map(kind)
                .iter()
                .filter(|(id, e)| {
                    !auth.lock.map(kind).contains_key(*id)
                        && base.lock.map(kind).get(*id).is_some_and(|b| b.hash != e.hash)
                })
                .map(|(id, _)| item_name(kind, id)),
        );
    }
    let (mut symbols, mut new_symbols) = (Vec::new(), Vec::new());
    for (id, source, hash, before) in wanted(ItemKind::Symbol) {
        match check_locked(store::read_vendored_symbol(dir, &id), Symbol::hash, &hash) {
            Ok(s) => {
                out.sent.push(item_name(ItemKind::Symbol, &id));
                match before {
                    None => symbols.push((s, source)),
                    Some(before) => new_symbols.push((s, source, before)),
                }
            }
            // A repair whose file is not back here yet waits quietly: the session works as is.
            // One that is back but cannot be sent says why, and so does one whose file cannot
            // be looked for (no access, or an id that makes no file name).
            Err(e) if before.is_none() && auth.lock.symbols.contains_key(&id) => {
                if !store::vendor_path(dir, ItemKind::Symbol, &id).is_ok_and(|p| matches!(p.try_exists(), Ok(false))) {
                    out.unsent_repairs.push((item_name(ItemKind::Symbol, &id), e));
                }
            }
            Err(e) => out.unreadable.push((item_name(ItemKind::Symbol, &id), e)),
        }
    }
    let (mut parts, mut new_parts) = (Vec::new(), Vec::new());
    for (id, source, hash, before) in wanted(ItemKind::Part) {
        match check_locked(store::read_vendored_part(dir, &id), Part::hash, &hash) {
            Ok(p) => {
                out.sent.push(item_name(ItemKind::Part, &id));
                match before {
                    None => parts.push((p, source)),
                    Some(before) => new_parts.push((p, source, before)),
                }
            }
            Err(e) if before.is_none() && auth.lock.parts.contains_key(&id) => {
                if !store::vendor_path(dir, ItemKind::Part, &id).is_ok_and(|p| matches!(p.try_exists(), Ok(false))) {
                    out.unsent_repairs.push((item_name(ItemKind::Part, &id), e));
                }
            }
            Err(e) => out.unreadable.push((item_name(ItemKind::Part, &id), e)),
        }
    }
    let alone = |kind: ItemKind, id: &str| apart.contains(&item_name(kind, id));
    for (s, source) in symbols.extract_if(.., |(s, _)| alone(ItemKind::Symbol, s.id.as_str())) {
        out.edits.push(ProjectEdit::AddItems {
            symbols: vec![(s, source)],
            parts: Vec::new(),
        });
    }
    for (p, source) in parts.extract_if(.., |(p, _)| alone(ItemKind::Part, p.id.as_str())) {
        out.edits.push(ProjectEdit::AddItems {
            symbols: Vec::new(),
            parts: vec![(p, source)],
        });
    }
    for s in new_symbols.extract_if(.., |(s, _, _)| alone(ItemKind::Symbol, s.id.as_str())) {
        out.edits.push(ProjectEdit::UpdateItems {
            symbols: vec![s],
            parts: Vec::new(),
        });
    }
    for p in new_parts.extract_if(.., |(p, _, _)| alone(ItemKind::Part, p.id.as_str())) {
        out.edits.push(ProjectEdit::UpdateItems {
            symbols: Vec::new(),
            parts: vec![p],
        });
    }
    if !symbols.is_empty() || !parts.is_empty() {
        out.edits.push(ProjectEdit::AddItems { symbols, parts });
    }
    if !new_symbols.is_empty() || !new_parts.is_empty() {
        out.edits.push(ProjectEdit::UpdateItems {
            symbols: new_symbols,
            parts: new_parts,
        });
    }
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
                match open.library.iter_mut().find(|(i, _)| *i == id) {
                    Some(row) => row.1 = s.name.clone(),
                    None => {
                        open.library.push((id.clone(), s.name.clone()));
                        open.library.sort();
                    }
                }
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
                match open.part_library.iter_mut().find(|(i, _)| *i == id) {
                    Some(row) => row.1 = crate::state::part_label(p),
                    None => {
                        open.part_library.push((id.clone(), crate::state::part_label(p)));
                        open.part_library.sort();
                    }
                }
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

/// The picture files `project` uses: its logo and its pages' underlays, by project path.
pub fn used_pictures(project: &Project) -> std::collections::BTreeSet<String> {
    project
        .pages
        .iter()
        .filter_map(|p| p.underlay.as_ref().map(|u| u.file.clone()))
        .chain(project.meta.logo.as_ref().map(|l| l.file.clone()))
        .collect()
}

/// What [`new_pictures`] found.
#[derive(Debug, Default)]
pub struct NewPictures {
    /// One edit per picture, with its file.
    pub edits: Vec<ProjectEdit>,
    /// The pictures looked at: sent, missing here, or unreadable. Each waits a while before it
    /// is looked at again.
    pub tried: Vec<String>,
    /// Pictures that exist here but cannot be sent, with the reason.
    pub unreadable: Vec<(String, String)>,
    /// Pictures the session has that were replaced here by another file of the same name:
    /// sent (in `edits`) so the session refuses the name and this copy gives its file a name of
    /// its own, as for any picture clash.
    pub replaced: Vec<String>,
    /// Pictures whose file here was touched but still is the session's, with their version
    /// now ([`picture_version`]): looked at again only when they change again.
    pub same: Vec<(String, PictureVersion)>,
}

/// What tells one version of a picture file here from another without reading it: its size and
/// when it was last written.
pub type PictureVersion = (u64, Option<std::time::SystemTime>);

/// The version of picture `file` in `dir` ([`PictureVersion`]); `None` when it is not there.
pub fn picture_version(dir: &Path, file: &str) -> Option<PictureVersion> {
    let m = std::fs::metadata(dir.join(file)).ok()?;
    Some((m.len(), m.modified().ok()))
}

/// Whether a picture file here is still the one taken in at `was`, judged without reading it.
/// Only a version with a known write time can say so: without one (the file system gives
/// none), a same-size replacement would look unchanged, so the file is read again instead.
fn still_taken(now: Option<&PictureVersion>, was: &PictureVersion) -> bool {
    was.1.is_some() && now == Some(was)
}

/// The pictures `local` uses that the session (`session`, by project path) does not have, read
/// from `dir`, each as its own edit so each message stays small. Pictures named in `skip` were
/// tried a moment ago and are left out. A picture missing here is left out silently: nobody
/// can show it, and the window already says so. A picture the session has goes again when the
/// file here was replaced since this copy took the session's (`taken`, by its version then).
pub fn new_pictures(
    dir: &Path,
    local: &Project,
    session: &BTreeMap<String, Vec<u8>>,
    taken: &BTreeMap<String, PictureVersion>,
    skip: &std::collections::BTreeSet<String>,
) -> NewPictures {
    let mut out = NewPictures::default();
    for file in used_pictures(local) {
        if skip.contains(&file) {
            continue;
        }
        if let Some(theirs) = session.get(&file) {
            // Taken in here: unchanged since, it is the session's (no read needed).
            let Some(was) = taken.get(&file) else { continue };
            let now = picture_version(dir, &file);
            if still_taken(now.as_ref(), was) {
                continue;
            }
            match wc_pdf::read_picture(dir, &file) {
                Ok(Some(data)) if data == *theirs => {
                    if let Some(now) = now {
                        out.same.push((file, now));
                    }
                }
                // Gone here: the session's stays, and the window says it is missing.
                Ok(None) => {}
                // Another file: sent, or (empty, too big) the reason is told. Either way it is
                // looked at again only after a while.
                Ok(Some(data)) => {
                    out.tried.push(file.clone());
                    out.replaced.push(file.clone());
                    let picture = Picture { file, data };
                    match picture.problem() {
                        Some(why) => out.unreadable.push((picture.file, why)),
                        None => out.edits.push(ProjectEdit::AddPicture { picture }),
                    }
                }
                // Unreadable here: the session's stays, and the reason is told.
                Err(why) => {
                    out.tried.push(file.clone());
                    out.replaced.push(file.clone());
                    out.unreadable.push((file, why));
                }
            }
            continue;
        }
        out.tried.push(file.clone());
        if !Picture::file_ok(&file) {
            out.unreadable.push((
                file,
                format!(
                    "only pictures in the {} and {} folders are shared",
                    wc_core::Logo::DIR,
                    wc_core::Underlay::DIR
                ),
            ));
            continue;
        }
        match wc_pdf::read_picture(dir, &file) {
            Ok(Some(data)) => {
                let picture = Picture { file, data };
                match picture.problem() {
                    Some(why) => out.unreadable.push((picture.file, why)),
                    None => out.edits.push(ProjectEdit::AddPicture { picture }),
                }
            }
            Ok(None) => {}
            Err(why) => out.unreadable.push((file, why)),
        }
    }
    out
}

/// Copy the picture `file` of `open` under a new name in the same folder, one the session is not
/// likely to have (the name with a mark of the moment), and make the project name it instead.
/// The old file stays. Returns the new name.
fn rename_picture(open: &mut Open, file: &str) -> Result<String, String> {
    // Read as an export reads it: never through a link out of the project folder.
    let data = match wc_pdf::read_picture(&open.dir, file) {
        Ok(Some(data)) => data,
        Ok(None) => return Err(format!("cannot read {file}: the file is gone")),
        Err(why) => return Err(format!("cannot read {file}: {why}")),
    };
    let (folder, name) = file.rsplit_once('/').unwrap_or(("", file));
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    // Most file systems take names of up to 255 bytes: the stem gives way to the mark (a dash
    // and up to ten hex digits), a count (up to "-15") and the extension.
    let room = 255usize.saturating_sub(14 + if ext.is_empty() { 0 } else { ext.len() + 1 });
    let mut cut = stem.len().min(room);
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    let stem = &stem[..cut];
    let mark = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
        % 0xff_ffff_ffff;
    let mut new = String::new();
    for n in 0..16u32 {
        let base = if n == 0 {
            format!("{stem}-{mark:x}")
        } else {
            format!("{stem}-{mark:x}-{n}")
        };
        let candidate = match (folder.is_empty(), ext.is_empty()) {
            (true, true) => base,
            (true, false) => format!("{base}.{ext}"),
            (false, true) => format!("{folder}/{base}"),
            (false, false) => format!("{folder}/{base}.{ext}"),
        };
        if std::fs::symlink_metadata(open.dir.join(&candidate)).is_err() {
            new = candidate;
            break;
        }
    }
    if new.is_empty() {
        return Err("no free name".into());
    }
    let picture = Picture {
        file: new.clone(),
        data,
    };
    wc_share::write_picture(&open.dir, &picture).map_err(|e| e.to_string())?;
    if let Some(logo) = open.project.meta.logo.as_mut()
        && logo.file == file
    {
        logo.file = new.clone();
    }
    for page in &mut open.project.pages {
        if let Some(u) = page.underlay.as_mut()
            && u.file == file
        {
            u.file = new.clone();
        }
    }
    open.dirty = true;
    Ok(new)
}

/// The picture state of [`Share`] that [`rename_pictures`] keeps.
struct Pictures<'a> {
    renames_due: &'a mut BTreeMap<String, std::time::Instant>,
    pictures_out: &'a mut BTreeMap<String, std::time::Instant>,
    pictures_taken: &'a mut BTreeMap<String, PictureVersion>,
    replaced: &'a mut std::collections::BTreeSet<String>,
}

/// Give the pictures in `taken`, whose names the session has for other files, a name of
/// their own here, together with those whose rename failed a while ago. A failed rename is
/// kept and tried again after [`ITEM_RETRY_LATER`] while the project still uses the picture,
/// whether or not the session has the name: nothing else would bring it back. Meanwhile the
/// picture stays in `pictures_out` (of [`Share`]), so it is not sent again under the taken name.
/// A picture whose file here is the session's again (put back since it was sent) is not
/// renamed: it is taken as the session's (into `pictures_taken`, no longer `replaced`), and
/// nothing is sent.
fn rename_pictures(
    open: &mut Open,
    taken: Vec<String>,
    session: &BTreeMap<String, Vec<u8>>,
    pictures: Pictures<'_>,
    notes: &mut Vec<String>,
) {
    let Pictures {
        renames_due,
        pictures_out,
        pictures_taken,
        replaced,
    } = pictures;
    let used = used_pictures(&open.project);
    renames_due.retain(|file, _| used.contains(file));
    let due = renames_due
        .iter()
        .filter(|(_, at)| at.elapsed() >= ITEM_RETRY_LATER)
        .map(|(file, _)| file.clone())
        .collect::<Vec<_>>();
    // A name met again before its wait is over is left for then.
    let waiting = |f: &String| renames_due.get(f).is_some_and(|at| at.elapsed() < ITEM_RETRY_LATER);
    let files: std::collections::BTreeSet<String> = taken
        .into_iter()
        .filter(|f| used.contains(f) && !waiting(f))
        .chain(due)
        .collect();
    for file in files {
        if let Some(theirs) = session.get(&file) {
            // The version first: a write after it makes the next look read the file again.
            let before = picture_version(&open.dir, &file);
            if matches!(wc_pdf::read_picture(&open.dir, &file), Ok(Some(data)) if data == *theirs) {
                renames_due.remove(&file);
                pictures_out.remove(&file);
                replaced.remove(&file);
                // What the window showed while the file was another or gone is forgotten.
                open.underlays.remove(&file);
                open.picture_problems.remove(&file);
                if let Some(version) = before {
                    pictures_taken.insert(file, version);
                }
                continue;
            }
        }
        match rename_picture(open, &file) {
            Ok(new) => {
                pictures_out.remove(&file);
                renames_due.remove(&file);
                notes.push(format!(
                    "The session already has another file named {file}; your picture is renamed {new} here and sent again under that name."
                ));
            }
            Err(why) => {
                if renames_due.insert(file.clone(), std::time::Instant::now()).is_none() {
                    notes.push(format!(
                        "The session already has another file named {file}, and your picture could not be given a new name yet: {why}. It is tried again now and then."
                    ));
                }
            }
        }
    }
}

/// What [`take_pictures`] did: it writes the session's pictures that `open` uses and has not
/// taken yet (`taken`, by project path) into its folder, and forgets what the window showed for
/// them. A file of the same name with other content is never written over: a guest's copy
/// starts in an empty folder, so such a file is one this person picked here, and it is kept
/// (and named in the result, to get a name of its own). Pictures in `skip` (failed a moment
/// ago) are left for later.
#[derive(Debug, Default)]
pub struct TakenPictures {
    /// The pictures that could not be written, with the reason.
    pub failed: Vec<(String, String)>,
    /// The pictures this copy has another file under the session's name for.
    pub clashes: Vec<String>,
}

/// See [`TakenPictures`].
pub fn take_pictures(
    open: &mut Open,
    session: &BTreeMap<String, Vec<u8>>,
    taken: &mut BTreeMap<String, PictureVersion>,
    skip: &std::collections::BTreeSet<String>,
) -> TakenPictures {
    let mut out = TakenPictures::default();
    for file in used_pictures(&open.project) {
        // Once taken, a file changed here since is this copy's to send ([`new_pictures`]).
        if taken.contains_key(&file) || skip.contains(&file) {
            continue;
        }
        let Some(data) = session.get(&file) else { continue };
        let picture = Picture {
            file: file.clone(),
            data: data.clone(),
        };
        match wc_share::write_picture(&open.dir, &picture) {
            Ok(()) => {
                open.underlays.remove(&file);
                open.picture_problems.remove(&file);
                let version = picture_version(&open.dir, &file).unwrap_or((0, None));
                taken.insert(file, version);
            }
            Err(wc_share::Error::PictureNameTaken(_)) => out.clashes.push(file),
            Err(e) => out.failed.push((file, e.to_string())),
        }
    }
    out
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
    /// Items of the host's own updates the session took, as [`Guest::took_updates`].
    took_updates: Vec<(ItemKind, String, String)>,
    /// Pictures the host sent whose name the session has for another file (a picture replaced
    /// in its folder), since the last look: given a name of their own, as a guest's are.
    names_taken: Vec<String>,
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

    /// Whether a change that set this guest's own page is unanswered ([`Guest::own_page_waits`]).
    /// The host's own changes are answered at once.
    fn own_page_waits(&self) -> bool {
        match self {
            Role::Host(_) => false,
            Role::Guest(j) => j.guest.own_page_waits(),
        }
    }

    /// This person's pages whose change the session refused, taken back whole since the last
    /// call ([`Guest::retaken`]), with the records and other pages' numbers it touched.
    fn retaken(&mut self) -> (Vec<String>, wc_share::Parts, BTreeMap<String, String>) {
        match self {
            Role::Host(_) => Default::default(),
            Role::Guest(j) => (
                std::mem::take(&mut j.guest.retaken),
                std::mem::take(&mut j.guest.retaken_records),
                std::mem::take(&mut j.guest.retaken_numbers),
            ),
        }
    }

    /// The picture files the session has, by project path.
    fn pictures(&self) -> &BTreeMap<String, Vec<u8>> {
        match self {
            Role::Host(h) => h.host.pictures(),
            Role::Guest(j) => &j.guest.pictures,
        }
    }

    /// Pictures this person sent whose name the session has for another file
    /// ([`wc_share::Refusal::PictureNameTaken`]), since the last call.
    fn picture_names_taken(&mut self) -> Vec<String> {
        match self {
            Role::Host(h) => std::mem::take(&mut h.names_taken),
            Role::Guest(j) => std::mem::take(&mut j.guest.refusals)
                .into_iter()
                .filter_map(|r| match r {
                    wc_share::Refusal::PictureNameTaken { file } => Some(file),
                    _ => None,
                })
                .collect(),
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
                let updated = edit.updated_items();
                match h.host.edit_project(edit) {
                    Ok(()) => h.took_updates.extend(updated),
                    // Given a name of its own on the next look, as a guest's picture is.
                    Err(wc_share::Refusal::PictureNameTaken { file }) => h.names_taken.push(file),
                    Err(e) => notes.push(format!("Your change was not kept: {e}")),
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

    /// The items of this person's updates the session took since the last call, as
    /// [`Guest::took_updates`].
    fn took_updates(&mut self) -> Vec<(ItemKind, String, String)> {
        match self {
            Role::Host(h) => std::mem::take(&mut h.took_updates),
            Role::Guest(j) => std::mem::take(&mut j.guest.took_updates),
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
                        Ok(Some(msg)) => {
                            j.guest.handle(msg);
                            // Nothing after a welcome from another version can be read safely,
                            // and a close behind it would hide why.
                            if matches!(j.guest.status, GuestStatus::WrongVersion(_)) {
                                break;
                            }
                        }
                        Ok(None) => break,
                        Err(_) => {
                            // A host that declined or ended the session closes the connection
                            // after saying so; only an unexplained close is news.
                            if !matches!(
                                j.guest.status,
                                GuestStatus::Declined | GuestStatus::WrongVersion(_) | GuestStatus::Closed
                            ) {
                                notes.push(closed_note(&j.link));
                                j.guest.status = GuestStatus::Closed;
                            }
                            break;
                        }
                    }
                }
                // A picture name the session has for another file is handled by renaming the
                // picture ([`Role::picture_names_taken`]), not reported.
                let (taken, other): (Vec<_>, Vec<_>) = std::mem::take(&mut j.guest.refusals)
                    .into_iter()
                    .partition(|r| matches!(r, wc_share::Refusal::PictureNameTaken { .. }));
                j.guest.refusals = taken;
                for r in other {
                    notes.push(format!("The host did not take a change: {r}"));
                }
                match j.guest.status {
                    GuestStatus::Declined => {
                        notes.push("The host declined your request to join.".into());
                        false
                    }
                    GuestStatus::WrongVersion(v) => {
                        notes.push(format!("You cannot join this session: {v}."));
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
    /// The field `error` is about.
    error_about: ErrorAbout,
}

/// Which field of the join form an error is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ErrorAbout {
    /// The host: it could not be reached, or it refused.
    #[default]
    Address,
    /// The folder to keep the copy in.
    Folder,
}

impl Form {
    /// Show an error about one field.
    fn fail(&mut self, about: ErrorAbout, error: String) {
        self.error = Some(error);
        self.error_about = about;
    }

    /// An error is about the field it was for: once that field is changed (another address
    /// picked or typed, another folder typed), it no longer applies. `addr` and `folder` are
    /// the fields as they were before this frame's edits.
    fn forget_error_unless(&mut self, addr: &str, folder: &str) {
        let changed = match self.error_about {
            ErrorAbout::Address => self.addr != addr,
            ErrorAbout::Folder => self.folder != folder,
        };
        if changed {
            self.error = None;
        }
    }
}

/// How a discovered session is listed in the Join dialog, and whether it can be joined.
fn session_row(s: &wc_share::discovery::Session) -> (String, bool) {
    let mut label = format!("{}  ·  {}  ({})", s.project, s.host, s.addr);
    let joinable = s.protocol == wc_share::PROTOCOL_VERSION;
    if !joinable {
        label.push_str(&format!(
            "  ·  WiringCraft session version {}, this one uses {}; cannot be joined",
            s.protocol,
            wc_share::PROTOCOL_VERSION
        ));
    }
    (label, joinable)
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
            error_about: ErrorAbout::Address,
        }
    }
}

/// The shared session, if there is one, and its windows.
#[derive(Default)]
pub struct Share {
    pub role: Option<Role>,
    /// The open project as of the last sync.
    base: Option<Project>,
    /// The session's symbol lock as of the last sync: an item it no longer locks was taken back.
    session_lock: Option<wc_core::Lockfile>,
    /// Items the session took back whose copies wait to be unloaded until nothing here uses
    /// them ([`unload_taken_back`]).
    unload: std::collections::BTreeSet<(ItemKind, String)>,
    /// Items the session took back whose entries in the symbol and part lists wait for the
    /// session's change to be in ([`delist_taken_back`]).
    delist: std::collections::BTreeSet<(ItemKind, String)>,
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
    /// The items this copy's waiting changes wait for ([`Held`]), as of the last sync.
    holding: Vec<String>,
    /// Undo asked for while changes were waiting: they are taken back here at the next sync,
    /// instead of an older change in the session.
    drop_held: bool,
    /// Items whose file could not be read, sent or taken, and updates not sent because the
    /// session took their item back (`stale <name>`), reported once each.
    unreadable: std::collections::BTreeSet<String>,
    /// Pictures looked at for the session and not taken yet, by project path, with when: each
    /// is looked at again only after [`ITEM_RETRY_LATER`]. Changes never wait for a picture.
    pictures_out: BTreeMap<String, std::time::Instant>,
    /// Pictures this copy sent to the session, by project path: only a refusal of one of
    /// these makes this copy give a picture a name of its own. Kept while the session lasts,
    /// as a refusal can arrive after the session's own picture of that name.
    pictures_sent: std::collections::BTreeSet<String>,
    /// The session's pictures written into this copy (or found there already), by project path,
    /// with the file's version then: one replaced here since goes out again.
    pictures_taken: BTreeMap<String, PictureVersion>,
    /// Pictures sent as a replacement of the session's file of that name: waited for until the
    /// session refuses the name (and this copy renames its file), not taken as arrived.
    pictures_replaced: std::collections::BTreeSet<String>,
    /// Session pictures that could not be written into this copy, with when: each is tried
    /// again only after [`ITEM_RETRY_LATER`].
    pictures_failed: BTreeMap<String, std::time::Instant>,
    /// Pictures whose name the session has for another file and that could not be given a new
    /// name here, with when: tried again after [`ITEM_RETRY_LATER`] while the project uses them.
    renames_due: BTreeMap<String, std::time::Instant>,
    /// Pictures picked here under a name the session has for another file, found while taking
    /// the session's pictures: they get a name of their own at the next sync, before what goes
    /// out is worked out.
    clashes: Vec<String>,
    /// Files the session lacks ([`repairs_due`]) whose copy here was looked for and could not
    /// be sent (missing, unreadable, or another version), with when and [`repair_stamp`] then:
    /// each is read again only after [`ITEM_WAIT`], or at once once this copy pins or loads
    /// another version (putting the pinned copy back from a library does), so a copy put back
    /// is still found without reading files every frame.
    repair_reads: BTreeMap<String, (std::time::Instant, RepairStamp)>,
}

/// Changes of this copy kept back while the items they use are not in the session yet: the
/// pages, devices and project properties that use such an item for the first time. Everything
/// else goes out as usual.
#[derive(Debug, Default)]
struct Held {
    /// Those targets as this copy has them.
    now: wc_share::Parts,
    /// The same targets as the copy had synced them.
    was: wc_share::Parts,
    /// The items they wait for, by [`item_name`].
    items: Vec<String>,
    /// The targets of each device rename that waits ([`Held::hold_renames`]): each group is
    /// put back all together or not at all; separate renames are kept or lost on their own.
    renames: Vec<RenameGroup>,
}

/// See [`Held::renames`].
#[derive(Debug, Default)]
struct RenameGroup {
    pages: BTreeSet<String>,
    devices: BTreeSet<wc_core::DeviceTag>,
    panels: BTreeSet<String>,
    meta: bool,
}

impl RenameGroup {
    fn merge(&mut self, other: RenameGroup) {
        self.pages.extend(other.pages);
        self.devices.extend(other.devices);
        self.panels.extend(other.panels);
        self.meta |= other.meta;
    }
}

impl Held {
    /// The targets of `local` that use, for the first time since `base`, an item named in
    /// `waiting`.
    fn find(local: &Project, base: &Project, waiting: &BTreeSet<String>) -> Held {
        let waits = |kind: ItemKind, id: &str| waiting.contains(&item_name(kind, id));
        let fresh = FreshUses::find(local, base);
        let mut held = Held::default();
        let mut items = std::collections::BTreeSet::new();
        for (id, symbols) in &fresh.pages {
            let new: Vec<&&str> = symbols.iter().filter(|s| waits(ItemKind::Symbol, s)).collect();
            if !new.is_empty() {
                items.extend(new.iter().map(|id| item_name(ItemKind::Symbol, id)));
                held.now.pages.insert((*id).to_owned(), local.page(id).cloned());
            }
        }
        for (tag, parts) in &fresh.devices {
            let new: Vec<&&str> = parts.iter().filter(|p| waits(ItemKind::Part, p)).collect();
            if !new.is_empty() {
                items.extend(new.iter().map(|id| item_name(ItemKind::Part, id)));
                held.now
                    .devices
                    .insert((*tag).clone(), local.devices.get(*tag).cloned());
            }
        }
        let jumper_parts: Vec<&&str> = fresh.jumpers.iter().filter(|p| waits(ItemKind::Part, p)).collect();
        if !jumper_parts.is_empty() {
            items.extend(jumper_parts.iter().map(|id| item_name(ItemKind::Part, id)));
            held.now.meta = Some(Box::new(local.meta.clone()));
        }
        held.hold_renames(local, base);
        // A new page's comments wait with it, however it came to wait: the session has no page
        // to put them on.
        let new_pages: Vec<String> = held
            .now
            .pages
            .keys()
            .filter(|id| base.page(id).is_none())
            .cloned()
            .collect();
        for id in new_pages {
            let on: Vec<_> = local.comments.iter().filter(|c| c.page == id).cloned().collect();
            held.now.comments.insert(id, on);
        }
        held.was = wc_share::Parts::read(base, &held.now);
        held.items = items.into_iter().collect();
        held
    }

    /// A held device the base does not have may be a rename: the old device going away and
    /// what was retagged to the new tag wait with it, so the session never has the new tag
    /// without its device, nor loses the old one first.
    fn hold_renames(&mut self, local: &Project, base: &Project) {
        let new_tags: Vec<wc_core::DeviceTag> = self
            .now
            .devices
            .keys()
            .filter(|t| !base.devices.contains_key(*t))
            .cloned()
            .collect();
        if new_tags.is_empty() {
            return;
        }
        let named = |t: &wc_core::DeviceTag| new_tags.contains(t);
        let gone: Vec<wc_core::DeviceTag> = base
            .devices
            .keys()
            .filter(|t| !local.devices.contains_key(*t))
            .cloned()
            .collect();
        // Only a device gone since `base` whose placements or cables now name a new tag instead
        // is the old side of a rename; other devices removed here go out as usual. Symbols and
        // wires keep their ids, so each one names its own pair.
        let mut pairs: BTreeSet<(wc_core::DeviceTag, wc_core::DeviceTag)> = BTreeSet::new();
        for b in &base.pages {
            let Some(l) = local.page(&b.id) else { continue };
            for s in &b.symbols {
                if let Some(old) = s.tag.as_ref().filter(|t| gone.contains(t))
                    && let Some(new) = l.symbols.iter().find(|n| n.id == s.id).and_then(|n| n.tag.as_ref())
                    && named(new)
                {
                    pairs.insert((old.clone(), new.clone()));
                }
            }
            for w in &b.wires {
                if let Some(old) = w.cable.as_ref().map(|c| &c.cable).filter(|t| gone.contains(t))
                    && let Some(new) = l.wires.iter().find(|n| n.id == w.id).and_then(|n| n.cable.as_ref())
                    && named(&new.cable)
                {
                    pairs.insert((old.clone(), new.cable.clone()));
                }
            }
        }
        // Panel footprints and jumpers carry no id: an old tag whose entries, in order, are
        // exactly a new tag's entries now, and that kept none under its own tag, is renamed to
        // it when no other tag matches either side. A removed entry may shift the others, so the place in the list says nothing.
        use wc_core::panel::PanelItem;
        let mut by_content: BTreeSet<(wc_core::DeviceTag, wc_core::DeviceTag)> = BTreeSet::new();
        for b in &base.panels {
            let Some(l) = local.panels.iter().find(|l| l.id == b.id) else {
                continue;
            };
            let footprints = |p: &wc_core::panel::Panel, tag: &wc_core::DeviceTag| -> Vec<String> {
                p.items
                    .iter()
                    .filter_map(|i| match i {
                        PanelItem::Device {
                            tag: t,
                            at,
                            width,
                            height,
                        } if t == tag => Some(format!("{at:?} {width} {height}")),
                        _ => None,
                    })
                    .collect()
            };
            for old in &gone {
                let was = footprints(b, old);
                if was.is_empty() || !footprints(l, old).is_empty() {
                    continue;
                }
                for new in &new_tags {
                    if footprints(l, new) == was && footprints(b, new).is_empty() {
                        by_content.insert((old.clone(), new.clone()));
                    }
                }
            }
        }
        let strips = |p: &Project, tag: &wc_core::DeviceTag| -> Vec<(Vec<String>, Option<String>)> {
            p.meta
                .jumpers
                .iter()
                .filter(|j| &j.strip == tag)
                .map(|j| (j.terminals.clone(), j.part.clone()))
                .collect()
        };
        for old in &gone {
            let was = strips(base, old);
            if was.is_empty() || !strips(local, old).is_empty() {
                continue;
            }
            for new in &new_tags {
                if strips(local, new) == was && strips(base, new).is_empty() {
                    by_content.insert((old.clone(), new.clone()));
                }
            }
        }
        for (old, new) in &by_content {
            let unique = by_content.iter().filter(|(o, _)| o == old).count() == 1
                && by_content.iter().filter(|(_, n)| n == new).count() == 1;
            if unique {
                pairs.insert((old.clone(), new.clone()));
            }
        }
        // Each rename is its own group; renames that share a tag or a target wait as one.
        let mut groups: Vec<RenameGroup> = Vec::new();
        let join = |groups: &mut Vec<RenameGroup>, tags: &[&wc_core::DeviceTag]| -> usize {
            let hit: Vec<usize> = (0..groups.len())
                .filter(|&i| tags.iter().any(|t| groups[i].devices.contains(*t)))
                .collect();
            let mut into = match hit.first() {
                Some(&i) => i,
                None => {
                    groups.push(RenameGroup::default());
                    groups.len() - 1
                }
            };
            for &i in hit.iter().skip(1).rev() {
                let g = groups.remove(i);
                if i < into {
                    into -= 1;
                }
                groups[into].merge(g);
            }
            groups[into].devices.extend(tags.iter().map(|t| (*t).clone()));
            into
        };
        for (old, new) in &pairs {
            self.now.devices.insert(old.clone(), None);
            join(&mut groups, &[old, new]);
        }
        let paired: Vec<wc_core::DeviceTag> = pairs.iter().map(|(_, n)| n.clone()).collect();
        // The paired new tags among those a target names.
        let renamed_in = |names: &dyn Fn(&wc_core::DeviceTag) -> bool| -> Vec<&wc_core::DeviceTag> {
            paired.iter().filter(|t| names(t)).collect()
        };
        for p in &local.pages {
            if page_mentions(p, &named) && base.page(&p.id) != Some(p) {
                self.now.pages.insert(p.id.clone(), Some(p.clone()));
                let tags = renamed_in(&|t| page_mentions(p, &|x| x == t));
                if !tags.is_empty() {
                    let g = join(&mut groups, &tags);
                    groups[g].pages.insert(p.id.clone());
                }
            }
        }
        for panel in &local.panels {
            if panel_mentions(panel, &named) && base.panels.iter().find(|b| b.id == panel.id) != Some(panel) {
                self.now.panels.insert(panel.id.clone(), Some(panel.clone()));
                let tags = renamed_in(&|t| panel_mentions(panel, &|x| x == t));
                if !tags.is_empty() {
                    let g = join(&mut groups, &tags);
                    groups[g].panels.insert(panel.id.clone());
                }
            }
        }
        if local.meta != base.meta && local.meta.jumpers.iter().any(|j| named(&j.strip)) {
            self.now.meta = Some(Box::new(local.meta.clone()));
            let tags = renamed_in(&|t| local.meta.jumpers.iter().any(|j| &j.strip == t));
            if !tags.is_empty() {
                let g = join(&mut groups, &tags);
                groups[g].meta = true;
            }
        }
        self.renames = groups;
    }

    fn is_empty(&self) -> bool {
        self.now.is_empty()
    }

    /// Put the held changes back into `project`, which now holds the session's project. A target
    /// the session changed meanwhile keeps the session's version; those are returned. The
    /// targets of a waiting rename go together: when one of them is lost, all are. A page
    /// counts as unchanged when only its number changed, and keeps the session's number unless
    /// this copy renumbered it.
    fn put_back(&self, project: &mut Project) -> Vec<String> {
        let mut lost = Vec::new();
        let mut gone = RenameGroup::default();
        let mut keep = wc_share::Parts::default();
        for (id, now) in &self.now.pages {
            let Some(now) = now else { continue };
            let was = self.was.pages.get(id).cloned().flatten();
            match (project.page(id), was) {
                (Some(session), Some(was)) if wc_share::same_but_number(session, &was) => {
                    let mut page = now.clone();
                    if now.number == was.number {
                        page.number = session.number.clone();
                    }
                    keep.pages.insert(id.clone(), Some(page));
                }
                // A page new here, and still free in the session.
                (None, None) => {
                    keep.pages.insert(id.clone(), Some(now.clone()));
                }
                _ => {
                    gone.pages.insert(id.clone());
                    lost.push(format!("page {}", now.number));
                }
            }
        }
        for (tag, now) in &self.now.devices {
            let was = self.was.devices.get(tag).cloned().flatten();
            if project.devices.get(tag) == was.as_ref() {
                keep.devices.insert(tag.clone(), now.clone());
            } else {
                gone.devices.insert(tag.clone());
                lost.push(format!("device {tag}"));
            }
        }
        for (id, now) in &self.now.panels {
            let was = self.was.panels.get(id).cloned().flatten();
            if project.panels.iter().find(|p| &p.id == id) == was.as_ref() {
                keep.panels.insert(id.clone(), now.clone());
            } else {
                gone.panels.insert(id.clone());
                lost.push(format!("panel {id}"));
            }
        }
        for (page, now) in &self.now.comments {
            let was = self.was.comments.get(page).cloned().unwrap_or_default();
            let session: Vec<_> = project.comments.iter().filter(|c| &c.page == page).cloned().collect();
            if session == was {
                keep.comments.insert(page.clone(), now.clone());
            } else {
                lost.push(format!("the comments on page {page}"));
            }
        }
        if let (Some(now), Some(was)) = (&self.now.meta, &self.was.meta) {
            if project.meta == **was {
                keep.meta = Some(now.clone());
            } else {
                gone.meta = true;
                lost.push("the project properties".into());
            }
        }
        // A rename whose page the session changed must not leave the new tag here without the
        // page that uses it, nor the page without its device.
        for group in &self.renames {
            let hit = !group.pages.is_disjoint(&gone.pages)
                || !group.devices.is_disjoint(&gone.devices)
                || !group.panels.is_disjoint(&gone.panels)
                || (group.meta && gone.meta);
            if !hit {
                continue;
            }
            keep.pages.retain(|id, _| !group.pages.contains(id));
            keep.comments.retain(|id, _| !group.pages.contains(id));
            keep.devices.retain(|tag, _| !group.devices.contains(tag));
            keep.panels.retain(|id, _| !group.panels.contains(id));
            if group.meta {
                keep.meta = None;
            }
            lost.push("the device rename that waited with it".into());
        }
        keep.apply(project);
        // The items they use stay locked here: `incoming` keeps entries the session lacks.
        lost
    }

    /// Take the held changes back in `project`: each target as the copy had synced it.
    fn drop_from(&self, project: &mut Project) {
        self.was.apply(project);
    }
}

/// Whether a symbol or cable on `page` names a device `tag` picks.
fn page_mentions(page: &Page, tag: &dyn Fn(&wc_core::DeviceTag) -> bool) -> bool {
    page.symbols.iter().any(|s| s.tag.as_ref().is_some_and(tag))
        || page
            .wires
            .iter()
            .any(|w| w.cable.as_ref().is_some_and(|c| tag(&c.cable)))
}

/// Whether `panel` places a device `tag` picks.
fn panel_mentions(panel: &wc_core::panel::Panel, tag: &dyn Fn(&wc_core::DeviceTag) -> bool) -> bool {
    panel
        .items
        .iter()
        .any(|i| matches!(i, wc_core::panel::PanelItem::Device { tag: t, .. } if tag(t)))
}

/// One symbol or part sent to the session and not taken yet.
#[derive(Debug, Clone)]
struct ItemOut {
    at: std::time::Instant,
    tries: u32,
    /// Not taken after [`ITEM_TRIES`] tries: changes no longer wait for it, and it is sent again
    /// only every [`ITEM_RETRY_LATER`].
    gave_up: bool,
    /// The version (lock hash here) it was sent in: a later version of the item is another
    /// send, not held back by this one's wait.
    hash: Option<String>,
}

/// The hash this copy locks the item [`item_name`] names in.
fn local_hash(local: &Project, name: &str) -> Option<String> {
    named(name).and_then(|(kind, id)| local.lock.map(kind).get(id).map(|e| e.hash.clone()))
}

/// The version of the item [`item_name`] names that `open` pins, and of the copy it has loaded.
type RepairStamp = (Option<String>, Option<String>);

/// See [`RepairStamp`].
fn repair_stamp(open: &Open, name: &str) -> RepairStamp {
    let loaded = named(name).and_then(|(kind, id)| match kind {
        ItemKind::Symbol => open.symbols.get(id).map(Symbol::hash),
        ItemKind::Part => open.parts.get(id).map(Part::hash),
    });
    (local_hash(&open.project, name), loaded)
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
            // Sessions heard by an earlier browser may be gone; listing them would also hide
            // why browsing cannot start again.
            form.sessions.clear();
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
        // A change waiting here is undone here (see `take_back`), even with nothing in the
        // session's history yet.
        !self.holding.is_empty()
            || self
                .role
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
        // The latest change here never reached the session: undo it here, not an older one there.
        if !redo && change.is_none() && !self.holding.is_empty() {
            self.drop_held = true;
            return Ok(());
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
            protocol: wc_share::PROTOCOL_VERSION,
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
        let pictures = wc_pdf::read_pictures(&open.dir, &open.project);
        if !pictures.unreadable.is_empty() {
            let names: Vec<String> = pictures
                .unreadable
                .iter()
                .map(|(file, why)| format!("{file} ({why})"))
                .collect();
            self.notes.push(format!(
                "These pictures cannot be shared, so guests will not see them: {}",
                names.join(", ")
            ));
        }
        host.share_pictures(pictures.files);
        self.role = Some(Role::Host(Box::new(Hosting {
            host,
            hub: Hub::new(),
            listener: Some(listener),
            announcer,
            port,
            dir: open.dir.clone(),
            took_updates: Vec::new(),
            names_taken: Vec::new(),
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
        self.session_lock = None;
        self.unload.clear();
        self.delist.clear();
        self.unreadable.clear();
        self.pictures_out.clear();
        self.pictures_sent.clear();
        self.pictures_taken.clear();
        self.pictures_replaced.clear();
        self.pictures_failed.clear();
        self.holding.clear();
        self.drop_held = false;
        self.renames_due.clear();
        self.clashes.clear();
        self.repair_reads.clear();
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
    pub fn frame(&mut self, mut open: Option<&mut Open>, page: Option<String>, busy: bool) -> Option<Open> {
        let role = self.role.as_mut()?;
        if !role.pump(&mut self.notes) {
            // A refusal that came with the end of the session still takes its page back: the
            // action is not left half kept in this copy.
            if let Some(open) = open.as_deref_mut()
                && role.dir().is_some_and(|d| d == open.dir)
            {
                drain_retaken(role, open, None, &mut self.notes);
            }
            // A join that was turned away goes back to the Join dialog, which says why: always
            // for another version (the person has to update one side), for a plain decline only
            // while the status window is still open (the note says it otherwise).
            let shown = self.view == View::Status;
            let refused = match role {
                Role::Guest(j) => match &j.guest.status {
                    GuestStatus::WrongVersion(v) => Some(format!("You cannot join this session: {v}.")),
                    GuestStatus::Declined if shown => Some("The host declined your request to join.".to_owned()),
                    _ => None,
                },
                Role::Host(_) => None,
            };
            self.role = None;
            self.base = None;
            self.forget_items();
            if let Some(why) = refused {
                self.show_join();
                if let Some(f) = self.form.as_mut() {
                    f.fail(ErrorAbout::Address, why);
                }
            }
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
                    self.session_lock = Some(project.lock.clone());
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
        // The session refused an action that changed this person's own page with project
        // records: the page takes the session's version, also over what was changed on it while
        // the answer was on its way, so the action is not half kept.
        let base = self.base.get_or_insert_with(|| open.project.clone());
        drain_retaken(role, open, Some(base), &mut self.notes);
        // While such a change is on its way, later changes wait in the window: sent now, they
        // could not be taken back with it.
        if role.own_page_waits() {
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
        take_updates(role, base, &mut self.items_out);
        // Symbols and parts used for the first time go out first. Until the session has them,
        // the changes that may draw them wait in the window: a page must not reach the session
        // without its symbols, and an item the session refused is sent again until it is in.
        let auth = role.project()?;
        // Items the session took back are dropped here first, or they would look new below.
        follow_take_backs(
            &mut self.session_lock,
            &mut self.unload,
            &mut self.delist,
            open,
            base,
            auth,
        );
        // An item the session took is done; one this copy no longer locks (an undo, or an
        // assistant command that rewrote the project) is not sent again, so nothing waits for it.
        // A file the session still lacks is not taken, though its pin is there.
        let repair = repairs_due(role);
        self.items_out.retain(|name, _| {
            (!session_took(auth, &open.project, base, name) || repair.contains(name))
                && session_has(&open.project, name)
        });
        for (name, out) in self.items_out.iter_mut() {
            if !out.gave_up && out.tries >= ITEM_TRIES && out.at.elapsed() >= ITEM_WAIT {
                out.gave_up = true;
                // An update is not kept apart from the project: the copy takes the session's
                // version again below, as everyone else works on that one.
                self.notes.push(if repair.contains(name) {
                    format!("The session could not store the file of {name}; others without the library cannot draw it until it can.")
                } else if session_has(auth, name) {
                    format!(
                        "The session did not take your update of {name}; your copy takes the session's version again."
                    )
                } else {
                    format!(
                        "The session did not take {name}; your changes that use it wait until it does, and it is sent again now and then."
                    )
                });
            }
        }
        // A file the session no longer lacks is no repair to look for.
        self.repair_reads.retain(|name, _| repair.contains(name));
        let skip: BTreeSet<String> = self
            .items_out
            .iter()
            // One given up is still sent now and then, so it gets in once its file can be read
            // or the session can store it; changes no longer wait for it.
            // A version changed here since is sent at once.
            .filter(|(n, o)| {
                o.at.elapsed() < if o.gave_up { ITEM_RETRY_LATER } else { ITEM_WAIT }
                    && o.hash == local_hash(&open.project, n)
            })
            .map(|(n, _)| n.clone())
            .chain(
                self.repair_reads
                    .iter()
                    .filter(|(n, (at, stamp))| at.elapsed() < ITEM_WAIT && *stamp == repair_stamp(open, n))
                    .map(|(n, _)| n.clone()),
            )
            .collect();
        // An item sent before and not taken goes alone, so one the session cannot store does
        // not keep out the others it was sent with.
        let apart = self.items_out.keys().cloned().collect();
        // The host's session files: one it locks but has no file of goes out again from here.
        let items = new_items(&open.dir, &open.project, base, auth, &skip, &apart, &repair);
        let now = std::time::Instant::now();
        // A repair looked for and not sent is looked for again only a while later.
        for name in repair.iter().filter(|n| !skip.contains(*n)) {
            if !items.sent.contains(name) && !items.unreadable.iter().any(|(n, _)| n == name) {
                self.repair_reads.insert(name.clone(), (now, repair_stamp(open, name)));
            }
        }
        for (name, why) in &items.unsent_repairs {
            if self.unreadable.insert(format!("repair {name}")) {
                self.notes.push(format!(
                    "The file of {name} is back but cannot be shared ({why}); others without the library cannot draw it."
                ));
            }
        }
        for (name, why) in &items.unreadable {
            // A file that cannot be read counts as a try: the changes that may draw the item
            // wait for it as for a sent one, and go once the tries are used up.
            let hash = local_hash(&open.project, name);
            let out = self.items_out.entry(name.clone()).or_insert(ItemOut {
                at: now,
                tries: 0,
                gave_up: false,
                hash: hash.clone(),
            });
            // Another version starts its tries afresh.
            if out.hash != hash {
                *out = ItemOut {
                    at: now,
                    tries: 0,
                    gave_up: false,
                    hash,
                };
            }
            out.at = now;
            out.tries += 1;
            if self.unreadable.insert(name.clone()) {
                self.notes.push(format!(
                    "The file of {name} could not be read ({why}); others cannot draw it until it can."
                ));
            }
        }
        for name in &items.stale {
            if self.unreadable.insert(format!("stale {name}")) {
                self.notes.push(format!(
                    "Your update of {name} was not sent: the session took that item back meanwhile."
                ));
            }
        }
        for name in &items.sent {
            self.unreadable.remove(name);
            let hash = local_hash(&open.project, name);
            let out = self.items_out.entry(name.clone()).or_insert(ItemOut {
                at: now,
                tries: 0,
                gave_up: false,
                hash: hash.clone(),
            });
            // Another version starts its tries afresh.
            if out.hash != hash {
                *out = ItemOut {
                    at: now,
                    tries: 0,
                    gave_up: false,
                    hash,
                };
            }
            out.at = now;
            out.tries += 1;
        }
        if !items.edits.is_empty() {
            for edit in items.edits {
                role.edit_project(edit, &mut self.notes);
            }
            role.pump(&mut self.notes);
            take_updates(role, base, &mut self.items_out);
            let repair = repairs_due(role);
            let auth = role.project()?;
            self.items_out
                .retain(|name, _| !session_took(auth, &open.project, base, name) || repair.contains(name));
            // What the session took in is the baseline now, so a take-back before the last
            // look of this frame is still seen.
            follow_take_backs(
                &mut self.session_lock,
                &mut self.unload,
                &mut self.delist,
                open,
                base,
                auth,
            );
        }
        // A picture whose name the session (or the host's folder) has for another file gets a
        // new name here; the picture and the changes naming it go out again under that name.
        // Only a refusal of a picture this copy sent counts: a refusal naming any other path
        // must not make this copy share a file of its own.
        let mut taken: Vec<String> = role
            .picture_names_taken()
            .into_iter()
            .filter(|f| self.pictures_sent.contains(f))
            .collect();
        taken.append(&mut self.clashes);
        rename_pictures(
            open,
            taken,
            role.pictures(),
            Pictures {
                renames_due: &mut self.renames_due,
                pictures_out: &mut self.pictures_out,
                pictures_taken: &mut self.pictures_taken,
                replaced: &mut self.pictures_replaced,
            },
            &mut self.notes,
        );
        // Pictures used for the first time go out before the change that names them. Changes do
        // not wait for them: without its picture a page still works, it just shows a gap.
        let session_pictures = role.pictures();
        let replaced = &self.pictures_replaced;
        self.pictures_out.retain(|file, at| {
            (!session_pictures.contains_key(file) || replaced.contains(file)) && at.elapsed() < ITEM_RETRY_LATER
        });
        let skip = self.pictures_out.keys().cloned().collect();
        let pictures = new_pictures(&open.dir, &open.project, session_pictures, &self.pictures_taken, &skip);
        for file in pictures.tried {
            self.pictures_out.insert(file, now);
        }
        self.pictures_replaced.extend(pictures.replaced);
        self.pictures_taken.extend(pictures.same);
        for (file, why) in pictures.unreadable {
            if self.unreadable.insert(format!("picture {file}")) {
                self.notes.push(format!(
                    "The picture {file} cannot be shared ({why}); others will not see it."
                ));
            }
        }
        for edit in pictures.edits {
            if let ProjectEdit::AddPicture { picture } = &edit {
                self.pictures_sent.insert(picture.file.clone());
            }
            role.edit_project(edit, &mut self.notes);
        }
        // While an item is on its way (a few tries, seconds at most) everything waits: what
        // comes in would put the session's lock entry back over an update sent from here. A
        // missing file sent again in the session's own version is no such item (the session's
        // entry is the one sent), so only the changes that use it wait for it, below. An update
        // of an item whose file the session lacks still holds everything.
        let repair = repairs_due(role);
        let auth = role.project()?;
        let only_repair =
            |name: &String, o: &ItemOut| repair.contains(name) && o.hash.is_some() && o.hash == local_hash(auth, name);
        if self
            .items_out
            .iter()
            .any(|(name, o)| !o.gave_up && !only_repair(name, o))
        {
            return None;
        }
        // After that, only a change that uses an item the session has not taken waits, however
        // long: the session refuses a page that draws a symbol it does not have. Others' changes
        // come in meanwhile; the session never had the item, so nothing replaces its entry here.
        let undo = std::mem::take(&mut self.drop_held);
        // For an Undo, what waited when it was asked waits still, also when its item has come
        // in since: it never reached the session, so it is taken back here, not there.
        let mut waiting: BTreeSet<String> = self.items_out.keys().cloned().collect();
        // A file the session lacks: a new use of its item waits for it too, or a guest without
        // the library gets a placement it cannot draw. Uses the session has already are left.
        waiting.extend(repairs_due(role));
        if undo {
            waiting.extend(self.holding.iter().cloned());
        }
        let held = Held::find(&open.project, self.base.as_ref()?, &waiting);
        if undo && !held.is_empty() {
            // Undo of a change that never reached the session: taken back here only.
            let before = open.project.clone();
            held.drop_from(&mut open.project);
            if open.project != before {
                // Also after a save: the copy on disk still has the change.
                open.dirty = true;
            }
            self.holding.clear();
            self.notes.push(format!(
                "Your change that waited for {} was undone here; it had not reached the session.",
                held.items.join(", ")
            ));
            return None;
        }
        self.holding = held.items.clone();
        // Only what does not wait goes out: the waiting targets as the copy had synced them.
        let sendable = (!held.is_empty()).then(|| {
            let mut p = open.project.clone();
            held.drop_from(&mut p);
            p
        });
        let base = self.base.get_or_insert_with(|| open.project.clone());
        let mut out = outgoing(
            sendable.as_ref().unwrap_or(&open.project),
            base,
            mine,
            &role.locks(),
            me,
        );
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
        fold_own_page(&mut out, base);
        let sent = out.page.is_some() || !out.edits.is_empty();
        // One action, one change: the edits that can go together are checked and recorded by
        // the host as one batch, all or nothing.
        for edit in ProjectEdit::batched(out.edits) {
            role.edit_project(edit, &mut self.notes);
        }
        if let Some(p) = out.page {
            role.edit_page(p, &mut self.notes);
        }
        if sent {
            // The host answers what was sent this frame at once; the reply goes out now.
            role.pump(&mut self.notes);
        }
        let base = self.base.get_or_insert_with(|| open.project.clone());
        // A refusal in that answer takes its pages back now, before anything is done on them.
        drain_retaken(role, open, Some(base), &mut self.notes);
        take_updates(role, base, &mut self.items_out);
        let auth = role.project()?;
        let conflicting = conflicts(&open.project, base, auth);
        if !conflicting.is_empty() {
            self.notes.push(format!(
                "Someone else used {} first in another version; your copy takes theirs.",
                conflicting.join(", ")
            ));
        }
        follow_take_backs(
            &mut self.session_lock,
            &mut self.unload,
            &mut self.delist,
            open,
            base,
            auth,
        );
        // Pictures this copy's own changes name: a clash among them is a file picked here.
        let base_uses = used_pictures(base);
        let named_here: std::collections::BTreeSet<String> = used_pictures(&open.project)
            .into_iter()
            .filter(|f| !base_uses.contains(f))
            .collect();
        let before = (!held.is_empty()).then(|| open.project.clone());
        let mut changed = incoming(&mut open.project, auth);
        // What this copy has synced to, with the session's lock; the waiting changes go on top
        // and are found again next time.
        let synced = before.as_ref().map(|_| {
            let mut p = open.project.clone();
            p.lock = auth.lock.clone();
            p
        });
        if let Some(before) = &before {
            let lost = held.put_back(&mut open.project);
            if !lost.is_empty() {
                self.notes.push(format!(
                    "Your waiting change to {} was not kept: the session changed it meanwhile.",
                    lost.join(", ")
                ));
            }
            changed = open.project != *before;
        }
        // The session's versions are what this copy has synced to: an item updated here later
        // differs from them ([`updated_here`]).
        if let Some(base) = self.base.as_mut() {
            base.lock = auth.lock.clone();
        }
        // This copy now holds the session's versions: a take-back of those is what is followed.
        if let Some(seen) = self.session_lock.as_mut() {
            *seen = next_seen(seen, &open.project, auth);
        }
        unload_taken_back(&mut self.unload, open);
        if let Some(why) = delist_taken_back(&mut self.delist, open)
            && self.unreadable.insert("library".into())
        {
            self.notes.push(why);
        }
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
        self.pictures_failed.retain(|_, at| at.elapsed() < ITEM_RETRY_LATER);
        // A clash whose rename waits to be tried again is not compared again meanwhile.
        let skip = self
            .pictures_failed
            .keys()
            .chain(
                self.renames_due
                    .iter()
                    .filter(|(_, at)| at.elapsed() < ITEM_RETRY_LATER)
                    .map(|(f, _)| f),
            )
            .cloned()
            .collect();
        let TakenPictures { failed, clashes } = take_pictures(open, role.pictures(), &mut self.pictures_taken, &skip);
        // A picture picked here under a name the session has for another file gets a name of
        // its own at the next sync, as when the session refuses it. One only the session's
        // drawings name is theirs: the file here is kept, but neither renamed nor shared.
        let (mine, theirs): (Vec<_>, Vec<_>) = clashes.into_iter().partition(|f| named_here.contains(f));
        self.clashes = mine;
        let now = std::time::Instant::now();
        for file in theirs {
            if self.unreadable.insert(format!("clash {file}")) {
                self.notes.push(format!(
                    "The session's picture {file} differs from your file of that name; your copy keeps showing your file."
                ));
            }
            // Compared again only after a while, as a picture that could not be written: once
            // the file here is gone, the session's picture comes in.
            self.pictures_failed.insert(file, now);
        }
        for (file, _) in &failed {
            self.pictures_failed.insert(file.clone(), now);
        }
        let failed: Vec<String> = failed
            .into_iter()
            .filter(|(file, _)| self.unreadable.insert(format!("picture {file}")))
            .map(|(file, why)| format!("{file} ({why})"))
            .collect();
        if !failed.is_empty() {
            self.notes.push(format!(
                "Pictures from the session could not be written into your copy: {}",
                failed.join(", ")
            ));
        }
        if changed {
            // Others' changes are in the window, not yet in this copy's files.
            open.dirty = true;
        }
        if let Some(synced) = synced {
            if changed || sent || !out.dropped.is_empty() {
                open.forget_terminal_change();
            }
            self.base = Some(synced);
        } else if changed || sent || !out.dropped.is_empty() {
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
                let typed = (form.addr.clone(), form.folder.clone());
                for s in &form.sessions {
                    let (label, joinable) = session_row(s);
                    // A session on another version cannot be joined, so it cannot be picked either.
                    let row = egui::SelectableLabel::new(form.addr == s.addr.to_string(), label);
                    if ui.add_enabled(joinable, row).clicked() {
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
                form.forget_error_unless(&typed.0, &typed.1);
                if let Some(e) = &form.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                join = ui
                    .add_enabled(!form.addr.trim().is_empty(), egui::Button::new("Ask to join"))
                    .clicked();
            });
        if join {
            self.ask_to_join(crate::prefs::project_path);
        } else if !keep {
            if let Some(f) = self.form.as_mut() {
                f.browser = None;
            }
            self.view = View::Closed;
        }
    }

    /// "Ask to join" with what the join form holds. `folder` resolves the typed folder (see
    /// [`crate::prefs::project_path`]); an error is shown on the field it is about.
    fn ask_to_join(&mut self, folder: impl FnOnce(&str) -> Result<PathBuf, String>) {
        let Some(form) = self.form.as_ref() else {
            return;
        };
        let (addr, name, email) = (form.addr.clone(), form.name.clone(), form.email.clone());
        let joined = folder(&form.folder)
            .map_err(|e| (ErrorAbout::Folder, e))
            .and_then(|folder| {
                self.join(&addr, &name, &email, folder)
                    .map_err(|e| (ErrorAbout::Address, e))
            });
        match joined {
            Ok(()) => {
                if let Some(f) = self.form.as_mut() {
                    f.browser = None;
                }
                self.view = View::Status;
            }
            Err((about, e)) => {
                if let Some(f) = self.form.as_mut() {
                    f.fail(about, e);
                }
            }
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
                if !self.holding.is_empty() {
                    ui.label(format!(
                        "Your changes that use {} wait until the session has it.",
                        self.holding.join(", ")
                    ));
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
    // A file the session sent ahead of the change that locks it (an undo's earlier version)
    // waits: the copy takes it once that change is here.
    for (id, s) in symbols {
        if project.lock.symbols.get(id).is_some_and(|e| e.hash != s.hash()) {
            continue;
        }
        store::write_vendored_symbol(&dir, s).map_err(|e| e.to_string())?;
    }
    for (id, p) in parts {
        if project.lock.parts.get(id).is_some_and(|e| e.hash != p.hash()) {
            continue;
        }
        store::write_vendored_part(&dir, p).map_err(|e| e.to_string())?;
    }
    let mut open = Open::load(&dir)?;
    // Symbols the session did not send come from the libraries here, but only in the version the
    // session pinned: another version would make this copy differ from everyone else's.
    let wanted: Vec<(String, String)> = open
        .project
        .lock
        .symbols
        .iter()
        .filter(|(id, _)| !open.symbols.contains_key(*id))
        .map(|(id, e)| (id.clone(), e.hash.clone()))
        .collect();
    let mut missing = Vec::new();
    if !wanted.is_empty() {
        match wc_library::Library::open(&dir, &open.project) {
            Ok(lib) => {
                for (id, pin) in wanted {
                    match lib.symbol(&id) {
                        Ok((s, _)) if s.hash() == pin => {
                            store::write_vendored_symbol(&dir, &s).map_err(|e| e.to_string())?;
                            open.symbols.insert(id, s);
                        }
                        Ok(_) => missing.push(format!("{id} (the library here has another version)")),
                        Err(e) => missing.push(format!("{id} ({e})")),
                    }
                }
            }
            Err(e) => missing.extend(wanted.into_iter().map(|(id, _)| format!("{id} ({e})"))),
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

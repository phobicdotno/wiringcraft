//! Editing several items at once: move, delete, rotate, mirror, copy and paste.

use std::collections::{BTreeMap, BTreeSet};

use wc_core::geom::{Point, Rotation};
use wc_core::{Device, DeviceTag, Endpoint, Page, PlacedSymbol, Wire};

use crate::state::{Open, Selection};

/// Copied items, kept in the app between pages (and projects).
#[derive(Debug, Clone, Default)]
pub struct Clipboard {
    pub symbols: Vec<PlacedSymbol>,
    pub wires: Vec<Wire>,
    /// Devices of the copied symbols, so a paste into another project keeps descriptions and parts.
    pub devices: BTreeMap<DeviceTag, Device>,
    /// Top-left corner of what was copied; a paste puts this corner at the pointer.
    pub anchor: Point,
}

impl Clipboard {
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty() && self.wires.is_empty()
    }

    /// A plain-text line for the system clipboard: the copied device tags.
    pub fn summary(&self) -> String {
        let tags: Vec<String> = self
            .symbols
            .iter()
            .filter_map(|s| s.tag.as_ref().map(ToString::to_string))
            .collect();
        format!(
            "WiringCraft: {} symbols, {} wires {}",
            self.symbols.len(),
            self.wires.len(),
            tags.join(" ")
        )
        .trim_end()
        .to_owned()
    }
}

fn ids(sel: &[Selection]) -> (BTreeSet<&str>, BTreeSet<&str>) {
    let mut syms = BTreeSet::new();
    let mut wires = BTreeSet::new();
    for s in sel {
        match s {
            Selection::Symbol(id) => syms.insert(id.as_str()),
            Selection::Wire(id) => wires.insert(id.as_str()),
        };
    }
    (syms, wires)
}

fn pin_item(e: &Endpoint) -> Option<&str> {
    match e {
        Endpoint::Pin { item, .. } => Some(item),
        Endpoint::Point { .. } => None,
    }
}

/// Wires that move with a selection: the selected ones, and those with both ends on selected
/// symbols (so their bends follow the symbols).
fn carried_wires<'a>(page: &'a Page, syms: &BTreeSet<&str>, wires: &BTreeSet<&str>) -> Vec<&'a Wire> {
    page.wires
        .iter()
        .filter(|w| {
            wires.contains(w.id.as_str())
                || [&w.from, &w.to]
                    .iter()
                    .all(|e| pin_item(e).is_some_and(|i| syms.contains(i)))
        })
        .collect()
}

impl Open {
    /// Every symbol and wire on a page.
    pub fn select_all(&self, page: usize) -> Vec<Selection> {
        let Some(p) = self.project.pages.get(page) else {
            return Vec::new();
        };
        p.symbols
            .iter()
            .map(|s| Selection::Symbol(s.id.clone()))
            .chain(p.wires.iter().map(|w| Selection::Wire(w.id.clone())))
            .collect()
    }

    /// Items lying completely inside the rectangle `a`–`b` (page mm).
    pub fn select_in(&self, page: usize, a: Point, b: Point) -> Vec<Selection> {
        let Some(p) = self.project.pages.get(page) else {
            return Vec::new();
        };
        let (lo, hi) = wc_core::geom::bounds_of(&[a, b]);
        let inside = |q: Point| q.x >= lo.x && q.x <= hi.x && q.y >= lo.y && q.y <= hi.y;
        let mut out = Vec::new();
        for s in &p.symbols {
            if let Some((c, d)) = self.symbol_bounds(p, &s.id)
                && inside(c)
                && inside(d)
            {
                out.push(Selection::Symbol(s.id.clone()));
            }
        }
        for w in &p.wires {
            if let Some(pts) = self.wire_points(p, w)
                && pts.iter().all(|q| inside(*q))
            {
                out.push(Selection::Wire(w.id.clone()));
            }
        }
        out
    }

    /// Move the selection by `d`. The caller takes the undo checkpoint (once per drag).
    pub fn move_by(&mut self, page: usize, sel: &[Selection], d: Point) {
        let (syms, wires) = ids(sel);
        let Some(p) = self.project.pages.get(page) else {
            return;
        };
        let carried: BTreeSet<String> = carried_wires(p, &syms, &wires)
            .into_iter()
            .map(|w| w.id.clone())
            .collect();
        let Some(p) = self.project.pages.get_mut(page) else {
            return;
        };
        for s in &mut p.symbols {
            if syms.contains(s.id.as_str()) {
                s.at = s.at + d;
            }
        }
        for w in &mut p.wires {
            if !carried.contains(&w.id) {
                continue;
            }
            for v in &mut w.via {
                *v = *v + d;
            }
            for e in [&mut w.from, &mut w.to] {
                if let Endpoint::Point { at } = e {
                    *at = *at + d;
                }
            }
        }
        self.dirty = true;
    }

    /// Delete several items in one undo step. Wires on deleted symbols go too.
    pub fn delete_many(&mut self, page: usize, sel: &[Selection]) {
        if sel.is_empty() {
            return;
        }
        self.checkpoint();
        let (syms, wires) = ids(sel);
        let Some(p) = self.project.pages.get_mut(page) else {
            return;
        };
        p.symbols.retain(|s| !syms.contains(s.id.as_str()));
        p.wires.retain(|w| {
            !wires.contains(w.id.as_str())
                && ![&w.from, &w.to]
                    .iter()
                    .any(|e| pin_item(e).is_some_and(|i| syms.contains(i)))
        });
    }

    /// Rotate the selected symbols 90° each, in one undo step.
    pub fn rotate_many(&mut self, page: usize, sel: &[Selection]) {
        self.change_symbols(page, sel, |s| {
            s.rotation = match s.rotation {
                Rotation::R0 => Rotation::R90,
                Rotation::R90 => Rotation::R180,
                Rotation::R180 => Rotation::R270,
                Rotation::R270 => Rotation::R0,
            };
        });
    }

    /// Mirror the selected symbols left to right, in one undo step.
    pub fn mirror_many(&mut self, page: usize, sel: &[Selection]) {
        self.change_symbols(page, sel, |s| s.mirror = !s.mirror);
    }

    fn change_symbols(&mut self, page: usize, sel: &[Selection], f: impl Fn(&mut PlacedSymbol)) {
        let (syms, _) = ids(sel);
        if syms.is_empty() {
            return;
        }
        self.checkpoint();
        if let Some(p) = self.project.pages.get_mut(page) {
            p.symbols
                .iter_mut()
                .filter(|s| syms.contains(s.id.as_str()))
                .for_each(f);
        }
    }

    /// Copy the selection. A selected wire whose end sits on a symbol that is not copied keeps
    /// that end as a free point, so the copy looks the same.
    pub fn copy(&self, page: usize, sel: &[Selection]) -> Clipboard {
        let (syms, wires) = ids(sel);
        let Some(p) = self.project.pages.get(page) else {
            return Clipboard::default();
        };
        let symbols: Vec<PlacedSymbol> = p
            .symbols
            .iter()
            .filter(|s| syms.contains(s.id.as_str()))
            .cloned()
            .collect();
        let mut out_wires = Vec::new();
        for w in carried_wires(p, &syms, &wires) {
            let mut w = w.clone();
            for e in [&mut w.from, &mut w.to] {
                if pin_item(e).is_some_and(|i| !syms.contains(i)) {
                    match self.end_pos(p, e) {
                        Some(at) => *e = Endpoint::Point { at },
                        None => continue,
                    }
                }
            }
            out_wires.push(w);
        }
        let devices = symbols
            .iter()
            .filter_map(|s| s.tag.as_ref())
            .filter_map(|t| self.project.devices.get(t).map(|d| (t.clone(), d.clone())))
            .collect();
        let mut pts: Vec<Point> = symbols
            .iter()
            .filter_map(|s| self.symbol_bounds(p, &s.id).map(|(a, _)| a))
            .collect();
        for w in &out_wires {
            pts.extend(self.wire_points(p, w).unwrap_or_default());
        }
        let anchor = if pts.is_empty() {
            Point::new(0.0, 0.0)
        } else {
            wc_core::geom::bounds_of(&pts).0
        };
        Clipboard {
            symbols,
            wires: out_wires,
            devices,
            anchor,
        }
    }

    /// Paste so the clipboard's anchor lands on `at`. Pasted symbols get fresh ids and the next
    /// free device tags (one new device per copied device). Returns the pasted items.
    pub fn paste(&mut self, page: usize, clip: &Clipboard, at: Point) -> Vec<Selection> {
        if clip.is_empty() || self.project.pages.get(page).is_none() {
            return Vec::new();
        }
        for s in &clip.symbols {
            // Symbols copied from another project are taken from the library when possible.
            if !self.symbols.contains_key(&s.symbol) {
                let _ = self.ensure_symbol(&s.symbol);
            }
        }
        self.checkpoint();
        let d = Point::new(at.x - clip.anchor.x, at.y - clip.anchor.y);
        let mut renamed: BTreeMap<DeviceTag, DeviceTag> = BTreeMap::new();
        let mut ids: BTreeMap<String, String> = BTreeMap::new();
        let mut out = Vec::new();
        for s in &clip.symbols {
            let mut s = s.clone();
            s.tag = s.tag.and_then(|old| {
                if let Some(t) = renamed.get(&old) {
                    return Some(t.clone());
                }
                let t = self.free_tag_like(&old)?;
                let device = self
                    .project
                    .devices
                    .get(&old)
                    .or_else(|| clip.devices.get(&old))
                    .cloned()
                    .unwrap_or_default();
                self.project.devices.insert(t.clone(), device);
                renamed.insert(old, t.clone());
                Some(t)
            });
            let Some(p) = self.project.pages.get_mut(page) else {
                break;
            };
            let id = p.next_id("s");
            ids.insert(s.id.clone(), id.clone());
            s.id = id.clone();
            s.at = s.at + d;
            p.symbols.push(s);
            out.push(Selection::Symbol(id));
        }
        let Some(p) = self.project.pages.get_mut(page) else {
            return out;
        };
        for w in &clip.wires {
            let mut w = w.clone();
            w.id = p.next_id("w");
            w.number = None;
            for v in &mut w.via {
                *v = *v + d;
            }
            for e in [&mut w.from, &mut w.to] {
                match e {
                    Endpoint::Point { at } => *at = *at + d,
                    Endpoint::Pin { item, .. } => {
                        if let Some(n) = ids.get(item.as_str()) {
                            *item = n.clone();
                        }
                    }
                }
            }
            out.push(Selection::Wire(w.id.clone()));
            p.wires.push(w);
        }
        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::state::tests::project;

    fn setup() -> (tempfile::TempDir, Open, usize, String, String) {
        let (tmp, mut o) = project();
        let page = o.project.pages.iter().position(|p| p.id == "p0002").unwrap();
        let a = o.place(page, "wc.coil", Point::new(50.0, 50.0)).unwrap();
        let b = o.place(page, "wc.coil", Point::new(90.0, 50.0)).unwrap();
        o.add_wire(
            page,
            Endpoint::Pin {
                item: a.clone(),
                pin: "A2".into(),
            },
            Endpoint::Pin {
                item: b.clone(),
                pin: "A2".into(),
            },
            vec![Point::new(50.0, 70.0), Point::new(90.0, 70.0)],
        );
        (tmp, o, page, a, b)
    }

    #[test]
    fn box_select_moves_symbols_with_their_wire_bends() {
        let (_tmp, mut o, page, a, b) = setup();
        let all = o.select_in(page, Point::new(0.0, 0.0), Point::new(200.0, 200.0));
        assert_eq!(all.len(), 3);
        let only_a = o.select_in(page, Point::new(30.0, 30.0), Point::new(70.0, 65.0));
        assert_eq!(only_a, vec![Selection::Symbol(a.clone())]);
        let sel = vec![Selection::Symbol(a.clone()), Selection::Symbol(b)];
        o.checkpoint();
        o.move_by(page, &sel, Point::new(10.0, 5.0));
        let p = &o.project.pages[page];
        assert_eq!(p.symbol(&a).unwrap().at, Point::new(60.0, 55.0));
        assert_eq!(p.wires[0].via[0], Point::new(60.0, 75.0));
    }

    #[test]
    fn copy_paste_gets_new_ids_and_tags_and_keeps_the_wire() {
        let (_tmp, mut o, page, a, b) = setup();
        let sel = o.select_all(page);
        let clip = o.copy(page, &sel);
        assert_eq!((clip.symbols.len(), clip.wires.len()), (2, 1));
        let pasted = o.paste(page, &clip, Point::new(150.0, 100.0));
        assert_eq!(pasted.len(), 3);
        let p = &o.project.pages[page];
        assert_eq!(p.symbols.len(), 4);
        let tags: BTreeSet<_> = p.symbols.iter().map(|s| s.tag.clone().unwrap()).collect();
        assert_eq!(tags.len(), 4, "every pasted device gets its own tag");
        let w = p.wires.last().unwrap();
        let ends: Vec<_> = [&w.from, &w.to].iter().filter_map(|e| pin_item(e)).collect();
        assert!(
            ends.iter().all(|i| *i != a && *i != b),
            "pasted wire joins the pasted symbols"
        );
        // Undo removes the whole paste.
        assert!(o.undo());
        assert_eq!(o.project.pages[page].symbols.len(), 2);
    }

    #[test]
    fn copying_a_wire_alone_frees_its_ends_and_delete_many_is_one_step() {
        let (_tmp, mut o, page, a, _b) = setup();
        let w = o.project.pages[page].wires[0].id.clone();
        let clip = o.copy(page, &[Selection::Wire(w)]);
        assert!(clip.symbols.is_empty());
        assert!(matches!(clip.wires[0].from, Endpoint::Point { .. }));
        o.mirror_many(page, &[Selection::Symbol(a.clone())]);
        assert!(o.project.pages[page].symbol(&a).unwrap().mirror);
        let all = o.select_all(page);
        o.delete_many(page, &all);
        assert!(o.project.pages[page].symbols.is_empty() && o.project.pages[page].wires.is_empty());
        assert!(o.undo());
        assert_eq!(o.project.pages[page].symbols.len(), 2);
    }
}

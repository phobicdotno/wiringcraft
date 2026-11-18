//! Editing several items at once: move, delete, rotate, mirror, copy and paste.

use std::collections::{BTreeMap, BTreeSet};

use wc_core::geom::{Align, Point, Rotation, Shape};
use wc_core::{Device, DeviceTag, Endpoint, Page, PlacedSymbol, Wire};

use crate::state::{Open, Selection};

/// Copied items, kept in the app between pages (and projects).
#[derive(Debug, Clone, Default)]
pub struct Clipboard {
    pub symbols: Vec<PlacedSymbol>,
    pub wires: Vec<Wire>,
    /// Devices of the copied symbols, so a paste into another project keeps descriptions and parts.
    pub devices: BTreeMap<DeviceTag, Device>,
    /// Free drawings (lines, rectangles, texts).
    pub graphics: Vec<Shape>,
    /// Top-left corner of what was copied; a paste puts this corner at the pointer.
    pub anchor: Point,
}

impl From<wc_core::macros::Macro> for Clipboard {
    fn from(m: wc_core::macros::Macro) -> Self {
        Self {
            symbols: m.symbols,
            wires: m.wires,
            devices: m.devices,
            graphics: m.graphics,
            anchor: m.anchor,
        }
    }
}

impl Clipboard {
    /// The copied group as a macro to save.
    pub fn to_macro(&self, name: &str, description: &str) -> wc_core::macros::Macro {
        wc_core::macros::Macro {
            name: name.trim().to_owned(),
            description: description.trim().to_owned(),
            symbols: self.symbols.clone(),
            wires: self
                .wires
                .iter()
                .cloned()
                .map(|mut w| {
                    // Numbers belong to a drawing, not to the reusable group.
                    w.number = None;
                    w
                })
                .collect(),
            graphics: self.graphics.clone(),
            devices: self.devices.clone(),
            anchor: self.anchor,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty() && self.wires.is_empty() && self.graphics.is_empty()
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
            Selection::Graphic(_) => false,
        };
    }
    (syms, wires)
}

/// Point the graphics in `sel` at the same items after the page's graphics changed from `old`
/// to `new` (a shared session merged others' changes in, which can move an item to another
/// place in the list). An item is found by its content, repeats by their order (one to one) when
/// the list has as many copies as before; when copies came or went, only a copy still in its own
/// place stays selected. A selected item that is gone, or cannot be told apart from its copies,
/// is no longer selected.
/// Symbols and wires are left as they are.
pub fn remap_graphics(sel: &mut Vec<Selection>, old: &[Shape], new: &[Shape]) {
    if old == new {
        return;
    }
    sel.retain_mut(|s| {
        let Selection::Graphic(i) = s else {
            return true;
        };
        let Some(item) = old.get(*i) else {
            return false;
        };
        let copies = |list: &[Shape]| list.iter().filter(|x| *x == item).count();
        if copies(old) != copies(new) {
            // Copies came or went: which one is the selected one is not known.
            return false;
        }
        // The how-many-th copy of this item it is, found again in the new list.
        let nth = copies(&old[..*i]);
        match new.iter().enumerate().filter(|(_, x)| *x == item).nth(nth) {
            Some((j, _)) => {
                *i = j;
                true
            }
            None => false,
        }
    });
}

fn graphic_ids(sel: &[Selection]) -> BTreeSet<usize> {
    sel.iter()
        .filter_map(|s| match s {
            Selection::Graphic(i) => Some(*i),
            _ => None,
        })
        .collect()
}

/// Rough box of a text: the editor and the PDF use similar sans fonts, about 0.6 em per letter.
pub fn text_box(at: Point, height: f64, align: Align, text: &str) -> (Point, Point) {
    let w = 0.6 * height * text.chars().count().max(1) as f64;
    let x0 = match align {
        Align::Left => at.x,
        Align::Center => at.x - w / 2.0,
        Align::Right => at.x - w,
    };
    (Point::new(x0, at.y - height), Point::new(x0 + w, at.y))
}

/// Bounds of a free drawing, with the real extent of texts.
pub fn graphic_bounds(g: &Shape) -> (Point, Point) {
    match g {
        Shape::Text {
            at,
            text,
            height,
            align,
            angle,
        } => {
            let (a, b) = text_box(*at, *height, *align, text);
            if *angle == 0.0 || !angle.is_finite() {
                return (a, b);
            }
            // Turn the box's corners around the anchor (counter-clockwise on paper, y down).
            let (sin, cos) = angle.to_radians().sin_cos();
            let corners = [a, b, Point::new(a.x, b.y), Point::new(b.x, a.y)].map(|q| {
                let (dx, dy) = (q.x - at.x, q.y - at.y);
                Point::new(at.x + dx * cos + dy * sin, at.y - dx * sin + dy * cos)
            });
            wc_core::geom::bounds_of(&corners)
        }
        // The drawn part of an arc, so a selection box around a quarter arc takes it.
        Shape::Arc {
            center,
            radius,
            start,
            end,
        } => wc_core::geom::bounds_of(&crate::canvas::arc_points(*center, *radius, *start, *end)),
        _ => g.bounds(),
    }
}

fn seg_dist(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    };
    p.distance(Point::new(a.x + t * dx, a.y + t * dy))
}

/// Whether `at` lies inside a closed outline (even-odd rule).
fn inside_outline(at: Point, pts: &[Point]) -> bool {
    let n = pts.len();
    let mut odd = false;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + n - 1) % n]);
        if (a.y > at.y) != (b.y > at.y) && at.x < (b.x - a.x) * (at.y - a.y) / (b.y - a.y) + a.x {
            odd = !odd;
        }
    }
    odd
}

/// Is `at` on the drawing (within `tol` mm of its lines, inside a filled shape or a text)?
pub fn graphic_hit(g: &Shape, at: Point, tol: f64) -> bool {
    let near_path = |pts: &[Point]| pts.windows(2).any(|ab| seg_dist(at, ab[0], ab[1]) <= tol);
    match g {
        Shape::Line { a, b } => seg_dist(at, *a, *b) <= tol,
        Shape::Polyline { points, closed, filled } => {
            let mut pts = points.clone();
            if (*closed || *filled)
                && let Some(f) = points.first()
            {
                pts.push(*f);
            }
            near_path(&pts) || (*filled && inside_outline(at, points))
        }
        Shape::Rect { min, max } => near_path(&[*min, Point::new(max.x, min.y), *max, Point::new(min.x, max.y), *min]),
        Shape::Circle { center, radius, filled } => {
            let d = at.distance(*center);
            if *filled {
                d <= radius + tol
            } else {
                (d - radius).abs() <= tol
            }
        }
        // The same chords the canvas draws, so a click on the visible stroke picks it and the
        // open part of the arc does not.
        Shape::Arc {
            center,
            radius,
            start,
            end,
        } => near_path(&crate::canvas::arc_points(*center, *radius, *start, *end)),
        Shape::Text { .. } => {
            let (a, b) = graphic_bounds(g);
            at.x >= a.x - tol && at.x <= b.x + tol && at.y >= a.y - tol && at.y <= b.y + tol
        }
    }
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
            .chain((0..p.graphics.len()).map(Selection::Graphic))
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
        for (i, g) in p.graphics.iter().enumerate() {
            let (c, d) = graphic_bounds(g);
            if inside(c) && inside(d) {
                out.push(Selection::Graphic(i));
            }
        }
        out
    }

    /// Add a free drawing to a page and return its selection. Refused (None) on a page someone else has open in a shared session.
    pub fn add_graphic(&mut self, page: usize, g: Shape) -> Option<Selection> {
        self.project.pages.get(page)?;
        if self.held_reason(page).is_some() {
            return None;
        }
        self.checkpoint();
        let p = self.project.pages.get_mut(page)?;
        p.graphics.push(g);
        Some(Selection::Graphic(p.graphics.len() - 1))
    }

    /// Change a text's content and height. An empty text is refused.
    pub fn set_text(&mut self, page: usize, index: usize, new_text: &str, new_height: f64) -> bool {
        let Some(Shape::Text { text, height, .. }) = self.project.pages.get(page).and_then(|p| p.graphics.get(index))
        else {
            return false;
        };
        let h = new_height.clamp(0.5, 50.0);
        if new_text.is_empty() || (text == new_text && *height == h) {
            return false;
        }
        self.checkpoint();
        if let Some(Shape::Text { text, height, .. }) =
            self.project.pages.get_mut(page).and_then(|p| p.graphics.get_mut(index))
        {
            *text = new_text.to_owned();
            *height = h;
        }
        true
    }

    /// Replace free drawing `index` on `page` with `new`, a shape of the same kind with other
    /// values (a circle's centre and radius, an arc's radius and angles, a polyline closed or
    /// open), as one undo step: `Ok(true)`. `Ok(false)` when nothing changes. Refused with the
    /// reason for another kind, a value that is not a number, a radius that is not above zero,
    /// an arc with no sweep, or a polyline closed with fewer than three points. An arc angle
    /// that is changed is kept between 0 and 360 degrees (one left as it was is kept as stored);
    /// an opened polyline is no longer filled (only a closed one can be).
    pub fn set_shape(&mut self, page: usize, index: usize, new: Shape) -> Result<bool, &'static str> {
        let Some(old) = self.project.pages.get(page).and_then(|p| p.graphics.get(index)) else {
            return Err("The drawing is no longer there.");
        };
        if std::mem::discriminant(old) != std::mem::discriminant(&new) {
            return Err("The drawing is no longer of this kind.");
        }
        let mut new = new;
        match (&mut new, old) {
            (Shape::Circle { center, radius, .. }, _) => {
                if !(center.x.is_finite() && center.y.is_finite() && radius.is_finite()) {
                    return Err("Enter a number.");
                }
                if *radius <= 0.0 {
                    return Err("The radius must be above zero.");
                }
            }
            (
                Shape::Arc {
                    center,
                    radius,
                    start,
                    end,
                },
                old,
            ) => {
                if ![center.x, center.y, *radius, *start, *end]
                    .iter()
                    .all(|v| v.is_finite())
                {
                    return Err("Enter a number.");
                }
                if *radius <= 0.0 {
                    return Err("The radius must be above zero.");
                }
                let Shape::Arc {
                    start: old_start,
                    end: old_end,
                    ..
                } = *old
                else {
                    return Err("The drawing is no longer of this kind.");
                };
                // Within a full turn; a remainder rounded up to 360 is 0 (as -1e-14 is).
                let turn = |a: f64| {
                    let a = a.rem_euclid(360.0);
                    if a >= 360.0 { 0.0 } else { a }
                };
                let moved = (*start != old_start, *end != old_end);
                if moved.0 {
                    *start = turn(*start);
                }
                if moved.1 {
                    *end = turn(*end);
                }
                // Equal angles are refused, and so is a sweep as the canvas draws it that
                // rounds to nothing (360° to 0°, 370.1° to 10.1°). Angles a whole turn apart
                // are not alike for that: 720° to 0° draws a full turn, as 720° to 100° draws.
                let drawn = crate::canvas::arc_sweep(*start, *end);
                if (moved.0 || moved.1) && (*start == *end || drawn.abs() <= 1e-9) {
                    return Err("The start and end angles must differ.");
                }
            }
            (Shape::Polyline { points, closed, filled }, _) => {
                if *closed && points.len() < 3 {
                    return Err("A polyline needs three points or more to close.");
                }
                if !*closed {
                    *filled = false;
                }
            }
            _ => {}
        }
        if *old == new {
            return Ok(false);
        }
        self.checkpoint();
        if let Some(g) = self.project.pages.get_mut(page).and_then(|p| p.graphics.get_mut(index)) {
            *g = new;
        }
        Ok(true)
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
        for i in graphic_ids(sel) {
            if let Some(g) = p.graphics.get_mut(i) {
                *g = g.translate(d);
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
        let gone = graphic_ids(sel);
        let mut i = 0;
        p.graphics.retain(|_| {
            i += 1;
            !gone.contains(&(i - 1))
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
        let graphics: Vec<Shape> = graphic_ids(sel)
            .into_iter()
            .filter_map(|i| p.graphics.get(i).cloned())
            .collect();
        for g in &graphics {
            let (a, b) = graphic_bounds(g);
            pts.push(a);
            pts.push(b);
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
            graphics,
            anchor,
        }
    }

    /// Paste so the clipboard's anchor lands on `at`. Pasted symbols get fresh ids and the next
    /// free device tags (one new device per copied device). Returns the pasted items, or why
    /// nothing was pasted when a symbol it needs can't be loaded (nothing is added then).
    pub fn paste(&mut self, page: usize, clip: &Clipboard, at: Point) -> Result<Vec<Selection>, String> {
        if clip.is_empty() || self.project.pages.get(page).is_none() || self.held_reason(page).is_some() {
            return Ok(Vec::new());
        }
        // Every symbol it uses, checked against its pin (a loaded copy may be another version);
        // those copied from another project are taken from the library when possible.
        let ids: std::collections::BTreeSet<&str> = clip.symbols.iter().map(|s| s.symbol.as_str()).collect();
        self.ensure_symbols(ids)?;
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
            return Ok(out);
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
        for g in &clip.graphics {
            p.graphics.push(g.translate(d));
            out.push(Selection::Graphic(p.graphics.len() - 1));
        }
        Ok(out)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_graphic_selection_follows_its_item_when_the_list_is_merged() {
        let l = |x: f64| Shape::Line {
            a: Point::new(x, 0.0),
            b: Point::new(x, 5.0),
        };
        let mut sel = vec![
            Selection::Graphic(0),
            Selection::Symbol("s1".into()),
            Selection::Graphic(2),
            Selection::Graphic(3),
        ];
        // Item 0 moved to the end, item 1 dropped, a second copy of item 2 came in: which of
        // the two is the selected one cannot be told, so neither is selected.
        let old = [l(1.0), l(2.0), l(3.0), l(4.0)];
        let new = [l(3.0), l(3.0), l(4.0), l(1.0)];
        remap_graphics(&mut sel, &old, &new);
        assert_eq!(
            sel,
            vec![
                Selection::Graphic(3),
                Selection::Symbol("s1".into()),
                Selection::Graphic(2)
            ]
        );
        // A selected item that is gone is dropped.
        let mut sel = vec![Selection::Graphic(1)];
        remap_graphics(&mut sel, &old, &new);
        assert!(sel.is_empty());
        // Of two copies one changed: which copy is left is not known, so neither selection
        // stays, and the changed one is not handed to the other copy.
        let (a, b, c) = (l(1.0), l(2.0), l(3.0));
        let (old, new) = ([a.clone(), b.clone(), a.clone()], [c.clone(), b.clone(), a.clone()]);
        let mut sel = vec![Selection::Graphic(2)];
        remap_graphics(&mut sel, &old, &new);
        assert!(sel.is_empty());
        let mut sel = vec![Selection::Graphic(0)];
        remap_graphics(&mut sel, &old, &new);
        assert!(sel.is_empty());
        // A copy came back in front of the selected one: an equal copy in its old place is
        // not taken for it.
        let (d, e) = (l(4.0), l(5.0));
        let before = [b.clone(), a.clone(), d.clone()];
        let after = [b.clone(), a.clone(), c.clone(), a.clone(), d.clone(), e.clone()];
        let mut sel = vec![Selection::Graphic(1)];
        remap_graphics(&mut sel, &before, &after);
        assert!(sel.is_empty());
        // Repeats that all moved keep their order.
        let new = [b.clone(), a.clone(), a.clone()];
        let mut sel = vec![Selection::Graphic(0), Selection::Graphic(2)];
        remap_graphics(&mut sel, &old, &new);
        assert_eq!(sel, vec![Selection::Graphic(1), Selection::Graphic(2)]);
        // Both copies selected and moved behind another item: each lands on its own copy.
        let (old, new) = ([a.clone(), a.clone(), b.clone()], [b.clone(), a.clone(), a.clone()]);
        let mut sel = vec![Selection::Graphic(0), Selection::Graphic(1)];
        remap_graphics(&mut sel, &old, &new);
        assert_eq!(sel, vec![Selection::Graphic(1), Selection::Graphic(2)]);
    }
    use crate::state::tests::project;

    #[test]
    fn a_click_inside_a_filled_outline_hits_it() {
        let sq = |filled| Shape::Polyline {
            points: vec![
                Point::new(0.0, 0.0),
                Point::new(20.0, 0.0),
                Point::new(20.0, 20.0),
                Point::new(0.0, 20.0),
            ],
            closed: true,
            filled,
        };
        assert!(graphic_hit(&sq(true), Point::new(10.0, 10.0), 1.0), "inside a fill");
        assert!(
            !graphic_hit(&sq(false), Point::new(10.0, 10.0), 1.0),
            "inside an outline only"
        );
        assert!(!graphic_hit(&sq(true), Point::new(30.0, 10.0), 1.0), "outside");
        assert!(graphic_hit(&sq(true), Point::new(20.5, 10.0), 1.0), "near the edge");
    }

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
        let pasted = o.paste(page, &clip, Point::new(150.0, 100.0)).unwrap();
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
    fn macros_save_list_and_insert_like_a_paste() {
        let (_tmp, mut o, page, _a, _b) = setup();
        o.project.pages[page].wires[0].number = Some("7".into());
        let sel = o.select_all(page);
        let clip = o.copy(page, &sel);
        assert!(o.save_macro(&clip, "  ", "").is_err(), "a macro needs a name");
        let path = o.save_macro(&clip, "Two coils", "Coils sharing A2").unwrap();
        assert!(path.ends_with("macros/two-coils.json"));
        let found = o.macros();
        assert_eq!(found.len(), 1);
        assert_eq!(
            (
                found[0].source.as_str(),
                found[0].name.as_str(),
                found[0].description.as_str()
            ),
            ("project", "Two coils", "Coils sharing A2")
        );
        // Inserting lands where the group was drawn, with new tags, and the wire loses its number.
        let inserted = o.insert_macro(page, &path).unwrap();
        assert_eq!(inserted.len(), 3);
        let p = &o.project.pages[page];
        let tags: BTreeSet<_> = p.symbols.iter().map(|s| s.tag.clone().unwrap()).collect();
        assert_eq!(tags.len(), 4);
        assert_eq!(p.symbols[2].at, p.symbols[0].at);
        assert_eq!(p.wires.last().unwrap().number, None);
        // A broken file is listed with its error instead of hiding the others.
        std::fs::write(path.with_file_name("broken.json"), "{").unwrap();
        let found = o.macros();
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|m| m.error.is_some()));
    }

    #[test]
    fn a_circle_an_arc_and_a_polyline_take_new_values_as_one_undo_step_each() {
        let (_tmp, mut o, page, _a, _b) = setup();
        let n = o.project.pages[page].graphics.len();
        let circle = Shape::Circle {
            center: Point::new(100.0, 100.0),
            radius: 5.0,
            filled: false,
        };
        let arc = Shape::Arc {
            center: Point::new(150.0, 100.0),
            radius: 8.0,
            start: 0.0,
            end: 90.0,
        };
        let line = Shape::Polyline {
            points: vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0)],
            closed: true,
            filled: true,
        };
        for g in [&circle, &arc, &line] {
            o.add_graphic(page, g.clone()).unwrap();
        }
        let at = |o: &Open, i: usize| o.project.pages[page].graphics[n + i].clone();

        // A circle: a new centre and radius.
        let bigger = Shape::Circle {
            center: Point::new(110.0, 105.0),
            radius: 7.5,
            filled: false,
        };
        assert_eq!(o.set_shape(page, n, bigger.clone()), Ok(true));
        assert_eq!(at(&o, 0), bigger);
        // No radius at all or another kind: refused with a reason; the same values: no change.
        assert_eq!(o.set_shape(page, n, bigger.clone()), Ok(false));
        for bad in [
            Shape::Circle {
                center: Point::new(110.0, 105.0),
                radius: 0.0,
                filled: false,
            },
            Shape::Circle {
                center: Point::new(f64::NAN, 105.0),
                radius: 3.0,
                filled: false,
            },
            arc.clone(),
        ] {
            assert!(o.set_shape(page, n, bad).is_err());
        }
        assert_eq!(at(&o, 0), bigger);

        // An arc: angles past a full turn are kept between 0 and 360.
        assert_eq!(
            o.set_shape(
                page,
                n + 1,
                Shape::Arc {
                    center: Point::new(150.0, 100.0),
                    radius: 12.0,
                    start: -90.0,
                    end: 450.0,
                }
            ),
            Ok(true)
        );
        assert_eq!(
            at(&o, 1),
            Shape::Arc {
                center: Point::new(150.0, 100.0),
                radius: 12.0,
                start: 270.0,
                end: 90.0,
            }
        );

        // A polyline opened is no longer filled; two points cannot be closed.
        let Shape::Polyline { points, .. } = line.clone() else {
            unreachable!()
        };
        assert_eq!(
            o.set_shape(
                page,
                n + 2,
                Shape::Polyline {
                    points: points.clone(),
                    closed: false,
                    filled: true,
                }
            ),
            Ok(true)
        );
        assert_eq!(
            at(&o, 2),
            Shape::Polyline {
                points: points.clone(),
                closed: false,
                filled: false,
            }
        );
        assert!(
            o.set_shape(
                page,
                n + 2,
                Shape::Polyline {
                    points: points[..2].to_vec(),
                    closed: true,
                    filled: false,
                }
            )
            .is_err()
        );

        // Each change is one undo step, newest first.
        assert!(o.undo());
        assert_eq!(at(&o, 2), line);
        assert!(o.undo());
        assert_eq!(at(&o, 1), arc);
        assert!(o.undo());
        assert_eq!(at(&o, 0), circle);
    }

    #[test]
    fn an_arc_keeps_the_angles_left_alone_and_refuses_a_sweep_rounded_to_nothing() {
        let (_tmp, mut o, page, _a, _b) = setup();
        let n = o.project.pages[page].graphics.len();
        // Stored past a full turn (read from a file): drawn sweeping more than 360 degrees.
        let wide = Shape::Arc {
            center: Point::new(150.0, 100.0),
            radius: 8.0,
            start: 0.0,
            end: 450.0,
        };
        o.add_graphic(page, wide).unwrap();
        // Only the radius changes: the angles stay as stored.
        assert_eq!(
            o.set_shape(
                page,
                n,
                Shape::Arc {
                    center: Point::new(150.0, 100.0),
                    radius: 9.0,
                    start: 0.0,
                    end: 450.0,
                }
            ),
            Ok(true)
        );
        assert_eq!(
            o.project.pages[page].graphics[n],
            Shape::Arc {
                center: Point::new(150.0, 100.0),
                radius: 9.0,
                start: 0.0,
                end: 450.0,
            }
        );
        // A start a hair below 0 rounds to a full turn: the same as an end of 0, no sweep.
        let before = o.project.clone();
        assert!(
            o.set_shape(
                page,
                n,
                Shape::Arc {
                    center: Point::new(150.0, 100.0),
                    radius: 9.0,
                    start: -1e-14,
                    end: 0.0,
                }
            )
            .is_err()
        );
        assert_eq!(o.project, before);
        // Rounded to a full turn with a sweep left: kept as 0, not 360.
        assert_eq!(
            o.set_shape(
                page,
                n,
                Shape::Arc {
                    center: Point::new(150.0, 100.0),
                    radius: 9.0,
                    start: -1e-14,
                    end: 90.0,
                }
            ),
            Ok(true)
        );
        assert_eq!(
            o.project.pages[page].graphics[n],
            Shape::Arc {
                center: Point::new(150.0, 100.0),
                radius: 9.0,
                start: 0.0,
                end: 90.0,
            }
        );
        // Only the end changed, but the sweep drawn from the stored start rounds to nothing.
        o.project.pages[page].graphics[n] = Shape::Arc {
            center: Point::new(150.0, 100.0),
            radius: 9.0,
            start: 370.1,
            end: 90.0,
        };
        assert!(
            o.set_shape(
                page,
                n,
                Shape::Arc {
                    center: Point::new(150.0, 100.0),
                    radius: 9.0,
                    start: 370.1,
                    end: 10.1,
                }
            )
            .is_err()
        );
        // Stored from a file past a full turn: the canvas draws 720° to 100° as -260°.
        o.project.pages[page].graphics[n] = Shape::Arc {
            center: Point::new(150.0, 100.0),
            radius: 9.0,
            start: 720.0,
            end: 90.0,
        };
        let wide = Shape::Arc {
            center: Point::new(150.0, 100.0),
            radius: 9.0,
            start: 720.0,
            end: 100.0,
        };
        assert_eq!(o.set_shape(page, n, wide.clone()), Ok(true));
        assert_eq!(o.project.pages[page].graphics[n], wide);
        // 720° to 0° is a full turn as drawn, though the angles match within one turn.
        o.project.pages[page].graphics[n] = Shape::Arc {
            center: Point::new(150.0, 100.0),
            radius: 9.0,
            start: 720.0,
            end: 90.0,
        };
        let full = Shape::Arc {
            center: Point::new(150.0, 100.0),
            radius: 9.0,
            start: 720.0,
            end: 0.0,
        };
        assert_eq!(o.set_shape(page, n, full.clone()), Ok(true));
        assert_eq!(o.project.pages[page].graphics[n], full);
        // 360° to 0° draws nothing.
        let before = Shape::Arc {
            center: Point::new(150.0, 100.0),
            radius: 9.0,
            start: 360.0,
            end: 90.0,
        };
        o.project.pages[page].graphics[n] = before.clone();
        let none = Shape::Arc {
            center: Point::new(150.0, 100.0),
            radius: 9.0,
            start: 360.0,
            end: 0.0,
        };
        assert!(o.set_shape(page, n, none).is_err());
        assert_eq!(o.project.pages[page].graphics[n], before);
    }

    #[test]
    fn free_drawings_are_hit_selected_moved_copied_and_deleted() {
        let (_tmp, mut o, page, _a, _b) = setup();
        let line = o
            .add_graphic(
                page,
                Shape::Line {
                    a: Point::new(200.0, 20.0),
                    b: Point::new(260.0, 20.0),
                },
            )
            .unwrap();
        let text = o
            .add_graphic(
                page,
                Shape::Text {
                    at: Point::new(200.0, 40.0),
                    text: "Pump 1".into(),
                    height: 3.0,
                    align: Align::Left,
                    angle: 0.0,
                },
            )
            .unwrap();
        let p = &o.project.pages[page];
        assert!(graphic_hit(&p.graphics[0], Point::new(230.0, 20.5), 1.0));
        assert!(!graphic_hit(&p.graphics[0], Point::new(230.0, 25.0), 1.0));
        assert!(graphic_hit(&p.graphics[1], Point::new(203.0, 39.0), 0.5));
        let boxed = o.select_in(page, Point::new(190.0, 10.0), Point::new(270.0, 45.0));
        assert_eq!(boxed, vec![line.clone(), text.clone()]);
        assert!(o.set_text(page, 1, "Pump 2", 5.0));
        assert!(!o.set_text(page, 1, "", 5.0));
        o.move_by(page, std::slice::from_ref(&line), Point::new(0.0, 10.0));
        assert_eq!(
            o.project.pages[page].graphics[0],
            Shape::Line {
                a: Point::new(200.0, 30.0),
                b: Point::new(260.0, 30.0),
            }
        );
        let clip = o.copy(page, &boxed);
        assert_eq!(clip.graphics.len(), 2);
        let pasted = o.paste(page, &clip, Point::new(10.0, 10.0)).unwrap();
        assert_eq!(pasted, vec![Selection::Graphic(2), Selection::Graphic(3)]);
        o.delete_many(page, &[line]);
        let left = &o.project.pages[page].graphics;
        assert_eq!(left.len(), 3);
        assert!(matches!(&left[0], Shape::Text { text, .. } if text == "Pump 2"));
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

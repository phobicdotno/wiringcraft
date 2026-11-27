//! The black box tool: drag a rectangle on the page, click its edges to add connection points
//! (a name is asked for each), then click inside it or press Enter to tag it and put it on the
//! page ([`crate::state::Open::add_black_box`]). A placed box is changed afterwards in the
//! properties panel with a [`BoxEdit`] ([`crate::state::Open::edit_black_box`]).

use std::collections::BTreeMap;

use wc_core::geom::{Point, Rotation};
use wc_core::{BoxPin, BoxSide, Symbol};

/// A black box being drawn on one page.
#[derive(Debug, Clone, PartialEq)]
pub struct BoxDraft {
    /// The page it is drawn on, by id: on another page it is dropped.
    pub page_id: String,
    /// The corner the drag (or the first click) started at.
    pub from: Point,
    /// The opposite corner, once the rectangle is drawn.
    pub to: Option<Point>,
    /// The connection points added so far.
    pub pins: Vec<BoxPin>,
}

impl BoxDraft {
    pub fn new(page_id: &str, from: Point) -> Self {
        Self {
            page_id: page_id.to_owned(),
            from,
            to: None,
            pins: Vec::new(),
        }
    }

    /// The rectangle as (top-left, bottom-right), once it is drawn.
    pub fn rect(&self) -> Option<(Point, Point)> {
        let to = self.to?;
        Some(corners(self.from, to))
    }

    /// Where a connection point sits on the page.
    pub fn pin_at(&self, pin: &BoxPin) -> Option<Point> {
        let (min, max) = self.rect()?;
        Some(match pin.side {
            BoxSide::Top => Point::new(min.x + pin.offset, min.y),
            BoxSide::Bottom => Point::new(min.x + pin.offset, max.y),
            BoxSide::Left => Point::new(min.x, min.y + pin.offset),
            BoxSide::Right => Point::new(max.x, min.y + pin.offset),
        })
    }

    /// Add a connection point named `name` at `side`/`offset`, when the box takes it (a name,
    /// on its edge, a name and place no other point has). The draft stays as it was otherwise.
    pub fn add_pin(&mut self, side: BoxSide, offset: f64, name: &str) -> Result<(), String> {
        let (min, max) = self.rect().ok_or("draw the box's rectangle first")?;
        let mut pins = self.pins.clone();
        pins.push(BoxPin {
            name: name.trim().to_owned(),
            side,
            offset,
        });
        wc_core::black_box("box.draft", "", max.x - min.x, max.y - min.y, &pins).map_err(|e| e.to_string())?;
        self.pins = pins;
        Ok(())
    }

    /// The name offered for the next connection point: the lowest number no point has.
    pub fn next_pin_name(&self) -> String {
        (1..)
            .map(|n: usize| n.to_string())
            .find(|n| !self.pins.iter().any(|p| p.name == *n))
            .unwrap_or_default()
    }
}

/// One connection point in the black box form.
#[derive(Debug, Clone, PartialEq)]
pub struct PinRow {
    /// Its name in the box's symbol, for a point the box had: renaming it moves its wires along.
    pub was: Option<String>,
    pub name: String,
    pub side: BoxSide,
    pub offset: f64,
}

/// The properties panel's form for a placed black box: its size and connection points as typed,
/// applied as one change.
#[derive(Debug, Clone, PartialEq)]
pub struct BoxEdit {
    pub width: f64,
    pub height: f64,
    pub rows: Vec<PinRow>,
    /// Why the last Apply was refused.
    pub error: Option<String>,
    base: (f64, f64, Vec<BoxPin>),
}

impl BoxEdit {
    /// The form for black box symbol `sym`; `None` when it is not one.
    pub fn load(sym: &Symbol) -> Option<Self> {
        let (width, height, pins) = wc_core::black_box_layout(sym)?;
        Some(Self {
            width,
            height,
            rows: pins
                .iter()
                .map(|p| PinRow {
                    was: Some(p.name.clone()),
                    name: p.name.clone(),
                    side: p.side,
                    offset: p.offset,
                })
                .collect(),
            error: None,
            base: (width, height, pins),
        })
    }

    /// The connection points as typed.
    pub fn pins(&self) -> Vec<BoxPin> {
        self.rows
            .iter()
            .map(|r| BoxPin {
                name: r.name.trim().to_owned(),
                side: r.side,
                offset: r.offset,
            })
            .collect()
    }

    /// The points renamed in the form: old name to new.
    /// The points the box had when the form was opened.
    pub fn old_pins(&self) -> &[BoxPin] {
        &self.base.2
    }

    pub fn renamed(&self) -> BTreeMap<String, String> {
        self.rows
            .iter()
            .filter_map(|r| {
                let was = r.was.as_ref()?;
                let now = r.name.trim();
                (was != now).then(|| (was.clone(), now.to_owned()))
            })
            .collect()
    }

    /// The points the box had that the form no longer has (by identity, not by name: a new
    /// point may take a removed one's name).
    pub fn removed(&self) -> std::collections::BTreeSet<String> {
        self.base
            .2
            .iter()
            .filter(|p| !self.rows.iter().any(|r| r.was.as_deref() == Some(p.name.as_str())))
            .map(|p| p.name.clone())
            .collect()
    }

    /// Whether anything was typed that the box does not have yet: a rename counts even when
    /// the points end up as they were, since the wires on the renamed point follow it.
    pub fn changed(&self) -> bool {
        (self.width, self.height, self.pins()) != self.base || !self.renamed().is_empty() || !self.removed().is_empty()
    }

    /// Add a point: the lowest number no point has, on the left edge at the first free 5 mm
    /// step (the middle of the edge when every step is taken).
    pub fn add_row(&mut self) {
        let name = (1..)
            .map(|n: usize| n.to_string())
            .find(|n| !self.rows.iter().any(|r| r.name.trim() == n))
            .unwrap_or_default();
        let taken = |o: f64| {
            self.rows
                .iter()
                .any(|r| r.side == BoxSide::Left && (r.offset - o).abs() < 1e-6)
        };
        let offset = (1..)
            .map(|n| f64::from(n) * 5.0)
            .take_while(|o| *o < self.height)
            .find(|o| !taken(*o))
            .unwrap_or(self.height / 2.0);
        self.rows.push(PinRow {
            was: None,
            name,
            side: BoxSide::Left,
            offset,
        });
    }
}

/// The name of a box edge in the form.
pub fn side_label(side: BoxSide) -> &'static str {
    match side {
        BoxSide::Top => "Top",
        BoxSide::Bottom => "Bottom",
        BoxSide::Left => "Left",
        BoxSide::Right => "Right",
    }
}

/// `a` and `b` as (top-left, bottom-right).
pub fn corners(a: Point, b: Point) -> (Point, Point) {
    (
        Point::new(a.x.min(b.x), a.y.min(b.y)),
        Point::new(a.x.max(b.x), a.y.max(b.y)),
    )
}

/// The edge of the box `min`..`max` within `tol` mm of `at`, and how far along it (mm from its
/// left or top end, kept on the edge). The nearest edge wins; at a corner, top and bottom come
/// before left and right.
pub fn edge_at(min: Point, max: Point, at: Point, tol: f64) -> Option<(BoxSide, f64)> {
    let along_x = (min.x - tol..=max.x + tol).contains(&at.x);
    let along_y = (min.y - tol..=max.y + tol).contains(&at.y);
    let x_off = (at.x - min.x).clamp(0.0, max.x - min.x);
    let y_off = (at.y - min.y).clamp(0.0, max.y - min.y);
    [
        (BoxSide::Top, along_x, (at.y - min.y).abs(), x_off),
        (BoxSide::Bottom, along_x, (at.y - max.y).abs(), x_off),
        (BoxSide::Left, along_y, (at.x - min.x).abs(), y_off),
        (BoxSide::Right, along_y, (at.x - max.x).abs(), y_off),
    ]
    .into_iter()
    .filter(|(_, along, d, _)| *along && *d <= tol)
    .min_by(|a, b| a.2.total_cmp(&b.2))
    .map(|(side, _, _, off)| (side, off))
}

/// Whether `at` is inside the box, away from its edges by more than `tol`.
pub fn inside(min: Point, max: Point, at: Point, tol: f64) -> bool {
    at.x > min.x + tol && at.x < max.x - tol && at.y > min.y + tol && at.y < max.y - tol
}

/// What the status bar says at each step.
pub fn hint(draft: Option<&BoxDraft>) -> &'static str {
    match draft {
        None => "Drag the black box's rectangle, or click one corner.",
        Some(d) if d.to.is_none() => "Click the opposite corner (Esc cancels).",
        Some(d) if d.pins.is_empty() => {
            "Click an edge to add a connection point; click inside the box or press Enter to tag it (Esc cancels)."
        }
        Some(_) => {
            "Click an edge for another connection point; click inside the box or press Enter to tag it and put it on the page (Esc cancels)."
        }
    }
}

/// The connection points of a box resized to a new size, each kept on its edge (#524).
pub use wc_core::box_edit::resized_pins;

/// Whether a black box edit changed the connection point `pin`: renamed, taken off, moved, or
/// its name taken by another point (also a new point just like one taken off). A wire being
/// drawn from such a point would be saved on a stale point, so the Properties form and the
/// assistant both drop it.
pub(crate) fn point_changed(
    pin: &str,
    old: &[BoxPin],
    new: &[BoxPin],
    renamed: &BTreeMap<String, String>,
    removed: &std::collections::BTreeSet<String>,
) -> bool {
    renamed.contains_key(pin)
        || removed.contains(pin)
        || renamed.values().any(|to| to == pin)
        || old.iter().find(|p| p.name == pin) != new.iter().find(|p| p.name == pin)
}

/// The size of a box `start` (width, height) after its sizing corner was dragged by `d` (page
/// mm) on a box placed turned by `rotation` and maybe mirrored: in whole steps of `step`, and
/// at least one step (and 1 mm) each way.
#[cfg(test)]
pub fn dragged_size(start: (f64, f64), d: Point, rotation: Rotation, mirror: bool, step: f64) -> (f64, f64) {
    dragged_box(start, FAR, d, rotation, mirror, step).0
}

/// A corner of a black box in its own frame: whether it is at the box's far end along x (where
/// its width ends) and along y (where its height ends). The box's origin is `(false, false)`.
pub type Corner = (bool, bool);

/// The box's far corner, where its width and height end: sizing from it keeps the placement.
pub const FAR: Corner = (true, true);

/// The four corners of a box, far corner first.
pub const CORNERS: [Corner; 4] = [FAR, (false, true), (true, false), (false, false)];

/// Where `corner` of a `w` by `h` box is in the box's own frame.
pub fn corner_at(corner: Corner, w: f64, h: f64) -> Point {
    Point::new(if corner.0 { w } else { 0.0 }, if corner.1 { h } else { 0.0 })
}

/// A box `start` (width, height) after its `corner` was dragged by `d` (page mm), on a box
/// placed turned by `rotation` and maybe mirrored: its new size, in whole steps of `step` and
/// at least one step (and 1 mm) each way, and how far its origin moves in its own frame so the
/// opposite corner stays put. Only a near corner moves the origin.
pub fn dragged_box(
    start: (f64, f64),
    corner: Corner,
    d: Point,
    rotation: Rotation,
    mirror: bool,
    step: f64,
) -> ((f64, f64), Point) {
    let local = untransform(d, rotation, mirror);
    let step = if step.is_finite() && step > 0.0 { step } else { 1.0 };
    // At least 1 mm, rounded up to whole steps, so the smallest box is on the grid too.
    let least = (1.0 / step).ceil().max(1.0) * step;
    let whole = |v: f64| ((v / step).round() * step).max(least);
    let w = whole(if corner.0 { start.0 + local.x } else { start.0 - local.x });
    let h = whole(if corner.1 { start.1 + local.y } else { start.1 - local.y });
    let shift = Point::new(
        if corner.0 { 0.0 } else { start.0 - w },
        if corner.1 { 0.0 } else { start.1 - h },
    );
    ((w, h), shift)
}

/// `pins` of a box whose origin moved by `shift` in its own frame to a `width` by `height`
/// box: each point keeps its place along its edge where it can (its offset less the shift
/// along that edge), and is kept on the edge, at its end, where it cannot.
pub fn shifted_pins(pins: &[BoxPin], shift: Point, width: f64, height: f64) -> Vec<BoxPin> {
    let moved: Vec<BoxPin> = pins
        .iter()
        .map(|p| BoxPin {
            name: p.name.clone(),
            side: p.side,
            offset: match p.side {
                BoxSide::Top | BoxSide::Bottom => p.offset - shift.x,
                BoxSide::Left | BoxSide::Right => p.offset - shift.y,
            },
        })
        .collect();
    resized_pins(&moved, width, height)
}

/// `p` taken back from the page into a placed symbol's own frame: the inverse of
/// [`Point::transform`].
pub fn untransform(p: Point, rotation: Rotation, mirror: bool) -> Point {
    let back = match rotation {
        Rotation::R0 => Rotation::R0,
        Rotation::R90 => Rotation::R270,
        Rotation::R180 => Rotation::R180,
        Rotation::R270 => Rotation::R90,
    };
    let p = p.transform(back, false);
    if mirror { Point::new(-p.x, p.y) } else { p }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn a_dragged_corner_sizes_the_box_in_its_own_frame_and_keeps_points_on_their_edges() {
        // Every placement: taking a point back undoes the transform.
        for rot in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
            for mirror in [false, true] {
                let q = p(3.0, -7.0);
                assert_eq!(
                    untransform(q.transform(rot, mirror), rot, mirror),
                    q,
                    "{rot:?} {mirror}"
                );
            }
        }
        // Unturned: the drag adds to the size, in whole grid steps.
        assert_eq!(
            dragged_size((40.0, 20.0), p(9.0, -4.0), Rotation::R0, false, 2.5),
            (50.0, 15.0)
        );
        // Turned a quarter clockwise: the box's width runs down the page.
        assert_eq!(
            dragged_size((40.0, 20.0), p(0.0, 10.0), Rotation::R90, false, 2.5),
            (50.0, 20.0)
        );
        // Mirrored: the box's width runs to the left.
        assert_eq!(
            dragged_size((40.0, 20.0), p(-10.0, 0.0), Rotation::R0, true, 2.5),
            (50.0, 20.0)
        );
        // Never smaller than a step, nor than 1 mm.
        assert_eq!(
            dragged_size((40.0, 20.0), p(-100.0, -100.0), Rotation::R0, false, 2.5),
            (2.5, 2.5)
        );
        assert_eq!(
            dragged_size((40.0, 20.0), p(-100.0, -100.0), Rotation::R0, false, 0.5),
            (1.0, 1.0)
        );
        // A step that does not divide 1 mm: the least size is the first step past it.
        assert_eq!(
            dragged_size((40.0, 20.0), p(-100.0, -100.0), Rotation::R0, false, 0.75),
            (1.5, 1.5)
        );

        let pin = |name: &str, side, offset| BoxPin {
            name: name.into(),
            side,
            offset,
        };
        let pins = [
            pin("1", BoxSide::Top, 35.0),
            pin("2", BoxSide::Left, 5.0),
            pin("3", BoxSide::Right, 15.0),
            pin("4", BoxSide::Bottom, 10.0),
        ];
        assert_eq!(
            resized_pins(&pins, 20.0, 10.0),
            vec![
                pin("1", BoxSide::Top, 20.0),
                pin("2", BoxSide::Left, 5.0),
                pin("3", BoxSide::Right, 10.0),
                pin("4", BoxSide::Bottom, 10.0),
            ]
        );
    }

    #[test]
    fn a_click_near_an_edge_names_the_edge_and_the_offset_along_it() {
        let (min, max) = (p(10.0, 20.0), p(40.0, 40.0));
        assert_eq!(edge_at(min, max, p(15.0, 20.5), 1.0), Some((BoxSide::Top, 5.0)));
        assert_eq!(edge_at(min, max, p(25.0, 39.4), 1.0), Some((BoxSide::Bottom, 15.0)));
        assert_eq!(edge_at(min, max, p(9.5, 30.0), 1.0), Some((BoxSide::Left, 10.0)));
        assert_eq!(edge_at(min, max, p(40.0, 25.0), 1.0), Some((BoxSide::Right, 5.0)));
        // Inside, away from every edge, or beyond an edge's ends: no edge.
        assert_eq!(edge_at(min, max, p(25.0, 30.0), 1.0), None);
        assert_eq!(edge_at(min, max, p(45.0, 20.0), 1.0), None);
        // Just past a corner: kept on the edge, top before left.
        assert_eq!(edge_at(min, max, p(9.5, 19.5), 1.0), Some((BoxSide::Top, 0.0)));
        assert!(inside(min, max, p(25.0, 30.0), 1.0));
        assert!(!inside(min, max, p(10.5, 30.0), 1.0));
    }

    #[test]
    fn the_box_form_reads_a_box_and_tells_renames_and_changes() {
        let pins = [
            BoxPin {
                name: "1".into(),
                side: BoxSide::Left,
                offset: 5.0,
            },
            BoxPin {
                name: "2".into(),
                side: BoxSide::Right,
                offset: 5.0,
            },
        ];
        let sym = wc_core::black_box("box.p1.s1", "Black box", 30.0, 20.0, &pins).unwrap();
        let mut f = BoxEdit::load(&sym).unwrap();
        assert_eq!((f.width, f.height, f.pins()), (30.0, 20.0, pins.to_vec()));
        assert!(!f.changed());
        f.rows[1].name = " OUT ".into();
        assert!(f.changed());
        assert_eq!(f.renamed(), [("2".to_owned(), "OUT".to_owned())].into_iter().collect());
        f.add_row();
        assert_eq!(
            (f.rows[2].name.as_str(), f.rows[2].side, f.rows[2].offset),
            ("2", BoxSide::Left, 10.0)
        );
        assert_eq!(f.rows[2].was, None);
        assert_eq!(f.renamed().len(), 1, "a new point is no rename");
        let mut other = sym.clone();
        other.category = "Relays".into();
        assert!(BoxEdit::load(&other).is_none());

        // Point 1 removed, 2 renamed to 1 and moved where 1 was, a new 2 where 2 was: the
        // points are as before, but the wires on the old 2 follow it, so it is a change.
        let mut f = BoxEdit::load(&sym).unwrap();
        f.rows.remove(0);
        f.rows[0].name = "1".into();
        f.rows[0].side = BoxSide::Left;
        f.add_row();
        f.rows[1].side = BoxSide::Right;
        f.rows[1].offset = 5.0;
        assert_eq!(f.pins(), pins.to_vec());
        assert!(f.changed());
        assert_eq!(f.removed(), ["1".to_owned()].into_iter().collect());
    }

    #[test]
    fn a_draft_takes_named_points_on_its_edges_and_refuses_clashes() {
        let mut d = BoxDraft::new("p1", p(40.0, 40.0));
        assert!(d.add_pin(BoxSide::Top, 0.0, "1").is_err(), "no rectangle yet");
        d.to = Some(p(10.0, 20.0));
        assert_eq!(d.rect(), Some((p(10.0, 20.0), p(40.0, 40.0))));
        assert_eq!(d.next_pin_name(), "1");
        d.add_pin(BoxSide::Left, 10.0, " 1 ").unwrap();
        assert_eq!(d.pins[0].name, "1", "names are trimmed");
        assert_eq!(d.pin_at(&d.pins[0]), Some(p(10.0, 30.0)));
        assert_eq!(d.next_pin_name(), "2");
        let err = d.add_pin(BoxSide::Right, 5.0, "1").unwrap_err();
        assert!(err.contains("two connection points are named 1"), "{err}");
        assert!(d.add_pin(BoxSide::Left, 10.0, "2").is_err(), "same place");
        assert!(d.add_pin(BoxSide::Right, 5.0, "  ").is_err(), "no name");
        assert_eq!(d.pins.len(), 1, "a refused point leaves the draft as it was");
        d.add_pin(BoxSide::Right, 5.0, "2").unwrap();
        assert_eq!(d.pin_at(&d.pins[1]), Some(p(40.0, 25.0)));
    }

    #[test]
    fn a_dragged_near_corner_moves_the_origin_so_the_opposite_corner_stays() {
        assert_eq!(corner_at(FAR, 20.0, 10.0), p(20.0, 10.0));
        assert_eq!(corner_at((false, true), 20.0, 10.0), p(0.0, 10.0));
        // The far corner: the origin stays.
        assert_eq!(
            dragged_box((20.0, 10.0), FAR, p(5.0, 5.0), Rotation::R0, false, 1.0),
            ((25.0, 15.0), p(0.0, 0.0))
        );
        // The origin corner dragged up and left: bigger, and the origin goes with it.
        assert_eq!(
            dragged_box((20.0, 10.0), (false, false), p(-5.0, -4.0), Rotation::R0, false, 1.0),
            ((25.0, 14.0), p(-5.0, -4.0))
        );
        // Dragged past the opposite corner: the least size, the far corner still in place.
        assert_eq!(
            dragged_box((20.0, 10.0), (false, false), p(50.0, 50.0), Rotation::R0, false, 2.5),
            ((2.5, 2.5), p(17.5, 7.5))
        );
        // Bottom-left corner of a box turned a quarter: the box's x runs down the page, so a
        // drag up the page grows its width from the origin side.
        assert_eq!(
            dragged_box((20.0, 10.0), (false, true), p(0.0, -5.0), Rotation::R90, false, 1.0),
            ((25.0, 10.0), p(-5.0, 0.0))
        );
        // Every placement: the opposite corner is where it was on the page.
        for rot in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
            for mirror in [false, true] {
                for corner in CORNERS {
                    let ((w, h), shift) = dragged_box((20.0, 10.0), corner, p(3.0, -2.0), rot, mirror, 1.0);
                    let opposite = (!corner.0, !corner.1);
                    let was = corner_at(opposite, 20.0, 10.0).transform(rot, mirror);
                    let now = (corner_at(opposite, w, h) + shift).transform(rot, mirror);
                    assert!(was.distance(now) < 1e-9, "{rot:?} {mirror} {corner:?}");
                }
            }
        }

        let pin = |name: &str, side, offset| BoxPin {
            name: name.into(),
            side,
            offset,
        };
        let pins = [
            pin("1", BoxSide::Top, 15.0),
            pin("2", BoxSide::Left, 5.0),
            pin("3", BoxSide::Right, 2.0),
        ];
        // The origin moved 5 left and 4 up: the points keep their place, their offsets grow.
        assert_eq!(
            shifted_pins(&pins, p(-5.0, -4.0), 25.0, 14.0),
            vec![
                pin("1", BoxSide::Top, 20.0),
                pin("2", BoxSide::Left, 9.0),
                pin("3", BoxSide::Right, 6.0)
            ]
        );
        // The origin moved in past a point: it is kept at the start of its edge.
        assert_eq!(
            shifted_pins(&pins, p(10.0, 3.0), 10.0, 7.0),
            vec![
                pin("1", BoxSide::Top, 5.0),
                pin("2", BoxSide::Left, 2.0),
                pin("3", BoxSide::Right, 0.0)
            ]
        );
    }
}

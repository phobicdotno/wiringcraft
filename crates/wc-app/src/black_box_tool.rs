//! The black box tool: drag a rectangle on the page, click its edges to add connection points
//! (a name is asked for each), then click inside it or press Enter to tag it and put it on the
//! page ([`crate::state::Open::add_black_box`]). A placed box is changed afterwards in the
//! properties panel with a [`BoxEdit`] ([`crate::state::Open::edit_black_box`]).

use std::collections::BTreeMap;

use wc_core::geom::Point;
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

    /// Whether anything was typed that the box does not have yet.
    pub fn changed(&self) -> bool {
        (self.width, self.height, self.pins()) != self.base
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
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
}

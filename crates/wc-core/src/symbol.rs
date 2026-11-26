//! Symbols: the reusable drawing of a device function with named connection points (pins).

use serde::{Deserialize, Serialize};

use crate::geom::{Point, Rotation, Shape, bounds_of};
use crate::hash::content_hash;

/// Which way a wire leaves a pin; used by routing and by wire-end snapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PinDir {
    Up,
    #[default]
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    /// Connection designation printed next to the pin, e.g. `13`, `A1`, `L1`.
    pub name: String,
    pub at: Point,
    #[serde(default)]
    pub dir: PinDir,
}

/// What a symbol stands for, used for cross-references between coil and contacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolRole {
    /// Drives other symbols with the same tag (relay coil, contactor coil).
    Main,
    /// Driven by a main symbol with the same tag (auxiliary contact).
    Auxiliary,
    #[default]
    Standalone,
    /// A terminal of a terminal strip.
    Terminal,
    /// An interruption point: the wire continues at every other interruption point with the
    /// same tag, usually on another page.
    Interruption,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    /// Stable library id, e.g. `wc.contact.no`. Never reused for a different meaning.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub category: String,
    /// Letter code suggested for new devices (e.g. `K` for relays).
    #[serde(default)]
    pub letter_code: String,
    #[serde(default)]
    pub role: SymbolRole,
    pub graphics: Vec<Shape>,
    pub pins: Vec<Pin>,
    /// Where the device tag is printed, relative to the symbol origin.
    #[serde(default)]
    pub tag_at: Point,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// What the symbol is in the library it was imported from, such as a SolidWorks
    /// Electrical block's GUID, so a later import replaces a copy only with the same item
    /// (#301). Empty when it was not imported or its source has no such identity.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

/// Starts [`Symbol::source`] when the import found more than one item with that identity in one
/// library: which of them a later import's item is cannot be told.
pub const AMBIGUOUS_SOURCE: &str = "ambiguous:";

/// Why an imported `new` symbol must not replace the project's copy `old` of the same id, if it
/// must not: they come from different items of their library, or which item cannot be told
/// (#301). A copy without a recorded source (from before sources were recorded, or made in the
/// project) is not refused here.
pub fn replace_refused(old: &Symbol, new: &Symbol) -> Option<String> {
    if old.source.is_empty() {
        return None;
    }
    if old.source.starts_with(AMBIGUOUS_SOURCE) || new.source.starts_with(AMBIGUOUS_SOURCE) {
        return Some(format!(
            "symbol {}: its library has more than one item it could be ({}), so which one replaces it cannot be told",
            new.id,
            if new.source.starts_with(AMBIGUOUS_SOURCE) {
                &new.source
            } else {
                &old.source
            }
        ));
    }
    (old.source != new.source).then(|| {
        format!(
            "symbol {}: the import's item is another one of its library ({}) than the project's copy ({})",
            new.id, new.source, old.source
        )
    })
}

impl Symbol {
    pub fn hash(&self) -> String {
        content_hash(self)
    }

    pub fn pin(&self, name: &str) -> Option<&Pin> {
        self.pins.iter().find(|p| p.name == name)
    }

    /// The box drawn for a placed symbol whose definition is missing: 4 mm round its origin.
    pub fn unknown_bounds(at: Point) -> (Point, Point) {
        (at + Point::new(-4.0, -4.0), at + Point::new(4.0, 4.0))
    }

    /// The box around the symbol's drawing when placed at `at`, origin included.
    pub fn placed_bounds(&self, at: Point, rotation: Rotation, mirror: bool) -> (Point, Point) {
        let mut pts = vec![at];
        for g in &self.graphics {
            let (a, b) = g.placed(at, rotation, mirror).bounds();
            pts.extend([a, b]);
        }
        bounds_of(&pts)
    }
}

/// The edge of a black box a connection point sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxSide {
    Top,
    Bottom,
    Left,
    Right,
}

impl BoxSide {
    /// `left`, `right`, `top` or `bottom` (also `l`, `r`, `t`, `b`), any case.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "top" | "t" | "up" => Some(BoxSide::Top),
            "bottom" | "b" | "down" => Some(BoxSide::Bottom),
            "left" | "l" => Some(BoxSide::Left),
            "right" | "r" => Some(BoxSide::Right),
            _ => None,
        }
    }
}

/// A connection point of a black box: its name, the edge it sits on and how far along that
/// edge, in mm from the edge's left or top end.
#[derive(Debug, Clone, PartialEq)]
pub struct BoxPin {
    pub name: String,
    pub side: BoxSide,
    pub offset: f64,
}

/// A black box (device box): a rectangle `width` by `height` mm with its top-left corner at
/// the origin and connection points on its edges, for a device that has no symbol of its own.
/// Wires leave each point away from the box. Refuses a box without area, a point off its edge,
/// a point without a name, two points of one name and two points at one place.
pub fn black_box(id: &str, name: &str, width: f64, height: f64, pins: &[BoxPin]) -> crate::Result<Symbol> {
    let bad = |why: String| Err(crate::Error::Invalid(why));
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return bad(format!(
            "a black box needs a width and height above zero, not {width} by {height}"
        ));
    }
    let mut out: Vec<Pin> = Vec::new();
    for p in pins {
        let pin_name = p.name.trim();
        if pin_name.is_empty() {
            return bad("a connection point of a black box needs a name".into());
        }
        let length = match p.side {
            BoxSide::Top | BoxSide::Bottom => width,
            BoxSide::Left | BoxSide::Right => height,
        };
        if !(p.offset.is_finite() && (0.0..=length).contains(&p.offset)) {
            return bad(format!(
                "connection point {pin_name}: {} mm is off its edge, which is {length} mm long",
                p.offset
            ));
        }
        let (at, dir) = match p.side {
            BoxSide::Top => (Point::new(p.offset, 0.0), PinDir::Up),
            BoxSide::Bottom => (Point::new(p.offset, height), PinDir::Down),
            BoxSide::Left => (Point::new(0.0, p.offset), PinDir::Left),
            BoxSide::Right => (Point::new(width, p.offset), PinDir::Right),
        };
        if out.iter().any(|q| q.name == pin_name) {
            return bad(format!("two connection points are named {pin_name}"));
        }
        if let Some(q) = out.iter().find(|q| q.at.distance(at) < 1e-6) {
            return bad(format!(
                "connection points {} and {pin_name} are at the same place",
                q.name
            ));
        }
        out.push(Pin {
            name: pin_name.to_owned(),
            at,
            dir,
        });
    }
    Ok(Symbol {
        source: String::new(),
        id: id.to_owned(),
        name: name.to_owned(),
        category: "Black box".into(),
        letter_code: String::new(),
        role: SymbolRole::Standalone,
        graphics: vec![Shape::Rect {
            min: Point::new(0.0, 0.0),
            max: Point::new(width, height),
        }],
        pins: out,
        tag_at: Point::new(0.0, -2.0),
        description: String::new(),
    })
}

/// The size (width, height) and connection points of a black box made by [`black_box`], read
/// back from its symbol so the box can be edited: `None` for any other symbol.
pub fn black_box_layout(sym: &Symbol) -> Option<(f64, f64, Vec<BoxPin>)> {
    if sym.category != "Black box" {
        return None;
    }
    let [Shape::Rect { min, max }] = sym.graphics.as_slice() else {
        return None;
    };
    if min.x != 0.0 || min.y != 0.0 {
        return None;
    }
    let pins = sym
        .pins
        .iter()
        .map(|p| {
            let (side, offset) = match p.dir {
                PinDir::Up => (BoxSide::Top, p.at.x),
                PinDir::Down => (BoxSide::Bottom, p.at.x),
                PinDir::Left => (BoxSide::Left, p.at.y),
                PinDir::Right => (BoxSide::Right, p.at.y),
            };
            BoxPin {
                name: p.name.clone(),
                side,
                offset,
            }
        })
        .collect();
    Some((max.x, max.y, pins))
}

#[cfg(test)]
mod black_box_tests {
    use super::*;

    fn pin(name: &str, side: BoxSide, offset: f64) -> BoxPin {
        BoxPin {
            name: name.into(),
            side,
            offset,
        }
    }

    #[test]
    fn points_sit_on_their_edges_facing_out() -> crate::Result<()> {
        let s = black_box(
            "box.a1",
            "-A1",
            40.0,
            20.0,
            &[
                pin("1", BoxSide::Left, 5.0),
                pin("2", BoxSide::Right, 15.0),
                pin("PE", BoxSide::Bottom, 40.0),
                pin("X", BoxSide::Top, 0.0),
            ],
        )?;
        let at = |n: &str| s.pin(n).map(|p| (p.at, p.dir));
        assert_eq!(at("1"), Some((Point::new(0.0, 5.0), PinDir::Left)));
        assert_eq!(at("2"), Some((Point::new(40.0, 15.0), PinDir::Right)));
        assert_eq!(at("PE"), Some((Point::new(40.0, 20.0), PinDir::Down)));
        assert_eq!(at("X"), Some((Point::new(0.0, 0.0), PinDir::Up)));
        assert_eq!(s.graphics.len(), 1);
        Ok(())
    }

    #[test]
    fn refuses_a_flat_box_and_points_that_do_not_fit() {
        assert!(black_box("b", "", 0.0, 10.0, &[]).is_err());
        assert!(black_box("b", "", 10.0, f64::NAN, &[]).is_err());
        assert!(black_box("b", "", 10.0, 10.0, &[pin("1", BoxSide::Left, 10.5)]).is_err());
        assert!(black_box("b", "", 10.0, 10.0, &[pin("1", BoxSide::Left, -1.0)]).is_err());
        assert!(black_box("b", "", 10.0, 10.0, &[pin(" ", BoxSide::Left, 1.0)]).is_err());
        let twice = [pin("1", BoxSide::Left, 1.0), pin("1", BoxSide::Right, 1.0)];
        assert!(black_box("b", "", 10.0, 10.0, &twice).is_err());
        // The top-left corner is on the top and the left edge.
        let corner = [pin("1", BoxSide::Left, 0.0), pin("2", BoxSide::Top, 0.0)];
        assert!(black_box("b", "", 10.0, 10.0, &corner).is_err());
    }

    #[test]
    fn a_black_box_reads_back_its_size_and_points() -> crate::Result<()> {
        let pins = [
            pin("1", BoxSide::Left, 5.0),
            pin("2", BoxSide::Right, 15.0),
            pin("PE", BoxSide::Bottom, 40.0),
            pin("X", BoxSide::Top, 0.0),
        ];
        let s = black_box("box.a1", "-A1", 40.0, 20.0, &pins)?;
        assert_eq!(black_box_layout(&s), Some((40.0, 20.0, pins.to_vec())));
        // Built again from what was read, it is the same symbol.
        let (w, h, read) = black_box_layout(&s).unwrap_or_default();
        assert_eq!(black_box("box.a1", "-A1", w, h, &read)?, s);
        let mut other = s.clone();
        other.category = "Relays".into();
        assert_eq!(black_box_layout(&other), None);
        Ok(())
    }

    #[test]
    fn sides_parse_in_short_and_long_form() {
        assert_eq!(BoxSide::parse("Left"), Some(BoxSide::Left));
        assert_eq!(BoxSide::parse("b"), Some(BoxSide::Bottom));
        assert_eq!(BoxSide::parse("middle"), None);
    }
}

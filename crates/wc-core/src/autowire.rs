//! Auto-wiring: which pins should be joined by a wire because they line up.
//!
//! Two pins are joined when they face each other on one straight line (same x for pins that
//! point up and down, same y for pins that point left and right) with nothing between them:
//! no other pin, no other symbol and no wire end. Pins of different symbols that touch are
//! joined too. Pins that already have a wire are left alone.

use std::collections::BTreeSet;

use crate::geom::{Point, Rotation, bounds_of};
use crate::project::{Endpoint, Page};
use crate::symbol::{PinDir, Symbol};

/// How close two coordinates must be to count as the same, in mm.
const SAME: f64 = 0.01;

/// A pin of a placed symbol, on the page.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedPin {
    pub item: String,
    pub pin: String,
    pub at: Point,
    /// Where a wire leaves the pin, after the symbol's rotation and mirroring.
    pub dir: PinDir,
}

/// The direction a pin points after its symbol is rotated and mirrored.
pub fn turned(dir: PinDir, rotation: Rotation, mirror: bool) -> PinDir {
    let v = match dir {
        PinDir::Up => Point::new(0.0, -1.0),
        PinDir::Down => Point::new(0.0, 1.0),
        PinDir::Left => Point::new(-1.0, 0.0),
        PinDir::Right => Point::new(1.0, 0.0),
    };
    let v = v.transform(rotation, mirror);
    if v.y < -0.5 {
        PinDir::Up
    } else if v.y > 0.5 {
        PinDir::Down
    } else if v.x < 0.0 {
        PinDir::Left
    } else {
        PinDir::Right
    }
}

fn step(dir: PinDir) -> (f64, f64) {
    match dir {
        PinDir::Up => (0.0, -1.0),
        PinDir::Down => (0.0, 1.0),
        PinDir::Left => (-1.0, 0.0),
        PinDir::Right => (1.0, 0.0),
    }
}

fn opposite(dir: PinDir) -> PinDir {
    match dir {
        PinDir::Up => PinDir::Down,
        PinDir::Down => PinDir::Up,
        PinDir::Left => PinDir::Right,
        PinDir::Right => PinDir::Left,
    }
}

/// Every pin on a page, placed. Symbols missing from `symbol` are skipped.
pub fn placed_pins<'a>(page: &Page, symbol: impl Fn(&str) -> Option<&'a Symbol>) -> Vec<PlacedPin> {
    let mut out = Vec::new();
    for s in &page.symbols {
        let Some(sym) = symbol(&s.symbol) else { continue };
        for p in &sym.pins {
            out.push(PlacedPin {
                item: s.id.clone(),
                pin: p.name.clone(),
                at: p.at.transform(s.rotation, s.mirror) + s.at,
                dir: turned(p.dir, s.rotation, s.mirror),
            });
        }
    }
    out
}

/// The wires to add for the symbols in `items` (just placed or moved): one `(from, to)` pair per
/// pin pair that lines up or touches. Each pin is used at most once.
pub fn suggest<'a>(
    page: &Page,
    symbol: impl Fn(&str) -> Option<&'a Symbol> + Copy,
    items: &BTreeSet<String>,
) -> Vec<(Endpoint, Endpoint)> {
    let pins = placed_pins(page, symbol);
    let wired: BTreeSet<(&str, &str)> = page
        .wires
        .iter()
        .flat_map(|w| [&w.from, &w.to])
        .filter_map(|e| match e {
            Endpoint::Pin { item, pin } => Some((item.as_str(), pin.as_str())),
            Endpoint::Point { .. } => None,
        })
        .collect();
    let free = |p: &PlacedPin| !wired.contains(&(p.item.as_str(), p.pin.as_str()));
    // Where existing wires end, for "nothing in between".
    let wire_ends: Vec<Point> = page
        .wires
        .iter()
        .flat_map(|w| [&w.from, &w.to])
        .filter_map(|e| match e {
            Endpoint::Point { at } => Some(*at),
            Endpoint::Pin { item, pin } => pins.iter().find(|p| &p.item == item && &p.pin == pin).map(|p| p.at),
        })
        .collect();
    let boxes: Vec<(&str, Point, Point)> = page
        .symbols
        .iter()
        .filter_map(|s| {
            let sym = symbol(&s.symbol)?;
            let mut pts = vec![s.at];
            for g in &sym.graphics {
                let (a, b) = g.placed(s.at, s.rotation, s.mirror).bounds();
                pts.extend([a, b]);
            }
            let (lo, hi) = bounds_of(&pts);
            Some((s.id.as_str(), lo, hi))
        })
        .collect();

    let mut used: BTreeSet<usize> = BTreeSet::new();
    let mut out = Vec::new();
    let mut join = |i: usize, j: usize, used: &mut BTreeSet<usize>| {
        if used.contains(&i) || used.contains(&j) {
            return;
        }
        used.insert(i);
        used.insert(j);
        let end = |p: &PlacedPin| Endpoint::Pin {
            item: p.item.clone(),
            pin: p.pin.clone(),
        };
        out.push((end(&pins[i]), end(&pins[j])));
    };

    let moving = |p: &PlacedPin| items.contains(&p.item);
    // Touching pins first: they need no room between them.
    for (i, a) in pins.iter().enumerate() {
        if !moving(a) || !free(a) {
            continue;
        }
        for (j, b) in pins.iter().enumerate() {
            if a.item != b.item && free(b) && a.at.distance(b.at) < SAME && (!moving(b) || i < j) {
                join(i, j, &mut used);
            }
        }
    }
    for (i, a) in pins.iter().enumerate() {
        if !moving(a) || !free(a) || used.contains(&i) {
            continue;
        }
        let (dx, dy) = step(a.dir);
        // The nearest pin straight ahead, of any symbol.
        let mut best: Option<(f64, usize)> = None;
        for (j, b) in pins.iter().enumerate() {
            if i == j {
                continue;
            }
            let (vx, vy) = (b.at.x - a.at.x, b.at.y - a.at.y);
            let along = vx * dx + vy * dy;
            let across = (vx * dy - vy * dx).abs();
            if along > SAME && across < SAME && best.is_none_or(|(d, _)| along < d) {
                best = Some((along, j));
            }
        }
        let Some((_, j)) = best else { continue };
        let b = &pins[j];
        if b.item == a.item || b.dir != opposite(a.dir) || !free(b) || used.contains(&j) {
            continue;
        }
        if blocked(a.at, b.at, &wire_ends, &boxes, [&a.item, &b.item]) {
            continue;
        }
        join(i, j, &mut used);
    }
    out
}

/// Whether the straight run from `a` to `b` passes a wire end or another symbol's body.
fn blocked(a: Point, b: Point, wire_ends: &[Point], boxes: &[(&str, Point, Point)], ends: [&String; 2]) -> bool {
    let (lo, hi) = bounds_of(&[a, b]);
    let vertical = (a.x - b.x).abs() < SAME;
    let strictly_between = |p: Point| {
        if vertical {
            (p.x - a.x).abs() < SAME && p.y > lo.y + SAME && p.y < hi.y - SAME
        } else {
            (p.y - a.y).abs() < SAME && p.x > lo.x + SAME && p.x < hi.x - SAME
        }
    };
    if wire_ends.iter().any(|p| strictly_between(*p)) {
        return true;
    }
    boxes.iter().any(|(id, bl, bh)| {
        if ends.iter().any(|e| e.as_str() == *id) {
            return false;
        }
        // The run crosses the box when it overlaps it on both axes (open along the run, closed
        // across it, so a symbol whose outline is on the line still blocks).
        let across = if vertical {
            a.x >= bl.x - SAME && a.x <= bh.x + SAME
        } else {
            a.y >= bl.y - SAME && a.y <= bh.y + SAME
        };
        let along = if vertical {
            bh.y > lo.y + SAME && bl.y < hi.y - SAME
        } else {
            bh.x > lo.x + SAME && bl.x < hi.x - SAME
        };
        across && along
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::PlacedSymbol;
    use crate::geom::Shape;
    use crate::symbol::Pin;

    /// A two-pin part like the library's coil: pin 1 on top pointing up, pin 2 below pointing
    /// down, body from y 4 to 11.
    fn part() -> Symbol {
        let mut s: Symbol = serde_json::from_str(r#"{"id":"part","name":"Part","graphics":[],"pins":[]}"#).unwrap();
        s.pins = vec![
            Pin {
                name: "1".into(),
                at: Point::new(0.0, 0.0),
                dir: PinDir::Up,
            },
            Pin {
                name: "2".into(),
                at: Point::new(0.0, 15.0),
                dir: PinDir::Down,
            },
        ];
        s.graphics = vec![
            Shape::Line {
                a: Point::new(0.0, 0.0),
                b: Point::new(0.0, 4.0),
            },
            Shape::Rect {
                min: Point::new(-4.0, 4.0),
                max: Point::new(4.0, 11.0),
            },
            Shape::Line {
                a: Point::new(0.0, 11.0),
                b: Point::new(0.0, 15.0),
            },
        ];
        s
    }

    fn put(page: &mut Page, id: &str, x: f64, y: f64, rotation: Rotation) {
        page.symbols.push(PlacedSymbol {
            id: id.into(),
            symbol: "part".into(),
            at: Point::new(x, y),
            rotation,
            mirror: false,
            tag: None,
        });
    }

    fn run(page: &Page, items: &[&str]) -> Vec<(String, String)> {
        let sym = part();
        let items = items.iter().map(|s| (*s).to_owned()).collect();
        let name = |e: &Endpoint| match e {
            Endpoint::Pin { item, pin } => format!("{item}:{pin}"),
            Endpoint::Point { .. } => "point".into(),
        };
        suggest(page, |_| Some(&sym), &items)
            .iter()
            .map(|(a, b)| (name(a), name(b)))
            .collect()
    }

    #[test]
    fn facing_pins_get_a_wire() {
        let mut page = Page::new("p1", "1", "");
        put(&mut page, "a", 50.0, 20.0, Rotation::R0);
        put(&mut page, "b", 50.0, 60.0, Rotation::R0);
        assert_eq!(run(&page, &["b"]), vec![("b:1".into(), "a:2".into())]);
        // Not on the same line: nothing.
        let mut off = Page::new("p1", "1", "");
        put(&mut off, "a", 50.0, 20.0, Rotation::R0);
        put(&mut off, "b", 52.5, 60.0, Rotation::R0);
        assert!(run(&off, &["b"]).is_empty());
    }

    #[test]
    fn pins_facing_away_get_none() {
        let mut page = Page::new("p1", "1", "");
        put(&mut page, "a", 50.0, 20.0, Rotation::R0);
        put(&mut page, "b", 50.0, 60.0, Rotation::R180);
        // b turned half round: its pin 1 (y 60) now points down, away from a, and only its
        // pin 2 (y 45, pointing up) faces a's pin 2 (y 35, pointing down).
        assert_eq!(run(&page, &["b"]), vec![("b:2".into(), "a:2".into())]);
        // Side by side, pins point up and down, never at each other.
        let mut side = Page::new("p1", "1", "");
        put(&mut side, "a", 50.0, 20.0, Rotation::R0);
        put(&mut side, "b", 70.0, 20.0, Rotation::R0);
        assert!(run(&side, &["b"]).is_empty());
    }

    #[test]
    fn a_symbol_or_wire_end_in_between_blocks_it() {
        let mut page = Page::new("p1", "1", "");
        put(&mut page, "a", 50.0, 0.0, Rotation::R0);
        put(&mut page, "mid", 52.0, 30.0, Rotation::R90);
        put(&mut page, "b", 50.0, 60.0, Rotation::R0);
        // "mid" lies across the line between a and b (turned, it spans x 37..52, y 26..34).
        assert!(run(&page, &["b"]).is_empty());

        let mut page = Page::new("p1", "1", "");
        put(&mut page, "a", 50.0, 0.0, Rotation::R0);
        put(&mut page, "b", 50.0, 60.0, Rotation::R0);
        page.wires.push(crate::Wire {
            id: "w1".into(),
            from: Endpoint::Point {
                at: Point::new(50.0, 40.0),
            },
            to: Endpoint::Point {
                at: Point::new(80.0, 40.0),
            },
            via: Vec::new(),
            number: None,
            potential: None,
            cable: None,
        });
        assert!(run(&page, &["b"]).is_empty(), "a wire ends on the line");
    }

    #[test]
    fn touching_pins_are_joined() {
        let mut page = Page::new("p1", "1", "");
        put(&mut page, "a", 50.0, 20.0, Rotation::R0);
        put(&mut page, "b", 50.0, 35.0, Rotation::R0);
        assert_eq!(run(&page, &["b"]), vec![("b:1".into(), "a:2".into())]);
    }

    #[test]
    fn rotated_symbols_line_up_sideways() {
        let mut page = Page::new("p1", "1", "");
        // Turned a quarter: pin 1 at the symbol's origin points right, pin 2 at x-15 points left.
        put(&mut page, "a", 50.0, 20.0, Rotation::R90);
        put(&mut page, "b", 100.0, 20.0, Rotation::R90);
        assert_eq!(turned(PinDir::Up, Rotation::R90, false), PinDir::Right);
        assert_eq!(run(&page, &["b"]), vec![("b:2".into(), "a:1".into())]);
    }

    #[test]
    fn wired_pins_are_left_alone() {
        let mut page = Page::new("p1", "1", "");
        put(&mut page, "a", 50.0, 20.0, Rotation::R0);
        put(&mut page, "b", 50.0, 60.0, Rotation::R0);
        page.wires.push(crate::Wire {
            id: "w1".into(),
            from: Endpoint::Pin {
                item: "a".into(),
                pin: "2".into(),
            },
            to: Endpoint::Point {
                at: Point::new(90.0, 35.0),
            },
            via: Vec::new(),
            number: None,
            potential: None,
            cable: None,
        });
        assert!(run(&page, &["b"]).is_empty());
    }
}

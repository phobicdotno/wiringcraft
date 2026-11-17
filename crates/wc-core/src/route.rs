//! Wire route styles: the bend points a wire gets between its two ends.
//!
//! A wire is drawn as a polyline from its first end through its `via` points to its last end.
//! A [`Route`] style generates those points from where the ends are and which way they leave
//! their pins:
//!
//! - [`Route::Square`]: square corners, for example "right, down, right".
//! - [`Route::Sloped`]: the same, with the middle turn as a 45 degree slope, for example
//!   "right, slope down, right".
//! - [`Route::Straight`]: one straight line.

use serde::{Deserialize, Serialize};

use crate::geom::{Point, Rotation};
use crate::symbol::PinDir;

/// How close two coordinates must be to count as the same, in mm.
const SAME: f64 = 1e-6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    /// Square corners. Wires saved before route styles existed are this.
    #[default]
    Square,
    /// Square corners with the middle turn as a 45 degree slope.
    Sloped,
    /// One straight line between the ends.
    Straight,
}

impl Route {
    pub const ALL: [Route; 3] = [Route::Square, Route::Sloped, Route::Straight];

    pub fn label(self) -> &'static str {
        match self {
            Route::Square => "Square corners",
            Route::Sloped => "Sloped middle (45°)",
            Route::Straight => "Straight line",
        }
    }

    pub fn is_square(&self) -> bool {
        *self == Route::Square
    }
}

/// Which way a wire leaves its end: along x or along y.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    /// The axis of a pin after its symbol is rotated (mirroring keeps the axis).
    pub fn of(dir: PinDir, rotation: Rotation) -> Axis {
        let flat = matches!(dir, PinDir::Left | PinDir::Right);
        let turned = matches!(rotation, Rotation::R90 | Rotation::R270);
        if flat != turned {
            Axis::Horizontal
        } else {
            Axis::Vertical
        }
    }
}

/// The bend points of a wire from `a` to `b` in `style`. `a_axis` and `b_axis` say which way
/// the wire leaves each end; `None` (a free point) follows the other end, or the longer
/// distance when neither end has one.
///
/// Ends on one straight line are always joined straight. When their pins point across that
/// line (two side pins one above the other), a square route would have to leave the line and
/// come back, and which side is free depends on the drawing round it, so that is left to the
/// user to draw by hand.
pub fn via(a: Point, a_axis: Option<Axis>, b: Point, b_axis: Option<Axis>, style: Route) -> Vec<Point> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    if style == Route::Straight || dx.abs() < SAME || dy.abs() < SAME {
        return Vec::new();
    }
    let fallback = if dx.abs() >= dy.abs() {
        Axis::Horizontal
    } else {
        Axis::Vertical
    };
    let a_axis = a_axis.or(b_axis).unwrap_or(fallback);
    let b_axis = b_axis.unwrap_or(a_axis);
    let sloped = style == Route::Sloped;
    let pts = match (a_axis, b_axis) {
        (Axis::Horizontal, Axis::Horizontal) => {
            let mx = a.x + dx / 2.0;
            if sloped {
                // The slope takes as much of the height as half the width allows, so both ends
                // keep a straight run along their axis; any rest of the height is a straight
                // drop after it.
                let d = (dx.abs() / 2.0).min(dy.abs());
                let (sx, sy) = (dx.signum(), dy.signum());
                vec![
                    Point::new(mx - sx * d / 2.0, a.y),
                    Point::new(mx + sx * d / 2.0, a.y + sy * d),
                    Point::new(mx + sx * d / 2.0, b.y),
                ]
            } else {
                vec![Point::new(mx, a.y), Point::new(mx, b.y)]
            }
        }
        (Axis::Vertical, Axis::Vertical) => {
            let my = a.y + dy / 2.0;
            if sloped {
                let d = (dy.abs() / 2.0).min(dx.abs());
                let (sx, sy) = (dx.signum(), dy.signum());
                vec![
                    Point::new(a.x, my - sy * d / 2.0),
                    Point::new(a.x + sx * d, my + sy * d / 2.0),
                    Point::new(b.x, my + sy * d / 2.0),
                ]
            } else {
                vec![Point::new(a.x, my), Point::new(b.x, my)]
            }
        }
        // One end leaves along x and the other along y: one corner, or a cut corner.
        (Axis::Horizontal, Axis::Vertical) => corner(a, b, Point::new(b.x, a.y), sloped),
        (Axis::Vertical, Axis::Horizontal) => corner(a, b, Point::new(a.x, b.y), sloped),
    };
    tidy(a, b, pts)
}

/// One corner at `c` between `a` and `b`; sloped, the corner is cut at 45 degrees halfway along
/// the shorter leg.
fn corner(a: Point, b: Point, c: Point, sloped: bool) -> Vec<Point> {
    if !sloped {
        return vec![c];
    }
    let d = a.distance(c).min(c.distance(b)) / 2.0;
    let toward = |from: Point, to: Point| {
        let len = from.distance(to);
        Point::new(from.x + (to.x - from.x) * d / len, from.y + (to.y - from.y) * d / len)
    };
    vec![toward(c, a), toward(c, b)]
}

/// Drop points that repeat the one before them (or the ends), so a route has no zero-length legs.
fn tidy(a: Point, b: Point, pts: Vec<Point>) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(pts.len());
    for p in pts {
        let last = out.last().copied().unwrap_or(a);
        if p.distance(last) >= SAME {
            out.push(p);
        }
    }
    while out.last().is_some_and(|p| p.distance(b) < SAME) {
        out.pop();
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    /// Every leg is horizontal, vertical, or (when `slopes`) at 45 degrees.
    fn legs_ok(a: Point, b: Point, via: &[Point], slopes: bool) -> bool {
        let mut all = vec![a];
        all.extend_from_slice(via);
        all.push(b);
        all.windows(2).all(|w| {
            let (dx, dy) = ((w[1].x - w[0].x).abs(), (w[1].y - w[0].y).abs());
            dx < SAME || dy < SAME || (slopes && (dx - dy).abs() < SAME)
        })
    }

    #[test]
    fn square_between_two_side_pins_goes_across_down_across() {
        let (a, b) = (p(0.0, 0.0), p(40.0, 20.0));
        let v = via(a, Some(Axis::Horizontal), b, Some(Axis::Horizontal), Route::Square);
        assert_eq!(v, vec![p(20.0, 0.0), p(20.0, 20.0)]);
    }

    #[test]
    fn square_between_two_top_and_bottom_pins_goes_down_across_down() {
        let (a, b) = (p(0.0, 0.0), p(40.0, 20.0));
        let v = via(a, Some(Axis::Vertical), b, Some(Axis::Vertical), Route::Square);
        assert_eq!(v, vec![p(0.0, 10.0), p(40.0, 10.0)]);
    }

    #[test]
    fn sloped_between_side_pins_slopes_in_the_middle() {
        let (a, b) = (p(0.0, 0.0), p(40.0, 20.0));
        let v = via(a, Some(Axis::Horizontal), b, Some(Axis::Horizontal), Route::Sloped);
        assert_eq!(v, vec![p(10.0, 0.0), p(30.0, 20.0)]);
        assert!(legs_ok(a, b, &v, true));
        // Going left and up too.
        let v = via(b, Some(Axis::Horizontal), a, Some(Axis::Horizontal), Route::Sloped);
        assert_eq!(v, vec![p(30.0, 20.0), p(10.0, 0.0)]);
    }

    #[test]
    fn sloped_with_more_height_than_width_drops_the_rest() {
        let (a, b) = (p(0.0, 0.0), p(10.0, 30.0));
        let v = via(a, Some(Axis::Horizontal), b, Some(Axis::Horizontal), Route::Sloped);
        // Both ends keep a straight run sideways before and after the 5 mm slope.
        assert_eq!(v, vec![p(2.5, 0.0), p(7.5, 5.0), p(7.5, 30.0)]);
        assert!(legs_ok(a, b, &v, true));
        // The same turned on its side, for top and bottom pins.
        let b = p(30.0, 10.0);
        let v = via(a, Some(Axis::Vertical), b, Some(Axis::Vertical), Route::Sloped);
        assert_eq!(v, vec![p(0.0, 2.5), p(5.0, 7.5), p(30.0, 7.5)]);
        assert!(legs_ok(a, b, &v, true));
    }

    #[test]
    fn sloped_ends_leave_along_their_axis() {
        let a = p(0.0, 0.0);
        for b in [p(10.0, 30.0), p(40.0, 20.0), p(-10.0, -30.0), p(5.0, 50.0)] {
            let v = via(a, Some(Axis::Horizontal), b, Some(Axis::Horizontal), Route::Sloped);
            let (first, last) = (v[0], v[v.len() - 1]);
            assert!(
                (first.y - a.y).abs() < SAME && (first.x - a.x).abs() > SAME,
                "{b:?}: {v:?}"
            );
            assert!(
                (last.y - b.y).abs() < SAME && (last.x - b.x).abs() > SAME,
                "{b:?}: {v:?}"
            );
        }
    }

    #[test]
    fn side_pins_on_one_vertical_line_are_joined_straight() {
        // No square route without a detour; see `via`.
        let (a, b) = (p(0.0, 0.0), p(0.0, 30.0));
        for style in Route::ALL {
            assert!(via(a, Some(Axis::Horizontal), b, Some(Axis::Horizontal), style).is_empty());
        }
    }

    #[test]
    fn sloped_between_top_and_bottom_pins() {
        let (a, b) = (p(0.0, 0.0), p(20.0, 40.0));
        let v = via(a, Some(Axis::Vertical), b, Some(Axis::Vertical), Route::Sloped);
        assert_eq!(v, vec![p(0.0, 10.0), p(20.0, 30.0)]);
        assert!(legs_ok(a, b, &v, true));
    }

    #[test]
    fn mixed_ends_get_one_corner_or_a_cut_corner() {
        let (a, b) = (p(0.0, 0.0), p(40.0, 20.0));
        let v = via(a, Some(Axis::Horizontal), b, Some(Axis::Vertical), Route::Square);
        assert_eq!(v, vec![p(40.0, 0.0)]);
        let v = via(a, Some(Axis::Vertical), b, Some(Axis::Horizontal), Route::Square);
        assert_eq!(v, vec![p(0.0, 20.0)]);
        let v = via(a, Some(Axis::Horizontal), b, Some(Axis::Vertical), Route::Sloped);
        assert_eq!(v, vec![p(30.0, 0.0), p(40.0, 10.0)]);
        assert!(legs_ok(a, b, &v, true));
    }

    #[test]
    fn straight_and_lined_up_ends_need_no_bends() {
        let (a, b) = (p(0.0, 0.0), p(40.0, 20.0));
        assert!(via(a, Some(Axis::Horizontal), b, Some(Axis::Horizontal), Route::Straight).is_empty());
        for style in Route::ALL {
            assert!(via(a, Some(Axis::Vertical), p(0.0, 30.0), Some(Axis::Vertical), style).is_empty());
            assert!(via(a, Some(Axis::Horizontal), p(25.0, 0.0), None, style).is_empty());
        }
    }

    #[test]
    fn free_points_follow_the_other_end_or_the_longer_distance() {
        let (a, b) = (p(0.0, 0.0), p(40.0, 20.0));
        assert_eq!(
            via(a, None, b, Some(Axis::Vertical), Route::Square),
            vec![p(0.0, 10.0), p(40.0, 10.0)]
        );
        assert_eq!(via(a, None, b, None, Route::Square), vec![p(20.0, 0.0), p(20.0, 20.0)]);
        assert_eq!(
            via(a, None, p(10.0, 40.0), None, Route::Square),
            vec![p(0.0, 20.0), p(10.0, 20.0)]
        );
    }

    #[test]
    fn rotation_turns_the_pin_axis() {
        assert_eq!(Axis::of(PinDir::Up, Rotation::R0), Axis::Vertical);
        assert_eq!(Axis::of(PinDir::Up, Rotation::R90), Axis::Horizontal);
        assert_eq!(Axis::of(PinDir::Left, Rotation::R270), Axis::Vertical);
        assert_eq!(Axis::of(PinDir::Right, Rotation::R180), Axis::Horizontal);
    }

    #[test]
    fn older_wires_load_as_square_and_square_is_not_written() {
        let old =
            r#"{"id":"w1","from":{"at":{"x":0.0,"y":0.0}},"to":{"at":{"x":10.0,"y":5.0}},"via":[{"x":10.0,"y":0.0}]}"#;
        let w: crate::Wire = serde_json::from_str(old).unwrap();
        assert_eq!(w.route, Route::Square);
        assert_eq!(w.via, vec![p(10.0, 0.0)]);
        assert!(!serde_json::to_string(&w).unwrap().contains("route"));
        let sloped = crate::Wire {
            route: Route::Sloped,
            ..w
        };
        let text = serde_json::to_string(&sloped).unwrap();
        assert!(text.contains(r#""route":"sloped""#), "{text}");
        let back: crate::Wire = serde_json::from_str(&text).unwrap();
        assert_eq!(back, sloped);
    }
}

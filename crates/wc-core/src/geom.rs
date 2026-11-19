//! Page geometry in millimetres. The y axis points down, like a drawing sheet read from the top.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn distance(self, o: Point) -> f64 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }

    /// Rotate by a multiple of 90 degrees (clockwise on the page) and optionally mirror in x.
    pub fn transform(self, rotation: Rotation, mirror: bool) -> Point {
        let p = if mirror { Point::new(-self.x, self.y) } else { self };
        match rotation {
            Rotation::R0 => p,
            Rotation::R90 => Point::new(-p.y, p.x),
            Rotation::R180 => Point::new(-p.x, -p.y),
            Rotation::R270 => Point::new(p.y, -p.x),
        }
    }
}

impl std::ops::Add for Point {
    type Output = Point;
    fn add(self, o: Point) -> Point {
        Point::new(self.x + o.x, self.y + o.y)
    }
}

/// Symbols on a schematic turn in quarter steps only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Rotation {
    #[default]
    #[serde(rename = "0")]
    R0,
    #[serde(rename = "90")]
    R90,
    #[serde(rename = "180")]
    R180,
    #[serde(rename = "270")]
    R270,
}

impl Rotation {
    pub fn from_degrees(d: i64) -> Option<Self> {
        match d.rem_euclid(360) {
            0 => Some(Self::R0),
            90 => Some(Self::R90),
            180 => Some(Self::R180),
            270 => Some(Self::R270),
            _ => None,
        }
    }

    /// The turn in degrees: 0, 90, 180 or 270.
    pub fn degrees(self) -> i64 {
        match self {
            Self::R0 => 0,
            Self::R90 => 90,
            Self::R180 => 180,
            Self::R270 => 270,
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Self::R0
    }
}

/// Horizontal text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// A drawing primitive, used for symbol graphics and free page graphics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Shape {
    Line {
        a: Point,
        b: Point,
    },
    Polyline {
        points: Vec<Point>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        closed: bool,
        /// Filled with the line colour (a solid hatch, an arrow head); a filled polyline is closed.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        filled: bool,
    },
    Rect {
        min: Point,
        max: Point,
    },
    Circle {
        center: Point,
        radius: f64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        filled: bool,
    },
    /// Arc from `start` to `end` degrees, counter-clockwise in mathematical sense on a y-down page
    /// (that is, clockwise as seen on paper).
    Arc {
        center: Point,
        radius: f64,
        start: f64,
        end: f64,
    },
    Text {
        at: Point,
        text: String,
        #[serde(default = "default_text_height")]
        height: f64,
        #[serde(default, skip_serializing_if = "is_left")]
        align: Align,
        /// Reading direction in degrees, counter-clockwise on paper: 90 reads bottom to top.
        #[serde(default, skip_serializing_if = "is_zero")]
        angle: f64,
    },
    /// A linear dimension between `a` and `b`: extension lines out to a dimension line drawn
    /// `offset` mm to the left of the direction from `a` to `b` (negative: to the right), with
    /// arrows at both ends and the distance in mm as text above its middle.
    Dimension {
        a: Point,
        b: Point,
        #[serde(default, skip_serializing_if = "is_zero")]
        offset: f64,
        #[serde(default = "default_text_height")]
        height: f64,
    },
}

fn default_text_height() -> f64 {
    2.5
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

fn is_left(a: &Align) -> bool {
    *a == Align::Left
}

impl Shape {
    /// Apply a placement: rotate/mirror around the local origin, then move by `offset`.
    pub fn placed(&self, offset: Point, rotation: Rotation, mirror: bool) -> Shape {
        let t = |p: Point| p.transform(rotation, mirror) + offset;
        match self {
            Shape::Line { a, b } => Shape::Line { a: t(*a), b: t(*b) },
            Shape::Polyline { points, closed, filled } => Shape::Polyline {
                points: points.iter().map(|p| t(*p)).collect(),
                closed: *closed,
                filled: *filled,
            },
            Shape::Rect { min, max } => {
                let (a, b) = (t(*min), t(*max));
                Shape::Rect {
                    min: Point::new(a.x.min(b.x), a.y.min(b.y)),
                    max: Point::new(a.x.max(b.x), a.y.max(b.y)),
                }
            }
            Shape::Circle { center, radius, filled } => Shape::Circle {
                center: t(*center),
                radius: *radius,
                filled: *filled,
            },
            Shape::Arc {
                center,
                radius,
                start,
                end,
            } => {
                // Mirror flips the sweep; each quarter turn adds 90 degrees on a y-down page.
                let quarter = match rotation {
                    Rotation::R0 => 0.0,
                    Rotation::R90 => 90.0,
                    Rotation::R180 => 180.0,
                    Rotation::R270 => 270.0,
                };
                let (s, e) = if mirror {
                    (180.0 - end, 180.0 - start)
                } else {
                    (*start, *end)
                };
                Shape::Arc {
                    center: t(*center),
                    radius: *radius,
                    start: s + quarter,
                    end: e + quarter,
                }
            }
            // Text keeps its reading direction when its symbol turns, so it stays readable.
            Shape::Text {
                at,
                text,
                height,
                align,
                angle,
            } => Shape::Text {
                at: t(*at),
                text: text.clone(),
                height: *height,
                align: *align,
                angle: *angle,
            },
            // A mirror turns left of a→b into right of it.
            Shape::Dimension { a, b, offset, height } => Shape::Dimension {
                a: t(*a),
                b: t(*b),
                offset: if mirror { -offset } else { *offset },
                height: *height,
            },
        }
    }

    /// Axis-aligned bounds (min, max). Text counts as its anchor point only.
    pub fn bounds(&self) -> (Point, Point) {
        let pts: Vec<Point> = match self {
            Shape::Line { a, b } => vec![*a, *b],
            Shape::Polyline { points, .. } => points.clone(),
            Shape::Rect { min, max } => vec![*min, *max],
            Shape::Circle { center, radius, .. } | Shape::Arc { center, radius, .. } => vec![
                Point::new(center.x - radius, center.y - radius),
                Point::new(center.x + radius, center.y + radius),
            ],
            Shape::Text { at, .. } => vec![*at],
            Shape::Dimension { .. } => {
                let parts = self.dimension_parts().unwrap_or_default();
                parts
                    .iter()
                    .flat_map(|s| {
                        let (a, b) = s.bounds();
                        [a, b]
                    })
                    .collect()
            }
        };
        bounds_of(&pts)
    }

    /// The drawing of a [`Shape::Dimension`] as plain shapes (extension lines, the dimension
    /// line, two filled arrow heads and the measured length as text), for painting and export.
    /// `None` for any other shape; empty for a dimension of no length.
    pub fn dimension_parts(&self) -> Option<Vec<Shape>> {
        let Shape::Dimension { a, b, offset, height } = self else {
            return None;
        };
        let (a, b, offset, height) = (*a, *b, *offset, *height);
        let len = a.distance(b);
        if len <= 0.0 || !len.is_finite() {
            return Some(Vec::new());
        }
        // Unit direction a→b and its left normal on a y-down page (left as seen on paper).
        let u = Point::new((b.x - a.x) / len, (b.y - a.y) / len);
        let n = Point::new(u.y, -u.x);
        let at = |p: Point, k: f64| Point::new(p.x + n.x * k, p.y + n.y * k);
        let (da, db) = (at(a, offset), at(b, offset));
        // Extension lines run from a small gap off the measured point to just past the line.
        let side = if offset < 0.0 { -1.0 } else { 1.0 };
        let (gap, over) = (DIM_GAP.min(offset.abs()), DIM_OVERSHOOT);
        let mut out = Vec::new();
        if offset != 0.0 {
            out.push(Shape::Line {
                a: at(a, gap * side),
                b: at(a, offset + over * side),
            });
            out.push(Shape::Line {
                a: at(b, gap * side),
                b: at(b, offset + over * side),
            });
        }
        out.push(Shape::Line { a: da, b: db });
        // Arrow heads point outwards to the extension lines, shortened on a very short dimension.
        let arrow = DIM_ARROW.min(len / 3.0);
        let head = |tip: Point, dir: f64| {
            let back = Point::new(tip.x + u.x * arrow * dir, tip.y + u.y * arrow * dir);
            let w = arrow / 3.0;
            Shape::Polyline {
                points: vec![
                    tip,
                    Point::new(back.x + n.x * w, back.y + n.y * w),
                    Point::new(back.x - n.x * w, back.y - n.y * w),
                ],
                closed: true,
                filled: true,
            }
        };
        out.push(head(da, 1.0));
        out.push(head(db, -1.0));
        // The text sits on the outer side of the line, reading left to right or bottom to top.
        let mut angle = (-u.y).atan2(u.x).to_degrees();
        if angle > 90.0 + 1e-9 {
            angle -= 180.0;
        } else if angle <= -90.0 + 1e-9 {
            angle += 180.0;
        }
        let mid = Point::new((da.x + db.x) / 2.0, (da.y + db.y) / 2.0);
        // "Up" for the text is its reading direction turned left on paper.
        let r = angle.to_radians();
        let up = Point::new(-r.sin(), -r.cos());
        let lift = DIM_TEXT_GAP;
        out.push(Shape::Text {
            at: Point::new(mid.x + up.x * lift, mid.y + up.y * lift),
            text: format_length(len),
            height,
            align: Align::Center,
            angle: if angle.abs() < 1e-9 { 0.0 } else { angle },
        });
        Some(out)
    }

    pub fn translate(&self, d: Point) -> Shape {
        self.placed(d, Rotation::R0, false)
    }

    /// Scaled by `k` about (0, 0); radii, text heights and dimension offsets scale too.
    pub fn scaled(&self, k: f64) -> Shape {
        let p = |p: &Point| Point::new(p.x * k, p.y * k);
        match self {
            Shape::Line { a, b } => Shape::Line { a: p(a), b: p(b) },
            Shape::Polyline { points, closed, filled } => Shape::Polyline {
                points: points.iter().map(p).collect(),
                closed: *closed,
                filled: *filled,
            },
            Shape::Rect { min, max } => Shape::Rect {
                min: p(min),
                max: p(max),
            },
            Shape::Circle { center, radius, filled } => Shape::Circle {
                center: p(center),
                radius: radius * k,
                filled: *filled,
            },
            Shape::Arc {
                center,
                radius,
                start,
                end,
            } => Shape::Arc {
                center: p(center),
                radius: radius * k,
                start: *start,
                end: *end,
            },
            Shape::Text {
                at,
                text,
                height,
                align,
                angle,
            } => Shape::Text {
                at: p(at),
                text: text.clone(),
                height: height * k,
                align: *align,
                angle: *angle,
            },
            Shape::Dimension { a, b, offset, height } => Shape::Dimension {
                a: p(a),
                b: p(b),
                offset: offset * k,
                height: height * k,
            },
        }
    }
}

/// Gap between a measured point and its extension line, in mm.
const DIM_GAP: f64 = 1.0;
/// How far extension lines run past the dimension line, in mm.
const DIM_OVERSHOOT: f64 = 1.5;
/// Arrow head length, in mm.
const DIM_ARROW: f64 = 2.5;
/// Gap between the dimension line and its text baseline, in mm.
const DIM_TEXT_GAP: f64 = 1.0;

/// A length in mm as a dimension shows it: one decimal, without a trailing ".0".
pub fn format_length(mm: f64) -> String {
    let r = (mm * 10.0).round() / 10.0;
    if r.fract() == 0.0 {
        format!("{r:.0}")
    } else {
        format!("{r:.1}")
    }
}

pub fn bounds_of(pts: &[Point]) -> (Point, Point) {
    let mut min = Point::new(f64::INFINITY, f64::INFINITY);
    let mut max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in pts {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    if pts.is_empty() {
        (Point::default(), Point::default())
    } else {
        (min, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_turns_compose_to_identity() {
        let p = Point::new(3.0, -2.0);
        let mut q = p;
        for _ in 0..4 {
            q = q.transform(Rotation::R90, false);
        }
        assert_eq!(p, q);
        assert_eq!(p.transform(Rotation::R90, false), Point::new(2.0, 3.0));
    }

    fn dim(a: (f64, f64), b: (f64, f64), offset: f64) -> Shape {
        Shape::Dimension {
            a: Point::new(a.0, a.1),
            b: Point::new(b.0, b.1),
            offset,
            height: 2.5,
        }
    }

    fn dim_text(s: &Shape) -> (Point, String, f64) {
        s.dimension_parts()
            .unwrap_or_default()
            .iter()
            .find_map(|p| match p {
                Shape::Text { at, text, angle, .. } => Some((*at, text.clone(), *angle)),
                _ => None,
            })
            .unwrap_or_else(|| unreachable!("a dimension has a text"))
    }

    #[test]
    fn a_dimension_draws_its_lines_arrows_and_length() {
        // Left of a→b going right on a y-down page is up on paper: the line sits 10 mm above.
        let d = dim((10.0, 50.0), (35.0, 50.0), 10.0);
        let parts = d.dimension_parts().unwrap_or_default();
        assert!(parts.contains(&Shape::Line {
            a: Point::new(10.0, 40.0),
            b: Point::new(35.0, 40.0)
        }));
        assert_eq!(parts.iter().filter(|p| matches!(p, Shape::Line { .. })).count(), 3);
        assert_eq!(
            parts
                .iter()
                .filter(|p| matches!(p, Shape::Polyline { filled: true, .. }))
                .count(),
            2
        );
        let (at, text, angle) = dim_text(&d);
        assert_eq!((text.as_str(), angle), ("25", 0.0));
        assert!(at.y < 40.0, "the text is above the dimension line: {at:?}");
        let (min, max) = d.bounds();
        assert!(min.y <= 40.0 && max.y >= 50.0 - DIM_GAP && min.x <= 10.0 && max.x >= 35.0);
        // Drawn right to left it reads the same way round.
        let back = dim((35.0, 50.0), (10.0, 50.0), -10.0);
        assert_eq!(dim_text(&back).2, 0.0);
        // Measured upwards on paper it reads bottom to top, left of the line.
        let up = dim((10.0, 50.0), (10.0, 20.0), 5.0);
        let (at, text, angle) = dim_text(&up);
        assert_eq!((text.as_str(), angle), ("30", 90.0));
        assert!(at.x < 5.0, "{at:?}");
        assert_eq!(format_length(12.345), "12.3");
        assert!(
            dim((1.0, 1.0), (1.0, 1.0), 3.0)
                .dimension_parts()
                .is_some_and(|p| p.is_empty())
        );
        assert_eq!(
            Shape::Line {
                a: Point::default(),
                b: Point::new(1.0, 0.0)
            }
            .dimension_parts(),
            None
        );
    }

    #[test]
    fn a_dimension_moves_mirrors_and_round_trips() {
        let d = dim((10.0, 50.0), (35.0, 50.0), 10.0);
        let moved = d.translate(Point::new(5.0, 5.0));
        assert_eq!(moved, dim((15.0, 55.0), (40.0, 55.0), 10.0));
        // Mirrored left to right, the direction from a to b turns round: the offset flips so
        // the dimension line stays above the points.
        let mirrored = d.placed(Point::default(), Rotation::R0, true);
        assert_eq!(mirrored, dim((-10.0, 50.0), (-35.0, 50.0), -10.0));
        assert!(mirrored.bounds().0.y <= 40.0);
        let json = serde_json::to_string(&d).unwrap_or_default();
        assert!(json.contains("\"kind\":\"dimension\""), "{json}");
        assert_eq!(serde_json::from_str::<Shape>(&json).ok(), Some(d));
        // Height and offset have defaults.
        let bare = serde_json::from_str::<Shape>(r#"{"kind":"dimension","a":{"x":0,"y":0},"b":{"x":5,"y":0}}"#).ok();
        assert_eq!(bare, Some(dim((0.0, 0.0), (5.0, 0.0), 0.0)));
    }

    #[test]
    fn rotation_from_degrees() {
        assert_eq!(Rotation::from_degrees(-90), Some(Rotation::R270));
        assert_eq!(Rotation::from_degrees(45), None);
    }
}

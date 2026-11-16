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
    },
}

fn default_text_height() -> f64 {
    2.5
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
            Shape::Polyline { points, closed } => Shape::Polyline {
                points: points.iter().map(|p| t(*p)).collect(),
                closed: *closed,
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
            Shape::Text {
                at,
                text,
                height,
                align,
            } => Shape::Text {
                at: t(*at),
                text: text.clone(),
                height: *height,
                align: *align,
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
        };
        bounds_of(&pts)
    }

    pub fn translate(&self, d: Point) -> Shape {
        self.placed(d, Rotation::R0, false)
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

    #[test]
    fn rotation_from_degrees() {
        assert_eq!(Rotation::from_degrees(-90), Some(Rotation::R270));
        assert_eq!(Rotation::from_degrees(45), None);
    }
}

//! ASCII DXF reading: symbols and whole drawings.
//!
//! Supported entities: LINE, CIRCLE, ARC, ELLIPSE, SPLINE, HATCH, SOLID/TRACE, LWPOLYLINE,
//! POLYLINE/VERTEX, TEXT, MTEXT, POINT, INSERT (block references, exploded or turned into
//! library symbols) and DIMENSION (drawn by its block). Ellipses and splines become polylines through sampled points; a solid hatch
//! and a SOLID become filled outlines, a patterned hatch its outline. Other entities are counted
//! in one warning per kind.
//! DXF has y pointing up; WiringCraft pages have y pointing down, so y is flipped on the way in.

use std::collections::{BTreeMap, BTreeSet};

use wc_core::geom::{Align, Point, Rotation, Shape};
use wc_core::{DeviceTag, PlacedSymbol};

use crate::{Error, Result, make_symbol};

/// One entity as group-code/value pairs.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Entity {
    pub(crate) kind: String,
    pub(crate) codes: Vec<(i32, String)>,
}

impl Entity {
    fn get(&self, code: i32) -> Option<&str> {
        self.codes.iter().find(|(c, _)| *c == code).map(|(_, v)| v.as_str())
    }
    pub(crate) fn f(&self, code: i32) -> f64 {
        self.get(code)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(0.0)
    }
    fn f_or(&self, code: i32, d: f64) -> f64 {
        self.get(code)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(d)
    }
    fn layer(&self) -> String {
        self.get(8).unwrap_or("").to_uppercase()
    }
    fn all(&self, code: i32) -> Vec<f64> {
        self.codes
            .iter()
            .filter(|(c, _)| *c == code)
            .filter_map(|(_, v)| v.trim().parse().ok())
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Drawing {
    pub(crate) entities: Vec<Entity>,
    pub(crate) blocks: BTreeMap<String, Vec<Entity>>,
    /// The base point of each block that has one other than (0, 0): the point of the block an
    /// INSERT puts on its insertion point.
    pub(crate) bases: BTreeMap<String, (f64, f64)>,
}

impl Drawing {
    /// The entities of block `name`: its own spelling first, else any spelling (block names are
    /// not case-sensitive).
    fn block(&self, name: &str) -> Option<(&String, &Vec<Entity>)> {
        self.blocks
            .get_key_value(name)
            .or_else(|| self.blocks.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)))
    }
}

/// How deep block references (and dimensions) are followed inside each other.
const MAX_NESTING: usize = 8;

/// The skipped-count key for entities in blocks nested deeper than the nesting limit (8).
pub const TOO_DEEP: &str = "entities in blocks nested too deep (or inserting themselves)";

/// The most entities one import draws, block references expanded: a few nested blocks that
/// each insert the next many times would otherwise fill the memory.
pub const MAX_DRAWN: usize = 2_000_000;

/// The key under which entities left out over [`MAX_DRAWN`] are counted.
pub(crate) const OVER_LIMIT: &str = "over the limit";

/// Shapes and POINT positions, each with its layer.
pub(crate) type Drawn = (Vec<(String, Shape)>, Vec<(String, Point)>);

/// Draw `ents` of `d` as page shapes with their layers (y flipped, unscaled), block references
/// expanded, and the POINT positions. Entities not drawn are counted in `skipped`. Each entity
/// visited takes one from `budget`; when it runs out the rest is left out (and counted).
pub(crate) fn draw(d: &Drawing, ents: &[Entity], skipped: &mut BTreeMap<String, usize>, budget: &mut usize) -> Drawn {
    draw_mapped(d, ents, Xf::ID.m, 0.0, skipped, budget)
}

/// [`draw`], with `ents` first mapped by the linear map `m` (row by row: x' = m[0] x + m[1] y,
/// y' = m[2] x + m[3] y). Texts turn by `rot` degrees.
pub(crate) fn draw_mapped(
    d: &Drawing,
    ents: &[Entity],
    m: [f64; 4],
    rot: f64,
    skipped: &mut BTreeMap<String, usize>,
    budget: &mut usize,
) -> Drawn {
    let mut out = Vec::new();
    let mut points = Vec::new();
    shapes(
        d,
        ents,
        Xf { m, at: (0.0, 0.0), rot },
        1.0,
        0,
        &|_| false,
        &mut out,
        &mut points,
        skipped,
        budget,
    );
    (out, points)
}

fn pairs(text: &str) -> Result<Vec<(i32, String)>> {
    let mut lines = text.lines();
    let mut out = Vec::new();
    while let Some(code) = lines.next() {
        let code = code.trim();
        if code.is_empty() {
            continue;
        }
        let code: i32 = code
            .parse()
            .map_err(|_| Error::Format(format!("not an ASCII DXF file (group code {code:?})")))?;
        let value = lines.next().unwrap_or("").trim_end_matches('\r').to_owned();
        out.push((code, value));
    }
    Ok(out)
}

fn parse(text: &str) -> Result<Drawing> {
    if text.starts_with("AutoCAD Binary DXF") {
        return Err(Error::Format("binary DXF is not supported; save as ASCII DXF".into()));
    }
    let pairs = pairs(text)?;
    let mut d = Drawing::default();
    let mut section = String::new();
    let mut current: Option<Entity> = None;
    let mut block: Option<(String, Vec<Entity>)> = None;
    // POLYLINE collects its VERTEX entities until SEQEND.
    let mut polyline: Option<Entity> = None;
    let mut iter = pairs.into_iter().peekable();
    let finish = |e: Entity,
                  block: &mut Option<(String, Vec<Entity>)>,
                  polyline: &mut Option<Entity>,
                  d: &mut Drawing| match e.kind.as_str() {
        "POLYLINE" => *polyline = Some(e),
        "VERTEX" => {
            if let Some(p) = polyline.as_mut() {
                p.codes.push((10, e.get(10).unwrap_or("0").into()));
                p.codes.push((20, e.get(20).unwrap_or("0").into()));
            }
        }
        "SEQEND" => {
            if let Some(mut p) = polyline.take() {
                p.kind = "LWPOLYLINE".into();
                match block.as_mut() {
                    Some((_, ents)) => ents.push(p),
                    None => d.entities.push(p),
                }
            }
        }
        "BLOCK" | "ENDBLK" | "" => {}
        _ => match block.as_mut() {
            Some((_, ents)) => ents.push(e),
            None => d.entities.push(e),
        },
    };
    while let Some((code, value)) = iter.next() {
        if code == 0 {
            if let Some(e) = current.take() {
                if e.kind == "BLOCK" {
                    let name = e.get(2).unwrap_or("").to_owned();
                    let base = (e.f(10), e.f(20));
                    if base != (0.0, 0.0) {
                        d.bases.insert(name.clone(), base);
                    }
                    block = Some((name, Vec::new()));
                } else if e.kind == "ENDBLK" {
                    if let Some((name, ents)) = block.take() {
                        d.blocks.insert(name, ents);
                    }
                } else {
                    finish(e, &mut block, &mut polyline, &mut d);
                }
            }
            match value.as_str() {
                "SECTION" => {
                    if let Some((2, name)) = iter.peek().cloned() {
                        section = name;
                        iter.next();
                    }
                }
                "ENDSEC" => section.clear(),
                "EOF" => break,
                kind if section == "ENTITIES" || section == "BLOCKS" => {
                    current = Some(Entity {
                        kind: kind.to_owned(),
                        codes: Vec::new(),
                    });
                }
                _ => {}
            }
        } else if let Some(e) = current.as_mut() {
            e.codes.push((code, value));
        }
    }
    if let Some(e) = current.take() {
        finish(e, &mut block, &mut polyline, &mut d);
    }
    Ok(d)
}

/// Placement of a block reference: a 2D affine map, so placements nested in mirrored or
/// unevenly scaled ones compose exactly, plus the sum of the turns for text directions.
#[derive(Debug, Clone, Copy)]
struct Xf {
    /// Linear part, row by row: x' = m[0] x + m[1] y, y' = m[2] x + m[3] y.
    m: [f64; 4],
    at: (f64, f64),
    /// The turns of the placements, added up: texts keep their reading direction by it.
    rot: f64,
}

impl Xf {
    const ID: Xf = Xf {
        m: [1.0, 0.0, 0.0, 1.0],
        at: (0.0, 0.0),
        rot: 0.0,
    };
    /// Scale by `sx`, `sy` (a negative scale mirrors), turn by `rot` degrees counter-clockwise,
    /// then move to `at`: how a block reference places its block.
    fn insert(at: (f64, f64), sx: f64, sy: f64, rot: f64) -> Xf {
        let (s, c) = rot.to_radians().sin_cos();
        Xf {
            m: [c * sx, -s * sy, s * sx, c * sy],
            at,
            rot,
        }
    }
    /// A move by `by`.
    fn shift(by: (f64, f64)) -> Xf {
        Xf { at: by, ..Xf::ID }
    }
    /// DXF coordinates (y up) to DXF coordinates.
    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.m;
        (self.at.0 + m[0] * x + m[1] * y, self.at.1 + m[2] * x + m[3] * y)
    }
    /// This placement inside `outer`: `outer` applied after this one.
    fn then(&self, outer: &Xf) -> Xf {
        let (a, b) = (&outer.m, &self.m);
        Xf {
            m: [
                a[0] * b[0] + a[1] * b[2],
                a[0] * b[1] + a[1] * b[3],
                a[2] * b[0] + a[3] * b[2],
                a[2] * b[1] + a[3] * b[3],
            ],
            at: outer.apply(self.at.0, self.at.1),
            rot: self.rot + outer.rot,
        }
    }
    fn det(&self) -> f64 {
        self.m[0] * self.m[3] - self.m[1] * self.m[2]
    }
    /// How lengths grow (the mean of the two axes' scales): radii and text heights.
    fn len_scale(&self) -> f64 {
        self.det().abs().sqrt()
    }
    /// Where the direction at angle 0 points once placed, in degrees, written as close to the
    /// sum of the turns as possible so plain turns keep their angles as written.
    fn x_angle(&self) -> f64 {
        let a = self.m[2].atan2(self.m[0]).to_degrees();
        self.rot + normal_angle(a - self.rot)
    }
}

#[allow(clippy::too_many_arguments)]
/// Convert entities to page shapes (y flipped, scaled). Returns shapes and POINT positions.
fn shapes(
    d: &Drawing,
    ents: &[Entity],
    xf: Xf,
    scale: f64,
    depth: usize,
    skip_layer: &dyn Fn(&str) -> bool,
    out: &mut Vec<(String, Shape)>,
    points: &mut Vec<(String, Point)>,
    skipped: &mut BTreeMap<String, usize>,
    budget: &mut usize,
) {
    if depth > MAX_NESTING {
        // Block nesting this deep is either a cycle or not worth drawing; say what is left out.
        if !ents.is_empty() {
            *skipped.entry(TOO_DEEP.to_owned()).or_default() += ents.len();
        }
        return;
    }
    let p = |x: f64, y: f64| {
        let (x, y) = xf.apply(x, y);
        Point::new(x * scale, -y * scale)
    };
    let r_scale = xf.len_scale() * scale;
    for (i, e) in ents.iter().enumerate() {
        if *budget == 0 {
            // This entity and every one after it at this level are left out.
            *skipped.entry(OVER_LIMIT.to_owned()).or_default() += ents.len() - i;
            return;
        }
        *budget -= 1;
        let layer = e.layer();
        if skip_layer(&layer) {
            continue;
        }
        // An extrusion pointing down (left by mirroring in AutoCAD) mirrors the entity's own
        // coordinate system in x. LINE, ELLIPSE and SPLINE are in world coordinates already.
        let flip = e.f_or(230, 1.0) < 0.0;
        let q = |x: f64, y: f64| if flip { p(-x, y) } else { p(x, y) };
        let shape = match e.kind.as_str() {
            "LINE" => Some(Shape::Line {
                a: p(e.f(10), e.f(20)),
                b: p(e.f(11), e.f(21)),
            }),
            "CIRCLE" => Some(Shape::Circle {
                center: q(e.f(10), e.f(20)),
                radius: e.f(40) * r_scale,
                filled: false,
            }),
            "ARC" => {
                // Mirrored in x, the arc from s to e runs from 180 - e to 180 - s.
                let (s, en) = if flip {
                    (180.0 - e.f(51), 180.0 - e.f(50))
                } else {
                    (e.f(50), e.f(51))
                };
                // A mirroring placement turns the angle a into x_angle - a, so the arc runs the
                // other way round; any other placement turns it by x_angle.
                let turn = xf.x_angle();
                let (s, en) = if xf.det() < 0.0 {
                    (turn - en, turn - s)
                } else {
                    (s + turn, en + turn)
                };
                Some(Shape::Arc {
                    center: q(e.f(10), e.f(20)),
                    radius: e.f(40) * r_scale,
                    start: -en,
                    end: -s,
                })
            }
            "LWPOLYLINE" => {
                let xs = e.all(10);
                let ys = e.all(20);
                let pts: Vec<Point> = xs
                    .iter()
                    .zip(&ys)
                    .filter(|(x, y)| x.is_finite() && y.is_finite())
                    .map(|(x, y)| q(*x, *y))
                    .collect();
                (pts.len() >= 2).then(|| Shape::Polyline {
                    points: pts,
                    closed: (e.f(70) as i64) & 1 == 1,
                    filled: false,
                })
            }
            "ELLIPSE" => {
                // The minor axis is the extrusion turned onto the major one, so a downward
                // extrusion runs the parameter the other way round.
                let ratio = e.f_or(40, 1.0);
                let (pts, closed) = ellipse_points(
                    (e.f(10), e.f(20)),
                    (e.f(11), e.f(21)),
                    if flip { -ratio } else { ratio },
                    e.f_or(41, 0.0),
                    e.f_or(42, std::f64::consts::TAU),
                );
                polyline(pts.into_iter().map(|(x, y)| p(x, y)).collect(), closed, false)
            }
            "SPLINE" => {
                let (pts, closed) = spline_points(e);
                polyline(pts.into_iter().map(|(x, y)| p(x, y)).collect(), closed, false)
            }
            "HATCH" => {
                let mut solid = e.get(2).is_some_and(|n| n.trim().eq_ignore_ascii_case("SOLID")) || e.f(70) as i64 == 1;
                // A loop whose points are not all numbers, in the drawing or once placed on the
                // page (a huge radius or scale), is left out and counted. A solid hatch that lost
                // one is drawn as outlines: the lost loop may have been a hole.
                let placed = |(x, y): (f64, f64)| {
                    let pt = q(x, y);
                    x.is_finite() && y.is_finite() && pt.x.is_finite() && pt.y.is_finite()
                };
                let (loops, bad): (Vec<_>, Vec<_>) = hatch_loops(e)
                    .into_iter()
                    .partition(|lp| lp.iter().all(|pt| placed(*pt)));
                if !bad.is_empty() {
                    *skipped.entry(HATCH_NOT_NUMBERS.to_owned()).or_default() += 1;
                    solid = false;
                }
                // Islands inside a solid fill stay open: each is cut into the loop around it.
                // Style (75): 0 fills every other level, 1 only the outermost area, 2 ignores
                // islands.
                let style = hatch_style(e);
                let (outlines, filled) = if solid {
                    match with_holes(&loops, style) {
                        Some(o) => (o, true),
                        None => {
                            // Too many holes to cut in: the boundaries, not a wrong fill.
                            *skipped.entry(HATCH_UNFILLED.to_owned()).or_default() += 1;
                            (loops, false)
                        }
                    }
                } else {
                    (loops, false)
                };
                for lp in outlines {
                    if let Some(s) = polyline(lp.into_iter().map(|(x, y)| q(x, y)).collect(), true, filled) {
                        out.push((layer.clone(), s));
                    }
                }
                None
            }
            "TEXT" | "MTEXT" | "ATTRIB" => {
                let raw = e.get(1).unwrap_or("");
                let text = if e.kind == "MTEXT" {
                    mtext_plain(raw)
                } else {
                    raw.to_owned()
                };
                let align = match e.f(72) as i64 {
                    1 | 4 => Align::Center,
                    2 => Align::Right,
                    _ => Align::Left,
                };
                (!text.trim().is_empty()).then(|| Shape::Text {
                    at: p(e.f(10), e.f(20)),
                    text,
                    height: (e.f_or(40, 2.5) * r_scale).max(0.5),
                    align,
                    // Code 50: rotation in degrees, counter-clockwise; a turned block turns it too.
                    angle: normal_angle(e.f(50) + xf.rot),
                })
            }
            "SOLID" | "TRACE" => {
                // Corners in the order 1, 2, 4, 3: the third and fourth make a "Z".
                let mut pts = vec![q(e.f(10), e.f(20)), q(e.f(11), e.f(21)), q(e.f(13), e.f(23))];
                let third = q(e.f(12), e.f(22));
                if pts.last().is_none_or(|l| l.distance(third) > 1e-9) {
                    pts.push(third);
                }
                polyline(pts, true, true)
            }
            "POINT" => {
                points.push((layer.clone(), p(e.f(10), e.f(20))));
                None
            }
            "INSERT" => {
                if let Some((name, inner)) = e.get(2).and_then(|n| d.blocks.get_key_value(n)) {
                    // The block's base point goes on the insertion point.
                    let (bx, by) = d.bases.get(name).copied().unwrap_or_default();
                    let ixf = Xf::shift((-bx, -by))
                        .then(&Xf::insert(
                            (e.f(10), e.f(20)),
                            e.f_or(41, 1.0),
                            e.f_or(42, 1.0),
                            e.f(50),
                        ))
                        .then(&xf);
                    shapes(
                        d,
                        inner,
                        ixf,
                        scale,
                        depth + 1,
                        skip_layer,
                        out,
                        points,
                        skipped,
                        budget,
                    );
                }
                None
            }
            // A dimension is drawn by its (anonymous) block, placed as it is.
            "DIMENSION" if e.get(2).and_then(|n| d.block(n)).is_some() => {
                if let Some((name, inner)) = e.get(2).and_then(|n| d.block(n)) {
                    // Placed by its base point like any block (normally (0, 0) for a dimension).
                    let (bx, by) = d.bases.get(name).copied().unwrap_or_default();
                    let dxf = Xf::shift((-bx, -by)).then(&xf);
                    shapes(
                        d,
                        inner,
                        dxf,
                        scale,
                        depth + 1,
                        skip_layer,
                        out,
                        points,
                        skipped,
                        budget,
                    );
                }
                None
            }
            // Attribute templates of a block (their values come with the INSERT) and paper-space
            // viewports, which draw nothing themselves.
            "ATTDEF" | "VIEWPORT" => None,
            other => {
                *skipped.entry(other.to_owned()).or_default() += 1;
                None
            }
        };
        if let Some(s) = shape {
            out.push((layer, s));
        }
    }
}

/// A polyline of at least two points, or nothing.
fn polyline(points: Vec<Point>, closed: bool, filled: bool) -> Option<Shape> {
    (points.len() >= 2).then_some(Shape::Polyline { points, closed, filled })
}

/// Segments per full turn when an ellipse or arc is sampled.
const TURN_STEPS: f64 = 72.0;

/// A sweep brought into (0, `full`]: a whole number of turns is one full turn. `None` when it is
/// not a number.
fn turn(sweep: f64, full: f64) -> Option<f64> {
    if !sweep.is_finite() {
        return None;
    }
    let r = sweep.rem_euclid(full);
    Some(if r <= 1e-12 * full { full } else { r })
}

/// Points along an ellipse (DXF coordinates): centre, end of the major axis relative to the
/// centre, minor/major ratio and the start and end parameters in radians. A full ellipse comes
/// back closed, without repeating its first point.
fn ellipse_points(c: (f64, f64), major: (f64, f64), ratio: f64, t0: f64, t1: f64) -> (Vec<(f64, f64)>, bool) {
    let minor = (-major.1 * ratio, major.0 * ratio);
    // File angles can be anything: bring the sweep into (0, one turn].
    let Some(sweep) = turn(t1 - t0, std::f64::consts::TAU) else {
        return (Vec::new(), false);
    };
    // A huge start angle would swallow the small steps added to it.
    let t0 = t0.rem_euclid(std::f64::consts::TAU);
    let full = sweep >= std::f64::consts::TAU - 1e-9;
    let n = ((TURN_STEPS * sweep / std::f64::consts::TAU).ceil() as usize).max(4);
    let at = |t: f64| {
        let (s, k) = t.sin_cos();
        (c.0 + k * major.0 + s * minor.0, c.1 + k * major.1 + s * minor.1)
    };
    let last = if full { n - 1 } else { n };
    let pts = (0..=last).map(|i| at(t0 + sweep * i as f64 / n as f64)).collect();
    (pts, full)
}

/// Points along a circular arc (DXF coordinates, degrees), counter-clockwise from `a0` to `a1`,
/// or clockwise when `ccw` is false (then `a1` is reached going down from `a0`).
fn arc_points(c: (f64, f64), r: f64, a0: f64, a1: f64, ccw: bool) -> Vec<(f64, f64)> {
    let Some(sweep) = turn(if ccw { a1 - a0 } else { a0 - a1 }, 360.0) else {
        return Vec::new();
    };
    let a0 = a0.rem_euclid(360.0);
    let n = ((TURN_STEPS * sweep / 360.0).ceil() as usize).max(2);
    let dir = if ccw { 1.0 } else { -1.0 };
    (0..=n)
        .map(|i| {
            let t = (a0 + dir * sweep * i as f64 / n as f64).to_radians();
            (c.0 + r * t.cos(), c.1 + r * t.sin())
        })
        .collect()
}

/// A point on a B-spline (de Boor): homogeneous control points (x*w, y*w, w), knots, degree.
fn de_boor(ctrl: &[(f64, f64, f64)], knots: &[f64], p: usize, u: f64) -> Option<(f64, f64)> {
    let n = ctrl.len();
    if n <= p || knots.len() != n + p + 1 {
        return None;
    }
    let mut k = p;
    while k + 1 < n && knots.get(k + 1).is_some_and(|&t| t <= u) {
        k += 1;
    }
    let mut d: Vec<(f64, f64, f64)> = ctrl.get(k - p..=k)?.to_vec();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let i = j + k - p;
            let (ki, kr) = (*knots.get(i)?, *knots.get(i + p + 1 - r)?);
            let denom = kr - ki;
            let a = if denom.abs() < 1e-12 { 0.0 } else { (u - ki) / denom };
            let (q, s) = (*d.get(j - 1)?, *d.get(j)?);
            *d.get_mut(j)? = (
                (1.0 - a) * q.0 + a * s.0,
                (1.0 - a) * q.1 + a * s.1,
                (1.0 - a) * q.2 + a * s.2,
            );
        }
    }
    let (x, y, w) = *d.get(p)?;
    (w.abs() > 1e-12).then(|| (x / w, y / w))
}

/// The highest spline degree evaluated as a curve; above it a spline is drawn through its fit or
/// control points.
const MAX_SPLINE_DEGREE: usize = 15;

/// The most point pairs tried as a bridge from one hole to the outline around it; a larger hole
/// leaves the hatch unfilled rather than searched for longer.
const BRIDGE_BUDGET: usize = 4_000_000;

/// The most vertex visits spent finding which loop of a hatch lies in which; past it the hatch
/// is left unfilled.
const NESTING_BUDGET: usize = 20_000_000;

/// The most work (point pairs, wall edges collected and crossing tests) spent on all the bridges
/// of one hatch together; past it the hatch is left unfilled.
const HATCH_BRIDGE_WORK: usize = 20_000_000;

/// The `skipped` key of a solid hatch drawn as its boundaries because its holes could not be cut.
const HATCH_UNFILLED: &str = "solid HATCH (holes too complex)";

/// The key under which hatches with a boundary that is not all numbers are counted.
const HATCH_NOT_NUMBERS: &str = "HATCH with a boundary that is not numbers";

/// Points along a B-spline from its control points, knots and weights, sampled evenly over the
/// knot range. `None` when the data do not make a spline.
fn bspline_points(ctrl: &[(f64, f64)], knots: &[f64], weights: &[f64], degree: usize) -> Option<Vec<(f64, f64)>> {
    let n = ctrl.len();
    // Drawings use degree 3 or so; a huge degree would cost samples x degree^2 steps.
    if degree == 0
        || degree > MAX_SPLINE_DEGREE
        || n <= degree
        || knots.len() != n + degree + 1
        || knots.windows(2).any(|w| w[1] < w[0])
    {
        return None;
    }
    let w = |i: usize| {
        weights
            .get(i)
            .copied()
            .filter(|w| *w > 0.0 && weights.len() == n)
            .unwrap_or(1.0)
    };
    let h: Vec<(f64, f64, f64)> = ctrl
        .iter()
        .enumerate()
        .map(|(i, (x, y))| (x * w(i), y * w(i), w(i)))
        .collect();
    let (u0, u1) = (*knots.get(degree)?, *knots.get(n)?);
    if u1 <= u0 {
        return None;
    }
    let steps = (n * 16).clamp(16, 2000);
    let mut out = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        // The last sample sits a hair inside the range so it falls in the last span.
        let u = if i == steps {
            u1
        } else {
            u0 + (u1 - u0) * i as f64 / steps as f64
        };
        let u = if i == steps { u - (u1 - u0) * 1e-12 } else { u };
        out.push(de_boor(&h, knots, degree, u).filter(|p| p.0.is_finite() && p.1.is_finite())?);
    }
    if let (Some(last), Some(end)) = (out.last_mut(), ctrl.last()) {
        // A clamped spline ends on its last control point; snap the rounding away.
        if (last.0 - end.0).hypot(last.1 - end.1) < 1e-6 {
            *last = *end;
        }
    }
    Some(out)
}

/// Points of a SPLINE entity: the curve through its control points, else through its fit
/// points; and whether it is closed.
fn spline_points(e: &Entity) -> (Vec<(f64, f64)>, bool) {
    let closed = (e.f(70) as i64) & 1 == 1;
    // Whole pairs only, both coordinates finite: a NaN would make the project file unreadable.
    let finite = |(x, y): &(f64, f64)| x.is_finite() && y.is_finite();
    let ctrl: Vec<(f64, f64)> = e.all(10).into_iter().zip(e.all(20)).filter(finite).collect();
    let fit: Vec<(f64, f64)> = e.all(11).into_iter().zip(e.all(21)).filter(finite).collect();
    let degree = e.f_or(71, 3.0).max(0.0) as usize;
    if let Some(pts) = bspline_points(&ctrl, &e.all(40), &e.all(41), degree) {
        return (pts, closed);
    }
    if fit.len() >= 2 {
        return (fit, closed);
    }
    (ctrl, closed)
}

/// Reads the group codes of an entity in order.
struct Cursor<'a> {
    codes: &'a [(i32, String)],
    i: usize,
}

impl Cursor<'_> {
    /// The value of the next pair when it has `code`; the cursor moves on only then.
    fn take(&mut self, code: i32) -> Option<f64> {
        let (c, v) = self.codes.get(self.i)?;
        if *c != code {
            return None;
        }
        self.i += 1;
        v.trim().parse::<f64>().ok().filter(|v| v.is_finite()).or(Some(0.0))
    }
    fn count(&mut self, code: i32) -> usize {
        self.take(code).map_or(0, |n| n.clamp(0.0, 100_000.0) as usize)
    }
    fn point(&mut self, x: i32, y: i32) -> Option<(f64, f64)> {
        Some((self.take(x)?, self.take(y)?))
    }
}

/// Points of a polyline vertex list with bulges: an arc between two points where the bulge is
/// not zero (bulge = tan(sweep / 4), positive counter-clockwise).
fn bulged(vertices: &[((f64, f64), f64)], closed: bool) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let n = vertices.len();
    for i in 0..n {
        let (a, bulge) = vertices[i];
        out.push(a);
        let Some(&(b, _)) = vertices.get(i + 1).or(if closed { vertices.first() } else { None }) else {
            continue;
        };
        if bulge.abs() < 1e-9 {
            continue;
        }
        let chord = (b.0 - a.0).hypot(b.1 - a.1);
        if chord < 1e-12 {
            continue;
        }
        let sweep = 4.0 * bulge.atan();
        let r = chord / (2.0 * (sweep / 2.0).sin()).abs();
        // Centre: from the chord's midpoint along its normal.
        let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let h = r * (sweep / 2.0).cos() * sweep.signum();
        let (nx, ny) = (-(b.1 - a.1) / chord, (b.0 - a.0) / chord);
        let c = (mid.0 + nx * h, mid.1 + ny * h);
        let a0 = (a.1 - c.1).atan2(a.0 - c.0).to_degrees();
        let a1 = (b.1 - c.1).atan2(b.0 - c.0).to_degrees();
        let pts = arc_points(c, r, a0, a1, bulge > 0.0);
        out.extend(pts.iter().skip(1).take(pts.len().saturating_sub(2)));
    }
    out
}

/// The boundary loops of a HATCH, as DXF points. Unreadable loops are left out.
fn hatch_loops(e: &Entity) -> Vec<Vec<(f64, f64)>> {
    let mut cur = Cursor { codes: &e.codes, i: 0 };
    // Skip the elevation point, extrusion, pattern name and flags up to the loop count.
    let Some(start) = e.codes.iter().position(|(c, _)| *c == 91) else {
        return Vec::new();
    };
    cur.i = start;
    let loops = cur.count(91);
    let mut out = Vec::new();
    for _ in 0..loops {
        let Some(flags) = cur.take(92) else { break };
        let mut pts: Vec<(f64, f64)> = Vec::new();
        if (flags as i64) & 2 == 2 {
            let has_bulge = cur.take(72).unwrap_or(0.0) != 0.0;
            let _closed = cur.take(73);
            let n = cur.count(93);
            let mut vs = Vec::with_capacity(n);
            for _ in 0..n {
                let Some(v) = cur.point(10, 20) else { break };
                let bulge = if has_bulge { cur.take(42).unwrap_or(0.0) } else { 0.0 };
                vs.push((v, bulge));
            }
            pts = bulged(&vs, true);
        } else {
            let edges = cur.count(93);
            for edge in 0..edges {
                let Some(kind) = cur.take(72) else { break };
                let seg: Vec<(f64, f64)> = match kind as i64 {
                    1 => match (cur.point(10, 20), cur.point(11, 21)) {
                        (Some(a), Some(b)) => vec![a, b],
                        _ => break,
                    },
                    2 => {
                        let (Some(c), Some(r), Some(a0), Some(a1)) =
                            (cur.point(10, 20), cur.take(40), cur.take(50), cur.take(51))
                        else {
                            break;
                        };
                        // A clockwise edge stores its angles mirrored.
                        if cur.take(73).unwrap_or(1.0) != 0.0 {
                            arc_points(c, r, a0, a1, true)
                        } else {
                            arc_points(c, r, -a0, -a1, false)
                        }
                    }
                    3 => {
                        let (Some(c), Some(m), Some(ratio), Some(a0), Some(a1)) = (
                            cur.point(10, 20),
                            cur.point(11, 21),
                            cur.take(40),
                            cur.take(50),
                            cur.take(51),
                        ) else {
                            break;
                        };
                        // A clockwise edge stores its angles mirrored, as an arc edge does: it runs
                        // clockwise from -a0 to -a1, the same points as counter-clockwise from -a1
                        // to -a0, reversed.
                        if cur.take(73).unwrap_or(1.0) != 0.0 {
                            ellipse_points(c, m, ratio, a0.to_radians(), a1.to_radians()).0
                        } else {
                            let (mut p, _) = ellipse_points(c, m, ratio, (-a1).to_radians(), (-a0).to_radians());
                            p.reverse();
                            p
                        }
                    }
                    4 => {
                        let degree = cur.count(94);
                        let rational = cur.take(73).unwrap_or(0.0) != 0.0;
                        let _periodic = cur.take(74);
                        let nk = cur.count(95);
                        let nc = cur.count(96);
                        let knots: Vec<f64> = (0..nk).filter_map(|_| cur.take(40)).collect();
                        let mut ctrl = Vec::with_capacity(nc);
                        let mut weights = Vec::new();
                        for _ in 0..nc {
                            let Some(v) = cur.point(10, 20) else { break };
                            ctrl.push(v);
                            if rational && let Some(w) = cur.take(42) {
                                weights.push(w);
                            }
                        }
                        // Fit points (R2010 and later): a count, then the points or the tangents.
                        // Older files have no count; a 97 there is the loop's own. Before
                        // another edge a 97 can only be the count (often 0, with nothing after).
                        let next_is_count = matches!(cur.codes.get(cur.i), Some((97, _)));
                        // A zero count is read here too: if it was the loop's own, the loop
                        // has no source objects either, so nothing is lost.
                        let zero = matches!(cur.codes.get(cur.i), Some((97, v)) if v.trim() == "0");
                        let fit_follows = matches!(cur.codes.get(cur.i + 1), Some((11 | 12, _)))
                            || (next_is_count && (zero || edge + 1 < edges));
                        let nf = if fit_follows { cur.count(97) } else { 0 };
                        let fit: Vec<(f64, f64)> = (0..nf).filter_map(|_| cur.point(11, 21)).collect();
                        let _ = (cur.point(12, 22), cur.point(13, 23));
                        bspline_points(&ctrl, &knots, &weights, degree)
                            .or((fit.len() >= 2).then_some(fit))
                            .unwrap_or(ctrl)
                    }
                    _ => break,
                };
                // Edges may be stored either way round: take the end nearer the loop so far
                // first, and do not repeat the shared point.
                let mut seg = seg;
                let d =
                    |a: (f64, f64), b: Option<&(f64, f64)>| b.map_or(f64::INFINITY, |b| (a.0 - b.0).hypot(a.1 - b.1));
                if let Some(&end) = pts.last()
                    && d(end, seg.last()) + 1e-9 < d(end, seg.first())
                {
                    seg.reverse();
                }
                let skip = usize::from(pts.last().is_some_and(|end| d(*end, seg.first()) < 1e-9));
                pts.extend(seg.into_iter().skip(skip));
            }
        }
        // Source boundary objects: a count and that many handles.
        while cur.take(97).is_some() {
            while cur.take(330).is_some() {}
        }
        if pts.len() >= 2
            && pts
                .first()
                .zip(pts.last())
                .is_some_and(|(a, b)| (a.0 - b.0).hypot(a.1 - b.1) < 1e-9)
        {
            pts.pop();
        }
        if pts.len() >= 3 {
            out.push(pts);
        }
    }
    out
}

/// The edges of a loop as point pairs, the last one closing it.
fn loop_edges(lp: &[(f64, f64)]) -> impl Iterator<Item = ((f64, f64), (f64, f64))> + '_ {
    lp.iter().copied().zip(lp.iter().copied().cycle().skip(1))
}

/// Twice the signed area of a loop (positive when counter-clockwise). Taken relative to its first
/// point, so loops far from the origin keep their small area terms.
fn area2(lp: &[(f64, f64)]) -> f64 {
    let Some(&o) = lp.first() else { return 0.0 };
    loop_edges(lp)
        .map(|(a, b)| (a.0 - o.0) * (b.1 - o.1) - (b.0 - o.0) * (a.1 - o.1))
        .sum()
}

/// Whether `pt` lies inside the loop (even-odd rule).
fn inside(pt: (f64, f64), lp: &[(f64, f64)]) -> bool {
    let mut odd = false;
    for (b, a) in loop_edges(lp) {
        if (a.1 > pt.1) != (b.1 > pt.1) && pt.0 < (b.0 - a.0) * (pt.1 - a.1) / (b.1 - a.1) + a.0 {
            odd = !odd;
        }
    }
    odd
}

/// The hatch style (code 75): 0 normal, 1 outer, 2 ignore. Read after the boundary loops, where
/// the code stands; a 75 elsewhere does not occur in a HATCH.
fn hatch_style(e: &Entity) -> i64 {
    e.get(75)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|s| (0..=2).contains(s))
        .unwrap_or(0)
}

/// Whether segments p-q and c-d cross at a point inside both (touching at an end does not count).
fn crosses(p: (f64, f64), q: (f64, f64), c: (f64, f64), d: (f64, f64)) -> bool {
    let side = |a: (f64, f64), b: (f64, f64), x: (f64, f64)| (b.0 - a.0) * (x.1 - a.1) - (b.1 - a.1) * (x.0 - a.0);
    let eps = 1e-12;
    let (d1, d2) = (side(p, q, c), side(p, q, d));
    let (d3, d4) = (side(c, d, p), side(c, d, q));
    ((d1 > eps && d2 < -eps) || (d1 < -eps && d2 > eps)) && ((d3 > eps && d4 < -eps) || (d3 < -eps && d4 > eps))
}

/// The loops of a solid fill as outlines that fill right with the non-zero and even-odd rules
/// alike: every loop inside an odd number of others is a hole, cut into the loop just around it
/// along the shortest bridge, so one outline goes round the solid part and back round the hole.
/// `None` when the loops are too many or too big to work out within the budgets: a fill that
/// covered the holes would be wrong.
fn with_holes(loops: &[Vec<(f64, f64)>], style: i64) -> Option<Vec<Vec<(f64, f64)>>> {
    let mut loops: Vec<Vec<(f64, f64)>> = loops.iter().filter(|l| l.len() >= 3).cloned().collect();
    loops.sort_by(|a, b| area2(b).abs().total_cmp(&area2(a).abs()));
    // For each loop, the smallest loop around it (bigger, so earlier) and its nesting depth.
    let mut parent: Vec<Option<usize>> = Vec::with_capacity(loops.len());
    let mut depth: Vec<usize> = Vec::with_capacity(loops.len());
    let mut work = 0usize;
    for (i, lp) in loops.iter().enumerate() {
        let pt = *lp.first()?;
        let mut found = None;
        for k in (0..i).rev() {
            let other = loops.get(k)?;
            work = work.saturating_add(other.len());
            if work > NESTING_BUDGET {
                return None;
            }
            if inside(pt, other) {
                found = Some(k);
                break;
            }
        }
        depth.push(found.and_then(|k| depth.get(k)).map_or(0, |d| d + 1));
        parent.push(found);
    }
    let all_edges: usize = loops.iter().map(Vec::len).sum();
    // Work spent on bridges so far, over every hole of this hatch.
    let mut bridge_work = 0usize;
    let mut outer: Vec<Option<Vec<(f64, f64)>>> = vec![None; loops.len()];
    for (i, lp) in loops.iter().enumerate() {
        // Outer style keeps the outermost level and its holes; Ignore only the outermost.
        let level = *depth.get(i)?;
        if (style == 1 && level >= 2) || (style == 2 && level >= 1) {
            continue;
        }
        let mut lp = lp.clone();
        if level.is_multiple_of(2) {
            // A solid part: counter-clockwise.
            if area2(&lp) < 0.0 {
                lp.reverse();
            }
            outer[i] = Some(lp);
            continue;
        }
        // A hole: clockwise, cut into the innermost solid part around it.
        if area2(&lp) > 0.0 {
            lp.reverse();
        }
        // The innermost solid part around it.
        let mut host = *parent.get(i)?;
        while let Some(k) = host
            && outer.get(k)?.is_none()
        {
            host = *parent.get(k)?;
        }
        let Some(host) = host else { continue };
        let Some(o) = outer.get_mut(host)?.as_mut() else {
            continue;
        };
        // The shortest bridge from the hole to the outline that crosses no edge of the outline,
        // of this hole or of any other loop.
        // A bounded search: the pairs, then the crossing tests, each within a budget.
        let pair_count = o.len().saturating_mul(lp.len());
        if pair_count > BRIDGE_BUDGET {
            return None;
        }
        bridge_work = bridge_work
            .saturating_add(pair_count)
            .saturating_add(all_edges)
            .saturating_add(o.len());
        if bridge_work > HATCH_BRIDGE_WORK {
            return None;
        }
        let mut pairs: Vec<(f64, usize, usize)> = Vec::with_capacity(o.len() * lp.len());
        for (a, p) in o.iter().enumerate() {
            for (b, h) in lp.iter().enumerate() {
                pairs.push(((p.0 - h.0).hypot(p.1 - h.1), a, b));
            }
        }
        pairs.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut walls: Vec<((f64, f64), (f64, f64))> = loop_edges(o).collect();
        walls.extend(loop_edges(&lp));
        for (k, other) in loops.iter().enumerate() {
            if k != i && k != host {
                walls.extend(loop_edges(other));
            }
        }
        let tries = (BRIDGE_BUDGET * 4 / walls.len().max(1)).max(1);
        let clear = |p: (f64, f64), h: (f64, f64)| !walls.iter().any(|&(c, d)| crosses(p, h, c, d));
        // No clear bridge within the budget: leave the hatch unfilled rather than fill the hole.
        let mut found = None;
        for &(_, a, b) in pairs.iter().take(tries) {
            bridge_work = bridge_work.saturating_add(walls.len());
            if bridge_work > HATCH_BRIDGE_WORK {
                return None;
            }
            if o.get(a).zip(lp.get(b)).is_some_and(|(p, h)| clear(*p, *h)) {
                found = Some((a, b));
                break;
            }
        }
        let (a, b) = found?;
        let mut cut: Vec<(f64, f64)> = o.get(..=a)?.to_vec();
        cut.extend(lp.get(b..)?.iter().chain(lp.get(..=b)?));
        cut.extend(o.get(a..)?);
        *o = cut;
    }
    Some(outer.into_iter().flatten().collect())
}

/// One warning per entity kind that was not read.
fn skipped_warnings(skipped: BTreeMap<String, usize>) -> Vec<String> {
    skipped
        .into_iter()
        .map(|(kind, n)| {
            if kind == OVER_LIMIT {
                format!("{n} entities not drawn: the drawing expands to more than {MAX_DRAWN} entities")
            } else if kind == TOO_DEEP {
                let s = if n == 1 { "y" } else { "ies" };
                format!(
                    "{n} entit{s} not drawn: in blocks nested more than {MAX_NESTING} deep (or inserting themselves)"
                )
            } else if kind == HATCH_NOT_NUMBERS {
                let s = if n == 1 { "" } else { "es" };
                format!("{n} HATCH{s} with a boundary that is not numbers: that boundary left out")
            } else if kind == HATCH_UNFILLED {
                let s = if n == 1 { "" } else { "es" };
                format!("{n} solid HATCH{s} with too many or too complex holes drawn as outline only, not filled")
            } else {
                format!("{n} {kind} entit{} not read", if n == 1 { "y" } else { "ies" })
            }
        })
        .collect()
}

/// MTEXT carries inline formatting codes; keep only the text.
fn mtext_plain(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('P') => out.push(' '),
                Some('\\') => out.push('\\'),
                Some('{') => out.push('{'),
                Some('}') => out.push('}'),
                Some(_) => {
                    // Formatting code: skip to ';' if it takes an argument.
                    let rest: String = chars.clone().take_while(|c| *c != ';' && *c != '\\').collect();
                    if chars.clone().nth(rest.chars().count()) == Some(';') {
                        for _ in 0..=rest.chars().count() {
                            chars.next();
                        }
                    }
                }
                None => {}
            },
            '{' | '}' => {}
            c => out.push(c),
        }
    }
    out
}

fn is_pin_layer(layer: &str) -> bool {
    matches!(layer, "PIN" | "PINS" | "WC_PIN" | "CONNECTION" | "CONNECTIONS")
}

/// Import a symbol drawing. Pins are POINT entities, or circles on a pin layer (`PIN`, `PINS`,
/// `WC_PIN`, `CONNECTION`); a text on the pin layer names the nearest pin. Units are taken as mm.
pub fn symbol(text: &str, id: &str, name: &str) -> Result<wc_core::Symbol> {
    symbol_with_warnings(text, id, name).map(|(s, _)| s)
}

/// Like [`symbol`], with one warning per kind of entity that was not read.
pub fn symbol_with_warnings(text: &str, id: &str, name: &str) -> Result<(wc_core::Symbol, Vec<String>)> {
    let d = parse(text)?;
    let mut raw = Vec::new();
    let mut points = Vec::new();
    let mut skipped = BTreeMap::new();
    shapes(
        &d,
        &d.entities,
        Xf::ID,
        1.0,
        0,
        &|_| false,
        &mut raw,
        &mut points,
        &mut skipped,
        &mut { MAX_DRAWN },
    );
    let mut pins: Vec<(Option<String>, Point)> = points.into_iter().map(|(_, p)| (None, p)).collect();
    let mut labels = Vec::new();
    let mut graphics = Vec::new();
    for (layer, s) in raw {
        match (is_pin_layer(&layer), &s) {
            (true, Shape::Circle { center, .. }) => pins.push((None, *center)),
            (true, Shape::Text { at, text, .. }) => labels.push((*at, text.clone())),
            (true, _) => {}
            (false, _) => graphics.push(s),
        }
    }
    for (at, label) in labels {
        if let Some(pin) = pins
            .iter_mut()
            .filter(|p| p.0.is_none())
            .min_by(|a, b| a.1.distance(at).total_cmp(&b.1.distance(at)))
            && pin.1.distance(at) < 6.0
        {
            pin.0 = Some(label.trim().to_owned());
        }
    }
    // Top-to-bottom, left-to-right, so the first pin (the origin) is the top one.
    pins.sort_by(|a, b| a.1.y.total_cmp(&b.1.y).then(a.1.x.total_cmp(&b.1.x)));
    Ok((make_symbol(id, name, graphics, pins)?, skipped_warnings(skipped)))
}

/// The result of importing a drawing onto a page.
#[derive(Debug, Default)]
pub struct ImportedPage {
    pub graphics: Vec<Shape>,
    pub symbols: Vec<PlacedSymbol>,
    /// One per kind of entity that was not read: `3 3DFACE entities not read`.
    pub warnings: Vec<String>,
}

fn normalise_block_name(n: &str) -> String {
    n.trim().to_lowercase().replace([' ', '/'], "-")
}

/// Import a whole drawing. Block references whose name matches a library symbol id
/// (case-insensitive) become placed symbols, with a device tag from an attribute named TAG,
/// DEVICE, BMK or REF; everything else is kept as graphics. The drawing is moved so its top-left
/// corner sits inside the page frame.
pub fn page(text: &str, scale: f64, known_symbols: &BTreeSet<String>) -> Result<ImportedPage> {
    if !(scale.is_finite() && scale > 0.0) {
        return Err(Error::Format("scale must be a positive number".into()));
    }
    let d = parse(text)?;
    let known: BTreeMap<String, &String> = known_symbols.iter().map(|s| (normalise_block_name(s), s)).collect();
    let mut out = ImportedPage::default();
    let mut raw = Vec::new();
    let mut points = Vec::new();
    let mut plain = Vec::new();
    // Attributes follow their INSERT in the entity list.
    let mut i = 0;
    while i < d.entities.len() {
        let e = &d.entities[i];
        let mut attribs = Vec::new();
        let mut j = i + 1;
        while j < d.entities.len() && d.entities[j].kind == "ATTRIB" {
            attribs.push(&d.entities[j]);
            j += 1;
        }
        let symbol = (e.kind == "INSERT")
            .then(|| e.get(2).and_then(|n| known.get(&normalise_block_name(n))))
            .flatten();
        match symbol {
            Some(id) => {
                let tag = attribs
                    .iter()
                    .find(|a| {
                        matches!(
                            a.get(2).map(str::to_uppercase).as_deref(),
                            Some("TAG" | "DEVICE" | "BMK" | "REF")
                        )
                    })
                    .and_then(|a| a.get(1))
                    .and_then(|v| DeviceTag::parse(v).ok());
                let rotation = Rotation::from_degrees(-(e.f(50).round() as i64)).unwrap_or_default();
                out.symbols.push(PlacedSymbol {
                    id: String::new(),
                    symbol: (*id).clone(),
                    at: Point::new(e.f(10) * scale, -e.f(20) * scale),
                    rotation,
                    mirror: e.f_or(41, 1.0) < 0.0,
                    tag,
                });
            }
            None => {
                plain.push(e.clone());
                plain.extend(attribs.iter().map(|a| (*a).clone()));
            }
        }
        i = j;
    }
    let mut skipped = BTreeMap::new();
    shapes(
        &d,
        &plain,
        Xf::ID,
        scale,
        0,
        &|_| false,
        &mut raw,
        &mut points,
        &mut skipped,
        &mut { MAX_DRAWN },
    );
    out.graphics = raw.into_iter().map(|(_, s)| s).collect();
    out.warnings = skipped_warnings(skipped);
    // Move into the frame: top-left at (20, 20) mm.
    let mut pts: Vec<Point> = out
        .graphics
        .iter()
        .flat_map(|s| {
            let (a, b) = s.bounds();
            [a, b]
        })
        .collect();
    pts.extend(out.symbols.iter().map(|s| s.at));
    if pts.is_empty() {
        return Err(Error::Format("the drawing has no entities WiringCraft can read".into()));
    }
    let (min, _) = wc_core::geom::bounds_of(&pts);
    let shift = Point::new(20.0 - min.x, 20.0 - min.y);
    out.graphics = out.graphics.iter().map(|s| s.translate(shift)).collect();
    for s in &mut out.symbols {
        s.at = s.at + shift;
    }
    Ok(out)
}

/// Degrees in (-180, 180], 0 for anything unusable.
fn normal_angle(a: f64) -> f64 {
    if !a.is_finite() {
        return 0.0;
    }
    let r = a.rem_euclid(360.0);
    if r > 180.0 { r - 360.0 } else { r }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dxf(entities: &str, blocks: &str) -> String {
        format!(
            "0\nSECTION\n2\nHEADER\n0\nENDSEC\n0\nSECTION\n2\nBLOCKS\n{blocks}0\nENDSEC\n0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n"
        )
    }

    #[test]
    fn symbol_with_points_and_labels() -> Result<()> {
        let ents = "0\nLINE\n8\n0\n10\n0\n20\n20\n11\n0\n21\n0\n\
                    0\nPOINT\n8\nPIN\n10\n0\n20\n20\n0\nPOINT\n8\nPIN\n10\n0\n20\n0\n\
                    0\nTEXT\n8\nPIN\n10\n1\n20\n19\n40\n2\n1\nA1\n\
                    0\nCIRCLE\n8\n0\n10\n0\n20\n10\n40\n4\n";
        let s = symbol(&dxf(ents, ""), "my.coil", "Coil")?;
        assert_eq!(s.pins.len(), 2);
        assert_eq!(s.pins[0].name, "A1", "the top pin got the label next to it");
        assert_eq!(s.pins[0].at, Point::new(0.0, 0.0));
        assert_eq!(
            s.pins[1].at,
            Point::new(0.0, 20.0),
            "y flipped: the lower pin is further down"
        );
        assert_eq!(s.graphics.len(), 2);
        Ok(())
    }

    #[test]
    fn page_with_known_block_and_exploded_block() -> Result<()> {
        let blocks = "0\nBLOCK\n2\nWC.COIL\n0\nLINE\n10\n0\n20\n0\n11\n5\n21\n0\n0\nENDBLK\n\
                      0\nBLOCK\n2\nLOGO\n0\nLINE\n10\n0\n20\n0\n11\n1\n21\n0\n0\nLINE\n10\n0\n20\n0\n11\n0\n21\n1\n0\nENDBLK\n";
        let ents = "0\nINSERT\n2\nWC.COIL\n10\n50\n20\n100\n66\n1\n0\nATTRIB\n2\nTAG\n1\n-K7\n\
                    0\nINSERT\n2\nLOGO\n10\n0\n20\n0\n41\n2\n\
                    0\nLWPOLYLINE\n90\n3\n70\n1\n10\n0\n20\n0\n10\n10\n20\n0\n10\n10\n20\n10\n";
        let known: BTreeSet<String> = ["wc.coil".to_string()].into();
        let p = page(&dxf(ents, blocks), 1.0, &known)?;
        assert_eq!(p.symbols.len(), 1);
        assert_eq!(p.symbols[0].tag.as_ref().map(|t| t.to_string()).as_deref(), Some("-K7"));
        assert_eq!(p.graphics.len(), 3, "logo exploded into two lines plus the polyline");
        Ok(())
    }

    #[test]
    fn hostile_input() {
        assert!(symbol("not a dxf", "x", "x").is_err());
        assert!(symbol("AutoCAD Binary DXF\r\n", "x", "x").is_err());
        assert!(symbol(&dxf("", ""), "x", "x").is_err());
        assert!(page(&dxf("", ""), 0.0, &BTreeSet::new()).is_err());
        // A block that inserts itself must not recurse forever.
        let blocks = "0\nBLOCK\n2\nA\n0\nINSERT\n2\nA\n0\nLINE\n10\n0\n20\n0\n11\n1\n21\n1\n0\nENDBLK\n";
        let r = page(&dxf("0\nINSERT\n2\nA\n", blocks), 1.0, &BTreeSet::new());
        assert!(r.is_ok());
        // Truncated pair list and garbage numbers.
        let _ = symbol("0\nSECTION\n2\nENTITIES\n0\nLINE\n10\nNaN\n20", "x", "x");
    }

    #[test]
    fn a_turned_block_inside_a_mirrored_one_is_placed_by_both() -> Result<()> {
        // INNER: a line and a quarter arc, inserted turned by 90 degrees; the whole drawn
        // mirrored in x (#231).
        let blocks = "0\nBLOCK\n2\nINNER\n0\nLINE\n10\n0\n20\n0\n11\n10\n21\n0\n\
                      0\nARC\n10\n0\n20\n0\n40\n2\n50\n0\n51\n90\n0\nENDBLK\n";
        let d = parse(&dxf("0\nINSERT\n2\nINNER\n10\n0\n20\n0\n50\n90\n", blocks))?;
        let mut skipped = BTreeMap::new();
        let mut budget = MAX_DRAWN;
        let (shapes, _) = draw_mapped(&d, &d.entities, [-1.0, 0.0, 0.0, 1.0], 0.0, &mut skipped, &mut budget);
        let near = |a: Point, b: Point| a.distance(b) < 1e-9;
        // (10, 0) turns to (0, 10), stays there when mirrored in x, and is (0, -10) on the page.
        assert!(
            shapes
                .iter()
                .any(|(_, s)| matches!(s, Shape::Line { a, b } if near(*a, Point::new(0.0, 0.0)) && near(*b, Point::new(0.0, -10.0)))),
            "{shapes:?}"
        );
        // The arc 0..90 turns to 90..180, mirrored in x to 0..90: -90..0 on the y-down page.
        assert!(
            shapes.iter().any(|(_, s)| matches!(s, Shape::Arc { start, end, .. }
                if (*start + 90.0).abs() < 1e-9 && end.abs() < 1e-9)),
            "{shapes:?}"
        );
        Ok(())
    }

    #[test]
    fn text_rotation_is_kept() -> Result<()> {
        let ents = "0\nTEXT\n8\n0\n10\n5\n20\n5\n40\n2\n50\n450\n1\nUP\n\
                    0\nTEXT\n8\n0\n10\n5\n20\n9\n40\n2\n50\n270\n1\nDOWN\n\
                    0\nTEXT\n8\n0\n10\n9\n20\n9\n40\n2\n1\nFLAT\n";
        let p = page(&dxf(ents, ""), 1.0, &BTreeSet::new())?;
        let angles: Vec<f64> = p
            .graphics
            .iter()
            .filter_map(|g| {
                if let Shape::Text { angle, .. } = g {
                    Some(*angle)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(angles, [90.0, -90.0, 0.0]);
        assert_eq!(normal_angle(f64::NAN), 0.0);
        Ok(())
    }

    #[test]
    fn mtext_formatting_is_stripped() {
        assert_eq!(mtext_plain(r"{\fArial|b1;Motor}\PM1"), "Motor M1");
    }

    /// Every graphic of a drawing in DXF coordinates turned to page ones (y down, scale 1).
    fn drawn(entities: &str, blocks: &str) -> Result<(Vec<Shape>, BTreeMap<String, usize>)> {
        let d = parse(&dxf(entities, blocks))?;
        let (mut raw, mut points, mut skipped) = (Vec::new(), Vec::new(), BTreeMap::new());
        shapes(
            &d,
            &d.entities,
            Xf::ID,
            1.0,
            0,
            &|_| false,
            &mut raw,
            &mut points,
            &mut skipped,
            &mut { MAX_DRAWN },
        );
        Ok((raw.into_iter().map(|(_, s)| s).collect(), skipped))
    }

    fn poly(s: &Shape) -> (&[Point], bool, bool) {
        match s {
            Shape::Polyline { points, closed, filled } => (points, *closed, *filled),
            _ => (&[], false, false),
        }
    }

    fn near(a: Point, b: Point) -> bool {
        a.distance(b) < 1e-6
    }

    #[test]
    fn full_and_half_ellipses_are_sampled_on_the_curve() -> Result<()> {
        // Centre (10, 10), major axis 5 along x, ratio 0.5: the minor half axis is 2.5.
        let ents = "0\nELLIPSE\n8\n0\n10\n10\n20\n10\n11\n5\n21\n0\n40\n0.5\n41\n0\n42\n6.283185307179586\n\
                    0\nELLIPSE\n8\n0\n10\n10\n20\n10\n11\n5\n21\n0\n40\n0.5\n41\n0\n42\n3.141592653589793\n";
        let (g, skipped) = drawn(ents, "")?;
        assert!(skipped.is_empty());
        assert_eq!(g.len(), 2);
        let on = |q: &Point| (((q.x - 10.0) / 5.0).powi(2) + ((-q.y - 10.0) / 2.5).powi(2) - 1.0).abs() < 1e-9;
        let (full, closed, _) = poly(&g[0]);
        assert!(closed && full.len() > 30 && full.iter().all(on));
        assert!(full.iter().any(|q| near(*q, Point::new(15.0, -10.0))));
        assert!(
            full.iter().any(|q| near(*q, Point::new(10.0, -12.5))),
            "top of the ellipse, y flipped"
        );
        let (half, closed, _) = poly(&g[1]);
        assert!(!closed && half.iter().all(on));
        assert!(near(half[0], Point::new(15.0, -10.0)));
        assert!(near(
            *half.last().unwrap_or(&Point::new(0.0, 0.0)),
            Point::new(5.0, -10.0)
        ));
        assert!(half.iter().all(|q| q.y <= -10.0 + 1e-9), "the upper half only");
        Ok(())
    }

    #[test]
    fn a_rotated_ellipse_and_a_spline_inside_a_block_follow_the_insert() -> Result<()> {
        // Block: ellipse at the origin, major 5 along x, ratio 0.5; a quadratic spline from
        // (0,0) over (10,0) to (10,10). Inserted at (100, 0), turned 90 degrees.
        let blocks = "0\nBLOCK\n2\nB\n\
            0\nELLIPSE\n10\n0\n20\n0\n11\n5\n21\n0\n40\n0.5\n\
            0\nSPLINE\n71\n2\n72\n6\n73\n3\n40\n0\n40\n0\n40\n0\n40\n1\n40\n1\n40\n1\n10\n0\n20\n0\n10\n10\n20\n0\n10\n10\n20\n10\n\
            0\nENDBLK\n";
        let (g, _) = drawn("0\nINSERT\n2\nB\n10\n100\n20\n0\n50\n90\n", blocks)?;
        assert_eq!(g.len(), 2);
        // Turned 90 degrees the major axis runs along y: x within 2.5 of 100, y within 5.
        let (e, closed, _) = poly(&g[0]);
        assert!(closed);
        assert!(
            e.iter()
                .all(|q| (((q.x - 100.0) / 2.5).powi(2) + (q.y / 5.0).powi(2) - 1.0).abs() < 1e-9)
        );
        assert!(e.iter().any(|q| near(*q, Point::new(100.0, -5.0))));
        // The spline: block (x, y) lands on page (100 - y, -x).
        let (s, closed, _) = poly(&g[1]);
        assert!(!closed);
        assert!(near(s[0], Point::new(100.0, 0.0)));
        assert!(near(
            *s.last().unwrap_or(&Point::new(0.0, 0.0)),
            Point::new(90.0, -10.0)
        ));
        // u = 0.5 of the quadratic: (7.5, 2.5) in the block.
        assert!(s.iter().any(|q| near(*q, Point::new(97.5, -7.5))), "{s:?}");
        Ok(())
    }

    #[test]
    fn a_spline_without_knots_goes_through_its_fit_points() -> Result<()> {
        let ents = "0\nSPLINE\n71\n3\n74\n3\n11\n0\n21\n0\n11\n5\n21\n5\n11\n10\n21\n0\n";
        let (g, _) = drawn(ents, "")?;
        let (s, _, _) = poly(&g[0]);
        assert_eq!(s, [Point::new(0.0, 0.0), Point::new(5.0, -5.0), Point::new(10.0, 0.0)]);
        Ok(())
    }

    #[test]
    fn solid_hatches_become_filled_outlines() -> Result<()> {
        // A polyline loop (square) and an edge loop (triangle of lines with one arc edge).
        let ents = "0\nHATCH\n8\n0\n10\n0\n20\n0\n30\n0\n2\nSOLID\n70\n1\n71\n0\n91\n2\n\
            92\n2\n72\n0\n73\n1\n93\n4\n10\n0\n20\n0\n10\n10\n20\n0\n10\n10\n20\n10\n10\n0\n20\n10\n97\n0\n\
            92\n1\n93\n3\n72\n1\n10\n20\n20\n0\n11\n30\n21\n0\n72\n2\n10\n25\n20\n0\n40\n5\n50\n0\n51\n180\n73\n1\n72\n1\n10\n20\n20\n0\n11\n20\n21\n0\n97\n0\n\
            75\n0\n76\n1\n98\n0\n\
            0\nHATCH\n8\n0\n2\nANSI31\n70\n0\n91\n1\n92\n2\n72\n0\n73\n1\n93\n3\n10\n0\n20\n0\n10\n5\n20\n0\n10\n0\n20\n5\n97\n0\n";
        let (g, skipped) = drawn(ents, "")?;
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(g.len(), 3);
        let (sq, closed, filled) = poly(&g[0]);
        assert!(closed && filled);
        assert_eq!(
            sq,
            [
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(10.0, -10.0),
                Point::new(0.0, -10.0)
            ]
        );
        let (half_disc, _, filled) = poly(&g[1]);
        assert!(filled);
        assert!(
            half_disc.iter().any(|q| near(*q, Point::new(25.0, -5.0))),
            "top of the arc edge"
        );
        assert!(half_disc.iter().all(|q| q.x >= 20.0 - 1e-9 && q.x <= 30.0 + 1e-9));
        let (_, closed, filled) = poly(&g[2]);
        assert!(closed && !filled, "a patterned hatch keeps its outline");
        Ok(())
    }

    #[test]
    fn a_bulge_in_a_hatch_loop_is_an_arc() {
        // From (0,0) to (10,0) with bulge 1: a half circle turning counter-clockwise around
        // (5,0), so it passes (5,-5).
        let pts = bulged(&[((0.0, 0.0), 1.0), ((10.0, 0.0), 0.0)], false);
        assert!(
            pts.iter().any(|p| (p.0 - 5.0).abs() < 1e-9 && (p.1 + 5.0).abs() < 1e-9),
            "{pts:?}"
        );
        assert!(pts.iter().all(|p| ((p.0 - 5.0).hypot(p.1) - 5.0).abs() < 1e-9));
    }

    #[test]
    fn entities_not_read_are_counted_per_kind() -> Result<()> {
        let ents = "0\n3DFACE\n8\n0\n0\n3DFACE\n8\n0\n0\nMLINE\n8\n0\n0\nLINE\n10\n0\n20\n0\n11\n1\n21\n0\n";
        let p = page(&dxf(ents, ""), 1.0, &BTreeSet::new())?;
        assert_eq!(p.warnings, ["2 3DFACE entities not read", "1 MLINE entity not read"]);
        Ok(())
    }

    #[test]
    fn a_text_in_a_turned_block_turns_with_it() -> Result<()> {
        let blocks = "0\nBLOCK\n2\nB\n0\nTEXT\n10\n0\n20\n0\n40\n2\n1\nK1\n0\nENDBLK\n";
        let turned = "0\nINSERT\n2\nB\n10\n0\n20\n0\n50\n90\n";
        let (g, _) = drawn(turned, blocks)?;
        assert!(
            matches!(&g[..], [Shape::Text { angle, .. }] if (*angle - 90.0).abs() < 1e-9),
            "{g:?}"
        );
        let (g, _) = drawn("0\nINSERT\n2\nB\n10\n0\n20\n0\n", blocks)?;
        assert!(matches!(&g[..], [Shape::Text { angle, .. }] if *angle == 0.0), "{g:?}");
        Ok(())
    }

    #[test]
    fn every_entity_past_the_budget_is_counted() -> Result<()> {
        let line = "0\nLINE\n10\n0\n20\n0\n11\n1\n21\n0\n";
        let d = parse(&dxf(&line.repeat(5), ""))?;
        let mut skipped = BTreeMap::new();
        let mut budget = 2;
        let (shapes, _) = draw(&d, &d.entities, &mut skipped, &mut budget);
        assert_eq!(shapes.len(), 2);
        assert_eq!(skipped.get(OVER_LIMIT), Some(&3), "{skipped:?}");
        assert_eq!(budget, 0);
        Ok(())
    }

    #[test]
    fn a_solid_is_a_filled_outline() -> Result<()> {
        // A triangle (third and fourth corner the same) and a square, corners in "Z" order.
        let ents = "0\nSOLID\n10\n0\n20\n0\n11\n4\n21\n0\n12\n0\n22\n3\n13\n0\n23\n3\n\
            0\nSOLID\n10\n0\n20\n0\n11\n1\n21\n0\n12\n0\n22\n1\n13\n1\n23\n1\n";
        let (g, skipped) = drawn(ents, "")?;
        assert!(skipped.is_empty());
        assert_eq!(poly(&g[0]).0.len(), 3);
        let (square, closed, filled) = poly(&g[1]);
        assert!(closed && filled);
        assert_eq!(square[2], Point::new(1.0, -1.0), "the fourth corner comes third");
        Ok(())
    }

    #[test]
    fn a_hole_in_a_solid_hatch_stays_open() -> Result<()> {
        // Outer square 0..10 and an island square 4..6, both as polyline loops.
        let ents = "0\nHATCH\n2\nSOLID\n70\n1\n91\n2\n\
            92\n3\n72\n0\n73\n1\n93\n4\n10\n0\n20\n0\n10\n10\n20\n0\n10\n10\n20\n10\n10\n0\n20\n10\n97\n0\n\
            92\n18\n72\n0\n73\n1\n93\n4\n10\n4\n20\n4\n10\n6\n20\n4\n10\n6\n20\n6\n10\n4\n20\n6\n97\n0\n";
        let (g, _) = drawn(ents, "")?;
        assert_eq!(g.len(), 1, "one outline with the hole cut in");
        let (pts, _, filled) = poly(&g[0]);
        assert!(filled);
        let lp: Vec<(f64, f64)> = pts.iter().map(|q| (q.x, -q.y)).collect();
        assert!(
            (area2(&lp).abs() / 2.0 - 96.0).abs() < 1e-9,
            "100 minus the 4 of the hole"
        );
        assert!(!inside((5.0, 5.0), &lp), "the island is not filled");
        assert!(inside((1.0, 1.0), &lp));
        Ok(())
    }

    #[test]
    fn hatch_edges_stored_backwards_and_clockwise_still_make_the_outline() -> Result<()> {
        // Square 0..10 with its second edge stored end to start, and a clockwise ellipse edge
        // (a circle of radius 5 round (5, 10)) from 0 down to -180, stored as 0 and 180.
        let ents = "0\nHATCH\n2\nSOLID\n70\n1\n91\n1\n92\n1\n93\n4\n\
            72\n1\n10\n0\n20\n0\n11\n10\n21\n0\n\
            72\n1\n10\n10\n20\n10\n11\n10\n21\n0\n\
            72\n3\n10\n5\n20\n10\n11\n5\n21\n0\n40\n1\n50\n0\n51\n180\n73\n0\n\
            72\n1\n10\n0\n20\n10\n11\n0\n21\n0\n97\n0\n";
        let (g, _) = drawn(ents, "")?;
        let (pts, _, _) = poly(&g[0]);
        let lp: Vec<(f64, f64)> = pts.iter().map(|q| (q.x, -q.y)).collect();
        // The clockwise half circle from (10,10) to (0,10) dips down to (5,5).
        assert!(
            lp.iter().any(|p| (p.0 - 5.0).abs() < 1e-9 && (p.1 - 5.0).abs() < 1e-9),
            "{lp:?}"
        );
        assert!(
            lp.iter().all(|p| p.1 <= 10.0 + 1e-9),
            "nothing above the square: {lp:?}"
        );
        // No zig-zag: the outline never crosses itself back along the bottom.
        assert!((area2(&lp).abs() / 2.0 - (100.0 - std::f64::consts::PI * 12.5)).abs() < 0.5);
        Ok(())
    }

    #[test]
    fn an_old_spline_edge_without_fit_points_keeps_the_next_loop() -> Result<()> {
        // Loop 1: a straight degree-1 spline edge and a line back; no fit-point count. Loop 2:
        // a triangle. Both must come in.
        let ents = "0\nHATCH\n2\nSOLID\n70\n1\n91\n2\n\
            92\n1\n93\n3\n\
            72\n4\n94\n1\n73\n0\n74\n0\n95\n4\n96\n2\n40\n0\n40\n0\n40\n1\n40\n1\n10\n0\n20\n0\n10\n10\n20\n0\n\
            72\n1\n10\n10\n20\n0\n11\n5\n21\n5\n\
            72\n1\n10\n5\n20\n5\n11\n0\n21\n0\n97\n0\n\
            92\n2\n72\n0\n73\n1\n93\n3\n10\n20\n20\n0\n10\n30\n20\n0\n10\n25\n20\n5\n97\n0\n";
        let (g, _) = drawn(ents, "")?;
        assert_eq!(g.len(), 2, "{g:?}");
        Ok(())
    }

    #[test]
    fn a_mirrored_arc_and_ellipse_land_on_the_other_side() -> Result<()> {
        // Extrusion (0, 0, -1): x of the entity's own coordinates runs the other way.
        let ents = "0\nARC\n10\n10\n20\n0\n40\n5\n50\n0\n51\n90\n230\n-1\n\
                    0\nELLIPSE\n10\n0\n20\n0\n11\n5\n21\n0\n40\n0.5\n41\n0\n42\n1.5707963267948966\n230\n-1\n\
                    0\nVIEWPORT\n";
        let (g, skipped) = drawn(ents, "")?;
        assert!(skipped.is_empty(), "a viewport is no loss: {skipped:?}");
        // The arc's centre is mirrored to (-10, 0); 0..90 becomes 90..180.
        assert_eq!(
            g[0],
            Shape::Arc {
                center: Point::new(-10.0, 0.0),
                radius: 5.0,
                start: -180.0,
                end: -90.0
            }
        );
        // The quarter ellipse runs from (5, 0) to (0, -2.5) instead of (0, 2.5).
        let (e, _, _) = poly(&g[1]);
        assert!(near(e[0], Point::new(5.0, 0.0)));
        assert!(
            near(*e.last().unwrap_or(&Point::new(0.0, 0.0)), Point::new(0.0, 2.5)),
            "{e:?}"
        );
        Ok(())
    }

    fn filled_area(lp: &[(f64, f64)]) -> f64 {
        area2(lp).abs() / 2.0
    }

    #[test]
    fn a_bridge_never_crosses_another_hole_or_the_outline() {
        let sq = |x0: f64, y0: f64, x1: f64, y1: f64| vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
        // A small hole sits between the big hole's corner and the outline's corner.
        let out = with_holes(
            &[sq(0.0, 0.0, 10.0, 10.0), sq(3.0, 3.0, 9.0, 9.0), sq(9.3, 9.3, 9.6, 9.6)],
            0,
        )
        .unwrap_or_default();
        assert_eq!(out.len(), 1);
        let want = 100.0 - 36.0 - 0.09;
        assert!((filled_area(&out[0]) - want).abs() < 1e-9, "{}", filled_area(&out[0]));
        // Every bridge leaves the hole without crossing any edge of the result.
        let lp = &out[0];
        let n = lp.len();
        for i in 0..n {
            for j in (i + 2)..n {
                if i == 0 && j == n - 1 {
                    continue;
                }
                assert!(
                    !crosses(lp[i], lp[(i + 1) % n], lp[j], lp[(j + 1) % n]),
                    "edges {i} and {j} cross"
                );
            }
        }
        // An outline with a slot: the hole must not bridge across the slot.
        let slotted = vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (5.5, 10.0),
            (5.05, 6.0),
            (5.5, 2.0),
            (4.9, 2.0),
            (4.9, 10.0),
            (0.0, 10.0),
        ];
        let out = with_holes(&[slotted.clone(), sq(4.6, 5.9, 4.8, 6.1)], 0).unwrap_or_default();
        let want = filled_area(&slotted) - 0.04;
        assert!((filled_area(&out[0]) - want).abs() < 1e-9);
        let lp = &out[0];
        let n = lp.len();
        for i in 0..n {
            for j in (i + 2)..n {
                if i == 0 && j == n - 1 {
                    continue;
                }
                assert!(
                    !crosses(lp[i], lp[(i + 1) % n], lp[j], lp[(j + 1) % n]),
                    "edges {i} and {j} cross"
                );
            }
        }
    }

    #[test]
    fn a_new_spline_edge_with_no_fit_points_keeps_the_edges_after_it() -> Result<()> {
        // R2010: spline edge with `97 0`, then two line edges closing the triangle.
        let ents = "0\nHATCH\n2\nSOLID\n70\n1\n91\n1\n92\n1\n93\n3\n\
            72\n4\n94\n1\n73\n0\n74\n0\n95\n4\n96\n2\n40\n0\n40\n0\n40\n1\n40\n1\n10\n0\n20\n0\n10\n10\n20\n0\n97\n0\n\
            72\n1\n10\n10\n20\n0\n11\n5\n21\n5\n\
            72\n1\n10\n5\n20\n5\n11\n0\n21\n0\n97\n0\n";
        let (g, _) = drawn(ents, "")?;
        assert_eq!(g.len(), 1);
        let (pts, _, _) = poly(&g[0]);
        assert!(
            pts.iter().any(|q| near(*q, Point::new(5.0, -5.0))),
            "the line edges came in: {pts:?}"
        );
        Ok(())
    }

    #[test]
    fn huge_or_odd_angles_give_a_bounded_curve() -> Result<()> {
        let ents = "0\nELLIPSE\n10\n0\n20\n0\n11\n5\n21\n0\n40\n0.5\n41\n0\n42\n1e300\n\
            0\nELLIPSE\n10\n0\n20\n0\n11\n5\n21\n0\n40\n0.5\n41\n0\n42\n-20\n\
            0\nHATCH\n2\nSOLID\n70\n1\n91\n1\n92\n1\n93\n2\n\
            72\n2\n10\n0\n20\n0\n40\n5\n50\n0\n51\n1e12\n73\n1\n72\n1\n10\n5\n20\n0\n11\n5\n21\n0\n97\n0\n";
        let (g, _) = drawn(ents, "")?;
        for s in &g {
            let (pts, _, _) = poly(s);
            assert!(!pts.is_empty() && pts.len() <= 80, "{}", pts.len());
            assert!(pts.iter().all(|p| p.x.is_finite() && p.y.is_finite()));
        }
        // -20 radians is the same as -20 + 4 turns: about 5.13 rad of sweep, not 4 points.
        let (p, _, _) = poly(&g[1]);
        assert!(p.len() > 50);
        assert_eq!(turn(f64::NAN, 360.0), None);
        assert_eq!(turn(720.0, 360.0), Some(360.0));
        Ok(())
    }

    #[test]
    fn spline_points_that_are_not_numbers_are_left_out() -> Result<()> {
        let ents = "0\nSPLINE\n71\n3\n11\n0\n21\n0\n11\nNaN\n21\n1\n11\n5\n21\n1e999\n11\n10\n21\n0\n";
        let (g, _) = drawn(ents, "")?;
        let (s, _, _) = poly(&g[0]);
        assert_eq!(s, [Point::new(0.0, 0.0), Point::new(10.0, 0.0)]);
        Ok(())
    }

    #[test]
    fn a_clockwise_arc_edge_and_a_counter_clockwise_ellipse_edge() -> Result<()> {
        // Square 0..10 whose top is a clockwise arc edge round (5, 10) radius 5 (stored mirrored:
        // 0 and 180 stand for 0 and -180), and a second loop with a counter-clockwise ellipse
        // edge (half ellipse above the x axis) closed by a line.
        let ents = "0\nHATCH\n2\nSOLID\n70\n1\n91\n2\n\
            92\n1\n93\n4\n72\n1\n10\n0\n20\n0\n11\n10\n21\n0\n72\n1\n10\n10\n20\n0\n11\n10\n21\n10\n\
            72\n2\n10\n5\n20\n10\n40\n5\n50\n0\n51\n180\n73\n0\n72\n1\n10\n0\n20\n10\n11\n0\n21\n0\n97\n0\n\
            92\n1\n93\n2\n72\n3\n10\n30\n20\n0\n11\n5\n21\n0\n40\n0.5\n50\n0\n51\n180\n73\n1\n72\n1\n10\n25\n20\n0\n11\n35\n21\n0\n97\n0\n";
        let (g, _) = drawn(ents, "")?;
        assert_eq!(g.len(), 2);
        let (a, _, _) = poly(&g[0]);
        assert!(
            a.iter().any(|q| near(*q, Point::new(5.0, -5.0))),
            "the clockwise arc dips into the square"
        );
        let (b, _, _) = poly(&g[1]);
        assert!(
            b.iter().any(|q| near(*q, Point::new(30.0, -2.5))),
            "the top of the half ellipse"
        );
        Ok(())
    }

    #[test]
    fn hatch_styles_decide_which_islands_are_filled() {
        let sq = |a: f64, b: f64| vec![(a, a), (b, a), (b, b), (a, b)];
        let loops = || vec![sq(0.0, 10.0), sq(2.0, 8.0), sq(4.0, 6.0)];
        let area = |style| {
            with_holes(&loops(), style)
                .unwrap_or_default()
                .iter()
                .map(|l| area2(l).abs() / 2.0)
                .sum::<f64>()
        };
        // Normal: ring, hole, island again.
        assert!((area(0) - (100.0 - 36.0 + 4.0)).abs() < 1e-9);
        // Outer: the outermost ring only.
        assert!((area(1) - 64.0).abs() < 1e-9);
        // Ignore: the whole outer loop.
        assert!((area(2) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn a_huge_start_angle_still_gives_a_curve() {
        let (pts, closed) = ellipse_points((0.0, 0.0), (5.0, 0.0), 0.5, 1e300, 1e300);
        assert!(closed);
        let xs: Vec<f64> = pts.iter().map(|p| p.0).collect();
        let spread = xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min);
        assert!(spread > 9.0, "a real ellipse, not one point: {spread}");
        let arc = arc_points((0.0, 0.0), 1.0, 1e20, 1e20 + 90.0, true);
        let first = arc[0];
        assert!(arc.iter().any(|p| (p.0 - first.0).hypot(p.1 - first.1) > 1.0));
    }

    #[test]
    fn a_spline_of_huge_degree_is_not_evaluated() {
        let n = 1001;
        let ctrl: Vec<(f64, f64)> = (0..n).map(|i| (i as f64, 0.0)).collect();
        let mut knots = vec![0.0; n];
        knots.extend(vec![1.0; n]);
        let started = std::time::Instant::now();
        assert!(bspline_points(&ctrl, &knots, &[], n - 1).is_none());
        assert!(started.elapsed().as_secs() < 1);
        // A normal degree still works.
        let k3 = [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        assert!(bspline_points(&ctrl[..4], &k3, &[], 3).is_some());
    }

    #[test]
    fn a_hole_too_big_to_search_leaves_the_hatch_unfilled_not_hung_on() {
        let ring = |r: f64, n: usize| -> Vec<(f64, f64)> {
            (0..n)
                .map(|i| {
                    let t = std::f64::consts::TAU * i as f64 / n as f64;
                    (r * t.cos(), r * t.sin())
                })
                .collect()
        };
        let started = std::time::Instant::now();
        let out = with_holes(&[ring(10.0, 3000), ring(5.0, 3000)], 0);
        assert!(out.is_none(), "no fill over the hole");
        assert!(started.elapsed().as_secs() < 5);
    }

    #[test]
    fn many_nested_loops_stop_at_the_budget() {
        // 5000 nested squares: each inside the one before.
        let loops: Vec<Vec<(f64, f64)>> = (0..5000)
            .map(|i| {
                let (a, b) = (i as f64, 20_000.0 - i as f64);
                vec![(a, a), (b, a), (b, b), (a, b)]
            })
            .collect();
        let started = std::time::Instant::now();
        // Nesting found walking out from the smallest loop around: cheap for proper nesting.
        assert!(with_holes(&loops[..40], 1).is_some());
        // Side by side loops need every pair compared; that many stop at the budget.
        let apart: Vec<Vec<(f64, f64)>> = (0..5000)
            .map(|i| {
                let a = i as f64 * 3.0;
                vec![(a, 0.0), (a + 1.0, 0.0), (a + 1.0, 1.0), (a, 1.0)]
            })
            .collect();
        assert!(with_holes(&apart, 0).is_none());
        // Deep nesting with normal style: every other loop is a hole to bridge, and all the
        // bridges together stop at the hatch's budget.
        let deep: Vec<Vec<(f64, f64)>> = (0..20_000)
            .map(|i| {
                let (a, b) = (i as f64, 50_000.0 - i as f64);
                vec![(a, a), (b, a), (b, b), (a, b)]
            })
            .collect();
        assert!(with_holes(&deep, 0).is_none());
        assert!(started.elapsed().as_secs() < 10);
    }

    #[test]
    fn a_hole_far_from_the_origin_keeps_its_winding() {
        let c = f64::from(1u32 << 30);
        let sq = |a: f64, b: f64| vec![(c + a, c + a), (c + b, c + a), (c + b, c + b), (c + a, c + b)];
        assert!((area2(&sq(0.0, 4.0)) - 32.0).abs() < 1e-6);
        let out = with_holes(&[sq(0.0, 4.0), sq(1.0, 2.0)], 0).unwrap_or_default();
        assert_eq!(out.len(), 1);
        assert!((area2(&out[0]) / 2.0 - 15.0).abs() < 1e-6, "{}", area2(&out[0]) / 2.0);
    }

    #[test]
    fn spline_weights_pull_the_curve() {
        // A rational quadratic with these weights is an exact quarter circle.
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let ctrl = [(1.0, 0.0, 1.0), (h, h, h), (0.0, 1.0, 1.0)];
        let knots = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let mid = de_boor(&ctrl, &knots, 2, 0.5).unwrap_or((f64::NAN, f64::NAN));
        assert!((mid.0 - h).abs() < 1e-9 && (mid.1 - h).abs() < 1e-9, "{mid:?}");
        // And bspline_points applies the weights it is given: every sample on the unit circle.
        let pts = bspline_points(&[(1.0, 0.0), (1.0, 1.0), (0.0, 1.0)], &knots, &[1.0, h, 1.0], 2).unwrap_or_default();
        assert!(!pts.is_empty());
        assert!(pts.iter().all(|p| (p.0.hypot(p.1) - 1.0).abs() < 1e-9));
        let plain = bspline_points(&[(1.0, 0.0), (1.0, 1.0), (0.0, 1.0)], &knots, &[], 2).unwrap_or_default();
        assert!(
            plain.iter().any(|p| (p.0.hypot(p.1) - 1.0).abs() > 0.01),
            "unweighted is no circle"
        );
    }

    #[test]
    fn a_block_is_placed_by_its_base_point() -> Result<()> {
        let blocks = "0\nBLOCK\n2\nB\n10\n5\n20\n5\n0\nLINE\n10\n5\n20\n5\n11\n10\n21\n5\n0\nENDBLK\n";
        let (g, _) = drawn("0\nINSERT\n2\nB\n10\n0\n20\n0\n", blocks)?;
        // (0, 0) to (5, 0) in DXF coordinates; page coordinates flip y.
        assert_eq!(
            g,
            [Shape::Line {
                a: Point::new(0.0, 0.0),
                b: Point::new(5.0, 0.0)
            }]
        );
        // Turned and scaled about the base point too: (5, 5) stays on the insertion point.
        let (g, _) = drawn("0\nINSERT\n2\nB\n10\n100\n20\n0\n41\n2\n50\n90\n", blocks)?;
        let placed = |s: &Shape| match s {
            Shape::Line { a, b } => {
                a.distance(Point::new(100.0, 0.0)) < 1e-9 && b.distance(Point::new(100.0, -10.0)) < 1e-9
            }
            _ => false,
        };
        assert!(g.len() == 1 && g.iter().all(placed), "{g:?}");
        Ok(())
    }

    #[test]
    fn a_dimension_is_drawn_by_its_block() -> Result<()> {
        let blocks = "0\nBLOCK\n2\n*D1\n0\nLINE\n10\n0\n20\n0\n11\n0\n21\n5\n0\nENDBLK\n";
        // The second refers to the block in other letters; the third to a missing one.
        let (g, skipped) = drawn(
            "0\nDIMENSION\n2\n*D1\n10\n7\n20\n7\n0\nDIMENSION\n2\n*d1\n0\nDIMENSION\n2\n*D9\n",
            blocks,
        )?;
        let line = Shape::Line {
            a: Point::new(0.0, 0.0),
            b: Point::new(0.0, -5.0),
        };
        assert_eq!(g, [line.clone(), line]);
        assert_eq!(skipped.get("DIMENSION"), Some(&1));
        Ok(())
    }

    #[test]
    fn a_dimension_past_the_nesting_limit_is_counted() -> Result<()> {
        // B0 .. B6 each insert the next; B7, at the deepest level drawn, holds a dimension whose
        // block would be one level deeper. An empty block at the limit counts nothing.
        let mut blocks = String::new();
        for b in 0..7 {
            blocks += &format!("0\nBLOCK\n2\nB{b}\n0\nINSERT\n2\nB{}\n0\nENDBLK\n", b + 1);
        }
        blocks += "0\nBLOCK\n2\nB7\n0\nDIMENSION\n2\n*D1\n0\nINSERT\n2\nEMPTY\n0\nENDBLK\n";
        blocks += "0\nBLOCK\n2\n*D1\n0\nLINE\n10\n0\n20\n0\n11\n0\n21\n5\n0\nENDBLK\n";
        blocks += "0\nBLOCK\n2\nEMPTY\n0\nENDBLK\n";
        let (g, skipped) = drawn("0\nINSERT\n2\nB0\n", &blocks)?;
        assert!(g.is_empty(), "{g:?}");
        assert_eq!(skipped.get(TOO_DEEP), Some(&1), "{skipped:?}");
        assert!(!skipped.contains_key("DIMENSION"), "{skipped:?}");
        let warnings = skipped_warnings(skipped);
        assert_eq!(
            warnings,
            ["1 entity not drawn: in blocks nested more than 8 deep (or inserting themselves)"]
        );
        Ok(())
    }

    #[test]
    fn a_dimension_block_is_placed_by_its_base_point() -> Result<()> {
        let blocks = "0\nBLOCK\n2\n*D2\n10\n5\n20\n5\n0\nLINE\n10\n5\n20\n5\n11\n10\n21\n5\n0\nENDBLK\n";
        let (g, _) = drawn("0\nDIMENSION\n2\n*D2\n", blocks)?;
        assert_eq!(
            g,
            [Shape::Line {
                a: Point::new(0.0, 0.0),
                b: Point::new(5.0, 0.0)
            }]
        );
        Ok(())
    }
}

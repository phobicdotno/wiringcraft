//! ASCII DXF reading: symbols and whole drawings.
//!
//! Supported entities: LINE, CIRCLE, ARC, ELLIPSE, SPLINE, HATCH, LWPOLYLINE, POLYLINE/VERTEX,
//! TEXT, MTEXT, POINT and INSERT (block references, exploded or turned into library symbols).
//! Ellipses and splines become polylines through sampled points; a solid hatch becomes a filled
//! outline, a patterned one its outline. Other entities are counted in one warning per kind.
//! DXF has y pointing up; WiringCraft pages have y pointing down, so y is flipped on the way in.

use std::collections::{BTreeMap, BTreeSet};

use wc_core::geom::{Align, Point, Rotation, Shape};
use wc_core::{DeviceTag, PlacedSymbol};

use crate::{Error, Result, make_symbol};

/// One entity as group-code/value pairs.
#[derive(Debug, Clone, Default)]
struct Entity {
    kind: String,
    codes: Vec<(i32, String)>,
}

impl Entity {
    fn get(&self, code: i32) -> Option<&str> {
        self.codes.iter().find(|(c, _)| *c == code).map(|(_, v)| v.as_str())
    }
    fn f(&self, code: i32) -> f64 {
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

#[derive(Debug, Default)]
struct Drawing {
    entities: Vec<Entity>,
    blocks: BTreeMap<String, Vec<Entity>>,
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
                    block = Some((e.get(2).unwrap_or("").to_owned(), Vec::new()));
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

/// Placement of a block reference.
#[derive(Debug, Clone, Copy)]
struct Xf {
    at: (f64, f64),
    sx: f64,
    sy: f64,
    rot: f64,
}

impl Xf {
    const ID: Xf = Xf {
        at: (0.0, 0.0),
        sx: 1.0,
        sy: 1.0,
        rot: 0.0,
    };
    /// DXF coordinates (y up) to DXF coordinates.
    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let (x, y) = (x * self.sx, y * self.sy);
        let (s, c) = self.rot.to_radians().sin_cos();
        (self.at.0 + x * c - y * s, self.at.1 + x * s + y * c)
    }
    fn then(&self, outer: &Xf) -> Xf {
        let at = outer.apply(self.at.0, self.at.1);
        Xf {
            at,
            sx: self.sx * outer.sx,
            sy: self.sy * outer.sy,
            rot: self.rot + outer.rot,
        }
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
) {
    if depth > 8 {
        return; // block nesting this deep is either a cycle or not worth drawing
    }
    let p = |x: f64, y: f64| {
        let (x, y) = xf.apply(x, y);
        Point::new(x * scale, -y * scale)
    };
    let r_scale = xf.sx.abs() * scale;
    for e in ents {
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
                let (s, en) = (s + xf.rot, en + xf.rot);
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
                let pts: Vec<Point> = xs.iter().zip(&ys).map(|(x, y)| q(*x, *y)).collect();
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
                let solid = e.get(2).is_some_and(|n| n.trim().eq_ignore_ascii_case("SOLID")) || e.f(70) as i64 == 1;
                let loops = hatch_loops(e);
                // Islands inside a solid fill stay open: each is cut into the loop around it.
                let outlines = if solid { with_holes(loops) } else { loops };
                for lp in outlines {
                    if let Some(s) = polyline(lp.into_iter().map(|(x, y)| q(x, y)).collect(), true, solid) {
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
                    // Code 50: rotation in degrees, counter-clockwise.
                    angle: normal_angle(e.f(50)),
                })
            }
            "POINT" => {
                points.push((layer.clone(), p(e.f(10), e.f(20))));
                None
            }
            "INSERT" => {
                if let Some(inner) = e.get(2).and_then(|n| d.blocks.get(n)) {
                    let ixf = Xf {
                        at: (e.f(10), e.f(20)),
                        sx: e.f_or(41, 1.0),
                        sy: e.f_or(42, 1.0),
                        rot: e.f(50),
                    }
                    .then(&xf);
                    shapes(d, inner, ixf, scale, depth + 1, skip_layer, out, points, skipped);
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

/// Points along an ellipse (DXF coordinates): centre, end of the major axis relative to the
/// centre, minor/major ratio and the start and end parameters in radians. A full ellipse comes
/// back closed, without repeating its first point.
fn ellipse_points(c: (f64, f64), major: (f64, f64), ratio: f64, t0: f64, t1: f64) -> (Vec<(f64, f64)>, bool) {
    let minor = (-major.1 * ratio, major.0 * ratio);
    let mut sweep = t1 - t0;
    if sweep <= 0.0 {
        sweep += std::f64::consts::TAU;
    }
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
    let mut sweep = if ccw { a1 - a0 } else { a0 - a1 };
    if sweep <= 0.0 {
        sweep += 360.0;
    }
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
    while k + 1 < n && knots[k + 1] <= u {
        k += 1;
    }
    let mut d: Vec<(f64, f64, f64)> = ctrl.get(k - p..=k)?.to_vec();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let i = j + k - p;
            let denom = knots[i + p + 1 - r] - knots[i];
            let a = if denom.abs() < 1e-12 {
                0.0
            } else {
                (u - knots[i]) / denom
            };
            let (q, s) = (d[j - 1], d[j]);
            d[j] = (
                (1.0 - a) * q.0 + a * s.0,
                (1.0 - a) * q.1 + a * s.1,
                (1.0 - a) * q.2 + a * s.2,
            );
        }
    }
    let (x, y, w) = d[p];
    (w.abs() > 1e-12).then(|| (x / w, y / w))
}

/// Points along a B-spline from its control points, knots and weights, sampled evenly over the
/// knot range. `None` when the data do not make a spline.
fn bspline_points(ctrl: &[(f64, f64)], knots: &[f64], weights: &[f64], degree: usize) -> Option<Vec<(f64, f64)>> {
    let n = ctrl.len();
    if degree == 0 || n <= degree || knots.len() != n + degree + 1 || knots.windows(2).any(|w| w[1] < w[0]) {
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
    let (u0, u1) = (knots[degree], knots[n]);
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
        out.push(de_boor(&h, knots, degree, u)?);
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
    let ctrl: Vec<(f64, f64)> = e.all(10).into_iter().zip(e.all(20)).collect();
    let fit: Vec<(f64, f64)> = e.all(11).into_iter().zip(e.all(21)).collect();
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
                        let fit_follows = matches!(cur.codes.get(cur.i + 1), Some((11 | 12, _)))
                            || (next_is_count && edge + 1 < edges);
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

/// Twice the signed area of a loop (positive when counter-clockwise).
fn area2(lp: &[(f64, f64)]) -> f64 {
    let n = lp.len();
    (0..n)
        .map(|i| {
            let (a, b) = (lp[i], lp[(i + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum()
}

/// Whether `pt` lies inside the loop (even-odd rule).
fn inside(pt: (f64, f64), lp: &[(f64, f64)]) -> bool {
    let mut odd = false;
    let n = lp.len();
    for i in 0..n {
        let (a, b) = (lp[i], lp[(i + n - 1) % n]);
        if (a.1 > pt.1) != (b.1 > pt.1) && pt.0 < (b.0 - a.0) * (pt.1 - a.1) / (b.1 - a.1) + a.0 {
            odd = !odd;
        }
    }
    odd
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
fn with_holes(loops: Vec<Vec<(f64, f64)>>) -> Vec<Vec<(f64, f64)>> {
    let mut loops: Vec<Vec<(f64, f64)>> = loops.into_iter().filter(|l| l.len() >= 3).collect();
    loops.sort_by(|a, b| area2(b).abs().total_cmp(&area2(a).abs()));
    // For each loop, the loops around it (they are all bigger, so earlier).
    let around: Vec<Vec<usize>> = (0..loops.len())
        .map(|i| (0..i).filter(|&k| inside(loops[i][0], &loops[k])).collect())
        .collect();
    let mut outer: Vec<Option<Vec<(f64, f64)>>> = vec![None; loops.len()];
    for i in 0..loops.len() {
        let mut lp = loops[i].clone();
        if around[i].len().is_multiple_of(2) {
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
        let Some(&host) = around[i].iter().rev().find(|&&k| outer[k].is_some()) else {
            continue;
        };
        let Some(o) = outer[host].as_mut() else { continue };
        // The shortest bridge from the hole to the outline that crosses no edge of the outline,
        // of this hole or of any other loop.
        let mut pairs: Vec<(f64, usize, usize)> = Vec::with_capacity(o.len() * lp.len());
        for (a, p) in o.iter().enumerate() {
            for (b, h) in lp.iter().enumerate() {
                pairs.push(((p.0 - h.0).hypot(p.1 - h.1), a, b));
            }
        }
        pairs.sort_by(|x, y| x.0.total_cmp(&y.0));
        let edges_of = |l: &[(f64, f64)]| -> Vec<((f64, f64), (f64, f64))> {
            (0..l.len()).map(|k| (l[k], l[(k + 1) % l.len()])).collect()
        };
        let mut walls = edges_of(o);
        walls.extend(edges_of(&lp));
        for (k, other) in loops.iter().enumerate() {
            if k != i && k != host {
                walls.extend(edges_of(other));
            }
        }
        let clear = |p: (f64, f64), h: (f64, f64)| !walls.iter().any(|&(c, d)| crosses(p, h, c, d));
        let Some(&(_, a, b)) = pairs.iter().find(|(_, a, b)| clear(o[*a], lp[*b])) else {
            continue;
        };
        let mut cut: Vec<(f64, f64)> = o[..=a].to_vec();
        cut.extend(lp[b..].iter().chain(&lp[..=b]));
        cut.extend(&o[a..]);
        *o = cut;
    }
    outer.into_iter().flatten().collect()
}

/// One warning per entity kind that was not read.
fn skipped_warnings(skipped: BTreeMap<String, usize>) -> Vec<String> {
    skipped
        .into_iter()
        .map(|(kind, n)| format!("{n} {kind} entit{} not read", if n == 1 { "y" } else { "ies" }))
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
        let ents = "0\n3DFACE\n8\n0\n0\n3DFACE\n8\n0\n0\nSOLID\n8\n0\n0\nLINE\n10\n0\n20\n0\n11\n1\n21\n0\n";
        let p = page(&dxf(ents, ""), 1.0, &BTreeSet::new())?;
        assert_eq!(p.warnings, ["2 3DFACE entities not read", "1 SOLID entity not read"]);
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
        let out = with_holes(vec![
            sq(0.0, 0.0, 10.0, 10.0),
            sq(3.0, 3.0, 9.0, 9.0),
            sq(9.3, 9.3, 9.6, 9.6),
        ]);
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
        let out = with_holes(vec![slotted.clone(), sq(4.6, 5.9, 4.8, 6.1)]);
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
}

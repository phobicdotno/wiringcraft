//! SVG symbol import. Pins are marked by `id="pin:NAME"`, `data-pin="NAME"` or
//! `class="pin"` (named by order). One SVG user unit is one millimetre unless the root element
//! gives a width in mm together with a viewBox.

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use wc_core::geom::{Align, Point, Shape};

use crate::{Error, Result, make_symbol};

/// 2D affine transform `[a c e; b d f]`.
#[derive(Debug, Clone, Copy)]
struct M([f64; 6]);

impl M {
    const ID: M = M([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    fn mul(&self, o: &M) -> M {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = o.0;
        M([
            a * a2 + c * b2,
            b * a2 + d * b2,
            a * c2 + c * d2,
            b * c2 + d * d2,
            a * e2 + c * f2 + e,
            b * e2 + d * f2 + f,
        ])
    }
    fn p(&self, x: f64, y: f64) -> Point {
        let [a, b, c, d, e, f] = self.0;
        Point::new(a * x + c * y + e, b * x + d * y + f)
    }
    fn scale(&self) -> f64 {
        let [a, b, ..] = self.0;
        (a * a + b * b).sqrt()
    }
    /// Whether a circle stays a circle: x and y are scaled alike and stay at right angles
    /// (turns, mirrors and moves are fine; a `scale(2, 1)` or a skew is not).
    fn keeps_circles(&self) -> bool {
        let [a, b, c, d, ..] = self.0;
        let (x, y) = (a * a + b * b, c * c + d * d);
        let tol = 1e-9 * x.max(y).max(1.0);
        (x - y).abs() <= tol && (a * c + b * d).abs() <= tol
    }
}

fn numbers(s: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<f64>| {
        if let Ok(v) = cur.parse::<f64>()
            && v.is_finite()
        {
            out.push(v);
        }
        cur.clear();
    };
    let mut prev = ' ';
    for c in s.chars() {
        let starts_new = (c == '-' || c == '+') && !(prev == 'e' || prev == 'E') || (c == '.' && cur.contains('.'));
        if c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E' {
            if starts_new && !cur.is_empty() {
                flush(&mut cur, &mut out);
            }
            cur.push(c);
        } else {
            flush(&mut cur, &mut out);
        }
        prev = c;
    }
    flush(&mut cur, &mut out);
    out
}

fn parse_transform(s: &str) -> M {
    let mut m = M::ID;
    for part in s.split(')') {
        let Some((name, args)) = part.split_once('(') else {
            continue;
        };
        let v = numbers(args);
        let g = |i: usize, d: f64| v.get(i).copied().unwrap_or(d);
        let t = match name.trim().trim_start_matches(',').trim() {
            "translate" => M([1.0, 0.0, 0.0, 1.0, g(0, 0.0), g(1, 0.0)]),
            "scale" => M([g(0, 1.0), 0.0, 0.0, g(1, g(0, 1.0)), 0.0, 0.0]),
            "rotate" => {
                let (s, c) = g(0, 0.0).to_radians().sin_cos();
                let (cx, cy) = (g(1, 0.0), g(2, 0.0));
                M([1.0, 0.0, 0.0, 1.0, cx, cy])
                    .mul(&M([c, s, -s, c, 0.0, 0.0]))
                    .mul(&M([1.0, 0.0, 0.0, 1.0, -cx, -cy]))
            }
            "matrix" if v.len() == 6 => M([v[0], v[1], v[2], v[3], v[4], v[5]]),
            _ => M::ID,
        };
        m = m.mul(&t);
    }
    m
}

fn attr(e: &BytesStart, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == name.as_bytes())
        .and_then(|a| a.unescape_value().ok())
        .map(|v| v.into_owned())
}

fn num(e: &BytesStart, name: &str) -> f64 {
    attr(e, name).and_then(|v| numbers(&v).first().copied()).unwrap_or(0.0)
}

/// Path data to polylines. Curves and elliptical arcs are flattened into short straight segments.
/// The arguments of an arc command: groups of seven, whose fourth and fifth (the large-arc and
/// sweep flags) are one `0` or `1` each and may run into the next argument without a separator
/// (`0110 20` is the flags 0 and 1 and the end x 10). Reading stops at the first argument that is
/// not well formed, or at an arc with no arguments or a group cut short, as SVG stops drawing a
/// path at an error: the arguments read before it come back with `false`.
fn arc_numbers(s: &str) -> (Vec<f64>, bool) {
    let b = s.as_bytes();
    let at = |i: usize| b.get(i).copied();
    let digits = |i: &mut usize| {
        let from = *i;
        while at(*i).is_some_and(|c| c.is_ascii_digit()) {
            *i += 1;
        }
        *i > from
    };
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        while at(i).is_some_and(|c| c.is_ascii_whitespace() || c == b',') {
            i += 1;
        }
        let Some(first) = at(i) else { break };
        if matches!(out.len() % 7, 3 | 4) {
            match first {
                b'0' => out.push(0.0),
                b'1' => out.push(1.0),
                _ => return (out, false),
            }
            i += 1;
            continue;
        }
        let start = i;
        if matches!(first, b'+' | b'-') {
            i += 1;
        }
        let whole = digits(&mut i);
        let fraction = at(i) == Some(b'.') && {
            i += 1;
            digits(&mut i)
        };
        if !whole && !fraction {
            return (out, false);
        }
        if matches!(at(i), Some(b'e' | b'E')) {
            let mark = i;
            i += 1;
            if matches!(at(i), Some(b'+' | b'-')) {
                i += 1;
            }
            if !digits(&mut i) {
                i = mark;
            }
        }
        match s.get(start..i).and_then(|t| t.parse::<f64>().ok()) {
            Some(v) if v.is_finite() => out.push(v),
            _ => return (out, false),
        }
    }
    // An arc without arguments, or with a group of seven cut short, is an error too.
    let complete = !out.is_empty() && out.len() % 7 == 0;
    (out, complete)
}

/// A point of path data, before its transform.
type Xy = [f64; 2];

fn path(d: &str, m: &M) -> Vec<(Vec<Point>, bool)> {
    let mut out: Vec<(Vec<Point>, bool)> = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    let (mut x, mut y, mut sx, mut sy) = (0.0, 0.0, 0.0, 0.0);
    let args = |cmd: char, buf: &str| {
        if cmd.eq_ignore_ascii_case(&'A') {
            arc_numbers(buf)
        } else {
            (numbers(buf), true)
        }
    };
    // Tokenise into (command, numbers).
    let mut cmds: Vec<(char, Vec<f64>)> = Vec::new();
    let mut buf = String::new();
    let mut cmd = ' ';
    // An argument error ends the path: what came before it is drawn, nothing after it.
    let mut broken = false;
    for c in d.chars() {
        if c.is_ascii_alphabetic() && c != 'e' && c != 'E' {
            if cmd != ' ' {
                let (v, ok) = args(cmd, &buf);
                cmds.push((cmd, v));
                if !ok {
                    broken = true;
                    break;
                }
            }
            cmd = c;
            buf.clear();
        } else {
            buf.push(c);
        }
    }
    if cmd != ' ' && !broken {
        cmds.push((cmd, args(cmd, &buf).0));
    }
    let flush = |cur: &mut Vec<Point>, out: &mut Vec<(Vec<Point>, bool)>, closed: bool| {
        if cur.len() >= 2 {
            out.push((std::mem::take(cur), closed));
        } else {
            cur.clear();
        }
    };
    // The last cubic and quadratic control points, for `S` and `T` to mirror.
    let mut cubic_ctrl: Option<(f64, f64)> = None;
    let mut quad_ctrl: Option<(f64, f64)> = None;
    for (c, v) in cmds {
        let rel = c.is_ascii_lowercase();
        let (mut next_cubic, mut next_quad) = (None, None);
        let (ox, oy) = if rel { (x, y) } else { (0.0, 0.0) };
        match c.to_ascii_uppercase() {
            'M' => {
                flush(&mut cur, &mut out, false);
                for (i, p) in v.as_chunks::<2>().0.iter().enumerate() {
                    let (nx, ny) = if rel && i > 0 {
                        (x + p[0], y + p[1])
                    } else {
                        (ox + p[0], oy + p[1])
                    };
                    x = nx;
                    y = ny;
                    if i == 0 {
                        sx = x;
                        sy = y;
                    }
                    cur.push(m.p(x, y));
                }
            }
            'L' => {
                for p in v.as_chunks::<2>().0 {
                    let (bx, by) = if rel { (x, y) } else { (0.0, 0.0) };
                    x = bx + p[0];
                    y = by + p[1];
                    cur.push(m.p(x, y));
                }
            }
            'H' => {
                for h in &v {
                    x = if rel { x + h } else { *h };
                    cur.push(m.p(x, y));
                }
            }
            'V' => {
                for h in &v {
                    y = if rel { y + h } else { *h };
                    cur.push(m.p(x, y));
                }
            }
            'C' | 'S' => {
                // (first control point, `None` for `S`; second control point; end point)
                let groups: Vec<(Option<Xy>, Xy, Xy)> = if c.eq_ignore_ascii_case(&'C') {
                    let (groups, _) = v.as_chunks::<6>();
                    groups
                        .iter()
                        .map(|&[a, b, c, d, e, f]| (Some([a, b]), [c, d], [e, f]))
                        .collect()
                } else {
                    let (groups, _) = v.as_chunks::<4>();
                    groups.iter().map(|&[c, d, e, f]| (None, [c, d], [e, f])).collect()
                };
                for (c1, [c2x, c2y], [ex, ey]) in groups {
                    let (bx, by) = if rel { (x, y) } else { (0.0, 0.0) };
                    // `S` takes its first control point mirrored from the last cubic's second.
                    let (c1x, c1y) = match c1 {
                        Some([cx, cy]) => (bx + cx, by + cy),
                        None => reflect(cubic_ctrl, (x, y)),
                    };
                    let (c2x, c2y) = (bx + c2x, by + c2y);
                    let (ex, ey) = (bx + ex, by + ey);
                    let (x0, y0) = (x, y);
                    for k in 1..=CURVE_STEPS {
                        let t = f64::from(k) / f64::from(CURVE_STEPS);
                        let u = 1.0 - t;
                        let px = u * u * u * x0 + 3.0 * u * u * t * c1x + 3.0 * u * t * t * c2x + t * t * t * ex;
                        let py = u * u * u * y0 + 3.0 * u * u * t * c1y + 3.0 * u * t * t * c2y + t * t * t * ey;
                        cur.push(m.p(px, py));
                    }
                    // Chained `S`s mirror each other's control point.
                    cubic_ctrl = Some((c2x, c2y));
                    next_cubic = cubic_ctrl;
                    x = ex;
                    y = ey;
                }
            }
            'Q' | 'T' => {
                // (control point, `None` for `T`; end point)
                let groups: Vec<(Option<Xy>, Xy)> = if c.eq_ignore_ascii_case(&'Q') {
                    let (groups, _) = v.as_chunks::<4>();
                    groups.iter().map(|&[a, b, e, f]| (Some([a, b]), [e, f])).collect()
                } else {
                    let (groups, _) = v.as_chunks::<2>();
                    groups.iter().map(|&[e, f]| (None, [e, f])).collect()
                };
                for (ctrl, [ex, ey]) in groups {
                    let (bx, by) = if rel { (x, y) } else { (0.0, 0.0) };
                    // `T` takes its control point mirrored from the last quadratic's.
                    let (cx, cy) = match ctrl {
                        Some([cx, cy]) => (bx + cx, by + cy),
                        None => reflect(quad_ctrl, (x, y)),
                    };
                    let (ex, ey) = (bx + ex, by + ey);
                    let (x0, y0) = (x, y);
                    for k in 1..=CURVE_STEPS {
                        let t = f64::from(k) / f64::from(CURVE_STEPS);
                        let u = 1.0 - t;
                        cur.push(m.p(
                            u * u * x0 + 2.0 * u * t * cx + t * t * ex,
                            u * u * y0 + 2.0 * u * t * cy + t * t * ey,
                        ));
                    }
                    // Chained `T`s mirror each other's control point.
                    quad_ctrl = Some((cx, cy));
                    next_quad = quad_ctrl;
                    x = ex;
                    y = ey;
                }
            }
            'A' => {
                for &[rx, ry, rotation, large, sweep, ex, ey] in v.as_chunks::<7>().0 {
                    let (bx, by) = if rel { (x, y) } else { (0.0, 0.0) };
                    let (ex, ey) = (bx + ex, by + ey);
                    let arc = Arc {
                        from: (x, y),
                        to: (ex, ey),
                        radii: (rx, ry),
                        rotation,
                        large: large != 0.0,
                        sweep: sweep != 0.0,
                    };
                    for (px, py) in arc.points() {
                        cur.push(m.p(px, py));
                    }
                    x = ex;
                    y = ey;
                }
            }
            'Z' => {
                x = sx;
                y = sy;
                flush(&mut cur, &mut out, true);
                cur.push(m.p(x, y));
            }
            _ => {}
        }
        cubic_ctrl = next_cubic;
        quad_ctrl = next_quad;
    }
    flush(&mut cur, &mut out, false);
    out
}

/// Straight segments per Bezier curve.
const CURVE_STEPS: u32 = 8;
/// Points round an ellipse drawn as a closed polyline.
const ELLIPSE_STEPS: usize = 48;

/// An elliptical arc as SVG path data gives it: from its start to its end point, with its radii,
/// the ellipse's rotation in degrees and the large-arc and sweep flags.
struct Arc {
    from: (f64, f64),
    to: (f64, f64),
    radii: (f64, f64),
    rotation: f64,
    large: bool,
    sweep: bool,
}

impl Arc {
    /// The points after the start, the last one the end point. Radii too small to reach the end
    /// point are scaled up; a zero radius, or values that are not finite, give a straight line
    /// (SVG 1.1, appendix F.6).
    fn points(&self) -> Vec<(f64, f64)> {
        let ((x1, y1), (x2, y2)) = (self.from, self.to);
        let line = vec![(x2, y2)];
        let (mut rx, mut ry) = (self.radii.0.abs(), self.radii.1.abs());
        if (x1 == x2 && y1 == y2) || rx == 0.0 || ry == 0.0 {
            return line;
        }
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let (dx, dy) = ((x1 - x2) / 2.0, (y1 - y2) / 2.0);
        let (x1p, y1p) = (cos * dx + sin * dy, -sin * dx + cos * dy);
        let grow = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
        if grow > 1.0 {
            rx *= grow.sqrt();
            ry *= grow.sqrt();
        }
        let (rx2, ry2) = (rx * rx, ry * ry);
        let den = rx2 * y1p * y1p + ry2 * x1p * x1p;
        // Huge radii overflow to infinity and NaN here, which the clamp below would hide.
        let product = rx2 * ry2;
        let ratio = (product - den) / den;
        if ![product, den, ratio].iter().all(|v| v.is_finite()) {
            return line;
        }
        let root = ratio.max(0.0).sqrt();
        let coef = if self.large == self.sweep { -root } else { root };
        let (cxp, cyp) = (coef * rx * y1p / ry, -coef * ry * x1p / rx);
        let (cx, cy) = (
            cos * cxp - sin * cyp + (x1 + x2) / 2.0,
            sin * cxp + cos * cyp + (y1 + y2) / 2.0,
        );
        let start = ((y1p - cyp) / ry).atan2((x1p - cxp) / rx);
        let end = ((-y1p - cyp) / ry).atan2((-x1p - cxp) / rx);
        let mut delta = end - start;
        if self.sweep && delta < 0.0 {
            delta += std::f64::consts::TAU;
        } else if !self.sweep && delta > 0.0 {
            delta -= std::f64::consts::TAU;
        }
        if ![cx, cy, rx, ry, start, delta].iter().all(|v| v.is_finite()) {
            return line;
        }
        // About one segment per 11.25 degrees.
        let steps = (delta.abs() / (std::f64::consts::PI / 16.0)).ceil().clamp(2.0, 64.0) as u32;
        let mut out: Vec<(f64, f64)> = (1..steps)
            .map(|k| {
                let (s, c) = (start + delta * f64::from(k) / f64::from(steps)).sin_cos();
                (cx + rx * cos * c - ry * sin * s, cy + rx * sin * c + ry * cos * s)
            })
            .collect();
        // End exactly on the given point, not on a rounded one.
        out.push((x2, y2));
        out
    }
}

/// Import an SVG symbol.
pub fn symbol(text: &str, id: &str, name: &str) -> Result<wc_core::Symbol> {
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<M> = vec![M::ID];
    let mut shapes = Vec::new();
    let mut pins: Vec<(Option<String>, Point)> = Vec::new();
    let mut pending_text: Option<(Point, f64, Align)> = None;
    let mut seen_root = false;
    let mut depth_guard = 0usize;
    loop {
        let ev = reader.read_event().map_err(|e| Error::Format(format!("SVG: {e}")))?;
        let (start, empty) = match &ev {
            Event::Start(e) => (Some(e.clone()), false),
            Event::Empty(e) => (Some(e.clone()), true),
            Event::End(_) => {
                if stack.len() > 1 {
                    stack.pop();
                }
                (None, false)
            }
            Event::Text(t) => {
                if let Some((at, h, align)) = pending_text.take() {
                    let s = t.unescape().map(|s| s.trim().to_owned()).unwrap_or_default();
                    if !s.is_empty() {
                        shapes.push(Shape::Text {
                            at,
                            text: s,
                            height: h,
                            align,
                            angle: 0.0,
                        });
                    }
                }
                (None, false)
            }
            Event::Eof => break,
            _ => (None, false),
        };
        let Some(e) = start else { continue };
        depth_guard += 1;
        if depth_guard > 200_000 {
            return Err(Error::Format("SVG too large".into()));
        }
        let parent = stack.last().copied().unwrap_or(M::ID);
        let mut m = attr(&e, "transform")
            .map(|t| parent.mul(&parse_transform(&t)))
            .unwrap_or(parent);
        let tag = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
        if tag == "svg" && !seen_root {
            seen_root = true;
            // width="40mm" with viewBox="0 0 80 160" means 0.5 mm per unit.
            if let (Some(w), Some(vb)) = (attr(&e, "width"), attr(&e, "viewBox")) {
                let vbn = numbers(&vb);
                if w.trim_end().ends_with("mm") && vbn.len() == 4 && vbn[2] > 0.0 {
                    let wmm = numbers(&w).first().copied().unwrap_or(vbn[2]);
                    let s = wmm / vbn[2];
                    m = m.mul(&M([s, 0.0, 0.0, s, -vbn[0] * s, -vbn[1] * s]));
                }
            }
        }
        let pin_name = attr(&e, "data-pin")
            .or_else(|| attr(&e, "id").and_then(|i| i.strip_prefix("pin:").map(str::to_owned)))
            .map(Some)
            .or_else(|| {
                attr(&e, "class")
                    .filter(|c| c.split_whitespace().any(|w| w == "pin"))
                    .map(|_| None)
            });
        let sc = m.scale();
        let shape: Option<Shape> = match tag.as_str() {
            "line" => Some(Shape::Line {
                a: m.p(num(&e, "x1"), num(&e, "y1")),
                b: m.p(num(&e, "x2"), num(&e, "y2")),
            }),
            "rect" => {
                let (x, y, w, h) = (num(&e, "x"), num(&e, "y"), num(&e, "width"), num(&e, "height"));
                Some(Shape::Polyline {
                    points: vec![m.p(x, y), m.p(x + w, y), m.p(x + w, y + h), m.p(x, y + h)],
                    closed: true,
                    filled: false,
                })
            }
            "circle" | "ellipse" => {
                let (rx, ry) = if tag == "circle" {
                    let r = num(&e, "r");
                    (r, r)
                } else {
                    // One radius left out takes the other's value (SVG 2's `auto`).
                    match (
                        attr(&e, "rx").map(|_| num(&e, "rx")),
                        attr(&e, "ry").map(|_| num(&e, "ry")),
                    ) {
                        (Some(rx), Some(ry)) => (rx, ry),
                        (Some(r), None) | (None, Some(r)) => (r, r),
                        (None, None) => (0.0, 0.0),
                    }
                };
                let (cx, cy) = (num(&e, "cx"), num(&e, "cy"));
                let filled = attr(&e, "fill").is_some_and(|f| {
                    f != "none" && f != "transparent" && f != "white" && f != "#fff" && f != "#ffffff"
                });
                if rx == ry && m.keeps_circles() {
                    Some(Shape::Circle {
                        center: m.p(cx, cy),
                        radius: rx * sc,
                        filled,
                    })
                } else {
                    // An ellipse, or a circle the transform stretches: points round it, placed
                    // through the transform.
                    let points = (0..ELLIPSE_STEPS)
                        .map(|i| {
                            let a = std::f64::consts::TAU * i as f64 / ELLIPSE_STEPS as f64;
                            m.p(cx + rx * a.cos(), cy + ry * a.sin())
                        })
                        .collect();
                    Some(Shape::Polyline {
                        points,
                        closed: true,
                        filled,
                    })
                }
            }
            "polyline" | "polygon" => {
                let v = numbers(&attr(&e, "points").unwrap_or_default());
                let pts: Vec<Point> = v.as_chunks::<2>().0.iter().map(|p| m.p(p[0], p[1])).collect();
                (pts.len() >= 2).then(|| Shape::Polyline {
                    points: pts,
                    closed: tag == "polygon",
                    filled: false,
                })
            }
            "path" => {
                let parts = path(&attr(&e, "d").unwrap_or_default(), &m);
                let mut first = None;
                for (pts, closed) in parts {
                    first.get_or_insert(pts[0]);
                    if pin_name.is_none() {
                        shapes.push(Shape::Polyline {
                            points: pts,
                            closed,
                            filled: false,
                        });
                    }
                }
                if let (Some(n), Some(p)) = (pin_name.clone(), first) {
                    pins.push((n, p));
                }
                None
            }
            "text" => {
                let size = attr(&e, "font-size")
                    .and_then(|s| numbers(&s).first().copied())
                    .unwrap_or(3.5);
                let align = match attr(&e, "text-anchor").as_deref() {
                    Some("middle") => Align::Center,
                    Some("end") => Align::Right,
                    _ => Align::Left,
                };
                pending_text = Some((m.p(num(&e, "x"), num(&e, "y")), size * sc * 0.72, align));
                None
            }
            _ => None,
        };
        if let Some(s) = shape {
            match pin_name {
                Some(n) => {
                    let at = match &s {
                        Shape::Circle { center, .. } => *center,
                        Shape::Line { a, .. } => *a,
                        other => {
                            let (a, b) = other.bounds();
                            Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
                        }
                    };
                    pins.push((n, at));
                }
                None => shapes.push(s),
            }
        }
        if !empty {
            stack.push(m);
        }
    }
    if !seen_root {
        return Err(Error::Format("not an SVG file".into()));
    }
    make_symbol(id, name, shapes, pins)
}

/// Mirrors the previous control point `prev` through the current point `cur`, the implied
/// control point of a smooth `S`/`T` segment. Without a previous control point, or when the
/// mirror is too large for an `f64`, the current point itself is the control point (what SVG
/// does with no previous control), so the curve samples stay finite.
fn reflect(prev: Option<(f64, f64)>, cur: (f64, f64)) -> (f64, f64) {
    let (x, y) = cur;
    match prev {
        Some((px, py)) => {
            let (rx, ry) = (x + (x - px), y + (y - py));
            if rx.is_finite() && ry.is_finite() {
                (rx, ry)
            } else {
                cur
            }
        }
        None => cur,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_shapes_and_transforms() -> Result<()> {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="40mm" viewBox="0 0 10 20">
            <g transform="translate(5,0)">
              <line x1="0" y1="0" x2="0" y2="20" stroke="black"/>
              <circle id="pin:A1" cx="0" cy="0" r="0.5"/>
              <circle data-pin="A2" cx="0" cy="20" r="0.5"/>
              <rect x="-2" y="7" width="4" height="6"/>
              <path d="M -3 2 l 6 0 c 1 1 2 2 3 3 Z"/>
              <text x="3" y="10" font-size="4">K</text>
            </g>
        </svg>"#;
        let s = symbol(svg, "svg.coil", "Coil")?;
        assert_eq!(s.pins.len(), 2);
        assert_eq!(s.pins[0].name, "A1");
        assert_eq!(s.pins[1].at, Point::new(0.0, 40.0), "viewBox scaled to mm");
        assert!(
            s.graphics
                .iter()
                .any(|g| matches!(g, Shape::Text { text, .. } if text == "K"))
        );
        assert!(s.graphics.len() >= 4);
        Ok(())
    }

    /// Pin positions by name. Pins are rounded to 0.001 mm, so they compare exactly.
    fn pin_at(s: &wc_core::Symbol, name: &str) -> Point {
        let Some(p) = s.pins.iter().find(|p| p.name == name) else {
            unreachable!("no pin {name}");
        };
        p.at
    }

    fn near(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
    }

    #[test]
    fn polylines_and_polygons() -> Result<()> {
        // The pin at 0,0 keeps the drawing where it is (the first pin becomes the origin).
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg">
            <circle id="pin:1" cx="0" cy="0" r="0.5"/>
            <polyline points="0,0 10,0 10,5"/>
            <polygon points="0 0 4 0 4 4"/>
            <polyline points="3,3"/>
            <polygon points=""/>
        </svg>"#;
        let s = symbol(svg, "svg.poly", "Poly")?;
        assert_eq!(
            s.graphics,
            vec![
                Shape::Polyline {
                    points: vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 5.0)],
                    closed: false,
                    filled: false,
                },
                Shape::Polyline {
                    points: vec![Point::new(0.0, 0.0), Point::new(4.0, 0.0), Point::new(4.0, 4.0)],
                    closed: true,
                    filled: false,
                },
            ],
            "a polyline stays open, a polygon is closed, fewer than 2 points is dropped"
        );
        Ok(())
    }

    #[test]
    fn ellipses_and_filled_circles() -> Result<()> {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg">
            <circle id="pin:1" cx="0" cy="0" r="0.5"/>
            <ellipse cx="10" cy="0" rx="3" ry="3"/>
            <circle cx="20" cy="0" r="1" fill="black"/>
            <circle cx="30" cy="0" r="1" fill="none"/>
            <circle cx="40" cy="0" r="1" fill="white"/>
            <circle cx="50" cy="0" r="1" fill="transparent"/>
            <circle cx="60" cy="0" r="1"/>
            <ellipse cx="70" cy="0" rx="1" ry="1" fill="red"/>
        </svg>"#;
        let s = symbol(svg, "svg.ell", "Ellipse")?;
        let circles: Vec<(f64, f64, bool)> = s
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Circle { center, radius, filled } => Some((center.x, *radius, *filled)),
                _ => None,
            })
            .collect();
        assert_eq!(
            circles,
            vec![
                (10.0, 3.0, false), // an ellipse with equal radii is a circle
                (20.0, 1.0, true),
                (30.0, 1.0, false),
                (40.0, 1.0, false),
                (50.0, 1.0, false),
                (60.0, 1.0, false),
                (70.0, 1.0, true),
            ]
        );
        Ok(())
    }

    /// The bounds of a closed polyline, or `None` for any other shape.
    fn outline_size(g: &Shape) -> Option<(f64, f64, bool)> {
        let Shape::Polyline {
            points,
            closed: true,
            filled,
        } = g
        else {
            return None;
        };
        let (xs, ys): (Vec<f64>, Vec<f64>) = points.iter().map(|p| (p.x, p.y)).unzip();
        let span = |v: &[f64]| v.iter().copied().fold(f64::MIN, f64::max) - v.iter().copied().fold(f64::MAX, f64::min);
        Some((span(&xs), span(&ys), *filled))
    }

    #[test]
    fn flat_ellipses_and_stretched_circles_keep_their_shape() -> Result<()> {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg">
            <circle id="pin:1" cx="0" cy="0" r="0.5"/>
            <ellipse cx="10" cy="0" rx="6" ry="2" fill="black"/>
            <circle cx="30" cy="0" r="2" transform="scale(2 1)"/>
            <ellipse cx="50" cy="0" rx="4"/>
            <circle cx="0" cy="20" r="2" transform="rotate(30) scale(-3 3)"/>
        </svg>"#;
        let s = symbol(svg, "svg.flat", "Flat")?;
        let Some(flat) = s.graphics.iter().find_map(outline_size) else {
            unreachable!("no outline in {:?}", s.graphics);
        };
        let outlines: Vec<(f64, f64, bool)> = s.graphics.iter().filter_map(outline_size).collect();
        assert_eq!(outlines.len(), 2, "{:?}", s.graphics);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(
            close(flat.0, 12.0) && close(flat.1, 4.0) && flat.2,
            "rx 6 ry 2 is 12 x 4, filled: {flat:?}"
        );
        let wide = outlines[1];
        assert!(
            close(wide.0, 8.0) && close(wide.1, 4.0) && !wide.2,
            "scale(2 1) stretches: {wide:?}"
        );
        let circles: Vec<f64> = s
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Circle { radius, .. } => Some(*radius),
                _ => None,
            })
            .collect();
        assert_eq!(circles.len(), 2, "{:?}", s.graphics);
        assert!(close(circles[0], 4.0), "an ellipse with only rx is a circle");
        assert!(close(circles[1], 6.0), "a turn and a mirror keep a circle round");
        Ok(())
    }

    #[test]
    fn pins_by_class_and_on_rect_line_path() -> Result<()> {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg">
            <circle class="pin" cx="0" cy="0" r="0.5"/>
            <rect class="big pin" x="10" y="0" width="4" height="2"/>
            <line data-pin="L" x1="20" y1="1" x2="25" y2="1"/>
            <path id="pin:P" d="M 30 5 L 40 5 L 40 8"/>
            <rect class="pins" x="0" y="10" width="40" height="1"/>
        </svg>"#;
        let s = symbol(svg, "svg.pins", "Pins")?;
        let names: Vec<&str> = s.pins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["1", "2", "L", "P"], "class=\"pin\" pins are named by order");
        assert_eq!(pin_at(&s, "1"), Point::new(0.0, 0.0));
        assert_eq!(pin_at(&s, "2"), Point::new(12.0, 1.0), "a rect pin sits at its centre");
        assert_eq!(
            pin_at(&s, "L"),
            Point::new(20.0, 1.0),
            "a line pin sits at its first point"
        );
        assert_eq!(
            pin_at(&s, "P"),
            Point::new(30.0, 5.0),
            "a path pin sits at its first point"
        );
        // Pin elements are not drawn; class="pins" is not a pin.
        assert_eq!(s.graphics.len(), 1);
        assert!(matches!(&s.graphics[0], Shape::Polyline { points, .. } if points[0] == Point::new(0.0, 10.0)));
        Ok(())
    }

    #[test]
    fn text_anchor_sets_alignment() -> Result<()> {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg">
            <circle id="pin:1" cx="0" cy="0" r="0.5"/>
            <text x="0" y="5" font-size="5">L</text>
            <text x="0" y="10" text-anchor="middle">C</text>
            <text x="0" y="15" text-anchor="end">R</text>
        </svg>"#;
        let s = symbol(svg, "svg.text", "Text")?;
        let texts: Vec<(&str, Align, Point)> = s
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Text { text, align, at, .. } => Some((text.as_str(), *align, *at)),
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                ("L", Align::Left, Point::new(0.0, 5.0)),
                ("C", Align::Center, Point::new(0.0, 10.0)),
                ("R", Align::Right, Point::new(0.0, 15.0)),
            ]
        );
        Ok(())
    }

    #[test]
    fn rotate_scale_and_matrix_transforms() -> Result<()> {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg">
            <circle id="pin:O" cx="0" cy="0" r="0.5"/>
            <circle id="pin:R" cx="10" cy="0" r="0.5" transform="rotate(90)"/>
            <circle id="pin:RC" cx="10" cy="5" r="0.5" transform="rotate(90 5 5)"/>
            <circle id="pin:S" cx="3" cy="4" r="0.5" transform="scale(2)"/>
            <circle id="pin:S2" cx="3" cy="4" r="0.5" transform="scale(2, 3)"/>
            <circle id="pin:M" cx="1" cy="1" r="0.5" transform="matrix(1 0 0 1 7 8)"/>
            <g transform="translate(10,0) rotate(90)">
              <circle id="pin:TR" cx="5" cy="0" r="0.5"/>
            </g>
            <circle cx="20" cy="0" r="1.5" transform="scale(2)"/>
        </svg>"#;
        let s = symbol(svg, "svg.tf", "Transforms")?;
        assert_eq!(pin_at(&s, "R"), Point::new(0.0, 10.0), "rotate(90) turns +x into +y");
        assert_eq!(pin_at(&s, "RC"), Point::new(5.0, 10.0), "rotate about a centre");
        assert_eq!(pin_at(&s, "S"), Point::new(6.0, 8.0));
        assert_eq!(pin_at(&s, "S2"), Point::new(6.0, 12.0));
        assert_eq!(pin_at(&s, "M"), Point::new(8.0, 9.0));
        assert_eq!(pin_at(&s, "TR"), Point::new(10.0, 5.0), "rotate first, then translate");
        let [Shape::Circle { center, radius, .. }] = s.graphics.as_slice() else {
            unreachable!("one drawn circle, got {:?}", s.graphics);
        };
        assert!(near(*center, Point::new(40.0, 0.0)));
        assert!((radius - 3.0).abs() < 1e-9, "scale grows the radius");
        Ok(())
    }

    #[test]
    fn hostile_svgs() {
        assert!(symbol("", "x", "x").is_err());
        assert!(symbol("<html></html>", "x", "x").is_err());
        let _ = symbol("<svg><path d=\"M 1e999 0 L NaN 3 Z z z\"/></svg>", "x", "x");
        assert!(symbol("<svg><line x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"/></svg>", "../evil", "x").is_err());
    }

    /// The points of the one polyline in `d`, untransformed.
    fn one_line(d: &str) -> Vec<Point> {
        let lines = path(d, &M::ID);
        assert_eq!(lines.len(), 1, "{d}: {lines:?}");
        lines.into_iter().next().map(|(pts, _)| pts).unwrap_or_default()
    }

    fn near(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
    }

    #[test]
    fn arcs_follow_their_circle() {
        // A half circle of radius 5 round (5, 0). The sweep flag goes the way of growing angles,
        // which with y pointing down is clockwise on screen: through y < 0.
        for d in ["M 0 0 A 5 5 0 0 1 10 0", "m 0 0 a 5 5 0 0 1 10 0"] {
            let pts = one_line(d);
            assert!(pts.len() > 8, "{d}: flattened, not one segment");
            assert!(
                near(pts[pts.len() - 1], Point::new(10.0, 0.0)),
                "{d}: ends on its end point"
            );
            for p in &pts {
                assert!(((p.x - 5.0).hypot(p.y) - 5.0).abs() < 1e-9, "{d}: {p:?} on the circle");
                assert!(p.y <= 1e-9, "{d}: {p:?} on the sweep side");
            }
            assert!(pts.iter().any(|p| p.y < -4.9), "{d}: reaches the top");
        }
        // The other sweep goes round the other side.
        assert!(one_line("M 0 0 A 5 5 0 0 0 10 0").iter().all(|p| p.y >= -1e-9));
        // The large arc of a circle of radius 10 through two points 10 apart is the long way round.
        let small = one_line("M 0 0 A 10 10 0 0 1 10 0").len();
        let large = one_line("M 0 0 A 10 10 0 1 1 10 0").len();
        assert!(large > small * 3, "{small} {large}");
        // Radii too small for the end point grow until they reach it: again a half circle.
        let pts = one_line("M 0 0 A 1 1 0 0 1 10 0");
        assert!(pts.iter().all(|p| ((p.x - 5.0).hypot(p.y) - 5.0).abs() < 1e-9));
        // A rotated ellipse: radii 10 and 5 turned by 90 degrees run 5 wide and 10 high.
        let pts = one_line("M 0 0 A 10 5 90 0 1 0 20");
        assert!(pts.iter().any(|p| (p.x + 5.0).abs() < 1e-9 || (p.x - 5.0).abs() < 1e-9));
    }

    #[test]
    fn quadratic_and_smooth_curves_are_flattened() {
        // A quadratic from (0, 0) to (10, 0) with its control at (5, 10) peaks at (5, 5).
        let pts = one_line("M 0 0 Q 5 10 10 0");
        assert!(pts.iter().any(|p| near(*p, Point::new(5.0, 5.0))));
        // `T` mirrors the control point: the next hump goes below the line.
        let pts = one_line("M 0 0 Q 5 10 10 0 T 20 0");
        assert!(pts.iter().any(|p| near(*p, Point::new(15.0, -5.0))));
        assert!(near(pts[pts.len() - 1], Point::new(20.0, 0.0)));
        // `S` mirrors the last cubic's second control point.
        let pts = one_line("M 0 0 C 0 10 10 10 10 0 s 10 -10 10 0");
        assert!(pts.iter().any(|p| near(*p, Point::new(15.0, -7.5))), "{pts:?}");
        assert!(near(pts[pts.len() - 1], Point::new(20.0, 0.0)));
        // Without a curve before it, `T` and `S` take the current point as their control.
        let pts = one_line("M 0 0 L 1 0 T 11 0");
        assert!(pts.iter().all(|p| p.y.abs() < 1e-9));
    }

    #[test]
    fn arc_flags_may_run_into_the_next_number() {
        // Flags 0 and 1, then the end point (10, 20); the line goes on from there.
        let pts = one_line("M 0 0 A 10 10 0 0110 20 L 30 20");
        let Some(end) = pts.iter().position(|p| near(*p, Point::new(10.0, 20.0))) else {
            unreachable!("the arc ends on (10, 20): {pts:?}");
        };
        assert!(end > 1, "the arc is flattened");
        assert_eq!(pts[end + 1..], [Point::new(30.0, 20.0)]);
        assert_eq!(
            arc_numbers("10,10 0 1,0-5.5.5 1 1 0 0 1 1e1 1e+1").0,
            vec![
                10.0, 10.0, 0.0, 1.0, 0.0, -5.5, 0.5, 1.0, 1.0, 0.0, 0.0, 1.0, 10.0, 10.0
            ]
        );
        // A flag that is not 0 or 1 ends the arguments, and the path: the line before the arc
        // stays, nothing after it is drawn.
        assert_eq!(arc_numbers("5 5 0 2 1 10 0"), (vec![5.0, 5.0, 0.0], false));
        assert_eq!(
            one_line("M 0 0 L 1 0 A 5 5 0 2 1 10 0 L 20 0"),
            [Point::new(0.0, 0.0), Point::new(1.0, 0.0)]
        );
        // So does an arc cut short or without arguments; whole arcs, also several in a row, go on.
        for d in ["M 0 0 L 1 0 A 5 5 0 0 1 10 L 20 0", "M 0 0 L 1 0 A L 20 0"] {
            assert_eq!(one_line(d), [Point::new(0.0, 0.0), Point::new(1.0, 0.0)], "{d}");
        }
        let pts = one_line("M 0 0 A 5 5 0 0 1 10 0 5 5 0 0 1 20 0 L 30 0");
        assert!(near(pts[pts.len() - 2], Point::new(20.0, 0.0)));
        assert!(near(pts[pts.len() - 1], Point::new(30.0, 0.0)));
    }

    #[test]
    fn hostile_arcs_do_not_panic() {
        for d in [
            "M 0 0 A 0 0 0 0 1 10 0",
            "M 0 0 A 5 5 0 0 1 0 0",
            "M 0 0 A NaN 5 0 0 1 10 0",
            "M 0 0 A 1e308 1e308 1e308 1 1 10 0",
            "M 0 0 A 1e-300 1e-300 0 0 1 1e300 0",
            "M 0 0 A 5 5 0 0 1",
        ] {
            let _ = path(d, &M::ID);
        }
        // A zero radius is a straight line.
        assert_eq!(one_line("M 0 0 A 0 5 0 0 1 10 0").len(), 2);
        assert_eq!(one_line("M 0 0 A 0 0 0 0 1 10 0").len(), 2);
        // Radii whose squares overflow are a straight line too, not a guessed arc.
        assert_eq!(one_line("M 0 0 A 1e308 1e308 0 0 1 10 0").len(), 2);
    }

    #[test]
    fn unrepresentable_smooth_reflections_fall_back_to_the_current_point() {
        // The mirrored control point is beyond f64::MAX; the curve must still stay finite.
        for d in [
            "M 9e307 0 Q -1e308 0 9e307 0 T 0 0",
            "M 9e307 0 C -1e308 0 -1e308 0 9e307 0 S 0 0 0 0",
            "M 0 9e307 Q 0 -1e308 0 9e307 T 0 0",
            "M 0 9e307 C 0 -1e308 0 -1e308 0 9e307 S 0 0 0 0",
        ] {
            let pts = one_line(d);
            assert!(!pts.is_empty(), "{d}");
            assert!(pts.iter().all(|p| p.x.is_finite() && p.y.is_finite()), "{d}: {pts:?}");
        }
    }

    #[test]
    fn number_lists() {
        assert_eq!(numbers("1-2.5.5e1,3"), vec![1.0, -2.5, 0.5e1, 3.0]);
    }

    #[test]
    fn smooth_reflections_near_the_largest_numbers_stay_finite() {
        // Reflecting the control point as 2x - px would overflow here; x + (x - px) does not.
        for (d, ctrl) in [
            ("M 1e308 0 Q 1e308 1 1e308 2 T 1e308 4", Point::new(1e308, 3.0)),
            (
                "M 1e308 0 C 1e308 1 1e308 1 1e308 2 S 1e308 4 1e308 4",
                Point::new(1e308, 3.0),
            ),
        ] {
            let pts = one_line(d);
            assert!(pts.iter().all(|p| p.x.is_finite() && p.y.is_finite()), "{d}: {pts:?}");
            assert!(
                pts.iter().any(|p| p.y > 2.0 && p.y <= 4.0),
                "{d}: reaches {ctrl:?} side"
            );
        }
    }
}

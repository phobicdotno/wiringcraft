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

/// Path data to polylines. Curves are flattened; arcs become straight segments to their end.
fn path(d: &str, m: &M) -> Vec<(Vec<Point>, bool)> {
    let mut out: Vec<(Vec<Point>, bool)> = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    let (mut x, mut y, mut sx, mut sy) = (0.0, 0.0, 0.0, 0.0);
    // Tokenise into (command, numbers).
    let mut cmds: Vec<(char, Vec<f64>)> = Vec::new();
    let mut buf = String::new();
    let mut cmd = ' ';
    for c in d.chars() {
        if c.is_ascii_alphabetic() && c != 'e' && c != 'E' {
            if cmd != ' ' {
                cmds.push((cmd, numbers(&buf)));
            }
            cmd = c;
            buf.clear();
        } else {
            buf.push(c);
        }
    }
    if cmd != ' ' {
        cmds.push((cmd, numbers(&buf)));
    }
    let flush = |cur: &mut Vec<Point>, out: &mut Vec<(Vec<Point>, bool)>, closed: bool| {
        if cur.len() >= 2 {
            out.push((std::mem::take(cur), closed));
        } else {
            cur.clear();
        }
    };
    for (c, v) in cmds {
        let rel = c.is_ascii_lowercase();
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
            'L' | 'T' => {
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
            'C' | 'S' | 'Q' | 'A' => {
                let n = match c.to_ascii_uppercase() {
                    'C' => 6,
                    'S' | 'Q' => 4,
                    _ => 7,
                };
                for seg in v.chunks_exact(n) {
                    let (bx, by) = if rel { (x, y) } else { (0.0, 0.0) };
                    let (ex, ey) = (bx + seg[n - 2], by + seg[n - 1]);
                    if n == 6 {
                        // Cubic Bezier flattened in 8 steps.
                        let (x0, y0) = (x, y);
                        let (c1x, c1y, c2x, c2y) = (bx + seg[0], by + seg[1], bx + seg[2], by + seg[3]);
                        for k in 1..=8 {
                            let t = k as f64 / 8.0;
                            let u = 1.0 - t;
                            let px = u * u * u * x0 + 3.0 * u * u * t * c1x + 3.0 * u * t * t * c2x + t * t * t * ex;
                            let py = u * u * u * y0 + 3.0 * u * u * t * c1y + 3.0 * u * t * t * c2y + t * t * t * ey;
                            cur.push(m.p(px, py));
                        }
                    } else {
                        cur.push(m.p(ex, ey));
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
    }
    flush(&mut cur, &mut out, false);
    out
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
                })
            }
            "circle" | "ellipse" => {
                let r = if tag == "circle" {
                    num(&e, "r")
                } else {
                    (num(&e, "rx") + num(&e, "ry")) / 2.0
                };
                let filled = attr(&e, "fill").is_some_and(|f| {
                    f != "none" && f != "transparent" && f != "white" && f != "#fff" && f != "#ffffff"
                });
                Some(Shape::Circle {
                    center: m.p(num(&e, "cx"), num(&e, "cy")),
                    radius: r * sc,
                    filled,
                })
            }
            "polyline" | "polygon" => {
                let v = numbers(&attr(&e, "points").unwrap_or_default());
                let pts: Vec<Point> = v.as_chunks::<2>().0.iter().map(|p| m.p(p[0], p[1])).collect();
                (pts.len() >= 2).then(|| Shape::Polyline {
                    points: pts,
                    closed: tag == "polygon",
                })
            }
            "path" => {
                let parts = path(&attr(&e, "d").unwrap_or_default(), &m);
                let mut first = None;
                for (pts, closed) in parts {
                    first.get_or_insert(pts[0]);
                    if pin_name.is_none() {
                        shapes.push(Shape::Polyline { points: pts, closed });
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

    #[test]
    fn hostile_svgs() {
        assert!(symbol("", "x", "x").is_err());
        assert!(symbol("<html></html>", "x", "x").is_err());
        let _ = symbol("<svg><path d=\"M 1e999 0 L NaN 3 Z z z\"/></svg>", "x", "x");
        assert!(symbol("<svg><line x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"/></svg>", "../evil", "x").is_err());
    }

    #[test]
    fn number_lists() {
        assert_eq!(numbers("1-2.5.5e1,3"), vec![1.0, -2.5, 0.5e1, 3.0]);
    }
}

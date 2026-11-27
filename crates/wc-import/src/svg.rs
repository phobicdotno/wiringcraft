//! SVG symbol import. Pins are marked by `id="pin:NAME"`, `data-pin="NAME"` or
//! `class="pin"` (named by order). One SVG user unit is one millimetre unless the root element
//! gives a width in an absolute unit (mm, cm, Q, in, pt, pc) together with a viewBox. An absolute
//! height as well fits the viewBox into both by `preserveAspectRatio`, as a browser does.

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
    /// The turn of a text drawn with this transform, in whole degrees from 0 to 360: the
    /// direction of its x axis. Negated, since SVG's y runs down and a text angle turns
    /// counter-clockwise on the page.
    fn text_angle(&self) -> f64 {
        let [a, b, ..] = self.0;
        // `+ 0.0` turns a -0 from an unturned text into 0.
        (-b.atan2(a).to_degrees()).round().rem_euclid(360.0) + 0.0
    }
    fn scale(&self) -> f64 {
        let [a, b, ..] = self.0;
        (a * a + b * b).sqrt()
    }
    /// How long a unit up the page comes out: a text's height, which follows the y axis when x
    /// and y are scaled apart (`preserveAspectRatio="none"`, `scale(2, 1)`).
    fn y_scale(&self) -> f64 {
        let [_, _, c, d, ..] = self.0;
        (c * c + d * d).sqrt()
    }
    /// Whether a circle stays a circle: x and y are scaled alike and stay at right angles
    /// (turns, mirrors and moves are fine; a `scale(2, 1)` or a skew is not).
    fn keeps_circles(&self) -> bool {
        let [a, b, c, d, ..] = self.0;
        let (x, y) = (a * a + b * b, c * c + d * d);
        // Relative to the transform's own scale, so a tiny `scale(0.00001 0.00002)` still counts
        // as stretched. A transform that shrinks to nothing leaves a dot either way.
        let tol = 1e-9 * x.max(y);
        (x - y).abs() <= tol && (a * c + b * d).abs() <= tol
    }
    /// Whether the transform flattens everything onto a line or a point: it can't be undone,
    /// and SVG doesn't draw an element with it. Only a determinant of exactly 0 counts; a very
    /// thin but invertible transform (`scale(1 1e-10)`, `matrix(1 0 1 1e-13 0 0)`) still
    /// draws its thin outline.
    fn is_singular(&self) -> bool {
        let [a, b, c, d, ..] = self.0;
        // a·d − b·c without the rounding of the subtraction (Kahan's way, with fused
        // multiply-adds), so nearly parallel but invertible axes don't come out as exactly 0.
        let w = b * c;
        let err = (-b).mul_add(c, w);
        a.mul_add(d, -w) + err == 0.0
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
            "skewX" => M([1.0, 0.0, g(0, 0.0).to_radians().tan(), 1.0, 0.0, 0.0]),
            "skewY" => M([1.0, g(0, 0.0).to_radians().tan(), 0.0, 1.0, 0.0, 0.0]),
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

/// A presentation value such as `fill`: a property of the same name in `style` wins over the
/// attribute (`style="fill:#000"` over `fill="none"`). Within `style`, an `!important`
/// declaration wins over a normal one, and the last one wins between equals.
fn presentation(e: &BytesStart, name: &str) -> Option<String> {
    attr(e, "style")
        .and_then(|st| {
            let decls: Vec<String> = st
                .split(';')
                .filter_map(|decl| {
                    let (k, v) = decl.split_once(':')?;
                    k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
                })
                .collect();
            decls
                .iter()
                .rev()
                .find(|v| split_important(v).1)
                .or_else(|| decls.last())
                .cloned()
        })
        .or_else(|| attr(e, name))
}

/// Splits a trailing `!important` (any case, with or without spaces around the `!`) off a
/// declaration value: the trimmed value, and whether it was important.
fn split_important(v: &str) -> (&str, bool) {
    let v = v.trim();
    let word = "important";
    let Some(i) = v.len().checked_sub(word.len()) else {
        return (v, false);
    };
    if !v.is_char_boundary(i) || !v[i..].eq_ignore_ascii_case(word) {
        return (v, false);
    }
    match v[..i].trim_end().strip_suffix('!') {
        Some(rest) => (rest.trim_end(), true),
        None => (v, false),
    }
}

/// A `font-size` value in user units: a bare number or `px` as it is, `pt` at 4/3 px each.
/// Other units (`em`, `%`, ...) and keywords are not read.
fn font_size(v: &str) -> Option<f64> {
    let v = v.trim();
    let (n, scale) = if let Some(n) = v.strip_suffix("pt") {
        (n, 4.0 / 3.0)
    } else {
        (v.strip_suffix("px").unwrap_or(v), 1.0)
    };
    let n: f64 = n.trim().parse().ok()?;
    // Zero is a size that was set (a browser draws nothing), so it is kept; a size that is not a
    // number, below zero or too big to scale is not read.
    let px = n * scale;
    (n >= 0.0 && px.is_finite()).then_some(px)
}

/// Elements whose content a browser never draws in place: definitions, markers, clip paths,
/// masks and patterns, and the tooltip and notes elements (`<title>`, `<desc>`, `<metadata>`),
/// whose words never join a label. A `<symbol>` is drawn only through `<use>`, which is not
/// imported.
const NOT_DRAWN: [&str; 9] = [
    "defs", "symbol", "marker", "clipPath", "mask", "pattern", "title", "desc", "metadata",
];

/// Whether an element is drawn at all, and whether it is visible, from its parent's
/// `(drawn, visible)`. `display:none` and the containers in [`NOT_DRAWN`] hide everything under
/// them for good; `visibility` is passed down like a fill, so a child can set `visible` again.
fn shown(e: &BytesStart, tag: &str, (drawn, visible): (bool, bool)) -> (bool, bool) {
    let value = |name| presentation(e, name).map(|v| split_important(&v).0.trim().to_ascii_lowercase());
    let drawn = drawn && !NOT_DRAWN.contains(&tag) && value("display").as_deref() != Some("none");
    let visible = match value("visibility").as_deref() {
        Some("hidden" | "collapse") => false,
        Some("visible") => true,
        _ => visible,
    };
    (drawn, visible)
}

/// Font size and text alignment set on an element or passed down from its parents, as a fill
/// is. `None` is set nowhere above: the size falls back to 3.5, the alignment to the left.
#[derive(Clone, Copy, Default)]
struct Font {
    size: Option<f64>,
    align: Option<Align>,
}

/// The font of an element from its parent's: its own `font-size` and `text-anchor` (from
/// `style` first, then the attribute) win, and a value that is not read (`inherit`, `em`, ...)
/// keeps the parent's.
fn font(e: &BytesStart, parent: Font) -> Font {
    let value = |name| presentation(e, name).map(|v| split_important(&v).0.trim().to_ascii_lowercase());
    let align = match value("text-anchor").as_deref() {
        Some("start") => Some(Align::Left),
        Some("middle") => Some(Align::Center),
        Some("end") => Some(Align::Right),
        _ => parent.align,
    };
    Font {
        size: value("font-size").and_then(|s| font_size(&s)).or(parent.size),
        align,
    }
}

/// Whether a fill paints the shape: not `none`, `transparent` or white (`#fff`, `white`,
/// `rgb(255,255,255)`, `rgb(100%,100%,100%)`). No fill, or `inherit`, takes the parent's answer.
/// A trailing `!important` is ignored.
fn paints(fill: Option<&str>, parent: bool) -> bool {
    let Some(f) = fill else { return parent };
    let f = split_important(f).0.to_ascii_lowercase();
    match f.as_str() {
        "inherit" => parent,
        "" | "none" | "transparent" | "white" | "#fff" | "#ffffff" => false,
        _ => !is_white_rgb(&f),
    }
}

/// Whether a `fill-opacity` (a number or a percentage, a trailing `!important` ignored) is 0,
/// so the fill draws nothing. No value, `inherit` or one that is not read takes the parent's.
fn clear_fill(opacity: Option<&str>, parent: bool) -> bool {
    let Some(v) = opacity else { return parent };
    let v = split_important(v).0;
    let n = match v.strip_suffix('%') {
        Some(p) => p.trim().parse::<f64>().ok(),
        None => v.parse::<f64>().ok(),
    };
    match n {
        Some(n) if !n.is_nan() => n <= 0.0,
        _ => parent,
    }
}

/// `rgb(255,255,255)` or `rgb(100%,100%,100%)`, any spacing, already lower case.
fn is_white_rgb(f: &str) -> bool {
    let Some(inner) = f.strip_prefix("rgb").map(str::trim_start) else {
        return false;
    };
    let Some(inner) = inner.strip_prefix('(').and_then(|r| r.strip_suffix(')')) else {
        return false;
    };
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    parts.len() == 3 && parts.iter().all(|p| matches!(*p, "255" | "100%"))
}

/// A length in an absolute unit (`mm`, `cm`, `Q`, `in`, `pt`, `pc`) in millimetres. A bare
/// number, `px` and relative units give `None`: the user unit then stays one millimetre. Units
/// are matched in any letter case, as CSS reads them (`160q` is `160Q`).
fn length_mm(v: &str) -> Option<f64> {
    let v = v.trim().to_ascii_lowercase();
    let (n, per) = [
        ("mm", 1.0),
        ("cm", 10.0),
        ("q", 0.25),
        ("in", 25.4),
        ("pt", 25.4 / 72.0),
        ("pc", 25.4 / 6.0),
    ]
    .into_iter()
    .find_map(|(unit, per)| v.strip_suffix(unit).map(|n| (n, per)))?;
    let mm = n.trim().parse::<f64>().ok()? * per;
    mm.is_finite().then_some(mm)
}

/// The viewBox `vb` (x, y, width, height) fitted to a root `width` and `height` in millimetres,
/// as a browser does. With no absolute height the width alone sets the scale. With both,
/// `preserveAspectRatio` decides: `meet` (the default) takes the smaller scale, `slice` the
/// larger, and `none` scales each axis on its own. Where the drawing sits inside the box does not
/// matter here: the symbol is moved to its first pin afterwards.
fn viewbox_fit(w: f64, h: Option<f64>, vb: [f64; 4], par: &str) -> M {
    let sx = w / vb[2];
    let (sx, sy) = match h.filter(|h| *h > 0.0 && vb[3] > 0.0).map(|h| h / vb[3]) {
        None => (sx, sx),
        Some(sy) => {
            let par = par.to_ascii_lowercase();
            // `defer` matters only for an image, and is passed over here.
            let mut words = par.split_whitespace().skip_while(|w| *w == "defer");
            match (words.next(), words.next()) {
                (Some("none"), _) => (sx, sy),
                (_, Some("slice")) => (sx.max(sy), sx.max(sy)),
                _ => (sx.min(sy), sx.min(sy)),
            }
        }
    };
    M([sx, 0.0, 0.0, sy, -vb[0] * sx, -vb[1] * sy])
}

fn num(e: &BytesStart, name: &str) -> f64 {
    attr(e, name).and_then(|v| numbers(&v).first().copied()).unwrap_or(0.0)
}

/// Path data to polylines. Curves and elliptical arcs are flattened into short straight segments.
/// The arguments of an arc command: groups of seven, whose fourth and fifth (the large-arc and
/// sweep flags) are one `0` or `1` each and may run into the next argument without a separator
/// (`0110 20` is the flags 0 and 1 and the end x 10); arguments are parted by white space and at
/// most one comma. Reading stops at the first argument that is
/// not well formed, or at an arc with no arguments or a group cut short, as SVG stops drawing a
/// path at an error: the arguments read before it come back with the reason.
fn arc_numbers(s: &str) -> (Vec<f64>, Option<String>) {
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
        // Between arguments: white space with at most one comma.
        let mut commas = 0;
        while let Some(c) = at(i).filter(|c| c.is_ascii_whitespace() || *c == b',') {
            commas += usize::from(c == b',');
            i += 1;
        }
        if commas > 1 {
            let n = out.len() + 1;
            return (out, Some(format!("arc argument {n} comes after more than one comma")));
        }
        let Some(first) = at(i) else { break };
        if matches!(out.len() % 7, 3 | 4) {
            match first {
                b'0' => out.push(0.0),
                b'1' => out.push(1.0),
                _ => {
                    let n = out.len() + 1;
                    return (out, Some(format!("arc argument {n} is a flag that is not 0 or 1")));
                }
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
            let n = out.len() + 1;
            return (out, Some(format!("arc argument {n} is not a number")));
        }
        if matches!(at(i), Some(b'e' | b'E')) {
            i += 1;
            if matches!(at(i), Some(b'+' | b'-')) {
                i += 1;
            }
            // An `e` or sign without digits after it is not a number, as `5e` or `5e+`.
            if !digits(&mut i) {
                let n = out.len() + 1;
                return (out, Some(format!("arc argument {n} is not a number")));
            }
        }
        match s.get(start..i).and_then(|t| t.parse::<f64>().ok()) {
            Some(v) if v.is_finite() => out.push(v),
            _ => {
                let n = out.len() + 1;
                return (out, Some(format!("arc argument {n} is not a finite number")));
            }
        }
    }
    // An arc without arguments, or with a group of seven cut short, is an error too.
    let error = if out.is_empty() {
        Some("an arc has no arguments".to_owned())
    } else if out.len() % 7 != 0 {
        let n = out.len();
        Some(format!("an arc is cut short after argument {n} (an arc takes 7)"))
    } else {
        None
    };
    (out, error)
}

/// The most points in the filled outlines of one `<path>` with several closed sub-paths; a
/// bigger fill would take too long to cut into triangles for the canvas, so it stays outlines.
const MAX_HOLE_FILL_POINTS: usize = 20_000;

/// Whether `pt` lies on an edge of the loop (within a hair).
fn on_edge(pt: (f64, f64), lp: &[(f64, f64)]) -> bool {
    lp.iter().zip(lp.iter().cycle().skip(1)).any(|(&a, &b)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 {
            (((pt.0 - a.0) * dx + (pt.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (cx, cy) = (a.0 + t * dx - pt.0, a.1 + t * dy - pt.1);
        cx * cx + cy * cy <= 1e-18_f64.max(len2 * 1e-20)
    })
}

/// The closed sub-paths of one filled `<path>` as filled outlines with their holes cut in (see
/// [`crate::dxf::with_holes_kept`]), so a ring or a letter O keeps its hole open, and the
/// indices of the sub-paths that are not an edge of the fill (too few points, or inside one
/// that runs the same way under `nonzero`), to be drawn as outlines. With `nonzero` a sub-path
/// inside another one that runs the same way adds to it rather than cutting a hole, so only the
/// outlines where the fill starts or stops are kept; with `evenodd` every nested outline
/// switches it. Nesting is read from a point of each sub-path that is on no other one (a hole
/// may touch the outline around it), so sub-paths that cross each other are taken as nested or
/// apart, not as overlapping. `None` when the sub-paths are too many or too big to work out.
fn solid_parts(rings: &[Vec<Point>], even_odd: bool) -> Option<(Vec<Vec<Point>>, Vec<usize>)> {
    use crate::dxf::{Loop, NESTING_BUDGET, area2, inside, with_holes_kept};
    let mut left_out: Vec<usize> = Vec::new();
    let mut loops: Vec<(usize, Loop)> = Vec::new();
    for (i, r) in rings.iter().enumerate() {
        let l: Loop = r.iter().map(|p| (p.x, p.y)).collect();
        if l.len() >= 3 {
            loops.push((i, l));
        } else {
            left_out.push(i);
        }
    }
    // Start each loop at a point on no other loop, so the nesting tests (here and in
    // `with_holes_kept`, which read the first point) are not thrown by a touching edge.
    let mut work = 0usize;
    let all: Vec<Loop> = loops.iter().map(|(_, l)| l.clone()).collect();
    for (n, (_, lp)) in loops.iter_mut().enumerate() {
        let mut start = None;
        for (v, &pt) in lp.iter().enumerate() {
            let mut clear = true;
            for (k, other) in all.iter().enumerate() {
                if k == n {
                    continue;
                }
                work = work.saturating_add(other.len());
                if work > NESTING_BUDGET {
                    return None;
                }
                if on_edge(pt, other) {
                    clear = false;
                    break;
                }
            }
            if clear {
                start = Some(v);
                break;
            }
        }
        lp.rotate_left(start?);
    }
    if !even_odd {
        // Biggest first, so the loop around another comes before it.
        loops.sort_by(|a, b| area2(&b.1).abs().total_cmp(&area2(&a.1).abs()));
        // The winding number inside each loop (but outside the loops in it): its own turn plus
        // those of every loop around it.
        let mut winding: Vec<i64> = Vec::with_capacity(loops.len());
        let mut keep: Vec<bool> = Vec::with_capacity(loops.len());
        for (i, (_, lp)) in loops.iter().enumerate() {
            let pt = *lp.first()?;
            let mut around = None;
            for k in (0..i).rev() {
                let other = &loops.get(k)?.1;
                work = work.saturating_add(other.len());
                if work > NESTING_BUDGET {
                    return None;
                }
                if inside(pt, other) {
                    around = Some(k);
                    break;
                }
            }
            let turn = if area2(lp) >= 0.0 { 1 } else { -1 };
            let outside = around.and_then(|k| winding.get(k).copied()).unwrap_or(0);
            let w = outside + turn;
            // An outline is an edge of the fill only where the fill starts or stops there.
            keep.push((w != 0) != (outside != 0));
            winding.push(w);
        }
        let mut kept = keep.into_iter();
        loops.retain(|(i, _)| {
            let k = kept.next().unwrap_or(false);
            if !k {
                left_out.push(*i);
            }
            k
        });
    }
    let only: Vec<Loop> = loops.iter().map(|(_, l)| l.clone()).collect();
    let (solid, _) = with_holes_kept(&only, 0)?;
    if solid.iter().map(Vec::len).sum::<usize>() > MAX_HOLE_FILL_POINTS {
        return None;
    }
    left_out.sort_unstable();
    Some((
        solid
            .into_iter()
            .map(|l| l.into_iter().map(|(x, y)| Point::new(x, y)).collect())
            .collect(),
        left_out,
    ))
}

/// A point of path data, before its transform.
type Xy = [f64; 2];

/// The polylines of path data, and why drawing stopped early, if it did: the reason and the
/// number of the command in the path, counted from 1.
fn path(d: &str, m: &M) -> (Vec<(Vec<Point>, bool)>, Option<String>) {
    let mut out: Vec<(Vec<Point>, bool)> = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    let (mut x, mut y, mut sx, mut sy) = (0.0, 0.0, 0.0, 0.0);
    let args = |cmd: char, buf: &str| {
        if cmd.eq_ignore_ascii_case(&'A') {
            arc_numbers(buf)
        } else {
            (numbers(buf), None)
        }
    };
    // Tokenise into (command, numbers).
    let mut cmds: Vec<(char, Vec<f64>)> = Vec::new();
    let mut buf = String::new();
    let mut cmd = ' ';
    // An argument error ends the path: what came before it is drawn, nothing after it.
    let mut stopped: Option<String> = None;
    let stop_at = |n: usize, cmd: char, why: String| format!("stopped at command {n} ({cmd}): {why}");
    for c in d.chars() {
        // In an arc, a letter that is no command stays with the arguments, so that the arc
        // reports the argument it spoils.
        let in_arc = cmd.eq_ignore_ascii_case(&'A') && !"MmZzLlHhVvCcSsQqTtAa".contains(c);
        if c.is_ascii_alphabetic() && c != 'e' && c != 'E' && !in_arc {
            if cmd != ' ' {
                let (v, error) = args(cmd, &buf);
                cmds.push((cmd, v));
                if let Some(why) = error {
                    stopped = Some(stop_at(cmds.len(), cmd, why));
                    break;
                }
            }
            cmd = c;
            buf.clear();
        } else {
            buf.push(c);
        }
    }
    if cmd != ' ' && stopped.is_none() {
        let (v, error) = args(cmd, &buf);
        cmds.push((cmd, v));
        stopped = error.map(|why| stop_at(cmds.len(), cmd, why));
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
    (out, stopped)
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
    /// The points after the start, the last one the end point; none when the end point is the
    /// start. Radii too small to reach the end point are scaled up; a zero radius, or values that
    /// are not finite, give a straight line (SVG 1.1, appendix F.6).
    fn points(&self) -> Vec<(f64, f64)> {
        let ((x1, y1), (x2, y2)) = (self.from, self.to);
        let line = vec![(x2, y2)];
        let (mut rx, mut ry) = (self.radii.0.abs(), self.radii.1.abs());
        // Coinciding end points draw nothing (as if the arc were left out).
        if x1 == x2 && y1 == y2 {
            return Vec::new();
        }
        if rx == 0.0 || ry == 0.0 {
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
    symbol_with_warnings(text, id, name).map(|(s, _)| s)
}

/// Like [`symbol`], with one warning per path that stopped drawing at bad data, naming the path
/// and why it stopped.
pub fn symbol_with_warnings(text: &str, id: &str, name: &str) -> Result<(wc_core::Symbol, Vec<String>)> {
    let mut warnings = Vec::new();
    let mut path_count = 0usize;
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<M> = vec![M::ID];
    // Whether each open element's fill paints, passed to its children; kept in step with `stack`.
    // A bool, not the fill text, so deep nesting under a long fill value keeps no copies of it.
    let mut fills: Vec<bool> = vec![false];
    // Whether each open element's `fill-opacity` is 0, passed to its children; kept in step with
    // `stack`. Kept apart from `fills`, so a child that sets the opacity again fills in its
    // parent's colour.
    let mut clear_fills: Vec<bool> = vec![false];
    // Whether each open element's `fill-rule` is `evenodd` (else `nonzero`, the default),
    // passed to its children; kept in step with `stack`.
    let mut even_odd_rules: Vec<bool> = vec![false];
    // `(drawn, visible)` of each open element (see [`shown`]); kept in step with `stack`.
    let mut shows: Vec<(bool, bool)> = vec![(true, true)];
    // The font of each open element (see [`font`]); kept in step with `stack`.
    let mut fonts: Vec<Font> = vec![Font::default()];
    let mut shapes = Vec::new();
    let mut pins: Vec<(Option<String>, Point)> = Vec::new();
    // Where a `<text>` still waiting for its words starts, and its angle.
    let mut pending_text: Option<(Point, f64)> = None;
    // The open `<text>`'s own `x` and `y`, for a `<tspan>` that sets only one of them.
    let mut text_xy: Option<(f64, f64)> = None;
    // The label the open `<text>` is writing into (an index in `shapes`), and whether a space
    // comes before its next words.
    let mut label: Option<(usize, bool)> = None;
    let mut seen_root = false;
    let mut depth_guard = 0usize;
    loop {
        let ev = reader.read_event().map_err(|e| Error::Format(format!("SVG: {e}")))?;
        let (start, empty) = match &ev {
            Event::Start(e) => (Some(e.clone()), false),
            Event::Empty(e) => (Some(e.clone()), true),
            Event::End(e) => {
                // A `<text>` with no words in it must not hand its place to later text. A
                // `<text>` that is not drawn (one in another namespace inside `<metadata>`, say)
                // never took the state, so its end leaves the open `<text>`'s state alone.
                if e.local_name().as_ref() == b"text" && shows.last().is_none_or(|s| s.0) {
                    pending_text = None;
                    text_xy = None;
                    label = None;
                }
                if stack.len() > 1 {
                    stack.pop();
                    fills.pop();
                    clear_fills.pop();
                    even_odd_rules.pop();
                    shows.pop();
                    fonts.pop();
                }
                (None, false)
            }
            Event::Text(t) => {
                // Only text in a `<text>` is read; other text is ignored. A hidden run
                // (a `<tspan>` with `display="none"` or `visibility="hidden"`) is skipped and
                // leaves the label waiting for a visible one. The first run's own element (the
                // `<text>` or a `<tspan>` in it) gives the size and alignment; later runs on the
                // same line (`K<tspan>1</tspan>`) are added to its words, with one space where
                // the file has white space between them.
                let shown_run = shows.last().copied().unwrap_or((true, true)) == (true, true);
                if (pending_text.is_some() || label.is_some()) && shown_run {
                    let raw = t.unescape().map_err(|e| Error::Format(format!("SVG: {e}")))?;
                    let s = raw.trim().to_owned();
                    let space_after = raw.ends_with(char::is_whitespace);
                    if let Some((i, space)) = label
                        && pending_text.is_none()
                    {
                        if s.is_empty() {
                            label = Some((i, space || !raw.is_empty()));
                        } else if let Some(Shape::Text { text, .. }) = shapes.get_mut(i) {
                            if space || raw.starts_with(char::is_whitespace) {
                                text.push(' ');
                            }
                            text.push_str(&s);
                            label = Some((i, space_after));
                        }
                    } else if let (Some((at, angle)), false) = (pending_text, s.is_empty()) {
                        // Skip the line breaks and indents between `<text>` and a `<tspan>`.
                        pending_text = None;
                        label = Some((shapes.len(), space_after));
                        let f = fonts.last().copied().unwrap_or_default();
                        let sc = stack.last().copied().unwrap_or(M::ID).y_scale();
                        shapes.push(Shape::Text {
                            at,
                            text: s,
                            height: f.size.unwrap_or(3.5) * sc * 0.72,
                            align: f.align.unwrap_or(Align::Left),
                            angle,
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
        let paint = paints(
            presentation(&e, "fill").as_deref(),
            fills.last().copied().unwrap_or(false),
        );
        let clear = clear_fill(
            presentation(&e, "fill-opacity").as_deref(),
            clear_fills.last().copied().unwrap_or(false),
        );
        // A fill with opacity 0 draws nothing; the outline stays.
        let filled = paint && !clear;
        let parent_even_odd = even_odd_rules.last().copied().unwrap_or(false);
        let even_odd = match presentation(&e, "fill-rule").map(|v| split_important(&v).0.to_ascii_lowercase()) {
            Some(v) if v == "evenodd" => true,
            Some(v) if v == "nonzero" => false,
            _ => parent_even_odd,
        };
        let tag = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
        if tag == "svg" && !seen_root {
            seen_root = true;
            // width="40mm" with viewBox="0 0 80 160" means 0.5 mm per unit.
            if let (Some(w), Some(vb)) = (attr(&e, "width"), attr(&e, "viewBox")) {
                let vbn = numbers(&vb);
                if let (Some(wmm), 4) = (length_mm(&w), vbn.len())
                    && vbn[2] > 0.0
                {
                    let hmm = attr(&e, "height").and_then(|h| length_mm(&h));
                    let par = attr(&e, "preserveAspectRatio").unwrap_or_default();
                    let fit = viewbox_fit(wmm, hmm, [vbn[0], vbn[1], vbn[2], vbn[3]], &par);
                    if fit.0.iter().any(|v| !v.is_finite()) {
                        let h = attr(&e, "height").unwrap_or_default();
                        return Err(Error::Format(format!(
                            "SVG: width=\"{w}\" height=\"{h}\" viewBox=\"{vb}\" give a size that cannot be drawn"
                        )));
                    }
                    m = m.mul(&fit);
                }
            }
        }
        let show = shown(&e, &tag, shows.last().copied().unwrap_or((true, true)));
        // Every `<path>` counts, hidden or not, so a warning names its place in the file.
        if tag == "path" {
            path_count += 1;
        }
        let fnt = font(&e, fonts.last().copied().unwrap_or_default());
        // An invisible `<text>` or `<tspan>` still sets up its label and place: a `<tspan>` in
        // it can be visible again, and each run of words is checked on its own.
        if !show.0 || (!show.1 && tag != "text" && tag != "tspan") {
            // Hidden: no shapes, pins or text, but its children still need its state.
            if !empty {
                stack.push(m);
                fills.push(paint);
                clear_fills.push(clear);
                even_odd_rules.push(even_odd);
                shows.push(show);
                fonts.push(fnt);
            }
            continue;
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
                    filled,
                })
            }
            "circle" | "ellipse" => {
                let (rx, ry) = if tag == "circle" {
                    let r = num(&e, "r");
                    (r, r)
                } else {
                    // A radius left out or written `auto` takes the other's value (SVG 2).
                    let radius = |name: &str| {
                        attr(&e, name)
                            .filter(|v| !v.trim().eq_ignore_ascii_case("auto"))
                            .map(|_| num(&e, name))
                    };
                    match (radius("rx"), radius("ry")) {
                        (Some(rx), Some(ry)) => (rx, ry),
                        (Some(r), None) | (None, Some(r)) => (r, r),
                        (None, None) => (0.0, 0.0),
                    }
                };
                let (cx, cy) = (num(&e, "cx"), num(&e, "cy"));
                if m.is_singular() && pin_name.is_none() && !(rx == ry && m.keeps_circles()) {
                    // Squashed flat: SVG draws nothing, and an outline would only stretch
                    // the symbol's bounds. (`scale(0)` still keeps the radius-0 dot it had,
                    // and a pin keeps its place.)
                    None
                } else if rx == ry && m.keeps_circles() {
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
                // A filled polyline is closed, as SVG fills it; two points stay a plain line.
                let filled = filled && pts.len() >= 3;
                (pts.len() >= 2).then(|| Shape::Polyline {
                    points: pts,
                    closed: tag == "polygon" || filled,
                    filled,
                })
            }
            "path" => {
                let (parts, stopped) = path(&attr(&e, "d").unwrap_or_default(), &m);
                if let Some(why) = stopped {
                    let which = match attr(&e, "id") {
                        Some(i) => {
                            // The id is the file's text: control characters (an ESC that would
                            // drive a terminal) are written as escapes.
                            let i: String = i
                                .chars()
                                .map(|c| {
                                    if c.is_control() {
                                        c.escape_default().to_string()
                                    } else {
                                        c.to_string()
                                    }
                                })
                                .collect();
                            format!("path {path_count} (id \"{i}\")")
                        }
                        None => format!("path {path_count}"),
                    };
                    warnings.push(format!("{which} is drawn only in part: {why}"));
                }
                let first = parts.first().and_then(|(pts, _)| pts.first().copied());
                // Shapes have no holes or fill rule, so a `d` with several closed sub-paths
                // (a ring, a letter O) has its holes cut into the solid parts around them
                // (see [`solid_parts`]). An open sub-path stays a line.
                let rings = parts.iter().filter(|(_, closed)| *closed).count();
                let solid = if filled && rings > 1 && pin_name.is_none() {
                    let closed: Vec<Vec<Point>> = parts
                        .iter()
                        .filter(|(_, closed)| *closed)
                        .map(|(pts, _)| pts.clone())
                        .collect();
                    let solid = solid_parts(&closed, even_odd);
                    if solid.is_none() {
                        warnings.push(format!(
                            "path {path_count} is drawn as outlines: its holes are too many or too big to fill"
                        ));
                    }
                    solid
                } else {
                    None
                };
                if pin_name.is_none() {
                    let mut ring = 0usize;
                    for (pts, closed) in parts {
                        if closed {
                            ring += 1;
                            // A closed sub-path in the fill is drawn by it; the others (too few
                            // points, or not an edge of the fill) keep their outline.
                            if let Some((_, left_out)) = &solid
                                && left_out.binary_search(&(ring - 1)).is_err()
                            {
                                continue;
                            }
                        }
                        shapes.push(Shape::Polyline {
                            points: pts,
                            closed,
                            filled: filled && closed && rings <= 1,
                        });
                    }
                    for pts in solid.map(|(fill, _)| fill).unwrap_or_default() {
                        shapes.push(Shape::Polyline {
                            points: pts,
                            closed: true,
                            filled: true,
                        });
                    }
                }
                if let (Some(n), Some(p)) = (pin_name.clone(), first) {
                    pins.push((n, p));
                }
                None
            }
            "text" => {
                // The size and alignment are read when its words come (see `fonts`).
                if !empty {
                    let xy = (num(&e, "x"), num(&e, "y"));
                    pending_text = Some((m.p(xy.0, xy.1), m.text_angle()));
                    text_xy = Some(xy);
                    label = None;
                }
                None
            }
            // A `<tspan>` placed with its own `x` or `y` (a new line, as Inkscape writes one)
            // starts a label of its own there; the other coordinate is the text's.
            "tspan" => {
                if let Some((tx, ty)) = text_xy
                    && (attr(&e, "x").is_some() || attr(&e, "y").is_some())
                {
                    let x = attr(&e, "x").map_or(tx, |_| num(&e, "x"));
                    let y = attr(&e, "y").map_or(ty, |_| num(&e, "y"));
                    pending_text = Some((m.p(x, y), m.text_angle()));
                    label = None;
                }
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
            fills.push(paint);
            clear_fills.push(clear);
            even_odd_rules.push(even_odd);
            shows.push(show);
            fonts.push(fnt);
        }
    }
    if !seen_root {
        return Err(Error::Format("not an SVG file".into()));
    }
    let empty = shapes.is_empty() && pins.is_empty();
    match make_symbol(id, name, shapes, pins) {
        Ok(s) => Ok((s, warnings)),
        // Nothing drawn because a path stopped at bad data: say why.
        Err(Error::Format(m)) if empty && !warnings.is_empty() => {
            Err(Error::Format(format!("{m}: {}", warnings.join("; "))))
        }
        Err(err) => Err(err),
    }
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

    #[test]
    fn viewbox_width_in_other_absolute_units_is_converted_to_mm() -> Result<()> {
        // Each width is 40 mm over a viewBox 80 wide: 0.5 mm per unit.
        for w in [
            "40mm",
            "4cm",
            "40mm ",
            "160Q",
            "160q",
            "4CM",
            "113.38582677165354pt",
            "9.448818897637796pc",
        ] {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" viewBox="10 0 80 160">
                    <circle id="pin:1" cx="10" cy="0" r="1"/><circle id="pin:2" cx="90" cy="160" r="1"/>
                </svg>"#
            );
            let s = symbol(&svg, "svg.u", "U")?;
            let (a, b) = (pin_at(&s, "1"), pin_at(&s, "2"));
            assert!(
                ((b.x - a.x) - 40.0).abs() < 1e-3 && ((b.y - a.y) - 80.0).abs() < 1e-3,
                "{w}: {a:?} {b:?}"
            );
        }
        // One inch over 100 units: 0.254 mm per unit.
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1in" viewBox="0 0 100 100">
            <circle id="pin:1" cx="0" cy="0" r="1"/><circle id="pin:2" cx="100" cy="0" r="1"/>
        </svg>"#;
        let s = symbol(svg, "svg.u", "U")?;
        assert!(((pin_at(&s, "2").x - pin_at(&s, "1").x) - 25.4).abs() < 1e-3);
        // No unit, or px, keeps one unit per millimetre.
        for w in ["40", "40px"] {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" viewBox="0 0 80 160">
                    <circle id="pin:1" cx="0" cy="0" r="1"/><circle id="pin:2" cx="80" cy="0" r="1"/>
                </svg>"#
            );
            let s = symbol(&svg, "svg.u", "U")?;
            assert!(((pin_at(&s, "2").x - pin_at(&s, "1").x) - 80.0).abs() < 1e-3, "{w}");
        }
        Ok(())
    }

    #[test]
    fn viewbox_fits_an_absolute_width_and_height_by_preserve_aspect_ratio() -> Result<()> {
        // A 40 x 40 mm box over a viewBox 80 x 160: the height limits it to 0.25 mm per unit,
        // so the drawing is 20 x 40 mm, whatever the alignment.
        let fit = |par: &str| -> Result<(Point, Point)> {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="40mm" viewBox="10 0 80 160" {par}>
                    <circle id="pin:1" cx="10" cy="0" r="1"/><circle id="pin:2" cx="90" cy="160" r="1"/>
                </svg>"#
            );
            let s = symbol(&svg, "svg.u", "U")?;
            Ok((pin_at(&s, "1"), pin_at(&s, "2")))
        };
        let size = |(a, b): (Point, Point)| (b.x - a.x, b.y - a.y);
        let close = |(x, y): (f64, f64), (ex, ey): (f64, f64)| (x - ex).abs() < 1e-3 && (y - ey).abs() < 1e-3;
        for par in [
            "",
            r#"preserveAspectRatio="xMidYMid meet""#,
            r#"preserveAspectRatio="xMinYMax""#,
        ] {
            let got = fit(par)?;
            assert!(close(size(got), (20.0, 40.0)), "{par}: {got:?}");
        }
        // slice takes the larger scale, none scales each axis on its own.
        let got = fit(r#"preserveAspectRatio="xMidYMid slice""#)?;
        assert!(close(size(got), (40.0, 80.0)), "{got:?}");
        let got = fit(r#"preserveAspectRatio="none""#)?;
        assert!(close(size(got), (40.0, 40.0)), "{got:?}");
        // `defer` in front is passed over.
        let got = fit(r#"preserveAspectRatio="defer xMidYMid slice""#)?;
        assert!(close(size(got), (40.0, 80.0)), "{got:?}");
        // A viewBox so thin that its scale runs to infinity is an error, not a symbol of NaN.
        let thin = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="40mm" viewBox="0 0 80 1e-308" preserveAspectRatio="none">
            <line x1="0" y1="0" x2="80" y2="0"/></svg>"#;
        assert!(symbol(thin, "svg.u", "U").is_err());
        // With none, a text is as high as the y axis makes it: 10 units at 0.25 mm.
        let text = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="40mm" viewBox="0 0 80 160" preserveAspectRatio="none">
            <line x1="0" y1="0" x2="80" y2="160"/><text x="0" y="20" font-size="10">K</text></svg>"#;
        let s = symbol(text, "svg.u", "U")?;
        let heights: Vec<f64> = s
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Text { height, .. } => Some(*height),
                _ => None,
            })
            .collect();
        assert!(heights.len() == 1 && (heights[0] - 1.8).abs() < 1e-9, "{heights:?}");
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
            <ellipse cx="0" cy="40" rx="auto" ry="5"/>
            <ellipse cx="0" cy="60" rx="1.5" ry=" AUTO "/>
            <circle cx="0" cy="0" r="1000000" transform="translate(0 80) scale(0.00001 0.00002)"/>
        </svg>"#;
        let s = symbol(svg, "svg.flat", "Flat")?;
        let Some(flat) = s.graphics.iter().find_map(outline_size) else {
            unreachable!("no outline in {:?}", s.graphics);
        };
        let outlines: Vec<(f64, f64, bool)> = s.graphics.iter().filter_map(outline_size).collect();
        assert_eq!(outlines.len(), 3, "{:?}", s.graphics);
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
        let tiny = outlines[2];
        let near_eq = |a: f64, b: f64| (a - b).abs() < 1e-6;
        assert!(
            near_eq(tiny.0, 20.0) && near_eq(tiny.1, 40.0),
            "a tiny stretch still stretches: {tiny:?}"
        );
        let circles: Vec<f64> = s
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Circle { radius, .. } => Some(*radius),
                _ => None,
            })
            .collect();
        assert_eq!(circles.len(), 4, "{:?}", s.graphics);
        assert!(close(circles[0], 4.0), "an ellipse with only rx is a circle");
        assert!(close(circles[1], 6.0), "a turn and a mirror keep a circle round");
        assert!(close(circles[2], 5.0), "rx=\"auto\" takes ry");
        assert!(close(circles[3], 1.5), "ry=\"auto\" takes rx");
        Ok(())
    }

    #[test]
    fn circles_squashed_flat_are_left_out() -> Result<()> {
        let plain = r#"<svg xmlns="http://www.w3.org/2000/svg">
            <circle id="pin:1" cx="0" cy="0" r="0.5"/>
            <line x1="0" y1="0" x2="4" y2="0"/>
        </svg>"#;
        let want = symbol(plain, "svg.flat", "Flat")?;
        for t in ["scale(0 1)", "scale(1 0)", "matrix(1 2 2 4 0 0)"] {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg">
                <circle id="pin:1" cx="0" cy="0" r="0.5"/>
                <line x1="0" y1="0" x2="4" y2="0"/>
                <circle cx="0" cy="20" r="2" transform="{t}"/>
                <ellipse cx="0" cy="20" rx="2" ry="5" transform="{t}"/>
            </svg>"#
            );
            let s = symbol(&svg, "svg.flat", "Flat")?;
            assert_eq!(
                format!("{s:?}"),
                format!("{want:?}"),
                "{t}: no outline, and the same bounds"
            );
        }
        // A pin squashed flat still marks where the pin is.
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg">
            <circle id="pin:1" cx="0" cy="0" r="0.5" transform="translate(3 4) scale(0 1)"/>
            <line x1="0" y1="0" x2="4" y2="0"/>
        </svg>"#;
        let s = symbol(svg, "svg.flatpin", "Flat pin")?;
        assert_eq!(s.pins.len(), 1, "{:?}", s.pins);
        // Very thin, or nearly parallel axes, but still invertible: drawn as a flat outline.
        for t in [
            "scale(1 1e-10)",
            "matrix(1 0 1 1e-13 0 0)",
            "matrix(1 1.0000000000000002 0.9999999999999998 1 0 0)",
        ] {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg">
                <circle id="pin:1" cx="0" cy="0" r="0.5"/>
                <circle cx="0" cy="0" r="2" transform="translate(0 20) {t}"/>
            </svg>"#
            );
            let s = symbol(&svg, "svg.thin", "Thin")?;
            let Some(thin) = s.graphics.iter().find_map(outline_size) else {
                unreachable!("{t} is not singular: {:?}", s.graphics);
            };
            assert!(thin.0.max(thin.1) >= 4.0 - 1e-9, "{t}: {thin:?}");
        }
        Ok(())
    }

    #[test]
    fn skewed_circles_come_in_as_outlines() -> Result<()> {
        // skewX(45) moves each point right by its height: the same as matrix(1 0 1 1 0 0).
        // matrix(1 0 0.6 0.8 0 0) keeps both axes 1 long but no longer at right angles.
        for (skew, [a, b, c, d]) in [
            ("skewX(45)", [1.0, 0.0, 1.0, 1.0]),
            ("matrix(1 0 1 1 0 0)", [1.0, 0.0, 1.0, 1.0]),
            ("skewY(45)", [1.0, 1.0, 0.0, 1.0]),
            ("matrix(1 0 0.6 0.8 0 0)", [1.0, 0.0, 0.6, 0.8]),
        ] {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg">
                <circle id="pin:1" cx="0" cy="0" r="0.5"/>
                <circle cx="0" cy="0" r="2" transform="{skew}"/>
            </svg>"#
            );
            let s = symbol(&svg, "svg.skew", "Skew")?;
            let Some(Shape::Polyline {
                points, closed: true, ..
            }) = s.graphics.first()
            else {
                unreachable!("{skew}: {:?}", s.graphics);
            };
            // Every point of the circle of radius 2 round (0, 0), moved by the skew: undoing
            // the skew puts it back on the circle.
            let det = a * d - b * c;
            for p in points {
                let q = Point::new((d * p.x - c * p.y) / det, (a * p.y - b * p.x) / det);
                assert!(
                    (q.x.hypot(q.y) - 2.0).abs() < 1e-9,
                    "{skew}: {p:?} not on the skewed circle"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn fills_from_attribute_style_and_group() -> Result<()> {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg">
            <circle id="pin:1" cx="0" cy="0" r="0.5"/>
            <polygon points="0 0 4 0 2 3" fill="black"/>
            <path style="fill:#000" d="M0 0 L4 0 L2 3 Z"/>
            <rect x="0" y="0" width="4" height="2" fill="none"/>
            <rect x="0" y="0" width="4" height="2" style="stroke:red; fill:none" fill="black"/>
            <polygon points="0 0 4 0 2 3" fill="#FFFFFF"/>
            <circle cx="10" cy="0" r="1" style="fill:black"/>
            <polyline points="0,0 4,0 2,3" fill="red"/>
            <polyline points="0,0 4,0" fill="red"/>
            <path fill="black" d="M0 0 L4 0 L2 3"/>
            <g style="fill:#000000"><rect x="0" y="0" width="1" height="1"/></g>
            <g fill="black"><rect x="0" y="0" width="1" height="1" fill="none"/></g>
            <rect x="0" y="0" width="1" height="1"/>
        </svg>"##;
        let s = symbol(svg, "svg.fill", "Fill")?;
        let got: Vec<(bool, bool)> = s
            .graphics
            .iter()
            .map(|g| match g {
                Shape::Polyline { closed, filled, .. } => (*closed, *filled),
                Shape::Circle { filled, .. } => (true, *filled),
                _ => (false, false),
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (true, true),   // polygon fill="black"
                (true, true),   // closed path, fill in style
                (true, false),  // rect fill="none"
                (true, false),  // style fill:none wins over fill="black"
                (true, false),  // white polygon, any case
                (true, true),   // circle, fill in style
                (true, true),   // filled 3-point polyline is closed
                (false, false), // 2-point polyline stays a line
                (false, false), // open path stays a line
                (true, true),   // fill from the parent group
                (true, false),  // own fill="none" wins over the group
                (true, false),  // no fill after the groups close
            ]
        );
        Ok(())
    }

    /// `filled` of each imported shape, in order.
    fn fills_of(svg: &str) -> Result<Vec<bool>> {
        let s = symbol(svg, "svg.fill", "Fill")?;
        Ok(s.graphics
            .iter()
            .map(|g| match g {
                Shape::Polyline { filled, .. } | Shape::Circle { filled, .. } => *filled,
                _ => false,
            })
            .collect())
    }

    #[test]
    fn fill_ignores_important_and_reads_white_rgb() -> Result<()> {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg">
            <rect width="1" height="1" style="fill:none!important" fill="black"/>
            <rect width="1" height="1" style="fill: none !IMPORTANT"/>
            <rect width="1" height="1" style="fill:#000 !important"/>
            <rect width="1" height="1" fill="rgb(255,255,255)"/>
            <rect width="1" height="1" fill="RGB( 255 , 255 , 255 )"/>
            <rect width="1" height="1" style="fill:rgb(100%, 100%, 100%)"/>
            <rect width="1" height="1" fill="rgb(255,0,0)"/>
            <rect width="1" height="1" fill="rgb(0%,0%,0%)"/>
            <rect width="1" height="1" style="fill:none!important;fill:black"/>
            <rect width="1" height="1" style="fill:black !important;fill:none"/>
            <rect width="1" height="1" style="fill:black;fill:none"/>
            <rect width="1" height="1" style="fill:none;fill:black"/>
            <rect width="1" height="1" style="fill:none ! important"/>
            <rect width="1" height="1" style="fill:none ! IMPORTANT;fill:black"/>
        </svg>"##;
        assert_eq!(
            fills_of(svg)?,
            vec![
                false, false, true, false, false, false, true, true, false, true, false, true, false, false,
            ]
        );
        Ok(())
    }

    #[test]
    fn fill_inherit_takes_the_parent_fill() -> Result<()> {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg">
            <g fill="none"><rect width="1" height="1" fill="inherit"/></g>
            <g fill="black"><rect width="1" height="1" fill="inherit"/></g>
            <g fill="none"><g fill="inherit"><rect width="1" height="1" fill="inherit"/></g></g>
            <g fill="none"><rect width="1" height="1" fill="black"/></g>
            <rect width="1" height="1" fill="inherit"/>
        </svg>"##;
        assert_eq!(fills_of(svg)?, vec![false, true, false, true, false]);
        Ok(())
    }

    #[test]
    fn a_fill_with_opacity_zero_does_not_fill() -> Result<()> {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg">
            <rect width="1" height="1" fill="#000" fill-opacity="0"/>
            <rect width="1" height="1" style="fill:#000000;fill-opacity:0"/>
            <rect width="1" height="1" fill="#000" fill-opacity="0%"/>
            <rect width="1" height="1" fill="#000" fill-opacity="0.5"/>
            <g fill="#000" fill-opacity="0"><rect width="1" height="1"/></g>
            <g fill-opacity="0"><rect width="1" height="1" fill="#000"/></g>
            <g fill="#000" fill-opacity="0"><rect width="1" height="1" fill-opacity="1"/></g>
            <rect width="1" height="1" fill="#000" style="fill-opacity:0" fill-opacity="1"/>
        </svg>"##;
        assert_eq!(
            fills_of(svg)?,
            vec![false, false, false, true, false, false, true, false]
        );
        Ok(())
    }

    #[test]
    fn defs_markers_and_hidden_groups_are_skipped() -> Result<()> {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg">
            <defs>
                <circle cx="5" cy="5" r="2"/>
                <path d="M0 0 L9 9"/>
                <marker id="arrow"><path d="M0 0 L2 1 L0 2 Z" fill="black"/></marker>
            </defs>
            <defs/>
            <line x1="0" y1="0" x2="10" y2="0"/>
            <g display="none"><line x1="0" y1="1" x2="10" y2="1"/><circle data-pin="1" r="1"/></g>
            <g style="display: none"><g><text x="0" y="0">Hidden</text></g></g>
            <symbol id="s"><rect width="3" height="3"/></symbol>
            <clipPath id="c"><rect width="4" height="4"/></clipPath>
            <mask id="m"><rect width="5" height="5"/></mask>
            <pattern id="p"><rect width="6" height="6"/></pattern>
            <rect width="7" height="7" display="none"/>
            <g display="none"><g display="inline"><line x1="0" y1="2" x2="10" y2="2"/></g></g>
            <line x1="0" y1="3" x2="10" y2="3" visibility="hidden"/>
            <g style="visibility:hidden"><line x1="0" y1="4" x2="10" y2="4"/></g>
            <g visibility="hidden"><line x1="0" y1="5" x2="10" y2="5" visibility="visible"/></g>
            <line x1="0" y1="6" x2="10" y2="6"/>
            <text x="0" y="7"><tspan display="none">No1</tspan><tspan>Yes1</tspan></text>
            <text x="0" y="8"><tspan visibility="hidden">No2</tspan><tspan>Yes2</tspan></text>
            <text x="0" y="9" visibility="hidden"><tspan visibility="visible">Yes3</tspan></text>
            <text x="0" y="10" visibility="hidden">No3</text>
            <text x="0" y="11" display="none"><tspan visibility="visible">No4</tspan></text>
        </svg>"##;
        let s = symbol(svg, "svg.hidden", "Hidden")?;
        assert!(s.pins.is_empty(), "pin in a hidden group: {:?}", s.pins);
        // Only the visible lines and texts come in; anything else shows up as NaN.
        let ys: Vec<(f64, &str)> = s
            .graphics
            .iter()
            .map(|g| match g {
                Shape::Line { a, .. } => (a.y, ""),
                Shape::Text { at, text, .. } => (at.y, text.as_str()),
                _ => (f64::NAN, ""),
            })
            .collect();
        assert_eq!(
            ys,
            vec![
                (0.0, ""),
                (5.0, ""),
                (6.0, ""),
                (7.0, "Yes1"),
                (8.0, "Yes2"),
                (9.0, "Yes3")
            ],
            "{:?}",
            s.graphics
        );
        Ok(())
    }

    #[test]
    fn long_fill_under_deep_nesting_is_not_copied_per_level() -> Result<()> {
        // A 1 MB fill value with 5,000 nested `inherit` groups under it. Copying the fill
        // text per level would need about 5 GB; the per-level state is a bool, so this stays
        // small and quick.
        let long = format!("url(#{})", "a".repeat(1 << 20));
        let depth = 5_000;
        let mut svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg"><g fill="{long}">"#);
        svg.push_str(&r#"<g fill="inherit">"#.repeat(depth));
        svg.push_str(r#"<rect width="1" height="1" fill="inherit"/>"#);
        svg.push_str(r#"<rect width="1" height="1" fill="none"/>"#);
        svg.push_str(&"</g>".repeat(depth));
        svg.push_str("</g></svg>");
        // A `url(...)` paint fills; `inherit` carries it all the way down.
        assert_eq!(fills_of(&svg)?, vec![true, false]);
        Ok(())
    }

    /// The filled outlines of an SVG, as DXF-style points for [`crate::dxf::inside`].
    fn solid_outlines(svg: &str) -> Result<Vec<Vec<(f64, f64)>>> {
        let s = symbol(svg, "svg.holes", "Holes")?;
        Ok(s.graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Polyline {
                    points, filled: true, ..
                } => Some(points.iter().map(|p| (p.x, p.y)).collect()),
                _ => None,
            })
            .collect())
    }

    /// Whether a point is painted by any of the filled outlines.
    fn painted(outlines: &[Vec<(f64, f64)>], pt: (f64, f64)) -> bool {
        outlines.iter().any(|o| crate::dxf::inside(pt, o))
    }

    #[test]
    fn path_with_a_hole_is_filled_around_it() -> Result<()> {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg">
            <path fill="black" d="M0 0H10V10H0Z M3 3V7H7V3Z"/>
            <path fill="black" d="M0 0H10V10H0Z M20 0 L30 0"/>
        </svg>"##;
        // The square with a hole running the other way: one filled outline. One closed
        // sub-path plus an open line: the closed one filled, the line not.
        assert_eq!(fills_of(svg)?, vec![true, true, false]);
        let first = r##"<svg xmlns="http://www.w3.org/2000/svg">
            <path fill="black" d="M0 0H10V10H0Z M3 3V7H7V3Z"/>
        </svg>"##;
        let o = solid_outlines(first)?;
        assert_eq!(o.len(), 1);
        assert!(painted(&o, (1.0, 1.0)), "the ring is painted");
        assert!(!painted(&o, (5.0, 5.0)), "the hole stays open");
        Ok(())
    }

    #[test]
    fn fill_rule_decides_a_hole_that_runs_the_same_way() -> Result<()> {
        // Both squares run the same way: nonzero (the default) paints the inner one too,
        // evenodd leaves it open, also when the rule comes from a group's style.
        let same = "M0 0H10V10H0Z M3 3H7V7H3Z";
        let nonzero = format!(r##"<svg xmlns="http://www.w3.org/2000/svg"><path fill="black" d="{same}"/></svg>"##);
        let o = solid_outlines(&nonzero)?;
        assert_eq!(o.len(), 1);
        assert!(painted(&o, (5.0, 5.0)), "nonzero: no hole");
        assert!(painted(&o, (1.0, 1.0)));
        for svg in [
            format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg"><path fill="black" fill-rule="evenodd" d="{same}"/></svg>"##
            ),
            format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg"><g style="fill-rule:evenodd"><path fill="black" d="{same}"/></g></svg>"##
            ),
        ] {
            let o = solid_outlines(&svg)?;
            assert_eq!(o.len(), 1, "{svg}");
            assert!(!painted(&o, (5.0, 5.0)), "evenodd: a hole ({svg})");
            assert!(painted(&o, (1.0, 1.0)), "{svg}");
        }
        // A child's own rule wins over its group's.
        let back = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg"><g fill-rule="evenodd"><path fill="black" fill-rule="nonzero" d="{same}"/></g></svg>"##
        );
        assert!(painted(&solid_outlines(&back)?, (5.0, 5.0)));
        Ok(())
    }

    #[test]
    fn path_with_an_island_in_its_hole_and_a_part_apart() -> Result<()> {
        // A ring with a dot in its hole, and a square of its own beside it.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg">
            <path fill="black" fill-rule="evenodd"
                d="M0 0H10V10H0Z M2 2H8V8H2Z M4 4H6V6H4Z M20 0H24V4H20Z"/>
        </svg>"##;
        let o = solid_outlines(svg)?;
        assert_eq!(o.len(), 3, "ring, island and the square apart");
        assert!(painted(&o, (1.0, 1.0)));
        assert!(!painted(&o, (3.0, 3.0)), "the hole");
        assert!(painted(&o, (5.0, 5.0)), "the island in it");
        assert!(painted(&o, (22.0, 2.0)), "the square apart");
        assert!(!painted(&o, (15.0, 2.0)));
        Ok(())
    }

    #[test]
    fn a_hole_that_touches_the_outline_around_it_stays_open() -> Result<()> {
        // The triangle starts on the square's right edge.
        for rule in ["nonzero", "evenodd"] {
            for d in ["M0 0H10V10H0Z M10 5L7 3L7 7Z", "M0 0H10V10H0Z M10 5L7 7L7 3Z"] {
                let svg = format!(
                    r##"<svg xmlns="http://www.w3.org/2000/svg"><path fill="black" fill-rule="{rule}" d="{d}"/></svg>"##
                );
                let o = solid_outlines(&svg)?;
                assert!(painted(&o, (2.0, 5.0)), "{rule} {d}");
                if rule == "evenodd" || d.ends_with("L7 3L7 7Z") {
                    // A hole: evenodd always, nonzero when it runs the other way.
                    assert!(!painted(&o, (8.0, 5.0)), "{rule} {d}");
                } else {
                    assert!(painted(&o, (8.0, 5.0)), "nonzero, same way: no hole ({d})");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn sub_paths_left_out_of_the_fill_keep_their_outline() -> Result<()> {
        // A closed two-point line, and (nonzero) a square inside one that runs the same way:
        // neither is an edge of the fill, so both stay outlines beside the one filled shape.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg">
            <path fill="black" d="M0 0H10V10H0Z M20 0L30 0Z M3 3H7V7H3Z"/>
        </svg>"##;
        let mut fills = fills_of(svg)?;
        fills.sort_unstable();
        assert_eq!(fills, vec![false, false, true]);
        Ok(())
    }

    #[test]
    fn a_huge_fill_with_holes_stays_outlines_and_warns() -> Result<()> {
        let mut d = String::from("M0 0");
        for i in 1..=MAX_HOLE_FILL_POINTS {
            d.push_str(&format!(" L{} {}", i as f64 * 0.01, if i % 2 == 0 { 0.0 } else { 0.5 }));
        }
        d.push_str(" V-100 H0 Z M1 -50 H2 V-49 H1 Z");
        let svg = format!(r##"<svg xmlns="http://www.w3.org/2000/svg"><path fill="black" d="{d}"/></svg>"##);
        let (s, warnings) = symbol_with_warnings(&svg, "svg.huge", "Huge")?;
        assert!(
            s.graphics
                .iter()
                .all(|g| !matches!(g, Shape::Polyline { filled: true, .. })),
            "no fill"
        );
        assert!(warnings.iter().any(|w| w.contains("drawn as outlines")), "{warnings:?}");
        Ok(())
    }

    #[test]
    fn unfilled_path_with_several_sub_paths_stays_outlines() -> Result<()> {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg">
            <path fill="none" d="M0 0H10V10H0Z M3 3V7H7V3Z"/>
        </svg>"##;
        assert_eq!(fills_of(svg)?, vec![false, false]);
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
    fn turned_text_keeps_its_angle() -> Result<()> {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg">
            <circle id="pin:1" cx="0" cy="0" r="0.5"/>
            <text transform="rotate(-90 5 5)" x="5" y="5">K1</text>
            <g transform="rotate(-90 5 5)"><text x="5" y="5">K2</text></g>
            <text x="5" y="5">K3</text>
            <text transform="rotate(90)" x="0" y="0">K4</text>
        </svg>"#;
        let s = symbol(svg, "svg.turn", "Turn")?;
        let texts: Vec<(&str, f64, Point)> = s
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Text { text, angle, at, .. } => Some((text.as_str(), *angle, *at)),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 4);
        let want = [("K1", 90.0), ("K2", 90.0), ("K3", 0.0), ("K4", 270.0)];
        for ((text, angle, _), (w_text, w_angle)) in texts.iter().zip(want) {
            assert_eq!((*text, *angle), (w_text, w_angle));
        }
        // Turned round its own anchor, the text stays where it was.
        for (_, _, at) in &texts[..3] {
            assert!((at.x - 5.0).abs() < 1e-9 && (at.y - 5.0).abs() < 1e-9, "{at:?}");
        }
        Ok(())
    }

    #[test]
    fn text_size_and_alignment_in_style() -> Result<()> {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg">
            <circle id="pin:1" cx="0" cy="0" r="0.5"/>
            <text style="font-size:10px;text-anchor:end" x="0" y="0">A</text>
            <text font-size="10" text-anchor="end" x="0" y="5">B</text>
            <text font-size="4" style="font-size:8px" x="0" y="10">C</text>
            <text x="0" y="15">D</text>
            <text style="font-size:6pt; text-anchor: Middle" x="0" y="20">E</text>
            <text font-size="2em" x="0" y="25">F</text>
        </svg>"#;
        let s = symbol(svg, "svg.text", "Text")?;
        let texts: Vec<(&str, f64, Align)> = s
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Text {
                    text, height, align, ..
                } => Some((text.as_str(), *height, *align)),
                _ => None,
            })
            .collect();
        let h = |name: &str| texts.iter().find(|t| t.0 == name).map(|t| (t.1, t.2));
        let (Some(a), Some(b), Some(c), Some(d), Some(e), Some(f)) = (h("A"), h("B"), h("C"), h("D"), h("E"), h("F"))
        else {
            unreachable!("all six texts come in: {texts:?}");
        };
        assert_eq!(a, b, "style reads like the attributes");
        assert_eq!(a.1, Align::Right);
        assert!((c.0 - 0.8 * a.0).abs() < 1e-9, "style's 8 wins over the attribute's 4");
        assert!((d.0 - 0.35 * a.0).abs() < 1e-9, "the 3.5 fallback");
        assert!((e.0 - 0.8 * a.0).abs() < 1e-9, "6pt is 8px");
        assert_eq!(e.1, Align::Center);
        assert_eq!(f.0, d.0, "em is not read: the fallback");
        Ok(())
    }

    #[test]
    fn text_size_and_alignment_from_group_and_tspan() -> Result<()> {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg">
            <circle id="pin:1" cx="0" cy="0" r="0.5"/>
            <text font-size="10" x="0" y="0">REF</text>
            <g style="font-size:10px;text-anchor:end"><text x="0" y="5">G</text></g>
            <g font-size="4" text-anchor="end"><text style="font-size:10px;text-anchor:start" x="0" y="10">W</text></g>
            <text x="0" y="15"><tspan style="font-size:10px;text-anchor:middle">T</tspan></text>
            <g font-size="10"><g><text font-size="inherit" x="0" y="20">N</text></g></g>
            <g font-size="10"><text transform="scale(2)" x="0" y="25">S</text></g>
            <text x="0" y="30" text-anchor="end"><tspan>A</tspan></text>
        </svg>"#;
        let s = symbol(svg, "svg.text", "Text")?;
        let texts: Vec<(&str, f64, Align)> = s
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Text {
                    text, height, align, ..
                } => Some((text.as_str(), *height, *align)),
                _ => None,
            })
            .collect();
        let h = |name: &str| texts.iter().find(|t| t.0 == name).map(|t| (t.1, t.2));
        let (Some(r), Some(g), Some(w), Some(t), Some(n), Some(sc), Some(a)) =
            (h("REF"), h("G"), h("W"), h("T"), h("N"), h("S"), h("A"))
        else {
            unreachable!("all texts come in: {texts:?}");
        };
        assert_eq!(g, (r.0, Align::Right), "a parent group's size and alignment");
        assert_eq!(w, (r.0, Align::Left), "the text's own values win over its group's");
        assert_eq!(t, (r.0, Align::Center), "a tspan holding the words sets them");
        assert_eq!(n.0, r.0, "inherited through nested groups and `inherit`");
        assert!(
            (sc.0 - 2.0 * r.0).abs() < 1e-9,
            "an inherited size scales with the text"
        );
        assert_eq!(a.1, Align::Right, "a tspan without values keeps the text's");
        Ok(())
    }

    #[test]
    fn font_size_zero_is_kept_and_an_overflow_is_not_read() {
        assert_eq!(font_size("0"), Some(0.0));
        assert_eq!(font_size("0px"), Some(0.0));
        assert_eq!(font_size("0pt"), Some(0.0));
        assert_eq!(font_size("6pt"), Some(8.0));
        assert_eq!(font_size("1.5e308pt"), None, "pt to px overflows");
        assert_eq!(font_size("-1px"), None);
        assert_eq!(font_size("NaN"), None);
        assert_eq!(font_size("inf"), None);
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
    fn text_in_tspan_after_line_break() -> Result<()> {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 20">
            <text x="3" y="10" font-size="4">
              <tspan>K1</tspan>
            </text>
            <text x="3" y="14"/>
            <title>not a label</title>
            <text x="3" y="16">
            </text>
            <desc>also not a label</desc>
        </svg>"#;
        let s = symbol(svg, "svg.k1", "K1")?;
        let texts: Vec<&str> = s
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["K1"]);
        // Text that cannot be read is an error, not a skipped line break.
        let bad = r#"<svg><text x="0" y="0">&missing;<tspan>K1</tspan></text></svg>"#;
        assert!(symbol(bad, "svg.bad", "Bad").is_err());
        // Unreadable text with no label waiting for it is ignored, as before.
        let title = r#"<svg><title>&missing;</title><line x1="0" y1="0" x2="1" y2="1"/></svg>"#;
        assert!(symbol(title, "svg.title", "Title").is_ok());
        Ok(())
    }

    #[test]
    fn words_after_a_tspan_stay_in_the_label() -> Result<()> {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg">
            <circle id="pin:1" cx="0" cy="0" r="0.5"/>
            <text x="0" y="0">-K<tspan font-weight="bold">1</tspan></text>
            <text x="0" y="5">Relay <tspan>coil</tspan>
              <tspan>A1</tspan></text>
            <text x="1" y="10"><tspan x="1" y="10">Line one</tspan><tspan x="1" y="14">Line two</tspan></text>
            <text x="2" y="20">Top<tspan y="24">Low</tspan></text>
        </svg>"#;
        let s = symbol(svg, "svg.runs", "Runs")?;
        let o = pin_at(&s, "1");
        let texts: Vec<(&str, Point)> = s
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Text { text, at, .. } => Some((text.as_str(), Point::new(at.x + o.x, at.y + o.y))),
                _ => None,
            })
            .collect();
        let words: Vec<&str> = texts.iter().map(|t| t.0).collect();
        assert_eq!(
            words,
            vec!["-K1", "Relay coil A1", "Line one", "Line two", "Top", "Low"],
            "no words are lost"
        );
        assert!(
            near(texts[3].1, Point::new(1.0, 14.0)),
            "a tspan line starts at its own place"
        );
        assert!(near(texts[5].1, Point::new(2.0, 24.0)), "a missing x is the text's");
        Ok(())
    }

    #[test]
    fn a_hidden_tspan_places_its_visible_words_and_notes_stay_out() -> Result<()> {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg">
            <circle id="pin:1" cx="0" cy="0" r="0.5"/>
            <text x="0" y="0">A<tspan y="10" visibility="hidden"><tspan visibility="visible">B</tspan></tspan></text>
            <text x="0" y="20">C<tspan><title>tooltip</title>D</tspan><desc>note</desc></text>
            <text x="0" y="30">E<tspan y="40" display="none"><tspan visibility="visible">F</tspan></tspan></text>
        </svg>"#;
        let s = symbol(svg, "svg.hidden", "Hidden")?;
        let texts: Vec<(&str, Point)> = s
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Text { text, at, .. } => Some((text.as_str(), *at)),
                _ => None,
            })
            .collect();
        let words: Vec<&str> = texts.iter().map(|t| t.0).collect();
        assert_eq!(words, vec!["A", "B", "CD", "E"]);
        assert!(near(texts[1].1, Point::new(0.0, 10.0)), "the hidden tspan's y places B");
        Ok(())
    }

    #[test]
    fn a_text_inside_metadata_does_not_end_the_open_text() -> Result<()> {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg">
            <circle id="pin:1" cx="0" cy="0" r="0.5"/>
            <text x="0" y="5"><metadata><text xmlns="urn:notes">note</text></metadata>Relay</text>
        </svg>"#;
        let s = symbol(svg, "svg.meta", "Meta")?;
        let texts: Vec<(&str, Point)> = s
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Text { text, at, .. } => Some((text.as_str(), *at)),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 1, "{texts:?}");
        assert_eq!(texts[0].0, "Relay");
        assert!(near(texts[0].1, Point::new(0.0, 5.0)));
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
        let (lines, _) = path(d, &M::ID);
        assert_eq!(lines.len(), 1, "{d}: {lines:?}");
        lines.into_iter().next().map(|(pts, _)| pts).unwrap_or_default()
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
        assert_eq!(
            arc_numbers("5 5 0 2 1 10 0"),
            (
                vec![5.0, 5.0, 0.0],
                Some("arc argument 4 is a flag that is not 0 or 1".to_owned())
            )
        );
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
    fn a_path_cut_short_gives_one_warning_with_the_reason() -> Result<()> {
        let svg = r#"<svg>
            <path d="M 0 0 L 10 0"/>
            <path id="arm" d="M 0 0 L 1 0 A 5 5 0 0 1 10 L 20 0"/>
        </svg>"#;
        let (s, warnings) = symbol_with_warnings(svg, "svg.cut", "Cut")?;
        assert_eq!(
            warnings,
            [
                "path 2 (id \"arm\") is drawn only in part: stopped at command 3 (A): an arc is cut short \
                 after argument 6 (an arc takes 7)"
            ]
        );
        // What came before the error is still drawn.
        assert_eq!(s.graphics.len(), 2);
        // The other reasons, and a well-formed file gives none.
        assert_eq!(
            path("M 0 0 A 5 . 0 0 1 10 0", &M::ID).1.as_deref(),
            Some("stopped at command 2 (A): arc argument 2 is not a number")
        );
        assert_eq!(
            path("M 0 0 a", &M::ID).1.as_deref(),
            Some("stopped at command 2 (a): an arc has no arguments")
        );
        assert_eq!(
            path("M 0 0 A 5 5 0 0 1 1e999 0", &M::ID).1.as_deref(),
            Some("stopped at command 2 (A): arc argument 6 is not a finite number")
        );
        assert!(
            symbol_with_warnings("<svg><path d=\"M 0 0 A 5 5 0 0 1 10 0\"/></svg>", "x", "x")?
                .1
                .is_empty()
        );
        // A hidden path still counts, so the bad one after it is path 2, and a hidden bad path
        // gives no warning.
        for svg in [
            r#"<svg><defs><path d="M 0 0 L 1 0"/></defs><path d="M 0 0 L 1 0 A"/></svg>"#,
            r#"<svg><path display="none" d="M 0 0 L 1 0 A"/><path d="M 0 0 L 1 0 A"/></svg>"#,
        ] {
            let (_, warnings) = symbol_with_warnings(svg, "x", "x")?;
            assert_eq!(
                warnings,
                ["path 2 is drawn only in part: stopped at command 3 (A): an arc has no arguments"],
                "{svg}"
            );
        }
        Ok(())
    }

    #[test]
    fn malformed_arc_arguments_name_the_argument() {
        let why = |d: &str| path(d, &M::ID).1;
        // An exponent without digits spoils its own argument, not the next one.
        assert_eq!(
            arc_numbers("5e 5 0 0 1 10 0"),
            (vec![], Some("arc argument 1 is not a number".to_owned()))
        );
        assert_eq!(
            arc_numbers("5 5e+ 0 0 1 10 0"),
            (vec![5.0], Some("arc argument 2 is not a number".to_owned()))
        );
        assert_eq!(
            arc_numbers("5 5 0 0 1 2e+0 0"),
            (vec![5.0, 5.0, 0.0, 0.0, 1.0, 2.0, 0.0], None)
        );
        // A letter that is no command is a bad argument of the arc.
        assert_eq!(
            why("M 0 0 A 5 5 0 0 1 10x 0 L 20 0").as_deref(),
            Some("stopped at command 2 (A): arc argument 7 is not a number")
        );
        assert_eq!(why("M 0 0 A 5 5 0 0 1 10 0 L 20 0"), None);
        // One comma between arguments, not two; compact flags still read.
        assert_eq!(
            arc_numbers("5,,5 0 0 1 10 0"),
            (
                vec![5.0],
                Some("arc argument 2 comes after more than one comma".to_owned())
            )
        );
        assert_eq!(
            arc_numbers("5 , 5 0 0110 20"),
            (vec![5.0, 5.0, 0.0, 0.0, 1.0, 10.0, 20.0], None)
        );
        // An arc back to its start draws nothing.
        assert_eq!(one_line("M 0 0 L 10 0 A 5 5 0 0 1 10 0").len(), 2);
        // A drawing left empty by a stopped path says why.
        let Err(err) = symbol("<svg><path d=\"M 0 0 A 5 5 0 0 1 10\"/></svg>", "x", "x") else {
            unreachable!("an empty drawing is an error");
        };
        assert_eq!(
            err.to_string(),
            "the drawing is empty: path 1 is drawn only in part: stopped at command 2 (A): an arc is cut \
             short after argument 6 (an arc takes 7)"
        );
        // An id that decodes to a control character comes out escaped, not raw.
        let Err(err) = symbol("<svg><path id=\"&#27;[2J\" d=\"M 0 0 A\"/></svg>", "x", "x") else {
            unreachable!("an empty drawing is an error");
        };
        let msg = err.to_string();
        assert!(!msg.chars().any(char::is_control), "{msg:?}");
        assert!(
            msg.contains("(id \"\\u{1b}[2J\")") && msg.contains("an arc has no arguments"),
            "{msg}"
        );
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
            let _ = path(d, &M::ID).0;
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
            "M 0 0 Q -1e308 0 5e307 0 T 5e307 0",
            "M 0 0 C 0 0 -1e308 0 5e307 0 S 5e307 0 5e307 0",
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

//! Symbols from KiCad symbol libraries (`.kicad_sym`, KiCad 6 and later).
//!
//! The file is an S-expression: `(kicad_symbol_lib (symbol "Name" (property ...) (symbol
//! "Name_0_1" graphics...) (symbol "Name_1_1" pins...)) ...)`. Coordinates are millimetres with
//! y pointing up. Graphics (polylines, rectangles, circles, arcs, texts) and pins of the common
//! unit and each unit, body style 1, become one WiringCraft symbol per unit: a symbol with one
//! unit keeps its name, one with several becomes `<name>_A`, `<name>_B`, ... (a relay's coil
//! and its contacts). Derived symbols (`extends`) take their parent's drawing. Written from the
//! published file format description.

use std::collections::BTreeMap;

use wc_core::geom::{Align, Point, Shape};
use wc_core::symbol::{Pin, PinDir};
use wc_core::{Symbol, store};

use crate::{Error, Result};

/// Deepest nesting accepted; real libraries stay under ten.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
enum Sexp {
    Atom(String),
    List(Vec<Sexp>),
}

impl Sexp {
    fn head(&self) -> Option<&str> {
        match self {
            Sexp::List(items) => match items.first() {
                Some(Sexp::Atom(a)) => Some(a),
                _ => None,
            },
            Sexp::Atom(_) => None,
        }
    }
    fn items(&self) -> &[Sexp] {
        match self {
            Sexp::List(items) => items,
            Sexp::Atom(_) => &[],
        }
    }
    fn atom(&self) -> Option<&str> {
        match self {
            Sexp::Atom(a) => Some(a),
            Sexp::List(_) => None,
        }
    }
    /// Child lists with this head.
    fn all<'a>(&'a self, head: &'a str) -> impl Iterator<Item = &'a Sexp> + 'a {
        self.items().iter().filter(move |c| c.head() == Some(head))
    }
    fn first(&self, head: &str) -> Option<&Sexp> {
        self.items().iter().find(|c| c.head() == Some(head))
    }
    /// The atom at `i` (0 is the head).
    fn arg(&self, i: usize) -> Option<&str> {
        self.items().get(i).and_then(Sexp::atom)
    }
    fn num(&self, i: usize) -> Option<f64> {
        self.arg(i)?.parse::<f64>().ok().filter(|v| v.is_finite())
    }
}

/// Parse S-expressions without recursion, so deep nesting in a hostile file cannot overflow
/// the stack.
fn parse(text: &str) -> Result<Vec<Sexp>> {
    let mut stack: Vec<Vec<Sexp>> = vec![Vec::new()];
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '(' => {
                if stack.len() > MAX_DEPTH {
                    return Err(Error::Format("the file is nested too deeply".into()));
                }
                stack.push(Vec::new());
            }
            ')' => {
                let done = stack.pop().ok_or_else(|| Error::Format("unbalanced ')'".into()))?;
                stack
                    .last_mut()
                    .ok_or_else(|| Error::Format("unbalanced ')'".into()))?
                    .push(Sexp::List(done));
            }
            '"' => {
                let mut s = String::new();
                loop {
                    match chars.next() {
                        None => return Err(Error::Format("a string is not closed".into())),
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some(other) => s.push(other),
                            None => return Err(Error::Format("a string is not closed".into())),
                        },
                        Some(other) => s.push(other),
                    }
                }
                if let Some(top) = stack.last_mut() {
                    top.push(Sexp::Atom(s));
                }
            }
            c if c.is_whitespace() => {}
            c => {
                let mut s = String::from(c);
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace() || n == '(' || n == ')' || n == '"' {
                        break;
                    }
                    s.push(n);
                    chars.next();
                }
                if let Some(top) = stack.last_mut() {
                    top.push(Sexp::Atom(s));
                }
            }
        }
    }
    if stack.len() != 1 {
        return Err(Error::Format("the file ends inside a list".into()));
    }
    Ok(stack.pop().unwrap_or_default())
}

/// One imported symbol and what was left out of it.
#[derive(Debug, Clone)]
pub struct Imported {
    pub symbol: Symbol,
    /// The KiCad name, e.g. `Relay_SPDT`.
    pub kicad_name: String,
}

/// Read every symbol of a `.kicad_sym` library. Ids are `<prefix>.<name>` (lower case, other
/// characters as `_`), coordinates are multiplied by `scale`. Returns the symbols and warnings.
pub fn symbols(text: &str, prefix: &str, scale: f64) -> Result<(Vec<Imported>, Vec<String>)> {
    if !(scale.is_finite() && scale > 0.0 && scale <= 100.0) {
        return Err(Error::Format(format!("scale {scale} must be above 0 and at most 100")));
    }
    let top = parse(text)?;
    let lib = top
        .iter()
        .find(|s| s.head() == Some("kicad_symbol_lib"))
        .ok_or_else(|| {
            Error::Format("not a KiCad symbol library (KiCad 6 or later; old .lib files are not read)".into())
        })?;
    let by_name: BTreeMap<&str, &Sexp> = lib.all("symbol").filter_map(|s| Some((s.arg(1)?, s))).collect();
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    let id_of = |name: &str| format!("{}.{}", prefix.trim_end_matches('.'), crate::id_segment(name));
    // Every source symbol's own id is taken first, so a unit of `Relay` (`relay_a`) never
    // replaces a symbol the library itself calls `Relay_A`.
    let reserved: std::collections::BTreeSet<String> = by_name.keys().map(|n| id_of(n)).collect();
    let mut used = std::collections::BTreeSet::new();
    for (name, sym) in &by_name {
        // A derived symbol draws like its parent but keeps its own properties.
        let body = match sym.first("extends").and_then(|e| e.arg(1)) {
            Some(parent) => match by_name.get(parent) {
                Some(p) => *p,
                None => {
                    warnings.push(format!("{name}: its parent {parent} is not in the file, skipped"));
                    continue;
                }
            },
            None => *sym,
        };
        let base = body.arg(1).unwrap_or(name);
        let (units, styles) = units_of(body, base);
        if styles {
            warnings.push(format!("{name}: alternate body styles were skipped"));
        }
        // Unit 0 (common to every unit) alone: one symbol, as for a single unit.
        let several = units.len() > 1;
        for unit in units.into_iter().take(MAX_UNITS) {
            let unit_name = if several {
                format!("{name}_{}", unit_letter(unit))
            } else {
                (*name).to_owned()
            };
            let id = id_of(&unit_name);
            if (several && reserved.contains(&id)) || used.contains(&id) {
                warnings.push(format!("{unit_name}: its id {id} is another symbol's, skipped"));
                continue;
            }
            let mut skipped = BTreeMap::new();
            let mut hidden = Vec::new();
            let converted = convert(
                &unit_name,
                sym,
                body,
                base,
                unit,
                prefix,
                scale,
                &mut skipped,
                &mut hidden,
            );
            for (kind, n) in skipped {
                warnings.push(format!("{unit_name}: {n} {kind} item(s) not read"));
            }
            match converted {
                Ok(symbol) => {
                    // Usually power pins that connect without a wire in KiCad.
                    if !hidden.is_empty() {
                        let n = hidden.len();
                        let pins = if n == 1 { "pin" } else { "pins" };
                        warnings.push(format!(
                            "{unit_name}: {n} hidden {pins} left out ({})",
                            hidden.join(", ")
                        ));
                    }
                    // Taken only once there is a symbol: one that draws nothing leaves its id free.
                    used.insert(id);
                    out.push(Imported {
                        symbol,
                        kicad_name: unit_name,
                    });
                }
                Err(e) => warnings.push(format!("{unit_name}: {e}, skipped")),
            }
        }
    }
    Ok((out, warnings))
}

/// Most units read from one symbol; real parts stay far below (a quad gate has four).
const MAX_UNITS: usize = 64;

/// A sub-symbol's unit and body style from its name, `<base>_<unit>_<style>`.
fn unit_and_style(unit: &Sexp, base: &str) -> (u32, u32) {
    let suffix = unit.arg(1).and_then(|n| n.strip_prefix(base)).unwrap_or("");
    let mut parts = suffix.trim_start_matches('_').split('_');
    (
        parts.next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0),
        parts.next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(1),
    )
}

/// The units to import (each above 0, in order; `[1]` when only the common unit draws), and
/// whether alternate body styles were seen.
fn units_of(body: &Sexp, base: &str) -> (Vec<u32>, bool) {
    let mut units = std::collections::BTreeSet::new();
    let mut styles = false;
    for unit in body.all("symbol") {
        let (u, style) = unit_and_style(unit, base);
        if style > 1 {
            styles = true;
        } else if u > 0 {
            units.insert(u);
        }
    }
    if units.is_empty() {
        units.insert(1);
    }
    (units.into_iter().collect(), styles)
}

/// The letter of a unit as KiCad shows it: 1 is A, 26 is Z, 27 is AA.
fn unit_letter(mut unit: u32) -> String {
    let mut s = Vec::new();
    while unit > 0 {
        unit -= 1;
        s.push(char::from(b'A' + (unit % 26) as u8));
        unit /= 26;
    }
    s.iter().rev().collect()
}

fn property<'a>(sym: &'a Sexp, key: &str) -> Option<&'a str> {
    sym.all("property")
        .find(|p| p.arg(1) == Some(key))
        .and_then(|p| p.arg(2))
        .filter(|v| !v.is_empty() && *v != "~")
}

/// One unit of a symbol: the common unit's drawing and pins plus those of `wanted`. Graphic
/// items it cannot read are counted by kind in `skipped`; hidden pins are left out and named
/// (by number, else name) in `hidden`.
#[allow(clippy::too_many_arguments)]
fn convert(
    name: &str,
    sym: &Sexp,
    body: &Sexp,
    base: &str,
    wanted: u32,
    prefix: &str,
    scale: f64,
    skipped: &mut BTreeMap<String, usize>,
    hidden: &mut Vec<String>,
) -> Result<Symbol> {
    let id = format!("{}.{}", prefix.trim_end_matches('.'), crate::id_segment(name));
    store::check_id(&id)?;
    // KiCad y points up; the page's points down.
    let pt = |x: f64, y: f64| Point::new(x * scale, -y * scale);
    let xy = |s: &Sexp, head: &str| -> Option<Point> {
        let n = s.first(head)?;
        Some(pt(n.num(1)?, n.num(2)?))
    };
    // KiCad's `outline` fill is solid in the line colour; `background` and `none` stay hollow.
    let filled = |s: &Sexp| s.first("fill").and_then(|f| f.first("type")).and_then(|t| t.arg(1)) == Some("outline");
    let mut graphics = Vec::new();
    let mut pins: Vec<Pin> = Vec::new();
    for unit in body.all("symbol") {
        // Unit names end in `_<unit>_<style>`; unit 0 is common to all units.
        let (u, style) = unit_and_style(unit, base);
        if (u != 0 && u != wanted) || style > 1 {
            continue;
        }
        for g in unit.items() {
            // KiCad 8+: `(text private "text" ...)`, `(polyline private ...)`: a private item
            // shows only in KiCad's symbol editor. A text that reads "private" is not one.
            if g.arg(1) == Some("private") && (g.arg(2).is_some() || !matches!(g.head(), Some("text" | "text_box"))) {
                continue;
            }
            match g.head() {
                Some("polyline") => {
                    let mut points: Vec<Point> = g
                        .first("pts")
                        .map(|p| p.all("xy").filter_map(|x| Some(pt(x.num(1)?, x.num(2)?))).collect())
                        .unwrap_or_default();
                    // A polyline that ends where it starts is closed; drop the repeated point.
                    let ends_at_start = points.len() > 2 && points.first() == points.last();
                    if ends_at_start {
                        points.pop();
                    }
                    let filled = filled(g) && points.len() > 2;
                    match points.as_slice() {
                        [a, b] => graphics.push(Shape::Line { a: *a, b: *b }),
                        [_, _, ..] => graphics.push(Shape::Polyline {
                            points,
                            closed: ends_at_start || filled,
                            filled,
                        }),
                        _ => {}
                    }
                }
                Some("bezier") => {
                    // A cubic curve (start, two control points, end), flattened in 8 steps like
                    // the SVG importer does.
                    // Any point that does not parse leaves the whole curve out.
                    let p: Option<Vec<Point>> = g
                        .first("pts")
                        .and_then(|p| p.all("xy").map(|x| Some(pt(x.num(1)?, x.num(2)?))).collect());
                    if let Some([p0, p1, p2, p3]) = p.as_deref() {
                        let points = (0..=8)
                            .map(|k| {
                                let t = k as f64 / 8.0;
                                let u = 1.0 - t;
                                let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                                Point::new(
                                    a * p0.x + b * p1.x + c * p2.x + d * p3.x,
                                    a * p0.y + b * p1.y + c * p2.y + d * p3.y,
                                )
                            })
                            .collect();
                        graphics.push(Shape::Polyline {
                            points,
                            closed: false,
                            filled: false,
                        });
                    }
                }
                Some("rectangle") => {
                    if let (Some(a), Some(b)) = (xy(g, "start"), xy(g, "end")) {
                        let (min, max) = (
                            Point::new(a.x.min(b.x), a.y.min(b.y)),
                            Point::new(a.x.max(b.x), a.y.max(b.y)),
                        );
                        graphics.push(rect_shape(min, max, filled(g)));
                    }
                }
                Some("circle") => {
                    if let (Some(center), Some(r)) = (xy(g, "center"), g.first("radius").and_then(|r| r.num(1))) {
                        graphics.push(Shape::Circle {
                            center,
                            radius: r * scale,
                            filled: filled(g),
                        });
                    }
                }
                Some("arc") => {
                    if let (Some(a), Some(m), Some(b)) = (xy(g, "start"), xy(g, "mid"), xy(g, "end"))
                        && let Some(arc) = arc_through(a, m, b)
                    {
                        graphics.push(arc);
                    }
                }
                Some("text") => {
                    // A hidden text shows only in KiCad's symbol editor.
                    if g.first("effects").is_some_and(|e| hidden_from(e, 1)) {
                        continue;
                    }
                    if let (Some(t), Some(at)) = (g.arg(1), g.first("at")) {
                        graphics.extend(text_shapes(t, at, g.first("effects"), &pt, scale));
                    }
                }
                Some("pin") => {
                    let Some(at) = g.first("at") else { continue };
                    let (Some(x), Some(y)) = (at.num(1), at.num(2)) else {
                        continue;
                    };
                    let angle = at.num(3).unwrap_or(0.0).rem_euclid(360.0).round() as i64;
                    let length = g.first("length").and_then(|l| l.num(1)).unwrap_or(2.54) * scale;
                    let tip = pt(x, y);
                    // The pin line runs from its connection point towards the body at `angle`
                    // (y up), so the wire leaves the other way.
                    let (dir, inward) = match angle {
                        90 => (PinDir::Down, Point::new(0.0, -length)),
                        180 => (PinDir::Right, Point::new(-length, 0.0)),
                        270 => (PinDir::Up, Point::new(0.0, length)),
                        _ => (PinDir::Left, Point::new(length, 0.0)),
                    };
                    let text = |head: &str| {
                        g.first(head)
                            .and_then(|n| n.arg(1))
                            .filter(|v| !v.is_empty() && *v != "~")
                    };
                    let pin_name = text("number").or_else(|| text("name")).unwrap_or("").to_owned();
                    if g.items().iter().any(|i| i.atom() == Some("hide"))
                        || g.first("hide").and_then(|h| h.arg(1)) == Some("yes")
                    {
                        hidden.push(if pin_name.is_empty() { "?".to_owned() } else { pin_name });
                        continue;
                    }
                    if length > 0.0 {
                        graphics.extend(pin_shapes(g, tip, inward, length, scale));
                    }
                    pins.push(Pin {
                        name: pin_name,
                        at: tip,
                        dir,
                    });
                }
                // KiCad 7+: a text with an optional border, `at` its top-left corner. Its lines
                // are joined with a space into one text; margins are not kept.
                Some("text_box") => {
                    let (Some(t), Some(at)) = (g.arg(1), g.first("at")) else {
                        continue;
                    };
                    let (x, y) = (at.num(1).unwrap_or(0.0), at.num(2).unwrap_or(0.0));
                    let size = g.first("size");
                    let (w, h) = (
                        size.and_then(|s| s.num(1)).unwrap_or(0.0),
                        size.and_then(|s| s.num(2)).unwrap_or(0.0),
                    );
                    // `at` is one corner and `at + size` the other, both in KiCad's y-up
                    // coordinates, so either size may be negative.
                    let (a, b) = (pt(x, y), pt(x + w, y + h));
                    let (min, max) = (
                        Point::new(a.x.min(b.x), a.y.min(b.y)),
                        Point::new(a.x.max(b.x), a.y.max(b.y)),
                    );
                    let height = g
                        .first("effects")
                        .and_then(|e| e.first("font"))
                        .and_then(|f| f.first("size"))
                        .and_then(|s| s.num(1))
                        .unwrap_or(1.27)
                        * scale;
                    // A stroke width of 0 is KiCad's default width; only a negative one hides the border.
                    let stroke = g
                        .first("stroke")
                        .and_then(|s| s.first("width"))
                        .and_then(|w| w.num(1))
                        .unwrap_or(0.0);
                    let fill = g.first("fill").and_then(|f| f.first("type")).and_then(|t| t.arg(1));
                    if max.x > min.x && max.y > min.y && (stroke >= 0.0 || fill.is_some_and(|f| f != "none")) {
                        graphics.push(rect_shape(min, max, filled(g)));
                    }
                    let text = t
                        .split('\n')
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !text.is_empty() {
                        graphics.push(Shape::Text {
                            at: Point::new(min.x, min.y + height),
                            text,
                            height,
                            align: Align::Left,
                            angle: at.num(3).map(|a| if a > 360.0 { a / 10.0 } else { a }).unwrap_or(0.0),
                        });
                    }
                }
                // `unit_name` only names the unit; anything else draws something not read here.
                Some("unit_name") | None => {}
                Some(other) => *skipped.entry(other.to_owned()).or_default() += 1,
            }
        }
    }
    if graphics.is_empty() && pins.is_empty() {
        return Err(Error::Format("the symbol draws nothing".into()));
    }
    // A derived symbol shows its own fields (its Value is its own name).
    graphics.extend(visible_fields(sym, &pt, scale));
    // Pins without a designation are numbered; repeated designations get a mark.
    let mut used = std::collections::BTreeSet::new();
    for (i, p) in pins.iter_mut().enumerate() {
        if p.name.trim().is_empty() {
            p.name = (i + 1).to_string();
        }
        while !used.insert(p.name.clone()) {
            p.name.push('\'');
        }
    }
    // The first pin is the origin, as for the other importers.
    let origin = pins.first().map(|p| p.at).unwrap_or(Point::new(0.0, 0.0));
    let shift = Point::new(-origin.x, -origin.y);
    let snap = |v: f64| (v * 1000.0).round() / 1000.0;
    let fix = |p: Point| Point::new(snap(p.x + shift.x), snap(p.y + shift.y));
    let graphics: Vec<Shape> = graphics.iter().map(|s| s.translate(shift)).collect();
    for p in &mut pins {
        p.at = fix(p.at);
    }
    let reference = property(sym, "Reference").unwrap_or("");
    let tag_at = property_at(sym, "Reference")
        .map(|(x, y)| fix(pt(x, y)))
        .unwrap_or(Point::new(-5.0, 0.0));
    Ok(Symbol {
        source: String::new(),
        id,
        name: name.replace('_', " "),
        category: String::new(),
        letter_code: reference.chars().take_while(|c| c.is_ascii_alphabetic()).collect(),
        role: Default::default(),
        graphics,
        pins,
        tag_at,
        description: property(sym, "Description")
            .or_else(|| property(sym, "ki_description"))
            .unwrap_or("")
            .to_owned(),
    })
}

/// The pin line from `tip` to the body (`tip + inward`, `length` long) with KiCad's marks for
/// the pin's shape, the word after its type: `inverted` draws a circle at the body end (radius
/// half the pin number's text size), `clock` a triangle into the body (half the pin name's
/// size), `inverted_clock` both. Other shapes draw the plain line.
fn pin_shapes(pin: &Sexp, tip: Point, inward: Point, length: f64, scale: f64) -> Vec<Shape> {
    let font = |head: &str| {
        pin.first(head)
            .and_then(|n| n.first("effects"))
            .and_then(|e| e.first("font"))
            .and_then(|f| f.first("size"))
            .and_then(|s| s.num(1))
            .filter(|v| *v > 0.0)
    };
    let number_size = font("number").unwrap_or(1.27);
    let radius = number_size / 2.0 * scale;
    let clock = font("name").unwrap_or(number_size) / 2.0 * scale;
    let shape = pin.arg(2).unwrap_or("line");
    let (inverted, clocked) = match shape {
        "inverted" => (true, false),
        "clock" => (false, true),
        "inverted_clock" => (true, true),
        _ => (false, false),
    };
    // `u` points from the tip into the body, `n` square to it.
    let u = Point::new(inward.x / length, inward.y / length);
    let n = Point::new(-u.y, u.x);
    let root = Point::new(tip.x + inward.x, tip.y + inward.y);
    let at = |from: Point, along: f64, across: f64| {
        Point::new(from.x + u.x * along + n.x * across, from.y + u.y * along + n.y * across)
    };
    let mut out = Vec::new();
    if inverted {
        out.push(Shape::Circle {
            center: at(root, -radius, 0.0),
            radius,
            filled: false,
        });
        if length > 2.0 * radius {
            out.push(Shape::Line {
                a: tip,
                b: at(root, -2.0 * radius, 0.0),
            });
        }
    } else {
        out.push(Shape::Line { a: tip, b: root });
    }
    if clocked {
        out.push(Shape::Polyline {
            points: vec![at(root, 0.0, clock), at(root, clock, 0.0), at(root, 0.0, -clock)],
            closed: false,
            filled: false,
        });
    }
    out
}

/// A KiCad text at `at` with its `effects` (size, justification), as one text per line.
fn text_shapes(t: &str, at: &Sexp, effects: Option<&Sexp>, pt: &impl Fn(f64, f64) -> Point, scale: f64) -> Vec<Shape> {
    let justify = effects.and_then(|e| e.first("justify"));
    let has = |word: &str| justify.is_some_and(|j| j.items().iter().any(|w| w.atom() == Some(word)));
    let align = if has("left") {
        Align::Left
    } else if has("right") {
        Align::Right
    } else {
        Align::Center
    };
    let height = effects
        .and_then(|e| e.first("font"))
        .and_then(|f| f.first("size"))
        .and_then(|s| s.num(1))
        .unwrap_or(1.27);
    let height = height * scale;
    // KiCad stores tenths of a degree in old files and degrees now.
    let angle = at.num(3).map(|a| if a > 360.0 { a / 10.0 } else { a }).unwrap_or(0.0);
    let first = pt(at.num(1).unwrap_or(0.0), at.num(2).unwrap_or(0.0));
    let (sin, cos) = angle.to_radians().sin_cos();
    // KiCad puts `at` at the middle of the text block unless `justify` says
    // `top` or `bottom`; WiringCraft's `at` is the baseline of the first line.
    // So move the first baseline down: by the text height from the top edge,
    // and up by the other lines from the bottom edge.
    let pitch = 1.5 * height;
    let below = pitch * t.lines().count().saturating_sub(1) as f64;
    let drop = if has("top") {
        height
    } else if has("bottom") {
        -below
    } else {
        (height - below) / 2.0
    };
    // A text with line breaks becomes one text per line, each one line pitch
    // further down, square to the reading direction (y runs down here).
    let mut out = Vec::new();
    for (k, line) in t.lines().enumerate() {
        let line: String = line.chars().filter(|c| !c.is_control()).collect();
        if line.trim().is_empty() {
            continue;
        }
        let step = drop + pitch * k as f64;
        out.push(Shape::Text {
            at: Point::new(first.x + step * sin, first.y + step * cos),
            text: line,
            height,
            align,
            angle,
        });
    }
    out
}

/// The fields KiCad shows on the symbol, other than `Reference` (the tag) and `Description`:
/// usually Value. Hidden ones (KiCad 6/7 `(effects ... hide)`, KiCad 8+ `(hide yes)`) and
/// KiCad's own `ki_` keys are left out.
fn visible_fields(sym: &Sexp, pt: &impl Fn(f64, f64) -> Point, scale: f64) -> Vec<Shape> {
    let mut out = Vec::new();
    for p in sym.all("property") {
        let (Some(key), Some(value), Some(at)) = (p.arg(1), p.arg(2), p.first("at")) else {
            continue;
        };
        let effects = p.first("effects");
        if matches!(key, "Reference" | "Description")
            || key.starts_with("ki_")
            || value.trim().is_empty()
            || value == "~"
            || hidden_from(p, 3)
            || effects.is_some_and(|e| hidden_from(e, 1))
        {
            continue;
        }
        out.extend(text_shapes(value, at, effects, pt, scale));
    }
    out
}

/// Whether `s` is hidden: a bare `hide` flag (KiCad 6/7), read from item `from` on so a
/// property's key and value, or a text that reads "hide", do not count, or `(hide yes)`
/// (KiCad 8+).
fn hidden_from(s: &Sexp, from: usize) -> bool {
    s.items().iter().skip(from).any(|i| i.atom() == Some("hide"))
        || s.first("hide").and_then(|h| h.arg(1)) == Some("yes")
}

/// A rectangle from `min` to `max`. A rectangle has no fill of its own: a filled one becomes a
/// filled polyline.
fn rect_shape(min: Point, max: Point, filled: bool) -> Shape {
    if filled {
        Shape::Polyline {
            points: vec![min, Point::new(max.x, min.y), max, Point::new(min.x, max.y)],
            closed: true,
            filled: true,
        }
    } else {
        Shape::Rect { min, max }
    }
}

fn property_at(sym: &Sexp, key: &str) -> Option<(f64, f64)> {
    let at = sym.all("property").find(|p| p.arg(1) == Some(key))?.first("at")?;
    Some((at.num(1)?, at.num(2)?))
}

/// The arc from `a` through `m` to `b` (page coordinates, y down), as a centre, radius and
/// increasing angles. `None` when the points are on a line.
fn arc_through(a: Point, m: Point, b: Point) -> Option<Shape> {
    let d = 2.0 * (a.x * (m.y - b.y) + m.x * (b.y - a.y) + b.x * (a.y - m.y));
    if d.abs() < 1e-12 {
        return None;
    }
    let sq = |p: Point| p.x * p.x + p.y * p.y;
    let cx = (sq(a) * (m.y - b.y) + sq(m) * (b.y - a.y) + sq(b) * (a.y - m.y)) / d;
    let cy = (sq(a) * (b.x - m.x) + sq(m) * (a.x - b.x) + sq(b) * (m.x - a.x)) / d;
    let center = Point::new(cx, cy);
    let radius = ((a.x - cx).powi(2) + (a.y - cy).powi(2)).sqrt();
    let ang = |p: Point| (p.y - cy).atan2(p.x - cx).to_degrees().rem_euclid(360.0);
    let (sa, ma, ea) = (ang(a), ang(m), ang(b));
    // Sweep with increasing angle from start to end; if that misses the middle point, the
    // arc runs the other way, so swap the ends.
    let within = |from: f64, to: f64, x: f64| (x - from).rem_euclid(360.0) <= (to - from).rem_euclid(360.0);
    let (start, end) = if within(sa, ea, ma) { (sa, ea) } else { (ea, sa) };
    let end = if end <= start { end + 360.0 } else { end };
    Some(Shape::Arc {
        center,
        radius,
        start,
        end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIB: &str = r#"(kicad_symbol_lib (version 20231120) (generator "test")
  (symbol "Relay_Coil" (pin_names (offset 0)) (in_bom yes) (on_board yes)
    (property "Reference" "K" (at 3.81 1.27 0) (effects (font (size 1.27 1.27))))
    (property "Value" "Relay_Coil" (at 0 0 0))
    (property "Description" "Relay coil, \"generic\"" (at 0 0 0))
    (symbol "Relay_Coil_0_1"
      (rectangle (start -2.54 1.27) (end 2.54 -1.27) (stroke (width 0.254)) (fill (type none)))
      (polyline (pts (xy -1.27 -1.27) (xy 1.27 1.27)) (stroke (width 0)))
      (arc (start 0 2.54) (mid 1.27 1.27) (end 0 0))
      (circle (center 0 0) (radius 0.5) (fill (type outline)))
      (text "K" (at 0 0 900) (effects (font (size 1 1))))
    )
    (symbol "Relay_Coil_1_1"
      (pin passive line (at 0 3.81 270) (length 2.54) (name "~" (effects (font (size 1.27 1.27)))) (number "A1" (effects (font (size 1.27 1.27)))))
      (pin passive line (at 0 -3.81 90) (length 2.54) (name "~") (number "A2"))
      (pin power_in line (at 5 0 180) (length 2.54) hide (name "GND") (number "3"))
    )
    (symbol "Relay_Coil_1_2" (pin passive line (at 9 9 0) (length 1) (name "x") (number "9")))
    (symbol "Relay_Coil_2_1" (pin passive line (at 9 9 0) (length 1) (name "y") (number "8")))
  )
  (symbol "Relay_Coil_24V" (extends "Relay_Coil")
    (property "Reference" "K" (at 3.81 1.27 0))
    (property "Description" "24 V coil" (at 0 0 0)))
  (symbol "Orphan" (extends "Missing"))
)"#;

    #[test]
    fn reads_units_pins_graphics_and_derived_symbols() -> Result<()> {
        let (syms, warnings) = symbols(LIB, "kicad.relays", 1.0)?;
        // Two units each for the relay and the derived 24 V relay.
        assert_eq!(syms.len(), 4, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("Orphan")), "{warnings:?}");
        assert!(
            warnings.iter().any(|w| w == "Relay_Coil_A: 1 hidden pin left out (3)"),
            "{warnings:?}"
        );
        assert!(
            !warnings.iter().any(|w| w.contains("only the first unit")),
            "{warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.starts_with("Relay_Coil:") && w.contains("alternate body styles")),
            "{warnings:?}"
        );
        let coil = &syms[0].symbol;
        assert_eq!(syms[0].kicad_name, "Relay_Coil_A");
        assert_eq!(coil.id, "kicad.relays.relay_coil_a");
        assert_eq!(coil.name, "Relay Coil A");
        assert_eq!(coil.letter_code, "K");
        assert_eq!(coil.description, "Relay coil, \"generic\"");
        // Pin A1 at the top is the origin; A2 is 7.62 mm below it; the hidden pin is left out.
        let pins: Vec<(&str, Point, PinDir)> = coil.pins.iter().map(|p| (p.name.as_str(), p.at, p.dir)).collect();
        assert_eq!(
            pins,
            vec![
                ("A1", Point::new(0.0, 0.0), PinDir::Up),
                ("A2", Point::new(0.0, 7.62), PinDir::Down)
            ]
        );
        // rectangle, polyline (as a line), arc, circle, text, two pin lines, the Value field
        assert_eq!(coil.graphics.len(), 8, "{:?}", coil.graphics);
        assert!(coil.graphics.iter().any(|g| matches!(g, Shape::Rect { min, max }
            if (min.x + 2.54).abs() < 1e-9 && (min.y - 2.54).abs() < 1e-9 && (max.y - 5.08).abs() < 1e-9)));
        assert!(
            coil.graphics
                .iter()
                .any(|g| matches!(g, Shape::Circle { filled: true, .. }))
        );
        assert!(
            coil.graphics
                .iter()
                .any(|g| matches!(g, Shape::Text { angle, .. } if (*angle - 90.0).abs() < 1e-9))
        );
        // The arc bulges right: from the top point clockwise on paper (increasing angle,
        // y down) through the right to the bottom point.
        let arc = coil.graphics.iter().find_map(|g| match g {
            Shape::Arc {
                center,
                radius,
                start,
                end,
            } => Some((*center, *radius, *start, *end)),
            _ => None,
        });
        let Some((c, r, s, e)) = arc else {
            return Err(Error::Format("no arc".into()));
        };
        assert!(
            (c.x).abs() < 1e-9 && (c.y - 2.54).abs() < 1e-9 && (r - 1.27).abs() < 1e-9,
            "{c:?} {r}"
        );
        assert!((s - 270.0).abs() < 1e-6 && (e - 450.0).abs() < 1e-6, "{s} {e}");
        // The pin line of A1 runs from the pin down into the body.
        assert!(coil.graphics.contains(&Shape::Line {
            a: Point::new(0.0, 0.0),
            b: Point::new(0.0, 2.54)
        }));
        // Unit B: the common drawing with its own pin only.
        let contacts = &syms[1].symbol;
        assert_eq!(contacts.id, "kicad.relays.relay_coil_b");
        let names: Vec<&str> = contacts.pins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["8"]);
        assert!(
            contacts
                .graphics
                .iter()
                .any(|g| matches!(g, Shape::Circle { filled: true, .. }))
        );
        let derived = &syms[2].symbol;
        assert_eq!(derived.id, "kicad.relays.relay_coil_24v_a");
        assert_eq!(derived.description, "24 V coil");
        assert_eq!(derived.pins.len(), 2);
        assert_eq!(syms[3].symbol.id, "kicad.relays.relay_coil_24v_b");
        Ok(())
    }

    #[test]
    fn texts_keep_their_left_or_right_justification() -> Result<()> {
        let lib = r#"(kicad_symbol_lib
  (symbol "J" (property "Reference" "J" (at 0 0 0))
    (symbol "J_0_1"
      (text "L" (at 0 0 0) (effects (font (size 1.27 1.27)) (justify left)))
      (text "R" (at 0 2 0) (effects (font (size 1.27 1.27)) (justify right bottom)))
      (text "C" (at 0 4 0) (effects (font (size 1.27 1.27)) (justify top)))
      (text "N" (at 0 6 0) (effects (font (size 1.27 1.27)))))
    (symbol "J_1_1" (pin passive line (at 0 0 0) (length 1) (name "~") (number "1")))))"#;
        let (syms, warnings) = symbols(lib, "k", 1.0)?;
        assert_eq!(syms.len(), 1, "{warnings:?}");
        let aligns: Vec<(&str, Align)> = syms[0]
            .symbol
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Text { text, align, .. } => Some((text.as_str(), *align)),
                _ => None,
            })
            .collect();
        assert_eq!(
            aligns,
            vec![
                ("L", Align::Left),
                ("R", Align::Right),
                ("C", Align::Center),
                ("N", Align::Center)
            ]
        );
        Ok(())
    }

    #[test]
    fn filled_closed_polylines_and_rectangles_come_in_filled() -> Result<()> {
        let lib = r#"(kicad_symbol_lib
  (symbol "D" (property "Reference" "D" (at 0 0 0))
    (symbol "D_0_1"
      (polyline (pts (xy -1 1) (xy -1 -1) (xy 1 0) (xy -1 1)) (stroke (width 0.254)) (fill (type outline)))
      (polyline (pts (xy 1 1) (xy 1 -1)) (stroke (width 0.254)) (fill (type none)))
      (polyline (pts (xy 2 1) (xy 3 1) (xy 3 2) (xy 2 1)) (fill (type background)))
      (polyline (pts (xy 0 5) (xy 2 5) (xy 1 6)) (fill (type outline)))
      (polyline (pts (xy 0 8) (xy 2 8) (xy 0 8)) (fill (type none)))
      (rectangle (start 4 1) (end 5 -1) (fill (type outline)))
      (rectangle (start 6 1) (end 7 -1) (fill (type background))))
    (symbol "D_1_1"
      (pin passive line (at 0 0 0) (length 1) (name "~") (number "1"))
      (pin passive line (at 9 0 180) (length 1) (name "~") (number "2")))))"#;
        let (syms, warnings) = symbols(lib, "k", 1.0)?;
        assert_eq!(syms.len(), 1, "{warnings:?}");
        let g = &syms[0].symbol.graphics;
        // The triangle: closed, filled, three points without the repeated first one.
        assert!(
            g.contains(&Shape::Polyline {
                points: vec![Point::new(-1.0, -1.0), Point::new(-1.0, 1.0), Point::new(1.0, 0.0)],
                closed: true,
                filled: true,
            }),
            "{g:?}"
        );
        // The bar stays a line.
        assert!(g.contains(&Shape::Line {
            a: Point::new(1.0, -1.0),
            b: Point::new(1.0, 1.0)
        }));
        // An outline fill closes a triangle that does not repeat its first point.
        assert!(
            g.contains(&Shape::Polyline {
                points: vec![Point::new(0.0, -5.0), Point::new(2.0, -5.0), Point::new(1.0, -6.0)],
                closed: true,
                filled: true,
            }),
            "{g:?}"
        );
        // Three points that end where they start are a line there and back.
        assert!(g.contains(&Shape::Line {
            a: Point::new(0.0, -8.0),
            b: Point::new(2.0, -8.0)
        }));
        // A background fill is closed but hollow.
        assert!(
            g.iter()
                .any(|s| matches!(s, Shape::Polyline { points, closed: true, filled: false } if points.len() == 3))
        );
        // A filled rectangle becomes a filled polyline; a background one stays a rectangle.
        assert!(
            g.iter()
                .any(|s| matches!(s, Shape::Polyline { points, closed: true, filled: true } if points.len() == 4))
        );
        assert!(
            g.iter()
                .any(|s| matches!(s, Shape::Rect { min, .. } if (min.x - 6.0).abs() < 1e-9))
        );
        assert!(
            !g.iter()
                .any(|s| matches!(s, Shape::Rect { min, .. } if (min.x - 4.0).abs() < 1e-9))
        );
        Ok(())
    }

    #[test]
    fn a_unit_never_takes_the_id_of_another_symbol() -> Result<()> {
        let lib = r#"(kicad_symbol_lib
  (symbol "Relay"
    (symbol "Relay_1_1" (pin passive line (at 0 3 270) (length 1) (name "~") (number "A1")))
    (symbol "Relay_2_1" (pin passive line (at 0 3 270) (length 1) (name "~") (number "11"))))
  (symbol "Relay_A"
    (symbol "Relay_A_1_1" (pin passive line (at 0 3 270) (length 1) (name "~") (number "X")))))"#;
        let (syms, warnings) = symbols(lib, "k", 1.0)?;
        let ids: Vec<&str> = syms.iter().map(|s| s.symbol.id.as_str()).collect();
        assert_eq!(ids, ["k.relay_b", "k.relay_a"], "{warnings:?}");
        assert_eq!(syms[1].symbol.pins[0].name, "X", "the library's own Relay_A is kept");
        assert!(
            warnings.iter().any(|w| w.starts_with("Relay_A: its id k.relay_a")),
            "{warnings:?}"
        );
        Ok(())
    }

    #[test]
    fn texts_move_from_kicads_vertical_centre_to_their_baseline() -> Result<()> {
        let lib = r#"(kicad_symbol_lib
  (symbol "V" (property "Reference" "V" (at 0 0 0))
    (symbol "V_0_1"
      (text "mid" (at 0 -4 0) (effects (font (size 2 2))))
      (text "top" (at 0 -4 0) (effects (font (size 2 2)) (justify left top)))
      (text "bottom" (at 0 -4 0) (effects (font (size 2 2)) (justify bottom)))
      (text "turned" (at 0 -4 90) (effects (font (size 2 2))))
      (text "two\nlines" (at 0 -4 0) (effects (font (size 2 2)))))
    (symbol "V_1_1" (pin passive line (at 0 0 0) (length 1) (name "~") (number "1")))))"#;
        let (syms, warnings) = symbols(lib, "k", 1.0)?;
        assert_eq!(syms.len(), 1, "{warnings:?}");
        let at = |name: &str| {
            syms[0].symbol.graphics.iter().find_map(|g| match g {
                Shape::Text { text, at, .. } if text == name => Some(*at),
                _ => None,
            })
        };
        // KiCad's (0, -4) is (0, 4) here, y down. No vertical word: half the height down.
        assert_eq!(at("mid"), Some(Point::new(0.0, 5.0)));
        assert_eq!(at("top"), Some(Point::new(0.0, 6.0)));
        assert_eq!(at("bottom"), Some(Point::new(0.0, 4.0)));
        // A text reading bottom to top has its "down" to the right.
        let turned = at("turned").unwrap_or(Point::new(f64::NAN, f64::NAN));
        assert!(
            (turned.x - 1.0).abs() < 1e-9 && (turned.y - 4.0).abs() < 1e-9,
            "{turned:?}"
        );
        // Two lines are centred as a block: the first baseline sits above the middle.
        assert_eq!(at("two"), Some(Point::new(0.0, 3.5)));
        assert_eq!(at("lines"), Some(Point::new(0.0, 6.5)));
        Ok(())
    }

    #[test]
    fn a_text_with_line_breaks_becomes_one_text_per_line() -> Result<()> {
        let lib = r#"(kicad_symbol_lib
  (symbol "Psu"
    (symbol "Psu_0_1"
      (text "A\nB" (at 0 0 0) (effects (font (size 2 2))))
      (text "C\n\nD" (at 0 0 90) (effects (font (size 2 2))))
      (text "A	B" (at 9 9 0) (effects (font (size 2 2)))))
    (symbol "Psu_1_1" (pin passive line (at 0 3 270) (length 1) (name "~") (number "1")))))"#;
        let (syms, warnings) = symbols(lib, "k", 1.0)?;
        let texts: Vec<(&str, Point)> = syms[0]
            .symbol
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Text { text, at, .. } => Some((text.as_str(), *at)),
                _ => None,
            })
            .collect();
        let names: Vec<&str> = texts.iter().map(|t| t.0).collect();
        // A real tab inside a string (KiCad does not escape it) is dropped, not printed as `?`.
        assert_eq!(names, ["A", "B", "C", "D", "AB"], "{warnings:?}");
        assert!(texts.iter().all(|t| !t.0.chars().any(char::is_control)));
        // Horizontal text: the second line sits below the first (y runs down).
        assert!(texts[1].1.y > texts[0].1.y && (texts[1].1.x - texts[0].1.x).abs() < 1e-9);
        // Text reading bottom to top: its next line is to the right; the empty line keeps its gap.
        assert!((texts[3].1.x - texts[2].1.x - 6.0).abs() < 1e-9, "{texts:?}");
        assert!((texts[3].1.y - texts[2].1.y).abs() < 1e-9);
        Ok(())
    }

    #[test]
    fn a_text_box_comes_in_as_text_and_border_and_unknown_items_are_reported() -> Result<()> {
        let lib = r#"(kicad_symbol_lib
  (symbol "T" (property "Reference" "T" (at 0 0 0))
    (symbol "T_0_1"
      (text_box "24 V\nDC" (at 1 4 0) (size 6 -3) (stroke (width 0.254) (type default)) (fill (type none))
        (effects (font (size 2 2)) (justify left top)))
      (text_box "bare" (at 10 4 0) (size 6 3) (stroke (width -1) (type default)) (fill (type none))
        (effects (font (size 1.27 1.27))))
      (text_box "up" (at 20 4 0) (size 6 3) (stroke (width 0) (type default)) (fill (type none))
        (effects (font (size 1 1))))
      (text_box "solid" (at 30 4 0) (size 2 2) (stroke (width -1)) (fill (type outline))
        (effects (font (size 1 1))))
      (text_box "turned" (at 40 4 90) (size 3 6) (stroke (width -1)) (fill (type none))
        (effects (font (size 1 1))))
      (text_box private "hidden" (at 50 4 0) (size 6 3) (stroke (width 0)) (fill (type none))
        (effects (font (size 1 1))))
      (image (at 0 0) (data "AAAA"))
      (image (at 1 1) (data "AAAA")))
    (symbol "T_1_1"
      (unit_name "Coil")
      (pin passive line (at 0 0 0) (length 1) (name "~") (number "1")))))"#;
        let (syms, warnings) = symbols(lib, "k", 1.0)?;
        assert_eq!(syms.len(), 1, "{warnings:?}");
        let g = &syms[0].symbol.graphics;
        // KiCad writes a negative height (the box hangs below `at`, y up). The text sits one text
        // height below the box's top-left corner, left-aligned.
        assert!(
            g.contains(&Shape::Text {
                at: Point::new(1.0, -2.0),
                text: "24 V DC".into(),
                height: 2.0,
                align: Align::Left,
                angle: 0.0,
            }),
            "{g:?}"
        );
        // A visible border comes in as a rectangle; a box without one draws only its text.
        assert!(
            g.contains(&Shape::Rect {
                min: Point::new(1.0, -4.0),
                max: Point::new(7.0, -1.0),
            }),
            "{g:?}"
        );
        // A positive height puts the box above `at` (KiCad's y points up), and the text one text
        // height below the box's top edge.
        assert!(
            g.contains(&Shape::Rect {
                min: Point::new(20.0, -7.0),
                max: Point::new(26.0, -4.0),
            }),
            "{g:?}"
        );
        assert!(g.iter().any(|s| matches!(s, Shape::Text { text, at, .. }
            if text == "up" && *at == Point::new(20.0, -6.0))));
        assert_eq!(g.iter().filter(|s| matches!(s, Shape::Rect { .. })).count(), 2, "{g:?}");
        assert!(
            g.iter()
                .any(|s| matches!(s, Shape::Text { text, .. } if text == "bare"))
        );
        // An outline fill draws a filled box even without a stroke.
        assert!(
            g.contains(&Shape::Polyline {
                points: vec![
                    Point::new(30.0, -6.0),
                    Point::new(32.0, -6.0),
                    Point::new(32.0, -4.0),
                    Point::new(30.0, -4.0)
                ],
                closed: true,
                filled: true,
            }),
            "{g:?}"
        );
        // The box's angle goes to its text.
        assert!(g.iter().any(|s| matches!(s, Shape::Text { text, angle, .. }
            if text == "turned" && (*angle - 90.0).abs() < 1e-9)));
        // A private box is left out, text and border.
        assert!(
            !g.iter()
                .any(|s| matches!(s, Shape::Text { text, .. } if text == "hidden" || text == "private"))
        );
        assert_eq!(warnings, ["T: 2 image item(s) not read"]);
        Ok(())
    }

    #[test]
    fn hidden_texts_are_left_out() -> Result<()> {
        // KiCad draws a hidden text only in its symbol editor: `hide` (KiCad 6/7) or
        // `(hide yes)` (KiCad 8+) in its effects.
        let lib = r#"(kicad_symbol_lib
  (symbol "H" (property "Reference" "H" (at 0 0 0))
    (symbol "H_0_1"
      (text "old" (at 0 5 0) (effects (font (size 1 1)) hide))
      (text "new" (at 0 6 0) (effects (font (size 1 1)) (hide yes)))
      (text "hide" (at 0 7 0) (effects (font (size 1 1))))
      (text "shown" (at 0 8 0) (effects (font (size 1 1)) (hide no))))
    (symbol "H_1_1"
      (pin passive line (at 0 0 0) (length 0) (name "~") (number "1")))))"#;
        let (syms, warnings) = symbols(lib, "k", 1.0)?;
        assert_eq!(syms.len(), 1, "{warnings:?}");
        let texts: Vec<&str> = syms[0]
            .symbol
            .graphics
            .iter()
            .filter_map(|s| match s {
                Shape::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        // A text that reads "hide" is not hidden by its own content.
        assert_eq!(texts, ["hide", "shown"]);
        Ok(())
    }

    #[test]
    fn private_texts_and_shapes_are_left_out() -> Result<()> {
        // KiCad 8+: a private item shows only in KiCad's symbol editor.
        let lib = r#"(kicad_symbol_lib
  (symbol "P" (property "Reference" "P" (at 0 0 0))
    (symbol "P_0_1"
      (text private "editor note" (at 0 5 0) (effects (font (size 1 1))))
      (text "private" (at 0 8 0) (effects (font (size 1 1))))
      (polyline private (pts (xy 0 0) (xy 5 5)))
      (rectangle private (start 0 0) (end 2 2))
      (circle private (center 0 0) (radius 1))
      (arc private (start 0 1) (mid 1 0) (end 0 -1))
      (bezier private (pts (xy 0 0) (xy 1 2) (xy 3 2) (xy 4 0)))
      (polyline (pts (xy 0 0) (xy 3 0))))
    (symbol "P_1_1"
      (pin passive line (at 0 0 0) (length 0) (name "~") (number "1")))))"#;
        let (syms, warnings) = symbols(lib, "k", 1.0)?;
        assert_eq!(syms.len(), 1, "{warnings:?}");
        let g = &syms[0].symbol.graphics;
        // Only the public line and the text that reads "private" are drawn.
        assert_eq!(g.len(), 2, "{g:?}");
        assert!(g.iter().any(|s| matches!(s, Shape::Line { .. })), "{g:?}");
        // The private bezier would come in as a flattened polyline.
        assert!(!g.iter().any(|s| matches!(s, Shape::Polyline { .. })), "{g:?}");
        assert!(
            g.iter()
                .any(|s| matches!(s, Shape::Text { text, at, .. } if text == "private" && at.y < -7.0)),
            "{g:?}"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        Ok(())
    }

    #[test]
    fn a_symbol_that_fails_leaves_its_id_to_the_next() -> Result<()> {
        let lib = r#"(kicad_symbol_lib
  (symbol "A")
  (symbol "a" (symbol "a_1_1" (pin passive line (at 0 3 270) (length 1) (name "~") (number "1")))))"#;
        let (syms, warnings) = symbols(lib, "k", 1.0)?;
        let ids: Vec<&str> = syms.iter().map(|s| s.symbol.id.as_str()).collect();
        assert_eq!(ids, ["k.a"], "{warnings:?}");
        assert_eq!(syms[0].kicad_name, "a");
        Ok(())
    }

    #[test]
    fn a_single_unit_keeps_its_name_and_unit_letters_count_like_kicad() -> Result<()> {
        let lib = r#"(kicad_symbol_lib
  (symbol "Fuse" (property "Reference" "F" (at 0 0 0))
    (symbol "Fuse_0_1" (rectangle (start -1 2) (end 1 -2)))
    (symbol "Fuse_1_1" (pin passive line (at 0 3 270) (length 1) (name "~") (number "1"))
                       (pin passive line (at 0 -3 90) (length 1) (name "~") (number "2")))))"#;
        let (syms, warnings) = symbols(lib, "k", 1.0)?;
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].symbol.id, "k.fuse");
        assert_eq!(syms[0].kicad_name, "Fuse");
        assert_eq!(
            [1, 2, 26, 27, 28, 52, 53].map(unit_letter),
            ["A", "B", "Z", "AA", "AB", "AZ", "BA"]
        );
        Ok(())
    }

    #[test]
    fn hidden_pins_are_left_out_with_one_warning_naming_them() -> Result<()> {
        let lib = r#"(kicad_symbol_lib
  (symbol "U" (property "Reference" "U" (at 0 0 0))
    (symbol "U_1_1"
      (pin passive line (at 0 0 0) (length 1) (name "IN") (number "1"))
      (pin power_in line (at 0 5 270) (length 1) hide (name "VCC") (number "14"))
      (pin power_in line (at 0 -5 90) (length 1) (hide yes) (name "GND") (number "~"))))
  (symbol "V" (symbol "V_1_1" (pin passive line (at 0 0 0) (length 1) (name "~") (number "1")))))"#;
        let (syms, warnings) = symbols(lib, "k", 1.0)?;
        assert_eq!(syms.len(), 2, "{warnings:?}");
        let names: Vec<&str> = syms[0].symbol.pins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["1"]);
        // Only the pin line of the visible pin is drawn.
        assert_eq!(syms[0].symbol.graphics.len(), 1, "{:?}", syms[0].symbol.graphics);
        // The pin without a number is named by its name; symbol V has no hidden pins.
        assert_eq!(warnings, ["U: 2 hidden pins left out (14, GND)"]);
        Ok(())
    }

    #[test]
    fn a_bezier_comes_in_as_a_flattened_polyline() -> Result<()> {
        // The pin sits at 0,0 so page coordinates are KiCad's with y flipped.
        let lib = r#"(kicad_symbol_lib
  (symbol "Curve"
    (symbol "Curve_0_1"
      (bezier (pts (xy 0 0) (xy 0 4) (xy 4 4) (xy 4 0)) (stroke (width 0)) (fill (type none)))
      (bezier (pts (xy 0 0) (xy 1 1) (xy 2 2)))
      (bezier (pts (xy 0 0) (xy 1 1) (xy 2 x) (xy 3 3) (xy 4 4))))
    (symbol "Curve_1_1" (pin passive line (at 0 0 0) (length 0) (name "~") (number "1")))))"#;
        let (syms, warnings) = symbols(lib, "k", 1.0)?;
        assert_eq!(syms.len(), 1, "{warnings:?}");
        let curves: Vec<&Vec<Point>> = syms[0]
            .symbol
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Polyline {
                    points,
                    closed: false,
                    filled: false,
                } => Some(points),
                _ => None,
            })
            .collect();
        // The three-point bezier and the one with a point that does not parse are left out.
        let [points] = curves.as_slice() else {
            unreachable!("one curve expected: {:?}", syms[0].symbol.graphics)
        };
        let near = |p: Point, x: f64, y: f64| (p.x - x).abs() < 1e-9 && (p.y - y).abs() < 1e-9;
        assert!(near(points[0], 0.0, 0.0), "{points:?}");
        assert!(near(points[points.len() - 1], 4.0, 0.0), "{points:?}");
        // At t = 0.5 the curve is at (2, 3) in KiCad, (2, -3) on the page.
        assert!(points.iter().any(|p| near(*p, 2.0, -3.0)), "{points:?}");
        Ok(())
    }

    #[test]
    fn visible_fields_such_as_value_come_in_as_texts() -> Result<()> {
        let lib = r#"(kicad_symbol_lib (version 20231120) (generator "test")
  (symbol "Lamp"
    (property "Reference" "H" (at 2 2 0) (effects (font (size 1.27 1.27))))
    (property "Value" "Lamp" (at 0 -3 90) (effects (font (size 2 2)) (justify left)))
    (property "Footprint" "Lamp:Big" (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
    (property "Datasheet" "https://example.com" (at 0 0 0) (effects (font (size 1.27 1.27))) (hide yes))
    (property "Description" "A lamp" (at 0 0 0) (effects (font (size 1.27 1.27))))
    (property "ki_keywords" "light" (at 0 0 0) (effects (font (size 1.27 1.27))))
    (property "Voltage" "24 V" (at 4 1 0) (effects (font (size 1 1))))
    (property "Spare" "~" (at 0 0 0))
    (property "Note" "hide" (at 6 0 0) (effects (font (size 1 1))))
    (symbol "Lamp_1_1" (pin passive line (at 0 0 270) (length 0) (name "1") (number "1"))))
)"#;
        let (syms, warnings) = symbols(lib, "k", 1.0)?;
        let [lamp] = syms.as_slice() else {
            unreachable!("one symbol expected: {warnings:?}")
        };
        let texts: Vec<(&str, Point, f64, f64, Align)> = lamp
            .symbol
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Text {
                    text,
                    at,
                    height,
                    align,
                    angle,
                } => Some((text.as_str(), *at, *height, *angle, *align)),
                _ => None,
            })
            .collect();
        let names: Vec<&str> = texts.iter().map(|t| t.0).collect();
        // Reference, Description, KiCad's own keys, hidden fields and empty ones are not drawn.
        // A field whose text is "hide" is still shown.
        assert_eq!(names, vec!["Lamp", "24 V", "hide"], "{texts:?}");
        // Placed like a text item: the Value reads bottom to top, its baseline half its
        // height to the right of `at` (0, 3 on the page).
        let (_, at, height, angle, align) = texts[0];
        assert!((at.x - 1.0).abs() < 1e-9 && (at.y - 3.0).abs() < 1e-9, "{at:?}");
        assert!((height - 2.0).abs() < 1e-9 && (angle - 90.0).abs() < 1e-9);
        assert_eq!(align, Align::Left);
        let (_, at, height, _, align) = texts[1];
        assert!((at.x - 4.0).abs() < 1e-9 && (at.y + 0.5).abs() < 1e-9, "{at:?}");
        assert!((height - 1.0).abs() < 1e-9);
        assert_eq!(align, Align::Center);
        Ok(())
    }

    #[test]
    fn inverted_and_clock_pins_draw_their_circle_and_triangle() -> Result<()> {
        // A NAND output with its inversion circle, and a flip-flop's clock input.
        let lib = r#"(kicad_symbol_lib
  (symbol "Nand" (property "Reference" "U" (at 0 0 0))
    (symbol "Nand_1_1"
      (pin output inverted (at 10 0 180) (length 5) (name "~") (number "3" (effects (font (size 1.27 1.27)))))))
  (symbol "Clk" (property "Reference" "U" (at 0 0 0))
    (symbol "Clk_1_1"
      (pin input clock (at 0 0 0) (length 5) (name "C" (effects (font (size 1 1)))) (number "1"))))
  (symbol "Both" (property "Reference" "U" (at 0 0 0))
    (symbol "Both_1_1"
      (pin input inverted_clock (at 0 0 0) (length 5) (name "C") (number "1")))))"#;
        let (syms, warnings) = symbols(lib, "k", 1.0)?;
        assert_eq!(syms.len(), 3, "{warnings:?}");
        let near = |p: Point, x: f64, y: f64| (p.x - x).abs() < 1e-6 && (p.y - y).abs() < 1e-6;
        let graphics = |name: &str| {
            syms.iter()
                .find(|s| s.kicad_name == name)
                .map(|s| s.symbol.graphics.clone())
                .unwrap_or_default()
        };
        // The tip is the origin; the body is 5 mm to the left. The circle touches the body end
        // and the line stops at the circle.
        let g = graphics("Nand");
        assert_eq!(g.len(), 2, "{g:?}");
        assert!(
            g.iter()
                .any(|s| matches!(s, Shape::Circle { center, radius, filled: false }
                if near(*center, -4.365, 0.0) && (radius - 0.635).abs() < 1e-6)),
            "{g:?}"
        );
        assert!(
            g.iter()
                .any(|s| matches!(s, Shape::Line { a, b } if near(*a, 0.0, 0.0) && near(*b, -3.73, 0.0))),
            "{g:?}"
        );
        // The clock triangle points into the body, half the name's text size deep.
        let g = graphics("Clk");
        assert_eq!(g.len(), 2, "{g:?}");
        assert!(
            g.iter()
                .any(|s| matches!(s, Shape::Line { a, b } if near(*a, 0.0, 0.0) && near(*b, 5.0, 0.0))),
            "{g:?}"
        );
        assert!(
            g.iter()
                .any(|s| matches!(s, Shape::Polyline { points, closed: false, .. }
                if points.len() == 3 && near(points[1], 5.5, 0.0)
                    && near(points[0], 5.0, 0.5) && near(points[2], 5.0, -0.5))),
            "{g:?}"
        );
        // Both marks at once.
        let g = graphics("Both");
        assert!(g.iter().any(|s| matches!(s, Shape::Circle { .. })), "{g:?}");
        assert!(g.iter().any(|s| matches!(s, Shape::Polyline { .. })), "{g:?}");
        Ok(())
    }

    #[test]
    fn hostile_files_are_errors_not_crashes() {
        assert!(symbols("", "k", 1.0).is_err());
        assert!(symbols("(kicad_symbol_lib", "k", 1.0).is_err());
        assert!(symbols("(kicad_symbol_lib))", "k", 1.0).is_err());
        assert!(symbols("(kicad_symbol_lib (symbol \"a", "k", 1.0).is_err());
        assert!(symbols(&"(".repeat(100_000), "k", 1.0).is_err());
        assert!(symbols("(kicad_symbol_lib)", "k", f64::NAN).is_err());
        assert!(symbols("EESchema-LIBRARY Version 2.4", "k", 1.0).is_err());
        // Nonsense numbers and empty symbols are skipped with a warning.
        let r = symbols(
            "(kicad_symbol_lib (symbol \"x\" (symbol \"x_0_1\" (pin (at nan inf 0)) (arc (start 0 0) (mid 1 1) (end 2 2)))))",
            "k",
            1.0,
        );
        assert!(r.is_ok_and(|(s, w)| s.is_empty() && w.len() == 1));
    }
}

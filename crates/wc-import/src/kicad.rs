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
            if (several && reserved.contains(&id)) || !used.insert(id.clone()) {
                warnings.push(format!("{unit_name}: its id {id} is another symbol's, skipped"));
                continue;
            }
            match convert(&unit_name, sym, body, base, unit, prefix, scale) {
                Ok(symbol) => out.push(Imported {
                    symbol,
                    kicad_name: unit_name,
                }),
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

/// One unit of a symbol: the common unit's drawing and pins plus those of `wanted`.
fn convert(name: &str, sym: &Sexp, body: &Sexp, base: &str, wanted: u32, prefix: &str, scale: f64) -> Result<Symbol> {
    let id = format!("{}.{}", prefix.trim_end_matches('.'), crate::id_segment(name));
    store::check_id(&id)?;
    // KiCad y points up; the page's points down.
    let pt = |x: f64, y: f64| Point::new(x * scale, -y * scale);
    let xy = |s: &Sexp, head: &str| -> Option<Point> {
        let n = s.first(head)?;
        Some(pt(n.num(1)?, n.num(2)?))
    };
    let mut graphics = Vec::new();
    let mut pins: Vec<Pin> = Vec::new();
    for unit in body.all("symbol") {
        // Unit names end in `_<unit>_<style>`; unit 0 is common to all units.
        let (u, style) = unit_and_style(unit, base);
        if (u != 0 && u != wanted) || style > 1 {
            continue;
        }
        for g in unit.items() {
            match g.head() {
                Some("polyline") => {
                    let points: Vec<Point> = g
                        .first("pts")
                        .map(|p| p.all("xy").filter_map(|x| Some(pt(x.num(1)?, x.num(2)?))).collect())
                        .unwrap_or_default();
                    match points.as_slice() {
                        [a, b] => graphics.push(Shape::Line { a: *a, b: *b }),
                        [_, _, ..] => graphics.push(Shape::Polyline {
                            points,
                            closed: false,
                            filled: false,
                        }),
                        _ => {}
                    }
                }
                Some("rectangle") => {
                    if let (Some(a), Some(b)) = (xy(g, "start"), xy(g, "end")) {
                        graphics.push(Shape::Rect {
                            min: Point::new(a.x.min(b.x), a.y.min(b.y)),
                            max: Point::new(a.x.max(b.x), a.y.max(b.y)),
                        });
                    }
                }
                Some("circle") => {
                    if let (Some(center), Some(r)) = (xy(g, "center"), g.first("radius").and_then(|r| r.num(1))) {
                        let filled =
                            g.first("fill").and_then(|f| f.first("type")).and_then(|t| t.arg(1)) == Some("outline");
                        graphics.push(Shape::Circle {
                            center,
                            radius: r * scale,
                            filled,
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
                    if let (Some(t), Some(at)) = (g.arg(1), g.first("at")) {
                        let height = g
                            .first("effects")
                            .and_then(|e| e.first("font"))
                            .and_then(|f| f.first("size"))
                            .and_then(|s| s.num(1))
                            .unwrap_or(1.27);
                        graphics.push(Shape::Text {
                            at: pt(at.num(1).unwrap_or(0.0), at.num(2).unwrap_or(0.0)),
                            text: t.to_owned(),
                            height: height * scale,
                            align: Align::Center,
                            // KiCad stores tenths of a degree in old files and degrees now.
                            angle: at.num(3).map(|a| if a > 360.0 { a / 10.0 } else { a }).unwrap_or(0.0),
                        });
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
                    let hidden = g.items().iter().any(|i| i.atom() == Some("hide"))
                        || g.first("hide").and_then(|h| h.arg(1)) == Some("yes");
                    if hidden {
                        continue;
                    }
                    if length > 0.0 {
                        graphics.push(Shape::Line {
                            a: tip,
                            b: Point::new(tip.x + inward.x, tip.y + inward.y),
                        });
                    }
                    let text = |head: &str| {
                        g.first(head)
                            .and_then(|n| n.arg(1))
                            .filter(|v| !v.is_empty() && *v != "~")
                    };
                    let pin_name = text("number").or_else(|| text("name")).unwrap_or("").to_owned();
                    pins.push(Pin {
                        name: pin_name,
                        at: tip,
                        dir,
                    });
                }
                _ => {}
            }
        }
    }
    if graphics.is_empty() && pins.is_empty() {
        return Err(Error::Format("the symbol draws nothing".into()));
    }
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
        // rectangle, polyline (as a line), arc, circle, text, two pin lines
        assert_eq!(coil.graphics.len(), 7, "{:?}", coil.graphics);
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

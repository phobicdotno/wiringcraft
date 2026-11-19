//! Symbols from QElectroTech elements (`.elmt`, XML).
//!
//! An element is `<definition type="element" ...>` with `<names>` (one `<name lang="..">` per
//! language) and a `<description>` holding the drawing: `line`, `rect`, `ellipse`, `circle`,
//! `arc`, `polygon`, `text`, `dynamic_text`/`input` (texts filled in on the schematic) and
//! `terminal` (connection points with an orientation `n`, `e`, `s` or `w`). Coordinates are
//! screen pixels (y down) around the element's hotspot; QElectroTech's grid is 10 px. Written
//! from the published element format; no element files are copied into WiringCraft.

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use wc_core::geom::{Align, Point, Shape};
use wc_core::symbol::{Pin, PinDir, SymbolRole};
use wc_core::{Symbol, store};

use crate::{Error, Result};

/// Millimetres per QElectroTech pixel by default: the 10 px grid lands on a 5 mm grid.
pub const DEFAULT_SCALE: f64 = 0.5;

/// Largest element file read; real elements are a few kilobytes.
pub const MAX_LEN: usize = 4 << 20;
/// Most terminals read from one element; real ones have a few dozen at most.
const MAX_TERMINALS: usize = 1000;
/// Most reasons listed for values that could not be read; the rest are counted.
const MAX_REASONS: usize = 20;
/// Points used for an ellipse that is not a circle, or an elliptic arc.
const ELLIPSE_STEPS: usize = 48;

/// One imported element.
#[derive(Debug, Clone)]
pub struct Imported {
    pub symbol: Symbol,
    /// The element's name as QElectroTech shows it (English when given).
    pub qet_name: String,
}

fn attr(e: &BytesStart, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == name.as_bytes())
        .and_then(|a| a.unescape_value().ok())
        .map(|v| v.into_owned())
}

/// The number in attribute `name`. A value that is there but not a finite number is `None`
/// with its reason in `bad`.
fn num(e: &BytesStart, tag: &str, name: &str, bad: &mut Vec<String>) -> Option<f64> {
    let raw = e.attributes().flatten().find(|a| a.key.as_ref() == name.as_bytes())?;
    let v = match raw.unescape_value() {
        Ok(v) => v,
        Err(err) => {
            bad.push(format!("<{tag}> {name}: {err}"));
            return None;
        }
    };
    match v.trim().parse::<f64>() {
        Ok(n) if n.is_finite() => Some(n),
        _ => {
            bad.push(format!("<{tag}> {name} = {:?} is not a number", v.trim()));
            None
        }
    }
}

/// Every coordinate and size of `s` is a finite number.
fn finite(s: &Shape) -> bool {
    let p = |p: &Point| p.x.is_finite() && p.y.is_finite();
    match s {
        Shape::Line { a, b } => p(a) && p(b),
        Shape::Polyline { points, .. } => points.iter().all(p),
        Shape::Rect { min, max } => p(min) && p(max),
        Shape::Circle { center, radius, .. } => p(center) && radius.is_finite(),
        Shape::Arc {
            center,
            radius,
            start,
            end,
        } => p(center) && radius.is_finite() && start.is_finite() && end.is_finite(),
        Shape::Text { at, height, angle, .. } => p(at) && height.is_finite() && angle.is_finite(),
    }
}

/// `filling:...` from a QElectroTech style string; anything but `none` fills.
fn filled(e: &BytesStart) -> bool {
    attr(e, "style").is_some_and(|s| {
        s.split(';').any(|kv| {
            kv.split_once(':')
                .is_some_and(|(k, v)| k.trim() == "filling" && !matches!(v.trim(), "" | "none"))
        })
    })
}

/// Points on the ellipse in the box (`x`, `y`, `w`, `h`), from `start` sweeping `sweep` degrees
/// counter-clockwise on screen (Qt's convention, 0 at three o'clock).
fn ellipse_points(x: f64, y: f64, w: f64, h: f64, start: f64, sweep: f64) -> Vec<Point> {
    let (cx, cy, rx, ry) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
    let steps = ((ELLIPSE_STEPS as f64 * sweep.abs() / 360.0).ceil() as usize).max(2);
    (0..=steps)
        .map(|i| {
            let a = (start + sweep * i as f64 / steps as f64).to_radians();
            Point::new(cx + rx * a.cos(), cy - ry * a.sin())
        })
        .collect()
}

/// Read one `.elmt` element. The id is `<prefix>.<file stem>`-style text given by the caller,
/// coordinates are multiplied by `scale`. Returns the symbol and warnings.
pub fn element(text: &str, id: &str, scale: f64) -> Result<(Imported, Vec<String>)> {
    if !(scale.is_finite() && scale > 0.0 && scale <= 100.0) {
        return Err(Error::Format(format!("scale {scale} must be above 0 and at most 100")));
    }
    if text.len() > MAX_LEN {
        return Err(Error::Format("the element file is too large".into()));
    }
    store::check_id(id)?;

    let mut reader = Reader::from_str(text);
    let mut warnings = Vec::new();
    let mut graphics: Vec<Shape> = Vec::new();
    let mut pins: Vec<(String, Point, PinDir)> = Vec::new();
    let mut tag_at: Option<Point> = None;
    let mut names: Vec<(String, String)> = Vec::new();
    let mut link_type = String::new();
    let mut seen_definition = false;
    let mut in_description = false;
    // Open elements, to know where a `<name>` text or a dynamic text's `<info_name>` belongs.
    let mut path: Vec<String> = Vec::new();
    let mut name_lang: Option<String> = None;
    // A dynamic text waiting for its `<info_name>`: its position.
    let mut dynamic_at: Option<Point> = None;
    let mut unknown: std::collections::BTreeMap<String, usize> = Default::default();
    let mut bad: Vec<String> = Vec::new();

    loop {
        let ev = reader
            .read_event()
            .map_err(|e| Error::Format(format!("not a readable element file: {e}")))?;
        let (e, empty) = match &ev {
            Event::Start(e) => (Some(e.clone()), false),
            Event::Empty(e) => (Some(e.clone()), true),
            _ => (None, false),
        };
        if let Some(e) = e {
            let tag = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
            if path.is_empty() {
                if tag != "definition" || attr(&e, "type").is_some_and(|t| t != "element") {
                    return Err(Error::Format("not a QElectroTech element (.elmt)".into()));
                }
                seen_definition = true;
                link_type = attr(&e, "link_type").unwrap_or_default();
            }
            match (tag.as_str(), path.last().map(String::as_str)) {
                ("description", Some("definition")) => in_description = true,
                ("name", Some("names")) => name_lang = Some(attr(&e, "lang").unwrap_or_default()),
                _ if in_description && path.last().map(String::as_str) == Some("description") => {
                    shape(
                        &tag,
                        &e,
                        scale,
                        &mut graphics,
                        &mut pins,
                        &mut tag_at,
                        &mut dynamic_at,
                        &mut unknown,
                        &mut bad,
                    );
                    if pins.len() > MAX_TERMINALS {
                        return Err(Error::Format(format!("more than {MAX_TERMINALS} terminals")));
                    }
                }
                _ => {}
            }
            if !empty {
                path.push(tag);
            } else if tag == "description" {
                in_description = false;
            }
            continue;
        }
        match ev {
            Event::End(_) => {
                if let Some(tag) = path.pop() {
                    match tag.as_str() {
                        "description" => in_description = false,
                        "name" => name_lang = None,
                        "dynamic_text" => dynamic_at = None,
                        _ => {}
                    }
                }
            }
            Event::Text(t) => {
                let v = t.unescape().map(|s| s.trim().to_owned()).unwrap_or_default();
                if let Some(lang) = &name_lang {
                    if !v.is_empty() {
                        names.push((lang.clone(), v));
                    }
                } else if path.last().map(String::as_str) == Some("info_name")
                    && v == "label"
                    && let Some(at) = dynamic_at
                {
                    tag_at = Some(at);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !seen_definition {
        return Err(Error::Format("not a QElectroTech element (.elmt)".into()));
    }
    if !path.is_empty() {
        return Err(Error::Format("the file ends inside an element".into()));
    }
    for (kind, n) in unknown {
        warnings.push(format!("{n} <{kind}> item(s) not read"));
    }
    let more = bad.len().saturating_sub(MAX_REASONS);
    warnings.extend(bad.into_iter().take(MAX_REASONS));
    if more > 0 {
        warnings.push(format!("{more} more value(s) not read"));
    }
    if graphics.is_empty() && pins.is_empty() {
        return Err(Error::Format("the element draws nothing".into()));
    }
    let qet_name = names
        .iter()
        .find(|(l, _)| l == "en")
        .or_else(|| names.first())
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| id.rsplit('.').next().unwrap_or(id).to_owned());

    // Unnamed terminals are numbered in order; a repeated name gets a mark and a count
    // (`x`, `x'`, `x'2`, ...), so names stay short however often one repeats.
    let mut used = std::collections::BTreeSet::new();
    let mut repeats: std::collections::BTreeMap<String, usize> = Default::default();
    let mut out_pins: Vec<Pin> = Vec::new();
    for (i, (name, at, dir)) in pins.into_iter().enumerate() {
        let base = if name.trim().is_empty() {
            (i + 1).to_string()
        } else {
            name
        };
        let mut name = base.clone();
        while !used.insert(name.clone()) {
            let n = repeats.entry(base.clone()).or_insert(0);
            *n += 1;
            name = if *n == 1 {
                format!("{base}'")
            } else {
                format!("{base}'{n}")
            };
        }
        out_pins.push(Pin { name, at, dir });
    }
    // The first pin is the origin, as for the other importers.
    let origin = out_pins.first().map(|p| p.at).unwrap_or(Point::new(0.0, 0.0));
    let shift = Point::new(-origin.x, -origin.y);
    let snap = |v: f64| (v * 1000.0).round() / 1000.0;
    let fix = |p: Point| Point::new(snap(p.x + shift.x), snap(p.y + shift.y));
    let graphics: Vec<Shape> = graphics.iter().map(|g| g.translate(shift)).collect();
    for p in &mut out_pins {
        p.at = fix(p.at);
    }
    let tag_at = match tag_at {
        Some(at) => fix(at),
        None => {
            let xs = graphics.iter().map(|g| g.bounds().0.x);
            Point::new(snap(xs.fold(0.0, f64::min) - 3.0), 0.0)
        }
    };
    if !(graphics.iter().all(finite)
        && out_pins.iter().all(|p| p.at.x.is_finite() && p.at.y.is_finite())
        && tag_at.x.is_finite()
        && tag_at.y.is_finite())
    {
        return Err(Error::Format("coordinates are out of range".into()));
    }
    let role = match link_type.as_str() {
        "master" => SymbolRole::Main,
        "slave" => SymbolRole::Auxiliary,
        "terminal" => SymbolRole::Terminal,
        "next_report" | "previous_report" => SymbolRole::Interruption,
        _ => SymbolRole::Standalone,
    };
    let symbol = Symbol {
        source: String::new(),
        id: id.to_owned(),
        name: qet_name.clone(),
        category: String::new(),
        letter_code: String::new(),
        role,
        graphics,
        pins: out_pins,
        tag_at,
        description: String::new(),
    };
    Ok((Imported { symbol, qet_name }, warnings))
}

/// One drawing item of `<description>`.
#[allow(clippy::too_many_arguments)]
fn shape(
    tag: &str,
    e: &BytesStart,
    scale: f64,
    graphics: &mut Vec<Shape>,
    pins: &mut Vec<(String, Point, PinDir)>,
    tag_at: &mut Option<Point>,
    dynamic_at: &mut Option<Point>,
    unknown: &mut std::collections::BTreeMap<String, usize>,
    bad: &mut Vec<String>,
) {
    let pt = |x: f64, y: f64| Point::new(x * scale, y * scale);
    let bad = std::cell::RefCell::new(bad);
    let n = |k: &str| num(e, tag, k, &mut bad.borrow_mut());
    let mut skip = || *unknown.entry(tag.to_owned()).or_default() += 1;
    match tag {
        "line" => match (n("x1"), n("y1"), n("x2"), n("y2")) {
            (Some(x1), Some(y1), Some(x2), Some(y2)) => graphics.push(Shape::Line {
                a: pt(x1, y1),
                b: pt(x2, y2),
            }),
            _ => skip(),
        },
        "rect" => match (n("x"), n("y"), n("width"), n("height")) {
            (Some(x), Some(y), Some(w), Some(h)) => graphics.push(Shape::Rect {
                min: pt(x.min(x + w), y.min(y + h)),
                max: pt(x.max(x + w), y.max(y + h)),
            }),
            _ => skip(),
        },
        "ellipse" | "circle" => {
            let bx = if tag == "circle" {
                n("diameter").and_then(|d| Some((n("x")?, n("y")?, d, d)))
            } else {
                (|| Some((n("x")?, n("y")?, n("width")?, n("height")?)))()
            };
            match bx {
                Some((x, y, w, h)) if w > 0.0 && h > 0.0 => {
                    if (w - h).abs() < 1e-9 {
                        graphics.push(Shape::Circle {
                            center: pt(x + w / 2.0, y + h / 2.0),
                            radius: w / 2.0 * scale,
                            filled: filled(e),
                        });
                    } else {
                        let points = ellipse_points(x, y, w, h, 0.0, 360.0)
                            .into_iter()
                            .take(ELLIPSE_STEPS)
                            .map(|p| pt(p.x, p.y))
                            .collect();
                        graphics.push(Shape::Polyline {
                            points,
                            closed: true,
                            filled: false,
                        });
                    }
                }
                _ => skip(),
            }
        }
        "arc" => match (n("x"), n("y"), n("width"), n("height"), n("start"), n("angle")) {
            (Some(x), Some(y), Some(w), Some(h), Some(start), Some(sweep))
                if w > 0.0 && h > 0.0 && sweep.abs() > 1e-9 =>
            {
                let sweep = sweep.clamp(-360.0, 360.0);
                if (w - h).abs() < 1e-9 {
                    // Qt angles turn counter-clockwise on screen; WiringCraft's turn the other
                    // way, so negate and run from the smaller to the larger angle.
                    let (a, b) = (-start, -(start + sweep));
                    let (from, to) = (a.min(b), a.max(b));
                    let from_n = from.rem_euclid(360.0);
                    graphics.push(Shape::Arc {
                        center: pt(x + w / 2.0, y + h / 2.0),
                        radius: w / 2.0 * scale,
                        start: from_n,
                        end: from_n + (to - from),
                    });
                } else {
                    let points = ellipse_points(x, y, w, h, start, sweep)
                        .into_iter()
                        .map(|p| pt(p.x, p.y))
                        .collect();
                    graphics.push(Shape::Polyline {
                        points,
                        closed: false,
                        filled: false,
                    });
                }
            }
            _ => skip(),
        },
        "polygon" => {
            let mut points = Vec::new();
            for i in 1.. {
                match (n(&format!("x{i}")), n(&format!("y{i}"))) {
                    (Some(x), Some(y)) => points.push(pt(x, y)),
                    _ => break,
                }
                if i > 10_000 {
                    break;
                }
            }
            let closed = attr(e, "closed").is_none_or(|c| c != "false");
            match points.as_slice() {
                [a, b] if !closed => graphics.push(Shape::Line { a: *a, b: *b }),
                [_, _, ..] => graphics.push(Shape::Polyline {
                    points,
                    closed,
                    filled: false,
                }),
                _ => skip(),
            }
        }
        "text" => match (n("x"), n("y"), attr(e, "text")) {
            (Some(x), Some(y), Some(t)) if !t.trim().is_empty() => {
                // Point size from `size`, or from the Qt font string `family,size,...`.
                let size = n("size")
                    .or_else(|| attr(e, "font")?.split(',').nth(1)?.trim().parse().ok())
                    .filter(|v: &f64| v.is_finite() && *v > 0.0)
                    .unwrap_or(9.0);
                // Qt turns clockwise on screen; WiringCraft's angle turns the other way.
                let angle = (-n("rotation").unwrap_or(0.0)).rem_euclid(360.0);
                graphics.push(Shape::Text {
                    at: pt(x, y),
                    text: t,
                    height: size * 0.75 * scale,
                    align: Align::Left,
                    angle,
                });
            }
            _ => skip(),
        },
        // Texts filled in on the schematic: only the label's place is used, for the tag.
        "input" => {
            if attr(e, "tagg").as_deref() == Some("label")
                && let (Some(x), Some(y)) = (n("x"), n("y"))
            {
                *tag_at = Some(pt(x, y));
            }
        }
        "dynamic_text" => *dynamic_at = n("x").zip(n("y")).map(|(x, y)| pt(x, y)),
        "terminal" => match (n("x"), n("y")) {
            (Some(x), Some(y)) => {
                let dir = match attr(e, "orientation").as_deref() {
                    Some("n") => PinDir::Up,
                    Some("e") => PinDir::Right,
                    Some("w") => PinDir::Left,
                    _ => PinDir::Down,
                };
                pins.push((attr(e, "name").unwrap_or_default(), pt(x, y), dir));
            }
            _ => skip(),
        },
        _ => skip(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-written relay contact element in the published format.
    const CONTACT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<definition type="element" link_type="slave" width="30" height="50" hotspot_x="15" hotspot_y="25" version="0.80">
  <uuid uuid="{00000000-0000-0000-0000-000000000001}"/>
  <names>
    <name lang="fr">Contact NO</name>
    <name lang="en">NO contact</name>
  </names>
  <informations>hand-written test element</informations>
  <description>
    <line x1="0" y1="-20" x2="0" y2="-5" end1="none" end2="none" style="line-style:normal;line-weight:normal;filling:none;color:black"/>
    <line x1="-5" y1="-5" x2="0" y2="5" style="line-style:normal"/>
    <rect x="-10" y="-10" width="20" height="20" style="filling:none"/>
    <arc x="-10" y="-10" width="20" height="20" start="0" angle="90" style="filling:none"/>
    <ellipse x="-4" y="-4" width="8" height="8" style="filling:black"/>
    <ellipse x="-6" y="-3" width="12" height="6" style="filling:none"/>
    <polygon x1="0" y1="5" x2="0" y2="20" closed="false"/>
    <polygon x1="0" y1="0" x2="2" y2="0" x3="1" y3="2"/>
    <text x="6" y="0" size="6" text="13" rotation="90"/>
    <dynamic_text x="-25" y="-5" z="5" text_from="ElementInfo" font_size="9">
      <text></text>
      <info_name>label</info_name>
    </dynamic_text>
    <terminal x="0" y="-20" orientation="n" name="13"/>
    <terminal x="0" y="20" orientation="s"/>
    <mystery x="1"/>
  </description>
</definition>"#;

    #[test]
    fn reads_shapes_terminals_name_and_label_place() -> Result<()> {
        let (imp, warnings) = element(CONTACT, "qet.contact_no", DEFAULT_SCALE)?;
        let s = &imp.symbol;
        assert_eq!(imp.qet_name, "NO contact");
        assert_eq!(s.id, "qet.contact_no");
        assert_eq!(s.role, SymbolRole::Auxiliary);
        assert_eq!(warnings, vec!["1 <mystery> item(s) not read".to_owned()]);
        // Terminal 13 at the top is the origin; the second is 40 px = 20 mm below it.
        let pins: Vec<(&str, Point, PinDir)> = s.pins.iter().map(|p| (p.name.as_str(), p.at, p.dir)).collect();
        assert_eq!(
            pins,
            vec![
                ("13", Point::new(0.0, 0.0), PinDir::Up),
                ("2", Point::new(0.0, 20.0), PinDir::Down)
            ]
        );
        // 2 lines, rect, arc, circle, ellipse, open 2-point polygon (line), triangle, text
        assert_eq!(s.graphics.len(), 9, "{:?}", s.graphics);
        assert!(s.graphics.contains(&Shape::Line {
            a: Point::new(0.0, 0.0),
            b: Point::new(0.0, 7.5)
        }));
        assert!(s.graphics.contains(&Shape::Rect {
            min: Point::new(-5.0, 5.0),
            max: Point::new(5.0, 15.0)
        }));
        assert!(s.graphics.contains(&Shape::Circle {
            center: Point::new(0.0, 10.0),
            radius: 2.0,
            filled: true
        }));
        // Qt's quarter from three o'clock to twelve (counter-clockwise on screen) is 270..360
        // in WiringCraft's angles (y down).
        assert!(s.graphics.contains(&Shape::Arc {
            center: Point::new(0.0, 10.0),
            radius: 5.0,
            start: 270.0,
            end: 360.0
        }));
        let ellipse = s.graphics.iter().find_map(|g| match g {
            Shape::Polyline {
                points, closed: true, ..
            } if points.len() == ELLIPSE_STEPS => Some(points),
            _ => None,
        });
        let Some(ellipse) = ellipse else {
            return Err(Error::Format("no ellipse".into()));
        };
        // Width 12 px, height 6 px -> 6 mm by 3 mm around the centre (0, 10).
        let (min, max) = wc_core::geom::bounds_of(ellipse);
        assert!(
            (min.x + 3.0).abs() < 1e-9 && (max.x - 3.0).abs() < 1e-9,
            "{min:?} {max:?}"
        );
        assert!(
            (min.y - 8.5).abs() < 1e-9 && (max.y - 11.5).abs() < 1e-9,
            "{min:?} {max:?}"
        );
        assert!(
            s.graphics
                .iter()
                .any(|g| matches!(g, Shape::Polyline { points, closed: true, .. } if points.len() == 3))
        );
        assert!(
            s.graphics
                .iter()
                .any(|g| matches!(g, Shape::Text { text, angle, at, .. }
            if text == "13" && (*angle - 270.0).abs() < 1e-9 && *at == Point::new(3.0, 10.0)))
        );
        // The label's dynamic text gives the tag place.
        assert_eq!(s.tag_at, Point::new(-12.5, 7.5));
        Ok(())
    }

    #[test]
    fn old_elements_with_circle_and_input_read_too() -> Result<()> {
        let old = r#"<definition type="element" width="20" height="20" hotspot_x="10" hotspot_y="10">
  <names><name lang="de">Lampe</name></names>
  <description>
    <circle x="-5" y="-5" diameter="10"/>
    <input x="-15" y="0" text="" tagg="label"/>
    <terminal x="-10" y="0" orientation="w"/>
    <terminal x="10" y="0" orientation="e"/>
  </description>
</definition>"#;
        let (imp, warnings) = element(old, "qet.lamp", 1.0)?;
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(imp.qet_name, "Lampe");
        assert_eq!(imp.symbol.role, SymbolRole::Standalone);
        assert_eq!(imp.symbol.tag_at, Point::new(-5.0, 0.0));
        let dirs: Vec<PinDir> = imp.symbol.pins.iter().map(|p| p.dir).collect();
        assert_eq!(dirs, vec![PinDir::Left, PinDir::Right]);
        assert_eq!(imp.symbol.pins[1].at, Point::new(20.0, 0.0));
        assert!(imp.symbol.graphics.contains(&Shape::Circle {
            center: Point::new(10.0, 0.0),
            radius: 5.0,
            filled: false
        }));
        Ok(())
    }

    #[test]
    fn a_bad_value_keeps_its_reason_and_repeated_names_stay_short() -> Result<()> {
        let el = r#"<definition type="element"><description>
<circle x="-5" y="-5" diameter="10"/>
<line x1="oops" y1="0" x2="1" y2="1"/>
<terminal x="0" y="0" orientation="n" name="x"/>
<terminal x="0" y="10" orientation="s" name="x"/>
<terminal x="0" y="20" orientation="s" name="x"/>
</description></definition>"#;
        let (imp, warnings) = element(el, "qet.x", 1.0)?;
        assert!(
            warnings.contains(&"<line> x1 = \"oops\" is not a number".to_owned()),
            "{warnings:?}"
        );
        let names: Vec<&str> = imp.symbol.pins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["x", "x'", "x'2"]);
        Ok(())
    }

    #[test]
    fn too_many_terminals_and_huge_coordinates_are_errors() {
        let many = format!(
            "<definition type=\"element\"><description>{}</description></definition>",
            "<terminal x=\"0\" y=\"0\" name=\"x\"/>".repeat(MAX_TERMINALS + 1)
        );
        assert!(element(&many, "qet.x", 1.0).is_err_and(|e| e.to_string().contains("terminals")));
        let huge = r#"<definition type="element"><description>
<terminal x="1e308" y="0"/><terminal x="-1e308" y="0"/></description></definition>"#;
        assert!(element(huge, "qet.x", 100.0).is_err_and(|e| e.to_string().contains("out of range")));
    }

    #[test]
    fn hostile_files_are_errors_not_crashes() {
        let id = "qet.x";
        assert!(element("", id, 1.0).is_err());
        assert!(element("<svg/>", id, 1.0).is_err());
        assert!(element("<definition type=\"element\"><description>", id, 1.0).is_err());
        assert!(element("<definition type=\"element\"><description></definition>", id, 1.0).is_err());
        assert!(element("<definition type=\"element\"><description/></definition>", id, 1.0).is_err());
        assert!(element("<definition type=\"border\"/>", id, 1.0).is_err());
        assert!(element("<definition type=\"element\"/>", "bad id!", 1.0).is_err());
        assert!(element("<definition type=\"element\"/>", id, f64::NAN).is_err());
        assert!(element(&"<a>".repeat(200_000), id, 1.0).is_err());
        // Broken numbers are skipped with a warning; nothing left to draw is an error.
        let r = element(
            "<definition type=\"element\"><description><line x1=\"nan\" y1=\"0\" x2=\"1\" y2=\"1\"/><arc x=\"0\" y=\"0\" width=\"-1\" height=\"2\" start=\"0\" angle=\"9\"/></description></definition>",
            id,
            1.0,
        );
        assert!(r.is_err());
    }
}

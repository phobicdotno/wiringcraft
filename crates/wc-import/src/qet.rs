//! Symbols from QElectroTech elements (`.elmt`, XML).
//!
//! An element is `<definition type="element" ...>` with `<names>` (one `<name lang="..">` per
//! language) and a `<description>` holding the drawing: `line`, `rect`, `ellipse`, `circle`,
//! `arc`, `polygon`, `text`, `dynamic_text`/`input` (texts filled in on the schematic) and
//! `terminal` (connection points with an orientation `n`, `e`, `s` or `w`). Coordinates are
//! screen pixels (y down) around the element's hotspot; QElectroTech's grid is 10 px. Written
//! from the published element format; no element files are copied into WiringCraft. The
//! symbol's description comes from `<elementInformations>`, the entry named `description`, and
//! its letter code from the entry named `label` when that reads as a reference such as `-K1`.

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
/// Segments in each quarter-ellipse corner of a rounded rectangle.
const CORNER_STEPS: usize = 6;

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
        Shape::Dimension { a, b, offset, height } => p(a) && p(b) && offset.is_finite() && height.is_finite(),
    }
}

/// Whether a rectangle, ellipse, circle or polygon comes in solid (#875, #924): WiringCraft fills in the line colour, so
/// only a dark filling is kept. `white`, light grey and the hatch patterns stay hollow, as does
/// `none`, since drawing them solid in the line colour would hide what the element shows.
fn solid_fill(e: &BytesStart) -> bool {
    attr(e, "style").is_some_and(|s| {
        s.split(';').any(|kv| {
            kv.split_once(':').is_some_and(|(k, v)| {
                k.trim() == "filling"
                    && !matches!(
                        v.trim().to_ascii_lowercase().as_str(),
                        "" | "none" | "white" | "lightgray" | "hor" | "ver" | "bdiag" | "fdiag"
                    )
            })
        })
    })
}

/// A closed outline, plus a fill under it when `fill` is set. QElectroTech draws a filled shape
/// with its outline, while WiringCraft draws a filled polyline without one, so both are kept.
fn push_closed(graphics: &mut Vec<Shape>, points: Vec<Point>, fill: bool) {
    if fill {
        graphics.push(Shape::Polyline {
            points: points.clone(),
            closed: true,
            filled: true,
        });
    }
    graphics.push(Shape::Polyline {
        points,
        closed: true,
        filled: false,
    });
}

/// The end shape QElectroTech draws at `tip` of a line coming from `from` (`end1`/`end2`), sized
/// as QElectroTech sizes it from `len` (`lengthN`): `simple` is an open arrow and `triangle` a
/// filled one, both `len` long and `len` to each side; `circle` is a dot of radius `len` touching
/// the end; `diamond` reaches `2 * len` along the line and `len` to each side. `none` and unknown
/// values add nothing. Returns how far the line's shaft is cut back from `tip`, so that it stops
/// at the marker's inner edge, as QElectroTech draws it (0 for `simple`, `none` and unknown).
fn line_end(graphics: &mut Vec<Shape>, kind: &str, tip: Point, from: Point, len: f64, fill: bool) -> f64 {
    let (dx, dy) = (tip.x - from.x, tip.y - from.y);
    let d = dx.hypot(dy);
    if !(d > 1e-9 && len.is_finite() && len > 0.0) {
        return 0.0;
    }
    // Unit vector out of the line end, and one across it.
    let (ux, uy) = (dx / d, dy / d);
    let at = |along: f64, across: f64| Point::new(tip.x - ux * along - uy * across, tip.y - uy * along + ux * across);
    match kind {
        "simple" => {
            graphics.push(Shape::Line {
                a: at(len, len),
                b: tip,
            });
            graphics.push(Shape::Line {
                a: at(len, -len),
                b: tip,
            });
            0.0
        }
        "triangle" => {
            graphics.push(Shape::Polyline {
                points: vec![tip, at(len, len), at(len, -len)],
                closed: true,
                filled: true,
            });
            len
        }
        "circle" => {
            graphics.push(Shape::Circle {
                center: at(len, 0.0),
                radius: len,
                filled: fill,
            });
            2.0 * len
        }
        "diamond" => {
            graphics.push(Shape::Polyline {
                points: vec![tip, at(len, len), at(2.0 * len, 0.0), at(len, -len)],
                closed: true,
                filled: false,
            });
            2.0 * len
        }
        _ => 0.0,
    }
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

/// The outline of the rectangle from (`x0`, `y0`) to (`x1`, `y1`) with quarter-ellipse corners of
/// radii `rx` and `ry` (both above 0 and at most half the side), clockwise on screen from the
/// top-left corner. Straight sides join the corners.
fn rounded_rect_points(x0: f64, y0: f64, x1: f64, y1: f64, rx: f64, ry: f64) -> Vec<Point> {
    // Corner centres and the screen angle (y down) each quarter starts at.
    let corners = [
        (x0 + rx, y0 + ry, 180.0),
        (x1 - rx, y0 + ry, 270.0),
        (x1 - rx, y1 - ry, 0.0),
        (x0 + rx, y1 - ry, 90.0),
    ];
    let mut points: Vec<Point> = Vec::with_capacity(4 * (CORNER_STEPS + 1));
    for (cx, cy, start) in corners {
        for i in 0..=CORNER_STEPS {
            let a = (start + 90.0 * i as f64 / CORNER_STEPS as f64).to_radians();
            let p = Point::new(cx + rx * a.cos(), cy + ry * a.sin());
            // A radius of half the side makes two corners meet; keep the point once.
            if points
                .last()
                .is_none_or(|q| (q.x - p.x).abs() > 1e-9 || (q.y - p.y).abs() > 1e-9)
            {
                points.push(p);
            }
        }
    }
    if points.len() > 1
        && points
            .first()
            .zip(points.last())
            .is_some_and(|(a, b)| (a.x - b.x).abs() <= 1e-9 && (a.y - b.y).abs() <= 1e-9)
    {
        points.pop();
    }
    points
}

/// The letter code in an element's default label: `-K1` and `K1` give `K`, `-PG1` gives `PG`.
/// QElectroTech's label is free text, so only a label of an optional `-` (the product sign of
/// IEC 81346), one to three letters and then digits counts; a name such as `SICK UM 30` or a
/// formula such as `%prefix` gives no letter code.
fn letter_code(label: &str) -> String {
    let l = label.trim();
    let l = l.strip_prefix('-').unwrap_or(l);
    let letters = l.chars().take_while(|c| c.is_ascii_alphabetic()).count();
    match (l.get(..letters), l.get(letters..)) {
        (Some(code), Some(number))
            if (1..=3).contains(&letters) && !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) =>
        {
            code.to_ascii_uppercase()
        }
        _ => String::new(),
    }
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
    // The element's `<elementInformation>` texts named `description` and `label`, and which one is open.
    let mut description = String::new();
    let mut label = String::new();
    let mut in_info: Option<&'static str> = None;
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
                // An empty `<name/>` has no end tag to close it, so it opens nothing.
                ("name", Some("names")) if !empty => name_lang = Some(attr(&e, "lang").unwrap_or_default()),
                ("elementInformation", Some("elementInformations")) if path.len() == 2 && !empty => {
                    in_info = match attr(&e, "name").as_deref() {
                        Some("description") => Some("description"),
                        Some("label") => Some("label"),
                        _ => None,
                    };
                }
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
                        "elementInformation" => in_info = None,
                        "dynamic_text" => dynamic_at = None,
                        _ => {}
                    }
                }
            }
            // An information is kept whole (text around a comment, CDATA) and trimmed once at the end.
            Event::Text(t) if in_info.is_some() => {
                let (info, into) = match in_info {
                    Some("label") => ("label", &mut label),
                    _ => ("description", &mut description),
                };
                match t.unescape() {
                    Ok(v) => into.push_str(&v),
                    Err(e) => warnings.push(format!(
                        "the element's {info} has text that cannot be read ({e}); it is left out"
                    )),
                }
            }
            Event::CData(t) if in_info.is_some() => match in_info {
                Some("label") => label.push_str(&String::from_utf8_lossy(&t)),
                _ => description.push_str(&String::from_utf8_lossy(&t)),
            },
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
    // (`x`, `x'`, `x'2`, ...), so names stay short however often one repeats. The names the
    // file gives are taken first, so an unnamed terminal's number never renames a named one.
    // Spaces around a name are dropped, so ` 13 ` is the same terminal name as `13`.
    let mut used = std::collections::BTreeSet::new();
    let mut repeats: std::collections::BTreeMap<String, usize> = Default::default();
    let mut unique = |base: String| {
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
        name
    };
    let named: Vec<Option<String>> = pins
        .iter()
        .map(|(name, ..)| (!name.trim().is_empty()).then(|| unique(name.trim().to_owned())))
        .collect();
    let mut out_pins: Vec<Pin> = Vec::new();
    for (i, ((_, at, dir), name)) in pins.into_iter().zip(named).enumerate() {
        let name = name.unwrap_or_else(|| unique((i + 1).to_string()));
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
        letter_code: letter_code(&label),
        role,
        graphics,
        pins: out_pins,
        tag_at,
        description: description.trim().to_owned(),
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
            (Some(x1), Some(y1), Some(x2), Some(y2)) => {
                let (a, b) = (pt(x1, y1), pt(x2, y2));
                let (mut cut_a, mut cut_b) = (0.0, 0.0);
                for (i, tip, from, cut) in [(1, a, b, &mut cut_a), (2, b, a, &mut cut_b)] {
                    let Some(kind) = attr(e, &format!("end{i}")) else {
                        continue;
                    };
                    let len = n(&format!("length{i}")).unwrap_or(1.5) * scale;
                    *cut = line_end(graphics, &kind, tip, from, len, solid_fill(e));
                }
                // The shaft stops at the inner edge of the end shapes; none is left when they
                // meet or overlap.
                let d = (b.x - a.x).hypot(b.y - a.y);
                if cut_a == 0.0 && cut_b == 0.0 {
                    graphics.push(Shape::Line { a, b });
                } else if cut_a + cut_b < d {
                    let (ux, uy) = ((b.x - a.x) / d, (b.y - a.y) / d);
                    graphics.push(Shape::Line {
                        a: Point::new(a.x + ux * cut_a, a.y + uy * cut_a),
                        b: Point::new(b.x - ux * cut_b, b.y - uy * cut_b),
                    });
                }
            }
            _ => skip(),
        },
        "rect" => match (n("x"), n("y"), n("width"), n("height")) {
            (Some(x), Some(y), Some(w), Some(h)) => {
                let (x0, y0, x1, y1) = (x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h));
                // QElectroTech 0.8+ saves corner radii as `rx`/`ry` in absolute element units
                // (it draws them with Qt's `drawRoundedRect` in absolute size mode), and Qt keeps
                // each one to at most half the side, so they are clamped the same way here.
                // A radius of 0 or less means no rounding, as in Qt.
                let rx = n("rx").unwrap_or(0.0).min((x1 - x0) / 2.0);
                let ry = n("ry").unwrap_or(0.0).min((y1 - y0) / 2.0);
                if rx > 0.0 && ry > 0.0 {
                    let points = rounded_rect_points(x0, y0, x1, y1, rx, ry)
                        .into_iter()
                        .map(|p| pt(p.x, p.y))
                        .collect();
                    push_closed(graphics, points, solid_fill(e));
                    return;
                }
                let (min, max) = (pt(x0, y0), pt(x1, y1));
                if solid_fill(e) {
                    // WiringCraft's rectangle has no fill, so a solid one also gets a filled
                    // closed polyline under its outline, as `push_closed` does for polygons.
                    graphics.push(Shape::Polyline {
                        points: vec![min, Point::new(max.x, min.y), max, Point::new(min.x, max.y)],
                        closed: true,
                        filled: true,
                    });
                }
                graphics.push(Shape::Rect { min, max });
            }
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
                            filled: solid_fill(e),
                        });
                    } else {
                        let points = ellipse_points(x, y, w, h, 0.0, 360.0)
                            .into_iter()
                            .take(ELLIPSE_STEPS)
                            .map(|p| pt(p.x, p.y))
                            .collect();
                        push_closed(graphics, points, solid_fill(e));
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
            // A filled shape has no open side, so it is drawn closed whatever `closed` says.
            let fill = solid_fill(e);
            match points.as_slice() {
                [a, b] if !closed => graphics.push(Shape::Line { a: *a, b: *b }),
                [_, _, ..] if closed || fill => push_closed(graphics, points, fill),
                [_, _, ..] => graphics.push(Shape::Polyline {
                    points,
                    closed: false,
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
                // `x`/`y` is the left end of the baseline, unless `anchor="alignment"` makes it the
                // point `Halignment` names (centre or right end; absent is left). Without the
                // anchor, `Halignment` only lines up the lines of a multi-line text, so the text
                // stays left-aligned at its left end. `Valignment` does not move the point, which
                // stays on the baseline.
                let align = match attr(e, "Halignment").as_deref() {
                    _ if attr(e, "anchor").as_deref() != Some("alignment") => Align::Left,
                    Some("AlignHCenter") => Align::Center,
                    Some("AlignRight") => Align::Right,
                    _ => Align::Left,
                };
                graphics.push(Shape::Text {
                    at: pt(x, y),
                    text: t,
                    height: size * 0.75 * scale,
                    align,
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
    fn description_comes_from_the_element_information() -> Result<()> {
        let el = |infos: &str| {
            format!(
                r#"<definition type="element"><names><name lang="en">K</name></names>
<elementInformations>{infos}</elementInformations>
<informations>author notes</informations>
<description><line x1="0" y1="0" x2="10" y2="0"/></description></definition>"#
            )
        };
        let with = el(
            r#"<elementInformation show="1" name="manufacturer">ACME</elementInformation>
<elementInformation show="1" name="description"> Relay coil &amp; contact </elementInformation>"#,
        );
        let (imp, _) = element(&with, "q.k", DEFAULT_SCALE)?;
        assert_eq!(imp.symbol.description, "Relay coil & contact");
        // Text around a comment keeps its spaces, and CDATA counts as text.
        let split = el(
            r#"<elementInformation name="description"> Relay <!-- x --> coil <![CDATA[& <contact>]]> </elementInformation>"#,
        );
        let (imp, warnings) = element(&split, "q.k", DEFAULT_SCALE)?;
        assert_eq!(imp.symbol.description, "Relay  coil & <contact>");
        assert!(warnings.is_empty(), "{warnings:?}");
        // Text that cannot be decoded is left out with a warning that says why.
        let bad = el(r#"<elementInformation name="description">Relay &bogus; coil</elementInformation>"#);
        let (imp, warnings) = element(&bad, "q.k", DEFAULT_SCALE)?;
        assert_eq!(imp.symbol.description, "");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("description") && w.contains("bogus")),
            "{warnings:?}"
        );
        for infos in [
            "",
            r#"<elementInformation show="1" name="description"/>"#,
            r#"<elementInformation show="1" name="designation">Coil</elementInformation>"#,
        ] {
            let (imp, _) = element(&el(infos), "q.k", DEFAULT_SCALE)?;
            assert_eq!(imp.symbol.description, "", "{infos}");
        }
        Ok(())
    }

    #[test]
    fn letter_code_comes_from_the_label_information() -> Result<()> {
        let el = |infos: &str| {
            format!(
                r#"<definition type="element"><elementInformations>{infos}</elementInformations>
<description><line x1="0" y1="0" x2="10" y2="0"/></description></definition>"#
            )
        };
        for (label, code) in [
            ("-K1", "K"),
            (" -PG1 ", "PG"),
            ("A", ""),
            ("-PG", ""),
            ("q12", "Q"),
            ("<![CDATA[-F]]>3", "F"),
            ("SICK UM 30", ""),
            ("Motor", ""),
            ("%prefix%autonum", ""),
            ("-K1.2", ""),
            ("", ""),
        ] {
            let infos = format!(
                r#"<elementInformation show="1" name="description">Relay</elementInformation>
<elementInformation show="1" name="label">{label}</elementInformation>"#
            );
            let (imp, warnings) = element(&el(&infos), "q.k", DEFAULT_SCALE)?;
            assert_eq!(imp.symbol.letter_code, code, "{label}");
            assert_eq!(imp.symbol.description, "Relay", "{label}");
            assert!(warnings.is_empty(), "{label}: {warnings:?}");
        }
        // No label information, or an empty one, gives no letter code.
        for infos in ["", r#"<elementInformation name="label"/>"#] {
            let (imp, _) = element(&el(infos), "q.k", DEFAULT_SCALE)?;
            assert_eq!(imp.symbol.letter_code, "", "{infos}");
        }
        Ok(())
    }

    #[test]
    fn text_alignment_follows_halignment_with_the_alignment_anchor() -> Result<()> {
        let align = |attrs: &str| -> Result<Align> {
            let el = format!(
                r#"<definition type="element"><description><text x="10" y="4" text="K" size="9" {attrs}/></description></definition>"#
            );
            let (imp, _) = element(&el, "q.k", DEFAULT_SCALE)?;
            let Some(Shape::Text { align, at, .. }) = imp.symbol.graphics.first() else {
                unreachable!("{:?}", imp.symbol.graphics)
            };
            // The point stays where the file puts it, whatever the alignment.
            assert_eq!((at.x, at.y), (5.0, 2.0), "{attrs}");
            Ok(*align)
        };
        assert_eq!(align("")?, Align::Left);
        assert_eq!(align(r#"Halignment="AlignHCenter" anchor="alignment""#)?, Align::Center);
        assert_eq!(
            align(r#"Halignment="AlignRight" anchor="alignment" Valignment="AlignBottom""#)?,
            Align::Right
        );
        assert_eq!(align(r#"Halignment="AlignLeft" anchor="alignment""#)?, Align::Left);
        assert_eq!(align(r#"anchor="alignment""#)?, Align::Left);
        // Without the anchor, x/y is the text's left end, so it stays left-aligned there.
        assert_eq!(align(r#"Halignment="AlignRight""#)?, Align::Left);
        assert_eq!(
            align(r#"Halignment="AlignHCenter" Valignment="AlignVCenter""#)?,
            Align::Left
        );
        Ok(())
    }

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
    fn filled_polygons_and_ellipses_come_in_filled() -> Result<()> {
        let el = r#"<definition type="element"><description>
<polygon x1="0" y1="0" x2="4" y2="0" x3="2" y3="4" closed="false" style="filling:black"/>
<polygon x1="10" y1="0" x2="14" y2="0" x3="12" y3="4" style="filling:none"/>
<ellipse x="-6" y="-3" width="12" height="6" style="filling:black"/>
<ellipse x="-6" y="7" width="12" height="6" style="filling:none"/>
<polygon x1="20" y1="0" x2="24" y2="0" x3="22" y3="4" style="filling:white"/>
<ellipse x="-6" y="17" width="12" height="6" style="filling:white"/>
<terminal x="0" y="0" orientation="n"/>
</description></definition>"#;
        let (imp, _) = element(el, "qet.filled", 1.0)?;
        let polys: Vec<(usize, bool, bool)> = imp
            .symbol
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Polyline { points, closed, filled } => Some((points.len(), *closed, *filled)),
                _ => None,
            })
            .collect();
        assert_eq!(
            polys,
            // A filled shape comes in as its fill plus its outline.
            vec![
                (3, true, true),
                (3, true, false),
                (3, true, false),
                (ELLIPSE_STEPS, true, true),
                (ELLIPSE_STEPS, true, false),
                (ELLIPSE_STEPS, true, false),
                (3, true, false),
                (ELLIPSE_STEPS, true, false)
            ]
        );
        Ok(())
    }

    #[test]
    fn line_ends_come_in_as_arrows_dots_and_diamonds() -> Result<()> {
        let el = r#"<definition type="element"><description>
<line x1="0" y1="0" x2="10" y2="0" end1="none" end2="triangle" length2="2" style="filling:none"/>
<line x1="0" y1="10" x2="10" y2="10" end1="simple" end2="none"/>
<line x1="0" y1="20" x2="0" y2="40" end1="circle" end2="diamond" length1="4" style="filling:black"/>
<line x1="0" y1="50" x2="10" y2="50" end1="none" end2="wiggle"/>
<line x1="0" y1="60" x2="3" y2="60" end1="circle" end2="none"/>
<terminal x="0" y="0" orientation="n"/>
</description></definition>"#;
        let (imp, warnings) = element(el, "qet.ends", 1.0)?;
        assert!(warnings.is_empty(), "{warnings:?}");
        let g = &imp.symbol.graphics;
        let close = |a: Point, b: Point| (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9;
        // The filled arrow: tip at (x2, y2), 2 long and 2 to each side.
        let tris: Vec<&Vec<Point>> = g
            .iter()
            .filter_map(|s| match s {
                Shape::Polyline {
                    points, filled: true, ..
                } => Some(points),
                _ => None,
            })
            .collect();
        let [tri] = tris.as_slice() else {
            unreachable!("one filled arrow: {g:?}")
        };
        assert_eq!(tri.len(), 3);
        assert!(close(tri[0], Point::new(10.0, 0.0)), "{tri:?}");
        assert!(
            tri[1..]
                .iter()
                .all(|p| (p.x - 8.0).abs() < 1e-9 && (p.y.abs() - 2.0).abs() < 1e-9)
        );
        // The open arrow: two short lines meeting at (x1, y1), 1.5 long and 1.5 to each side.
        let tip = Point::new(0.0, 10.0);
        let arms: Vec<Point> = g
            .iter()
            .filter_map(|s| match s {
                Shape::Line { a, b } if close(*b, tip) => Some(*a),
                _ => None,
            })
            .collect();
        assert!(
            arms.len() == 2
                && arms
                    .iter()
                    .all(|a| close(*a, Point::new(1.5, 8.5)) || close(*a, Point::new(1.5, 11.5))),
            "{arms:?}"
        );
        // A dot of radius 4 touching (x1, y1), filled as the style says, and a hollow diamond
        // reaching 2 * 1.5 back from (x2, y2).
        assert!(
            g.iter()
                .any(|s| matches!(s, Shape::Circle { center, radius, filled: true }
            if close(*center, Point::new(0.0, 24.0)) && (*radius - 4.0).abs() < 1e-9))
        );
        assert!(g.iter().any(|s| matches!(s, Shape::Polyline { points, closed: true, filled: false }
            if points.len() == 4 && close(points[0], Point::new(0.0, 40.0)) && close(points[2], Point::new(0.0, 37.0)))));
        // The shafts stop at the inner edge of the markers; `simple`, `none` and unknown ends
        // keep theirs.
        for shaft in [
            (Point::new(0.0, 0.0), Point::new(8.0, 0.0)),
            (Point::new(0.0, 10.0), Point::new(10.0, 10.0)),
            (Point::new(0.0, 28.0), Point::new(0.0, 37.0)),
            (Point::new(0.0, 50.0), Point::new(10.0, 50.0)),
        ] {
            assert!(
                g.iter()
                    .any(|s| matches!(s, Shape::Line { a, b } if close(*a, shaft.0) && close(*b, shaft.1))),
                "{shaft:?} in {g:?}"
            );
        }
        // Four shafts, two arrow arms, one arrow, two dots, one diamond: the dot on the last
        // line covers all of it, so no shaft is left there.
        assert_eq!(g.len(), 10, "{g:?}");
        Ok(())
    }

    #[test]
    fn light_filled_circles_and_rects_stay_hollow_dark_ones_fill() -> Result<()> {
        let el = r#"<definition type="element"><description>
<circle x="0" y="0" diameter="4" style="filling:white"/>
<ellipse x="10" y="0" width="4" height="4" style="filling:black"/>
<ellipse x="20" y="0" width="4" height="4" style="filling:hor"/>
<rect x="0" y="10" width="4" height="2" style="filling:black"/>
<rect x="10" y="10" width="4" height="2" style="filling:white"/>
<terminal x="0" y="0" orientation="n"/>
</description></definition>"#;
        let (imp, _) = element(el, "qet.fills", 1.0)?;
        let circles: Vec<bool> = imp
            .symbol
            .graphics
            .iter()
            .filter_map(|g| match g {
                Shape::Circle { filled, .. } => Some(*filled),
                _ => None,
            })
            .collect();
        assert_eq!(circles, vec![false, true, false]);
        let rects: Vec<&Shape> = imp
            .symbol
            .graphics
            .iter()
            .filter(|g| matches!(g, Shape::Rect { .. } | Shape::Polyline { .. }))
            .collect();
        let (a, b) = (Point::new(0.0, 10.0), Point::new(4.0, 12.0));
        assert_eq!(
            rects,
            vec![
                // The black rectangle: its fill, then its outline.
                &Shape::Polyline {
                    points: vec![a, Point::new(4.0, 10.0), b, Point::new(0.0, 12.0)],
                    closed: true,
                    filled: true
                },
                &Shape::Rect { min: a, max: b },
                // The white one stays a plain rectangle.
                &Shape::Rect {
                    min: Point::new(10.0, 10.0),
                    max: Point::new(14.0, 12.0)
                }
            ]
        );
        Ok(())
    }

    #[test]
    fn rounded_rects_come_in_as_closed_polylines_with_round_corners() -> Result<()> {
        let el = r#"<definition type="element"><description>
<rect x="0" y="0" width="20" height="10" rx="2" ry="2" style="filling:none"/>
<rect x="0" y="20" width="20" height="10" style="filling:none"/>
<rect x="0" y="40" width="20" height="10" rx="0" ry="0" style="filling:none"/>
<rect x="0" y="60" width="4" height="10" rx="50" ry="3" style="filling:black"/>
<rect x="0" y="80" width="20" height="10" rx="-2" ry="2" style="filling:none"/>
<terminal x="0" y="0" orientation="n"/>
</description></definition>"#;
        let (imp, _) = element(el, "qet.rounded", 1.0)?;
        let g = &imp.symbol.graphics;
        let Some(Shape::Polyline {
            points,
            closed: true,
            filled: false,
        }) = g.first()
        else {
            unreachable!("expected a closed polyline, got {:?}", g.first())
        };
        let (mut lo, mut hi) = (Point::new(f64::MAX, f64::MAX), Point::new(f64::MIN, f64::MIN));
        for p in points {
            (lo.x, lo.y, hi.x, hi.y) = (lo.x.min(p.x), lo.y.min(p.y), hi.x.max(p.x), hi.y.max(p.y));
        }
        assert!(lo.x.abs() < 1e-9 && lo.y.abs() < 1e-9);
        assert!((hi.x - 20.0).abs() < 1e-9 && (hi.y - 10.0).abs() < 1e-9);
        for corner in [(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)] {
            assert!(
                !points
                    .iter()
                    .any(|p| (p.x - corner.0).abs() < 1e-6 && (p.y - corner.1).abs() < 1e-6),
                "a point sits on the square corner {corner:?}"
            );
        }
        // No radii, or radii of 0: still a plain rectangle.
        assert_eq!(
            g[1],
            Shape::Rect {
                min: Point::new(0.0, 20.0),
                max: Point::new(20.0, 30.0)
            }
        );
        assert_eq!(
            g[2],
            Shape::Rect {
                min: Point::new(0.0, 40.0),
                max: Point::new(20.0, 50.0)
            }
        );
        // A radius past half the side is clamped to it, and a filled one gets its fill.
        let (
            Shape::Polyline {
                points: fill,
                filled: true,
                ..
            },
            Shape::Polyline { points: line, .. },
        ) = (&g[3], &g[4])
        else {
            unreachable!("expected a fill and an outline, got {:?}", &g[3..])
        };
        assert_eq!(fill, line);
        assert!(line.iter().all(|p| (0.0..=4.0 + 1e-9).contains(&p.x)));
        assert!(
            line.iter()
                .any(|p| (p.x - 2.0).abs() < 1e-9 && (p.y - 60.0).abs() < 1e-9)
        );
        // A negative radius means no rounding, as in Qt.
        assert_eq!(
            g[5],
            Shape::Rect {
                min: Point::new(0.0, 80.0),
                max: Point::new(20.0, 90.0)
            }
        );
        assert_eq!(g.len(), 6);
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
    fn an_empty_name_does_not_take_the_texts_after_it() -> Result<()> {
        let el = r#"<definition type="element" width="20" height="20" hotspot_x="10" hotspot_y="10">
  <names><name lang="fr"/></names>
  <informations>hand-written test element</informations>
  <description>
    <circle x="-5" y="-5" diameter="10"/>
    <dynamic_text x="-25" y="-5" text_from="ElementInfo"><text></text><info_name>label</info_name></dynamic_text>
    <terminal x="-10" y="0" orientation="w"/>
  </description>
</definition>"#;
        let (imp, _) = element(el, "qet.lamp", 1.0)?;
        assert_eq!(imp.qet_name, "lamp");
        assert_eq!(imp.symbol.tag_at, Point::new(-15.0, -5.0));
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
    fn a_named_terminal_keeps_its_name_when_an_unnamed_one_comes_first() -> Result<()> {
        let el = r#"<definition type="element"><description>
<line x1="0" y1="0" x2="0" y2="30"/>
<terminal x="0" y="0" orientation="n"/>
<terminal x="0" y="10" orientation="s" name="1"/>
<terminal x="0" y="20" orientation="s" name="2"/>
<terminal x="0" y="30" orientation="s"/>
</description></definition>"#;
        let (imp, _) = element(el, "qet.x", 1.0)?;
        let names: Vec<&str> = imp.symbol.pins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["1'", "1", "2", "4"]);
        Ok(())
    }

    #[test]
    fn terminal_names_lose_surrounding_spaces() -> Result<()> {
        let el = r#"<definition type="element"><description>
<line x1="0" y1="0" x2="0" y2="20"/>
<terminal x="0" y="0" orientation="n" name=" 13 "/>
<terminal x="0" y="10" orientation="s" name="13"/>
<terminal x="0" y="20" orientation="s" name="   "/>
</description></definition>"#;
        let (imp, _) = element(el, "qet.x", 1.0)?;
        let names: Vec<&str> = imp.symbol.pins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["13", "13'", "3"]);
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

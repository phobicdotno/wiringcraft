//! EPLAN window macros (`.ema`) drawn as WiringCraft symbols.
//!
//! A macro is XML: an `EplanPxfRoot` with one `MacroVariant` per representation type and
//! variant. Objects and their attributes have numbered names. What was worked out from real
//! maker macros, and is read here:
//!
//! - `MacroVariant`: `MacroFuncType` is the representation type (1 multi-line, 2 single-line,
//!   4 overview, 6 panel layout, 10 3D mounting layout; `DocumentType` is the kind of page the
//!   macro was made on, and a panel layout can be made on a schematic page), `VariantId` the
//!   variant (0 is A), and `ReferencePoint` the macro's insertion point (`x/y`, millimetres,
//!   y up).
//! - `ESymLib`: the symbol libraries the variant uses, by `ID`.
//! - Graphics, nested in any depth of groups: `O31` a line (`A531` to `A532`), `O34` a polyline
//!   (`A621`, points separated by `;`), `O89` a rectangle (corners `A1651`, `A1652`), `O32` an
//!   arc or circle (centre `A555`, radius the first number of `A556`, from angle `A553` to
//!   `A554` in radians, counter-clockwise) and `O30` a text (`A501` its point, `A511` its text in
//!   several languages, its child `S54x505` the height `A961` and the alignment `A966`).
//! - `O17`, a symbol placed from a library: library `A1261`, symbol `A1262`, variant `A1263`
//!   (0 to 3 turn it in quarter steps, 4 to 7 also mirror it), function category `A1381` and
//!   its refinement `A1382`, connection point designations `A185` (separated by `;`), its point
//!   in the child `S40x1201` (`A762`), and its device tag letter `P20013` and counting number
//!   `P20014` in a property child. A macro of several devices (a circuit) leads each pin name
//!   with its device's tag: `K1:A1`.
//!   A black box (category 301) has its outline in the child `S89x5`: corners `A1651` and
//!   `A1652` relative to its point.
//!
//! The multi-line representation, variant A, becomes the symbol: its graphics, black box
//! outlines, and a pin for each connection point. A placement with one connection point is a
//! device or PLC connection point and becomes one pin, facing up in variant 0 and a quarter turn
//! further for each variant. EPLAN keeps the drawing of library symbols in its own libraries,
//! not in the macro, so a placement with several connection points is drawn with the
//! WiringCraft symbol of the same kind ([`crate::eplan::drawing::symbol_for`]), first
//! connection point at its point, as the EPLAN project import does. What is left out is counted.
//!
//! Input is hostile: a damaged macro is an error, never a panic.

use std::collections::BTreeMap;
use std::f64::consts::TAU;

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use wc_core::Symbol;
use wc_core::geom::{Align, Point, Shape, bounds_of};
use wc_core::symbol::{Pin, PinDir, SymbolRole};

use super::pick_language;
use crate::eplan::EplanFunction;
use crate::eplan::drawing::{dir_index, rotation_of, symbol_for, vector};
use crate::{Error, Result, id_segment};

/// Representation type of a multi-line schematic.
pub const MULTI_LINE: u32 = 1;
/// Function category of a black box.
const BLACK_BOX: u16 = 301;
const MAX_DEPTH: usize = 128;
/// Most graphics and placements read from one macro.
const MAX_ITEMS: usize = 200_000;
/// Text height when a text gives none.
const TEXT_HEIGHT: f64 = 2.5;

/// A macro read as a symbol, and what of the macro it leaves out, counted by what it is.
#[derive(Debug, Clone, PartialEq)]
pub struct MacroSymbol {
    pub symbol: Symbol,
    pub left_out: BTreeMap<String, usize>,
}

/// A symbol placed from a library, in EPLAN coordinates.
#[derive(Debug, Default, Clone)]
struct Placement {
    at: Option<Point>,
    library: String,
    variant: u8,
    kind: u16,
    detail: u16,
    /// Connection point designations; `None` when the placement has none.
    pins: Option<Vec<String>>,
    letter: String,
    /// Device tag counting number `P20014`, e.g. the `1` of `K1`.
    number: String,
    /// Black box outline, relative to `at`.
    outline: Option<(Point, Point)>,
}

/// A text being read, in EPLAN coordinates.
#[derive(Debug, Clone)]
struct Text {
    at: Point,
    text: String,
    height: f64,
    align: Align,
}

#[derive(Debug, Default)]
struct Variant {
    doc: Option<u32>,
    id: u32,
    reference: Point,
    libraries: BTreeMap<String, String>,
    shapes: Vec<Shape>,
    placements: Vec<Placement>,
    texts: Vec<Text>,
    /// Objects of other kinds, and objects whose numbers could not be read.
    other: usize,
    damaged: usize,
}

fn attr(e: &BytesStart<'_>, name: &str) -> Result<Option<String>> {
    match e
        .try_get_attribute(name)
        .map_err(|err| Error::Format(format!("bad attribute: {err}")))?
    {
        Some(a) => Ok(Some(
            a.unescape_value()
                .map_err(|err| Error::Format(format!("bad value of {name}: {err}")))?
                .into_owned(),
        )),
        None => Ok(None),
    }
}

fn number(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// `x/y` or `x/y/z` as a point (z is dropped).
fn point(s: &str) -> Option<Point> {
    let mut it = s.split('/');
    let x = number(it.next()?)?;
    let y = number(it.next()?)?;
    Some(Point::new(x, y))
}

fn attr_point(e: &BytesStart<'_>, name: &str) -> Result<Option<Point>> {
    Ok(attr(e, name)?.as_deref().and_then(point))
}

fn attr_number(e: &BytesStart<'_>, name: &str) -> Result<Option<f64>> {
    Ok(attr(e, name)?.as_deref().and_then(number))
}

fn attr_u16(e: &BytesStart<'_>, name: &str) -> Result<Option<u16>> {
    Ok(attr(e, name)?.and_then(|s| s.trim().parse::<u16>().ok()))
}

/// The language EPLAN writes for a text kept the same in every language.
const NO_LANGUAGE: &str = "??_??@";

/// A text in several languages (`de_DE@Eingang;en_US@Input;`), or in none (`??_??@24 V;`).
fn text_of(s: &str) -> String {
    let segments: Vec<&str> = s.split(';').filter(|t| !t.trim().is_empty()).collect();
    let (plain, translated): (Vec<&str>, Vec<&str>) =
        segments.iter().partition(|t| t.trim_start().starts_with(NO_LANGUAGE));
    if translated.is_empty()
        && let Some(t) = plain.first()
    {
        return t.trim_start().trim_start_matches(NO_LANGUAGE).trim().to_owned();
    }
    pick_language(&translated.join(";"))
}

/// EPLAN's text alignment numbers run left, centre, right in each row of three.
fn alignment(n: u32) -> Align {
    match n.saturating_sub(1) % 3 {
        0 => Align::Left,
        1 => Align::Center,
        _ => Align::Right,
    }
}

/// A graphic object, in EPLAN coordinates; `Ok(None)` when its numbers cannot be read.
fn graphic(tag: &str, e: &BytesStart<'_>) -> Result<Option<Shape>> {
    Ok(match tag {
        "O31" => match (attr_point(e, "A531")?, attr_point(e, "A532")?) {
            (Some(a), Some(b)) => Some(Shape::Line { a, b }),
            _ => None,
        },
        "O34" => {
            let points: Option<Vec<Point>> = attr(e, "A621")?
                .unwrap_or_default()
                .split(';')
                .filter(|p| !p.trim().is_empty())
                .map(point)
                .collect();
            match points {
                Some(points) if points.len() >= 2 => Some(Shape::Polyline {
                    points,
                    closed: false,
                    filled: false,
                }),
                _ => None,
            }
        }
        "O89" => match (attr_point(e, "A1651")?, attr_point(e, "A1652")?) {
            (Some(a), Some(b)) => Some(Shape::Rect {
                min: Point::new(a.x.min(b.x), a.y.min(b.y)),
                max: Point::new(a.x.max(b.x), a.y.max(b.y)),
            }),
            _ => None,
        },
        "O32" => {
            let center = attr_point(e, "A555")?;
            let radius = attr(e, "A556")?
                .as_deref()
                .and_then(|s| s.split('/').next().and_then(number))
                .filter(|r| *r > 0.0);
            let start = attr_number(e, "A553")?;
            let end = attr_number(e, "A554")?;
            match (center, radius, start, end) {
                (Some(center), Some(radius), Some(start), Some(end)) => {
                    let sweep = end - start;
                    if sweep.abs() < 1e-9 || (sweep.abs() - TAU).abs() < 1e-6 || sweep.abs() > TAU {
                        Some(Shape::Circle {
                            center,
                            radius,
                            filled: false,
                        })
                    } else {
                        // Kept in radians here; turned to degrees when the y axis is flipped.
                        Some(Shape::Arc {
                            center,
                            radius,
                            start,
                            end,
                        })
                    }
                }
                _ => None,
            }
        }
        _ => None,
    })
}

/// The object tags read as graphics.
fn is_graphic(tag: &str) -> bool {
    matches!(tag, "O31" | "O34" | "O89" | "O32")
}

/// Every variant of a macro, as read.
fn variants(text: &str) -> Result<Vec<Variant>> {
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<String> = Vec::new();
    let mut out: Vec<Variant> = Vec::new();
    let mut current: Option<Variant> = None;
    let mut placement: Option<Placement> = None;
    let mut text_item: Option<Text> = None;
    let mut root_seen = false;
    let mut items = 0usize;
    loop {
        let ev = reader
            .read_event()
            .map_err(|e| Error::Format(format!("is not readable XML: {e}")))?;
        let (start, empty) = match &ev {
            Event::Start(e) => (Some(e), false),
            Event::Empty(e) => (Some(e), true),
            _ => (None, false),
        };
        if let Some(e) = start {
            if stack.len() >= MAX_DEPTH {
                return Err(Error::Format("is nested too deeply".into()));
            }
            let tag = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
            let parent = stack.last().map(String::as_str);
            if stack.is_empty() {
                if tag != "EplanPxfRoot" {
                    return Err(Error::Format(format!("is not an EPLAN macro (its root is {tag})")));
                }
                root_seen = true;
            } else if tag == "MacroVariant" && parent == Some("EplanPxfRoot") {
                let v = Variant {
                    doc: attr(e, "MacroFuncType")?.and_then(|s| s.trim().parse().ok()),
                    id: attr(e, "VariantId")?.and_then(|s| s.trim().parse().ok()).unwrap_or(0),
                    reference: attr_point(e, "ReferencePoint")?.unwrap_or_default(),
                    ..Default::default()
                };
                if empty {
                    out.push(v);
                } else {
                    current = Some(v);
                }
            } else if let Some(v) = current.as_mut() {
                if tag.starts_with('O') {
                    items += 1;
                    if items > MAX_ITEMS {
                        return Err(Error::Format(format!("holds more than {MAX_ITEMS} objects")));
                    }
                }
                match tag.as_str() {
                    "ESymLib" => {
                        if let (Some(id), Some(name)) = (attr(e, "ID")?, attr(e, "Name")?) {
                            v.libraries.insert(id.trim().to_owned(), name);
                        }
                    }
                    t if is_graphic(t) => match graphic(t, e)? {
                        Some(s) => v.shapes.push(s),
                        None => v.damaged += 1,
                    },
                    "O30" => match attr_point(e, "A501")? {
                        Some(at) => {
                            let t = Text {
                                at,
                                text: text_of(&attr(e, "A511")?.unwrap_or_default()),
                                height: TEXT_HEIGHT,
                                align: Align::Left,
                            };
                            if empty {
                                v.texts.push(t);
                            } else {
                                text_item = Some(t);
                            }
                        }
                        None => v.damaged += 1,
                    },
                    "S54x505" if parent == Some("O30") => {
                        if let Some(t) = text_item.as_mut() {
                            if let Some(h) = attr_number(e, "A961")?.filter(|h| *h > 0.0) {
                                t.height = h;
                            }
                            if let Some(a) = attr(e, "A966")?.and_then(|s| s.trim().parse::<u32>().ok()) {
                                t.align = alignment(a);
                            }
                        }
                    }
                    "O17" => {
                        let library = attr(e, "A1261")?.unwrap_or_default();
                        let p = Placement {
                            library: v.libraries.get(library.trim()).cloned().unwrap_or(library),
                            variant: attr_u16(e, "A1263")?.map_or(0, |n| (n % 8) as u8),
                            kind: attr_u16(e, "A1381")?.unwrap_or(0),
                            detail: attr_u16(e, "A1382")?.unwrap_or(0),
                            pins: attr(e, "A185")?.map(|s| s.split(';').map(|p| p.trim().to_owned()).collect()),
                            ..Default::default()
                        };
                        if empty {
                            // Without its point child it cannot be placed.
                            v.damaged += 1;
                        } else {
                            placement = Some(p);
                        }
                    }
                    "S40x1201" if parent == Some("O17") => {
                        if let Some(p) = placement.as_mut() {
                            p.at = attr_point(e, "A762")?;
                        }
                    }
                    "S89x5" if parent == Some("O17") => {
                        if let Some(p) = placement.as_mut() {
                            p.outline = attr_point(e, "A1651")?.zip(attr_point(e, "A1652")?);
                        }
                    }
                    t if parent == Some("O17") && t.starts_with('P') => {
                        if let Some(p) = placement.as_mut()
                            && p.letter.is_empty()
                            && let Some(letter) = attr(e, "P20013")?
                        {
                            let letter = letter.trim();
                            if letter.chars().all(|c| c.is_alphanumeric()) {
                                p.letter = letter.to_owned();
                                let n = attr(e, "P20014")?.unwrap_or_default();
                                let n = n.trim();
                                p.number = if n.chars().all(|c| c.is_alphanumeric()) {
                                    n.to_owned()
                                } else {
                                    String::new()
                                };
                            }
                        }
                    }
                    // Groups and pages only hold other objects; the macro's own frame (O37) is
                    // not drawn.
                    "O4" | "O14" | "O26" | "O37" | "O76" => {}
                    t if t.starts_with('O') => v.other += 1,
                    _ => {}
                }
            }
            if !empty {
                stack.push(tag);
            }
            continue;
        }
        match ev {
            Event::End(_) => {
                let tag = stack.pop().unwrap_or_default();
                match tag.as_str() {
                    "O17" => {
                        if let (Some(p), Some(v)) = (placement.take(), current.as_mut()) {
                            if p.at.is_some() {
                                v.placements.push(p);
                            } else {
                                v.damaged += 1;
                            }
                        }
                    }
                    "O30" => {
                        if let (Some(t), Some(v)) = (text_item.take(), current.as_mut()) {
                            v.texts.push(t);
                        }
                    }
                    "MacroVariant" => {
                        if let Some(v) = current.take() {
                            out.push(v);
                        }
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !root_seen {
        return Err(Error::Format("is not an EPLAN macro (no EplanPxfRoot)".into()));
    }
    if !stack.is_empty() {
        return Err(Error::Format("ends inside an element".into()));
    }
    Ok(out)
}

/// What a representation type is called in the report.
fn representation(doc: Option<u32>) -> String {
    match doc {
        Some(1) => "multi-line".into(),
        Some(2) => "single-line".into(),
        Some(4) => "overview".into(),
        Some(6) => "panel layout".into(),
        Some(10) => "3D".into(),
        Some(n) => format!("type {n}"),
        None => "unknown type".into(),
    }
}

fn add(left_out: &mut BTreeMap<String, usize>, what: &str, n: usize) {
    if n > 0 {
        *left_out.entry(what.to_owned()).or_default() += n;
    }
}

/// Pin direction after EPLAN's variant `v` turned a pin facing `d`.
fn turned(d: PinDir, variant: u8) -> PinDir {
    let (rotation, mirror) = rotation_of(variant);
    match dir_index(vector(d).transform(rotation, mirror)) {
        0 => PinDir::Up,
        1 => PinDir::Right,
        2 => PinDir::Down,
        _ => PinDir::Left,
    }
}

/// Read a macro's multi-line representation, variant A, as a symbol. `name` is the macro's
/// file name in the archive; the symbol's id is `eplan.` and its file name without folders and
/// extension. A macro without a multi-line representation, or with nothing drawn in it, is an
/// error that says so.
pub fn read(name: &str, text: &str) -> Result<MacroSymbol> {
    let all = variants(text)?;
    let multi: Vec<&Variant> = all.iter().filter(|v| v.doc == Some(MULTI_LINE)).collect();
    let Some(v) = multi.iter().find(|v| v.id == 0).or_else(|| multi.first()).copied() else {
        let mut kinds: Vec<String> = all.iter().map(|v| representation(v.doc)).collect();
        kinds.dedup();
        return Err(Error::Format(if kinds.is_empty() {
            "holds no representation".into()
        } else {
            format!("has no multi-line representation (only {})", kinds.join(", "))
        }));
    };
    let mut left_out = BTreeMap::new();
    for other in all.iter().filter(|o| !std::ptr::eq(*o, v)) {
        let what = if other.doc == Some(MULTI_LINE) {
            "other variants of the multi-line representation".to_owned()
        } else {
            format!("{} representations", representation(other.doc))
        };
        add(&mut left_out, &what, 1);
    }
    add(
        &mut left_out,
        "objects of other kinds (connections, potentials, references)",
        v.other,
    );
    add(&mut left_out, "objects whose numbers could not be read", v.damaged);

    // EPLAN's y axis points up; the symbol's down, with the reference point as its origin.
    let r = v.reference;
    let page = |p: Point| Point::new(p.x - r.x, r.y - p.y);
    let deg = |a: f64| (a.to_degrees()).rem_euclid(360.0);
    let mut graphics: Vec<Shape> = Vec::new();
    for s in &v.shapes {
        graphics.push(match s {
            // Flipping the y axis turns a counter-clockwise arc from a to b into one from -b
            // to -a.
            Shape::Arc {
                center,
                radius,
                start,
                end,
            } => Shape::Arc {
                center: page(*center),
                radius: *radius,
                start: deg(-end),
                end: deg(-start),
            },
            Shape::Line { a, b } => Shape::Line {
                a: page(*a),
                b: page(*b),
            },
            Shape::Polyline { points, closed, filled } => Shape::Polyline {
                points: points.iter().map(|p| page(*p)).collect(),
                closed: *closed,
                filled: *filled,
            },
            Shape::Rect { min, max } => {
                let (a, b) = (page(*min), page(*max));
                Shape::Rect {
                    min: Point::new(a.x.min(b.x), a.y.min(b.y)),
                    max: Point::new(a.x.max(b.x), a.y.max(b.y)),
                }
            }
            Shape::Circle { center, radius, filled } => Shape::Circle {
                center: page(*center),
                radius: *radius,
                filled: *filled,
            },
            other => other.clone(),
        });
    }
    for t in &v.texts {
        if t.text.trim().is_empty() {
            continue;
        }
        graphics.push(Shape::Text {
            at: page(t.at),
            text: t.text.clone(),
            height: t.height,
            align: t.align,
            angle: 0.0,
        });
    }

    let mut pins: Vec<Pin> = Vec::new();
    // The device tag of each pin's placement, by pin.
    let mut owners: Vec<String> = Vec::new();
    let mut letter = String::new();
    let mut drawn_kinds: Vec<SymbolRole> = Vec::new();
    let mut box_count = 0usize;
    for p in &v.placements {
        let Some(at) = p.at else { continue };
        if letter.is_empty() && !p.letter.is_empty() {
            letter = p.letter.clone();
        }
        let origin = page(at);
        if p.kind == BLACK_BOX {
            if let Some((a, b)) = p.outline {
                let (a, b) = (page(at + a), page(at + b));
                graphics.push(Shape::Rect {
                    min: Point::new(a.x.min(b.x), a.y.min(b.y)),
                    max: Point::new(a.x.max(b.x), a.y.max(b.y)),
                });
                box_count += 1;
            }
            continue;
        }
        let Some(names) = &p.pins else {
            add(&mut left_out, "placed symbols without connection points", 1);
            continue;
        };
        if names.len() <= 1 {
            let name = names.first().cloned().unwrap_or_default();
            pins.push(Pin {
                name,
                at: origin,
                dir: turned(PinDir::Up, p.variant),
            });
            owners.push(format!("{}{}", p.letter, p.number));
            if p.library != "SPECIAL" {
                add(&mut left_out, "library graphics of connection points (pins kept)", 1);
            }
            continue;
        }
        let f = EplanFunction {
            kind: p.kind,
            detail: p.detail,
            variant: p.variant,
            pins: names.clone(),
            ..Default::default()
        };
        let Some(s) = symbol_for(&f) else {
            add(&mut left_out, "placed symbols without connection points", 1);
            continue;
        };
        let (rotation, mirror) = rotation_of(p.variant);
        for g in &s.graphics {
            graphics.push(g.placed(origin, rotation, mirror));
        }
        for pin in &s.pins {
            pins.push(Pin {
                name: pin.name.clone(),
                at: origin + pin.at.transform(rotation, mirror),
                dir: turned(pin.dir, p.variant),
            });
            owners.push(format!("{}{}", p.letter, p.number));
        }
        drawn_kinds.push(s.role);
        add(
            &mut left_out,
            "library symbols drawn as the WiringCraft symbol of the same kind",
            1,
        );
    }

    // Each pin name once: blanks are numbered, and in a macro of several devices (a circuit)
    // each name is led by its device's tag, `K1:13`; repeats left are marked.
    let devices: std::collections::BTreeSet<&str> =
        owners.iter().map(String::as_str).filter(|t| !t.is_empty()).collect();
    let prefix = devices.len() > 1;
    let mut seen: Vec<String> = Vec::new();
    for (k, pin) in pins.iter_mut().enumerate() {
        if pin.name.is_empty() {
            pin.name = (k + 1).to_string();
        }
        if prefix && let Some(tag) = owners.get(k).filter(|t| !t.is_empty()) {
            pin.name = format!("{tag}:{}", pin.name);
        }
        while seen.contains(&pin.name) {
            pin.name.push('\'');
        }
        seen.push(pin.name.clone());
    }

    if graphics.is_empty() && pins.is_empty() {
        return Err(Error::Format("draws nothing in its multi-line representation".into()));
    }
    let mut pts: Vec<Point> = pins.iter().map(|p| p.at).collect();
    for g in &graphics {
        let (a, b) = g.bounds();
        pts.extend([a, b]);
    }
    let (min, _) = bounds_of(&pts);
    let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = leaf.rsplit_once('.').map_or(leaf, |(stem, _)| stem);
    let role = match drawn_kinds.as_slice() {
        [one] if box_count == 0 => *one,
        _ => SymbolRole::Standalone,
    };
    Ok(MacroSymbol {
        symbol: Symbol {
            id: format!("eplan.{}", id_segment(stem)),
            name: stem.to_owned(),
            category: "EPLAN macros".into(),
            letter_code: letter,
            role,
            graphics,
            pins,
            tag_at: Point::new(min.x, min.y - 2.0),
            description: format!("EPLAN macro {leaf}, multi-line, variant {}", variant_letter(v.id)),
        },
        left_out,
    })
}

/// EPLAN's variant letter: 0 is A.
fn variant_letter(id: u32) -> String {
    char::from_u32(u32::from(b'A') + id.min(25)).map_or_else(|| id.to_string(), String::from)
}

#[cfg(test)]
mod tests;

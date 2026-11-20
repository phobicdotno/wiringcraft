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
//!   several languages, its child `S54x505` the height `A961` and the alignment `A966`); `O83`
//!   is a text with the same attributes. `O88` is a location box (counted, not drawn) and `O132`
//!   a group.
//! - `O17`, a symbol placed from a library: library `A1261`, symbol `A1262`, variant `A1263`
//!   (0 to 3 turn it in quarter steps, 4 to 7 also mirror it), function category `A1381` and
//!   its refinement `A1382`, connection point designations `A185` (separated by `;`), its point
//!   in the child `S40x1201` (`A762`), and its device tag letter `P20013` and counting number
//!   `P20014` in a property child. A macro of several devices (a circuit) leads each pin name
//!   with its device's tag: `K1:A1`.
//!   A black box (category 301) has its outline in the child `S89x5`: corners `A1651` and
//!   `A1652` relative to its point. The symbol's device tag letter is the black box's, else that
//!   of the device with the most connection points.
//!
//! The multi-line representation, variant A, becomes the symbol: its graphics, black box
//! outlines, and a pin for each connection point. A placement with one connection point is a
//! device or PLC connection point and becomes one pin, facing up in variant 0 and a quarter turn
//! further for each variant. EPLAN keeps the drawing of library symbols in its own libraries,
//! not in the macro, so a placement with several connection points is drawn with the
//! WiringCraft symbol of the same kind ([`crate::eplan::drawing::symbol_for`]), first
//! connection point at its point, as the EPLAN project import does. Variant A may hold only
//! some of a device's terminals; named connection points that only the other multi-line
//! variants hold (or, when none do, the overview) are added as pins in a column to the right of
//! the drawing, facing right. What is left out is counted.
//!
//! Input is hostile: a damaged macro is an error, never a panic.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::TAU;

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use wc_core::Symbol;
use wc_core::geom::{Align, Point, Shape, bounds_of};
use wc_core::symbol::{Pin, PinDir, SymbolRole};

use super::pick_language;
use crate::eplan::EplanFunction;
use crate::eplan::drawing::{KIND_TERMINAL, UniqueNames, dir_index, rotation_of, symbol_for, vector};
use crate::{Error, Result, id_segment};

/// Representation type of a multi-line schematic.
pub const MULTI_LINE: u32 = 1;
/// Representation type of an overview.
const OVERVIEW: u32 = 4;
/// Distance between connection points taken from other variants, in millimetres.
const PIN_STEP: f64 = 4.0;
/// Function category of a black box.
const BLACK_BOX: u16 = 301;
const MAX_DEPTH: usize = 128;
/// Most graphics and placements read from one macro.
const MAX_ITEMS: usize = 200_000;
/// Most representation kinds named when a macro has no multi-line representation.
const MAX_KINDS_NAMED: usize = 8;
/// Longest device tag letter or counting number kept; a longer one is not a tag.
const MAX_TAG: usize = 32;
/// Most connection points read from one macro.
const MAX_PINS: usize = 10_000;
/// Most connection point designations read from one placed symbol.
const MAX_DESIGNATIONS: usize = 512;
/// Longest symbol id stem taken from a macro's file name; ids must stay within 128 characters.
const MAX_STEM: usize = 100;
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
    /// Location boxes, not drawn.
    location_boxes: usize,
}

fn attr(e: &BytesStart<'_>, name: &str) -> Result<Option<String>> {
    match e
        .try_get_attribute(name)
        .map_err(|err| Error::Format(format!("has a bad attribute: {err}")))?
    {
        Some(a) => Ok(Some(
            a.unescape_value()
                .map_err(|err| Error::Format(format!("has a bad value of {name}: {err}")))?
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

/// A placed symbol's connection point designations (`A185`, separated by `;`).
fn designations(s: &str) -> Result<Vec<String>> {
    let names: Vec<String> = s
        .split(';')
        .take(MAX_DESIGNATIONS + 1)
        .map(|p| p.trim().to_owned())
        .collect();
    if names.len() > MAX_DESIGNATIONS {
        return Err(Error::Format(format!(
            "places a symbol with more than {MAX_DESIGNATIONS} connection points"
        )));
    }
    Ok(names)
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
                    // O83 is a text too, with the same attributes.
                    "O30" | "O83" => match attr_point(e, "A501")? {
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
                    "S54x505" if matches!(parent, Some("O30" | "O83")) => {
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
                            pins: match attr(e, "A185")? {
                                Some(s) => Some(designations(&s)?),
                                None => None,
                            },
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
                            if letter.len() <= MAX_TAG && letter.chars().all(|c| c.is_alphanumeric()) {
                                p.letter = letter.to_owned();
                                let n = attr(e, "P20014")?.unwrap_or_default();
                                let n = n.trim();
                                p.number = if n.len() <= MAX_TAG && n.chars().all(|c| c.is_alphanumeric()) {
                                    n.to_owned()
                                } else {
                                    String::new()
                                };
                            }
                        }
                    }
                    "O88" => v.location_boxes += 1,
                    // Groups and pages only hold other objects; the macro's own frame (O37) is
                    // not drawn.
                    "O4" | "O14" | "O26" | "O37" | "O76" | "O132" => {}
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
                    "O30" | "O83" => {
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
    if !multi.is_empty() && !multi.iter().any(|v| v.id == 0) {
        return Err(Error::Format(
            "has no variant A of its multi-line representation".into(),
        ));
    }
    let Some(v) = multi.iter().find(|v| v.id == 0).copied() else {
        // Each kind once, in the order first seen; only the first few are named.
        let mut seen = std::collections::BTreeSet::new();
        let mut kinds: Vec<String> = Vec::new();
        let mut more = 0usize;
        for k in all.iter().map(|v| representation(v.doc)) {
            if seen.insert(k.clone()) {
                if kinds.len() < MAX_KINDS_NAMED {
                    kinds.push(k);
                } else {
                    more += 1;
                }
            }
        }
        if more > 0 {
            kinds.push(format!("{more} more"));
        }
        return Err(Error::Format(if kinds.is_empty() {
            "holds no representation".into()
        } else {
            format!("has no multi-line representation (only {})", kinds.join(", "))
        }));
    };
    let mut left_out = BTreeMap::new();
    add(
        &mut left_out,
        "objects of other kinds (connections, potentials, references)",
        v.other,
    );
    add(&mut left_out, "objects whose numbers could not be read", v.damaged);
    add(&mut left_out, "location boxes", v.location_boxes);
    let pin_count: usize = v
        .placements
        .iter()
        .filter(|p| p.kind != BLACK_BOX)
        .map(|p| p.pins.as_ref().map_or(0, |n| n.len().max(1)))
        .sum();
    if pin_count > MAX_PINS {
        return Err(Error::Format(format!("holds more than {MAX_PINS} connection points")));
    }
    // The named connection points variant A has, and those it leaves out that the other
    // variants hold.
    let mut have: BTreeSet<(String, String, String)> = BTreeSet::new();
    for p in v.placements.iter().filter(|p| p.kind != BLACK_BOX) {
        for name in p.pins.iter().flatten() {
            have.insert((p.letter.clone(), p.number.clone(), name.clone()));
        }
    }
    let extra = other_points(&all, v, &have);

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
    let mut owners: Vec<(String, String)> = Vec::new();
    // The device tag letter of the black box, if it has one.
    let mut box_letter = String::new();
    let mut drawn_kinds: Vec<SymbolRole> = Vec::new();
    let mut box_count = 0usize;
    for p in &v.placements {
        let Some(at) = p.at else { continue };
        let origin = page(at);
        if p.kind == BLACK_BOX {
            if box_letter.is_empty() {
                box_letter = p.letter.clone();
            }
            if let Some((a, b)) = p.outline {
                let (a, b) = (page(at + a), page(at + b));
                graphics.push(Shape::Rect {
                    min: Point::new(a.x.min(b.x), a.y.min(b.y)),
                    max: Point::new(a.x.max(b.x), a.y.max(b.y)),
                });
                box_count += 1;
            } else {
                add(&mut left_out, "black boxes without an outline", 1);
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
            owners.push((p.letter.clone(), p.number.clone()));
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
        // A terminal keeps the macro's own designations, in order.
        let own = |k: usize| -> Option<String> {
            (p.kind == KIND_TERMINAL)
                .then(|| names.get(k).filter(|n| !n.is_empty()).cloned())
                .flatten()
        };
        for (k, pin) in s.pins.iter().enumerate() {
            pins.push(Pin {
                name: own(k).unwrap_or_else(|| pin.name.clone()),
                at: origin + pin.at.transform(rotation, mirror),
                dir: turned(pin.dir, p.variant),
            });
            owners.push((p.letter.clone(), p.number.clone()));
        }
        drawn_kinds.push(s.role);
        add(
            &mut left_out,
            "library symbols drawn as the WiringCraft symbol of the same kind",
            1,
        );
    }

    // Connection points only the other variants hold go in a column right of the drawing, a
    // grid step apart, facing right.
    let mut column: Vec<Point> = pins.iter().map(|p| p.at).collect();
    for g in &graphics {
        let (a, b) = g.bounds();
        column.extend([a, b]);
    }
    let (top, right) = if column.is_empty() {
        (0.0, 0.0)
    } else {
        let (min, max) = bounds_of(&column);
        (min.y, max.x + PIN_STEP)
    };
    // The variants that gave a pin; they are not counted as left out.
    let mut givers: BTreeSet<usize> = BTreeSet::new();
    for (row, (from, letter, number, name)) in extra.into_iter().enumerate() {
        givers.insert(from);
        pins.push(Pin {
            name,
            at: Point::new(right, top + PIN_STEP * row as f64),
            dir: PinDir::Right,
        });
        owners.push((letter, number));
        if pins.len() > MAX_PINS {
            return Err(Error::Format(format!("holds more than {MAX_PINS} connection points")));
        }
    }
    for (k, other) in all.iter().enumerate() {
        if std::ptr::eq(other, v) || givers.contains(&k) {
            continue;
        }
        let what = if other.doc == Some(MULTI_LINE) {
            "other variants of the multi-line representation".to_owned()
        } else {
            format!("{} representations", representation(other.doc))
        };
        add(&mut left_out, &what, 1);
    }

    // Each pin name once: blanks are numbered, and in a macro of several devices (a circuit)
    // each name is led by its device's tag, `K1:13`; repeats left are marked.
    let tags: Vec<String> = owners.iter().map(|(l, n)| format!("{l}{n}")).collect();
    // Pins by device tag, and where each tag was first seen.
    let mut per_device: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for (k, t) in tags.iter().enumerate().filter(|(_, t)| !t.is_empty()) {
        per_device.entry(t.as_str()).or_insert((0, k)).0 += 1;
    }
    let prefix = per_device.len() > 1;
    let mut unique = UniqueNames::default();
    for (k, pin) in pins.iter_mut().enumerate() {
        let mut name = std::mem::take(&mut pin.name);
        if name.is_empty() {
            name = (k + 1).to_string();
        }
        if prefix && let Some(tag) = tags.get(k).filter(|t| !t.is_empty()) {
            name = format!("{tag}:{name}");
        }
        pin.name = unique.unique(name);
    }
    // The symbol's letter: the black box's, else that of the device owning the most pins (the
    // first seen of equals).
    let letter = if box_letter.is_empty() {
        per_device
            .values()
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
            .and_then(|(_, k)| owners.get(*k))
            .map(|(l, _)| l.clone())
            .unwrap_or_default()
    } else {
        box_letter
    };

    if graphics.is_empty() && pins.is_empty() {
        return Err(Error::Format("draws nothing in its multi-line representation".into()));
    }
    let mut pts: Vec<Point> = pins.iter().map(|p| p.at).collect();
    for g in &graphics {
        let (a, b) = g.bounds();
        pts.extend([a, b]);
    }
    if pts.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
        return Err(Error::Format("has coordinates too large to draw".into()));
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
            source: String::new(),
            id: format!("eplan.{}", id_segment(&stem.chars().take(MAX_STEM).collect::<String>())),
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

/// The named connection points of the macro's other multi-line variants, in variant order, as
/// (index of the variant in `all`, device tag letter, counting number, designation). A variant
/// A may hold only some of a device's terminals (a PLC module with 1-10 in A and 11-20 in B).
/// Only points not in `have` (variant A's) are taken, each once. When no other multi-line
/// variant adds any, the overview representation's are taken, since it lists all of them.
fn other_points<'a>(
    all: &'a [Variant],
    a: &Variant,
    have: &BTreeSet<(String, String, String)>,
) -> Vec<(usize, String, String, String)> {
    let named = |k: usize, v: &'a Variant| {
        v.placements
            .iter()
            .filter(|p| p.kind != BLACK_BOX && p.at.is_some())
            .flat_map(move |p| {
                p.pins
                    .iter()
                    .flatten()
                    .filter(|n| !n.is_empty())
                    .map(move |n| (k, p.letter.clone(), p.number.clone(), n.clone()))
            })
    };
    let mut others: Vec<(usize, &Variant)> = all
        .iter()
        .enumerate()
        .filter(|(_, o)| o.doc == Some(MULTI_LINE) && !std::ptr::eq(*o, a))
        .collect();
    others.sort_by_key(|(_, o)| o.id);
    let overview = all.iter().enumerate().find(|(_, o)| o.doc == Some(OVERVIEW));
    let mut points = Vec::new();
    // Each new point once, and no more than one past the most a macro may hold: the caller
    // refuses it then anyway.
    let mut seen = have.clone();
    for (k, o) in others.into_iter().chain(overview) {
        if o.doc == Some(OVERVIEW) && !points.is_empty() {
            break;
        }
        for (k, letter, number, name) in named(k, o) {
            if seen.insert((letter.clone(), number.clone(), name.clone())) {
                points.push((k, letter, number, name));
                if points.len() > MAX_PINS {
                    return points;
                }
            }
        }
    }
    points
}

/// EPLAN's variant letter: 0 is A.
fn variant_letter(id: u32) -> String {
    char::from_u32(u32::from(b'A') + id.min(25)).map_or_else(|| id.to_string(), String::from)
}

#[cfg(test)]
mod tests;

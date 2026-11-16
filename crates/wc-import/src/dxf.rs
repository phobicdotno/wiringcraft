//! ASCII DXF reading: symbols and whole drawings.
//!
//! Supported entities: LINE, CIRCLE, ARC, LWPOLYLINE, POLYLINE/VERTEX, TEXT, MTEXT, POINT and
//! INSERT (block references, exploded or turned into library symbols). DXF has y pointing up;
//! WiringCraft pages have y pointing down, so y is flipped on the way in.

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
        let shape = match e.kind.as_str() {
            "LINE" => Some(Shape::Line {
                a: p(e.f(10), e.f(20)),
                b: p(e.f(11), e.f(21)),
            }),
            "CIRCLE" => Some(Shape::Circle {
                center: p(e.f(10), e.f(20)),
                radius: e.f(40) * r_scale,
                filled: false,
            }),
            "ARC" => {
                let (s, en) = (e.f(50) + xf.rot, e.f(51) + xf.rot);
                Some(Shape::Arc {
                    center: p(e.f(10), e.f(20)),
                    radius: e.f(40) * r_scale,
                    start: -en,
                    end: -s,
                })
            }
            "LWPOLYLINE" => {
                let xs = e.all(10);
                let ys = e.all(20);
                let pts: Vec<Point> = xs.iter().zip(&ys).map(|(x, y)| p(*x, *y)).collect();
                (pts.len() >= 2).then(|| Shape::Polyline {
                    points: pts,
                    closed: (e.f(70) as i64) & 1 == 1,
                })
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
                    shapes(d, inner, ixf, scale, depth + 1, skip_layer, out, points);
                }
                None
            }
            _ => None,
        };
        if let Some(s) = shape {
            out.push((layer, s));
        }
    }
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
    let d = parse(text)?;
    let mut raw = Vec::new();
    let mut points = Vec::new();
    shapes(&d, &d.entities, Xf::ID, 1.0, 0, &|_| false, &mut raw, &mut points);
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
    make_symbol(id, name, graphics, pins)
}

/// The result of importing a drawing onto a page.
#[derive(Debug, Default)]
pub struct ImportedPage {
    pub graphics: Vec<Shape>,
    pub symbols: Vec<PlacedSymbol>,
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
    shapes(&d, &plain, Xf::ID, scale, 0, &|_| false, &mut raw, &mut points);
    out.graphics = raw.into_iter().map(|(_, s)| s).collect();
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
    fn mtext_formatting_is_stripped() {
        assert_eq!(mtext_plain(r"{\fArial|b1;Motor}\PM1"), "Motor M1");
    }
}

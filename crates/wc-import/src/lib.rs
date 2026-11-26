//! Importers: symbols from DXF and SVG, parts from CSV and Excel workbooks, schematic drawings from DXF.
//!
//! Input is treated as hostile: malformed files produce errors or are skipped, never panics.

pub mod aml;
pub mod archive;
pub mod bmecat;
pub mod csv;
pub mod dwg;
pub mod dxf;
pub mod edz;
pub mod eplan;
pub mod kicad;
pub mod pdf;
mod pdf_codecs;
pub mod qet;
pub mod raster;
pub mod step;
pub mod svg;
pub mod swe;
pub mod wago;
pub mod xlsx;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Format(String),
    #[error(transparent)]
    Core(#[from] wc_core::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// `s` with control characters (escape, line breaks, ...) written as `\u{1b}`, so a name from a
/// source file prints as one line of plain text and cannot steer the terminal. Other characters,
/// backslashes in paths included, are kept as they are.
pub(crate) fn printable(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() {
                c.escape_unicode().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// One segment of an imported item's id, from a name in the source: ASCII letters and digits
/// in lower case and `-` are kept, everything else becomes `_` (`Relay Coil` is `relay_coil`).
pub fn id_segment(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Give a read part its id and add it to `out`, keyed by id. `what` names the source row or
/// product in warnings ("row 3", "product 2"). Returns false when the part was skipped.
pub(crate) fn add_part(
    out: &mut std::collections::BTreeMap<String, wc_core::Part>,
    mut part: wc_core::Part,
    what: &str,
    warnings: &mut Vec<String>,
) -> bool {
    part.id = wc_core::Part::make_id(&part.manufacturer, &part.order_number);
    if let Err(e) = wc_core::store::check_id(&part.id) {
        warnings.push(format!(
            "{what}: cannot make an id from {:?} ({e}), skipped",
            part.order_number
        ));
        return false;
    }
    if out.insert(part.id.clone(), part).is_some() {
        warnings.push(format!("{what}: duplicate order number, the later one wins"));
    }
    true
}

use wc_core::geom::{Point, Shape, bounds_of};
use wc_core::symbol::{Pin, PinDir};

/// Turn raw shapes and pin positions into a symbol: origin at the first pin (or the top-left
/// corner), pins named in order when no name was found, wire direction guessed from which edge
/// of the drawing a pin is nearest.
pub(crate) fn make_symbol(
    id: &str,
    name: &str,
    shapes: Vec<Shape>,
    pins: Vec<(Option<String>, Point)>,
) -> Result<wc_core::Symbol> {
    wc_core::store::check_id(id)?;
    if shapes.is_empty() && pins.is_empty() {
        return Err(Error::Format("the drawing is empty".into()));
    }
    let mut pts: Vec<Point> = shapes
        .iter()
        .flat_map(|s| {
            let (a, b) = s.bounds();
            [a, b]
        })
        .collect();
    pts.extend(pins.iter().map(|p| p.1));
    let (min, max) = bounds_of(&pts);
    let origin = pins.first().map(|p| p.1).unwrap_or(min);
    let shift = Point::new(-origin.x, -origin.y);
    let snap = |v: f64| (v * 1000.0).round() / 1000.0;
    let fix = |p: Point| Point::new(snap(p.x + shift.x), snap(p.y + shift.y));
    let graphics = shapes.iter().map(|s| s.translate(shift)).collect();
    let mut used = std::collections::BTreeSet::new();
    let mut out_pins = Vec::new();
    for (i, (n, at)) in pins.iter().enumerate() {
        let mut name = n
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| (i + 1).to_string());
        while !used.insert(name.clone()) {
            name.push('\'');
        }
        let d = [
            (at.y - min.y, PinDir::Up),
            (max.y - at.y, PinDir::Down),
            (at.x - min.x, PinDir::Left),
            (max.x - at.x, PinDir::Right),
        ];
        let dir = d.iter().fold(d[0], |best, c| if c.0 < best.0 { *c } else { best }).1;
        out_pins.push(Pin {
            name,
            at: fix(*at),
            dir,
        });
    }
    Ok(wc_core::Symbol {
        source: String::new(),
        id: id.into(),
        name: name.into(),
        category: String::new(),
        letter_code: String::new(),
        role: Default::default(),
        graphics,
        pins: out_pins,
        tag_at: Point::new(snap(min.x + shift.x - 3.0), snap((min.y + max.y) / 2.0 + shift.y)),
        description: String::new(),
    })
}

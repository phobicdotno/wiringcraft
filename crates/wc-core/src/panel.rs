//! Panel layouts: the mounting plate of a cabinet with its DIN rails, wire ducts and the
//! devices placed on them, in millimetres with y pointing down from the plate's top-left corner.
//!
//! Each layout is a file `panels/<id>.panel.json`. Device footprints come from the part's
//! `Width` and `Height` properties, or the extents of its footprint drawing; [`arrange`] puts devices that are not on the plate yet onto
//! the rails, left to right and top to bottom.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::geom::{Point, Rotation};
use crate::tag::DeviceTag;
use crate::{Part, Project};

/// Footprint used for a device whose part has no size: one 18 mm DIN module, 90 mm high.
pub const DEFAULT_SIZE: (f64, f64) = (18.0, 90.0);
/// Space left between neighbouring devices on a rail.
pub const GAP: f64 = 1.0;
/// Width of the wire ducts that [`Panel::with_rails`] draws.
pub const DUCT: f64 = 40.0;
/// Smallest height of a row of devices between two ducts.
pub const ROW: f64 = 125.0;
/// Shortest length a rail, and smallest width or height a duct, can be resized to.
pub const MIN_RESIZE: f64 = 10.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    /// Stable id, also the file stem.
    pub id: String,
    pub name: String,
    /// Mounting plate size in millimetres.
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub items: Vec<PanelItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PanelItem {
    /// A 35 mm DIN rail: `at` is its left end on the centre line.
    Rail { at: Point, length: f64 },
    /// A wire duct over a rectangle.
    Duct { min: Point, max: Point },
    /// A device's footprint. `width` and `height` are the part's size as it stands upright;
    /// `rotation` turns it clockwise on the plate in quarter steps, so at 90 or 270 degrees it
    /// covers `height` x `width`. `at` is the top-left corner of the turned outline.
    Device {
        tag: DeviceTag,
        at: Point,
        width: f64,
        height: f64,
        #[serde(default, skip_serializing_if = "Rotation::is_default")]
        rotation: Rotation,
    },
}

/// The size a `width` x `height` footprint covers on the plate when turned by `rotation`.
pub fn turned_size(width: f64, height: f64, rotation: Rotation) -> (f64, f64) {
    match rotation {
        Rotation::R0 | Rotation::R180 => (width, height),
        Rotation::R90 | Rotation::R270 => (height, width),
    }
}

/// Where a shape drawn in an upright `width` x `height` box with its top-left corner at (0, 0)
/// lands when that box is turned by `rotation` and its turned outline put with its top-left
/// corner at `at`.
pub fn turn_into_place(shape: &crate::Shape, at: Point, width: f64, height: f64, rotation: Rotation) -> crate::Shape {
    // Point::transform turns (0, 0)-(w, h) into a box with one corner at the origin; shift it so
    // its top-left corner is at `at`.
    let offset = match rotation {
        Rotation::R0 => at,
        Rotation::R90 => Point::new(at.x + height, at.y),
        Rotation::R180 => Point::new(at.x + width, at.y + height),
        Rotation::R270 => Point::new(at.x, at.y + width),
    };
    shape.placed(offset, rotation, false)
}

/// The next quarter turn clockwise.
pub fn next_quarter(rotation: Rotation) -> Rotation {
    match rotation {
        Rotation::R0 => Rotation::R90,
        Rotation::R90 => Rotation::R180,
        Rotation::R180 => Rotation::R270,
        Rotation::R270 => Rotation::R0,
    }
}

impl PanelItem {
    /// A device's outline on the plate, as (top-left, bottom-right), turned by its rotation.
    pub fn device_bounds(&self) -> Option<(Point, Point)> {
        match self {
            PanelItem::Device {
                at,
                width,
                height,
                rotation,
                ..
            } => {
                let (w, h) = turned_size(*width, *height, *rotation);
                Some((*at, Point::new(at.x + w, at.y + h)))
            }
            _ => None,
        }
    }

    /// The outline the item covers on the plate, as (top-left, bottom-right): a rail is 35 mm
    /// tall about its centre line, a duct is its rectangle, a device is turned as it stands.
    pub fn bounds(&self) -> (Point, Point) {
        match self {
            PanelItem::Rail { at, length } => (Point::new(at.x, at.y - 17.5), Point::new(at.x + length, at.y + 17.5)),
            PanelItem::Duct { min, max } => (
                Point::new(min.x.min(max.x), min.y.min(max.y)),
                Point::new(min.x.max(max.x), min.y.max(max.y)),
            ),
            PanelItem::Device { .. } => self.device_bounds().unwrap_or_default(),
        }
    }

    /// The point a drag moves the item by: a rail's left end on its centre line, a duct's
    /// `min` corner, a device's `at`.
    pub fn anchor(&self) -> Point {
        match self {
            PanelItem::Rail { at, .. } | PanelItem::Device { at, .. } => *at,
            PanelItem::Duct { min, .. } => *min,
        }
    }

    /// Moves the item so its anchor goes to `to`, as near as it can while its outline stays on
    /// a `plate_width` x `plate_height` plate; an item larger than the plate goes to its left or
    /// top edge. Size, length and rotation stay as they are.
    pub fn move_on_plate(&mut self, to: Point, plate_width: f64, plate_height: f64) {
        let (a, b) = self.bounds();
        let from = self.anchor();
        let x = (a.x + to.x - from.x).clamp(0.0, (plate_width - (b.x - a.x)).max(0.0));
        let y = (a.y + to.y - from.y).clamp(0.0, (plate_height - (b.y - a.y)).max(0.0));
        let (dx, dy) = (x - a.x, y - a.y);
        let shift = |p: &mut Point| *p = Point::new(p.x + dx, p.y + dy);
        match self {
            PanelItem::Rail { at, .. } | PanelItem::Device { at, .. } => shift(at),
            PanelItem::Duct { min, max } => {
                shift(min);
                shift(max);
            }
        }
    }
}

/// Whether the outline from `min` (top-left) to `max` (bottom-right) lies on a plate of
/// `width` x `height` millimetres. A small tolerance lets an outline flush with the plate's
/// edge through, so a size typed exactly or reached by adding up decimals still fits. An
/// outline or plate size that is not a finite number never fits.
pub fn outline_on_plate(width: f64, height: f64, min: Point, max: Point) -> bool {
    let finite = [width, height, min.x, min.y, max.x, max.y];
    if !finite.iter().all(|v| v.is_finite()) {
        return false;
    }
    let eps = 1e-9;
    !(min.x < -eps || min.y < -eps || max.x > width + eps || max.y > height + eps)
}

/// The rectangle two corners span, whichever way round they are given. A non-finite corner
/// is passed through unchanged so [`outline_on_plate`] still refuses it, since `f64::min` and
/// `f64::max` would otherwise drop a NaN in favour of the finite side.
fn spanned(a: Point, b: Point) -> (Point, Point) {
    if [a.x, a.y, b.x, b.y].iter().all(|v| v.is_finite()) {
        (
            Point::new(a.x.min(b.x), a.y.min(b.y)),
            Point::new(a.x.max(b.x), a.y.max(b.y)),
        )
    } else {
        (a, b)
    }
}

impl Panel {
    /// Whether a `w` x `h` millimetre outline with its top-left corner at `at` lies on this
    /// plate, edges included. See [`outline_on_plate`].
    pub fn outline_fits(&self, at: Point, w: f64, h: f64) -> bool {
        outline_on_plate(self.width, self.height, at, Point::new(at.x + w, at.y + h))
    }

    /// An empty plate.
    pub fn new(id: impl Into<String>, name: impl Into<String>, width: f64, height: f64) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            width,
            height,
            items: Vec::new(),
        }
    }

    /// A plate with the usual arrangement: ducts around the edge and between rows, and a
    /// DIN rail in the middle of every row: as many rows of at least [`ROW`] millimetres as fit,
    /// sharing the height evenly.
    pub fn with_rails(id: impl Into<String>, name: impl Into<String>, width: f64, height: f64) -> Self {
        let mut p = Self::new(id, name, width, height);
        if !(width.is_finite() && height.is_finite()) || width < 3.0 * DUCT || height < 2.0 * DUCT + ROW {
            return p;
        }
        p.items.push(PanelItem::Duct {
            min: Point::new(0.0, 0.0),
            max: Point::new(DUCT, height),
        });
        p.items.push(PanelItem::Duct {
            min: Point::new(width - DUCT, 0.0),
            max: Point::new(width, height),
        });
        let rows = ((height - DUCT) / (ROW + DUCT)).floor().clamp(1.0, 100.0) as usize;
        // Rows share the plate's height evenly, so no strip is left over at the bottom.
        let row = (height - DUCT * (rows as f64 + 1.0)) / rows as f64;
        for r in 0..=rows {
            let y = r as f64 * (row + DUCT);
            p.items.push(PanelItem::Duct {
                min: Point::new(DUCT, y),
                max: Point::new(width - DUCT, y + DUCT),
            });
            if r < rows {
                p.items.push(PanelItem::Rail {
                    at: Point::new(DUCT + 5.0, y + DUCT + row / 2.0),
                    length: width - 2.0 * DUCT - 10.0,
                });
            }
        }
        p
    }

    /// The right end a rail at `index` cannot be shortened past: the right edge of the
    /// devices clipped onto it (their middle within 20 mm of its centre line and their outline
    /// over its length). A rail drawn right to left (a negative length) counts from its real
    /// left end. `None` when it is not a rail or carries no device.
    pub fn rail_devices_end(&self, index: usize) -> Option<f64> {
        let Some(PanelItem::Rail { at, length }) = self.items.get(index) else {
            return None;
        };
        let (left, right) = (at.x.min(at.x + length), at.x.max(at.x + length));
        self.items
            .iter()
            .filter_map(PanelItem::device_bounds)
            .filter(|(a, b)| ((a.y + b.y) / 2.0 - at.y).abs() < 20.0 && b.x > left && a.x < right)
            .map(|(_, b)| b.x)
            .reduce(f64::max)
    }

    /// Changes the length of the rail at `index` or the size of the duct there, so the rail's
    /// right end or the duct's bottom-right corner goes to `to`, as near as it can: the left
    /// end or top-left corner stays put, the item stays on the plate and at least
    /// [`MIN_RESIZE`] millimetres long or wide, and a rail is never shortened past the
    /// devices on it (see [`Panel::rail_devices_end`]); they stay where they are. A rail drawn
    /// right to left (a negative length) is resized from its real left end and is stored from
    /// that end afterwards, like a duct whose corners were the other way round. Returns
    /// whether anything changed; a device, an index past the end, or an item whose left end
    /// or top-left corner is less than [`MIN_RESIZE`] from the plate's right or bottom edge
    /// changes nothing.
    pub fn resize_item(&mut self, index: usize, to: Point) -> bool {
        let (pw, ph) = (self.width, self.height);
        let devices_end = self.rail_devices_end(index);
        let Some(item) = self.items.get_mut(index) else {
            return false;
        };
        if !(to.x.is_finite() && to.y.is_finite()) {
            return false;
        }
        let before = item.clone();
        // The new length from `from`, or `None` when even the shortest one would stick out
        // past the plate's edge.
        let reach = |from: f64, to: f64, floor: f64, plate: f64| {
            let lo = (from + MIN_RESIZE).max(floor);
            (lo <= plate).then(|| to.clamp(lo, plate) - from)
        };
        match item {
            PanelItem::Rail { at, length } => {
                // A negative length runs left from `at`, so the left end is the far one.
                let left = if *length < 0.0 && length.is_finite() {
                    at.x + *length
                } else {
                    at.x
                };
                let Some(l) = reach(left, to.x, devices_end.unwrap_or(f64::MIN), pw) else {
                    return false;
                };
                at.x = left;
                *length = l;
            }
            PanelItem::Duct { min, max } => {
                let (a, _) = before.bounds();
                let (Some(w), Some(h)) = (reach(a.x, to.x, f64::MIN, pw), reach(a.y, to.y, f64::MIN, ph)) else {
                    return false;
                };
                *min = a;
                *max = Point::new(a.x + w, a.y + h);
            }
            PanelItem::Device { .. } => return false,
        }
        *item != before
    }

    /// Turn the device at `index` a quarter clockwise about its middle, keeping it on the plate.
    /// Returns false, changing nothing, when `index` is not a device or the turned device would
    /// not fit on the plate.
    pub fn rotate_device(&mut self, index: usize) -> bool {
        let (pw, ph) = (self.width, self.height);
        let Some(PanelItem::Device {
            at,
            width,
            height,
            rotation,
            ..
        }) = self.items.get_mut(index)
        else {
            return false;
        };
        let (w, h) = turned_size(*width, *height, *rotation);
        let middle = Point::new(at.x + w / 2.0, at.y + h / 2.0);
        let next = next_quarter(*rotation);
        let (w, h) = turned_size(*width, *height, next);
        // A turn whose outline is wider or taller than the plate would stick out of it.
        if w > pw || h > ph {
            return false;
        }
        *rotation = next;
        *at = Point::new(
            (middle.x - w / 2.0).clamp(0.0, (pw - w).max(0.0)),
            (middle.y - h / 2.0).clamp(0.0, (ph - h).max(0.0)),
        );
        true
    }

    /// Put the device `tag` on the plate, with the top-left corner of its outline turned by
    /// `rotation` at `at`, or move and turn it there when it is on the plate already. `size` is
    /// its upright width and height. Nothing changes and an error says why when the size is not
    /// a positive finite number or the turned outline would not lie on the plate.
    pub fn place_device(
        &mut self,
        tag: DeviceTag,
        at: Point,
        size: (f64, f64),
        rotation: Rotation,
    ) -> Result<(), String> {
        let (width, height) = size;
        if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
            return Err(format!("{tag}: size must be positive, got {width} x {height} mm"));
        }
        if !(at.x.is_finite() && at.y.is_finite()) {
            return Err(format!("{tag}: position must be finite"));
        }
        let (w, h) = turned_size(width, height, rotation);
        if !self.outline_fits(at, w, h) {
            return Err(format!(
                "{tag}: a {w} x {h} mm outline at {},{} is not on the {} x {} mm plate",
                at.x, at.y, self.width, self.height
            ));
        }
        let placed = PanelItem::Device {
            tag: tag.clone(),
            at,
            width,
            height,
            rotation,
        };
        match self
            .items
            .iter_mut()
            .find(|i| matches!(i, PanelItem::Device { tag: t, .. } if *t == tag))
        {
            Some(item) => *item = placed,
            None => self.items.push(placed),
        }
        Ok(())
    }

    /// Take the device `tag` off the plate. Returns false when it was not on it.
    pub fn remove_device(&mut self, tag: &DeviceTag) -> bool {
        let before = self.items.len();
        self.items
            .retain(|i| !matches!(i, PanelItem::Device { tag: t, .. } if t == tag));
        self.items.len() != before
    }

    /// The device `tag` on the plate, if it is there.
    pub fn device(&self, tag: &DeviceTag) -> Option<&PanelItem> {
        self.items
            .iter()
            .find(|i| matches!(i, PanelItem::Device { tag: t, .. } if t == tag))
    }

    /// Tags of the devices on the plate.
    pub fn device_tags(&self) -> BTreeSet<DeviceTag> {
        self.items
            .iter()
            .filter_map(|i| match i {
                PanelItem::Device { tag, .. } => Some(tag.clone()),
                _ => None,
            })
            .collect()
    }

    /// Change the plate's size to `width` x `height` millimetres, leaving rails, ducts and
    /// devices where they are. Nothing changes and an error says why when the size is not a
    /// positive finite number or something on the plate would stick out of the new size; the
    /// error names each such thing.
    pub fn resize(&mut self, width: f64, height: f64) -> Result<(), String> {
        if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
            return Err(format!("plate size must be positive, got {width} x {height} mm"));
        }
        let outside = |min: Point, max: Point| !outline_on_plate(width, height, min, max);
        let sticking_out: Vec<String> = self
            .items
            .iter()
            .filter_map(|item| match item {
                PanelItem::Rail { at, length } => {
                    // A 35 mm rail on its centre line. A negative length runs left from `at`.
                    let (min, max) = spanned(Point::new(at.x, at.y - 17.5), Point::new(at.x + length, at.y + 17.5));
                    outside(min, max).then(|| format!("rail at {},{}", at.x, at.y))
                }
                PanelItem::Duct { min, max } => {
                    // Corners may be given either way round; check the rectangle they span.
                    let (lo, hi) = spanned(*min, *max);
                    outside(lo, hi).then(|| format!("duct {},{} to {},{}", min.x, min.y, max.x, max.y))
                }
                PanelItem::Device { tag, .. } => {
                    let (min, max) = item.device_bounds()?;
                    // A negative width or height runs back from `at`; check the whole outline.
                    let (min, max) = spanned(min, max);
                    outside(min, max).then(|| tag.to_string())
                }
            })
            .collect();
        if !sticking_out.is_empty() {
            return Err(format!(
                "a {width} x {height} mm plate would leave these sticking out: {}",
                sticking_out.join(", ")
            ));
        }
        self.width = width;
        self.height = height;
        Ok(())
    }
}

/// A part's footprint (width, height) in millimetres, if it has one. Each dimension comes from
/// the part's `Width` or `Height` property, else from the extents of its footprint drawing
/// ([`crate::part::Footprint`]), else from the size of its 3D model ([`crate::ModelSize`]); a
/// part with one property and a drawing takes the other dimension from the drawing.
pub fn part_size(part: &Part) -> Option<(f64, f64)> {
    let find = |keys: &[&str]| {
        part.properties
            .iter()
            .find(|(k, _)| keys.iter().any(|key| k.trim().eq_ignore_ascii_case(key)))
            .and_then(|(_, v)| leading_number(v))
            .filter(|v| *v > 0.0 && *v <= 5000.0)
    };
    let w = find(&["width", "width mm", "breite", "bredde"]);
    let h = find(&["height", "height mm", "höhe", "hoehe", "høyde", "hoyde"]);
    // Each dimension: the part's own property first, else the footprint drawing's extent.
    let drawn = part
        .footprint
        .as_ref()
        .filter(|f| crate::part::usable_extent(f.width) && crate::part::usable_extent(f.height));
    let model = part.model.filter(crate::ModelSize::is_usable);
    Some((
        w.or(drawn.map(|f| f.width)).or(model.map(|m| m.width))?,
        h.or(drawn.map(|f| f.height)).or(model.map(|m| m.height))?,
    ))
}

/// A part's depth (out of the plate) in millimetres, if it has one: its `Depth` property, else
/// the depth of its 3D model ([`crate::ModelSize`]).
pub fn part_depth(part: &Part) -> Option<f64> {
    part.properties
        .iter()
        .find(|(k, _)| {
            matches!(
                k.trim().to_lowercase().as_str(),
                "depth" | "depth mm" | "tiefe" | "dybde"
            )
        })
        .and_then(|(_, v)| leading_number(v))
        .filter(|d| *d > 0.0 && *d <= MAX_DEPTH)
        .or_else(|| {
            part.model
                .filter(crate::ModelSize::is_usable)
                .map(|m| m.depth)
                .filter(|d| *d <= MAX_DEPTH)
        })
}

/// Deepest a device on a mounting plate may be.
pub const MAX_DEPTH: f64 = 2000.0;

/// Whether a part goes in the cabinet door (push buttons, pilot lights, meters) rather than
/// on the mounting plate: its `Mounting` property says `door` or `front`.
pub fn on_door(part: &Part) -> bool {
    part.properties.iter().any(|(k, v)| {
        let k = k.trim().to_lowercase();
        let v = v.trim().to_lowercase();
        matches!(k.as_str(), "mounting" | "montage" | "montering")
            && matches!(v.as_str(), "door" | "front" | "tür" | "dør")
    })
}

/// The number at the start of a value like `17.5 mm` or `17,5`.
fn leading_number(v: &str) -> Option<f64> {
    let s: String = v
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .map(|c| if c == ',' { '.' } else { c })
        .collect();
    s.parse().ok().filter(|v: &f64| v.is_finite())
}

/// What [`arrange`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Arranged {
    /// Devices put on the plate.
    pub placed: Vec<String>,
    /// Of those, the ones drawn with [`DEFAULT_SIZE`] because their part has no size.
    pub estimated: Vec<String>,
    /// Devices that did not fit on any rail.
    pub did_not_fit: Vec<String>,
}

/// Put the project's devices that have a part and are not on the plate yet onto its rails, in
/// tag order, left to right and then rail by rail, each centred on its rail. A device that would
/// stick out over the plate's top or bottom edge on a rail goes on a later rail where it does not,
/// or is listed as not fitting; only the part of a rail on the plate is used. Door-mounted parts ([`on_door`]) are left out. `location` (such as `CAB1`, with or without
/// the `+`) keeps only devices in that location. Devices already placed stay where they are.
pub fn arrange(
    panel: &mut Panel,
    project: &Project,
    parts: &BTreeMap<String, Part>,
    location: Option<&str>,
) -> Arranged {
    let wanted = location.map(|l| l.trim().trim_start_matches('+').to_owned());
    let mut tags: BTreeSet<DeviceTag> = project
        .pages
        .iter()
        .flat_map(|p| p.symbols.iter().filter_map(|s| s.tag.clone()))
        .collect();
    tags.extend(project.devices.keys().cloned());
    let on_plate = panel.device_tags();
    let mut todo: Vec<(DeviceTag, (f64, f64), bool)> = Vec::new();
    for tag in tags {
        if on_plate.contains(&tag) {
            continue;
        }
        if let Some(w) = &wanted
            && tag.location.join(".") != *w
        {
            continue;
        }
        let Some(part) = project
            .devices
            .get(&tag)
            .and_then(|d| d.part.as_ref())
            .and_then(|id| parts.get(id))
        else {
            continue;
        };
        if on_door(part) {
            continue;
        }
        match part_size(part) {
            Some(size) => todo.push((tag, size, false)),
            None => todo.push((tag, DEFAULT_SIZE, true)),
        }
    }
    // Free space on each rail starts after the devices already standing on it.
    let mut rails: Vec<(Point, f64, f64)> = panel
        .items
        .iter()
        .filter_map(|i| match i {
            PanelItem::Rail { at, length } => {
                let used_to = panel
                    .items
                    .iter()
                    .filter_map(|d| d.device_bounds())
                    // Only devices over this rail's own length: one on another rail further
                    // along the same row does not use up this one.
                    .filter(|(min, max)| min.y <= at.y && max.y >= at.y && max.x > at.x && min.x < at.x + length)
                    .map(|(_, max)| max.x + GAP)
                    // A rail starting left of the plate is used from the plate's edge.
                    .fold(at.x.max(0.0), f64::max);
                Some((*at, *length, used_to))
            }
            _ => None,
        })
        .collect();
    rails.sort_by(|a, b| a.0.y.total_cmp(&b.0.y).then(a.0.x.total_cmp(&b.0.x)));
    let mut out = Arranged::default();
    let plate_height = panel.height;
    let mut rail = 0;
    for (tag, (w, h), estimated) in todo {
        let name = tag.to_string();
        let mut placed = false;
        // From the first rail with room left; a rail too near the plate's edge for this device's
        // height is passed over for it alone, keeping its room for the devices after it.
        let mut r = rail;
        while let Some((at, length, cursor)) = rails.get_mut(r) {
            let top = at.y - h / 2.0;
            let on_plate = top >= -1e-9 && top + h <= plate_height + 1e-9;
            // Past any device already on the plate that this outline would cover, such as a
            // tall one from a lower rail reaching up over this one.
            let mut x = *cursor;
            while let Some(past) = panel
                .items
                .iter()
                .filter_map(PanelItem::device_bounds)
                .filter(|(min, max)| min.x < x + w && max.x > x && min.y < top + h && max.y > top)
                .map(|(_, max)| max.x + GAP)
                .reduce(f64::max)
            {
                x = past;
            }
            // Only the part of the rail that lies on the plate counts.
            let along = x + w <= (at.x + *length).min(panel.width) + 1e-9;
            if along && on_plate {
                panel.items.push(PanelItem::Device {
                    tag: tag.clone(),
                    at: Point::new(x, top),
                    width: w,
                    height: h,
                    rotation: Rotation::R0,
                });
                *cursor = x + w + GAP;
                placed = true;
                break;
            }
            // Only a device that could stand on this rail but finds it full uses it up; one
            // too tall for it leaves it to the devices after it.
            if !along && on_plate && r == rail {
                rail += 1;
            }
            r += 1;
        }
        if placed {
            if estimated {
                out.estimated.push(name.clone());
            }
            out.placed.push(name);
        } else {
            out.did_not_fit.push(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Device, Page, PlacedSymbol};

    fn tag(s: &str) -> Option<DeviceTag> {
        DeviceTag::parse(s).ok()
    }

    #[test]
    fn rails_and_ducts_have_an_outline_on_the_plate() {
        let rail = PanelItem::Rail {
            at: Point::new(20.0, 100.0),
            length: 300.0,
        };
        assert_eq!(rail.bounds(), (Point::new(20.0, 82.5), Point::new(320.0, 117.5)));
        // A duct drawn from its far corner still covers the same rectangle.
        let duct = PanelItem::Duct {
            min: Point::new(60.0, 40.0),
            max: Point::new(0.0, 0.0),
        };
        assert_eq!(duct.bounds(), (Point::new(0.0, 0.0), Point::new(60.0, 40.0)));
    }

    #[test]
    fn resize_refuses_items_running_backwards_or_with_a_nan_corner_off_the_plate() {
        let items = [
            PanelItem::Rail {
                at: Point::new(50.0, 100.0),
                length: -100.0,
            },
            PanelItem::Rail {
                at: Point::new(50.0, 100.0),
                length: f64::NAN,
            },
            PanelItem::Duct {
                min: Point::new(650.0, 100.0),
                max: Point::new(590.0, 140.0),
            },
            PanelItem::Duct {
                min: Point::new(f64::NAN, 100.0),
                max: Point::new(590.0, 140.0),
            },
            PanelItem::Device {
                tag: DeviceTag::default(),
                at: Point::new(50.0, 50.0),
                width: -100.0,
                height: 40.0,
                rotation: Rotation::R0,
            },
        ];
        for item in items {
            let mut p = Panel::new("p", "p", 600.0, 800.0);
            p.items.push(item.clone());
            assert!(p.resize(600.0, 800.0).is_err(), "{item:?} is refused");
        }
        // Reversed but on the plate is fine.
        let mut p = Panel::new("p", "p", 600.0, 800.0);
        p.items.push(PanelItem::Rail {
            at: Point::new(150.0, 100.0),
            length: -100.0,
        });
        p.items.push(PanelItem::Duct {
            min: Point::new(590.0, 140.0),
            max: Point::new(10.0, 100.0),
        });
        assert_eq!(p.resize(600.0, 800.0), Ok(()));
    }

    #[test]
    fn arranges_devices_on_rails_by_part_size() {
        let mut project = Project::new("x");
        let mut page = Page::new("p1", "1", "A");
        for (i, t) in ["+CAB1-S1", "+CAB1-Q1", "+CAB1-K1", "+CAB1-K2", "+FIELD-M1", "+CAB1-X9"]
            .iter()
            .enumerate()
        {
            page.symbols.push(PlacedSymbol {
                id: format!("s{i}"),
                symbol: "any".into(),
                at: Point::default(),
                rotation: Default::default(),
                mirror: false,
                tag: tag(t),
            });
        }
        project.pages.push(page);
        let mut parts = BTreeMap::new();
        let mut contactor = Part {
            id: "c".into(),
            ..Default::default()
        };
        contactor.properties.insert("Width".into(), "45 mm".into());
        contactor.properties.insert("Høyde".into(), "85,5".into());
        parts.insert("c".to_string(), contactor);
        let mut button = Part {
            id: "pb".into(),
            ..Default::default()
        };
        button.properties.insert("Mounting".into(), "Door".into());
        parts.insert("pb".to_string(), button);
        parts.insert(
            "plain".to_string(),
            Part {
                id: "plain".into(),
                ..Default::default()
            },
        );
        for (t, part) in [
            ("+CAB1-K1", "c"),
            ("+CAB1-K2", "c"),
            ("+CAB1-Q1", "plain"),
            ("+FIELD-M1", "c"),
        ] {
            if let Some(t) = tag(t) {
                project.devices.insert(
                    t,
                    Device {
                        part: Some(part.into()),
                        ..Default::default()
                    },
                );
            }
        }
        // A plate with one row: rail from x 45 to 155 (110 mm long).
        let mut panel = Panel::with_rails("main", "Main", 200.0, 205.0);
        let rails: Vec<_> = panel
            .items
            .iter()
            .filter(|i| matches!(i, PanelItem::Rail { .. }))
            .collect();
        assert_eq!(rails.len(), 1);
        assert!(
            matches!(rails[0], PanelItem::Rail { at, length } if at.x == 45.0 && at.y == 102.5 && *length == 110.0)
        );

        let done = arrange(&mut panel, &project, &parts, Some("+CAB1"));
        // In tag order: K1 (45), K2 (45), Q1 (18, no size) needs 18 but only 110-92 = 18 left.
        assert_eq!(done.placed, vec!["+CAB1-K1", "+CAB1-K2", "+CAB1-Q1"]);
        assert_eq!(done.estimated, vec!["+CAB1-Q1"]);
        assert!(done.did_not_fit.is_empty());
        let k2 = panel.items.iter().find_map(|i| match i {
            PanelItem::Device { tag: t, at, height, .. } if t.to_string() == "+CAB1-K2" => Some((*at, *height)),
            _ => None,
        });
        assert_eq!(k2, Some((Point::new(91.0, 102.5 - 85.5 / 2.0), 85.5)));

        // Arranging again places nothing new; the field device needs a plate of its own.
        assert_eq!(arrange(&mut panel, &project, &parts, Some("CAB1")), Arranged::default());
        let mut small = Panel::with_rails("s", "S", 150.0, 205.0);
        let done = arrange(&mut small, &project, &parts, None);
        assert_eq!(done.placed, vec!["+CAB1-K1"]);
        assert_eq!(done.did_not_fit.len(), 3);
    }

    #[test]
    fn a_part_without_a_size_takes_its_3d_model_size_last() {
        let model = crate::ModelSize {
            width: 30.0,
            height: 80.0,
            depth: 95.0,
        };
        let mut p = Part {
            model: Some(model),
            ..Part::default()
        };
        assert_eq!(part_size(&p), Some((30.0, 80.0)));
        assert_eq!(part_depth(&p), Some(95.0));
        p.properties.insert("Depth".into(), "60 mm".into());
        p.properties.insert("Width".into(), "45".into());
        assert_eq!(part_depth(&p), Some(60.0), "the part's own depth wins");
        assert_eq!(part_size(&p), Some((45.0, 80.0)), "and its own width");
        p.footprint = crate::Footprint::from_shapes(vec![crate::Shape::Rect {
            min: Point::new(0.0, 0.0),
            max: Point::new(22.5, 100.0),
        }]);
        assert_eq!(part_size(&p), Some((45.0, 100.0)), "the drawing before the model");

        let mut deep = Part {
            model: Some(crate::ModelSize { depth: 2500.0, ..model }),
            ..Part::default()
        };
        assert_eq!(part_depth(&deep), None, "deeper than a plate takes");
        deep.model = Some(crate::ModelSize { width: 0.0, ..model });
        assert_eq!(part_size(&deep), None, "an unusable model is no size");
        assert_eq!(part_depth(&deep), None);
    }

    #[test]
    fn a_part_without_a_size_takes_its_footprint_extents() {
        use crate::part::Footprint;
        let outline = |w: f64, h: f64| crate::Shape::Rect {
            min: Point::new(100.0, 50.0),
            max: Point::new(100.0 + w, 50.0 + h),
        };
        let Some(f) = Footprint::from_shapes(vec![outline(22.5, 100.0)]) else {
            unreachable!("a 22.5 x 100 outline is a footprint")
        };
        assert_eq!((f.width, f.height), (22.5, 100.0));
        assert_eq!(
            f.shapes[0],
            outline(22.5, 100.0).translate(Point::new(-100.0, -50.0)),
            "moved to the origin"
        );
        let mut p = Part {
            footprint: Some(f.clone()),
            ..Part::default()
        };
        assert_eq!(part_size(&p), Some((22.5, 100.0)));
        p.properties.insert("Width".into(), "45".into());
        p.properties.insert("Height".into(), "90".into());
        assert_eq!(part_size(&p), Some((45.0, 90.0)), "the part's own size wins");
        p.properties.remove("Height");
        assert_eq!(
            part_size(&p),
            Some((45.0, 100.0)),
            "a missing height comes from the drawing"
        );
        p.footprint = None;
        assert_eq!(part_size(&p), None, "one dimension alone is no size");
        // A quarter arc spans its radius, not its whole circle.
        let Some(arc) = Footprint::from_shapes(vec![crate::Shape::Arc {
            center: Point::new(50.0, 50.0),
            radius: 10.0,
            start: 0.0,
            end: 90.0,
        }]) else {
            unreachable!("a quarter arc has a size")
        };
        assert!(
            (arc.width - 10.0).abs() < 1e-9 && (arc.height - 10.0).abs() < 1e-9,
            "{arc:?}"
        );
        let Some(circle) = Footprint::from_shapes(vec![crate::Shape::Circle {
            center: Point::new(5.0, 5.0),
            radius: 5.0,
            filled: false,
        }]) else {
            unreachable!("a circle has a size")
        };
        assert_eq!((circle.width, circle.height), (10.0, 10.0));
        // An arc across a quarter point reaches it.
        let Some(over) = Footprint::from_shapes(vec![crate::Shape::Arc {
            center: Point::new(0.0, 0.0),
            radius: 10.0,
            start: 315.0,
            end: 45.0,
        }]) else {
            unreachable!()
        };
        assert!(
            (over.height - 2.0 * 10.0 * 45f64.to_radians().sin()).abs() < 1e-9,
            "{over:?}"
        );
        assert!(
            (over.width - 10.0 * (1.0 - 45f64.to_radians().cos())).abs() < 1e-9,
            "{over:?}"
        );
        // Angles far out of range (an imported ARC with 50=0, 51=-1e21) do not overflow.
        let far = Footprint::from_shapes(vec![crate::Shape::Arc {
            center: Point::new(0.0, 0.0),
            radius: 10.0,
            start: 1e21,
            end: -0.0,
        }]);
        assert!(far.is_some_and(|f| f.width <= 20.0 && f.height <= 20.0));
        // In a box of the drawing's size it is drawn as is; in a smaller box it shrinks evenly
        // and is centred; in a larger one it keeps its size and is centred.
        let at = Point::new(10.0, 20.0);
        assert_eq!(
            f.placed(at, 22.5, 100.0),
            vec![outline(22.5, 100.0).translate(Point::new(-90.0, -30.0))]
        );
        let crate::Shape::Rect { min, max } = &f.placed(at, 22.5, 50.0)[0] else {
            unreachable!()
        };
        assert!((max.y - min.y - 50.0).abs() < 1e-9 && (max.x - min.x - 11.25).abs() < 1e-9);
        assert!((min.x - (10.0 + 5.625)).abs() < 1e-9 && (min.y - 20.0).abs() < 1e-9);
        let crate::Shape::Rect { min, .. } = &f.placed(at, 45.0, 100.0)[0] else {
            unreachable!()
        };
        assert!((min.x - (10.0 + 11.25)).abs() < 1e-9);
        assert!(Footprint::from_shapes(Vec::new()).is_none());
        assert!(
            Footprint::from_shapes(vec![outline(0.0, 10.0)]).is_none(),
            "a flat drawing has no size"
        );
    }

    #[test]
    fn a_turned_device_is_stored_with_its_rotation_and_an_upright_one_without() {
        let Some(t) = tag("+CAB1-G1") else { unreachable!() };
        let device = |rotation| PanelItem::Device {
            tag: t.clone(),
            at: Point::new(10.0, 20.0),
            width: 22.5,
            height: 100.0,
            rotation,
        };
        let Ok(upright) = serde_json::to_string(&device(Rotation::R0)) else {
            unreachable!()
        };
        assert!(!upright.contains("rotation"), "existing files do not change: {upright}");
        let Ok(turned) = serde_json::to_string(&device(Rotation::R90)) else {
            unreachable!()
        };
        assert!(turned.contains(r#""rotation":"90""#), "{turned}");
        let Ok(back) = serde_json::from_str::<PanelItem>(&turned) else {
            unreachable!()
        };
        assert_eq!(back, device(Rotation::R90));
        let Ok(old) = serde_json::from_str::<PanelItem>(&upright) else {
            unreachable!()
        };
        assert_eq!(old, device(Rotation::R0));
        // Turned a quarter, it covers 100 x 22.5 mm on the plate.
        assert_eq!(
            device(Rotation::R90).device_bounds(),
            Some((Point::new(10.0, 20.0), Point::new(110.0, 42.5)))
        );
        assert_eq!(
            device(Rotation::R180).device_bounds(),
            Some((Point::new(10.0, 20.0), Point::new(32.5, 120.0)))
        );
    }

    #[test]
    fn rotating_a_device_turns_it_about_its_middle_and_keeps_it_on_the_plate() {
        let Some(t) = tag("-G1") else { unreachable!() };
        let mut p = Panel::new("p", "P", 300.0, 200.0);
        p.items.push(PanelItem::Rail {
            at: Point::new(0.0, 100.0),
            length: 300.0,
        });
        p.items.push(PanelItem::Device {
            tag: t,
            at: Point::new(100.0, 50.0),
            width: 20.0,
            height: 100.0,
            rotation: Rotation::R0,
        });
        assert!(!p.rotate_device(0), "a rail does not turn");
        assert!(p.rotate_device(1));
        // Middle (110, 100) stays; the outline is now 100 x 20.
        assert_eq!(
            p.items[1].device_bounds(),
            Some((Point::new(60.0, 90.0), Point::new(160.0, 110.0)))
        );
        assert!(p.rotate_device(1) && p.rotate_device(1) && p.rotate_device(1));
        assert_eq!(
            p.items[1].device_bounds(),
            Some((Point::new(100.0, 50.0), Point::new(120.0, 150.0)))
        );
        // Near the edge, turning keeps the outline on the plate.
        if let PanelItem::Device { at, .. } = &mut p.items[1] {
            *at = Point::new(0.0, 50.0);
        }
        assert!(p.rotate_device(1));
        assert_eq!(
            p.items[1].device_bounds(),
            Some((Point::new(0.0, 90.0), Point::new(100.0, 110.0)))
        );
        assert!(!p.rotate_device(9));
        // Turned, a device longer than the plate is wide would stick out: refused.
        let mut narrow = Panel::new("n", "N", 60.0, 120.0);
        let Some(t) = tag("-G2") else { unreachable!() };
        narrow.items.push(PanelItem::Device {
            tag: t,
            at: Point::new(20.0, 10.0),
            width: 20.0,
            height: 100.0,
            rotation: Rotation::R0,
        });
        let before = narrow.clone();
        assert!(!narrow.rotate_device(0));
        assert_eq!(narrow, before);
    }

    #[test]
    fn a_footprint_turns_with_its_device() {
        // A line along the part's top edge, and its frame.
        let Some(f) = crate::part::Footprint::from_shapes(vec![
            crate::Shape::Rect {
                min: Point::new(0.0, 0.0),
                max: Point::new(20.0, 100.0),
            },
            crate::Shape::Line {
                a: Point::new(0.0, 0.0),
                b: Point::new(20.0, 0.0),
            },
        ]) else {
            unreachable!()
        };
        let at = Point::new(10.0, 30.0);
        assert_eq!(
            f.placed_turned(at, 20.0, 100.0, Rotation::R0),
            f.placed(at, 20.0, 100.0)
        );
        let frame_and_edge = |r| {
            let shapes = f.placed_turned(at, 20.0, 100.0, r);
            let (crate::Shape::Rect { min, max }, crate::Shape::Line { a, b }) = (shapes[0].clone(), shapes[1].clone())
            else {
                unreachable!()
            };
            (min, max, a, b)
        };
        let close = |p: Point, x: f64, y: f64| (p.x - x).abs() < 1e-9 && (p.y - y).abs() < 1e-9;
        // A quarter clockwise: the frame lies on its side and the top edge becomes the right edge.
        let (min, max, a, b) = frame_and_edge(Rotation::R90);
        assert!(close(min, 10.0, 30.0) && close(max, 110.0, 50.0), "{min:?} {max:?}");
        assert!(close(a, 110.0, 30.0) && close(b, 110.0, 50.0), "{a:?} {b:?}");
        // Half a turn: the top edge is at the bottom.
        let (min, max, a, b) = frame_and_edge(Rotation::R180);
        assert!(close(min, 10.0, 30.0) && close(max, 30.0, 130.0));
        assert!(close(a, 30.0, 130.0) && close(b, 10.0, 130.0), "{a:?} {b:?}");
        // Three quarters: the top edge is the left edge.
        let (min, max, a, b) = frame_and_edge(Rotation::R270);
        assert!(close(min, 10.0, 30.0) && close(max, 110.0, 50.0));
        assert!(close(a, 10.0, 50.0) && close(b, 10.0, 30.0), "{a:?} {b:?}");
    }

    #[test]
    fn an_outline_fits_on_the_plate_up_to_its_edge_but_not_past_it() {
        let p = Panel::new("p", "P", 600.0, 800.0);
        // Exactly on each edge.
        assert!(p.outline_fits(Point::new(0.0, 0.0), 600.0, 800.0));
        assert!(p.outline_fits(Point::new(555.0, 715.0), 45.0, 85.0));
        // Decimals that add up to a hair over the edge (0.1 + 0.2 > 0.3) still count as flush.
        let q = Panel::new("q", "Q", 0.3, 0.3);
        assert!(q.outline_fits(Point::new(0.1, 0.1), 0.2, 0.2));
        // Just past an edge, on every side.
        assert!(!p.outline_fits(Point::new(555.001, 0.0), 45.0, 85.0));
        assert!(!p.outline_fits(Point::new(0.0, 715.001), 45.0, 85.0));
        assert!(!p.outline_fits(Point::new(-0.001, 0.0), 45.0, 85.0));
        assert!(!p.outline_fits(Point::new(0.0, -0.001), 45.0, 85.0));
        // The same rule for a plate size that is not the panel's own, as resizing uses.
        let (min, max) = (Point::new(100.0, 400.0), Point::new(145.0, 485.0));
        assert!(outline_on_plate(145.0, 485.0, min, max));
        assert!(!outline_on_plate(144.999, 485.0, min, max));
        assert!(!outline_on_plate(145.0, 484.999, min, max));
        // Something that is not a number is never on the plate.
        assert!(!p.outline_fits(Point::new(f64::NAN, 0.0), 45.0, 85.0));
        assert!(!p.outline_fits(Point::new(0.0, 0.0), f64::NAN, 85.0));
        assert!(!p.outline_fits(Point::new(0.0, 0.0), f64::INFINITY, 85.0));
        assert!(!outline_on_plate(f64::NAN, 485.0, min, max));
        assert!(!outline_on_plate(145.0, 485.0, Point::new(100.0, f64::NAN), max));
    }

    #[test]
    fn resizing_a_plate_keeps_its_layout_and_refuses_a_size_that_leaves_things_out() {
        let mut p = Panel::with_rails("p", "P", 600.0, 800.0);
        let Some(t) = tag("-K1") else { unreachable!() };
        let Ok(()) = p.place_device(t, Point::new(100.0, 400.0), (45.0, 85.0), Rotation::R0) else {
            unreachable!()
        };
        let items = p.items.clone();
        // Taller: rails, ducts and the device stay as they are.
        let Ok(()) = p.resize(600.0, 1000.0) else {
            unreachable!()
        };
        assert_eq!((p.width, p.height), (600.0, 1000.0));
        assert_eq!(p.items, items);
        // Narrower than the ducts and rails reach, and shorter than the device sits: refused,
        // naming them, and nothing changes.
        let before = p.clone();
        let Err(err) = p.resize(300.0, 450.0) else {
            unreachable!()
        };
        assert!(
            err.contains("-K1") && err.contains("rail at ") && err.contains("duct "),
            "{err}"
        );
        assert_eq!(p, before);
        // Only the device sticks out.
        let mut lone = Panel::new("l", "L", 600.0, 800.0);
        lone.items = vec![items[items.len() - 1].clone()];
        let Err(err) = lone.resize(140.0, 800.0) else {
            unreachable!()
        };
        assert_eq!(err, "a 140 x 800 mm plate would leave these sticking out: -K1");
        // Flush with the new edge is fine.
        let Ok(()) = lone.resize(145.0, 485.0) else {
            unreachable!()
        };
        for (w, h) in [(0.0, 100.0), (100.0, -1.0), (f64::NAN, 100.0), (100.0, f64::INFINITY)] {
            assert!(lone.resize(w, h).is_err(), "{w} x {h}");
        }
        assert_eq!((lone.width, lone.height), (145.0, 485.0));
    }

    #[test]
    fn a_device_taller_than_its_row_goes_on_a_rail_where_it_stays_on_the_plate() {
        let mut project = Project::new("x");
        let mut parts = BTreeMap::new();
        for (id, h) in [("tall", 250.0), ("huge", 900.0), ("small", 80.0)] {
            let mut part = Part {
                id: id.into(),
                ..Default::default()
            };
            part.properties.insert("Width".into(), "45".into());
            part.properties.insert("Height".into(), format!("{h}"));
            parts.insert(id.to_string(), part);
        }
        for (t, part) in [("-A1", "tall"), ("-A2", "huge"), ("-K1", "small")] {
            if let Some(t) = tag(t) {
                project.devices.insert(
                    t,
                    Device {
                        part: Some(part.into()),
                        ..Default::default()
                    },
                );
            }
        }
        // Four rows of 150 mm; the first rail is at y 115, the second at y 305.
        let mut panel = Panel::with_rails("main", "Main", 600.0, 800.0);
        let done = arrange(&mut panel, &project, &parts, None);
        assert_eq!(done.placed, vec!["-A1", "-K1"]);
        assert_eq!(done.did_not_fit, vec!["-A2"], "taller than the plate");
        for item in &panel.items {
            if let Some((min, max)) = item.device_bounds() {
                assert!(min.y >= 0.0 && max.y <= panel.height, "{item:?} is off the plate");
            }
        }
        let at = |t: &str| {
            panel.items.iter().find_map(|i| match i {
                PanelItem::Device { tag, at, .. } if tag.to_string() == t => Some(*at),
                _ => None,
            })
        };
        // The tall device goes down to the second rail; the small one after it still uses the
        // first rail's room.
        assert_eq!(at("-A1"), Some(Point::new(45.0, 305.0 - 125.0)));
        assert_eq!(at("-K1"), Some(Point::new(45.0, 115.0 - 40.0)));
    }

    #[test]
    fn a_device_is_not_put_over_a_tall_one_from_a_lower_rail() {
        let mut project = Project::new("x");
        let mut parts = BTreeMap::new();
        for (id, h) in [("tall", 500.0), ("small", 80.0)] {
            let mut part = Part {
                id: id.into(),
                ..Default::default()
            };
            part.properties.insert("Width".into(), "45".into());
            part.properties.insert("Height".into(), format!("{h}"));
            parts.insert(id.to_string(), part);
        }
        for (t, part) in [("-A1", "tall"), ("-K1", "small")] {
            if let Some(t) = tag(t) {
                project.devices.insert(
                    t,
                    Device {
                        part: Some(part.into()),
                        ..Default::default()
                    },
                );
            }
        }
        // -A1 goes on the second rail (y 305) and reaches up over the first (y 115).
        let mut panel = Panel::with_rails("main", "Main", 600.0, 800.0);
        let done = arrange(&mut panel, &project, &parts, None);
        assert_eq!(done.placed, vec!["-A1", "-K1"]);
        let outlines: Vec<(Point, Point)> = panel.items.iter().filter_map(PanelItem::device_bounds).collect();
        assert_eq!(outlines[0], (Point::new(45.0, 55.0), Point::new(90.0, 555.0)));
        // -K1 stays on the first rail, beside -A1 rather than inside it.
        assert_eq!(outlines[1], (Point::new(91.0, 75.0), Point::new(136.0, 155.0)));
    }

    #[test]
    fn a_device_too_tall_for_every_rail_leaves_the_rails_to_the_rest() {
        let mut project = Project::new("x");
        let mut parts = BTreeMap::new();
        for (id, w, h) in [("tall", 80.0, 500.0), ("huge", 45.0, 900.0), ("small", 18.0, 80.0)] {
            let mut part = Part {
                id: id.into(),
                ..Default::default()
            };
            part.properties.insert("Width".into(), format!("{w}"));
            part.properties.insert("Height".into(), format!("{h}"));
            parts.insert(id.to_string(), part);
        }
        for (t, part) in [("-A1", "tall"), ("-A2", "huge"), ("-K1", "small")] {
            if let Some(t) = tag(t) {
                project.devices.insert(
                    t,
                    Device {
                        part: Some(part.into()),
                        ..Default::default()
                    },
                );
            }
        }
        // Rails from x 45 to 155 at y 115, 305, 495 and 685.
        let mut panel = Panel::with_rails("main", "Main", 200.0, 800.0);
        let done = arrange(&mut panel, &project, &parts, None);
        assert_eq!(done.placed, vec!["-A1", "-K1"]);
        assert_eq!(done.did_not_fit, vec!["-A2"]);
        let outlines: Vec<(Point, Point)> = panel.items.iter().filter_map(PanelItem::device_bounds).collect();
        assert_eq!(outlines[0], (Point::new(45.0, 55.0), Point::new(125.0, 555.0)));
        // -K1 still gets the first rail, beside -A1.
        assert_eq!(outlines[1], (Point::new(126.0, 75.0), Point::new(144.0, 155.0)));
    }

    #[test]
    fn hostile_sizes() {
        assert!(Panel::with_rails("a", "a", f64::NAN, 1e300).items.is_empty());
        assert!(Panel::with_rails("a", "a", 1e9, 1e9).items.len() < 500);
        let mut p = Part::default();
        p.properties.insert("width".into(), "1e999".into());
        p.properties.insert("height".into(), "-4".into());
        assert_eq!(part_size(&p), None);
    }

    #[test]
    fn a_rail_moves_by_its_left_end_and_stays_on_the_plate() {
        let mut rail = PanelItem::Rail {
            at: Point::new(10.0, 50.0),
            length: 200.0,
        };
        rail.move_on_plate(Point::new(40.0, 120.0), 600.0, 400.0);
        assert_eq!(rail.anchor(), Point::new(40.0, 120.0));
        // Past the right and top edges it stops with its ends and its 35 mm height on the plate.
        rail.move_on_plate(Point::new(500.0, 0.0), 600.0, 400.0);
        assert_eq!(rail.anchor(), Point::new(400.0, 17.5));
        let PanelItem::Rail { length, .. } = rail else {
            unreachable!()
        };
        assert_eq!(length, 200.0);
    }

    #[test]
    fn a_duct_moves_whole_and_stays_on_the_plate() {
        let mut duct = PanelItem::Duct {
            min: Point::new(0.0, 0.0),
            max: Point::new(60.0, 300.0),
        };
        duct.move_on_plate(Point::new(100.0, 20.0), 600.0, 400.0);
        assert_eq!(duct.bounds(), (Point::new(100.0, 20.0), Point::new(160.0, 320.0)));
        duct.move_on_plate(Point::new(-30.0, 250.0), 600.0, 400.0);
        assert_eq!(duct.bounds(), (Point::new(0.0, 100.0), Point::new(60.0, 400.0)));
        // Larger than the plate: it goes to the top edge.
        duct.move_on_plate(Point::new(0.0, 50.0), 600.0, 200.0);
        assert_eq!(duct.bounds(), (Point::new(0.0, 0.0), Point::new(60.0, 300.0)));
    }

    /// A rail on a 600 x 400 plate from x 100 to 300 on y 100, with a 50 mm device on it
    /// ending at x 210.
    fn rail_with_device() -> Panel {
        let mut p = Panel::new("p", "p", 600.0, 400.0);
        p.items.push(PanelItem::Rail {
            at: Point::new(100.0, 100.0),
            length: 200.0,
        });
        p.items.push(PanelItem::Device {
            tag: DeviceTag::default(),
            at: Point::new(160.0, 55.0),
            width: 50.0,
            height: 90.0,
            rotation: Rotation::R0,
        });
        p
    }

    fn rail_length(p: &Panel) -> f64 {
        let Some(PanelItem::Rail { at, length }) = p.items.first() else {
            unreachable!()
        };
        assert_eq!(*at, Point::new(100.0, 100.0), "the left end stays put");
        *length
    }

    #[test]
    fn a_rail_resizes_by_its_right_end_and_stays_on_the_plate() {
        let mut p = rail_with_device();
        assert!(p.resize_item(0, Point::new(450.0, 0.0)));
        assert_eq!(rail_length(&p), 350.0);
        // Past the plate's right edge it stops there.
        assert!(p.resize_item(0, Point::new(900.0, 100.0)));
        assert_eq!(rail_length(&p), 500.0);
        assert!(
            !p.resize_item(0, Point::new(900.0, 100.0)),
            "no change, nothing to undo"
        );
        assert!(!p.resize_item(1, Point::new(300.0, 300.0)), "a device is not resized");
        assert!(!p.resize_item(5, Point::new(300.0, 300.0)));
    }

    #[test]
    fn a_rail_is_not_shortened_past_its_devices() {
        let mut p = rail_with_device();
        assert_eq!(p.rail_devices_end(0), Some(210.0));
        assert!(p.resize_item(0, Point::new(120.0, 100.0)));
        assert_eq!(rail_length(&p), 110.0);
        assert_eq!(p.items[1].anchor(), Point::new(160.0, 55.0), "the device stays");
        // Without the device it goes down to the shortest rail.
        p.items.pop();
        assert_eq!(p.rail_devices_end(0), None);
        assert!(p.resize_item(0, Point::new(-50.0, 100.0)));
        assert_eq!(rail_length(&p), MIN_RESIZE);
    }

    #[test]
    fn a_rail_drawn_right_to_left_resizes_by_its_real_ends() {
        // The same rail as `rail_with_device`, drawn from x 300 back to x 100.
        let mut p = rail_with_device();
        p.items[0] = PanelItem::Rail {
            at: Point::new(300.0, 100.0),
            length: -200.0,
        };
        assert_eq!(p.rail_devices_end(0), Some(210.0));
        // Shortened, it stops at its device and keeps its left end at x 100.
        assert!(p.resize_item(0, Point::new(120.0, 100.0)));
        assert_eq!(rail_length(&p), 110.0);
        assert_eq!(p.items[1].anchor(), Point::new(160.0, 55.0), "the device stays");
        assert!(p.resize_item(0, Point::new(450.0, 100.0)));
        assert_eq!(rail_length(&p), 350.0);
    }

    #[test]
    fn a_duct_resizes_by_its_far_corner_and_stays_on_the_plate() {
        let mut p = Panel::new("p", "p", 600.0, 400.0);
        // Corners stored the other way round still resize from the top-left one.
        p.items.push(PanelItem::Duct {
            min: Point::new(100.0, 360.0),
            max: Point::new(0.0, 300.0),
        });
        assert!(p.resize_item(0, Point::new(250.0, 340.0)));
        assert_eq!(p.items[0].bounds(), (Point::new(0.0, 300.0), Point::new(250.0, 340.0)));
        assert!(p.resize_item(0, Point::new(-20.0, 900.0)));
        assert_eq!(
            p.items[0].bounds(),
            (Point::new(0.0, 300.0), Point::new(MIN_RESIZE, 400.0))
        );
        assert!(!p.resize_item(0, Point::new(f64::NAN, 10.0)));
    }

    #[test]
    fn an_item_too_near_the_plates_far_edge_is_not_resized_off_it() {
        let mut p = Panel::new("p", "p", 600.0, 400.0);
        p.items.push(PanelItem::Rail {
            at: Point::new(595.0, 100.0),
            length: 5.0,
        });
        p.items.push(PanelItem::Duct {
            min: Point::new(595.0, 100.0),
            max: Point::new(600.0, 120.0),
        });
        p.items.push(PanelItem::Duct {
            min: Point::new(100.0, 395.0),
            max: Point::new(200.0, 400.0),
        });
        let before = p.items.clone();
        assert!(!p.resize_item(0, Point::new(700.0, 100.0)));
        assert!(!p.resize_item(1, Point::new(700.0, 300.0)));
        assert!(!p.resize_item(2, Point::new(300.0, 500.0)));
        assert_eq!(p.items, before, "nothing changes");
    }

    #[test]
    fn arrange_ignores_devices_on_another_rail_in_the_same_row() {
        // Two rails side by side on one row; the right one already carries a device.
        let mut panel = Panel::new("p", "P", 600.0, 300.0);
        panel.items.push(PanelItem::Rail {
            at: Point::new(10.0, 150.0),
            length: 200.0,
        });
        panel.items.push(PanelItem::Rail {
            at: Point::new(300.0, 150.0),
            length: 200.0,
        });
        let (Some(on_right), Some(new)) = (tag("-K1"), tag("-K2")) else {
            unreachable!()
        };
        let Ok(()) = panel.place_device(on_right.clone(), Point::new(300.0, 105.0), (45.0, 90.0), Rotation::R0) else {
            unreachable!()
        };
        let mut project = Project::new("x");
        let mut parts = BTreeMap::new();
        let mut part = Part {
            id: "c".into(),
            ..Default::default()
        };
        part.properties.insert("Width".into(), "45".into());
        part.properties.insert("Height".into(), "90".into());
        parts.insert("c".to_string(), part);
        for t in [&on_right, &new] {
            project.devices.insert(
                t.clone(),
                Device {
                    part: Some("c".into()),
                    ..Default::default()
                },
            );
        }
        let done = arrange(&mut panel, &project, &parts, None);
        assert_eq!(done.placed, vec![new.to_string()]);
        assert_eq!(
            panel.device(&new).and_then(PanelItem::device_bounds),
            Some((Point::new(10.0, 105.0), Point::new(55.0, 195.0))),
            "the empty left rail takes it from its own left end"
        );
    }

    #[test]
    fn arrange_uses_only_the_part_of_a_rail_on_the_plate() {
        // One rail starts 30 mm left of the plate, the other runs 50 mm past its right edge.
        let mut panel = Panel::new("p", "P", 200.0, 300.0);
        panel.items.push(PanelItem::Rail {
            at: Point::new(-30.0, 75.0),
            length: 100.0,
        });
        panel.items.push(PanelItem::Rail {
            at: Point::new(100.0, 225.0),
            length: 150.0,
        });
        let mut project = Project::new("x");
        let mut parts = BTreeMap::new();
        let mut part = Part {
            id: "c".into(),
            ..Default::default()
        };
        part.properties.insert("Width".into(), "45".into());
        part.properties.insert("Height".into(), "90".into());
        parts.insert("c".to_string(), part);
        let tags: Vec<DeviceTag> = ["-K1", "-K2", "-K3", "-K4"].iter().filter_map(|t| tag(t)).collect();
        for t in &tags {
            project.devices.insert(
                t.clone(),
                Device {
                    part: Some("c".into()),
                    ..Default::default()
                },
            );
        }
        let done = arrange(&mut panel, &project, &parts, None);
        // 0..70 on the plate holds one 45 mm device; 100..200 holds two.
        assert_eq!(done.placed.len(), 3, "{done:?}");
        assert_eq!(done.did_not_fit, vec![tags[3].to_string()]);
        assert_eq!(
            panel.device(&tags[0]).and_then(PanelItem::device_bounds),
            Some((Point::new(0.0, 30.0), Point::new(45.0, 120.0)))
        );
        for t in &tags[..3] {
            let Some((min, max)) = panel.device(t).and_then(PanelItem::device_bounds) else {
                unreachable!()
            };
            assert!(
                panel.outline_fits(min, max.x - min.x, max.y - min.y),
                "{t} at {min:?} is on the plate"
            );
        }
    }
}

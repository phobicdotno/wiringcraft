//! Panel layouts: the mounting plate of a cabinet with its DIN rails, wire ducts and the
//! devices placed on them, in millimetres with y pointing down from the plate's top-left corner.
//!
//! Each layout is a file `panels/<id>.panel.json`. Device footprints come from the part's
//! `Width` and `Height` properties; [`arrange`] puts devices that are not on the plate yet onto
//! the rails, left to right and top to bottom.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::geom::Point;
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
    /// A device's footprint; `at` is its top-left corner.
    Device {
        tag: DeviceTag,
        at: Point,
        width: f64,
        height: f64,
    },
}

impl Panel {
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
}

/// A part's footprint (width, height) in millimetres from its properties, if it has one.
pub fn part_size(part: &Part) -> Option<(f64, f64)> {
    let find = |keys: &[&str]| {
        part.properties
            .iter()
            .find(|(k, _)| keys.iter().any(|key| k.trim().eq_ignore_ascii_case(key)))
            .and_then(|(_, v)| leading_number(v))
            .filter(|v| *v > 0.0 && *v <= 5000.0)
    };
    let w = find(&["width", "width mm", "breite", "bredde"])?;
    let h = find(&["height", "height mm", "höhe", "hoehe", "høyde", "hoyde"])?;
    Some((w, h))
}

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
/// tag order, left to right and then rail by rail. Door-mounted parts ([`on_door`]) are left out. `location` (such as `CAB1`, with or without
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
                    .filter_map(|d| match d {
                        PanelItem::Device {
                            at: da, width, height, ..
                        } if da.y <= at.y && da.y + height >= at.y && da.x + width > at.x => Some(da.x + width + GAP),
                        _ => None,
                    })
                    .fold(at.x, f64::max);
                Some((*at, *length, used_to))
            }
            _ => None,
        })
        .collect();
    rails.sort_by(|a, b| a.0.y.total_cmp(&b.0.y).then(a.0.x.total_cmp(&b.0.x)));
    let mut out = Arranged::default();
    let mut rail = 0;
    for (tag, (w, h), estimated) in todo {
        let name = tag.to_string();
        let mut placed = false;
        while let Some((at, length, cursor)) = rails.get_mut(rail) {
            if *cursor + w <= at.x + *length + 1e-9 {
                panel.items.push(PanelItem::Device {
                    tag: tag.clone(),
                    at: Point::new(*cursor, at.y - h / 2.0),
                    width: w,
                    height: h,
                });
                *cursor += w + GAP;
                placed = true;
                break;
            }
            rail += 1;
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
    fn hostile_sizes() {
        assert!(Panel::with_rails("a", "a", f64::NAN, 1e300).items.is_empty());
        assert!(Panel::with_rails("a", "a", 1e9, 1e9).items.len() < 500);
        let mut p = Part::default();
        p.properties.insert("width".into(), "1e999".into());
        p.properties.insert("height".into(), "-4".into());
        assert_eq!(part_size(&p), None);
    }
}

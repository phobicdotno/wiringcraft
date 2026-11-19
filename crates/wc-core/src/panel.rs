//! Panel layouts: the mounting plate of a cabinet with its DIN rails, wire ducts and the
//! devices placed on them, in millimetres with y pointing down from the plate's top-left corner.
//!
//! Each layout is a file `panels/<id>.panel.json`. Device footprints come from the part's
//! `Width` and `Height` properties, or the extents of its footprint drawing; [`arrange`] puts devices that are not on the plate yet onto
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
        .filter(|f| f.width > 0.0 && f.height > 0.0 && f.width <= 5000.0 && f.height <= 5000.0);
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
    fn hostile_sizes() {
        assert!(Panel::with_rails("a", "a", f64::NAN, 1e300).items.is_empty());
        assert!(Panel::with_rails("a", "a", 1e9, 1e9).items.len() < 500);
        let mut p = Part::default();
        p.properties.insert("width".into(), "1e999".into());
        p.properties.insert("height".into(), "-4".into());
        assert_eq!(part_size(&p), None);
    }
}

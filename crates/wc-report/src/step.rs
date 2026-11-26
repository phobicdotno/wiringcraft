//! A panel layout as a 3D model in STEP (ISO 10303-21, AP214): the mounting plate, DIN rails,
//! wire ducts and every device as an exact box solid named after what it is, so a CAD program
//! (SolidCraft or any other) can check the cabinet's depth, door clearance and fit.
//!
//! Millimetres; x runs right along the plate, y up from its bottom edge, z out of the plate.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use wc_core::panel::{Panel, PanelItem};
use wc_core::{Part, Project};

/// Mounting plate thickness.
pub const PLATE: f64 = 2.0;
/// Height of a top-hat rail (35 x 7.5).
pub const RAIL: f64 = 7.5;
/// Height of a wire duct.
pub const DUCT_HEIGHT: f64 = 60.0;
/// Depth of a device whose part does not say.
pub const DEFAULT_DEPTH: f64 = 75.0;

/// One axis-aligned box: name, min corner and max corner.
type Block = (String, [f64; 3], [f64; 3]);

/// A part's depth (out of the plate): its `Depth` property, else its 3D model's depth.
pub use wc_core::panel::part_depth;

/// A panel layout as STEP, with the devices whose depth was guessed.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelStep {
    /// The STEP file text.
    pub text: String,
    /// Tags of the devices drawn [`DEFAULT_DEPTH`] deep because their part gives no depth.
    pub default_depth: Vec<String>,
}

/// The boxes of a panel layout (plate, rails, ducts and devices), and the tags of the devices
/// drawn [`DEFAULT_DEPTH`] deep.
fn blocks(project: &Project, panel: &Panel, parts: &BTreeMap<String, Part>) -> (Vec<Block>, Vec<String>) {
    let top = panel.height;
    // Page y (down from the top) to model y (up from the bottom).
    let y = |v: f64| top - v;
    let mut out: Vec<Block> = vec![(
        format!("Mounting plate {}", panel.name),
        [0.0, 0.0, 0.0],
        [panel.width, panel.height, PLATE],
    )];
    let mut guessed: Vec<String> = Vec::new();
    let mut rails: Vec<(f64, f64, f64)> = Vec::new();
    for item in &panel.items {
        match item {
            PanelItem::Rail { at, length } => {
                rails.push((at.x, at.x + length, at.y));
                out.push((
                    "DIN rail".into(),
                    [at.x, y(at.y + 17.5), PLATE],
                    [at.x + length, y(at.y - 17.5), PLATE + RAIL],
                ));
            }
            PanelItem::Duct { min, max } => out.push((
                "Wire duct".into(),
                [min.x, y(max.y), PLATE],
                [max.x, y(min.y), PLATE + DUCT_HEIGHT],
            )),
            PanelItem::Device { .. } => {}
        }
    }
    for item in &panel.items {
        if let (PanelItem::Device { tag, .. }, Some((min, max))) = (item, item.device_bounds()) {
            // The block covers the turned outline.
            let (at, width, height) = (min, max.x - min.x, max.y - min.y);
            let depth = project
                .devices
                .get(tag)
                .and_then(|d| d.part.as_ref())
                .and_then(|id| parts.get(id))
                .and_then(part_depth);
            let depth = depth.unwrap_or_else(|| {
                guessed.push(tag.to_string());
                DEFAULT_DEPTH
            });
            // A device on a rail sits on the rail; anything else on the plate.
            let middle = at.y + height / 2.0;
            let on_rail = rails
                .iter()
                .any(|(x0, x1, ry)| (middle - ry).abs() < 1.0 && at.x + width > *x0 && at.x < *x1);
            let base = if on_rail { PLATE + RAIL } else { PLATE };
            out.push((
                tag.to_string(),
                [at.x, y(at.y + height), base],
                [at.x + width, y(at.y), base + depth],
            ));
        }
    }
    out.retain(|(_, a, b)| (0..3).all(|k| a[k].is_finite() && b[k].is_finite() && b[k] - a[k] > 1e-6));
    (out, guessed)
}

/// STEP text with one solid per plate, rail, duct and device, and the tags of the devices
/// whose depth was guessed. `stamp` is the ISO 8601 time written in the header.
pub fn panel_step(project: &Project, panel: &Panel, parts: &BTreeMap<String, Part>, stamp: &str) -> PanelStep {
    let mut w = Writer::default();
    let (boxes, default_depth) = blocks(project, panel, parts);
    let solids: Vec<usize> = boxes.iter().map(|(name, a, b)| w.block(name, *a, *b)).collect();
    let refs: Vec<String> = solids.iter().map(|s| format!("#{s}")).collect();
    // Product structure and units, after the geometry.
    let ctx_app = w.add("APPLICATION_CONTEXT('automotive design')");
    w.add(&format!(
        "APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#{ctx_app})"
    ));
    let pctx = w.add(&format!("PRODUCT_CONTEXT('',#{ctx_app},'mechanical')"));
    let name = step_text(&format!("{} {}", project.meta.name, panel.name));
    let product = w.add(&format!("PRODUCT({name},{name},'',(#{pctx}))"));
    let pdf = w.add(&format!("PRODUCT_DEFINITION_FORMATION('','',#{product})"));
    let pdctx = w.add(&format!(
        "PRODUCT_DEFINITION_CONTEXT('part definition',#{ctx_app},'design')"
    ));
    let pd = w.add(&format!("PRODUCT_DEFINITION('design','',#{pdf},#{pdctx})"));
    let pds = w.add(&format!("PRODUCT_DEFINITION_SHAPE('','',#{pd})"));
    let len = w.add("(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.))");
    let ang = w.add("(NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.))");
    let sol = w.add("(NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT())");
    let unc = w.add(&format!(
        "UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-06),#{len},'distance_accuracy_value','')"
    ));
    let gctx = w.add(&format!(
        "(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{unc})) GLOBAL_UNIT_ASSIGNED_CONTEXT((#{len},#{ang},#{sol})) REPRESENTATION_CONTEXT('',''))"
    ));
    let origin = w.placement([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let rep = w.add(&format!(
        "ADVANCED_BREP_SHAPE_REPRESENTATION('',(#{origin},{}),#{gctx})",
        refs.join(",")
    ));
    w.add(&format!("SHAPE_DEFINITION_REPRESENTATION(#{pds},#{rep})"));

    let mut out = String::new();
    let _ = write!(
        out,
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('WiringCraft panel layout'),'2;1');\nFILE_NAME({},{},(''),(''),'WiringCraft','WiringCraft','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN {{ 1 0 10303 214 1 1 1 1 }}'));\nENDSEC;\nDATA;\n",
        step_text(&panel.id),
        step_text(stamp)
    );
    for (i, e) in w.entities.iter().enumerate() {
        let _ = writeln!(out, "#{}={e};", i + 1);
    }
    out.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    PanelStep {
        text: out,
        default_depth,
    }
}

/// A STEP string: quotes doubled, non-ASCII as `\X2\` hex, which every reader takes.
fn step_text(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\'' => out.push_str("''"),
            '\\' => out.push_str("\\\\"),
            ' '..='~' => out.push(c),
            _ => {
                let mut buf = [0u16; 2];
                out.push_str("\\X2\\");
                for unit in c.encode_utf16(&mut buf) {
                    let _ = write!(out, "{unit:04X}");
                }
                out.push_str("\\X0\\");
            }
        }
    }
    out.push('\'');
    out
}

fn real(v: f64) -> String {
    let s = format!("{v:.6}");
    // STEP reals keep their decimal point: `45.`, `9.5`.
    s.trim_end_matches('0').to_owned()
}

#[derive(Default)]
struct Writer {
    entities: Vec<String>,
}

impl Writer {
    /// Add an entity; returns its number.
    fn add(&mut self, e: &str) -> usize {
        self.entities.push(e.to_owned());
        self.entities.len()
    }

    fn point(&mut self, p: [f64; 3]) -> usize {
        self.add(&format!(
            "CARTESIAN_POINT('',({},{},{}))",
            real(p[0]),
            real(p[1]),
            real(p[2])
        ))
    }

    fn dir(&mut self, d: [f64; 3]) -> usize {
        self.add(&format!("DIRECTION('',({},{},{}))", real(d[0]), real(d[1]), real(d[2])))
    }

    fn placement(&mut self, at: [f64; 3], axis: [f64; 3], refd: [f64; 3]) -> usize {
        let p = self.point(at);
        let a = self.dir(axis);
        let r = self.dir(refd);
        self.add(&format!("AXIS2_PLACEMENT_3D('',#{p},#{a},#{r})"))
    }

    /// An exact box solid from `a` (min corner) to `b` (max corner).
    fn block(&mut self, name: &str, a: [f64; 3], b: [f64; 3]) -> usize {
        // Corners: bit 0 = x, bit 1 = y, bit 2 = z at max.
        let corner = |i: usize| {
            [
                if i & 1 == 0 { a[0] } else { b[0] },
                if i & 2 == 0 { a[1] } else { b[1] },
                if i & 4 == 0 { a[2] } else { b[2] },
            ]
        };
        let vertices: Vec<usize> = (0..8)
            .map(|i| {
                let p = self.point(corner(i));
                self.add(&format!("VERTEX_POINT('',#{p})"))
            })
            .collect();
        // Edges along each axis, from the lower corner to the upper one.
        let mut edges: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        for i in 0..8usize {
            for bit in [1usize, 2, 4] {
                if i & bit == 0 {
                    let j = i | bit;
                    let axis = match bit {
                        1 => [1.0, 0.0, 0.0],
                        2 => [0.0, 1.0, 0.0],
                        _ => [0.0, 0.0, 1.0],
                    };
                    let k = bit.trailing_zeros() as usize;
                    let length = b[k] - a[k];
                    let p = self.point(corner(i));
                    let d = self.dir(axis);
                    let v = self.add(&format!("VECTOR('',#{d},{})", real(length)));
                    let line = self.add(&format!("LINE('',#{p},#{v})"));
                    let (vi, vj) = (vertices[i], vertices[j]);
                    let e = self.add(&format!("EDGE_CURVE('',#{vi},#{vj},#{line},.T.)"));
                    edges.insert((i, j), e);
                }
            }
        }
        // Faces with outward normals; each loop runs counter-clockwise seen from outside.
        let faces: [([usize; 4], [f64; 3], [f64; 3]); 6] = [
            ([0, 2, 3, 1], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]),
            ([4, 5, 7, 6], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            ([0, 1, 5, 4], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0]),
            ([2, 6, 7, 3], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
            ([0, 4, 6, 2], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([1, 3, 7, 5], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ];
        let mut face_ids = Vec::new();
        for (ring, normal, refd) in faces {
            let mut oriented = Vec::new();
            for k in 0..4 {
                let (from, to) = (ring[k], ring[(k + 1) % 4]);
                let (key, forward) = if from < to {
                    ((from, to), true)
                } else {
                    ((to, from), false)
                };
                let Some(e) = edges.get(&key) else { continue };
                let o = self.add(&format!(
                    "ORIENTED_EDGE('',*,*,#{e},{})",
                    if forward { ".T." } else { ".F." }
                ));
                oriented.push(format!("#{o}"));
            }
            let lp = self.add(&format!("EDGE_LOOP('',({}))", oriented.join(",")));
            let bound = self.add(&format!("FACE_OUTER_BOUND('',#{lp},.T.)"));
            let pl = self.placement(corner(ring[0]), normal, refd);
            let plane = self.add(&format!("PLANE('',#{pl})"));
            face_ids.push(format!(
                "#{}",
                self.add(&format!("ADVANCED_FACE('',(#{bound}),#{plane},.T.)"))
            ));
        }
        let shell = self.add(&format!("CLOSED_SHELL('',({}))", face_ids.join(",")));
        self.add(&format!("MANIFOLD_SOLID_BREP({},#{shell})", step_text(name)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wc_core::DeviceTag;
    use wc_core::geom::Point;
    use wc_core::project::Device;

    #[test]
    fn a_turned_device_is_a_turned_block() -> Result<(), wc_core::Error> {
        let project = Project::new("x");
        let mut panel = Panel::new("main", "Cabinet", 400.0, 300.0);
        panel.items.push(PanelItem::Device {
            tag: DeviceTag::parse("-G1")?,
            at: Point::new(50.0, 100.0),
            width: 20.0,
            height: 100.0,
            rotation: wc_core::geom::Rotation::R90,
        });
        let step = panel_step(&project, &panel, &BTreeMap::new(), "2026-11-19T23:00:00").text;
        // Lying on its side it covers x 50..150 and y 100..120 (model y 180..200).
        assert!(step.contains("CARTESIAN_POINT('',(50.,180.,2.))"), "{step}");
        assert!(step.contains("CARTESIAN_POINT('',(150.,200.,77.))"));
        Ok(())
    }

    #[test]
    fn panel_as_step_solids() -> Result<(), wc_core::Error> {
        let mut project = Project::new("Pump station Ålesund");
        let tag = DeviceTag::parse("+CAB1-K1")?;
        project.devices.insert(
            tag.clone(),
            Device {
                part: Some("c".into()),
                ..Default::default()
            },
        );
        let mut part = Part::default();
        part.properties.insert("Depth".into(), "95 mm".into());
        let parts: BTreeMap<String, Part> = [("c".to_string(), part)].into();
        let mut panel = Panel::with_rails("main", "Cabinet 'A'", 400.0, 205.0);
        // On the rail (middle at 102.5) and one loose on the plate.
        panel.items.push(PanelItem::Device {
            tag: tag.clone(),
            at: Point::new(45.0, 60.0),
            width: 45.0,
            height: 85.0,
            rotation: wc_core::geom::Rotation::R0,
        });
        panel.items.push(PanelItem::Device {
            tag: DeviceTag::parse("-X9")?,
            at: Point::new(300.0, 50.0),
            width: 10.0,
            height: 10.0,
            rotation: wc_core::geom::Rotation::R0,
        });
        let done = panel_step(&project, &panel, &parts, "2026-11-16T23:00:00");
        // Only X9 has no depth to go on.
        assert_eq!(done.default_depth, ["-X9"]);
        let step = done.text;
        assert!(step.starts_with("ISO-10303-21;"));
        assert!(step.ends_with("END-ISO-10303-21;\n"));
        // Plate, 2 side ducts, 2 row ducts, 1 rail, 2 devices.
        assert_eq!(step.matches("MANIFOLD_SOLID_BREP(").count(), 8);
        assert_eq!(step.matches("ADVANCED_FACE(").count(), 48);
        assert!(step.contains("MANIFOLD_SOLID_BREP('+CAB1-K1',"));
        assert!(step.contains("'Pump station \\X2\\00C5\\X0\\lesund Cabinet ''A'''"));
        // K1 sits on the rail (z 9.5) and is 95 deep; X9 sits on the plate and is 75 deep.
        assert!(step.contains("CARTESIAN_POINT('',(45.,60.,9.5))"), "K1 bottom corner");
        assert!(step.contains("CARTESIAN_POINT('',(90.,145.,104.5))"), "K1 top corner");
        assert!(step.contains("CARTESIAN_POINT('',(300.,145.,2.))"), "X9 on the plate");
        assert!(step.contains("CARTESIAN_POINT('',(310.,155.,77.))"));
        // Every entity reference points at an entity that exists.
        let count = step.lines().filter(|l| l.starts_with('#')).count();
        for r in step.split('#').skip(1) {
            let n: String = r.chars().take_while(char::is_ascii_digit).collect();
            let n: usize = n.parse().unwrap_or(0);
            assert!(n >= 1 && n <= count, "#{n} of {count}");
        }
        Ok(())
    }

    #[test]
    fn every_edge_is_used_once_each_way() {
        // In a closed box each edge appears in two faces, once forwards and once backwards.
        let mut w = Writer::default();
        w.block("b", [0.0; 3], [1.0, 2.0, 3.0]);
        let mut uses: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for e in &w.entities {
            if let Some(rest) = e.strip_prefix("ORIENTED_EDGE('',*,*,") {
                let (edge, sense) = rest.split_once(',').unwrap_or(("", ""));
                let u = uses.entry(edge.to_owned()).or_default();
                if sense.starts_with(".T.") {
                    u.0 += 1;
                } else {
                    u.1 += 1;
                }
            }
        }
        assert_eq!(uses.len(), 12);
        assert!(uses.values().all(|u| *u == (1, 1)), "{uses:?}");
    }
}

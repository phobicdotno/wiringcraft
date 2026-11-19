//! A part's size from the maker's 3D model in STEP (ISO 10303-21, AP203, AP214 or AP242): the
//! bounding box of its geometry, in millimetres (#313).
//!
//! What counts: the vertices of the model's boundary (B-rep), the full extent of every circle and
//! ellipse (a cylinder's round side has no vertex at its widest point), and the control points
//! of B-spline curves (a curve lies inside them). A model without vertices is measured by its
//! tessellated coordinates, else by its loose points. B-spline surfaces are not measured: their
//! control nets often reach far past the trimmed faces.
//!
//! The length unit is read from the file (millimetre, metre, inch, ...). The model's axes are
//! taken as they are: x across the plate, y up it and z out of it.
//!
//! In an assembly each part is measured where it is placed, nested assemblies included (see
//! the `assembly` module).

use std::collections::{BTreeMap, BTreeSet};

use wc_core::ModelSize;

use crate::{Error, Result, printable};

/// Deepest nesting of brackets read in one entity.
const MAX_NESTING: usize = 32;

/// A piece of measured geometry: a point, or a whole circle or ellipse, kept as its centre and
/// its two semi-axis vectors so it can be turned with its part and still be measured exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Geom {
    Point([f64; 3]),
    Conic { centre: [f64; 3], u: [f64; 3], v: [f64; 3] },
}

impl Geom {
    /// The two corners of its box: on each axis k a conic spans its centre plus and minus
    /// hypot(u_k, v_k).
    fn bounds(&self) -> [[f64; 3]; 2] {
        match *self {
            Geom::Point(p) => [p, p],
            Geom::Conic { centre, u, v } => {
                let mut lo = centre;
                let mut hi = centre;
                for k in 0..3 {
                    let e = u[k].hypot(v[k]);
                    lo[k] -= e;
                    hi[k] += e;
                }
                [lo, hi]
            }
        }
    }
}

/// `r` made square to the unit axis `n`, of length 1, with how far it was from the axis (the
/// length of n x r, the sine of the angle between them). (n x r) x n is r's part square to the
/// axis, with no cancellation of a large along-axis part.
fn square_to(n: [f64; 3], r: [f64; 3]) -> Option<([f64; 3], f64)> {
    let side = cross(n, unit(r)?);
    Some((unit(cross(side, n))?, dot(side, side).sqrt()))
}

/// Rounding noise, from a direction along an axis that is not tilted exactly the same.
const NOISE: f64 = 64.0 * f64::EPSILON;

/// One parameter of an entity instance.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Ref(u64),
    Num(f64),
    Str(String),
    Enum(String),
    List(Vec<Value>),
    /// A typed parameter such as `LENGTH_MEASURE(25.4)`.
    Typed(String, Vec<Value>),
    /// `$` or `*`.
    Unset,
}

impl Value {
    /// Every instance this value refers to, in lists and typed parameters too.
    fn collect_refs(&self, out: &mut BTreeSet<u64>) {
        match self {
            Value::Ref(id) => {
                out.insert(*id);
            }
            Value::List(items) | Value::Typed(_, items) => {
                for item in items {
                    item.collect_refs(out);
                }
            }
            _ => {}
        }
    }

    fn as_ref_id(&self) -> Option<u64> {
        match self {
            Value::Ref(id) => Some(*id),
            _ => None,
        }
    }

    fn as_num(&self) -> Option<f64> {
        match self {
            Value::Num(v) if v.is_finite() => Some(*v),
            Value::Typed(_, args) if args.len() == 1 => args[0].as_num(),
            _ => None,
        }
    }

    fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(items) => Some(items),
            _ => None,
        }
    }
}

/// An entity instance: one or more (name, parameters) records; more than one for a complex
/// instance such as `(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.))`.
type Instance = Vec<(String, Vec<Value>)>;

/// Entity names whose parameters are read; everything else is only counted.
const READ: &[&str] = &[
    "CARTESIAN_POINT",
    "VERTEX_POINT",
    "CIRCLE",
    "ELLIPSE",
    "AXIS2_PLACEMENT_3D",
    "DIRECTION",
    "B_SPLINE_CURVE_WITH_KNOTS",
    "QUASI_UNIFORM_CURVE",
    "UNIFORM_CURVE",
    "BEZIER_CURVE",
    "POLYLINE",
    "COORDINATES_LIST",
    "CONVERSION_BASED_UNIT",
    "LENGTH_MEASURE_WITH_UNIT",
    "ITEM_DEFINED_TRANSFORMATION",
    "MAPPED_ITEM",
    "CARTESIAN_TRANSFORMATION_OPERATOR_3D",
    "GLOBAL_UNIT_ASSIGNED_CONTEXT",
];

/// The bounding box of a STEP model and warnings about what may make it wrong. An error when
/// the text is not STEP or holds no 3D geometry of a usable size.
pub fn model_size(text: &str) -> Result<(ModelSize, Vec<String>)> {
    if !text.trim_start().starts_with("ISO-10303-21") {
        return Err(Error::Format(
            "not a STEP file: it does not start with ISO-10303-21".into(),
        ));
    }
    // A file cut short could still measure as a smaller part: only a whole exchange is read.
    if !text.trim_end().ends_with("END-ISO-10303-21;") {
        return Err(Error::Format(
            "the STEP file is incomplete: it does not end with END-ISO-10303-21;".into(),
        ));
    }
    let (instances, unread) = parse(text);
    let mut warnings = Vec::new();
    if unread > 0 {
        warnings.push(format!("{unread} entities could not be read and were left out"));
    }
    let point = |id: u64| -> Option<[f64; 3]> {
        let (name, args) = simple(&instances, id)?;
        (name == "CARTESIAN_POINT").then_some(())?;
        coords(args.get(1)?)
    };

    // Each point with the instance it was measured from, to place it in its assembly.
    let mut pts: Vec<(u64, Geom)> = Vec::new();
    // The instances whose points are measured: their representations pick the length unit.
    let mut measured: BTreeSet<u64> = BTreeSet::new();
    let mut vertices = 0usize;
    for (id, inst) in &instances {
        for (name, args) in inst {
            if name == "VERTEX_POINT"
                && let Some(p) = args.get(1).and_then(Value::as_ref_id).and_then(point)
            {
                pts.push((*id, Geom::Point(p)));
                vertices += 1;
                measured.insert(*id);
            }
        }
    }
    // Curves: whole circles and ellipses, and the control points of B-spline curves. They bound
    // a B-rep's round edges, and are the geometry of a wireframe model without vertices.
    let mut curves: Vec<(u64, Geom)> = Vec::new();
    let mut doubtful = 0usize;
    for (id, inst) in &instances {
        for (name, args) in inst {
            let found: Vec<Geom> = match name.as_str() {
                "CIRCLE" | "ELLIPSE" => match conic(&instances, name, args, &point) {
                    Some((g, unsure)) => {
                        doubtful += usize::from(unsure);
                        vec![g]
                    }
                    None => Vec::new(),
                },
                // Simple instances name the curve first: (name, degree, points, ...); in a
                // complex instance B_SPLINE_CURVE(degree, points, ...) has no name.
                "B_SPLINE_CURVE_WITH_KNOTS" | "BEZIER_CURVE" | "UNIFORM_CURVE" | "QUASI_UNIFORM_CURVE"
                    if inst.len() == 1 =>
                {
                    points(curve_points(args.get(2), &point))
                }
                "B_SPLINE_CURVE" => points(curve_points(args.get(1), &point)),
                // A wireframe's straight edges: its corner points.
                "POLYLINE" => points(curve_points(args.get(1), &point)),
                _ => Vec::new(),
            };
            if !found.is_empty() {
                measured.insert(*id);
                curves.extend(found.into_iter().map(|g| (*id, g)));
            }
        }
    }
    if vertices == 0 {
        // Tessellated models (AP242) list their coordinates.
        for (id, inst) in &instances {
            for (name, args) in inst {
                if name == "COORDINATES_LIST"
                    && let Some(list) = args.get(2).and_then(Value::as_list)
                {
                    let before = pts.len();
                    pts.extend(list.iter().filter_map(coords).map(|p| (*id, Geom::Point(p))));
                    if pts.len() > before {
                        measured.insert(*id);
                    }
                }
            }
        }
    }
    pts.extend(curves);
    if doubtful > 0 {
        warnings.push(format!(
            "{doubtful} ellipses have a reference direction all but along their axis; their turn \
             about the axis, and so the size, may be off"
        ));
    }
    if pts.is_empty() {
        // Else every loose point that is not only the origin of a placement or of a
        // transformation operator (its local origin, third of its last five attributes): a point
        // something else lists too, such as a representation, is geometry as well.
        let origin_at = |name: &str, args: &[Value]| match name {
            "AXIS2_PLACEMENT_3D" => Some(1),
            "CARTESIAN_TRANSFORMATION_OPERATOR_3D" => args.len().checked_sub(3),
            _ => None,
        };
        let mut origins: BTreeSet<u64> = BTreeSet::new();
        let mut listed: BTreeSet<u64> = BTreeSet::new();
        for (name, args) in instances.values().flatten() {
            let at = origin_at(name, args);
            for (k, arg) in args.iter().enumerate() {
                if Some(k) == at {
                    origins.extend(arg.as_ref_id());
                } else {
                    arg.collect_refs(&mut listed);
                }
            }
        }
        let origins: BTreeSet<u64> = origins.difference(&listed).copied().collect();
        for id in instances.keys().filter(|id| !origins.contains(id)) {
            if let Some(p) = point(*id) {
                pts.push((*id, Geom::Point(p)));
                measured.insert(*id);
            }
        }
    }
    if pts.is_empty() {
        return Err(Error::Format(
            "the model has no 3D geometry WiringCraft can measure".into(),
        ));
    }

    let (mm, unit_warning) = length_unit(text, &instances, &measured);
    warnings.extend(unit_warning);

    // An assembly's parts, each moved to where it is placed.
    let placed = assembly::place(text, &instances, pts, &point);
    if placed.missed > 0 {
        warnings.push(format!(
            "the model is an assembly ({} placed parts); {} of its placements could not be applied, so its size may be off",
            placed.placements, placed.missed
        ));
    }
    if placed.cut {
        warnings.push(
            "the assembly nests too deep, places its parts in a loop or uses too many of them; \
             some parts were measured where they are drawn or by their own box, so its size may be off"
                .into(),
        );
    }
    let pts = placed.points;

    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for g in &pts {
        let [lo, hi] = g.bounds();
        for k in 0..3 {
            min[k] = min[k].min(lo[k]);
            max[k] = max[k].max(hi[k]);
        }
    }
    let size = ModelSize {
        width: (max[0] - min[0]) * mm,
        height: (max[1] - min[1]) * mm,
        depth: (max[2] - min[2]) * mm,
    };
    if !size.is_usable() {
        return Err(Error::Format(format!(
            "the model is {} x {} x {} mm, not a usable part size (each side above 0 and at most {} mm)",
            size.width,
            size.height,
            size.depth,
            ModelSize::MAX
        )));
    }
    Ok((size, warnings))
}

/// The one record of a simple instance.
fn simple(instances: &BTreeMap<u64, Instance>, id: u64) -> Option<(&str, &[Value])> {
    match instances.get(&id)?.as_slice() {
        [(name, args)] => Some((name.as_str(), args.as_slice())),
        _ => None,
    }
}

/// Three finite coordinates from a list such as `(1.,2.,3.)`; 2D points are not 3D geometry.
fn coords(v: &Value) -> Option<[f64; 3]> {
    match v.as_list()? {
        [x, y, z] => Some([x.as_num()?, y.as_num()?, z.as_num()?]),
        _ => None,
    }
}

fn points(list: Vec<[f64; 3]>) -> Vec<Geom> {
    list.into_iter().map(Geom::Point).collect()
}

/// The points of a curve's control list.
fn curve_points(list: Option<&Value>, point: &impl Fn(u64) -> Option<[f64; 3]>) -> Vec<[f64; 3]> {
    list.and_then(Value::as_list)
        .map(|items| items.iter().filter_map(Value::as_ref_id).filter_map(point).collect())
        .unwrap_or_default()
}

/// A whole circle or ellipse as its centre and semi-axis vectors: the first semi-axis runs
/// along the placement's reference direction `u` and the second along `v` = axis x `u`. With
/// whether its turn about the axis is in doubt.
fn conic(
    instances: &BTreeMap<u64, Instance>,
    name: &str,
    args: &[Value],
    point: &impl Fn(u64) -> Option<[f64; 3]>,
) -> Option<(Geom, bool)> {
    let (a, b) = match name {
        "CIRCLE" => {
            let r = args.get(2)?.as_num()?;
            (r, r)
        }
        _ => (args.get(2)?.as_num()?, args.get(3)?.as_num()?),
    };
    if !(a.is_finite() && a > 0.0 && b.is_finite() && b > 0.0) {
        return None;
    }
    let (kind, placement) = simple(instances, args.get(1)?.as_ref_id()?)?;
    (kind == "AXIS2_PLACEMENT_3D").then_some(())?;
    let centre = point(placement.get(1)?.as_ref_id()?)?;
    let direction = |v: Option<&Value>| -> Option<Option<[f64; 3]>> {
        match v {
            Some(Value::Ref(id)) => {
                let (kind, d) = simple(instances, *id)?;
                (kind == "DIRECTION").then_some(())?;
                Some(Some(coords(d.get(1)?)?))
            }
            _ => Some(None),
        }
    };
    // No axis given: the placement's z axis.
    let n = unit(direction(placement.get(2))?.unwrap_or([0.0, 0.0, 1.0]))?;
    // The reference direction made square to the axis; without a usable one, any direction in
    // the curve's plane (the standard's own default).
    let along = |r: [f64; 3]| square_to(n, r);
    let noise = NOISE;
    // The file's own reference direction is taken however close to the axis it lies, as long
    // as it is not on it; one that close to the axis that rounding could have made it is
    // reported, as the curve's turn about its axis may then be off.
    let given = direction(placement.get(3))?.and_then(along);
    let mut doubtful = false;
    let u = match given {
        Some((u, len)) => {
            doubtful = len <= noise && a != b;
            u
        }
        None => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
            .into_iter()
            .filter_map(along)
            .find(|(_, len)| *len > noise)
            .map(|(u, _)| u)?,
    };
    let v = cross(n, u);
    Some((
        Geom::Conic {
            centre,
            u: [a * u[0], a * u[1], a * u[2]],
            v: [b * v[0], b * v[1], b * v[2]],
        },
        doubtful,
    ))
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `v` scaled to length 1, if it is not zero. Scaled by its largest component first, so a
/// direction given in tiny or huge ratios keeps its way.
fn unit(v: [f64; 3]) -> Option<[f64; 3]> {
    let big = v.iter().fold(0.0f64, |m, c| m.max(c.abs()));
    if !(big.is_finite() && big > 0.0) {
        return None;
    }
    let w = [v[0] / big, v[1] / big, v[2] / big];
    let len = dot(w, w).sqrt();
    Some([w[0] / len, w[1] / len, w[2] / len])
}

/// Millimetres per length unit of the file, and a warning when that is a guess. The units the
/// geometry's contexts assign (`GLOBAL_UNIT_ASSIGNED_CONTEXT`) count; only a file without such
/// a context falls back to every length unit it defines (a conversion-based unit names its base
/// unit too, so that would find both).
fn length_unit(text: &str, instances: &BTreeMap<u64, Instance>, measured: &BTreeSet<u64>) -> (f64, Option<String>) {
    // The shape representations (with items) and the contexts they are drawn in; when none is
    // named, every context that assigns units.
    let representations: Vec<(Vec<u64>, u64)> = instances
        .values()
        .flatten()
        .filter(|(name, _)| name.ends_with("REPRESENTATION"))
        .filter_map(|(_, args)| {
            let items: Vec<u64> = args.get(1)?.as_list()?.iter().filter_map(Value::as_ref_id).collect();
            let context = args.get(2)?.as_ref_id()?;
            (!items.is_empty()).then_some((items, context))
        })
        .collect();
    let units_of = |id: &u64| -> Vec<u64> {
        instances
            .get(id)
            .into_iter()
            .flatten()
            .filter(|(name, _)| name == "GLOBAL_UNIT_ASSIGNED_CONTEXT")
            .filter_map(|(_, args)| args.first().and_then(Value::as_list))
            .flat_map(|units| units.iter().filter_map(Value::as_ref_id))
            .collect()
    };
    let context_mm = |context: u64| -> Vec<f64> {
        let mut mm: Vec<f64> = Vec::new();
        for unit in units_of(&context) {
            if let Some(v) = unit_mm(instances, unit, 0)
                && !mm.iter().any(|f| (f - v).abs() <= f.abs() * 1e-9)
            {
                mm.push(v);
            }
        }
        mm
    };
    let mut used_contexts: Vec<u64> = representations.iter().map(|(_, c)| *c).collect();
    used_contexts.sort_unstable();
    used_contexts.dedup();
    let differ = used_contexts
        .iter()
        .map(|c| context_mm(*c))
        .collect::<Vec<_>>()
        .windows(2)
        .any(|w| w[0].len() != w[1].len() || w[0].iter().zip(&w[1]).any(|(a, b)| (a - b).abs() > a.abs() * 1e-9));
    if differ {
        // The contexts disagree: keep those whose representation reaches the measured geometry.
        let reaching = geometry_contexts(text, measured, &representations);
        if !reaching.is_empty() {
            used_contexts = reaching;
        }
    }
    let mut assigned: Vec<u64> = used_contexts.iter().flat_map(units_of).collect();
    if assigned.is_empty() {
        assigned = instances.keys().flat_map(units_of).collect();
    }
    let candidates: Vec<u64> = if assigned.is_empty() {
        instances.keys().copied().collect()
    } else {
        assigned
    };
    let mut found: Vec<f64> = Vec::new();
    for id in candidates {
        if let Some(mm) = unit_mm(instances, id, 0)
            && !found.iter().any(|f| (f - mm).abs() <= f.abs() * 1e-9)
        {
            found.push(mm);
        }
    }
    match found.as_slice() {
        [] => (
            1.0,
            Some("the file names no length unit; millimetres are assumed".into()),
        ),
        [mm] => (*mm, None),
        [mm, ..] => (
            *mm,
            Some(format!(
                "the file names {} length units; the first ({mm} mm) is used",
                found.len()
            )),
        ),
    }
}

/// The contexts of the representations whose items lead, by references, to the instances that
/// were measured (`geometry`). Only run when contexts disagree, as it scans every instance's references.
fn geometry_contexts(text: &str, geometry: &BTreeSet<u64>, representations: &[(Vec<u64>, u64)]) -> Vec<u64> {
    let mut links: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for statement in statements(text) {
        let Some((id, body)) = statement.trim().strip_prefix('#').and_then(|r| r.split_once('=')) else {
            continue;
        };
        if let Ok(id) = id.trim().parse::<u64>() {
            links.insert(id, refs_in(body));
        }
    }
    let mut out: Vec<u64> = Vec::new();
    for (items, context) in representations {
        if out.contains(context) {
            continue;
        }
        let mut seen: BTreeSet<u64> = BTreeSet::new();
        let mut todo: Vec<u64> = items.clone();
        let mut reaches = false;
        while let Some(id) = todo.pop() {
            if !seen.insert(id) {
                continue;
            }
            if geometry.contains(&id) {
                reaches = true;
                break;
            }
            if let Some(next) = links.get(&id) {
                todo.extend(next.iter().filter(|n| !seen.contains(n)));
            }
        }
        if reaches {
            out.push(*context);
        }
    }
    out
}

/// The instance references (`#123`) in an instance's text, outside strings.
fn refs_in(body: &str) -> Vec<u64> {
    let mut out = Vec::new();
    let b = body.as_bytes();
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            in_str = c != b'\'';
            i += 1;
            continue;
        }
        if c == b'\'' {
            in_str = true;
            i += 1;
        } else if c == b'#' {
            let start = i + 1;
            let mut end = start;
            while end < b.len() && b[end].is_ascii_digit() {
                end += 1;
            }
            if let Some(id) = body.get(start..end).and_then(|d| d.parse().ok()) {
                out.push(id);
            }
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

/// Millimetres per unit for an instance that is a length unit.
fn unit_mm(instances: &BTreeMap<u64, Instance>, id: u64, depth: usize) -> Option<f64> {
    let inst = instances.get(&id)?;
    if depth > 4 || !inst.iter().any(|(name, _)| name == "LENGTH_UNIT") {
        return None;
    }
    for (name, args) in inst {
        match name.as_str() {
            "SI_UNIT" => {
                let Some(Value::Enum(unit)) = args.get(1) else {
                    return None;
                };
                if unit != "METRE" {
                    return None;
                }
                let per_metre = match args.first() {
                    Some(Value::Enum(prefix)) => match prefix.as_str() {
                        "KILO" => 1e6,
                        "HECTO" => 1e5,
                        "DECA" => 1e4,
                        "DECI" => 100.0,
                        "CENTI" => 10.0,
                        "MILLI" => 1.0,
                        "MICRO" => 1e-3,
                        "NANO" => 1e-6,
                        _ => return None,
                    },
                    _ => 1000.0,
                };
                return Some(per_metre);
            }
            "CONVERSION_BASED_UNIT" => {
                if let Some(Value::Str(unit)) = args.first() {
                    match unit.trim().to_lowercase().as_str() {
                        "inch" | "in" => return Some(25.4),
                        "foot" | "ft" => return Some(304.8),
                        _ => {}
                    }
                }
                // Else as its measure says: LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(v), #unit).
                let (kind, measure) = simple(instances, args.get(1)?.as_ref_id()?)?;
                (kind == "LENGTH_MEASURE_WITH_UNIT").then_some(())?;
                let v = measure.first()?.as_num()?;
                let base = unit_mm(instances, measure.get(1)?.as_ref_id()?, depth + 1)?;
                return Some(v * base).filter(|mm| mm.is_finite() && *mm > 0.0);
            }
            _ => {}
        }
    }
    None
}

/// The entity instances of the file whose parameters matter here, by id, and how many
/// instances could not be read.
fn parse(text: &str) -> (BTreeMap<u64, Instance>, usize) {
    let mut out = BTreeMap::new();
    let mut unread = 0usize;
    for statement in statements(text) {
        let s = statement.trim();
        let Some(rest) = s.strip_prefix('#') else {
            continue;
        };
        let Some((id, body)) = rest.split_once('=') else {
            unread += 1;
            continue;
        };
        let Ok(id) = id.trim().parse::<u64>() else {
            unread += 1;
            continue;
        };
        let body = body.trim();
        // Only instances that hold something read here are parsed.
        let wanted = READ.iter().any(|name| body.contains(name))
            || body.contains("LENGTH_UNIT")
            || body.contains("REPRESENTATION")
            || body.contains("B_SPLINE_CURVE");
        if !wanted {
            continue;
        }
        let mut p = Parser {
            s: body.as_bytes(),
            i: 0,
        };
        match p.instance() {
            Some(inst) => {
                out.insert(id, inst);
            }
            None => unread += 1,
        }
    }
    (out, unread)
}

/// The text's statements: split at `;` outside strings, with comments removed.
fn statements(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            cur.push(c);
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    cur.push('\'');
                    chars.next();
                } else {
                    in_str = false;
                }
            }
            continue;
        }
        match c {
            '\'' => {
                in_str = true;
                cur.push(c);
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut last = '\0';
                for c in chars.by_ref() {
                    if last == '*' && c == '/' {
                        break;
                    }
                    last = c;
                }
                cur.push(' ');
            }
            ';' => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self.s.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        self.skip_ws();
        (self.s.get(self.i) == Some(&c)).then(|| self.i += 1)
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.s.get(self.i).copied()
    }

    fn keyword(&mut self) -> Option<String> {
        self.skip_ws();
        let start = self.i;
        while self
            .s
            .get(self.i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'!')
        {
            self.i += 1;
        }
        (self.i > start).then(|| String::from_utf8_lossy(&self.s[start..self.i]).to_ascii_uppercase())
    }

    /// `NAME(params)` or `(NAME(params) NAME(params) ...)`, and nothing after it.
    fn instance(&mut self) -> Option<Instance> {
        let inst = if self.peek()? == b'(' {
            self.i += 1;
            let mut records = Vec::new();
            while self.peek()? != b')' {
                let name = self.keyword()?;
                records.push((name, self.params(0)?));
            }
            self.i += 1;
            records
        } else {
            let name = self.keyword()?;
            vec![(name, self.params(0)?)]
        };
        self.skip_ws();
        (self.i == self.s.len()).then_some(inst)
    }

    /// `(a, b, ...)`.
    fn params(&mut self, depth: usize) -> Option<Vec<Value>> {
        if depth > MAX_NESTING {
            return None;
        }
        self.eat(b'(')?;
        let mut out = Vec::new();
        if self.peek()? == b')' {
            self.i += 1;
            return Some(out);
        }
        loop {
            out.push(self.value(depth)?);
            match self.peek()? {
                b',' => self.i += 1,
                b')' => {
                    self.i += 1;
                    return Some(out);
                }
                _ => return None,
            }
        }
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        match self.peek()? {
            b'#' => {
                self.i += 1;
                let start = self.i;
                while self.s.get(self.i).is_some_and(u8::is_ascii_digit) {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[start..self.i])
                    .ok()?
                    .parse()
                    .ok()
                    .map(Value::Ref)
            }
            b'$' | b'*' => {
                self.i += 1;
                Some(Value::Unset)
            }
            b'(' => self.params(depth + 1).map(Value::List),
            b'\'' => {
                self.i += 1;
                let mut bytes = Vec::new();
                loop {
                    match *self.s.get(self.i)? {
                        b'\'' if self.s.get(self.i + 1) == Some(&b'\'') => {
                            bytes.push(b'\'');
                            self.i += 2;
                        }
                        b'\'' => {
                            self.i += 1;
                            break;
                        }
                        c => {
                            bytes.push(c);
                            self.i += 1;
                        }
                    }
                }
                Some(Value::Str(printable(&String::from_utf8_lossy(&bytes))))
            }
            b'.' if self.s.get(self.i + 1).is_some_and(u8::is_ascii_alphabetic) => {
                self.i += 1;
                let name = self.keyword()?;
                self.eat(b'.')?;
                Some(Value::Enum(name))
            }
            b'"' => {
                // Binary: kept only as present.
                self.i += 1;
                while *self.s.get(self.i)? != b'"' {
                    self.i += 1;
                }
                self.i += 1;
                Some(Value::Unset)
            }
            c if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' => {
                let start = self.i;
                while self
                    .s
                    .get(self.i)
                    .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.' | b'E' | b'e'))
                {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[start..self.i])
                    .ok()?
                    .parse()
                    .ok()
                    .map(Value::Num)
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let name = self.keyword()?;
                let args = self.params(depth + 1)?;
                Some(Value::Typed(name, args))
            }
            _ => None,
        }
    }
}

mod assembly;

#[cfg(test)]
mod tests;

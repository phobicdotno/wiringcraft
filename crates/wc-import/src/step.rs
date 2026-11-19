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

use std::collections::{BTreeMap, BTreeSet};

use wc_core::ModelSize;

use crate::{Error, Result, printable};

/// Deepest nesting of brackets read in one entity.
const MAX_NESTING: usize = 32;

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
    let (mm, unit_warning) = length_unit(&instances);
    warnings.extend(unit_warning);

    let point = |id: u64| -> Option<[f64; 3]> {
        let (name, args) = simple(&instances, id)?;
        (name == "CARTESIAN_POINT").then_some(())?;
        coords(args.get(1)?)
    };

    let mut pts: Vec<[f64; 3]> = Vec::new();
    let mut vertices = 0usize;
    for inst in instances.values() {
        for (name, args) in inst {
            if name == "VERTEX_POINT"
                && let Some(p) = args.get(1).and_then(Value::as_ref_id).and_then(point)
            {
                pts.push(p);
                vertices += 1;
            }
        }
    }
    // Curves: whole circles and ellipses, and the control points of B-spline curves. They bound
    // a B-rep's round edges, and are the geometry of a wireframe model without vertices.
    let mut curves: Vec<[f64; 3]> = Vec::new();
    for inst in instances.values() {
        for (name, args) in inst {
            match name.as_str() {
                "CIRCLE" | "ELLIPSE" => {
                    if let Some(extent) = conic_extent(&instances, name, args, &point) {
                        curves.extend(extent);
                    }
                }
                // Simple instances name the curve first: (name, degree, points, ...); in a
                // complex instance B_SPLINE_CURVE(degree, points, ...) has no name.
                "B_SPLINE_CURVE_WITH_KNOTS" | "BEZIER_CURVE" | "UNIFORM_CURVE" | "QUASI_UNIFORM_CURVE"
                    if inst.len() == 1 =>
                {
                    curves.extend(curve_points(args.get(2), &point));
                }
                "B_SPLINE_CURVE" => curves.extend(curve_points(args.get(1), &point)),
                // A wireframe's straight edges: its corner points.
                "POLYLINE" => curves.extend(curve_points(args.get(1), &point)),
                _ => {}
            }
        }
    }
    if vertices == 0 {
        // Tessellated models (AP242) list their coordinates.
        for inst in instances.values() {
            for (name, args) in inst {
                if name == "COORDINATES_LIST"
                    && let Some(list) = args.get(2).and_then(Value::as_list)
                {
                    pts.extend(list.iter().filter_map(coords));
                }
            }
        }
    }
    pts.extend(curves);
    if pts.is_empty() {
        // Else every loose point that is not the origin of a placement.
        let origins: BTreeSet<u64> = instances
            .values()
            .flatten()
            .filter(|(name, _)| name == "AXIS2_PLACEMENT_3D")
            .filter_map(|(_, args)| args.get(1).and_then(Value::as_ref_id))
            .collect();
        pts.extend(
            instances
                .iter()
                .filter(|(id, _)| !origins.contains(id))
                .filter_map(|(id, _)| point(*id)),
        );
    }
    if pts.is_empty() {
        return Err(Error::Format(
            "the model has no 3D geometry WiringCraft can measure".into(),
        ));
    }

    let placed = instances
        .values()
        .flatten()
        .filter(|(name, _)| name == "MAPPED_ITEM" || name == "ITEM_DEFINED_TRANSFORMATION")
        .count();
    if placed > 0 {
        warnings.push(format!(
            "the model is an assembly ({placed} placed parts); their placements are not applied, so its size may be off"
        ));
    }

    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for p in &pts {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
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

/// The points of a curve's control list.
fn curve_points(list: Option<&Value>, point: &impl Fn(u64) -> Option<[f64; 3]>) -> Vec<[f64; 3]> {
    list.and_then(Value::as_list)
        .map(|items| items.iter().filter_map(Value::as_ref_id).filter_map(point).collect())
        .unwrap_or_default()
}

/// The two corners of the box around a whole circle or ellipse. Its first semi-axis runs along
/// the placement's reference direction `u` and its second along `v` = axis x `u`; on each model
/// axis k the curve spans its centre plus and minus hypot(a u_k, b v_k).
fn conic_extent(
    instances: &BTreeMap<u64, Instance>,
    name: &str,
    args: &[Value],
    point: &impl Fn(u64) -> Option<[f64; 3]>,
) -> Option<[[f64; 3]; 2]> {
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
    let along = |r: [f64; 3]| {
        let r = unit(r)?;
        // (n x r) x n is r's part square to the axis, of length sin(angle), with no
        // cancellation of the large along-axis part; any nearly parallel direction keeps its way.
        let side = cross(n, r);
        // Along the axis itself: only rounding noise is left, so no direction in the plane.
        (dot(side, side).sqrt() > 64.0 * f64::EPSILON).then_some(())?;
        unit(cross(side, n))
    };
    let u = direction(placement.get(3))?
        .and_then(along)
        .or_else(|| along([1.0, 0.0, 0.0]))
        .or_else(|| along([0.0, 1.0, 0.0]))?;
    let v = cross(n, u);
    let mut lo = centre;
    let mut hi = centre;
    for k in 0..3 {
        let e = (a * u[k]).hypot(b * v[k]);
        lo[k] -= e;
        hi[k] += e;
    }
    Some([lo, hi])
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
fn length_unit(instances: &BTreeMap<u64, Instance>) -> (f64, Option<String>) {
    // The contexts the shape representations (with items) are drawn in; when none is
    // named, every context that assigns units.
    let used_contexts: Vec<u64> = instances
        .values()
        .flatten()
        .filter(|(name, args)| {
            name.ends_with("REPRESENTATION")
                && args
                    .get(1)
                    .and_then(Value::as_list)
                    .is_some_and(|items| !items.is_empty())
        })
        .filter_map(|(_, args)| args.get(2).and_then(Value::as_ref_id))
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

#[cfg(test)]
mod tests;

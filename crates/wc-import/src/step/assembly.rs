//! Placements of an assembly's parts (#500). A STEP assembly draws each part in its own
//! coordinates and places it in its parent with a transformation: a
//! `REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION` naming an `ITEM_DEFINED_TRANSFORMATION`
//! between two axis placements, or a `MAPPED_ITEM` that maps a `REPRESENTATION_MAP` onto a
//! target placement or transformation operator. Here every measured point is moved into the coordinates of the top of the
//! assembly, once for every place its part is used, nested assemblies included.

use std::collections::{BTreeMap, BTreeSet};

use super::{Geom, Instance, NOISE, Value, coords, cross, dot, refs_in, simple, square_to, statements, unit};

const WITH: &str = "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION";

/// Deepest chain of placements followed from the top of the assembly.
const MAX_DEPTH: usize = 16;
/// Most part uses measured; a crafted file can reuse one part a great many times.
const MAX_USES: usize = 10_000;
/// Most points moved in all; past it a part use counts by the corners of its own box.
const MAX_POINTS: usize = 2_000_000;

/// A placement: `p' = r p + t`, the columns of `r` being the placed x, y and z axes. It is
/// rigid, except where a mapped item's transformation operator scales it; only rigid ones are
/// inverted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Placement {
    r: [[f64; 3]; 3],
    t: [f64; 3],
}

impl Placement {
    const IDENTITY: Placement = Placement {
        r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        t: [0.0; 3],
    };

    fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let v = self.turn(p);
        [v[0] + self.t[0], v[1] + self.t[1], v[2] + self.t[2]]
    }

    /// A vector turned, not moved.
    fn turn(&self, p: [f64; 3]) -> [f64; 3] {
        let mut out = [0.0; 3];
        for (row, o) in out.iter_mut().enumerate() {
            *o = self.r[row][0] * p[0] + self.r[row][1] * p[1] + self.r[row][2] * p[2];
        }
        out
    }

    /// A piece of geometry placed: a conic keeps its shape, turned with its part.
    fn place(&self, g: &Geom) -> Geom {
        match *g {
            Geom::Point(p) => Geom::Point(self.apply(p)),
            Geom::Conic { centre, u, v } => Geom::Conic {
                centre: self.apply(centre),
                u: self.turn(u),
                v: self.turn(v),
            },
        }
    }

    /// `outer` after `self`.
    fn then(&self, outer: &Placement) -> Placement {
        let mut r = [[0.0; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (0..3).map(|k| outer.r[i][k] * self.r[k][j]).sum();
            }
        }
        Placement {
            r,
            t: outer.apply(self.t),
        }
    }

    /// The inverse; `r` is a rotation, so its transpose.
    fn inverse(&self) -> Placement {
        let mut r = [[0.0; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = self.r[j][i];
            }
        }
        let back = Placement { r, t: [0.0; 3] }.apply(self.t);
        Placement {
            r,
            t: [-back[0], -back[1], -back[2]],
        }
    }
}

/// A direction parameter: `Some(None)` when it is not given (`$`).
fn direction(instances: &BTreeMap<u64, Instance>, v: Option<&Value>) -> Option<Option<[f64; 3]>> {
    match v {
        Some(Value::Ref(id)) => {
            let (kind, d) = simple(instances, *id)?;
            (kind == "DIRECTION").then_some(())?;
            Some(Some(coords(d.get(1)?)?))
        }
        _ => Some(None),
    }
}

/// The placement an `AXIS2_PLACEMENT_3D` stands for: its origin, its axis as z and its
/// reference direction, made square to the axis, as x. A reference direction is taken however
/// close to the axis it lies, as long as it is not on it, as for circles.
pub(super) fn axis_placement(
    instances: &BTreeMap<u64, Instance>,
    id: u64,
    point: &impl Fn(u64) -> Option<[f64; 3]>,
) -> Option<Placement> {
    let (kind, args) = simple(instances, id)?;
    (kind == "AXIS2_PLACEMENT_3D").then_some(())?;
    let origin = point(args.get(1)?.as_ref_id()?)?;
    let z = unit(direction(instances, args.get(2))?.unwrap_or([0.0, 0.0, 1.0]))?;
    let x = x_axis(z, direction(instances, args.get(3))?)?;
    let y = cross(z, x);
    Some(Placement {
        r: [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]],
        t: origin,
    })
}

/// The x axis square to the unit axis `z`: the reference direction made square, or, when it is
/// not given or lies on the axis, the first of the world's x and y that does not.
fn x_axis(z: [f64; 3], reference: Option<[f64; 3]>) -> Option<[f64; 3]> {
    match reference.and_then(|r| square_to(z, r)) {
        Some((x, _)) => Some(x),
        None => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
            .into_iter()
            .filter_map(|r| square_to(z, r))
            .find(|(_, len)| *len > NOISE)
            .map(|(x, _)| x),
    }
}

/// The placement a `CARTESIAN_TRANSFORMATION_OPERATOR_3D` stands for, the other target a
/// mapped item can have: its local origin, axes as the standard derives them (axis3 as z,
/// axis1 made square to it as x, axis2 or the world's y made square to both as y) and its
/// scale. The operator's
/// last five attributes are read, as writers differ in the names before them.
pub(super) fn operator_placement(
    instances: &BTreeMap<u64, Instance>,
    id: u64,
    point: &impl Fn(u64) -> Option<[f64; 3]>,
) -> Option<Placement> {
    let (kind, args) = simple(instances, id)?;
    (kind == "CARTESIAN_TRANSFORMATION_OPERATOR_3D").then_some(())?;
    let [axis1, axis2, origin, scale, axis3] = args.get(args.len().checked_sub(5)?..)? else {
        return None;
    };
    let origin = point(origin.as_ref_id()?)?;
    let scale = match scale {
        Value::Num(s) if s.is_finite() && *s > 0.0 => *s,
        Value::Unset => 1.0,
        _ => return None,
    };
    let z = unit(direction(instances, Some(axis3))?.unwrap_or([0.0, 0.0, 1.0]))?;
    let x = x_axis(z, direction(instances, Some(axis1))?)?;
    // axis2, or the world's y when it is not given, made square to both: it keeps its side,
    // so the operator can mirror. Only when it lies along them is y taken to the right of x.
    let v = direction(instances, Some(axis2))?.unwrap_or([0.0, 1.0, 0.0]);
    let (vz, vx) = (dot(v, z), dot(v, x));
    let y = unit([0, 1, 2].map(|k| v[k] - vz * z[k] - vx * x[k])).unwrap_or_else(|| cross(z, x));
    let [x, y, z] = [x, y, z].map(|a| a.map(|c| c * scale));
    Some(Placement {
        r: [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]],
        t: origin,
    })
}

/// Union-find over representations drawn in the same coordinates.
struct Frames(BTreeMap<u64, u64>);

impl Frames {
    fn find(&mut self, mut id: u64) -> u64 {
        let mut path = Vec::new();
        while let Some(&up) = self.0.get(&id) {
            if up == id {
                break;
            }
            path.push(id);
            id = up;
        }
        for p in path {
            self.0.insert(p, id);
        }
        id
    }

    fn join(&mut self, a: u64, b: u64) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0.insert(b, a);
        }
    }
}

/// What [`place`] did.
#[derive(Debug, Default)]
pub(super) struct Placed {
    pub points: Vec<Geom>,
    /// Transformations and mapped items found.
    pub placements: usize,
    /// Of those, the ones that could not be applied.
    pub missed: usize,
    /// The assembly was cut short: too deep, too many part uses or too many points.
    pub cut: bool,
}

/// Move each point, given with the instance it was measured from, into the coordinates of the
/// top of the assembly. A file without placements keeps its points as they are.
pub(super) fn place(
    text: &str,
    instances: &BTreeMap<u64, Instance>,
    pts: Vec<(u64, Geom)>,
    point: &impl Fn(u64) -> Option<[f64; 3]>,
) -> Placed {
    let named = |id: u64, name: &str| instances.get(&id).is_some_and(|i| i.iter().any(|(n, _)| n == name));
    let placements = instances
        .values()
        .filter(|i| {
            i.iter()
                .any(|(n, _)| n == "MAPPED_ITEM" || n == "ITEM_DEFINED_TRANSFORMATION")
        })
        .count();
    if placements == 0 {
        return Placed {
            points: pts.into_iter().map(|(_, p)| p).collect(),
            ..Placed::default()
        };
    }

    // Representations and their items.
    let mut items: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for (id, inst) in instances {
        for (name, args) in inst {
            if name.ends_with("REPRESENTATION")
                && let Some(list) = args.get(1).and_then(Value::as_list)
            {
                items
                    .entry(*id)
                    .or_default()
                    .extend(list.iter().filter_map(Value::as_ref_id));
            }
        }
    }
    let mut frames = Frames(items.keys().map(|id| (*id, *id)).collect());
    // Relationships: (child, parent, transformation), the transformation absent when both are
    // drawn in the same coordinates.
    let mut links: Vec<(u64, u64, Option<u64>)> = Vec::new();
    for inst in instances.values() {
        // A complex instance names the two representations in its REPRESENTATION_RELATIONSHIP
        // record and the transformation alone in its ..._WITH_TRANSFORMATION record; a simple
        // ..._WITH_TRANSFORMATION instance has all five attributes in one.
        let Some((child, parent)) = inst.iter().find_map(|(name, args)| {
            matches!(
                name.as_str(),
                "REPRESENTATION_RELATIONSHIP" | "SHAPE_REPRESENTATION_RELATIONSHIP" | WITH
            )
            .then_some(())?;
            let a = args.get(2)?.as_ref_id()?;
            let b = args.get(3)?.as_ref_id()?;
            (items.contains_key(&a) && items.contains_key(&b)).then_some((a, b))
        }) else {
            continue;
        };
        let transformation = inst.iter().find_map(|(name, args)| {
            (name == WITH).then_some(())?;
            if args.len() >= 5 { args.get(4) } else { args.first() }?.as_ref_id()
        });
        if transformation.is_none() {
            frames.join(parent, child);
        }
        links.push((child, parent, transformation));
    }
    // Every frame each item is listed in: an item may belong to more than one representation.
    let mut item_frames: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
    for (rep, list) in &items {
        let frame = frames.find(*rep);
        for i in list {
            item_frames.entry(*i).or_default().insert(frame);
        }
    }
    // Item `i` is listed in frame `f` and not in `other`.
    let only_in = |i: u64, f: u64, other: u64| {
        item_frames
            .get(&i)
            .is_some_and(|fs| fs.contains(&f) && !fs.contains(&other))
    };

    // Placements of child frames in their parents.
    let mut children: BTreeMap<u64, Vec<(u64, Placement)>> = BTreeMap::new();
    // The transformations and mapped items applied, each counted once however often it is used.
    let mut applied: BTreeSet<u64> = BTreeSet::new();
    for (child, parent, transformation) in links {
        let Some(t) = transformation else { continue };
        let (child, parent) = (frames.find(child), frames.find(parent));
        let Some((kind, args)) = simple(instances, t) else {
            continue;
        };
        if kind != "ITEM_DEFINED_TRANSFORMATION" {
            continue;
        }
        let (Some(a), Some(b)) = (
            args.get(2).and_then(Value::as_ref_id),
            args.get(3).and_then(Value::as_ref_id),
        ) else {
            continue;
        };
        // The first item belongs to the child as the standard has it; some writers swap them,
        // which shows by which representation lists each placement. An item both list, such as
        // a shared origin, tells nothing.
        let swapped = only_in(a, parent, child) || only_in(b, child, parent);
        let (from, to) = if swapped { (b, a) } else { (a, b) };
        if let (Some(from), Some(to)) = (
            axis_placement(instances, from, point),
            axis_placement(instances, to, point),
        ) && child != parent
        {
            children
                .entry(parent)
                .or_default()
                .push((child, from.inverse().then(&to)));
            applied.insert(t);
        }
    }
    let mut mapped_items: BTreeSet<u64> = BTreeSet::new();
    for (rep, list) in &items {
        for &item in list {
            let Some(("MAPPED_ITEM", args)) = simple(instances, item) else {
                continue;
            };
            mapped_items.insert(item);
            let found = (|| {
                let (kind, map) = simple(instances, args.get(1)?.as_ref_id()?)?;
                (kind == "REPRESENTATION_MAP").then_some(())?;
                let from = axis_placement(instances, map.first()?.as_ref_id()?, point)?;
                let mapped = map.get(1)?.as_ref_id()?;
                items.contains_key(&mapped).then_some(())?;
                let target = args.get(2)?.as_ref_id()?;
                let to = axis_placement(instances, target, point)
                    .or_else(|| operator_placement(instances, target, point))?;
                Some((mapped, from.inverse().then(&to)))
            })();
            if let Some((mapped, placement)) = found {
                let (child, parent) = (frames.find(mapped), frames.find(*rep));
                if child != parent {
                    children.entry(parent).or_default().push((child, placement));
                    applied.insert(item);
                }
            }
        }
    }

    // Which frame each measured instance is drawn in: reached from a representation's items,
    // without going through placements or other representations.
    let mut graph: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for statement in statements(text) {
        let Some((id, body)) = statement.trim().strip_prefix('#').and_then(|r| r.split_once('=')) else {
            continue;
        };
        if let Ok(id) = id.trim().parse::<u64>() {
            graph.insert(id, refs_in(body));
        }
    }
    let stop = |id: u64| {
        mapped_items.contains(&id)
            || items.contains_key(&id)
            || named(id, "REPRESENTATION_MAP")
            || named(id, "ITEM_DEFINED_TRANSFORMATION")
    };
    // An item can be listed by more than one representation, so each frame keeps its own
    // geometry: once in a frame however many paths reach it, but in every frame that reaches it.
    let mut seen: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
    let reps: Vec<u64> = items.keys().copied().collect();
    for rep in reps {
        let frame = frames.find(rep);
        let seen = seen.entry(frame).or_default();
        let mut todo: Vec<u64> = items.get(&rep).cloned().unwrap_or_default();
        while let Some(id) = todo.pop() {
            if stop(id) || !seen.insert(id) {
                continue;
            }
            if let Some(next) = graph.get(&id) {
                todo.extend(next.iter().filter(|n| !seen.contains(n)));
            }
        }
    }
    let mut out = Placed {
        placements,
        missed: placements.saturating_sub(applied.len()),
        ..Placed::default()
    };
    // Each instance's geometry is kept once; a frame lists the instances it holds, and the
    // geometry is placed only as each use of the frame is walked, within the point budget.
    let owned: BTreeSet<u64> = seen.values().flatten().copied().collect();
    let mut geom_of: BTreeMap<u64, Vec<Geom>> = BTreeMap::new();
    for (id, p) in pts {
        if owned.contains(&id) {
            geom_of.entry(id).or_default().push(p);
        } else {
            // Geometry outside every representation stays where it is.
            out.points.push(p);
        }
    }
    // Per frame: its instances with geometry and how many pieces they hold.
    let own: BTreeMap<u64, (Vec<u64>, usize)> = seen
        .iter()
        .filter_map(|(frame, ids)| {
            let list: Vec<u64> = ids.iter().copied().filter(|id| geom_of.contains_key(id)).collect();
            let count = list.iter().filter_map(|id| geom_of.get(id)).map(Vec::len).sum();
            (count > 0).then_some((*frame, (list, count)))
        })
        .collect();
    let pieces = |list: &[u64]| -> Vec<&Geom> { list.iter().filter_map(|id| geom_of.get(id)).flatten().collect() };

    // Walk down from the frames no one places.
    let placed: BTreeSet<u64> = children.values().flatten().map(|(c, _)| *c).collect();
    let mut tops: Vec<u64> = own
        .keys()
        .chain(children.keys())
        .copied()
        .filter(|f| !placed.contains(f))
        .collect();
    tops.sort_unstable();
    tops.dedup();
    let mut reached: BTreeSet<u64> = BTreeSet::new();
    let mut uses = 0usize;
    // Each part's own box, worked out once when it is first needed.
    let mut boxes: BTreeMap<u64, [[f64; 3]; 2]> = BTreeMap::new();
    let mut todo: Vec<(u64, Placement, usize)> = tops.iter().map(|f| (*f, Placement::IDENTITY, 0)).collect();
    while let Some((frame, at, depth)) = todo.pop() {
        if depth > MAX_DEPTH || uses >= MAX_USES {
            out.cut = true;
            continue;
        }
        uses += 1;
        reached.insert(frame);
        if let Some((list, count)) = own.get(&frame) {
            if out.points.len() + count <= MAX_POINTS {
                out.points.extend(pieces(list).into_iter().map(|g| at.place(g)));
            } else {
                // The corners of the part's own box: the same size at quarter turns, a little
                // more at others.
                out.cut = true;
                let [lo, hi] = *boxes.entry(frame).or_insert_with(|| {
                    let mut lo = [f64::INFINITY; 3];
                    let mut hi = [f64::NEG_INFINITY; 3];
                    for g in pieces(list) {
                        let [a, b] = g.bounds();
                        for k in 0..3 {
                            lo[k] = lo[k].min(a[k]);
                            hi[k] = hi[k].max(b[k]);
                        }
                    }
                    [lo, hi]
                });
                for c in 0..8 {
                    let corner = [
                        if c & 1 == 0 { lo[0] } else { hi[0] },
                        if c & 2 == 0 { lo[1] } else { hi[1] },
                        if c & 4 == 0 { lo[2] } else { hi[2] },
                    ];
                    out.points.push(Geom::Point(at.apply(corner)));
                }
            }
        }
        for (child, placement) in children.get(&frame).into_iter().flatten() {
            todo.push((*child, placement.then(&at), depth + 1));
        }
    }
    // Parts only placed in a loop of placements: as drawn.
    // Each piece once, however many unreached frames share it: as drawn, they would only
    // repeat it.
    let unreached: BTreeSet<u64> = own
        .iter()
        .filter(|(frame, _)| !reached.contains(frame))
        .flat_map(|(_, (list, _))| list.iter().copied())
        .collect();
    if !unreached.is_empty() {
        out.cut = true;
        out.points
            .extend(unreached.iter().filter_map(|id| geom_of.get(id)).flatten());
    }
    out
}

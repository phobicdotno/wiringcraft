//! Placements of an assembly's parts (#500). A STEP assembly draws each part in its own
//! coordinates and places it in its parent with a transformation: a
//! `REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION` naming an `ITEM_DEFINED_TRANSFORMATION`
//! between two axis placements, or a `MAPPED_ITEM` that maps a `REPRESENTATION_MAP` onto a
//! target placement or transformation operator. Here every measured point is moved into the coordinates of the top of the
//! assembly, once for every place its part is used, nested assemblies included.

use std::collections::{BTreeMap, BTreeSet};

use super::{Geom, Instance, Value, coords, cross, dot, refs_in, simple, square_to, statements, unit};

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

    /// A scaling by `k` about the origin.
    fn scaling(k: f64) -> Placement {
        Placement {
            r: [[k, 0.0, 0.0], [0.0, k, 0.0], [0.0, 0.0, k]],
            t: [0.0; 3],
        }
    }

    /// The same placement with its translation in another length unit, `k` of the old per new:
    /// the turn is kept, the move scaled.
    fn moved_in(&self, k: f64) -> Placement {
        Placement {
            r: self.r,
            t: self.t.map(|c| c * k),
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

/// A direction parameter: `Some(None)` when it is not given (`$`), `None` when it is not a
/// direction, so the placement counts as not applied rather than taking the default.
fn direction(instances: &BTreeMap<u64, Instance>, v: Option<&Value>) -> Option<Option<[f64; 3]>> {
    match v {
        Some(Value::Ref(id)) => {
            let (kind, d) = simple(instances, *id)?;
            (kind == "DIRECTION").then_some(())?;
            Some(Some(coords(d.get(1)?)?))
        }
        None | Some(Value::Unset) => Some(None),
        Some(_) => None,
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
/// not given or lies on the axis, the world's x made square, or its y when x lies exactly on
/// the axis.
fn x_axis(z: [f64; 3], reference: Option<[f64; 3]>) -> Option<[f64; 3]> {
    match reference.and_then(|r| square_to(z, r)) {
        Some((x, _)) => Some(x),
        // The first that can be made square: its length may underflow to nothing for a tilt
        // of 1e-200, yet the direction is still there.
        None => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
            .into_iter()
            .find_map(|r| square_to(z, r))
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
    // axis2, or the world's y when it is not given, picks the side of y, so the operator can
    // mirror; when it lies along x or z, y is to the right of x.
    // Square to both by construction: the normal of x and z, on axis2's side.
    let v = direction(instances, Some(axis2))?.unwrap_or([0.0, 1.0, 0.0]);
    let right = cross(z, x);
    let y = if dot(v, right) < 0.0 { right.map(|c| -c) } else { right };
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
    /// Some geometry was converted by the file's length unit, as its own context's unit was not
    /// known (or there are no placements, and the whole file is in the file's unit).
    pub file_unit_used: bool,
    /// Of that, geometry or a placement drawn in a representation whose context names no
    /// single length unit of its own.
    pub context_unit_unknown: bool,
}

/// Move each point, given with the instance it was measured from, into the coordinates of the
/// top of the assembly, in millimetres. Each representation's geometry and the moves of the
/// placements drawn in it are converted by its context's length unit (`context_mm`, millimetres
/// per unit) before placements are composed, so an inch part placed in a millimetre assembly
/// keeps its size; geometry whose context's unit is not known takes `file_mm`. A file without
/// placements keeps its points where they are, in `file_mm`.
pub(super) fn place(
    text: &str,
    instances: &BTreeMap<u64, Instance>,
    pts: Vec<(u64, Geom)>,
    point: &impl Fn(u64) -> Option<[f64; 3]>,
    file_mm: f64,
    context_mm: &impl Fn(u64) -> Option<f64>,
) -> Placed {
    let in_file_unit = Placement::scaling(file_mm);
    let named = |id: u64, name: &str| instances.get(&id).is_some_and(|i| i.iter().any(|(n, _)| n == name));
    // A relationship with a transformation names it in its ..._WITH_TRANSFORMATION record:
    // alone in a complex instance, last of five attributes in a simple one.
    let transformation_of = |inst: &Instance| {
        inst.iter().find_map(|(name, args)| {
            (name == WITH).then_some(())?;
            if args.len() >= 5 { args.get(4) } else { args.first() }?.as_ref_id()
        })
    };
    // Placements found, a relationship whose transformation is not there among them.
    let placements = instances
        .values()
        .filter(|i| {
            i.iter()
                .any(|(n, _)| n == "MAPPED_ITEM" || n == "ITEM_DEFINED_TRANSFORMATION")
        })
        .count()
        + instances
            .values()
            .filter(|i| {
                i.iter().any(|(n, _)| n == WITH)
                    && transformation_of(i).is_none_or(|t| !named(t, "ITEM_DEFINED_TRANSFORMATION"))
            })
            .count();
    if placements == 0 {
        return Placed {
            points: pts.into_iter().map(|(_, p)| in_file_unit.place(&p)).collect(),
            file_unit_used: true,
            ..Placed::default()
        };
    }

    // Representations and their items.
    let mut items: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    // The context each representation's items are drawn in.
    let mut context: BTreeMap<u64, u64> = BTreeMap::new();
    for (id, inst) in instances {
        for (name, args) in inst {
            if name.ends_with("REPRESENTATION")
                && let Some(list) = args.get(1).and_then(Value::as_list)
            {
                items
                    .entry(*id)
                    .or_default()
                    .extend(list.iter().filter_map(Value::as_ref_id));
                if let Some(c) = args.get(2).and_then(Value::as_ref_id) {
                    context.insert(*id, c);
                }
            }
        }
    }
    // Millimetres per unit of a representation's context, when known.
    let rep_mm = |rep: u64| context.get(&rep).and_then(|c| context_mm(*c));
    // Whether a representation's unit was not known and the file's was used instead.
    let unknown = std::cell::Cell::new(false);
    let unit_of = |rep: u64| {
        rep_mm(rep).unwrap_or_else(|| {
            unknown.set(true);
            file_mm
        })
    };
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
        let transformation = transformation_of(inst);
        // Without a transformation, two representations share coordinates only in the same
        // context; in different ones they stay apart.
        if transformation.is_none() && context.get(&parent).is_some_and(|c| context.get(&child) == Some(c)) {
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
        // Each placement's move in its own representation's unit; a frame only joins
        // representations of one context, so the representation's unit is its frame's.
        // The file's unit standing in counts only once the placement is applied.
        let (child_mm, parent_mm) = (rep_mm(child), rep_mm(parent));
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
            if child_mm.is_none() || parent_mm.is_none() {
                unknown.set(true);
            }
            let (from, to) = (
                from.moved_in(child_mm.unwrap_or(file_mm)),
                to.moved_in(parent_mm.unwrap_or(file_mm)),
            );
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
                let mapped = map.get(1)?.as_ref_id()?;
                items.contains_key(&mapped).then_some(())?;
                let (child_mm, parent_mm) = (rep_mm(mapped), rep_mm(*rep));
                let from =
                    axis_placement(instances, map.first()?.as_ref_id()?, point)?.moved_in(child_mm.unwrap_or(file_mm));
                let target = args.get(2)?.as_ref_id()?;
                let to = axis_placement(instances, target, point)
                    .or_else(|| operator_placement(instances, target, point))?
                    .moved_in(parent_mm.unwrap_or(file_mm));
                let guessed = child_mm.is_none() || parent_mm.is_none();
                Some((mapped, from.inverse().then(&to), guessed))
            })();
            if let Some((mapped, placement, guessed)) = found {
                let (child, parent) = (frames.find(mapped), frames.find(*rep));
                if child != parent {
                    // The file's unit standing in counts only once the placement is applied.
                    unknown.set(unknown.get() || guessed);
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
    // Many frames sharing large geometry would hold many entries: past MAX_POINTS in all the
    // rest is left out and the assembly counts as cut short.
    let mut seen: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
    let mut entries = 0usize;
    let mut owning_cut = false;
    let reps: Vec<u64> = items.keys().copied().collect();
    'reps: for rep in reps {
        let frame = frames.find(rep);
        let seen = seen.entry(frame).or_default();
        let mut todo: Vec<u64> = items.get(&rep).cloned().unwrap_or_default();
        while let Some(id) = todo.pop() {
            if stop(id) || !seen.insert(id) {
                continue;
            }
            entries += 1;
            if entries > MAX_POINTS {
                owning_cut = true;
                break 'reps;
            }
            if let Some(next) = graph.get(&id) {
                todo.extend(next.iter().filter(|n| !seen.contains(n)));
            }
        }
    }
    let mut out = Placed {
        placements,
        missed: placements.saturating_sub(applied.len()),
        cut: owning_cut,
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
            // Geometry outside every representation stays where it is, in the file's unit.
            out.points.push(in_file_unit.place(&p));
            out.file_unit_used = true;
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
    // Each frame's geometry in millimetres; the file's unit when its own is not known.
    let frame_mm: BTreeMap<u64, f64> = own.keys().map(|frame| (*frame, unit_of(*frame))).collect();
    let to_mm = |frame: u64| Placement::scaling(frame_mm.get(&frame).copied().unwrap_or(file_mm));
    out.context_unit_unknown = unknown.get();
    out.file_unit_used |= out.context_unit_unknown;

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
            let at = to_mm(frame).then(&at);
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
        // A frame at the deepest level measures its own geometry; its children would be too
        // deep, so they are not composed at all.
        if depth == MAX_DEPTH {
            if children.get(&frame).is_some_and(|c| !c.is_empty()) {
                out.cut = true;
            }
        } else {
            for (child, placement) in children.get(&frame).into_iter().flatten() {
                todo.push((*child, placement.then(&at), depth + 1));
            }
        }
    }
    // Parts only placed in a loop of placements: as drawn.
    // Each piece once, however many unreached frames share it: as drawn, they would only
    // repeat it.
    // Each piece in the unit of the first unreached frame that holds it.
    let mut unreached: BTreeMap<u64, u64> = BTreeMap::new();
    for (frame, (list, _)) in own.iter().filter(|(frame, _)| !reached.contains(frame)) {
        for id in list {
            unreached.entry(*id).or_insert(*frame);
        }
    }
    if !unreached.is_empty() {
        out.cut = true;
        for (id, frame) in &unreached {
            let scale = to_mm(*frame);
            out.points
                .extend(geom_of.get(id).into_iter().flatten().map(|g| scale.place(g)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The instances of `data`, a list of `#id=...;` statements, and a point lookup over them.
    fn parsed(data: &str) -> BTreeMap<u64, Instance> {
        super::super::parse(data).0
    }

    fn point_in(instances: &BTreeMap<u64, Instance>) -> impl Fn(u64) -> Option<[f64; 3]> + '_ {
        |id| {
            let (name, args) = simple(instances, id)?;
            (name == "CARTESIAN_POINT").then_some(())?;
            coords(args.get(1)?)
        }
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-9)
    }

    /// A placement turned a quarter about z (x to y) and moved to (1, 2, 3).
    fn quarter_turn() -> Placement {
        Placement {
            r: [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            t: [1.0, 2.0, 3.0],
        }
    }

    #[test]
    fn a_placement_and_its_inverse_cancel_out() {
        let p = quarter_turn();
        let back = p.then(&p.inverse());
        for q in [[0.0; 3], [5.0, -7.0, 2.5], [1e3, 0.0, -1e-3]] {
            assert!(close(back.apply(q), q), "{:?}", back.apply(q));
            assert!(close(p.inverse().apply(p.apply(q)), q));
        }
        // `then` runs the inner placement first: turn then move, not move then turn.
        let moved = Placement {
            t: [10.0, 0.0, 0.0],
            ..Placement::IDENTITY
        };
        let turned = Placement {
            t: [0.0; 3],
            ..quarter_turn()
        };
        assert!(close(moved.then(&turned).apply([0.0; 3]), [0.0, 10.0, 0.0]));
        assert!(close(turned.then(&moved).apply([0.0; 3]), [10.0, 0.0, 0.0]));
        // A unit change scales the move and keeps the turn.
        let inch = quarter_turn().moved_in(25.4);
        assert!(close(inch.apply([0.0; 3]), [25.4, 50.8, 76.2]));
        assert!(close(inch.turn([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
    }

    #[test]
    fn a_placed_conic_turns_its_axes_but_does_not_move_them() {
        let g = Geom::Conic {
            centre: [1.0, 0.0, 0.0],
            u: [2.0, 0.0, 0.0],
            v: [0.0, 3.0, 0.0],
        };
        let Geom::Conic { centre, u, v } = quarter_turn().place(&g) else {
            unreachable!()
        };
        assert!(close(centre, [1.0, 3.0, 3.0]));
        assert!(close(u, [0.0, 2.0, 0.0]));
        assert!(close(v, [-3.0, 0.0, 0.0]));
    }

    #[test]
    fn axis_placements_with_bad_references_are_not_read() {
        let instances = parsed(
            "#1=CARTESIAN_POINT('',(1.,2.,3.));\n#2=DIRECTION('',(0.,0.,2.));\n#3=DIRECTION('',(0.,0.,0.));\n\
             #10=AXIS2_PLACEMENT_3D('',#1,#2,$);\n\
             #11=AXIS2_PLACEMENT_3D('',#99,#2,$);\n\
             #12=AXIS2_PLACEMENT_3D('',#1,#3,$);\n\
             #13=AXIS2_PLACEMENT_3D('',#1,#1,$);\n\
             #14=AXIS2_PLACEMENT_3D('',#1,#2,#2);\n\
             #15=AXIS2_PLACEMENT_3D('',#1,'z',$);\n",
        );
        let point = point_in(&instances);
        // A good one, its axis made unit and x the world's.
        let Some(p) = axis_placement(&instances, 10, &point) else {
            unreachable!("a usable placement")
        };
        assert!(close(p.apply([1.0, 0.0, 1.0]), [2.0, 2.0, 4.0]));
        // A reference direction along the axis falls back to the world's x.
        let Some(on_axis) = axis_placement(&instances, 14, &point) else {
            unreachable!("a usable placement")
        };
        assert_eq!(on_axis, p);
        // Its origin missing, its axis of no length, a point as its axis, a string as its axis,
        // and an instance that is not an axis placement at all.
        for id in [11, 12, 13, 15, 1, 404] {
            assert!(axis_placement(&instances, id, &point).is_none(), "#{id}");
        }
    }

    #[test]
    fn operator_placements_read_their_scale_and_side() {
        let instances = parsed(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));\n#2=DIRECTION('',(0.,-1.,0.));\n\
             #20=CARTESIAN_TRANSFORMATION_OPERATOR_3D('','',$,$,#1,$,$);\n\
             #21=CARTESIAN_TRANSFORMATION_OPERATOR_3D('','',$,$,#1,0.,$);\n\
             #22=CARTESIAN_TRANSFORMATION_OPERATOR_3D('','',$,$,#1,-2.,$);\n\
             #23=CARTESIAN_TRANSFORMATION_OPERATOR_3D('','',$,$,#1,'big',$);\n\
             #24=CARTESIAN_TRANSFORMATION_OPERATOR_3D($,#1,$,$);\n\
             #25=CARTESIAN_TRANSFORMATION_OPERATOR_3D('','',$,#2,#1,3.,$);\n",
        );
        let point = point_in(&instances);
        // No scale given is a scale of one.
        assert_eq!(operator_placement(&instances, 20, &point), Some(Placement::IDENTITY));
        // A scale of nothing, below nothing or not a number; too few attributes.
        for id in [21, 22, 23, 24] {
            assert!(operator_placement(&instances, id, &point).is_none(), "#{id}");
        }
        // axis2 against the world's y mirrors y, and the scale applies to every axis.
        let Some(m) = operator_placement(&instances, 25, &point) else {
            unreachable!("a usable operator")
        };
        assert!(close(m.apply([1.0, 1.0, 1.0]), [3.0, -3.0, 3.0]));
    }

    #[test]
    fn frames_join_into_one_root_and_shorten_their_paths() {
        let mut f = Frames((1..=5).map(|id| (id, id)).collect());
        f.join(1, 2);
        f.join(2, 3);
        f.join(4, 5);
        assert_eq!(f.find(3), f.find(1));
        assert_ne!(f.find(3), f.find(4));
        // Joining what is already joined changes nothing; an unknown id is its own frame.
        let root = f.find(1);
        f.join(3, 1);
        assert_eq!(f.find(2), root);
        assert_eq!(f.find(42), 42);
        // After a find every member of a long path points straight at its root.
        let mut long = Frames([(1, 1), (2, 1), (3, 2), (4, 3)].into_iter().collect());
        assert_eq!(long.find(4), 1);
        assert_eq!((long.0.get(&4), long.0.get(&3)), (Some(&1), Some(&1)));
    }

    /// A chain of `n` placements: representation #(1000 + k) placed in #(1000 + k + 1), each 1
    /// further along x, the first holding the point #1 at the origin.
    fn chain(n: u64) -> String {
        let mut out = String::from("#1=CARTESIAN_POINT('',(0.,0.,0.));\n#2=REPRESENTATION_CONTEXT('','');\n");
        for k in 0..=n {
            let items = if k == 0 {
                format!("#{},#1", 3000 + k)
            } else {
                format!("#{}", 3000 + k)
            };
            out.push_str(&format!(
                "#{}=SHAPE_REPRESENTATION('',({items}),#2);\n#{}=AXIS2_PLACEMENT_3D('',#{},$,$);\n#{}=CARTESIAN_POINT('',({}.,0.,0.));\n",
                1000 + k,
                3000 + k,
                4000 + k,
                4000 + k,
                k
            ));
        }
        for k in 0..n {
            out.push_str(&format!(
                "#{}=(REPRESENTATION_RELATIONSHIP('','',#{},#{}) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{}) SHAPE_REPRESENTATION_RELATIONSHIP());\n#{}=ITEM_DEFINED_TRANSFORMATION('','',#{},#{});\n",
                5000 + k,
                1000 + k,
                1001 + k,
                6000 + k,
                6000 + k,
                3000 + k,
                3001 + k
            ));
        }
        out
    }

    fn place_text(text: &str) -> Placed {
        let instances = parsed(text);
        let point = point_in(&instances);
        let pts: Vec<(u64, Geom)> = point(1).map(|p| (1, Geom::Point(p))).into_iter().collect();
        assert_eq!(pts.len(), 1);
        place(text, &instances, pts, &point, 1.0, &|_| Some(1.0))
    }

    #[test]
    fn a_chain_as_deep_as_allowed_adds_up_and_one_deeper_is_cut() {
        let n = MAX_DEPTH as u64;
        let deepest = place_text(&chain(n));
        assert_eq!((deepest.placements, deepest.missed, deepest.cut), (MAX_DEPTH, 0, false));
        // Each placement moves the part 1 along x (from origin k to origin k + 1).
        assert_eq!(deepest.points, vec![Geom::Point([n as f64, 0.0, 0.0])]);

        let too_deep = place_text(&chain(n + 1));
        assert!(too_deep.cut);
        assert_eq!(too_deep.missed, 0);
        // The part is never reached from the top, so it is measured as drawn.
        assert_eq!(too_deep.points, vec![Geom::Point([0.0; 3])]);
    }

    #[test]
    fn placements_naming_what_is_not_there_are_missed() {
        // The part placed through a transformation between placements that are not in the
        // file, by a relationship whose transformation is not there, and by a mapped item whose
        // map is not there: none applies, and the part stays as drawn.
        let text = format!(
            "{}#7000=SHAPE_REPRESENTATION('',(#7001),#2);\n#7001=MAPPED_ITEM('',#8888,#3001);\n\
             #7100=(REPRESENTATION_RELATIONSHIP('','',#1000,#7000) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#7777) SHAPE_REPRESENTATION_RELATIONSHIP());\n",
            chain(1).replace(
                "ITEM_DEFINED_TRANSFORMATION('','',#3000,#3001)",
                "ITEM_DEFINED_TRANSFORMATION('','',#9998,#9999)"
            )
        );
        let placed = place_text(&text);
        assert_eq!((placed.placements, placed.missed), (3, 3));
        assert!(!placed.cut);
        assert_eq!(placed.points, vec![Geom::Point([0.0; 3])]);
    }

    #[test]
    fn a_part_mapped_into_itself_is_not_a_placement() {
        // #1000 lists a mapped item of itself: a loop of one, which is not applied.
        let text = format!(
            "{}#1000=SHAPE_REPRESENTATION('',(#3000,#1,#7001),#2);\n#7001=MAPPED_ITEM('',#7002,#3000);\n#7002=REPRESENTATION_MAP(#3000,#1000);\n",
            chain(0).replace("#1000=SHAPE_REPRESENTATION('',(#3000,#1),#2);\n", "")
        );
        let placed = place_text(&text);
        assert_eq!((placed.placements, placed.missed, placed.cut), (1, 1, false));
        assert_eq!(placed.points, vec![Geom::Point([0.0; 3])]);
    }

    #[test]
    fn without_placements_points_only_take_the_file_unit() {
        let text = "#1=CARTESIAN_POINT('',(1.,2.,3.));\n#2=REPRESENTATION_CONTEXT('','');\n#1000=SHAPE_REPRESENTATION('',(#1),#2);\n";
        let instances = parsed(text);
        let point = point_in(&instances);
        let placed = place(
            text,
            &instances,
            vec![(1, Geom::Point([1.0, 2.0, 3.0]))],
            &point,
            25.4,
            &|_| Some(1.0),
        );
        assert_eq!((placed.placements, placed.missed, placed.cut), (0, 0, false));
        assert!(placed.file_unit_used);
        assert!(!placed.context_unit_unknown);
        let [Geom::Point(p)] = placed.points[..] else {
            unreachable!("one point: {:?}", placed.points)
        };
        assert!(close(p, [25.4, 50.8, 76.2]), "{p:?}");
    }
}

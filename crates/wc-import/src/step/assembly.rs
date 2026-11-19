//! Placements of an assembly's parts (#500). A STEP assembly draws each part in its own
//! coordinates and places it in its parent with a transformation: a
//! `REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION` naming an `ITEM_DEFINED_TRANSFORMATION`
//! between two axis placements, or a `MAPPED_ITEM` that maps a `REPRESENTATION_MAP` onto a
//! target placement. Here every measured point is moved into the coordinates of the top of the
//! assembly, once for every place its part is used, nested assemblies included.

use std::collections::{BTreeMap, BTreeSet};

use super::{Instance, Value, coords, cross, dot, refs_in, simple, statements, unit};

/// Deepest chain of placements followed from the top of the assembly.
const MAX_DEPTH: usize = 16;
/// Most part uses measured; a crafted file can reuse one part a great many times.
const MAX_USES: usize = 10_000;
/// Most points moved in all; past it a part use counts by the corners of its own box.
const MAX_POINTS: usize = 2_000_000;

/// A rigid placement: `p' = r p + t`, the columns of `r` being the placed x, y and z axes.
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
        let mut out = self.t;
        for (row, o) in out.iter_mut().enumerate() {
            *o += self.r[row][0] * p[0] + self.r[row][1] * p[1] + self.r[row][2] * p[2];
        }
        out
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
/// reference direction, made square to the axis, as x.
pub(super) fn axis_placement(
    instances: &BTreeMap<u64, Instance>,
    id: u64,
    point: &impl Fn(u64) -> Option<[f64; 3]>,
) -> Option<Placement> {
    let (kind, args) = simple(instances, id)?;
    (kind == "AXIS2_PLACEMENT_3D").then_some(())?;
    let origin = point(args.get(1)?.as_ref_id()?)?;
    let z = unit(direction(instances, args.get(2))?.unwrap_or([0.0, 0.0, 1.0]))?;
    let square = |r: [f64; 3]| {
        let r = unit(r)?;
        let d = dot(r, z);
        let x = unit([r[0] - d * z[0], r[1] - d * z[1], r[2] - d * z[2]])?;
        // Too close to the axis to give a way round it.
        (d.abs() < 1.0 - 1e-9).then_some(x)
    };
    let x = direction(instances, args.get(3))?
        .and_then(square)
        .or_else(|| square([1.0, 0.0, 0.0]))
        .or_else(|| square([0.0, 1.0, 0.0]))?;
    let y = cross(z, x);
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
    pub points: Vec<[f64; 3]>,
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
    pts: Vec<(u64, [f64; 3])>,
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
        let Some((child, parent)) = inst.iter().find_map(|(name, args)| {
            (name == "REPRESENTATION_RELATIONSHIP" || name == "SHAPE_REPRESENTATION_RELATIONSHIP").then_some(())?;
            let a = args.get(2)?.as_ref_id()?;
            let b = args.get(3)?.as_ref_id()?;
            (items.contains_key(&a) && items.contains_key(&b)).then_some((a, b))
        }) else {
            continue;
        };
        let transformation = inst.iter().find_map(|(name, args)| {
            (name == "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION").then_some(())?;
            args.first()?.as_ref_id()
        });
        if transformation.is_none() {
            frames.join(parent, child);
        }
        links.push((child, parent, transformation));
    }
    let item_frame: BTreeMap<u64, u64> = items
        .iter()
        .flat_map(|(rep, list)| list.iter().map(move |i| (*i, *rep)))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|(i, rep)| (i, frames.find(rep)))
        .collect();

    // Placements of child frames in their parents.
    let mut children: BTreeMap<u64, Vec<(u64, Placement)>> = BTreeMap::new();
    let mut applied = 0usize;
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
        // which shows by which representation lists each placement.
        let swapped = item_frame.get(&a) == Some(&parent) || item_frame.get(&b) == Some(&child);
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
            applied += 1;
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
                let to = axis_placement(instances, args.get(2)?.as_ref_id()?, point)?;
                Some((mapped, from.inverse().then(&to)))
            })();
            if let Some((mapped, placement)) = found {
                let (child, parent) = (frames.find(mapped), frames.find(*rep));
                if child != parent {
                    children.entry(parent).or_default().push((child, placement));
                    applied += 1;
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
    let mut owner: BTreeMap<u64, u64> = BTreeMap::new();
    let reps: Vec<u64> = items.keys().copied().collect();
    for rep in reps {
        let frame = frames.find(rep);
        let mut todo: Vec<u64> = items.get(&rep).cloned().unwrap_or_default();
        while let Some(id) = todo.pop() {
            if stop(id) || owner.contains_key(&id) {
                continue;
            }
            owner.insert(id, frame);
            if let Some(next) = graph.get(&id) {
                todo.extend(next.iter().filter(|n| !owner.contains_key(n)));
            }
        }
    }
    let mut own: BTreeMap<u64, Vec<[f64; 3]>> = BTreeMap::new();
    let mut out = Placed {
        placements,
        missed: placements.saturating_sub(applied),
        ..Placed::default()
    };
    for (id, p) in pts {
        match owner.get(&id) {
            Some(frame) => own.entry(*frame).or_default().push(p),
            // Geometry outside every representation stays where it is.
            None => out.points.push(p),
        }
    }

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
    let mut todo: Vec<(u64, Placement, usize)> = tops.iter().map(|f| (*f, Placement::IDENTITY, 0)).collect();
    while let Some((frame, at, depth)) = todo.pop() {
        if depth > MAX_DEPTH || uses >= MAX_USES {
            out.cut = true;
            continue;
        }
        uses += 1;
        reached.insert(frame);
        if let Some(list) = own.get(&frame) {
            if out.points.len() + list.len() <= MAX_POINTS {
                out.points.extend(list.iter().map(|p| at.apply(*p)));
            } else {
                // The corners of the part's own box: the same size at quarter turns, a little
                // more at others.
                out.cut = true;
                let mut lo = [f64::INFINITY; 3];
                let mut hi = [f64::NEG_INFINITY; 3];
                for p in list {
                    for k in 0..3 {
                        lo[k] = lo[k].min(p[k]);
                        hi[k] = hi[k].max(p[k]);
                    }
                }
                for c in 0..8 {
                    let corner = [
                        if c & 1 == 0 { lo[0] } else { hi[0] },
                        if c & 2 == 0 { lo[1] } else { hi[1] },
                        if c & 4 == 0 { lo[2] } else { hi[2] },
                    ];
                    out.points.push(at.apply(corner));
                }
            }
        }
        for (child, placement) in children.get(&frame).into_iter().flatten() {
            todo.push((*child, placement.then(&at), depth + 1));
        }
    }
    // Parts only placed in a loop of placements: as drawn.
    for (frame, list) in &own {
        if !reached.contains(frame) {
            out.cut = true;
            out.points.extend(list);
        }
    }
    out
}

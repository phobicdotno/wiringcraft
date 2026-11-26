//! Changing a black box ([`crate::black_box`]) that is already placed: its new size and
//! connection points, and what that does to the wires on it. Shared by the app (which adds an
//! undo step) and the command line (which writes the files), so both follow one set of rules.

use std::collections::{BTreeMap, BTreeSet};

use crate::{BoxPin, BoxSide, Endpoint, Point, Project, Symbol};

/// The connection points `pins` of a box resized to `width` by `height`: each stays on its
/// edge, its offset along the edge clamped to the new edge length (#524). The size must be
/// finite and above zero (as [`crate::black_box`] asks): callers check it first, and a negative
/// or NaN size panics in the clamp.
pub fn resized_pins(pins: &[BoxPin], width: f64, height: f64) -> Vec<BoxPin> {
    pins.iter()
        .map(|p| BoxPin {
            name: p.name.clone(),
            side: p.side,
            offset: match p.side {
                BoxSide::Top | BoxSide::Bottom => p.offset.clamp(0.0, width),
                BoxSide::Left | BoxSide::Right => p.offset.clamp(0.0, height),
            },
        })
        .collect()
}

/// The points of a box resized to `width` by `height` when no new list of points is given:
/// the old ones, each kept on its edge ([`resized_pins`], with the same size precondition) and
/// renamed as `renamed` asks.
pub fn kept_pins(pins: &[BoxPin], renamed: &BTreeMap<String, String>, width: f64, height: f64) -> Vec<BoxPin> {
    let mut kept = resized_pins(pins, width, height);
    for p in &mut kept {
        if let Some(to) = renamed.get(&p.name) {
            p.name.clone_from(to);
        }
    }
    kept
}

/// One wire end that a box edit changes.
#[derive(Debug, Clone, PartialEq)]
pub struct EndChange {
    /// The index of the wire's page in [`Project::pages`].
    pub page: usize,
    /// The index of the wire in its page's wires.
    pub wire: usize,
    /// `false` for the wire's `from` end, `true` for its `to` end.
    pub to_end: bool,
    /// The end it becomes: the point under its new name, or a free end where a removed
    /// point was.
    pub end: Endpoint,
}

/// What a box edit changes, worked out by [`plan`] without changing anything.
#[derive(Debug, Clone, PartialEq)]
pub struct BoxEditPlan {
    /// The box's new symbol (same id, name, description, tag place, role and letter code).
    pub symbol: Symbol,
    /// The wire ends that follow a renamed point or keep a free end where a point was removed.
    pub ends: Vec<EndChange>,
    /// What to check: wires on points that are gone or moved, and other boxes placing the same
    /// symbol (they change too).
    pub warnings: Vec<String>,
}

impl BoxEditPlan {
    /// Put the wire end changes into `project` (the symbol file and lock entry are the
    /// caller's to write). Wires are found by page and wire index, so this must be the project
    /// the plan was made from, with no page or wire added, removed or reordered in between.
    pub fn apply_ends(&self, project: &mut Project) {
        for c in &self.ends {
            if let Some(w) = project.pages.get_mut(c.page).and_then(|p| p.wires.get_mut(c.wire)) {
                *(if c.to_end { &mut w.to } else { &mut w.from }) = c.end.clone();
            }
        }
    }
}

/// How a placed black box should change: the box `item` on page index `page`, which places
/// symbol `old`.
#[derive(Debug, Clone, Copy)]
pub struct BoxEdit<'a> {
    pub page: usize,
    pub item: &'a str,
    /// New width and height in mm.
    pub size: (f64, f64),
    /// Every connection point of the box after the edit.
    pub pins: &'a [BoxPin],
    /// Old point name to new one; the wires on it follow.
    pub renamed: &'a BTreeMap<String, String>,
    /// Old points taken off (a new point may reuse such a name, and is still another point).
    pub removed: &'a BTreeSet<String>,
    /// How far `item` itself moves (page mm), as when a box is sized from a near corner and
    /// its opposite corner stays put (#529). Other boxes placing the symbol stay put.
    pub moved: Point,
}

/// Work out the edit `edit` of a black box whose symbol is `old`, in `project`: its new
/// symbol, the wire ends that change and the warnings. A rename of a point the box does not
/// have, one to itself, or one of a removed point is ignored. Returns `Ok(None)` when nothing
/// would change. Refuses a symbol that is not a black box, a new box [`crate::black_box`]
/// refuses, a rename to a point the new box does not have, and a removed point whose wire's
/// free end would join another wire's free end there that it was not connected to.
pub fn plan(project: &Project, old: &Symbol, edit: &BoxEdit<'_>) -> crate::Result<Option<BoxEditPlan>> {
    let bad = |why: String| Err(crate::Error::Invalid(why));
    if crate::black_box_layout(old).is_none() {
        return bad(format!("{} is not a black box", edit.item));
    }
    let placed = project
        .pages
        .get(edit.page)
        .and_then(|p| p.symbol(edit.item))
        .filter(|s| s.symbol == old.id);
    let Some(placed) = placed else {
        return bad(format!("no box {} placing symbol {} on this page", edit.item, old.id));
    };
    let (rotation, mirror) = (placed.rotation, placed.mirror);
    let moves = edit.moved.x != 0.0 || edit.moved.y != 0.0;
    let (width, height) = edit.size;
    let mut new = crate::black_box(&old.id, &old.name, width, height, edit.pins)?;
    new.tag_at = old.tag_at;
    new.description.clone_from(&old.description);
    new.source.clone_from(&old.source);
    new.role = old.role;
    new.letter_code.clone_from(&old.letter_code);
    let renamed: BTreeMap<&str, &str> = edit
        .renamed
        .iter()
        .filter(|(from, to)| from != to && old.pin(from).is_some() && !edit.removed.contains(*from))
        .map(|(from, to)| (from.as_str(), to.as_str()))
        .collect();
    if let Some((_, to)) = renamed.iter().find(|(_, to)| new.pin(to).is_none()) {
        return bad(format!("the box has no connection point {to} to rename a point to"));
    }
    // Two points renamed to one name would join their wires into one connection.
    let mut targets = BTreeSet::new();
    if let Some(to) = renamed.values().find(|to| !targets.insert(**to)) {
        return bad(format!("two connection points are renamed to {to}"));
    }
    let removed: Vec<&str> = edit
        .removed
        .iter()
        .filter(|n| old.pin(n).is_some())
        .map(String::as_str)
        .collect();
    if new == *old && renamed.is_empty() && removed.is_empty() && !moves {
        return Ok(None);
    }
    // What an old point is now: its new name, or `None` when it is gone. A name another point
    // was renamed to, or a new point took, is that point's, not this one's.
    let taken: Vec<&str> = renamed.values().copied().collect();
    let now_of = |pin: &str| -> Option<String> {
        if removed.contains(&pin) {
            return None;
        }
        if let Some(to) = renamed.get(pin) {
            return Some((*to).to_owned());
        }
        (!taken.contains(&pin) && new.pin(pin).is_some()).then(|| pin.to_owned())
    };
    // Every placed copy of the symbol changes: name the others, and the wires on points that
    // are gone or moved.
    let mut warnings = Vec::new();
    let mut ends = Vec::new();
    let mut others = 0usize;
    for (pi, p) in project.pages.iter().enumerate() {
        let users: Vec<_> = p.symbols.iter().filter(|s| s.symbol == old.id).collect();
        others += users.iter().filter(|u| !(pi == edit.page && u.id == edit.item)).count();
        for (wi, w) in p.wires.iter().enumerate() {
            for (to_end, end) in [(false, &w.from), (true, &w.to)] {
                let Endpoint::Pin { item: on, pin } = end else {
                    continue;
                };
                let Some(user) = users.iter().find(|u| u.id == *on) else {
                    continue;
                };
                match (old.pin(pin), now_of(pin).and_then(|n| new.pin(&n))) {
                    (was, None) => {
                        warnings.push(format!(
                            "wire {} on page {} ended on connection point {pin} of {on}, which is gone: its end stays where the point was",
                            w.id, p.number
                        ));
                        if let Some(a) = was {
                            ends.push(EndChange {
                                page: pi,
                                wire: wi,
                                to_end,
                                end: Endpoint::Point {
                                    at: a.at.transform(user.rotation, user.mirror) + user.at,
                                },
                            });
                        }
                    }
                    (Some(a), Some(b)) => {
                        // The moved box's points are compared where they are on the page.
                        let (was, now) = if moves && pi == edit.page && on == edit.item {
                            (
                                a.at.transform(rotation, mirror),
                                b.at.transform(rotation, mirror) + edit.moved,
                            )
                        } else {
                            (a.at, b.at)
                        };
                        if was.distance(now) > 1e-6 || a.dir != b.dir {
                            warnings.push(format!(
                                "wire {} on page {} ends on connection point {} of {on}, which moved: check its route",
                                w.id, p.number, b.name
                            ));
                        }
                        if b.name != *pin {
                            ends.push(EndChange {
                                page: pi,
                                wire: wi,
                                to_end,
                                end: Endpoint::Pin {
                                    item: on.clone(),
                                    pin: b.name.clone(),
                                },
                            });
                        }
                    }
                    (None, Some(_)) => {
                        warnings.push(format!(
                            "wire {} on page {} ends on connection point {pin} of {on}, which the box did not have before: it now ends on the new point",
                            w.id, p.number
                        ));
                    }
                }
            }
        }
    }
    if others > 0 {
        warnings.push(format!(
            "{others} other placed box{} use{} the same symbol and changed too",
            if others == 1 { "" } else { "es" },
            if others == 1 { "s" } else { "" }
        ));
    }
    let plan = BoxEditPlan {
        symbol: new,
        ends,
        warnings,
    };
    // A removed point's free end on another wire's free end would join the two, which were
    // not connected before (#609).
    if plan.ends.iter().any(|c| matches!(c.end, Endpoint::Point { .. })) {
        let mut after = project.clone();
        plan.apply_ends(&mut after);
        if let Some(j) = crate::connectivity::new_point_joins(project, &after).first() {
            let number = project.pages.get(j.page).map_or("?", |p| p.number.as_str());
            return bad(format!(
                "wire {} on page {number} would keep a free end at ({}, {}), where wire {} already ends, and the two would be joined; move one of these wire ends first",
                j.wire, j.at.x, j.at.y, j.other
            ));
        }
    }
    Ok(Some(plan))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rotation;
    use crate::{Page, PlacedSymbol, Wire};

    fn pin(name: &str, side: BoxSide, offset: f64) -> BoxPin {
        BoxPin {
            name: name.into(),
            side,
            offset,
        }
    }

    fn wire(id: &str, from: Endpoint, to: Endpoint) -> Wire {
        Wire {
            id: id.into(),
            from,
            to,
            via: Vec::new(),
            number: None,
            potential: None,
            cable: None,
            route: Default::default(),
            spec: Default::default(),
        }
    }

    fn on(item: &str, pin: &str) -> Endpoint {
        Endpoint::Pin {
            item: item.into(),
            pin: pin.into(),
        }
    }

    fn free(x: f64, y: f64) -> Endpoint {
        Endpoint::Point { at: Point::new(x, y) }
    }

    /// A 20 by 10 box `b1` at (100, 50) with points A (left, 5) and B (right, 5), wire w1 on A
    /// and w2 on B; a second page places the same symbol as `b9` with wire w3 on A.
    fn setup() -> (Project, Symbol) {
        let sym = crate::black_box(
            "bb",
            "Box",
            20.0,
            10.0,
            &[pin("A", BoxSide::Left, 5.0), pin("B", BoxSide::Right, 5.0)],
        )
        .unwrap_or_else(|e| unreachable!("{e}"));
        let mut p = Project::new("t");
        let place = |id: &str, at: Point| PlacedSymbol {
            id: id.into(),
            symbol: "bb".into(),
            at,
            rotation: Rotation::R0,
            mirror: false,
            tag: None,
        };
        let mut one = Page::new("p1", "1", "One");
        one.symbols.push(place("b1", Point::new(100.0, 50.0)));
        one.wires.push(wire("w1", on("b1", "A"), free(80.0, 55.0)));
        one.wires.push(wire("w2", free(140.0, 55.0), on("b1", "B")));
        let mut two = Page::new("p2", "2", "Two");
        two.symbols.push(place("b9", Point::new(10.0, 10.0)));
        two.wires.push(wire("w3", on("b9", "A"), free(0.0, 15.0)));
        p.pages = vec![one, two];
        (p, sym)
    }

    fn edit<'a>(
        size: (f64, f64),
        pins: &'a [BoxPin],
        renamed: &'a BTreeMap<String, String>,
        removed: &'a BTreeSet<String>,
    ) -> BoxEdit<'a> {
        BoxEdit {
            page: 0,
            item: "b1",
            size,
            pins,
            renamed,
            removed,
            moved: Point::new(0.0, 0.0),
        }
    }

    fn run(p: &Project, sym: &Symbol, e: &BoxEdit<'_>) -> BoxEditPlan {
        match plan(p, sym, e) {
            Ok(Some(plan)) => plan,
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn nothing_to_change_is_none() {
        let (p, sym) = setup();
        let pins = [pin("A", BoxSide::Left, 5.0), pin("B", BoxSide::Right, 5.0)];
        let (r, d) = (BTreeMap::new(), BTreeSet::new());
        assert!(matches!(plan(&p, &sym, &edit((20.0, 10.0), &pins, &r, &d)), Ok(None)));
    }

    #[test]
    fn swapped_points_swap_their_wires_on_every_copy() {
        let (mut p, sym) = setup();
        let pins = [pin("B", BoxSide::Left, 5.0), pin("A", BoxSide::Right, 5.0)];
        let r: BTreeMap<String, String> = [("A".into(), "B".into()), ("B".into(), "A".into())].into();
        let d = BTreeSet::new();
        let plan = run(&p, &sym, &edit((20.0, 10.0), &pins, &r, &d));
        plan.apply_ends(&mut p);
        assert_eq!(p.pages[0].wires[0].from, on("b1", "B"));
        assert_eq!(p.pages[0].wires[1].to, on("b1", "A"));
        assert_eq!(p.pages[1].wires[0].from, on("b9", "B"));
        // The points kept their places, so no route warnings; the other box is named.
        assert_eq!(
            plan.warnings,
            vec!["1 other placed box uses the same symbol and changed too"]
        );
    }

    #[test]
    fn a_removed_point_leaves_a_free_end_where_it_was_even_when_its_name_is_reused() {
        let (mut p, sym) = setup();
        // A is taken off and a new point A is added on the top edge.
        let pins = [pin("A", BoxSide::Top, 10.0), pin("B", BoxSide::Right, 5.0)];
        let r = BTreeMap::new();
        let d: BTreeSet<String> = ["A".to_owned()].into();
        let plan = run(&p, &sym, &edit((20.0, 10.0), &pins, &r, &d));
        plan.apply_ends(&mut p);
        assert_eq!(p.pages[0].wires[0].from, free(100.0, 55.0));
        assert_eq!(p.pages[1].wires[0].from, free(10.0, 15.0));
        assert_eq!(p.pages[0].wires[1].to, on("b1", "B"));
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.starts_with("wire w1 on page 1 ended on connection point A"))
        );
    }

    #[test]
    fn a_removed_point_may_not_join_its_wire_to_another_net_at_a_free_end() {
        let (mut p, sym) = setup();
        let pins = [pin("B", BoxSide::Right, 5.0)];
        let r = BTreeMap::new();
        let d: BTreeSet<String> = ["A".to_owned()].into();
        // w5 ends where A is (100, 55) and at w1's other end: already w1's net, so it may.
        p.pages[0].wires.push(wire("w5", free(100.0, 55.0), free(80.0, 55.0)));
        let plan = run(&p, &sym, &edit((20.0, 10.0), &pins, &r, &d));
        assert_eq!(plan.ends.len(), 2);
        // w6 ends there instead, on no net of w1's: refused, naming both wires.
        p.pages[0].wires.pop();
        p.pages[0].wires.push(wire("w6", free(100.0, 55.0), free(0.0, 0.0)));
        let Err(err) = plan_err(&p, &sym, &edit((20.0, 10.0), &pins, &r, &d)) else {
            unreachable!()
        };
        assert!(
            err.contains("wire w1 on page 1") && err.contains("wire w6") && err.contains("joined"),
            "{err}"
        );
    }

    fn plan_err(p: &Project, sym: &Symbol, e: &BoxEdit<'_>) -> Result<(), String> {
        plan(p, sym, e).map(|_| ()).map_err(|e| e.to_string())
    }

    #[test]
    fn points_pulled_onto_a_smaller_box_and_a_moved_box_warn_about_their_routes() {
        let (p, sym) = setup();
        let Some((_, _, old)) = crate::black_box_layout(&sym) else {
            unreachable!()
        };
        let r = BTreeMap::new();
        let d = BTreeSet::new();
        let pins = kept_pins(&old, &r, 20.0, 4.0);
        assert_eq!(pins, vec![pin("A", BoxSide::Left, 4.0), pin("B", BoxSide::Right, 4.0)]);
        let plan = run(&p, &sym, &edit((20.0, 4.0), &pins, &r, &d));
        assert!(plan.ends.is_empty());
        assert_eq!(plan.warnings.iter().filter(|w| w.contains("which moved")).count(), 3);
        // Same size, but the box moves: only its own wires are checked against the page.
        let pins = kept_pins(&old, &r, 20.0, 10.0);
        let mut e = edit((20.0, 10.0), &pins, &r, &d);
        e.moved = Point::new(5.0, 0.0);
        let plan = run(&p, &sym, &e);
        assert_eq!(plan.warnings.iter().filter(|w| w.contains("which moved")).count(), 2);
    }

    #[test]
    fn corner_collisions_and_bad_renames_are_refused() {
        let (p, sym) = setup();
        let (r, d) = (BTreeMap::new(), BTreeSet::new());
        // Two points at one corner, and a box without area, are refused by the box itself.
        let pins = [pin("A", BoxSide::Top, 20.0), pin("B", BoxSide::Right, 0.0)];
        assert!(plan(&p, &sym, &edit((20.0, 10.0), &pins, &r, &d)).is_err());
        assert!(plan(&p, &sym, &edit((0.0, 10.0), &[], &r, &d)).is_err());
        let bad_rename: BTreeMap<String, String> = [("A".into(), "Q".into())].into();
        let pins = [pin("A", BoxSide::Left, 5.0)];
        let err = plan(&p, &sym, &edit((20.0, 10.0), &pins, &bad_rename, &d)).err();
        assert!(err.is_some_and(|e| e.to_string().contains("no connection point Q")));
        // Two points renamed to one: their wires would join.
        let both: BTreeMap<String, String> = [("A".into(), "Q".into()), ("B".into(), "Q".into())].into();
        let pins = [pin("Q", BoxSide::Left, 5.0)];
        let err = plan(&p, &sym, &edit((20.0, 10.0), &pins, &both, &d)).err();
        assert!(
            err.as_ref().is_some_and(|e| e.to_string().contains("renamed to Q")),
            "{err:?}"
        );
        let pins = [pin("A", BoxSide::Left, 5.0)];
        // A valid edit of a box that is not on the page.
        let mut e = edit((30.0, 10.0), &pins, &r, &d);
        e.item = "nope";
        let err = plan(&p, &sym, &e).err();
        assert!(
            err.as_ref().is_some_and(|e| e.to_string().contains("no box nope")),
            "{err:?}"
        );
    }
}

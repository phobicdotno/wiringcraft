//! Find and replace across a project: free texts, device tags, wire numbers, potentials and
//! device descriptions on every page.
//!
//! [`find`] lists the hits; [`replace`] changes chosen hits. Replacing a device tag renames
//! the device everywhere it appears ([`Project::rename_device`]), so a renamed tag never leaves
//! a symbol, cable core, panel item or jumper behind.

use std::collections::BTreeMap;

use crate::geom::Shape;
use crate::project::Project;
use crate::tag::DeviceTag;

/// What a hit is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FindKind {
    /// A free text on a page.
    Text,
    /// A placed symbol's device tag.
    Tag,
    /// A wire number.
    WireNumber,
    /// A wire's potential (signal name).
    Potential,
    /// A device description.
    Description,
}

impl FindKind {
    /// Every kind, in the order hits are listed.
    pub const ALL: [FindKind; 5] = [
        FindKind::Text,
        FindKind::Tag,
        FindKind::WireNumber,
        FindKind::Potential,
        FindKind::Description,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FindKind::Text => "Text",
            FindKind::Tag => "Device tag",
            FindKind::WireNumber => "Wire number",
            FindKind::Potential => "Potential",
            FindKind::Description => "Description",
        }
    }
}

/// Where a hit is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FindTarget {
    /// A placed symbol on `page`, by id.
    Symbol { page: String, id: String },
    /// A wire on `page`, by id.
    Wire { page: String, id: String },
    /// A free drawing on `page`, by its index in the page's graphics.
    Graphic { page: String, index: usize },
    /// A device, by tag. A device that is drawn also gets its first symbol in `shown_at`.
    Device {
        tag: DeviceTag,
        shown_at: Option<(String, String)>,
    },
}

impl FindTarget {
    /// The page to open and the item to select there, when there is one.
    pub fn page(&self) -> Option<&str> {
        match self {
            FindTarget::Symbol { page, .. } | FindTarget::Wire { page, .. } | FindTarget::Graphic { page, .. } => {
                Some(page)
            }
            FindTarget::Device { shown_at, .. } => shown_at.as_ref().map(|(p, _)| p.as_str()),
        }
    }
}

/// One place where the search text was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub kind: FindKind,
    pub target: FindTarget,
    /// The whole value the text was found in, e.g. the full tag `-K12`.
    pub value: String,
}

/// How to compare.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FindOptions {
    /// Upper and lower case must match.
    pub match_case: bool,
}

/// Whether `needle` occurs in `hay`.
fn contains(hay: &str, needle: &str, opts: FindOptions) -> bool {
    !positions(hay, needle, opts, false).is_empty()
}

/// Whether a match may not start or end between `a` and `b`: both are letters or both are
/// digits, so they belong to one part of a tag (`K` and `15` in `-K15`).
fn one_part(a: char, b: char) -> bool {
    (a.is_alphabetic() && b.is_alphabetic()) || (a.is_numeric() && b.is_numeric())
}

/// Whether a match may not end between `a` and `b`: [`one_part`], or a number and the letter
/// that follows it (`1` and `A` in `-K1A`, which is another device than `-K1`).
fn one_part_at_end(a: char, b: char) -> bool {
    one_part(a, b) || (a.is_numeric() && b.is_alphabetic())
}

/// The char positions in `hay` where `needle` starts, without overlaps, left to right. With
/// `whole_parts` only where it covers whole runs of letters and of digits, so `-K1` is not
/// found in `-K15`, and where no letter follows its last digit, so not in `-K1A` either.
fn positions(hay: &str, needle: &str, opts: FindOptions, whole_parts: bool) -> Vec<usize> {
    let h: Vec<char> = hay.chars().collect();
    let n: Vec<char> = needle.chars().collect();
    let mut out = Vec::new();
    if n.is_empty() || n.len() > h.len() {
        return out;
    }
    let same = |a: char, b: char| {
        if opts.match_case {
            a == b
        } else {
            a == b || a.to_lowercase().eq(b.to_lowercase())
        }
    };
    let mut i = 0;
    while i + n.len() <= h.len() {
        let here = h
            .get(i..i + n.len())
            .is_some_and(|w| w.iter().zip(&n).all(|(&a, &b)| same(a, b)));
        let around = |at: usize| {
            at.checked_sub(1)
                .and_then(|b| h.get(b))
                .copied()
                .zip(h.get(at).copied())
        };
        let cut = around(i).is_some_and(|(a, b)| one_part(a, b))
            || around(i + n.len()).is_some_and(|(a, b)| one_part_at_end(a, b));
        if here && !(whole_parts && cut) {
            out.push(i);
            i += n.len();
        } else {
            i += 1;
        }
    }
    out
}

/// `hay` with every occurrence of `needle` replaced by `with`.
pub fn replace_text(hay: &str, needle: &str, with: &str, opts: FindOptions) -> String {
    replace_at(hay, needle, with, &positions(hay, needle, opts, false))
}

/// A device tag with `needle` replaced by `with` where it covers whole parts of the tag: runs
/// of letters and runs of digits are not cut, so `-K1` to `-K2` leaves `-K15` as it is, while
/// `K` to `Q` makes it `-Q15`.
pub fn replace_in_tag(tag: &str, needle: &str, with: &str, opts: FindOptions) -> String {
    replace_at(tag, needle, with, &positions(tag, needle, opts, true))
}

/// `hay` with the `needle` at each of the char positions `at` replaced by `with`.
fn replace_at(hay: &str, needle: &str, with: &str, at: &[usize]) -> String {
    if at.is_empty() {
        return hay.to_owned();
    }
    let n = needle.chars().count();
    let mut out = String::new();
    let mut next = at.iter().copied().peekable();
    let mut skip = 0usize;
    for (i, c) in hay.chars().enumerate() {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        if next.peek() == Some(&i) {
            next.next();
            out.push_str(with);
            skip = n - 1;
        } else {
            out.push(c);
        }
    }
    out
}

/// Why a replace leaves a hit as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LeftAlone {
    /// The text was found inside a part of the tag, so the tag is another device's: `-K15`
    /// for `-K1` ([`replace_in_tag`]).
    AnotherTag,
    /// The value is the same after the replace: the replacement is what already stands there.
    SameValue,
}

impl LeftAlone {
    /// The reason in a few words, as shown next to the hit.
    pub fn reason(self) -> &'static str {
        match self {
            LeftAlone::AnotherTag => "another tag",
            LeftAlone::SameValue => "already so",
        }
    }
}

/// Whether [`replace`] of `query` with `with` would leave this hit (from [`find`] with the same
/// query and options) as it is, and why. `None` when the replace changes it, also when the
/// change would be refused (a renamed tag that is not a valid tag).
pub fn left_alone(hit: &Hit, query: &str, with: &str, opts: FindOptions) -> Option<LeftAlone> {
    if hit.kind == FindKind::Tag {
        if positions(&hit.value, query, opts, true).is_empty() {
            return Some(LeftAlone::AnotherTag);
        }
        let new = replace_in_tag(&hit.value, query, with, opts);
        let same = match (DeviceTag::parse(&hit.value), DeviceTag::parse(&new)) {
            (Ok(from), Ok(to)) => from == to,
            _ => new == hit.value,
        };
        return same.then_some(LeftAlone::SameValue);
    }
    (replace_text(&hit.value, query, with, opts) == hit.value).then_some(LeftAlone::SameValue)
}

/// Every hit for `query` in the project, page by page in page order, then the device
/// descriptions. An empty query finds nothing.
pub fn find(project: &Project, query: &str, opts: FindOptions) -> Vec<Hit> {
    let mut hits = Vec::new();
    if query.is_empty() {
        return hits;
    }
    for page in &project.pages {
        for (index, g) in page.graphics.iter().enumerate() {
            if let Shape::Text { text, .. } = g
                && contains(text, query, opts)
            {
                hits.push(Hit {
                    kind: FindKind::Text,
                    target: FindTarget::Graphic {
                        page: page.id.clone(),
                        index,
                    },
                    value: text.clone(),
                });
            }
        }
        for s in &page.symbols {
            if let Some(tag) = &s.tag {
                let value = tag.to_string();
                if contains(&value, query, opts) {
                    hits.push(Hit {
                        kind: FindKind::Tag,
                        target: FindTarget::Symbol {
                            page: page.id.clone(),
                            id: s.id.clone(),
                        },
                        value,
                    });
                }
            }
        }
        for w in &page.wires {
            for (kind, value) in [(FindKind::WireNumber, &w.number), (FindKind::Potential, &w.potential)] {
                if let Some(v) = value
                    && contains(v, query, opts)
                {
                    hits.push(Hit {
                        kind,
                        target: FindTarget::Wire {
                            page: page.id.clone(),
                            id: w.id.clone(),
                        },
                        value: v.clone(),
                    });
                }
            }
        }
    }
    let shown = project.tag_index();
    // A device that no page draws is found by its tag here; a drawn one by its symbols above.
    // A cable that only wire cores name (no symbol, no device entry) is found here too.
    let undrawn: std::collections::BTreeSet<&DeviceTag> = project
        .devices
        .keys()
        .chain(core_cables(project))
        .filter(|t| !shown.contains_key(*t))
        .collect();
    for tag in undrawn {
        let value = tag.to_string();
        if contains(&value, query, opts) {
            hits.push(Hit {
                kind: FindKind::Tag,
                target: FindTarget::Device {
                    tag: tag.clone(),
                    shown_at: None,
                },
                value,
            });
        }
    }
    for (tag, d) in &project.devices {
        if contains(&d.description, query, opts) {
            hits.push(Hit {
                kind: FindKind::Description,
                target: FindTarget::Device {
                    tag: tag.clone(),
                    shown_at: shown.get(tag).and_then(|at| at.first().cloned()),
                },
                value: d.description.clone(),
            });
        }
    }
    hits
}

/// The cables named by wire cores on any page.
fn core_cables(project: &Project) -> impl Iterator<Item = &DeviceTag> {
    project
        .pages
        .iter()
        .flat_map(|p| p.wires.iter().filter_map(|w| w.cable.as_ref().map(|c| &c.cable)))
}

/// Replace `query` with `with` in the chosen `hits` (from [`find`] with the same query and
/// options). All or nothing: when a renamed tag is not a valid tag or is already used, or a hit
/// no longer holds the value it was found with (the project changed since the search), nothing
/// changes and the reason comes back. Returns how many values changed.
///
/// A device tag hit renames that device everywhere it appears, once per device, even when
/// several of its symbols are chosen. In a tag the query counts only where it covers whole
/// parts ([`replace_in_tag`]): a hit on `-K15` for `-K1` keeps its tag.
pub fn replace(
    project: &mut Project,
    hits: &[Hit],
    query: &str,
    with: &str,
    opts: FindOptions,
) -> Result<usize, String> {
    replace_renaming(project, hits, query, with, opts).map(|(n, _)| n)
}

/// [`replace`], also returning the devices it renamed: old tag to new tag.
pub fn replace_renaming(
    project: &mut Project,
    hits: &[Hit],
    query: &str,
    with: &str,
    opts: FindOptions,
) -> Result<(usize, BTreeMap<DeviceTag, DeviceTag>), String> {
    if query.is_empty() {
        return Ok((0, BTreeMap::new()));
    }
    let mut p = project.clone();
    let mut changed = 0usize;
    let mut renames: BTreeMap<DeviceTag, DeviceTag> = BTreeMap::new();
    let stale = || "The project changed since the search; search again.".to_owned();
    for hit in hits {
        match (&hit.kind, &hit.target) {
            (FindKind::Text, FindTarget::Graphic { page, index }) => {
                if let Some(Shape::Text { text, .. }) = p
                    .pages
                    .iter_mut()
                    .find(|pg| &pg.id == page)
                    .and_then(|pg| pg.graphics.get_mut(*index))
                {
                    if *text != hit.value {
                        return Err(stale());
                    }
                    let new = replace_text(text, query, with, opts);
                    if new != *text {
                        *text = new;
                        changed += 1;
                    }
                } else {
                    return Err(stale());
                }
            }
            (FindKind::WireNumber | FindKind::Potential, FindTarget::Wire { page, id }) => {
                let wire = p
                    .pages
                    .iter_mut()
                    .find(|pg| &pg.id == page)
                    .and_then(|pg| pg.wires.iter_mut().find(|w| &w.id == id));
                if let Some(w) = wire {
                    let field = if hit.kind == FindKind::WireNumber {
                        &mut w.number
                    } else {
                        &mut w.potential
                    };
                    if field.as_deref() != Some(hit.value.as_str()) {
                        return Err(stale());
                    }
                    if let Some(v) = field {
                        let new = replace_text(v, query, with, opts);
                        if new != *v {
                            // An emptied number or potential is no number or potential.
                            *field = (!new.trim().is_empty()).then_some(new);
                            changed += 1;
                        }
                    }
                } else {
                    return Err(stale());
                }
            }
            (FindKind::Description, FindTarget::Device { tag, .. }) => {
                if let Some(d) = p.devices.get_mut(tag) {
                    if d.description != hit.value {
                        return Err(stale());
                    }
                    let new = replace_text(&d.description, query, with, opts);
                    if new != d.description {
                        d.description = new;
                        changed += 1;
                    }
                } else {
                    return Err(stale());
                }
            }
            (FindKind::Tag, target @ (FindTarget::Symbol { .. } | FindTarget::Device { .. })) => {
                let tag = match target {
                    FindTarget::Symbol { page, id } => {
                        p.page(page).and_then(|pg| pg.symbol(id)).and_then(|s| s.tag.clone())
                    }
                    FindTarget::Device { tag, .. } => {
                        (p.devices.contains_key(tag) || core_cables(&p).any(|c| c == tag)).then(|| tag.clone())
                    }
                    _ => None,
                };
                if tag.as_ref().map(ToString::to_string).as_deref() != Some(hit.value.as_str()) {
                    return Err(stale());
                }
                if let Some(from) = tag {
                    let text = replace_in_tag(&from.to_string(), query, with, opts);
                    let to = DeviceTag::parse(&text).map_err(|e| format!("{from} would become \"{text}\": {e}"))?;
                    if to != from {
                        renames.insert(from, to);
                    }
                }
            }
            _ => {}
        }
    }
    // Two devices may not end up with one tag, and a new tag may not be taken by a device that
    // keeps its own.
    let mut targets: BTreeMap<&DeviceTag, &DeviceTag> = BTreeMap::new();
    for (from, to) in &renames {
        if let Some(other) = targets.insert(to, from) {
            return Err(format!("{other} and {from} would both become {to}."));
        }
    }
    // A tag that only a wire's cable core names is in use too.
    let cables: std::collections::BTreeSet<DeviceTag> = p
        .pages
        .iter()
        .flat_map(|pg| pg.wires.iter())
        .filter_map(|w| w.cable.as_ref().map(|c| c.cable.clone()))
        .collect();
    let taken: Vec<DeviceTag> = p
        .devices
        .keys()
        .cloned()
        .chain(p.tag_index().into_keys())
        .chain(p.meta.strips.iter().map(|l| l.strip.clone()))
        .chain(cables.iter().cloned())
        .filter(|t| !renames.contains_key(t))
        .collect();
    if let Some(to) = renames.values().find(|to| taken.contains(to)) {
        return Err(format!("{to} is already used."));
    }
    // A renamed strip's spares that are not renamed themselves go with it, so their new tags
    // must be free, also of the other renames and of each other.
    let mut spare_targets: std::collections::BTreeSet<DeviceTag> = std::collections::BTreeSet::new();
    let drawn = p.tag_index();
    for (from, to) in &renames {
        for spare in p
            .meta
            .strip_layout(from)
            .map(|l| l.spares.as_slice())
            .unwrap_or_default()
        {
            let (mut old, mut new) = (from.clone(), to.clone());
            old.product.push(spare.clone());
            new.product.push(spare.clone());
            if !p.devices.contains_key(&old) || renames.contains_key(&old) {
                continue;
            }
            let held = (p.devices.contains_key(&new) || drawn.contains_key(&new) || cables.contains(&new))
                && !renames.contains_key(&new);
            if held || targets.contains_key(&new) || !spare_targets.insert(new.clone()) {
                return Err(format!("{new} is already used."));
            }
        }
    }
    // Renamed in two steps through placeholders, so `-K1` -> `-K2` and `-K2` -> `-K3` in one
    // replace do not run into each other. A placeholder is a tag nothing in the project uses
    // and no rename gives.
    let mut used: std::collections::BTreeSet<DeviceTag> = p
        .devices
        .keys()
        .cloned()
        .chain(p.tag_index().into_keys())
        .chain(p.meta.strips.iter().map(|l| l.strip.clone()))
        .chain(renames.values().cloned())
        .chain(cables.iter().cloned())
        .collect();
    let mut next = 0usize;
    let mut placed: Vec<(DeviceTag, DeviceTag)> = Vec::new();
    // Deepest tags first: a strip's spare renamed in the same replace moves under its own
    // rename before the strip would take it along.
    let mut order: Vec<(&DeviceTag, &DeviceTag)> = renames.iter().collect();
    order.sort_by_key(|(from, _)| std::cmp::Reverse(from.product.len()));
    for (from, to) in order {
        // The strip's spares go through the placeholder too, so their tags there must be free.
        let spares: Vec<String> = p.meta.strip_layout(from).map(|l| l.spares.clone()).unwrap_or_default();
        let temp = loop {
            let t = DeviceTag::parse(&format!("-WCFIND{next}")).map_err(|e| e.to_string())?;
            next += 1;
            let spare_free = spares.iter().all(|n| {
                let mut s = t.clone();
                s.product.push(n.clone());
                !used.contains(&s)
            });
            if !used.contains(&t) && spare_free {
                break t;
            }
        };
        used.insert(temp.clone());
        p.rename_device(from, &temp);
        placed.push((temp, to.clone()));
    }
    for (temp, to) in &placed {
        // A device no page draws changes only its entry: still one change.
        changed += p.rename_device(temp, to).max(1);
    }
    // A terminal or spare renamed along with its strip (`-X12.S2` -> `-X15.S5` with `-X12` ->
    // `-X15`) keeps its place on the rail under its new number. Each layout's numbers are
    // mapped from one snapshot, so `S1` -> `S11` and `S11` -> `S1111` do not run into each other.
    let split = |t: &DeviceTag| {
        let mut strip = t.clone();
        strip.product.pop().map(|n| (strip, n))
    };
    let mut numbers: BTreeMap<DeviceTag, BTreeMap<String, String>> = BTreeMap::new();
    for (from, to) in &renames {
        let (Some((old_strip, old_n)), Some((new_strip, new_n))) = (split(from), split(to)) else {
            continue;
        };
        if renames.get(&old_strip) == Some(&new_strip) {
            numbers.entry(new_strip).or_default().insert(old_n, new_n);
        }
    }
    for l in &mut p.meta.strips {
        if let Some(map) = numbers.get(&l.strip) {
            for list in [&mut l.spares, &mut l.order] {
                let mut out: Vec<String> = Vec::new();
                for n in list.iter() {
                    let m = map.get(n).unwrap_or(n);
                    if !out.contains(m) {
                        out.push(m.clone());
                    }
                }
                *list = out;
            }
        }
    }
    *project = p;
    Ok((changed, renames))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Point;
    use crate::project::{Device, Endpoint, Page, PlacedSymbol, Wire};

    fn tag(s: &str) -> DeviceTag {
        DeviceTag::parse(s).unwrap_or_else(|e| unreachable!("{s}: {e}"))
    }

    fn sample() -> Project {
        let mut p = Project::new("Find");
        let mut a = Page::new("p1", "1", "Supply");
        a.graphics.push(Shape::Text {
            at: Point::new(10.0, 10.0),
            text: "Pump 1 supply".into(),
            height: 3.5,
            align: Default::default(),
            angle: 0.0,
        });
        for (id, t) in [("s1", "-K1"), ("s2", "-K12"), ("s3", "-F1")] {
            a.symbols.push(PlacedSymbol {
                id: id.into(),
                symbol: "wc.coil".into(),
                at: Point::new(20.0, 20.0),
                rotation: Default::default(),
                mirror: false,
                tag: Some(tag(t)),
            });
        }
        let mut w = Wire {
            id: "w1".into(),
            from: Endpoint::Point {
                at: Point::new(0.0, 0.0),
            },
            to: Endpoint::Point {
                at: Point::new(5.0, 0.0),
            },
            via: Vec::new(),
            number: Some("K1-101".into()),
            potential: Some("L1".into()),
            cable: None,
            route: Default::default(),
            spec: Default::default(),
        };
        a.wires.push(w.clone());
        let mut b = Page::new("p2", "2", "Motor");
        w.id = "w2".into();
        w.number = Some("201".into());
        b.wires.push(w);
        b.symbols.push(PlacedSymbol {
            id: "s1".into(),
            symbol: "wc.contact".into(),
            at: Point::new(20.0, 20.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(tag("-K1")),
        });
        p.pages = vec![a, b];
        p.devices.insert(
            tag("-K1"),
            Device {
                description: "Pump relay".into(),
                ..Default::default()
            },
        );
        p.devices.insert(
            tag("-X9"),
            Device {
                description: "Spare pump terminal".into(),
                ..Default::default()
            },
        );
        p
    }

    #[test]
    fn finds_every_kind_on_every_page() {
        let p = sample();
        let hits = find(&p, "k1", FindOptions::default());
        let kinds: Vec<(FindKind, &str)> = hits.iter().map(|h| (h.kind, h.value.as_str())).collect();
        assert_eq!(
            kinds,
            [
                (FindKind::Tag, "-K1"),
                (FindKind::Tag, "-K12"),
                (FindKind::WireNumber, "K1-101"),
                (FindKind::Tag, "-K1"),
            ]
        );
        assert_eq!(hits[3].target.page(), Some("p2"));
        assert!(find(&p, "k1", FindOptions { match_case: true }).is_empty());

        let pump = find(&p, "pump", FindOptions::default());
        assert_eq!(pump.len(), 3, "{pump:?}");
        assert_eq!(pump[0].kind, FindKind::Text);
        let undrawn = pump
            .iter()
            .find(|h| h.value == "Spare pump terminal")
            .map(|h| h.target.page());
        assert_eq!(undrawn, Some(None), "a device that is not drawn has no page");
        let drawn = pump.iter().find(|h| h.value == "Pump relay").map(|h| h.target.page());
        assert_eq!(drawn, Some(Some("p1")));
        assert_eq!(find(&p, "L1", FindOptions::default()).len(), 2, "both wires' potential");
        assert!(find(&p, "", FindOptions::default()).is_empty());
    }

    #[test]
    fn replaces_texts_numbers_potentials_and_descriptions() {
        let mut p = sample();
        let opts = FindOptions::default();
        let hits = find(&p, "pump", opts);
        assert_eq!(replace(&mut p, &hits, "pump", "Fan", opts), Ok(3));
        assert!(matches!(&p.pages[0].graphics[0], Shape::Text { text, .. } if text == "Fan 1 supply"));
        assert_eq!(p.devices[&tag("-K1")].description, "Fan relay");
        assert_eq!(p.devices[&tag("-X9")].description, "Spare Fan terminal");

        // Only the chosen hit changes.
        let hits = find(&p, "L1", opts);
        assert_eq!(replace(&mut p, &hits[..1], "L1", "L2", opts), Ok(1));
        assert_eq!(p.pages[0].wires[0].potential.as_deref(), Some("L2"));
        assert_eq!(p.pages[1].wires[0].potential.as_deref(), Some("L1"));

        // Replacing a whole wire number with nothing removes it.
        let hits = find(&p, "201", opts);
        assert_eq!(replace(&mut p, &hits, "201", "", opts), Ok(1));
        assert_eq!(p.pages[1].wires[0].number, None);
    }

    #[test]
    fn a_tag_is_renamed_everywhere_once() {
        let mut p = sample();
        let opts = FindOptions { match_case: true };
        // Only the symbol on page 2 is chosen; the device is renamed on both pages.
        let hits: Vec<Hit> = find(&p, "-K1", opts)
            .into_iter()
            .filter(|h| h.kind == FindKind::Tag && h.value == "-K1" && h.target.page() == Some("p2"))
            .collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(replace(&mut p, &hits, "-K1", "-K5", opts), Ok(2));
        let tags: Vec<String> = p
            .pages
            .iter()
            .flat_map(|pg| {
                pg.symbols
                    .iter()
                    .filter_map(|s| s.tag.as_ref().map(ToString::to_string))
            })
            .collect();
        assert_eq!(tags, ["-K5", "-K12", "-F1", "-K5"]);
        assert!(p.devices.contains_key(&tag("-K5")) && !p.devices.contains_key(&tag("-K1")));
        assert_eq!(p.pages[0].wires[0].number.as_deref(), Some("K1-101"), "not chosen");
    }

    #[test]
    fn tags_can_move_up_one_in_a_single_replace() {
        let mut p = sample();
        p.devices.insert(tag("-K1.1"), Device::default());
        let opts = FindOptions::default();
        // -K1 -> -K1.1 would clash with the existing -K1.1, unless that moves too: "K1" ->
        // "K1.1" makes it -K1.1.1, so both move and nothing clashes. -K12 is another device.
        let hits: Vec<Hit> = find(&p, "K1", opts)
            .into_iter()
            .filter(|h| h.kind == FindKind::Tag)
            .collect();
        assert_eq!(replace(&mut p, &hits, "K1", "K1.1", opts), Ok(3));
        assert!(p.devices.contains_key(&tag("-K1.1")) && p.devices.contains_key(&tag("-K1.1.1")));
        let tags: Vec<String> = p.pages[0]
            .symbols
            .iter()
            .filter_map(|s| s.tag.as_ref().map(ToString::to_string))
            .collect();
        assert_eq!(tags, ["-K1.1", "-K12", "-F1"]);
    }

    #[test]
    fn a_tag_replace_leaves_longer_tags_alone() {
        let mut p = sample();
        let opts = FindOptions::default();
        // -K12 is listed as a hit for "-K1", but it is another device: only -K1 is renamed.
        let hits = find(&p, "-K1", opts);
        assert!(hits.iter().any(|h| h.value == "-K12"));
        assert_eq!(replace(&mut p, &hits, "-K1", "-K2", opts), Ok(2));
        let tags: Vec<String> = p.pages[0]
            .symbols
            .iter()
            .filter_map(|s| s.tag.as_ref().map(ToString::to_string))
            .collect();
        assert_eq!(tags, ["-K2", "-K12", "-F1"]);
        assert_eq!(p.pages[0].wires[0].number.as_deref(), Some("K1-101"), "no tag in it");

        assert_eq!(replace_in_tag("-K15", "K1", "K2", opts), "-K15");
        assert_eq!(replace_in_tag("-K15", "5", "6", opts), "-K15");
        assert_eq!(replace_in_tag("-XK1", "K1", "K2", opts), "-XK1", "K is half of XK");
        assert_eq!(replace_in_tag("-K15", "k", "Q", opts), "-Q15", "a whole letter code");
        assert_eq!(replace_in_tag("-K1A", "-K1", "-K2", opts), "-K1A", "1A is one number");
        assert_eq!(replace_in_tag("-K1A", "1", "2", opts), "-K1A");
        assert_eq!(replace_in_tag("-K1A", "1A", "2B", opts), "-K2B");
        assert_eq!(replace_in_tag("-K1.A", "-K1", "-K2", opts), "-K2.A");
        assert_eq!(replace_in_tag("=A1+C1-K1", "1", "2", opts), "=A2+C2-K2");
        assert_eq!(replace_in_tag("-X1.11", "X1", "X2", opts), "-X2.11");
        assert_eq!(replace_in_tag("-X1.11", "-X1.11", "-X1.1", opts), "-X1.1");
    }

    #[test]
    fn a_clash_or_a_bad_tag_changes_nothing() {
        let mut p = sample();
        let before = p.clone();
        let opts = FindOptions { match_case: true };
        let hits = find(&p, "F1", opts);
        // -F1 -> -K12 is taken by a device that keeps its tag.
        assert_eq!(
            replace(&mut p, &hits, "F1", "K12", opts),
            Err("-K12 is already used.".into())
        );
        assert_eq!(p, before);
        let all = find(&p, "-", opts);
        assert!(replace(&mut p, &all, "-", "", opts).is_err(), "a tag without its sign");
        assert_eq!(p, before);
        let both: Vec<Hit> = find(&p, "-K1", opts)
            .into_iter()
            .filter(|h| h.kind == FindKind::Tag)
            .collect();
        // -K1 -> -F1 is taken; -K12 -> -F12 would be fine.
        assert!(replace(&mut p, &both, "K1", "F1", opts).is_err());
        assert_eq!(p, before);
    }

    #[test]
    fn an_undrawn_device_is_found_and_renamed_by_its_tag() {
        let mut p = sample();
        p.devices.insert(tag("-Q5"), Device::default());
        let opts = FindOptions::default();
        let hits = find(&p, "Q5", opts);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, FindKind::Tag);
        assert_eq!(hits[0].target.page(), None);
        assert_eq!(
            replace(&mut p, &hits, "Q5", "Q6", opts),
            Ok(1),
            "a change, so it is kept"
        );
        assert!(p.devices.contains_key(&tag("-Q6")) && !p.devices.contains_key(&tag("-Q5")));
        // A drawn device is found by its symbols only, not once more as a device.
        assert!(
            find(&p, "K12", opts)
                .iter()
                .all(|h| matches!(h.target, FindTarget::Symbol { .. }))
        );
    }

    #[test]
    fn a_cable_only_wire_cores_name_is_found_and_renamed() {
        let mut p = sample();
        // -W1 has two cores drawn but no cable symbol and no device entry.
        for (i, core) in ["1", "2"].into_iter().enumerate() {
            if let Some(w) = p.pages.get_mut(i).and_then(|pg| pg.wires.first_mut()) {
                w.cable = Some(crate::project::CableCore {
                    cable: tag("-W1"),
                    core: core.into(),
                });
            }
        }
        let opts = FindOptions { match_case: true };
        let hits = find(&p, "-W1", opts);
        assert_eq!(hits.len(), 1, "found once, not once per core: {hits:?}");
        assert_eq!(hits[0].kind, FindKind::Tag);
        assert_eq!(hits[0].target.page(), None);
        assert_eq!(replace(&mut p, &hits, "-W1", "-W5", opts), Ok(1));
        let cables: Vec<_> = p
            .pages
            .iter()
            .flat_map(|pg| pg.wires.iter().filter_map(|w| w.cable.as_ref()))
            .map(|c| (c.cable.to_string(), c.core.clone()))
            .collect();
        assert_eq!(cables, [("-W5".into(), "1".into()), ("-W5".into(), "2".into())]);
        assert!(!p.devices.contains_key(&tag("-W5")), "no device entry is made up");
        // A drawn cable is found by its symbol only.
        p.pages[0].symbols[0].tag = Some(tag("-W5"));
        assert!(
            find(&p, "-W5", opts)
                .iter()
                .all(|h| matches!(h.target, FindTarget::Symbol { .. }))
        );
    }

    #[test]
    fn a_renamed_strip_keeps_its_rail_order_and_spares() {
        let mut p = sample();
        p.pages[1].symbols.push(PlacedSymbol {
            id: "t1".into(),
            symbol: "wc.terminal".into(),
            at: Point::new(40.0, 20.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(tag("-X1")),
        });
        let layout = |strip: &str| crate::project::StripLayout {
            strip: tag(strip),
            order: vec!["2".into(), "1".into()],
            spares: vec!["S1".into()],
        };
        p.meta.strips.push(layout("-X1"));
        let opts = FindOptions { match_case: true };
        let hits = find(&p, "X1", opts);
        assert_eq!(replace(&mut p, &hits, "X1", "X2", opts), Ok(1));
        assert_eq!(p.meta.strips, [layout("-X2")], "the layout follows the strip");

        // A tag that only a layout uses (a strip of spares) is taken.
        p.meta.strips.push(layout("-X3"));
        let before = p.clone();
        let hits = find(&p, "X2", opts);
        assert_eq!(
            replace(&mut p, &hits, "X2", "X3", opts),
            Err("-X3 is already used.".into())
        );
        assert_eq!(p, before);
    }

    #[test]
    fn a_renamed_strip_stays_in_strip_order_and_takes_its_spares_parts() {
        let mut p = sample();
        p.pages[1].symbols.push(PlacedSymbol {
            id: "t1".into(),
            symbol: "wc.terminal".into(),
            at: Point::new(40.0, 20.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(tag("-X1")),
        });
        let layout = |strip: &str, order: &str| crate::project::StripLayout {
            strip: tag(strip),
            order: vec![order.into()],
            spares: vec!["S1".into()],
        };
        // -WCFIND0 is the first placeholder a rename would pick.
        p.meta.strips = vec![layout("-WCFIND0", "9"), layout("-X1", "2"), layout("-X2", "3")];
        p.devices.insert(
            tag("-X1.S1"),
            Device {
                part: Some("WAGO.2002-1201".into()),
                ..Default::default()
            },
        );
        let opts = FindOptions { match_case: true };
        let hits: Vec<Hit> = find(&p, "-X1", opts).into_iter().filter(|h| h.value == "-X1").collect();
        assert_eq!(replace(&mut p, &hits, "X1", "X3", opts), Ok(1));
        assert_eq!(
            p.meta.strips,
            [layout("-WCFIND0", "9"), layout("-X2", "3"), layout("-X3", "2")],
            "each layout keeps its own tag and contents, in strip order"
        );
        assert_eq!(
            p.devices.get(&tag("-X3.S1")).and_then(|d| d.part.as_deref()),
            Some("WAGO.2002-1201"),
            "the spare's part goes with it"
        );
        assert!(!p.devices.contains_key(&tag("-X1.S1")));

        // A spare's new tag that another device has is taken.
        p.devices.insert(tag("-X4.S1"), Device::default());
        let before = p.clone();
        let hits: Vec<Hit> = find(&p, "-X3", opts).into_iter().filter(|h| h.value == "-X3").collect();
        assert_eq!(
            replace(&mut p, &hits, "X3", "X4", opts),
            Err("-X4.S1 is already used.".into())
        );
        assert_eq!(p, before);
    }

    #[test]
    fn strip_numbers_and_spare_tags_follow_a_rename() {
        let opts = FindOptions { match_case: true };
        let sym = |id: &str, t: &str| PlacedSymbol {
            id: id.into(),
            symbol: "wc.terminal".into(),
            at: Point::new(40.0, 20.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(tag(t)),
        };
        let layout = |strip: &str, order: &[&str], spares: &[&str]| crate::project::StripLayout {
            strip: tag(strip),
            order: order.iter().map(|n| n.to_string()).collect(),
            spares: spares.iter().map(|n| n.to_string()).collect(),
        };
        let part = Device {
            part: Some("A".into()),
            ..Default::default()
        };
        let all = |p: &Project, q: &str| -> Vec<Hit> {
            find(p, q, opts)
                .into_iter()
                .filter(|h| h.kind == FindKind::Tag)
                .collect()
        };

        // Drawn terminals renamed with their strip keep their rail order.
        let mut p = Project::new("Strip");
        let mut pg = Page::new("p1", "1", "Strip");
        pg.symbols = vec![sym("a", "-X2"), sym("b", "-X2.2"), sym("c", "-X2.1")];
        p.pages = vec![pg];
        p.meta.strips = vec![layout("-X2", &["2", "1"], &[])];
        let hits = all(&p, "2");
        assert!(replace(&mut p, &hits, "2", "5", opts).is_ok());
        assert_eq!(p.meta.strips, [layout("-X5", &["5", "1"], &[])]);

        // Spare numbers follow the rename as tags do: S1 -> S7, and S11 is a longer number left
        // alone.
        let mut p = Project::new("Strip");
        let mut pg = Page::new("p1", "1", "Strip");
        pg.symbols = vec![sym("a", "-X1")];
        p.pages = vec![pg];
        p.meta.strips = vec![layout("-X1", &["S1", "S11"], &["S1", "S11"])];
        p.devices.insert(tag("-X1.S1"), part.clone());
        p.devices.insert(tag("-X1.S11"), part.clone());
        let hits = all(&p, "1");
        assert!(replace(&mut p, &hits, "1", "7", opts).is_ok());
        let l = &p.meta.strips[0];
        assert_eq!(l.strip, tag("-X7"));
        assert_eq!(
            (l.spares.as_slice(), l.order.as_slice()),
            (
                &["S7".to_owned(), "S11".to_owned()][..],
                &["S7".to_owned(), "S11".to_owned()][..]
            )
        );

        // A spare may not move onto a drawn tag, nor go through a placeholder whose spare tag
        // is in use.
        let mut p = Project::new("Strip");
        let mut pg = Page::new("p1", "1", "Strip");
        pg.symbols = vec![sym("a", "-X1"), sym("b", "-X2.S1")];
        p.pages = vec![pg];
        p.meta.strips = vec![layout("-X1", &[], &["S1"])];
        p.devices.insert(tag("-X1.S1"), part.clone());
        let hits: Vec<Hit> = all(&p, "X1").into_iter().filter(|h| h.value == "-X1").collect();
        let before = p.clone();
        assert_eq!(
            replace(&mut p, &hits, "X1", "X2", opts),
            Err("-X2.S1 is already used.".into())
        );
        assert_eq!(p, before);
        p.pages[0].symbols.pop();
        // A tag only a cable core names is taken too.
        p.pages[0].wires.push(Wire {
            id: "w1".into(),
            from: Endpoint::Point {
                at: Point::new(0.0, 0.0),
            },
            to: Endpoint::Point {
                at: Point::new(5.0, 0.0),
            },
            via: Vec::new(),
            number: None,
            potential: None,
            cable: Some(crate::project::CableCore {
                cable: tag("-X2.S1"),
                core: "1".into(),
            }),
            route: Default::default(),
            spec: Default::default(),
        });
        assert_eq!(
            replace(&mut p, &hits, "X1", "X2", opts),
            Err("-X2.S1 is already used.".into())
        );
        p.pages[0].wires[0].cable = Some(crate::project::CableCore {
            cable: tag("-WCFIND0.S1"),
            core: "1".into(),
        });
        p.devices.insert(tag("-WCFIND0.S1"), Device::default());
        assert!(replace(&mut p, &hits, "X1", "X2", opts).is_ok());
        assert_eq!(p.devices.get(&tag("-X2.S1")), Some(&part));
        assert_eq!(p.devices.get(&tag("-WCFIND0.S1")), Some(&Device::default()));
        assert_eq!(
            p.pages[0].wires[0].cable.as_ref().map(|c| c.cable.clone()),
            Some(tag("-WCFIND0.S1"))
        );
    }

    #[test]
    fn a_strip_and_its_spares_renamed_together() {
        let mut p = sample();
        let part = |p: &Project, t: &str| p.devices.get(&tag(t)).and_then(|d| d.part.clone());
        let with_part = |part: &str| Device {
            part: Some(part.into()),
            ..Default::default()
        };
        p.meta.strips.push(crate::project::StripLayout {
            strip: tag("-X1"),
            order: vec!["S2".into()],
            spares: vec!["S2".into()],
        });
        p.devices.insert(tag("-X1"), Device::default());
        p.devices.insert(tag("-X1.S2"), with_part("A"));
        p.devices.insert(tag("-X2.S1"), with_part("B"));
        let opts = FindOptions { match_case: true };
        let pick = |p: &Project, query: &str, tags: &[&str]| -> Vec<Hit> {
            find(p, query, opts)
                .into_iter()
                .filter(|h| h.kind == FindKind::Tag && tags.contains(&h.value.as_str()))
                .collect()
        };
        // -X1 takes its spare to -X2.S2, where -X2.S1 is renamed too.
        let before = p.clone();
        let hits = pick(&p, "1", &["-X1", "-X2.S1"]);
        assert_eq!(
            replace(&mut p, &hits, "1", "2", opts),
            Err("-X2.S2 is already used.".into())
        );
        assert_eq!(p, before);

        // A spare renamed with its strip goes where its own rename says, not where the strip
        // would take it.
        let mut p = sample();
        p.meta.strips.push(crate::project::StripLayout {
            strip: tag("-X1"),
            order: vec!["S1".into()],
            spares: vec!["S1".into()],
        });
        p.devices.insert(tag("-X1"), Device::default());
        p.devices.insert(tag("-X1.S1"), with_part("A"));
        let hits = pick(&p, "1", &["-X1", "-X1.S1"]);
        let (_, renamed) = replace_renaming(&mut p, &hits, "1", "5", opts).unwrap_or_default();
        assert_eq!(renamed.get(&tag("-X1.S1")), Some(&tag("-X5.S5")));
        assert_eq!(part(&p, "-X5.S5"), Some("A".into()));
        assert!(!p.devices.contains_key(&tag("-X5.S1")));
        let layout = &p.meta.strips[0];
        assert_eq!(
            (layout.spares.clone(), layout.order.clone()),
            (vec!["S5".into()], vec!["S5".into()])
        );
        assert_eq!(
            layout.arrange(&[]),
            [("S5".to_owned(), true)],
            "the part's spare is on the rail"
        );
        assert!(p.devices.keys().all(|t| !t.to_string().contains("WCFIND")));

        // A strip renamed to its own unassigned spare returns, and its layout stays.
        p.rename_device(&tag("-X5"), &tag("-X5.S1"));
        assert_eq!(p.meta.strips.len(), 1);
        assert_eq!(p.meta.strips[0].strip, tag("-X5"));
    }

    #[test]
    fn a_placeholder_tag_in_use_is_not_overwritten() {
        let mut p = sample();
        p.devices.insert(
            tag("-WCFIND0"),
            Device {
                description: "Keep me".into(),
                ..Default::default()
            },
        );
        let opts = FindOptions { match_case: true };
        let hits: Vec<Hit> = find(&p, "F1", opts)
            .into_iter()
            .filter(|h| h.kind == FindKind::Tag)
            .collect();
        assert!(replace(&mut p, &hits, "F1", "F2", opts).is_ok());
        assert_eq!(
            p.devices.get(&tag("-WCFIND0")).map(|d| d.description.as_str()),
            Some("Keep me")
        );
        assert!(p.tag_index().contains_key(&tag("-F2")));
    }

    #[test]
    fn a_hit_that_changed_since_the_search_changes_nothing() {
        let mut p = sample();
        let opts = FindOptions::default();
        let hits = find(&p, "pump", opts);
        // Another edit puts a different text where the found one was.
        if let Some(Shape::Text { text, .. }) = p.pages[0].graphics.get_mut(0) {
            *text = "Fan pump".into();
        }
        let before = p.clone();
        assert!(replace(&mut p, &hits, "pump", "fan", opts).is_err());
        assert_eq!(p, before);
    }

    #[test]
    fn a_deleted_target_changes_nothing() {
        let mut p = sample();
        let opts = FindOptions::default();
        // A text that is still there, then a wire that was deleted after the search.
        let hits = find(&p, "1", opts);
        assert!(hits.iter().any(|h| h.kind == FindKind::Text));
        p.pages[1].wires.clear();
        let before = p.clone();
        assert!(replace(&mut p, &hits, "1", "2", opts).is_err());
        assert_eq!(p, before);
        let hits = find(&p, "Spare", opts);
        p.devices.remove(&tag("-X9"));
        let before = p.clone();
        assert!(replace(&mut p, &hits, "Spare", "Used", opts).is_err());
        assert_eq!(p, before);
    }

    #[test]
    fn case_insensitive_replace_keeps_the_rest_as_written() {
        let opts = FindOptions::default();
        assert_eq!(replace_text("Pump PUMP pump", "pump", "Fan", opts), "Fan Fan Fan");
        assert_eq!(replace_text("aaa", "aa", "b", opts), "ba");
        assert_eq!(replace_text("Ölpumpe", "öl", "Wasser", opts), "Wasserpumpe");
        assert_eq!(
            replace_text("Pump", "pump", "Fan", FindOptions { match_case: true }),
            "Pump"
        );
    }

    /// A hit the replace will not change is told apart before the replace, with the reason, and
    /// what [`left_alone`] says agrees with what [`replace`] then does.
    #[test]
    fn a_hit_the_replace_leaves_alone_is_told_with_the_reason() -> Result<(), String> {
        let p = sample();
        let opts = FindOptions::default();
        let hits = find(&p, "-K1", opts);
        let told: Vec<(&str, Option<LeftAlone>)> = hits
            .iter()
            .filter(|h| h.kind == FindKind::Tag)
            .map(|h| (h.value.as_str(), left_alone(h, "-K1", "-K2", opts)))
            .collect();
        assert_eq!(
            told,
            [("-K1", None), ("-K12", Some(LeftAlone::AnotherTag)), ("-K1", None)]
        );
        // A letter part is whole in `-K12`, so `K` to `Q` changes it.
        let hits = find(&p, "K", opts);
        assert!(
            hits.iter()
                .filter(|h| h.kind == FindKind::Tag)
                .all(|h| left_alone(h, "K", "Q", opts).is_none())
        );
        // The replacement is what stands there already: nothing changes, in a tag or a text.
        let hits = find(&p, "k1", opts);
        assert!(hits.len() > 3, "{hits:?}");
        for h in &hits {
            let expect = if h.value == "-K12" {
                LeftAlone::AnotherTag
            } else {
                LeftAlone::SameValue
            };
            assert_eq!(left_alone(h, "k1", "K1", opts), Some(expect), "{h:?}");
        }
        // It agrees with the replace: only the hits not left alone change.
        let hits = find(&p, "-K1", opts);
        let alone: Vec<Hit> = hits
            .iter()
            .filter(|h| left_alone(h, "-K1", "-K2", opts).is_some())
            .cloned()
            .collect();
        assert_eq!(alone.len(), 1);
        let mut q = p.clone();
        assert_eq!(replace(&mut q, &alone, "-K1", "-K2", opts)?, 0);
        assert_eq!(q, p);
        // One occurrence in the tag changes and another does not: the hit counts as changed.
        let mixed = Hit {
            kind: FindKind::Tag,
            target: hits[0].target.clone(),
            value: "=A1+C1-K11".into(),
        };
        assert_eq!(replace_in_tag(&mixed.value, "1", "2", opts), "=A2+C2-K11");
        assert_eq!(left_alone(&mixed, "1", "2", opts), None);
        // A change that would be refused is not "left alone".
        let hits = find(&p, "-K1", opts);
        assert_eq!(left_alone(&hits[0], "-K1", "?", opts), None);
        assert_eq!(LeftAlone::AnotherTag.reason(), "another tag");
        Ok(())
    }
}

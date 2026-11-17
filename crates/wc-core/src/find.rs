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
    !positions(hay, needle, opts).is_empty()
}

/// The char positions in `hay` where `needle` starts, without overlaps, left to right.
fn positions(hay: &str, needle: &str, opts: FindOptions) -> Vec<usize> {
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
        if here {
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
    let at = positions(hay, needle, opts);
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

/// Replace `query` with `with` in the chosen `hits` (from [`find`] with the same query and
/// options). All or nothing: when a renamed tag is not a valid tag or is already used, nothing
/// changes and the reason comes back. Returns how many values changed.
///
/// A device tag hit renames that device everywhere it appears, once per device, even when
/// several of its symbols are chosen.
pub fn replace(
    project: &mut Project,
    hits: &[Hit],
    query: &str,
    with: &str,
    opts: FindOptions,
) -> Result<usize, String> {
    if query.is_empty() {
        return Ok(0);
    }
    let mut p = project.clone();
    let mut changed = 0usize;
    let mut renames: BTreeMap<DeviceTag, DeviceTag> = BTreeMap::new();
    for hit in hits {
        match (&hit.kind, &hit.target) {
            (FindKind::Text, FindTarget::Graphic { page, index }) => {
                if let Some(Shape::Text { text, .. }) = p
                    .pages
                    .iter_mut()
                    .find(|pg| &pg.id == page)
                    .and_then(|pg| pg.graphics.get_mut(*index))
                {
                    let new = replace_text(text, query, with, opts);
                    if new != *text {
                        *text = new;
                        changed += 1;
                    }
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
                    if let Some(v) = field {
                        let new = replace_text(v, query, with, opts);
                        if new != *v {
                            // An emptied number or potential is no number or potential.
                            *field = (!new.trim().is_empty()).then_some(new);
                            changed += 1;
                        }
                    }
                }
            }
            (FindKind::Description, FindTarget::Device { tag, .. }) => {
                if let Some(d) = p.devices.get_mut(tag) {
                    let new = replace_text(&d.description, query, with, opts);
                    if new != d.description {
                        d.description = new;
                        changed += 1;
                    }
                }
            }
            (FindKind::Tag, FindTarget::Symbol { page, id }) => {
                let tag = p.page(page).and_then(|pg| pg.symbol(id)).and_then(|s| s.tag.clone());
                if let Some(from) = tag {
                    let text = replace_text(&from.to_string(), query, with, opts);
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
    let taken: Vec<DeviceTag> = p
        .devices
        .keys()
        .cloned()
        .chain(p.tag_index().into_keys())
        .filter(|t| !renames.contains_key(t))
        .collect();
    if let Some(to) = renames.values().find(|to| taken.contains(to)) {
        return Err(format!("{to} is already used."));
    }
    // Renamed in two steps through placeholders, so `-K1` -> `-K2` and `-K2` -> `-K3` in one
    // replace do not run into each other.
    let mut placed: Vec<(DeviceTag, DeviceTag)> = Vec::new();
    for (i, (from, to)) in renames.iter().enumerate() {
        let temp = DeviceTag::parse(&format!("-WCFIND{i}")).map_err(|e| e.to_string())?;
        p.rename_device(from, &temp);
        placed.push((temp, to.clone()));
    }
    for (temp, to) in &placed {
        changed += p.rename_device(temp, to);
    }
    *project = p;
    Ok(changed)
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
        let opts = FindOptions::default();
        // -K1 -> -K12 would clash with the existing -K12, unless -K12 moves too: "K1" -> "K12"
        // makes -K12 into -K122, so both move and nothing clashes.
        let hits: Vec<Hit> = find(&p, "K1", opts)
            .into_iter()
            .filter(|h| h.kind == FindKind::Tag)
            .collect();
        assert_eq!(replace(&mut p, &hits, "K1", "K12", opts), Ok(3));
        assert!(p.devices.contains_key(&tag("-K12")));
        let tags: Vec<String> = p.pages[0]
            .symbols
            .iter()
            .filter_map(|s| s.tag.as_ref().map(ToString::to_string))
            .collect();
        assert_eq!(tags, ["-K12", "-K122", "-F1"]);
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
}

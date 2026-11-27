//! Placing a group of symbols, wires and drawings on a page, as a paste or a macro does: each
//! symbol gets a fresh id, each device a fresh tag like its old one (one new device per device
//! of the group), wires follow their symbols and everything moves so the group's anchor lands
//! on the point given.
//!
//! The callers differ in two choices, made explicit here: whose device data a new device
//! takes ([`DeviceData`]), and what happens when no free tag is left ([`OutOfTags`]).

use std::collections::{BTreeMap, BTreeSet};

use crate::geom::{Point, Shape};
use crate::project::{Device, Endpoint, PlacedSymbol, Project, Wire};
use crate::{DeviceTag, Error, Result};

/// What is placed: a clipboard's or a macro's items, as they were drawn.
#[derive(Debug, Clone, Copy)]
pub struct Group<'a> {
    pub symbols: &'a [PlacedSymbol],
    pub wires: &'a [Wire],
    pub graphics: &'a [Shape],
    /// The group's own data for its devices, by their tags in the group.
    pub devices: &'a BTreeMap<DeviceTag, Device>,
    /// The point that lands where the group is placed.
    pub anchor: Point,
}

/// Whose data a placed device takes, looked up by its tag in the group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceData {
    /// The project's device of that tag, else the group's: a copy within one project.
    ProjectFirst,
    /// The group's, else the project's: a copy from another project.
    GroupFirst,
    /// The group's only: a macro, whose tags name nothing in the project.
    GroupOnly,
}

/// What happens to a device when every tag like its old one is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutOfTags {
    /// Nothing is placed, and the error says which tag ran out.
    Fail,
    /// Its symbols are placed without a tag.
    Untagged,
}

/// What was placed: the new symbol and wire ids, and the indexes of the new drawings in the
/// page's graphics, each in the group's order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placed {
    pub symbols: Vec<String>,
    pub wires: Vec<String>,
    pub graphics: Vec<usize>,
}

/// The first free tag with the same function, location and letter code as `like`, numbered
/// 1 to 9999, that is not in `used` and does not hold a tag in `used`: strip `-X1` is taken
/// when only its terminal `-X1.3` is drawn, as `-K1` is by its part `-K1.2`.
pub fn free_tag_like(used: &BTreeSet<DeviceTag>, like: &DeviceTag) -> Option<DeviceTag> {
    let letter = like.letter_code();
    // Each used tag's parents: the same tag with fewer product levels.
    let parents: BTreeSet<DeviceTag> = used
        .iter()
        .flat_map(|t| {
            (1..t.product.len()).map(|n| DeviceTag {
                product: t.product[..n].to_vec(),
                ..t.clone()
            })
        })
        .collect();
    (1..10_000).find_map(|n| {
        let mut t = like.clone();
        *t.product.last_mut()? = format!("{letter}{n}");
        (!used.contains(&t) && !parents.contains(&t)).then_some(t)
    })
}

/// `old` under the new tag of its parent (its tag one product level up), when the group holds
/// that parent: `-X1.3` of a strip `-X1` placed as `-X2` is `-X2.3`, and placed as `-X1` it
/// stays `-X1.3`.
fn under_renamed_parent(renamed: &BTreeMap<&DeviceTag, Option<DeviceTag>>, old: &DeviceTag) -> Option<DeviceTag> {
    let (last, rest) = old.product.split_last()?;
    if rest.is_empty() {
        return None;
    }
    let parent = DeviceTag {
        product: rest.to_vec(),
        ..old.clone()
    };
    let new_parent = renamed.get(&parent)?.as_ref()?;
    let mut t = new_parent.clone();
    t.product.push(last.clone());
    Some(t)
}

/// The first product `<letter>1` to `<letter>9999` that no tag in `used` has, whatever its
/// function and location, nor has as the first level of a longer product: terminal `-X1.3` or
/// part `-K1.2` takes `X1` or `K1` too. `None` for an empty letter code or when all are taken.
pub fn free_product(used: &BTreeSet<DeviceTag>, letter: &str) -> Option<String> {
    if letter.is_empty() {
        return None;
    }
    let taken: BTreeSet<&str> = used
        .iter()
        .filter_map(|t| t.product.first())
        .map(String::as_str)
        .collect();
    (1..10_000)
        .map(|n| format!("{letter}{n}"))
        .find(|p| !taken.contains(p.as_str()))
}

/// Every tag the project uses: on its pages, in its device data, as the cable of a wire
/// core (a cable with cores but no symbol drawn and no device entry is still taken) and on a
/// fieldbus network (its nodes, link ends and link cables).
pub fn used_tags(project: &Project) -> BTreeSet<DeviceTag> {
    let mut used: BTreeSet<DeviceTag> = project.tag_index().into_keys().collect();
    used.extend(project.devices.keys().cloned());
    used.extend(
        project
            .pages
            .iter()
            .flat_map(|p| &p.wires)
            .filter_map(|w| w.cable.as_ref().map(|c| c.cable.clone())),
    );
    for n in &project.meta.networks {
        let nodes = n.nodes.iter().map(|x| x.device.parse::<DeviceTag>().ok());
        let cables = n.links.iter().map(|l| l.cable.parse::<DeviceTag>().ok());
        // Each end on its own, so a link with one end that cannot be read still takes the other.
        let ends = n.links.iter().flat_map(|l| [&l.from, &l.to]);
        let ends = ends.map(|e| crate::network::Port::parse(e).ok().map(|p| p.device));
        used.extend(nodes.chain(cables).chain(ends).flatten());
    }
    used
}

/// Place `group` on page `page` (an index) so its anchor lands on `at`. New tags and device
/// data are worked out before anything changes, so a failure leaves the project as it was.
pub fn place(
    project: &mut Project,
    page: usize,
    group: Group<'_>,
    at: Point,
    data: DeviceData,
    out_of_tags: OutOfTags,
) -> Result<Placed> {
    let no_page = || Error::Invalid(format!("no page {}", page.saturating_add(1)));
    if project.pages.get(page).is_none() {
        return Err(no_page());
    }
    // The new tag of each old one (`None`: placed without), and the devices to add.
    let mut used = used_tags(project);
    let mut renamed: BTreeMap<&DeviceTag, Option<DeviceTag>> = BTreeMap::new();
    let mut devices: Vec<(DeviceTag, Device)> = Vec::new();
    // The cable of a wire core is a device of the group too, unless the group is a copy within
    // this project: there the tag names the same real cable, while a macro's or another
    // project's `-W1` would join an unrelated cable of the project.
    let cables = group
        .wires
        .iter()
        .filter(|_| data != DeviceData::ProjectFirst)
        .filter_map(|w| w.cable.as_ref().map(|c| &c.cable));
    // Shorter tags first, so a strip (`-X1`) has its new tag before its terminals (`-X1.3`).
    let mut olds: Vec<&DeviceTag> = group
        .symbols
        .iter()
        .filter_map(|s| s.tag.as_ref())
        .chain(cables)
        .collect();
    olds.sort_by_key(|t| t.product.len());
    // A device the group holds only as a part or terminal (`-K2.1` without `-K2`) is taken too,
    // so another device of the group does not land on it and absorb that part. A parent under a
    // device the group holds itself (`-K1.1` of `-K1` and `-K1.1.1`) is not: it moves with `-K1`.
    let own: BTreeSet<&DeviceTag> = olds.iter().copied().collect();
    let prefix = |t: &DeviceTag, n: usize| {
        t.product.get(..n).map(|product| DeviceTag {
            product: product.to_vec(),
            ..t.clone()
        })
    };
    let held: Vec<DeviceTag> = olds
        .iter()
        .flat_map(|t| (1..t.product.len()).filter_map(move |n| prefix(t, n)))
        .filter(|parent| {
            !(1..=parent.product.len())
                .filter_map(|n| prefix(parent, n))
                .any(|p| own.contains(&p))
        })
        .collect();
    used.extend(held);
    // Two passes: first every tag that keeps its own number (on its own or on its strip's new
    // tag), then the ones whose number is taken there, so a taken number never pushes another
    // terminal off a number that was free (#1780).
    let mut pending = olds;
    for first in [true, false] {
        let mut deferred: Vec<&DeviceTag> = Vec::new();
        for old in std::mem::take(&mut pending) {
            if renamed.contains_key(old) {
                continue;
            }
            // A terminal (or other sub-tag) of a strip the group holds follows the strip to its
            // new tag, keeping its own number when that is free there, else taking the next free
            // number on that strip.
            // A part of a device the group holds but placed untagged (out of tags) is untagged
            // too: on its own it would join the family of the project's device of that name.
            let parent_untagged = old.product.split_last().is_some_and(|(_, rest)| {
                !rest.is_empty()
                    && renamed
                        .get(&DeviceTag {
                            product: rest.to_vec(),
                            ..old.clone()
                        })
                        .is_some_and(Option::is_none)
            });
            if parent_untagged {
                renamed.insert(old, None);
                continue;
            }
            let followed = under_renamed_parent(&renamed, old);
            if first {
                let parent_deferred = old.product.split_last().is_some_and(|(_, rest)| {
                    deferred.iter().any(|d| {
                        d.product.as_slice() == rest && d.function == old.function && d.location == old.location
                    })
                });
                if parent_deferred || followed.as_ref().is_some_and(|t| used.contains(t)) {
                    deferred.push(old);
                    continue;
                }
            }
            let new = match followed
                .clone()
                .filter(|t| !used.contains(t))
                .or_else(|| free_tag_like(&used, followed.as_ref().unwrap_or(old)))
            {
                Some(t) => t,
                None if out_of_tags == OutOfTags::Fail => {
                    return Err(Error::Invalid(format!("no free tag like {old}")));
                }
                None => {
                    renamed.insert(old, None);
                    continue;
                }
            };
            let (project_data, group_data) = (project.devices.get(old), group.devices.get(old));
            let device = match data {
                DeviceData::ProjectFirst => project_data.or(group_data),
                DeviceData::GroupFirst => group_data.or(project_data),
                DeviceData::GroupOnly => group_data,
            }
            .cloned()
            .unwrap_or_default();
            used.insert(new.clone());
            devices.push((new.clone(), device));
            renamed.insert(old, Some(new));
        }
        pending = deferred;
    }
    project.devices.extend(devices);

    let d = Point::new(at.x - group.anchor.x, at.y - group.anchor.y);
    let p = project.pages.get_mut(page).ok_or_else(no_page)?;
    let mut placed = Placed::default();
    let mut ids: BTreeMap<&str, String> = BTreeMap::new();
    for s in group.symbols {
        let mut new = s.clone();
        new.tag = s.tag.as_ref().and_then(|t| renamed.get(t).cloned().flatten());
        new.id = p.next_id("s");
        new.at = s.at + d;
        ids.insert(s.id.as_str(), new.id.clone());
        placed.symbols.push(new.id.clone());
        p.symbols.push(new);
    }
    for w in group.wires {
        let mut w = w.clone();
        // A core of a cable the group holds (drawn in it, or named by its cores outside a copy
        // within the project) runs in that cable under its new tag; when the cable is placed
        // without a tag, the core runs in no cable (not in the project's cable of the old tag).
        if let Some(c) = &w.cable
            && let Some(new) = renamed.get(&c.cable)
        {
            w.cable = new.clone().map(|t| crate::project::CableCore {
                cable: t,
                core: c.core.clone(),
            });
        }
        w.id = p.next_id("w");
        w.number = None;
        for v in &mut w.via {
            *v = *v + d;
        }
        for e in [&mut w.from, &mut w.to] {
            match e {
                Endpoint::Point { at } => *at = *at + d,
                Endpoint::Pin { item, .. } => {
                    if let Some(n) = ids.get(item.as_str()) {
                        *item = n.clone();
                    }
                }
            }
        }
        placed.wires.push(w.id.clone());
        p.wires.push(w);
    }
    for g in group.graphics {
        p.graphics.push(g.translate(d));
        placed.graphics.push(p.graphics.len() - 1);
    }
    Ok(placed)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::project::Page;

    fn tag(s: &str) -> DeviceTag {
        DeviceTag::parse(s).unwrap()
    }

    fn symbol(id: &str, t: &str) -> PlacedSymbol {
        PlacedSymbol {
            id: id.into(),
            symbol: "wc.coil".into(),
            at: Point::new(10.0, 10.0),
            rotation: Default::default(),
            mirror: false,
            tag: Some(tag(t)),
        }
    }

    fn device(description: &str) -> Device {
        Device {
            description: description.into(),
            ..Device::default()
        }
    }

    /// A project whose page 1 holds -K1, with its own data for -K1.
    fn project() -> Project {
        let mut p = Project::new("t");
        let mut page = Page::new("p1", "1", "One");
        page.symbols.push(symbol("s1", "-K1"));
        p.pages.push(page);
        p.devices.insert(tag("-K1"), device("project's"));
        p
    }

    #[test]
    fn a_free_tag_is_not_the_strip_or_device_of_a_drawn_terminal_or_part() {
        let mut p = project();
        // Strip -X1 is drawn only as its terminal -X1.3, -K2 only as its part -K2.1.
        p.pages[0].symbols.push(symbol("s2", "-X1.3"));
        p.pages[0].symbols.push(symbol("s3", "-K2.1"));
        let used = used_tags(&p);
        assert_eq!(free_tag_like(&used, &tag("-X7")), Some(tag("-X2")));
        assert_eq!(free_tag_like(&used, &tag("-K9")), Some(tag("-K3")));
        // Another function's -X1 does not take this one's; a terminal still gets a free number.
        assert_eq!(free_tag_like(&used, &tag("=A-X7")), Some(tag("=A-X1")));
        assert_eq!(free_tag_like(&used, &tag("-X1.3")), Some(tag("-X1.1")));
    }

    #[test]
    fn a_pasted_part_follows_its_device_when_only_the_part_is_drawn() {
        // The project draws -K1 (page 1) and its part -K1.1.
        let mut p = project();
        p.pages[0].symbols.push(symbol("s2", "-K1.1"));
        let symbols = [symbol("a", "-K1"), symbol("b", "-K1.1")];
        let none = BTreeMap::new();
        let group = Group {
            symbols: &symbols,
            wires: &[],
            graphics: &[],
            devices: &none,
            anchor: Point::new(0.0, 0.0),
        };
        let Ok(placed) = place(
            &mut p,
            0,
            group,
            Point::new(0.0, 0.0),
            DeviceData::ProjectFirst,
            OutOfTags::Fail,
        ) else {
            unreachable!("placed")
        };
        let Some(page) = p.pages.first() else {
            unreachable!("page 1")
        };
        let new_tags: Vec<Option<String>> = placed
            .symbols
            .iter()
            .map(|id| page.symbol(id).and_then(|s| s.tag.as_ref()).map(ToString::to_string))
            .collect();
        assert_eq!(new_tags, [Some("-K2".to_string()), Some("-K2.1".to_string())]);
    }

    #[test]
    fn a_pasted_device_does_not_land_on_a_device_the_group_holds_only_as_a_part() {
        // The project draws only -K1.1; the group holds device -K7 and the part -K2.1 alone.
        let mut p = Project::new("t");
        let mut page = Page::new("p1", "1", "One");
        page.symbols.push(symbol("s1", "-K1.1"));
        p.pages.push(page);
        let symbols = [symbol("a", "-K7"), symbol("b", "-K2.1")];
        let none = BTreeMap::new();
        let group = Group {
            symbols: &symbols,
            wires: &[],
            graphics: &[],
            devices: &none,
            anchor: Point::new(0.0, 0.0),
        };
        let Ok(placed) = place(
            &mut p,
            0,
            group,
            Point::new(0.0, 0.0),
            DeviceData::ProjectFirst,
            OutOfTags::Fail,
        ) else {
            unreachable!("placed")
        };
        let Some(page) = p.pages.first() else {
            unreachable!("page 1")
        };
        let new_tags: Vec<Option<String>> = placed
            .symbols
            .iter()
            .map(|id| page.symbol(id).and_then(|s| s.tag.as_ref()).map(ToString::to_string))
            .collect();
        // -K1 is held by -K1.1 and -K2 by the pasted -K2.1, so -K7 becomes -K3.
        assert_eq!(new_tags, [Some("-K3".to_string()), Some("-K2.1".to_string())]);
    }

    #[test]
    fn a_pasted_device_keeps_its_tag_when_the_group_holds_its_nested_part() {
        // An empty page; the group holds -K1 and -K1.1.1 but not -K1.1.
        let mut p = Project::new("t");
        p.pages.push(Page::new("p1", "1", "One"));
        let symbols = [symbol("a", "-K1"), symbol("b", "-K1.1.1")];
        let none = BTreeMap::new();
        let group = Group {
            symbols: &symbols,
            wires: &[],
            graphics: &[],
            devices: &none,
            anchor: Point::new(0.0, 0.0),
        };
        let Ok(placed) = place(
            &mut p,
            0,
            group,
            Point::new(0.0, 0.0),
            DeviceData::ProjectFirst,
            OutOfTags::Fail,
        ) else {
            unreachable!("placed")
        };
        let Some(page) = p.pages.first() else {
            unreachable!("page 1")
        };
        let new_tags: Vec<Option<String>> = placed
            .symbols
            .iter()
            .map(|id| page.symbol(id).and_then(|s| s.tag.as_ref()).map(ToString::to_string))
            .collect();
        assert_eq!(new_tags, [Some("-K1".to_string()), Some("-K1.1.1".to_string())]);
    }

    #[test]
    fn a_free_product_skips_device_data_terminals_and_other_functions() {
        let mut p = project();
        // -K2 has device data but no symbol (its symbol was deleted); -K3 is in another
        // function; -X1 is drawn only as its terminals.
        p.devices.insert(tag("-K2"), device("left behind"));
        p.pages[0].symbols.push(symbol("s2", "=PUMP-K3"));
        p.pages[0].symbols.push(symbol("s3", "-X1.3"));
        let used = used_tags(&p);
        assert_eq!(free_product(&used, "K").as_deref(), Some("K4"));
        assert_eq!(free_product(&used, "X").as_deref(), Some("X2"));
        assert_eq!(free_product(&used, "Q").as_deref(), Some("Q1"));
        assert_eq!(free_product(&used, ""), None);
    }

    #[test]
    fn device_data_comes_from_the_side_asked_for() {
        let group_devices = BTreeMap::from([(tag("-K1"), device("group's"))]);
        let symbols = [symbol("a", "-K1"), symbol("b", "-K5")];
        let group = Group {
            symbols: &symbols,
            wires: &[],
            graphics: &[],
            devices: &group_devices,
            anchor: Point::new(0.0, 0.0),
        };
        let description = |data| {
            let mut p = project();
            let placed = place(&mut p, 0, group, Point::new(0.0, 0.0), data, OutOfTags::Fail).unwrap();
            assert_eq!(placed.symbols.len(), 2);
            let k = |t: &str| p.devices.get(&tag(t)).map(|d| d.description.clone());
            // -K1 becomes -K2; -K5 becomes -K3, with no data on either side for -K5.
            (k("-K2"), k("-K3"))
        };
        let (g, p) = (Some("group's".to_string()), Some("project's".to_string()));
        assert_eq!(description(DeviceData::ProjectFirst), (p.clone(), Some(String::new())));
        assert_eq!(description(DeviceData::GroupFirst), (g.clone(), Some(String::new())));
        assert_eq!(description(DeviceData::GroupOnly), (g, Some(String::new())));

        // Without the group's data for -K1, the project's is taken unless only the group's counts.
        let none = BTreeMap::new();
        let group = Group {
            devices: &none,
            ..group
        };
        for (data, want) in [
            (DeviceData::ProjectFirst, p.clone()),
            (DeviceData::GroupFirst, p),
            (DeviceData::GroupOnly, Some(String::new())),
        ] {
            let mut pr = project();
            place(&mut pr, 0, group, Point::new(0.0, 0.0), data, OutOfTags::Fail).unwrap();
            assert_eq!(
                pr.devices.get(&tag("-K2")).map(|d| d.description.clone()),
                want,
                "{data:?}"
            );
        }
    }

    #[test]
    fn a_tag_only_a_network_names_is_taken() {
        let mut p = project();
        p.meta.networks.push(crate::network::Network {
            name: "PN1".into(),
            nodes: vec![crate::network::Node {
                device: "-K2".into(),
                ..Default::default()
            }],
            links: vec![
                crate::network::Link {
                    from: "-K3:X1P1".into(),
                    to: "-K4:X1P1".into(),
                    cable: "-W1".into(),
                    ..Default::default()
                },
                // The other end cannot be read; -K5 is still taken.
                crate::network::Link {
                    from: "-K5:X1P2".into(),
                    to: String::new(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let used = used_tags(&p);
        for t in ["-K2", "-K3", "-K4", "-K5", "-W1"] {
            assert!(used.contains(&tag(t)), "{t}");
        }
        assert_eq!(free_tag_like(&used, &tag("-W1")), Some(tag("-W2")));
        assert_eq!(free_tag_like(&used, &tag("-K1")), Some(tag("-K6")));
    }

    #[test]
    fn a_core_whose_cable_is_not_drawn_in_the_group_gets_a_new_cable_unless_copied_within() {
        // Core BN of -W1, with the group's data for -W1 but no symbol tagged -W1.
        let core = Wire {
            id: "w1".into(),
            from: Endpoint::Point {
                at: Point::new(0.0, 0.0),
            },
            to: Endpoint::Point {
                at: Point::new(5.0, 0.0),
            },
            via: vec![],
            number: None,
            potential: None,
            cable: Some(crate::project::CableCore {
                cable: tag("-W1"),
                core: "BN".into(),
            }),
            route: Default::default(),
            spec: Default::default(),
        };
        let wires = [core];
        let group_devices = BTreeMap::from([(tag("-W1"), device("group's cable"))]);
        let group = Group {
            symbols: &[],
            wires: &wires,
            graphics: &[],
            devices: &group_devices,
            anchor: Point::new(0.0, 0.0),
        };
        let cable_of = |p: &Project, placed: &Placed| {
            let w = p.pages[0].wires.iter().find(|w| w.id == placed.wires[0]).unwrap();
            w.cable.as_ref().map(|c| (c.cable.to_string(), c.core.clone()))
        };
        // The project already has a cable -W1 of its own.
        let mut p = project();
        p.devices.insert(tag("-W1"), device("project's cable"));
        for data in [DeviceData::GroupOnly, DeviceData::GroupFirst] {
            let mut p = p.clone();
            let first = place(&mut p, 0, group, Point::new(0.0, 0.0), data, OutOfTags::Fail).unwrap();
            let second = place(&mut p, 0, group, Point::new(0.0, 0.0), data, OutOfTags::Fail).unwrap();
            assert_eq!(cable_of(&p, &first), Some(("-W2".into(), "BN".into())), "{data:?}");
            assert_eq!(cable_of(&p, &second), Some(("-W3".into(), "BN".into())), "{data:?}");
            assert_eq!(p.devices[&tag("-W2")].description, "group's cable");
            assert_eq!(p.devices[&tag("-W1")].description, "project's cable");
        }
        // A copy within the project keeps the core in the project's cable.
        let placed = place(
            &mut p,
            0,
            group,
            Point::new(0.0, 0.0),
            DeviceData::ProjectFirst,
            OutOfTags::Fail,
        )
        .unwrap();
        assert_eq!(cable_of(&p, &placed), Some(("-W1".into(), "BN".into())));
        assert!(!p.devices.contains_key(&tag("-W2")));
    }

    #[test]
    fn a_part_of_a_device_placed_untagged_is_placed_untagged_too() {
        let mut p = project();
        // Every -K1 to -K9999 is taken, -K1 by its part -K1.1 only.
        p.devices.insert(tag("-K1.1"), Device::default());
        for n in 2..10_000 {
            p.devices.insert(tag(&format!("-K{n}")), Device::default());
        }
        let symbols = [symbol("a", "-K1"), symbol("b", "-K1.2")];
        let none = BTreeMap::new();
        let group = Group {
            symbols: &symbols,
            wires: &[],
            graphics: &[],
            devices: &none,
            anchor: Point::new(0.0, 0.0),
        };
        let Ok(placed) = place(
            &mut p,
            0,
            group,
            Point::new(5.0, 0.0),
            DeviceData::ProjectFirst,
            OutOfTags::Untagged,
        ) else {
            unreachable!("placed")
        };
        let Some(page) = p.pages.first() else {
            unreachable!("page 1")
        };
        let tags: Vec<Option<String>> = placed
            .symbols
            .iter()
            .map(|id| page.symbol(id).and_then(|s| s.tag.as_ref()).map(ToString::to_string))
            .collect();
        // -K1.2 does not join the project's -K1.
        assert_eq!(tags, [None, None]);
    }

    #[test]
    fn when_tags_run_out_it_fails_untouched_or_places_untagged() {
        let mut p = project();
        // Every -K1 to -K9999 is taken.
        for n in 2..10_000 {
            p.devices.insert(tag(&format!("-K{n}")), Device::default());
        }
        let symbols = [symbol("a", "-K1"), symbol("b", "-X1")];
        let none = BTreeMap::new();
        let group = Group {
            symbols: &symbols,
            wires: &[],
            graphics: &[],
            devices: &none,
            anchor: Point::new(0.0, 0.0),
        };
        let before = p.clone();
        let err = place(
            &mut p,
            0,
            group,
            Point::new(5.0, 0.0),
            DeviceData::ProjectFirst,
            OutOfTags::Fail,
        );
        assert!(err.unwrap_err().to_string().contains("no free tag like -K1"));
        assert_eq!(p, before, "nothing changed");

        let placed = place(
            &mut p,
            0,
            group,
            Point::new(5.0, 0.0),
            DeviceData::ProjectFirst,
            OutOfTags::Untagged,
        )
        .unwrap();
        let page = &p.pages[0];
        let tags: Vec<Option<String>> = placed
            .symbols
            .iter()
            .map(|id| page.symbol(id).unwrap().tag.as_ref().map(ToString::to_string))
            .collect();
        assert_eq!(tags, [None, Some("-X1".to_string())]);

        // A core of a cable placed without a tag runs in no cable, not in the project's -W1.
        for n in 1..10_000 {
            p.devices.insert(tag(&format!("-W{n}")), Device::default());
        }
        let cable = [symbol("c", "-W1")];
        let core = Wire {
            id: "w1".into(),
            from: Endpoint::Point {
                at: Point::new(0.0, 0.0),
            },
            to: Endpoint::Point {
                at: Point::new(5.0, 0.0),
            },
            via: vec![],
            number: None,
            potential: None,
            cable: Some(crate::project::CableCore {
                cable: tag("-W1"),
                core: "BN".into(),
            }),
            route: Default::default(),
            spec: Default::default(),
        };
        let wires = [core];
        let group = Group {
            symbols: &cable,
            wires: &wires,
            ..group
        };
        let placed = place(
            &mut p,
            0,
            group,
            Point::new(0.0, 0.0),
            DeviceData::ProjectFirst,
            OutOfTags::Untagged,
        )
        .unwrap();
        let w = p.pages[0].wires.iter().find(|w| w.id == placed.wires[0]).unwrap();
        assert_eq!(w.cable, None);

        // A page index past the end is an error, also the largest one.
        let none = place(
            &mut p,
            usize::MAX,
            group,
            Point::new(0.0, 0.0),
            DeviceData::ProjectFirst,
            OutOfTags::Fail,
        );
        assert!(none.is_err());
    }

    fn tags(list: &[&str]) -> BTreeSet<DeviceTag> {
        list.iter().map(|t| tag(t)).collect()
    }

    fn point_wire(id: &str, from: Endpoint, to: Endpoint) -> Wire {
        Wire {
            id: id.into(),
            from,
            to,
            via: vec![],
            number: None,
            potential: None,
            cable: None,
            route: Default::default(),
            spec: Default::default(),
        }
    }

    #[test]
    fn free_tag_like_keeps_function_and_location_and_takes_the_lowest_gap() {
        let used = tags(&["=A1+L1-KF1", "=A1+L1-KF2", "=A1+L1-KF4", "-KF3"]);
        let Some(t) = free_tag_like(&used, &tag("=A1+L1-KF2")) else {
            unreachable!("a free tag")
        };
        // -KF3 is taken only without function and location, so it is free here.
        assert_eq!(t.to_string(), "=A1+L1-KF3");
        // The old number does not matter, only the letter code.
        let Some(t) = free_tag_like(&used, &tag("=A1+L1-KF99")) else {
            unreachable!("a free tag")
        };
        assert_eq!(t.to_string(), "=A1+L1-KF3");
        let Some(t) = free_tag_like(&BTreeSet::new(), &tag("-X7")) else {
            unreachable!("a free tag")
        };
        assert_eq!(t.to_string(), "-X1");
    }

    #[test]
    fn free_tag_like_without_a_product_or_with_every_number_taken_is_none() {
        // Parsing refuses such a tag, but the field is public.
        let no_product = DeviceTag {
            function: vec!["A1".into()],
            ..DeviceTag::default()
        };
        assert_eq!(free_tag_like(&BTreeSet::new(), &no_product), None);
        let used: BTreeSet<DeviceTag> = (1..10_000).map(|n| tag(&format!("-Q{n}"))).collect();
        assert_eq!(free_tag_like(&used, &tag("-Q1")), None);
        // Only 1 to 9999 count: -Q10000 is never offered.
        let Some(t) = free_tag_like(&used, &tag("-QA1")) else {
            unreachable!("another letter code is free")
        };
        assert_eq!(t.to_string(), "-QA1");
    }

    #[test]
    fn used_tags_joins_page_tags_and_device_data() {
        let mut p = project();
        p.devices.insert(tag("-F3"), Device::default());
        let Some(page) = p.pages.first_mut() else {
            unreachable!("page 1")
        };
        page.symbols.push(symbol("s2", "-X2"));
        page.symbols.push(PlacedSymbol {
            tag: None,
            ..symbol("s3", "-X9")
        });
        assert_eq!(used_tags(&p), tags(&["-K1", "-F3", "-X2"]));
    }

    #[test]
    fn a_missing_page_is_an_error_naming_it_one_based_and_changes_nothing() {
        let mut p = project();
        let before = p.clone();
        let symbols = [symbol("a", "-K1")];
        let none = BTreeMap::new();
        let group = Group {
            symbols: &symbols,
            wires: &[],
            graphics: &[],
            devices: &none,
            anchor: Point::new(0.0, 0.0),
        };
        let res = place(
            &mut p,
            2,
            group,
            Point::new(0.0, 0.0),
            DeviceData::GroupOnly,
            OutOfTags::Fail,
        );
        let Err(e) = res else {
            unreachable!("page 3 does not exist")
        };
        assert!(e.to_string().contains("no page 3"), "{e}");
        assert_eq!(p, before);
    }

    #[test]
    fn symbols_wires_and_drawings_move_by_anchor_to_point_and_wires_follow_new_ids() {
        let mut p = project();
        let mut a = symbol("s1", "-K1");
        a.at = Point::new(10.0, 20.0);
        let b = PlacedSymbol {
            tag: None,
            ..symbol("s2", "-X1")
        };
        let symbols = [a, b];
        let mut w = point_wire(
            "w1",
            Endpoint::Pin {
                item: "s1".into(),
                pin: "A1".into(),
            },
            Endpoint::Point {
                at: Point::new(30.0, 20.0),
            },
        );
        w.via = vec![Point::new(20.0, 25.0)];
        w.number = Some("7".into());
        // A pin on an item outside the group keeps its id.
        let outside = point_wire(
            "w9",
            Endpoint::Pin {
                item: "s2".into(),
                pin: "1".into(),
            },
            Endpoint::Pin {
                item: "elsewhere".into(),
                pin: "2".into(),
            },
        );
        let wires = [w, outside];
        let graphics = [Shape::Line {
            a: Point::new(0.0, 0.0),
            b: Point::new(10.0, 0.0),
        }];
        let none = BTreeMap::new();
        let group = Group {
            symbols: &symbols,
            wires: &wires,
            graphics: &graphics,
            devices: &none,
            anchor: Point::new(10.0, 20.0),
        };
        let Ok(placed) = place(
            &mut p,
            0,
            group,
            Point::new(100.0, 200.0),
            DeviceData::ProjectFirst,
            OutOfTags::Fail,
        ) else {
            unreachable!("placed")
        };
        // s1 is taken on the page, so the group's symbols get s2 and s3; its wires w1 and w2.
        assert_eq!(placed.symbols, ["s2", "s3"]);
        assert_eq!(placed.wires, ["w1", "w2"]);
        assert_eq!(placed.graphics, [0]);
        let Some(page) = p.pages.first() else {
            unreachable!("page 1")
        };
        let Some(s) = page.symbol("s2") else { unreachable!("s2") };
        assert_eq!(s.at, Point::new(100.0, 200.0));
        assert_eq!(s.tag.as_ref().map(ToString::to_string).as_deref(), Some("-K2"));
        let Some(s) = page.symbol("s3") else { unreachable!("s3") };
        assert_eq!(s.tag, None);
        assert_eq!(s.at, Point::new(100.0, 190.0));

        let [w1, w2] = page.wires.as_slice() else {
            unreachable!("two new wires")
        };
        assert_eq!((w1.id.as_str(), w2.id.as_str()), ("w1", "w2"));
        assert!(matches!(&w1.from, Endpoint::Pin { item, pin } if item == "s2" && pin == "A1"));
        assert!(matches!(w1.to, Endpoint::Point { at } if at == Point::new(120.0, 200.0)));
        assert_eq!(w1.via, [Point::new(110.0, 205.0)]);
        assert_eq!(w1.number, None, "a placed wire is numbered afresh");
        assert!(matches!(&w2.from, Endpoint::Pin { item, .. } if item == "s3"));
        assert!(matches!(&w2.to, Endpoint::Pin { item, .. } if item == "elsewhere"));
        assert!(matches!(
            page.graphics.first(),
            Some(Shape::Line { a, b }) if *a == Point::new(90.0, 180.0) && *b == Point::new(100.0, 180.0)
        ));
    }

    #[test]
    fn one_device_of_the_group_becomes_one_new_device_and_its_cable_cores_follow() {
        let mut p = project();
        p.devices.insert(tag("-W1"), device("project cable"));
        // Two symbols of -K1 (coil and contact) and a cable -W1 with a core.
        let symbols = [symbol("a", "-K1"), symbol("b", "-K1"), symbol("c", "-W1")];
        let mut core = point_wire(
            "w1",
            Endpoint::Point {
                at: Point::new(0.0, 0.0),
            },
            Endpoint::Point {
                at: Point::new(5.0, 0.0),
            },
        );
        core.cable = Some(crate::project::CableCore {
            cable: tag("-W1"),
            core: "BU".into(),
        });
        // A core of a cable the group does not hold stays in that cable.
        let mut other = core.clone();
        other.id = "w2".into();
        other.cable = Some(crate::project::CableCore {
            cable: tag("-W8"),
            core: "BK".into(),
        });
        let wires = [core, other];
        let none = BTreeMap::new();
        let group = Group {
            symbols: &symbols,
            wires: &wires,
            graphics: &[],
            devices: &none,
            anchor: Point::new(0.0, 0.0),
        };
        let Ok(placed) = place(
            &mut p,
            0,
            group,
            Point::new(0.0, 0.0),
            DeviceData::ProjectFirst,
            OutOfTags::Fail,
        ) else {
            unreachable!("placed")
        };
        let Some(page) = p.pages.first() else {
            unreachable!("page 1")
        };
        let new_tags: Vec<Option<String>> = placed
            .symbols
            .iter()
            .map(|id| page.symbol(id).and_then(|s| s.tag.as_ref()).map(ToString::to_string))
            .collect();
        let want = |s: &str| Some(s.to_string());
        assert_eq!(new_tags, [want("-K2"), want("-K2"), want("-W2")]);
        assert_eq!(
            p.devices.keys().map(ToString::to_string).collect::<Vec<_>>(),
            ["-K1", "-K2", "-W1", "-W2"]
        );
        assert_eq!(
            p.devices.get(&tag("-W2")).map(|d| d.description.as_str()),
            Some("project cable")
        );
        let cable_of = |id: &str| {
            page.wires
                .iter()
                .find(|w| w.id == id)
                .and_then(|w| w.cable.as_ref())
                .map(|c| (c.cable.to_string(), c.core.clone()))
        };
        assert_eq!(cable_of("w1"), Some(("-W2".into(), "BU".into())));
        assert_eq!(cable_of("w2"), Some(("-W8".into(), "BK".into())));
    }
}

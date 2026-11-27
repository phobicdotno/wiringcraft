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
/// 1 to 9999, that is not in `used`.
pub fn free_tag_like(used: &BTreeSet<DeviceTag>, like: &DeviceTag) -> Option<DeviceTag> {
    let letter = like.letter_code();
    (1..10_000).find_map(|n| {
        let mut t = like.clone();
        *t.product.last_mut()? = format!("{letter}{n}");
        (!used.contains(&t)).then_some(t)
    })
}

/// Every tag the project uses: on its pages, in its device data and as the cable of a wire
/// core (a cable with cores but no symbol drawn and no device entry is still taken).
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
    for old in group.symbols.iter().filter_map(|s| s.tag.as_ref()) {
        if renamed.contains_key(old) {
            continue;
        }
        let new = match free_tag_like(&used, old) {
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
        // A core of a cable the group holds runs in that cable under its new tag; when the
        // cable is placed without a tag, the core runs in no cable (not in the project's cable
        // of the old tag).
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
}

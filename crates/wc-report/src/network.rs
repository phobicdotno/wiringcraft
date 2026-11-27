//! The network list: the project's fieldbus networks ([`wc_core::network`]) device by device,
//! with each device's address, its ports and what they are cabled to.

use wc_core::network::Network;

use crate::{Inputs, Table, all_tags, pages_of};

/// Networks: one row per port link of each device on a network (a device without links gets
/// one row), in the order the networks and their devices are written. Each row links to the
/// first page the device is drawn on.
pub fn network_list(i: &Inputs<'_>) -> Table {
    let mut t = Table::new(
        "Networks",
        &[
            "Network",
            "Type",
            "Device",
            "Description",
            "Address",
            "Station",
            "Port",
            "Connected to",
            "Cable",
            "Pages",
        ],
    );
    let p = i.project;
    let index = p.tag_index();
    for net in &p.meta.networks {
        let ports = net.ports();
        for node in &net.nodes {
            let Ok(tag) = node.device.parse::<wc_core::DeviceTag>() else {
                continue;
            };
            // Schematic pages only: an overview shows the device again.
            let uses: Vec<(String, String)> = index
                .get(&tag)
                .into_iter()
                .flatten()
                .filter(|(pid, _)| p.page(pid).is_some_and(|pg| !pg.overview))
                .cloned()
                .collect();
            let first_page = uses
                .iter()
                .filter_map(|(pid, _)| p.page(pid))
                .min_by_key(|pg| wc_core::project::natural_key(&pg.number))
                .map(|pg| pg.id.clone())
                .unwrap_or_default();
            let head = vec![
                net.name.clone(),
                net.kind_label(),
                tag.to_string(),
                p.devices.get(&tag).map(|d| d.description.clone()).unwrap_or_default(),
                node.address.clone(),
                node.station.clone(),
            ];
            let mut links: Vec<[String; 3]> = Vec::new();
            for (a, b, link) in &ports {
                for (mine, other) in [(a, b), (b, a)] {
                    if mine.device == tag {
                        links.push([mine.port.clone(), other.to_string(), link.cable.clone()]);
                    }
                }
            }
            if links.is_empty() {
                links.push(Default::default());
            }
            for l in links {
                let mut row = head.clone();
                row.extend(l);
                row.push(pages_of(p, &uses));
                if !first_page.is_empty() {
                    t.row_pages.resize(t.rows.len(), String::new());
                    t.row_pages.push(first_page.clone());
                }
                t.rows.push(row);
            }
        }
    }
    t
}

/// What is wrong with the project's networks: everything [`Network::warnings`] finds, two
/// networks with one name, and devices on a network that the project has neither drawn nor
/// listed. Empty when there are no networks.
pub fn network_warnings(i: &Inputs<'_>) -> Vec<String> {
    let nets: &[Network] = &i.project.meta.networks;
    let mut out = Vec::new();
    if nets.is_empty() {
        return out;
    }
    let tags = all_tags(i);
    let mut names = std::collections::BTreeSet::new();
    for net in nets {
        out.extend(net.warnings());
        if !net.name.trim().is_empty() && !names.insert(net.name.trim()) {
            out.push(format!("network {}: a second network has this name", net.name));
        }
        for node in &net.nodes {
            if let Ok(tag) = node.device.parse::<wc_core::DeviceTag>()
                && !tags.contains(&tag)
            {
                out.push(format!("network {}: {tag} is not a device of the project", net.name));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use wc_core::geom::{Point, Rotation};
    use wc_core::network::{Link, Network, Node};
    use wc_core::{DeviceTag, Page, PlacedSymbol, Project};

    use super::{network_list, network_warnings};
    use crate::Inputs;

    fn place(id: &str, tag: &str) -> PlacedSymbol {
        PlacedSymbol {
            id: id.into(),
            symbol: "box".into(),
            at: Point::new(0.0, 0.0),
            rotation: Rotation::R0,
            mirror: false,
            tag: tag.parse::<DeviceTag>().ok(),
        }
    }

    fn page(id: &str, number: &str, overview: bool, tags: &[&str]) -> Page {
        let mut pg = Page::new(id, number, "Page");
        pg.overview = overview;
        pg.symbols = tags
            .iter()
            .enumerate()
            .map(|(n, t)| place(&format!("{id}s{n}"), t))
            .collect();
        pg
    }

    fn node(device: &str) -> Node {
        Node {
            device: device.into(),
            ..Default::default()
        }
    }

    fn link(from: &str, to: &str) -> Link {
        Link {
            from: from.into(),
            to: to.into(),
            ..Default::default()
        }
    }

    fn net(name: &str, nodes: &[&str], links: Vec<Link>) -> Network {
        Network {
            name: name.into(),
            kind: "profinet".into(),
            nodes: nodes.iter().map(|d| node(d)).collect(),
            links,
            ..Default::default()
        }
    }

    fn with<R>(p: &Project, f: impl FnOnce(&Inputs<'_>) -> R) -> R {
        let (symbols, parts) = (BTreeMap::new(), BTreeMap::new());
        f(&Inputs {
            project: p,
            symbols: &symbols,
            parts: &parts,
        })
    }

    /// The Pages column (index 9) of every row.
    fn pages_col(rows: &[Vec<String>]) -> Vec<String> {
        rows.iter().map(|r| r.get(9).cloned().unwrap_or_default()).collect()
    }

    #[test]
    fn pages_sort_naturally_skip_overviews_and_link_the_first_page() {
        let mut p = Project::new("T");
        p.pages = vec![
            page("p10", "10", false, &["-A1"]),
            page("p1", "1", true, &["-A1", "-K1"]),
            page("p2", "2", false, &["-A1"]),
        ];
        p.meta.networks = vec![net("PN", &["-A1", "-K1"], vec![])];
        let t = with(&p, network_list);
        assert_eq!(pages_col(&t.rows), ["2, 10", ""]);
        // The lowest page by number, not the first page written; the overview never counts.
        assert_eq!(t.row_pages, ["p2"]);
    }

    #[test]
    fn row_pages_stay_aligned_when_an_undrawn_device_comes_first() {
        let mut p = Project::new("T");
        p.pages = vec![page("p3", "3", false, &["-K2"])];
        p.meta.networks = vec![net(
            "PN",
            &["-K1", "-K2"],
            vec![link("-K1:P1", "-K2:P1"), link("-K2:P2", "-K1:P2")],
        )];
        let t = with(&p, network_list);
        assert_eq!(t.rows.len(), 4);
        // Rows of -K1 have no page; both rows of -K2 link to p3.
        assert_eq!(t.row_pages, ["", "", "p3", "p3"]);
    }

    #[test]
    fn bare_tag_and_unreadable_link_ends() {
        let mut p = Project::new("T");
        p.meta.networks = vec![Network {
            kind: " CustomBus ".into(),
            ..net(
                "B",
                &["-A1", "-K1"],
                vec![link("-A1", "-K1:1"), link("-A1:2", "not a tag!")],
            )
        }];
        let t = with(&p, network_list);
        let row = |c: &[&str]| c.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        // The second link has an unreadable end, so it is left out; a bare tag has no port.
        assert_eq!(
            t.rows,
            [
                row(&["B", "CustomBus", "-A1", "", "", "", "", "-K1:1", "", ""]),
                row(&["B", "CustomBus", "-K1", "", "", "", "1", "-A1", "", ""]),
            ]
        );
    }

    #[test]
    fn unreadable_node_is_not_listed_but_is_warned_about() {
        let mut p = Project::new("T");
        p.pages = vec![page("p1", "1", false, &["-K1"])];
        p.meta.networks = vec![net("PN", &["not a tag!", "-K1"], vec![])];
        let t = with(&p, network_list);
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.rows.first().and_then(|r| r.get(2)).map(String::as_str), Some("-K1"));
        let w = with(&p, network_warnings);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w.iter().all(|m| m.contains("node \"not a tag!\"")), "{w:?}");
    }

    #[test]
    fn duplicate_names_compare_trimmed_and_blank_names_are_not_duplicates() {
        let mut p = Project::new("T");
        p.meta.networks = vec![
            net("PN1", &[], vec![]),
            net(" PN1 ", &[], vec![]),
            net("", &[], vec![]),
            net("  ", &[], vec![]),
        ];
        let w = with(&p, network_warnings);
        assert_eq!(
            w,
            [
                "network  PN1 : a second network has this name",
                "a network has no name",
                "a network has no name",
            ]
        );
    }

    #[test]
    fn a_device_drawn_only_on_an_overview_is_still_a_device_of_the_project() {
        let mut p = Project::new("T");
        p.pages = vec![page("p1", "1", true, &["-K1"])];
        p.meta.networks = vec![net("PN", &["-K1", "-K2"], vec![])];
        assert_eq!(
            with(&p, network_warnings),
            ["network PN: -K2 is not a device of the project"]
        );
        let t = with(&p, network_list);
        assert_eq!(pages_col(&t.rows), ["", ""]);
        assert!(t.row_pages.is_empty());
    }
}

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
            let uses = index.get(&tag).map(Vec::as_slice).unwrap_or(&[]);
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
                row.push(pages_of(p, uses));
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

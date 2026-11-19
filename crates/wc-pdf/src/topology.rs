//! Network topology pages: each fieldbus network ([`wc_core::network`]) drawn as boxes for its
//! devices and lines for its links, after the network list.
//!
//! The devices are laid out from the order of the links: the first node (or the first link
//! end) on the left, each device it links to one column further right, so a line runs across
//! the page and a switch fans out into a tree. A link that would close a loop (a ring back to
//! the first device) is drawn once, routed under the boxes.

use wc_core::network::{Network, Port};
use wc_core::{Page, Project};

use crate::layout::{self, MARGIN, frame, text_fit};
use crate::writer::{Drawn, Link, Pen};

/// Where each device of a network goes and how its links run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Topology {
    /// The devices by their tag as written, each with its column and row in the layout.
    pub boxes: Vec<(String, usize, usize)>,
    /// Links between a device and one it was reached from (box indexes), with the link.
    pub tree: Vec<(usize, usize, usize)>,
    /// Links drawn under the boxes: ones that close a loop, or join two devices of the same
    /// column (box indexes), with the link.
    pub under: Vec<(usize, usize, usize)>,
}

/// The device a link end or a node names, as written (trimmed); a tag that does not parse is
/// kept as text so it is still drawn.
fn key(s: &str) -> String {
    match Port::parse(s) {
        Ok(p) => p.device.to_string(),
        Err(_) => s.trim().to_owned(),
    }
}

/// Lay out `net`: devices in node order then in order of first mention by a link, placed by
/// a breadth-first walk of the links from the first one.
pub fn layout_network(net: &Network) -> Topology {
    let mut names: Vec<String> = Vec::new();
    let mut add = |n: String| {
        if !n.is_empty() && !names.contains(&n) {
            names.push(n);
        }
    };
    for n in &net.nodes {
        add(key(&n.device));
    }
    for l in &net.links {
        add(key(&l.from));
        add(key(&l.to));
    }
    let index = |n: &str| names.iter().position(|x| x == n);
    let ends: Vec<Option<(usize, usize)>> = net
        .links
        .iter()
        .map(|l| Some((index(&key(&l.from))?, index(&key(&l.to))?)))
        .collect();
    let mut place: Vec<Option<(usize, usize)>> = vec![None; names.len()];
    let mut rows_in: Vec<usize> = Vec::new();
    let mut used = vec![false; net.links.len()];
    let mut topo = Topology::default();
    for start in 0..names.len() {
        if place[start].is_some() {
            continue;
        }
        // A part of the network not linked to what is drawn so far starts below it.
        let row = rows_in.iter().copied().max().unwrap_or(0);
        if rows_in.is_empty() {
            rows_in.push(0);
        }
        place[start] = Some((0, row));
        rows_in[0] = rows_in[0].max(row + 1);
        let mut queue = std::collections::VecDeque::from([start]);
        while let Some(at) = queue.pop_front() {
            let Some((col, _)) = place[at] else { continue };
            for (k, e) in ends.iter().enumerate() {
                let Some((a, b)) = *e else { continue };
                if used[k] || (a != at && b != at) {
                    continue;
                }
                let other = if a == at { b } else { a };
                used[k] = true;
                if other == at {
                    continue;
                }
                if place[other].is_none() {
                    let c = col + 1;
                    if rows_in.len() <= c {
                        rows_in.resize(c + 1, 0);
                    }
                    // Not above the device it hangs from.
                    let parent_row = place[at].map_or(0, |p| p.1);
                    let r = rows_in[c].max(parent_row);
                    rows_in[c] = r + 1;
                    place[other] = Some((c, r));
                    topo.tree.push((at, other, k));
                    queue.push_back(other);
                } else {
                    topo.under.push((at, other, k));
                }
            }
        }
    }
    topo.boxes = names
        .into_iter()
        .zip(place)
        .map(|(n, p)| {
            let (c, r) = p.unwrap_or_default();
            (n, c, r)
        })
        .collect();
    topo
}

const TEXT: f64 = 2.2;
const LINE: f64 = 3.2;

/// One topology page for `net`: frame and title block, a box per device with its tag,
/// description, address and station, and each link once with its port names and cable tag.
/// A box links to the first page its device is drawn on.
pub fn draw_network_page(
    project: &Project,
    page: &Page,
    index: usize,
    total: usize,
    net: &Network,
    date: &str,
) -> Drawn {
    let (w, h) = page.size.mm();
    let mut pen = Pen::new(h);
    pen.ops.push_str("1 J 1 j\n");
    pen.colour(0.0, 0.0, 0.0);
    frame(&mut pen, project, page, index, total, date);
    let band = project.meta.template.band_height();
    let (x0, x1) = (MARGIN + 10.0, w - MARGIN - 10.0);
    let title_y = MARGIN + layout::HEADER + 10.0;
    let kind = net.kind_label();
    let title = match (kind.is_empty(), net.description.trim().is_empty()) {
        (true, true) => format!("Network {}", net.name),
        (false, true) => format!("Network {} ({kind})", net.name),
        (true, false) => format!("Network {}: {}", net.name, net.description.trim()),
        (false, false) => format!("Network {} ({kind}): {}", net.name, net.description.trim()),
    };
    pen.text((x0, title_y), 4.0, 0, &text_fit(&title, 4.0, x1 - x0));

    let topo = layout_network(net);
    let cols = topo.boxes.iter().map(|b| b.1 + 1).max().unwrap_or(1);
    let rows = topo.boxes.iter().map(|b| b.2 + 1).max().unwrap_or(1);
    let (y0, y1) = (title_y + 8.0, h - MARGIN - band - 6.0);
    // Room for the lanes of the links drawn under the boxes.
    let lanes = topo.under.len() as f64 * 4.0;
    let step_x = ((x1 - x0) / cols as f64).min(70.0);
    let box_w = (step_x - 18.0).clamp(20.0, 48.0);
    let box_h = 4.0 * LINE + 2.0;
    let step_y = ((y1 - y0 - lanes) / rows as f64).min(box_h + 16.0).max(box_h + 4.0);
    let spot = |c: usize, r: usize| (x0 + c as f64 * step_x, y0 + r as f64 * step_y);

    let tags = project.tag_index();
    let mut links = Vec::new();
    pen.width(0.25);
    for (name, c, r) in &topo.boxes {
        let (bx, by) = spot(*c, *r);
        pen.rect((bx, by), (bx + box_w, by + box_h));
        let tag = name.parse::<wc_core::DeviceTag>().ok();
        let node = tag.as_ref().and_then(|t| net.node(t));
        let description = tag
            .as_ref()
            .and_then(|t| project.devices.get(t))
            .map(|d| d.description.trim().to_owned())
            .unwrap_or_default();
        let lines = [
            name.clone(),
            description,
            node.map(|n| n.address.trim().to_owned()).unwrap_or_default(),
            node.map(|n| n.station.trim().to_owned()).unwrap_or_default(),
        ];
        for (i, t) in lines.iter().enumerate() {
            let size = if i == 0 { TEXT + 0.4 } else { TEXT };
            pen.text(
                (bx + 1.5, by + 1.0 + LINE * (i as f64 + 1.0)),
                size,
                0,
                &text_fit(t, size, box_w - 3.0),
            );
        }
        if let Some(target) = tag.as_ref().and_then(|t| tags.get(t)).and_then(|at| at.first()) {
            links.push(Link {
                min: (bx, by),
                max: (bx + box_w, by + box_h),
                target_page_id: target.0.clone(),
            });
        }
    }
    let port = |s: &str| Port::parse(s).map(|p| p.port).unwrap_or_default();
    // Each link's ends: its `from` end at the first box given.
    let ends = |a: usize, k: usize| {
        let l = &net.links[k];
        let a_is_from = key(&l.from) == topo.boxes[a].0;
        if a_is_from {
            (port(&l.from), port(&l.to))
        } else {
            (port(&l.to), port(&l.from))
        }
    };
    for &(a, b, k) in &topo.tree {
        let (pa, pb) = spot(topo.boxes[a].1, topo.boxes[a].2);
        let (qa, qb) = spot(topo.boxes[b].1, topo.boxes[b].2);
        let start = (pa + box_w, pb + box_h / 2.0);
        let end = (qa, qb + box_h / 2.0);
        let mid = (start.0 + end.0) / 2.0;
        pen.polyline(&[start, (mid, start.1), (mid, end.1), end], false);
        let (from_port, to_port) = ends(a, k);
        pen.text((start.0 + 1.0, start.1 - 1.0), TEXT, 0, &from_port);
        pen.text((end.0 - 1.0, end.1 - 1.0), TEXT, 2, &to_port);
        let cable = net.links[k].cable.trim();
        pen.text((mid + 1.0, (start.1 + end.1) / 2.0 + LINE), TEXT, 0, cable);
    }
    let bottom = topo.boxes.iter().map(|b| spot(b.1, b.2).1 + box_h).fold(y0, f64::max);
    for (lane, &(a, b, k)) in topo.under.iter().enumerate() {
        let (pa, pb) = spot(topo.boxes[a].1, topo.boxes[a].2);
        let (qa, qb) = spot(topo.boxes[b].1, topo.boxes[b].2);
        let y = bottom + 4.0 * (lane as f64 + 1.0);
        let start = (pa + box_w / 2.0, pb + box_h);
        let end = (qa + box_w / 2.0 + 2.0, qb + box_h);
        pen.polyline(&[start, (start.0, y), (end.0, y), end], false);
        let (from_port, to_port) = ends(a, k);
        pen.text((start.0 + 1.0, start.1 + LINE), TEXT, 0, &from_port);
        pen.text((end.0 + 1.0, end.1 + LINE), TEXT, 0, &to_port);
        let cable = net.links[k].cable.trim();
        pen.text(((start.0 + end.0) / 2.0, y - 1.0), TEXT, 1, cable);
    }
    if topo.boxes.is_empty() {
        pen.colour(0.45, 0.45, 0.45);
        pen.text((x0, y0 + LINE), TEXT, 0, "No devices on this network.");
    }
    Drawn {
        ops: pen.ops,
        links,
        prims: pen.prims,
        layers: pen.layers,
        underlay: None,
        pictures: Vec::new(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use wc_core::network::{Link as NetLink, Node};

    fn node(d: &str) -> Node {
        Node {
            device: d.into(),
            ..Node::default()
        }
    }

    fn link(a: &str, b: &str) -> NetLink {
        NetLink {
            from: a.into(),
            to: b.into(),
            ..NetLink::default()
        }
    }

    #[test]
    fn a_line_runs_across_a_switch_fans_out_and_a_ring_is_drawn_under() {
        // PLC - switch - two drives.
        let net = Network {
            name: "PN1".into(),
            nodes: ["-A1", "-K10", "-T1", "-T2"].into_iter().map(node).collect(),
            links: vec![
                link("-A1:X1P1", "-K10:P1"),
                link("-K10:P2", "-T1:X1"),
                link("-K10:P3", "-T2:X1"),
            ],
            ..Network::default()
        };
        let t = layout_network(&net);
        let at = |n: &str| t.boxes.iter().find(|b| b.0 == n).map(|b| (b.1, b.2)).unwrap();
        assert_eq!(
            (at("-A1"), at("-K10"), at("-T1"), at("-T2")),
            ((0, 0), (1, 0), (2, 0), (2, 1))
        );
        assert_eq!(t.tree.len(), 3);
        assert!(t.under.is_empty());

        // A ring: the last device links back to the first. Every link is drawn once.
        let ring = Network {
            links: vec![
                link("-A1:P1", "-K10:P1"),
                link("-K10:P2", "-T1:P1"),
                link("-T1:P2", "-A1:P2"),
            ],
            nodes: ["-A1", "-K10", "-T1"].into_iter().map(node).collect(),
            ..Network::default()
        };
        let t = layout_network(&ring);
        assert_eq!(t.tree.len() + t.under.len(), 3);
        let mut drawn: Vec<usize> = t.tree.iter().chain(&t.under).map(|e| e.2).collect();
        drawn.sort_unstable();
        assert_eq!(drawn, [0, 1, 2]);
    }
}

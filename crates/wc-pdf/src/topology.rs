//! Network topology pages: each fieldbus network ([`wc_core::network`]) drawn as boxes for its
//! devices and lines for its links, after the network list.
//!
//! The devices are laid out from the order of the links: the first node (or the first link
//! end) on the left, each device it links to one column further right, so a line runs across
//! the page and a switch fans out into a tree. A link that would close a loop (a ring back to
//! the first device) is drawn once, routed beside the boxes to a lane under them; a link
//! between two ports of one device is a small loop over its box. The sheet grows (a larger
//! ISO size, else a custom one) until every box has room.

use std::collections::VecDeque;

use wc_core::network::{Network, Port};
use wc_core::{Page, PageSize, Project};

use crate::layout::{self, MARGIN, frame, text_fit};
use crate::writer::{Drawn, Link, Pen};

/// Where each device of a network goes and how its links run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Topology {
    /// The devices by their tag as written, each with its column and row in the layout.
    pub boxes: Vec<(String, usize, usize)>,
    /// Links between a device and one it was reached from (box indexes), with the link.
    pub tree: Vec<(usize, usize, usize)>,
    /// Links drawn in lanes under the boxes: ones that close a loop, or join two devices
    /// already placed (box indexes), with the link.
    pub under: Vec<(usize, usize, usize)>,
    /// Links between two ports of one device (box index), with the link.
    pub own: Vec<(usize, usize)>,
    /// Links an end of which names no device (an empty end), by index.
    pub unplaced: Vec<usize>,
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
    let mut topo = Topology {
        unplaced: ends
            .iter()
            .enumerate()
            .filter(|(_, e)| e.is_none())
            .map(|(k, _)| k)
            .collect(),
        ..Topology::default()
    };
    let mut place: Vec<Option<(usize, usize)>> = vec![None; names.len()];
    // The next free row of each column.
    let mut rows_in: Vec<usize> = Vec::new();
    let mut used = vec![false; ends.len()];
    let take_row = |rows_in: &mut Vec<usize>, col: usize, at_least: usize| -> usize {
        if rows_in.len() <= col {
            rows_in.resize(col + 1, 0);
        }
        let slot = rows_in.get_mut(col);
        let row = slot.as_ref().map_or(0, |n| **n).max(at_least);
        if let Some(n) = slot {
            *n = row + 1;
        }
        row
    };
    for start in 0..names.len() {
        if !matches!(place.get(start), Some(None)) {
            continue;
        }
        // A part of the network not linked to what is drawn so far starts below it.
        let below = rows_in.iter().copied().max().unwrap_or(0);
        let row = take_row(&mut rows_in, 0, below);
        if let Some(slot) = place.get_mut(start) {
            *slot = Some((0, row));
        }
        let mut queue = VecDeque::from([start]);
        while let Some(at) = queue.pop_front() {
            let Some(&Some((col, at_row))) = place.get(at) else {
                continue;
            };
            for (k, (e, done)) in ends.iter().zip(used.iter_mut()).enumerate() {
                let Some((a, b)) = *e else { continue };
                if *done || (a != at && b != at) {
                    continue;
                }
                *done = true;
                let other = if a == at { b } else { a };
                if other == at {
                    topo.own.push((at, k));
                    continue;
                }
                if matches!(place.get(other), Some(None)) {
                    // Not above the device it hangs from.
                    let r = take_row(&mut rows_in, col + 1, at_row);
                    if let Some(slot) = place.get_mut(other) {
                        *slot = Some((col + 1, r));
                    }
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
/// A box: four lines of text.
const BOX_H: f64 = 4.0 * LINE + 2.0;
/// The narrowest a box gets, and the room between two columns for a link's labels.
const MIN_BOX_W: f64 = 22.0;
const GAP_X: f64 = 40.0;
/// Room between two rows, for a loop over a box and its label.
const GAP_Y: f64 = 8.0;
/// Room for each lane under the boxes.
const LANE: f64 = 4.0;
/// A lane at least this long has a label at each end and one in the middle.
const LONG_LANE: f64 = 24.0;

/// Where the boxes go on a sheet: the top left of column 0, row 0, the steps between columns
/// and rows, and the size of a box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub x0: f64,
    pub y0: f64,
    pub step_x: f64,
    pub step_y: f64,
    pub box_w: f64,
    pub box_h: f64,
}

impl Grid {
    /// The top left corner of the box in column `c`, row `r`.
    pub fn spot(&self, c: usize, r: usize) -> (f64, f64) {
        (self.x0 + c as f64 * self.step_x, self.y0 + r as f64 * self.step_y)
    }

    /// Where the link lanes leave the side of a box of column `c`: in the room left of it,
    /// clear of the boxes and of the labels of the links into and out of that room.
    fn side(&self, c: usize) -> f64 {
        let x = self.x0 + c as f64 * self.step_x;
        (x - (self.step_x - self.box_w) / 2.0 - 1.5).max(MARGIN + 2.0)
    }
}

/// The columns and rows a layout uses.
fn extent(topo: &Topology) -> (usize, usize) {
    let cols = topo.boxes.iter().map(|b| b.1 + 1).max().unwrap_or(1);
    let rows = topo.boxes.iter().map(|b| b.2 + 1).max().unwrap_or(1);
    (cols, rows)
}

/// The top of the drawing area: below the title line.
fn top() -> f64 {
    MARGIN + layout::HEADER + 18.0
}

/// The room over a box its same-device loops take beyond the first: one line each.
fn loop_room(topo: &Topology) -> f64 {
    let most = (0..topo.boxes.len())
        .map(|b| topo.own.iter().filter(|o| o.0 == b).count())
        .max()
        .unwrap_or(0);
    most.saturating_sub(1) as f64 * LINE
}

/// The `nth` same-device loop over a box at `(bx, by)` `box_w` wide, and where its label goes:
/// each one wider and higher than the one before, so they nest without touching. `count` is
/// how many loops the box has.
pub fn own_path(bx: f64, by: f64, box_w: f64, nth: usize, count: usize) -> ([(f64, f64); 4], (f64, f64)) {
    // Each loop's legs step 1.5 mm out from the one inside it, closer when there are many, so
    // the outermost still ends on the box, half a millimetre in from its corners.
    let step = match count {
        0 | 1 => 0.0,
        n => 1.5f64.min((box_w * 0.15 - 0.5) / (n - 1) as f64),
    };
    let spread = step * nth as f64;
    let (l, r) = (bx + box_w * 0.15 - spread, bx + box_w * 0.85 + spread);
    let y = by - 3.0 - LINE * nth as f64;
    ([(l, by), (l, y), (r, y), (r, by)], ((l + r) / 2.0, y - 0.8))
}

/// The space `topo` needs on a sheet, in millimetres, with a title block `band` high.
fn needed(topo: &Topology, band: f64) -> (f64, f64) {
    let (cols, rows) = extent(topo);
    let w = 2.0 * (MARGIN + 10.0) + cols as f64 * (MIN_BOX_W + GAP_X);
    let lanes = (topo.under.len() + 1) as f64 * LANE;
    let h = top() + rows as f64 * (BOX_H + GAP_Y + loop_room(topo)) + lanes + 6.0 + band + MARGIN;
    (w, h)
}

/// The sheet a network is drawn on: `wanted` (the size of the first schematic page) when the
/// layout fits it, else the first ISO size that fits, else a custom sheet. A layout too large
/// even for the largest custom sheet gets that, with a warning.
pub fn sheet(wanted: PageSize, band: f64, net: &Network) -> (PageSize, Option<String>) {
    let (w, h) = needed(&layout_network(net), band);
    let fits = |s: &PageSize| {
        let (sw, sh) = s.mm();
        sw >= w && sh >= h
    };
    if let Some(s) = std::iter::once(wanted).chain(PageSize::NAMED).find(fits) {
        return (s, None);
    }
    let (lo, hi) = (f64::from(PageSize::MIN_MM), f64::from(PageSize::MAX_MM));
    let side = |v: f64| v.ceil().clamp(lo, hi) as u32;
    let size = PageSize::Custom {
        width: side(w),
        height: side(h),
    };
    let warn = (w > hi || h > hi).then(|| {
        format!(
            "network {}: its topology needs {w:.0} x {h:.0} mm, more than the largest sheet; some devices are drawn off the sheet",
            net.name
        )
    });
    (size, warn)
}

/// The grid for `topo` on a sheet `w` by `h` mm with a title block `band` high: columns and
/// rows spread over the room there is, never closer than a box and its labels need.
pub fn grid(topo: &Topology, w: f64, h: f64, band: f64) -> Grid {
    let (cols, rows) = extent(topo);
    let (x0, x1) = (MARGIN + 10.0, w - MARGIN - 10.0);
    let extra = loop_room(topo);
    let (y0, y1) = (top() + extra, h - MARGIN - band - 6.0);
    let lanes = (topo.under.len() + 1) as f64 * LANE;
    let step_x = ((x1 - x0) / cols as f64).clamp(MIN_BOX_W + GAP_X, 88.0);
    let box_w = (step_x - GAP_X).clamp(MIN_BOX_W, 48.0);
    let low = BOX_H + GAP_Y + extra;
    let step_y = ((y1 - y0 - lanes) / rows as f64).clamp(low, low + 8.0);
    Grid {
        x0,
        y0,
        step_x,
        step_y,
        box_w,
        box_h: BOX_H,
    }
}

/// The path of a link drawn in lane `lane` under the boxes, from the box at `a` to the one
/// at `b` (column, row): out of the lower left of each box into the room left of its column,
/// down that room to the lane, and along it.
pub fn under_path(g: &Grid, a: (usize, usize), b: (usize, usize), bottom: f64, lane: usize) -> Vec<(f64, f64)> {
    let (ax, ay) = g.spot(a.0, a.1);
    let (bx, by) = g.spot(b.0, b.1);
    let y = bottom + LANE * (lane as f64 + 1.0);
    let (sa, sb) = (ay + g.box_h - 1.5, by + g.box_h - 1.5);
    // The two ends' runs down a shared room stay apart.
    let (xa, xb) = (g.side(a.0), g.side(b.0) + 1.5);
    vec![(ax, sa), (xa, sa), (xa, y), (xb, y), (xb, sb), (bx, sb)]
}

/// The lowest edge of any box.
fn bottom_of(topo: &Topology, g: &Grid) -> f64 {
    topo.boxes
        .iter()
        .map(|b| g.spot(b.1, b.2).1 + g.box_h)
        .fold(g.y0, f64::max)
}

/// A point on the sheet, in mm.
type Pt = (f64, f64);

/// The path of a tree link from the box at `a` to the box at `b` (top left corners), and where
/// its `from` port, `to` port and cable labels go. Links fanning out of one box share its
/// right edge, so the labels sit on the link's own branch at the height of `b`: the port
/// names over it, one at each end, and the cable under it.
fn tree_path(g: &Grid, a: Pt, b: Pt) -> (Vec<Pt>, [Pt; 3]) {
    let start = (a.0 + g.box_w, a.1 + g.box_h / 2.0);
    let end = (b.0, b.1 + g.box_h / 2.0);
    let mid = (start.0 + end.0) / 2.0;
    let path = vec![start, (mid, start.1), (mid, end.1), end];
    let labels = [
        (mid + 1.0, end.1 - 1.0),
        (end.0 - 1.0, end.1 - 1.0),
        (mid + 1.0, end.1 + TEXT + 0.6),
    ];
    (path, labels)
}

/// A link label: where it goes, how it is aligned there (0 left, 1 centred, 2 right, as for
/// [`Pen::text`]) and its text, already cut to the room it has.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub at: Pt,
    pub align: u8,
    pub text: String,
}

impl Label {
    fn new(at: Pt, align: u8, s: &str, room: f64) -> Self {
        Label {
            at,
            align,
            text: text_fit(s.trim(), TEXT, room.max(0.0)),
        }
    }

    /// Its left and right ends on the sheet.
    #[cfg(test)]
    fn span(&self) -> (f64, f64) {
        let tw = crate::writer::text_width_mm(&self.text, TEXT);
        match self.align {
            0 => (self.at.0, self.at.0 + tw),
            1 => (self.at.0 - tw / 2.0, self.at.0 + tw / 2.0),
            _ => (self.at.0 - tw, self.at.0),
        }
    }
}

/// A link's port names and cable on one line, `P1 - P2 -W1`, `room` mm wide at most. When it
/// is too long each of the three is cut to a third of the room, so none is lost.
fn joined(from: &str, to: &str, cable: &str, room: f64) -> String {
    let (from, to, cable) = (from.trim(), to.trim(), cable.trim());
    let whole = format!("{from} - {to} {cable}");
    if crate::writer::text_width_mm(whole.trim(), TEXT) <= room {
        return whole.trim().to_owned();
    }
    let gaps = crate::writer::text_width_mm(" -  ", TEXT);
    let third = ((room - gaps) / 3.0).max(0.0);
    let cut = |s: &str| {
        if s.is_empty() {
            String::new()
        } else {
            text_fit(s, TEXT, third)
        }
    };
    format!("{} - {} {}", cut(from), cut(to), cut(cable)).trim().to_owned()
}

/// The lines of every link of `net` laid out as `topo` on grid `g`, on a sheet `w` mm wide,
/// and their port and cable labels, each cut to the room beside its line.
pub fn link_drawing(topo: &Topology, g: &Grid, net: &Network, w: f64) -> (Vec<Vec<Pt>>, Vec<Label>) {
    let at = |i: usize| topo.boxes.get(i).map(|b| g.spot(b.1, b.2));
    let port = |s: &str| Port::parse(s).map(|p| p.port).unwrap_or_default();
    // A link's port names and cable, its `from` end first when that is at box `a`.
    let ends = |a: usize, k: usize| -> Option<(String, String, String)> {
        let l = net.links.get(k)?;
        let a_is_from = topo.boxes.get(a).is_some_and(|b| key(&l.from) == b.0);
        let (p, q) = if a_is_from {
            (port(&l.from), port(&l.to))
        } else {
            (port(&l.to), port(&l.from))
        };
        Some((p, q, l.cable.trim().to_owned()))
    };
    let (mut paths, mut labels) = (Vec::new(), Vec::new());
    for &(a, b, k) in &topo.tree {
        let (Some(pa), Some(qa), Some((from_port, to_port, cable))) = (at(a), at(b), ends(a, k)) else {
            continue;
        };
        let (path, [from_at, to_at, cable_at]) = tree_path(g, pa, qa);
        // The two port names share the branch from the junction to the box.
        let branch = to_at.0 + 1.0 - from_at.0 + 1.0;
        let half = branch / 2.0 - 1.5;
        labels.push(Label::new(from_at, 0, &from_port, half));
        labels.push(Label::new(to_at, 2, &to_port, half));
        labels.push(Label::new(cable_at, 0, &cable, branch - 2.0));
        paths.push(path);
    }
    let bottom = bottom_of(topo, g);
    let (left, right) = (MARGIN + 1.0, w - MARGIN - 1.0);
    for (lane, &(a, b, k)) in topo.under.iter().enumerate() {
        let (Some(ba), Some(bb), Some((from_port, to_port, cable))) =
            (topo.boxes.get(a), topo.boxes.get(b), ends(a, k))
        else {
            continue;
        };
        let path = under_path(g, (ba.1, ba.2), (bb.1, bb.2), bottom, lane);
        // The labels go over the link's own lane, so links that leave one box apart from
        // each other (and share the run beside it) are told apart by their lanes.
        if let (Some(&(lx, ly)), Some(&(rx, _))) = (path.get(2), path.get(3)) {
            let (len, y) = ((rx - lx).abs(), ly - 0.8);
            if len >= LONG_LANE {
                // Each port name at its own end of the lane, the cable between them.
                let (from_align, to_align, dir) = if lx <= rx { (0, 2, 1.0) } else { (2, 0, -1.0) };
                let quarter = len / 4.0 - 1.0;
                labels.push(Label::new((lx + dir, y), from_align, &from_port, quarter));
                labels.push(Label::new((rx - dir, y), to_align, &to_port, quarter));
                labels.push(Label::new(((lx + rx) / 2.0, y), 1, &cable, len / 2.0 - 2.0));
            } else {
                // A short lane: one label over it, as wide as a gap.
                let c = (lx + rx) / 2.0;
                let half = (g.step_x - g.box_w) / 2.0 - 1.0;
                let half = half.min(c - left).min(right - c);
                let label = joined(&from_port, &to_port, &cable, 2.0 * half);
                labels.push(Label::new((c, y), 1, &label, 2.0 * half));
            }
        }
        paths.push(path);
    }
    for (n, &(a, k)) in topo.own.iter().enumerate() {
        let (Some((bx, by)), Some((from_port, to_port, cable))) = (at(a), ends(a, k)) else {
            continue;
        };
        // Loops over the same box nest, one above the other, each label on its own line in
        // the room over the box. A label may run across the legs of the loops around it and
        // into half of the gaps either side, so even on a narrow box each of its port names
        // and cable keeps a few letters.
        let nth = topo.own.iter().take(n).filter(|o| o.0 == a).count();
        let count = topo.own.iter().filter(|o| o.0 == a).count();
        let (path, label_at) = own_path(bx, by, g.box_w, nth, count);
        let c = label_at.0;
        let room = 2.0 * (g.step_x / 2.0 - 1.0).min(c - left).min(right - c);
        let label = joined(&from_port, &to_port, &cable, room);
        labels.push(Label::new(label_at, 1, &label, room));
        paths.push(path.to_vec());
    }
    (paths, labels)
}

/// One topology page for `net`: frame and title block, a box per device with its tag,
/// description, address and station, and each link once with its port names and cable tag.
/// A box links to the first page its device is drawn on. A node or link end that does not
/// read as a device tag is still drawn as written, and why is added to `warnings`.
pub fn draw_network_page(
    project: &Project,
    page: &Page,
    index: usize,
    total: usize,
    net: &Network,
    date: &str,
    warnings: &mut Vec<String>,
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

    let mut warn = |why: String| {
        let w = format!("network {}: {why}", net.name);
        if !warnings.contains(&w) {
            warnings.push(w);
        }
    };
    for n in &net.nodes {
        if let Err(why) = n.device.trim().parse::<wc_core::DeviceTag>() {
            warn(format!("node {:?}: {why}", n.device.trim()));
        }
    }
    for (k, l) in net.links.iter().enumerate() {
        for end in [&l.from, &l.to] {
            if let Err(why) = Port::parse(end) {
                warn(format!("link {}: {why}", k + 1));
            }
        }
    }
    let topo = layout_network(net);
    for &k in &topo.unplaced {
        if let Some(l) = net.links.get(k) {
            warn(format!(
                "link {} ({} to {}) is not drawn: an end names no device",
                k + 1,
                l.from.trim(),
                l.to.trim()
            ));
        }
    }
    let g = grid(&topo, w, h, band);

    let tags = project.tag_index();
    let mut links = Vec::new();
    pen.width(0.25);
    for (name, c, r) in &topo.boxes {
        let (bx, by) = g.spot(*c, *r);
        pen.rect((bx, by), (bx + g.box_w, by + g.box_h));
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
                &text_fit(t, size, g.box_w - 3.0),
            );
        }
        if let Some(target) = tag.as_ref().and_then(|t| tags.get(t)).and_then(|at| at.first()) {
            links.push(Link {
                min: (bx, by),
                max: (bx + g.box_w, by + g.box_h),
                target_page_id: target.0.clone(),
            });
        }
    }
    let (paths, labels) = link_drawing(&topo, &g, net, w);
    for path in &paths {
        pen.polyline(path, false);
    }
    for l in &labels {
        pen.text(l.at, TEXT, l.align, &l.text);
    }
    if topo.boxes.is_empty() {
        pen.colour(0.45, 0.45, 0.45);
        pen.text((x0, g.y0 + LINE), TEXT, 0, "No devices on this network.");
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

    fn ring() -> Network {
        Network {
            links: vec![
                link("-A1:P1", "-K10:P1"),
                link("-K10:P2", "-T1:P1"),
                link("-T1:P2", "-A1:P2"),
            ],
            nodes: ["-A1", "-K10", "-T1"].into_iter().map(node).collect(),
            ..Network::default()
        }
    }

    /// Whether an axis-aligned segment runs through the inside of a box.
    fn crosses(p: (f64, f64), q: (f64, f64), min: (f64, f64), max: (f64, f64)) -> bool {
        let inside = |v: f64, lo: f64, hi: f64| v > lo && v < hi;
        let overlaps = |a: f64, b: f64, lo: f64, hi: f64| a.min(b) < hi && a.max(b) > lo;
        if (p.0 - q.0).abs() < 1e-9 {
            inside(p.0, min.0, max.0) && overlaps(p.1, q.1, min.1, max.1)
        } else {
            inside(p.1, min.1, max.1) && overlaps(p.0, q.0, min.0, max.0)
        }
    }

    /// The boxes of a layout on its sheet do not overlap and stay inside the frame.
    fn fits_its_sheet(net: &Network) -> (PageSize, Grid, Topology) {
        let band = 30.0;
        let (size, warn) = sheet(PageSize::A3, band, net);
        assert!(warn.is_none());
        let (w, h) = size.mm();
        let topo = layout_network(net);
        let g = grid(&topo, w, h, band);
        let rects: Vec<((f64, f64), (f64, f64))> = topo
            .boxes
            .iter()
            .map(|b| {
                let (x, y) = g.spot(b.1, b.2);
                ((x, y), (x + g.box_w, y + g.box_h))
            })
            .collect();
        for (i, a) in rects.iter().enumerate() {
            assert!(
                a.0.0 >= MARGIN && a.1.0 <= w - MARGIN,
                "box {i} is inside the sheet across"
            );
            assert!(
                a.1.1 + (topo.under.len() as f64 + 1.0) * LANE <= h - MARGIN - band,
                "box {i} is above the title block"
            );
            for b in rects.iter().skip(i + 1) {
                let apart = a.1.0 <= b.0.0 || b.1.0 <= a.0.0 || a.1.1 <= b.0.1 || b.1.1 <= a.0.1;
                assert!(apart, "{a:?} overlaps {b:?}");
            }
        }
        (size, g, topo)
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
        // The switch's two ports are labelled on their own branches, not on top of each other.
        let g = grid(&t, 420.0, 297.0, 30.0);
        let spot = |i: usize| g.spot(t.boxes[i].1, t.boxes[i].2);
        let from_labels: Vec<(f64, f64)> = t
            .tree
            .iter()
            .filter(|e| t.boxes[e.0].0 == "-K10")
            .map(|&(a, b, _)| {
                let (path, labels) = tree_path(&g, spot(a), spot(b));
                let end = path[path.len() - 1];
                assert!(
                    (labels[0].1 - (end.1 - 1.0)).abs() < 1e-9,
                    "on the branch to {}",
                    t.boxes[b].0
                );
                labels[0]
            })
            .collect();
        assert_eq!(from_labels.len(), 2);
        assert!((from_labels[0].1 - from_labels[1].1).abs() >= LINE, "{from_labels:?}");

        // A ring: the last device links back to the first. Every link is drawn once.
        let t = layout_network(&ring());
        assert_eq!(t.tree.len() + t.under.len(), 3);
        let mut drawn: Vec<usize> = t.tree.iter().chain(&t.under).map(|e| e.2).collect();
        drawn.sort_unstable();
        assert_eq!(drawn, [0, 1, 2]);
    }

    #[test]
    fn long_port_and_cable_names_are_cut_to_the_room_beside_their_line() {
        let long = |s: &str| format!("{s}-PORT-WITH-A-VERY-LONG-NAME");
        let mut net = Network {
            nodes: ["-A1", "-K10", "-T1", "-T2"].into_iter().map(node).collect(),
            links: vec![
                link(&long("-A1:X1"), &long("-K10:P1")),
                link(&long("-K10:P2"), &long("-T1:X1")),
                link(&long("-K10:P3"), &long("-T2:X1")),
                // Back to the start: drawn under the boxes.
                link(&long("-T2:X2"), &long("-A1:X2")),
                link(&long("-K10:P4"), &long("-K10:P5")),
                link(&long("-K10:P6"), &long("-K10:P7")),
            ],
            ..Network::default()
        };
        for l in &mut net.links {
            l.cable = "-W1-A-CABLE-TAG-FAR-TOO-LONG-FOR-ANY-GAP".into();
        }
        let (size, g, topo) = fits_its_sheet(&net);
        assert_eq!((topo.under.len(), topo.own.len()), (1, 2));
        let (w, _) = size.mm();
        let (_, labels) = link_drawing(&topo, &g, &net, w);
        // Three tree links with three labels each; the ring link joins two devices of one
        // column, so its short lane has one label; and a label per loop.
        assert_eq!(labels.len(), 3 * 3 + 1 + 2);
        // A label holding both port names and the cable keeps a piece of each.
        let joined: Vec<&Label> = labels.iter().filter(|l| l.text.contains(" - ")).collect();
        assert_eq!(joined.len(), 3, "{labels:?}");
        for l in joined {
            let (ports, cable) = l.text.rsplit_once(' ').unwrap();
            let (from, to) = ports.split_once(" - ").unwrap();
            assert!(from.len() > 3 && to.len() > 3, "{l:?} shows both ports");
            assert!(
                cable.starts_with("-W") && cable.ends_with("..."),
                "{l:?} shows the cable"
            );
        }
        for l in &labels {
            assert!(l.text.ends_with("..."), "{l:?} is cut");
            let (x0, x1) = l.span();
            assert!(x0 >= MARGIN && x1 <= w - MARGIN, "{l:?} is inside the frame");
            // The text runs from its baseline up one text height.
            let (y0, y1) = (l.at.1 - TEXT, l.at.1);
            for b in &topo.boxes {
                let (bx, by) = g.spot(b.1, b.2);
                let apart = x1 <= bx || x0 >= bx + g.box_w || y1 <= by || y0 >= by + g.box_h;
                assert!(apart, "{l:?} overlaps {}", b.0);
            }
        }
        apart(&labels);
    }

    /// No two labels overlap.
    fn apart(labels: &[Label]) {
        for (i, a) in labels.iter().enumerate() {
            for b in labels.iter().skip(i + 1) {
                let ((a0, a1), (b0, b1)) = (a.span(), b.span());
                let apart = a1 <= b0 || b1 <= a0 || a.at.1 <= b.at.1 - TEXT || b.at.1 <= a.at.1 - TEXT;
                assert!(apart, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn ring_links_leaving_one_device_are_labelled_apart() {
        let net = Network {
            links: vec![
                link("-A1:P1", "-B1:P1"),
                link("-A1:P2", "-C1:P1"),
                link("-A1:P3", "-D1:P1"),
                link("-B1:P2", "-C1:P2"),
                link("-B1:P3", "-D1:P2"),
            ],
            ..Network::default()
        };
        let (size, g, topo) = fits_its_sheet(&net);
        assert_eq!(topo.under.len(), 2);
        let (_, labels) = link_drawing(&topo, &g, &net, size.mm().0);
        // B1's two ring ports are each on a label of their own lane, under the boxes.
        let bottom = bottom_of(&topo, &g);
        let with = |t: &str| {
            labels
                .iter()
                .filter(|l| l.text.split(' ').next() == Some(t) && l.at.1 > bottom)
                .count()
        };
        assert_eq!((with("P2"), with("P3")), (1, 1), "{labels:?}");
        apart(&labels);

        // A ring link across columns has a lane long enough for a label at each end and the
        // cable between them.
        let mut net = Network {
            links: vec![
                link("-A1:P1", "-B1:P1"),
                link("-B1:P2", "-C1:P1"),
                link("-C1:P2", "-D1:P1"),
                link("-D1:P2", "-E1:P1"),
                link("-E1:P2", "-B1:P3"),
            ],
            ..Network::default()
        };
        for (i, l) in net.links.iter_mut().enumerate() {
            l.cable = format!("-W{}", i + 1);
        }
        let (size, g, topo) = fits_its_sheet(&net);
        assert_eq!(topo.under.len(), 1);
        let (_, labels) = link_drawing(&topo, &g, &net, size.mm().0);
        let bottom = bottom_of(&topo, &g);
        let mut lane: Vec<&str> = labels
            .iter()
            .filter(|l| l.at.1 > bottom)
            .map(|l| l.text.as_str())
            .collect();
        lane.sort_unstable();
        assert_eq!(lane, ["-W4", "P1", "P2"]);
        apart(&labels);
    }

    #[test]
    fn a_ring_link_runs_beside_the_boxes_not_through_them() {
        let (_, g, topo) = fits_its_sheet(&ring());
        assert_eq!(topo.under.len(), 1);
        let bottom = bottom_of(&topo, &g);
        for (lane, &(a, b, _)) in topo.under.iter().enumerate() {
            let (ba, bb) = (&topo.boxes[a], &topo.boxes[b]);
            let path = under_path(&g, (ba.1, ba.2), (bb.1, bb.2), bottom, lane);
            for seg in path.windows(2) {
                for bx in &topo.boxes {
                    let (x, y) = g.spot(bx.1, bx.2);
                    assert!(
                        !crosses(seg[0], seg[1], (x, y), (x + g.box_w, y + g.box_h)),
                        "{seg:?} crosses {}",
                        bx.0
                    );
                }
            }
        }
    }

    #[test]
    fn a_long_line_and_a_wide_switch_get_a_sheet_they_fit() {
        let names: Vec<String> = (1..=30).map(|n| format!("-A{n}")).collect();
        let line = Network {
            nodes: names.iter().map(|n| node(n)).collect(),
            links: names
                .windows(2)
                .map(|p| link(&format!("{}:P2", p[0]), &format!("{}:P1", p[1])))
                .collect(),
            ..Network::default()
        };
        let (size, ..) = fits_its_sheet(&line);
        assert!(size.mm().0 > PageSize::A3.mm().0, "{size:?}");

        // Two loops with long names on a box of that line, its boxes at their narrowest: every
        // port name and cable keeps its first letters.
        let mut looped = line.clone();
        for (a, b) in [("P7", "P8"), ("P9", "P10")] {
            let mut l = link(
                &format!("-A15:{a}-A-VERY-LONG-PORT"),
                &format!("-A15:{b}-A-VERY-LONG-PORT"),
            );
            l.cable = "-W123-A-VERY-LONG-CABLE-TAG".into();
            looped.links.push(l);
        }
        let (size, g, topo) = fits_its_sheet(&looped);
        assert!((g.box_w - MIN_BOX_W).abs() < 1e-9, "{g:?}");
        let (_, labels) = link_drawing(&topo, &g, &looped, size.mm().0);
        let joined: Vec<&Label> = labels.iter().filter(|l| l.text.contains(" - ")).collect();
        assert_eq!(joined.len(), 2, "{labels:?}");
        for l in joined {
            let (ports, cable) = l.text.rsplit_once(' ').unwrap();
            let (from, to) = ports.split_once(" - ").unwrap();
            for (part, start) in [(from, "P"), (to, "P"), (cable, "-W1")] {
                let kept = part.trim_end_matches("...");
                assert!(kept.starts_with(start) && kept.len() >= 2, "{part:?} in {l:?}");
            }
        }
        apart(&labels);

        let star = Network {
            nodes: std::iter::once(node("-K1"))
                .chain((1..=20).map(|n| node(&format!("-T{n}"))))
                .collect(),
            links: (1..=20)
                .map(|n| link(&format!("-K1:P{n}"), &format!("-T{n}:X1")))
                .collect(),
            ..Network::default()
        };
        let (size, ..) = fits_its_sheet(&star);
        assert!(size.mm().1 > PageSize::A3.mm().1, "{size:?}");
    }

    #[test]
    fn a_link_between_two_ports_of_one_device_is_kept() {
        let net = Network {
            links: vec![link("-K10:P1", "-K10:P2")],
            ..Network::default()
        };
        let t = layout_network(&net);
        assert_eq!(t.own, [(0, 0)]);
        assert!(t.tree.is_empty() && t.under.is_empty());

        // Two loops on one box nest apart, each label over its own loop and under the next.
        let two = Network {
            links: vec![link("-K10:P1", "-K10:P2"), link("-K10:P3", "-K10:P4")],
            ..Network::default()
        };
        let (_, g, t) = fits_its_sheet(&two);
        assert_eq!(t.own, [(0, 0), (0, 1)]);
        let (bx, by) = g.spot(0, 0);
        let (inner, inner_label) = own_path(bx, by, g.box_w, 0, 2);
        let (outer, outer_label) = own_path(bx, by, g.box_w, 1, 2);
        assert!(
            outer[0].0 < inner[0].0 && outer[3].0 > inner[3].0,
            "the outer loop is wider"
        );
        assert!(
            outer[1].1 + TEXT < inner_label.1,
            "the inner label is under the outer loop"
        );
        assert!(inner_label.1 < inner[1].1 && outer_label.1 < outer[1].1);
        assert!(outer_label.1 - TEXT > MARGIN + layout::HEADER + 10.0, "below the title");

        // Many loops on one box all end on its top edge, each leg apart from the next.
        let many = Network {
            links: (0..12)
                .map(|i| link(&format!("-K10:P{}", 2 * i + 1), &format!("-K10:P{}", 2 * i + 2)))
                .collect(),
            ..Network::default()
        };
        let (_, g, t) = fits_its_sheet(&many);
        assert_eq!(t.own.len(), 12);
        let (bx, by) = g.spot(0, 0);
        let mut legs: Vec<f64> = Vec::new();
        for nth in 0..12 {
            let (path, _) = own_path(bx, by, g.box_w, nth, 12);
            for p in [path[0], path[3]] {
                assert!(
                    p.0 > bx && p.0 < bx + g.box_w && p.1 == by,
                    "loop {nth} ends on the box: {p:?}"
                );
                legs.push(p.0);
            }
        }
        legs.sort_by(f64::total_cmp);
        assert!(legs.windows(2).all(|w| w[1] - w[0] > 0.1), "{legs:?}");
    }
}

//! Network topology pages: each fieldbus network ([`wc_core::network`]) drawn as boxes for its
//! devices and lines for its links, after the network list.
//!
//! The devices are laid out from the order of the links: the first node (or the first link
//! end) on the left, each device it links to one column further right, so a line runs across
//! the page and a switch fans out into a tree. A link that would close a loop (a ring back to
//! the first device) is drawn once, routed beside the boxes to a lane under them; a link
//! between two ports of one device is a small loop over its box. The sheet grows (a larger
//! ISO size, else a custom one) until every box has room.
//!
//! Each vertical run between the columns gets a track of its own when its span meets another
//! one's, so two cables never share a line and look connected (#486).
//!
//! Device tags and link labels are never cut: one too long for its place is drawn in smaller
//! type, down to [`LABEL_MIN`], and past that it is keyed `[n]` there and written whole in a
//! legend under the drawing (#493).

use std::collections::{BTreeMap, VecDeque};

use wc_core::network::{Network, Port};
use wc_core::{Page, PageSize, Project};

use crate::layout::{self, MARGIN, fit_at, frame};
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
        Err(_) => as_text(s),
    }
}

/// The device a node names: its tag, or the trimmed text when it is not one. A node is a
/// device, never a port, so `-K10:P1` as a node stays its own (warned about) box instead of
/// joining `-K10`.
fn node_key(s: &str) -> String {
    match s.trim().parse::<wc_core::DeviceTag>() {
        Ok(t) => t.to_string(),
        Err(_) => as_text(s),
    }
}

/// Text that is not a tag, trimmed, with each [`BLANK`] doubled: distinct texts stay distinct,
/// and none can be taken for the name of a blank-tag node (one [`BLANK`] then digits).
fn as_text(s: &str) -> String {
    s.trim().replace(BLANK, &format!("{BLANK}{BLANK}"))
}

/// Starts the box name of a node with a blank tag. Tags never contain it and [`as_text`]
/// replaces it in other text, so such a name cannot collide with a tag, a link end or another
/// node.
const BLANK: char = '\u{1}';

/// The box name of node `i` (from 0): its [`node_key`], or [`BLANK`] and its number when its
/// tag is blank, so a declared node keeps a box for its address and station.
fn node_name(i: usize, n: &wc_core::network::Node) -> String {
    let k = node_key(&n.device);
    if k.is_empty() { format!("{BLANK}{}", i + 1) } else { k }
}

/// How a box name is printed: a blank-tag node as `(node N)`.
fn shown(name: &str) -> String {
    match name.strip_prefix(BLANK) {
        Some(n) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => format!("(node {n})"),
        _ => name.replace(&format!("{BLANK}{BLANK}"), &BLANK.to_string()),
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
    for (i, n) in net.nodes.iter().enumerate() {
        add(node_name(i, n));
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
/// Smallest type an address is shrunk to so that it fits its box whole, in mm.
const ADDRESS_MIN: f64 = 1.6;
/// Smallest type a device tag or link label is shrunk to before it is keyed to the legend.
const LABEL_MIN: f64 = 1.6;
/// The type of a device tag in its box.
const TAG_TEXT: f64 = TEXT + 0.4;
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
/// Room between two vertical runs side by side in one gap, at most.
const TRACK: f64 = 1.5;
/// How close two runs may come end to end on one track.
const TRACK_CLEAR: f64 = 1.0;
/// How far above its middle a box's tree links leave it when a branch of another box runs
/// into the next column at the height of its middle.
const EXIT_SHIFT: f64 = 2.5;

/// Where the boxes go on a sheet: the top left of column 0, row 0, the steps between columns
/// and rows, the size of a box, and how far each row is pushed down by the loops over the rows
/// up to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub x0: f64,
    pub y0: f64,
    pub step_x: f64,
    pub step_y: f64,
    pub box_w: f64,
    pub box_h: f64,
    /// For each row, the loop room of that row and every row above it ([`loop_rooms`]).
    pub loops_above: Vec<f64>,
}

impl Grid {
    /// The top left corner of the box in column `c`, row `r`.
    pub fn spot(&self, c: usize, r: usize) -> (f64, f64) {
        let loops = self
            .loops_above
            .get(r)
            .or(self.loops_above.last())
            .copied()
            .unwrap_or(0.0);
        (
            self.x0 + c as f64 * self.step_x,
            self.y0 + r as f64 * self.step_y + loops,
        )
    }

    /// The room left of the boxes of column `c` that vertical runs use: the x of its first
    /// track, and the least x a track may take. The first track is the middle of the gap
    /// between two columns; the labels of the links into column `c` start just right of it,
    /// and further tracks step left from it, so no run crosses a label. Left of column 0
    /// the tracks keep clear of the frame.
    fn gap(&self, c: usize) -> (f64, f64) {
        let x = self.x0 + c as f64 * self.step_x;
        if c == 0 {
            ((x - 2.0).max(MARGIN + 2.0), MARGIN + 2.0)
        } else {
            let prev = x - self.step_x + self.box_w;
            ((prev + x) / 2.0, prev + 1.5)
        }
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

/// The room over each row that its boxes' same-device loops take beyond the first: one line
/// for each further loop of the box in that row with the most (#492). A row without loops
/// takes none.
fn loop_rooms(topo: &Topology) -> Vec<f64> {
    let (_, rows) = extent(topo);
    let mut room = vec![0.0f64; rows];
    for (b, (_, _, r)) in topo.boxes.iter().enumerate() {
        let loops = topo.own.iter().filter(|o| o.0 == b).count();
        if let Some(row) = room.get_mut(*r) {
            *row = row.max(loops.saturating_sub(1) as f64 * LINE);
        }
    }
    room
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

/// The height of a legend of `keys` lines under the lanes.
fn legend_height(keys: usize) -> f64 {
    if keys == 0 { 0.0 } else { keys as f64 * LINE + 2.0 }
}

/// The space `topo` needs on a sheet, in millimetres, with a title block `band` high and a
/// legend of `keys` lines.
fn needed(topo: &Topology, band: f64, keys: usize) -> (f64, f64) {
    let (cols, rows) = extent(topo);
    let w = 2.0 * (MARGIN + 10.0) + cols as f64 * (MIN_BOX_W + GAP_X);
    let lanes = (topo.under.len() + 1) as f64 * LANE;
    let loops: f64 = loop_rooms(topo).iter().sum();
    let h = top() + rows as f64 * (BOX_H + GAP_Y) + loops + lanes + legend_height(keys) + 6.0 + band + MARGIN;
    (w, h)
}

/// The legend `net` laid out as `topo` gets on a sheet `w` mm wide. The room of every tag and
/// label depends only on the width of the sheet, so this is the legend the page draws.
fn legend_for(topo: &Topology, net: &Network, w: f64) -> Legend {
    let (_, h) = needed(topo, 0.0, 0);
    let g = grid(topo, w, h, 0.0, 0);
    let mut legend = Legend::default();
    for b in &topo.boxes {
        tag_label(&b.0, &g, &mut legend);
    }
    link_drawing(topo, &g, net, w, &mut legend);
    legend
}

/// The sheet a network is drawn on: `wanted` (the size of the first schematic page) when the
/// layout fits it, else the first ISO size that fits, else a custom sheet. A layout too large
/// even for the largest custom sheet gets that, with a warning.
pub fn sheet(wanted: PageSize, band: f64, net: &Network) -> (PageSize, Option<String>) {
    let topo = layout_network(net);
    let (w, _) = needed(&topo, band, 0);
    // The legend, and so the height, depends on how wide the sheet is.
    let height = |sw: f64| needed(&topo, band, legend_for(&topo, net, sw).0.len()).1;
    let fits = |s: &PageSize| {
        let (sw, sh) = s.mm();
        sw >= w && sh >= height(sw)
    };
    if let Some(s) = std::iter::once(wanted).chain(PageSize::NAMED).find(fits) {
        return (s, None);
    }
    let (lo, hi) = (f64::from(PageSize::MIN_MM), f64::from(PageSize::MAX_MM));
    let side = |v: f64| v.ceil().clamp(lo, hi) as u32;
    let h = height(f64::from(side(w)));
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

/// The grid for `topo` on a sheet `w` by `h` mm with a title block `band` high and a legend
/// of `keys` lines: columns and rows spread over the room there is, never closer than a box
/// and its labels need.
pub fn grid(topo: &Topology, w: f64, h: f64, band: f64, keys: usize) -> Grid {
    let (cols, rows) = extent(topo);
    let (x0, x1) = (MARGIN + 10.0, w - MARGIN - 10.0);
    // Each row is pushed down by the loops over it and over the rows above: the sheet's
    // height (`needed`) counts the same rooms.
    let loops_above: Vec<f64> = loop_rooms(topo)
        .into_iter()
        .scan(0.0, |sum, room| {
            *sum += room;
            Some(*sum)
        })
        .collect();
    let loops = loops_above.last().copied().unwrap_or(0.0);
    let (y0, y1) = (top(), h - MARGIN - band - 6.0);
    let lanes = (topo.under.len() + 1) as f64 * LANE + legend_height(keys);
    let step_x = ((x1 - x0) / cols as f64).clamp(MIN_BOX_W + GAP_X, 88.0);
    let box_w = (step_x - GAP_X).clamp(MIN_BOX_W, 48.0);
    let low = BOX_H + GAP_Y;
    let step_y = ((y1 - y0 - lanes - loops) / rows as f64).clamp(low, low + 8.0);
    Grid {
        x0,
        y0,
        step_x,
        step_y,
        box_w,
        box_h: BOX_H,
        loops_above,
    }
}

/// The height of lane `lane` under the boxes, the lowest edge of which is `bottom`.
fn lane_y(bottom: f64, lane: usize) -> f64 {
    bottom + LANE * (lane as f64 + 1.0)
}

/// Where the under links of a box whose top is at `y` leave its left side.
fn attach(g: &Grid, y: f64) -> f64 {
    y + g.box_h - 1.5
}

/// The path of a link drawn in lane `lane` under the boxes, from the box at `a` to the one
/// at `b` (column, row): out of the lower left of each box to its run at `xa` and `xb` in
/// the room left of its column ([`routes`]), down that run to the lane, and along it.
pub fn under_path(
    g: &Grid,
    a: (usize, usize),
    b: (usize, usize),
    bottom: f64,
    lane: usize,
    (xa, xb): (f64, f64),
) -> Vec<(f64, f64)> {
    let (ax, ay) = g.spot(a.0, a.1);
    let (bx, by) = g.spot(b.0, b.1);
    let y = lane_y(bottom, lane);
    let (sa, sb) = (attach(g, ay), attach(g, by));
    vec![(ax, sa), (xa, sa), (xa, y), (xb, y), (xb, sb), (bx, sb)]
}

/// A vertical run in the room left of a column: the one the tree links out of a box share,
/// or the one that takes a box's under links down to their lanes (box indexes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Run {
    Tree(usize),
    Under(usize),
}

/// Where the links of a layout run between the columns.
#[derive(Debug, Clone, Default, PartialEq)]
struct Routes {
    /// The x of each run.
    x: BTreeMap<Run, f64>,
    /// The height at which the tree links of a box (by index) leave its right side.
    exit: BTreeMap<usize, f64>,
}

/// The tracks of every vertical run of `topo` on grid `g`, with lanes under `bottom` (#486).
///
/// Runs in one gap whose spans meet (within [`TRACK_CLEAR`]) get separate tracks, the one
/// that starts highest nearest the middle; runs whose spans are apart may share one. The
/// tree links of one box share a run as one bus, and so do the under links of one box. A
/// box's tree links leave it a little higher when a branch of another box runs into the
/// next column at the height of its middle, so the two do not read as one line.
fn routes(topo: &Topology, g: &Grid, bottom: f64) -> Routes {
    let place = |b: usize| topo.boxes.get(b).map(|bx| (bx.1, bx.2, g.spot(bx.1, bx.2).1));
    let centre = |y: f64| y + g.box_h / 2.0;
    let mut routes = Routes::default();
    for &(a, _, _) in &topo.tree {
        let Some((c, r, y)) = place(a) else { continue };
        let crossed = topo
            .tree
            .iter()
            .any(|&(s, t, _)| s != a && place(t).is_some_and(|(tc, tr, _)| tc == c + 1 && tr == r));
        let shift = if crossed { EXIT_SHIFT } else { 0.0 };
        routes.exit.insert(a, centre(y) - shift);
    }
    // Each run's gap (by the column right of it) and the top and bottom of its span.
    let mut spans: BTreeMap<Run, (usize, f64, f64)> = BTreeMap::new();
    let mut add = |run: Run, gap: usize, y: f64| {
        let e = spans.entry(run).or_insert((gap, y, y));
        e.1 = e.1.min(y);
        e.2 = e.2.max(y);
    };
    for &(a, b, _) in &topo.tree {
        let (Some(&ey), Some((tc, _, ty))) = (routes.exit.get(&a), place(b)) else {
            continue;
        };
        add(Run::Tree(a), tc, ey);
        add(Run::Tree(a), tc, centre(ty));
    }
    for (lane, &(a, b, _)) in topo.under.iter().enumerate() {
        for e in [a, b] {
            let Some((c, _, y)) = place(e) else { continue };
            add(Run::Under(e), c, attach(g, y));
            add(Run::Under(e), c, lane_y(bottom, lane));
        }
    }
    let mut by_gap: BTreeMap<usize, Vec<(f64, f64, Run)>> = BTreeMap::new();
    for (run, (gap, lo, hi)) in spans {
        by_gap.entry(gap).or_default().push((lo, hi, run));
    }
    for (gap, mut runs) in by_gap {
        // First fit by top: the fewest tracks for spans on a line.
        runs.sort_by(|p, q| p.0.total_cmp(&q.0).then(p.2.cmp(&q.2)));
        let mut ends: Vec<f64> = Vec::new();
        let mut tracks = Vec::with_capacity(runs.len());
        for &(lo, hi, run) in &runs {
            let t = match ends.iter().position(|&end| end + TRACK_CLEAR < lo) {
                Some(t) => t,
                None => {
                    ends.push(hi);
                    ends.len() - 1
                }
            };
            if let Some(end) = ends.get_mut(t) {
                *end = hi;
            }
            tracks.push((run, t));
        }
        let (first, least) = g.gap(gap);
        // Closer together when there are more than the gap holds at the full step.
        let step = match ends.len() {
            0 | 1 => TRACK,
            n => TRACK.min((first - least).max(0.0) / (n - 1) as f64),
        };
        for (run, t) in tracks {
            routes.x.insert(run, first - step * t as f64);
        }
    }
    routes
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

/// The path of a tree link from the box at `a` to the box at `b` (top left corners), leaving
/// `a` at height `exit` and running down the track at `x`, and where its `from` port, `to`
/// port and cable labels go. Links fanning out of one box share its run, so the labels sit
/// on the link's own branch at the height of `b`, right of every track of the gap: the port
/// names over it, one at each end, and the cable under it.
fn tree_path(g: &Grid, a: Pt, b: Pt, exit: f64, x: f64) -> (Vec<Pt>, [Pt; 3]) {
    let start = (a.0 + g.box_w, exit);
    let end = (b.0, b.1 + g.box_h / 2.0);
    let mid = (start.0 + end.0) / 2.0;
    let path = vec![start, (x, start.1), (x, end.1), end];
    let labels = [
        (mid + 1.0, end.1 - 1.0),
        (end.0 - 1.0, end.1 - 1.0),
        (mid + 1.0, end.1 + TEXT + 0.6),
    ];
    (path, labels)
}

/// The type `s` is drawn in to fit `room` mm whole: `size`, else smaller down to
/// [`LABEL_MIN`]; `None` when it does not fit even then.
fn whole_size(s: &str, room: f64, size: f64) -> Option<f64> {
    let need = crate::writer::text_width_mm(s, size);
    if need <= room {
        return Some(size);
    }
    // Widths scale with the type; a hair under, so rounding never makes it overrun.
    let fit = size * room / need * 0.999;
    (fit >= LABEL_MIN).then_some(fit)
}

/// The names too long for their place even in the smallest type, in the order they were
/// keyed: each is keyed `[n]` where it belongs (n from 1) and written whole under the drawing.
/// A name keyed twice keeps its first key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Legend(pub Vec<String>);

impl Legend {
    /// The key of `s`, adding it when it is new.
    fn key(&mut self, s: &str) -> String {
        let n = match self.0.iter().position(|x| x == s) {
            Some(i) => i + 1,
            None => {
                self.0.push(s.to_owned());
                self.0.len()
            }
        };
        format!("[{n}]")
    }
}

/// `s` as drawn in `room` mm at most: whole, in smaller type if it must be, else its key in
/// `legend`. Returns the text and its type size.
fn fitted(s: &str, room: f64, size: f64, legend: &mut Legend) -> (String, f64) {
    let room = room.max(0.0);
    match whole_size(s, room, size) {
        Some(size) => (s.to_owned(), size),
        None => {
            let key = legend.key(s);
            let size = whole_size(&key, room, size).unwrap_or(LABEL_MIN);
            (key, size)
        }
    }
}

/// A device's tag as its box `g.box_w` wide shows it ([`fitted`]), and its type size.
fn tag_label(name: &str, g: &Grid, legend: &mut Legend) -> (String, f64) {
    fitted(&shown(name), g.box_w - 3.0, TAG_TEXT, legend)
}

/// A link label: where it goes, how it is aligned there (0 left, 1 centred, 2 right, as for
/// [`Pen::text`]), its text and its type size. The text is whole, or its key in the legend.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub at: Pt,
    pub align: u8,
    pub text: String,
    pub size: f64,
}

impl Label {
    fn new(at: Pt, align: u8, s: &str, room: f64, legend: &mut Legend) -> Self {
        let (text, size) = fitted(s.trim(), room, TEXT, legend);
        Label { at, align, text, size }
    }

    /// Its left and right ends on the sheet.
    #[cfg(test)]
    fn span(&self) -> (f64, f64) {
        let tw = crate::writer::text_width_mm(&self.text, self.size);
        match self.align {
            0 => (self.at.0, self.at.0 + tw),
            1 => (self.at.0 - tw / 2.0, self.at.0 + tw / 2.0),
            _ => (self.at.0 - tw, self.at.0),
        }
    }
}

/// A link's port names and cable on one line, `P1 - P2 -W1`.
fn joined(from: &str, to: &str, cable: &str) -> String {
    format!("{} - {} {}", from.trim(), to.trim(), cable.trim())
        .trim()
        .to_owned()
}

/// The lines of every link of `net` laid out as `topo` on grid `g`, on a sheet `w` mm wide,
/// and their port and cable labels, each fitted to the room beside its line; what does not fit
/// is keyed in `legend`.
pub fn link_drawing(
    topo: &Topology,
    g: &Grid,
    net: &Network,
    w: f64,
    legend: &mut Legend,
) -> (Vec<Vec<Pt>>, Vec<Label>) {
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
    let bottom = bottom_of(topo, g);
    let routes = routes(topo, g, bottom);
    let track = |run: Run| routes.x.get(&run).copied();
    for &(a, b, k) in &topo.tree {
        let (Some(pa), Some(qa), Some((from_port, to_port, cable))) = (at(a), at(b), ends(a, k)) else {
            continue;
        };
        let (Some(&exit), Some(x)) = (routes.exit.get(&a), track(Run::Tree(a))) else {
            continue;
        };
        let (path, [from_at, to_at, cable_at]) = tree_path(g, pa, qa, exit, x);
        // The two port names share the branch from the junction to the box.
        let branch = to_at.0 + 1.0 - from_at.0 + 1.0;
        let half = branch / 2.0 - 1.5;
        labels.push(Label::new(from_at, 0, &from_port, half, legend));
        labels.push(Label::new(to_at, 2, &to_port, half, legend));
        labels.push(Label::new(cable_at, 0, &cable, branch - 2.0, legend));
        paths.push(path);
    }
    let (left, right) = (MARGIN + 1.0, w - MARGIN - 1.0);
    for (lane, &(a, b, k)) in topo.under.iter().enumerate() {
        let (Some(ba), Some(bb), Some((from_port, to_port, cable))) =
            (topo.boxes.get(a), topo.boxes.get(b), ends(a, k))
        else {
            continue;
        };
        let (Some(xa), Some(xb)) = (track(Run::Under(a)), track(Run::Under(b))) else {
            continue;
        };
        let path = under_path(g, (ba.1, ba.2), (bb.1, bb.2), bottom, lane, (xa, xb));
        // The labels go over the link's own lane, so links that leave one box apart from
        // each other (and share the run beside it) are told apart by their lanes.
        if let (Some(&(lx, ly)), Some(&(rx, _))) = (path.get(2), path.get(3)) {
            let (len, y) = ((rx - lx).abs(), ly - 0.8);
            if len >= LONG_LANE {
                // Each port name at its own end of the lane, the cable between them.
                let (from_align, to_align, dir) = if lx <= rx { (0, 2, 1.0) } else { (2, 0, -1.0) };
                let quarter = len / 4.0 - 1.0;
                labels.push(Label::new((lx + dir, y), from_align, &from_port, quarter, legend));
                labels.push(Label::new((rx - dir, y), to_align, &to_port, quarter, legend));
                labels.push(Label::new(((lx + rx) / 2.0, y), 1, &cable, len / 2.0 - 2.0, legend));
            } else {
                // A short lane: one label over it, as wide as a gap.
                let c = (lx + rx) / 2.0;
                let half = (g.step_x - g.box_w) / 2.0 - 1.0;
                let half = half.min(c - left).min(right - c);
                let label = joined(&from_port, &to_port, &cable);
                labels.push(Label::new((c, y), 1, &label, 2.0 * half, legend));
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
        // into half of the gaps either side.
        let nth = topo.own.iter().take(n).filter(|o| o.0 == a).count();
        let count = topo.own.iter().filter(|o| o.0 == a).count();
        let (path, label_at) = own_path(bx, by, g.box_w, nth, count);
        let c = label_at.0;
        let room = 2.0 * (g.step_x / 2.0 - 1.0).min(c - left).min(right - c);
        let label = joined(&from_port, &to_port, &cable);
        labels.push(Label::new(label_at, 1, &label, room, legend));
        paths.push(path.to_vec());
    }
    (paths, labels)
}

/// One topology page for `net`: frame and title block, a box per device with its tag,
/// description, address and station, and each link once with its port names and cable tag.
/// A box links to the first page its device is drawn on. A node or link end that does not
/// read as a device tag is still drawn as written, and why is added to `warnings`.
#[allow(clippy::too_many_arguments)]
pub fn draw_network_page(
    project: &Project,
    page: &Page,
    index: usize,
    total: usize,
    net: &Network,
    date: &str,
    exported: &std::collections::BTreeSet<&str>,
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
    pen.text((x0, title_y), 4.0, 0, &fit_at(&title, x1 - x0, 4.0));

    let mut warn = |why: String| {
        let w = format!("network {}: {why}", net.name);
        if !warnings.contains(&w) {
            warnings.push(w);
        }
    };
    // Warnings name nodes and links by their number, not by text that failed to parse: the
    // app writes them to its local log, and unparsed text can be any drawing text.
    for (i, n) in net.nodes.iter().enumerate() {
        if let Some(why) = tag_reason(&n.device) {
            warn(format!("node {} is not a device tag: {why}", i + 1));
        }
    }
    for (k, l) in net.links.iter().enumerate() {
        for (side, end) in [("from", &l.from), ("to", &l.to)] {
            if Port::parse(end).is_err() {
                // The tag part, split as `Port::parse` splits it.
                let tag = end.trim().rsplit_once(':').map_or(end.trim(), |(tag, _)| tag);
                let why = tag_reason(tag).unwrap_or("not a device tag");
                warn(format!("link {}: its {side} end is not a device or port: {why}", k + 1));
            }
        }
    }
    let topo = layout_network(net);
    for &k in &topo.unplaced {
        if k < net.links.len() {
            warn(format!("link {} is not drawn: an end names no device", k + 1));
        }
    }
    let keys = legend_for(&topo, net, w).0.len();
    let g = grid(&topo, w, h, band, keys);
    let mut legend = Legend::default();

    let tags = project.tag_index();
    let mut links = Vec::new();
    pen.width(0.25);
    for (name, c, r) in &topo.boxes {
        let (bx, by) = g.spot(*c, *r);
        pen.rect((bx, by), (bx + g.box_w, by + g.box_h));
        let tag = name.parse::<wc_core::DeviceTag>().ok();
        // Found by the key the box was laid out under, so a node whose tag is not valid
        // still shows its address and station.
        let node = net
            .nodes
            .iter()
            .enumerate()
            .find(|(i, n)| node_name(*i, n) == *name)
            .map(|(_, n)| n);
        let description = tag
            .as_ref()
            .and_then(|t| project.devices.get(t))
            .map(|d| d.description.trim().to_owned())
            .unwrap_or_default();
        let lines = [
            shown(name),
            description,
            node.map(|n| n.address.trim().to_owned()).unwrap_or_default(),
            node.map(|n| n.station.trim().to_owned()).unwrap_or_default(),
        ];
        for (i, t) in lines.iter().enumerate() {
            if i == 0 {
                // A tag is only useful whole, so it is never cut.
                let (text, size) = tag_label(name, &g, &mut legend);
                pen.text((bx + 1.5, by + 1.0 + LINE), size, 0, &text);
                continue;
            }
            // An address is only useful whole: smaller type before it is cut.
            let size = if i == 2 {
                let room = g.box_w - 3.0;
                let mut s = TEXT;
                while s > ADDRESS_MIN && crate::writer::text_width_mm(t, s) > room {
                    s = (s - 0.05).max(ADDRESS_MIN);
                }
                s
            } else {
                TEXT
            };
            pen.text(
                (bx + 1.5, by + 1.0 + LINE * (i as f64 + 1.0)),
                size,
                0,
                &fit_at(t, g.box_w - 3.0, size),
            );
        }
        // The first place the device is drawn on a page this PDF holds.
        let target = tag
            .as_ref()
            .and_then(|t| tags.get(t))
            .and_then(|at| at.iter().find(|a| exported.contains(a.0.as_str())));
        if let Some(target) = target {
            links.push(Link {
                min: (bx, by),
                max: (bx + g.box_w, by + g.box_h),
                target_page_id: target.0.clone(),
            });
        }
    }
    let (paths, labels) = link_drawing(&topo, &g, net, w, &mut legend);
    for path in &paths {
        pen.polyline(path, false);
    }
    for l in &labels {
        pen.text(l.at, l.size, l.align, &l.text);
    }
    // The names keyed on the drawing, whole, under the lanes.
    let start = bottom_of(&topo, &g) + (topo.under.len() + 1) as f64 * LANE;
    for (i, name) in legend.0.iter().enumerate() {
        let line = format!("[{}] {name}", i + 1);
        pen.text(
            (x0, start + LINE * (i as f64 + 1.0)),
            TEXT,
            0,
            &fit_at(&line, x1 - x0, TEXT),
        );
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

/// Why `text` is not a device tag, as a fixed reason that never quotes the text; `None` when it
/// is one.
fn tag_reason(text: &str) -> Option<&'static str> {
    match text.trim().parse::<wc_core::DeviceTag>() {
        Ok(_) => None,
        Err(wc_core::Error::Tag(_, why)) => Some(why),
        Err(_) => Some("not a device tag"),
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
        let g = grid(&topo, w, h, band, legend_for(&topo, net, w).0.len());
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
        let g = grid(&t, 420.0, 297.0, 30.0, 0);
        let spot = |i: usize| g.spot(t.boxes[i].1, t.boxes[i].2);
        let r = routes(&t, &g, bottom_of(&t, &g));
        let from_labels: Vec<(f64, f64)> = t
            .tree
            .iter()
            .filter(|e| t.boxes[e.0].0 == "-K10")
            .map(|&(a, b, _)| {
                let (path, labels) = tree_path(&g, spot(a), spot(b), r.exit[&a], r.x[&Run::Tree(a)]);
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
    fn a_node_named_like_a_port_stays_its_own_box() {
        let net = Network {
            nodes: vec![node("-K10:P1")],
            links: vec![link("-A1:P1", "-K10:P2")],
            ..Network::default()
        };
        let t = layout_network(&net);
        let names: Vec<&str> = t.boxes.iter().map(|b| b.0.as_str()).collect();
        assert_eq!(names, ["-K10:P1", "-A1", "-K10"]);
    }

    #[test]
    fn every_ring_link_is_stroked_not_only_labelled() {
        let net = ring();
        let t = layout_network(&net);
        let g = grid(&t, 420.0, 297.0, 30.0, 0);
        let (paths, _) = link_drawing(&t, &g, &net, 420.0, &mut Legend::default());
        assert_eq!(paths.len(), 3, "{paths:?}");
        for p in &paths {
            assert!(p.len() >= 2, "{p:?}");
            let length: f64 = p.windows(2).map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1)).sum();
            assert!(length > 1.0, "{p:?}");
        }
        // The page draws each of them: three open strokes besides the boxes and the frame.
        let mut project = Project::new("x");
        project.pages.clear();
        let page = Page::new("network-n", "N1", "Network");
        let mut warnings = Vec::new();
        let drawn = draw_network_page(&project, &page, 0, 1, &net, "", &Default::default(), &mut warnings);
        let strokes = drawn.ops.matches(" l\nS\n").count();
        assert!(strokes >= 3, "{strokes} open strokes");
        let plain = Network {
            links: Vec::new(),
            ..net.clone()
        };
        let none = draw_network_page(&project, &page, 0, 1, &plain, "", &Default::default(), &mut warnings);
        assert_eq!(strokes - none.ops.matches(" l\nS\n").count(), 3, "one stroke per link");
    }

    #[test]
    fn an_address_is_shrunk_to_fit_its_box_whole() {
        let names: Vec<String> = (1..=30).map(|n| format!("-A{n}")).collect();
        let mut net = Network {
            nodes: names.iter().map(|n| node(n)).collect(),
            links: names
                .windows(2)
                .map(|p| link(&format!("{}:P2", p[0]), &format!("{}:P1", p[1])))
                .collect(),
            ..Network::default()
        };
        net.nodes[0].address = "192.168.100.100".into();
        net.nodes[1].address = "192.168.100.101".into();
        let (size, g, _) = fits_its_sheet(&net);
        assert!((g.box_w - MIN_BOX_W).abs() < 1e-9, "{g:?}");
        let mut page = Page::new("network-n", "N1", "Network");
        page.size = size;
        let mut warnings = Vec::new();
        let drawn = draw_network_page(
            &Project::new("x"),
            &page,
            0,
            1,
            &net,
            "",
            &Default::default(),
            &mut warnings,
        );
        for a in ["192.168.100.100", "192.168.100.101"] {
            assert!(drawn.ops.contains(&crate::writer::pdf_text(a)), "{a} is drawn whole");
        }
    }

    #[test]
    fn long_port_and_cable_names_are_keyed_to_a_legend_not_cut() {
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
        let mut legend = Legend::default();
        let (_, labels) = link_drawing(&topo, &g, &net, w, &mut legend);
        // Three tree links with three labels each; the ring link joins two devices of one
        // column, so its short lane has one label; and a label per loop.
        assert_eq!(labels.len(), 3 * 3 + 1 + 2);
        // Every label is a key into the legend, which holds each name once and whole: the
        // port names, the cable, and the three lines of both ports and the cable.
        for l in &labels {
            let n: usize = l.text.trim_start_matches('[').trim_end_matches(']').parse().unwrap();
            assert!(n >= 1 && n <= legend.0.len(), "{l:?}");
        }
        for name in [long("X1"), long("P1"), long("P2"), long("X2")] {
            assert_eq!(legend.0.iter().filter(|e| **e == name).count(), 1, "{legend:?}");
        }
        assert!(
            legend
                .0
                .contains(&"-W1-A-CABLE-TAG-FAR-TOO-LONG-FOR-ANY-GAP".to_owned())
        );
        let lines = legend.0.iter().filter(|e| e.contains(" - ")).count();
        assert_eq!(lines, 3, "{legend:?}");
        assert!(legend.0.iter().all(|e| !e.ends_with("...")));
        for l in &labels {
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
        let (_, labels) = link_drawing(&topo, &g, &net, size.mm().0, &mut Legend::default());
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
        let (_, labels) = link_drawing(&topo, &g, &net, size.mm().0, &mut Legend::default());
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
        let r = routes(&topo, &g, bottom);
        for (lane, &(a, b, _)) in topo.under.iter().enumerate() {
            let (ba, bb) = (&topo.boxes[a], &topo.boxes[b]);
            let xs = (r.x[&Run::Under(a)], r.x[&Run::Under(b)]);
            let path = under_path(&g, (ba.1, ba.2), (bb.1, bb.2), bottom, lane, xs);
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
        let mut legend = Legend::default();
        let (_, labels) = link_drawing(&topo, &g, &looped, size.mm().0, &mut legend);
        let keyed: Vec<&Label> = labels.iter().filter(|l| l.text.starts_with('[')).collect();
        assert_eq!(keyed.len(), 2, "{labels:?}");
        assert_eq!(
            legend.0,
            [
                "P7-A-VERY-LONG-PORT - P8-A-VERY-LONG-PORT -W123-A-VERY-LONG-CABLE-TAG",
                "P9-A-VERY-LONG-PORT - P10-A-VERY-LONG-PORT -W123-A-VERY-LONG-CABLE-TAG"
            ]
        );
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

    /// One looped child among many on a switch: only its row makes room for its loops, so the
    /// sheet is no taller than the rows and that one row's loops need, and the loops still
    /// clear the box above (#492).
    #[test]
    fn only_the_row_with_loops_makes_room_for_them() {
        let mut links: Vec<NetLink> = (0..40)
            .map(|i| link(&format!("-Q1:X1P{}", i + 1), &format!("-A{}:X1", i + 1)))
            .collect();
        links.extend((0..10).map(|i| link(&format!("-A7:P{}", 2 * i + 1), &format!("-A7:P{}", 2 * i + 2))));
        let net = Network {
            links,
            ..Network::default()
        };
        let band = 30.0;
        let topo = layout_network(&net);
        let (_, rows) = extent(&topo);
        let rooms = loop_rooms(&topo);
        assert_eq!(rooms.iter().filter(|r| **r > 0.0).count(), 1, "{rooms:?}");
        let loops: f64 = rooms.iter().sum();
        assert_eq!(loops, 9.0 * LINE);
        let (_, h) = needed(&topo, band, 0);
        let per_row = top() + rows as f64 * (BOX_H + GAP_Y) + loops;
        assert!(h <= per_row + (topo.under.len() + 1) as f64 * LANE + 6.0 + band + MARGIN + 1e-9);
        assert!(h < f64::from(PageSize::MAX_MM), "{h}");
        // Fits a sheet without a warning, every box apart and above the title block.
        let (_, g, topo) = fits_its_sheet(&net);
        let Some(&(_, c, r)) = topo.boxes.iter().find(|b| b.0 == "-A7") else {
            unreachable!("-A7 has a box")
        };
        let (bx, by) = g.spot(c, r);
        let (_, label) = own_path(bx, by, g.box_w, 9, 10);
        // The outermost loop's label clears every box in the rows above it.
        for b in topo.boxes.iter().filter(|b| b.2 < r) {
            let (_, y) = g.spot(b.1, b.2);
            assert!(y + g.box_h < label.1 - TEXT, "{} overlaps the loops of -A7", b.0);
        }
        // A row without loops keeps the plain step.
        let (_, y0) = g.spot(c, r - 2);
        let (_, y1) = g.spot(c, r - 1);
        assert!((y1 - y0 - g.step_y).abs() < 1e-9);
    }

    /// A link's path, with the two devices (box indexes) it joins.
    type LinkPath = ((usize, usize), Vec<Pt>);

    /// Each link of `net` drawn on its sheet, and the labels.
    fn drawn_links(net: &Network) -> (Grid, Topology, Vec<LinkPath>, Vec<Label>) {
        let (size, g, topo) = fits_its_sheet(net);
        let (paths, labels) = link_drawing(&topo, &g, net, size.mm().0, &mut Legend::default());
        let owners = topo
            .tree
            .iter()
            .chain(&topo.under)
            .map(|e| (e.0, e.1))
            .chain(topo.own.iter().map(|o| (o.0, o.0)));
        let links = owners.zip(paths).collect();
        (g, topo, links, labels)
    }

    /// Whether two axis-aligned segments lie on one line and overlap or touch there.
    fn on_top(p: Pt, q: Pt, r: Pt, s: Pt) -> bool {
        let eps = 1e-6;
        let meet =
            |a0: f64, a1: f64, b0: f64, b1: f64| a0.min(a1) <= b0.max(b1) + eps && b0.min(b1) <= a0.max(a1) + eps;
        let vertical = |a: Pt, b: Pt| (a.0 - b.0).abs() < eps;
        let horizontal = |a: Pt, b: Pt| (a.1 - b.1).abs() < eps;
        if vertical(p, q) && vertical(r, s) && (p.0 - r.0).abs() < eps {
            meet(p.1, q.1, r.1, s.1)
        } else if horizontal(p, q) && horizontal(r, s) && (p.1 - r.1).abs() < eps {
            meet(p.0, q.0, r.0, s.0)
        } else {
            false
        }
    }

    /// No two links that do not share a device have a segment on top of each other's, and
    /// no link runs through a box. Links of one device may share its run, as a bus.
    fn kept_apart(net: &Network) {
        let (g, topo, links, labels) = drawn_links(net);
        for (i, (ea, pa)) in links.iter().enumerate() {
            for seg in pa.windows(2) {
                for bx in &topo.boxes {
                    let (x, y) = g.spot(bx.1, bx.2);
                    assert!(
                        !crosses(seg[0], seg[1], (x, y), (x + g.box_w, y + g.box_h)),
                        "{seg:?} crosses {}",
                        bx.0
                    );
                }
            }
            for (eb, pb) in links.iter().skip(i + 1) {
                let shared = [ea.0, ea.1].iter().any(|d| *d == eb.0 || *d == eb.1);
                if shared {
                    continue;
                }
                for s in pa.windows(2) {
                    for t in pb.windows(2) {
                        let (na, nb) = (&topo.boxes[ea.0].0, &topo.boxes[eb.0].0);
                        assert!(
                            !on_top(s[0], s[1], t[0], t[1]),
                            "the link from {na} ({s:?}) lies on the one from {nb} ({t:?})"
                        );
                    }
                }
            }
        }
        apart(&labels);
    }

    #[test]
    fn tree_links_of_two_boxes_in_one_column_get_their_own_tracks() {
        // B and C are in one column; B-F runs over rows 0-2 and C-G over rows 1-3 (#486).
        let net = Network {
            links: vec![
                link("-A:P1", "-B:P1"),
                link("-A:P2", "-C:P1"),
                link("-B:P2", "-D:P1"),
                link("-B:P3", "-E:P1"),
                link("-B:P4", "-F:P1"),
                link("-C:P2", "-G:P1"),
            ],
            ..Network::default()
        };
        let topo = layout_network(&net);
        let at = |n: &str| topo.boxes.iter().find(|b| b.0 == n).map(|b| (b.1, b.2)).unwrap();
        assert_eq!(
            (at("-B"), at("-C"), at("-F"), at("-G")),
            ((1, 0), (1, 1), (2, 2), (2, 3))
        );
        kept_apart(&net);
        // C leaves above its middle: the branch of B into E runs at that height.
        let (g, topo, links, _) = drawn_links(&net);
        let c = topo.boxes.iter().position(|b| b.0 == "-C").unwrap();
        let (_, cy) = g.spot(1, 1);
        let out_of_c = links.iter().find(|(e, _)| e.0 == c).unwrap();
        assert!((out_of_c.1[0].1 - (cy + g.box_h / 2.0 - EXIT_SHIFT)).abs() < 1e-9);
        // A plain line keeps its links straight through the middle of the boxes.
        let line = Network {
            links: vec![link("-A:P1", "-B:P1"), link("-B:P2", "-C:P1")],
            ..Network::default()
        };
        let (_, _, links, _) = drawn_links(&line);
        for (_, p) in links {
            assert!(p.iter().all(|q| (q.1 - p[0].1).abs() < 1e-9), "{p:?}");
        }
    }

    #[test]
    fn ring_approaches_and_tree_branches_get_their_own_tracks() {
        // A to B, C, D and E, then B-D and C-E: the approaches of the two ring links overlap
        // beside the boxes (#486).
        let fan = |extra: &[(&str, &str)]| Network {
            links: [
                ("-A:P1", "-B:P1"),
                ("-A:P2", "-C:P1"),
                ("-A:P3", "-D:P1"),
                ("-A:P4", "-E:P1"),
            ]
            .iter()
            .chain(extra)
            .map(|(a, b)| link(a, b))
            .collect(),
            ..Network::default()
        };
        let net = fan(&[("-B:P2", "-D:P2"), ("-C:P2", "-E:P2")]);
        let topo = layout_network(&net);
        assert_eq!(topo.under.len(), 2);
        kept_apart(&net);
        // An under link and the tree branch from A to E would share a run below C.
        let net = fan(&[("-B:P2", "-C:P2")]);
        assert_eq!(layout_network(&net).under.len(), 1);
        kept_apart(&net);
        // The runs of one gap stay between the columns, clear of the boxes either side.
        let (g, topo, _, _) = drawn_links(&net);
        let r = routes(&topo, &g, bottom_of(&topo, &g));
        let (x1, _) = g.spot(1, 0);
        for x in r.x.values() {
            assert!(*x > x1 - (g.step_x - g.box_w) && *x < x1, "{x} is in the gap");
        }
    }

    #[test]
    fn many_runs_in_one_gap_stay_inside_it() {
        // Ten devices in one column, each linked under to the one five rows down: every run
        // overlaps the others, and they still fit between the columns.
        let mut links: Vec<NetLink> = (1..=10)
            .map(|n| link(&format!("-A:P{n}"), &format!("-T{n}:X1")))
            .collect();
        links.extend((1..=5).map(|n| link(&format!("-T{n}:X2"), &format!("-T{}:X2", n + 5))));
        let net = Network {
            links,
            ..Network::default()
        };
        assert_eq!(layout_network(&net).under.len(), 5);
        kept_apart(&net);

        // Twenty under runs and a tree bus need more tracks than the gap holds at the full
        // step: they close up instead of running into the box of the column before.
        let mut links: Vec<NetLink> = (1..=20)
            .map(|n| link(&format!("-A:P{n}"), &format!("-T{n}:X1")))
            .collect();
        links.extend((1..=10).map(|n| link(&format!("-T{n}:X2"), &format!("-T{}:X2", n + 10))));
        let net = Network {
            links,
            ..Network::default()
        };
        let topo = layout_network(&net);
        assert_eq!(topo.under.len(), 10);
        let (g, topo, _, _) = drawn_links(&net);
        let r = routes(&topo, &g, bottom_of(&topo, &g));
        let (first, least) = g.gap(1);
        assert!(
            r.x.len() * 3 / 2 > (first - least) as usize,
            "{} runs fill the gap",
            r.x.len()
        );
        for x in r.x.values() {
            assert!(
                *x >= least - 1e-9 && *x <= first + 1e-9,
                "{x} is outside the gap {least}..{first}"
            );
        }
        kept_apart(&net);
    }

    /// A topology page for `net` on the sheet it gets, with its warnings.
    fn page_of(net: &Network) -> (Drawn, Vec<String>) {
        let band = Project::new("x").meta.template.band_height();
        let (size, warn) = sheet(PageSize::A3, band, net);
        assert!(warn.is_none());
        let mut page = Page::new("network-n", "N1", "Network");
        page.size = size;
        let mut warnings = Vec::new();
        let drawn = draw_network_page(
            &Project::new("x"),
            &page,
            0,
            1,
            net,
            "",
            &Default::default(),
            &mut warnings,
        );
        (drawn, warnings)
    }

    #[test]
    fn port_names_a_little_too_long_for_their_branch_are_drawn_whole() {
        // Each name needs about 9.2 mm at the full type, the branch gives 8.5 (#493).
        let net = Network {
            links: vec![link("-K1:X1P10", "-T1:X1"), link("-K1:X1P11", "-T2:X1")],
            ..Network::default()
        };
        let (size, g, topo) = fits_its_sheet(&net);
        let mut legend = Legend::default();
        let (_, labels) = link_drawing(&topo, &g, &net, size.mm().0, &mut legend);
        assert!(legend.0.is_empty(), "{legend:?}");
        for name in ["X1P10", "X1P11"] {
            let l = labels.iter().find(|l| l.text == name).unwrap();
            assert!(l.size < TEXT && l.size >= LABEL_MIN, "{l:?}");
        }
        apart(&labels);
        let (drawn, _) = page_of(&net);
        for name in ["X1P10", "X1P11"] {
            assert!(
                drawn.ops.contains(&crate::writer::pdf_text(name)),
                "{name} is drawn whole"
            );
        }
        assert!(!drawn.ops.contains("X1P..."));
    }

    #[test]
    fn long_device_tags_in_narrow_boxes_are_drawn_whole() {
        // A 30-device line leaves the narrowest boxes, 19 mm for the tag.
        let mut names: Vec<String> = (1..=28).map(|n| format!("-A{n}")).collect();
        names.insert(10, "=PUMP+CAB1-K1".into());
        names.insert(11, "=PUMP+CAB1-K2".into());
        let net = Network {
            nodes: names.iter().map(|n| node(n)).collect(),
            links: names
                .windows(2)
                .map(|p| link(&format!("{}:P2", p[0]), &format!("{}:P1", p[1])))
                .collect(),
            ..Network::default()
        };
        let (_, g, _) = fits_its_sheet(&net);
        assert!((g.box_w - MIN_BOX_W).abs() < 1e-9, "{g:?}");
        let (drawn, _) = page_of(&net);
        for tag in ["=PUMP+CAB1-K1", "=PUMP+CAB1-K2"] {
            assert!(
                drawn.ops.contains(&crate::writer::pdf_text(tag)),
                "{tag} is drawn whole"
            );
        }

        // A tag too long even for the smallest type is keyed, and written whole in the legend
        // under the drawing, inside the sheet.
        let long = "=PUMPSTATION-NORTH+CABINET-ONE-K1";
        let mut net = net;
        net.nodes[0].device = long.into();
        net.links[0].from = format!("{long}:P2");
        let mut legend = Legend::default();
        let (_, g, _) = fits_its_sheet(&net);
        let (text, size) = tag_label(long, &g, &mut legend);
        assert_eq!((text.as_str(), size), ("[1]", TAG_TEXT));
        let (drawn, _) = page_of(&net);
        assert!(drawn.ops.contains(&crate::writer::pdf_text(&format!("[1] {long}"))));
        assert!(!drawn.ops.contains("=PUMPSTATION..."));
    }

    #[test]
    fn a_long_legend_makes_the_sheet_taller() {
        let topo_of = |n: usize| Network {
            links: (1..=n)
                .map(|i| link(&format!("-K1:PORT-{i}-WITH-A-NAME-FAR-TOO-LONG"), &format!("-T{i}:X1")))
                .collect(),
            ..Network::default()
        };
        let band = 30.0;
        let net = topo_of(12);
        let topo = layout_network(&net);
        let keys = legend_for(&topo, &net, PageSize::A3.mm().0).0.len();
        assert_eq!(keys, 12);
        let (size, warn) = sheet(PageSize::A4, band, &net);
        assert!(warn.is_none());
        let (w, h) = size.mm();
        assert!(h >= needed(&topo, band, legend_for(&topo, &net, w).0.len()).1);
        assert!(needed(&topo, band, keys).1 > needed(&topo, band, 0).1);
    }
}

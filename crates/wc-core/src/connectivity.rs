//! Potentials: which pins are electrically connected, across all pages.
//!
//! Wires join their two ends; wire ends that meet at the same point join; a free wire end on
//! another wire's segment (a T) joins that wire; wires that carry the same potential name join
//! across pages.

use std::collections::BTreeMap;

use crate::geom::Point;
use crate::project::{Endpoint, Page, Project};
use crate::symbol::SymbolRole;

/// A pin on a placed symbol: (page id, placed symbol id, pin name).
pub type PinRef = (String, String, String);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Net {
    /// Potential names attached to this net (normally zero or one; more is a design error).
    pub names: Vec<String>,
    pub pins: Vec<PinRef>,
}

#[derive(Default)]
struct Dsu {
    parent: Vec<usize>,
}

impl Dsu {
    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.parent[b] = a;
        }
    }
}

/// The connection graph: every node and which ones are joined.
#[derive(Default)]
struct Graph {
    ids: BTreeMap<Node, usize>,
    dsu: Dsu,
}

impl Graph {
    fn id_of(&mut self, n: Node) -> usize {
        let next = self.ids.len();
        let dsu = &mut self.dsu;
        *self.ids.entry(n).or_insert_with(|| {
            dsu.parent.push(next);
            next
        })
    }

    /// Join each wire of `page` to its two ends and to its potential name. Returns each
    /// wire's `from` node, in the page's wire order.
    fn add_wires(&mut self, page: &Page) -> Vec<usize> {
        let node = |e: &Endpoint| match e {
            Endpoint::Pin { item, pin } => Node::Pin((page.id.clone(), item.clone(), pin.clone())),
            Endpoint::Point { at } => {
                let (x, y) = point_key(*at);
                Node::Point(page.id.clone(), x, y)
            }
        };
        let mut from = Vec::with_capacity(page.wires.len());
        for w in &page.wires {
            let a = self.id_of(node(&w.from));
            let b = self.id_of(node(&w.to));
            self.dsu.union(a, b);
            // Spaces around a name are no part of it, and a blank name is no potential: else
            // every wire with an empty one would be joined into one nameless net.
            if let Some(name) = w.potential.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                let p = self.id_of(Node::Potential(name.to_owned()));
                self.dsu.union(a, p);
            }
            from.push(a);
        }
        // A free end on another wire's segment (a T, where the drawing puts a junction dot)
        // joins that wire. Only segments whose both ends are known here count: free ends and
        // bends, as a pin's place needs its symbol.
        let paths: Vec<Vec<Point>> = page
            .wires
            .iter()
            .map(|w| {
                let free = |e: &Endpoint| match e {
                    Endpoint::Point { at } => Some(*at),
                    Endpoint::Pin { .. } => None,
                };
                free(&w.from)
                    .into_iter()
                    .chain(w.via.iter().copied())
                    .chain(free(&w.to))
                    .collect()
            })
            .collect();
        for (wi, w) in page.wires.iter().enumerate() {
            for e in [&w.from, &w.to] {
                let Endpoint::Point { at: q } = e else { continue };
                for (oi, path) in paths.iter().enumerate() {
                    if oi == wi || !path.windows(2).any(|ab| on_segment(*q, ab[0], ab[1])) {
                        continue;
                    }
                    if let (Some(&end), Some(&other)) = (from.get(wi), from.get(oi)) {
                        // The end is joined to its own wire already, so joining the wires joins it.
                        self.dsu.union(end, other);
                    }
                }
            }
        }
        from
    }
}

/// Whether `p` lies on the segment from `a` to `b` (ends included), within 0.01 mm.
fn on_segment(p: Point, a: Point, b: Point) -> bool {
    const EPS: f64 = 0.01;
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    // Only a zero-length segment is a point: a short one is measured along its length, so the
    // answer does not depend on which way the wire was drawn.
    if len2 == 0.0 {
        return (p.x - a.x).hypot(p.y - a.y) < EPS;
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    (a.x + t * dx - p.x).hypot(a.y + t * dy - p.y) < EPS
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Node {
    Pin(PinRef),
    /// A free point, in hundredths of a millimetre.
    Point(String, i64, i64),
    Potential(String),
    /// Interruption points sharing a device tag.
    Link(String),
}

/// Where a free wire end is, as connectivity tells ends apart: in hundredths of a millimetre.
/// Two free ends with the same key on one page are joined.
fn point_key(at: Point) -> (i64, i64) {
    ((at.x * 100.0).round() as i64, (at.y * 100.0).round() as i64)
}

/// A free wire end that would join another wire there, which it was not connected to before.
#[derive(Debug, Clone, PartialEq)]
pub struct NewJoin {
    /// The index of the page in [`Project::pages`].
    pub page: usize,
    /// The wire whose end was on a pin and is now a free end.
    pub wire: String,
    /// The other wire with a free end at the same place.
    pub other: String,
    pub at: Point,
}

/// Which wires of `project` are connected, by wire ends that meet (on a pin or at a free point)
/// and by potential names: (page index, wire index) to a group number. Interruption points and
/// jumpers are left out, so wires they alone connect are in different groups.
fn wire_groups(project: &Project) -> BTreeMap<(usize, usize), usize> {
    let mut g = Graph::default();
    let mut first = Vec::new();
    for (pi, page) in project.pages.iter().enumerate() {
        first.extend(g.add_wires(page).into_iter().enumerate().map(|(wi, a)| ((pi, wi), a)));
    }
    first.into_iter().map(|(w, a)| (w, g.dsu.find(a))).collect()
}

/// The free wire ends of `after` that join another wire's free end at the same place,
/// although in `before` (the same project before the change, with the same pages and wires in
/// the same order: a wire is known by its place in its page, as ids need not be unique) that
/// end was on a pin and the two wires were not connected. This is what turning wire ends on
/// pins into free ends (a black box point that is removed keeps its wires' ends where it was)
/// must not do: it would join nets that were separate (#609). Wires that were connected
/// already, such as two on the same pin, may still meet.
pub fn new_point_joins(before: &Project, after: &Project) -> Vec<NewJoin> {
    let mut groups = None;
    let mut joins = Vec::new();
    for (pi, (bp, ap)) in before.pages.iter().zip(&after.pages).enumerate() {
        // Every free end of the page after: (wire index, whether that end was on a pin before,
        // where it is).
        type FreeEnd = (usize, bool, Point);
        let mut at: BTreeMap<(i64, i64), Vec<FreeEnd>> = BTreeMap::new();
        for (wi, w) in ap.wires.iter().enumerate() {
            let was = bp.wires.get(wi);
            for (k, end) in [&w.from, &w.to].into_iter().enumerate() {
                let Endpoint::Point { at: p } = end else {
                    continue;
                };
                let on_pin = was
                    .map(|b| if k == 0 { &b.from } else { &b.to })
                    .is_some_and(|e| matches!(e, Endpoint::Pin { .. }));
                at.entry(point_key(*p)).or_default().push((wi, on_pin, *p));
            }
        }
        let id = |wi: usize| ap.wires.get(wi).map_or_else(String::new, |w| w.id.clone());
        for ends in at.values() {
            for &(wi, on_pin, p) in ends {
                if !on_pin {
                    continue;
                }
                let groups = groups.get_or_insert_with(|| wire_groups(before));
                let group = |i: usize| groups.get(&(pi, i)).copied();
                let connected = |o: usize| group(wi).is_some() && group(wi) == group(o);
                if let Some(&(other, ..)) = ends.iter().find(|(o, ..)| *o != wi && !connected(*o)) {
                    joins.push(NewJoin {
                        page: pi,
                        wire: id(wi),
                        other: id(other),
                        at: p,
                    });
                }
            }
        }
    }
    joins
}

/// Every wire end on a pin of placed symbol `item` of `page`: the pin's name and its node.
fn wired_pins<'a>(page: &'a Page, item: &'a str) -> impl Iterator<Item = (&'a str, Node)> + 'a {
    page.wires
        .iter()
        .flat_map(|w| [&w.from, &w.to])
        .filter_map(move |e| match e {
            Endpoint::Pin { item: i, pin } if i == item => {
                Some((pin.as_str(), Node::Pin((page.id.clone(), i.clone(), pin.clone()))))
            }
            _ => None,
        })
}

/// Group every connected pin into nets. Pins with no wire are left out.
pub fn nets(project: &Project) -> Vec<Net> {
    nets_linked(project, |_| None)
}

/// Like [`nets`], and also joins interruption points. `role(symbol id)` is the role of a placed
/// symbol's library symbol (`None` when it is not known): every interruption point is connected
/// to the others with the same device tag (one on an overview page, `Page::overview`, only to
/// those on overview pages), and only terminals (or symbols of unknown role) take part in
/// jumpers.
pub fn nets_linked(project: &Project, role: impl Fn(&str) -> Option<SymbolRole>) -> Vec<Net> {
    wire_nets(project, role).0
}

/// A wire: (page id, wire id).
pub type WireRef = (String, String);

/// Like [`nets_linked`], and also says which net each wire belongs to (an index into the nets).
/// A wire that touches no pin, directly or through other wires, belongs to no net. Two wires
/// with the same id on one page share a key here: [`wire_nets_by_place`] tells them apart.
pub fn wire_nets(project: &Project, role: impl Fn(&str) -> Option<SymbolRole>) -> (Vec<Net>, BTreeMap<WireRef, usize>) {
    let (nets, by_place) = wire_nets_by_place(project, role);
    let wires = by_place
        .into_iter()
        .filter_map(|((pi, wi), n)| {
            let page = project.pages.get(pi)?;
            Some(((page.id.clone(), page.wires.get(wi)?.id.clone()), n))
        })
        .collect();
    (nets, wires)
}

/// Like [`wire_nets`], with each wire known by its place: (page index, wire index in the page),
/// as wire ids need not be unique on a page.
pub fn wire_nets_by_place(
    project: &Project,
    role: impl Fn(&str) -> Option<SymbolRole>,
) -> (Vec<Net>, BTreeMap<(usize, usize), usize>) {
    let (mut g, wire_nodes) = linked_graph(project, role);
    let mut index_of_root: BTreeMap<usize, usize> = BTreeMap::new();
    let mut nets = Vec::new();
    for (root, net) in groups(&mut g) {
        if !net.pins.is_empty() {
            index_of_root.insert(root, nets.len());
            nets.push(net);
        }
    }
    let wires = wire_nodes
        .into_iter()
        .filter_map(|(w, node)| index_of_root.get(&g.dsu.find(node)).map(|n| (w, *n)))
        .collect();
    (nets, wires)
}

/// The nets of [`nets_linked`] that carry more than one potential name (a short, such as L1
/// joined to L2), together with such groups that touch no pin: wires that meet only each other
/// at junctions still short their potentials, though they make no net of their own.
pub fn shorted_nets(project: &Project, role: impl Fn(&str) -> Option<SymbolRole>) -> Vec<Net> {
    let (mut g, _) = linked_graph(project, role);
    groups(&mut g).into_values().filter(|n| n.names.len() > 1).collect()
}

/// Every connected group of `g`, by its root: its pins and potential names.
fn groups(g: &mut Graph) -> BTreeMap<usize, Net> {
    let mut groups: BTreeMap<usize, Net> = BTreeMap::new();
    for (node, id) in &g.ids {
        let root = g.dsu.find(*id);
        let net = groups.entry(root).or_default();
        match node {
            Node::Pin(p) => net.pins.push(p.clone()),
            Node::Potential(n) => net.names.push(n.clone()),
            Node::Point(..) | Node::Link(_) => {}
        }
    }
    groups
}

/// Each wire's `from` node, by its place: ((page index, wire index), node).
type WireNodes = Vec<((usize, usize), usize)>;

/// The connection graph of [`wire_nets_by_place`], with each wire's `from` node by its place.
fn linked_graph(project: &Project, role: impl Fn(&str) -> Option<SymbolRole>) -> (Graph, WireNodes) {
    let mut g = Graph::default();
    let mut wire_nodes: WireNodes = Vec::new();
    for (pi, page) in project.pages.iter().enumerate() {
        let from = g.add_wires(page);
        wire_nodes.extend(from.into_iter().enumerate().map(|(wi, a)| ((pi, wi), a)));
        for s in &page.symbols {
            let Some(tag) = &s.tag else { continue };
            if role(&s.symbol) != Some(SymbolRole::Interruption) {
                continue;
            }
            // Only interruption points that are wired take part; their pins join under the tag.
            // A copy on an overview page links nothing in the schematic (as the project check
            // counts partners), so overview points join only each other.
            let link = if page.overview {
                Node::Link(format!("\u{0}overview\u{0}{tag}"))
            } else {
                Node::Link(tag.to_string())
            };
            let wired: Vec<usize> = wired_pins(page, &s.id).map(|(_, n)| g.id_of(n)).collect();
            for n in wired {
                let l = g.id_of(link.clone());
                g.dsu.union(n, l);
            }
        }
    }
    // Jumpers: the jumper joins the same side of its terminals, on any page: every wired pin `1`
    // of them is one net, every pin `2` another. A terminal's two sides stay apart, as they are
    // without a jumper, so wire numbers and the terminal plan keep telling them apart.
    // Terminals are known by the numbering the terminal plan shows, so a terminal tagged with
    // only its strip takes part too. A device drawn with a terminal's tag is no terminal.
    let numbers = if project.meta.jumpers.is_empty() {
        Default::default()
    } else {
        crate::project::terminal_numbers(project, &role)
    };
    for (k, j) in project.meta.jumpers.iter().enumerate() {
        for page in &project.pages {
            for s in &page.symbols {
                let terminal = role(&s.symbol).is_none_or(|r| r == SymbolRole::Terminal);
                if !j.has(&numbers, page, s, terminal) {
                    continue;
                }
                let wired: Vec<(String, usize)> = wired_pins(page, &s.id)
                    .map(|(pin, n)| (pin.to_owned(), g.id_of(n)))
                    .collect();
                for (pin, n) in wired {
                    let l = g.id_of(Node::Link(format!("\u{0}jumper\u{0}{k}\u{0}{pin}")));
                    g.dsu.union(n, l);
                }
            }
        }
    }
    (g, wire_nodes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Point;
    use crate::project::{Page, Wire};

    fn wire(id: &str, from: Endpoint, to: Endpoint, potential: Option<&str>) -> Wire {
        Wire {
            id: id.into(),
            from,
            to,
            via: vec![],
            number: None,
            potential: potential.map(Into::into),
            cable: None,
            route: Default::default(),
            spec: Default::default(),
        }
    }
    fn pin(item: &str, pin: &str) -> Endpoint {
        Endpoint::Pin {
            item: item.into(),
            pin: pin.into(),
        }
    }

    #[test]
    fn a_pin_end_made_free_where_another_free_end_is_is_a_new_join() {
        let free = |x: f64, y: f64| Endpoint::Point { at: Point::new(x, y) };
        let mut page = Page::new("a", "1", "");
        page.wires = vec![
            wire("w1", pin("b1", "IN"), pin("k1", "A1"), None),
            wire("w2", pin("b1", "IN"), pin("k2", "A1"), None),
            wire("w3", free(40.0, 45.0), pin("k3", "A1"), None),
            wire("w4", pin("b1", "OUT"), pin("k4", "A1"), None),
        ];
        let mut before = Project::new("t");
        before.pages = vec![page];
        // IN (at 40, 45) and OUT (at 60, 45) are removed: their wires keep free ends there.
        let mut after = before.clone();
        after.pages[0].wires[0].from = free(40.0, 45.0);
        after.pages[0].wires[1].from = free(40.0, 45.0);
        after.pages[0].wires[3].from = free(60.0, 45.0);
        let joins = new_point_joins(&before, &after);
        // w1 and w2 were on the same point and still meet; w3's free end was there already.
        let join = |wire: &str| NewJoin {
            page: 0,
            wire: wire.into(),
            other: "w3".into(),
            at: Point::new(40.0, 45.0),
        };
        assert_eq!(joins, vec![join("w1"), join("w2")]);
        // Without w3 nothing new joins.
        before.pages[0].wires.remove(2);
        after.pages[0].wires.remove(2);
        assert_eq!(new_point_joins(&before, &after), vec![]);
        // Two points removed at one place (boxes on top of each other) join their wires.
        after.pages[0].wires[2].from = free(40.0, 45.0);
        assert_eq!(new_point_joins(&before, &after).len(), 3);
    }

    #[test]
    fn wires_sharing_an_id_are_told_apart_by_their_place() {
        let free = |x: f64, y: f64| Endpoint::Point { at: Point::new(x, y) };
        let mut page = Page::new("a", "1", "");
        // Two unrelated wires with one id (a replaced page can hold such): the second ends
        // freely where box point A is.
        page.wires = vec![
            wire("w1", pin("b1", "A"), pin("k1", "A1"), None),
            wire("w1", free(40.0, 45.0), pin("k2", "A1"), None),
        ];
        let mut before = Project::new("t");
        before.pages = vec![page];
        let mut after = before.clone();
        after.pages[0].wires[0].from = free(40.0, 45.0);
        let joins = new_point_joins(&before, &after);
        assert_eq!(joins.len(), 1, "{joins:?}");
        assert_eq!(joins[0].at, Point::new(40.0, 45.0));
    }

    #[test]
    fn a_free_end_on_a_wire_already_connected_is_no_new_join() {
        let free = |x: f64, y: f64| Endpoint::Point { at: Point::new(x, y) };
        let mut page = Page::new("a", "1", "");
        // w1 runs from box point A to C; w2 from where A is to the same C: one net already.
        page.wires = vec![
            wire("w1", pin("b1", "A"), pin("k1", "C"), None),
            wire("w2", free(40.0, 45.0), pin("k1", "C"), None),
            wire("w3", pin("b1", "B"), pin("k3", "A1"), Some("L1")),
            wire("w4", free(60.0, 45.0), pin("k4", "A1"), Some("L1")),
        ];
        let mut before = Project::new("t");
        before.pages = vec![page];
        let mut after = before.clone();
        after.pages[0].wires[0].from = free(40.0, 45.0);
        // w3 and w4 carry the same potential.
        after.pages[0].wires[2].from = free(60.0, 45.0);
        assert_eq!(new_point_joins(&before, &after), vec![]);
        // On another net it is refused.
        before.pages[0].wires[1].to = pin("k2", "C");
        after.pages[0].wires[1].to = pin("k2", "C");
        assert_eq!(new_point_joins(&before, &after).len(), 1);
    }

    #[test]
    fn jumpered_terminals_are_one_net_across_pages() {
        use crate::project::PlacedSymbol;
        use crate::tag::DeviceTag;
        let term = |id: &str, tag: &str| PlacedSymbol {
            id: id.into(),
            symbol: "wc.terminal".into(),
            at: Point::new(0.0, 0.0),
            rotation: Default::default(),
            mirror: false,
            tag: DeviceTag::parse(tag).ok(),
        };
        let mut p = Project::new("t");
        let mut a = Page::new("a", "1", "");
        a.symbols = vec![term("t3", "-X1.3"), term("t9", "-X1.9")];
        a.wires = vec![
            wire("w1", pin("t3", "1"), pin("k1", "A1"), None),
            wire("w2", pin("t9", "2"), pin("k2", "A1"), None),
        ];
        let mut b = Page::new("b", "2", "");
        b.symbols = vec![term("t4", "-X1.4")];
        b.wires = vec![wire("w3", pin("t4", "1"), pin("m1", "U"), None)];
        p.pages = vec![a, b];
        assert_eq!(nets(&p).len(), 3, "no jumper, three nets");
        let x1 = DeviceTag::parse("-X1").unwrap_or_default();
        assert!(p.meta.add_jumper(&x1, &["3".to_owned(), "4".to_owned()]).is_ok());
        let joined = nets(&p);
        assert_eq!(joined.len(), 2, "3 and 4 jumpered, 9 not");
        let big = joined.iter().find(|n| n.pins.len() == 4);
        assert!(big.is_some_and(|n| n.pins.iter().any(|q| q.2 == "U") && n.pins.iter().any(|q| q.2 == "A1")));
        // The other side of a jumpered terminal is not joined to this one.
        p.pages[1].wires.push(wire("w4", pin("t4", "2"), pin("m2", "V"), None));
        let sides = nets(&p);
        assert_eq!(sides.len(), 3, "pin 2 of -X1.4 is its own net");
        assert!(
            sides
                .iter()
                .any(|n| n.pins.iter().any(|q| q.2 == "V") && n.pins.len() == 2)
        );
        // A coil drawn with the tag -X1.4 is no terminal: with roles known it stays off the jumper.
        let mut coil = term("c1", "-X1.4");
        coil.symbol = "wc.coil".into();
        p.pages[1].symbols.push(coil);
        p.pages[1].wires.push(wire("w5", pin("c1", "1"), pin("m3", "W"), None));
        let role = |id: &str| {
            Some(if id == "wc.terminal" {
                SymbolRole::Terminal
            } else {
                SymbolRole::Standalone
            })
        };
        let w_net = |nets: &[Net]| {
            nets.iter()
                .find(|n| n.pins.iter().any(|q| q.2 == "W"))
                .map(|n| n.pins.len())
        };
        assert_eq!(w_net(&nets_linked(&p, role)), Some(2), "the coil's wire is its own net");
        assert_eq!(w_net(&nets(&p)), Some(6), "without roles the tag alone decides");
    }

    #[test]
    fn terminals_tagged_with_only_their_strip_are_jumpered_by_their_plan_number() {
        use crate::project::PlacedSymbol;
        use crate::tag::DeviceTag;
        let term = |id: &str, x: f64| PlacedSymbol {
            id: id.into(),
            symbol: "wc.terminal".into(),
            at: Point::new(x, 0.0),
            rotation: Default::default(),
            mirror: false,
            tag: DeviceTag::parse("-X1").ok(),
        };
        let mut p = Project::new("t");
        let mut a = Page::new("a", "1", "");
        // Drawn right to left: the left one (b) is terminal 1, the right one (a) terminal 2.
        a.symbols = vec![term("ta", 50.0), term("tb", 10.0)];
        a.wires = vec![
            wire("w1", pin("ta", "1"), pin("k1", "A1"), None),
            wire("w2", pin("tb", "1"), pin("k2", "A1"), None),
            wire("w3", pin("ta", "2"), pin("k3", "A1"), None),
            wire("w4", pin("tb", "2"), pin("k4", "A1"), None),
        ];
        p.pages = vec![a];
        let role = |id: &str| (id == "wc.terminal").then_some(SymbolRole::Terminal);
        let numbers = crate::project::terminal_numbers(&p, role);
        assert_eq!(numbers[&("a".to_owned(), "tb".to_owned())].1, "1");
        assert_eq!(numbers[&("a".to_owned(), "ta".to_owned())].1, "2");
        assert_eq!(nets_linked(&p, role).len(), 4, "no jumper");
        let x1 = DeviceTag::parse("-X1").unwrap_or_default();
        assert!(p.meta.add_jumper(&x1, &["1".to_owned(), "2".to_owned()]).is_ok());
        let joined = nets_linked(&p, role);
        assert_eq!(joined.len(), 2, "one net per side");
        assert!(joined.iter().all(|n| n.pins.len() == 4));
    }

    #[test]
    fn the_largest_terminal_number_does_not_overflow() {
        use crate::project::PlacedSymbol;
        use crate::tag::DeviceTag;
        let term = |id: &str, tag: &str| PlacedSymbol {
            id: id.into(),
            symbol: "wc.terminal".into(),
            at: Point::new(0.0, 0.0),
            rotation: Default::default(),
            mirror: false,
            tag: DeviceTag::parse(tag).ok(),
        };
        let mut p = Project::new("t");
        let mut a = Page::new("a", "1", "");
        a.symbols = vec![term("big", "-X1.18446744073709551615"), term("bare", "-X1")];
        p.pages = vec![a];
        let numbers = crate::project::terminal_numbers(&p, |_| Some(SymbolRole::Terminal));
        assert_eq!(numbers[&("a".to_owned(), "big".to_owned())].1, "18446744073709551615");
        assert!(
            !numbers.contains_key(&("a".to_owned(), "bare".to_owned())),
            "no number left for it"
        );
    }

    #[test]
    fn a_free_end_on_another_wire_joins_it() {
        let at = |x: f64, y: f64| Endpoint::Point { at: Point::new(x, y) };
        let mut p = Project::new("t");
        let mut a = Page::new("p2", "2", "");
        // -K1:A2 down to 20,40; a bus from 20,40 to 100,40; -S1:13 ends on the bus at 60,40.
        a.wires.push(wire("w1", pin("s2", "A2"), at(20.0, 40.0), None));
        a.wires.push(wire("w2", at(20.0, 40.0), at(100.0, 40.0), None));
        a.wires.push(wire("w3", pin("s1", "13"), at(60.0, 40.0), None));
        // A wire from a pin with a bend at 100,80 and a free end at 140,80: -Q1:1 ends on its
        // bent part, and -Q2:1 on the bend itself.
        let mut bent = wire("w4", pin("s3", "1"), at(140.0, 80.0), None);
        bent.via = vec![Point::new(100.0, 60.0), Point::new(100.0, 80.0)];
        a.wires.push(bent);
        a.wires.push(wire("w5", pin("s4", "1"), at(120.0, 80.0), None));
        a.wires.push(wire("w6", pin("s5", "1"), at(100.0, 60.0), None));
        // A free end beside a wire, not on it, stays apart.
        a.wires.push(wire("w7", pin("s6", "1"), at(60.0, 40.5), None));
        p.pages = vec![a];
        let nets = nets(&p);
        let mut pins: Vec<Vec<&str>> = nets
            .iter()
            .map(|n| n.pins.iter().map(|(_, item, _)| item.as_str()).collect())
            .collect();
        pins.sort();
        assert_eq!(pins, [vec!["s1", "s2"], vec!["s3", "s4", "s5"], vec!["s6"]]);
    }

    #[test]
    fn a_short_segment_is_measured_the_same_both_ways() {
        let (a, b, p) = (Point::new(0.0, 0.0), Point::new(0.009, 0.0), Point::new(0.018, 0.0));
        assert!(on_segment(p, a, b));
        assert!(on_segment(p, b, a));
        assert!(on_segment(a, a, a));
        assert!(!on_segment(Point::new(0.03, 0.0), a, b));
    }

    #[test]
    fn joins_through_points_and_potentials() {
        let mut p = Project::new("t");
        let mut a = Page::new("a", "1", "");
        let j = Endpoint::Point {
            at: Point::new(10.0, 10.0),
        };
        a.wires.push(wire("w1", pin("s1", "1"), j.clone(), None));
        a.wires.push(wire("w2", j, pin("s2", "1"), Some("L1")));
        let mut b = Page::new("b", "2", "");
        b.wires.push(wire(
            "w1",
            pin("s1", "2"),
            Endpoint::Point {
                at: Point::new(0.0, 0.0),
            },
            Some("L1"),
        ));
        b.wires.push(wire("w2", pin("s3", "1"), pin("s4", "1"), None));
        p.pages = vec![a, b];
        let nets = nets(&p);
        assert_eq!(nets.len(), 2);
        let big = nets.iter().find(|n| n.names == ["L1"]);
        assert_eq!(big.map(|n| n.pins.len()), Some(3));
    }

    #[test]
    fn blank_potentials_join_nothing_and_names_are_trimmed() {
        let mut p = Project::new("t");
        let mut a = Page::new("a", "1", "");
        a.wires = vec![
            wire("w1", pin("s1", "1"), pin("s2", "1"), Some("")),
            wire("w2", pin("s3", "1"), pin("s4", "1"), Some("  ")),
            wire("w3", pin("s5", "1"), pin("s6", "1"), Some(" L1 ")),
        ];
        let mut b = Page::new("b", "2", "");
        b.wires = vec![wire("w1", pin("s7", "1"), pin("s8", "1"), Some("L1"))];
        p.pages = vec![a, b];
        let nets = nets(&p);
        assert_eq!(nets.len(), 3, "{nets:?}");
        assert!(nets.iter().filter(|n| n.pins.len() == 2).all(|n| n.names.is_empty()));
        let l1 = nets.iter().find(|n| n.pins.len() == 4);
        assert_eq!(l1.map(|n| n.names.clone()), Some(vec!["L1".to_owned()]));
    }

    #[test]
    fn potentials_joined_only_by_wires_are_a_short() {
        let mut p = Project::new("t");
        let mut a = Page::new("a", "1", "");
        let free = |x: f64| Endpoint::Point {
            at: Point::new(x, 10.0),
        };
        // An L1 wire and an L2 wire meet at a junction; no device pin anywhere.
        a.wires = vec![
            wire("w1", free(0.0), free(10.0), Some("L1")),
            wire("w2", free(10.0), free(20.0), Some("L2")),
            wire("w3", free(30.0), free(40.0), Some("L3")),
        ];
        p.pages = vec![a];
        assert!(nets(&p).is_empty(), "no pin, no net");
        let shorts = shorted_nets(&p, |_| None);
        assert_eq!(shorts.len(), 1, "{shorts:?}");
        assert_eq!(shorts[0].names, ["L1", "L2"]);
        assert!(shorts[0].pins.is_empty());
        // With a pin on it, the short is the net itself.
        p.pages[0].wires.push(wire("w4", free(20.0), pin("k1", "A1"), None));
        assert_eq!(shorted_nets(&p, |_| None), nets(&p));
    }

    #[test]
    fn interruption_points_with_the_same_tag_join() {
        use crate::project::PlacedSymbol;
        let arrow = |id: &str, tag: &str| PlacedSymbol {
            id: id.into(),
            symbol: "arrow".into(),
            at: Point::new(0.0, 0.0),
            rotation: Default::default(),
            mirror: false,
            tag: crate::DeviceTag::parse(tag).ok(),
        };
        let mut p = Project::new("t");
        let mut a = Page::new("a", "1", "");
        a.symbols.push(arrow("s9", "-L1"));
        a.wires.push(wire("w1", pin("s1", "1"), pin("s9", "1"), None));
        let mut b = Page::new("b", "2", "");
        b.symbols.push(arrow("s9", "-L1"));
        b.wires.push(wire("w1", pin("s9", "1"), pin("s2", "1"), None));
        p.pages = vec![a, b];
        assert_eq!(nets(&p).len(), 2);
        let linked = nets_linked(&p, |s| (s == "arrow").then_some(SymbolRole::Interruption));
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].pins.len(), 4);
    }

    #[test]
    fn an_interruption_point_on_an_overview_joins_no_schematic_net() {
        use crate::project::PlacedSymbol;
        use std::collections::BTreeSet;
        let arrow = |id: &str| PlacedSymbol {
            id: id.into(),
            symbol: "arrow".into(),
            at: Point::new(0.0, 0.0),
            rotation: Default::default(),
            mirror: false,
            tag: crate::DeviceTag::parse("-X1").ok(),
        };
        let page = |id: &str, from: &str, overview: bool| {
            let mut pg = Page::new(id, id, "");
            pg.overview = overview;
            pg.symbols.push(arrow("s9"));
            pg.wires.push(wire("w1", pin(from, "1"), pin("s9", "1"), None));
            pg
        };
        let mut p = Project::new("t");
        p.pages = vec![
            page("a", "s1", false),
            page("b", "s2", false),
            page("o", "s3", true),
            page("q", "s4", true),
        ];
        let linked = nets_linked(&p, |s| (s == "arrow").then_some(SymbolRole::Interruption));
        let mut found: Vec<BTreeSet<&str>> = linked
            .iter()
            .map(|n| n.pins.iter().map(|(pg, _, _)| pg.as_str()).collect())
            .collect();
        found.sort();
        // The schematic points join each other, the overview copies each other, never across.
        assert_eq!(found, vec![BTreeSet::from(["a", "b"]), BTreeSet::from(["o", "q"])]);
    }
}

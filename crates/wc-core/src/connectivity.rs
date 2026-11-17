//! Potentials: which pins are electrically connected, across all pages.
//!
//! Wires join their two ends; wire ends that meet at the same point join; wires that carry the
//! same potential name join across pages.

use std::collections::BTreeMap;

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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Node {
    Pin(PinRef),
    /// A free point, in hundredths of a millimetre.
    Point(String, i64, i64),
    Potential(String),
    /// Interruption points sharing a device tag.
    Link(String),
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
/// to the others with the same device tag, and only terminals (or symbols of unknown role) take
/// part in jumpers.
pub fn nets_linked(project: &Project, role: impl Fn(&str) -> Option<SymbolRole>) -> Vec<Net> {
    wire_nets(project, role).0
}

/// A wire: (page id, wire id).
pub type WireRef = (String, String);

/// Like [`nets_linked`], and also says which net each wire belongs to (an index into the nets).
/// A wire that touches no pin, directly or through other wires, belongs to no net.
pub fn wire_nets(project: &Project, role: impl Fn(&str) -> Option<SymbolRole>) -> (Vec<Net>, BTreeMap<WireRef, usize>) {
    let mut ids: BTreeMap<Node, usize> = BTreeMap::new();
    let mut dsu = Dsu { parent: Vec::new() };
    let mut wire_nodes: Vec<(WireRef, usize)> = Vec::new();
    let mut id_of = |n: Node, dsu: &mut Dsu| -> usize {
        let next = ids.len();
        *ids.entry(n).or_insert_with(|| {
            dsu.parent.push(next);
            next
        })
    };
    for page in &project.pages {
        let node = |e: &Endpoint| match e {
            Endpoint::Pin { item, pin } => Node::Pin((page.id.clone(), item.clone(), pin.clone())),
            Endpoint::Point { at } => Node::Point(
                page.id.clone(),
                (at.x * 100.0).round() as i64,
                (at.y * 100.0).round() as i64,
            ),
        };
        for w in &page.wires {
            let a = id_of(node(&w.from), &mut dsu);
            wire_nodes.push(((page.id.clone(), w.id.clone()), a));
            let b = id_of(node(&w.to), &mut dsu);
            dsu.union(a, b);
            if let Some(name) = &w.potential {
                let p = id_of(Node::Potential(name.clone()), &mut dsu);
                dsu.union(a, p);
            }
        }
        for s in &page.symbols {
            let Some(tag) = &s.tag else { continue };
            if role(&s.symbol) != Some(SymbolRole::Interruption) {
                continue;
            }
            // Only interruption points that are wired take part; their pins join under the tag.
            let link = Node::Link(tag.to_string());
            let wired: Vec<usize> = wired_pins(page, &s.id).map(|(_, n)| id_of(n, &mut dsu)).collect();
            for n in wired {
                let l = id_of(link.clone(), &mut dsu);
                dsu.union(n, l);
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
                    .map(|(pin, n)| (pin.to_owned(), id_of(n, &mut dsu)))
                    .collect();
                for (pin, n) in wired {
                    let l = id_of(Node::Link(format!("\u{0}jumper\u{0}{k}\u{0}{pin}")), &mut dsu);
                    dsu.union(n, l);
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Net> = BTreeMap::new();
    for (node, id) in &ids {
        let root = dsu.find(*id);
        let net = groups.entry(root).or_default();
        match node {
            Node::Pin(p) => net.pins.push(p.clone()),
            Node::Potential(n) => net.names.push(n.clone()),
            Node::Point(..) | Node::Link(_) => {}
        }
    }
    let mut index_of_root: BTreeMap<usize, usize> = BTreeMap::new();
    let mut nets = Vec::new();
    for (root, net) in groups {
        if !net.pins.is_empty() {
            index_of_root.insert(root, nets.len());
            nets.push(net);
        }
    }
    let wires = wire_nodes
        .into_iter()
        .filter_map(|(w, node)| index_of_root.get(&dsu.find(node)).map(|n| (w, *n)))
        .collect();
    (nets, wires)
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
}

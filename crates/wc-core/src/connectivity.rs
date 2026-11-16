//! Potentials: which pins are electrically connected, across all pages.
//!
//! Wires join their two ends; wire ends that meet at the same point join; wires that carry the
//! same potential name join across pages.

use std::collections::BTreeMap;

use crate::project::{Endpoint, Project};

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

/// Group every connected pin into nets. Pins with no wire are left out.
pub fn nets(project: &Project) -> Vec<Net> {
    nets_linked(project, |_| false)
}

/// Like [`nets`], and also joins interruption points: every placed symbol for which
/// `is_interruption(symbol id)` holds is connected to the others with the same device tag.
pub fn nets_linked(project: &Project, is_interruption: impl Fn(&str) -> bool) -> Vec<Net> {
    wire_nets(project, is_interruption).0
}

/// A wire: (page id, wire id).
pub type WireRef = (String, String);

/// Like [`nets_linked`], and also says which net each wire belongs to (an index into the nets).
/// A wire that touches no pin, directly or through other wires, belongs to no net.
pub fn wire_nets(project: &Project, is_interruption: impl Fn(&str) -> bool) -> (Vec<Net>, BTreeMap<WireRef, usize>) {
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
            if !is_interruption(&s.symbol) {
                continue;
            }
            // Only interruption points that are wired take part; their pins join under the tag.
            let link = Node::Link(tag.to_string());
            let wired: Vec<usize> = page
                .wires
                .iter()
                .flat_map(|w| [&w.from, &w.to])
                .filter_map(|e| match e {
                    Endpoint::Pin { item, pin } if item == &s.id => {
                        Some(Node::Pin((page.id.clone(), item.clone(), pin.clone())))
                    }
                    _ => None,
                })
                .map(|n| id_of(n, &mut dsu))
                .collect();
            for n in wired {
                let l = id_of(link.clone(), &mut dsu);
                dsu.union(n, l);
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
        }
    }
    fn pin(item: &str, pin: &str) -> Endpoint {
        Endpoint::Pin {
            item: item.into(),
            pin: pin.into(),
        }
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
        let linked = nets_linked(&p, |s| s == "arrow");
        assert_eq!(linked.len(), 1);
        assert_eq!(linked[0].pins.len(), 4);
    }
}

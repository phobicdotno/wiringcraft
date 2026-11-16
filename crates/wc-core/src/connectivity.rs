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
}

/// Group every connected pin into nets. Pins with no wire are left out.
pub fn nets(project: &Project) -> Vec<Net> {
    let mut ids: BTreeMap<Node, usize> = BTreeMap::new();
    let mut dsu = Dsu { parent: Vec::new() };
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
            let b = id_of(node(&w.to), &mut dsu);
            dsu.union(a, b);
            if let Some(name) = &w.potential {
                let p = id_of(Node::Potential(name.clone()), &mut dsu);
                dsu.union(a, p);
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
            Node::Point(..) => {}
        }
    }
    groups.into_values().filter(|n| !n.pins.is_empty()).collect()
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
}

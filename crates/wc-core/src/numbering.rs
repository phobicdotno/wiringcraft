//! Wire numbering by rule. Every wire of one connection (net) gets the same number, so a
//! number names a connection wherever it is drawn, across junctions and pages.

use std::collections::{BTreeMap, BTreeSet};

use crate::connectivity::{WireRef, wire_nets};
use crate::geom::Point;
use crate::project::{Endpoint, Page, Project};

/// How numbers are made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// 1, 2, 3 in drawing order: page by page, left to right, top to bottom.
    Sequential,
    /// Page and column where the connection starts: `3.4`, then `3.4.2`, `3.4.3` for more
    /// connections starting in the same column.
    PageColumn,
    /// The potential's name where the connection has one (`24V`), otherwise sequential.
    Potential,
}

impl Rule {
    pub const ALL: [Rule; 3] = [Rule::Sequential, Rule::PageColumn, Rule::Potential];

    pub fn name(self) -> &'static str {
        match self {
            Rule::Sequential => "sequential",
            Rule::PageColumn => "page-column",
            Rule::Potential => "potential",
        }
    }

    /// The rule named `s`: its [`Rule::name`] in any case, with `_` for `-` (`page_column`,
    /// as the assistant writes it) and spaces around it ignored.
    pub fn parse(s: &str) -> Option<Rule> {
        let s = s.trim().replace('_', "-");
        Rule::ALL.into_iter().find(|r| r.name().eq_ignore_ascii_case(&s))
    }
}

/// What numbering changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Numbered {
    /// Connections that got a number.
    pub connections: usize,
    /// Wires whose number changed.
    pub wires: usize,
}

/// Number the project's wires by `rule`. A connection that already has a number on any of its
/// wires keeps it (and its other wires take it) unless `overwrite` is set; new numbers never
/// repeat one already in use. Wires that touch no pin are left alone, and so are the wires of
/// overview pages (`Page::overview`): an overview draws the schematic's connections again, so
/// its wires take no numbers and adding one does not change the schematic's numbers.
///
/// `role(symbol id)` joins interruption points and jumpers as for the net list, and
/// `position(page, end)` gives a wire end's place on the page (pin ends need the symbol).
pub fn number_wires(
    project: &mut Project,
    rule: Rule,
    overwrite: bool,
    role: impl Fn(&str) -> Option<crate::symbol::SymbolRole>,
    position: impl Fn(&Page, &Endpoint) -> Option<Point>,
) -> Numbered {
    // Overview pages keep their place (page indices stay valid) but draw nothing here.
    let mut schematic = project.clone();
    for page in schematic.pages.iter_mut().filter(|p| p.overview) {
        page.wires.clear();
        page.symbols.clear();
    }
    let (nets, wire_net) = wire_nets(&schematic, role);
    // Where each net starts: the first page it is drawn on, then its leftmost, topmost point.
    let mut start: BTreeMap<usize, (usize, f64, f64)> = BTreeMap::new();
    let mut existing: BTreeMap<usize, String> = BTreeMap::new();
    for (pi, page) in project.pages.iter().enumerate() {
        for w in &page.wires {
            let Some(&n) = wire_net.get(&(page.id.clone(), w.id.clone())) else {
                continue;
            };
            // Kept trimmed, so `" 5"` and a new `"5"` cannot both exist.
            if let Some(num) = w.number.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                existing.entry(n).or_insert_with(|| num.to_string());
            }
            let mut pts: Vec<Point> = w.via.clone();
            pts.extend([&w.from, &w.to].into_iter().filter_map(|e| position(page, e)));
            for p in pts.iter().filter(|p| p.x.is_finite() && p.y.is_finite()) {
                let here = (pi, p.x, p.y);
                let e = start.entry(n).or_insert(here);
                if (here.0, here.1, here.2) < (e.0, e.1, e.2) {
                    *e = here;
                }
            }
            start.entry(n).or_insert((pi, f64::MAX, f64::MAX));
        }
    }
    let mut order: Vec<(usize, (usize, f64, f64))> = start.into_iter().collect();
    order.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut used: BTreeSet<String> = if overwrite {
        BTreeSet::new()
    } else {
        existing.values().cloned().collect()
    };
    let mut counter = 0usize;
    let mut next_sequential = |used: &mut BTreeSet<String>| loop {
        counter += 1;
        let s = counter.to_string();
        if used.insert(s.clone()) {
            return s;
        }
    };
    let mut chosen: BTreeMap<usize, String> = BTreeMap::new();
    for (n, (pi, x, _)) in &order {
        if !overwrite && let Some(num) = existing.get(n) {
            chosen.insert(*n, num.clone());
            continue;
        }
        let number = match rule {
            Rule::Sequential => next_sequential(&mut used),
            // Trimmed like kept numbers, so `" 1 "` cannot sit beside a kept `"1"`.
            Rule::Potential => match nets
                .get(*n)
                .and_then(|net| net.names.first())
                .map(|name| name.trim())
                .filter(|name| !name.is_empty())
            {
                Some(name) if used.insert(name.to_string()) => name.to_string(),
                _ => next_sequential(&mut used),
            },
            Rule::PageColumn => {
                let page = project.pages.get(*pi);
                let base = format!(
                    "{}.{}",
                    page.map(|p| p.number.trim()).unwrap_or(""),
                    page.map(|p| project.column_of(p, *x)).unwrap_or(1)
                );
                let mut k = 1;
                let mut candidate = base.clone();
                while !used.insert(candidate.clone()) {
                    k += 1;
                    candidate = format!("{base}.{k}");
                }
                candidate
            }
        };
        chosen.insert(*n, number);
    }

    let mut out = Numbered {
        connections: chosen.len(),
        wires: 0,
    };
    for page in &mut project.pages {
        for w in &mut page.wires {
            let key: WireRef = (page.id.clone(), w.id.clone());
            let Some(num) = wire_net.get(&key).and_then(|n| chosen.get(n)) else {
                continue;
            };
            if w.number.as_deref() != Some(num.as_str()) {
                w.number = Some(num.clone());
                out.wires += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Wire;

    fn wire(id: &str, from: Endpoint, to: Endpoint, number: Option<&str>, potential: Option<&str>) -> Wire {
        Wire {
            id: id.into(),
            from,
            to,
            via: vec![],
            number: number.map(Into::into),
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
    fn point(x: f64, y: f64) -> Endpoint {
        Endpoint::Point { at: Point::new(x, y) }
    }

    /// Page 3: a net through a junction at the right (two wires), a net at the left, a wire to
    /// nowhere, and a 24V net. Pins sit at x = 10 * their number.
    fn project() -> Project {
        let mut p = Project::new("t");
        let mut page = Page::new("p3", "3", "Control");
        page.wires = vec![
            wire("a", pin("s1", "30"), point(300.0, 50.0), None, None),
            wire("b", point(300.0, 50.0), pin("s2", "31"), None, None),
            wire("c", pin("s3", "5"), pin("s4", "6"), None, None),
            wire("d", point(1.0, 1.0), point(2.0, 2.0), None, None),
            wire("e", pin("s5", "20"), pin("s6", "21"), None, Some("24V")),
        ];
        p.pages.push(page);
        p
    }

    fn numbers(p: &Project) -> Vec<Option<&str>> {
        p.pages[0].wires.iter().map(|w| w.number.as_deref()).collect()
    }

    fn pos(_: &Page, e: &Endpoint) -> Option<Point> {
        match e {
            Endpoint::Pin { pin, .. } => pin.parse::<f64>().ok().map(|n| Point::new(n * 10.0, 50.0)),
            Endpoint::Point { at } => Some(*at),
        }
    }

    #[test]
    fn a_rule_is_read_in_any_case_with_underscores_or_dashes() {
        for r in Rule::ALL {
            assert_eq!(Rule::parse(r.name()), Some(r));
        }
        assert_eq!(Rule::parse("page_column"), Some(Rule::PageColumn));
        assert_eq!(Rule::parse(" Page-Column "), Some(Rule::PageColumn));
        assert_eq!(Rule::parse("SEQUENTIAL"), Some(Rule::Sequential));
        assert_eq!(Rule::parse("Potential\n"), Some(Rule::Potential));
        assert_eq!(Rule::parse("potentials"), None);
        assert_eq!(Rule::parse("pagecolumn"), None);
        assert_eq!(Rule::parse(""), None);
    }

    #[test]
    fn sequential_in_drawing_order_and_one_number_per_connection() {
        let mut p = project();
        let n = number_wires(&mut p, Rule::Sequential, false, |_| None, pos);
        // Left to right: c (x 50), e (x 200), a/b (x 300); d touches no pin.
        assert_eq!(numbers(&p), [Some("3"), Some("3"), Some("1"), None, Some("2")]);
        assert_eq!((n.connections, n.wires), (3, 4));
        // Running it again changes nothing.
        assert_eq!(number_wires(&mut p, Rule::Sequential, false, |_| None, pos).wires, 0);
    }

    #[test]
    fn page_column_and_potential_rules() {
        let mut p = project();
        number_wires(&mut p, Rule::PageColumn, false, |_| None, pos);
        // A3: columns are 40 mm wide from x = 10.
        assert_eq!(numbers(&p), [Some("3.8"), Some("3.8"), Some("3.2"), None, Some("3.5")]);
        let mut p = project();
        number_wires(&mut p, Rule::Potential, false, |_| None, pos);
        assert_eq!(numbers(&p), [Some("2"), Some("2"), Some("1"), None, Some("24V")]);
    }

    #[test]
    fn page_column_numbers_use_the_templates_columns() {
        let mut p = project();
        p.meta.template.columns = 20;
        number_wires(&mut p, Rule::PageColumn, false, |_| None, pos);
        // A3 with 20 columns of 20 mm from x = 10: not the built-in 3.8, 3.2 and 3.5.
        assert_eq!(
            numbers(&p),
            [Some("3.15"), Some("3.15"), Some("3.3"), None, Some("3.10")]
        );
    }

    #[test]
    fn existing_numbers_are_kept_unless_overwritten() {
        let mut p = project();
        p.pages[0].wires[1].number = Some("1".into());
        number_wires(&mut p, Rule::Sequential, false, |_| None, pos);
        // a/b keep 1 (now on both wires); the others skip it.
        assert_eq!(numbers(&p), [Some("1"), Some("1"), Some("2"), None, Some("3")]);
        number_wires(&mut p, Rule::Sequential, true, |_| None, pos);
        assert_eq!(numbers(&p), [Some("3"), Some("3"), Some("1"), None, Some("2")]);
    }

    #[test]
    fn kept_numbers_are_trimmed_so_they_are_not_given_twice() {
        let mut p = project();
        p.pages[0].wires[2].number = Some(" 1 ".into());
        number_wires(&mut p, Rule::Sequential, false, |_| None, pos);
        // c keeps 1 without the spaces; the others get 2 and 3, not 1 again.
        assert_eq!(numbers(&p), [Some("3"), Some("3"), Some("1"), None, Some("2")]);
    }

    #[test]
    fn potential_names_are_trimmed_so_they_do_not_repeat_a_kept_number() {
        let mut p = project();
        p.pages[0].wires[2].number = Some("1".into());
        p.pages[0].wires[4].potential = Some(" 1 ".into());
        number_wires(&mut p, Rule::Potential, false, |_| None, pos);
        // c keeps 1; e's potential reads as 1 too, so it falls back to the next free number.
        assert_eq!(numbers(&p), [Some("3"), Some("3"), Some("1"), None, Some("2")]);
    }

    #[test]
    fn overview_pages_are_left_alone_and_take_no_numbers() {
        let mut p = project();
        // An overview as page 1 draws a connection again; it comes first in drawing order.
        let mut overview = Page::new("p1", "1", "Overview");
        overview.overview = true;
        overview.wires = vec![wire("o", pin("s1", "1"), pin("s2", "2"), None, None)];
        p.pages.insert(0, overview);
        number_wires(&mut p, Rule::Sequential, false, |_| None, pos);
        // The schematic page is numbered as without the overview; the overview's wire is not.
        assert_eq!(p.pages[0].wires[0].number, None);
        let schematic: Vec<Option<&str>> = p.pages[1].wires.iter().map(|w| w.number.as_deref()).collect();
        assert_eq!(schematic, [Some("3"), Some("3"), Some("1"), None, Some("2")]);
    }

    #[test]
    fn same_column_gets_a_suffix_and_hostile_positions_are_harmless() {
        let mut p = project();
        p.pages[0].wires[2] = wire("c", pin("s3", "29"), pin("s4", "28"), None, None);
        number_wires(&mut p, Rule::PageColumn, false, |_| None, pos);
        assert_eq!(numbers(&p)[2], Some("3.7"));
        assert_eq!(numbers(&p)[0], Some("3.8"));
        let mut p = project();
        number_wires(
            &mut p,
            Rule::PageColumn,
            false,
            |_| None,
            |_, _| Some(Point::new(f64::NAN, f64::INFINITY)),
        );
        // Every wire that touches a pin still gets a number.
        assert_eq!(numbers(&p).iter().filter(|n| n.is_some()).count(), 4);
    }
}

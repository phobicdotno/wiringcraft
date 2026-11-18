//! Eplan pages rebuilt as drawn: free graphics, device symbols at their original positions and
//! turned as in Eplan, and wires traced between connection points the way Eplan connects them
//! automatically.
//!
//! Eplan keeps symbol graphics in its symbol libraries, not in the project, so each device gets a
//! WiringCraft symbol of the same kind (make contact, break contact, changeover, coil, lamp,
//! terminal, multi-pole switch, or a plain box) with Eplan's connection point names. Symbols are
//! laid out on Eplan's 4 mm grid with the first connection point at the symbol origin, as Eplan
//! does, so wires meet the pins.
//!
//! Eplan draws a wire between two connection points that face each other on one line with
//! nothing between them, and corners and junctions are small connection symbols of their own
//! ([`EplanNode`]). [`pages`] does the same.

use std::collections::{BTreeMap, BTreeSet};

use wc_core::geom::{Align, Point, Rotation, Shape};
use wc_core::symbol::{Pin, PinDir, SymbolRole};
use wc_core::{Endpoint, Page, PlacedSymbol, Symbol, Wire};

use super::{EplanConnection, EplanFunction, EplanNode, EplanProject, device_tag, split_end};

/// Function categories (Eplan's function definition numbers) this module draws specially.
pub const KIND_TERMINAL: u16 = 100;
pub const KIND_BREAKER: u16 = 501;
pub const KIND_BUTTON_CONTACT: u16 = 600;
pub const KIND_LAMP: u16 = 900;
/// Relay and contactor contacts.
pub const KIND_CONTACTS: std::ops::Range<u16> = 200..300;

/// Where Eplan's page origin (bottom left, y up) lands on a WiringCraft A3 page (top left,
/// y down): Eplan frames are drawn inside a 10 mm margin on the same sheet.
const LEFT: f64 = 10.0;
const BOTTOM: f64 = 293.0;
/// Two points closer than this are on one line.
const ALIGNED: f64 = 0.05;
/// Pages drawn at a larger scale than this are mounting layouts: graphics only, no symbols.
const MAX_SCHEMATIC_SCALE: f64 = 2.0;
/// Pin spacing: Eplan's grid.
const GRID: f64 = 4.0;
/// Length of two-pin symbols, top pin to bottom pin.
const LENGTH: f64 = 8.0;

/// The rebuilt pages, the symbols they use and the Eplan device names placed on them.
#[derive(Debug, Default)]
pub struct Drawing {
    pub pages: Vec<Page>,
    pub symbols: Vec<Symbol>,
    pub drawn: BTreeSet<String>,
    pub warnings: Vec<String>,
}

/// Directions a wire node connects in, as up, right, down, left. `None` for node symbols this
/// importer does not know.
pub fn node_directions(symbol: u16, variant: u8) -> Option<[bool; 4]> {
    let dirs = match symbol {
        // Corner: right and down in variant 0, each variant a quarter turn further.
        70 => {
            let mut d = [false, true, true, false];
            for _ in 0..variant % 4 {
                // A quarter turn counter-clockwise on paper: up -> left -> down -> right -> up.
                d = [d[1], d[2], d[3], d[0]];
            }
            d
        }
        // Junctions off a vertical wire, and off a horizontal one.
        66 => [true, true, true, false],
        67 => [true, false, true, true],
        71 => [false, true, true, true],
        72 => [true, true, false, true],
        _ => return None,
    };
    Some(dirs)
}

pub(crate) fn rotation_of(variant: u8) -> (Rotation, bool) {
    let r = match variant % 4 {
        0 => Rotation::R0,
        1 => Rotation::R270,
        2 => Rotation::R180,
        _ => Rotation::R90,
    };
    (r, variant >= 4)
}

pub(crate) fn vector(d: PinDir) -> Point {
    match d {
        PinDir::Up => Point::new(0.0, -1.0),
        PinDir::Down => Point::new(0.0, 1.0),
        PinDir::Left => Point::new(-1.0, 0.0),
        PinDir::Right => Point::new(1.0, 0.0),
    }
}

/// Index into an up, right, down, left array.
pub(crate) fn dir_index(v: Point) -> usize {
    if v.y < -0.5 {
        0
    } else if v.x > 0.5 {
        1
    } else if v.y > 0.5 {
        2
    } else {
        3
    }
}

fn safe(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    s.trim_matches('-').chars().take(60).collect()
}

fn line(ax: f64, ay: f64, bx: f64, by: f64) -> Shape {
    Shape::Line {
        a: Point::new(ax, ay),
        b: Point::new(bx, by),
    }
}

fn label(x: f64, y: f64, text: &str) -> Shape {
    Shape::Text {
        at: Point::new(x, y),
        text: text.to_owned(),
        height: 1.8,
        align: Align::Left,
        angle: 0.0,
    }
}

fn pin(name: &str, x: f64, y: f64, dir: PinDir) -> Pin {
    Pin {
        name: name.to_owned(),
        at: Point::new(x, y),
        dir,
    }
}

/// Pin names: Eplan's, with blanks numbered and repeats marked so each is unique.
fn pin_names(f: &EplanFunction) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (k, p) in f.pins.iter().enumerate() {
        let mut name = if p.trim().is_empty() {
            (k + 1).to_string()
        } else {
            p.trim().to_owned()
        };
        while out.contains(&name) {
            name.push('\'');
        }
        out.push(name);
    }
    out
}

/// A make or break contact: top pin at the origin, bottom pin `LENGTH` below.
fn contact(g: &mut Vec<Shape>, x: f64, make: bool) {
    g.push(line(x, 0.0, x, 2.5));
    g.push(line(x, LENGTH, x, 5.5));
    if make {
        g.push(line(x, 5.5, x - 2.2, 2.0));
    } else {
        g.push(line(x, 2.5, x + 2.0, 2.5));
        g.push(line(x, 5.5, x + 2.4, 2.1));
    }
}

/// Last digit of a contact pin name (`13` -> 3).
fn last_digit(s: &str) -> Option<u32> {
    s.chars().last().and_then(|c| c.to_digit(10))
}

/// The symbol for a function, or `None` when it has no connection points (black boxes, cable
/// definitions, push-button heads).
pub fn symbol_for(f: &EplanFunction) -> Option<Symbol> {
    let names = pin_names(f);
    let n = names.len();
    if n == 0 {
        return None;
    }
    let mut g = Vec::new();
    let mut pins = Vec::new();
    let is_coil = n == 2 && names.iter().all(|p| p.to_ascii_uppercase().starts_with('A'));
    let contacts = KIND_CONTACTS.contains(&f.kind) || f.kind == KIND_BUTTON_CONTACT;
    let (kind, role, tag_at) = if f.kind == KIND_TERMINAL {
        g.push(Shape::Circle {
            center: Point::new(0.0, 0.0),
            radius: 1.0,
            filled: false,
        });
        if !f.terminal.is_empty() {
            g.push(label(1.5, -1.2, &f.terminal));
        }
        pins.push(pin("1", 0.0, -1.0, PinDir::Up));
        pins.push(pin("2", 0.0, 1.0, PinDir::Down));
        ("terminal", SymbolRole::Terminal, Point::new(-8.0, 0.8))
    } else if is_coil {
        g.push(line(0.0, 0.0, 0.0, 2.0));
        g.push(line(0.0, 6.0, 0.0, LENGTH));
        g.push(Shape::Rect {
            min: Point::new(-3.0, 2.0),
            max: Point::new(3.0, 6.0),
        });
        pins.push(pin(&names[0], 0.0, 0.0, PinDir::Up));
        pins.push(pin(&names[1], 0.0, LENGTH, PinDir::Down));
        ("coil", SymbolRole::Main, Point::new(-12.0, 4.5))
    } else if f.kind == KIND_LAMP && n == 2 {
        g.push(line(0.0, 0.0, 0.0, 2.0));
        g.push(line(0.0, 6.0, 0.0, LENGTH));
        g.push(Shape::Circle {
            center: Point::new(0.0, 4.0),
            radius: 2.0,
            filled: false,
        });
        g.push(line(-1.4, 2.6, 1.4, 5.4));
        g.push(line(-1.4, 5.4, 1.4, 2.6));
        pins.push(pin(&names[0], 0.0, 0.0, PinDir::Up));
        pins.push(pin(&names[1], 0.0, LENGTH, PinDir::Down));
        ("lamp", SymbolRole::Standalone, Point::new(-10.0, 4.5))
    } else if contacts && n == 2 {
        let make = if f.kind == KIND_BUTTON_CONTACT {
            f.detail != 2
        } else {
            !(names.iter().all(|p| matches!(last_digit(p), Some(1 | 2)))
                || names.iter().all(|p| matches!(last_digit(p), Some(5 | 6))))
        };
        contact(&mut g, 0.0, make);
        pins.push(pin(&names[0], 0.0, 0.0, PinDir::Up));
        pins.push(pin(&names[1], 0.0, LENGTH, PinDir::Down));
        let role = if f.kind == KIND_BUTTON_CONTACT {
            SymbolRole::Standalone
        } else {
            SymbolRole::Auxiliary
        };
        let kind = match (f.kind == KIND_BUTTON_CONTACT, make) {
            (true, true) => "button-make",
            (true, false) => "button-break",
            (false, true) => "make",
            (false, false) => "break",
        };
        (kind, role, Point::new(-10.0, 4.5))
    } else if contacts && n == 3 {
        // Break (top left), common (bottom right), make (top right); Eplan lists them in
        // that order. Their last digits are 2, 1 and 4, or 6, 5 and 8 on timed contacts.
        let mut slots: [Option<String>; 3] = [None, None, None];
        for p in &names {
            let slot = match last_digit(p) {
                Some(2 | 6) => 0,
                Some(1 | 5) => 1,
                Some(4 | 8) => 2,
                _ => continue,
            };
            if slots[slot].is_none() {
                slots[slot] = Some(p.clone());
            }
        }
        if slots.iter().any(Option::is_none) {
            slots = [Some(names[0].clone()), Some(names[1].clone()), Some(names[2].clone())];
        }
        let [Some(brk), Some(common), Some(mk)] = slots else {
            return None;
        };
        g.push(line(0.0, 0.0, 0.0, 2.5));
        g.push(line(0.0, 2.5, 1.6, 2.5));
        g.push(line(GRID, 0.0, GRID, 2.5));
        g.push(line(GRID, LENGTH, GRID, 5.5));
        g.push(line(GRID, 5.5, 1.2, 2.2));
        pins.push(pin(&brk, 0.0, 0.0, PinDir::Up));
        pins.push(pin(&common, GRID, LENGTH, PinDir::Down));
        pins.push(pin(&mk, GRID, 0.0, PinDir::Up));
        ("changeover", SymbolRole::Auxiliary, Point::new(-10.0, 4.5))
    } else if f.kind == KIND_BREAKER && n.is_multiple_of(2) {
        for k in 0..n / 2 {
            let x = GRID * k as f64;
            contact(&mut g, x, true);
            g.push(line(x - 1.5, 2.5, x - 0.5, 2.5));
            pins.push(pin(&names[2 * k], x, 0.0, PinDir::Up));
            pins.push(pin(&names[2 * k + 1], x, LENGTH, PinDir::Down));
        }
        ("switch", SymbolRole::Standalone, Point::new(-10.0, 4.5))
    } else {
        let w = GRID * (n as f64 - 1.0);
        g.push(Shape::Rect {
            min: Point::new(-2.0, 2.0),
            max: Point::new(w + 2.0, LENGTH),
        });
        for (k, name) in names.iter().enumerate() {
            let x = GRID * k as f64;
            g.push(line(x, 0.0, x, 2.0));
            pins.push(pin(name, x, 0.0, PinDir::Up));
        }
        ("box", SymbolRole::Standalone, Point::new(-10.0, 5.5))
    };
    // Connection point names beside the pins (the terminal shows its designation instead).
    if kind != "terminal" {
        for p in &pins {
            let y = if p.dir == PinDir::Up {
                p.at.y + 1.8
            } else {
                p.at.y - 0.4
            };
            g.push(label(p.at.x + 0.6, y, &p.name));
        }
    }
    let key: Vec<&str> = pins.iter().map(|p| p.name.as_str()).collect();
    let id = format!("eplan.{kind}.{}", safe(&key.join("-")));
    Some(Symbol {
        id: if kind == "terminal" {
            format!("eplan.terminal.{}", safe(&f.terminal))
        } else {
            id
        },
        name: format!("Eplan {kind} ({})", key.join(", ")),
        category: "imported".into(),
        letter_code: String::new(),
        role,
        graphics: g,
        pins,
        tag_at,
        description: "Drawn by the Eplan import in place of the symbol from Eplan's library".into(),
    })
}

/// A connection point on a rebuilt page.
struct End {
    at: Point,
    /// Up, right, down, left.
    dirs: [bool; 4],
    endpoint: Endpoint,
}

/// Give drawn wires the cross-section and colour of the Eplan connection they draw: the shortest
/// chain of wires on this page between the connection's two pins (through junctions). A terminal
/// end (`X2:3`) is any pin of terminal 3 of strip X2. A wire keeps the first spec it gets.
/// Returns how many wires got one.
pub(super) fn spec_from_connections(
    page: &mut Page,
    owners: &[(String, String, String)],
    connections: &[EplanConnection],
) -> usize {
    let key = |e: &Endpoint| match e {
        Endpoint::Pin { item, pin } => format!("p\u{1}{item}\u{1}{pin}"),
        Endpoint::Point { at } => format!(
            "q\u{1}{}\u{1}{}",
            (at.x * 100.0).round() as i64,
            (at.y * 100.0).round() as i64
        ),
    };
    let mut adj: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
    for (i, w) in page.wires.iter().enumerate() {
        let (a, b) = (key(&w.from), key(&w.to));
        adj.entry(a.clone()).or_default().push((i, b.clone()));
        adj.entry(b).or_default().push((i, a));
    }
    let pins_of = |dev: &str, pin: &str| -> Vec<String> {
        let mut out = Vec::new();
        for (item, _, terminal) in owners.iter().filter(|(_, t, _)| t == dev) {
            if terminal.is_empty() {
                let k = format!("p\u{1}{item}\u{1}{pin}");
                if adj.contains_key(&k) {
                    out.push(k);
                }
            } else if terminal == pin || pin.split(':').next() == Some(terminal.as_str()) {
                let prefix = format!("p\u{1}{item}\u{1}");
                out.extend(adj.keys().filter(|k| k.starts_with(&prefix)).cloned());
            }
        }
        out
    };
    let mut given = 0;
    for c in connections {
        let spec = c.spec();
        if spec.is_empty() {
            continue;
        }
        let ((da, Some(pa)), (db, Some(pb))) = (split_end(&c.from), split_end(&c.to)) else {
            continue;
        };
        let (sources, targets) = (pins_of(da, pa), pins_of(db, pb));
        if sources.is_empty() || targets.is_empty() {
            continue;
        }
        // Breadth-first from every source pin; the first target reached ends the search.
        let mut prev: BTreeMap<String, (usize, String)> = BTreeMap::new();
        let mut seen: BTreeSet<String> = sources.iter().cloned().collect();
        let mut queue: std::collections::VecDeque<String> = sources.into_iter().collect();
        let mut found = None;
        while let Some(n) = queue.pop_front() {
            if targets.contains(&n) {
                found = Some(n);
                break;
            }
            for (w, next) in adj.get(&n).into_iter().flatten() {
                if seen.insert(next.clone()) {
                    prev.insert(next.clone(), (*w, n.clone()));
                    queue.push_back(next.clone());
                }
            }
        }
        let mut at = found;
        while let Some(n) = at {
            let Some((w, back)) = prev.get(&n) else { break };
            if let Some(wire) = page.wires.get_mut(*w)
                && wire.spec.is_empty()
            {
                wire.spec = spec.clone();
                given += 1;
            }
            at = Some(back.clone());
        }
    }
    given
}

/// Wires between connection points that face each other on one line with nothing between.
fn autoconnect(ends: &[End]) -> Vec<(usize, usize)> {
    let mut out = BTreeSet::new();
    let steps = [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)];
    for (i, a) in ends.iter().enumerate() {
        for (d, (dx, dy)) in steps.iter().enumerate() {
            if !a.dirs[d] {
                continue;
            }
            let mut best: Option<(f64, usize)> = None;
            for (j, b) in ends.iter().enumerate() {
                if i == j {
                    continue;
                }
                let (vx, vy) = (b.at.x - a.at.x, b.at.y - a.at.y);
                let along = vx * dx + vy * dy;
                let across = (vx * dy - vy * dx).abs();
                if along > ALIGNED && across < ALIGNED && best.is_none_or(|(d0, _)| along < d0) {
                    best = Some((along, j));
                }
            }
            if let Some((_, j)) = best
                && ends[j].dirs[(d + 2) % 4]
            {
                out.insert((i.min(j), i.max(j)));
            }
        }
    }
    out.into_iter().collect()
}

fn to_page(p: Point, scale: f64) -> Point {
    Point::new(p.x / scale + LEFT, BOTTOM - p.y / scale)
}

fn shape_to_page(s: &Shape, scale: f64) -> Option<Shape> {
    Some(match s {
        Shape::Line { a, b } => Shape::Line {
            a: to_page(*a, scale),
            b: to_page(*b, scale),
        },
        Shape::Rect { min, max } => {
            let (a, b) = (to_page(*min, scale), to_page(*max, scale));
            Shape::Rect {
                min: Point::new(a.x.min(b.x), a.y.min(b.y)),
                max: Point::new(a.x.max(b.x), a.y.max(b.y)),
            }
        }
        Shape::Circle { center, radius, filled } => Shape::Circle {
            center: to_page(*center, scale),
            radius: radius / scale,
            filled: *filled,
        },
        Shape::Text {
            at,
            text,
            height,
            align,
            angle,
        } => Shape::Text {
            at: to_page(*at, scale),
            text: text.clone(),
            height: (height / scale).clamp(1.0, 20.0),
            align: *align,
            angle: *angle,
        },
        _ => return None,
    })
}

/// Rebuild every Eplan page that has graphics or placed devices. Pages are numbered from
/// `first_page_number`, with ids `eplan-page-<n>`. Positions are divided by the page scale.
/// Devices are placed only on schematic pages; mounting layouts (drawn at more than 1:2) get
/// their graphics only.
pub fn pages(p: &EplanProject, first_page_number: usize) -> Drawing {
    let mut out = Drawing::default();
    let mut symbols: BTreeMap<String, Symbol> = BTreeMap::new();
    let mut unknown_nodes = 0;
    for pg in &p.pages {
        let scale = if pg.scale.is_finite() && pg.scale > 0.0 {
            pg.scale
        } else {
            1.0
        };
        let schematic = scale <= MAX_SCHEMATIC_SCALE;
        let graphics: Vec<Shape> = p
            .graphics
            .iter()
            .filter(|g| g.page == pg.id)
            .filter_map(|g| shape_to_page(&g.shape, scale))
            .collect();
        let functions: Vec<(&EplanFunction, Point)> = p
            .functions
            .iter()
            .filter(|f| schematic && f.page == Some(pg.id))
            .filter_map(|f| f.at.map(|at| (f, at)))
            .collect();
        if graphics.is_empty() && functions.is_empty() {
            continue;
        }
        let n = out.pages.len() + 1;
        let title = if pg.description.is_empty() {
            format!("Eplan page {}", pg.name)
        } else {
            format!("Eplan page {}: {}", pg.name, pg.description)
        };
        let mut page = Page::new(
            format!("eplan-page-{n}"),
            (first_page_number + n - 1).to_string(),
            title,
        );
        page.size = wc_core::PageSize::A3;
        page.graphics = graphics;
        let mut ends: Vec<End> = Vec::new();
        // Placed symbol id, Eplan device name and terminal number, to find connection ends.
        let mut owners: Vec<(String, String, String)> = Vec::new();
        for (f, at) in functions {
            let at = to_page(at, scale);
            let Some(sym) = symbol_for(f) else {
                // No connection points: show the name of cables and the like where Eplan does.
                if f.kind == 400 {
                    page.graphics.push(Shape::Text {
                        at,
                        text: f.tag.clone(),
                        height: 2.5,
                        align: Align::Left,
                        angle: 0.0,
                    });
                }
                continue;
            };
            let name = if f.kind == KIND_TERMINAL && !f.terminal.is_empty() {
                // Shown as `-X1.7`: strip X1, terminal 7.
                format!("{}-{}", f.tag, f.terminal)
            } else {
                f.tag.clone()
            };
            let (rotation, mirror) = rotation_of(f.variant);
            let item = page.next_id("s");
            owners.push((item.clone(), f.tag.clone(), f.terminal.clone()));
            for pn in &sym.pins {
                let mut dirs = [false; 4];
                dirs[dir_index(vector(pn.dir).transform(rotation, mirror))] = true;
                ends.push(End {
                    at: at + pn.at.transform(rotation, mirror),
                    dirs,
                    endpoint: Endpoint::Pin {
                        item: item.clone(),
                        pin: pn.name.clone(),
                    },
                });
            }
            page.symbols.push(PlacedSymbol {
                id: item,
                symbol: sym.id.clone(),
                at,
                rotation,
                mirror,
                tag: device_tag(&name),
            });
            out.drawn.insert(f.tag.clone());
            symbols.entry(sym.id.clone()).or_insert(sym);
        }
        for node in p.nodes.iter().filter(|nd| schematic && nd.page == pg.id) {
            let EplanNode {
                at, symbol, variant, ..
            } = node;
            // A node of a kind not known here still joins wires: in every direction where a
            // pin or node facing it lines up, which is how a cross or T reads on the page.
            let dirs = node_directions(*symbol, *variant).unwrap_or_else(|| {
                unknown_nodes += 1;
                [true; 4]
            });
            let at = to_page(*at, scale);
            ends.push(End {
                at,
                dirs,
                endpoint: Endpoint::Point { at },
            });
        }
        for (a, b) in autoconnect(&ends) {
            let id = page.next_id("w");
            page.wires.push(Wire {
                id,
                from: ends[a].endpoint.clone(),
                to: ends[b].endpoint.clone(),
                via: vec![],
                number: None,
                potential: None,
                cable: None,
                route: Default::default(),
                spec: Default::default(),
            });
        }
        spec_from_connections(&mut page, &owners, &p.connections);
        if page.symbols.is_empty() && page.graphics.is_empty() {
            continue;
        }
        out.pages.push(page);
    }
    if unknown_nodes > 0 {
        out.warnings.push(format!(
            "{unknown_nodes} wire corner(s) or junction(s) of an unknown kind were joined in every direction a \
             wire lines up; check them"
        ));
    }
    out.symbols = symbols.into_values().collect();
    out
}

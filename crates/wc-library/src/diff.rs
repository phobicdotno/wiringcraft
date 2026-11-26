//! Human-readable differences between two versions of a library item, for the update view.

use wc_core::{Part, Symbol};

pub fn diff_symbols(old: &Symbol, new: &Symbol) -> Vec<String> {
    let mut out = Vec::new();
    if old.name != new.name {
        out.push(format!("name: {:?} -> {:?}", old.name, new.name));
    }
    if old.category != new.category {
        out.push(format!("category: {:?} -> {:?}", old.category, new.category));
    }
    if old.letter_code != new.letter_code {
        out.push(format!("letter code: {:?} -> {:?}", old.letter_code, new.letter_code));
    }
    if old.role != new.role {
        out.push(format!("role: {:?} -> {:?}", old.role, new.role));
    }
    for p in &old.pins {
        match new.pin(&p.name) {
            None => out.push(format!("pin {} removed (wires on it will be left open)", p.name)),
            Some(n) if n.at != p.at => out.push(format!(
                "pin {} moved ({}, {}) -> ({}, {})",
                p.name, p.at.x, p.at.y, n.at.x, n.at.y
            )),
            Some(_) => {}
        }
    }
    for p in &new.pins {
        if old.pin(&p.name).is_none() {
            out.push(format!("pin {} added", p.name));
        }
    }
    if old.graphics != new.graphics {
        out.push(format!(
            "graphics changed ({} -> {} shapes)",
            old.graphics.len(),
            new.graphics.len()
        ));
    }
    if old.tag_at != new.tag_at {
        out.push("tag position moved".into());
    }
    if old.description != new.description {
        out.push("description changed".into());
    }
    out
}

pub fn diff_parts(old: &Part, new: &Part) -> Vec<String> {
    let mut out = Vec::new();
    let field = |out: &mut Vec<String>, name: &str, a: &str, b: &str| {
        if a != b {
            out.push(format!("{name}: {a:?} -> {b:?}"));
        }
    };
    field(&mut out, "manufacturer", &old.manufacturer, &new.manufacturer);
    field(&mut out, "order number", &old.order_number, &new.order_number);
    field(&mut out, "description", &old.description, &new.description);
    field(
        &mut out,
        "symbol",
        old.symbol.as_deref().unwrap_or(""),
        new.symbol.as_deref().unwrap_or(""),
    );
    for (k, v) in &old.properties {
        match new.properties.get(k) {
            None => out.push(format!("property {k} removed")),
            Some(n) if n != v => out.push(format!("{k}: {v:?} -> {n:?}")),
            Some(_) => {}
        }
    }
    for k in new.properties.keys() {
        if !old.properties.contains_key(k) {
            out.push(format!("property {k} added"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wc_core::geom::{Point, Shape};
    use wc_core::symbol::{Pin, PinDir, SymbolRole};

    fn coil() -> Symbol {
        let pin = |name: &str, y: f64| Pin {
            name: name.into(),
            at: Point::new(0.0, y),
            dir: PinDir::Up,
        };
        Symbol {
            id: "coil".into(),
            name: "Coil".into(),
            category: "relays".into(),
            letter_code: "K".into(),
            role: SymbolRole::Main,
            graphics: vec![Shape::Line {
                a: Point::new(0.0, 0.0),
                b: Point::new(0.0, 10.0),
            }],
            pins: vec![pin("A1", 0.0), pin("A2", 10.0)],
            tag_at: Point::default(),
            description: String::new(),
            source: String::new(),
        }
    }

    fn contactor() -> Part {
        Part {
            id: "acme.c1".into(),
            manufacturer: "Acme".into(),
            order_number: "C1".into(),
            description: "Contactor".into(),
            symbol: Some("coil".into()),
            properties: [("Voltage", "24 V"), ("Width", "45")]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            ..Part::default()
        }
    }

    #[test]
    fn the_same_item_has_no_changes() {
        assert!(diff_symbols(&coil(), &coil()).is_empty());
        assert!(diff_parts(&contactor(), &contactor()).is_empty());
    }

    #[test]
    fn every_symbol_change_gets_its_line() {
        let old = coil();
        let mut new = coil();
        new.name = "Relay coil".into();
        new.category = "coils".into();
        new.letter_code = "Q".into();
        new.role = SymbolRole::Auxiliary;
        new.pins.retain(|p| p.name != "A2");
        new.pins.push(Pin {
            name: "A3".into(),
            at: Point::new(5.0, 10.0),
            dir: PinDir::Down,
        });
        new.graphics.push(Shape::Line {
            a: Point::new(-2.0, 5.0),
            b: Point::new(2.0, 5.0),
        });
        new.tag_at = Point::new(4.0, 0.0);
        new.description = "A relay coil".into();
        assert_eq!(
            diff_symbols(&old, &new),
            vec![
                "name: \"Coil\" -> \"Relay coil\"",
                "category: \"relays\" -> \"coils\"",
                "letter code: \"K\" -> \"Q\"",
                "role: Main -> Auxiliary",
                "pin A2 removed (wires on it will be left open)",
                "pin A3 added",
                "graphics changed (1 -> 2 shapes)",
                "tag position moved",
                "description changed",
            ]
        );
    }

    #[test]
    fn a_pin_that_moved_says_from_where_to_where() {
        let mut new = coil();
        new.pins[1].at = Point::new(0.0, 15.0);
        assert_eq!(diff_symbols(&coil(), &new), vec!["pin A2 moved (0, 10) -> (0, 15)"]);
    }

    #[test]
    fn every_part_change_gets_its_line() {
        let old = contactor();
        let mut new = contactor();
        new.manufacturer = "Acme AG".into();
        new.order_number = "C1-24".into();
        new.description = "Contactor 24 V".into();
        new.symbol = None;
        new.properties.insert("Voltage".into(), "230 V".into());
        new.properties.remove("Width");
        new.properties.insert("Height".into(), "80".into());
        assert_eq!(
            diff_parts(&old, &new),
            vec![
                "manufacturer: \"Acme\" -> \"Acme AG\"",
                "order number: \"C1\" -> \"C1-24\"",
                "description: \"Contactor\" -> \"Contactor 24 V\"",
                "symbol: \"coil\" -> \"\"",
                "Voltage: \"24 V\" -> \"230 V\"",
                "property Width removed",
                "property Height added",
            ]
        );
    }
}

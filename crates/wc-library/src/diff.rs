//! Human-readable differences between two versions of a library item, for the update view.

use wc_core::{Contacts, ModelSize, Part, Symbol};

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
        let Some(n) = new.pin(&p.name) else {
            out.push(format!("pin {} removed (wires on it will be left open)", p.name));
            continue;
        };
        if n.at != p.at {
            out.push(format!(
                "pin {} moved ({}, {}) -> ({}, {})",
                p.name, p.at.x, p.at.y, n.at.x, n.at.y
            ));
        }
        if n.dir != p.dir {
            out.push(format!("pin {} turned {:?} -> {:?}", p.name, p.dir, n.dir));
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
    if old.contacts != new.contacts {
        let contacts = |c: &Option<Contacts>| c.as_ref().map_or("not given".into(), Contacts::describe);
        out.push(format!(
            "contacts: {} -> {}",
            contacts(&old.contacts),
            contacts(&new.contacts)
        ));
    }
    match (&old.footprint, &new.footprint) {
        (None, Some(_)) => out.push("footprint drawing added".into()),
        (Some(_), None) => out.push("footprint drawing removed".into()),
        (Some(a), Some(b)) if a != b => out.push("footprint drawing changed".into()),
        _ => {}
    }
    if old.model != new.model {
        let size = |m: &Option<ModelSize>| {
            m.map_or("not given".into(), |m| {
                format!("{}×{}×{} mm", m.width, m.height, m.depth)
            })
        };
        out.push(format!("3D size: {} -> {}", size(&old.model), size(&new.model)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wc_core::Footprint;
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
    fn a_pin_that_only_turned_gets_its_line() {
        let mut new = coil();
        new.pins[0].dir = PinDir::Left;
        let changes = diff_symbols(&coil(), &new);
        assert!(!changes.is_empty());
        assert_eq!(changes, vec!["pin A1 turned Up -> Left"]);
        new.pins[0].at = Point::new(5.0, 0.0);
        assert_eq!(
            diff_symbols(&coil(), &new),
            vec!["pin A1 moved (0, 0) -> (5, 0)", "pin A1 turned Up -> Left"]
        );
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

    #[test]
    fn a_changed_contact_list_says_from_what_to_what() {
        let mut old = contactor();
        old.contacts = Some(Contacts {
            main: 3,
            no: 1,
            ..Contacts::default()
        });
        let mut new = old.clone();
        new.contacts = Some(Contacts {
            main: 3,
            no: 1,
            nc: 1,
            co: 0,
        });
        assert_eq!(
            diff_parts(&old, &new),
            vec!["contacts: 3 main, 1 NO -> 3 main, 1 NO, 1 NC"]
        );
        new.contacts = None;
        assert_eq!(diff_parts(&old, &new), vec!["contacts: 3 main, 1 NO -> not given"]);
    }

    #[test]
    fn a_changed_footprint_drawing_gets_its_line() {
        let line = |x: f64| Shape::Line {
            a: Point::new(0.0, 0.0),
            b: Point::new(x, 90.0),
        };
        let mut old = contactor();
        old.footprint = Footprint::from_shapes(vec![line(45.0)]);
        let mut new = old.clone();
        new.footprint = Footprint::from_shapes(vec![line(45.0), line(20.0)]);
        assert_eq!(diff_parts(&old, &new), vec!["footprint drawing changed"]);
        assert_eq!(diff_parts(&contactor(), &old), vec!["footprint drawing added"]);
        assert_eq!(diff_parts(&old, &contactor()), vec!["footprint drawing removed"]);
    }

    #[test]
    fn a_changed_3d_size_says_from_what_to_what() {
        let size = |depth: f64| ModelSize {
            width: 45.0,
            height: 90.0,
            depth,
        };
        let mut old = contactor();
        old.model = Some(size(70.0));
        let mut new = old.clone();
        new.model = Some(size(75.5));
        assert_eq!(diff_parts(&old, &new), vec!["3D size: 45×90×70 mm -> 45×90×75.5 mm"]);
        assert_eq!(
            diff_parts(&contactor(), &old),
            vec!["3D size: not given -> 45×90×70 mm"]
        );
    }

    #[test]
    fn id_and_source_are_not_symbol_changes() {
        let mut new = coil();
        new.id = "coil2".into();
        new.source = "vendor/coil.svg".into();
        assert!(diff_symbols(&coil(), &new).is_empty());
    }

    #[test]
    fn reordered_pins_are_not_a_change() {
        let mut new = coil();
        new.pins.reverse();
        assert!(diff_symbols(&coil(), &new).is_empty());
    }

    #[test]
    fn graphics_with_the_same_count_still_count_as_changed() {
        let mut new = coil();
        new.graphics = vec![Shape::Line {
            a: Point::new(0.0, 0.0),
            b: Point::new(0.0, 12.0),
        }];
        assert_eq!(diff_symbols(&coil(), &new), vec!["graphics changed (1 -> 1 shapes)"]);
    }

    #[test]
    fn every_removed_pin_comes_before_every_added_pin() {
        let old = coil();
        let mut new = coil();
        new.pins = vec![Pin {
            name: "X".into(),
            at: Point::new(0.0, 0.0),
            dir: PinDir::Up,
        }];
        assert_eq!(
            diff_symbols(&old, &new),
            vec![
                "pin A1 removed (wires on it will be left open)",
                "pin A2 removed (wires on it will be left open)",
                "pin X added",
            ]
        );
        assert_eq!(
            diff_symbols(&new, &old),
            vec![
                "pin X removed (wires on it will be left open)",
                "pin A1 added",
                "pin A2 added",
            ]
        );
    }

    #[test]
    fn part_id_is_not_a_part_change() {
        let mut new = contactor();
        new.id = "acme.c1-renamed".into();
        assert!(diff_parts(&contactor(), &new).is_empty());
    }

    #[test]
    fn a_symbol_given_where_there_was_none_reads_from_empty() {
        let mut old = contactor();
        old.symbol = None;
        assert_eq!(diff_parts(&old, &contactor()), vec!["symbol: \"\" -> \"coil\""]);
        let mut blank = contactor();
        blank.symbol = Some(String::new());
        assert!(diff_parts(&old, &blank).is_empty());
    }

    #[test]
    fn property_lines_follow_the_name_order() {
        let mut old = contactor();
        old.properties.clear();
        let mut new = old.clone();
        for k in ["Zeta", "Alpha", "Mid"] {
            new.properties.insert(k.into(), "1".into());
        }
        assert_eq!(
            diff_parts(&old, &new),
            vec!["property Alpha added", "property Mid added", "property Zeta added"]
        );
        assert_eq!(
            diff_parts(&new, &old),
            vec![
                "property Alpha removed",
                "property Mid removed",
                "property Zeta removed",
            ]
        );
    }

    #[test]
    fn contacts_given_for_the_first_time_read_from_not_given() {
        let mut new = contactor();
        new.contacts = Some(Contacts {
            nc: 2,
            ..Contacts::default()
        });
        assert_eq!(diff_parts(&contactor(), &new), vec!["contacts: not given -> 2 NC"]);
    }

    #[test]
    fn a_removed_3d_size_reads_to_not_given() {
        let mut old = contactor();
        old.model = Some(ModelSize {
            width: 22.5,
            height: 100.0,
            depth: 60.0,
        });
        assert_eq!(
            diff_parts(&old, &contactor()),
            vec!["3D size: 22.5×100×60 mm -> not given"]
        );
    }
}

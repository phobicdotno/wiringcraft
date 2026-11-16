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

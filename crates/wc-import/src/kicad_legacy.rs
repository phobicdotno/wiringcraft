//! KiCad's old symbol libraries (`.lib`, `EESchema-LIBRARY`, KiCad 5 and before), rewritten as
//! a `.kicad_sym` library so [`crate::kicad::symbols`] reads them like a new one.
//!
//! The file is line based: `DEF name ref ...` starts a symbol, `F0`/`F1`/... lines give its
//! fields (reference, value, footprint, datasheet, then own fields), `ALIAS` names other
//! symbols drawn the same, and the lines between `DRAW` and `ENDDRAW` draw it: `P` polyline,
//! `S` rectangle, `C` circle, `A` arc, `B` Bézier curve, `T` text and `X` pin. Lengths are in
//! mils (a thousandth of an inch) with y pointing up. Each drawing item names its unit (0 for
//! every unit) and body style (0 for both, 2 for the De Morgan alternative). Written from the
//! published file format description.

use std::fmt::Write as _;

use crate::{Error, Result};

/// The first word of an old KiCad symbol library.
pub const HEADER: &str = "EESchema-LIBRARY";
/// Millimetres per mil.
const MM: f64 = 0.0254;
/// Most symbols read from one file; real libraries have a few hundred.
const MAX_SYMBOLS: usize = 20_000;

/// Whether `text` is an old KiCad symbol library.
pub fn is_legacy(text: &str) -> bool {
    text.trim_start_matches('\u{feff}').trim_start().starts_with(HEADER)
}

/// A string as an S-expression atom: quoted, with `\` and `"` escaped.
fn q(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Mils as millimetres, written short.
fn mm(v: f64) -> String {
    let v = (v * MM * 10_000.0).round() / 10_000.0;
    // `-0` reads oddly.
    let v = if v == 0.0 { 0.0 } else { v };
    format!("{v}")
}

/// Split a line into words; a word in double quotes keeps its spaces (and `\"` its quote).
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut word = String::new();
        if c == '"' {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(n) = chars.next() {
                            word.push(n);
                        }
                    }
                    '"' => break,
                    c => word.push(c),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                word.push(c);
                chars.next();
            }
        }
        out.push(word);
    }
    out
}

/// A text as KiCad's old files write it: `~` for an empty one, and in unquoted texts `~` for
/// a space.
fn text(word: &str, quoted_spaces: bool) -> String {
    if word == "~" {
        String::new()
    } else if quoted_spaces {
        word.to_owned()
    } else {
        word.replace('~', " ")
    }
}

fn num(w: &[String], i: usize) -> Option<f64> {
    w.get(i)?.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn int(w: &[String], i: usize) -> Option<i64> {
    w.get(i)?.parse::<i64>().ok()
}

/// `(fill (type ...))` for an old fill letter: `F` filled in the line colour, `f` in the
/// background colour, anything else none.
fn fill(letter: Option<&String>) -> &'static str {
    match letter.map(String::as_str) {
        Some("F") => "(fill (type outline))",
        Some("f") => "(fill (type background))",
        _ => "(fill (type none))",
    }
}

/// One symbol being read.
#[derive(Default)]
struct Def {
    name: String,
    reference: String,
    units: i64,
    /// `F` lines as `(property ...)` items.
    properties: Vec<String>,
    aliases: Vec<String>,
    /// Drawing items by (unit, style).
    items: std::collections::BTreeMap<(i64, i64), Vec<String>>,
    line: usize,
}

/// The `(property ...)` of an `F<n>` line: `F0 "R" x y size H|V V|I hjust vjust...`.
fn field(n: usize, w: &[String], def: &Def) -> Option<String> {
    let key = match n {
        0 => "Reference".to_owned(),
        1 => "Value".to_owned(),
        2 => "Footprint".to_owned(),
        3 => "Datasheet".to_owned(),
        // Own fields name themselves after the justification.
        _ => w.get(9).cloned().filter(|k| !k.is_empty())?,
    };
    let value = text(w.get(1)?, true);
    let value = if n == 1 && value.is_empty() {
        def.name.clone()
    } else {
        value
    };
    let (x, y, size) = (num(w, 2)?, num(w, 3)?, num(w, 4).unwrap_or(50.0));
    let angle = if w.get(5).map(String::as_str) == Some("V") {
        90
    } else {
        0
    };
    let hidden = w.get(6).map(String::as_str) == Some("I");
    let justify = justify(w.get(7).map(String::as_str), w.get(8).and_then(|s| s.get(..1)));
    Some(format!(
        "(property {} {} (at {} {} {angle}) (effects (font (size {s} {s})){justify}{}))",
        q(&key),
        q(&value),
        mm(x),
        mm(y),
        if hidden { " hide" } else { "" },
        s = mm(size),
    ))
}

/// ` (justify ...)` for old alignment letters (`L`, `C`, `R` across, `T`, `C`, `B` up and down),
/// or nothing when both are centred.
fn justify(h: Option<&str>, v: Option<&str>) -> String {
    let mut words = Vec::new();
    match h {
        Some("L") => words.push("left"),
        Some("R") => words.push("right"),
        _ => {}
    }
    match v {
        Some("T") => words.push("top"),
        Some("B") => words.push("bottom"),
        _ => {}
    }
    if words.is_empty() {
        String::new()
    } else {
        format!(" (justify {})", words.join(" "))
    }
}

/// The `.kicad_sym` item of one drawing line, with its unit and body style; `None` for a line
/// that is not read (its letter is returned as the warning's word).
fn draw_item(w: &[String]) -> std::result::Result<(i64, i64, String), String> {
    let kind = w.first().map(String::as_str).unwrap_or("");
    let bad = || format!("{kind} line");
    let stroke = |i: usize| format!("(stroke (width {}) (type default))", mm(num(w, i).unwrap_or(0.0)));
    match kind {
        "P" | "B" => {
            // P count unit style width x y ... fill
            let n = int(w, 1).filter(|n| *n >= 2 && *n <= 10_000).ok_or_else(bad)? as usize;
            let (unit, style) = (int(w, 2).ok_or_else(bad)?, int(w, 3).ok_or_else(bad)?);
            let mut pts = String::new();
            for k in 0..n {
                let (x, y) = (num(w, 5 + 2 * k).ok_or_else(bad)?, num(w, 6 + 2 * k).ok_or_else(bad)?);
                let _ = write!(pts, " (xy {} {})", mm(x), mm(y));
            }
            let head = if kind == "B" && n == 4 { "bezier" } else { "polyline" };
            Ok((
                unit,
                style,
                format!("({head} (pts{pts}) {} {})", stroke(4), fill(w.get(5 + 2 * n))),
            ))
        }
        "S" => {
            // S x1 y1 x2 y2 unit style width fill
            let (x1, y1, x2, y2) = (
                num(w, 1).ok_or_else(bad)?,
                num(w, 2).ok_or_else(bad)?,
                num(w, 3).ok_or_else(bad)?,
                num(w, 4).ok_or_else(bad)?,
            );
            Ok((
                int(w, 5).ok_or_else(bad)?,
                int(w, 6).ok_or_else(bad)?,
                format!(
                    "(rectangle (start {} {}) (end {} {}) {} {})",
                    mm(x1),
                    mm(y1),
                    mm(x2),
                    mm(y2),
                    stroke(7),
                    fill(w.get(8))
                ),
            ))
        }
        "C" => {
            // C x y r unit style width fill
            let (x, y, r) = (
                num(w, 1).ok_or_else(bad)?,
                num(w, 2).ok_or_else(bad)?,
                num(w, 3).ok_or_else(bad)?,
            );
            Ok((
                int(w, 4).ok_or_else(bad)?,
                int(w, 5).ok_or_else(bad)?,
                format!(
                    "(circle (center {} {}) (radius {}) {} {})",
                    mm(x),
                    mm(y),
                    mm(r),
                    stroke(6),
                    fill(w.get(7))
                ),
            ))
        }
        "A" => {
            // A x y r start end unit style width fill sx sy ex ey (angles in tenths of a degree)
            let (cx, cy, r) = (
                num(w, 1).ok_or_else(bad)?,
                num(w, 2).ok_or_else(bad)?,
                num(w, 3).ok_or_else(bad)?,
            );
            let (unit, style) = (int(w, 6).ok_or_else(bad)?, int(w, 7).ok_or_else(bad)?);
            // The end points as written, else from the angles.
            let at = |deg: f64| (cx + r * deg.to_radians().cos(), cy + r * deg.to_radians().sin());
            let start_angle = num(w, 4).map(|a| a / 10.0);
            let end_angle = num(w, 5).map(|a| a / 10.0);
            let start = match (num(w, 10), num(w, 11)) {
                (Some(x), Some(y)) => (x, y),
                _ => at(start_angle.ok_or_else(bad)?),
            };
            let end = match (num(w, 12), num(w, 13)) {
                (Some(x), Some(y)) => (x, y),
                _ => at(end_angle.ok_or_else(bad)?),
            };
            // KiCad draws the shorter way round from start to end, never more than half a turn.
            let angle = |(x, y): (f64, f64)| (y - cy).atan2(x - cx).to_degrees();
            let (a, b) = (angle(start), angle(end));
            // Half a turn exactly runs anticlockwise, as KiCad draws it.
            let mut sweep = (b - a).rem_euclid(360.0);
            if sweep > 180.0 + 1e-9 {
                sweep -= 360.0;
            }
            let mid = at(a + sweep / 2.0);
            if r <= 0.0 || sweep.abs() < 1e-6 {
                return Err(bad());
            }
            Ok((
                unit,
                style,
                format!(
                    "(arc (start {} {}) (mid {} {}) (end {} {}) {} {})",
                    mm(start.0),
                    mm(start.1),
                    mm(mid.0),
                    mm(mid.1),
                    mm(end.0),
                    mm(end.1),
                    stroke(8),
                    fill(w.get(9))
                ),
            ))
        }
        "T" => {
            // T angle x y size hidden unit style text [italic bold H V]
            let angle = num(w, 1).ok_or_else(bad)? / 10.0;
            let (x, y, size) = (
                num(w, 2).ok_or_else(bad)?,
                num(w, 3).ok_or_else(bad)?,
                num(w, 4).ok_or_else(bad)?,
            );
            let hidden = int(w, 5).unwrap_or(0) != 0;
            let (unit, style) = (int(w, 6).ok_or_else(bad)?, int(w, 7).ok_or_else(bad)?);
            let t = text(w.get(8).ok_or_else(bad)?, false);
            let justify = justify(w.get(11).map(String::as_str), w.get(12).map(String::as_str));
            Ok((
                unit,
                style,
                format!(
                    "(text {} (at {} {} {}) (effects (font (size {s} {s})){justify}{}))",
                    q(&t),
                    mm(x),
                    mm(y),
                    angle,
                    if hidden { " hide" } else { "" },
                    s = mm(size),
                ),
            ))
        }
        "X" => {
            // X name number x y length orientation number_size name_size unit style type [shape]
            let name = text(w.get(1).ok_or_else(bad)?, true);
            let number = text(w.get(2).ok_or_else(bad)?, true);
            let (x, y, length) = (
                num(w, 3).ok_or_else(bad)?,
                num(w, 4).ok_or_else(bad)?,
                num(w, 5).ok_or_else(bad)?,
            );
            let angle = match w.get(6).map(String::as_str) {
                Some("U") => 90,
                Some("L") => 180,
                Some("D") => 270,
                _ => 0,
            };
            let (number_size, name_size) = (num(w, 7).unwrap_or(50.0), num(w, 8).unwrap_or(50.0));
            let (unit, style) = (int(w, 9).ok_or_else(bad)?, int(w, 10).ok_or_else(bad)?);
            let kind = match w.get(11).map(String::as_str) {
                Some("I") => "input",
                Some("O") => "output",
                Some("B") => "bidirectional",
                Some("T") => "tri_state",
                Some("U") => "unspecified",
                Some("W") => "power_in",
                Some("w") => "power_out",
                Some("C") => "open_collector",
                Some("E") => "open_emitter",
                Some("N") => "no_connect",
                _ => "passive",
            };
            let shape_word = w.get(12).map(String::as_str).unwrap_or("");
            let hidden = shape_word.starts_with('N');
            let marks = shape_word.trim_start_matches('N');
            let shape = match (marks.contains('I'), marks.contains('C')) {
                (true, true) => "inverted_clock",
                (true, false) => "inverted",
                (false, true) => "clock",
                _ => "line",
            };
            Ok((
                unit,
                style,
                format!(
                    "(pin {kind} {shape} (at {} {} {angle}) (length {}){} (name {} (effects (font (size {ns} {ns})))) (number {} (effects (font (size {nn} {nn})))))",
                    mm(x),
                    mm(y),
                    mm(length),
                    if hidden { " hide" } else { "" },
                    q(&name),
                    q(&number),
                    ns = mm(name_size),
                    nn = mm(number_size),
                ),
            ))
        }
        _ => Err(bad()),
    }
}

/// The `.kicad_sym` text of an old library, and warnings for lines that were not read.
pub fn to_kicad_sym(text_in: &str) -> Result<(String, Vec<String>)> {
    if !is_legacy(text_in) {
        return Err(Error::Format(
            "not an old KiCad symbol library (EESchema-LIBRARY)".into(),
        ));
    }
    let mut out = String::from("(kicad_symbol_lib (version 20211014) (generator wiringcraft_legacy)\n");
    let mut warnings = Vec::new();
    let mut def: Option<Def> = None;
    let mut drawing = false;
    let mut in_fplist = false;
    let mut count = 0usize;
    let mut skipped: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for (i, raw) in text_in.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let w = words(line);
        let Some(head) = w.first().map(String::as_str) else {
            continue;
        };
        if in_fplist {
            in_fplist = head != "$ENDFPLIST";
            continue;
        }
        match (head, def.as_mut()) {
            ("DEF", None) => {
                let name = text(w.get(1).map(String::as_str).unwrap_or(""), true);
                // A name starting with `~` hides the value field.
                let name = name.strip_prefix('~').unwrap_or(&name).to_owned();
                if name.is_empty() {
                    warnings.push(format!("line {n}: a symbol without a name, skipped"));
                    def = Some(Def {
                        line: n,
                        ..Def::default()
                    });
                    continue;
                }
                def = Some(Def {
                    reference: text(w.get(2).map(String::as_str).unwrap_or(""), true),
                    units: int(&w, 7).unwrap_or(1).clamp(1, 1000),
                    name,
                    line: n,
                    ..Def::default()
                });
                drawing = false;
            }
            ("DEF", Some(open)) => {
                return Err(Error::Format(format!(
                    "line {n}: a symbol starts before the one from line {} ends (ENDDEF)",
                    open.line
                )));
            }
            ("ENDDEF", Some(_)) => {
                let Some(done) = def.take() else { continue };
                if done.name.is_empty() {
                    continue;
                }
                count += 1;
                if count > MAX_SYMBOLS {
                    return Err(Error::Format(format!("more than {MAX_SYMBOLS} symbols in one library")));
                }
                if !skipped.is_empty() {
                    for (kind, k) in std::mem::take(&mut skipped) {
                        warnings.push(format!("{}: {k} {kind}(s) not read", done.name));
                    }
                }
                write_symbol(&mut out, &done);
            }
            (_, None) => {}
            (f, Some(d))
                if !drawing && f.len() > 1 && f.starts_with('F') && f[1..].chars().all(|c| c.is_ascii_digit()) =>
            {
                let k: usize = f[1..].parse().unwrap_or(usize::MAX);
                if d.name.is_empty() {
                    continue;
                }
                match field(k, &w, d) {
                    Some(p) => d.properties.push(p),
                    None => *skipped.entry(format!("{f} field")).or_default() += 1,
                }
            }
            ("ALIAS", Some(d)) => d.aliases.extend(w.iter().skip(1).map(|a| text(a, true))),
            ("$FPLIST", Some(_)) => in_fplist = true,
            ("DRAW", Some(_)) => drawing = true,
            ("ENDDRAW", Some(_)) => drawing = false,
            (_, Some(d)) if drawing => {
                if d.name.is_empty() {
                    continue;
                }
                match draw_item(&w) {
                    Ok((unit, style, item)) => d.items.entry((unit.max(0), style.max(0))).or_default().push(item),
                    Err(kind) => *skipped.entry(kind).or_default() += 1,
                }
            }
            _ => {}
        }
    }
    if let Some(open) = def {
        return Err(Error::Format(format!(
            "the symbol from line {} has no ENDDEF; the file is cut short",
            open.line
        )));
    }
    out.push_str(")\n");
    Ok((out, warnings))
}

/// Write one symbol, and one derived symbol per alias, as `.kicad_sym` items.
fn write_symbol(out: &mut String, d: &Def) {
    let _ = writeln!(out, "  (symbol {}", q(&d.name));
    let has_reference = d.properties.iter().any(|p| p.starts_with("(property \"Reference\""));
    if !has_reference {
        let _ = writeln!(out, "    (property \"Reference\" {} (at 0 0 0))", q(&d.reference));
    }
    for p in &d.properties {
        let _ = writeln!(out, "    {p}");
    }
    for ((unit, style), items) in &d.items {
        // Style 0 draws in both styles: KiCad 6 calls the normal style 1.
        let style = (*style).max(1);
        let unit = if d.units > 1 { *unit } else { (*unit).min(1) };
        let _ = writeln!(out, "    (symbol {}", q(&format!("{}_{unit}_{style}", d.name)));
        for item in items {
            let _ = writeln!(out, "      {item}");
        }
        out.push_str("    )\n");
    }
    out.push_str("  )\n");
    for alias in d.aliases.iter().filter(|a| !a.is_empty() && **a != d.name) {
        let _ = writeln!(
            out,
            "  (symbol {} (extends {}) (property \"Reference\" {} (at 0 0 0)) (property \"Value\" {} (at 0 0 0) (effects hide)))",
            q(alias),
            q(&d.name),
            q(&d.reference),
            q(alias)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELAY: &str = r#"EESchema-LIBRARY Version 2.4
#encoding utf-8
#
# RELAY
#
DEF RELAY K 0 40 Y Y 2 L N
F0 "K" 0 250 50 H V C CNN
F1 "RELAY" 0 -250 50 H V C CNN
F2 "" 0 0 50 H I C CNN
F3 "" 0 0 50 H I C CNN
$FPLIST
 Relay_*
$ENDFPLIST
ALIAS RELAY_24V
DRAW
S -100 100 100 -100 1 1 10 N
T 0 0 0 50 0 1 1 Coil~A Normal 0 C C
X A1 A1 0 200 100 D 50 50 1 1 P
X A2 A2 0 -200 100 U 50 50 1 1 P
A 0 0 50 0 1800 2 1 0 N 50 0 -50 0
X 11 11 -100 -200 100 U 50 50 2 1 P I
X 14 14 100 -200 100 U 50 50 2 0 P N
P 2 2 2 0 0 0 100 100 N
C 0 0 20 2 1 0 F
Z 1 2 3
ENDDRAW
ENDDEF
#
#End Library
"#;

    #[test]
    fn an_old_library_reads_as_the_new_format() -> Result<()> {
        assert!(is_legacy(RELAY) && !is_legacy("(kicad_symbol_lib)"));
        let (sym, warnings) = to_kicad_sym(RELAY)?;
        assert_eq!(warnings, ["RELAY: 1 Z line(s) not read"]);
        let (symbols, warnings) = crate::kicad::symbols(&sym, "kicad.relays", 1.0)?;
        // Unit 2's style-2 polyline is the De Morgan style.
        assert!(
            warnings.iter().any(|w| w.contains("alternate body styles")),
            "{warnings:?}"
        );
        let ids: std::collections::BTreeSet<&str> = symbols.iter().map(|i| i.symbol.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "kicad.relays.relay_24v_a",
                "kicad.relays.relay_24v_b",
                "kicad.relays.relay_a",
                "kicad.relays.relay_b"
            ]
            .into()
        );
        let coil = symbols
            .iter()
            .find(|i| i.symbol.id == "kicad.relays.relay_a")
            .ok_or(Error::Format("no coil".into()))?;
        let pins: Vec<(&str, f64, f64)> = coil
            .symbol
            .pins
            .iter()
            .map(|p| (p.name.as_str(), p.at.x, p.at.y))
            .collect();
        // 200 mil up, 400 mil apart: A1 is the origin, A2 10.16 mm below (y runs down).
        assert_eq!(pins, [("A1", 0.0, 0.0), ("A2", 0.0, 10.16)]);
        assert_eq!(coil.symbol.letter_code, "K");
        assert!(
            coil.symbol
                .graphics
                .iter()
                .any(|g| matches!(g, wc_core::Shape::Text { text, .. } if text == "Coil A")),
            "{:?}",
            coil.symbol.graphics
        );
        let contact = symbols
            .iter()
            .find(|i| i.symbol.id == "kicad.relays.relay_b")
            .ok_or(Error::Format("no contact".into()))?;
        // Pin 14 is hidden (`N`), pin 11 drawn with its inverted mark; the arc and circle of
        // style 1 are read.
        let names: Vec<&str> = contact.symbol.pins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["11"]);
        assert!(
            contact
                .symbol
                .graphics
                .iter()
                .any(|g| matches!(g, wc_core::Shape::Arc { .. }))
        );
        assert!(
            contact
                .symbol
                .graphics
                .iter()
                .any(|g| matches!(g, wc_core::Shape::Circle { filled: true, .. }))
        );
        assert!(
            symbols.iter().any(|i| i.symbol.id == "kicad.relays.relay_24v_a"),
            "the alias"
        );
        Ok(())
    }

    #[test]
    fn a_cut_short_or_doubled_symbol_is_refused() {
        let cut = RELAY.replace("ENDDEF", "");
        assert!(to_kicad_sym(&cut).is_err_and(|e| e.to_string().contains("no ENDDEF")));
        let twice = RELAY.replace("ALIAS RELAY_24V", "DEF X X 0 40 Y Y 1 L N");
        assert!(twice.contains("DEF X"));
        assert!(to_kicad_sym(&twice).is_err_and(|e| e.to_string().contains("before the one from line 6")));
        assert!(to_kicad_sym("(kicad_symbol_lib)").is_err());
    }

    #[test]
    fn quoted_words_keep_spaces_and_odd_names_are_escaped() -> Result<()> {
        assert_eq!(words(r#"F0 "a \"b\" c" 0 0"#), ["F0", "a \"b\" c", "0", "0"]);
        let lib = "EESchema-LIBRARY Version 2.4\nDEF Say\"Hi\\ U 0 40 Y Y 1 F N\nF0 \"U\" 0 0 50 H V C CNN\nDRAW\nX ~ 1 0 0 100 R 50 50 0 0 P\nENDDRAW\nENDDEF\n";
        let (sym, warnings) = to_kicad_sym(lib)?;
        assert!(warnings.is_empty(), "{warnings:?}");
        let (symbols, _) = crate::kicad::symbols(&sym, "kicad.x", 1.0)?;
        assert_eq!(symbols.first().map(|i| i.kicad_name.as_str()), Some("Say\"Hi\\"));
        Ok(())
    }
}

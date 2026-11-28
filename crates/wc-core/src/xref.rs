//! How the cross-reference listing under a coil is laid out: one line per auxiliary contact of
//! its tag, below the symbol's graphics. One definition for the PDF and canvas that draw it
//! (`wc_pdf::xref_marks`) and the fit check of `macro generate` that measures it
//! ([`crate::generate::extent`]).

use crate::geom::Point;

/// The text height of a cross-reference, in millimetres.
pub const HEIGHT: f64 = 1.8;
/// How far right of the symbol's position the contact lines start.
pub const INDENT: f64 = 5.0;
/// How far below the symbol's lowest graphics the first line's baseline sits.
pub const FIRST_LINE: f64 = 3.5;
/// The distance between two contact lines.
pub const LINE_PITCH: f64 = 2.6;

/// Where contact line `i` (from 0) of a symbol at `at` is drawn, left-aligned, when its
/// graphics reach down to `below`.
pub fn contact_line_at(at: Point, below: f64, i: usize) -> Point {
    Point::new(at.x + INDENT, below + FIRST_LINE + LINE_PITCH * i as f64)
}

/// A contact's line: its named pins joined by `-`, then its location (`13-14 3/2.B`); a contact
/// without named pins shows its location alone.
pub fn contact_text(pins: &[String], location: &str) -> String {
    let pins: Vec<&str> = pins.iter().map(|p| p.trim()).filter(|p| !p.is_empty()).collect();
    if pins.is_empty() {
        location.to_owned()
    } else {
        format!("{} {location}", pins.join("-"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_contact_line_lists_its_named_pins_and_steps_down() {
        let pins = vec!["13".to_owned(), " ".to_owned(), "14".to_owned()];
        assert_eq!(contact_text(&pins, "3/2.B"), "13-14 3/2.B");
        assert_eq!(contact_text(&[], "3/2.B"), "3/2.B");
        let at = contact_line_at(Point::new(10.0, 20.0), 30.0, 2);
        assert!((at.x - 15.0).abs() < 1e-9);
        assert!((at.y - (30.0 + 3.5 + 5.2)).abs() < 1e-9);
    }
}

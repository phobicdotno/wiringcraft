//! Text metrics of the font the PDF and the editor print with (Helvetica), so everything that
//! fits text into a box measures it the same way.

/// Helvetica advance widths (per 1000 em) for the printable ASCII characters, from Adobe's
/// font metrics (WinAnsi: `'` is the straight quote).
const HELVETICA_ASCII: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556,
    556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833,
    722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556,
    556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334,
    260, 334, 584,
];

/// Helvetica advance widths (per 1000 em) for the Latin-1 characters U+00A0 to U+00FF, which
/// `pdf_text` writes as the same WinAnsi codes.
const HELVETICA_LATIN1: [u16; 96] = [
    278, 333, 556, 556, 556, 556, 260, 556, 333, 737, 370, 556, 584, 333, 737, 333, 400, 584, 333, 333, 333, 556, 537,
    278, 333, 333, 365, 556, 834, 834, 834, 611, 667, 667, 667, 667, 667, 667, 1000, 722, 667, 667, 667, 667, 278, 278,
    278, 278, 722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667, 611, 556, 556, 556, 556, 556,
    556, 889, 500, 556, 556, 556, 556, 278, 278, 278, 278, 556, 556, 556, 556, 556, 556, 556, 584, 611, 556, 556, 556,
    556, 500, 556, 500,
];

/// The width of `s` drawn in Helvetica at `height_mm` (cap height), from the font's advance
/// widths, for the glyphs `pdf_text` writes (ASCII and Latin-1 at their AFM widths); characters
/// `pdf_text` cannot encode are measured as the `?` it writes.
pub fn text_width_mm(s: &str, height_mm: f64) -> f64 {
    let em = height_mm / 0.72;
    let units: f64 = s
        .chars()
        .map(|c| {
            let code = c as u32;
            match c {
                ' '..='~' => code
                    .checked_sub(32)
                    .and_then(|i| HELVETICA_ASCII.get(i as usize))
                    .map_or(556.0, |w| f64::from(*w)),
                '\u{a0}'..='\u{ff}' => code
                    .checked_sub(0xa0)
                    .and_then(|i| HELVETICA_LATIN1.get(i as usize))
                    .map_or(556.0, |w| f64::from(*w)),
                // What `pdf_text` writes for these: WinAnsi dashes and bullet, `>` for an arrow.
                '\u{2013}' => 556.0,
                '\u{2014}' => 1000.0,
                '\u{2022}' => 350.0,
                '\u{2192}' => 584.0,
                // Anything else is written as `?`.
                _ => 556.0,
            }
        })
        .sum();
    units / 1000.0 * em
}

/// Whether the PDF writer prints `c` as itself: printable ASCII and Latin-1 (WinAnsi), plus the
/// dashes and bullet. Anything else comes out as `?` or a stand-in.
pub fn pdf_prints(c: char) -> bool {
    matches!(c, ' '..='~' | '\u{a0}'..='\u{ff}' | '\u{2013}' | '\u{2014}' | '\u{2022}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_letters_are_measured_at_their_own_widths() {
        // Helvetica's i-grave is as narrow as an i, not a wide default.
        assert!((text_width_mm("ì", 0.72) - 0.278).abs() < 1e-9);
        assert!((text_width_mm("Æ", 0.72) - 1.0).abs() < 1e-9);
        assert!((text_width_mm("ø", 0.72) - 0.611).abs() < 1e-9);
        assert!((text_width_mm("\u{bd}", 0.72) - 0.834).abs() < 1e-9);
    }

    #[test]
    fn the_pdf_prints_ascii_and_latin1_only() {
        assert!("A01P1æøåÆØÅ".chars().all(pdf_prints));
        assert!(!pdf_prints('Д'));
        assert!(!pdf_prints('\u{2192}'));
    }
}

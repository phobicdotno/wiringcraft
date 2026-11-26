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

/// The characters WinAnsi puts at codes 0x80 to 0x9F (euro, curly quotes, ellipsis, dashes,
/// Œ Š Ž and so on), with their code and Helvetica advance width (per 1000 em). 0x81, 0x8D, 0x8F, 0x90
/// and 0x9D are unused.
const WINANSI_HIGH: [(char, u8, u16); 27] = [
    ('\u{20ac}', 0x80, 556),  // euro
    ('\u{201a}', 0x82, 222),  // single low quote
    ('\u{0192}', 0x83, 556),  // florin
    ('\u{201e}', 0x84, 333),  // double low quote
    ('\u{2026}', 0x85, 1000), // ellipsis
    ('\u{2020}', 0x86, 556),  // dagger
    ('\u{2021}', 0x87, 556),  // double dagger
    ('\u{02c6}', 0x88, 333),  // circumflex
    ('\u{2030}', 0x89, 1000), // per mille
    ('\u{0160}', 0x8a, 667),  // S caron
    ('\u{2039}', 0x8b, 333),  // single left angle quote
    ('\u{0152}', 0x8c, 1000), // OE
    ('\u{017d}', 0x8e, 611),  // Z caron
    ('\u{2018}', 0x91, 222),  // left single quote
    ('\u{2019}', 0x92, 222),  // right single quote
    ('\u{201c}', 0x93, 333),  // left double quote
    ('\u{201d}', 0x94, 333),  // right double quote
    ('\u{2022}', 0x95, 350),  // bullet
    ('\u{2013}', 0x96, 556),  // en dash
    ('\u{2014}', 0x97, 1000), // em dash
    ('\u{02dc}', 0x98, 333),  // small tilde
    ('\u{2122}', 0x99, 1000), // trade mark
    ('\u{0161}', 0x9a, 500),  // s caron
    ('\u{203a}', 0x9b, 333),  // single right angle quote
    ('\u{0153}', 0x9c, 944),  // oe
    ('\u{017e}', 0x9e, 500),  // z caron
    ('\u{0178}', 0x9f, 667),  // Y diaeresis
];

/// The WinAnsi code the built-in PDF font prints `c` with: printable ASCII and Latin-1 map to
/// themselves, and the euro, curly quotes, ellipsis, dashes and the other 0x80 to 0x9F glyphs to
/// their codes there. `None` when the font has no glyph for `c`.
pub fn winansi_code(c: char) -> Option<u8> {
    match c {
        ' '..='~' | '\u{a0}'..='\u{ff}' => u8::try_from(u32::from(c)).ok(),
        _ => WINANSI_HIGH.iter().find(|(h, _, _)| *h == c).map(|(_, code, _)| *code),
    }
}

/// The width of `s` drawn in Helvetica at `height_mm` (cap height), from the font's advance
/// widths, for the glyphs `pdf_text` writes (ASCII, Latin-1 and the rest of WinAnsi at their AFM
/// widths); characters `pdf_text` cannot encode are measured as the `?` it writes.
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
                // `pdf_text` writes `>` for an arrow.
                '\u{2192}' => 584.0,
                // The rest of WinAnsi at its own widths; anything else is written as `?`.
                _ => WINANSI_HIGH
                    .iter()
                    .find(|(h, _, _)| *h == c)
                    .map_or(556.0, |(_, _, w)| f64::from(*w)),
            }
        })
        .sum();
    units / 1000.0 * em
}

/// Whether the PDF writer prints `c` as itself: printable ASCII, Latin-1 and the rest of WinAnsi
/// (euro, curly quotes, ellipsis, dashes, bullet, Œ Š Ž ...). Anything else comes out as `?` or
/// a stand-in.
pub fn pdf_prints(c: char) -> bool {
    winansi_code(c).is_some()
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

    #[test]
    fn the_rest_of_winansi_prints_at_its_own_widths() {
        assert!("€‚ƒ„…†‡ˆ‰Š‹ŒŽ‘’“”•–—˜™š›œžŸ".chars().all(pdf_prints));
        assert_eq!(winansi_code('€'), Some(0x80));
        assert_eq!(winansi_code('Ž'), Some(0x8e));
        assert_eq!(winansi_code('‘'), Some(0x91));
        assert_eq!(winansi_code('Ÿ'), Some(0x9f));
        assert_eq!(winansi_code('Д'), None);
        assert!((text_width_mm("€", 0.72) - 0.556).abs() < 1e-9);
        assert!((text_width_mm("…", 0.72) - 1.0).abs() < 1e-9);
        assert!((text_width_mm("’", 0.72) - 0.222).abs() < 1e-9);
        assert!((text_width_mm("œ", 0.72) - 0.944).abs() < 1e-9);
    }
}

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

/// The width of `s` drawn in Helvetica at `height_mm` (cap height), from the font's advance
/// widths, for the glyphs `pdf_text` writes. Latin-1 letters use their AFM widths where listed,
/// else a wide default; characters `pdf_text` cannot encode are measured as the `?` it writes.
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
                'æ' => 889.0,
                'Æ' => 1000.0,
                'ø' => 611.0,
                'ö' | 'ó' | 'ò' | 'ô' | 'õ' | 'é' | 'è' | 'ê' | 'ë' | 'å' | 'ä' | 'á' | 'à' | 'â' | 'ã' | 'ü' | 'ú'
                | 'ù' | 'û' | 'ñ' => 556.0,
                'Ø' | 'Ö' | 'Ó' | 'Ò' | 'Ô' | 'Õ' => 778.0,
                'Å' | 'Ä' | 'Á' | 'À' | 'Â' | 'Ã' | 'É' | 'È' | 'Ê' | 'Ë' => 667.0,
                'Ü' | 'Ú' | 'Ù' | 'Û' | 'Ñ' => 722.0,
                'ß' => 611.0,
                '°' => 400.0,
                // What `pdf_text` writes for these: WinAnsi dashes and bullet, `>` for an arrow.
                '\u{2013}' => 556.0,
                '\u{2014}' => 1000.0,
                '\u{2022}' => 350.0,
                '\u{2192}' => 584.0,
                // Helvetica's widths for the WinAnsi fractions ¼ ½ ¾.
                '\u{bc}' | '\u{bd}' | '\u{be}' => 834.0,
                // Other Latin-1 letters: a wide estimate, so fitted text never overflows.
                '\u{a0}'..='\u{ff}' => 778.0,
                // Anything else is written as `?`.
                _ => 556.0,
            }
        })
        .sum();
    units / 1000.0 * em
}

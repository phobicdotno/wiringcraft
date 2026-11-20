//! Decoders for the picture filters scanners write into PDFs: CCITT fax (Group 3 and 4), JBIG2
//! and JPEG 2000, through the pure-Rust `hayro` decoders (MIT OR Apache-2.0).
//!
//! The bi-level ones return rows of 1-bit samples padded to whole bytes, as a PDF filter hands
//! them to the picture: a 0 bit is black unless `BlackIs1` says otherwise.

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::raster::MAX_PIXELS;

/// Runs a third-party decoder on file data so that a bug it hits on a damaged picture refuses
/// that picture instead of closing the app.
fn guarded<T>(what: &str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(format!("the {what} picture is damaged")))
}

/// Whether a picture of `width` by `height` pixels is small enough to decode.
fn fits(width: u32, height: u32) -> bool {
    width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_PIXELS
}

/// Packs rows of black-or-white pixels, MSB first, each row padded to a whole byte. A black
/// pixel becomes `black_bit`.
struct Packer {
    width: u32,
    black_bit: bool,
    out: Vec<u8>,
    /// Pixels written in the current row.
    x: u32,
    rows: u32,
}

impl Packer {
    fn new(width: u32, black_bit: bool) -> Self {
        Packer {
            width,
            black_bit,
            out: Vec::new(),
            x: 0,
            rows: 0,
        }
    }

    fn push(&mut self, black: bool) {
        if self.x >= self.width {
            return;
        }
        if self.x.is_multiple_of(8) {
            self.out.push(0);
        }
        if black == self.black_bit
            && let Some(last) = self.out.last_mut()
        {
            *last |= 0x80 >> (self.x % 8);
        }
        self.x += 1;
    }

    fn end_row(&mut self) {
        while self.x < self.width {
            self.push(false);
        }
        self.x = 0;
        self.rows += 1;
    }
}

/// The parameters of a `CCITTFaxDecode` filter (PDF 32000-1, table 11).
pub(crate) struct FaxParms {
    /// `K`: below 0 Group 4, 0 Group 3 one-dimensional, above 0 Group 3 two-dimensional.
    pub k: i64,
    pub columns: u32,
    /// `Rows`, or the picture's height when the filter does not say.
    pub rows: u32,
    pub end_of_line: bool,
    pub encoded_byte_align: bool,
    pub end_of_block: bool,
    pub black_is_1: bool,
}

/// Decodes CCITT fax data into `parms.rows` rows of `parms.columns` 1-bit samples. A picture
/// damaged part way is kept with the rows before the damage (the rest white), and the second
/// value says so.
pub(crate) fn ccitt(data: &[u8], parms: &FaxParms) -> Result<(Vec<u8>, Option<String>), String> {
    if !fits(parms.columns, parms.rows) {
        return Err("the fax picture has no size or is too large".into());
    }
    let encoding = match parms.k {
        k if k < 0 => hayro_ccitt::EncodingMode::Group4,
        0 => hayro_ccitt::EncodingMode::Group3_1D,
        _ => hayro_ccitt::EncodingMode::Group3_2D,
    };
    let settings = hayro_ccitt::DecodeSettings {
        columns: parms.columns,
        rows: parms.rows,
        end_of_block: parms.end_of_block,
        end_of_line: parms.end_of_line,
        rows_are_byte_aligned: parms.encoded_byte_align,
        encoding,
        strict: false,
    };
    struct Rows(Packer);
    impl hayro_ccitt::Decoder for Rows {
        fn push_row(&mut self, changes: &[u32]) {
            let mut black = false;
            let mut next = changes.iter().copied().peekable();
            for x in 0..self.0.width {
                while next.peek().is_some_and(|c| *c <= x) {
                    next.next();
                    black = !black;
                }
                self.0.push(black);
            }
            self.0.end_row();
        }
    }
    // Black is a 0 sample unless the picture says BlackIs1.
    let mut rows = Rows(Packer::new(parms.columns, parms.black_is_1));
    let damage = guarded("fax", || {
        let mut ctx = hayro_ccitt::DecoderContext::new(settings);
        let result = hayro_ccitt::decode(data, &mut rows, &mut ctx);
        // A damaged tail still leaves the rows before it; only a picture with no row at all is
        // refused. Missing rows stay white, and the reason goes with the picture.
        let got = rows.0.rows;
        match result {
            Err(e) if got == 0 => Err(format!("the fax picture could not be decoded ({e})")),
            Ok(_) if got == 0 => Err("the fax picture holds no rows".into()),
            Err(e) => Ok(Some(format!(
                "the fax picture is damaged after row {got} of {} ({e}); the rest is left white",
                parms.rows
            ))),
            Ok(_) if got < parms.rows => Ok(Some(format!(
                "the fax picture ends after row {got} of {}; the rest is left white",
                parms.rows
            ))),
            Ok(_) => Ok(None),
        }
    })?;
    let mut packer = rows.0;
    while packer.rows < parms.rows {
        packer.end_row();
    }
    let stride = parms.columns.div_ceil(8) as usize;
    packer.out.truncate(stride * parms.rows as usize);
    Ok((packer.out, damage))
}

/// Decodes a JBIG2 picture in the embedded organization PDF uses, with the segments of its
/// `JBIG2Globals` stream if it has one. Returns its width, height and 1-bit samples (0 black).
pub(crate) fn jbig2(data: &[u8], globals: Option<&[u8]>) -> Result<(u32, u32, Vec<u8>), String> {
    jbig2_within(data, globals, JBIG2_BUDGET)
}

/// [`jbig2`] with `limit` bytes for the decoder's bitmaps.
fn jbig2_within(data: &[u8], globals: Option<&[u8]>, limit: usize) -> Result<(u32, u32, Vec<u8>), String> {
    guarded("JBIG2", || {
        // The decoder counts every bitmap it keeps (regions, pattern and symbol dictionaries
        // and the page) against this limit and stops once they go over it, so a few bytes
        // that declare huge or many symbols cannot use up the memory.
        let settings = hayro_jbig2::DecodeSettings {
            memory_limit: Some(limit),
            ..Default::default()
        };
        let too_large = |e: &hayro_jbig2::DecodeError, what: &str| {
            if *e == hayro_jbig2::DecodeError::MemoryLimitExceeded {
                "the JBIG2 picture needs too much memory to decode".to_string()
            } else {
                format!("{what} ({e})")
            }
        };
        let globals = globals
            .map(|g| hayro_jbig2::Globals::new(g, &settings))
            .transpose()
            .map_err(|e| too_large(&e, "the shared part of the JBIG2 picture could not be read"))?;
        let page = hayro_jbig2::Page::new_embedded(data, globals.as_ref(), &settings)
            .map_err(|e| too_large(&e, "the JBIG2 picture could not be read"))?;
        let (width, height) = (page.width(), page.height());
        if !fits(width, height) {
            return Err("the JBIG2 picture has no size or is too large".into());
        }
        let bitmap = page
            .decode()
            .map_err(|e| too_large(&e, "the JBIG2 picture could not be decoded"))?;
        let mut packer = Packer::new(width, false);
        for row in bitmap.rows().take(height as usize) {
            for x in 0..width {
                let word = row.get((x / 32) as usize).copied().unwrap_or(0);
                packer.push(word & (0x8000_0000 >> (x % 32)) != 0);
            }
            packer.end_row();
        }
        while packer.rows < height {
            packer.end_row();
        }
        Ok((width, height, packer.out))
    })
}

/// The most memory the JBIG2 decoder may keep for one picture's bitmaps, in bytes.
const JBIG2_BUDGET: usize = 256 << 20;

/// A decoded JPEG 2000 picture: 8-bit samples, one to four channels interleaved.
pub(crate) struct Jpx {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub samples: Vec<u8>,
}

/// The boxes of JP2 data, as (type, contents): a length of 0 runs to the end, 1 gives a 64-bit
/// length. `None` when a box does not fit in the data, which the decoder would read anyway in
/// its own way: such data is refused rather than guessed at.
fn jp2_boxes(data: &[u8]) -> Option<Vec<(&[u8], &[u8])>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < data.len() {
        let len = u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?);
        let kind = data.get(at + 4..at + 8)?;
        let (header, size) = match len {
            0 => (8, data.len() - at),
            1 => (
                16,
                usize::try_from(u64::from_be_bytes(data.get(at + 8..at + 16)?.try_into().ok()?)).ok()?,
            ),
            n => (8, n as usize),
        };
        if size < header {
            return None;
        }
        out.push((kind, data.get(at + header..at.checked_add(size)?)?));
        at += size;
    }
    Some(out)
}

/// How many full-size component buffers the decoder makes for a JPEG 2000 picture: the
/// component count of the SIZ marker (ISO 15444-1 A.5.1) of its codestream or, if more, the
/// most channels a palette in its header can map out (component-mapping entries, or palette
/// columns, which the decoder maps when there is no mapping). `None` for data whose effective
/// boxes are not plain: a box that does not fit, or a second codestream or header box.
fn jpx_components(data: &[u8]) -> Option<u16> {
    const CODESTREAM: [u8; 4] = [0xFF, 0x4F, 0xFF, 0x51];
    let csiz = |cs: &[u8]| -> Option<u16> {
        if cs.get(..4)? != CODESTREAM {
            return None;
        }
        // SOC, SIZ marker, Lsiz, Rsiz, eight 32-bit sizes, then Csiz.
        let c = 2 + 2 + 2 + 2 + 32;
        Some(u16::from_be_bytes(cs.get(c..c + 2)?.try_into().ok()?))
    };
    if data.starts_with(&CODESTREAM) {
        return csiz(data);
    }
    let boxes = jp2_boxes(data)?;
    let only = |kind: &[u8]| -> Option<Option<&[u8]>> {
        let mut found = boxes.iter().filter(|(k, _)| *k == kind);
        let first = found.next().map(|(_, b)| *b);
        found.next().is_none().then_some(first)
    };
    let stored = csiz(only(b"jp2c")??)?;
    let mapped = match only(b"jp2h")? {
        None => 0,
        Some(header) => jp2_boxes(header)?
            .iter()
            .map(|(kind, body)| match *kind {
                b"cmap" => body.len() / 4,
                b"pclr" => body.get(2).map_or(0, |n| usize::from(*n)),
                _ => 0,
            })
            .max()
            .unwrap_or(0),
    };
    Some(stored.max(u16::try_from(mapped).unwrap_or(u16::MAX)))
}

/// Decodes a JPEG 2000 picture (a JP2 file or a bare codestream). An alpha channel is dropped:
/// a scanned sheet is drawn opaque. `declared` is the channel count of the picture's PDF
/// colour space, if it names one: a bare codestream does not say CMYK, and the decoder would
/// take its fourth channel for alpha.
pub(crate) fn jpx(data: &[u8], declared: Option<u8>) -> Result<Jpx, String> {
    guarded("JPEG 2000", || {
        let image = hayro_jpeg2000::Image::new(data, &hayro_jpeg2000::DecodeSettings::default())
            .map_err(|e| format!("the JPEG 2000 picture could not be read ({e})"))?;
        let (width, height) = (image.width(), image.height());
        if !fits(width, height) {
            return Err("the JPEG 2000 picture has no size or is too large".into());
        }
        // Each component gets a buffer of the full picture before anything is checked: refuse
        // more than a picture can use, or more samples than the pixel limit, first.
        let stored =
            jpx_components(data).ok_or("the JPEG 2000 picture's header is unreadable, cut short or repeated")?;
        let shown = u16::from(image.color_space().num_channels()) + u16::from(image.has_alpha());
        let most = stored.max(shown);
        if !(1..=5).contains(&most) || u64::from(width) * u64::from(height) * u64::from(most) > MAX_PIXELS {
            return Err(format!(
                "the JPEG 2000 picture has too many channels ({most}) for its size"
            ));
        }
        let has_alpha = image.has_alpha();
        let mut ctx = hayro_jpeg2000::DecoderContext::default();
        let decoded = image
            .decode(&mut ctx)
            .map_err(|e| format!("the JPEG 2000 picture could not be decoded ({e})"))?;
        let all = decoded.components().len();
        // Four components in a picture the PDF calls CMYK are C, M, Y and K, whatever the
        // codestream left the decoder to guess.
        let cmyk = declared == Some(4) && all == 4;
        let channels = all - usize::from(has_alpha && all > 1 && !cmyk);
        if !matches!(channels, 1 | 3 | 4) {
            return Err(format!(
                "JPEG 2000 pictures with {channels} colour channel(s) are not supported yet"
            ));
        }
        let interleaved = decoded.data_u8();
        let samples = if channels == all {
            interleaved
        } else {
            interleaved
                .chunks(all)
                .flat_map(|px| px.iter().take(channels).copied())
                .collect()
        };
        Ok(Jpx {
            width,
            height,
            channels: channels as u8,
            samples,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_component_count_comes_from_the_siz_marker() {
        // A bare codestream: SOC, SIZ with one component.
        let mut cs = vec![0xFF, 0x4F, 0xFF, 0x51, 0x00, 0x29];
        cs.extend_from_slice(&[0; 34]);
        cs.extend_from_slice(&[0x00, 0x03, 0x07, 0x01, 0x01]);
        assert_eq!(jpx_components(&cs), Some(3));
        // The same inside a JP2 file: a signature box, then the codestream box.
        let mut jp2 = vec![0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];
        jp2.extend_from_slice(&(8 + cs.len() as u32).to_be_bytes());
        jp2.extend_from_slice(b"jp2c");
        jp2.extend_from_slice(&cs);
        assert_eq!(jpx_components(&jp2), Some(3));
        // A second codestream box (the decoder takes the last) is refused.
        let mut big = cs.clone();
        big[41] = 6;
        let mut two = jp2.clone();
        two.extend_from_slice(&(8 + big.len() as u32).to_be_bytes());
        two.extend_from_slice(b"jp2c");
        two.extend_from_slice(&big);
        assert_eq!(jpx_components(&two), None);
        // So is a box longer than the data (the decoder reads what is there).
        let mut over = jp2.clone();
        over.extend_from_slice(&(8 + big.len() as u32 + 100).to_be_bytes());
        over.extend_from_slice(b"jp2c");
        over.extend_from_slice(&big);
        assert_eq!(jpx_components(&over), None);
        // A palette's component mapping makes a buffer per entry: eight entries count as eight.
        let header = |child: &[u8], body: &[u8]| {
            let mut b = (16 + body.len() as u32).to_be_bytes().to_vec();
            b.extend_from_slice(b"jp2h");
            b.extend_from_slice(&(8 + body.len() as u32).to_be_bytes());
            b.extend_from_slice(child);
            b.extend_from_slice(body);
            b
        };
        let mut mapped = header(b"cmap", &[0, 0, 1, 0].repeat(8));
        mapped.extend_from_slice(&jp2);
        assert_eq!(jpx_components(&mapped), Some(8));
        // Without a mapping, the palette's columns count.
        let mut palette = header(b"pclr", &[0, 2, 7, 7, 7, 7, 7, 7, 7]);
        palette.extend_from_slice(&jp2);
        assert_eq!(jpx_components(&palette), Some(7));
        // Of several palettes the widest counts, valid or not: the decoder skips a bad one
        // and keeps an earlier one.
        let mut wide = vec![0, 0, 0, 20];
        wide.extend_from_slice(b"pclr");
        wide.extend_from_slice(&[0, 2, 9, 7, 7, 7, 7, 7, 7, 7, 7, 7]);
        wide.extend_from_slice(&[0, 0, 0, 10]);
        wide.extend_from_slice(b"pclr");
        wide.extend_from_slice(&[0, 0]); // invalid: no columns byte
        let mut both = (8 + wide.len() as u32).to_be_bytes().to_vec();
        both.extend_from_slice(b"jp2h");
        both.extend_from_slice(&wide);
        both.extend_from_slice(&jp2);
        assert_eq!(jpx_components(&both), Some(9));
        // Cut short or not JPEG 2000: none.
        assert_eq!(jpx_components(&cs[..30]), None);
        assert_eq!(jpx_components(b"%PDF"), None);
    }

    /// A JBIG2 test stream in the embedded organization PDF uses: the file without its header
    /// (ID string, flags and page count).
    fn embedded(file: &[u8]) -> &[u8] {
        file.get(13..).unwrap_or_default()
    }

    #[test]
    fn jbig2_text_like_scans_with_symbol_dictionaries_are_read() {
        let text = include_bytes!("../testdata/jbig2/symbol.jbig2");
        let generic = include_bytes!("../testdata/jbig2/generic_trailing_7fff_stripped.jbig2");
        let Ok((width, height, samples)) = jbig2(embedded(text), None) else {
            unreachable!("a symbol dictionary page decodes");
        };
        // The same picture coded as one generic region comes out the same.
        assert_eq!(jbig2(embedded(generic), None), Ok((width, height, samples.clone())));
        assert_eq!((width, height), (163, 109));
        assert!(samples.contains(&0), "the picture has black pixels");
    }

    #[test]
    fn jbig2_symbol_dictionaries_over_the_memory_limit_are_refused() {
        let text = include_bytes!("../testdata/jbig2/symbol.jbig2");
        let large = include_bytes!("../testdata/jbig2/memory_symbol_dictionary_large_symbol.jbig2");
        let many = include_bytes!("../testdata/jbig2/memory_symbol_dictionary_many_symbols.jbig2");
        let too_much = |r: Result<(u32, u32, Vec<u8>), String>| r.is_err_and(|e| e.contains("too much memory"));
        // One symbol larger than the limit, and many small symbols that add up past it.
        assert!(too_much(jbig2_within(embedded(large), None, 48 << 10)));
        assert!(too_much(jbig2_within(embedded(many), None, 8 << 10)));
        // An ordinary page needs far less than the limit used for PDFs, but not nothing.
        assert!(too_much(jbig2_within(embedded(text), None, 64)));
        assert!(jbig2_within(embedded(text), None, 1 << 20).is_ok());
    }
}

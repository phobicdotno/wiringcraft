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

/// Decodes CCITT fax data into `parms.rows` rows of `parms.columns` 1-bit samples.
pub(crate) fn ccitt(data: &[u8], parms: &FaxParms) -> Result<Vec<u8>, String> {
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
    guarded("fax", || {
        let mut ctx = hayro_ccitt::DecoderContext::new(settings);
        let result = hayro_ccitt::decode(data, &mut rows, &mut ctx);
        // A damaged tail still leaves the rows before it; only a picture with no row at all is
        // refused. Missing rows stay white.
        if rows.0.rows == 0 {
            return Err(match result {
                Err(e) => format!("the fax picture could not be decoded ({e})"),
                Ok(_) => "the fax picture holds no rows".into(),
            });
        }
        Ok(())
    })?;
    let mut packer = rows.0;
    while packer.rows < parms.rows {
        packer.end_row();
    }
    let stride = parms.columns.div_ceil(8) as usize;
    packer.out.truncate(stride * parms.rows as usize);
    Ok(packer.out)
}

/// Decodes a JBIG2 picture in the embedded organization PDF uses, with the segments of its
/// `JBIG2Globals` stream if it has one. Returns its width, height and 1-bit samples (0 black).
pub(crate) fn jbig2(data: &[u8], globals: Option<&[u8]>) -> Result<(u32, u32, Vec<u8>), String> {
    guarded("JBIG2", || {
        let settings = hayro_jbig2::DecodeSettings::default();
        let globals = globals
            .map(|g| hayro_jbig2::Globals::new(g, &settings))
            .transpose()
            .map_err(|e| format!("the shared part of the JBIG2 picture could not be read ({e})"))?;
        let page = hayro_jbig2::Page::new_embedded(data, globals.as_ref(), &settings)
            .map_err(|e| format!("the JBIG2 picture could not be read ({e})"))?;
        let (width, height) = (page.width(), page.height());
        if !fits(width, height) {
            return Err("the JBIG2 picture has no size or is too large".into());
        }
        struct Pixels(Packer);
        impl hayro_jbig2::Decoder for Pixels {
            fn push_pixel(&mut self, black: bool) {
                self.0.push(black);
            }
            fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
                for _ in 0..chunk_count.saturating_mul(8) {
                    self.0.push(black);
                }
            }
            fn next_line(&mut self) {
                self.0.end_row();
            }
        }
        let mut pixels = Pixels(Packer::new(width, false));
        page.decode(&mut pixels)
            .map_err(|e| format!("the JBIG2 picture could not be decoded ({e})"))?;
        let mut packer = pixels.0;
        while packer.rows < height {
            packer.end_row();
        }
        Ok((width, height, packer.out))
    })
}

/// A decoded JPEG 2000 picture: 8-bit samples, one to four channels interleaved.
pub(crate) struct Jpx {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub samples: Vec<u8>,
}

/// Decodes a JPEG 2000 picture (a JP2 file or a bare codestream). An alpha channel is dropped:
/// a scanned sheet is drawn opaque.
pub(crate) fn jpx(data: &[u8]) -> Result<Jpx, String> {
    guarded("JPEG 2000", || {
        let image = hayro_jpeg2000::Image::new(data, &hayro_jpeg2000::DecodeSettings::default())
            .map_err(|e| format!("the JPEG 2000 picture could not be read ({e})"))?;
        let (width, height) = (image.width(), image.height());
        if !fits(width, height) {
            return Err("the JPEG 2000 picture has no size or is too large".into());
        }
        let has_alpha = image.has_alpha();
        let mut ctx = hayro_jpeg2000::DecoderContext::default();
        let decoded = image
            .decode(&mut ctx)
            .map_err(|e| format!("the JPEG 2000 picture could not be decoded ({e})"))?;
        let all = decoded.components().len();
        let channels = all - usize::from(has_alpha && all > 1);
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

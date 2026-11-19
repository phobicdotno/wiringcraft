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
    // The decoder keeps every intermediate region of a page until the page is done and checks
    // each only against its own size limit, so the regions are added up here before it runs.
    let regions = jbig2_region_area(data)?.saturating_add(globals.map_or(Ok(0), jbig2_region_area)?);
    if regions > MAX_PIXELS {
        return Err("the JBIG2 picture's regions are too large".into());
    }
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

/// The pixels of all region segments (T.88 7.4.1: generic, refinement, text and halftone
/// regions, intermediate or immediate) in JBIG2 data in the embedded organization, read from
/// their segment headers. Data the headers do not describe is left to the decoder.
fn jbig2_region_area(data: &[u8]) -> Result<u64, String> {
    const REGIONS: [u8; 12] = [4, 6, 7, 20, 22, 23, 36, 38, 39, 40, 42, 43];
    let be32 = |at: usize| -> Option<u32> {
        data.get(at..at.checked_add(4)?)
            .and_then(|b| b.try_into().ok())
            .map(u32::from_be_bytes)
    };
    let mut total: u64 = 0;
    let mut at = 0usize;
    while at < data.len() {
        let Some(number) = be32(at) else { break };
        let Some(&flags) = data.get(at + 4) else { break };
        let Some(&refs) = data.get(at + 5) else { break };
        let mut pos = at + 6;
        let count = if refs >> 5 == 7 {
            let Some(long) = be32(at + 5) else { break };
            let count = (long & 0x1FFF_FFFF) as usize;
            pos = at + 9 + count.div_ceil(8).max(1);
            count
        } else {
            usize::from(refs >> 5)
        };
        let ref_size = if number <= 256 {
            1
        } else if number <= 65536 {
            2
        } else {
            4
        };
        pos = pos.saturating_add(count.saturating_mul(ref_size));
        pos = pos.saturating_add(if flags & 0x40 != 0 { 4 } else { 1 });
        let Some(length) = be32(pos) else { break };
        pos = pos.saturating_add(4);
        if REGIONS.contains(&(flags & 0x3F)) {
            let (Some(w), Some(h)) = (be32(pos), be32(pos + 4)) else {
                break;
            };
            total = total.saturating_add(u64::from(w) * u64::from(h));
        }
        if length == u32::MAX {
            // An immediate generic region of unknown length runs to the end of the stripe.
            break;
        }
        at = pos.saturating_add(length as usize);
    }
    Ok(total)
}

/// A decoded JPEG 2000 picture: 8-bit samples, one to four channels interleaved.
pub(crate) struct Jpx {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub samples: Vec<u8>,
}

/// The component count of a JPEG 2000 picture from its SIZ marker (ISO 15444-1 A.5.1): what
/// the decoder allocates a full-size buffer for each of.
fn jpx_components(data: &[u8]) -> Option<u16> {
    const CODESTREAM: [u8; 4] = [0xFF, 0x4F, 0xFF, 0x51];
    let mut at = 0usize;
    if !data.starts_with(&CODESTREAM) {
        // A JP2 file: find the contiguous codestream box.
        loop {
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
            if kind == b"jp2c" {
                at += header;
                break;
            }
            if size < header {
                return None;
            }
            at = at.checked_add(size)?;
        }
    }
    if data.get(at..at + 4)? != CODESTREAM {
        return None;
    }
    // SOC, SIZ marker, Lsiz, Rsiz, eight 32-bit sizes, then Csiz.
    let c = at + 2 + 2 + 2 + 2 + 32;
    Some(u16::from_be_bytes(data.get(c..c + 2)?.try_into().ok()?))
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
        let stored = jpx_components(data).ok_or("the JPEG 2000 picture has no readable header")?;
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
        // Cut short or not JPEG 2000: none.
        assert_eq!(jpx_components(&cs[..30]), None);
        assert_eq!(jpx_components(b"%PDF"), None);
    }

    #[test]
    fn jbig2_regions_are_added_up_from_their_headers() {
        let segment = |number: u32, kind: u8, data: &[u8]| {
            let mut b = number.to_be_bytes().to_vec();
            b.extend_from_slice(&[kind, 0, 1]);
            b.extend_from_slice(&(data.len() as u32).to_be_bytes());
            b.extend_from_slice(data);
            b
        };
        let region = |w: u32, h: u32| {
            let mut d = w.to_be_bytes().to_vec();
            d.extend_from_slice(&h.to_be_bytes());
            d.extend_from_slice(&[0; 10]);
            d
        };
        let mut data = segment(0, 48, &[0; 19]);
        data.extend(segment(1, 36, &region(100, 200)));
        data.extend(segment(2, 38, &region(10, 10)));
        assert_eq!(jbig2_region_area(&data), Ok(20_100));
        // A cut-off header stops the count without an error.
        assert_eq!(jbig2_region_area(&data[..30]), Ok(0));
    }
}

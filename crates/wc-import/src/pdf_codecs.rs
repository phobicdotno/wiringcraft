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

/// [`jbig2`] with `limit` bytes for the decoder's bitmaps and retained contexts. The decoder
/// counts the shared segments (`JBIG2Globals`) and the page's own separately, and the shared ones
/// stay in memory while the page decodes, so each gets half.
fn jbig2_within(data: &[u8], globals: Option<&[u8]>, limit: usize) -> Result<(u32, u32, Vec<u8>), String> {
    let half = limit / 2;
    // What symbol dictionaries keep of their arithmetic decoder is not counted by the decoder.
    let share = |segments: &[u8]| -> Result<usize, String> {
        half.checked_sub(jbig2_dictionary_bytes(segments)?)
            .ok_or_else(|| "the JBIG2 picture needs too much memory to decode".to_string())
    };
    if let Some(n) = jbig2_missing_segment(data, globals)? {
        return Err(format!(
            "the JBIG2 picture refers to its segment {n}, which neither the picture nor its JBIG2Globals holds"
        ));
    }
    let globals_limit = globals.map_or(Ok(half), share)?;
    let page_limit = share(data)?;
    guarded("JBIG2", || {
        // The decoder counts every bitmap it keeps (regions, pattern and symbol dictionaries
        // and the page) against its limit and stops once they go over it, so a few bytes
        // that declare huge or many symbols cannot use up the memory.
        let settings = |limit: usize| hayro_jbig2::DecodeSettings {
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
            .map(|g| hayro_jbig2::Globals::new(g, &settings(globals_limit)))
            .transpose()
            .map_err(|e| too_large(&e, "the shared part of the JBIG2 picture could not be read"))?;
        let page = hayro_jbig2::Page::new_embedded(data, globals.as_ref(), &settings(page_limit))
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

/// What one symbol dictionary that keeps its arithmetic decoder contexts for a later one
/// (T.88 7.4.2.1.1, "bitmap coding context retained") holds: one byte for each of the 65 536
/// generic and 8 192 refinement contexts, in `hayro-jbig2`.
const JBIG2_CONTEXTS: usize = 65_536 + 8_192;

/// What a symbol dictionary segment whose data starts with `head` keeps outside the decoder's
/// memory limit (see [`jbig2_dictionary_bytes`]). Its flags (7.4.2.1.1) say whether it keeps
/// its contexts (bit 9) and how many AT pixel bytes come before the number of exported symbols:
/// 8 or 2 for arithmetic coding with template 0 or another (bits 0 and 10-11), and 4 more for
/// refinement with refinement template 0 (bits 1 and 12). A header cut short counts as nothing:
/// the decoder refuses it.
fn symbol_dictionary_bytes(head: &[u8]) -> usize {
    let Some(flags) = head.get(..2).map(|f| u16::from_be_bytes([f[0], f[1]])) else {
        return 0;
    };
    let contexts = if flags & 0x0200 != 0 { JBIG2_CONTEXTS } else { 0 };
    let huffman = flags & 0x0001 != 0;
    let refinement = flags & 0x0002 != 0;
    let at = match (huffman, (flags >> 10) & 3) {
        (true, _) => 0,
        (false, 0) => 8,
        (false, _) => 2,
    } + if refinement && flags & 0x1000 == 0 { 4 } else { 0 };
    let exported = head
        .get(2 + at..6 + at)
        .map_or(0, |n| u32::from_be_bytes([n[0], n[1], n[2], n[3]]) as usize);
    contexts.saturating_add(exported.saturating_mul(size_of::<usize>()))
}

/// One segment header of JBIG2 data in the embedded organization (7.2): its number, type, the
/// segments it refers to, and its data (cut at the end of the data; empty for a generic region
/// of unknown length).
struct Jbig2Segment<'a> {
    number: u32,
    kind: u8,
    refers_to: Vec<u32>,
    data: &'a [u8],
}

/// The segments of JBIG2 data in the embedded organization, read from their headers. The walk
/// follows the decoder's own segment parser, including generic regions of unknown length; data
/// it cannot follow ends the list, as the decoder cannot follow it either.
fn jbig2_segments(data: &[u8]) -> Result<Vec<Jbig2Segment<'_>>, String> {
    let be32 = |at: usize| -> Option<u32> {
        data.get(at..at.checked_add(4)?)
            .and_then(|b| b.try_into().ok())
            .map(u32::from_be_bytes)
    };
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < data.len() {
        let Some(number) = be32(at) else { break };
        let Some(&flags) = data.get(at + 4) else { break };
        let Some(&refs) = data.get(at + 5) else { break };
        let (count, mut pos) = match refs >> 5 {
            7 => {
                let Some(long) = be32(at + 5) else { break };
                let count = (long & 0x1FFF_FFFF) as usize;
                // The retain bits: one for this segment and one per referred-to segment.
                (count, (at + 9).saturating_add((count + 1).div_ceil(8)))
            }
            5 | 6 => return Err("the JBIG2 picture has a damaged segment header".into()),
            short => (usize::from(short), at + 6),
        };
        let ref_size = if number <= 256 {
            1
        } else if number <= 65536 {
            2
        } else {
            4
        };
        let refs_end = pos.saturating_add(count.saturating_mul(ref_size));
        let Some(ref_bytes) = data.get(pos..refs_end) else {
            break;
        };
        let refers_to = ref_bytes
            .chunks_exact(ref_size)
            .map(|c| c.iter().fold(0u32, |n, b| (n << 8) | u32::from(*b)))
            .collect();
        pos = refs_end.saturating_add(if flags & 0x40 != 0 { 4 } else { 1 });
        let Some(length) = be32(pos) else { break };
        pos = pos.saturating_add(4);
        if length == u32::MAX {
            out.push(Jbig2Segment {
                number,
                kind: flags & 0x3F,
                refers_to,
                data: &[],
            });
            // An immediate generic region of unknown length ends with a marker and a 4-byte
            // row count (7.2.7, 7.4.6.4), searched for after its region info, flags and AT
            // pixels, as the decoder does.
            let Some(&region_flags) = data.get(pos + 17) else { break };
            let mmr = region_flags & 1 != 0;
            let at_bytes = match (mmr, (region_flags >> 1) & 3, region_flags & 0x10 != 0) {
                (true, _, _) => 0,
                (false, 0, false) => 8,
                (false, 0, true) => 24,
                (false, _, _) => 2,
            };
            let marker: [u8; 2] = if mmr { [0x00, 0x00] } else { [0xFF, 0xAC] };
            let from = pos + 18 + at_bytes;
            let Some(end) = data
                .get(from..)
                .and_then(|rest| rest.windows(6).position(|w| w[..2] == marker))
            else {
                break;
            };
            at = from + end + 6;
            continue;
        }
        let end = pos.saturating_add(length as usize).min(data.len());
        out.push(Jbig2Segment {
            number,
            kind: flags & 0x3F,
            refers_to,
            data: data.get(pos..end).unwrap_or_default(),
        });
        at = pos.saturating_add(length as usize);
    }
    Ok(out)
}

/// The bytes the JBIG2 decoder keeps outside its memory limit for the symbol dictionaries
/// (7.4.2) in JBIG2 data in the embedded organization, read from their segment headers: the
/// arithmetic decoder contexts of a dictionary that retains them, and the list of its exported
/// symbols (one pointer each; re-exporting earlier dictionaries' symbols shares the bitmaps but
/// not the list). The decoder keeps both until the picture is done.
fn jbig2_dictionary_bytes(data: &[u8]) -> Result<usize, String> {
    const SYMBOLS: u8 = 0;
    Ok(jbig2_segments(data)?
        .iter()
        .filter(|s| s.kind == SYMBOLS)
        .fold(0usize, |total, s| total.saturating_add(symbol_dictionary_bytes(s.data))))
}

/// A segment that a segment of the picture (`data`) or of its shared part (`globals`) refers
/// to, and that neither holds. The decoder skips such a reference, so a text region whose
/// symbol dictionary is missing would come out blank without a word (#523).
fn jbig2_missing_segment(data: &[u8], globals: Option<&[u8]>) -> Result<Option<u32>, String> {
    let mut segments = jbig2_segments(data)?;
    if let Some(g) = globals {
        segments.extend(jbig2_segments(g)?);
    }
    let held: std::collections::BTreeSet<u32> = segments.iter().map(|s| s.number).collect();
    Ok(segments
        .iter()
        .flat_map(|s| s.refers_to.iter())
        .find(|r| !held.contains(r))
        .copied())
}

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
    fn jbig2_symbol_dictionaries_shared_through_globals_are_read() {
        let text = include_bytes!("../testdata/jbig2/symbol.jbig2");
        let global = include_bytes!("../testdata/jbig2/symbol_global.jbig2");
        // Its first segment is the symbol dictionary, for page 0 (shared): an 11-byte header
        // whose last 4 bytes give the data length. A PDF keeps that segment in the picture's
        // `JBIG2Globals` stream and the page's segments (text region, end of page) in the
        // picture itself.
        let segments = embedded(global);
        let Some(length) = segments.get(7..11).and_then(|b| <[u8; 4]>::try_from(b).ok()) else {
            unreachable!("the test stream has a segment header");
        };
        let split = 11 + u32::from_be_bytes(length) as usize;
        let (globals, page) = segments.split_at(split);
        assert_eq!(globals.get(4).map(|f| f & 0x3F), Some(0), "a symbol dictionary");
        assert_eq!(globals.get(6), Some(&0), "for page 0");
        let Ok(single) = jbig2(embedded(text), None) else {
            unreachable!("the single-stream picture decodes");
        };
        assert_eq!(
            jbig2(page, Some(globals)),
            Ok(single.clone()),
            "the same pixels from shared segments"
        );
        // Without its globals the text region's dictionary is not there: said, not drawn
        // blank (#523).
        assert_eq!(
            jbig2(page, None),
            Err(
                "the JBIG2 picture refers to its segment 0, which neither the picture nor its JBIG2Globals holds"
                    .into()
            )
        );
        assert_eq!(jbig2_missing_segment(page, Some(globals)), Ok(None));
        assert_eq!(jbig2_missing_segment(embedded(text), None), Ok(None));
        // The shared dictionary counts against the shared half of the limit.
        assert!(jbig2_within(page, Some(globals), 64).is_err_and(|e| e.contains("too much memory")));
        assert!(jbig2_within(page, Some(globals), 8 << 10).is_ok());
    }

    #[test]
    fn jbig2_references_of_every_size_are_read() {
        // Segment 300 (2-byte references) refers to 299 and 2; segment 2 (1-byte) to 1.
        let mut data = vec![0, 0, 0, 2, 0x06, 0x20, 1, 1, 0, 0, 0, 0];
        data.extend_from_slice(&[0, 0, 1, 0x2C, 0x06, 0x40, 0x01, 0x2B, 0x00, 0x02, 1, 0, 0, 0, 0]);
        let Ok(segments) = jbig2_segments(&data) else {
            unreachable!("the headers are whole");
        };
        let read: Vec<(u32, Vec<u32>)> = segments.iter().map(|s| (s.number, s.refers_to.clone())).collect();
        assert_eq!(read, vec![(2, vec![1]), (300, vec![299, 2])]);
        assert_eq!(jbig2_missing_segment(&data, None), Ok(Some(1)));
        let globals = [0, 0, 0, 1, 0x00, 0x00, 0, 0, 0, 0, 0];
        assert_eq!(jbig2_missing_segment(&data, Some(&globals)), Ok(Some(299)));
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
        // The page needs about 3 KiB; it gets half the limit, the shared segments the other.
        assert!(jbig2_within(embedded(text), None, 8 << 10).is_ok());
        assert!(too_much(jbig2_within(embedded(text), None, 4 << 10)));
        assert!(too_much(jbig2_within(embedded(text), Some(embedded(large)), 48 << 10)));
    }

    /// A symbol dictionary segment that keeps its arithmetic decoder contexts and has no
    /// symbols.
    fn kept_contexts(number: u32) -> Vec<u8> {
        let mut b = number.to_be_bytes().to_vec();
        // Symbol dictionary (type 0), no referred-to segments, page 1, 10 bytes of data.
        b.extend_from_slice(&[0, 0, 1, 0, 0, 0, 10]);
        // Flags 0x0200 (contexts kept, template 0), then 8 AT bytes; the header is cut short
        // before the number of exported symbols, which then counts as none.
        b.extend_from_slice(&[0x02, 0x00, 0, 0, 0, 0, 0, 0, 0, 0]);
        b
    }

    #[test]
    fn jbig2_symbol_dictionaries_that_keep_their_contexts_count_against_the_limit() {
        let many: Vec<u8> = (0..1000).flat_map(kept_contexts).collect();
        assert_eq!(jbig2_dictionary_bytes(&many), Ok(1000 * JBIG2_CONTEXTS));
        // 1000 such dictionaries are about 70 MiB, more than half of a 128 MiB limit.
        let refused = jbig2_within(&many, None, 128 << 20);
        assert!(refused.is_err_and(|e| e.contains("too much memory")));
        // In the shared segments too.
        let refused = jbig2_within(&[], Some(&many), 128 << 20);
        assert!(refused.is_err_and(|e| e.contains("too much memory")));
        // A dictionary without the flag keeps none, and a cut-off header stops the walk.
        let mut plain = kept_contexts(0);
        plain[11] = 0;
        assert_eq!(jbig2_dictionary_bytes(&plain), Ok(0));
        assert_eq!(jbig2_dictionary_bytes(&many[..5]), Ok(0));
        assert!(jbig2_dictionary_bytes(&[0, 0, 0, 0, 0, 0xA0]).is_err());
    }

    #[test]
    fn jbig2_symbol_dictionaries_that_export_many_symbols_count_against_the_limit() {
        let exporting = |number: u32| {
            let mut b = number.to_be_bytes().to_vec();
            b.extend_from_slice(&[0, 0, 1, 0, 0, 0, 10]);
            // Huffman coded (no AT pixels), 2^20 exported symbols, no new ones.
            b.extend_from_slice(&[0x00, 0x01, 0x00, 0x10, 0x00, 0x00, 0, 0, 0, 0]);
            b
        };
        let one = size_of::<usize>() << 20;
        assert_eq!(jbig2_dictionary_bytes(&exporting(0)), Ok(one));
        // Twenty dictionaries that re-export a million symbols each keep 160 MiB of pointers
        // on a 64-bit target, more than half of a 128 MiB limit.
        let many: Vec<u8> = (0..20).flat_map(exporting).collect();
        let refused = jbig2_within(&many, None, 128 << 20);
        assert!(refused.is_err_and(|e| e.contains("too much memory")));

        // Where the number of exported symbols is, by the coding and templates in the flags.
        let count_at = |flags: u16, at: usize| {
            let mut head = flags.to_be_bytes().to_vec();
            head.resize(2 + at, 0);
            head.extend_from_slice(&3u32.to_be_bytes());
            symbol_dictionary_bytes(&head)
        };
        let three = 3 * size_of::<usize>();
        assert_eq!(count_at(0x0000, 8), three); // arithmetic, template 0
        assert_eq!(count_at(0x0400, 2), three); // arithmetic, template 1
        assert_eq!(count_at(0x0002, 12), three); // with refinement, refinement template 0
        assert_eq!(count_at(0x1002, 8), three); // refinement template 1
        assert_eq!(count_at(0x0003, 4), three); // Huffman with refinement
        assert_eq!(count_at(0x0201, 0), JBIG2_CONTEXTS + three);
        assert_eq!(symbol_dictionary_bytes(&[0x00]), 0);
    }
}

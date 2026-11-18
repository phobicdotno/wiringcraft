//! The container of AutoCAD 2007 drawings (`AC1021`), which differs from the 2004 and 2010 one.
//!
//! A 2007 file starts with 0x80 bytes of metadata, then a file header of 0x400 bytes. Its first
//! 3 x 255 bytes are three Reed-Solomon (255, 239) code words, interleaved byte by byte; their
//! data holds the compressed header, which says where the page map is, which page holds the
//! section map, and how both are stored. Pages start at 0x480 and follow each other in the
//! order of the page map, which lists each page's number and size.
//!
//! The page map and the section map are *system pages*: compressed, padded to 8 bytes, repeated
//! a number of times and coded as interleaved Reed-Solomon (255, 239) words. The section map
//! lists each section by a UTF-16 name with its pages: where each page goes in the section, its
//! sizes and a checksum. *Data pages* are compressed and coded as Reed-Solomon (255, 251)
//! words, interleaved or not. A page of zeros is not written at all; the section map then skips
//! its place. 2007 files use their own LZ77-like compression, unlike 2004 and 2010.
//!
//! Written from the published Open Design Specification for .dwg files, chapter 5. Where it is
//! unclear (the field polynomials, where the first instruction of the compressed header starts,
//! the length of a section name) a real AutoCAD 2007 file settled it. Code words of the file
//! header and the system pages are checked, not corrected: a damaged one gives an error. Data
//! pages are checked with their checksum.

use std::collections::BTreeMap;

use super::{MAX_PAGE, MAX_PAGES, Reader, Section, Sections, Stream, Version, bad, offset, to_usize};
use crate::Result;

/// Where the file header starts.
const FILE_HEADER_AT: usize = 0x80;
/// The code words of the file header.
const HEADER_WORDS: usize = 3;
/// What the file header decompresses to.
const HEADER_SIZE: usize = 0x110;
/// Page addresses in the page map count from here.
const PAGES_START: u64 = 0x480;
/// The length of a Reed-Solomon code word, and how many of its bytes are data in system pages
/// and in data pages.
const WORD: usize = 255;
const SYSTEM_DATA: usize = 239;
const PAGE_DATA: usize = 251;
/// Length of a section description in the section map, without its name and pages, and of a
/// page description.
const SECTION_DESCRIPTION: usize = 0x40;
const PAGE_DESCRIPTION: usize = 0x38;
/// The most bytes a section name may take.
const MAX_NAME: u64 = 0x200;
/// The most times a system page may be repeated.
const MAX_REPEAT: u64 = 0x100;

/// GF(256) for Reed-Solomon words, from its field polynomial.
pub(super) struct Field {
    /// The generator's powers 0 to 254.
    exp: [u8; 255],
    /// The power each non-zero byte is (0 for 0, never looked up).
    log: [u8; 256],
}

impl Field {
    /// Built at compile time: an index out of range would fail the build, not a read.
    const fn new(poly: u16) -> Self {
        let mut exp = [0u8; 255];
        let mut log = [0u8; 256];
        let mut x: u16 = 1;
        let mut i = 0;
        while i < 255 {
            exp[i] = x as u8;
            log[(x & 0xFF) as usize] = i as u8;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= poly;
            }
            i += 1;
        }
        Field { exp, log }
    }

    fn log(&self, a: u8) -> usize {
        usize::from(self.log.get(usize::from(a)).copied().unwrap_or(0))
    }

    pub(super) fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            return 0;
        }
        self.power(self.log(a) + self.log(b))
    }

    /// The `i`th power of the field's generator.
    pub(super) fn power(&self, i: usize) -> u8 {
        self.exp.get(i % 255).copied().unwrap_or(0)
    }

    /// Whether `word` (byte `i` the coefficient of x^i) is a code word with `parity` check
    /// bytes: zero at the generator's powers 1 to `parity`.
    pub(super) fn is_code_word(&self, word: &[u8], parity: usize) -> bool {
        (1..=parity).all(|j| {
            let root = self.power(j);
            word.iter().rev().fold(0u8, |s, &c| self.mul(s, root) ^ c) == 0
        })
    }
}

/// The field of the file header and system pages: x^8 + x^6 + x^5 + x^3 + 1.
pub(super) const SYSTEM_FIELD: Field = Field::new(0x169);
/// The field of data pages, x^8 + x^4 + x^3 + x^2 + 1. Their code words are not checked (each
/// page has a checksum), only the tests write them.
#[cfg(test)]
pub(super) const PAGE_FIELD: Field = Field::new(0x11D);

/// Read the named sections of an AutoCAD 2007 file.
pub(super) fn read_sections(bytes: &[u8], max_total: usize) -> Result<Sections> {
    let header = file_header(bytes)?;
    let h = Reader(header.as_slice());
    let page_map_at = h
        .u64(0x38)?
        .checked_add(PAGES_START)
        .ok_or_else(|| bad("the page map address is out of range"))?;
    let page_map = system_page(
        bytes,
        page_map_at,
        None,
        Stored {
            compressed: h.u64(0x50)?,
            size: h.u64(0x58)?,
            repeat: h.u64(0x18)?,
        },
        "page map",
    )?;
    let pages = parse_page_map(&page_map, bytes.len())?;
    let section_map_id = h.u64(0xC0)?;
    let (section_map_at, extent) = *pages.get(&section_map_id).ok_or_else(|| {
        bad(format!(
            "the section map (page {section_map_id}) is not in the page map"
        ))
    })?;
    let section_map = system_page(
        bytes,
        section_map_at,
        Some(extent),
        Stored {
            compressed: h.u64(0xB0)?,
            size: h.u64(0xC8)?,
            repeat: h.u64(0xD8)?,
        },
        &format!("section map (page {section_map_id})"),
    )?;
    read_section_map(bytes, &section_map, &pages, max_total)
}

/// Decode the file header: check its code words, take their data and decompress it.
fn file_header(bytes: &[u8]) -> Result<Vec<u8>> {
    let words = bytes
        .get(FILE_HEADER_AT..FILE_HEADER_AT + HEADER_WORDS * WORD)
        .ok_or_else(|| bad("the file is too short to hold its header"))?;
    let data = deinterleave(words, HEADER_WORDS, SYSTEM_DATA, &SYSTEM_FIELD)
        .map_err(|_| bad("the file header is damaged (Reed-Solomon check failed)"))?;
    let r = Reader(data.as_slice());
    // A negative length is a header stored without compression.
    let length = r.i32(0x18)?;
    let stored = length < 0;
    let length = length.unsigned_abs() as usize;
    let packed = data
        .get(0x20..0x20 + length)
        .ok_or_else(|| bad("the file header is damaged (its length is out of range)"))?;
    let header = if stored {
        packed.to_vec()
    } else {
        decompress(packed, HEADER_SIZE).map_err(|e| bad(format!("the file header: {e}")))?
    };
    if header.len() < HEADER_SIZE {
        return Err(bad("the file header is cut off"));
    }
    Ok(header)
}

/// Split `words` code words, interleaved byte by byte, check each and join their first `data`
/// bytes.
fn deinterleave(raw: &[u8], words: usize, data: usize, field: &Field) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(words * data);
    let mut word = [0u8; WORD];
    for j in 0..words {
        for (i, b) in word.iter_mut().enumerate() {
            *b = raw
                .get(i * words + j)
                .copied()
                .ok_or_else(|| bad("a Reed-Solomon word is cut off"))?;
        }
        if !field.is_code_word(&word, WORD - data) {
            return Err(bad("a Reed-Solomon word is damaged"));
        }
        out.extend(word.iter().take(data));
    }
    Ok(out)
}

/// How a system page is stored, from the file header.
struct Stored {
    compressed: u64,
    size: u64,
    repeat: u64,
}

/// Read a system page (page map or section map) at `at`. `extent` is the page's size from the
/// page map, which it stays in (the page map itself is read before there is one).
fn system_page(bytes: &[u8], at: u64, extent: Option<u64>, how: Stored, what: &str) -> Result<Vec<u8>> {
    let start = offset(at, bytes.len(), what)?;
    let size = to_usize(u32::try_from(how.size).unwrap_or(u32::MAX), MAX_PAGE, what)?;
    let compressed = to_usize(u32::try_from(how.compressed).unwrap_or(u32::MAX), MAX_PAGE, what)?;
    if how.repeat == 0 || how.repeat > MAX_REPEAT {
        return Err(bad(format!("the {what} is repeated {} times", how.repeat)));
    }
    // The data padded to 8 bytes, repeated, makes up the data of the code words.
    let words = (compressed.next_multiple_of(8) as u64 * how.repeat).div_ceil(SYSTEM_DATA as u64);
    let length = words * WORD as u64;
    if extent.is_some_and(|e| length > e) {
        return Err(bad(format!("the {what} reaches past its page")));
    }
    let raw = usize::try_from(length)
        .ok()
        .and_then(|l| bytes.get(start..start.checked_add(l)?))
        .ok_or_else(|| bad(format!("the {what} is cut off")))?;
    let data = deinterleave(raw, words as usize, SYSTEM_DATA, &SYSTEM_FIELD)
        .map_err(|_| bad(format!("the {what} is damaged (Reed-Solomon check failed)")))?;
    let packed = data
        .get(..compressed)
        .ok_or_else(|| bad(format!("the {what} is cut off")))?;
    unpack(packed, size).map_err(|e| bad(format!("the {what}: {e}")))
}

/// Data stored compressed when that made it shorter, else as it is.
fn unpack(packed: &[u8], size: usize) -> Result<Vec<u8>> {
    match packed.len().cmp(&size) {
        std::cmp::Ordering::Less => decompress(packed, size),
        std::cmp::Ordering::Equal => Ok(packed.to_vec()),
        std::cmp::Ordering::Greater => Err(bad(format!("{} stored bytes for {size} bytes of data", packed.len()))),
    }
}

/// The address and size of each page, by page number, from the decompressed page map: pairs of
/// size and number, a negative number for unused space.
fn parse_page_map(map: &[u8], file_len: usize) -> Result<BTreeMap<u64, (u64, u64)>> {
    if !map.len().is_multiple_of(16) {
        return Err(bad(format!(
            "the page map ends inside an entry ({} bytes, entries of 16)",
            map.len()
        )));
    }
    let r = Reader(map);
    let mut pages = BTreeMap::new();
    let mut at = PAGES_START;
    let mut pos = 0;
    while pos + 16 <= map.len() {
        let size = r.i64(pos)?;
        let number = r.i64(pos + 8)?;
        pos += 16;
        if size <= 0 {
            return Err(bad(format!("page {number} has size {size} in the page map")));
        }
        if number >= 0 && pages.insert(number as u64, (at, size as u64)).is_some() {
            return Err(bad(format!("page {number} is listed twice in the page map")));
        }
        if pages.len() > MAX_PAGES {
            return Err(bad("the page map lists too many pages"));
        }
        if at >= file_len as u64 {
            return Err(bad("the page map reaches past the end of the file"));
        }
        at = at
            .checked_add(size as u64)
            .ok_or_else(|| bad("the page map reaches past the end of the file"))?;
    }
    Ok(pages)
}

/// Read every section the decompressed section map describes.
fn read_section_map(bytes: &[u8], map: &[u8], pages: &BTreeMap<u64, (u64, u64)>, max_total: usize) -> Result<Sections> {
    let r = Reader(map);
    let mut pos = 0;
    let mut sections = Vec::new();
    let mut skipped = Vec::new();
    let mut total = 0usize;
    // What the pages may still decompress to.
    let mut budget = max_total;
    // Each section's number is its place in the map, from 1.
    let mut index = 0u32;
    while pos + SECTION_DESCRIPTION <= map.len() {
        let size = r.u64(pos)?;
        let encrypted = r.u64(pos + 0x10)?;
        let name_len = r.u64(pos + 0x20)?;
        let encoding = r.u64(pos + 0x30)?;
        let page_count = r.u64(pos + 0x38)?;
        pos += SECTION_DESCRIPTION;
        index += 1;
        // The name in UTF-16, its length in bytes with the ending zero.
        let name_len = usize::try_from(name_len)
            .ok()
            .filter(|n| *n as u64 <= MAX_NAME && pos + n <= map.len())
            .ok_or_else(|| bad("a section name in the section map is cut off"))?;
        let name = utf16(map.get(pos..pos + name_len).unwrap_or_default());
        pos += name_len;
        let list_len = usize::try_from(page_count)
            .ok()
            .and_then(|c| c.checked_mul(PAGE_DESCRIPTION))
            .filter(|l| pos.checked_add(*l).is_some_and(|end| end <= map.len()))
            .ok_or_else(|| bad(format!("the page list of section {name} is cut off")))?;
        let list = pos;
        pos += list_len;
        if encrypted == 1 {
            skipped.push(name);
            continue;
        }
        // The map ends with an unnamed entry without pages.
        if name.is_empty() && page_count == 0 {
            continue;
        }
        let size = usize::try_from(size)
            .ok()
            .filter(|s| total.checked_add(*s).is_some_and(|t| t <= max_total))
            .ok_or_else(|| bad(format!("section {name} is too large")))?;
        let interleaved = match encoding {
            4 => true,
            1 => false,
            e => return Err(bad(format!("section {name} has unknown encoding {e}"))),
        };
        let mut data = vec![0u8; size];
        // Every page listed so far, by start: a start may only be listed again for the same
        // page with the same sizes and checksum. Empty pages are in here too.
        let mut listed: BTreeMap<usize, PageEntry> = BTreeMap::new();
        // The ends of the pages that hold bytes, by start, for the overlap check. Empty pages
        // are left out, so that one inside another page cannot hide it from a later page.
        let mut filled: BTreeMap<usize, usize> = BTreeMap::new();
        for i in 0..page_count as usize {
            let at = list + i * PAGE_DESCRIPTION;
            let page = PageEntry {
                start: r.u64(at)?,
                number: r.u64(at + 0x10)?,
                size: r.u64(at + 0x18)?,
                compressed: r.u64(at + 0x20)?,
                checksum: r.u64(at + 0x28)?,
            };
            let number = page.number;
            let (address, extent) = *pages.get(&number).ok_or_else(|| {
                bad(format!(
                    "section {name} uses page {number}, which is not in the page map"
                ))
            })?;
            let start = usize::try_from(page.start)
                .ok()
                .filter(|s| *s <= size)
                .ok_or_else(|| bad(format!("section {name}, page {number} starts past its end")))?;
            // A start already used: the same page listed again is skipped (its bytes are
            // there), anything else is refused before it is read or copied.
            if let Some(other) = listed.get(&start) {
                if other.number != number {
                    return Err(bad(format!(
                        "section {name}, page {number} starts where page {} does",
                        other.number
                    )));
                }
                if (other.size, other.compressed, other.checksum) != (page.size, page.compressed, page.checksum) {
                    return Err(bad(format!(
                        "section {name}, page {number} is listed again with other sizes or another checksum"
                    )));
                }
                // A repeat costs what reading the page again would, so that repeats stay bounded.
                let cost = usize::try_from(page.size.max(page.compressed))
                    .unwrap_or(usize::MAX)
                    .max(1);
                budget = budget
                    .checked_sub(cost)
                    .ok_or_else(|| bad("the file's pages decompress to too much"))?;
                continue;
            }
            let read = data_page(bytes, address, extent, &page, interleaved, &mut budget)
                .map_err(|e| bad(format!("section {name}, page {number}: {e}")))?;
            let end = start
                .checked_add(read.len())
                .filter(|e| *e <= data.len())
                .ok_or_else(|| bad(format!("section {name}, page {number} reaches past its end")))?;
            // Pages that hold bytes may not overlap.
            if end > start {
                let before = filled.range(..start).next_back().is_some_and(|(_, e)| *e > start);
                let after = filled.range(start + 1..).next().is_some_and(|(s, _)| *s < end);
                if before || after {
                    return Err(bad(format!("section {name}, page {number} overlaps another page")));
                }
                filled.insert(start, end);
            }
            if let Some(to) = data.get_mut(start..end) {
                to.copy_from_slice(&read);
            }
            listed.insert(start, page);
        }
        total += size;
        sections.push(Section { name, id: index, data });
    }
    if pos != map.len() {
        return Err(bad(format!(
            "the section map ends inside a section description ({} bytes left)",
            map.len() - pos
        )));
    }
    Ok(Sections {
        version: Version::R2007,
        sections,
        skipped,
        codepage: bytes
            .get(0x13..0x15)
            .and_then(|b| b.try_into().ok())
            .map_or(0, u16::from_le_bytes),
    })
}

/// A UTF-16 name up to its first zero.
fn utf16(raw: &[u8]) -> String {
    let units = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|u| *u != 0);
    char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// A page as the section map describes it.
struct PageEntry {
    start: u64,
    number: u64,
    size: u64,
    compressed: u64,
    checksum: u64,
}

/// Read one data page at `address` that the page map gives `extent` bytes: take its data from
/// its code words, decompress it and check its checksum. What the page may give is taken from
/// `budget` before it is read.
fn data_page(
    bytes: &[u8],
    address: u64,
    extent: u64,
    page: &PageEntry,
    interleaved: bool,
    budget: &mut usize,
) -> Result<Vec<u8>> {
    let start = offset(address, bytes.len(), "data page")?;
    let size = to_usize(u32::try_from(page.size).unwrap_or(u32::MAX), MAX_PAGE, "page")?;
    let compressed = to_usize(
        u32::try_from(page.compressed).unwrap_or(u32::MAX),
        MAX_PAGE,
        "page's stored data",
    )?;
    // Every page costs at least a byte, also an empty one listed many times.
    let cost = size.max(compressed).max(1);
    *budget = budget
        .checked_sub(cost)
        .ok_or_else(|| bad("the file's pages decompress to too much"))?;
    let words = compressed.next_multiple_of(8).div_ceil(PAGE_DATA);
    // Interleaved, the data is spread over all code words; else it comes first, followed by
    // the check bytes of all words.
    let length = if interleaved { words * WORD } else { compressed };
    if length as u64 > extent {
        return Err(bad("the page's data reaches past its page"));
    }
    let raw = bytes
        .get(start..start.saturating_add(length))
        .ok_or_else(|| bad("the page's data is cut off"))?;
    let packed = if interleaved {
        let mut data = Vec::with_capacity(words * PAGE_DATA);
        for j in 0..words {
            data.extend((0..PAGE_DATA).filter_map(|i| raw.get(i * words + j)));
        }
        data.truncate(compressed);
        data
    } else {
        raw.to_vec()
    };
    let data = unpack(&packed, size)?;
    if u64::from(page_checksum(0, &data)) != page.checksum {
        return Err(bad("the page's data is damaged (wrong checksum)"));
    }
    Ok(data)
}

/// The checksum of a 2007 data page: two sums of the bytes modulo 65521 started from a mix of
/// `seed` and the length, the bytes taken in pairs from the end of each group of eight.
pub(super) fn page_checksum(seed: u64, data: &[u8]) -> u32 {
    let seed = seed
        .wrapping_add(data.len() as u64)
        .wrapping_mul(0x343FD)
        .wrapping_add(0x26_9EC3);
    let (mut low, mut high) = (seed & 0xFFFF, (seed >> 16) & 0xFFFF);
    for chunk in data.chunks(0x15B0) {
        let (groups, rest) = chunk.as_chunks::<8>();
        for &[b0, b1, b2, b3, b4, b5, b6, b7] in groups {
            for b in [b6, b7, b4, b5, b2, b3, b0, b1] {
                low += u64::from(b);
                high += low;
            }
        }
        let order: &[usize] = match rest.len() {
            4 => &[2, 3, 0, 1],
            5 => &[2, 3, 0, 1, 4],
            6 => &[2, 3, 0, 1, 4, 5],
            7 => &[2, 3, 0, 1, 4, 5, 6],
            n => &[0, 1, 2][..n.min(3)],
        };
        for &i in order {
            low += u64::from(rest.get(i).copied().unwrap_or(0));
            high += low;
        }
        low %= 0xFFF1;
        high %= 0xFFF1;
    }
    ((high << 16) | (low & 0xFFFF)) as u32
}

/// The order a literal run of 1 to 32 bytes is stored in: for each length, parts of the
/// stored run (length, where it starts) in the order they are output. Each part is itself
/// stored in the order of its length.
const LITERAL_ORDER: [&[(usize, usize)]; 33] = [
    &[],
    &[(1, 0)],
    &[(1, 1), (1, 0)],
    &[(1, 2), (1, 1), (1, 0)],
    &[(1, 0), (1, 1), (1, 2), (1, 3)],
    &[(1, 4), (4, 0)],
    &[(1, 5), (4, 1), (1, 0)],
    &[(2, 5), (4, 1), (1, 0)],
    &[(4, 0), (4, 4)],
    &[(1, 8), (8, 0)],
    &[(1, 9), (8, 1), (1, 0)],
    &[(2, 9), (8, 1), (1, 0)],
    &[(4, 8), (8, 0)],
    &[(1, 12), (4, 8), (8, 0)],
    &[(1, 13), (4, 9), (8, 1), (1, 0)],
    &[(2, 13), (4, 9), (8, 1), (1, 0)],
    &[(8, 8), (8, 0)],
    &[(8, 9), (1, 8), (8, 0)],
    &[(1, 17), (16, 1), (1, 0)],
    &[(3, 16), (16, 0)],
    &[(4, 16), (16, 0)],
    &[(1, 20), (4, 16), (16, 0)],
    &[(2, 20), (4, 16), (16, 0)],
    &[(3, 20), (4, 16), (16, 0)],
    &[(8, 16), (16, 0)],
    &[(8, 17), (1, 16), (16, 0)],
    &[(1, 25), (8, 17), (1, 16), (16, 0)],
    &[(2, 25), (8, 17), (1, 16), (16, 0)],
    &[(4, 24), (8, 16), (16, 0)],
    &[(1, 28), (4, 24), (8, 16), (16, 0)],
    &[(2, 28), (4, 24), (8, 16), (16, 0)],
    &[(1, 30), (4, 26), (8, 18), (16, 2), (2, 0)],
    &[(16, 16), (16, 0)],
];

/// Output a stored literal run of at most 32 bytes in its order.
pub(super) fn unshuffle(stored: &[u8], out: &mut Vec<u8>) {
    if stored.len() <= 1 {
        out.extend_from_slice(stored);
        return;
    }
    for &(len, at) in LITERAL_ORDER.get(stored.len()).copied().unwrap_or_default() {
        if let Some(part) = stored.get(at..at + len) {
            unshuffle(part, out);
        }
    }
}

/// Decompress the 2007 compression into exactly `size` bytes.
///
/// The stream is literal runs, each followed by one or more copies of earlier output. The
/// last byte of each copy instruction carries the length of a short literal run after it.
pub fn decompress(src: &[u8], size: usize) -> Result<Vec<u8>> {
    let mut r = Stream { src, pos: 0 };
    let mut out = Vec::new();
    let mut len = 0usize;
    let mut op = 0u8;
    // An instruction already read that starts a literal run.
    let mut have_op = false;
    // A stream may start with 0x2? and two bytes, then a byte whose low bits are a literal
    // length.
    if src.first().is_some_and(|b| b & 0xF0 == 0x20) {
        r.pos = 3;
        len = usize::from(r.byte()? & 7);
    }
    while r.pos < src.len() {
        if len == 0 {
            if !have_op {
                op = r.byte()?;
            }
            len = literal_length(&mut r, op)?;
        }
        have_op = false;
        literal(&mut r, &mut out, len, size)?;
        len = 0;
        if r.pos >= src.len() {
            break;
        }
        op = r.byte()?;
        let (mut next, mut back, mut count) = instruction(&mut r, op)?;
        loop {
            copy(&mut out, back, count, size)?;
            len = usize::from(next & 7);
            if len != 0 || r.pos >= src.len() {
                break;
            }
            op = r.byte()?;
            if op >> 4 == 0 {
                have_op = true;
                break;
            }
            if op >> 4 == 15 {
                op &= 15;
            }
            (next, back, count) = instruction(&mut r, op)?;
        }
    }
    if len != 0 || have_op {
        return Err(bad("compressed data ends before a literal run it announces"));
    }
    if out.len() != size {
        return Err(bad(format!(
            "compressed data gives {} bytes, the page holds {size}",
            out.len()
        )));
    }
    Ok(out)
}

/// The length of a literal run whose instruction is `op`.
fn literal_length(r: &mut Stream, op: u8) -> Result<usize> {
    let mut len = usize::from(op) + 8;
    if len == 0x17 {
        let n = usize::from(r.byte()?);
        len += n;
        if n == 0xFF {
            loop {
                let n = usize::from(r.byte()?) | (usize::from(r.byte()?) << 8);
                len = len.saturating_add(n);
                if n != 0xFFFF {
                    break;
                }
            }
        }
    }
    Ok(len)
}

/// A copy instruction starting with `op`: the byte that ends it (its low bits a literal
/// length), how far back the copy starts and how many bytes it copies.
fn instruction(r: &mut Stream, op: u8) -> Result<(u8, usize, usize)> {
    let low = usize::from(op & 0x0F);
    Ok(match op >> 4 {
        0 => {
            let b = usize::from(r.byte()?);
            let next = r.byte()?;
            let n = usize::from(next);
            (next, ((n & 0x78) << 5) + 1 + b, low + 0x13 + ((n >> 3) & 0x10))
        }
        1 => {
            let b = usize::from(r.byte()?);
            let next = r.byte()?;
            (next, ((usize::from(next) & 0xF8) << 5) + 1 + b, low + 3)
        }
        2 => {
            let mut back = usize::from(r.byte()?);
            back |= usize::from(r.byte()?) << 8;
            let mut count = usize::from(op & 7);
            if op & 8 == 0 {
                let next = r.byte()?;
                count += usize::from(next & 0xF8);
                (next, back, count)
            } else {
                back += 1;
                count += usize::from(r.byte()?) << 3;
                let next = r.byte()?;
                count += (usize::from(next & 0xF8) << 8) + 0x100;
                (next, back, count)
            }
        }
        _ => {
            let next = r.byte()?;
            (next, ((usize::from(next) & 0xF8) << 1) + low + 1, usize::from(op >> 4))
        }
    })
}

fn copy(out: &mut Vec<u8>, back: usize, count: usize, size: usize) -> Result<()> {
    let from = out
        .len()
        .checked_sub(back)
        .filter(|_| back > 0)
        .ok_or_else(|| bad("compressed data refers to bytes before its start"))?;
    if out.len() + count > size {
        return Err(bad("compressed data is longer than its page"));
    }
    for i in from..from + count {
        let b = out.get(i).copied().unwrap_or(0);
        out.push(b);
    }
    Ok(())
}

fn literal(r: &mut Stream, out: &mut Vec<u8>, n: usize, size: usize) -> Result<()> {
    if out.len().checked_add(n).is_none_or(|end| end > size) {
        return Err(bad("compressed data is longer than its page"));
    }
    let stored = r
        .src
        .get(
            r.pos
                ..r.pos
                    .checked_add(n)
                    .ok_or_else(|| bad("compressed data ends too early"))?,
        )
        .ok_or_else(|| bad("compressed data ends too early"))?;
    for run in stored.chunks(32) {
        unshuffle(run, out);
    }
    r.pos += n;
    Ok(())
}

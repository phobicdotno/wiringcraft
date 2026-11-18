//! The container of AutoCAD DWG files from AutoCAD 2004 to 2018 (`AC1018`, `AC1021`,
//! `AC1024`, `AC1027`, `AC1032`): the first step of reading DWG drawings, which SolidWorks
//! Electrical stores its symbol and page graphics in (#59).
//!
//! A DWG file of these versions is a set of pages. A short header at the start holds a second,
//! XOR-scrambled header that says where the *page map* (each page's number and size) and the
//! *section map* (each named section and the pages it is made of) are. Both maps and most
//! section pages are compressed with an LZ77-like scheme. [`read_sections`] follows the maps and
//! returns each named section (`AcDb:Header`, `AcDb:Classes`, `AcDb:Handles`,
//! `AcDb:AcDbObjects`, ...) as plain bytes, for the later steps that decode objects and entities.
//!
//! AutoCAD 2007 files (`AC1021`) use a different container, with Reed-Solomon coded pages and
//! another compression; [`read_sections`] reads them too and returns the same sections (#215).
//! AutoCAD 2013 and 2018 files (`AC1027`, `AC1032`) are in the 2004 container again, with one
//! more section (`AcDb:AcDsPrototype_1b`, the data store), and are read like 2010 ones (#292).
//! Older and newer versions are refused with a clear message. Sections stored encrypted
//! (password-protected drawings) are listed in [`Sections::skipped`] and not returned.
//!
//! [`read_drawing`] goes on to decode the objects in those sections into blocks and shapes; see
//! [`objects`].
//!
//! Written from the published Open Design Specification for .dwg files and LibreDWG's reading
//! of it, tested with files the tests write themselves and checked against 1666 real AutoCAD
//! 2004 and 2010 files of a SolidWorks Electrical symbol library. The 2007 container is written
//! from the specification only (see [`r2007`]) and checked against a real AutoCAD 2007 drawing;
//! 2013 and 2018 support is written from the specification only and checked against 33 real
//! AutoCAD 2013 and 110 real AutoCAD 2018 drawings.
//! Input is treated as hostile: every size and offset is checked, and a damaged file gives an
//! error naming what is wrong, never a panic.

use std::collections::BTreeMap;

use crate::{Error, Result};

mod bits;
pub mod objects;
pub mod r2007;

pub use objects::{Drawing, read_drawing};

/// The most a single page may decompress to, in bytes.
pub const MAX_PAGE: usize = 64 * 1024 * 1024;
/// The most all sections of a file may hold in all, in bytes, so a small hostile file cannot
/// fill the memory. It also bounds the work: all the pages a file's sections list may
/// decompress to this much in all, so a page listed many times cannot keep the reader busy.
pub const MAX_TOTAL: usize = 512 * 1024 * 1024;
/// The most pages a file may have.
pub const MAX_PAGES: usize = 1_000_000;

/// Where the scrambled second header starts, and its length.
const ENCRYPTED_HEADER_AT: usize = 0x80;
const ENCRYPTED_HEADER_LEN: usize = 0x6C;
/// What the second header starts with once unscrambled.
const HEADER_MAGIC: &[u8; 12] = b"AcFssFcAJMB\0";
/// Page addresses in the page map count from here.
const PAGES_START: u64 = 0x100;
/// Page type markers.
const PAGE_MAP: u32 = 0x4163_0E3B;
const SECTION_MAP: u32 = 0x4163_003B;
const DATA_PAGE: u32 = 0x4163_043B;
/// A data page's header is scrambled with this, XOR the page's address.
const DATA_PAGE_MASK: u32 = 0x4164_536B;
/// Length of a system page's header (page map, section map) and of a data page's header.
const SYSTEM_PAGE_HEADER: usize = 0x14;
/// Where the second header keeps its CRC-32.
const HEADER_CRC_AT: usize = 0x68;
const DATA_PAGE_HEADER: usize = 0x20;
/// Length of a section description in the section map, without its page list.
const SECTION_DESCRIPTION: usize = 0x60;

/// The DWG versions whose container this module reads, oldest first: a later version compares
/// greater, so `v >= Version::R2010` means 2010 or later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Version {
    /// AutoCAD 2004 to 2006 (`AC1018`).
    R2004,
    /// AutoCAD 2007 to 2009 (`AC1021`).
    R2007,
    /// AutoCAD 2010 to 2012 (`AC1024`).
    R2010,
    /// AutoCAD 2013 to 2017 (`AC1027`).
    R2013,
    /// AutoCAD 2018 and later (`AC1032`).
    R2018,
}

/// One named section of a DWG file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The section's name, such as `AcDb:AcDbObjects`.
    pub name: String,
    /// The section's number in the section map.
    pub id: u32,
    /// The section's bytes, decompressed and in order.
    pub data: Vec<u8>,
}

/// The sections of a DWG file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sections {
    pub version: Version,
    /// The sections in the order the section map lists them.
    pub sections: Vec<Section>,
    /// Names of sections not read because they are stored encrypted.
    pub skipped: Vec<String>,
}

impl Sections {
    /// The bytes of the section with this name, if the file has it.
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.sections.iter().find(|s| s.name == name).map(|s| s.data.as_slice())
    }
}

fn bad(what: impl Into<String>) -> Error {
    Error::Format(format!("DWG: {}", what.into()))
}

/// Read the version string at the start of a DWG file.
pub fn version(bytes: &[u8]) -> Result<Version> {
    let tag = bytes
        .get(..6)
        .ok_or_else(|| bad("the file is too short to be a drawing"))?;
    match tag {
        b"AC1018" => Ok(Version::R2004),
        b"AC1021" => Ok(Version::R2007),
        b"AC1024" => Ok(Version::R2010),
        b"AC1027" => Ok(Version::R2013),
        b"AC1032" => Ok(Version::R2018),
        t if t.starts_with(b"AC") => Err(bad(format!(
            "drawing version {} is not supported; save it as AutoCAD 2018, 2013, 2010, 2007 or 2004",
            String::from_utf8_lossy(t)
        ))),
        _ => Err(bad("not a DWG drawing (it does not start with a version)")),
    }
}

/// Read the named sections of a DWG file of a supported version.
pub fn read_sections(bytes: &[u8]) -> Result<Sections> {
    read_sections_with(bytes, MAX_TOTAL)
}

/// [`read_sections`] with another bound than [`MAX_TOTAL`] on what the sections may hold and
/// their pages may decompress to, in bytes.
pub fn read_sections_with(bytes: &[u8], max_total: usize) -> Result<Sections> {
    let version = version(bytes)?;
    if version == Version::R2007 {
        return r2007::read_sections(bytes, max_total);
    }
    let header = second_header(bytes)?;
    let page_map_at = header
        .u64(0x54)?
        .checked_add(PAGES_START)
        .ok_or_else(|| bad("the page map address is out of range"))?;
    let section_map_id = header.u32(0x5C)?;

    let page_map = system_page(bytes, page_map_at, None, PAGE_MAP, "page map")?;
    let pages = parse_page_map(&page_map, bytes.len())?;

    let (section_map_at, section_map_extent) = *pages.get(&section_map_id).ok_or_else(|| {
        bad(format!(
            "the section map (page {section_map_id}) is not in the page map"
        ))
    })?;
    let section_map = system_page(
        bytes,
        section_map_at,
        Some(section_map_extent),
        SECTION_MAP,
        &format!("section map (page {section_map_id})"),
    )?;
    read_section_map(bytes, &section_map, &pages, version, max_total)
}

/// Unscramble the second header and check it is one.
fn second_header(bytes: &[u8]) -> Result<Reader<Vec<u8>>> {
    let scrambled = bytes
        .get(ENCRYPTED_HEADER_AT..ENCRYPTED_HEADER_AT + ENCRYPTED_HEADER_LEN)
        .ok_or_else(|| bad("the file is too short to hold its header"))?;
    let header = unscramble_header(scrambled);
    if !header.starts_with(HEADER_MAGIC) {
        return Err(bad("the file header is damaged"));
    }
    // A CRC-32 of the header with its own field as zero.
    let stored = Reader(&header).u32(HEADER_CRC_AT)?;
    let mut zeroed = header.clone();
    zeroed[HEADER_CRC_AT..HEADER_CRC_AT + 4].fill(0);
    if crc32(&zeroed) != stored {
        return Err(bad("the file header is damaged (wrong checksum)"));
    }
    Ok(Reader(header))
}

/// The checksum of system and data pages: two sums of the bytes modulo 65521, the first
/// continuing from the low half of `seed` and the second from its high half (Adler-32 with a
/// seed).
pub(crate) fn page_checksum(seed: u32, data: &[u8]) -> u32 {
    let (mut low, mut high) = (u64::from(seed & 0xFFFF), u64::from(seed >> 16));
    for chunk in data.chunks(0x15B0) {
        for &b in chunk {
            low += u64::from(b);
            high += low;
        }
        low %= 0xFFF1;
        high %= 0xFFF1;
    }
    ((high as u32) << 16) | low as u32
}

/// The CRC-32 (as zlib computes it) of `data`.
pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// XOR `data` with the byte sequence the second header is scrambled with. Doing it twice gives
/// the original back.
pub(crate) fn unscramble_header(data: &[u8]) -> Vec<u8> {
    let mut seed: u32 = 1;
    data.iter()
        .map(|b| {
            seed = seed.wrapping_mul(0x343FD).wrapping_add(0x26_9EC3);
            b ^ (seed >> 16) as u8
        })
        .collect()
}

/// Read a page map or section map page at `at`: check its type and decompress it.
/// `extent` is the page's size from the page map, which its header and data stay in (the page
/// map itself is read before there is one).
fn system_page(bytes: &[u8], at: u64, extent: Option<u64>, kind: u32, what: &str) -> Result<Vec<u8>> {
    let start = offset(at, bytes.len(), what)?;
    let head = Reader(
        bytes
            .get(start..start.saturating_add(SYSTEM_PAGE_HEADER))
            .ok_or_else(|| bad(format!("the {what} is cut off")))?,
    );
    if head.u32(0)? != kind {
        return Err(bad(format!("the {what} is damaged (wrong page type)")));
    }
    let size = to_usize(head.u32(4)?, MAX_PAGE, what)?;
    let compressed_size = head.u32(8)? as usize;
    if extent.is_some_and(|e| SYSTEM_PAGE_HEADER as u64 + compressed_size as u64 > e) {
        return Err(bad(format!("the {what} reaches past its page")));
    }
    let compression = head.u32(0x0C)?;
    let data_at = start + SYSTEM_PAGE_HEADER;
    let data = bytes
        .get(data_at..data_at.saturating_add(compressed_size))
        .ok_or_else(|| bad(format!("the {what} is cut off")))?;
    // The checksum of the header (its own field as zero), carried on over the data.
    let mut zeroed = bytes
        .get(start..start + SYSTEM_PAGE_HEADER)
        .ok_or_else(|| bad(format!("the {what} is cut off")))?
        .to_vec();
    zeroed[0x10..0x14].fill(0);
    if page_checksum(page_checksum(0, &zeroed), data) != head.u32(0x10)? {
        return Err(bad(format!("the {what} is damaged (wrong checksum)")));
    }
    match compression {
        2 => decompress(data, size).map_err(|e| bad(format!("the {what}: {e}"))),
        1 => data
            .get(..size)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| bad(format!("the {what} is cut off"))),
        c => Err(bad(format!("the {what} has unknown compression {c}"))),
    }
}

/// The address of each page, by page number, from the decompressed page map.
fn parse_page_map(map: &[u8], file_len: usize) -> Result<BTreeMap<u32, (u64, u64)>> {
    let r = Reader(map);
    let mut pages = BTreeMap::new();
    let mut at = PAGES_START;
    let mut pos = 0;
    while pos + 8 <= map.len() {
        let number = r.i32(pos)?;
        let size = r.i32(pos + 4)?;
        pos += 8;
        if size <= 0 {
            return Err(bad(format!("page {number} has size {size} in the page map")));
        }
        if number < 0 {
            // A gap (unused space): parent, left, right and a zero follow.
            pos += 16;
        } else if pages.insert(number as u32, (at, size as u64)).is_some() {
            return Err(bad(format!("page {number} is listed twice in the page map")));
        }
        if pages.len() > MAX_PAGES {
            return Err(bad("the page map lists too many pages"));
        }
        // Each page starts in the file. The last one's padding may be cut short (a real file
        // ends one byte before it); reading a page checks what it holds.
        if at >= file_len as u64 {
            return Err(bad("the page map reaches past the end of the file"));
        }
        at += size as u64;
    }
    Ok(pages)
}

/// Read every section the decompressed section map describes.
fn read_section_map(
    bytes: &[u8],
    map: &[u8],
    pages: &BTreeMap<u32, (u64, u64)>,
    version: Version,
    max_total: usize,
) -> Result<Sections> {
    let r = Reader(map);
    let count = r.u32(0)? as usize;
    let mut pos = 0x14;
    let mut sections = Vec::new();
    let mut skipped = Vec::new();
    let mut total = 0usize;
    // What the pages may still decompress to.
    let mut budget = max_total;
    for _ in 0..count {
        let size = r.u64(pos)?;
        let page_count = r.u32(pos + 0x08)? as usize;
        let max_page = r.u32(pos + 0x0C)?;
        let compressed = r.u32(pos + 0x14)?;
        let id = r.u32(pos + 0x18)?;
        let encrypted = r.u32(pos + 0x1C)?;
        let name = r.name(pos + 0x20)?;
        pos += SECTION_DESCRIPTION;
        let size = usize::try_from(size)
            .ok()
            .filter(|s| total.checked_add(*s).is_some_and(|t| t <= max_total))
            .ok_or_else(|| bad(format!("section {name} is too large")))?;
        let list_len = page_count
            .checked_mul(16)
            .filter(|l| pos + l <= map.len())
            .ok_or_else(|| bad(format!("the page list of section {name} is cut off")))?;
        let list = pos;
        pos += list_len;
        if encrypted == 1 {
            skipped.push(name);
            continue;
        }
        let mut data = Vec::new();
        for i in 0..page_count {
            let at = list + i * 16;
            let number = r.u32(at)?;
            let start = r.u64(at + 8)?;
            let (address, extent) = *pages.get(&number).ok_or_else(|| {
                bad(format!(
                    "section {name} uses page {number}, which is not in the page map"
                ))
            })?;
            let start = usize::try_from(start)
                .ok()
                .filter(|s| *s <= size)
                .ok_or_else(|| bad(format!("section {name}, page {number} starts past its end")))?;
            let page = data_page(bytes, address, extent, compressed, max_page, &mut budget)
                .map_err(|e| bad(format!("section {name}, page {number}: {e}")))?;
            // A page short of what the section still needs from it would leave a hole of zeros.
            // A section with bytes to give needs pages that can hold some.
            if max_page == 0 && start < size {
                return Err(bad(format!("section {name} has pages of size 0")));
            }
            let needs = (size - start).min(max_page as usize);
            if page.len() < needs {
                return Err(bad(format!(
                    "section {name}, page {number} gives {} bytes, the section needs {needs} from it",
                    page.len()
                )));
            }
            let end = start.saturating_add(page.len()).min(size);
            if data.len() < end {
                data.resize(end, 0);
            }
            if let (Some(to), Some(from)) = (data.get_mut(start..end), page.get(..end - start)) {
                to.copy_from_slice(from);
            }
        }
        data.resize(size, 0);
        total += size;
        sections.push(Section { name, id, data });
    }
    Ok(Sections {
        version,
        sections,
        skipped,
    })
}

/// Read one data page at `address`: unscramble its header, check it, and decompress it to at
/// most `max_page` bytes (the section's page size). What a page may give is taken from `budget`
/// before it is decompressed.
///
/// The header's word at 0x0C is the page's size in the file (header and data, rounded up to 32
/// bytes), not what it decompresses to; it is not needed.
fn data_page(
    bytes: &[u8],
    address: u64,
    extent: u64,
    compressed: u32,
    max_page: u32,
    budget: &mut usize,
) -> Result<Vec<u8>> {
    let start = offset(address, bytes.len(), "data page")?;
    let raw = bytes
        .get(start..start.saturating_add(DATA_PAGE_HEADER))
        .ok_or_else(|| bad("the page is cut off"))?;
    let head = Reader(unscramble_page_header(raw, address));
    if head.u32(0)? != DATA_PAGE {
        return Err(bad("the page is damaged (wrong page type)"));
    }
    let data_size = to_usize(head.u32(0x08)?, MAX_PAGE, "page")?;
    // The data stays in the page the page map gives it (only the last page's padding may be
    // missing from the file, which the read below checks).
    if DATA_PAGE_HEADER as u64 + data_size as u64 > extent {
        return Err(bad("the page's data reaches past its page"));
    }
    let data_at = start + DATA_PAGE_HEADER;
    let data = bytes
        .get(data_at..data_at.saturating_add(data_size))
        .ok_or_else(|| bad("the page's data is cut off"))?;
    let too_much = || bad("the file's pages decompress to too much");
    // Every page costs at least a byte, also an empty one listed many times; checking and
    // reading its stored bytes costs them.
    if data_size.max(1) > *budget {
        return Err(bad("the file's pages hold too much to read"));
    }
    // The data's checksum, then the header's (its own field as zero) carried on from it.
    let data_sum = page_checksum(0, data);
    if data_sum != head.u32(0x1C)? {
        return Err(bad("the page's data is damaged (wrong checksum)"));
    }
    let mut zeroed = head.0.clone();
    zeroed[0x18..0x1C].fill(0);
    if page_checksum(data_sum, &zeroed) != head.u32(0x18)? {
        return Err(bad("the page header is damaged (wrong checksum)"));
    }
    let page = match compressed {
        2 => decompress_at_most(data, to_usize(max_page, MAX_PAGE, "page")?)?,
        1 => data.to_vec(),
        c => return Err(bad(format!("unknown compression {c}"))),
    };
    // What the page gave, or its stored bytes when more (reading them costs too, also for a
    // page listed many times).
    *budget = budget
        .checked_sub(page.len().max(data_size).max(1))
        .ok_or_else(too_much)?;
    Ok(page)
}

/// XOR each 32-bit word of a data page header with the page's mask. Doing it twice gives the
/// original back.
pub(crate) fn unscramble_page_header(raw: &[u8], address: u64) -> Vec<u8> {
    let mask = DATA_PAGE_MASK ^ (address as u32);
    raw.chunks(4)
        .flat_map(|w| {
            let mut word = [0u8; 4];
            for (to, from) in word.iter_mut().zip(w) {
                *to = *from;
            }
            (u32::from_le_bytes(word) ^ mask).to_le_bytes()
        })
        .take(raw.len())
        .collect()
}

fn offset(at: u64, len: usize, what: &str) -> Result<usize> {
    usize::try_from(at)
        .ok()
        .filter(|a| *a < len)
        .ok_or_else(|| bad(format!("the {what} is past the end of the file")))
}

fn to_usize(n: u32, max: usize, what: &str) -> Result<usize> {
    let n = n as usize;
    if n > max {
        return Err(bad(format!("the {what} is too large ({n} bytes)")));
    }
    Ok(n)
}

/// Decompress DWG's LZ77-like page compression into exactly `size` bytes.
///
/// The stream starts with an optional run of literal bytes, then a list of instructions, each
/// copying bytes from earlier in the output and then a few new literal bytes, and ends with
/// `0x11`.
pub fn decompress(src: &[u8], size: usize) -> Result<Vec<u8>> {
    let out = decompress_at_most(src, size)?;
    if out.len() != size {
        return Err(bad(format!(
            "compressed data gives {} bytes, the page holds {size}",
            out.len()
        )));
    }
    Ok(out)
}

/// [`decompress`] where the data may give fewer bytes than `size`, but not more: a data page,
/// whose header does not say what it decompresses to.
pub fn decompress_at_most(src: &[u8], size: usize) -> Result<Vec<u8>> {
    let mut r = Stream { src, pos: 0 };
    let mut out = Vec::new();
    let mut op = r.byte()?;
    if op & 0xF0 == 0 {
        let n = literal_length(&mut r, op)?;
        literal(&mut r, &mut out, n, size)?;
        op = r.byte()?;
    }
    loop {
        let (len, back, lits) = match op {
            0x11 => break,
            0x10 => {
                let len = long_count(&mut r)? + 9;
                let (back, lits) = two_byte_offset(&mut r)?;
                (len, back + 0x3FFF, lits)
            }
            0x12..=0x1F => {
                let (back, lits) = two_byte_offset(&mut r)?;
                (usize::from(op & 0x0F) + 2, back + 0x3FFF, lits)
            }
            0x20 => {
                let len = long_count(&mut r)? + 0x21;
                let (back, lits) = two_byte_offset(&mut r)?;
                (len, back, lits)
            }
            0x21..=0x3F => {
                let (back, lits) = two_byte_offset(&mut r)?;
                (usize::from(op) - 0x1E, back, lits)
            }
            0x40..=0xFF => {
                let op2 = r.byte()?;
                let back = (usize::from(op2) << 2) | usize::from((op & 0x0C) >> 2);
                (usize::from(op >> 4) - 1, back, usize::from(op & 0x03))
            }
            _ => {
                return Err(bad(format!(
                    "compressed data is damaged (instruction {op:#04x} at byte {})",
                    r.pos - 1
                )));
            }
        };
        let from = out
            .len()
            .checked_sub(back + 1)
            .ok_or_else(|| bad("compressed data refers to bytes before its start"))?;
        if out.len() + len > size {
            return Err(bad("compressed data is longer than its page"));
        }
        for i in from..from + len {
            let b = out.get(i).copied().unwrap_or(0);
            out.push(b);
        }
        if lits == 0 {
            op = r.byte()?;
            if op & 0xF0 == 0 {
                let n = literal_length(&mut r, op)?;
                literal(&mut r, &mut out, n, size)?;
                op = r.byte()?;
            }
        } else {
            literal(&mut r, &mut out, lits, size)?;
            op = r.byte()?;
        }
    }
    Ok(out)
}

struct Stream<'a> {
    src: &'a [u8],
    pos: usize,
}

impl Stream<'_> {
    fn byte(&mut self) -> Result<u8> {
        let b = self
            .src
            .get(self.pos)
            .copied()
            .ok_or_else(|| bad("compressed data ends too early"))?;
        self.pos += 1;
        Ok(b)
    }
}

/// The length of a literal run whose first byte is `op` (`0x00`..=`0x0F`).
fn literal_length(r: &mut Stream, op: u8) -> Result<usize> {
    if op != 0 {
        return Ok(usize::from(op) + 3);
    }
    Ok(0x0F + long_count(r)? + 3)
}

/// A count written as zero bytes worth 0xFF each, ended by a non-zero byte that is added.
fn long_count(r: &mut Stream) -> Result<usize> {
    let mut n = 0usize;
    loop {
        let b = r.byte()?;
        if b != 0 {
            return Ok(n + usize::from(b));
        }
        n += 0xFF;
    }
}

/// A back-reference distance in two bytes; the low two bits of the first are a literal count.
fn two_byte_offset(r: &mut Stream) -> Result<(usize, usize)> {
    let a = r.byte()?;
    let b = r.byte()?;
    Ok(((usize::from(a) >> 2) | (usize::from(b) << 6), usize::from(a & 0x03)))
}

fn literal(r: &mut Stream, out: &mut Vec<u8>, n: usize, size: usize) -> Result<()> {
    if out.len() + n > size {
        return Err(bad("compressed data is longer than its page"));
    }
    let bytes = r
        .src
        .get(r.pos..r.pos + n)
        .ok_or_else(|| bad("compressed data ends too early"))?;
    out.extend_from_slice(bytes);
    r.pos += n;
    Ok(())
}

/// Checked little-endian reads from a buffer.
struct Reader<T: AsRef<[u8]>>(T);

impl<T: AsRef<[u8]>> Reader<T> {
    fn bytes<const N: usize>(&self, at: usize) -> Result<[u8; N]> {
        self.0
            .as_ref()
            .get(at..at.saturating_add(N))
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| bad("a map or header is cut off"))
    }

    fn u32(&self, at: usize) -> Result<u32> {
        self.bytes(at).map(u32::from_le_bytes)
    }

    fn i32(&self, at: usize) -> Result<i32> {
        self.bytes(at).map(i32::from_le_bytes)
    }

    fn u64(&self, at: usize) -> Result<u64> {
        self.bytes(at).map(u64::from_le_bytes)
    }

    fn i64(&self, at: usize) -> Result<i64> {
        self.bytes(at).map(i64::from_le_bytes)
    }

    /// A name in a 64-byte field, ended by a zero byte.
    fn name(&self, at: usize) -> Result<String> {
        let raw: [u8; 64] = self.bytes(at)?;
        let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
        Ok(String::from_utf8_lossy(raw.get(..end).unwrap_or(&raw)).into_owned())
    }
}

#[cfg(test)]
pub(crate) mod tests;

use std::collections::HashMap;

use super::*;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

// ---- A compressor for the tests, written from the same description as `decompress` ----

/// Compress `data` in DWG's page compression, with a greedy search for repeats. It uses every
/// kind of instruction: near copies (`0x40`..), copies up to 0x3FFF back (`0x20`..`0x3F`) and
/// copies further back (`0x10`..`0x1F`), short and long lengths, and literal runs of any length.
fn compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut seen: HashMap<[u8; 3], usize> = HashMap::new();
    let mut pending: Vec<u8> = Vec::new();
    // Where the low two bits of the last copy's offset byte are, for a literal count of 1..=3.
    let mut lit_slot: Option<usize> = None;
    let mut pos = 0;
    while pos < data.len() {
        let mut best: Option<(usize, usize)> = None; // (length, distance)
        if let Some(key) = data.get(pos..pos + 3).and_then(|k| <[u8; 3]>::try_from(k).ok()) {
            if let Some(&prev) = seen.get(&key) {
                let distance = pos - prev;
                let mut len = 0;
                while pos + len < data.len() && data[prev + len] == data[pos + len] && len < 2000 {
                    len += 1;
                }
                // Far copies (distance over 0x4000) need at least 4 bytes; the very first
                // literal run must be at least 4 bytes (or none) to be written as one.
                let first_ok = lit_slot.is_some() || pending.len() >= 4;
                if distance <= 0x7FFF && (len >= 4 || (len >= 3 && distance <= 0x4000)) && first_ok {
                    best = Some((len, distance));
                }
            }
            seen.insert(key, pos);
        }
        match best {
            None => {
                pending.push(data[pos]);
                pos += 1;
            }
            Some((len, distance)) => {
                flush_literals(&mut out, &mut pending, lit_slot);
                lit_slot = Some(copy(&mut out, len, distance - 1));
                for p in pos + 1..pos + len {
                    if let Some(key) = data.get(p..p + 3).and_then(|k| <[u8; 3]>::try_from(k).ok()) {
                        seen.insert(key, p);
                    }
                }
                pos += len;
            }
        }
    }
    flush_literals(&mut out, &mut pending, lit_slot);
    out.push(0x11);
    out
}

fn flush_literals(out: &mut Vec<u8>, pending: &mut Vec<u8>, lit_slot: Option<usize>) {
    let n = pending.len();
    if n == 0 {
        return;
    }
    match lit_slot {
        Some(slot) if n <= 3 => out[slot] |= n as u8,
        _ => {
            assert!(n >= 4, "a literal run of {n} bytes cannot start the data");
            if n <= 18 {
                out.push((n - 3) as u8);
            } else {
                out.push(0);
                long_count_bytes(out, n - 3 - 0x0F);
            }
        }
    }
    out.append(pending);
}

fn long_count_bytes(out: &mut Vec<u8>, k: usize) {
    assert!(k >= 1);
    let zeros = (k - 1) / 0xFF;
    out.extend(std::iter::repeat_n(0, zeros));
    out.push((k - zeros * 0xFF) as u8);
}

/// Write a copy of `len` bytes from `back + 1` bytes back; returns where its literal count goes.
fn copy(out: &mut Vec<u8>, len: usize, back: usize) -> usize {
    if back <= 0x3FF && (3..=14).contains(&len) {
        out.push((((len + 1) << 4) | ((back & 3) << 2)) as u8);
        out.push((back >> 2) as u8);
        return out.len() - 2;
    }
    let offset = if back <= 0x3FFF {
        if len <= 33 {
            out.push((len + 0x1E) as u8);
        } else {
            out.push(0x20);
            long_count_bytes(out, len - 0x21);
        }
        back
    } else {
        if len <= 17 {
            out.push(0x10 | (len - 2) as u8);
        } else {
            out.push(0x10);
            long_count_bytes(out, len - 9);
        }
        back - 0x3FFF
    };
    out.push(((offset & 0x3F) << 2) as u8);
    out.push((offset >> 6) as u8);
    out.len() - 2
}

/// Bytes that look random, from a fixed seed.
fn noise(n: usize, mut seed: u32) -> Vec<u8> {
    (0..n)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect()
}

// ---- A DWG writer for the tests ----

#[derive(Default)]
pub(super) struct TestSection {
    pub(super) name: &'static str,
    pub(super) data: Vec<u8>,
    pub(super) encrypted: bool,
    /// Pages stored without compression.
    pub(super) stored: bool,
    /// How many more times the section map lists each page (a hostile file).
    pub(super) listed_again: usize,
    /// Added to each page's stored data size in its header (a damaged file).
    pub(super) claimed_extra: u32,
    /// The page size the section map gives instead of the real one (a damaged file).
    pub(super) page_size: Option<u32>,
    /// 2007: pages coded without interleaving (encoding 1).
    pub(super) plain: bool,
    /// 2007: pages of zeros are not written, as AutoCAD does.
    pub(super) zero_pages_left_out: bool,
}

const MAX_PAGE_DATA: usize = 0x7400;

fn pad32(v: &mut Vec<u8>) {
    while !v.len().is_multiple_of(0x20) {
        v.push(0);
    }
}

fn system_page_bytes(kind: u32, data: &[u8]) -> Vec<u8> {
    let packed = compress(data);
    let mut page = Vec::new();
    page.extend(kind.to_le_bytes());
    page.extend((data.len() as u32).to_le_bytes());
    page.extend((packed.len() as u32).to_le_bytes());
    page.extend(2u32.to_le_bytes());
    let sum = page_checksum(page_checksum(0, &[page.as_slice(), &[0u8; 4]].concat()), &packed);
    page.extend(sum.to_le_bytes());
    page.extend(packed);
    pad32(&mut page);
    page
}

/// Write a DWG file of `version` holding `sections`, each split into pages of at most 0x7400
/// bytes, followed by the section map and the page map.
pub(super) fn write_dwg(version: &[u8; 6], sections: &[TestSection]) -> Vec<u8> {
    let mut file = vec![0u8; 0x100];
    file[..6].copy_from_slice(version);
    let mut page_sizes: Vec<(i32, i32)> = Vec::new();
    let mut next_page = 1u32;
    // Section map: header, then each description with its page list.
    let mut map = Vec::new();
    map.extend((sections.len() as u32).to_le_bytes());
    map.extend(2u32.to_le_bytes());
    map.extend((MAX_PAGE_DATA as u32).to_le_bytes());
    map.extend(0u32.to_le_bytes());
    map.extend((sections.len() as u32).to_le_bytes());
    for (index, section) in sections.iter().enumerate() {
        let chunks: Vec<&[u8]> = section.data.chunks(MAX_PAGE_DATA).collect();
        map.extend((section.data.len() as u64).to_le_bytes());
        map.extend(((chunks.len() * (1 + section.listed_again)) as u32).to_le_bytes());
        map.extend(section.page_size.unwrap_or(MAX_PAGE_DATA as u32).to_le_bytes());
        map.extend(1u32.to_le_bytes());
        map.extend(if section.stored { 1u32 } else { 2 }.to_le_bytes());
        map.extend((index as u32 + 1).to_le_bytes());
        map.extend(u32::from(section.encrypted).to_le_bytes());
        let mut name = [0u8; 64];
        name[..section.name.len()].copy_from_slice(section.name.as_bytes());
        map.extend(name);
        let mut start = 0u64;
        for chunk in chunks {
            let number = next_page;
            next_page += 1;
            let address = file.len() as u64;
            let packed = if section.stored {
                chunk.to_vec()
            } else {
                compress(chunk)
            };
            let data_sum = page_checksum(0, &packed);
            let mut words = [
                DATA_PAGE,
                index as u32 + 1,
                packed.len() as u32 + section.claimed_extra,
                // The page's size in the file: header and data, rounded up to 32 bytes.
                (DATA_PAGE_HEADER + packed.len()).next_multiple_of(0x20) as u32,
                start as u32,
                0,
                0,
                data_sum,
            ];
            let bytes = |w: &[u32; 8]| w.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
            words[6] = page_checksum(data_sum, &bytes(&words));
            let head = bytes(&words);
            let mut page = unscramble_page_header(&head, address);
            page.extend(&packed);
            pad32(&mut page);
            for _ in 0..=section.listed_again {
                map.extend(number.to_le_bytes());
                map.extend((packed.len() as u32).to_le_bytes());
                map.extend(start.to_le_bytes());
            }
            page_sizes.push((number as i32, page.len() as i32));
            file.extend(page);
            start += chunk.len() as u64;
        }
    }
    let section_map_id = next_page;
    let section_map = system_page_bytes(SECTION_MAP, &map);
    page_sizes.push((section_map_id as i32, section_map.len() as i32));
    file.extend(section_map);

    // The page map lists itself too, so its size is found by trying until it fits.
    let page_map_id = section_map_id + 1;
    let page_map_at = file.len() as u64;
    let mut size = 0x40;
    let page_map = loop {
        let mut entries = Vec::new();
        for (number, len) in page_sizes.iter().chain([&(page_map_id as i32, size)]) {
            entries.extend(number.to_le_bytes());
            entries.extend(len.to_le_bytes());
        }
        let mut page = system_page_bytes(PAGE_MAP, &entries);
        if page.len() <= size as usize {
            page.resize(size as usize, 0);
            break page;
        }
        size = page.len() as i32;
    };
    file.extend(page_map);

    let mut header = vec![0u8; ENCRYPTED_HEADER_LEN];
    header[..12].copy_from_slice(HEADER_MAGIC);
    header[0x50..0x54].copy_from_slice(&page_map_id.to_le_bytes());
    header[0x54..0x5C].copy_from_slice(&(page_map_at - PAGES_START).to_le_bytes());
    header[0x5C..0x60].copy_from_slice(&section_map_id.to_le_bytes());
    let crc = crc32(&header);
    header[HEADER_CRC_AT..HEADER_CRC_AT + 4].copy_from_slice(&crc.to_le_bytes());
    file[ENCRYPTED_HEADER_AT..ENCRYPTED_HEADER_AT + ENCRYPTED_HEADER_LEN].copy_from_slice(&unscramble_header(&header));
    file
}

fn sample_sections() -> Vec<TestSection> {
    // Objects: over two pages, partly repetitive and partly noise.
    let mut objects = Vec::new();
    for i in 0..3000u32 {
        objects.extend(b"LINE ");
        objects.extend(i.to_le_bytes());
    }
    objects.extend(noise(40_000, 7));
    vec![
        TestSection {
            name: "AcDb:Header",
            data: b"header variables: $INSUNITS=4 $LTSCALE=1.0".repeat(3),
            encrypted: false,
            ..TestSection::default()
        },
        TestSection {
            name: "AcDb:Classes",
            data: noise(300, 3),
            encrypted: false,
            ..TestSection::default()
        },
        TestSection {
            name: "AcDb:SummaryInfo",
            data: b"secret summary".to_vec(),
            encrypted: true,
            ..TestSection::default()
        },
        TestSection {
            name: "AcDb:Handles",
            data: Vec::new(),
            encrypted: false,
            ..TestSection::default()
        },
        TestSection {
            name: "AcDb:AcDbObjects",
            data: objects,
            encrypted: false,
            ..TestSection::default()
        },
    ]
}

// ---- Decompression ----

#[test]
fn a_hand_written_stream_decompresses() -> TestResult {
    // Four literal bytes, then three bytes copied from four back, then the end.
    let stream = [0x01, b'a', b'b', b'c', b'd', 0x4C, 0x00, 0x11];
    assert_eq!(decompress(&stream, 7)?, b"abcdabc");
    // The same copy carrying two literal bytes in its low bits, then a long copy (0x20) of 40
    // bytes from one back.
    let stream = [
        0x01, b'a', b'b', b'c', b'd', 0x4E, 0x00, b'x', b'y', 0x20, 0x07, 0x00, 0x00, 0x11,
    ];
    let mut want = b"abcdabcxy".to_vec();
    want.extend([b'y'; 40]);
    assert_eq!(decompress(&stream, want.len())?, want);
    Ok(())
}

#[test]
fn compressed_data_comes_back_the_same() -> TestResult {
    let mut long_run = b"start".to_vec();
    long_run.extend([b'z'; 5000]);
    let mut far = noise(0x5000, 11);
    far.extend_from_within(10..400); // a copy from over 0x4000 back
    far.extend(noise(30, 5));
    far.extend_from_within(0x100..0x10C); // a short far copy
    let mut literals = noise(7, 1);
    literals.extend(noise(600, 2)); // a long literal run
    let cases: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"abcd".to_vec(),
        b"abcabcabcabcabc".to_vec(),
        long_run,
        far,
        literals,
        noise(MAX_PAGE_DATA, 9),
        b"WiringCraft ".repeat(500),
    ];
    for data in cases {
        let packed = compress(&data);
        assert_eq!(decompress(&packed, data.len())?, data, "{} bytes", data.len());
    }
    Ok(())
}

#[test]
fn damaged_compressed_data_gives_an_error() {
    // A copy from before the start.
    assert!(decompress(&[0x01, b'a', b'b', b'c', b'd', 0x4C, 0x05, 0x11], 7).is_err());
    // Ends without the end marker.
    assert!(decompress(&[0x01, b'a', b'b', b'c', b'd'], 4).is_err());
    // More bytes than the page holds.
    assert!(decompress(&[0x01, b'a', b'b', b'c', b'd', 0x11], 3).is_err());
    // Fewer bytes than the page holds.
    assert!(decompress(&[0x01, b'a', b'b', b'c', b'd', 0x11], 8).is_err());
    // A long run of zero bytes in a length is bounded by the page size, not the memory.
    let mut bomb = vec![0x01, b'a', b'b', b'c', b'd', 0x20];
    bomb.extend([0u8; 10_000]);
    bomb.extend([0x01, 0x00, 0x00, 0x11]);
    assert!(decompress(&bomb, 100).is_err());
}

// ---- Files ----

#[test]
fn the_sections_of_a_2010_drawing_are_read() -> TestResult {
    let sections = sample_sections();
    let file = write_dwg(b"AC1024", &sections);
    let read = read_sections(&file)?;
    assert_eq!(read.version, Version::R2010);
    let names: Vec<&str> = read.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["AcDb:Header", "AcDb:Classes", "AcDb:Handles", "AcDb:AcDbObjects"]
    );
    for section in sections.iter().filter(|s| !s.encrypted) {
        assert_eq!(
            read.get(section.name),
            Some(section.data.as_slice()),
            "{}",
            section.name
        );
    }
    assert!(sections[4].data.len() > 2 * MAX_PAGE_DATA, "objects span three pages");
    assert_eq!(read.skipped, ["AcDb:SummaryInfo"]);
    assert_eq!(read.get("AcDb:SummaryInfo"), None);
    Ok(())
}

#[test]
fn a_2004_drawing_is_read_too() -> TestResult {
    let file = write_dwg(b"AC1018", &sample_sections()[..2]);
    let read = read_sections(&file)?;
    assert_eq!(read.version, Version::R2004);
    assert_eq!(read.get("AcDb:Classes"), Some(noise(300, 3).as_slice()));
    Ok(())
}

#[test]
fn other_versions_are_refused_with_a_reason() {
    let message = |bytes: &[u8]| match read_sections(bytes) {
        Err(e) => e.to_string(),
        Ok(_) => String::from("read"),
    };
    // AutoCAD 2007 is read; a 2010 container named 2007 is not one.
    let mut r2007 = write_dwg(b"AC1024", &sample_sections()[..1]);
    r2007[..6].copy_from_slice(b"AC1021");
    r2007.resize(0x1000, 0);
    assert!(
        message(&r2007).contains("file header is damaged"),
        "{}",
        message(&r2007)
    );
    let old = message(b"AC1015 and more");
    assert!(old.contains("AC1015") && old.contains("2010, 2007 or 2004"), "{old}");
    assert!(message(b"PK\x03\x04 a zip file").contains("not a DWG"));
    assert!(message(b"AC").contains("too short"));
}

#[test]
fn a_damaged_header_is_reported() {
    let mut file = write_dwg(b"AC1024", &sample_sections()[..1]);
    file[ENCRYPTED_HEADER_AT + 3] ^= 0x40;
    let message = read_sections(&file).map(|_| ()).map_err(|e| e.to_string());
    assert!(
        message.as_ref().is_err_and(|m| m.contains("header is damaged")),
        "{message:?}"
    );
}

#[test]
fn cut_or_flipped_files_give_errors_not_panics() -> TestResult {
    let file = write_dwg(b"AC1024", &sample_sections());
    // Every cut-off length near the structures, and a sample of the rest.
    for len in (0..file.len()).step_by(7) {
        let _ = read_sections(&file[..len]);
    }
    assert!(read_sections(&file[..page_map_data_end(&file)? - 1]).is_err());
    // Flip bits all over, one place at a time.
    let mut seed = 1u32;
    for i in 0..1500 {
        let mut damaged = file.clone();
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        let at = (seed as usize >> 4) % damaged.len();
        damaged[at] ^= 1 << (i % 8);
        let _ = read_sections(&damaged);
    }
    // Every byte of the headers and maps (all but the data pages), each set to 0xFF.
    let maps_at = file.len() - 0x800;
    for at in (0..0x100).chain(maps_at..file.len()) {
        let mut damaged = file.clone();
        damaged[at] = 0xFF;
        let _ = read_sections(&damaged);
    }
    Ok(())
}

#[test]
fn stored_pages_are_read_and_a_cut_off_one_is_refused() -> TestResult {
    let section = |claimed_extra| TestSection {
        name: "AcDb:Classes",
        data: noise(300, 3),
        stored: true,
        claimed_extra,
        ..TestSection::default()
    };
    let read = read_sections(&write_dwg(b"AC1024", &[section(0)]))?;
    assert_eq!(read.get("AcDb:Classes"), Some(noise(300, 3).as_slice()));
    // The header says the page stores more than its page holds.
    let message = read_sections(&write_dwg(b"AC1024", &[section(0x10_0000)])).map_err(|e| e.to_string());
    assert!(
        message.as_ref().is_err_and(|m| m.contains("past its page")),
        "{message:?}"
    );
    // Also when another page follows, whose bytes it would otherwise take.
    let mut first = section(0x40);
    first.data = noise(300, 4);
    let next = TestSection {
        name: "AcDb:Header",
        data: noise(300, 5),
        stored: true,
        ..TestSection::default()
    };
    let message = read_sections(&write_dwg(b"AC1024", &[first, next])).map_err(|e| e.to_string());
    assert!(
        message
            .as_ref()
            .is_err_and(|m| m.contains("AcDb:Classes") && m.contains("past its page")),
        "{message:?}"
    );
    Ok(())
}

#[test]
fn a_page_listed_many_times_cannot_make_endless_work() -> TestResult {
    let section = |listed_again| TestSection {
        name: "AcDb:AcDbObjects",
        data: b"WiringCraft ".repeat(MAX_PAGE_DATA / 12),
        listed_again,
        ..TestSection::default()
    };
    // Room for the stored bytes of a fifth listing, not for what it decompresses to.
    let budget = 4 * MAX_PAGE_DATA + 0x1000;
    // Listed four times, the page fits the budget (each copy lands at the same place).
    let read = read_sections_with(&write_dwg(b"AC1024", &[section(3)]), budget)?;
    assert_eq!(
        read.get("AcDb:AcDbObjects").map(<[u8]>::len),
        Some(section(0).data.len())
    );
    // Listed five times, what the fifth decompresses to is over the budget.
    let message = read_sections_with(&write_dwg(b"AC1024", &[section(4)]), budget).map_err(|e| e.to_string());
    assert!(
        message.as_ref().is_err_and(|m| m.contains("decompress to too much")),
        "{message:?}"
    );
    Ok(())
}

#[test]
fn the_checksums_match_known_values() {
    // zlib's CRC-32 of "123456789", and Adler-32 (seed 1) of "Wikipedia".
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(page_checksum(1, b"Wikipedia"), 0x11E6_0398);
    // Carried on from another seed: "A" from 0, then "BC" from its sum.
    assert_eq!(page_checksum(0, b"A"), 0x0041_0041);
    assert_eq!(page_checksum(page_checksum(0, b"A"), b"BC"), 0x018A_00C6);
    // Long enough to wrap the sums several times.
    let long = vec![0xFFu8; 100_000];
    assert_eq!(page_checksum(1, &long), adler(&long));
}

/// Adler-32 the plain way, to check the chunked one.
fn adler(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn error_of(file: &[u8]) -> String {
    match read_sections(file) {
        Ok(_) => "read without an error".to_owned(),
        Err(e) => e.to_string(),
    }
}

#[test]
fn a_changed_byte_that_still_decompresses_is_caught_by_its_checksum() -> TestResult {
    // A stored page: any byte of it still "decompresses".
    let section = TestSection {
        name: "AcDb:Classes",
        data: noise(300, 3),
        stored: true,
        ..TestSection::default()
    };
    let file = write_dwg(b"AC1024", &[section]);
    assert!(read_sections(&file).is_ok());
    // The first page is at 0x100, its data after a 0x20-byte header.
    let mut data = file.clone();
    data[0x120 + 5] ^= 0x01;
    let m = error_of(&data);
    assert!(
        m.contains("AcDb:Classes") && m.contains("page 1") && m.contains("page's data is damaged (wrong checksum)"),
        "{m}"
    );
    // Its header: the start offset (not read otherwise) changed.
    let mut head = file.clone();
    head[0x100 + 0x10] ^= 0x01;
    let m = error_of(&head);
    assert!(
        m.contains("page 1") && m.contains("page header") && m.contains("checksum"),
        "{m}"
    );
    // The second header, a byte past the fields the reader uses.
    let mut header = file.clone();
    header[ENCRYPTED_HEADER_AT + 0x64] ^= 0x01;
    let m = error_of(&header);
    assert!(m.contains("header is damaged") && m.contains("checksum"), "{m}");
    // The section map's compressed data: its last byte, the end of a literal run.
    let section_map = file
        .windows(4)
        .rposition(|w| w == SECTION_MAP.to_le_bytes())
        .ok_or("no section map")?;
    let packed = u32::from_le_bytes(file[section_map + 8..section_map + 12].try_into()?) as usize;
    let mut map = file.clone();
    map[section_map + SYSTEM_PAGE_HEADER + packed - 3] ^= 0x01;
    let m = error_of(&map);
    assert!(m.contains("section map") && m.contains("checksum"), "{m}");
    Ok(())
}

#[test]
fn a_last_page_shorter_than_the_page_size_is_read() -> TestResult {
    // One full compressed page and a short one, then a short stored section.
    let data = noise(MAX_PAGE_DATA + 100, 5);
    let sections = [
        TestSection {
            name: "AcDb:AcDbObjects",
            data: data.clone(),
            ..TestSection::default()
        },
        TestSection {
            name: "AcDb:Classes",
            data: noise(10, 6),
            stored: true,
            ..TestSection::default()
        },
    ];
    let read = read_sections(&write_dwg(b"AC1018", &sections))?;
    assert_eq!(read.get("AcDb:AcDbObjects"), Some(data.as_slice()));
    assert_eq!(read.get("AcDb:Classes"), Some(noise(10, 6).as_slice()));
    // At most the page size, never more.
    assert!(decompress_at_most(&[0x01, b'a', b'b', b'c', b'd', 0x11], 8).is_ok());
    assert!(decompress_at_most(&[0x01, b'a', b'b', b'c', b'd', 0x11], 3).is_err());
    Ok(())
}

/// Where the page map's data ends (the page map is the last page).
fn page_map_data_end(file: &[u8]) -> std::result::Result<usize, Box<dyn std::error::Error>> {
    let at = file
        .windows(4)
        .rposition(|w| w == PAGE_MAP.to_le_bytes())
        .ok_or("no page map")?;
    let packed = u32::from_le_bytes(file[at + 8..at + 12].try_into()?) as usize;
    Ok(at + SYSTEM_PAGE_HEADER + packed)
}

#[test]
fn a_file_short_of_its_last_padding_is_read_and_one_cut_into_its_data_is_not() -> TestResult {
    let file = write_dwg(b"AC1024", &sample_sections()[..1]);
    let end = page_map_data_end(&file)?;
    // The page map is the last page: cut right after its data (no padding), it is read.
    assert!(read_sections(&file[..end]).is_ok());
    // Cut into its data, it is refused.
    assert!(read_sections(&file[..end - 1]).is_err());
    Ok(())
}

#[test]
fn a_stored_page_listed_many_times_is_bounded_too() -> TestResult {
    // Each listing of a stored page costs its stored bytes.
    let section = |listed_again| TestSection {
        name: "AcDb:AcDbObjects",
        data: noise(4000, 7),
        stored: true,
        listed_again,
        ..TestSection::default()
    };
    let budget = 3 * 4000;
    assert!(read_sections_with(&write_dwg(b"AC1024", &[section(2)]), budget).is_ok());
    // The fourth listing's stored bytes are over the budget before they are read.
    let message = read_sections_with(&write_dwg(b"AC1024", &[section(3)]), budget).map_err(|e| e.to_string());
    assert!(
        message
            .as_ref()
            .is_err_and(|m| m.contains("pages hold too much to read")),
        "{message:?}"
    );
    Ok(())
}

#[test]
fn a_compressed_page_costs_what_it_gives_not_the_page_size() -> TestResult {
    // 300 bytes in a section whose pages may hold 0x7400: fits a budget of 1000.
    let section = TestSection {
        name: "AcDb:Classes",
        data: noise(300, 8),
        ..TestSection::default()
    };
    let read = read_sections_with(&write_dwg(b"AC1024", &[section]), 1000)?;
    assert_eq!(read.get("AcDb:Classes").map(<[u8]>::len), Some(300));
    Ok(())
}

#[test]
fn a_page_short_of_what_its_section_needs_is_refused() -> TestResult {
    // Two pages of 0x7400, but the section map says pages hold 0x100 more: the first page
    // would leave a hole of zeros.
    let section = TestSection {
        name: "AcDb:AcDbObjects",
        data: noise(MAX_PAGE_DATA + 100, 9),
        page_size: Some(MAX_PAGE_DATA as u32 + 0x100),
        ..TestSection::default()
    };
    let m = read_sections(&write_dwg(b"AC1024", &[section])).map_err(|e| e.to_string());
    assert!(m.as_ref().is_err_and(|m| m.contains("page 1 gives")), "{m:?}");
    Ok(())
}

#[test]
fn a_section_map_reaching_past_its_page_is_refused() -> TestResult {
    let mut file = write_dwg(b"AC1024", &sample_sections()[..1]);
    let at = file
        .windows(4)
        .rposition(|w| w == SECTION_MAP.to_le_bytes())
        .ok_or("no section map")?;
    let packed = u32::from_le_bytes(file[at + 8..at + 12].try_into()?);
    file[at + 8..at + 12].copy_from_slice(&(packed + 0x1000).to_le_bytes());
    let m = read_sections(&file).map_err(|e| e.to_string());
    assert!(
        m.as_ref()
            .is_err_and(|m| m.contains("section map") && m.contains("reaches past its page")),
        "{m:?}"
    );
    Ok(())
}

#[test]
fn a_section_with_pages_of_size_0_is_refused() -> TestResult {
    let section = TestSection {
        name: "AcDb:Classes",
        data: noise(300, 10),
        stored: true,
        page_size: Some(0),
        ..TestSection::default()
    };
    let m = read_sections(&write_dwg(b"AC1024", &[section])).map_err(|e| e.to_string());
    assert!(m.as_ref().is_err_and(|m| m.contains("pages of size 0")), "{m:?}");
    Ok(())
}

#[test]
fn a_changed_byte_in_a_compressed_page_that_still_decompresses_is_caught() -> TestResult {
    let section = TestSection {
        name: "AcDb:Classes",
        data: noise(300, 11),
        ..TestSection::default()
    };
    let file = write_dwg(b"AC1024", &[section]);
    assert!(read_sections(&file).is_ok());
    // The first page's compressed data, at 0x100 after its 0x20-byte header.
    let head = unscramble_page_header(&file[0x100..0x120], 0x100);
    let size = u32::from_le_bytes(head[8..12].try_into()?) as usize;
    let packed = &file[0x120..0x120 + size];
    let good = decompress_at_most(packed, MAX_PAGE_DATA)?;
    // A byte whose change still decompresses (a literal), to something else.
    let at = (1..size)
        .find(|&i| {
            let mut p = packed.to_vec();
            p[i] ^= 0x01;
            decompress_at_most(&p, MAX_PAGE_DATA).is_ok_and(|d| d != good)
        })
        .ok_or("no literal byte")?;
    let mut damaged = file.clone();
    damaged[0x120 + at] ^= 0x01;
    let m = error_of(&damaged);
    assert!(
        m.contains("AcDb:Classes") && m.contains("page 1") && m.contains("page's data is damaged (wrong checksum)"),
        "{m}"
    );
    Ok(())
}

// ---- AutoCAD 2007: a compressor, Reed-Solomon words and a file writer for the tests ----

use super::r2007::{self, Field, PAGE_FIELD, SYSTEM_FIELD, unshuffle};

/// Store a literal run so that the reader's order gives `run` back.
fn shuffle(run: &[u8], out: &mut Vec<u8>) {
    for part in run.chunks(32) {
        let index: Vec<u8> = (0..part.len() as u8).collect();
        let mut order = Vec::new();
        unshuffle(&index, &mut order);
        let mut stored = vec![0u8; part.len()];
        for (t, &from) in order.iter().enumerate() {
            stored[usize::from(from)] = part[t];
        }
        out.extend(stored);
    }
}

/// Compress `data` in the 2007 compression, with a greedy search for repeats. It writes short
/// and long literal runs (also one starting the stream with 0x20) and copies of each kind
/// but the one only reachable as 0xF?: near (0x30..), up to 0x2000 back (0x10..) and up to
/// 0xFFFF back, short and long (0x20..).
pub(super) fn compress_2007(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut seen: HashMap<[u8; 3], usize> = HashMap::new();
    let mut pending: Vec<u8> = Vec::new();
    // Where the last copy's final byte is, whose low bits can hold a literal count.
    let mut slot: Option<usize> = None;
    let mut pos = 0;
    while pos < data.len() {
        let mut best = None;
        if let Some(key) = data.get(pos..pos + 3).and_then(|k| <[u8; 3]>::try_from(k).ok()) {
            if let Some(&prev) = seen.get(&key) {
                let back = pos - prev;
                let mut len = 0;
                while pos + len < data.len() && data[prev + len] == data[pos + len] && len < 0x1_00FF {
                    len += 1;
                }
                if back <= 0xFFFF && len >= 3 {
                    best = Some((len, back));
                }
            }
            seen.insert(key, pos);
        }
        match best {
            None => {
                pending.push(data[pos]);
                pos += 1;
            }
            Some((len, back)) => {
                literals_2007(&mut out, &mut pending, slot);
                slot = Some(copy_2007(&mut out, len, back));
                for p in pos + 1..pos + len {
                    if let Some(key) = data.get(p..p + 3).and_then(|k| <[u8; 3]>::try_from(k).ok()) {
                        seen.insert(key, p);
                    }
                }
                pos += len;
            }
        }
    }
    literals_2007(&mut out, &mut pending, slot);
    out
}

fn literals_2007(out: &mut Vec<u8>, pending: &mut Vec<u8>, slot: Option<usize>) {
    let n = pending.len();
    if n == 0 {
        return;
    }
    match slot {
        Some(slot) if n <= 7 => out[slot] |= n as u8,
        None if n <= 7 => out.extend([0x20, 0, 0, n as u8]),
        _ if n <= 0x16 => out.push((n - 8) as u8),
        _ => {
            out.push(0x0F);
            let x = n - 0x17;
            if x < 0xFF {
                out.push(x as u8);
            } else {
                out.push(0xFF);
                let mut rest = x - 0xFF;
                while rest >= 0xFFFF {
                    out.extend([0xFF, 0xFF]);
                    rest -= 0xFFFF;
                }
                out.extend((rest as u16).to_le_bytes());
            }
        }
    }
    shuffle(pending, out);
    pending.clear();
}

/// Write a copy of `len` bytes from `back` bytes back; returns where its final byte is.
fn copy_2007(out: &mut Vec<u8>, len: usize, back: usize) -> usize {
    let b = back - 1;
    if (3..=14).contains(&len) && back <= 0x200 {
        out.push(((len << 4) | (b & 15)) as u8);
        out.push(((b >> 4) << 3) as u8);
    } else if len <= 18 && back <= 0x2000 {
        out.push(0x10 | (len - 3) as u8);
        out.push((b & 0xFF) as u8);
        out.push(((b >> 8) << 3) as u8);
    } else if len <= 0xFF {
        out.push(0x20 | (len & 7) as u8);
        out.extend((back as u16).to_le_bytes());
        out.push((len & 0xF8) as u8);
    } else {
        let l = len - 0x100;
        out.push(0x28 | (l & 7) as u8);
        out.extend((b as u16).to_le_bytes());
        out.push(((l >> 3) & 0xFF) as u8);
        out.push((((l >> 11) & 0x1F) << 3) as u8);
    }
    out.len() - 1
}

/// The data compressed when that is shorter, else as it is.
fn packed_2007(data: &[u8]) -> Vec<u8> {
    let packed = compress_2007(data);
    if packed.len() < data.len() {
        packed
    } else {
        data.to_vec()
    }
}

/// A Reed-Solomon word of `data` with `parity` check bytes after it, byte `i` the
/// coefficient of x^i, zero at the field's powers 1 to `parity`.
fn rs_word(field: &Field, data: &[u8], parity: usize) -> Vec<u8> {
    // Reversed, the word is the usual systematic one: data high, check bytes low, with roots
    // at the negative powers.
    let mut generator = vec![1u8];
    for j in 1..=parity {
        let root = field.power(255 - j);
        let mut next = vec![0u8; generator.len() + 1];
        for (i, &c) in generator.iter().enumerate() {
            next[i + 1] ^= c;
            next[i] ^= field.mul(c, root);
        }
        generator = next;
    }
    let mut rest = vec![0u8; 255];
    for (t, &b) in data.iter().enumerate() {
        rest[254 - t] = b;
    }
    for i in (parity..255).rev() {
        let c = rest[i];
        if c != 0 {
            for (k, &g) in generator.iter().enumerate() {
                rest[i - parity + k] ^= field.mul(c, g);
            }
        }
    }
    let mut word = data.to_vec();
    word.extend((0..parity).map(|i| rest[parity - 1 - i]));
    word
}

/// Code `data` (a whole number of `k`-byte blocks) as Reed-Solomon words, interleaved.
fn interleave(field: &Field, data: &[u8], k: usize) -> Vec<u8> {
    let words: Vec<Vec<u8>> = data.chunks(k).map(|d| rs_word(field, d, 255 - k)).collect();
    let mut out = vec![0u8; words.len() * 255];
    for (j, word) in words.iter().enumerate() {
        for (i, &b) in word.iter().enumerate() {
            out[i * words.len() + j] = b;
        }
    }
    out
}

/// A system page: the data compressed, padded to 8 bytes, repeated and coded. Returns the
/// page and its stored and full sizes.
fn system_page_2007(data: &[u8], repeat: usize) -> (Vec<u8>, u64, u64) {
    let packed = packed_2007(data);
    let mut aligned = packed.clone();
    aligned.resize(packed.len().next_multiple_of(8), 0);
    let mut pre = aligned.repeat(repeat);
    pre.resize(pre.len().next_multiple_of(239), 0);
    let mut page = interleave(&SYSTEM_FIELD, &pre, 239);
    pad32(&mut page);
    (page, packed.len() as u64, data.len() as u64)
}

/// Write an AutoCAD 2007 file holding `sections`, each split into pages of at most 0x7400
/// bytes, followed by the section map and the page map.
pub(super) fn write_dwg_2007(sections: &[TestSection]) -> Vec<u8> {
    write_dwg_2007_with(sections, &|_| {}, &|_| {})
}

/// A page map entry: page number and size.
type PageEntry2007 = (i64, i64);

/// [`write_dwg_2007`] with the section map's bytes and the page map's entries (number, size)
/// changed before they are written (a hostile file whose maps are still good code words).
fn write_dwg_2007_with(
    sections: &[TestSection],
    edit_section_map: &dyn Fn(&mut Vec<u8>),
    edit_page_map: &dyn Fn(&mut Vec<PageEntry2007>),
) -> Vec<u8> {
    let mut file = vec![0u8; 0x480];
    file[..6].copy_from_slice(b"AC1021");
    let mut pages: Vec<(i64, i64)> = Vec::new();
    let mut next = 1u64;
    let mut map = Vec::new();
    let put = |map: &mut Vec<u8>, values: &[u64]| {
        for v in values {
            map.extend(v.to_le_bytes());
        }
    };
    for section in sections {
        let mut listed: Vec<[u64; 7]> = Vec::new();
        let mut start = 0u64;
        for chunk in section.data.chunks(MAX_PAGE_DATA) {
            if section.zero_pages_left_out && chunk.iter().all(|b| *b == 0) {
                start += chunk.len() as u64;
                continue;
            }
            let number = next;
            next += 1;
            let packed = if section.stored {
                chunk.to_vec()
            } else {
                packed_2007(chunk)
            };
            let mut pre = packed.clone();
            pre.resize(packed.len().next_multiple_of(8), 0);
            pre.resize(pre.len().next_multiple_of(251), 0);
            let mut page = if section.plain {
                // The data of all words, then their check bytes.
                let mut page = pre.clone();
                for d in pre.chunks(251) {
                    page.extend(&rs_word(&PAGE_FIELD, d, 4)[251..]);
                }
                page
            } else {
                interleave(&PAGE_FIELD, &pre, 251)
            };
            pad32(&mut page);
            let sum = u64::from(r2007::page_checksum(0, chunk));
            let stored = packed.len() as u64 + u64::from(section.claimed_extra);
            for _ in 0..=section.listed_again {
                listed.push([start, page.len() as u64, number, chunk.len() as u64, stored, sum, 0]);
            }
            pages.push((number as i64, page.len() as i64));
            file.extend(page);
            start += chunk.len() as u64;
        }
        let name: Vec<u8> = section
            .name
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        let encoding = if section.plain { 1 } else { 4 };
        put(
            &mut map,
            &[
                section.data.len() as u64,
                MAX_PAGE_DATA as u64,
                u64::from(section.encrypted),
                0,
                name.len() as u64,
                0,
                encoding,
                listed.len() as u64,
            ],
        );
        map.extend(name);
        for entry in listed {
            put(&mut map, &entry);
        }
    }
    // The map ends with an unnamed entry, as AutoCAD writes it.
    put(&mut map, &[0, 0xF800, 0, 0, 0, 0, 4, 0]);
    let section_map_id = next;
    edit_section_map(&mut map);
    let (section_map, sm_stored, sm_size) = system_page_2007(&map, 2);
    pages.push((section_map_id as i64, section_map.len() as i64));
    file.extend(section_map);

    // The page map lists itself too, so its size is found by trying until it fits.
    let page_map_id = section_map_id + 1;
    let page_map_at = file.len() as u64;
    let mut size = 0x400;
    let (page_map, pm_stored, pm_size) = loop {
        let mut listed = pages.clone();
        listed.push((page_map_id as i64, size));
        edit_page_map(&mut listed);
        let mut entries = Vec::new();
        for (number, len) in &listed {
            entries.extend(len.to_le_bytes());
            entries.extend(number.to_le_bytes());
        }
        let (mut page, stored, full) = system_page_2007(&entries, 3);
        if page.len() <= size as usize {
            page.resize(size as usize, 0);
            break (page, stored, full);
        }
        size = page.len() as i64;
    };
    file.extend(page_map);

    let mut header = vec![0u8; 0x110];
    for (at, value) in [
        (0x00, 0x70),
        (0x08, file.len() as u64 + 0x400),
        (0x18, 3),
        (0x38, page_map_at - 0x480),
        (0x40, page_map_id),
        (0x50, pm_stored),
        (0x58, pm_size),
        (0x60, pages.len() as u64 + 1),
        (0x68, page_map_id),
        (0x70, 0x20),
        (0x78, 0x40),
        (0xA0, sections.len() as u64 + 2),
        (0xB0, sm_stored),
        (0xC0, section_map_id),
        (0xC8, sm_size),
        (0xD8, 2),
        (0xE8, 0x60100),
    ] {
        header[at..at + 8].copy_from_slice(&u64::to_le_bytes(value));
    }
    let packed = compress_2007(&header);
    let (length, body) = if packed.len() < header.len() {
        (packed.len() as i32, packed)
    } else {
        (-(header.len() as i32), header)
    };
    let mut block = vec![0u8; 0x18];
    block.extend(length.to_le_bytes());
    block.extend(0u32.to_le_bytes());
    block.extend(body);
    block.resize(block.len().next_multiple_of(8), 0);
    let mut pre = Vec::new();
    loop {
        pre.extend(&block);
        if pre.len() + block.len() > 3 * 239 {
            break;
        }
    }
    pre.resize(3 * 239, 0);
    file[0x80..0x80 + 3 * 255].copy_from_slice(&interleave(&SYSTEM_FIELD, &pre, 239));
    // The file header is written at the end too.
    let copy = file[0x80..0x480].to_vec();
    file.extend(copy);
    file
}

/// Where the page map of a 2007 file the tests wrote starts: the last page, right before the
/// copy of the file header at the end, found as the place where a changed byte damages it.
fn page_map_2007(file: &[u8]) -> std::result::Result<usize, Box<dyn std::error::Error>> {
    let end = file.len() - 0x400;
    (end.saturating_sub(0x2000)..end)
        .step_by(0x20)
        .find(|&at| {
            let mut damaged = file.to_vec();
            damaged[at] ^= 0x01;
            error_of(&damaged).contains("page map is damaged")
        })
        .ok_or_else(|| "no page map".into())
}

// ---- AutoCAD 2007 ----

#[test]
fn reed_solomon_words_are_checked() {
    for field in [&SYSTEM_FIELD, &PAGE_FIELD] {
        // The generator reaches every non-zero byte once.
        let mut powers: Vec<u8> = (0..255).map(|i| field.power(i)).collect();
        powers.sort_unstable();
        powers.dedup();
        assert_eq!(powers.len(), 255);
    }
    let data = noise(251, 21);
    let word = rs_word(&SYSTEM_FIELD, &data[..239], 16);
    assert_eq!(&word[..239], &data[..239]);
    assert!(SYSTEM_FIELD.is_code_word(&word, 16));
    assert!(PAGE_FIELD.is_code_word(&rs_word(&PAGE_FIELD, &data, 4), 4));
    // Any one changed byte, data or check byte, is seen.
    for at in [0, 100, 238, 239, 254] {
        let mut damaged = word.clone();
        damaged[at] ^= 0x10;
        assert!(!SYSTEM_FIELD.is_code_word(&damaged, 16), "byte {at}");
    }
}

#[test]
fn a_hand_written_2007_stream_decompresses() -> TestResult {
    // Started by 0x20 and two bytes, a literal run of two (stored backwards), then a copy of
    // six bytes from one back.
    let stream = [0x20, 0x00, 0x00, 0x02, 0x00, 0x70, 0x60, 0x03];
    assert_eq!(r2007::decompress(&stream, 8)?, [0x70, 0, 0, 0, 0, 0, 0, 0]);
    // A literal run of 32 bytes (0x0F, then 9 more than 0x17) comes out in groups of eight
    // from the end.
    let mut stream = vec![0x0F, 9];
    stream.extend(0u8..32);
    let mut want: Vec<u8> = (24..32).collect();
    want.extend(16..24);
    want.extend(8..16);
    want.extend(0..8);
    assert_eq!(r2007::decompress(&stream, 32)?, want);
    Ok(())
}

#[test]
fn the_2007_compression_comes_back_the_same() -> TestResult {
    let mut long_run = b"start".to_vec();
    long_run.extend([b'z'; 70_000]);
    let mut far = noise(0x9000, 12);
    far.extend_from_within(10..400); // a long copy from over 0x8000 back
    far.extend(noise(30, 5));
    far.extend_from_within(0x100..0x10C); // a short copy from far back
    far.extend(noise(30, 6));
    far.extend_from_within(0x8E00..0x8E10); // 16 bytes from under 0x2000 back
    let cases: Vec<Vec<u8>> = vec![
        b"abc".to_vec(),
        b"abcabcabcabcabc".to_vec(),
        b"WiringCraft ".repeat(500),
        noise(9, 1),
        noise(20, 2),
        noise(300, 3),
        noise(70_000, 4), // a literal run past 0xFFFF
        long_run,
        far,
    ];
    for data in cases {
        let packed = compress_2007(&data);
        assert_eq!(r2007::decompress(&packed, data.len())?, data, "{} bytes", data.len());
    }
    Ok(())
}

#[test]
fn damaged_2007_compressed_data_gives_an_error() {
    // A copy from before the start.
    assert!(r2007::decompress(&[0x20, 0, 0, 0x01, b'a', 0x30, 0x08], 4).is_err());
    // Ends inside a literal run.
    assert!(r2007::decompress(&[0x02, b'a', b'b'], 10).is_err());
    // More bytes than the page holds.
    assert!(r2007::decompress(&[0x20, 0, 0, 0x01, b'a', 0x30, 0x00], 3).is_err());
    // Fewer.
    assert!(r2007::decompress(&[0x20, 0, 0, 0x01, b'a', 0x30, 0x00], 9).is_err());
    // A literal length made of many 0xFFFF parts is bounded by the page size.
    let mut bomb = vec![0x0F, 0xFF];
    bomb.extend([0xFF; 10_000]);
    bomb.extend([0x01, 0x00]);
    assert!(r2007::decompress(&bomb, 100).is_err());
    assert!(r2007::decompress(&[], 1).is_err());
}

#[test]
fn the_sections_of_a_2007_drawing_are_read() -> TestResult {
    let sections = sample_sections();
    let read = read_sections(&write_dwg_2007(&sections))?;
    assert_eq!(read.version, Version::R2007);
    let names: Vec<&str> = read.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["AcDb:Header", "AcDb:Classes", "AcDb:Handles", "AcDb:AcDbObjects"]
    );
    for section in sections.iter().filter(|s| !s.encrypted) {
        assert_eq!(
            read.get(section.name),
            Some(section.data.as_slice()),
            "{}",
            section.name
        );
    }
    assert!(sections[4].data.len() > 2 * MAX_PAGE_DATA, "objects span three pages");
    assert_eq!(read.skipped, ["AcDb:SummaryInfo"]);
    Ok(())
}

#[test]
fn stored_plain_and_left_out_2007_pages_are_read() -> TestResult {
    let mut zeros = noise(100, 1);
    zeros.extend(vec![0u8; MAX_PAGE_DATA - 100]);
    zeros.extend(vec![0u8; MAX_PAGE_DATA]); // a page of zeros, not written
    zeros.extend(noise(500, 2));
    let sections = [
        TestSection {
            name: "AcDb:Classes",
            data: noise(300, 3),
            stored: true,
            ..TestSection::default()
        },
        TestSection {
            name: "AcDb:AppInfo",
            data: b"AppInfo ".repeat(100),
            plain: true,
            ..TestSection::default()
        },
        TestSection {
            name: "AcDb:Preview",
            data: noise(MAX_PAGE_DATA + 50, 4),
            plain: true,
            stored: true,
            ..TestSection::default()
        },
        TestSection {
            name: "AcDb:AcDbObjects",
            data: zeros,
            zero_pages_left_out: true,
            ..TestSection::default()
        },
    ];
    let read = read_sections(&write_dwg_2007(&sections))?;
    for section in &sections {
        assert_eq!(
            read.get(section.name),
            Some(section.data.as_slice()),
            "{}",
            section.name
        );
    }
    Ok(())
}

#[test]
fn a_damaged_2007_header_or_map_is_reported() -> TestResult {
    let file = write_dwg_2007(&sample_sections()[..2]);
    assert!(read_sections(&file).is_ok());
    // Any byte of the file header's words, data or check byte.
    for at in [0x80, 0x80 + 100, 0x80 + 3 * 239, 0x80 + 3 * 255 - 1] {
        let mut damaged = file.clone();
        damaged[at] ^= 0x04;
        let m = error_of(&damaged);
        assert!(m.contains("file header is damaged"), "{at:#x}: {m}");
    }
    // The page map, and the section map before it.
    let at = page_map_2007(&file)?;
    let mut damaged = file.clone();
    damaged[at + 7] ^= 0x80;
    assert!(error_of(&damaged).contains("page map is damaged"));
    let mut damaged = file.clone();
    damaged[at - 0x40] ^= 0x80;
    let m = error_of(&damaged);
    assert!(m.contains("section map") && m.contains("damaged"), "{m}");
    Ok(())
}

#[test]
fn a_changed_byte_in_a_2007_page_is_caught_by_its_checksum() -> TestResult {
    for plain in [false, true] {
        let section = TestSection {
            name: "AcDb:Classes",
            data: noise(300, 3),
            stored: true,
            plain,
            ..TestSection::default()
        };
        let file = write_dwg_2007(&[section]);
        assert!(read_sections(&file).is_ok());
        // The first page is at 0x480.
        let mut damaged = file.clone();
        damaged[0x480 + 5] ^= 0x01;
        let m = error_of(&damaged);
        assert!(
            m.contains("AcDb:Classes") && m.contains("page 1") && m.contains("wrong checksum"),
            "{m}"
        );
    }
    Ok(())
}

#[test]
fn a_2007_page_claiming_more_than_its_page_is_refused() {
    let section = TestSection {
        name: "AcDb:Classes",
        data: noise(300, 3),
        stored: true,
        plain: true,
        claimed_extra: 0x10_0000,
        ..TestSection::default()
    };
    let m = error_of(&write_dwg_2007(&[section]));
    assert!(m.contains("AcDb:Classes") && m.contains("page 1"), "{m}");
}

#[test]
fn a_2007_page_listed_many_times_cannot_make_endless_work() -> TestResult {
    let section = |listed_again| TestSection {
        name: "AcDb:AcDbObjects",
        data: b"WiringCraft ".repeat(MAX_PAGE_DATA / 12),
        listed_again,
        ..TestSection::default()
    };
    let budget = 4 * MAX_PAGE_DATA + 0x1000;
    let read = read_sections_with(&write_dwg_2007(&[section(3)]), budget)?;
    assert_eq!(
        read.get("AcDb:AcDbObjects").map(<[u8]>::len),
        Some(section(0).data.len())
    );
    let m = read_sections_with(&write_dwg_2007(&[section(4)]), budget).map_err(|e| e.to_string());
    assert!(m.as_ref().is_err_and(|m| m.contains("decompress to too much")), "{m:?}");
    Ok(())
}

#[test]
fn cut_or_flipped_2007_files_give_errors_not_panics() -> TestResult {
    let file = write_dwg_2007(&sample_sections());
    assert!(read_sections(&file).is_ok());
    for len in (0..file.len()).step_by(7) {
        let _ = read_sections(&file[..len]);
    }
    // Cut into the page map: refused.
    let page_map = page_map_2007(&file)?;
    assert!(read_sections(&file[..page_map + 0x100]).is_err());
    let mut seed = 3u32;
    for i in 0..1500 {
        let mut damaged = file.clone();
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        let at = (seed as usize >> 4) % damaged.len();
        damaged[at] ^= 1 << (i % 8);
        let _ = read_sections(&damaged);
    }
    // Every byte of the file header's words and of the maps, each set to 0xFF.
    let maps = page_map.saturating_sub(0x1000)..file.len() - 0x400;
    for at in (0x80..0x80 + 3 * 255).chain(maps) {
        let mut damaged = file.clone();
        damaged[at] = 0xFF;
        let _ = read_sections(&damaged);
    }
    Ok(())
}

#[test]
fn hostile_2007_maps_that_are_good_code_words_give_errors() {
    let sections = || sample_sections().into_iter().take(2).collect::<Vec<_>>();
    let message = |section_map: &dyn Fn(&mut Vec<u8>), page_map: &dyn Fn(&mut Vec<PageEntry2007>)| {
        error_of(&write_dwg_2007_with(&sections(), section_map, page_map))
    };
    let no_map = |_: &mut Vec<(i64, i64)>| {};
    let no_sections = |_: &mut Vec<u8>| {};
    // The first section's fields: page count at 0x38, encoding at 0x30, name length at 0x20;
    // its name (AcDb:Header) takes 24 bytes, then the first page's number at 0x10.
    let set = |at: usize, value: u64| move |m: &mut Vec<u8>| m[at..at + 8].copy_from_slice(&value.to_le_bytes());
    for (what, at, value, want) in [
        ("a page count near the largest number", 0x38, u64::MAX / 0x38, "cut off"),
        ("a page count past the map", 0x38, 1000, "cut off"),
        ("an unknown encoding", 0x30, 3, "unknown encoding 3"),
        (
            "a name length past the map",
            0x20,
            0x1000,
            "name in the section map is cut off",
        ),
        (
            "a page not in the page map",
            0x40 + 24 + 0x10,
            999,
            "page 999, which is not in the page map",
        ),
    ] {
        let m = message(&set(at, value), &no_map);
        assert!(m.contains(want), "{what}: {m}");
    }
    // The page map: a page of size 0 or less, a page listed twice, and a section map page too
    // small for it.
    let m = message(&no_sections, &|p: &mut Vec<(i64, i64)>| p[0].1 = -0x20);
    assert!(m.contains("has size -32"), "{m}");
    let m = message(&no_sections, &|p: &mut Vec<(i64, i64)>| p[1].0 = p[0].0);
    assert!(m.contains("listed twice"), "{m}");
    let m = message(&no_sections, &|p: &mut Vec<(i64, i64)>| {
        let n = p.len();
        p[n - 2].1 = 0x20;
    });
    assert!(m.contains("section map") && m.contains("reaches past its page"), "{m}");
}

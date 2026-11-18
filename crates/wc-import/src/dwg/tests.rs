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
    let mut r2007 = write_dwg(b"AC1024", &sample_sections()[..1]);
    r2007[..6].copy_from_slice(b"AC1021");
    assert!(message(&r2007).contains("AutoCAD 2007"), "{}", message(&r2007));
    assert!(message(b"AC1015 and more").contains("AC1015"));
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

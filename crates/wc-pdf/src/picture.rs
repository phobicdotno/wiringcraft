//! Pictures (underlays such as scanned sheets, and the project logo) as PDF image objects: JPEG
//! files are embedded as they are, PNG files are decoded and stored Flate-compressed.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;

use wc_core::{Project, Underlay};

use crate::writer::PageImage;

/// Largest picture embedded, in pixels; bigger ones are skipped with a warning.
const MAX_PIXELS: u64 = 200_000_000;

/// Every picture the project uses, by file: the pages' underlays and the project logo. Files
/// that are missing are left out (the export then says so); files that cannot be read, or that
/// lead out of the project folder through a link, are listed with the reason instead.
pub fn read_pictures(dir: &Path, project: &Project) -> Pictures {
    let underlays = project
        .pages
        .iter()
        .filter_map(|p| p.underlay.as_ref())
        .map(|u| u.file.as_str());
    let logo = project.meta.logo.as_ref().map(|l| l.file.as_str());
    let mut out = Pictures::default();
    for f in underlays.chain(logo) {
        if out.files.contains_key(f) || out.unreadable.contains_key(f) {
            continue;
        }
        match read_picture(dir, f) {
            Ok(Some(bytes)) => {
                out.files.insert(f.to_owned(), bytes);
            }
            Ok(None) => {}
            Err(why) => {
                out.unreadable.insert(f.to_owned(), why);
            }
        }
    }
    out
}

/// Picture files read for an export (see [`read_pictures`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pictures {
    /// The bytes of each picture that could be read, by project path.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Why each picture that exists could not be used, by project path.
    pub unreadable: BTreeMap<String, String>,
}

/// Read the picture `file` (a path relative to the project folder `dir`). `Ok(None)` when it
/// does not exist. Refused when the path is not a plain relative one, or when it leads out of
/// the project folder through a link.
pub fn read_picture(dir: &Path, file: &str) -> Result<Option<Vec<u8>>, String> {
    if !wc_core::project::relative_file_is_safe(file) {
        return Err("the path is not inside the project folder".into());
    }
    let path = dir.join(file);
    let real = match path.canonicalize() {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let base = dir.canonicalize().map_err(|e| e.to_string())?;
    if !real.starts_with(&base) {
        return Err("it links to a file outside the project folder".into());
    }
    std::fs::read(&real).map(Some).map_err(|e| e.to_string())
}

/// A picture file as an image object placed over the underlay's rectangle.
pub fn page_image(u: &Underlay, bytes: &[u8]) -> Result<PageImage, String> {
    let mut img = decode(bytes)?;
    img.min = (u.min.x.min(u.max.x), u.min.y.min(u.max.y));
    img.max = (u.min.x.max(u.max.x), u.min.y.max(u.max.y));
    Ok(img)
}

/// A picture file as an image object, not yet placed (`min` and `max` are zero).
pub fn decode(bytes: &[u8]) -> Result<PageImage, String> {
    let place = |width: u32, height: u32, colour_space, filter, extra, data| PageImage {
        width,
        height,
        colour_space,
        filter,
        extra,
        data,
        alpha: None,
        min: (0.0, 0.0),
        max: (0.0, 0.0),
    };
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let j = jpeg_info(bytes)
            .filter(|_| has_scan_data(bytes))
            .ok_or("the JPEG file is damaged")?;
        let colour_space = match j.components {
            1 => "DeviceGray",
            3 => "DeviceRGB",
            4 => "DeviceCMYK",
            n => return Err(format!("JPEG pictures with {n} colour channels are not supported")),
        };
        // Adobe writes CMYK JPEGs inverted.
        let extra = if j.components == 4 && j.adobe {
            " /Decode [1 0 1 0 1 0 1 0]"
        } else {
            ""
        };
        return Ok(place(
            j.width,
            j.height,
            colour_space,
            "DCTDecode",
            extra,
            bytes.to_vec(),
        ));
    }
    if bytes.starts_with(b"\x89PNG") {
        let mut reader = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(MAX_PIXELS * 4);
        reader.limits(limits);
        let img = reader
            .decode()
            .map_err(|e| format!("the PNG file cannot be read: {e}"))?;
        let (w, h) = (img.width(), img.height());
        let grey = matches!(
            img.color(),
            image::ColorType::L8 | image::ColorType::L16 | image::ColorType::La8
        );
        let (colour_space, raw) = if grey {
            ("DeviceGray", img.to_luma8().into_raw())
        } else {
            ("DeviceRGB", img.to_rgb8().into_raw())
        };
        // Transparent pixels (a logo's background) stay transparent through a soft mask.
        let alpha = img.color().has_alpha().then(|| img.to_rgba8()).and_then(|rgba| {
            let a: Vec<u8> = rgba.pixels().map(|p| p.0[3]).collect();
            a.iter()
                .any(|v| *v != 255)
                .then(|| miniz_oxide::deflate::compress_to_vec_zlib(&a, 6))
        });
        let data = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
        let mut out = place(w, h, colour_space, "FlateDecode", "", data);
        out.alpha = alpha;
        return Ok(out);
    }
    Err("only JPEG and PNG pictures can be shown".into())
}

/// Whether a JPEG has its scan (a start-of-scan marker) and ends with an end-of-image marker,
/// so a file cut short after its header is not embedded as if it were a picture.
fn has_scan_data(b: &[u8]) -> bool {
    let sos = b.windows(2).position(|w| w == [0xFF, 0xDA]);
    let eoi = b.windows(2).rposition(|w| w == [0xFF, 0xD9]);
    matches!((sos, eoi), (Some(s), Some(e)) if e > s + 2)
}

struct JpegInfo {
    width: u32,
    height: u32,
    components: u8,
    adobe: bool,
}

/// Size and channel count from a JPEG's frame header, and whether it carries an Adobe marker.
fn jpeg_info(b: &[u8]) -> Option<JpegInfo> {
    let mut i = 2;
    let mut adobe = false;
    while i + 4 <= b.len() {
        if *b.get(i)? != 0xFF {
            return None;
        }
        let marker = *b.get(i + 1)?;
        if marker == 0xFF {
            i += 1;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([*b.get(i + 2)?, *b.get(i + 3)?]));
        let seg = b.get(i + 4..i + 2 + len)?;
        if marker == 0xEE && seg.starts_with(b"Adobe") {
            adobe = true;
        }
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let height = u32::from(u16::from_be_bytes([*seg.get(1)?, *seg.get(2)?]));
            let width = u32::from(u16::from_be_bytes([*seg.get(3)?, *seg.get(4)?]));
            let components = *seg.get(5)?;
            if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
                return None;
            }
            return Some(JpegInfo {
                width,
                height,
                components,
                adobe,
            });
        }
        i += 2 + len;
    }
    None
}

//! Pictures (underlays such as scanned sheets, and the project logo) as PDF image objects: JPEG
//! files are embedded as they are, PNG files are decoded and stored Flate-compressed.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;

use wc_core::{Project, Underlay};

use crate::writer::PageImage;

/// Largest picture embedded, in pixels; bigger ones are skipped with a warning.
const MAX_PIXELS: u64 = 200_000_000;

/// Read every picture the project uses, by file: the pages' underlays and the project logo.
/// Files that are missing or outside the project are left out; the export then warns about them.
pub fn read_underlays(dir: &Path, project: &Project) -> BTreeMap<String, Vec<u8>> {
    let underlays = project
        .pages
        .iter()
        .filter_map(|p| p.underlay.as_ref())
        .filter(|u| u.file_is_safe())
        .map(|u| u.file.as_str());
    let logo = project
        .meta
        .logo
        .as_ref()
        .filter(|l| l.file_is_safe())
        .map(|l| l.file.as_str());
    underlays
        .chain(logo)
        .filter_map(|f| Some((f.to_owned(), std::fs::read(dir.join(f)).ok()?)))
        .collect()
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
        min: (0.0, 0.0),
        max: (0.0, 0.0),
    };
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let j = jpeg_info(bytes).ok_or("the JPEG file is damaged")?;
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
        let data = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
        return Ok(place(w, h, colour_space, "FlateDecode", "", data));
    }
    Err("only JPEG and PNG pictures can be shown".into())
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

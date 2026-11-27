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
    let (min, max) = underlay_rect(u)?;
    let mut img = decode(bytes)?;
    img.min = min;
    img.max = max;
    Ok(img)
}

/// A corner of a rectangle on the page, in millimetres.
pub(crate) type Corner = (f64, f64);

/// The rectangle an underlay covers (top-left and bottom-right, in millimetres), whichever
/// way round its corners are stored. Refused when a corner is not a finite number: `nan` and
/// `inf` are valid TOML numbers, so a hand-edited project.toml can hold them, and a PDF reader
/// refuses a page that places a picture at `NaN`.
pub(crate) fn underlay_rect(u: &Underlay) -> Result<(Corner, Corner), String> {
    if ![u.min.x, u.min.y, u.max.x, u.max.y].iter().all(|v| v.is_finite()) {
        return Err("a corner of its rectangle is not a number, so it is left out".into());
    }
    Ok((
        (u.min.x.min(u.max.x), u.min.y.min(u.max.y)),
        (u.min.x.max(u.max.x), u.min.y.max(u.max.y)),
    ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use wc_core::Page;
    use wc_core::geom::Point;

    fn png(img: image::DynamicImage) -> Vec<u8> {
        let mut out = Vec::new();
        assert!(
            img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
                .is_ok()
        );
        out
    }

    /// SOI, then `segments`, then a scan header, one scan byte and EOI.
    fn jpeg_with(segments: &[u8]) -> Vec<u8> {
        let mut b = vec![0xFF, 0xD8];
        b.extend(segments);
        b.extend([
            0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0x00, 0xFF, 0xD9,
        ]);
        b
    }

    /// A one-component frame header with marker `0xFF marker`, `width` by `height`.
    fn frame(marker: u8, width: u16, height: u16) -> Vec<u8> {
        let mut b = vec![0xFF, marker, 0x00, 0x0B, 0x08];
        b.extend(height.to_be_bytes());
        b.extend(width.to_be_bytes());
        b.extend([0x01, 0x01, 0x11, 0x00]);
        b
    }

    #[test]
    fn an_underlay_stored_corners_reversed_covers_the_same_rectangle() {
        let u = Underlay {
            file: "u.png".into(),
            min: Point::new(400.0, 10.0),
            max: Point::new(20.0, 290.0),
        };
        assert_eq!(underlay_rect(&u), Ok(((20.0, 10.0), (400.0, 290.0))));
        let bytes = png(image::DynamicImage::ImageRgb8(image::RgbImage::new(3, 2)));
        let Ok(img) = page_image(&u, &bytes) else {
            unreachable!("the picture is placed");
        };
        assert_eq!((img.min, img.max), ((20.0, 10.0), (400.0, 290.0)));
        assert_eq!((img.width, img.height), (3, 2));
    }

    #[test]
    fn a_bad_corner_is_refused_before_the_picture_is_decoded() {
        let u = Underlay {
            file: "u.png".into(),
            min: Point::new(0.0, 0.0),
            max: Point::new(f64::NAN, 10.0),
        };
        let Err(why) = page_image(&u, b"not a picture") else {
            unreachable!("a NaN corner was accepted");
        };
        assert!(why.contains("not a number"), "{why}");
    }

    #[test]
    fn pictures_that_are_neither_jpeg_nor_png_are_refused() {
        for bytes in [&b""[..], b"GIF89a", b"\xFF", b"\x89PN"] {
            let Err(why) = decode(bytes) else {
                unreachable!("{bytes:?} was accepted");
            };
            assert_eq!(why, "only JPEG and PNG pictures can be shown", "{bytes:?}");
        }
        let Err(why) = decode(b"\x89PNG\r\n\x1a\nbroken") else {
            unreachable!("a broken PNG was accepted");
        };
        assert!(why.starts_with("the PNG file cannot be read"), "{why}");
    }

    #[test]
    fn png_pictures_keep_their_colour_space_and_samples() {
        let grey = png(image::DynamicImage::ImageLuma8(
            image::GrayImage::from_raw(2, 1, vec![7, 9]).unwrap_or_default(),
        ));
        let Ok(img) = decode(&grey) else {
            unreachable!("grey PNG not decoded");
        };
        assert_eq!(
            (img.colour_space, img.filter, img.extra),
            ("DeviceGray", "FlateDecode", "")
        );
        assert_eq!(
            miniz_oxide::inflate::decompress_to_vec_zlib(&img.data).ok(),
            Some(vec![7, 9])
        );
        assert!(img.alpha.is_none());

        let rgb = png(image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            1,
            1,
            image::Rgb([1, 2, 3]),
        )));
        let Ok(img) = decode(&rgb) else {
            unreachable!("RGB PNG not decoded");
        };
        assert_eq!(img.colour_space, "DeviceRGB");
        assert_eq!(
            miniz_oxide::inflate::decompress_to_vec_zlib(&img.data).ok(),
            Some(vec![1, 2, 3])
        );
        assert_eq!((img.min, img.max), ((0.0, 0.0), (0.0, 0.0)), "not yet placed");
    }

    #[test]
    fn an_opaque_png_with_an_alpha_channel_gets_no_soft_mask() {
        let rgba = png(image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([5, 6, 7, 255]),
        )));
        let Ok(img) = decode(&rgba) else {
            unreachable!("RGBA PNG not decoded");
        };
        assert_eq!(img.colour_space, "DeviceRGB");
        assert!(img.alpha.is_none(), "every pixel is opaque");
        // A grey picture with a transparent pixel keeps grey samples and gets a mask.
        let la = png(image::DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_pixel(
            1,
            1,
            image::LumaA([50, 0]),
        )));
        let Ok(img) = decode(&la) else {
            unreachable!("grey-alpha PNG not decoded");
        };
        assert_eq!(img.colour_space, "DeviceGray");
        let Some(mask) = img.alpha else {
            unreachable!("the transparent pixel has no mask");
        };
        assert_eq!(miniz_oxide::inflate::decompress_to_vec_zlib(&mask).ok(), Some(vec![0]));
    }

    #[test]
    fn jpeg_frame_headers_are_found_past_padding_and_other_segments() {
        // Fill bytes before a marker, a Huffman table (0xC4, not a frame) and a progressive frame.
        let mut segs = vec![0xFF, 0xFF];
        segs.extend([0xFF, 0xC4, 0x00, 0x03, 0x00]);
        segs.extend(frame(0xC2, 640, 480));
        let Ok(img) = decode(&jpeg_with(&segs)) else {
            unreachable!("progressive JPEG not decoded");
        };
        assert_eq!((img.width, img.height, img.colour_space), (640, 480, "DeviceGray"));
    }

    #[test]
    fn jpegs_with_no_size_or_too_many_pixels_or_bad_markers_are_damaged() {
        for (what, segs) in [
            ("zero width", frame(0xC0, 0, 10)),
            ("zero height", frame(0xC0, 10, 0)),
            ("no frame header", vec![0xFF, 0xE0, 0x00, 0x02]),
            ("not a marker", vec![0x00, 0xC0, 0x00, 0x0B]),
            ("segment runs past the end", vec![0xFF, 0xC0, 0xFF, 0xF0]),
        ] {
            let Err(why) = decode(&jpeg_with(&segs)) else {
                unreachable!("{what}: accepted");
            };
            assert_eq!(why, "the JPEG file is damaged", "{what}");
        }
        // 65535 x 65535 is over the pixel limit.
        assert!(jpeg_info(&jpeg_with(&frame(0xC0, u16::MAX, u16::MAX))).is_none());
        assert!(jpeg_info(&jpeg_with(&frame(0xC0, 10_000, 10_000))).is_some());
    }

    #[test]
    fn a_jpeg_whose_end_marker_comes_before_its_scan_is_damaged() {
        let mut b = vec![0xFF, 0xD8];
        b.extend(frame(0xC0, 4, 4));
        b.extend([0xFF, 0xD9, 0xFF, 0xDA]);
        assert!(!has_scan_data(&b));
        assert_eq!(decode(&b).err().as_deref(), Some("the JPEG file is damaged"));
    }

    #[test]
    fn a_file_used_by_several_pages_and_the_logo_is_read_once_and_missing_ones_are_left_out() {
        let Ok(tmp) = tempfile::tempdir() else {
            unreachable!("no temp dir");
        };
        assert!(std::fs::create_dir_all(tmp.path().join("pictures")).is_ok());
        assert!(std::fs::write(tmp.path().join("pictures/a.png"), b"a").is_ok());
        let mut project = Project::new("x");
        project.pages = ["a", "a", "gone"]
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let mut p = Page::new(format!("p{i}"), i.to_string(), "x");
                p.underlay = Some(Underlay {
                    file: format!("pictures/{f}.png"),
                    min: Point::new(0.0, 0.0),
                    max: Point::new(1.0, 1.0),
                });
                p
            })
            .chain(std::iter::once(Page::new("plain", "9", "no underlay")))
            .collect();
        project.meta.logo = Some(wc_core::Logo {
            file: "pictures/a.png".into(),
            cover: None,
        });
        let pictures = read_pictures(tmp.path(), &project);
        assert_eq!(pictures.files.len(), 1);
        assert_eq!(pictures.files.get("pictures/a.png"), Some(&b"a".to_vec()));
        assert!(pictures.unreadable.is_empty(), "{:?}", pictures.unreadable);
    }

    #[test]
    fn an_absolute_path_is_refused() {
        let Ok(tmp) = tempfile::tempdir() else {
            unreachable!("no temp dir");
        };
        let file = tmp.path().join("x.png");
        assert!(std::fs::write(&file, b"x").is_ok());
        let Some(abs) = file.to_str() else {
            unreachable!("temp path is not UTF-8");
        };
        assert_eq!(
            read_picture(tmp.path(), abs),
            Err("the path is not inside the project folder".into())
        );
    }
}

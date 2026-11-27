//! Raster pictures for page underlays: raw PDF image samples to PNG, and turning a picture so
//! its rows and columns run the way the page is displayed.

use std::io::Cursor;

use image::{DynamicImage, GrayImage, ImageFormat, RgbImage};

/// Largest picture accepted, in pixels (a 600 dpi A0 scan is about 650 million; scans of
/// drawing sheets are far smaller). Bigger pictures are skipped rather than eating memory.
pub const MAX_PIXELS: u64 = 200_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RasterFormat {
    Jpeg,
    Png,
}

impl RasterFormat {
    pub fn extension(self) -> &'static str {
        match self {
            RasterFormat::Jpeg => "jpg",
            RasterFormat::Png => "png",
        }
    }

    /// The format of a file from its first bytes.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(&[0xFF, 0xD8]) {
            Some(RasterFormat::Jpeg)
        } else if bytes.starts_with(b"\x89PNG") {
            Some(RasterFormat::Png)
        } else {
            None
        }
    }

    fn image_format(self) -> ImageFormat {
        match self {
            RasterFormat::Jpeg => ImageFormat::Jpeg,
            RasterFormat::Png => ImageFormat::Png,
        }
    }
}

/// An encoded picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raster {
    pub format: RasterFormat,
    pub bytes: Vec<u8>,
}

/// How a picture's pixels lie on the page: whether columns run down the page instead of
/// across (`transposed`), and whether columns run right-to-left and rows bottom-to-top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Orientation {
    pub transposed: bool,
    pub flip_x: bool,
    pub flip_y: bool,
}

/// Uncompressed samples as PNG: 8 or 16 bits per sample, or 1 bit for black and white scans.
/// `channels` is 1 (grey), 3 (RGB) or 4 (CMYK, turned into RGB); 16-bit samples (big-endian,
/// as PDF stores them) keep their high byte. `invert` swaps black and white, as a PDF
/// `/Decode [1 0]` asks.
pub fn samples_to_png(width: u32, height: u32, channels: u8, bits: u8, invert: bool, data: &[u8]) -> Option<Raster> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    if bits == 16 && matches!(channels, 1 | 3) {
        let need = w.checked_mul(h)?.checked_mul(usize::from(channels))?.checked_mul(2)?;
        let high: Vec<u8> = data.get(..need)?.iter().step_by(2).copied().collect();
        return samples_to_png(width, height, channels, 8, invert, &high);
    }
    let img = match (channels, bits) {
        (1, 8) => {
            let need = w.checked_mul(h)?;
            let mut px = data.get(..need)?.to_vec();
            if invert {
                px.iter_mut().for_each(|v| *v = 255 - *v);
            }
            DynamicImage::ImageLuma8(GrayImage::from_raw(width, height, px)?)
        }
        (1, 1) => {
            // Rows are padded to whole bytes.
            let stride = w.div_ceil(8);
            let mut px = Vec::with_capacity(w.checked_mul(h)?);
            for row in 0..h {
                let line = data.get(row * stride..(row + 1) * stride)?;
                for x in 0..w {
                    let bit = (line.get(x / 8)? >> (7 - (x % 8))) & 1;
                    px.push(if (bit == 1) != invert { 255 } else { 0 });
                }
            }
            DynamicImage::ImageLuma8(GrayImage::from_raw(width, height, px)?)
        }
        (3, 8) => {
            let need = w.checked_mul(h)?.checked_mul(3)?;
            let mut px = data.get(..need)?.to_vec();
            if invert {
                px.iter_mut().for_each(|v| *v = 255 - *v);
            }
            DynamicImage::ImageRgb8(RgbImage::from_raw(width, height, px)?)
        }
        (4, 8) => {
            let need = w.checked_mul(h)?.checked_mul(4)?;
            let px: Vec<u8> = data
                .get(..need)?
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|cmyk| {
                    let ink = |i: usize| {
                        let v = u16::from(cmyk.get(i).copied().unwrap_or(0));
                        if invert { 255 - v } else { v }
                    };
                    let k = 255 - ink(3);
                    [0, 1, 2].map(|i| ((255 - ink(i)) * k / 255) as u8)
                })
                .collect();
            DynamicImage::ImageRgb8(RgbImage::from_raw(width, height, px)?)
        }
        _ => return None,
    };
    encode(&img, RasterFormat::Png)
}

/// Turn a picture so its columns run left to right and its rows top to bottom on the page.
/// An upright picture comes back as it is, byte for byte.
pub fn upright(raster: Raster, o: Orientation) -> Option<Raster> {
    if o == Orientation::default() {
        return Some(raster);
    }
    let img = decode(&raster)?;
    let (w, h) = (img.width(), img.height());
    let (ow, oh) = if o.transposed { (h, w) } else { (w, h) };
    // Output pixel (u, v) shows the source pixel at the same place on the page.
    let source = |u: u32, v: u32| -> (u32, u32) {
        let (along_x, along_y) = (
            if o.flip_x { ow - 1 - u } else { u },
            if o.flip_y { oh - 1 - v } else { v },
        );
        if o.transposed {
            (along_y, along_x)
        } else {
            (along_x, along_y)
        }
    };
    let out = match img {
        DynamicImage::ImageLuma8(g) => DynamicImage::ImageLuma8(GrayImage::from_fn(ow, oh, |u, v| {
            let (x, y) = source(u, v);
            *g.get_pixel(x.min(w - 1), y.min(h - 1))
        })),
        other => {
            let rgb = other.to_rgb8();
            DynamicImage::ImageRgb8(RgbImage::from_fn(ow, oh, |u, v| {
                let (x, y) = source(u, v);
                *rgb.get_pixel(x.min(w - 1), y.min(h - 1))
            }))
        }
    };
    encode(&out, raster.format)
}

/// Decode a picture, refusing ones over [`MAX_PIXELS`].
pub fn decode(raster: &Raster) -> Option<DynamicImage> {
    let mut reader = image::ImageReader::with_format(Cursor::new(&raster.bytes), raster.format.image_format());
    reader.limits(limits_for_decode());
    let (w, h) = reader.into_dimensions().ok()?;
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return None;
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(&raster.bytes), raster.format.image_format());
    reader.limits(limits_for_decode());
    reader.decode().ok()
}

fn limits_for_decode() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_PIXELS * 4);
    limits
}

fn encode(img: &DynamicImage, format: RasterFormat) -> Option<Raster> {
    let mut bytes = Vec::new();
    match format {
        RasterFormat::Png => img.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png).ok()?,
        RasterFormat::Jpeg => {
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90);
            img.write_with_encoder(enc).ok()?;
        }
    }
    Some(Raster { format, bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels(r: &Raster) -> Option<(u32, u32, Vec<u8>)> {
        let img = decode(r)?.to_luma8();
        Some((img.width(), img.height(), img.into_raw()))
    }

    #[test]
    fn one_bit_samples_and_turning() {
        // 3 x 2 picture, 1 bit: top row black white black, bottom row white white black.
        let png = samples_to_png(3, 2, 1, 1, false, &[0b0100_0000, 0b1100_0000]);
        assert_eq!(
            png.as_ref().and_then(pixels),
            Some((3, 2, vec![0, 255, 0, 255, 255, 0]))
        );
        let Some(png) = png else { return };
        // Upside down on the page: the bottom row is shown first.
        let flipped = upright(
            png.clone(),
            Orientation {
                flip_y: true,
                ..Default::default()
            },
        );
        assert_eq!(
            flipped.as_ref().and_then(pixels),
            Some((3, 2, vec![255, 255, 0, 0, 255, 0]))
        );
        // Turned a quarter: columns run down the page, so the result is 2 wide and 3 high.
        let turned = upright(
            png.clone(),
            Orientation {
                transposed: true,
                ..Default::default()
            },
        );
        assert_eq!(
            turned.as_ref().and_then(pixels),
            Some((2, 3, vec![0, 255, 255, 255, 0, 0]))
        );
        assert_eq!(upright(png.clone(), Orientation::default()), Some(png));
        // Short or impossible data never panics.
        assert!(samples_to_png(3, 2, 1, 1, false, &[0]).is_none());
        assert!(samples_to_png(0, 2, 1, 8, false, &[]).is_none());
        assert!(samples_to_png(u32::MAX, u32::MAX, 3, 8, false, &[]).is_none());
        assert!(samples_to_png(2, 2, 2, 8, false, &[0; 8]).is_none());
        assert!(samples_to_png(2, 2, 4, 8, false, &[0; 15]).is_none(), "short CMYK");
        assert!(samples_to_png(2, 1, 3, 16, false, &[0; 11]).is_none(), "short 16-bit");
        assert!(
            decode(&Raster {
                format: RasterFormat::Jpeg,
                bytes: vec![0xFF, 0xD8, 0, 1]
            })
            .is_none()
        );
    }

    fn grey(w: u32, h: u32, px: &[u8]) -> Option<Raster> {
        samples_to_png(w, h, 1, 8, false, px)
    }

    fn rgb_pixels(r: &Raster) -> Option<(u32, u32, Vec<u8>)> {
        let img = decode(r)?.to_rgb8();
        Some((img.width(), img.height(), img.into_raw()))
    }

    #[test]
    fn sniff_and_extension() {
        assert_eq!(RasterFormat::sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(RasterFormat::Jpeg));
        assert_eq!(RasterFormat::sniff(b"\x89PNG\r\n\x1a\n"), Some(RasterFormat::Png));
        assert_eq!(RasterFormat::sniff(b"GIF89a"), None);
        assert_eq!(RasterFormat::sniff(&[0xFF]), None, "one byte is not enough for JPEG");
        assert_eq!(RasterFormat::sniff(&[]), None);
        assert_eq!(RasterFormat::Jpeg.extension(), "jpg");
        assert_eq!(RasterFormat::Png.extension(), "png");
        // What samples_to_png makes is recognised as PNG.
        let png = grey(1, 1, &[7]);
        assert_eq!(
            png.as_ref().and_then(|r| RasterFormat::sniff(&r.bytes)),
            Some(RasterFormat::Png)
        );
        assert_eq!(png.map(|r| r.format), Some(RasterFormat::Png));
    }

    #[test]
    fn grey_samples_invert_and_ignore_extra_data() {
        let plain = grey(2, 1, &[0, 200, 99]);
        assert_eq!(plain.as_ref().and_then(pixels), Some((2, 1, vec![0, 200])));
        let inverted = samples_to_png(2, 1, 1, 8, true, &[0, 200]);
        assert_eq!(inverted.as_ref().and_then(pixels), Some((2, 1, vec![255, 55])));
    }

    #[test]
    fn one_bit_rows_are_padded_and_invert_swaps_black_and_white() {
        // 10 wide needs 2 bytes a row; the padding bits of each row are ignored.
        let rows = [0b1000_0000, 0b0011_1111, 0b0000_0000, 0b0100_0000];
        let expect_row0 = [255, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let expect_row1 = [0, 0, 0, 0, 0, 0, 0, 0, 0, 255];
        let png = samples_to_png(10, 2, 1, 1, false, &rows);
        assert_eq!(
            png.as_ref().and_then(pixels),
            Some((10, 2, [expect_row0, expect_row1].concat()))
        );
        let inverted = samples_to_png(10, 2, 1, 1, true, &rows);
        let flip = |row: [u8; 10]| row.map(|v| 255 - v);
        assert_eq!(
            inverted.as_ref().and_then(pixels),
            Some((10, 2, [flip(expect_row0), flip(expect_row1)].concat()))
        );
        // One byte short of the second padded row.
        assert!(samples_to_png(10, 2, 1, 1, false, &rows[..3]).is_none());
    }

    #[test]
    fn rgb_and_cmyk_samples() {
        let rgb = samples_to_png(2, 1, 3, 8, false, &[255, 0, 0, 10, 20, 30]);
        assert_eq!(
            rgb.as_ref().and_then(rgb_pixels),
            Some((2, 1, vec![255, 0, 0, 10, 20, 30]))
        );
        let rgb_inv = samples_to_png(1, 1, 3, 8, true, &[255, 0, 10]);
        assert_eq!(rgb_inv.as_ref().and_then(rgb_pixels), Some((1, 1, vec![0, 255, 245])));
        // CMYK: no ink is white, full black is black, full cyan is (0, 255, 255),
        // and half black halves the remaining colour.
        let cmyk = samples_to_png(
            4,
            1,
            4,
            8,
            false,
            &[0, 0, 0, 0, 0, 0, 0, 255, 255, 0, 0, 0, 0, 255, 0, 255 - 128],
        );
        assert_eq!(
            cmyk.as_ref().and_then(rgb_pixels),
            Some((4, 1, vec![255, 255, 255, 0, 0, 0, 0, 255, 255, 128, 0, 128]))
        );
        // Inverted CMYK: all-255 samples mean no ink at all.
        let cmyk_inv = samples_to_png(1, 1, 4, 8, true, &[255, 255, 255, 255]);
        assert_eq!(
            cmyk_inv.as_ref().and_then(rgb_pixels),
            Some((1, 1, vec![255, 255, 255]))
        );
    }

    #[test]
    fn sixteen_bit_samples_keep_the_high_byte() {
        let grey16 = samples_to_png(2, 1, 1, 16, false, &[0x12, 0xFF, 0xAB, 0x00]);
        assert_eq!(grey16.as_ref().and_then(pixels), Some((2, 1, vec![0x12, 0xAB])));
        let rgb16 = samples_to_png(1, 1, 3, 16, true, &[0x10, 1, 0x20, 2, 0x30, 3]);
        assert_eq!(
            rgb16.as_ref().and_then(rgb_pixels),
            Some((1, 1, vec![0xEF, 0xDF, 0xCF]))
        );
        // 16-bit CMYK and other bit depths are not handled.
        assert!(samples_to_png(1, 1, 4, 16, false, &[0; 8]).is_none());
        assert!(samples_to_png(1, 1, 1, 4, false, &[0; 1]).is_none());
        assert!(samples_to_png(1, 1, 3, 1, false, &[0; 1]).is_none());
    }

    #[test]
    fn upright_flip_x_and_quarter_turn_with_both_flips() {
        // 3 x 2: rows [10 20 30] and [40 50 60].
        let Some(png) = grey(3, 2, &[10, 20, 30, 40, 50, 60]) else {
            unreachable!()
        };
        let mirrored = upright(
            png.clone(),
            Orientation {
                flip_x: true,
                ..Default::default()
            },
        );
        assert_eq!(
            mirrored.as_ref().and_then(pixels),
            Some((3, 2, vec![30, 20, 10, 60, 50, 40]))
        );
        let both = upright(
            png.clone(),
            Orientation {
                flip_x: true,
                flip_y: true,
                ..Default::default()
            },
        );
        assert_eq!(
            both.as_ref().and_then(pixels),
            Some((3, 2, vec![60, 50, 40, 30, 20, 10]))
        );
        let turned = upright(
            png,
            Orientation {
                transposed: true,
                flip_x: true,
                flip_y: true,
            },
        );
        assert_eq!(
            turned.as_ref().and_then(pixels),
            Some((2, 3, vec![60, 30, 50, 20, 40, 10]))
        );
    }

    #[test]
    fn upright_keeps_colour_and_format() {
        let Some(png) = samples_to_png(2, 1, 3, 8, false, &[255, 0, 0, 0, 0, 255]) else {
            unreachable!()
        };
        let mirrored = upright(
            png,
            Orientation {
                flip_x: true,
                ..Default::default()
            },
        );
        assert_eq!(mirrored.as_ref().map(|r| r.format), Some(RasterFormat::Png));
        assert_eq!(
            mirrored.as_ref().and_then(rgb_pixels),
            Some((2, 1, vec![0, 0, 255, 255, 0, 0]))
        );

        // A JPEG stays a JPEG and takes the turned size.
        let Some(img) = grey(4, 2, &[0; 8]).as_ref().and_then(decode) else {
            unreachable!()
        };
        let Some(jpeg) = encode(&img, RasterFormat::Jpeg) else {
            unreachable!()
        };
        assert_eq!(RasterFormat::sniff(&jpeg.bytes), Some(RasterFormat::Jpeg));
        let turned = upright(
            jpeg,
            Orientation {
                transposed: true,
                ..Default::default()
            },
        );
        assert_eq!(turned.as_ref().map(|r| r.format), Some(RasterFormat::Jpeg));
        let size = turned.as_ref().and_then(decode).map(|i| (i.width(), i.height()));
        assert_eq!(size, Some((2, 4)));
    }

    #[test]
    fn upright_of_broken_bytes_is_none_unless_already_upright() {
        let junk = Raster {
            format: RasterFormat::Png,
            bytes: b"\x89PNG not really".to_vec(),
        };
        assert!(decode(&junk).is_none());
        assert!(
            upright(
                junk.clone(),
                Orientation {
                    flip_y: true,
                    ..Default::default()
                }
            )
            .is_none()
        );
        // Upright pictures are passed through untouched, without decoding.
        assert_eq!(upright(junk.clone(), Orientation::default()), Some(junk));
    }
}

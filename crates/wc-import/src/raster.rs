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

/// Uncompressed samples (8 bits, or 1 bit for black and white scans) as PNG. `channels` is 1
/// (grey) or 3 (RGB); `invert` swaps black and white, as a PDF `/Decode [1 0]` asks.
pub fn samples_to_png(width: u32, height: u32, channels: u8, bits: u8, invert: bool, data: &[u8]) -> Option<Raster> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
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
        assert!(samples_to_png(2, 2, 4, 8, false, &[0; 16]).is_none());
        assert!(
            decode(&Raster {
                format: RasterFormat::Jpeg,
                bytes: vec![0xFF, 0xD8, 0, 1]
            })
            .is_none()
        );
    }
}

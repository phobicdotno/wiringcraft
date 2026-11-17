//! Screenshots for the assistant: pasted from the clipboard or dropped on the panel as image
//! files, made small enough to send, and put back on the clipboard so the person can paste one
//! into an issue form in the browser.
//!
//! On Linux the clipboard is reached through X11, so a Wayland desktop needs XWayland for
//! pictures (GNOME and KDE run it by default); the button then says the clipboard is not
//! available. Dropping a file works either way.

use std::io::Cursor;
use std::path::Path;

use base64::Engine;
use eframe::egui;

/// Longest side sent, in pixels. Larger pictures cost more and are scaled down by the model anyway.
pub const MAX_SIDE: u32 = 1568;
/// Screenshots waiting to go with one message.
pub const MAX_WAITING: usize = 5;
/// Largest image file taken from a drop, in bytes.
pub const MAX_FILE: u64 = 30 * 1024 * 1024;

/// One screenshot, as RGBA pixels at the size it is sent.
#[derive(Clone)]
pub struct Shot {
    pub image: image::RgbaImage,
}

impl std::fmt::Debug for Shot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Shot({}×{})", self.image.width(), self.image.height())
    }
}

impl Shot {
    /// From RGBA bytes, scaled down to [`MAX_SIDE`].
    pub fn from_rgba(width: usize, height: usize, bytes: Vec<u8>) -> Result<Self, String> {
        let (w, h) = (
            u32::try_from(width).map_err(|_| "The picture is too large.")?,
            u32::try_from(height).map_err(|_| "The picture is too large.")?,
        );
        if w == 0 || h == 0 {
            return Err("The picture is empty.".into());
        }
        let image = image::RgbaImage::from_raw(w, h, bytes).ok_or("The picture's pixels do not match its size.")?;
        Ok(Self::fit(image))
    }

    /// From an image file (PNG or JPEG).
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let size = std::fs::metadata(path)
            .map_err(|e| format!("Could not read {}: {e}", path.display()))?
            .len();
        if size > MAX_FILE {
            return Err(format!(
                "{} is larger than {} MB.",
                path.display(),
                MAX_FILE / 1024 / 1024
            ));
        }
        let image = image::ImageReader::open(path)
            .and_then(|r| r.with_guessed_format())
            .map_err(|e| format!("Could not read {}: {e}", path.display()))?
            .decode()
            .map_err(|e| format!("{} is not a PNG or JPEG picture: {e}", path.display()))?;
        Ok(Self::fit(image.to_rgba8()))
    }

    fn fit(image: image::RgbaImage) -> Self {
        let (w, h) = image.dimensions();
        let long = w.max(h);
        if long <= MAX_SIDE {
            return Self { image };
        }
        let scale = f64::from(MAX_SIDE) / f64::from(long);
        let size = |n: u32| ((f64::from(n) * scale).round() as u32).clamp(1, MAX_SIDE);
        Self {
            image: image::imageops::resize(&image, size(w), size(h), image::imageops::FilterType::Triangle),
        }
    }

    /// As sent to the assistant: a base64 PNG.
    pub fn to_agent(&self) -> Result<wc_agent::Image, String> {
        let mut png = Vec::new();
        self.image
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| format!("Could not encode the screenshot: {e}"))?;
        Ok(wc_agent::Image {
            media_type: "image/png".into(),
            data: base64::engine::general_purpose::STANDARD.encode(png),
        })
    }

    /// For showing in the panel.
    pub fn color_image(&self) -> egui::ColorImage {
        let (w, h) = self.image.dimensions();
        egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], self.image.as_raw())
    }
}

/// The system clipboard. Kept open while the app runs, since on Linux what was copied is only
/// offered while the clipboard that copied it is open.
#[derive(Default)]
pub struct Clipboard(Option<arboard::Clipboard>);

impl Clipboard {
    fn get(&mut self) -> Result<&mut arboard::Clipboard, String> {
        if self.0.is_none() {
            self.0 = Some(arboard::Clipboard::new().map_err(|e| format!("The clipboard is not available: {e}"))?);
        }
        self.0.as_mut().ok_or_else(|| "The clipboard is not available.".into())
    }

    /// Whether the clipboard holds text (a paste then goes into the text box instead).
    pub fn has_text(&mut self) -> bool {
        self.get()
            .ok()
            .and_then(|c| c.get_text().ok())
            .is_some_and(|t| !t.trim().is_empty())
    }

    /// The picture on the clipboard, if there is one.
    pub fn paste(&mut self) -> Result<Option<Shot>, String> {
        match self.get()?.get_image() {
            Ok(img) => Shot::from_rgba(img.width, img.height, img.bytes.into_owned()).map(Some),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(format!("Could not read the clipboard: {e}")),
        }
    }

    /// Put a screenshot on the clipboard, to paste somewhere else.
    pub fn copy(&mut self, shot: &Shot) -> Result<(), String> {
        let (w, h) = shot.image.dimensions();
        self.get()?
            .set_image(arboard::ImageData {
                width: w as usize,
                height: h as usize,
                bytes: std::borrow::Cow::Borrowed(shot.image.as_raw()),
            })
            .map_err(|e| format!("Could not copy the screenshot: {e}"))
    }
}

/// Whether a dropped file looks like a picture, by its name.
pub fn is_picture(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn screenshots_are_checked_scaled_and_encoded() {
        assert!(Shot::from_rgba(0, 10, Vec::new()).is_err());
        assert!(Shot::from_rgba(2, 2, vec![0; 3]).is_err());
        let small = Shot::from_rgba(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        assert_eq!(small.image.dimensions(), (2, 1));
        let big = Shot::from_rgba(4000, 1000, vec![9; 4000 * 1000 * 4]).unwrap();
        assert_eq!(big.image.dimensions(), (MAX_SIDE, 392));
        let sent = small.to_agent().unwrap();
        assert_eq!(sent.media_type, "image/png");
        let png = base64::engine::general_purpose::STANDARD.decode(&sent.data).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert_eq!(small.color_image().size, [2, 1]);
    }

    #[test]
    fn dropped_files_are_read_or_refused() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("Shot.PNG");
        image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]))
            .save_with_format(&png, image::ImageFormat::Png)
            .unwrap();
        assert!(is_picture(&png));
        assert_eq!(Shot::from_file(&png).unwrap().image.dimensions(), (3, 2));
        let text = dir.path().join("notes.png");
        std::fs::write(&text, "not a picture").unwrap();
        assert!(Shot::from_file(&text).is_err());
        assert!(Shot::from_file(&dir.path().join("gone.png")).is_err());
        assert!(!is_picture(Path::new("page.json")));
    }
}

//! Colour tokens for the app. Everything paints with these, so light and dark stay consistent.

use eframe::egui::{self, Color32};

pub struct Theme {
    pub backdrop: Color32,
    pub paper: Color32,
    pub shadow: Color32,
    pub grid: Color32,
    pub ink: Color32,
    pub faint: Color32,
    pub wire: Color32,
    pub wire_label: Color32,
    pub pin: Color32,
    pub selection: Color32,
    pub ghost: Color32,
    pub error: Color32,
    pub icon: Color32,
    pub icon_active_bg: Color32,
    pub toolbar_bg: Color32,
}

impl Theme {
    pub fn of(ctx: &egui::Context) -> Self {
        // The paper stays white in both modes, like a printed sheet.
        let dark = ctx.style().visuals.dark_mode;
        Self {
            backdrop: if dark {
                Color32::from_rgb(0x2b, 0x2d, 0x31)
            } else {
                Color32::from_rgb(0x9a, 0xa1, 0xab)
            },
            paper: Color32::from_rgb(0xff, 0xff, 0xfc),
            shadow: Color32::from_black_alpha(70),
            grid: Color32::from_rgb(0xc4, 0xc9, 0xd0),
            ink: Color32::from_rgb(0x11, 0x14, 0x18),
            faint: Color32::from_rgb(0x6b, 0x71, 0x7a),
            wire: Color32::from_rgb(0x0d, 0x3b, 0x8c),
            wire_label: Color32::from_rgb(0x2b, 0x55, 0x9e),
            pin: Color32::from_rgb(0xb0, 0x3a, 0x2e),
            selection: Color32::from_rgb(0xe0, 0x6c, 0x00),
            ghost: Color32::from_rgba_unmultiplied(0x1f, 0x7a, 0x4d, 200),
            error: Color32::from_rgb(0xc6, 0x28, 0x28),
            icon: if dark {
                Color32::from_rgb(0xdd, 0xe1, 0xe6)
            } else {
                Color32::from_rgb(0x2a, 0x30, 0x38)
            },
            icon_active_bg: if dark {
                Color32::from_rgb(0x3d, 0x5a, 0x80)
            } else {
                Color32::from_rgb(0xc8, 0xd9, 0xef)
            },
            toolbar_bg: if dark {
                Color32::from_rgb(0x33, 0x36, 0x3b)
            } else {
                Color32::from_rgb(0xe9, 0xeb, 0xee)
            },
        }
    }
}

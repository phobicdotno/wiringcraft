//! Colour tokens, fonts and widget style for the app. Everything paints with these, so light
//! and dark stay consistent. The palette is WiringCraft's own: cool neutral greys with a
//! teal accent, and a white sheet in both modes like a printed drawing.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle,
};

/// Light or dark interface. Stored in the preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Light,
    Dark,
}

pub struct Theme {
    /// Window and panel background.
    pub panel: Color32,
    /// Slightly raised surface: toolbars, headers, cards.
    pub surface: Color32,
    /// Hairlines between areas.
    pub border: Color32,
    pub text: Color32,
    /// Secondary interface text: hints, counts, dates. Use this, not `faint`, off the sheet.
    pub muted: Color32,
    pub accent: Color32,
    /// Accent tint behind selected rows and pressed tools.
    pub accent_soft: Color32,
    pub hover: Color32,
    pub success: Color32,
    pub warning: Color32,

    // Canvas.
    pub backdrop: Color32,
    pub backdrop_dot: Color32,
    pub paper: Color32,
    pub shadow: Color32,
    pub grid: Color32,
    pub grid_major: Color32,
    pub ink: Color32,
    /// Secondary marks on the white sheet. Too dark for text on the dark window.
    pub faint: Color32,
    pub wire: Color32,
    pub wire_label: Color32,
    pub pin: Color32,
    pub selection: Color32,
    pub ghost: Color32,
    pub error: Color32,

    // Chrome.
    pub icon: Color32,
    pub toolbar_bg: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Theme {
    pub fn of(ctx: &egui::Context) -> Self {
        Self::new(ctx.style().visuals.dark_mode)
    }

    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                panel: rgb(0x1c1f24),
                surface: rgb(0x23272d),
                border: rgb(0x31363e),
                text: rgb(0xe4e7eb),
                muted: rgb(0x8e96a1),
                accent: rgb(0x3cc2b4),
                accent_soft: rgb(0x1d3a39),
                hover: rgb(0x2b3037),
                success: rgb(0x4cc38a),
                warning: rgb(0xf0a33a),
                backdrop: rgb(0x14161a),
                backdrop_dot: rgb(0x1f2228),
                paper: rgb(0xf7f7f3),
                shadow: Color32::from_black_alpha(140),
                grid: rgb(0xd3d7dd),
                grid_major: rgb(0xb3bac4),
                ink: rgb(0x14171c),
                faint: rgb(0x6b727c),
                wire: rgb(0x1a48a8),
                wire_label: rgb(0x2d5bb5),
                pin: rgb(0xc0432f),
                selection: rgb(0xe8830c),
                ghost: Color32::from_rgba_unmultiplied(0x0b, 0x8a, 0x7f, 210),
                error: rgb(0xe5484d),
                icon: rgb(0xc9ced5),
                toolbar_bg: rgb(0x23272d),
            }
        } else {
            Self {
                panel: rgb(0xf7f8fa),
                surface: rgb(0xffffff),
                border: rgb(0xe1e4e9),
                text: rgb(0x1f2329),
                muted: rgb(0x68707b),
                accent: rgb(0x0b7f77),
                accent_soft: rgb(0xd8efec),
                hover: rgb(0xebeef2),
                success: rgb(0x1f8f5a),
                warning: rgb(0xc26a00),
                backdrop: rgb(0xe2e5ea),
                backdrop_dot: rgb(0xd6dae0),
                paper: rgb(0xffffff),
                shadow: Color32::from_black_alpha(46),
                grid: rgb(0xd0d5dc),
                grid_major: rgb(0xaeb6c1),
                ink: rgb(0x14171c),
                faint: rgb(0x6b727c),
                wire: rgb(0x1a48a8),
                wire_label: rgb(0x2d5bb5),
                pin: rgb(0xc0432f),
                selection: rgb(0xe8830c),
                ghost: Color32::from_rgba_unmultiplied(0x0b, 0x8a, 0x7f, 210),
                error: rgb(0xd42f35),
                icon: rgb(0x3a414b),
                toolbar_bg: rgb(0xffffff),
            }
        }
    }
}

/// The font family used for headings and emphasis.
pub fn strong_family() -> FontFamily {
    FontFamily::Name("strong".into())
}

/// A heading-weight font of the given size.
pub fn strong(size: f32) -> FontId {
    FontId::new(size, strong_family())
}

/// Install Inter (bundled, SIL Open Font License) as the interface font.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "inter-semibold".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-SemiBold.ttf"
        ))),
    );
    if let Some(list) = fonts.families.get_mut(&FontFamily::Proportional) {
        list.insert(0, "inter".into());
    }
    let mut strong = vec!["inter-semibold".to_string()];
    strong.extend(
        fonts
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    fonts.families.insert(strong_family(), strong);
    ctx.set_fonts(fonts);
}

/// Apply the WiringCraft style for `mode` to the whole app.
pub fn apply(ctx: &egui::Context, mode: Mode) {
    let dark = mode == Mode::Dark;
    // Pin egui to the chosen mode. Left on "follow the system", egui keeps a
    // second, unstyled style for the other OS mode and swaps to it as soon as
    // the OS changes between dark and light, throwing away all of the below.
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    let t = Theme::new(dark);
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    let r_small = CornerRadius::same(5);
    v.panel_fill = t.panel;
    v.window_fill = t.surface;
    v.extreme_bg_color = if dark { rgb(0x16191d) } else { rgb(0xffffff) };
    v.faint_bg_color = if dark { rgb(0x21252b) } else { rgb(0xf1f3f6) };
    v.code_bg_color = t.hover;
    v.window_stroke = Stroke::new(1.0_f32, t.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = Shadow {
        offset: [0, 8],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 110 } else { 38 }),
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 100 } else { 30 }),
    };
    v.selection.bg_fill = t.accent_soft;
    v.selection.stroke = Stroke::new(1.0_f32, t.accent);
    v.hyperlink_color = t.accent;
    v.warn_fg_color = t.warning;
    v.error_fg_color = t.error;
    v.override_text_color = None;
    v.striped = true;
    v.interact_cursor = None;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.panel;
    w.noninteractive.weak_bg_fill = t.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, t.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, t.text);
    w.noninteractive.corner_radius = r_small;

    let field = if dark { rgb(0x2a2f36) } else { rgb(0xeef0f3) };
    w.inactive.bg_fill = field;
    w.inactive.weak_bg_fill = field;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, t.border);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, t.text);
    w.inactive.corner_radius = r_small;

    w.hovered.bg_fill = t.hover;
    w.hovered.weak_bg_fill = t.hover;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, t.border);
    w.hovered.fg_stroke = Stroke::new(1.0_f32, t.text);
    w.hovered.corner_radius = r_small;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = t.accent_soft;
    w.active.weak_bg_fill = t.accent_soft;
    w.active.bg_stroke = Stroke::new(1.0_f32, t.accent);
    w.active.fg_stroke = Stroke::new(1.0_f32, t.text);
    w.active.corner_radius = r_small;
    w.active.expansion = 0.0;

    w.open.bg_fill = t.hover;
    w.open.weak_bg_fill = t.hover;
    w.open.bg_stroke = Stroke::new(1.0_f32, t.border);
    w.open.fg_stroke = Stroke::new(1.0_f32, t.text);
    w.open.corner_radius = r_small;

    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Body, FontId::proportional(13.0)),
            (TextStyle::Button, FontId::proportional(13.0)),
            (TextStyle::Heading, strong(15.0)),
            (TextStyle::Monospace, FontId::monospace(12.0)),
        ]
        .into();
        let sp = &mut s.spacing;
        sp.item_spacing = egui::vec2(8.0, 5.0);
        sp.button_padding = egui::vec2(8.0, 3.0);
        sp.interact_size.y = 24.0;
        sp.menu_margin = Margin::same(6);
        sp.window_margin = Margin::same(14);
        sp.indent = 16.0;
        sp.combo_height = 260.0;
        sp.scroll.bar_width = 8.0;
        sp.scroll.floating = true;
        s.interaction.selectable_labels = false;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color32) -> f32 {
        let ch = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn interface_text_reads_on_panels_in_both_modes() {
        for dark in [false, true] {
            let t = Theme::new(dark);
            for bg in [t.panel, t.surface] {
                assert!(contrast(t.text, bg) >= 4.5, "text, dark={dark}");
                assert!(contrast(t.muted, bg) >= 4.5, "muted, dark={dark}");
            }
        }
    }
}

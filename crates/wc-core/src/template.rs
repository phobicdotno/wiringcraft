//! The page template: the frame's reference columns and the title block's boxes, shared by
//! every page of a project.
//!
//! It is kept in `project.toml` under `[template]`. A project without one draws the built-in
//! layout ([`PageTemplate::default`]), so the section is only written once the layout is changed.

use serde::{Deserialize, Serialize};

use crate::geom::Align;

/// Height of the title block band at the bottom of every page, in millimetres, unless the
/// template sets another ([`PageTemplate::height`]).
pub const TITLE_HEIGHT: f64 = 24.0;
/// The lowest title block band a template can ask for, in millimetres.
pub const MIN_TITLE_HEIGHT: f64 = 10.0;
/// The tallest title block band a template can ask for, in millimetres.
pub const MAX_TITLE_HEIGHT: f64 = 60.0;
/// The most reference columns a template can ask for.
pub const MAX_COLUMNS: usize = 40;
/// Padding between a title-block text and its box's sides, in millimetres.
pub const PAD: f64 = 1.5;

/// The values a title-block text can show besides the project's own title-block fields
/// (`customer`, `revision`, ... from [`crate::project::TITLE_FIELDS`] or any other key).
pub const SPECIAL_VALUES: &[(&str, &str)] = &[
    (PROJECT_NAME, "Project name"),
    (PAGE_TITLE, "Page title"),
    (PAGE_NUMBER, "Page number"),
    (SHEET, "Sheet (3 / 12)"),
];
/// The project's name.
pub const PROJECT_NAME: &str = "project.name";
/// The page's title.
pub const PAGE_TITLE: &str = "page.title";
/// The page's display number.
pub const PAGE_NUMBER: &str = "page.number";
/// The page's place in the document and the number of pages, as `3 / 12`.
pub const SHEET: &str = "page.sheet";

/// The layout every page is drawn on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageTemplate {
    /// Reference columns across the frame (1 to [`MAX_COLUMNS`]). Cross-references and wire
    /// numbers by column use the same count.
    #[serde(default = "default_columns")]
    pub columns: usize,
    /// Height of the title block band in millimetres ([`MIN_TITLE_HEIGHT`] to
    /// [`MAX_TITLE_HEIGHT`]); [`TITLE_HEIGHT`] when not set.
    #[serde(default = "default_height", skip_serializing_if = "is_default_height")]
    pub height: f64,
    /// The title block's boxes, drawn in this order.
    #[serde(default, rename = "box")]
    pub boxes: Vec<TitleBox>,
}

fn default_columns() -> usize {
    10
}

fn default_height() -> f64 {
    TITLE_HEIGHT
}

fn is_default_height(h: &f64) -> bool {
    *h == TITLE_HEIGHT
}

/// A box's left, top, right and bottom ([`TitleBox::resolve`]).
pub type BoxSides = (f64, f64, f64, f64);

/// The least width, in millimetres, a box measured from both sides of the frame keeps before
/// the title block is narrowed to fit ([`PageTemplate::boxes_on`]).
pub const MIN_MIDDLE_BOX: f64 = 40.0;

/// Which side of the frame a box edge is measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    #[default]
    Left,
    Right,
}

impl Side {
    fn is_left(&self) -> bool {
        *self == Side::Left
    }
}

/// One box of the title block. Its sides are millimetres from the frame's left or right edge,
/// so boxes measured from the right stay at the right on every sheet size; its top and bottom
/// are millimetres down from the top of the title block band (0 to its height).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TitleBox {
    /// The small caption in the box's top-left corner; empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub left: f64,
    #[serde(default, skip_serializing_if = "Side::is_left")]
    pub left_from: Side,
    pub right: f64,
    #[serde(default, skip_serializing_if = "Side::is_left")]
    pub right_from: Side,
    #[serde(default)]
    pub top: f64,
    #[serde(default = "default_bottom")]
    pub bottom: f64,
    /// The values written in the box.
    #[serde(default, rename = "text", skip_serializing_if = "Vec::is_empty")]
    pub texts: Vec<BoxText>,
    /// The project's logo goes in this box (when the project has one).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub logo: bool,
}

fn default_bottom() -> f64 {
    TITLE_HEIGHT
}

/// A value written in a title-block box. Too long a value is drawn smaller so it fits the box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoxText {
    /// What it shows: title-block field keys (`customer`, `revision`, ...) or the special
    /// values in [`SPECIAL_VALUES`]. Several are joined with ` · `, leaving out empty ones.
    pub values: Vec<String>,
    /// Text height in millimetres.
    pub size: f64,
    /// Baseline, in millimetres below the box's top.
    pub y: f64,
    #[serde(default)]
    pub align: Align,
    /// Distance from the box side the text is aligned to (unused when centred).
    #[serde(default = "default_inset")]
    pub inset: f64,
}

fn default_inset() -> f64 {
    PAD
}

impl BoxText {
    pub fn new(values: &[&str], size: f64, y: f64, align: Align) -> Self {
        Self {
            values: values.iter().map(|v| (*v).to_owned()).collect(),
            size,
            y,
            align,
            inset: PAD,
        }
    }

    fn inset(mut self, inset: f64) -> Self {
        self.inset = inset;
        self
    }
}

impl TitleBox {
    /// A box from `left` to `right` (each measured from the given side), `top` to `bottom`.
    pub fn new(label: &str, left: (f64, Side), right: (f64, Side), top: f64, bottom: f64) -> Self {
        Self {
            label: label.to_owned(),
            left: left.0,
            left_from: left.1,
            right: right.0,
            right_from: right.1,
            top,
            bottom,
            texts: Vec::new(),
            logo: false,
        }
    }

    fn with(mut self, text: BoxText) -> Self {
        self.texts.push(text);
        self
    }

    /// The box's sides on a frame running from `x0` to `x1`, clamped to the frame, and its top
    /// and bottom below the band's top, clamped to the band of height `band`. `None` when
    /// nothing is left of it.
    pub fn resolve(&self, x0: f64, x1: f64, band: f64) -> Option<(f64, f64, f64, f64)> {
        self.resolve_scaled(x0, x1, band, 1.0)
    }

    /// [`Self::resolve`] with every distance from a side multiplied by `scale`, for a frame
    /// narrower than the layout needs ([`PageTemplate::boxes_on`]).
    pub fn resolve_scaled(&self, x0: f64, x1: f64, band: f64, scale: f64) -> Option<(f64, f64, f64, f64)> {
        let at = |mm: f64, from: Side| match from {
            Side::Left => x0 + mm * scale,
            Side::Right => x1 - mm * scale,
        };
        let clamp = |v: f64, lo: f64, hi: f64| if v.is_nan() { lo } else { v.clamp(lo, hi) };
        let left = clamp(at(self.left, self.left_from), x0, x1);
        let right = clamp(at(self.right, self.right_from), x0, x1);
        let top = clamp(self.top, 0.0, band);
        let bottom = clamp(self.bottom, 0.0, band);
        (right - left > 1e-6 && bottom - top > 1e-6).then_some((left, top, right, bottom))
    }
}

impl Default for PageTemplate {
    /// The built-in layout: customer and who drew and checked it on the left, the project and
    /// page title in the middle, revision, date, page, location, sheet and logo on the right.
    fn default() -> Self {
        use Side::{Left, Right};
        let half = TITLE_HEIGHT / 2.0;
        let full = TITLE_HEIGHT;
        Self {
            columns: default_columns(),
            height: TITLE_HEIGHT,
            boxes: vec![
                TitleBox::new("Customer", (0.0, Left), (80.0, Left), 0.0, half).with(BoxText::new(
                    &["customer"],
                    3.0,
                    9.5,
                    Align::Left,
                )),
                TitleBox::new("Drawn by", (0.0, Left), (40.0, Left), half, full).with(BoxText::new(
                    &["drawn_by"],
                    2.5,
                    9.5,
                    Align::Left,
                )),
                TitleBox::new("Checked by", (40.0, Left), (80.0, Left), half, full).with(BoxText::new(
                    &["checked_by"],
                    2.5,
                    9.5,
                    Align::Left,
                )),
                TitleBox::new("Project", (80.0, Left), (150.0, Right), 0.0, full)
                    .with(BoxText::new(&[PROJECT_NAME], 4.0, 11.0, Align::Left).inset(3.0))
                    .with(BoxText::new(&[PAGE_TITLE], 3.0, 19.5, Align::Left).inset(3.0))
                    .with(BoxText::new(&["project_number", "plant"], 2.2, 22.0, Align::Right).inset(2.0)),
                TitleBox::new("Revision", (150.0, Right), (60.0, Right), 0.0, half).with(BoxText::new(
                    &["revision"],
                    2.5,
                    9.5,
                    Align::Left,
                )),
                TitleBox::new("Date", (150.0, Right), (60.0, Right), half, full).with(BoxText::new(
                    &["date"],
                    2.5,
                    9.5,
                    Align::Left,
                )),
                TitleBox::new("Page", (60.0, Right), (30.0, Right), 0.0, half).with(BoxText::new(
                    &[PAGE_NUMBER],
                    4.0,
                    10.5,
                    Align::Center,
                )),
                TitleBox::new("Location", (60.0, Right), (30.0, Right), half, full).with(BoxText::new(
                    &["location"],
                    2.5,
                    9.5,
                    Align::Center,
                )),
                TitleBox::new("Sheet", (30.0, Right), (0.0, Right), 0.0, half).with(BoxText::new(
                    &[SHEET],
                    3.0,
                    10.0,
                    Align::Center,
                )),
                TitleBox {
                    logo: true,
                    ..TitleBox::new("", (30.0, Right), (0.0, Right), half, full)
                },
            ],
        }
    }
}

impl PageTemplate {
    /// Whether this is the built-in layout (then it is not written to `project.toml`).
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Why this template cannot be used, if it cannot: a column count outside 1 to
    /// [`MAX_COLUMNS`], or a box or text whose numbers are not finite, a box that is not inside
    /// the title block band (top above bottom, both within 0 to its height), a band height
    /// outside [`MIN_TITLE_HEIGHT`] to [`MAX_TITLE_HEIGHT`], a negative
    /// edge distance or a text height that is not positive. For templates that come from
    /// outside the app's form (the assistant, a hand-edited file).
    pub fn problem(&self) -> Option<String> {
        if !(1..=MAX_COLUMNS).contains(&self.columns) {
            return Some(format!(
                "the column count must be 1 to {MAX_COLUMNS}, not {}",
                self.columns
            ));
        }
        if !(MIN_TITLE_HEIGHT..=MAX_TITLE_HEIGHT).contains(&self.height) {
            return Some(format!(
                "the title block height must be {MIN_TITLE_HEIGHT} to {MAX_TITLE_HEIGHT} mm, not {}",
                self.height
            ));
        }
        let band = self.height;
        for (i, b) in self.boxes.iter().enumerate() {
            let name = if b.label.is_empty() {
                format!("box {}", i + 1)
            } else {
                format!("box {} ({})", i + 1, b.label)
            };
            if ![b.left, b.right, b.top, b.bottom].iter().all(|v| v.is_finite()) {
                return Some(format!("{name}: every edge must be a number"));
            }
            if b.left < 0.0 || b.right < 0.0 {
                return Some(format!(
                    "{name}: left and right are distances from the frame edge, 0 or more"
                ));
            }
            if !(0.0 <= b.top && b.top < b.bottom && b.bottom <= band) {
                return Some(format!(
                    "{name}: top must be above bottom, both 0 to {band} mm down the title block"
                ));
            }
            for t in &b.texts {
                if !(t.size.is_finite() && t.size > 0.0 && t.y.is_finite() && t.inset.is_finite()) {
                    return Some(format!(
                        "{name}: a text needs a positive height and numbers for y and inset"
                    ));
                }
            }
        }
        None
    }

    /// The frame width the title block needs as laid out: every box measured from both sides
    /// keeps [`MIN_MIDDLE_BOX`] millimetres, and no box runs past the other side.
    pub fn needed_width(&self) -> f64 {
        self.boxes
            .iter()
            .map(|b| {
                let v = |mm: f64| if mm.is_finite() { mm.max(0.0) } else { 0.0 };
                match (b.left_from, b.right_from) {
                    (Side::Left, Side::Right) => v(b.left) + v(b.right) + MIN_MIDDLE_BOX,
                    (Side::Left, Side::Left) => v(b.right),
                    (Side::Right, Side::Right) => v(b.left),
                    // Its edges cross over: the box is visible on frames narrower than
                    // the sum, so it only needs each edge to stay on the frame.
                    (Side::Right, Side::Left) => v(b.left).max(v(b.right)),
                }
            })
            .fold(0.0, f64::max)
    }

    /// The boxes on a frame from `x0` to `x1` with a band `band` high, each with its sides
    /// ([`TitleBox::resolve`]). On a frame narrower than [`Self::needed_width`] (an upright
    /// A4 sheet) the whole layout is narrowed to fit, so boxes keep their order and never
    /// overlap; texts shrink to their boxes as always.
    pub fn boxes_on(&self, x0: f64, x1: f64, band: f64) -> Vec<(&TitleBox, BoxSides)> {
        let need = self.needed_width();
        let scale = if need > x1 - x0 && need > 0.0 {
            (x1 - x0) / need
        } else {
            1.0
        };
        self.boxes
            .iter()
            .filter_map(|b| b.resolve_scaled(x0, x1, band, scale).map(|r| (b, r)))
            .collect()
    }

    /// The title block band's height in use: [`Self::height`] kept within [`MIN_TITLE_HEIGHT`]
    /// to [`MAX_TITLE_HEIGHT`] ([`TITLE_HEIGHT`] for a value that is not a number).
    pub fn band_height(&self) -> f64 {
        if self.height.is_finite() {
            self.height.clamp(MIN_TITLE_HEIGHT, MAX_TITLE_HEIGHT)
        } else {
            TITLE_HEIGHT
        }
    }

    /// The column count in use: [`Self::columns`] kept within 1 to [`MAX_COLUMNS`].
    pub fn column_count(&self) -> usize {
        self.columns.clamp(1, MAX_COLUMNS)
    }
}

/// The label for a value a title-block text can show: a special value's or a title-block
/// field's label, or the key itself.
pub fn value_label(key: &str) -> &str {
    SPECIAL_VALUES
        .iter()
        .find(|(k, _)| *k == key)
        .map_or_else(|| crate::project::title_field_label(key), |(_, l)| l)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips_and_is_not_written() {
        let t = PageTemplate::default();
        assert!(t.is_default());
        let s = toml::to_string(&t).unwrap();
        let back: PageTemplate = toml::from_str(&s).unwrap();
        assert_eq!(back, t);
        let meta = toml::to_string(&crate::ProjectMeta::new("x")).unwrap();
        assert!(!meta.contains("template"), "{meta}");
    }

    #[test]
    fn a_short_hand_written_template_reads() {
        let t: PageTemplate = toml::from_str(
            r#"
            columns = 12
            [[box]]
            label = "Order"
            left = 0
            right = 50
            right_from = "right"
            [[box.text]]
            values = ["order"]
            size = 3
            y = 9.5
            "#,
        )
        .unwrap();
        assert_eq!(t.columns, 12);
        let b = &t.boxes[0];
        assert_eq!((b.top, b.bottom, b.right_from), (0.0, TITLE_HEIGHT, Side::Right));
        assert_eq!((b.texts[0].align, b.texts[0].inset), (Align::Left, PAD));
        assert_eq!(
            b.resolve(10.0, 410.0, TITLE_HEIGHT),
            Some((10.0, 0.0, 360.0, TITLE_HEIGHT))
        );
    }

    #[test]
    fn boxes_are_clamped_to_the_frame_and_empty_ones_dropped() {
        let b = TitleBox::new("", (-5.0, Side::Left), (500.0, Side::Left), -1.0, 99.0);
        assert_eq!(
            b.resolve(10.0, 410.0, TITLE_HEIGHT),
            Some((10.0, 0.0, 410.0, TITLE_HEIGHT))
        );
        let b = TitleBox::new("", (50.0, Side::Left), (40.0, Side::Left), 0.0, 12.0);
        assert_eq!(b.resolve(10.0, 410.0, TITLE_HEIGHT), None);
        let b = TitleBox::new("", (f64::NAN, Side::Left), (40.0, Side::Left), 0.0, 12.0);
        assert_eq!(b.resolve(10.0, 410.0, TITLE_HEIGHT), Some((10.0, 0.0, 50.0, 12.0)));
    }

    #[test]
    fn the_title_block_height_is_saved_only_when_changed_and_kept_in_range() {
        let t = PageTemplate::default();
        assert_eq!(t.height, TITLE_HEIGHT);
        assert!(!toml::to_string(&t).unwrap().contains("height"));
        let tall = PageTemplate {
            height: 36.0,
            ..Default::default()
        };
        let text = toml::to_string(&tall).unwrap();
        assert!(text.contains("height = 36"), "{text}");
        assert_eq!(toml::from_str::<PageTemplate>(&text).unwrap(), tall);
        assert!(!tall.is_default());
        assert_eq!(tall.problem(), None);
        // Out of range: named by problem, clamped where drawn.
        for (h, used) in [
            (5.0, MIN_TITLE_HEIGHT),
            (70.0, MAX_TITLE_HEIGHT),
            (f64::NAN, TITLE_HEIGHT),
        ] {
            let t = PageTemplate {
                height: h,
                ..Default::default()
            };
            assert!(t.problem().is_some_and(|p| p.contains("height")), "{h}");
            assert_eq!(t.band_height(), used);
        }
        // A box may reach the bottom of a taller band, not past it.
        let mut t = tall.clone();
        t.boxes[0].bottom = 36.0;
        assert_eq!(t.problem(), None);
        t.boxes[0].bottom = 36.5;
        assert!(t.problem().is_some_and(|p| p.contains("box 1")));
        assert_eq!(t.boxes[0].resolve(10.0, 410.0, 36.0).map(|r| r.3), Some(36.0));
    }

    #[test]
    fn the_built_in_template_is_valid_and_bad_ones_say_why() {
        assert_eq!(PageTemplate::default().problem(), None);
        let t = PageTemplate {
            columns: 0,
            ..Default::default()
        };
        assert!(t.problem().is_some_and(|p| p.contains("column count")));
        let mut t = PageTemplate::default();
        t.boxes[0].bottom = TITLE_HEIGHT + 1.0;
        assert!(t.problem().is_some_and(|p| p.contains("box 1")));
        let mut t = PageTemplate::default();
        t.boxes[0].left = f64::NAN;
        assert!(t.problem().is_some());
        let mut t = PageTemplate::default();
        t.boxes.push(
            TitleBox::new("x", (0.0, Side::Left), (1.0, Side::Left), 0.0, 5.0).with(BoxText::new(
                &["x"],
                0.0,
                1.0,
                Align::Left,
            )),
        );
        assert!(t.problem().is_some_and(|p| p.contains("text")));
    }

    #[test]
    fn column_count_is_kept_in_range() {
        let mut t = PageTemplate {
            columns: 0,
            ..Default::default()
        };
        assert_eq!(t.column_count(), 1);
        t.columns = 1000;
        assert_eq!(t.column_count(), MAX_COLUMNS);
    }

    #[test]
    fn a_narrow_frame_narrows_the_whole_title_block() {
        let t = PageTemplate::default();
        let band = t.band_height();
        // A3 and A4 landscape are wide enough: the boxes are where they always were.
        for x1 in [410.0, 287.0] {
            let wide: Vec<_> = t.boxes.iter().filter_map(|b| b.resolve(10.0, x1, band)).collect();
            let on: Vec<_> = t.boxes_on(10.0, x1, band).into_iter().map(|(_, r)| r).collect();
            assert_eq!(on, wide);
        }
        // An upright A4 sheet (190 mm inside the frame) keeps every box, in the frame and in
        // order: the project box still has room.
        let on = t.boxes_on(10.0, 200.0, band);
        assert_eq!(on.len(), t.boxes.len());
        assert!(on.iter().all(|(_, (l, _, r, _))| *l >= 10.0 && *r <= 200.0 && r > l));
        let project = on.iter().find(|(b, _)| b.label == "Project").map(|(_, r)| *r);
        assert!(project.is_some_and(|(l, _, r, _)| r - l > 20.0), "{project:?}");
    }

    #[test]
    fn a_box_with_crossed_anchors_that_fits_stays_as_it_is() {
        // Left edge 40 mm from the right, right edge 380 mm from the left: 20 mm wide on a
        // 400 mm frame. Nothing has to narrow for it.
        let t = PageTemplate {
            boxes: vec![TitleBox::new(
                "Logo",
                (40.0, Side::Right),
                (380.0, Side::Left),
                0.0,
                10.0,
            )],
            ..PageTemplate::default()
        };
        let band = t.band_height();
        let on: Vec<_> = t.boxes_on(0.0, 400.0, band).into_iter().map(|(_, r)| r).collect();
        assert_eq!(on, [(360.0, 0.0, 380.0, 10.0)]);
        // On a frame too narrow for its edges it narrows with the rest and stays visible.
        let on = t.boxes_on(0.0, 300.0, band);
        assert!(
            on.first().is_some_and(|(_, (l, _, r, _))| r > l && *r <= 300.0),
            "{on:?}"
        );
    }
}

//! Menus, the classic icon toolbar and the optional ribbon. All three show actions from the
//! same registry, so switching layouts never hides a command.

use eframe::egui::{self, Color32, CornerRadius, Rect, Sense, Stroke, Vec2};

use crate::actions::{Action, RIBBON, TOOLBAR};
use crate::icons;
use crate::prefs::Chrome;
use crate::theme::Theme;

/// What the chrome needs to know to draw a button: is it pressed (a current tool or a toggle
/// that is on), and can it be used right now.
pub trait Status {
    fn active(&self, a: Action) -> bool;
    fn enabled(&self, a: Action) -> bool;
}

/// A shortcut hint (an action's `shortcut`) as the keys are on `os`: the Ctrl shortcuts are
/// matched with the Command key, which on a Mac is Cmd, not Ctrl (#1492).
pub fn shortcut_hint(os: egui::os::OperatingSystem, shortcut: &str) -> String {
    match shortcut.strip_prefix("Ctrl+") {
        Some(rest) if os == egui::os::OperatingSystem::Mac => format!("Cmd+{rest}"),
        _ => shortcut.to_owned(),
    }
}

/// An action's `tip` as the keys are on `os`: a Ctrl shortcut named in the text (Redo's
/// Ctrl+Shift+Z) is Cmd on a Mac, as in [`shortcut_hint`].
pub fn tip_hint(os: egui::os::OperatingSystem, tip: &str) -> String {
    if os == egui::os::OperatingSystem::Mac {
        tip.replace("Ctrl+", "Cmd+")
    } else {
        tip.to_owned()
    }
}

#[derive(Default)]
pub struct ChromeState {
    pub ribbon_tab: usize,
}

impl ChromeState {
    /// The chrome with ribbon tab `ribbon_tab` open (from the preferences); a tab the ribbon
    /// does not have (an edited or older file) opens the first one, not an empty ribbon.
    pub fn with_ribbon_tab(ribbon_tab: usize) -> Self {
        Self {
            ribbon_tab: if ribbon_tab < RIBBON.len() { ribbon_tab } else { 0 },
        }
    }
}

const MENUS: &[(&str, &[Option<Action>])] = &[
    (
        "File",
        &[
            Some(Action::NewProject),
            Some(Action::Open),
            Some(Action::Save),
            Some(Action::SaveAs),
            None,
            Some(Action::ProjectProps),
            Some(Action::Translations),
            None,
            Some(Action::Check),
            Some(Action::LiveCheck),
            Some(Action::Reports),
            Some(Action::PlcRacks),
            Some(Action::ExportPdf),
            Some(Action::ExportDrawings),
            Some(Action::ExportAml),
            Some(Action::ExportWagoMarking),
            Some(Action::ExportWagoConfigurator),
            Some(Action::ImportWagoConfigurator),
            None,
            Some(Action::ImportEplan),
            Some(Action::ImportSwe),
            Some(Action::ImportSweLibrary),
            Some(Action::ImportAml),
            Some(Action::ImportPageDwg),
            Some(Action::ImportPageDxf),
            Some(Action::ImportPlcCsv),
            Some(Action::ImportPdf),
            Some(Action::ImportArchive),
            Some(Action::ImportPartsEdz),
            Some(Action::ImportPartsBmecat),
            Some(Action::ImportPartsCsv),
        ],
    ),
    (
        "Edit",
        &[
            Some(Action::Undo),
            Some(Action::Redo),
            None,
            Some(Action::Cut),
            Some(Action::Copy),
            Some(Action::Paste),
            Some(Action::Duplicate),
            Some(Action::SelectAll),
            Some(Action::Find),
            None,
            Some(Action::Rotate),
            Some(Action::Mirror),
            Some(Action::Delete),
            None,
            Some(Action::NumberWires),
            Some(Action::TerminalStrips),
        ],
    ),
    (
        "Insert",
        &[
            Some(Action::Select),
            Some(Action::PlaceSymbol),
            Some(Action::DrawWire),
            Some(Action::ToggleAutoWire),
            None,
            Some(Action::InsertMacro),
            Some(Action::SaveMacro),
        ],
    ),
    (
        "Page",
        &[
            Some(Action::AddPage),
            Some(Action::CoverPage),
            Some(Action::PageTemplate),
            Some(Action::DuplicatePage),
            Some(Action::SavePageMacro),
            Some(Action::PageProps),
            Some(Action::RenumberPages),
            None,
            Some(Action::PageUp),
            Some(Action::PageDown),
            None,
            Some(Action::PrevPage),
            Some(Action::NextPage),
            None,
            Some(Action::DeletePage),
            None,
            Some(Action::NewPanel),
            Some(Action::ArrangePanel),
            Some(Action::DeletePanel),
        ],
    ),
    (
        "View",
        &[
            Some(Action::ZoomIn),
            Some(Action::ZoomOut),
            Some(Action::ZoomFit),
            None,
            Some(Action::ToggleGrid),
            Some(Action::Layers),
            Some(Action::ToggleChrome),
            Some(Action::ToggleTheme),
            None,
            Some(Action::Assistant),
        ],
    ),
    (
        "Library",
        &[
            Some(Action::LibraryStatus),
            Some(Action::LibraryUpdate),
            None,
            Some(Action::NewSymbol),
            Some(Action::EditSymbol),
            Some(Action::ImportSymbols),
        ],
    ),
    (
        "Versions",
        &[
            Some(Action::Commit),
            Some(Action::NewBranch),
            Some(Action::History),
            None,
            Some(Action::CommentTool),
            Some(Action::PageHistory),
            None,
            Some(Action::ShareSession),
            Some(Action::JoinSession),
        ],
    ),
    (
        "Help",
        &[
            Some(Action::ReportProblem),
            Some(Action::RequestFunction),
            Some(Action::ReportMissing),
        ],
    ),
];

/// WiringCraft's mark: a sheet corner with a wire running through two nodes.
pub fn logo(p: &egui::Painter, rect: Rect, t: &Theme) {
    p.rect_filled(rect, CornerRadius::same(5), t.accent);
    let s = rect.width() / 20.0;
    let at = |x: f32, y: f32| rect.min + Vec2::new(x * s, y * s);
    let white = Color32::WHITE;
    let st = Stroke::new(1.8 * s, white);
    p.add(egui::Shape::line(
        vec![at(4.0, 14.0), at(9.0, 14.0), at(9.0, 6.0), at(16.0, 6.0)],
        st,
    ));
    p.circle_filled(at(4.0, 14.0), 2.0 * s, white);
    p.circle_filled(at(16.0, 6.0), 2.0 * s, white);
}

pub fn menu_bar(ui: &mut egui::Ui, status: &dyn Status, out: &mut Vec<Action>) {
    let t = Theme::of(ui.ctx());
    egui::menu::bar(ui, |ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
        logo(ui.painter(), r, &t);
        ui.label(egui::RichText::new("WiringCraft").font(crate::theme::strong(13.0)));
        ui.add_space(6.0);
        for (name, items) in MENUS {
            ui.menu_button(*name, |ui| {
                ui.set_min_width(230.0);
                for item in *items {
                    match item {
                        None => {
                            ui.separator();
                        }
                        Some(a) => {
                            let info = a.info();
                            let button = egui::Button::new(info.label)
                                .shortcut_text(shortcut_hint(ui.ctx().os(), info.shortcut))
                                .selected(status.active(*a));
                            if ui
                                .add_enabled(status.enabled(*a), button)
                                .on_hover_text(tip_hint(ui.ctx().os(), info.tip))
                                .clicked()
                            {
                                out.push(*a);
                                ui.close_menu();
                            }
                        }
                    }
                }
            });
        }
    });
}

/// A square icon button. Returns true when clicked.
fn icon_button(ui: &mut egui::Ui, a: Action, size: f32, status: &dyn Status, t: &Theme) -> bool {
    icon_button_rect(ui, a, None, size, status, t).0
}

/// [`icon_button`], also returning where the button is. With `id`, the button keeps that id
/// wherever it is laid out, so a press or keyboard focus stays with it when it moves.
fn icon_button_rect(
    ui: &mut egui::Ui,
    a: Action,
    id: Option<egui::Id>,
    size: f32,
    status: &dyn Status,
    t: &Theme,
) -> (bool, Rect) {
    let info = a.info();
    let enabled = status.enabled(a);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = match id {
        Some(id) => {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
            (rect, ui.interact(rect, id, sense))
        }
        None => ui.allocate_exact_size(Vec2::splat(size), sense),
    };
    let p = ui.painter();
    let radius = CornerRadius::same(6);
    let active = enabled && status.active(a);
    if active {
        p.rect_filled(rect, radius, t.accent_soft);
        p.rect_stroke(
            rect,
            radius,
            Stroke::new(1.0_f32, t.accent.gamma_multiply(0.55)),
            egui::StrokeKind::Inside,
        );
    } else if resp.is_pointer_button_down_on() && enabled {
        p.rect_filled(rect, radius, t.accent_soft);
    } else if resp.hovered() && enabled {
        p.rect_filled(rect, radius, t.hover);
    }
    let (c, accent) = if !enabled {
        (t.icon.gamma_multiply(0.3), t.icon.gamma_multiply(0.3))
    } else if active {
        (t.accent, t.accent)
    } else {
        (t.icon, t.accent)
    };
    icons::paint(p, rect.shrink(size * 0.2), info.icon, c, accent);
    let tip = if info.shortcut.is_empty() {
        format!("{}\n{}", info.label, tip_hint(ui.ctx().os(), info.tip))
    } else {
        let keys = shortcut_hint(ui.ctx().os(), info.shortcut);
        format!("{} ({keys})\n{}", info.label, tip_hint(ui.ctx().os(), info.tip))
    };
    // A pointer release counts only over the button where it is now: egui hit-tests against the
    // last frame, where a button with an id (a ribbon button) that has moved since still looks to
    // be under the pointer. A button without one is laid out in the same place every frame, and
    // keeps egui's own rule. Where the release that made the click happened counts (the frame's
    // first: egui clicks on it), not where the pointer went after it in the same frame. Enter or
    // Space on the focused button is no pointer click, and counts wherever the pointer is.
    let released_off = ui.input(|i| {
        let release = i.events.iter().find_map(|e| match e {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                ..
            } => Some(*pos),
            _ => None,
        });
        id.is_some() && release.is_some_and(|p| !rect.contains(p))
    });
    // A key counts only when it activated this button (egui's own keyboard click, which it
    // gives the button focused as the keys are handed out): focus that a Tab in the same frame
    // took elsewhere does not make a pointer release off the button count.
    let keyed = resp.clicked() && !resp.clicked_by(egui::PointerButton::Primary);
    (resp.on_hover_text(tip).clicked() && (keyed || !released_off), rect)
}

fn divider(ui: &mut egui::Ui, t: &Theme, height: f32) {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(1.0, height), Sense::hover());
    ui.painter()
        .line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0_f32, t.border));
    ui.add_space(4.0);
}

pub fn classic_toolbar(ui: &mut egui::Ui, status: &dyn Status, out: &mut Vec<Action>) {
    classic_toolbar_rects(ui, status, out);
}

/// Lay out the classic toolbar; returns the rectangles of its button groups and switches.
fn classic_toolbar_rects(ui: &mut egui::Ui, status: &dyn Status, out: &mut Vec<Action>) -> Vec<Rect> {
    const BUTTON: f32 = 30.0;
    const GAP: f32 = 2.0;
    let t = Theme::of(ui.ctx());
    let mut rects = Vec::new();
    // The switches sit on the right; the toolbar groups fill the rest and wrap onto a second
    // row when the window is too narrow for them, so no button is cut off.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            for a in [Action::ToggleChrome, Action::ToggleTheme] {
                if icon_button(ui, a, BUTTON, status, &t) {
                    out.push(a);
                }
            }
            // The switches, first.
            rects.push(ui.min_rect());
            ui.with_layout(
                egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                |ui| {
                    for (i, group) in TOOLBAR.iter().enumerate() {
                        if i > 0 {
                            divider(ui, &t, 20.0);
                        }
                        // A group stays together on one row: it asks for its whole width, so it
                        // goes to the next row when the rest of this one is too short.
                        let n = group.len() as f32;
                        let size = Vec2::new(n * BUTTON + (n - 1.0) * GAP, BUTTON);
                        let r =
                            ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = GAP;
                                for a in *group {
                                    if icon_button(ui, *a, BUTTON, status, &t) {
                                        out.push(*a);
                                    }
                                }
                            });
                        rects.push(r.response.rect);
                    }
                },
            );
        });
    });
    rects
}

pub fn ribbon(ui: &mut egui::Ui, state: &mut ChromeState, status: &dyn Status, out: &mut Vec<Action>) {
    ribbon_rects(ui, state, status, out);
}

/// Lay out the ribbon: its tabs and the open tab's groups; returns the rectangles of the open
/// tab's button icons.
fn ribbon_rects(ui: &mut egui::Ui, state: &mut ChromeState, status: &dyn Status, out: &mut Vec<Action>) -> Vec<Rect> {
    let t = Theme::of(ui.ctx());
    // Tabs: plain words with an accent underline under the open one.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        ui.add_space(4.0);
        for (i, (name, _)) in RIBBON.iter().enumerate() {
            let on = state.ribbon_tab == i;
            let text = egui::RichText::new(*name).color(if on { t.text } else { t.muted });
            let text = if on {
                text.font(crate::theme::strong(13.0))
            } else {
                text
            };
            // Not selectable: a tab is a button, so no text cursor on hover and a drag over
            // it does not select its word.
            let r = ui.add(egui::Label::new(text).selectable(false).sense(Sense::click()));
            if on {
                ui.painter().line_segment(
                    [
                        r.rect.left_bottom() + Vec2::new(0.0, 3.0),
                        r.rect.right_bottom() + Vec2::new(0.0, 3.0),
                    ],
                    Stroke::new(2.0_f32, t.accent),
                );
            } else if r.hovered() {
                ui.painter().line_segment(
                    [
                        r.rect.left_bottom() + Vec2::new(0.0, 3.0),
                        r.rect.right_bottom() + Vec2::new(0.0, 3.0),
                    ],
                    Stroke::new(2.0_f32, t.border),
                );
            }
            if r.clicked() {
                state.ribbon_tab = i;
            }
        }
    });
    ui.add_space(6.0);
    let Some((_, groups)) = RIBBON.get(state.ribbon_tab) else {
        return Vec::new();
    };
    ribbon_groups(ui, groups, status, out)
}

/// The interaction id of the ribbon button for `action` in group `group`: the same on every
/// row the group may wrap to.
fn ribbon_id(group: &str, action: Action) -> egui::Id {
    egui::Id::new(("ribbon", group, action))
}

/// A ribbon button's width, its icon and label.
const RIBBON_BUTTON: f32 = 66.0;
/// The room a divider takes between two ribbon groups.
const RIBBON_DIVIDER: f32 = 9.0;

/// The most lines a ribbon button's label wraps to.
const RIBBON_LABEL_ROWS: usize = 3;

/// A ribbon button's label, centred under its icon and wrapped onto up to
/// [`RIBBON_LABEL_ROWS`] lines rather than cut to one: cut to the button's width, labels that
/// start alike ("Move page up" and "Move page down", the three "Parts from ..." imports) all
/// read the same.
fn ribbon_label(text: &str, color: Color32) -> egui::text::LayoutJob {
    let mut job =
        egui::text::LayoutJob::simple(text.to_owned(), egui::FontId::proportional(11.0), color, RIBBON_BUTTON);
    job.wrap.max_rows = RIBBON_LABEL_ROWS;
    job.halign = egui::Align::Center;
    job
}

/// Lay out one ribbon tab's groups; returns the rectangles of its buttons. Groups go side by
/// side and start a new row when the next one would not fit, so every button stays on screen in
/// a narrow window; a group wider than the window wraps its own buttons. Each button's id comes
/// from its group and command, not its place, so a press stays with its button when the window
/// is resized and the groups move between rows.
fn ribbon_groups(
    ui: &mut egui::Ui,
    groups: &[crate::actions::RibbonGroup],
    status: &dyn Status,
    out: &mut Vec<Action>,
) -> Vec<Rect> {
    let t = Theme::of(ui.ctx());
    let avail = ui.available_width().max(RIBBON_BUTTON);
    let per_row = ((avail / RIBBON_BUTTON).floor() as usize).max(1);
    let width_of = |n: usize| n.min(per_row) as f32 * RIBBON_BUTTON;
    // Which groups share a row.
    let mut rows: Vec<Vec<&crate::actions::RibbonGroup>> = Vec::new();
    let mut x = 0.0;
    for group in groups {
        let w = width_of(group.1.len());
        match rows.last_mut() {
            Some(row) if x + RIBBON_DIVIDER + w <= avail => {
                row.push(group);
                x += RIBBON_DIVIDER + w;
            }
            _ => {
                rows.push(vec![group]);
                x = w;
            }
        }
    }
    let mut rects = Vec::new();
    for row in rows {
        // Top-aligned, so the icons line up when one label wraps to more lines than the rest.
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (k, &(gname, actions)) in row.into_iter().enumerate() {
                if k > 0 {
                    divider(ui, &t, 62.0);
                }
                let width = width_of(actions.len());
                ui.allocate_ui_with_layout(
                    Vec2::new(width, 70.0),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for line in actions.chunks(per_row) {
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                for a in line {
                                    let r = ui.push_id((gname, *a), |ui| {
                                        ui.allocate_ui_with_layout(
                                            Vec2::new(RIBBON_BUTTON, 52.0),
                                            egui::Layout::top_down(egui::Align::Center),
                                            |ui| {
                                                ui.set_width(RIBBON_BUTTON);
                                                ui.spacing_mut().item_spacing.y = 1.0;
                                                let (clicked, icon) = icon_button_rect(
                                                    ui,
                                                    *a,
                                                    Some(ribbon_id(gname, *a)),
                                                    34.0,
                                                    status,
                                                    &t,
                                                );
                                                if clicked {
                                                    out.push(*a);
                                                }
                                                let c = if status.enabled(*a) {
                                                    t.text
                                                } else {
                                                    t.muted.gamma_multiply(0.6)
                                                };
                                                ui.add(egui::Label::new(ribbon_label(a.info().label, c)).wrap());
                                                icon
                                            },
                                        )
                                        .inner
                                    });
                                    rects.push(r.inner);
                                }
                            });
                        }
                        ui.label(egui::RichText::new(gname.to_uppercase()).size(9.5).color(t.muted));
                    },
                );
            }
        });
    }
    rects
}

/// The whole command area under the window title, in the chosen layout.
pub fn show(ctx: &egui::Context, chrome: Chrome, state: &mut ChromeState, status: &dyn Status) -> Vec<Action> {
    let mut out = Vec::new();
    let t = Theme::of(ctx);
    egui::TopBottomPanel::top("menus")
        .frame(
            egui::Frame::new()
                .fill(t.panel)
                .inner_margin(egui::Margin::symmetric(8, 4)),
        )
        .show_separator_line(false)
        .show(ctx, |ui| menu_bar(ui, status, &mut out));
    egui::TopBottomPanel::top("commands")
        .frame(
            egui::Frame::new()
                .fill(t.toolbar_bg)
                .stroke(Stroke::new(1.0_f32, t.border))
                .inner_margin(egui::Margin::symmetric(8, 4)),
        )
        .show_separator_line(false)
        .show(ctx, |ui| match chrome {
            Chrome::Classic => classic_toolbar(ui, status, &mut out),
            Chrome::Ribbon => ribbon(ui, state, status, &mut out),
        });
    out
}

/// A small coloured dot (used for library states in panels).
pub fn dot(ui: &mut egui::Ui, c: Color32) {
    let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter()
        .circle_filled(Rect::from_center_size(r.center(), Vec2::splat(8.0)).center(), 4.0, c);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AllOn;
    impl Status for AllOn {
        fn active(&self, _: Action) -> bool {
            false
        }
        fn enabled(&self, _: Action) -> bool {
            true
        }
    }

    /// A button's tooltip is its label, its shortcut and its tip: a tip that only says the label
    /// again tells nothing (Redo, Zoom in and Zoom out did).
    #[test]
    fn no_tooltip_repeats_the_label() {
        for a in Action::ALL {
            let info = a.info();
            assert!(
                !info.tip.trim().eq_ignore_ascii_case(info.label.trim()),
                "{a:?}: the tip only repeats the label"
            );
        }
    }

    /// The rectangles of the classic toolbar's switches and button groups in a window `width`
    /// points wide.
    fn toolbar_rects(width: f32) -> Vec<Rect> {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(width, 600.0))),
            ..Default::default()
        };
        let mut rects = Vec::new();
        // The first frame loads the fonts; measure on the second.
        let _ = ctx.run(input.clone(), |_| {});
        let _ = ctx.run(input, |ctx| {
            egui::TopBottomPanel::top("tb").show(ctx, |ui| {
                rects = classic_toolbar_rects(ui, &AllOn, &mut Vec::new());
            });
        });
        rects
    }

    /// Run the ribbon with tab `tab` open for one frame per `(width, events)`, in a window
    /// `width` points wide; returns the icon rectangles of the last frame and every command
    /// clicked.
    fn ribbon_frames(tab: usize, frames: &[(f32, Vec<egui::Event>)]) -> (Vec<Rect>, Vec<Action>) {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = ChromeState { ribbon_tab: tab };
        let (mut rects, mut out) = (Vec::new(), Vec::new());
        // The first frame loads the fonts.
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        for (width, events) in frames {
            rects = ribbon_frame(&ctx, &mut state, *width, events.clone(), &mut out);
        }
        (rects, out)
    }

    /// The Start tab's buttons, in the order the ribbon lays them out.
    fn start_order() -> Vec<Action> {
        RIBBON[0].1.iter().flat_map(|(_, a)| a.iter().copied()).collect()
    }

    /// Where Undo is among the Start tab's buttons.
    fn undo_index() -> usize {
        let at = start_order().iter().position(|a| *a == Action::Undo);
        assert!(at.is_some(), "the Start tab has no Undo");
        at.unwrap_or_default()
    }

    /// One frame of the ribbon in a window `width` points wide, as the app lays it out; returns
    /// the icon rectangles and adds the commands clicked to `out`.
    fn ribbon_frame(
        ctx: &egui::Context,
        state: &mut ChromeState,
        width: f32,
        events: Vec<egui::Event>,
        out: &mut Vec<Action>,
    ) -> Vec<Rect> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(width, 600.0))),
            events,
            ..Default::default()
        };
        let mut rects = Vec::new();
        let _ = ctx.run(input, |ctx| {
            egui::TopBottomPanel::top("menus")
                .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 4)))
                .show(ctx, |ui| {
                    rects = ribbon_rects(ui, state, &AllOn, out);
                });
        });
        rects
    }

    /// The icon rectangles of ribbon tab `tab` in a window `width` points wide.
    fn ribbon_icons(tab: usize, width: f32) -> Vec<Rect> {
        ribbon_frames(tab, &[(width, Vec::new())]).0
    }

    /// The button at the other width where Undo is at `from`, if another one is there.
    fn other_where_undo_was(from: f32, to: f32) -> Option<Action> {
        let at = ribbon_icons(0, from).get(undo_index())?.center();
        let there = ribbon_icons(0, to).iter().position(|r| r.contains(at))?;
        start_order().get(there).copied().filter(|a| *a != Action::Undo)
    }

    /// The first window width in `widths` (stepped by 10 points) where widening by 10 points
    /// puts another button where Undo was. Found rather than fixed, so a new button on the
    /// Start tab does not break the tests that need such a pair.
    fn undo_moving_width(widths: std::ops::Range<u32>) -> Option<f32> {
        widths
            .step_by(10)
            .map(|w| w as f32)
            .find(|w| other_where_undo_was(*w, w + 10.0).is_some())
    }

    /// Two window widths 10 points apart, the window's usual size or wider, where Undo sits
    /// elsewhere in the wider one.
    fn undo_moving_widths() -> (f32, f32) {
        let w = undo_moving_width(1000..2500);
        assert!(w.is_some(), "no window width moves the Undo button");
        let w = w.unwrap_or(1210.0);
        (w, w + 10.0)
    }

    #[test]
    fn shortcut_hints_name_cmd_on_a_mac() {
        use egui::os::OperatingSystem::{Mac, Windows};
        assert_eq!(shortcut_hint(Mac, "Ctrl+S"), "Cmd+S");
        assert_eq!(shortcut_hint(Mac, "Ctrl+Shift+N"), "Cmd+Shift+N");
        assert_eq!(shortcut_hint(Mac, "Del"), "Del");
        assert_eq!(shortcut_hint(Mac, ""), "");
        assert_eq!(shortcut_hint(Windows, "Ctrl+S"), "Ctrl+S");
    }

    #[test]
    fn the_redo_tip_names_cmd_shift_z_on_a_mac() {
        use egui::os::OperatingSystem::{Mac, Nix, Windows};
        let tip = Action::Redo.info().tip;
        assert_eq!(
            tip_hint(Mac, tip),
            "Redo the last change you undid (Cmd+Shift+Z works too)"
        );
        assert_eq!(tip_hint(Windows, tip), tip);
        assert_eq!(tip_hint(Nix, tip), tip);
    }

    #[test]
    fn a_ribbon_press_stays_with_its_button_when_the_groups_move() {
        let undo = undo_index();
        let (narrow, wide) = undo_moving_widths();
        // Below the window's minimum of 900 points the groups wrap onto more rows (#351).
        let small = (600..890)
            .step_by(10)
            .map(|w| w as f32)
            .find(|w| other_where_undo_was(*w, w + 10.0).is_some() && other_where_undo_was(w + 10.0, *w).is_some());
        assert!(
            small.is_some(),
            "no width below the minimum moves the Undo button both ways"
        );
        let small = small.unwrap_or(740.0);
        // Widened and narrowed.
        for (from, to) in [(narrow, wide), (small, small + 10.0), (small + 10.0, small)] {
            let at = ribbon_icons(0, from)[undo].center();
            // At the other width another button sits where Undo was.
            let there = other_where_undo_was(from, to);
            assert!(
                there.is_some_and(|a| a != Action::Undo),
                "{from} to {to}: {there:?} at {at:?}"
            );
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let press_then_release_at = |width| {
                ribbon_frames(
                    0,
                    &[
                        (from, Vec::new()),
                        (from, vec![egui::Event::PointerMoved(at)]),
                        (from, vec![button(true)]),
                        (width, vec![button(false)]),
                    ],
                )
                .1
            };
            // Released over the button that took its place: a click on neither.
            let out = press_then_release_at(to);
            assert!(out.is_empty(), "{from} to {to}: {out:?}");
            // Released where it was pressed, the press is a click on Undo.
            assert_eq!(press_then_release_at(from), [Action::Undo], "{from}");
        }
    }

    #[test]
    fn a_ribbon_release_counts_where_it_happened_not_where_the_pointer_went_after() {
        let undo = undo_index();
        let (narrow, wide) = undo_moving_widths();
        let at = ribbon_icons(0, narrow)[undo].center();
        let now = ribbon_icons(0, wide)[undo].center();
        assert!(!ribbon_icons(0, wide)[undo].contains(at), "Undo moved");
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        // Released where Undo was, then moved over where it is now, in the same frame.
        let (_, out) = ribbon_frames(
            0,
            &[
                (narrow, Vec::new()),
                (narrow, vec![egui::Event::PointerMoved(at)]),
                (narrow, vec![button(at, true)]),
                (wide, vec![button(at, false), egui::Event::PointerMoved(now)]),
            ],
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn enter_on_a_focused_ribbon_button_counts_beside_a_cancelled_press() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = ChromeState { ribbon_tab: 0 };
        let mut out = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let rects = ribbon_frame(&ctx, &mut state, 1210.0, Vec::new(), &mut out);
        let undo = undo_index();
        let at = rects[undo].center();
        let off = egui::pos2(at.x, 590.0);
        ctx.memory_mut(|m| m.request_focus(ribbon_id("Edit", Action::Undo)));
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let key = |pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut frame = |events: Vec<egui::Event>, out: &mut Vec<Action>| {
            ribbon_frame(&ctx, &mut state, 1210.0, events, out);
        };
        frame(vec![egui::Event::PointerMoved(at)], &mut out);
        frame(vec![button(at, true)], &mut out);
        // Dragged off and let go (no click), and Enter, in one frame: one Undo.
        frame(
            vec![egui::Event::PointerMoved(off), button(off, false), key(true)],
            &mut out,
        );
        frame(vec![key(false)], &mut out);
        assert_eq!(out, [Action::Undo]);
        // The same, with a press on blank space after Enter in that frame, which takes the
        // focus away: Enter counted before it did.
        out.clear();
        ctx.memory_mut(|m| m.request_focus(ribbon_id("Edit", Action::Undo)));
        frame(Vec::new(), &mut out);
        frame(vec![egui::Event::PointerMoved(at)], &mut out);
        frame(vec![button(at, true)], &mut out);
        frame(
            vec![
                egui::Event::PointerMoved(off),
                button(off, false),
                key(true),
                button(off, true),
            ],
            &mut out,
        );
        frame(vec![key(false), button(off, false)], &mut out);
        assert_eq!(out, [Action::Undo]);
    }

    #[test]
    fn a_tab_and_enter_beside_a_release_off_the_button_do_not_count_as_its_click() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = ChromeState { ribbon_tab: 0 };
        let mut out = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let (narrow, wide) = undo_moving_widths();
        let rects = ribbon_frame(&ctx, &mut state, narrow, Vec::new(), &mut out);
        let at = rects[undo_index()].center();
        ctx.memory_mut(|m| m.request_focus(ribbon_id("Edit", Action::Undo)));
        let key = |key, pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        ribbon_frame(&ctx, &mut state, narrow, vec![egui::Event::PointerMoved(at)], &mut out);
        ribbon_frame(&ctx, &mut state, narrow, vec![button(at, true)], &mut out);
        // The ribbon widens (Undo moves), and in that frame Tab takes the focus on, Enter is
        // pressed, and the pointer is let go where Undo was: not Undo's click.
        ribbon_frame(
            &ctx,
            &mut state,
            wide,
            vec![
                key(egui::Key::Tab, true),
                key(egui::Key::Enter, true),
                button(at, false),
            ],
            &mut out,
        );
        ribbon_frame(&ctx, &mut state, wide, vec![key(egui::Key::Enter, false)], &mut out);
        assert!(!out.contains(&Action::Undo), "{out:?}");
    }

    #[test]
    fn a_ribbon_click_is_judged_by_the_release_that_made_it() {
        let undo = undo_index();
        let (narrow, wide) = undo_moving_widths();
        let at = ribbon_icons(0, narrow)[undo].center();
        let now = ribbon_icons(0, wide)[undo].center();
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        // Released where Undo was (the click), then pressed again and released over where it
        // is now, all in one frame: the first release is off Undo.
        let (_, out) = ribbon_frames(
            0,
            &[
                (narrow, Vec::new()),
                (narrow, vec![egui::Event::PointerMoved(at)]),
                (narrow, vec![button(at, true)]),
                (
                    wide,
                    vec![
                        button(at, false),
                        egui::Event::PointerMoved(now),
                        button(now, true),
                        button(now, false),
                    ],
                ),
            ],
        );
        assert!(!out.contains(&Action::Undo), "{out:?}");
    }

    #[test]
    fn a_focused_ribbon_button_keeps_its_focus_when_the_groups_move() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = ChromeState { ribbon_tab: 0 };
        let mut out = Vec::new();
        let mut frame = |width: f32, events: Vec<egui::Event>, out: &mut Vec<Action>| {
            ribbon_frame(&ctx, &mut state, width, events, out);
        };
        let key = |pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(1210.0, Vec::new(), &mut out);
        ctx.memory_mut(|m| m.request_focus(ribbon_id("Edit", Action::Undo)));
        frame(1210.0, Vec::new(), &mut out);
        // Wider, the Edit group moves up a row; Enter still undoes.
        frame(1220.0, Vec::new(), &mut out);
        frame(1220.0, vec![key(true)], &mut out);
        frame(1220.0, vec![key(false)], &mut out);
        assert_eq!(out, [Action::Undo]);
    }

    #[test]
    fn a_ribbon_tab_shows_no_text_cursor_and_opens_on_click() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let mut state = ChromeState { ribbon_tab: 1 };
        let mut out = Vec::new();
        // Just inside the first tab's word: 8 points of panel margin and 4 of space before it.
        let at = egui::pos2(16.0, 12.0);
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let frame = |state: &mut ChromeState, out: &mut Vec<Action>, events| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1210.0, 600.0))),
                events,
                ..Default::default()
            };
            ctx.run(input, |ctx| {
                egui::TopBottomPanel::top("menus")
                    .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 4)))
                    .show(ctx, |ui| {
                        ribbon_rects(ui, state, &AllOn, out);
                    });
            })
            .platform_output
            .cursor_icon
        };
        frame(&mut state, &mut out, vec![egui::Event::PointerMoved(at)]);
        let cursor = frame(&mut state, &mut out, vec![egui::Event::PointerMoved(at)]);
        assert_ne!(cursor, egui::CursorIcon::Text, "a ribbon tab reads as text to select");
        frame(&mut state, &mut out, vec![press(true)]);
        frame(&mut state, &mut out, vec![press(false)]);
        assert_eq!(state.ribbon_tab, 0, "the point is not on the first tab");
    }

    #[test]
    fn every_ribbon_label_is_shown_whole() {
        // A label cut short can read the same as its neighbour's ("Move page..." for both Move
        // page up and Move page down), so each wraps under its button and shows every word.
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            for (tab, groups) in RIBBON {
                for a in groups.iter().flat_map(|(_, a)| a.iter()) {
                    let label = a.info().label;
                    let galley = ctx.fonts(|f| f.layout_job(ribbon_label(label, Color32::WHITE)));
                    let shown: String = galley
                        .rows
                        .iter()
                        .flat_map(|r| r.glyphs.iter().map(|g| g.chr))
                        .filter(|c| !c.is_whitespace())
                        .collect();
                    let whole: String = label.chars().filter(|c| !c.is_whitespace()).collect();
                    assert_eq!(shown, whole, "{tab}: {label:?} is cut short");
                    assert!(
                        galley.size().x <= RIBBON_BUTTON + 0.5,
                        "{tab}: {label:?} is wider than its button"
                    );
                }
            }
        });
    }

    #[test]
    fn a_remembered_ribbon_tab_opens_again_and_one_out_of_range_opens_the_first() {
        assert_eq!(ChromeState::with_ribbon_tab(1).ribbon_tab, 1);
        assert_eq!(
            ChromeState::with_ribbon_tab(RIBBON.len() - 1).ribbon_tab,
            RIBBON.len() - 1
        );
        assert_eq!(ChromeState::with_ribbon_tab(RIBBON.len()).ribbon_tab, 0);
        assert_eq!(ChromeState::with_ribbon_tab(usize::MAX).ribbon_tab, 0);
    }

    #[test]
    fn every_ribbon_button_fits_the_smallest_window() {
        // 900 points is the window's minimum width: every button of every tab stays on screen.
        for (tab, (name, groups)) in RIBBON.iter().enumerate() {
            let rects = ribbon_icons(tab, 900.0);
            let buttons: usize = groups.iter().map(|(_, a)| a.len()).sum();
            assert_eq!(rects.len(), buttons, "{name}");
            for (i, r) in rects.iter().enumerate() {
                assert!(r.left() >= 0.0 && r.right() <= 900.0, "{name}: button {i} at {r:?}");
                for (j, o) in rects.iter().enumerate().skip(i + 1) {
                    assert!(
                        !r.shrink(0.5).intersects(o.shrink(0.5)),
                        "{name}: buttons {i} and {j} overlap: {r:?} {o:?}"
                    );
                }
            }
        }
        // The Start tab takes more than one row at that width, and one row in a wide window.
        let start = ribbon_icons(0, 900.0);
        assert!(start.iter().any(|r| r.top() > start[0].bottom()), "wraps at 900");
        let wide = ribbon_icons(0, 2400.0);
        assert!(
            wide.iter().all(|r| (r.center().y - wide[0].center().y).abs() < 1.0),
            "one row"
        );
        // Even a window narrower than the widest group keeps that group's buttons on screen.
        let narrow = ribbon_icons(0, 400.0);
        assert_eq!(narrow.len(), RIBBON[0].1.iter().map(|(_, a)| a.len()).sum::<usize>());
        for r in narrow {
            assert!(r.left() >= 0.0 && r.right() <= 400.0, "{r:?}");
        }
    }

    #[test]
    fn the_classic_toolbar_fits_the_smallest_window() {
        // 900 points is the window's minimum width: every button stays on screen.
        let rects = toolbar_rects(900.0);
        assert_eq!(rects.len(), TOOLBAR.len() + 1);
        for (i, r) in rects.iter().enumerate() {
            assert!(r.left() >= 0.0 && r.right() <= 900.0, "item {i} at {r:?}");
            for (j, o) in rects.iter().enumerate().skip(i + 1) {
                assert!(
                    !r.shrink(0.5).intersects(o.shrink(0.5)),
                    "items {i} and {j} overlap: {r:?} {o:?}"
                );
            }
        }
        // It takes more than one row at that width; a wide window shows one row.
        assert!(rects.iter().any(|r| r.top() > rects[1].bottom()), "wraps at 900");
        let wide = toolbar_rects(2400.0);
        assert!(
            wide.iter().all(|r| (r.center().y - wide[1].center().y).abs() < 1.0),
            "one row"
        );
    }

    #[test]
    fn no_two_classic_toolbar_buttons_share_an_icon() {
        // The classic toolbar shows icons without labels: each must say which command it is.
        let mut buttons: Vec<Action> = TOOLBAR.iter().flat_map(|g| g.iter().copied()).collect();
        buttons.extend([Action::ToggleChrome, Action::ToggleTheme]);
        for (i, a) in buttons.iter().enumerate() {
            for b in &buttons[i + 1..] {
                assert_ne!(a.info().icon, b.info().icon, "{a:?} and {b:?} look the same");
            }
        }
    }
}

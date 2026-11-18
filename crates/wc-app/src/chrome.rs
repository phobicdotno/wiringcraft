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

#[derive(Default)]
pub struct ChromeState {
    pub ribbon_tab: usize,
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
            None,
            Some(Action::Check),
            Some(Action::Reports),
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
            Some(Action::SelectAll),
            Some(Action::Find),
            None,
            Some(Action::Rotate),
            Some(Action::Mirror),
            Some(Action::Delete),
            None,
            Some(Action::NumberWires),
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
            Some(Action::PageProps),
            None,
            Some(Action::PageUp),
            Some(Action::PageDown),
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
                                .shortcut_text(info.shortcut)
                                .selected(status.active(*a));
                            if ui
                                .add_enabled(status.enabled(*a), button)
                                .on_hover_text(info.tip)
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
    icon_button_rect(ui, a, size, status, t).0
}

/// [`icon_button`], also returning where the button is.
fn icon_button_rect(ui: &mut egui::Ui, a: Action, size: f32, status: &dyn Status, t: &Theme) -> (bool, Rect) {
    let info = a.info();
    let enabled = status.enabled(a);
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), if enabled { Sense::click() } else { Sense::hover() });
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
        format!("{}\n{}", info.label, info.tip)
    } else {
        format!("{} ({})\n{}", info.label, info.shortcut, info.tip)
    };
    (resp.on_hover_text(tip).clicked(), rect)
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
            let r = ui.add(egui::Label::new(text).sense(Sense::click()));
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

/// A ribbon button's width, its icon and label.
const RIBBON_BUTTON: f32 = 66.0;
/// The room a divider takes between two ribbon groups.
const RIBBON_DIVIDER: f32 = 9.0;

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
        ui.horizontal(|ui| {
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
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                for a in line {
                                    let r = ui.push_id((gname, *a), |ui| {
                                        ui.allocate_ui_with_layout(
                                            Vec2::new(RIBBON_BUTTON, 52.0),
                                            egui::Layout::top_down(egui::Align::Center),
                                            |ui| {
                                                ui.set_width(RIBBON_BUTTON);
                                                ui.spacing_mut().item_spacing.y = 1.0;
                                                let (clicked, icon) = icon_button_rect(ui, *a, 34.0, status, &t);
                                                if clicked {
                                                    out.push(*a);
                                                }
                                                let c = if status.enabled(*a) {
                                                    t.text
                                                } else {
                                                    t.muted.gamma_multiply(0.6)
                                                };
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(a.info().label).size(11.0).color(c),
                                                    )
                                                    .wrap_mode(egui::TextWrapMode::Truncate),
                                                );
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
        let input = |width: f32, events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(width, 600.0))),
            events,
            ..Default::default()
        };
        // The first frame loads the fonts.
        let _ = ctx.run(input(frames[0].0, Vec::new()), |_| {});
        for (width, events) in frames {
            let _ = ctx.run(input(*width, events.clone()), |ctx| {
                egui::TopBottomPanel::top("menus")
                    .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 4)))
                    .show(ctx, |ui| {
                        rects = ribbon_rects(ui, &mut state, &AllOn, &mut out);
                    });
            });
        }
        (rects, out)
    }

    /// The icon rectangles of ribbon tab `tab` in a window `width` points wide.
    fn ribbon_icons(tab: usize, width: f32) -> Vec<Rect> {
        ribbon_frames(tab, &[(width, Vec::new())]).0
    }

    #[test]
    fn a_ribbon_press_stays_with_its_button_when_the_groups_move() {
        let start = RIBBON[0].1;
        let order: Vec<Action> = start.iter().flat_map(|(_, a)| a.iter().copied()).collect();
        let undo = order.iter().position(|a| *a == Action::Undo).unwrap_or(usize::MAX);
        // Two window widths 10 points apart where the groups wrap differently, so that in the
        // wider window another button sits where Undo was (found, so new buttons do not break
        // the test).
        let other_at = |narrow: f32| {
            let at = ribbon_icons(0, narrow).get(undo)?.center();
            let moved = ribbon_icons(0, narrow + 10.0);
            let there = moved.iter().position(|r| r.contains(at)).map(|i| order[i])?;
            (there != Action::Undo).then_some((narrow, at, there))
        };
        let found = (100..250).map(|k| k as f32 * 10.0).find_map(other_at);
        assert!(found.is_some(), "no width moves the Undo button");
        let Some((narrow, at, there)) = found else { return };
        let wide = narrow + 10.0;
        let there = Some(there);
        assert!(there.is_some_and(|a| a != Action::Undo), "{there:?} at {at:?}");
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let (_, out) = ribbon_frames(
            0,
            &[
                (narrow, Vec::new()),
                (narrow, vec![egui::Event::PointerMoved(at)]),
                (narrow, vec![button(true)]),
                (wide, vec![button(false)]),
            ],
        );
        assert!(out.is_empty(), "{out:?}");
        // Released where it was pressed, the press is a click on Undo.
        let (_, out) = ribbon_frames(
            0,
            &[
                (narrow, Vec::new()),
                (narrow, vec![egui::Event::PointerMoved(at)]),
                (narrow, vec![button(true)]),
                (narrow, vec![button(false)]),
            ],
        );
        assert_eq!(out, [Action::Undo]);
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
}

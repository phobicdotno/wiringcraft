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
            None,
            Some(Action::ProjectProps),
            None,
            Some(Action::Check),
            Some(Action::Reports),
            Some(Action::ExportPdf),
            Some(Action::ExportDrawings),
            Some(Action::ExportAml),
            Some(Action::ExportWagoMarking),
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
    ("Library", &[Some(Action::LibraryStatus), Some(Action::LibraryUpdate)]),
    (
        "Versions",
        &[
            Some(Action::Commit),
            Some(Action::NewBranch),
            Some(Action::History),
            None,
            Some(Action::CommentTool),
            Some(Action::PageHistory),
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
    resp.on_hover_text(tip).clicked()
}

fn divider(ui: &mut egui::Ui, t: &Theme, height: f32) {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(1.0, height), Sense::hover());
    ui.painter()
        .line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0_f32, t.border));
    ui.add_space(4.0);
}

pub fn classic_toolbar(ui: &mut egui::Ui, status: &dyn Status, out: &mut Vec<Action>) {
    let t = Theme::of(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, group) in TOOLBAR.iter().enumerate() {
            if i > 0 {
                divider(ui, &t, 20.0);
            }
            for a in *group {
                if icon_button(ui, *a, 30.0, status, &t) {
                    out.push(*a);
                }
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for a in [Action::ToggleChrome, Action::ToggleTheme] {
                if icon_button(ui, a, 30.0, status, &t) {
                    out.push(a);
                }
            }
        });
    });
}

pub fn ribbon(ui: &mut egui::Ui, state: &mut ChromeState, status: &dyn Status, out: &mut Vec<Action>) {
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
        return;
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (gi, (gname, actions)) in groups.iter().enumerate() {
            if gi > 0 {
                divider(ui, &t, 62.0);
            }
            let width = actions.len() as f32 * 66.0;
            ui.allocate_ui_with_layout(
                Vec2::new(width, 70.0),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for a in *actions {
                            ui.allocate_ui_with_layout(
                                Vec2::new(66.0, 52.0),
                                egui::Layout::top_down(egui::Align::Center),
                                |ui| {
                                    ui.set_width(66.0);
                                    ui.spacing_mut().item_spacing.y = 1.0;
                                    if icon_button(ui, *a, 34.0, status, &t) {
                                        out.push(*a);
                                    }
                                    let c = if status.enabled(*a) {
                                        t.text
                                    } else {
                                        t.muted.gamma_multiply(0.6)
                                    };
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(a.info().label).size(11.0).color(c))
                                            .wrap_mode(egui::TextWrapMode::Truncate),
                                    );
                                },
                            );
                        }
                    });
                    ui.label(egui::RichText::new(gname.to_uppercase()).size(9.5).color(t.muted));
                },
            );
        }
    });
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

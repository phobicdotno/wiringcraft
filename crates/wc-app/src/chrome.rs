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
            Some(Action::ExportPdf),
        ],
    ),
    (
        "Edit",
        &[
            Some(Action::Undo),
            Some(Action::Redo),
            None,
            Some(Action::Rotate),
            Some(Action::Delete),
        ],
    ),
    (
        "Insert",
        &[Some(Action::Select), Some(Action::PlaceSymbol), Some(Action::DrawWire)],
    ),
    (
        "Page",
        &[
            Some(Action::AddPage),
            Some(Action::DuplicatePage),
            Some(Action::PageProps),
            None,
            Some(Action::PageUp),
            Some(Action::PageDown),
            None,
            Some(Action::DeletePage),
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
        ],
    ),
    ("Library", &[Some(Action::LibraryStatus), Some(Action::LibraryUpdate)]),
    (
        "Versions",
        &[Some(Action::Commit), Some(Action::NewBranch), Some(Action::History)],
    ),
];

pub fn menu_bar(ui: &mut egui::Ui, status: &dyn Status, out: &mut Vec<Action>) {
    egui::menu::bar(ui, |ui| {
        for (name, items) in MENUS {
            ui.menu_button(*name, |ui| {
                ui.set_min_width(220.0);
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
    if status.active(a) {
        p.rect_filled(rect, CornerRadius::same(3), t.icon_active_bg);
        p.rect_stroke(
            rect,
            CornerRadius::same(3),
            Stroke::new(1.0_f32, t.selection.gamma_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
    } else if resp.hovered() && enabled {
        p.rect_filled(rect, CornerRadius::same(3), ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let c = if enabled { t.icon } else { t.icon.gamma_multiply(0.35) };
    icons::paint(p, rect.shrink(size * 0.15), info.icon, c);
    let tip = if info.shortcut.is_empty() {
        format!("{}\n{}", info.label, info.tip)
    } else {
        format!("{} ({})\n{}", info.label, info.shortcut, info.tip)
    };
    resp.on_hover_text(tip).clicked()
}

pub fn classic_toolbar(ui: &mut egui::Ui, status: &dyn Status, out: &mut Vec<Action>) {
    let t = Theme::of(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, group) in TOOLBAR.iter().enumerate() {
            if i > 0 {
                ui.add_space(3.0);
                let (r, _) = ui.allocate_exact_size(Vec2::new(1.0, 22.0), Sense::hover());
                ui.painter().line_segment(
                    [r.center_top(), r.center_bottom()],
                    Stroke::new(1.0_f32, t.faint.gamma_multiply(0.5)),
                );
                ui.add_space(3.0);
            }
            for a in *group {
                if icon_button(ui, *a, 26.0, status, &t) {
                    out.push(*a);
                }
            }
        }
    });
}

pub fn ribbon(ui: &mut egui::Ui, state: &mut ChromeState, status: &dyn Status, out: &mut Vec<Action>) {
    let t = Theme::of(ui.ctx());
    ui.horizontal(|ui| {
        for (i, (name, _)) in RIBBON.iter().enumerate() {
            if ui.selectable_label(state.ribbon_tab == i, *name).clicked() {
                state.ribbon_tab = i;
            }
        }
    });
    let Some((_, groups)) = RIBBON.get(state.ribbon_tab) else {
        return;
    };
    ui.horizontal(|ui| {
        for (gi, (gname, actions)) in groups.iter().enumerate() {
            if gi > 0 {
                ui.separator();
            }
            let width = actions.len() as f32 * 58.0;
            ui.allocate_ui_with_layout(
                Vec2::new(width, 66.0),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        for a in *actions {
                            ui.allocate_ui_with_layout(
                                Vec2::new(54.0, 52.0),
                                egui::Layout::top_down(egui::Align::Center),
                                |ui| {
                                    ui.set_width(54.0);
                                    if icon_button(ui, *a, 34.0, status, &t) {
                                        out.push(*a);
                                    }
                                    ui.label(egui::RichText::new(a.info().label).small());
                                },
                            );
                        }
                    });
                    ui.label(egui::RichText::new(*gname).small().color(t.faint));
                },
            );
        }
    });
}

/// The whole command area under the window title, in the chosen layout.
pub fn show(ctx: &egui::Context, chrome: Chrome, state: &mut ChromeState, status: &dyn Status) -> Vec<Action> {
    let mut out = Vec::new();
    let t = Theme::of(ctx);
    egui::TopBottomPanel::top("menus").show(ctx, |ui| menu_bar(ui, status, &mut out));
    egui::TopBottomPanel::top("commands")
        .frame(
            egui::Frame::new()
                .fill(t.toolbar_bg)
                .inner_margin(egui::Margin::symmetric(6, 4)),
        )
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

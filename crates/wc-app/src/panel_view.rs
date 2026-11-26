//! The panel layout view: the mounting plate with its ducts, DIN rails and device footprints,
//! drawn to scale. Devices are picked and dragged with the primary button, and the arrow keys move
//! the picked device by one step; rails and ducts are picked with it, so Delete takes them off;
//! the wheel zooms and the middle or secondary button pans, as on the schematic canvas.

use eframe::egui::{self, Align2, FontId, Rect, Sense, Stroke, Vec2};
use wc_core::Point;
use wc_core::panel::PanelItem;

use crate::canvas::{Editor, Xf};
use crate::state::Open;
use crate::theme;

/// Which device item lies under a plate position, topmost first.
fn device_at(items: &[PanelItem], at: Point) -> Option<usize> {
    items.iter().enumerate().rev().find_map(|(i, item)| {
        let (a, b) = item.device_bounds()?;
        (at.x >= a.x && at.x <= b.x && at.y >= a.y && at.y <= b.y).then_some(i)
    })
}

/// Where a device at `at`, `size` big as turned, goes when an arrow key moves it `step` mm in
/// the direction (`dx`, `dy`): kept on a plate `plate` big, as a drag keeps it.
fn nudged(at: Point, size: (f64, f64), plate: (f64, f64), step: f64, dx: f64, dy: f64) -> Point {
    let (w, h) = size;
    let (pw, ph) = plate;
    Point::new(
        (at.x + dx * step).clamp(0.0, (pw - w).max(0.0)),
        (at.y + dy * step).clamp(0.0, (ph - h).max(0.0)),
    )
}

/// Which item lies under a plate position: a device first, as they are drawn on top, then a
/// rail, then a duct, topmost first within each.
fn item_at(items: &[PanelItem], at: Point) -> Option<usize> {
    let under = |i: usize, item: &PanelItem| {
        let (a, b) = item.bounds();
        (at.x >= a.x && at.x <= b.x && at.y >= a.y && at.y <= b.y).then_some(i)
    };
    device_at(items, at)
        .or_else(|| {
            let rails = items.iter().enumerate().rev();
            rails
                .filter(|(_, it)| matches!(it, PanelItem::Rail { .. }))
                .find_map(|(i, it)| under(i, it))
        })
        .or_else(|| {
            let ducts = items.iter().enumerate().rev();
            ducts
                .filter(|(_, it)| matches!(it, PanelItem::Duct { .. }))
                .find_map(|(i, it)| under(i, it))
        })
}

/// The panel view's zoom limits, in screen points per millimetre: lower than the schematic's,
/// so a large plate fits.
pub const ZOOM_LIMITS: (f32, f32) = (0.05, 20.0);

/// `keyboard_edits` is false while something else has the keyboard, such as an app dialog: the
/// arrow keys then leave the picked device where it is.
pub fn show(ui: &mut egui::Ui, open: &mut Open, ed: &mut Editor, panel: usize, snap_step: f64, keyboard_edits: bool) {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    ed.view.size = rect.size();
    let painter = ui.painter_at(rect);
    let t = theme::Theme::of(ui.ctx());
    painter.rect_filled(rect, 0.0, t.backdrop);
    let Some(p) = open.project.panels.get(panel).cloned() else {
        ed.panel = None;
        return;
    };
    let (pw, ph) = (p.width.max(1.0), p.height.max(1.0));
    if ed.view.fit {
        let z = ((rect.width() - 64.0) / pw as f32)
            .min((rect.height() - 72.0) / ph as f32)
            .max(0.05);
        ed.view.zoom = z;
        ed.view.offset = Vec2::new(
            (rect.width() - pw as f32 * z) / 2.0,
            (rect.height() - ph as f32 * z) / 2.0 + 6.0,
        );
        ed.view.fit = false;
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.zoom_delta().ln() * 200.0);
        if scroll.abs() > 0.0
            && let Some(hp) = resp.hover_pos()
        {
            ed.view.zoom_about(hp - rect.min, (scroll / 300.0).exp(), ZOOM_LIMITS);
        }
    }
    if resp.dragged_by(egui::PointerButton::Middle) || resp.dragged_by(egui::PointerButton::Secondary) {
        ed.view.offset += resp.drag_delta();
    }
    let xf = Xf {
        origin: rect.min + ed.view.offset,
        zoom: ed.view.zoom,
    };

    // Pick and drag devices.
    let pointer = resp.interact_pointer_pos().or(resp.hover_pos()).map(|q| xf.mm(q));
    ed.cursor = pointer;
    // A drag starts where the button went down, which can be some way from where the pointer
    // is by the time the movement counts as a drag.
    let pressed_at = ui.input(|i| i.pointer.press_origin()).map(|q| xf.mm(q));
    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(at) = pressed_at.or(pointer)
    {
        ed.panel_pick = item_at(&p.items, at);
        if let Some(i) = ed.panel_pick
            && let Some(PanelItem::Device { at: a, .. }) = p.items.get(i)
        {
            ed.panel_drag = Some((Point::new(at.x - a.x, at.y - a.y), false));
        }
    }
    if resp.clicked_by(egui::PointerButton::Primary) {
        ed.panel_pick = pointer.and_then(|at| item_at(&p.items, at));
    }
    if let (Some((grab, checkpoint)), Some(i), Some(at)) = (ed.panel_drag, ed.panel_pick, pointer)
        && resp.dragged_by(egui::PointerButton::Primary)
    {
        let step = if snap_step > 0.0 { snap_step.min(5.0) } else { 1.0 };
        let snap = |v: f64| (v / step).round() * step;
        let mut to = Point::new(snap(at.x - grab.x), snap(at.y - grab.y));
        // Near a DIN rail, the device clips onto it: its middle goes on the rail's centre line.
        if let Some((a, b)) = p.items.get(i).and_then(PanelItem::device_bounds) {
            // The outline as turned on the plate.
            let height = b.y - a.y;
            let middle = to.y + height / 2.0;
            let rail = p.items.iter().find_map(|r| match r {
                PanelItem::Rail { at: ra, length }
                    if (ra.y - middle).abs() < 20.0 && at.x >= ra.x && at.x <= ra.x + length =>
                {
                    Some(ra.y)
                }
                _ => None,
            });
            if let Some(y) = rail {
                to.y = y - height / 2.0;
            }
        }
        let moved = matches!(p.items.get(i), Some(PanelItem::Device { at: a, .. }) if *a != to);
        if moved {
            if !checkpoint {
                open.checkpoint();
                ed.panel_drag = Some((grab, true));
            }
            if let Some(PanelItem::Device {
                at: a,
                width,
                height,
                rotation,
                ..
            }) = open.project.panels.get_mut(panel).and_then(|x| x.items.get_mut(i))
            {
                // Keep the device, as turned, on the plate.
                let (w, h) = wc_core::panel::turned_size(*width, *height, *rotation);
                *a = Point::new(to.x.clamp(0.0, (pw - w).max(0.0)), to.y.clamp(0.0, (ph - h).max(0.0)));
                open.edited();
            }
        }
    }
    if resp.drag_stopped() {
        ed.panel_drag = None;
    }
    // The arrow keys move the picked device by one step each press, one undo step a press, kept
    // on the plate as a drag keeps it. The keys are used up here, so nothing else (such as the
    // schematic page behind this view) acts on them too. Not while a dialog is open.
    if keyboard_edits
        && ed.panel_drag.is_none()
        && !ui.ctx().wants_keyboard_input()
        && let Some(i) = ed.panel_pick
        && matches!(p.items.get(i), Some(PanelItem::Device { .. }))
    {
        let step = if snap_step > 0.0 { snap_step.min(5.0) } else { 1.0 };
        // The presses in the order they came, so Right then Left ends where it started.
        let mut presses: Vec<(f64, f64)> = Vec::new();
        ui.input_mut(|inp| {
            inp.events.retain(|e| {
                let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = e
                else {
                    return true;
                };
                let d = match key {
                    egui::Key::ArrowLeft => (-1.0, 0.0),
                    egui::Key::ArrowRight => (1.0, 0.0),
                    egui::Key::ArrowUp => (0.0, -1.0),
                    egui::Key::ArrowDown => (0.0, 1.0),
                    _ => return true,
                };
                if !modifiers.is_none() {
                    return true;
                }
                presses.push(d);
                false
            });
        });
        for (dx, dy) in presses {
            let Some(PanelItem::Device {
                at,
                width,
                height,
                rotation,
                ..
            }) = open.project.panels.get(panel).and_then(|x| x.items.get(i))
            else {
                break;
            };
            let size = wc_core::panel::turned_size(*width, *height, *rotation);
            let to = nudged(*at, size, (pw, ph), step, dx, dy);
            if to == *at {
                continue;
            }
            open.checkpoint();
            if let Some(PanelItem::Device { at, .. }) =
                open.project.panels.get_mut(panel).and_then(|x| x.items.get_mut(i))
            {
                *at = to;
            }
            open.edited();
        }
    }
    let p = open.project.panels.get(panel).cloned().unwrap_or(p);

    // The plate.
    let plate = Rect::from_two_pos(xf.s(Point::new(0.0, 0.0)), xf.s(Point::new(pw, ph)));
    painter.add(
        egui::epaint::Shadow {
            offset: [0, 6],
            blur: 24,
            spread: 0,
            color: t.shadow,
        }
        .as_shape(plate, 2.0),
    );
    painter.rect_filled(plate, 2.0, t.paper);
    if plate.top() - rect.top() > 22.0 {
        painter.text(
            plate.left_top() + Vec2::new(0.0, -8.0),
            Align2::LEFT_BOTTOM,
            format!("Panel  ·  {}  ·  {} x {} mm", p.name, p.width, p.height),
            theme::strong(12.0),
            t.muted,
        );
    }
    let hair = Stroke::new(1.0_f32, t.faint);
    let picked_stroke = Stroke::new(2.0_f32, t.selection);
    for (i, item) in p.items.iter().enumerate() {
        let picked = ed.panel_pick == Some(i);
        match item {
            PanelItem::Duct { min, max } => {
                let r = Rect::from_two_pos(xf.s(*min), xf.s(*max));
                painter.rect_filled(r, 0.0, t.grid.gamma_multiply(0.6));
                painter.rect_stroke(
                    r,
                    0.0,
                    if picked { picked_stroke } else { hair },
                    egui::StrokeKind::Inside,
                );
                // Slots across the duct.
                let vertical = (max.y - min.y) > (max.x - min.x);
                let (len, along) = if vertical {
                    (max.y - min.y, 0)
                } else {
                    (max.x - min.x, 1)
                };
                let mut d = 15.0;
                while d < len && xf.zoom * 15.0 > 4.0 {
                    let (a, b) = if along == 0 {
                        (Point::new(min.x + 8.0, min.y + d), Point::new(max.x - 8.0, min.y + d))
                    } else {
                        (Point::new(min.x + d, min.y + 8.0), Point::new(min.x + d, max.y - 8.0))
                    };
                    painter.line_segment([xf.s(a), xf.s(b)], hair);
                    d += 15.0;
                }
            }
            PanelItem::Rail { at, length } => {
                let st = Stroke::new(1.0_f32, t.muted);
                let (a, b) = item.bounds();
                let r = Rect::from_two_pos(xf.s(a), xf.s(b));
                if picked {
                    painter.rect_filled(r, 0.0, t.accent_soft);
                }
                painter.rect_stroke(
                    r,
                    0.0,
                    if picked { picked_stroke } else { st },
                    egui::StrokeKind::Inside,
                );
                for dy in [-12.5, 12.5] {
                    painter.line_segment(
                        [
                            xf.s(Point::new(at.x, at.y + dy)),
                            xf.s(Point::new(at.x + length, at.y + dy)),
                        ],
                        st,
                    );
                }
            }
            PanelItem::Device { .. } => {}
        }
    }
    for (i, item) in p.items.iter().enumerate() {
        if let PanelItem::Device {
            tag,
            at,
            width,
            height,
            rotation,
        } = item
        {
            let (w, h) = wc_core::panel::turned_size(*width, *height, *rotation);
            let r = Rect::from_two_pos(xf.s(*at), xf.s(Point::new(at.x + w, at.y + h)));
            let picked = ed.panel_pick == Some(i);
            painter.rect_filled(r, 1.0, if picked { t.accent_soft } else { t.paper });
            // A part with a maker's outline drawing is drawn with it, inside a faint box (#312).
            let footprint = open
                .project
                .devices
                .get(tag)
                .and_then(|d| d.part.as_ref())
                .and_then(|id| open.parts.get(id))
                .and_then(|part| part.footprint.as_ref());
            if let Some(f) = footprint {
                let st = Stroke::new(1.0_f32, t.ink);
                for s in f.placed_turned(*at, *width, *height, *rotation) {
                    crate::canvas::paint_shape(&painter, &xf, &s, st);
                }
            }
            painter.rect_stroke(
                r,
                1.0,
                Stroke::new(
                    if picked { 2.0_f32 } else { 1.2 },
                    if picked {
                        t.selection
                    } else if footprint.is_some() {
                        t.faint
                    } else {
                        t.ink
                    },
                ),
                egui::StrokeKind::Inside,
            );
            let label = format!("-{}", tag.product.join("."));
            let size = (r.width() / (label.chars().count().max(1) as f32 * 0.62)).clamp(6.0, 14.0);
            if size * 0.62 * label.chars().count() as f32 <= r.width() + 1.0 {
                painter.text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    &label,
                    FontId::proportional(size),
                    t.ink,
                );
            }
        }
    }
    // The picked item's full tag or kind, size and place, at the bottom of the view.
    let info = ed.panel_pick.and_then(|i| p.items.get(i)).map(|item| match item {
        PanelItem::Device {
            tag,
            at,
            width,
            height,
            rotation,
        } => format!(
            "{tag}  ·  {width} x {height} mm{} at x {:.0}, y {:.0}  ·  drag to move, R to rotate, Delete to take it off the plate",
            turned_note(*rotation),
            at.x,
            at.y
        ),
        PanelItem::Rail { at, length } => format!(
            "DIN rail  ·  {length:.0} mm long at x {:.0}, y {:.0}  ·  Delete to take it off the plate",
            at.x, at.y
        ),
        PanelItem::Duct { .. } => {
            let (a, b) = item.bounds();
            format!(
                "Wire duct  ·  {:.0} x {:.0} mm at x {:.0}, y {:.0}  ·  Delete to take it off the plate",
                b.x - a.x,
                b.y - a.y,
                a.x,
                a.y
            )
        }
    });
    let hint = info.unwrap_or_else(|| {
        let n = p.device_tags().len();
        format!("{n} device(s) on the plate  ·  Page menu → Arrange devices puts the rest on the rails")
    });
    painter.text(
        rect.center_bottom() + Vec2::new(0.0, -12.0),
        Align2::CENTER_BOTTOM,
        hint,
        FontId::proportional(12.0),
        t.muted,
    );
}

/// ", turned 90°" for a turned device, nothing for an upright one.
pub fn turned_note(rotation: wc_core::geom::Rotation) -> String {
    use wc_core::geom::Rotation;
    match rotation {
        Rotation::R0 => String::new(),
        Rotation::R90 => ", turned 90°".into(),
        Rotation::R180 => ", turned 180°".into(),
        Rotation::R270 => ", turned 270°".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In the panel view Zoom out uses the panel view's limits, below the schematic's, and
    /// keeps the centre of the view in place (#826).
    #[test]
    fn zoom_keys_use_the_panel_limits() {
        let mut ed = Editor::default();
        ed.panel = Some(0);
        ed.view = crate::canvas::View {
            zoom: 0.1,
            offset: Vec2::new(100.0, 50.0),
            fit: false,
            size: Vec2::new(1000.0, 700.0),
        };
        let centre = |ed: &Editor| (ed.view.size / 2.0 - ed.view.offset) / ed.view.zoom;
        let before = centre(&ed);
        ed.zoom_keys(0.8);
        assert!((ed.view.zoom - 0.08).abs() < 1e-6, "{}", ed.view.zoom);
        assert!((centre(&ed) - before).length() < 1e-2, "{:?} {before:?}", centre(&ed));
        ed.view.zoom = 0.05;
        ed.zoom_keys(0.8);
        assert!((ed.view.zoom - 0.05).abs() < 1e-6, "{}", ed.view.zoom);
    }

    #[test]
    fn picks_the_topmost_device() {
        let dev = |x: f64| PanelItem::Device {
            tag: wc_core::DeviceTag::parse("-K1").unwrap_or_default(),
            at: Point::new(x, 0.0),
            width: 10.0,
            height: 10.0,
            rotation: wc_core::geom::Rotation::R0,
        };
        let items = vec![
            PanelItem::Rail {
                at: Point::new(0.0, 5.0),
                length: 100.0,
            },
            dev(0.0),
            dev(5.0),
        ];
        assert_eq!(device_at(&items, Point::new(7.0, 5.0)), Some(2));
        assert_eq!(device_at(&items, Point::new(2.0, 5.0)), Some(1));
        assert_eq!(device_at(&items, Point::new(50.0, 5.0)), None);
    }

    #[test]
    fn arrow_keys_move_a_device_one_step_and_keep_it_on_the_plate() {
        let at = Point::new(100.0, 50.0);
        assert_eq!(
            nudged(at, (40.0, 60.0), (400.0, 300.0), 5.0, 1.0, 0.0),
            Point::new(105.0, 50.0)
        );
        assert_eq!(
            nudged(at, (40.0, 60.0), (400.0, 300.0), 2.5, 0.0, -1.0),
            Point::new(100.0, 47.5)
        );
        // At the plate's edges it stays on the plate.
        let corner = Point::new(0.0, 240.0);
        assert_eq!(nudged(corner, (40.0, 60.0), (400.0, 300.0), 5.0, -1.0, 0.0), corner);
        assert_eq!(nudged(corner, (40.0, 60.0), (400.0, 300.0), 5.0, 0.0, 1.0), corner);
        assert_eq!(
            nudged(Point::new(358.0, 0.0), (40.0, 60.0), (400.0, 300.0), 5.0, 1.0, 0.0),
            Point::new(360.0, 0.0)
        );
    }

    #[test]
    fn arrow_keys_in_the_panel_view_move_the_picked_device() {
        let (_tmp, mut open) = crate::state::tests::project();
        open.project.panels.push(wc_core::panel::Panel {
            id: "panel1".into(),
            name: "Panel 1".into(),
            width: 400.0,
            height: 300.0,
            items: vec![PanelItem::Device {
                tag: wc_core::DeviceTag::parse("-K1").unwrap_or_default(),
                at: Point::new(100.0, 100.0),
                width: 40.0,
                height: 60.0,
                rotation: wc_core::geom::Rotation::R0,
            }],
        });
        let mut ed = Editor::default();
        ed.panel = Some(0);
        ed.panel_pick = Some(0);
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let input = egui::RawInput {
            events: vec![
                key(egui::Key::ArrowRight),
                key(egui::Key::ArrowRight),
                key(egui::Key::ArrowUp),
            ],
            ..Default::default()
        };
        let steps = open.undo_steps();
        let mut left = 0;
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut open, &mut ed, 0, 2.5, true));
            // The keys are used up: nothing after the panel view sees them.
            left = ctx.input(|i| i.events.len());
        });
        assert_eq!(left, 0);
        let Some(PanelItem::Device { at, .. }) = open.project.panels[0].items.first() else {
            unreachable!("the device is there")
        };
        assert_eq!(*at, Point::new(105.0, 97.5));
        // One undo step a press.
        assert_eq!(open.undo_steps(), steps + 3);
        for _ in 0..3 {
            assert!(open.undo());
        }
        let Some(PanelItem::Device { at, .. }) = open.project.panels[0].items.first() else {
            unreachable!("the device is there")
        };
        assert_eq!(*at, Point::new(100.0, 100.0));
    }

    #[test]
    fn arrow_keys_in_one_frame_move_the_device_in_the_order_pressed() {
        let (_tmp, mut open) = crate::state::tests::project();
        open.project.panels.push(wc_core::panel::Panel {
            id: "panel1".into(),
            name: "Panel 1".into(),
            width: 400.0,
            height: 300.0,
            items: vec![PanelItem::Device {
                tag: wc_core::DeviceTag::parse("-K1").unwrap_or_default(),
                at: Point::new(0.0, 0.0),
                width: 40.0,
                height: 60.0,
                rotation: wc_core::geom::Rotation::R0,
            }],
        });
        let mut ed = Editor::default();
        ed.panel = Some(0);
        ed.panel_pick = Some(0);
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let input = egui::RawInput {
            events: vec![key(egui::Key::ArrowRight), key(egui::Key::ArrowLeft)],
            ..Default::default()
        };
        let steps = open.undo_steps();
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut open, &mut ed, 0, 5.0, true));
        });
        // Right moves it off the edge, then Left brings it back: two undo steps.
        let Some(PanelItem::Device { at, .. }) = open.project.panels[0].items.first() else {
            unreachable!("the device is there")
        };
        assert_eq!(*at, Point::new(0.0, 0.0));
        assert_eq!(open.undo_steps(), steps + 2);
    }

    #[test]
    fn arrow_keys_leave_the_picked_device_alone_while_a_dialog_is_open() {
        let (_tmp, mut open) = crate::state::tests::project();
        open.project.panels.push(wc_core::panel::Panel {
            id: "panel1".into(),
            name: "Panel 1".into(),
            width: 400.0,
            height: 300.0,
            items: vec![PanelItem::Device {
                tag: wc_core::DeviceTag::parse("-K1").unwrap_or_default(),
                at: Point::new(100.0, 100.0),
                width: 40.0,
                height: 60.0,
                rotation: wc_core::geom::Rotation::R0,
            }],
        });
        let mut ed = Editor::default();
        ed.panel = Some(0);
        ed.panel_pick = Some(0);
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::ArrowRight,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let steps = open.undo_steps();
        let mut left = 0;
        let _ = ctx.run(input, |ctx| {
            // A dialog is open: keyboard edits are not allowed.
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut open, &mut ed, 0, 5.0, false));
            left = ctx.input(|i| i.events.len());
        });
        // The key is not used up, and the device and the undo history stay as they were.
        assert_eq!(left, 1);
        let Some(PanelItem::Device { at, .. }) = open.project.panels[0].items.first() else {
            unreachable!("the device is there")
        };
        assert_eq!(*at, Point::new(100.0, 100.0));
        assert_eq!(open.undo_steps(), steps);
    }

    #[test]
    fn picks_a_turned_device_by_its_turned_outline() {
        let items = vec![PanelItem::Device {
            tag: wc_core::DeviceTag::parse("-G1").unwrap_or_default(),
            at: Point::new(0.0, 0.0),
            width: 10.0,
            height: 40.0,
            rotation: wc_core::geom::Rotation::R90,
        }];
        // Turned, it covers 40 x 10 mm.
        assert_eq!(device_at(&items, Point::new(30.0, 5.0)), Some(0));
        assert_eq!(device_at(&items, Point::new(5.0, 30.0)), None);
    }

    #[test]
    fn picks_a_device_before_the_rail_and_the_rail_before_the_duct() {
        let items = vec![
            PanelItem::Duct {
                min: Point::new(0.0, 0.0),
                max: Point::new(200.0, 60.0),
            },
            PanelItem::Rail {
                at: Point::new(0.0, 50.0),
                length: 100.0,
            },
            PanelItem::Device {
                tag: wc_core::DeviceTag::parse("-K1").unwrap_or_default(),
                at: Point::new(10.0, 20.0),
                width: 18.0,
                height: 60.0,
                rotation: wc_core::geom::Rotation::R0,
            },
        ];
        assert_eq!(item_at(&items, Point::new(15.0, 50.0)), Some(2));
        // The rail is 35 mm tall about its centre line and lies over the duct.
        assert_eq!(item_at(&items, Point::new(60.0, 40.0)), Some(1));
        assert_eq!(item_at(&items, Point::new(60.0, 66.0)), Some(1));
        assert_eq!(item_at(&items, Point::new(150.0, 30.0)), Some(0));
        assert_eq!(item_at(&items, Point::new(150.0, 90.0)), None);
    }
}

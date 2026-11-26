//! The panel layout view: the mounting plate with its ducts, DIN rails and device footprints,
//! drawn to scale. Devices are picked and dragged with the primary button; the wheel zooms and
//! the middle or secondary button pans, as on the schematic canvas.

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

/// The panel view's zoom limits, in screen points per millimetre: lower than the schematic's,
/// so a large plate fits.
pub const ZOOM_LIMITS: (f32, f32) = (0.05, 20.0);

pub fn show(ui: &mut egui::Ui, open: &mut Open, ed: &mut Editor, panel: usize, snap_step: f64) {
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
        ed.panel_pick = device_at(&p.items, at);
        if let Some(i) = ed.panel_pick
            && let Some(PanelItem::Device { at: a, .. }) = p.items.get(i)
        {
            ed.panel_drag = Some((Point::new(at.x - a.x, at.y - a.y), None, *a));
        }
    }
    // Escape puts the device back where the drag started and drops the drag's undo step; the
    // release afterwards changes nothing.
    if let (Some((_, checkpoint, start)), Some(i)) = (ed.panel_drag, ed.panel_pick)
        && ui.input(|inp| inp.key_pressed(egui::Key::Escape))
    {
        ed.panel_drag = None;
        if let Some(step) = checkpoint
            && let Some(PanelItem::Device { at, .. }) =
                open.project.panels.get_mut(panel).and_then(|x| x.items.get_mut(i))
        {
            *at = start;
            open.edited();
            // Only when the latest step is still this drag's (nothing has touched the history
            // since its checkpoint): then it changes nothing any more.
            if open.history_count() == step && open.before_last_change() == Some(&open.project) {
                open.forget_terminal_change();
                open.forget_last_change();
            }
        }
    }
    if resp.clicked_by(egui::PointerButton::Primary) {
        ed.panel_pick = pointer.and_then(|at| device_at(&p.items, at));
    }
    if let (Some((grab, checkpoint, start)), Some(i), Some(at)) = (ed.panel_drag, ed.panel_pick, pointer)
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
            if checkpoint.is_none() {
                open.checkpoint();
                ed.panel_drag = Some((grab, Some(open.history_count()), start));
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
    for item in &p.items {
        match item {
            PanelItem::Duct { min, max } => {
                let r = Rect::from_two_pos(xf.s(*min), xf.s(*max));
                painter.rect_filled(r, 0.0, t.grid.gamma_multiply(0.6));
                painter.rect_stroke(r, 0.0, hair, egui::StrokeKind::Inside);
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
                let r = Rect::from_two_pos(
                    xf.s(Point::new(at.x, at.y - 17.5)),
                    xf.s(Point::new(at.x + length, at.y + 17.5)),
                );
                painter.rect_stroke(r, 0.0, st, egui::StrokeKind::Inside);
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
    // The picked device's full tag and size, at the bottom of the view.
    let info = ed.panel_pick.and_then(|i| p.items.get(i)).and_then(|item| match item {
        PanelItem::Device {
            tag,
            at,
            width,
            height,
            rotation,
        } => Some(format!(
            "{tag}  ·  {width} x {height} mm{} at x {:.0}, y {:.0}  ·  drag to move, R to rotate, Delete to take it off the plate",
            turned_note(*rotation),
            at.x,
            at.y
        )),
        _ => None,
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
    fn escape_during_a_device_drag_puts_the_device_back() {
        let (_tmp, mut open) = crate::state::tests::project();
        let mut panel = wc_core::panel::Panel {
            id: "panel1".into(),
            name: "Panel 1".into(),
            width: 400.0,
            height: 300.0,
            items: Vec::new(),
        };
        let start = Point::new(100.0, 100.0);
        panel.items.push(PanelItem::Device {
            tag: wc_core::DeviceTag::parse("-K1").unwrap_or_default(),
            at: start,
            width: 40.0,
            height: 60.0,
            rotation: wc_core::geom::Rotation::R0,
        });
        open.project.panels.push(panel);
        let mut ed = Editor::default();
        ed.panel = Some(0);
        ed.view.fit = true;
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let frame = |open: &mut Open, ed: &mut Editor, time: f64, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1000.0, 700.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| show(ui, open, ed, 0, 1.0));
            });
        };
        let button = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let device_at_now = |open: &Open| match open.project.panels[0].items.first() {
            Some(PanelItem::Device { at, .. }) => *at,
            _ => unreachable!("the device is there"),
        };
        // Lay the view out, then find where the device's middle is on screen.
        frame(&mut open, &mut ed, 0.5, vec![]);
        let probe = egui::pos2(500.0, 350.0);
        frame(&mut open, &mut ed, 0.6, vec![egui::Event::PointerMoved(probe)]);
        let Some(seen) = ed.cursor else {
            unreachable!("the pointer is over the plate")
        };
        let zoom = ed.view.zoom;
        let on_screen = |m: Point| probe + Vec2::new(((m.x - seen.x) as f32) * zoom, ((m.y - seen.y) as f32) * zoom);
        let a = on_screen(Point::new(120.0, 130.0));
        let steps_before = open.undo_steps();
        frame(&mut open, &mut ed, 1.0, vec![egui::Event::PointerMoved(a)]);
        frame(&mut open, &mut ed, 1.01, vec![button(a, true)]);
        for (k, t) in [(1.0, 1.05), (2.0, 1.1), (3.0, 1.15)] {
            let to = on_screen(Point::new(120.0 + 10.0 * k, 130.0));
            frame(&mut open, &mut ed, t, vec![egui::Event::PointerMoved(to)]);
        }
        assert_ne!(device_at_now(&open), start, "the drag moves the device");
        assert_eq!(open.undo_steps(), steps_before + 1);

        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut open, &mut ed, 1.2, vec![escape]);
        assert_eq!(device_at_now(&open), start, "Escape puts the device back");
        assert_eq!(open.undo_steps(), steps_before, "and drops the drag's undo step");
        assert!(ed.panel_drag.is_none());

        // Moving on and letting go changes nothing.
        let far = on_screen(Point::new(200.0, 160.0));
        frame(&mut open, &mut ed, 1.25, vec![egui::Event::PointerMoved(far)]);
        frame(&mut open, &mut ed, 1.3, vec![button(far, false)]);
        frame(&mut open, &mut ed, 1.35, vec![]);
        assert_eq!(device_at_now(&open), start);
        assert_eq!(open.undo_steps(), steps_before);
    }

    /// Dragged away and back to its start with the button held, then a file-only edit (a
    /// translation, as the assistant's `translations` tool sets one) makes an undo step whose
    /// project is the start too: Escape keeps that step and drops nothing.
    #[test]
    fn escape_keeps_an_undo_step_made_after_the_drags_own() {
        let (_tmp, mut open) = crate::state::tests::project();
        let start = Point::new(100.0, 100.0);
        open.project.panels.push(wc_core::panel::Panel {
            id: "panel1".into(),
            name: "Panel 1".into(),
            width: 400.0,
            height: 300.0,
            items: vec![PanelItem::Device {
                tag: wc_core::DeviceTag::parse("-K1").unwrap_or_default(),
                at: start,
                width: 40.0,
                height: 60.0,
                rotation: wc_core::geom::Rotation::R0,
            }],
        });
        let mut ed = Editor::default();
        ed.panel = Some(0);
        ed.view.fit = true;
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let frame = |open: &mut Open, ed: &mut Editor, time: f64, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1000.0, 700.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| show(ui, open, ed, 0, 1.0));
            });
        };
        let device_at_now = |open: &Open| match open.project.panels[0].items.first() {
            Some(PanelItem::Device { at, .. }) => *at,
            _ => unreachable!("the device is there"),
        };
        frame(&mut open, &mut ed, 0.5, vec![]);
        let probe = egui::pos2(500.0, 350.0);
        frame(&mut open, &mut ed, 0.6, vec![egui::Event::PointerMoved(probe)]);
        let Some(seen) = ed.cursor else {
            unreachable!("the pointer is over the plate")
        };
        let zoom = ed.view.zoom;
        let on_screen = |m: Point| probe + Vec2::new(((m.x - seen.x) as f32) * zoom, ((m.y - seen.y) as f32) * zoom);
        let a = on_screen(Point::new(120.0, 130.0));
        let steps_before = open.undo_steps();
        frame(&mut open, &mut ed, 1.0, vec![egui::Event::PointerMoved(a)]);
        let press = egui::Event::PointerButton {
            pos: a,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut open, &mut ed, 1.01, vec![press]);
        for (k, t) in [(1.0, 1.05), (2.0, 1.1), (3.0, 1.15), (0.0, 1.2)] {
            let to = on_screen(Point::new(120.0 + 10.0 * k, 130.0));
            frame(&mut open, &mut ed, t, vec![egui::Event::PointerMoved(to)]);
        }
        assert_eq!(device_at_now(&open), start, "dragged back to its start");
        assert_eq!(open.undo_steps(), steps_before + 1);

        let file = open.dir.join(wc_core::translate::TRANSLATIONS_FILE);
        let set = open.translations_change(|dir| wc_core::translate::set_translation(dir, "Motor", "nb", "Motor"));
        assert!(matches!(set, Ok(true)), "{set:?}");
        assert_eq!(open.undo_steps(), steps_before + 2, "the translation is an undo step");
        assert_eq!(open.before_last_change(), Some(&open.project));

        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut open, &mut ed, 1.25, vec![escape]);
        assert!(ed.panel_drag.is_none());
        assert_eq!(device_at_now(&open), start);
        assert_eq!(open.undo_steps(), steps_before + 2, "Escape drops no step");
        assert!(file.exists());
        // Undo takes back the translation, not the drag.
        assert!(open.undo());
        assert!(!file.exists(), "Undo takes the translation back");
        assert_eq!(open.undo_steps(), steps_before + 1);
    }
}

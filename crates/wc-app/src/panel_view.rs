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

/// Where a dragged device lands when its corner is asked to go to `to`: kept, as turned, on a
/// `pw` x `ph` plate. `None` for an item that is not a device.
fn kept_on_plate(item: &PanelItem, to: Point, pw: f64, ph: f64) -> Option<Point> {
    let PanelItem::Device {
        width,
        height,
        rotation,
        ..
    } = item
    else {
        return None;
    };
    let (w, h) = wc_core::panel::turned_size(*width, *height, *rotation);
    Some(Point::new(
        to.x.clamp(0.0, (pw - w).max(0.0)),
        to.y.clamp(0.0, (ph - h).max(0.0)),
    ))
}

pub fn show(ui: &mut egui::Ui, open: &mut Open, ed: &mut Editor, panel: usize, snap_step: f64) {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
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
            let new_zoom = (ed.view.zoom * (scroll / 300.0).exp()).clamp(0.05, 20.0);
            let anchor = hp - rect.min - ed.view.offset;
            ed.view.offset += anchor - anchor * (new_zoom / ed.view.zoom);
            ed.view.zoom = new_zoom;
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
            ed.panel_drag = Some((Point::new(at.x - a.x, at.y - a.y), false));
        }
    }
    if resp.clicked_by(egui::PointerButton::Primary) {
        ed.panel_pick = pointer.and_then(|at| device_at(&p.items, at));
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
        // Keep the device on the plate before comparing, so a drag against an edge that leaves
        // the device where it is takes no undo step.
        let to = p.items.get(i).and_then(|d| kept_on_plate(d, to, pw, ph)).unwrap_or(to);
        let moved = matches!(p.items.get(i), Some(PanelItem::Device { at: a, .. }) if *a != to);
        if moved {
            if !checkpoint {
                open.checkpoint();
                ed.panel_drag = Some((grab, true));
            }
            if let Some(PanelItem::Device { at: a, .. }) =
                open.project.panels.get_mut(panel).and_then(|x| x.items.get_mut(i))
            {
                *a = to;
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
    fn a_device_against_the_plate_edge_stays_put() {
        let item = PanelItem::Device {
            tag: wc_core::DeviceTag::parse("-K1").unwrap_or_default(),
            at: Point::new(0.0, 0.0),
            width: 10.0,
            height: 40.0,
            rotation: wc_core::geom::Rotation::R90,
        };
        // Dragged further out past the top-left corner, it stays where it is.
        assert_eq!(
            kept_on_plate(&item, Point::new(-15.0, -5.0), 100.0, 100.0),
            Some(Point::new(0.0, 0.0))
        );
        // Turned, it is 40 mm wide: the right edge stops it at x = 60.
        assert_eq!(
            kept_on_plate(&item, Point::new(90.0, 95.0), 100.0, 100.0),
            Some(Point::new(60.0, 90.0))
        );
        assert_eq!(
            kept_on_plate(&item, Point::new(20.0, 30.0), 100.0, 100.0),
            Some(Point::new(20.0, 30.0))
        );
        let rail = PanelItem::Rail {
            at: Point::new(0.0, 0.0),
            length: 10.0,
        };
        assert_eq!(kept_on_plate(&rail, Point::new(-5.0, 0.0), 100.0, 100.0), None);
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
}

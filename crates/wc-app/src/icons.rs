//! WiringCraft's own toolbar icons, drawn as vectors so they stay sharp at any scale.

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    NewProject,
    Open,
    Properties,
    Save,
    Undo,
    Redo,
    Select,
    Symbol,
    Wire,
    Delete,
    Rotate,
    Mirror,
    Text,
    Line,
    Rect,
    Copy,
    Cut,
    Paste,
    PageAdd,
    PageCopy,
    PageProps,
    PageUp,
    PageDown,
    PageDelete,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    Grid,
    Library,
    Update,
    Pdf,
    Reports,
    Check,
    Commit,
    Branch,
    History,
    Layout,
    Theme,
}

/// Draw `icon` inside `rect` (a square) with colour `c`; `accent` picks out the part that says
/// what the command does (the plus of "add", the ends of a wire, the tick of a check).
pub fn paint(p: &Painter, rect: Rect, icon: Icon, c: Color32, accent: Color32) {
    let s = rect.width() / 24.0;
    let o = rect.min;
    let at = |x: f32, y: f32| -> Pos2 { pos2(o.x + x * s, o.y + y * s) };
    let st = Stroke::new(1.6 * s, c);
    let thin = Stroke::new(1.1 * s, c);
    let ast = Stroke::new(1.9 * s, accent);
    let line = |a: (f32, f32), b: (f32, f32)| p.line_segment([at(a.0, a.1), at(b.0, b.1)], st);
    let aline = |a: (f32, f32), b: (f32, f32)| p.line_segment([at(a.0, a.1), at(b.0, b.1)], ast);
    let poly = |pts: &[(f32, f32)], closed: bool| {
        let v: Vec<Pos2> = pts.iter().map(|(x, y)| at(*x, *y)).collect();
        p.add(if closed {
            Shape::closed_line(v, st)
        } else {
            Shape::line(v, st)
        });
    };
    let arc = |cx: f32, cy: f32, r: f32, a0: f32, a1: f32| {
        let n = 14;
        let v: Vec<Pos2> = (0..=n)
            .map(|i| {
                let t = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
                at(cx + r * t.cos(), cy + r * t.sin())
            })
            .collect();
        p.add(Shape::line(v, st));
    };
    match icon {
        Icon::Open => {
            poly(
                &[
                    (3.0, 7.0),
                    (9.0, 7.0),
                    (11.0, 9.0),
                    (21.0, 9.0),
                    (21.0, 19.0),
                    (3.0, 19.0),
                ],
                true,
            );
            line((3.0, 12.0), (21.0, 12.0));
        }
        Icon::Save => {
            poly(&[(4.0, 4.0), (17.0, 4.0), (20.0, 7.0), (20.0, 20.0), (4.0, 20.0)], true);
            poly(&[(8.0, 4.0), (8.0, 9.0), (15.0, 9.0), (15.0, 4.0)], false);
            p.rect_stroke(
                Rect::from_min_max(at(7.5, 13.0), at(16.5, 20.0)),
                0.0,
                thin,
                eframe::egui::StrokeKind::Inside,
            );
        }
        Icon::Undo | Icon::Redo => {
            let flip = if icon == Icon::Redo { -1.0 } else { 1.0 };
            let fx = |x: f32| if flip < 0.0 { 24.0 - x } else { x };
            arc(
                12.0,
                13.0,
                7.0,
                if flip > 0.0 { 200.0 } else { -20.0 },
                if flip > 0.0 { 380.0 } else { 160.0 },
            );
            poly(&[(fx(3.5), 7.0), (fx(5.5), 12.5), (fx(10.5), 10.0)], false);
        }
        Icon::Select => {
            poly(
                &[
                    (6.0, 3.0),
                    (6.0, 19.0),
                    (10.0, 15.0),
                    (13.0, 21.0),
                    (15.5, 20.0),
                    (12.5, 14.0),
                    (18.0, 14.0),
                ],
                true,
            );
        }
        Icon::Symbol => {
            // A make contact between two leads.
            line((12.0, 2.0), (12.0, 8.0));
            line((12.0, 16.0), (12.0, 22.0));
            line((12.0, 16.0), (7.0, 8.5));
            p.circle_filled(at(12.0, 8.0), 1.6 * s, accent);
            p.circle_filled(at(12.0, 16.0), 1.6 * s, accent);
        }
        Icon::Wire => {
            poly(&[(3.0, 18.0), (10.0, 18.0), (10.0, 6.0), (21.0, 6.0)], false);
            p.circle_filled(at(3.0, 18.0), 2.3 * s, accent);
            p.circle_filled(at(21.0, 6.0), 2.3 * s, accent);
        }
        Icon::Delete => {
            poly(&[(6.0, 7.0), (7.5, 21.0), (16.5, 21.0), (18.0, 7.0)], false);
            line((4.0, 7.0), (20.0, 7.0));
            poly(&[(9.0, 7.0), (10.0, 4.0), (14.0, 4.0), (15.0, 7.0)], false);
            line((10.0, 10.0), (10.5, 18.0));
            line((14.0, 10.0), (13.5, 18.0));
        }
        Icon::Rotate => {
            arc(12.0, 12.0, 7.5, -60.0, 220.0);
            poly(&[(15.0, 2.5), (16.0, 5.6), (12.5, 6.8)], false);
        }
        Icon::PageAdd => {
            poly(&[(5.0, 3.0), (14.0, 3.0), (19.0, 8.0), (19.0, 21.0), (5.0, 21.0)], true);
            aline((12.0, 10.0), (12.0, 18.0));
            aline((8.0, 14.0), (16.0, 14.0));
        }
        Icon::Check => {
            arc(12.0, 12.0, 8.5, 0.0, 360.0);
            p.add(Shape::line(vec![at(8.0, 12.5), at(11.0, 15.5), at(16.5, 9.0)], ast));
        }
        Icon::Reports => {
            p.rect_stroke(
                Rect::from_min_max(at(3.0, 4.0), at(21.0, 20.0)),
                1.0 * s,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            line((3.0, 9.0), (21.0, 9.0));
            p.line_segment([at(3.0, 14.5), at(21.0, 14.5)], thin);
            p.line_segment([at(10.0, 4.0), at(10.0, 20.0)], thin);
        }
        Icon::Text => {
            line((5.0, 5.0), (19.0, 5.0));
            line((12.0, 5.0), (12.0, 20.0));
            line((9.0, 20.0), (15.0, 20.0));
        }
        Icon::Line => {
            line((4.0, 20.0), (20.0, 4.0));
            p.circle_filled(at(4.0, 20.0), 1.8 * s, c);
            p.circle_filled(at(20.0, 4.0), 1.8 * s, c);
        }
        Icon::Rect => {
            p.rect_stroke(
                Rect::from_min_max(at(3.0, 6.0), at(21.0, 18.0)),
                0.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
        }
        Icon::Mirror => {
            p.line_segment([at(12.0, 2.0), at(12.0, 22.0)], thin);
            poly(&[(9.5, 5.0), (3.0, 18.0), (9.5, 18.0)], true);
            poly(&[(14.5, 5.0), (21.0, 18.0), (14.5, 18.0)], true);
        }
        Icon::Copy => {
            poly(&[(4.0, 16.0), (4.0, 3.0), (14.0, 3.0)], false);
            p.rect_stroke(
                Rect::from_min_max(at(8.0, 7.0), at(20.0, 21.0)),
                1.0 * s,
                st,
                eframe::egui::StrokeKind::Inside,
            );
        }
        Icon::Cut => {
            arc(7.0, 17.5, 3.0, 0.0, 360.0);
            arc(17.0, 17.5, 3.0, 0.0, 360.0);
            line((9.0, 15.0), (17.0, 3.0));
            line((15.0, 15.0), (7.0, 3.0));
        }
        Icon::Paste => {
            poly(
                &[
                    (8.0, 4.0),
                    (5.0, 4.0),
                    (5.0, 21.0),
                    (19.0, 21.0),
                    (19.0, 4.0),
                    (16.0, 4.0),
                ],
                false,
            );
            p.rect_stroke(
                Rect::from_min_max(at(8.0, 2.5), at(16.0, 6.5)),
                1.0 * s,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            p.line_segment([at(8.5, 11.0), at(15.5, 11.0)], thin);
            p.line_segment([at(8.5, 15.0), at(15.5, 15.0)], thin);
        }
        Icon::PageCopy => {
            poly(&[(3.0, 17.0), (3.0, 3.0), (13.0, 3.0)], false);
            poly(
                &[(7.0, 7.0), (16.0, 7.0), (21.0, 12.0), (21.0, 21.0), (7.0, 21.0)],
                true,
            );
        }
        Icon::PageProps | Icon::Properties => {
            if icon == Icon::PageProps {
                poly(&[(5.0, 3.0), (14.0, 3.0), (19.0, 8.0), (19.0, 21.0), (5.0, 21.0)], true);
            } else {
                p.rect_stroke(
                    Rect::from_min_max(at(4.0, 3.0), at(20.0, 21.0)),
                    1.0 * s,
                    st,
                    eframe::egui::StrokeKind::Inside,
                );
            }
            for y in [10.0, 14.0, 18.0] {
                p.line_segment([at(8.0, y), at(10.0, y)], thin);
                p.line_segment([at(11.5, y), at(16.0, y)], thin);
            }
        }
        Icon::PageUp | Icon::PageDown => {
            poly(&[(4.0, 3.0), (12.0, 3.0), (16.0, 7.0), (16.0, 21.0), (4.0, 21.0)], true);
            let (a, b) = if icon == Icon::PageUp { (19.0, 7.0) } else { (7.0, 19.0) };
            line((20.0, a), (20.0, b));
            poly(
                &[
                    (17.5, b + (a - b).signum() * 3.0),
                    (20.0, b),
                    (22.5, b + (a - b).signum() * 3.0),
                ],
                false,
            );
        }
        Icon::PageDelete => {
            poly(&[(5.0, 3.0), (14.0, 3.0), (19.0, 8.0), (19.0, 21.0), (5.0, 21.0)], true);
            aline((9.0, 11.0), (15.0, 17.0));
            aline((15.0, 11.0), (9.0, 17.0));
        }
        Icon::NewProject => {
            poly(&[(2.5, 6.0), (2.5, 19.0), (15.0, 19.0)], false);
            poly(&[(2.5, 6.0), (8.0, 6.0), (10.0, 8.0), (17.0, 8.0), (17.0, 11.0)], false);
            aline((19.0, 13.0), (19.0, 21.0));
            aline((15.0, 17.0), (23.0, 17.0));
        }
        Icon::ZoomIn | Icon::ZoomOut | Icon::ZoomFit => {
            arc(10.0, 10.0, 6.5, 0.0, 360.0);
            line((15.0, 15.0), (21.0, 21.0));
            if icon != Icon::ZoomFit {
                aline((7.0, 10.0), (13.0, 10.0));
            }
            if icon == Icon::ZoomIn {
                aline((10.0, 7.0), (10.0, 13.0));
            }
            if icon == Icon::ZoomFit {
                p.rect_stroke(
                    Rect::from_min_max(at(7.0, 7.5), at(13.0, 12.5)),
                    0.0,
                    thin,
                    eframe::egui::StrokeKind::Inside,
                );
            }
        }
        Icon::Grid => {
            for i in 0..4 {
                for j in 0..4 {
                    p.circle_filled(at(4.5 + i as f32 * 5.0, 4.5 + j as f32 * 5.0), 1.1 * s, c);
                }
            }
        }
        Icon::Library => {
            p.rect_stroke(
                Rect::from_min_max(at(3.0, 5.0), at(8.0, 20.0)),
                0.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                Rect::from_min_max(at(9.0, 3.0), at(14.0, 20.0)),
                0.0,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            poly(&[(15.5, 6.0), (19.5, 5.0), (22.0, 19.0), (18.0, 20.0)], true);
        }
        Icon::Update => {
            arc(12.0, 12.0, 7.0, 200.0, 340.0);
            arc(12.0, 12.0, 7.0, 20.0, 160.0);
            p.add(Shape::line(vec![at(17.5, 3.5), at(18.6, 7.6), at(14.6, 8.4)], ast));
            p.add(Shape::line(vec![at(6.5, 20.5), at(5.4, 16.4), at(9.4, 15.6)], ast));
        }
        Icon::Pdf => {
            poly(&[(5.0, 3.0), (14.0, 3.0), (19.0, 8.0), (19.0, 21.0), (5.0, 21.0)], true);
            p.add(Shape::line(vec![at(14.0, 3.0), at(14.0, 8.0), at(19.0, 8.0)], ast));
            line((8.0, 12.0), (16.0, 12.0));
            line((8.0, 15.0), (16.0, 15.0));
            line((8.0, 18.0), (13.0, 18.0));
        }
        Icon::Commit => {
            line((2.0, 12.0), (8.0, 12.0));
            line((16.0, 12.0), (22.0, 12.0));
            p.circle_stroke(at(12.0, 12.0), 4.0 * s, ast);
        }
        Icon::Branch => {
            // Two subway lines splitting.
            line((6.0, 3.0), (6.0, 21.0));
            poly(&[(6.0, 8.0), (11.0, 13.0), (17.0, 13.0), (17.0, 21.0)], false);
            p.circle_filled(at(6.0, 4.0), 2.3 * s, c);
            p.circle_filled(at(17.0, 20.0), 2.3 * s, accent);
        }
        Icon::History => {
            arc(12.0, 12.0, 8.5, 0.0, 360.0);
            aline((12.0, 7.0), (12.0, 12.0));
            aline((12.0, 12.0), (15.5, 14.5));
        }
        Icon::Layout => {
            p.rect_stroke(
                Rect::from_min_max(at(3.0, 4.0), at(21.0, 20.0)),
                1.0 * s,
                st,
                eframe::egui::StrokeKind::Inside,
            );
            line((3.0, 9.0), (21.0, 9.0));
            line((8.0, 9.0), (8.0, 20.0));
        }
        Icon::Theme => {
            // Half-filled disc: light on one side, dark on the other.
            p.circle_stroke(at(12.0, 12.0), 8.0 * s, st);
            let half: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let t = (-90.0 + 180.0 * i as f32 / 16.0_f32).to_radians();
                    at(12.0 + 8.0 * t.cos(), 12.0 + 8.0 * t.sin())
                })
                .collect();
            p.add(Shape::convex_polygon(half, accent, Stroke::NONE));
        }
    }
    let _ = vec2;
}

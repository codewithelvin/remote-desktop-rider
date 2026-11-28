use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Close,
    Plus,
    Send,
    Lock,
    File,
    Bubble,
}

pub fn paint(p: &Painter, rect: Rect, g: Glyph, color: Color32) {
    let s = rect.width().min(rect.height());
    let o = rect.center() - vec2(s, s) / 2.0;
    let at = |x: f32, y: f32| -> Pos2 { o + vec2(x, y) * s };
    let line = |w: f32| Stroke::new((s * w).max(1.0), color);
    match g {
        Glyph::Close => {
            p.line_segment([at(0.3, 0.3), at(0.7, 0.7)], line(0.08));
            p.line_segment([at(0.7, 0.3), at(0.3, 0.7)], line(0.08));
        }
        Glyph::Plus => {
            p.line_segment([at(0.5, 0.24), at(0.5, 0.76)], line(0.08));
            p.line_segment([at(0.24, 0.5), at(0.76, 0.5)], line(0.08));
        }
        Glyph::Send => {
            let (tip, top, notch, bottom) = (at(0.8, 0.5), at(0.22, 0.24), at(0.36, 0.5), at(0.22, 0.76));
            p.add(Shape::convex_polygon(vec![top, tip, notch], color, Stroke::NONE));
            p.add(Shape::convex_polygon(vec![notch, tip, bottom], color, Stroke::NONE));
        }
        Glyph::Lock => {
            let body = Rect::from_min_max(at(0.27, 0.47), at(0.73, 0.8));
            p.rect_filled(body, CornerRadius::same((s * 0.07) as u8), color);
            let (cx, cy, r) = (0.5, 0.4, 0.14);
            let mut shackle = vec![at(cx - r, 0.48)];
            shackle.extend((0..=12).map(|i| {
                let a = std::f32::consts::PI * (1.0 + i as f32 / 12.0);
                at(cx + r * a.cos(), cy + r * a.sin())
            }));
            shackle.push(at(cx + r, 0.48));
            p.add(Shape::line(shackle, line(0.075)));
        }
        Glyph::File => {
            let outline = vec![at(0.28, 0.16), at(0.58, 0.16), at(0.74, 0.32), at(0.74, 0.84), at(0.28, 0.84)];
            p.add(Shape::closed_line(outline, line(0.06)));
            p.add(Shape::line(vec![at(0.58, 0.16), at(0.58, 0.32), at(0.74, 0.32)], line(0.06)));
        }
        Glyph::Bubble => {
            let body = Rect::from_min_max(at(0.16, 0.2), at(0.84, 0.66));
            p.rect_stroke(body, CornerRadius::same((s * 0.12) as u8), line(0.05), StrokeKind::Middle);
            p.add(Shape::line(vec![at(0.32, 0.66), at(0.3, 0.82), at(0.48, 0.66)], line(0.05)));
        }
    }
}

pub fn show(ui: &mut Ui, g: Glyph, size: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    paint(ui.painter(), rect, g, color);
    resp
}

pub fn button(ui: &mut Ui, g: Glyph, size: f32, enabled: bool, fill: Option<Color32>, color: Color32) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), sense);
    let resp = if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
    let p = ui.painter();
    if let Some(fill) = fill {
        p.circle_filled(rect.center(), size / 2.0, fill);
    } else if enabled && resp.hovered() {
        p.circle_filled(rect.center(), size / 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let color = if enabled { color } else { ui.visuals().weak_text_color().gamma_multiply(0.6) };
    paint(p, Rect::from_center_size(rect.center(), vec2(size, size) * 0.6), g, color);
    resp
}

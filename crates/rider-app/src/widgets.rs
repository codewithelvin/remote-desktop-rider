use eframe::egui::{
    self, Align2, Button, Color32, CornerRadius, FontFamily, FontId, Frame, Margin, Response, RichText, Sense, Stroke,
    Ui, vec2,
};
use rider_core::i18n::{Lang, tr};
use rider_core::trf;
use std::sync::mpsc;

pub struct Bus<M> {
    tx: mpsc::Sender<M>,
    rx: mpsc::Receiver<M>,
    ctx: egui::Context,
}

impl<M> Bus<M> {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx, ctx: ctx.clone() }
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    pub fn tx(&self) -> Tx<M> {
        Tx { tx: self.tx.clone(), ctx: self.ctx.clone() }
    }

    pub fn drain(&self) -> mpsc::TryIter<'_, M> {
        self.rx.try_iter()
    }
}

pub struct Tx<M> {
    tx: mpsc::Sender<M>,
    ctx: egui::Context,
}

impl<M> Clone for Tx<M> {
    fn clone(&self) -> Self {
        Self { tx: self.tx.clone(), ctx: self.ctx.clone() }
    }
}

impl<M> Tx<M> {
    pub fn send(&self, msg: M) {
        let _ = self.tx.send(msg);
        self.ctx.request_repaint();
    }
}

pub use rider_ui::theme::*;

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let dark = ui.visuals().dark_mode;
    Frame::new()
        .fill(card_fill(ui))
        .stroke(Stroke::new(1.0, border(dark)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str, right: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(self::title(title, 22.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
    if !subtitle.is_empty() {
        hint(ui, subtitle);
    }
    ui.add_space(6.0);
}

pub fn section(ui: &mut Ui, icon: &str, title: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).size(16.0).color(ACCENT));
        ui.label(self::title(title, 15.5));
    });
}

pub fn title(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).family(semibold()).size(size).strong()
}

pub fn title_fit(ui: &Ui, text: &str, size: f32, width: f32) -> RichText {
    let font = FontId::new(size, semibold());
    let w = ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font, Color32::WHITE).size().x);
    let size = if w > width { (size * width / w).max(size * 0.8).floor() } else { size };
    title(text, size)
}

pub fn primary(ui: &mut Ui, enabled: bool, text: &str) -> Response {
    primary_busy(ui, enabled, false, text)
}

pub fn primary_busy(ui: &mut Ui, enabled: bool, busy: bool, text: &str) -> Response {
    let text = RichText::new(text).family(semibold()).color(Color32::WHITE);
    busy_button(ui, enabled, busy, text, Color32::WHITE, Some(ACCENT))
}

pub fn secondary_busy(ui: &mut Ui, enabled: bool, busy: bool, text: &str) -> Response {
    let color = ui.visuals().text_color();
    busy_button(ui, enabled, busy, RichText::new(text), color, None)
}

fn busy_button(
    ui: &mut Ui,
    enabled: bool,
    busy: bool,
    text: RichText,
    color: Color32,
    fill: Option<Color32>,
) -> Response {
    let spinner_id = egui::IdSalt::new("busy-spinner");
    let side = ui.text_style_height(&egui::TextStyle::Button);
    let button =
        if busy { Button::new((egui::Atom::custom(spinner_id, vec2(side, side)), text)) } else { Button::new(text) };
    let button = match fill {
        Some(fill) => button.fill(fill),
        None => button,
    };
    let shown = ui.add_enabled_ui(enabled && !busy, |ui| button.atom_ui(ui)).inner;
    if let Some(rect) = shown.rect(spinner_id) {
        egui::Spinner::new().size(side).color(color).paint_at(ui, rect);
    }
    if busy && shown.response.contains_pointer() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Progress);
    }
    shown.response
}

pub fn danger(ui: &mut Ui, text: &str) -> Response {
    let red = ui.visuals().error_fg_color;
    ui.add(Button::new(RichText::new(text).family(semibold()).color(Color32::WHITE)).fill(red))
}

const ICON_BUTTON: egui::Vec2 = vec2(30.0, 30.0);

pub fn icon_button(ui: &mut Ui, icon: &str, tip: &str) -> Response {
    ui.add(Button::new(RichText::new(icon).size(15.0)).frame(false).min_size(ICON_BUTTON)).on_hover_text(tip)
}

pub fn checkbox(ui: &mut Ui, enabled: bool, value: &mut bool, text: &str) -> Response {
    ui.add_enabled(enabled, egui::Checkbox::new(value, text)).on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn icon_spinner(ui: &mut Ui) -> Response {
    let (rect, resp) = ui.allocate_exact_size(ICON_BUTTON, Sense::hover());
    let side = ui.text_style_height(&egui::TextStyle::Button);
    egui::Spinner::new().size(side).paint_at(ui, egui::Rect::from_center_size(rect.center(), vec2(side, side)));
    resp.on_hover_cursor(egui::CursorIcon::Progress)
}

pub fn copyable(ui: &mut Ui, text: &str, shown: RichText) {
    ui.label(shown);
    if icon_button(ui, icon::COPY, tr("Copy", "Kopyala")).clicked() {
        ui.ctx().copy_text(text.to_owned());
    }
}

pub fn avatar(ui: &mut Ui, icon: &str, status: Option<Color32>) {
    let (rect, _) = ui.allocate_exact_size(vec2(42.0, 42.0), Sense::hover());
    let p = ui.painter();
    p.circle_filled(rect.center(), 21.0, ACCENT.linear_multiply(if ui.visuals().dark_mode { 0.22 } else { 0.12 }));
    p.text(rect.center(), Align2::CENTER_CENTER, icon, FontId::proportional(19.0), ACCENT);
    if let Some(c) = status {
        let at = rect.right_bottom() - vec2(6.0, 6.0);
        p.circle_filled(at, 7.0, card_fill(ui));
        p.circle_filled(at, 5.0, c);
    }
}

pub fn dot(ui: &mut Ui, color: Option<Color32>) {
    let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
    match color {
        Some(c) => _ = ui.painter().circle_filled(rect.center(), 5.0, c),
        None => _ = ui.painter().circle_stroke(rect.center(), 4.5, (1.5, GREY)),
    }
}

pub fn pill(ui: &mut Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(color.linear_multiply(if ui.visuals().dark_mode { 0.25 } else { 0.14 }))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            let fg = if color == GREY { ui.visuals().text_color() } else { color };
            ui.label(RichText::new(text).size(12.5).color(fg))
        });
}

pub fn access_name(access: rider_core::proto::Access) -> &'static str {
    use rider_core::proto::Access;
    match access {
        Access::Full => tr("Full access", "Tam giriş"),
        Access::Standard => tr("Standard", "Standart"),
        Access::ViewOnly => tr("View only", "Yalnız baxış"),
    }
}

pub fn access_what(access: rider_core::proto::Access) -> &'static str {
    use rider_core::proto::Access;
    match access {
        Access::Full => tr(
            "See and control the screen, use the clipboard, send files",
            "Ekranı görmək və idarə etmək, mübadilə buferi, fayl göndərmək",
        ),
        Access::Standard => tr(
            "See and control the screen, use the clipboard; no files",
            "Ekranı görmək və idarə etmək, mübadilə buferi; fayllar olmadan",
        ),
        Access::ViewOnly => tr("See the screen only", "Yalnız ekranı görmək"),
    }
}

pub fn access_menu(ui: &mut Ui, access: &mut rider_core::proto::Access) -> bool {
    use rider_core::proto::Access;
    let before = *access;
    egui::ComboBox::from_id_salt("access-level")
        .selected_text(access_name(*access))
        .width(150.0)
        .show_ui(ui, |ui| {
            for a in Access::ALL {
                ui.selectable_value(access, a, access_name(a)).on_hover_text(access_what(a));
            }
        })
        .response
        .on_hover_text(tr("What they can do", "Nə edə bilər"));
    *access != before
}

pub fn access_choice(ui: &mut Ui, access: &mut rider_core::proto::Access) -> bool {
    use rider_core::proto::Access;
    let mut changed = false;
    for a in Access::ALL {
        let (name, what) = match a {
            Access::Full => (
                tr("Full access", "Tam giriş"),
                tr(
                    "See and control the screen, use the clipboard, send files",
                    "Ekranı görmək və idarə etmək, mübadilə buferi, fayl göndərmək",
                ),
            ),
            Access::Standard => (
                tr("Standard", "Standart"),
                tr(
                    "See and control the screen, use the clipboard; no files",
                    "Ekranı görmək və idarə etmək, mübadilə buferi; fayllar olmadan",
                ),
            ),
            Access::ViewOnly => (tr("View only", "Yalnız baxış"), tr("See the screen only", "Yalnız ekranı görmək")),
        };
        let selected = *access == a;
        let dark = ui.visuals().dark_mode;
        let resp = Frame::new()
            .fill(if selected { ACCENT.linear_multiply(if dark { 0.22 } else { 0.10 }) } else { Color32::TRANSPARENT })
            .stroke(Stroke::new(if selected { 1.5 } else { 1.0 }, if selected { ACCENT } else { border(dark) }))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    let p = ui.painter();
                    if selected {
                        p.circle_filled(rect.center(), 7.0, ACCENT);
                        p.circle_filled(rect.center(), 3.0, Color32::WHITE);
                    } else {
                        p.circle_stroke(rect.center(), 6.5, (1.5, GREY));
                    }
                    ui.vertical(|ui| {
                        ui.add(egui::Label::new(RichText::new(name).family(semibold()).size(14.0)).selectable(false));
                        ui.add(egui::Label::new(RichText::new(what).size(12.5).weak()).selectable(false));
                    });
                });
            })
            .response
            .interact(Sense::click());
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, name));
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !selected {
            *access = a;
            changed = true;
        }
        ui.add_space(4.0);
    }
    changed
}

pub fn nav_item(ui: &mut Ui, icon: &str, text: &str, selected: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, text));
    let ctx = ui.ctx().clone();
    let hover = ctx.animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
    let sel = ctx.animate_bool_with_time(resp.id.with("selected"), selected, 0.18);
    let p = ui.painter();
    if hover > 0.0 || sel > 0.0 {
        let tint = ACCENT.linear_multiply(if ui.visuals().dark_mode { 0.25 } else { 0.13 });
        let hovered = ui.visuals().widgets.hovered.weak_bg_fill;
        let fill = if sel > 0.0 {
            lerp_color(hovered.linear_multiply(hover), tint, sel)
        } else {
            hovered.linear_multiply(hover)
        };
        p.rect_filled(rect, CornerRadius::same(8), fill);
    }
    if sel > 0.0 {
        let h = 18.0 * sel;
        let bar = egui::Rect::from_min_size(rect.left_center() + vec2(0.0, -h / 2.0), vec2(3.0, h));
        p.rect_filled(bar, CornerRadius::same(2), ACCENT);
    }
    let color = if selected { ACCENT } else { ui.visuals().text_color() };
    p.text(rect.left_center() + vec2(16.0, 0.0), Align2::LEFT_CENTER, icon, FontId::proportional(16.0), color);
    p.text(
        rect.left_center() + vec2(44.0, 0.0),
        Align2::LEFT_CENTER,
        text,
        FontId::new(14.5, if selected { semibold() } else { FontFamily::Proportional }),
        ui.visuals().text_color(),
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn appear(ui: &mut Ui, since: f64) {
    let now = ui.input(|i| i.time);
    let p = ((now - since) / 0.22).clamp(0.0, 1.0) as f32;
    if p < 1.0 {
        let e = 1.0 - (1.0 - p).powi(3);
        ui.multiply_opacity(e);
        ui.add_space((1.0 - e) * 8.0);
        ui.ctx().request_repaint();
    }
}

pub fn big_code(ui: &mut Ui, code: &str) {
    Frame::new()
        .fill(ACCENT.linear_multiply(if ui.visuals().dark_mode { 0.18 } else { 0.08 }))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    let id = egui::Id::unique(("copied", code));
                    let now = ui.input(|i| i.time);
                    let copied = ui.data(|d| d.get_temp::<f64>(id)).is_some_and(|t| now - t < 2.0);
                    let label = if copied {
                        format!("{}  {}", icon::CHECK, tr("Copied", "Kopyalandı"))
                    } else {
                        format!("{}  {}", icon::COPY, tr("Copy", "Kopyala"))
                    };
                    let w = 130.0 + code.len() as f32 * 21.0;
                    ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                    ui.label(RichText::new(code).monospace().size(34.0).strong());
                    ui.add_space(12.0);
                    if ui.add(Button::new(label).min_size(vec2(108.0, 32.0))).clicked() {
                        ui.ctx().copy_text(code.to_owned());
                        ui.data_mut(|d| d.insert_temp(id, now));
                    }
                    if copied {
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
                    }
                });
            });
        });
}

pub fn modal(ctx: &egui::Context, name: &str, shape: impl std::hash::Hash + std::fmt::Debug) -> egui::Modal {
    let id = egui::Id::unique(name);
    let shape = egui::Id::unique(shape);
    let key = id.with("shape");
    let changed = ctx.data_mut(|d| {
        let old = d.get_temp::<egui::Id>(key);
        d.insert_temp(key, shape);
        old != Some(shape)
    });
    if changed {
        ctx.request_repaint();
    }
    egui::Modal::new(id).area(egui::Modal::default_area(id).sizing_pass(changed))
}

pub fn dialog_buttons(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(6.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
}

pub fn error(ui: &mut Ui, text: &str) {
    let color = ui.visuals().error_fg_color;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(icon::WARNING).color(color));
        ui.label(RichText::new(text).color(color));
    });
}

pub fn success(ui: &mut Ui, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(icon::CHECK).color(GREEN));
        ui.label(RichText::new(text).color(GREEN));
    });
}

pub fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).weak());
}

pub fn ago(then: u64) -> String {
    let now = rider_core::now_us() / 1_000_000;
    let s = now.saturating_sub(then);
    match s {
        0..60 => tr("just now", "indicə").into(),
        60..3600 => trf!("{} min ago", "{} dəq. əvvəl", s / 60),
        3600..86_400 => trf!("{} h ago", "{} saat əvvəl", s / 3600),
        86_400..172_800 => tr("yesterday", "dünən").into(),
        _ => trf!("{} days ago", "{} gün əvvəl", s / 86_400),
    }
}

pub fn language_switch(ui: &mut Ui) -> Option<Lang> {
    let current = rider_core::i18n::lang();
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let mut order = Lang::ALL;
        if ui.layout().prefer_right_to_left() {
            order.reverse();
        }
        for l in order {
            if flag(ui, l, l == current).clicked() && l != current {
                picked = Some(l);
            }
        }
    });
    if let Some(l) = picked {
        let _ = rider_core::i18n::save(l);
        ui.ctx().request_repaint();
    }
    picked
}

pub fn flag(ui: &mut Ui, lang: Lang, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 26.0), Sense::click());
    let resp = resp.on_hover_text(lang.native_name()).on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, lang.native_name()));
    let r = egui::Rect::from_center_size(rect.center(), vec2(30.0, 20.0));
    let p = ui.painter().with_clip_rect(r);
    let opacity = if selected || resp.hovered() { 1.0 } else { 0.55 };
    let c = |r: u8, g: u8, b: u8| Color32::from_rgb(r, g, b).gamma_multiply(opacity);
    let white = Color32::WHITE.gamma_multiply(opacity);
    match lang {
        Lang::Az => {
            let h = r.height() / 3.0;
            let band = |i: f32| egui::Rect::from_min_size(r.min + vec2(0.0, h * i), vec2(r.width(), h));
            p.rect_filled(band(0.0), 0, c(0, 181, 226));
            p.rect_filled(band(1.0), 0, c(239, 51, 64));
            p.rect_filled(band(2.0), 0, c(80, 158, 47));
            let m = r.center();
            p.circle_filled(m + vec2(-1.6, 0.0), 2.9, white);
            p.circle_filled(m + vec2(-0.7, 0.0), 2.4, c(239, 51, 64));
            let star = m + vec2(2.6, 0.0);
            for angle in [0.0f32, std::f32::consts::FRAC_PI_4] {
                let pts = (0..4)
                    .map(|k| {
                        let a = angle + k as f32 * std::f32::consts::FRAC_PI_2;
                        star + 1.35 * vec2(a.cos(), a.sin())
                    })
                    .collect();
                p.add(egui::Shape::convex_polygon(pts, white, Stroke::NONE));
            }
        }
        Lang::En => {
            let red = c(200, 16, 46);
            p.rect_filled(r, 0, c(1, 33, 105));
            for (a, b) in [(r.left_top(), r.right_bottom()), (r.left_bottom(), r.right_top())] {
                p.line_segment([a, b], Stroke::new(4.0, white));
                p.line_segment([a, b], Stroke::new(1.4, red));
            }
            let m = r.center();
            p.rect_filled(egui::Rect::from_center_size(m, vec2(r.width(), 6.0)), 0, white);
            p.rect_filled(egui::Rect::from_center_size(m, vec2(6.0, r.height())), 0, white);
            p.rect_filled(egui::Rect::from_center_size(m, vec2(r.width(), 3.4)), 0, red);
            p.rect_filled(egui::Rect::from_center_size(m, vec2(3.4, r.height())), 0, red);
        }
    }
    let outline = if selected {
        Stroke::new(2.0, ACCENT)
    } else {
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
    };
    ui.painter().rect_stroke(r.expand(if selected { 2.0 } else { 0.5 }), 3, outline, egui::StrokeKind::Middle);
    resp
}

pub fn short_fp(hex: &str) -> String {
    rider_core::identity::fp_from_hex(hex).map_or_default(|f| rider_core::identity::fp_short(&f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_naturally() {
        rider_core::i18n::set_current(Lang::En);
        let now = rider_core::now_us() / 1_000_000;
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - 125), "2 min ago");
        assert_eq!(ago(now - 3 * 3600), "3 h ago");
        assert_eq!(ago(now - 90_000), "yesterday");
        assert_eq!(ago(now - 5 * 86_400), "5 days ago");
    }
}

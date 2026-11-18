//! Look and feel (Windows fonts and icons, colours, light/dark), shared
//! components, and the worker → UI message channel.

use eframe::egui::{
    self, Align2, Button, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin,
    Response, RichText, Sense, Stroke, Ui, vec2,
};
use std::sync::{Arc, mpsc};

/// Messages from background work to the UI thread; sending wakes the UI.
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

// ------------------------------------------------------------------- colours

pub const ACCENT: Color32 = Color32::from_rgb(47, 111, 235);
pub const GREEN: Color32 = Color32::from_rgb(34, 160, 84);
pub const GREY: Color32 = Color32::from_rgb(138, 140, 148);
pub const AMBER: Color32 = Color32::from_rgb(214, 140, 20);

/// Segoe Fluent Icons / MDL2 Assets code points (fall back to the text font).
pub mod icon {
    pub const PC: &str = "\u{E7F4}";
    pub const HOME: &str = "\u{E80F}";
    pub const SETTINGS: &str = "\u{E713}";
    pub const ADD: &str = "\u{E710}";
    pub const DELETE: &str = "\u{E74D}";
    pub const KEY: &str = "\u{E8D7}";
    pub const SHIELD: &str = "\u{E7EF}";
    pub const GLOBE: &str = "\u{E774}";
    pub const COPY: &str = "\u{E8C8}";
    pub const DEVICES: &str = "\u{E772}";
    pub const CHECK: &str = "\u{E73E}";
    pub const WARNING: &str = "\u{E7BA}";
    pub const INFO: &str = "\u{E946}";
    pub const PLAY: &str = "\u{E768}";
}

fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

// --------------------------------------------------------------------- theme

/// Windows fonts (Segoe UI, Semibold, Fluent icons) and the app's colours for
/// both light and dark mode (egui follows the Windows setting).
pub fn setup(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir = std::env::var_os("WINDIR").map_or_else(|| r"C:\Windows".into(), std::path::PathBuf::from).join("Fonts");
    let mut load = |key: &str, files: &[&str]| -> bool {
        let Some(bytes) = files.iter().find_map(|f| std::fs::read(dir.join(f)).ok()) else { return false };
        fonts.font_data.insert(key.into(), Arc::new(FontData::from_owned(bytes)));
        true
    };
    let text = load("segoe", &["segoeui.ttf"]);
    let bold = load("segoe-semibold", &["seguisb.ttf"]);
    let icons = load("icons", &["SegoeIcons.ttf", "segmdl2.ttf"]);

    let mut proportional = fonts.families[&FontFamily::Proportional].clone();
    if text {
        proportional.insert(0, "segoe".into());
    }
    if icons {
        proportional.push("icons".into());
    }
    let mut strong = proportional.clone();
    if bold {
        strong.insert(0, "segoe-semibold".into());
    }
    fonts.families.insert(FontFamily::Proportional, proportional);
    fonts.families.insert(semibold(), strong);
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.button_padding = vec2(14.0, 6.0);
        s.spacing.interact_size.y = 30.0;
        s.spacing.window_margin = Margin::same(20);
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(egui::TextStyle::Monospace, FontId::monospace(13.5));

        let v = &mut s.visuals;
        let dark = v.dark_mode;
        v.selection.bg_fill = ACCENT.linear_multiply(if dark { 0.55 } else { 0.35 });
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.hyperlink_color = ACCENT;
        v.window_corner_radius = CornerRadius::same(12);
        v.window_stroke = Stroke::new(1.0, border(dark));
        v.window_shadow.blur = 24;
        v.popup_shadow.blur = 12;
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = CornerRadius::same(7);
        }
        if dark {
            v.panel_fill = Color32::from_rgb(25, 26, 30);
            v.window_fill = Color32::from_rgb(34, 35, 40);
            v.extreme_bg_color = Color32::from_rgb(20, 21, 24);
            v.faint_bg_color = Color32::from_rgb(31, 32, 37);
            v.widgets.inactive.weak_bg_fill = Color32::from_rgb(48, 50, 57);
            v.widgets.hovered.weak_bg_fill = Color32::from_rgb(60, 62, 70);
        } else {
            v.panel_fill = Color32::from_rgb(242, 243, 246);
            v.window_fill = Color32::WHITE;
            v.extreme_bg_color = Color32::WHITE;
            v.faint_bg_color = Color32::from_rgb(248, 249, 251);
            v.widgets.inactive.weak_bg_fill = Color32::from_rgb(228, 230, 235);
            v.widgets.hovered.weak_bg_fill = Color32::from_rgb(216, 219, 226);
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border(dark));
    });
}

fn border(dark: bool) -> Color32 {
    if dark { Color32::from_rgb(52, 54, 61) } else { Color32::from_rgb(222, 225, 231) }
}

fn card_fill(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode { Color32::from_rgb(33, 34, 39) } else { Color32::WHITE }
}

// ---------------------------------------------------------------- components

/// A rounded panel that groups related content.
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

/// Page title with an optional subtitle; `right` adds actions on the right.
pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str, right: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(self::title(title, 22.0));
            if !subtitle.is_empty() {
                hint(ui, subtitle);
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
    ui.add_space(6.0);
}

/// Section title inside a card, with an icon.
pub fn section(ui: &mut Ui, icon: &str, title: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).size(16.0).color(ACCENT));
        ui.label(self::title(title, 15.5));
    });
}

pub fn title(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).family(semibold()).size(size).strong()
}

/// The main action of a view: accent-filled.
pub fn primary(ui: &mut Ui, enabled: bool, text: &str) -> Response {
    ui.add_enabled(enabled, Button::new(RichText::new(text).family(semibold()).color(Color32::WHITE)).fill(ACCENT))
}

/// A destructive confirmation button.
pub fn danger(ui: &mut Ui, text: &str) -> Response {
    let red = ui.visuals().error_fg_color;
    ui.add(Button::new(RichText::new(text).family(semibold()).color(Color32::WHITE)).fill(red))
}

/// A frameless icon button with a tooltip.
pub fn icon_button(ui: &mut Ui, icon: &str, tip: &str) -> Response {
    ui.add(Button::new(RichText::new(icon).size(15.0)).frame(false).min_size(vec2(30.0, 30.0))).on_hover_text(tip)
}

/// Text followed by a copy button.
pub fn copyable(ui: &mut Ui, text: &str, shown: RichText) {
    ui.label(shown);
    if icon_button(ui, icon::COPY, "Copy").clicked() {
        ui.ctx().copy_text(text.to_owned());
    }
}

/// A round tile with an icon and a status dot (None: no dot).
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

/// A small status dot; `None` draws a hollow ring (unknown).
pub fn dot(ui: &mut Ui, color: Option<Color32>) {
    let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
    match color {
        Some(c) => _ = ui.painter().circle_filled(rect.center(), 5.0, c),
        None => _ = ui.painter().circle_stroke(rect.center(), 4.5, (1.5, GREY)),
    }
}

/// A rounded, tinted label such as "Online".
pub fn pill(ui: &mut Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(color.linear_multiply(if ui.visuals().dark_mode { 0.25 } else { 0.14 }))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            // Grey on grey is hard to read: use the normal text colour for it.
            let fg = if color == GREY { ui.visuals().text_color() } else { color };
            ui.label(RichText::new(text).size(12.5).color(fg))
        });
}

/// A sidebar entry; returns true when clicked.
pub fn nav_item(ui: &mut Ui, icon: &str, text: &str, selected: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, text));
    let p = ui.painter();
    if selected || resp.hovered() {
        let fill = if selected {
            ACCENT.linear_multiply(if ui.visuals().dark_mode { 0.25 } else { 0.13 })
        } else {
            ui.visuals().widgets.hovered.weak_bg_fill
        };
        p.rect_filled(rect, CornerRadius::same(8), fill);
    }
    if selected {
        let bar = egui::Rect::from_min_size(rect.left_top() + vec2(0.0, 10.0), vec2(3.0, 18.0));
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

/// A large, centred code (pairing / verification), with a copy button.
pub fn big_code(ui: &mut Ui, code: &str) {
    Frame::new()
        .fill(ACCENT.linear_multiply(if ui.visuals().dark_mode { 0.18 } else { 0.08 }))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    let w = 34.0 + code.len() as f32 * 21.0;
                    ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                    ui.label(RichText::new(code).monospace().size(34.0).strong());
                    if icon_button(ui, icon::COPY, "Copy").clicked() {
                        ui.ctx().copy_text(code.to_owned());
                    }
                });
            });
        });
}

/// Right-aligned dialog buttons, laid out right to left (primary first).
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

/// "5 min ago" style age of a Unix timestamp (seconds).
pub fn ago(then: u64) -> String {
    let now = rider_core::now_us() / 1_000_000;
    let s = now.saturating_sub(then);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        86_400..172_800 => "yesterday".into(),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// `2E1B-049F-6F4C-840D` from a hex fingerprint.
pub fn short_fp(hex: &str) -> String {
    rider_core::identity::fp_from_hex(hex).map(|f| rider_core::identity::fp_short(&f)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_naturally() {
        let now = rider_core::now_us() / 1_000_000;
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - 125), "2 min ago");
        assert_eq!(ago(now - 3 * 3600), "3 h ago");
        assert_eq!(ago(now - 90_000), "yesterday");
        assert_eq!(ago(now - 5 * 86_400), "5 days ago");
    }
}

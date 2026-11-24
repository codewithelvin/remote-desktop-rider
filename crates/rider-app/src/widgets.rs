//! Look and feel (Windows fonts and icons, colours, light/dark), shared
//! components, and the worker → UI message channel.

use eframe::egui::{
    self, Align2, Button, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin,
    Response, RichText, Sense, Stroke, Ui, vec2,
};
use rider_core::i18n::{Lang, tr};
use rider_core::trf;
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
#[cfg(windows)]
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
    pub const DOWNLOAD: &str = "\u{E896}";
}

/// macOS and Linux: the same icons from egui's bundled emoji fonts.
#[cfg(not(windows))]
#[allow(dead_code)] // some are only used by Manage (Windows)
pub mod icon {
    pub const PC: &str = "🖥";
    pub const HOME: &str = "🏠";
    pub const SETTINGS: &str = "⚙";
    pub const ADD: &str = "➕";
    pub const DELETE: &str = "🗑";
    pub const KEY: &str = "🔑";
    pub const SHIELD: &str = "🛡";
    pub const GLOBE: &str = "🌐";
    pub const COPY: &str = "📋";
    pub const DEVICES: &str = "📱";
    pub const CHECK: &str = "✔";
    pub const WARNING: &str = "⚠";
    pub const INFO: &str = "ℹ";
    pub const PLAY: &str = "▶";
    pub const DOWNLOAD: &str = "⬇";
}

fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

// --------------------------------------------------------------------- theme

/// System fonts: (regular, semibold, icons), first file found of each.
/// egui's bundled font lacks letters such as Azerbaijani "ə", so a full
/// system font goes first everywhere.
#[cfg(windows)]
fn font_files() -> [Vec<std::path::PathBuf>; 3] {
    let dir = std::env::var_os("WINDIR").map_or_else(|| r"C:\Windows".into(), std::path::PathBuf::from).join("Fonts");
    let f = |names: &[&str]| names.iter().map(|n| dir.join(n)).collect();
    [f(&["segoeui.ttf"]), f(&["seguisb.ttf"]), f(&["SegoeIcons.ttf", "segmdl2.ttf"])]
}

#[cfg(target_os = "macos")]
fn font_files() -> [Vec<std::path::PathBuf>; 3] {
    let f = |names: &[&str]| names.iter().map(Into::into).collect();
    [
        f(&["/System/Library/Fonts/Supplemental/Arial.ttf", "/Library/Fonts/Arial.ttf"]),
        f(&["/System/Library/Fonts/Supplemental/Arial Bold.ttf", "/Library/Fonts/Arial Bold.ttf"]),
        Vec::new(),
    ]
}

#[cfg(all(unix, not(target_os = "macos")))]
fn font_files() -> [Vec<std::path::PathBuf>; 3] {
    // Noto Sans (Ubuntu, Debian, Fedora), then DejaVu (nearly everywhere). Not
    // the Ubuntu font: its "ə" is a capital-height IPA schwa.
    let f = |names: &[&str]| names.iter().map(|n| std::path::Path::new("/usr/share/fonts").join(n)).collect();
    [
        f(&[
            "truetype/noto/NotoSans-Regular.ttf",
            "noto/NotoSans-Regular.ttf",
            "truetype/dejavu/DejaVuSans.ttf",
            "dejavu-sans-fonts/DejaVuSans.ttf",
            "TTF/DejaVuSans.ttf",
        ]),
        f(&[
            "truetype/noto/NotoSans-SemiBold.ttf",
            "noto/NotoSans-SemiBold.ttf",
            "truetype/noto/NotoSans-Medium.ttf",
            "truetype/noto/NotoSans-Bold.ttf",
            "noto/NotoSans-Bold.ttf",
            "truetype/dejavu/DejaVuSans-Bold.ttf",
            "dejavu-sans-fonts/DejaVuSans-Bold.ttf",
            "TTF/DejaVuSans-Bold.ttf",
        ]),
        Vec::new(),
    ]
}

/// A font with arrows, box drawing and triangles, if the system has one.
fn symbol_files() -> Vec<std::path::PathBuf> {
    if cfg!(windows) {
        return Vec::new(); // Segoe UI has them.
    }
    [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    ]
    .iter()
    .map(Into::into)
    .collect()
}

/// System fonts (Segoe UI on Windows) and the app's colours for both light
/// and dark mode (egui follows the system setting).
pub fn setup(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let [regular, semibold_files, icon_files] = font_files();
    let mut load = |key: &str, files: &[std::path::PathBuf]| -> bool {
        let Some(bytes) = files.iter().find_map(|f| std::fs::read(f).ok()) else { return false };
        fonts.font_data.insert(key.into(), Arc::new(FontData::from_owned(bytes)));
        true
    };
    let text = load("segoe", &regular);
    let bold = load("segoe-semibold", &semibold_files);
    let icons = load("icons", &icon_files);
    // Last resort for symbols the text fonts lack (Noto Sans has no "→", "─"
    // or "▾"): DejaVu Sans where installed, else the bundled Hack.
    let symbols = load("symbols", &symbol_files());

    let mut proportional = fonts.families[&FontFamily::Proportional].clone();
    if text {
        proportional.insert(0, "segoe".into());
    }
    if icons {
        proportional.push("icons".into());
    }
    if symbols {
        proportional.push("symbols".into());
    }
    proportional.push("Hack".into());
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
        // Hover, open and fade transitions: noticeable but quick.
        s.animation_time = 0.16;

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
            // Readable on the dark cards: body text near white, secondary
            // text about 7:1 against the card background (egui's default
            // "weak" grey is closer to 3:1).
            v.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(226, 228, 233);
            v.widgets.inactive.fg_stroke.color = Color32::from_rgb(232, 234, 238);
            v.weak_text_color = Some(Color32::from_rgb(168, 172, 182));
            v.error_fg_color = Color32::from_rgb(255, 128, 116);
            v.warn_fg_color = Color32::from_rgb(242, 184, 84);
        } else {
            v.panel_fill = Color32::from_rgb(242, 243, 246);
            v.window_fill = Color32::WHITE;
            v.extreme_bg_color = Color32::WHITE;
            v.faint_bg_color = Color32::from_rgb(248, 249, 251);
            v.widgets.inactive.weak_bg_fill = Color32::from_rgb(228, 230, 235);
            v.widgets.hovered.weak_bg_fill = Color32::from_rgb(216, 219, 226);
            v.weak_text_color = Some(Color32::from_rgb(88, 92, 104));
            v.error_fg_color = Color32::from_rgb(196, 43, 28);
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border(dark));
        // Text fields need their own outline and fill: egui draws them with
        // no border, so in light mode a white field on a white card (PIN,
        // server, pairing code) was invisible.
        v.text_edit_bg_color = Some(input_fill(dark));
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, input_border(dark));
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, if dark { Color32::from_rgb(110, 114, 126) } else { GREY });
    });
}

/// Force light or dark (`RIDER_THEME=light|dark`), for screenshots and
/// checking both looks; otherwise egui follows the Windows setting.
pub fn theme_override(ctx: &egui::Context) {
    match std::env::var("RIDER_THEME").as_deref() {
        Ok("light") => ctx.set_theme(egui::Theme::Light),
        Ok("dark") => ctx.set_theme(egui::Theme::Dark),
        _ => {}
    }
}

fn input_fill(dark: bool) -> Color32 {
    if dark { Color32::from_rgb(20, 21, 24) } else { Color32::from_rgb(250, 251, 253) }
}

fn input_border(dark: bool) -> Color32 {
    if dark { Color32::from_rgb(72, 75, 84) } else { Color32::from_rgb(190, 194, 204) }
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
    // Buttons share the row with the (short) title only; the subtitle gets
    // the full width below, so long translations never run under them.
    ui.horizontal(|ui| {
        ui.label(self::title(title, 22.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
    if !subtitle.is_empty() {
        hint(ui, subtitle);
    }
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

/// A title at `size`, or smaller if it would be wider than `width` (wide
/// fonts such as DejaVu on Linux), but never below 80 %.
pub fn title_fit(ui: &Ui, text: &str, size: f32, width: f32) -> RichText {
    let font = FontId::new(size, semibold());
    let w = ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font, Color32::WHITE).size().x);
    let size = if w > width { (size * width / w).max(size * 0.8).floor() } else { size };
    title(text, size)
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
    if icon_button(ui, icon::COPY, tr("Copy", "Kopyala")).clicked() {
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

/// The access level the person at this computer gives the other side: three
/// rows (title + what it means), the chosen one marked. True when changed.
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
                    // Not selectable: a click anywhere on the row, text
                    // included, picks it.
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

/// A sidebar entry; returns true when clicked.
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
        // Hover tint fades in; the selection tint takes over as it grows.
        let fill = if sel > 0.0 {
            lerp_color(hovered.linear_multiply(hover), tint, sel)
        } else {
            hovered.linear_multiply(hover)
        };
        p.rect_filled(rect, CornerRadius::same(8), fill);
    }
    if sel > 0.0 {
        // The accent bar grows from the middle.
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
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Fade and slide content in for a moment after `since` (egui time, seconds):
/// page switches, banners. Call at the top of the content.
pub fn appear(ui: &mut Ui, since: f64) {
    let now = ui.input(|i| i.time);
    let p = ((now - since) / 0.22).clamp(0.0, 1.0) as f32;
    if p < 1.0 {
        let e = 1.0 - (1.0 - p).powi(3); // ease out
        ui.multiply_opacity(e);
        ui.add_space((1.0 - e) * 8.0);
        ui.ctx().request_repaint();
    }
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
                    // "Copied" for a moment after a click, so it is clear it worked.
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

/// A dialog that fits its content again whenever `shape` changes (its step,
/// an error appearing or going away). egui otherwise keeps a dialog at the
/// largest size it ever had, leaving empty space below shorter content.
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
        // The sizing pass draws nothing: show the result on the next frame.
        ctx.request_repaint();
    }
    egui::Modal::new(id).area(egui::Modal::default_area(id).sizing_pass(changed))
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
        0..60 => tr("just now", "indicə").into(),
        60..3600 => trf!("{} min ago", "{} dəq. əvvəl", s / 60),
        3600..86_400 => trf!("{} h ago", "{} saat əvvəl", s / 3600),
        86_400..172_800 => tr("yesterday", "dünən").into(),
        _ => trf!("{} days ago", "{} gün əvvəl", s / 86_400),
    }
}

/// English / Azerbaijani switch: two flags. Returns the language picked.
pub fn language_switch(ui: &mut Ui) -> Option<Lang> {
    let current = rider_core::i18n::lang();
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        // Always English first, also inside right-to-left layouts (page headers).
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

/// A clickable flag (30×20) for a language; the current one is outlined.
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
            // Blue, red and green bands; white crescent and eight-pointed star.
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
            // Union Flag: blue field, diagonals, then the cross.
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

/// `2E1B-049F-6F4C-840D` from a hex fingerprint.
pub fn short_fp(hex: &str) -> String {
    rider_core::identity::fp_from_hex(hex).map(|f| rider_core::identity::fp_short(&f)).unwrap_or_default()
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

//! The look of Remote Desktop Rider, shared by the app and the session
//! window: system fonts (Segoe UI on Windows), colours, light and dark.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, Ui, vec2};
use std::sync::Arc;

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
    pub const LOCK: &str = "\u{E72E}";
    pub const GLOBE: &str = "\u{E774}";
    pub const COPY: &str = "\u{E8C8}";
    pub const DEVICES: &str = "\u{E772}";
    pub const CHECK: &str = "\u{E73E}";
    pub const WARNING: &str = "\u{E7BA}";
    pub const INFO: &str = "\u{E946}";
    pub const PLAY: &str = "\u{E768}";
    pub const DOWNLOAD: &str = "\u{E896}";
    pub const CLOSE: &str = "\u{E711}";
    pub const ATTACH: &str = "\u{E723}";
    pub const SEND: &str = "\u{E724}";
    pub const FILE: &str = "\u{E8A5}";
    pub const CHAT: &str = "\u{E8BD}";
    pub const COMPANY: &str = "\u{E716}";
    pub const SEARCH: &str = "\u{E721}";
    pub const REFRESH: &str = "\u{E72C}";
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
    pub const LOCK: &str = "🔒";
    pub const GLOBE: &str = "🌐";
    pub const COPY: &str = "📋";
    pub const DEVICES: &str = "📱";
    pub const CHECK: &str = "✔";
    pub const WARNING: &str = "⚠";
    pub const INFO: &str = "ℹ";
    pub const PLAY: &str = "▶";
    pub const DOWNLOAD: &str = "⬇";
    pub const CLOSE: &str = "✖";
    pub const ATTACH: &str = "📎";
    pub const SEND: &str = "➤";
    pub const FILE: &str = "📄";
    pub const CHAT: &str = "💬";
    pub const COMPANY: &str = "👥";
    pub const SEARCH: &str = "🔍";
    pub const REFRESH: &str = "🔄";
}

pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

// --------------------------------------------------------------------- theme

/// System fonts: (regular, semibold, icons), first file found of each.
/// egui's bundled font lacks letters such as Azerbaijani "ə", so a full
/// system font goes first everywhere.
#[cfg(windows)]
pub fn font_files() -> [Vec<std::path::PathBuf>; 3] {
    let dir = std::env::var_os("WINDIR").map_or_else(|| r"C:\Windows".into(), std::path::PathBuf::from).join("Fonts");
    let f = |names: &[&str]| names.iter().map(|n| dir.join(n)).collect();
    [f(&["segoeui.ttf"]), f(&["seguisb.ttf"]), f(&["SegoeIcons.ttf", "segmdl2.ttf"])]
}

#[cfg(target_os = "macos")]
pub fn font_files() -> [Vec<std::path::PathBuf>; 3] {
    let f = |names: &[&str]| names.iter().map(Into::into).collect();
    [
        f(&["/System/Library/Fonts/Supplemental/Arial.ttf", "/Library/Fonts/Arial.ttf"]),
        f(&["/System/Library/Fonts/Supplemental/Arial Bold.ttf", "/Library/Fonts/Arial Bold.ttf"]),
        Vec::new(),
    ]
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn font_files() -> [Vec<std::path::PathBuf>; 3] {
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
pub fn symbol_files() -> Vec<std::path::PathBuf> {
    if cfg!(windows) {
        // Segoe UI has the symbols; emoji in chat messages come from Segoe
        // UI Emoji (drawn in one colour).
        let dir = std::env::var_os("WINDIR").map_or_else(|| r"C:\Windows".into(), std::path::PathBuf::from);
        return vec![dir.join("Fonts").join("seguiemj.ttf")];
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

pub fn input_fill(dark: bool) -> Color32 {
    if dark { Color32::from_rgb(20, 21, 24) } else { Color32::from_rgb(250, 251, 253) }
}

pub fn input_border(dark: bool) -> Color32 {
    if dark { Color32::from_rgb(72, 75, 84) } else { Color32::from_rgb(190, 194, 204) }
}

pub fn border(dark: bool) -> Color32 {
    if dark { Color32::from_rgb(52, 54, 61) } else { Color32::from_rgb(222, 225, 231) }
}

pub fn card_fill(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode { Color32::from_rgb(33, 34, 39) } else { Color32::WHITE }
}

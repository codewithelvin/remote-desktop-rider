//! Session toolbar: a small bar at the top centre of the viewer with the
//! session time, Full screen, Ctrl+Alt+Del, one button per remote monitor,
//! Quality, Pin and Disconnect. It shows for a few seconds when the session
//! starts; after that only a small tab stays at the top centre, and clicking
//! that tab opens the bar. Pinned, it stays open. It never opens just because
//! the pointer reaches the top edge: that is where remote browser tabs and
//! title bars are, and a bar sliding in under the pointer would take the click
//! meant for them (e.g. turn "close tab" into Disconnect).
//!
//! Pinned, the bar does sit over remote tabs and title bars, so Disconnect
//! takes two clicks: the first turns it into "Click again to disconnect" for a
//! few seconds. A stray click (closing a remote tab) cannot end the session.
//!
//! The bar is rendered once per state (labels: see `text`; on Windows GDI with
//! Segoe UI and the Segoe MDL2 icon font) into a premultiplied ARGB
//! sprite, then alpha-blended over each frame: a few thousand pixels per draw.

use rider_core::i18n::tr;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Fullscreen,
    CtrlAltDel,
    /// Show this remote monitor (0-based).
    Monitor(u32),
    /// Open the quality menu.
    Quality,
    /// Keep the bar open (or not).
    Pin,
    Disconnect,
    /// The session time: shown, not clickable.
    Elapsed,
    /// The access level when it is not full access: shown, not clickable.
    Access,
    /// Open or close the chat.
    Chat,
}

impl Action {
    /// Shown on the bar but not a button.
    fn passive(self) -> bool {
        matches!(self, Action::Elapsed | Action::Access)
    }
}

/// What the bar shows besides the buttons that are always there.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Extras {
    /// Session time, e.g. "12:34" (hidden when empty).
    pub elapsed: String,
    /// Current quality label ("Auto", "720p"); `None` hides the button
    /// (the host cannot change it).
    pub quality: Option<String>,
    pub pinned: bool,
    /// The access level, when it is not full access ("View only").
    pub access: Option<String>,
    /// The person at the computer does not allow control: no Ctrl+Alt+Del.
    pub no_control: bool,
    /// The host has chat: the button, with this many unread messages.
    pub chat: Option<usize>,
    /// The chat panel is open (the button is highlighted).
    pub chat_open: bool,
    /// The remote monitors' names, as the host sends them.
    pub monitor_names: Vec<String>,
}

/// Shown at the start of a session so people learn where it is.
const INTRO: Duration = Duration::from_secs(4);
/// Stays this long after the pointer leaves it.
const LINGER: Duration = Duration::from_millis(800);
/// Opened by its tab but not reached by the pointer: hide again after this.
const OPEN_WAIT: Duration = Duration::from_secs(3);
/// After a first click on Disconnect, a second one within this ends the session.
const CONFIRM: Duration = Duration::from_secs(4);
/// The tab shown while the bar is hidden (logical px).
const TAB_W: f64 = 56.0;
const TAB_H: f64 = 12.0;

pub(crate) const BG: (u8, u8, u8, u8) = (30, 31, 36, 238);
const HOVER: (u8, u8, u8) = (62, 64, 74);
const ACCENT: (u8, u8, u8) = (47, 111, 235);
const DANGER: (u8, u8, u8) = (196, 43, 28);

struct Sprite {
    w: usize,
    h: usize,
    px: Vec<u32>,
    /// Horizontal extent of each button inside the sprite.
    buttons: Vec<(Action, usize, usize)>,
}

pub struct Toolbar {
    scale: f64,
    fullscreen: bool,
    /// Remote monitor count and the one shown.
    monitors: (u32, u32),
    extras: Extras,
    hover: Option<Action>,
    pressed: Option<Action>,
    /// Visible until this instant (None: pointer is on it, stay).
    visible_until: Option<Instant>,
    on_bar: bool,
    sprite: Option<Sprite>,
    /// Where the sprite was last drawn (x offset; it is always at y = 0).
    x: usize,
    /// Horizontal centre of the last drawn frame (where the tab is).
    centre: f64,
    /// The pointer is on the tab, and whether it was pressed there.
    tab_hover: bool,
    tab_pressed: bool,
    last: (f64, f64),
    /// Disconnect was clicked once: a second click before this ends the session.
    confirm_until: Option<Instant>,
}

impl Toolbar {
    pub fn new() -> Self {
        Self {
            scale: 1.0,
            fullscreen: false,
            monitors: (0, 0),
            extras: Extras::default(),
            hover: None,
            pressed: None,
            visible_until: Some(Instant::now() + INTRO),
            on_bar: false,
            sprite: None,
            x: 0,
            centre: 0.0,
            tab_hover: false,
            tab_pressed: false,
            last: (-1.0, -1.0),
            confirm_until: None,
        }
    }

    /// Update the state shown on the bar; returns true if it must be redrawn.
    pub fn set_state(&mut self, scale: f64, fullscreen: bool, monitors: (u32, u32)) -> bool {
        if (scale - self.scale).abs() < f64::EPSILON && fullscreen == self.fullscreen && monitors == self.monitors {
            return false;
        }
        self.scale = scale;
        self.fullscreen = fullscreen;
        self.monitors = monitors;
        self.sprite = None;
        true
    }

    /// Update the time, quality and pin shown; returns true if it must be redrawn.
    pub fn set_extras(&mut self, extras: Extras) -> bool {
        if extras == self.extras {
            return false;
        }
        self.extras = extras;
        self.sprite = None;
        self.visible()
    }

    /// Show the bar for a while (something new on it, e.g. a chat message).
    pub fn reveal(&mut self) {
        let until = Instant::now() + INTRO;
        if self.visible_until.is_none_or(|t| t < until) && !self.on_bar {
            self.visible_until = Some(until);
            self.sprite = None;
        }
    }

    /// The pointer is on the bar or its tab (shown as a plain arrow there).
    pub fn hovering(&self) -> bool {
        self.on_bar || self.hover.is_some() || self.tab_hover
    }

    pub fn visible(&self) -> bool {
        self.extras.pinned || self.on_bar || self.confirming() || self.visible_until.is_some_and(|t| Instant::now() < t)
    }

    /// Disconnect is waiting for its second click.
    fn confirming(&self) -> bool {
        self.confirm_until.is_some_and(|t| Instant::now() < t)
    }

    /// Drop an unconfirmed Disconnect once its time is up; true if the bar
    /// must be redrawn (the button reads "Disconnect" again).
    pub fn expire_confirm(&mut self) -> bool {
        if self.confirm_until.is_some() && !self.confirming() {
            self.confirm_until = None;
            self.sprite = None;
            return true;
        }
        false
    }

    /// A button's horizontal extent and the bar's height (window px), to
    /// open a menu under it.
    #[cfg_attr(not(windows), allow(dead_code))] // the quality menu is a Windows popup
    pub fn button_rect(&self, action: Action) -> Option<(usize, usize, usize)> {
        let s = self.sprite.as_ref()?;
        let &(_, a, b) = s.buttons.iter().find(|b| b.0 == action)?;
        Some((self.x + a, self.x + b, s.h))
    }

    /// When the bar should be hidden next (to schedule a redraw then).
    pub fn deadline(&self) -> Option<Instant> {
        let hide =
            if self.on_bar || self.extras.pinned { None } else { self.visible_until.filter(|t| Instant::now() < *t) };
        match (hide, self.confirm_until) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Pointer moved to (x, y) physical px. Returns true if the bar changed.
    pub fn pointer(&mut self, x: f64, y: f64) -> bool {
        self.last = (x, y);
        let before = (self.visible(), self.hover, self.tab_hover);
        let on = self.visible() && self.over_bar(x, y);
        self.tab_hover = !self.visible() && self.over_tab(x, y);
        if on {
            self.on_bar = true;
            self.visible_until = None;
        } else if self.on_bar {
            self.on_bar = false;
            self.visible_until = Some(Instant::now() + LINGER);
        }
        self.set_hover(if self.visible() { self.hit(x, y).filter(|a| !a.passive()) } else { None });
        (self.visible(), self.hover, self.tab_hover) != before
    }

    fn set_hover(&mut self, hover: Option<Action>) {
        if hover != self.hover {
            self.hover = hover;
            self.sprite = None;
        }
    }

    /// Pointer left the window.
    pub fn leave(&mut self) -> bool {
        let was = self.on_bar || self.hover.is_some() || self.tab_hover;
        self.tab_hover = false;
        self.last = (-1.0, -1.0);
        if self.on_bar {
            self.visible_until = Some(Instant::now() + LINGER);
        }
        self.on_bar = false;
        self.set_hover(None);
        was
    }

    /// A left-button press at the pointer: true if the bar takes it.
    pub fn press(&mut self) -> bool {
        if !self.visible() {
            self.tab_pressed = self.tab_hover;
            return self.tab_pressed;
        }
        if !self.on_bar {
            return false;
        }
        self.pressed = self.hover;
        true
    }

    /// The matching release: the action to run, if released on the same button.
    /// `None` with `taken == true` means the bar swallowed the click.
    pub fn release(&mut self) -> (bool, Option<Action>) {
        if std::mem::take(&mut self.tab_pressed) {
            if self.tab_hover {
                // Open; it stays while the pointer is on it (it is right there).
                self.tab_hover = false;
                self.visible_until = Some(Instant::now() + OPEN_WAIT);
                let (x, y) = self.last;
                self.pointer(x, y);
            }
            return (true, None);
        }
        match self.pressed.take() {
            Some(Action::Disconnect) if self.hover == Some(Action::Disconnect) => {
                if self.confirming() {
                    self.confirm_until = None;
                    return (true, Some(Action::Disconnect));
                }
                self.confirm_until = Some(Instant::now() + CONFIRM);
                self.sprite = None;
                (true, None)
            }
            Some(a) => (true, (self.hover == Some(a) && !a.passive()).then_some(a)),
            None => (self.on_bar && self.visible(), None),
        }
    }

    fn sprite_size(&self) -> (usize, usize) {
        self.sprite.as_ref().map_or((0, 0), |s| (s.w, s.h))
    }

    fn over_bar(&self, x: f64, y: f64) -> bool {
        let (w, h) = self.sprite_size();
        x >= self.x as f64 && x < (self.x + w) as f64 && y >= 0.0 && y < h as f64
    }

    fn over_tab(&self, x: f64, y: f64) -> bool {
        let (hw, h) = (TAB_W * self.scale / 2.0, TAB_H * self.scale);
        y >= 0.0 && y < h && (x - self.centre).abs() < hw
    }

    fn hit(&self, x: f64, y: f64) -> Option<Action> {
        if !self.visible() || !self.over_bar(x, y) {
            return None;
        }
        let sx = x as usize - self.x;
        self.sprite.as_ref()?.buttons.iter().find(|(_, a, b)| sx >= *a && sx < *b).map(|(act, _, _)| *act)
    }

    /// Draw over a frame of `w` x `h` pixels (XRGB).
    pub fn draw(&mut self, buf: &mut [u32], w: usize, h: usize) {
        if self.sprite.is_none() {
            self.sprite =
                render(self.scale, self.fullscreen, self.monitors, &self.extras, self.hover, self.confirming());
        }
        let Some(s) = &self.sprite else { return };
        self.x = w.saturating_sub(s.w) / 2;
        self.centre = w as f64 / 2.0;
        if !self.visible() {
            tab(buf, w, h, self.scale, self.tab_hover);
            return;
        }
        for y in 0..s.h.min(h) {
            let row = &mut buf[y * w + self.x..][..s.w.min(w - self.x)];
            for (d, &p) in row.iter_mut().zip(&s.px[y * s.w..]) {
                *d = blend(p, *d);
            }
        }
    }
}

impl Default for Toolbar {
    fn default() -> Self {
        Self::new()
    }
}

/// The closed toolbar: a small dark tab at the top centre, rounded at the
/// bottom, with a short grip line; lighter while the pointer is on it.
fn tab(buf: &mut [u32], w: usize, h: usize, scale: f64, hover: bool) {
    let (tw, th) = ((TAB_W * scale).round() as usize, (TAB_H * scale).round() as usize);
    let x0 = w.saturating_sub(tw) / 2;
    let r = 5.0 * scale;
    let (bg, grip) = if hover { (HOVER, 255u8) } else { ((BG.0, BG.1, BG.2), 190) };
    let (gw, gy0) = (tw as f64 * 0.4, th as f64 * 0.35);
    let gy1 = gy0 + (2.0 * scale).max(1.5);
    for y in 0..th.min(h) {
        for x in 0..tw.min(w - x0) {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let (cx, cy) = (fx.clamp(r, tw as f64 - r), fy.min(th as f64 - r));
            let a = (r - ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0);
            if a == 0.0 {
                continue;
            }
            let on_grip = (fx - tw as f64 / 2.0).abs() < gw / 2.0 && fy >= gy0 && fy < gy1;
            let (cr, cg, cb) = if on_grip { (grip, grip, grip) } else { bg };
            let d = &mut buf[y * w + x0 + x];
            *d = blend(premul(cr, cg, cb, (BG.3 as f64 * a) as u8), *d);
        }
    }
}

/// Premultiplied ARGB.
fn premul(r: u8, g: u8, b: u8, a: u8) -> u32 {
    let m = |c: u8| (c as u32 * a as u32 + 127) / 255;
    (a as u32) << 24 | m(r) << 16 | m(g) << 8 | m(b)
}

pub(crate) fn blend(src: u32, dst: u32) -> u32 {
    let a = src >> 24;
    if a == 255 {
        return src & 0x00FF_FFFF;
    }
    let inv = 255 - a;
    let ch = |s: u32| {
        let d = (dst >> s) & 0xFF;
        (((src >> s) & 0xFF) + (d * inv + 127) / 255).min(255) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// Rounded hover pill inside a button.
fn inside_round(x: usize, y: usize, w: usize, h: usize, scale: f64) -> bool {
    let inset = 4.0 * scale;
    let r = 6.0 * scale;
    let (x, y) = (x as f64 + 0.5, y as f64 + 0.5);
    let (x0, y0, x1, y1) = (inset, inset, w as f64 - inset, h as f64 - inset);
    if x < x0 || x > x1 || y < y0 || y > y1 {
        return false;
    }
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

/// Buttons: action, icon (Windows icon font; none elsewhere) and label.
fn labels(
    fullscreen: bool,
    (count, _): (u32, u32),
    extras: &Extras,
    confirming: bool,
) -> Vec<(Action, Option<char>, String)> {
    let icon = |c: char| cfg!(windows).then_some(c);
    let mut v = Vec::new();
    if !extras.elapsed.is_empty() {
        v.push((Action::Elapsed, icon('\u{E916}'), extras.elapsed.clone()));
    }
    if let Some(access) = &extras.access {
        // An eye: what the other side lets you do.
        v.push((Action::Access, icon('\u{E890}'), access.clone()));
    }
    v.push(if fullscreen {
        (Action::Fullscreen, icon('\u{E73F}'), tr("Exit full screen", "Tam ekrandan çıx").into())
    } else {
        (Action::Fullscreen, icon('\u{E740}'), tr("Full screen", "Tam ekran").into())
    });
    if !extras.no_control {
        v.push((Action::CtrlAltDel, icon('\u{E765}'), "Ctrl+Alt+Del".into()));
    }
    if count > 1 {
        v.extend((0..count).map(|i| {
            let name = extras.monitor_names.get(i as usize).map(String::as_str);
            (Action::Monitor(i), icon('\u{E7F4}'), monitor_label(i, name))
        }));
    }
    if let Some(unread) = extras.chat {
        let label = tr("Chat", "Söhbət");
        let text = if unread > 0 { format!("{label} \u{2022} {unread}") } else { label.into() };
        v.push((Action::Chat, icon('\u{E8BD}'), text));
    }
    if let Some(q) = &extras.quality {
        // Label plus a down chevron: it opens a menu.
        v.push((Action::Quality, icon('\u{E7B3}'), format!("{q}  \u{25BE}")));
    }
    // Without the icon font the pin needs a word (the pill shows it is on).
    let pin = if cfg!(windows) { String::new() } else { tr("Pin", "Sabitlə").into() };
    v.push((Action::Pin, icon(if extras.pinned { '\u{E77A}' } else { '\u{E718}' }), pin));
    let disconnect = if confirming {
        tr("Click again to disconnect", "Kəsmək üçün yenə klikləyin")
    } else {
        tr("Disconnect", "Bağlantını kəs")
    };
    v.push((Action::Disconnect, icon('\u{E711}'), disconnect.into()));
    v
}

/// One label to paint: left edge, icon and its width (Windows only), text.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct Label<'a> {
    pub x: i32,
    pub icon: Option<char>,
    pub icon_w: i32,
    pub text: &'a str,
}

#[cfg(windows)]
pub(crate) mod text {
    //! GDI: Segoe UI and the Segoe MDL2 icon font, grayscale anti-aliasing.
    use super::Label;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS, CreateCompatibleDC,
        CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DIB_RGB_COLORS, DeleteDC, DeleteObject, FF_DONTCARE, FW_NORMAL,
        GdiFlush, GetTextExtentPoint32W, HDC, HFONT, OUT_TT_PRECIS, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
        TextOutW,
    };
    use windows::core::PCWSTR;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    pub struct Fonts {
        dc: HDC,
        text: HFONT,
        icons: HFONT,
        scale: f64,
    }

    impl Fonts {
        pub fn new(scale: f64) -> Option<Self> {
            let font = |px: f64, face: &str| {
                let face: Vec<u16> = face.encode_utf16().chain(std::iter::once(0)).collect();
                // SAFETY: face is NUL-terminated and outlives the call.
                unsafe {
                    CreateFontW(
                        -(px * scale).round() as i32,
                        0,
                        0,
                        0,
                        FW_NORMAL.0 as i32,
                        0,
                        0,
                        0,
                        DEFAULT_CHARSET,
                        OUT_TT_PRECIS,
                        CLIP_DEFAULT_PRECIS,
                        ANTIALIASED_QUALITY,
                        FF_DONTCARE.0 as u32,
                        PCWSTR(face.as_ptr()),
                    )
                }
            };
            // SAFETY: a memory DC with no window; released in Drop.
            let dc = unsafe { CreateCompatibleDC(None) };
            if dc.is_invalid() {
                return None;
            }
            Some(Self { dc, text: font(13.0, "Segoe UI"), icons: font(14.0, "Segoe MDL2 Assets"), scale })
        }

        fn extent(&self, font: HFONT, s: &[u16]) -> (i32, i32) {
            let mut size = SIZE::default();
            // SAFETY: valid DC and font; `size` is a local out-param.
            unsafe {
                SelectObject(self.dc, font.into());
                let _ = GetTextExtentPoint32W(self.dc, s, &mut size);
            }
            (size.cx, size.cy)
        }

        pub fn icon_width(&self, c: char) -> i32 {
            self.extent(self.icons, &wide(&c.to_string())).0
        }

        pub fn text_width(&self, s: &str) -> i32 {
            self.extent(self.text, &wide(s)).0
        }

        /// White-on-black coverage (0-255) of all labels, `w` x `h`.
        pub fn paint(&self, w: usize, h: usize, labels: &[Label], gap: i32) -> Option<Vec<u8>> {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let hi = h as i32;
            let one = self.scale.round() as i32;
            let mut bits = std::ptr::null_mut();
            // SAFETY: `bits` receives the DIB's memory, valid until the bitmap is deleted.
            let bmp = unsafe { CreateDIBSection(Some(self.dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
            let mut coverage = vec![0u8; w * h];
            // SAFETY: drawing into our own DIB; it is read back after GdiFlush and
            // deleted before `bits` goes out of use.
            unsafe {
                let old = SelectObject(self.dc, bmp.into());
                SetBkMode(self.dc, TRANSPARENT);
                SetTextColor(self.dc, windows::Win32::Foundation::COLORREF(0x00FF_FFFF));
                for l in labels {
                    let mut x = l.x;
                    if let Some(c) = l.icon {
                        let icon = wide(&c.to_string());
                        let (_, ih) = self.extent(self.icons, &icon);
                        let _ = TextOutW(self.dc, x, (hi - ih) / 2 + one, &icon);
                        x += l.icon_w + gap;
                    }
                    let text = wide(l.text);
                    let (_, th) = self.extent(self.text, &text);
                    let _ = TextOutW(self.dc, x, (hi - th) / 2, &text);
                }
                let _ = GdiFlush();
                let src = std::slice::from_raw_parts(bits as *const u32, w * h);
                for (c, p) in coverage.iter_mut().zip(src) {
                    let (r, g, b) = ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF);
                    *c = r.max(g).max(b) as u8;
                }
                SelectObject(self.dc, old);
                let _ = DeleteObject(bmp.into());
            }
            Some(coverage)
        }
    }

    impl Drop for Fonts {
        fn drop(&mut self) {
            // SAFETY: objects created in `new`, no longer selected after DeleteDC.
            unsafe {
                let _ = DeleteDC(self.dc);
                let _ = DeleteObject(self.text.into());
                let _ = DeleteObject(self.icons.into());
            }
        }
    }
}

#[cfg(not(windows))]
pub(crate) mod text {
    //! Labels rasterized with ab_glyph: a full system font first (egui's
    //! bundled Ubuntu Light lacks letters such as "ə"), then the bundled
    //! fonts, then symbol fonts for "▾". Each character comes from the first
    //! font that has it.
    use super::Label;
    use ab_glyph::{Font, FontArc, FontVec, GlyphId, PxScale, ScaleFont, point};

    /// The first of these that exists is used.
    #[cfg(target_os = "macos")]
    const SYSTEM: &[&str] = &["/System/Library/Fonts/Supplemental/Arial.ttf", "/Library/Fonts/Arial.ttf"];
    #[cfg(not(target_os = "macos"))]
    /// Not the Ubuntu font: its "ə" is a capital-height IPA schwa.
    const SYSTEM: &[&str] = &[
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ];

    pub struct Fonts {
        /// In order of preference; never empty.
        fonts: Vec<FontArc>,
        px: PxScale,
    }

    impl Fonts {
        pub fn new(scale: f64) -> Option<Self> {
            let system = SYSTEM
                .iter()
                .find_map(|p| std::fs::read(p).ok())
                .and_then(|b| FontVec::try_from_vec(b).ok())
                .map(FontArc::new);
            let bundled = [
                epaint_default_fonts::UBUNTU_LIGHT,
                epaint_default_fonts::HACK_REGULAR,
                epaint_default_fonts::NOTO_EMOJI_REGULAR,
                epaint_default_fonts::EMOJI_ICON,
            ]
            .into_iter()
            .filter_map(|b| FontArc::try_from_slice(b).ok());
            let fonts: Vec<FontArc> = system.into_iter().chain(bundled).collect();
            (!fonts.is_empty()).then(|| Self { fonts, px: PxScale::from((16.0 * scale) as f32) })
        }

        pub fn icon_width(&self, _c: char) -> i32 {
            0
        }

        /// The font that has `c` (the first one if none does) and its glyph.
        fn glyph(&self, c: char) -> (usize, GlyphId) {
            self.fonts
                .iter()
                .enumerate()
                .map(|(i, f)| (i, f.glyph_id(c)))
                .find(|(_, id)| id.0 != 0)
                .unwrap_or((0, self.fonts[0].glyph_id(c)))
        }

        /// Pen positions: (font, glyph, x) per character, and the end x.
        fn layout(&self, s: &str, x0: f32) -> (Vec<(usize, GlyphId, f32)>, f32) {
            let mut x = x0;
            let mut prev: Option<(usize, GlyphId)> = None;
            let mut out = Vec::with_capacity(s.len());
            for c in s.chars() {
                let (i, id) = self.glyph(c);
                let f = self.fonts[i].as_scaled(self.px);
                if let Some((pi, p)) = prev
                    && pi == i
                {
                    x += f.kern(p, id);
                }
                out.push((i, id, x));
                x += f.h_advance(id);
                prev = Some((i, id));
            }
            (out, x)
        }

        pub fn text_width(&self, s: &str) -> i32 {
            self.layout(s, 0.0).1.ceil() as i32
        }

        /// White-on-black coverage (0-255) of all labels, `w` x `h`.
        pub fn paint(&self, w: usize, h: usize, labels: &[Label], _gap: i32) -> Option<Vec<u8>> {
            let main = self.fonts[0].as_scaled(self.px);
            let mut coverage = vec![0u8; w * h];
            let baseline = ((h as f32 - (main.ascent() - main.descent())) / 2.0 + main.ascent()).round();
            for l in labels {
                for (i, id, x) in self.layout(l.text, l.x as f32).0 {
                    let glyph = id.with_scale_and_position(self.px, point(x, baseline));
                    let Some(outline) = self.fonts[i].outline_glyph(glyph) else { continue };
                    let b = outline.px_bounds();
                    outline.draw(|gx, gy, v| {
                        let (px, py) = (b.min.x as i64 + gx as i64, b.min.y as i64 + gy as i64);
                        if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                            let d = &mut coverage[py as usize * w + px as usize];
                            *d = (*d).max((v.clamp(0.0, 1.0) * 255.0) as u8);
                        }
                    });
                }
            }
            Some(coverage)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_label_character_has_a_glyph() {
            let f = Fonts::new(1.0).unwrap();
            for c in
                "Kompüterlər Sürətli ə Ə ğ ş ç ö ı İ 1080p \u{25BE} \u{2026}".chars().filter(|c| !c.is_whitespace())
            {
                assert_ne!(f.glyph(c).1.0, 0, "{c:?}");
            }
        }
    }
}

/// "1 DELL U2720Q": the number (Alt+1) and the monitor's own name, cut to fit.
/// Hosts before 0.5 send only device names ("DISPLAY1"): then the number alone.
pub(crate) fn monitor_label(i: u32, name: Option<&str>) -> String {
    const MAX: usize = 18;
    match name.map(str::trim).filter(|n| !n.is_empty() && !n.starts_with("DISPLAY")) {
        Some(n) if n.chars().count() > MAX => {
            format!("{} {}\u{2026}", i + 1, n.chars().take(MAX - 1).collect::<String>())
        }
        Some(n) => format!("{} {n}", i + 1),
        None => (i + 1).to_string(),
    }
}

fn render(
    scale: f64,
    fullscreen: bool,
    monitors: (u32, u32),
    extras: &Extras,
    hover: Option<Action>,
    confirming: bool,
) -> Option<Sprite> {
    let fonts = text::Fonts::new(scale)?;
    let s = |v: f64| (v * scale).round() as i32;
    let (pad, gap, h) = (s(14.0), s(8.0), s(36.0));
    let items: Vec<_> = labels(fullscreen, monitors, extras, confirming)
        .into_iter()
        .map(|(act, icon, text)| {
            let iw = icon.map_or(0, |c| fonts.icon_width(c));
            let tw = fonts.text_width(&text);
            (act, icon, text, iw, tw)
        })
        .collect();
    let edge = s(6.0);
    let mut buttons = Vec::new();
    let mut x = edge;
    for (act, _, _, iw, tw) in &items {
        let inner = if *iw > 0 && *tw > 0 { iw + gap + tw } else { iw + tw };
        let bw = pad + inner + pad;
        buttons.push((*act, x as usize, (x + bw) as usize));
        x += bw;
    }
    let w = (x + edge) as usize;
    let hu = h as usize;

    let painted: Vec<Label> = items
        .iter()
        .zip(&buttons)
        .map(|((_, icon, text, iw, _), (_, x0, _))| Label { x: *x0 as i32 + pad, icon: *icon, icon_w: *iw, text })
        .collect();
    let coverage = fonts.paint(w, hu, &painted, gap)?;

    // Background: rounded at the bottom, flat against the top edge.
    let r = 10.0 * scale;
    // The monitor on screen is marked; the hovered button is lighter (or red).
    let pill = |act: Action| buttons.iter().find(|b| b.0 == act).map(|&(_, a, b)| (a, b));
    let active = (monitors.0 > 1).then(|| pill(Action::Monitor(monitors.1))).flatten().map(|(a, b)| (a, b, ACCENT));
    let pinned = extras.pinned.then(|| pill(Action::Pin)).flatten().map(|(a, b)| (a, b, ACCENT));
    // Unread messages light the chat button up too, so they are not missed.
    let chat_lit = extras.chat_open || extras.chat.is_some_and(|n| n > 0);
    let chat = chat_lit.then(|| pill(Action::Chat)).flatten().map(|(a, b)| (a, b, ACCENT));
    let confirm = confirming.then(|| pill(Action::Disconnect)).flatten().map(|(a, b)| (a, b, DANGER));
    let hovered = hover
        .and_then(|h| pill(h).map(|(a, b)| (a, b, if h == Action::Disconnect { DANGER } else { HOVER })))
        .filter(|_| hover != Some(Action::Monitor(monitors.1)) || monitors.0 < 2);
    let mut px = vec![0u32; w * hu];
    for y in 0..hu {
        for x in 0..w {
            let in_pill = |p: Option<(usize, usize, (u8, u8, u8))>| {
                p.filter(|&(a, b, _)| x >= a && x < b && inside_round(x - a, y, b - a, hu, scale)).map(|p| p.2)
            };
            let bg = in_pill(hovered)
                .or_else(|| in_pill(confirm))
                .or_else(|| in_pill(active))
                .or_else(|| in_pill(pinned))
                .or_else(|| in_pill(chat))
                .unwrap_or((BG.0, BG.1, BG.2));
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let cx = fx.clamp(r, w as f64 - r);
            let cy = fy.min(hu as f64 - r);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let a_shape = (r - d + 0.5).clamp(0.0, 1.0);
            if a_shape == 0.0 {
                continue;
            }
            let a = (BG.3 as f64 * a_shape) as u32;
            let t = coverage[y * w + x] as u32;
            // Text over the background, both premultiplied by the shape alpha.
            let mix = |c: u8| ((c as u32 * (255 - t) + 255 * t) / 255 * a + 127) / 255;
            px[y * w + x] = a << 24 | mix(bg.0) << 16 | mix(bg.1) << 8 | mix(bg.2);
        }
    }
    Some(Sprite { w, h: hu, px, buttons })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_labels() {
        assert_eq!(monitor_label(0, Some("DELL U2720Q")), "1 DELL U2720Q");
        assert_eq!(monitor_label(1, Some("Built-in display")), "2 Built-in display");
        // Older hosts: device names only, then the number alone.
        assert_eq!(monitor_label(2, Some("DISPLAY3")), "3");
        assert_eq!(monitor_label(0, None), "1");
        assert_eq!(monitor_label(0, Some("A very long monitor model name")), "1 A very long monit\u{2026}");
    }

    #[test]
    fn blend_math() {
        assert_eq!(blend(premul(255, 0, 0, 255), 0x0000_FF00), 0x00FF_0000);
        assert_eq!(blend(0, 0x0012_3456), 0x0012_3456);
        let half = blend(premul(255, 255, 255, 128), 0);
        assert!((0x7F..=0x81).contains(&(half & 0xFF)));
    }

    #[test]
    fn renders_and_hits() {
        let mut t = Toolbar::new();
        t.set_state(1.0, false, (2, 0));
        let mut buf = vec![0u32; 1200 * 800];
        t.draw(&mut buf, 1200, 800);
        let s = t.sprite.as_ref().expect("sprite");
        assert_eq!(s.buttons.len(), 6, "full screen, ctrl+alt+del, 2 monitors, pin, disconnect");
        assert!(s.w > 300 && s.w < 1000 && s.h == 36);
        // Some label pixels are bright.
        assert!(s.px.iter().any(|p| (p & 0xFF) > 200));
        // Hovering the first button during the intro.
        let (a, b) = (s.buttons[0].1, s.buttons[0].2);
        let x = (t.x + (a + b) / 2) as f64;
        assert!(t.pointer(x, 10.0));
        assert_eq!(t.hover, Some(Action::Fullscreen));
        assert!(t.press());
        assert_eq!(t.release(), (true, Some(Action::Fullscreen)));
    }

    #[test]
    fn top_edge_does_not_open_it_the_tab_does() {
        let mut t = Toolbar::new();
        t.set_state(1.25, false, (1, 0));
        let mut buf = vec![0u32; 1920 * 1080];
        t.draw(&mut buf, 1920, 1080);
        // Intro over, pointer elsewhere: hidden.
        t.visible_until = None;
        t.pointer(500.0, 300.0);
        assert!(!t.visible());
        // A remote browser tab at the top edge: the bar stays closed and the
        // click goes to the remote computer.
        for x in [300.0, 700.0, 1100.0] {
            t.pointer(x, 2.0);
            assert!(!t.visible(), "top edge at {x} must not open it");
            assert!(!t.press());
            assert_eq!(t.release(), (false, None));
        }
        // Clicking the tab opens it, and the click goes nowhere else.
        assert!(t.pointer(960.0, 4.0), "tab highlights");
        assert!(t.press());
        assert_eq!(t.release(), (true, None));
        assert!(t.visible());
        t.draw(&mut buf, 1920, 1080);
        t.pointer(960.0, 20.0);
        assert!(t.visible(), "stays while on it");
        t.pointer(960.0, 500.0);
        assert!(t.visible(), "lingers briefly");
        assert!(t.deadline().is_some());
    }

    #[test]
    fn extras_time_quality_and_pin() {
        let mut t = Toolbar::new();
        t.set_state(1.0, false, (1, 0));
        let extras = Extras { elapsed: "1:05".into(), quality: Some("720p".into()), pinned: true, ..Extras::default() };
        t.set_extras(extras.clone());
        let mut buf = vec![0u32; 1600 * 900];
        t.draw(&mut buf, 1600, 900);
        let acts: Vec<Action> = t.sprite.as_ref().unwrap().buttons.iter().map(|b| b.0).collect();
        assert_eq!(
            acts,
            [Action::Elapsed, Action::Fullscreen, Action::CtrlAltDel, Action::Quality, Action::Pin, Action::Disconnect]
        );
        // Pinned: stays open with the pointer away and no deadline.
        t.visible_until = None;
        t.pointer(800.0, 600.0);
        assert!(t.visible() && t.deadline().is_none());
        // The time is not a button.
        let (a, b, _) = t.button_rect(Action::Elapsed).unwrap();
        t.pointer(((a + b) / 2) as f64, 10.0);
        assert_eq!(t.hover, None);
        assert!(t.press());
        assert_eq!(t.release(), (true, None));
        assert!(!t.set_extras(extras), "unchanged");
    }

    #[test]
    fn view_only_shows_the_level_and_no_ctrl_alt_del() {
        let mut t = Toolbar::new();
        t.set_state(1.0, false, (1, 0));
        t.set_extras(Extras { access: Some("View only".into()), no_control: true, ..Extras::default() });
        let mut buf = vec![0u32; 1600 * 900];
        t.draw(&mut buf, 1600, 900);
        let acts: Vec<Action> = t.sprite.as_ref().unwrap().buttons.iter().map(|b| b.0).collect();
        assert_eq!(acts, [Action::Access, Action::Fullscreen, Action::Pin, Action::Disconnect]);
        // Chat, when the host has it, sits before quality.
        t.set_extras(Extras { chat: Some(2), quality: Some("Auto".into()), ..Extras::default() });
        t.draw(&mut buf, 1600, 900);
        let acts: Vec<Action> = t.sprite.as_ref().unwrap().buttons.iter().map(|b| b.0).collect();
        assert_eq!(
            acts,
            [Action::Fullscreen, Action::CtrlAltDel, Action::Chat, Action::Quality, Action::Pin, Action::Disconnect]
        );
        t.set_extras(Extras { access: Some("View only".into()), no_control: true, ..Extras::default() });
        t.draw(&mut buf, 1600, 900);
        // The level is a label, not a button.
        let (a, b, _) = t.button_rect(Action::Access).unwrap();
        t.pointer(((a + b) / 2) as f64, 10.0);
        assert_eq!(t.hover, None);
        assert!(t.press());
        assert_eq!(t.release(), (true, None));
    }

    #[test]
    fn pinned_disconnect_takes_two_clicks() {
        let mut t = Toolbar::new();
        t.set_state(1.0, false, (1, 0));
        t.set_extras(Extras { pinned: true, ..Extras::default() });
        let mut buf = vec![0u32; 1600 * 900];
        t.draw(&mut buf, 1600, 900);
        t.visible_until = None;
        let at = |t: &Toolbar| {
            let (a, b, _) = t.button_rect(Action::Disconnect).unwrap();
            ((a + b) / 2) as f64
        };
        // A remote tab's close button under the pinned bar: one click must not end the session.
        let x = at(&t);
        t.pointer(x, 10.0);
        assert!(t.press());
        assert_eq!(t.release(), (true, None), "first click only asks");
        assert!(t.confirming());
        // The label grew: find the button again, then confirm.
        t.draw(&mut buf, 1600, 900);
        let x = at(&t);
        t.pointer(x, 10.0);
        assert!(t.press());
        assert_eq!(t.release(), (true, Some(Action::Disconnect)));
        assert!(!t.confirming());
        // Unconfirmed, it times out and goes back to plain Disconnect.
        t.pointer(x, 10.0);
        t.press();
        t.release();
        t.confirm_until = Some(Instant::now() - Duration::from_millis(1));
        assert!(t.expire_confirm());
        assert!(!t.expire_confirm());
        t.draw(&mut buf, 1600, 900);
        let x = at(&t);
        t.pointer(x, 10.0);
        assert!(t.press());
        assert_eq!(t.release(), (true, None), "a fresh first click again");
    }
}

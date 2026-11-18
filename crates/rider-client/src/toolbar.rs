//! Session toolbar: a small bar at the top centre of the viewer with Full
//! screen, Ctrl+Alt+Del, one button per remote monitor and Disconnect. It shows for a few
//! seconds when the session starts and whenever the pointer touches the top
//! edge (a thin handle marks the spot); otherwise it stays out of the way.
//!
//! The bar is rendered once per state (labels via GDI with Segoe UI and the
//! Segoe MDL2 icon font, grayscale anti-aliasing) into a premultiplied ARGB
//! sprite, then alpha-blended over each frame: a few thousand pixels per draw.

use std::time::{Duration, Instant};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS, CreateCompatibleDC,
    CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DIB_RGB_COLORS, DeleteDC, DeleteObject, FF_DONTCARE, FW_NORMAL,
    GdiFlush, GetTextExtentPoint32W, HDC, HFONT, OUT_TT_PRECIS, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
    TextOutW,
};
use windows::core::PCWSTR;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Fullscreen,
    CtrlAltDel,
    /// Show this remote monitor (0-based).
    Monitor(u32),
    Disconnect,
}

/// Shown at the start of a session so people learn where it is.
const INTRO: Duration = Duration::from_secs(4);
/// Stays this long after the pointer leaves it.
const LINGER: Duration = Duration::from_millis(800);
/// Pointer this close to the top edge (logical px) reveals the bar.
const EDGE: f64 = 6.0;

const BG: (u8, u8, u8, u8) = (30, 31, 36, 238);
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
    hover: Option<Action>,
    pressed: Option<Action>,
    /// Visible until this instant (None: pointer is on it, stay).
    visible_until: Option<Instant>,
    on_bar: bool,
    sprite: Option<Sprite>,
    /// Where the sprite was last drawn (x offset; it is always at y = 0).
    x: usize,
}

impl Toolbar {
    pub fn new() -> Self {
        Self {
            scale: 1.0,
            fullscreen: false,
            monitors: (0, 0),
            hover: None,
            pressed: None,
            visible_until: Some(Instant::now() + INTRO),
            on_bar: false,
            sprite: None,
            x: 0,
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

    pub fn visible(&self) -> bool {
        self.on_bar || self.visible_until.is_some_and(|t| Instant::now() < t)
    }

    /// When the bar should be hidden next (to schedule a redraw then).
    pub fn deadline(&self) -> Option<Instant> {
        if self.on_bar { None } else { self.visible_until.filter(|t| Instant::now() < *t) }
    }

    /// Pointer moved to (x, y) physical px. Returns true if the bar changed.
    pub fn pointer(&mut self, x: f64, y: f64) -> bool {
        let before = (self.visible(), self.hover);
        let on = if self.visible() { self.over_bar(x, y) } else { y <= EDGE * self.scale && self.over_span(x) };
        if on {
            self.on_bar = true;
            self.visible_until = None;
        } else if self.on_bar {
            self.on_bar = false;
            self.visible_until = Some(Instant::now() + LINGER);
        }
        self.set_hover(if self.visible() { self.hit(x, y) } else { None });
        (self.visible(), self.hover) != before
    }

    fn set_hover(&mut self, hover: Option<Action>) {
        if hover != self.hover {
            self.hover = hover;
            self.sprite = None;
        }
    }

    /// Pointer left the window.
    pub fn leave(&mut self) -> bool {
        let was = self.on_bar || self.hover.is_some();
        if self.on_bar {
            self.visible_until = Some(Instant::now() + LINGER);
        }
        self.on_bar = false;
        self.set_hover(None);
        was
    }

    /// A left-button press at the pointer: true if the bar takes it.
    pub fn press(&mut self) -> bool {
        if !self.visible() || !self.on_bar {
            return false;
        }
        self.pressed = self.hover;
        true
    }

    /// The matching release: the action to run, if released on the same button.
    /// `None` with `taken == true` means the bar swallowed the click.
    pub fn release(&mut self) -> (bool, Option<Action>) {
        match self.pressed.take() {
            Some(a) => (true, (self.hover == Some(a)).then_some(a)),
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

    /// Within the bar's horizontal span (where the handle sits).
    fn over_span(&self, x: f64) -> bool {
        let (w, _) = self.sprite_size();
        x >= self.x as f64 && x < (self.x + w) as f64
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
            self.sprite = render(self.scale, self.fullscreen, self.monitors, self.hover);
        }
        let Some(s) = &self.sprite else { return };
        self.x = w.saturating_sub(s.w) / 2;
        if !self.visible() {
            handle(buf, w, h, self.scale);
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

/// A short rounded line at the top centre: "there is something here".
fn handle(buf: &mut [u32], w: usize, h: usize, scale: f64) {
    let hw = (56.0 * scale) as usize;
    let hh = ((4.0 * scale) as usize).max(3);
    let x0 = w.saturating_sub(hw) / 2;
    let px = premul(200, 202, 210, 150);
    for y in 0..hh.min(h) {
        for x in x0..(x0 + hw).min(w) {
            buf[y * w + x] = blend(px, buf[y * w + x]);
        }
    }
}

/// Premultiplied ARGB.
fn premul(r: u8, g: u8, b: u8, a: u8) -> u32 {
    let m = |c: u8| (c as u32 * a as u32 + 127) / 255;
    (a as u32) << 24 | m(r) << 16 | m(g) << 8 | m(b)
}

fn blend(src: u32, dst: u32) -> u32 {
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

fn labels(fullscreen: bool, (count, _): (u32, u32)) -> Vec<(Action, char, String)> {
    let mut v = vec![if fullscreen {
        (Action::Fullscreen, '\u{E73F}', "Exit full screen".into())
    } else {
        (Action::Fullscreen, '\u{E740}', "Full screen".into())
    }];
    v.push((Action::CtrlAltDel, '\u{E765}', "Ctrl+Alt+Del".into()));
    if count > 1 {
        v.extend((0..count).map(|i| (Action::Monitor(i), '\u{E7F4}', (i + 1).to_string())));
    }
    v.push((Action::Disconnect, '\u{E711}', "Disconnect".into()));
    v
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

struct Gdi {
    dc: HDC,
    text: HFONT,
    icons: HFONT,
}

impl Gdi {
    fn new(scale: f64) -> Option<Self> {
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
        Some(Self { dc, text: font(13.0, "Segoe UI"), icons: font(14.0, "Segoe MDL2 Assets") })
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
}

impl Drop for Gdi {
    fn drop(&mut self) {
        // SAFETY: objects created in `new`, no longer selected after DeleteDC.
        unsafe {
            let _ = DeleteDC(self.dc);
            let _ = DeleteObject(self.text.into());
            let _ = DeleteObject(self.icons.into());
        }
    }
}

fn render(scale: f64, fullscreen: bool, monitors: (u32, u32), hover: Option<Action>) -> Option<Sprite> {
    let gdi = Gdi::new(scale)?;
    let s = |v: f64| (v * scale).round() as i32;
    let (pad, gap, h) = (s(14.0), s(8.0), s(36.0));
    let items: Vec<_> = labels(fullscreen, monitors)
        .into_iter()
        .map(|(act, icon, text)| {
            let (icon, text) = (wide(&icon.to_string()), wide(&text));
            let iw = gdi.extent(gdi.icons, &icon).0;
            let tw = gdi.extent(gdi.text, &text).0;
            (act, icon, text, iw, tw)
        })
        .collect();
    let edge = s(6.0);
    let mut buttons = Vec::new();
    let mut x = edge;
    for (act, _, _, iw, tw) in &items {
        let bw = pad + iw + gap + tw + pad;
        buttons.push((*act, x as usize, (x + bw) as usize));
        x += bw;
    }
    let w = (x + edge) as usize;
    let hu = h as usize;

    // Labels: white on black into a top-down 32-bit DIB; coverage = brightness.
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: `bits` receives the DIB's memory, valid until the bitmap is deleted.
    let bmp = unsafe { CreateDIBSection(Some(gdi.dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
    let mut coverage = vec![0u8; w * hu];
    // SAFETY: drawing into our own DIB; it is read back after GdiFlush and
    // deleted before `bits` goes out of use.
    unsafe {
        let old = SelectObject(gdi.dc, bmp.into());
        SetBkMode(gdi.dc, TRANSPARENT);
        SetTextColor(gdi.dc, windows::Win32::Foundation::COLORREF(0x00FF_FFFF));
        for ((_, icon, text, iw, _), (_, x0, _)) in items.iter().zip(&buttons) {
            let x = *x0 as i32 + pad;
            let (_, ih) = gdi.extent(gdi.icons, icon);
            let _ = TextOutW(gdi.dc, x, (h - ih) / 2 + s(1.0), icon);
            let (_, th) = gdi.extent(gdi.text, text);
            let _ = TextOutW(gdi.dc, x + iw + gap, (h - th) / 2, text);
        }
        let _ = GdiFlush();
        let src = std::slice::from_raw_parts(bits as *const u32, w * hu);
        for (c, p) in coverage.iter_mut().zip(src) {
            let (r, g, b) = ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF);
            *c = r.max(g).max(b) as u8;
        }
        SelectObject(gdi.dc, old);
        let _ = DeleteObject(bmp.into());
    }

    // Background: rounded at the bottom, flat against the top edge.
    let r = 10.0 * scale;
    // The monitor on screen is marked; the hovered button is lighter (or red).
    let pill = |act: Action| buttons.iter().find(|b| b.0 == act).map(|&(_, a, b)| (a, b));
    let active = (monitors.0 > 1).then(|| pill(Action::Monitor(monitors.1))).flatten().map(|(a, b)| (a, b, ACCENT));
    let hovered = hover
        .and_then(|h| pill(h).map(|(a, b)| (a, b, if h == Action::Disconnect { DANGER } else { HOVER })))
        .filter(|_| hover != Some(Action::Monitor(monitors.1)) || monitors.0 < 2);
    let mut px = vec![0u32; w * hu];
    for y in 0..hu {
        for x in 0..w {
            let in_pill = |p: Option<(usize, usize, (u8, u8, u8))>| {
                p.filter(|&(a, b, _)| x >= a && x < b && inside_round(x - a, y, b - a, hu, scale)).map(|p| p.2)
            };
            let bg = in_pill(hovered).or_else(|| in_pill(active)).unwrap_or((BG.0, BG.1, BG.2));
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
        assert_eq!(s.buttons.len(), 5);
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
    fn edge_reveals_after_resize() {
        let mut t = Toolbar::new();
        t.set_state(1.25, false, (1, 0));
        let mut small = vec![0u32; 1000 * 600];
        t.draw(&mut small, 1000, 600);
        // Intro over, pointer elsewhere: hidden.
        t.visible_until = None;
        t.pointer(500.0, 300.0);
        assert!(!t.visible());
        // Full screen: larger window, new label; the bar re-centres.
        t.set_state(1.25, true, (1, 0));
        let mut big = vec![0u32; 1920 * 1080];
        t.draw(&mut big, 1920, 1080);
        assert!(t.pointer(960.0, 1.0), "touching the top edge shows it");
        assert!(t.visible());
        t.pointer(960.0, 15.0);
        assert!(t.visible(), "stays while on it");
        t.pointer(960.0, 500.0);
        assert!(t.visible(), "lingers briefly");
        assert!(t.deadline().is_some());
    }
}

//! "Connection lost, reconnecting" card over a dimmed picture: shown while
//! the host is silent or the viewer is reconnecting, so a frozen picture is
//! never mistaken for a working one.

use crate::toolbar::{BG, Label, blend, text};

struct Card {
    w: usize,
    h: usize,
    /// Premultiplied ARGB.
    px: Vec<u32>,
}

#[derive(Default)]
pub struct Overlay {
    /// Title and detail line; `None` hides the overlay.
    lines: Option<(String, String)>,
    scale: f64,
    card: Option<Card>,
}

impl Overlay {
    /// What to show (`None` hides it). True if the window must be redrawn.
    pub fn set(&mut self, lines: Option<(String, String)>) -> bool {
        if self.lines == lines {
            return false;
        }
        self.lines = lines;
        self.card = None;
        true
    }

    /// Dim the frame of `w` x `h` pixels (XRGB) and draw the card centred on it.
    pub fn draw(&mut self, buf: &mut [u32], w: usize, h: usize, scale: f64) {
        let Some((title, detail)) = &self.lines else { return };
        if self.scale != scale {
            self.scale = scale;
            self.card = None;
        }
        for p in buf.iter_mut() {
            *p = (*p >> 1) & 0x007F_7F7F;
        }
        if self.card.is_none() {
            self.card = render(title, detail, scale);
        }
        let Some(c) = &self.card else { return };
        let (x0, y0) = (w.saturating_sub(c.w) / 2, h.saturating_sub(c.h) / 2);
        for y in 0..c.h.min(h - y0) {
            let row = &mut buf[(y0 + y) * w + x0..][..c.w.min(w - x0)];
            for (d, &p) in row.iter_mut().zip(&c.px[y * c.w..]) {
                *d = blend(p, *d);
            }
        }
    }
}

fn render(title: &str, detail: &str, scale: f64) -> Option<Card> {
    let big = text::Fonts::new(scale * 1.4)?;
    let small = text::Fonts::new(scale)?;
    let s = |v: f64| (v * scale).round() as usize;
    let (pad_x, pad_y, gap, title_h, detail_h) = (s(32.0), s(18.0), s(2.0), s(28.0), s(20.0));
    let tw = big.text_width(title).max(0) as usize;
    let dw = small.text_width(detail).max(0) as usize;
    let w = tw.max(dw) + 2 * pad_x;
    let h = pad_y + title_h + gap + detail_h + pad_y;
    let line = |f: &text::Fonts, t: &str, tw: usize, lh: usize| {
        f.paint(w, lh, &[Label { x: ((w - tw) / 2) as i32, icon: None, icon_w: 0, text: t }], 0)
    };
    let top = line(&big, title, tw, title_h)?;
    let bottom = line(&small, detail, dw, detail_h)?;
    let detail_y = pad_y + title_h + gap;
    let r = 12.0 * scale;
    let mut px = vec![0u32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let (cx, cy) = (fx.clamp(r, w as f64 - r), fy.clamp(r, h as f64 - r));
            let shape = (r - ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0);
            if shape == 0.0 {
                continue;
            }
            // Title in white, the detail line a little dimmer.
            let (t, ink) = if (pad_y..pad_y + title_h).contains(&y) {
                (top[(y - pad_y) * w + x], 255)
            } else if (detail_y..detail_y + detail_h).contains(&y) {
                (bottom[(y - detail_y) * w + x], 200)
            } else {
                (0, 0)
            };
            let (a, t) = ((BG.3 as f64 * shape) as u32, t as u32);
            let mix = |c: u8| ((c as u32 * (255 - t) + ink * t) / 255 * a + 127) / 255;
            px[y * w + x] = a << 24 | mix(BG.0) << 16 | mix(BG.1) << 8 | mix(BG.2);
        }
    }
    Some(Card { w, h, px })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dims_and_draws_the_card() {
        let (w, h) = (400, 300);
        let mut buf = vec![0x00FF_FFFFu32; w * h];
        let mut o = Overlay::default();
        assert!(!o.set(None), "already hidden");
        assert!(o.set(Some(("Connection lost".into(), "Reconnecting… 0:05".into()))));
        o.draw(&mut buf, w, h, 1.0);
        assert_eq!(buf[0], 0x007F_7F7F, "corner dimmed by half");
        // Card top padding (no text there): dark, nearly opaque.
        let card_top = (h - 86) / 2;
        assert!(buf[(card_top + 9) * w + w / 2] & 0xFF < 0x40, "dark card in the middle");
        assert!(o.set(None));
    }
}

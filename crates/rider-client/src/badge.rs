use crate::toolbar::{BG, Label, blend, text};

const MARGIN: f64 = 10.0;
const ALPHA: u32 = 200;

struct Sprite {
    w: usize,
    h: usize,
    px: Vec<u32>,
}

#[derive(Default)]
pub struct Badge {
    text: String,
    scale: f64,
    sprite: Option<Sprite>,
    at: (usize, usize, usize, usize),
}

impl Badge {
    pub fn set(&mut self, text: String) -> bool {
        if self.text == text {
            return false;
        }
        self.text = text;
        self.sprite = None;
        true
    }

    pub fn under(&self, x: f64, y: f64) -> bool {
        let (bx, by, bw, bh) = self.at;
        !self.text.is_empty() && x >= bx as f64 && x < (bx + bw) as f64 && y >= by as f64 && y < (by + bh) as f64
    }

    pub fn draw(&mut self, buf: &mut [u32], w: usize, h: usize, scale: f64, pointer: (f64, f64)) {
        if self.text.is_empty() {
            return;
        }
        if self.scale != scale {
            self.scale = scale;
            self.sprite = None;
        }
        if self.sprite.is_none() {
            self.sprite = render(&self.text, scale);
        }
        let Some(s) = &self.sprite else { return };
        let m = (MARGIN * scale).round() as usize;
        if s.w + m > w || s.h + m > h {
            return;
        }
        self.at = (m, m, s.w, s.h);
        if self.under(pointer.0, pointer.1) {
            return;
        }
        for y in 0..s.h {
            let row = &mut buf[(m + y) * w + m..][..s.w];
            for (d, &p) in row.iter_mut().zip(&s.px[y * s.w..]) {
                *d = blend(p, *d);
            }
        }
    }
}

fn render(label: &str, scale: f64) -> Option<Sprite> {
    let fonts = text::Fonts::new(scale * 0.9)?;
    let s = |v: f64| (v * scale).round() as usize;
    let (pad, gap, h) = (s(10.0), s(6.0), s(26.0));
    let icon = cfg!(windows).then_some('\u{E7F4}');
    let iw = icon.map_or(0, |c| fonts.icon_width(c)).max(0) as usize;
    let tw = fonts.text_width(label).max(0) as usize;
    let w = pad + iw + if iw > 0 { gap } else { 0 } + tw + pad;
    let coverage = fonts.paint(w, h, &[Label { x: pad as i32, icon, icon_w: iw as i32, text: label }], gap as i32)?;
    let r = h as f64 / 2.0;
    let mut px = vec![0u32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let (cx, cy) = (fx.clamp(r, w as f64 - r), r);
            let shape = (r - ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0);
            if shape == 0.0 {
                continue;
            }
            let a = (ALPHA as f64 * shape) as u32;
            let t = coverage[y * w + x] as u32;
            let mix = |c: u8| ((c as u32 * (255 - t) + 255 * t) / 255 * a + 127) / 255;
            px[y * w + x] = a << 24 | mix(BG.0) << 16 | mix(BG.1) << 8 | mix(BG.2);
        }
    }
    Some(Sprite { w, h, px })
}

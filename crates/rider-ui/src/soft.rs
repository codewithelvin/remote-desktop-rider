use egui::epaint::{ClippedPrimitive, ImageDelta, Mesh, Primitive, TextureId, Vertex};
use egui::{Color32, Rect, TexturesDelta};
use std::collections::HashMap;

struct Texture {
    w: usize,
    h: usize,
    px: Vec<Color32>,
}

#[derive(Default)]
pub struct SoftRenderer {
    textures: HashMap<TextureId, Texture>,
}

pub struct Target<'a> {
    pub buf: &'a mut [u32],
    pub w: usize,
    pub h: usize,
    pub origin: (f32, f32),
}

impl SoftRenderer {
    pub fn update_textures(&mut self, delta: &TexturesDelta) {
        for (id, deltas) in &delta.set {
            for d in deltas {
                self.set(*id, d);
            }
        }
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    fn set(&mut self, id: TextureId, d: &ImageDelta) {
        let egui::ImageData::Color(img) = &d.image;
        let [w, h] = img.size;
        match d.pos {
            None => {
                self.textures.insert(id, Texture { w, h, px: img.pixels.clone() });
            }
            Some([x0, y0]) => {
                let Some(t) = self.textures.get_mut(&id) else { return };
                for y in 0..h.min(t.h.saturating_sub(y0)) {
                    let n = w.min(t.w.saturating_sub(x0));
                    let dst = (y0 + y) * t.w + x0;
                    t.px[dst..dst + n].copy_from_slice(&img.pixels[y * w..y * w + n]);
                }
            }
        }
    }

    pub fn paint(&self, prims: &[ClippedPrimitive], pixels_per_point: f32, target: &mut Target<'_>) {
        for p in prims {
            let Primitive::Mesh(mesh) = &p.primitive else { continue };
            let Some(tex) = self.textures.get(&mesh.texture_id) else { continue };
            let clip = clip_px(p.clip_rect, pixels_per_point, target);
            if clip.is_empty() {
                continue;
            }
            draw_mesh(mesh, tex, pixels_per_point, clip, target);
        }
    }
}

#[derive(Clone, Copy)]
struct Clip {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl Clip {
    fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }
}

fn clip_px(r: Rect, ppp: f32, t: &Target<'_>) -> Clip {
    let f = |v: f32, o: f32, max: usize| ((v * ppp - o).round() as i32).clamp(0, max as i32);
    Clip {
        x0: f(r.min.x, t.origin.0, t.w),
        y0: f(r.min.y, t.origin.1, t.h),
        x1: f(r.max.x, t.origin.0, t.w),
        y1: f(r.max.y, t.origin.1, t.h),
    }
}

fn draw_mesh(mesh: &Mesh, tex: &Texture, ppp: f32, clip: Clip, t: &mut Target<'_>) {
    let (ox, oy) = t.origin;
    let to_px = |v: &Vertex| (v.pos.x * ppp - ox, v.pos.y * ppp - oy);
    for tri in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = tri.map(|i| &mesh.vertices[i as usize]);
        triangle([a, b, c], [to_px(a), to_px(b), to_px(c)], tex, clip, t);
    }
}

#[inline]
fn edge(a: (f32, f32), b: (f32, f32), p: (f32, f32)) -> f32 {
    (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
}

#[allow(clippy::many_single_char_names)]
fn triangle(v: [&Vertex; 3], p: [(f32, f32); 3], tex: &Texture, clip: Clip, t: &mut Target<'_>) {
    let mut area = edge(p[0], p[1], p[2]);
    if area.abs() < 1e-6 {
        return;
    }
    let (mut v, mut p) = (v, p);
    if area < 0.0 {
        v.swap(1, 2);
        p.swap(1, 2);
        area = -area;
    }
    let min_x = (p.iter().map(|q| q.0).fold(f32::MAX, f32::min).floor() as i32).max(clip.x0);
    let max_x = (p.iter().map(|q| q.0).fold(f32::MIN, f32::max).ceil() as i32).min(clip.x1);
    let min_y = (p.iter().map(|q| q.1).fold(f32::MAX, f32::min).floor() as i32).max(clip.y0);
    let max_y = (p.iter().map(|q| q.1).fold(f32::MIN, f32::max).ceil() as i32).min(clip.y1);
    if min_x >= max_x || min_y >= max_y {
        return;
    }
    let bias = |a: (f32, f32), b: (f32, f32)| {
        #[expect(clippy::float_cmp, reason = "the top-left fill rule needs exact edge equality")]
        let top = a.1 == b.1 && b.0 < a.0;
        let left = b.1 > a.1;
        if top || left { 0.0 } else { -f32::EPSILON }
    };
    let (b0, b1, b2) = (bias(p[1], p[2]), bias(p[2], p[0]), bias(p[0], p[1]));
    let inv = 1.0 / area;

    let col = |c: Color32| [f32::from(c.r()), f32::from(c.g()), f32::from(c.b()), f32::from(c.a())];
    let (c0, c1, c2) = (col(v[0].color), col(v[1].color), col(v[2].color));
    let flat = v[0].color == v[1].color && v[1].color == v[2].color;
    let (tw, th) = (tex.w as f32, tex.h as f32);
    let uv = |q: &Vertex| (q.uv.x * tw, q.uv.y * th);
    let (t0, t1, t2) = (uv(v[0]), uv(v[1]), uv(v[2]));

    for y in min_y..max_y {
        let py = y as f32 + 0.5;
        let row = y as usize * t.w;
        for x in min_x..max_x {
            let q = (x as f32 + 0.5, py);
            let w0 = edge(p[1], p[2], q);
            let w1 = edge(p[2], p[0], q);
            let w2 = edge(p[0], p[1], q);
            if w0 + b0 < 0.0 || w1 + b1 < 0.0 || w2 + b2 < 0.0 {
                continue;
            }
            let (l0, l1, l2) = (w0 * inv, w1 * inv, w2 * inv);
            let c = if flat { c0 } else { std::array::from_fn(|i| c0[i] * l0 + c1[i] * l1 + c2[i] * l2) };
            let tu = (t0.0 * l0 + t1.0 * l1 + t2.0 * l2) as usize;
            let tv = (t0.1 * l0 + t1.1 * l1 + t2.1 * l2) as usize;
            let s = tex.px[tv.min(tex.h - 1) * tex.w + tu.min(tex.w - 1)];
            let m = |a: f32, b: u8| a * f32::from(b) / 255.0;
            let src = [m(c[0], s.r()), m(c[1], s.g()), m(c[2], s.b()), m(c[3], s.a())];
            if src[3] <= 0.0 && src[0] <= 0.0 && src[1] <= 0.0 && src[2] <= 0.0 {
                continue;
            }
            let i = row + x as usize;
            t.buf[i] = blend(t.buf[i], src);
        }
    }
}

#[inline]
fn blend(dst: u32, src: [f32; 4]) -> u32 {
    let k = 1.0 - src[3] / 255.0;
    let ch = |shift: u32, s: f32| {
        let d = ((dst >> shift) & 0xff) as f32;
        ((s + d * k).round().clamp(0.0, 255.0) as u32) << shift
    };
    ch(16, src[0]) | ch(8, src[1]) | ch(0, src[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, pos2};

    fn white_texture() -> SoftRenderer {
        let mut r = SoftRenderer::default();
        r.textures.insert(TextureId::default(), Texture { w: 1, h: 1, px: vec![Color32::WHITE] });
        r
    }

    fn quad(rect: Rect, color: Color32) -> ClippedPrimitive {
        let mut mesh = Mesh::default();
        mesh.add_colored_rect(rect, color);
        ClippedPrimitive { clip_rect: Rect::EVERYTHING, primitive: Primitive::Mesh(mesh) }
    }

    fn render(prims: &[ClippedPrimitive], ppp: f32, w: usize, h: usize, origin: (f32, f32)) -> Vec<u32> {
        let mut buf = vec![0u32; w * h];
        white_texture().paint(prims, ppp, &mut Target { buf: &mut buf, w, h, origin });
        buf
    }

    #[test]
    fn a_rectangle_covers_exactly_its_pixels_once() {
        let r = Rect::from_min_max(pos2(2.0, 1.0), pos2(6.0, 4.0));
        let buf = render(&[quad(r, Color32::from_rgba_premultiplied(100, 0, 0, 128))], 1.0, 8, 6, (0.0, 0.0));
        for y in 0..6 {
            for x in 0..8 {
                let inside = (2..6).contains(&x) && (1..4).contains(&y);
                assert_eq!(buf[y * 8 + x], if inside { 100 << 16 } else { 0 }, "({x},{y})");
            }
        }
    }

    #[test]
    fn scale_origin_and_clip() {
        let r = Rect::from_min_max(pos2(10.0, 0.0), pos2(12.0, 1.0));
        let buf = render(&[quad(r, Color32::WHITE)], 2.0, 6, 3, (20.0, 0.0));
        let lit: Vec<usize> = (0..18).filter(|&i| buf[i] != 0).collect();
        assert_eq!(lit, [0, 1, 2, 3, 6, 7, 8, 9]);
        let mut p = quad(r, Color32::WHITE);
        p.clip_rect = Rect::from_min_max(pos2(10.0, 0.0), pos2(11.0, 1.0));
        let buf = render(&[p], 2.0, 6, 3, (20.0, 0.0));
        assert_eq!((0..18).filter(|&i| buf[i] != 0).count(), 4);
    }

    #[test]
    fn premultiplied_blending() {
        assert_eq!(blend(0x00ff_ffff, [0.0, 0.0, 0.0, 255.0]), 0);
        assert_eq!(blend(0x0000_00ff, [128.0, 0.0, 0.0, 128.0]), (128 << 16) | 127);
        let _ = Pos2::ZERO;
    }

    #[test]
    fn partial_texture_updates() {
        let mut r = SoftRenderer::default();
        let id = TextureId::Managed(0);
        let full = egui::ColorImage::new([4, 2], vec![Color32::BLACK; 8]);
        r.set(id, &ImageDelta::full(full, egui::TextureOptions::default()));
        let patch = egui::ColorImage::new([2, 1], vec![Color32::WHITE; 2]);
        r.set(id, &ImageDelta::partial([1, 1], patch, egui::TextureOptions::default()));
        let t = &r.textures[&id];
        let white: Vec<usize> = (0..8).filter(|&i| t.px[i] == Color32::WHITE).collect();
        assert_eq!(white, [5, 6]);
    }
}

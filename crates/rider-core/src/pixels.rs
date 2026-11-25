use rayon::prelude::*;

pub const BACKGROUND: u32 = 0x0010_1014;
const MIN_ROWS_PER_TASK: usize = 16;

const YR: u32 = 47;
const YG: u32 = 157;
const YB: u32 = 16;
const UR: i32 = -26;
const UG: i32 = -87;
const UB: i32 = 112;
const VR: i32 = 112;
const VG: i32 = -102;
const VB: i32 = -10;

macro_rules! multiversion {
    ($vis:vis fn $name:ident($($arg:ident: $ty:ty),*) => $generic:ident) => {
        $vis fn $name($($arg: $ty),*) {
            #[cfg(target_arch = "x86_64")]
            {
                #[target_feature(enable = "avx2")]
                fn avx2($($arg: $ty),*) {
                    $generic($($arg),*)
                }
                if std::is_x86_feature_detected!("avx2") {
                    return unsafe { avx2($($arg),*) };
                }
            }
            $generic($($arg),*)
        }
    };
}

#[inline(always)]
fn luma_row(src: &[u8], dst: &mut [u8]) {
    for (o, px) in dst.iter_mut().zip(src.as_chunks::<4>().0) {
        let p = u32::from_le_bytes(*px);
        let (b, g, r) = (p & 0xFF, (p >> 8) & 0xFF, (p >> 16) & 0xFF);
        *o = (((YR * r + YG * g + YB * b + 128) >> 8) + 16) as u8;
    }
}

#[inline(always)]
fn chroma_row(top: &[u8], bottom: &[u8], dst: &mut [u8]) {
    for ((p0, p1), c) in top.as_chunks::<8>().0.iter().zip(bottom.as_chunks::<8>().0).zip(dst.as_chunks_mut::<2>().0) {
        let (a, b) = (u64::from_le_bytes(*p0), u64::from_le_bytes(*p1));
        let ch = |shift: u32| -> i32 {
            (((a >> shift) & 0xFF)
                + ((a >> (shift + 32)) & 0xFF)
                + ((b >> shift) & 0xFF)
                + ((b >> (shift + 32)) & 0xFF)) as i32
        };
        let (bs, gs, rs) = (ch(0), ch(8), ch(16));
        c[0] = (((UR * rs + UG * gs + UB * bs + 512) >> 10) + 128).clamp(0, 255) as u8;
        c[1] = (((VR * rs + VG * gs + VB * bs + 512) >> 10) + 128).clamp(0, 255) as u8;
    }
}

#[inline(always)]
fn row_pair_generic(top: &[u8], bottom: &[u8], y_top: &mut [u8], y_bottom: &mut [u8], uv: &mut [u8]) {
    luma_row(top, y_top);
    luma_row(bottom, y_bottom);
    chroma_row(top, bottom, uv);
}

multiversion!(fn row_pair(top: &[u8], bottom: &[u8], y_top: &mut [u8], y_bottom: &mut [u8], uv: &mut [u8]) => row_pair_generic);

pub fn bgra_to_nv12(src: &[u8], stride: usize, width: usize, height: usize, dst: &mut Vec<u8>) {
    assert!(width.is_multiple_of(2) && height.is_multiple_of(2), "NV12 needs even dimensions");
    assert!(stride >= width * 4 && src.len() >= stride * (height - 1) + width * 4, "source too small");
    let y_size = width * height;
    dst.resize(y_size + y_size / 2, 0);
    let (y_plane, uv_plane) = dst.split_at_mut(y_size);

    y_plane
        .par_chunks_exact_mut(width * 2)
        .zip(uv_plane.par_chunks_exact_mut(width))
        .enumerate()
        .with_min_len(MIN_ROWS_PER_TASK / 2)
        .for_each(|(pair, (y_rows, uv))| {
            let top = &src[2 * pair * stride..][..width * 4];
            let bottom = &src[(2 * pair + 1) * stride..][..width * 4];
            let (y_top, y_bottom) = y_rows.split_at_mut(width);
            row_pair(top, bottom, y_top, y_bottom, uv);
        });
}

pub struct Nv12Frame {
    pub data: Vec<u8>,
    pub stride: usize,
    pub alloc_height: usize,
    pub width: usize,
    pub height: usize,
}

#[inline(always)]
fn yuv_to_xrgb(y: u8, u: u8, v: u8) -> u32 {
    let c = (y as i32 - 16).max(0) * 298;
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    let r = ((c + 459 * e + 128) >> 8).clamp(0, 255) as u32;
    let g = ((c - 55 * d - 136 * e + 128) >> 8).clamp(0, 255) as u32;
    let b = ((c + 541 * d + 128) >> 8).clamp(0, 255) as u32;
    (r << 16) | (g << 8) | b
}

#[inline(always)]
fn xrgb_row_1to1_generic(y: &[u8], uv: &[u8], out: &mut [u32]) {
    for ((ys, c), o) in y.as_chunks::<2>().0.iter().zip(uv.as_chunks::<2>().0).zip(out.as_chunks_mut::<2>().0) {
        o[0] = yuv_to_xrgb(ys[0], c[0], c[1]);
        o[1] = yuv_to_xrgb(ys[1], c[0], c[1]);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Tap {
    a: u32,
    b: u32,
    f: u32,
}

fn taps(src: usize, dst: usize) -> Vec<Tap> {
    let last = src.saturating_sub(1) as i64;
    (0..dst as i64)
        .map(|x| {
            let p = ((2 * x + 1) * src as i64 * 256 / (2 * dst as i64) - 128).clamp(0, last * 256);
            let a = p >> 8;
            Tap { a: a as u32, b: (a + 1).min(last) as u32, f: (p & 255) as u32 }
        })
        .collect()
}

#[inline(always)]
fn mix(p: u32, q: u32, f: u32) -> u32 {
    p * (256 - f) + q * f
}

#[inline(always)]
fn blend_rows_generic(a: &[u8], b: &[u8], f: u32, out: &mut [u16]) {
    let (wa, wb) = ((256 - f) as u16, f as u16);
    for ((o, &p), &q) in out.iter_mut().zip(a).zip(b) {
        *o = p as u16 * wa + q as u16 * wb;
    }
}

#[inline(always)]
fn xrgb_row_bilinear_generic(y: &[u16], uv: &[u16], xt: &[Tap], ct: &[Tap], out: &mut [u32]) {
    assert!(xt.last().is_none_or(|t| (t.b as usize) < y.len()), "luma tap out of range");
    assert!(ct.last().is_none_or(|t| 2 * t.b as usize + 1 < uv.len()), "chroma tap out of range");
    let px = |p: u16, q: u16, f: u32| ((mix(p as u32, q as u32, f) + (1 << 15)) >> 16) as u8;
    for ((o, t), c) in out.iter_mut().zip(xt).zip(ct) {
        unsafe {
            let l = px(*y.get_unchecked(t.a as usize), *y.get_unchecked(t.b as usize), t.f);
            let (a, b) = (2 * c.a as usize, 2 * c.b as usize);
            let u = px(*uv.get_unchecked(a), *uv.get_unchecked(b), c.f);
            let v = px(*uv.get_unchecked(a + 1), *uv.get_unchecked(b + 1), c.f);
            *o = yuv_to_xrgb(l, u, v);
        }
    }
}

multiversion!(fn blend_rows(a: &[u8], b: &[u8], f: u32, out: &mut [u16]) => blend_rows_generic);
multiversion!(fn xrgb_row_bilinear(y: &[u16], uv: &[u16], xt: &[Tap], ct: &[Tap], out: &mut [u32]) => xrgb_row_bilinear_generic);
multiversion!(fn xrgb_row_1to1(y: &[u8], uv: &[u8], out: &mut [u32]) => xrgb_row_1to1_generic);

pub fn nv12_to_xrgb_fit(f: &Nv12Frame, dst: &mut [u32], dw: usize, dh: usize) -> (usize, usize, usize, usize) {
    assert!(dst.len() >= dw * dh, "destination too small");
    assert!(f.data.len() >= f.stride * f.alloc_height * 3 / 2 && f.width <= f.stride, "NV12 buffer too small");
    let rect @ (ox, oy, w, h) = fit_rect(f.width, f.height, dw, dh);
    if w == 0 || h == 0 {
        dst.fill(BACKGROUND);
        return rect;
    }
    let uv_base = f.stride * f.alloc_height;
    let unscaled = w == f.width && h == f.height && f.width.is_multiple_of(2);
    let (cw, ch) = ((f.width / 2).max(1), f.height.div_ceil(2));
    let (xt, ct, yt, cyt) =
        if unscaled { Default::default() } else { (taps(f.width, w), taps(cw, w), taps(f.height, h), taps(ch, h)) };
    let y_row = |r: u32| &f.data[r as usize * f.stride..][..f.width];
    let uv_row = |r: u32| &f.data[uv_base + r as usize * f.stride..][..f.width];

    let init = || if unscaled { (Vec::new(), Vec::new()) } else { (vec![0u16; f.width], vec![0u16; 2 * cw]) };
    dst[..dw * dh].par_chunks_exact_mut(dw).enumerate().with_min_len(MIN_ROWS_PER_TASK).for_each_init(
        init,
        |(yb, cb), (row, line)| {
            if row < oy || row >= oy + h {
                line.fill(BACKGROUND);
                return;
            }
            line[..ox].fill(BACKGROUND);
            line[ox + w..].fill(BACKGROUND);
            let out = &mut line[ox..ox + w];
            let r = row - oy;
            if unscaled {
                xrgb_row_1to1(y_row(r as u32), uv_row(r as u32 / 2), out);
            } else {
                let (t, c) = (yt[r], cyt[r]);
                blend_rows(y_row(t.a), y_row(t.b), t.f, yb);
                blend_rows(&uv_row(c.a)[..2 * cw], &uv_row(c.b)[..2 * cw], c.f, cb);
                xrgb_row_bilinear(yb, cb, &xt, &ct, out);
            }
        },
    );
    rect
}

pub fn fit_rect(sw: usize, sh: usize, dw: usize, dh: usize) -> (usize, usize, usize, usize) {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return (0, 0, 0, 0);
    }
    let (w, h) = if dw * sh <= dh * sw { (dw, sh * dw / sw) } else { (sw * dh / sh, dh) };
    ((dw - w) / 2, (dh - h) / 2, w, h)
}

type TapCache = (usize, usize, usize, usize, Vec<Tap>, Vec<Tap>);

#[derive(Default)]
pub struct Downscaler {
    half: [Vec<u8>; 2],
    out: Vec<u8>,
    taps: Option<TapCache>,
}

impl Downscaler {
    pub fn scale(&mut self, src: &[u8], stride: usize, sw: usize, sh: usize, dw: usize, dh: usize) -> &[u8] {
        assert!(dw > 0 && dh > 0 && dw <= sw && dh <= sh, "downscale only");
        let [mut a, mut b] = std::mem::take(&mut self.half);
        let (mut level, mut cs, mut cw, mut ch) = (0, stride, sw, sh);
        while cw >= 2 * dw && ch >= 2 * dh {
            let (nw, nh) = (cw / 2, ch / 2);
            level = match level {
                0 => {
                    halve(src, cs, nw, nh, &mut a);
                    1
                }
                1 => {
                    halve(&a, cs, nw, nh, &mut b);
                    2
                }
                _ => {
                    halve(&b, cs, nw, nh, &mut a);
                    1
                }
            };
            (cs, cw, ch) = (nw * 4, nw, nh);
        }
        let cur: &[u8] = match level {
            0 => src,
            1 => &a,
            _ => &b,
        };
        if (cw, ch) == (dw, dh) {
            self.out.clear();
            for y in 0..dh {
                self.out.extend_from_slice(&cur[y * cs..][..dw * 4]);
            }
        } else {
            if self.taps.as_ref().is_none_or(|t| (t.0, t.1, t.2, t.3) != (cw, ch, dw, dh)) {
                self.taps = Some((cw, ch, dw, dh, taps(cw, dw), taps(ch, dh)));
            }
            let (.., xt, yt) = self.taps.as_ref().unwrap();
            self.out.resize(dw * dh * 4, 0);
            self.out.par_chunks_exact_mut(dw * 4).enumerate().with_min_len(MIN_ROWS_PER_TASK).for_each(|(y, row)| {
                let t = yt[y];
                let (r0, r1) = (&cur[t.a as usize * cs..][..cw * 4], &cur[t.b as usize * cs..][..cw * 4]);
                for (px, xt) in row.as_chunks_mut::<4>().0.iter_mut().zip(xt) {
                    let (l, r) = (xt.a as usize * 4, xt.b as usize * 4);
                    for c in 0..4 {
                        let top = mix(r0[l + c] as u32, r0[r + c] as u32, xt.f);
                        let bottom = mix(r1[l + c] as u32, r1[r + c] as u32, xt.f);
                        px[c] = ((top * (256 - t.f) + bottom * t.f + 32_768) >> 16) as u8;
                    }
                }
            });
        }
        self.half = [a, b];
        &self.out
    }
}

fn halve(src: &[u8], stride: usize, w: usize, h: usize, out: &mut Vec<u8>) {
    out.resize(w * h * 4, 0);
    out.par_chunks_exact_mut(w * 4).enumerate().with_min_len(MIN_ROWS_PER_TASK).for_each(|(y, row)| {
        let (r0, r1) = (&src[2 * y * stride..][..w * 8], &src[(2 * y + 1) * stride..][..w * 8]);
        for ((o, a), b) in row.as_chunks_mut::<4>().0.iter_mut().zip(r0.as_chunks::<8>().0).zip(r1.as_chunks::<8>().0) {
            for c in 0..4 {
                o[c] = ((a[c] as u32 + a[c + 4] as u32 + b[c] as u32 + b[c + 4] as u32 + 2) >> 2) as u8;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_bgra(w: usize, h: usize, (r, g, b): (u8, u8, u8)) -> Vec<u8> {
        [b, g, r, 255].repeat(w * h)
    }

    #[test]
    fn roundtrip_colour_is_close() {
        for colour in [(255, 0, 0), (0, 255, 0), (0, 0, 255), (200, 120, 40), (255, 255, 255), (0, 0, 0)] {
            let (w, h) = (64, 32);
            let bgra = solid_bgra(w, h, colour);
            let mut nv12 = Vec::new();
            bgra_to_nv12(&bgra, w * 4, w, h, &mut nv12);
            let f = Nv12Frame { data: nv12, stride: w, alloc_height: h, width: w, height: h };
            let mut out = vec![0u32; w * h];
            nv12_to_xrgb_fit(&f, &mut out, w, h);
            let p = out[w * h / 2 + 3];
            let got = ((p >> 16 & 255) as i32, (p >> 8 & 255) as i32, (p & 255) as i32);
            let want = (colour.0 as i32, colour.1 as i32, colour.2 as i32);
            assert!(
                (got.0 - want.0).abs() < 6 && (got.1 - want.1).abs() < 6 && (got.2 - want.2).abs() < 6,
                "{want:?} -> {got:?}"
            );
        }
    }

    #[test]
    fn respects_stride_padding() {
        let (w, h, stride) = (8, 4, 8 * 4 + 16);
        let mut bgra = vec![0xEEu8; stride * h];
        for row in 0..h {
            bgra[row * stride..row * stride + w * 4].copy_from_slice(&solid_bgra(w, 1, (10, 20, 30)));
        }
        let mut a = Vec::new();
        let mut b = Vec::new();
        bgra_to_nv12(&bgra, stride, w, h, &mut a);
        bgra_to_nv12(&solid_bgra(w, h, (10, 20, 30)), w * 4, w, h, &mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn letterbox_borders_are_filled() {
        let f = Nv12Frame { data: vec![128; 16 * 8 * 3 / 2], stride: 16, alloc_height: 8, width: 16, height: 8 };
        let mut out = vec![0xFFFF_FFFF; 16 * 16];
        let (_, oy, _, h) = nv12_to_xrgb_fit(&f, &mut out, 16, 16);
        assert_eq!((oy, h), (4, 8));
        assert!(out[..16 * 4].iter().all(|&p| p == BACKGROUND));
        assert!(out[16 * 12..].iter().all(|&p| p == BACKGROUND));
    }

    #[test]
    fn taps_cover_the_source() {
        assert!(taps(5, 5).iter().enumerate().all(|(i, t)| t.a == i as u32 && t.f == 0));
        assert_eq!(taps(4, 2), vec![Tap { a: 0, b: 1, f: 128 }, Tap { a: 2, b: 3, f: 128 }]);
        let up = taps(2, 8);
        assert_eq!((up[0].a, up[0].f), (0, 0));
        assert_eq!((up[7].a, up[7].b, up[7].f), (1, 1, 0));
        for (src, dst) in [(1920, 1280), (1280, 1920), (7, 3), (1, 5)] {
            let t = taps(src, dst);
            assert!(t.windows(2).all(|p| p[0].a <= p[1].a && p[0].b <= p[1].b));
            assert!(t.iter().all(|t| t.a <= t.b && (t.b as usize) < src));
        }
    }

    #[test]
    fn thin_lines_survive_downscaling() {
        let (w, h) = (96, 8);
        let mut bgra = solid_bgra(w, h, (255, 255, 255));
        for y in 0..h {
            for x in (1..w).step_by(3) {
                bgra[(y * w + x) * 4..][..3].copy_from_slice(&[0, 0, 0]);
            }
        }
        let mut nv12 = Vec::new();
        bgra_to_nv12(&bgra, w * 4, w, h, &mut nv12);
        let f = Nv12Frame { data: nv12, stride: w, alloc_height: h, width: w, height: h };
        let (dw, dh) = (64, 8 * 64 / 96);
        let mut out = vec![0u32; dw * dh];
        nv12_to_xrgb_fit(&f, &mut out, dw, dh);
        let row = &out[dw * (dh / 2)..][..dw];
        for pair in row.as_chunks::<2>().0 {
            let darkest = pair.iter().map(|p| p & 255).min().unwrap();
            assert!(darkest < 200, "line lost: {pair:x?}");
        }
    }

    #[test]
    #[ignore = "timing; run with --release -- --ignored --nocapture"]
    fn scale_speed() {
        let (w, h) = (1920, 1080);
        let f = Nv12Frame { data: vec![100; w * h * 3 / 2], stride: w, alloc_height: h, width: w, height: h };
        let mut out = vec![0u32; 1280 * 720];
        let t = std::time::Instant::now();
        for _ in 0..100 {
            nv12_to_xrgb_fit(&f, &mut out, 1280, 720);
        }
        println!("1080p -> 720p: {:?} per frame", t.elapsed() / 100);
        let mut full = vec![0u32; w * h];
        let t = std::time::Instant::now();
        for _ in 0..100 {
            nv12_to_xrgb_fit(&f, &mut full, w, h);
        }
        println!("1080p 1:1: {:?} per frame", t.elapsed() / 100);
    }

    #[test]
    fn fit_letterboxes() {
        assert_eq!(fit_rect(1920, 1080, 1920, 1200), (0, 60, 1920, 1080));
        assert_eq!(fit_rect(1920, 1080, 800, 450), (0, 0, 800, 450));
        assert_eq!(fit_rect(1080, 1920, 1000, 1000), (219, 0, 562, 1000));
    }

    #[test]
    fn downscale_keeps_flat_colour_and_size() {
        let (sw, sh) = (1920, 1080);
        let px = [10u8, 120, 240, 255];
        let src: Vec<u8> = px.iter().copied().cycle().take(sw * sh * 4).collect();
        let mut d = Downscaler::default();
        for (dw, dh) in [(1280, 720), (852, 480), (960, 540), (1918, 1078)] {
            let out = d.scale(&src, sw * 4, sw, sh, dw, dh).to_vec();
            assert_eq!(out.len(), dw * dh * 4, "{dw}x{dh}");
            assert!(out.as_chunks::<4>().0.iter().all(|p| *p == px), "{dw}x{dh} colour");
        }
    }

    #[test]
    fn downscale_averages_detail() {
        let (sw, sh) = (8, 8);
        let src: Vec<u8> =
            (0..sw * sh).flat_map(|i| if (i % sw + i / sw) % 2 == 0 { [255u8; 4] } else { [0u8; 4] }).collect();
        let out = Downscaler::default().scale(&src, sw * 4, sw, sh, 4, 4).to_vec();
        assert!(out.iter().all(|&v| (127..=128).contains(&v)), "{out:?}");
    }
}

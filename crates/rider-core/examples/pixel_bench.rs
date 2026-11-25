use rider_core::pixels::{Nv12Frame, bgra_to_nv12, nv12_to_xrgb_fit};
use std::time::Instant;

fn main() {
    for (w, h) in [(1920usize, 1080usize), (2560, 1440)] {
        let bgra: Vec<u8> = (0..w * h * 4).map(|i| (i * 31 % 251) as u8).collect();
        let mut nv12 = Vec::new();
        bgra_to_nv12(&bgra, w * 4, w, h, &mut nv12);
        let n = 200;
        let t = Instant::now();
        for _ in 0..n {
            bgra_to_nv12(&bgra, w * 4, w, h, &mut nv12);
        }
        let enc = t.elapsed().as_secs_f64() * 1000.0 / n as f64;

        let frame = Nv12Frame { data: nv12.clone(), stride: w, alloc_height: h, width: w, height: h };
        let mut out = vec![0u32; w * h];
        nv12_to_xrgb_fit(&frame, &mut out, w, h);
        let t = Instant::now();
        for _ in 0..n {
            nv12_to_xrgb_fit(&frame, &mut out, w, h);
        }
        let dec = t.elapsed().as_secs_f64() * 1000.0 / n as f64;

        let (sw, sh) = (1280, 720);
        let mut small = vec![0u32; sw * sh];
        let t = Instant::now();
        for _ in 0..n {
            nv12_to_xrgb_fit(&frame, &mut small, sw, sh);
        }
        let scaled = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!(
            "{w}x{h}: BGRA->NV12 {enc:.2} ms, NV12->XRGB 1:1 {dec:.2} ms, NV12->XRGB scaled to {sw}x{sh} {scaled:.2} ms"
        );
    }
}

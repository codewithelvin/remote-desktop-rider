//! Portable H.264 software decoder (OpenH264, built from source) for viewers
//! on macOS and Linux. Hosts never send B-frames, so every access unit gives
//! its picture back right away (`decode_frame_no_delay`).

use anyhow::{Context, Result};
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use rider_core::pixels::Nv12Frame;

pub struct H264Decoder {
    dec: Decoder,
}

impl H264Decoder {
    /// The size is read from the stream itself; the arguments only match the
    /// Windows decoder's signature.
    pub fn new(_width: u32, _height: u32) -> Result<Self> {
        Ok(Self { dec: Decoder::new().context("cannot start the H.264 decoder")? })
    }

    /// Feed one Annex-B access unit and return any decoded pictures.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<Nv12Frame>> {
        let Some(yuv) = self.dec.decode(data).context("H.264 decode")? else { return Ok(Vec::new()) };
        Ok(vec![to_nv12(&yuv)])
    }
}

/// Planar I420 (separate U and V) to NV12 (interleaved UV).
fn to_nv12(src: &impl YUVSource) -> Nv12Frame {
    let (w, h) = src.dimensions();
    let (ys, us, vs) = src.strides();
    let (stride, rows) = (w.next_multiple_of(2), h.next_multiple_of(2));
    let mut data = vec![128u8; stride * rows * 3 / 2];
    let (y_plane, uv_plane) = data.split_at_mut(stride * rows);
    for (row, dst) in y_plane.chunks_exact_mut(stride).take(h).enumerate() {
        dst[..w].copy_from_slice(&src.y()[row * ys..][..w]);
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    for (row, dst) in uv_plane.chunks_exact_mut(stride).take(ch).enumerate() {
        let (u, v) = (&src.u()[row * us..][..cw], &src.v()[row * vs..][..cw]);
        for (pair, (&u, &v)) in dst.as_chunks_mut::<2>().0.iter_mut().zip(u.iter().zip(v)) {
            *pair = [u, v];
        }
    }
    Nv12Frame { data, stride, alloc_height: rows, width: w, height: h }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a test picture with the Windows (Media Foundation) encoder the
    /// hosts use, decode it here, and compare: what a macOS or Linux viewer
    /// would show from a real Windows host.
    #[cfg(windows)]
    #[test]
    fn decodes_what_windows_hosts_send() {
        let (w, h) = (320usize, 240usize);
        // A horizontal luma ramp, neutral chroma.
        let mut nv12 = vec![128u8; w * h * 3 / 2];
        for y in 0..h {
            for x in 0..w {
                nv12[y * w + x] = (32 + x * 192 / w) as u8;
            }
        }
        let mut enc = crate::H264Encoder::new(w as u32, h as u32, 30, 2_000_000, false).expect("encoder");
        let mut dec = H264Decoder::new(w as u32, h as u32).unwrap();
        let mut pictures = Vec::new();
        for i in 0..10 {
            for f in enc.encode(&nv12, i * 333_333, i == 0).expect("encode") {
                pictures.extend(dec.decode(&f.data).expect("decode"));
            }
        }
        assert!(!pictures.is_empty(), "no pictures decoded");
        let p = pictures.last().unwrap();
        assert_eq!((p.width, p.height), (w, h));
        let mut err = 0u64;
        for y in 0..h {
            for x in 0..w {
                err += (p.data[y * p.stride + x] as i64 - nv12[y * w + x] as i64).unsigned_abs();
            }
        }
        let mean = err as f64 / (w * h) as f64;
        assert!(mean < 4.0, "mean luma error {mean}");
        let uv = &p.data[p.stride * p.alloc_height..];
        assert!(uv[..w].iter().all(|&c| (c as i32 - 128).abs() < 8), "chroma stays neutral");
    }

    #[test]
    fn odd_sizes_round_up() {
        struct Src(Vec<u8>, Vec<u8>, Vec<u8>);
        impl YUVSource for Src {
            fn dimensions(&self) -> (usize, usize) {
                (5, 3)
            }
            fn strides(&self) -> (usize, usize, usize) {
                (8, 4, 4)
            }
            fn y(&self) -> &[u8] {
                &self.0
            }
            fn u(&self) -> &[u8] {
                &self.1
            }
            fn v(&self) -> &[u8] {
                &self.2
            }
        }
        let s = Src((0..24).collect(), vec![10; 8], vec![20; 8]);
        let f = to_nv12(&s);
        assert_eq!((f.stride, f.alloc_height, f.width, f.height), (6, 4, 5, 3));
        assert_eq!(&f.data[6..11], &[8, 9, 10, 11, 12]);
        assert_eq!(&f.data[24..30], &[10, 20, 10, 20, 10, 20]);
    }
}

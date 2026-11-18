//! The app icon, drawn in code: a blue tile with a monitor and a double
//! chevron ("remote, fast"). Used for the window and tray at run time and,
//! through `build.rs`, for the `.ico` embedded in the executable.

/// Straight-alpha RGBA pixels, `size` x `size`, rows top to bottom.
pub fn rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let p = ((x as f32 + 0.5) / s, (y as f32 + 0.5) / s);
            // Anti-aliasing: coverage from the signed distance in pixels.
            let cover = |d: f32| (0.5 - d * s).clamp(0.0, 1.0);

            let tile = cover(rounded_box(p, (0.5, 0.5), (0.47, 0.47), 0.22));
            let t = (p.0 + p.1) * 0.5; // diagonal gradient
            let bg = [lerp(66.0, 29.0, t), lerp(133.0, 78.0, t), lerp(244.0, 216.0, t)];

            let w = 0.034_f32.max(0.55 / s); // stroke half-width, at least ~1 px
            let screen = rounded_box(p, (0.5, 0.45), (0.30, 0.205), 0.06).abs() - w;
            let stand = rounded_box(p, (0.5, 0.695), (0.05, 0.045), 0.0);
            let foot = rounded_box(p, (0.5, 0.755), (0.16, 0.025), 0.025);
            let chevrons = [0.0, 0.12]
                .iter()
                .map(|dx| {
                    let (a, b, c) = ((0.385 + dx, 0.37), (0.465 + dx, 0.45), (0.385 + dx, 0.53));
                    segment(p, a, b).min(segment(p, b, c)) - w
                })
                .fold(f32::MAX, f32::min);
            let white = cover(screen.min(stand).min(foot).min(chevrons)) * tile;

            for c in bg {
                out.push(lerp(c, 255.0, white).round() as u8);
            }
            out.push((tile * 255.0).round() as u8);
        }
    }
    out
}

/// A Windows `.ico` with one 32-bit image per size (used by build.rs).
#[allow(dead_code)]
pub fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&n| bmp(n)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&n, img) in sizes.iter().zip(&images) {
        let dim = if n >= 256 { 0 } else { n as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend_from_slice(&img);
    }
    out
}

/// BITMAPINFOHEADER + bottom-up BGRA rows + an empty AND mask.
#[allow(dead_code)]
fn bmp(n: u32) -> Vec<u8> {
    let px = rgba(n);
    let mask_row = n.div_ceil(32) * 4;
    let mut out = Vec::with_capacity(40 + px.len() + (mask_row * n) as usize);
    for v in [40u32, n, n * 2] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&[1, 0, 32, 0]);
    out.extend_from_slice(&[0; 24]);
    for row in px.chunks_exact(n as usize * 4).rev() {
        for &[r, g, b, a] in row.as_chunks::<4>().0 {
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    out.resize(out.len() + (mask_row * n) as usize, 0);
    out
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Signed distance to a rounded box (centre, half size, corner radius).
fn rounded_box(p: (f32, f32), c: (f32, f32), half: (f32, f32), r: f32) -> f32 {
    let qx = (p.0 - c.0).abs() - (half.0 - r);
    let qy = (p.1 - c.1).abs() - (half.1 - r);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    outside + qx.max(qy).min(0.0) - r
}

/// Distance to the segment a-b.
fn segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (pax, pay, bax, bay) = (p.0 - a.0, p.1 - a.1, b.0 - a.0, b.1 - a.1);
    let h = ((pax * bax + pay * bay) / (bax * bax + bay * bay)).clamp(0.0, 1.0);
    (pax - bax * h).hypot(pay - bay * h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_opaque_inside_and_clear_at_corners() {
        let px = rgba(32);
        assert_eq!(px.len(), 32 * 32 * 4);
        assert_eq!(px[3], 0, "top-left corner is transparent");
        let centre = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(px[centre + 3], 255);
        let ico = ico(&[16, 32]);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!(ico.len(), 6 + 32 + bmp(16).len() + bmp(32).len());
    }
}

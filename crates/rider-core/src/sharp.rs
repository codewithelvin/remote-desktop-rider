use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

pub const TILE: usize = 128;
pub const MAX_REFRESH_BYTES: usize = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshHeader {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TilePng {
    pub index: u32,
    pub png: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub width: usize,
    pub height: usize,
    pub cols: usize,
    pub rows: usize,
}

impl Grid {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, cols: width.div_ceil(TILE), rows: height.div_ceil(TILE) }
    }

    pub fn len(&self) -> usize {
        self.cols * self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn rect(&self, index: usize) -> (usize, usize, usize, usize) {
        let (x, y) = ((index % self.cols) * TILE, (index / self.cols) * TILE);
        (x, y, TILE.min(self.width - x), TILE.min(self.height - y))
    }

    pub fn mask_len(&self) -> usize {
        self.len().div_ceil(8)
    }
}

pub fn mask_get(mask: &[u8], index: usize) -> bool {
    mask.get(index / 8).is_some_and(|b| b & (1 << (index % 8)) != 0)
}

pub fn mask_set(mask: &mut [u8], index: usize) {
    if let Some(b) = mask.get_mut(index / 8) {
        *b |= 1 << (index % 8);
    }
}

pub struct Tracker {
    grid: Grid,
    prev: Vec<u8>,
    sent: Vec<bool>,
}

impl Tracker {
    pub fn new() -> Self {
        Self { grid: Grid::new(0, 0), prev: Vec::new(), sent: Vec::new() }
    }

    pub fn grid(&self) -> Grid {
        self.grid
    }

    pub fn update(&mut self, bgra: &[u8], width: usize, height: usize) -> Vec<u8> {
        let grid = Grid::new(width, height);
        let mut dirty = vec![0u8; grid.mask_len()];
        if grid != self.grid || self.prev.len() != bgra.len() {
            self.grid = grid;
            self.sent = vec![false; grid.len()];
            dirty.fill(0xFF);
        } else {
            for i in 0..grid.len() {
                let (x, y, w, h) = grid.rect(i);
                let changed = (y..y + h).any(|r| {
                    let at = (r * width + x) * 4;
                    bgra[at..at + w * 4] != self.prev[at..at + w * 4]
                });
                if changed {
                    mask_set(&mut dirty, i);
                    self.sent[i] = false;
                }
            }
        }
        self.prev.clear();
        self.prev.extend_from_slice(bgra);
        dirty
    }

    pub fn lost(&mut self) {
        self.sent.fill(false);
    }

    pub fn unsent(&self) -> Vec<usize> {
        self.sent.iter().enumerate().filter(|(_, s)| !**s).map(|(i, _)| i).collect()
    }

    pub fn take(&mut self, tiles: &[usize]) -> Vec<(usize, Vec<u8>)> {
        tiles
            .iter()
            .filter(|&&i| i < self.grid.len())
            .map(|&i| {
                self.sent[i] = true;
                (i, tile_rgb(&self.prev, self.grid, i))
            })
            .collect()
    }
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

fn tile_rgb(bgra: &[u8], grid: Grid, index: usize) -> Vec<u8> {
    let (x, y, w, h) = grid.rect(index);
    let mut out = Vec::with_capacity(w * h * 3);
    for r in y..y + h {
        let at = (r * grid.width + x) * 4;
        for p in bgra[at..at + w * 4].as_chunks::<4>().0 {
            out.extend_from_slice(&[p[2], p[1], p[0]]);
        }
    }
    out
}

pub fn encode_tile(grid: Grid, index: usize, rgb: &[u8]) -> Result<TilePng> {
    let (_, _, w, h) = grid.rect(index);
    ensure!(rgb.len() == w * h * 3, "tile data does not match its size");
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header()?;
        writer.write_image_data(rgb)?;
        writer.finish()?;
    }
    Ok(TilePng { index: index as u32, png })
}

pub fn decode_tile(grid: Grid, tile: &TilePng) -> Result<(usize, Vec<u32>)> {
    let index = tile.index as usize;
    ensure!(index < grid.len(), "tile {index} is outside the picture");
    let (_, _, w, h) = grid.rect(index);
    let dec = png::Decoder::new(std::io::Cursor::new(&tile.png));
    let mut reader = dec.read_info().context("not a PNG tile")?;
    let info = reader.info();
    ensure!(info.width as usize == w && info.height as usize == h, "tile {index} has the wrong size");
    ensure!(
        info.color_type == png::ColorType::Rgb && info.bit_depth == png::BitDepth::Eight,
        "tile {index} is not 8-bit RGB"
    );
    let mut buf = vec![0u8; w * h * 3];
    reader.next_frame(&mut buf)?;
    let px = buf.as_chunks::<3>().0.iter().map(|p| (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]));
    Ok((index, px.collect()))
}

pub struct Overlay {
    grid: Grid,
    pixels: Vec<u32>,
    valid: Vec<Option<u64>>,
    dirtied: Vec<u64>,
    floor: u64,
}

impl Overlay {
    pub fn new() -> Self {
        Self { grid: Grid::new(0, 0), pixels: Vec::new(), valid: Vec::new(), dirtied: Vec::new(), floor: 0 }
    }

    fn resize(&mut self, width: usize, height: usize) {
        let grid = Grid::new(width, height);
        if grid != self.grid {
            self.grid = grid;
            self.pixels = vec![0; width * height];
            self.valid = vec![None; grid.len()];
            self.dirtied = vec![0; grid.len()];
        }
    }

    pub fn frame(&mut self, seq: u64, width: usize, height: usize, dirty: &[u8]) {
        if Grid::new(width, height) != self.grid {
            self.resize(width, height);
            self.floor = seq;
            self.dirtied.fill(seq);
            return;
        }
        for i in 0..self.grid.len() {
            if mask_get(dirty, i) {
                self.dirtied[i] = self.dirtied[i].max(seq);
                if self.valid[i].is_some_and(|v| v < seq) {
                    self.valid[i] = None;
                }
            }
        }
    }

    pub fn lost(&mut self, seq: u64) {
        self.valid.fill(None);
        self.floor = self.floor.max(seq);
    }

    pub fn insert(&mut self, seq: u64, width: usize, height: usize, index: usize, px: &[u32]) -> bool {
        if Grid::new(width, height) != self.grid || index >= self.grid.len() || seq < self.floor {
            return false;
        }
        if self.dirtied[index] > seq {
            return false;
        }
        let (x, y, w, h) = self.grid.rect(index);
        if px.len() != w * h {
            return false;
        }
        for r in 0..h {
            let at = (y + r) * self.grid.width + x;
            self.pixels[at..at + w].copy_from_slice(&px[r * w..(r + 1) * w]);
        }
        self.valid[index] = Some(seq);
        true
    }

    pub fn covers(&self, width: usize, height: usize) -> bool {
        Grid::new(width, height) == self.grid && self.valid.iter().any(Option::is_some)
    }

    pub fn paint(&self, dst: &mut [u32]) {
        for (i, v) in self.valid.iter().enumerate() {
            if v.is_none() {
                continue;
            }
            let (x, y, w, h) = self.grid.rect(i);
            for r in y..y + h {
                let at = r * self.grid.width + x;
                dst[at..at + w].copy_from_slice(&self.pixels[at..at + w]);
            }
        }
    }
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(w: usize, h: usize, shade: u8) -> Vec<u8> {
        (0..w * h).flat_map(|i| [(i % 251) as u8, shade, (i % 7) as u8, 255]).collect()
    }

    #[test]
    fn grid_covers_ragged_edges() {
        let g = Grid::new(300, 130);
        assert_eq!((g.cols, g.rows, g.len()), (3, 2, 6));
        assert_eq!(g.rect(2), (256, 0, 44, 128));
        assert_eq!(g.rect(5), (256, 128, 44, 2));
        assert_eq!(g.mask_len(), 1);
    }

    #[test]
    fn tracker_marks_only_changed_tiles() {
        let (w, h) = (300, 200);
        let mut t = Tracker::new();
        let first = screen(w, h, 1);
        assert!(t.update(&first, w, h).iter().all(|&b| b == 0xFF), "a new picture is all dirty");
        assert_eq!(t.unsent().len(), 6);
        let sent = t.take(&t.unsent());
        assert_eq!(sent.len(), 6);
        assert_eq!(t.unsent(), Vec::<usize>::new());
        let mut next = first.clone();
        let at = (150 * w + 260) * 4;
        next[at] ^= 0xFF;
        let dirty = t.update(&next, w, h);
        assert!(mask_get(&dirty, 5) && (0..5).all(|i| !mask_get(&dirty, i)));
        assert_eq!(t.unsent(), [5]);
        t.lost();
        assert_eq!(t.unsent().len(), 6);
    }

    #[test]
    fn tiles_survive_png_exactly() {
        let (w, h) = (300, 200);
        let mut t = Tracker::new();
        let bgra = screen(w, h, 9);
        t.update(&bgra, w, h);
        let grid = t.grid();
        for (i, rgb) in t.take(&[0, 5]) {
            let png = encode_tile(grid, i, &rgb).unwrap();
            let (index, px) = decode_tile(grid, &png).unwrap();
            assert_eq!(index, i);
            let (x, y, tw, _) = grid.rect(i);
            let src = (y * w + x) * 4;
            assert_eq!(
                px[0],
                (u32::from(bgra[src + 2]) << 16) | (u32::from(bgra[src + 1]) << 8) | u32::from(bgra[src])
            );
            assert_eq!(px.len(), rgb.len() / 3);
            assert_eq!(tw * grid.rect(i).3, px.len());
        }
        assert!(decode_tile(grid, &TilePng { index: 99, png: Vec::new() }).is_err());
    }

    #[test]
    fn overlay_keeps_tiles_until_they_change() {
        let (w, h) = (300, 200);
        let mut o = Overlay::new();
        o.frame(10, w, h, &[0xFF]);
        assert!(!o.covers(w, h));
        let px = vec![7u32; TILE * TILE];
        assert!(o.insert(10, w, h, 0, &px));
        assert!(o.covers(w, h));
        o.frame(11, w, h, &[0b10]);
        assert!(o.covers(w, h), "another tile changed");
        o.frame(12, w, h, &[0b1]);
        assert!(!o.covers(w, h), "its own tile changed");
        assert!(!o.insert(11, w, h, 0, &px), "a refresh older than the change is refused");
        assert!(o.insert(12, w, h, 0, &px));
        let mut dst = vec![0u32; w * h];
        o.paint(&mut dst);
        assert_eq!(dst[0], 7);
        assert_eq!(dst[TILE], 0);
        o.lost(20);
        assert!(!o.covers(w, h));
        assert!(!o.insert(19, w, h, 0, &px), "nothing from before the loss");
        o.frame(21, 640, 480, &[]);
        assert!(!o.insert(21, w, h, 0, &px), "the picture size changed");
    }
}

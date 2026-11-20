//! Write the app icon (`rider_core::icon`) as a PNG, for the macOS and Linux
//! packages: `cargo run -p rider-core --example icon-png -- 512 icon.png`.
//!
//! A minimal PNG encoder (stored, uncompressed deflate): the packaging tools
//! (iconutil, dpkg) recompress or do not care, and it needs no dependency.

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { 0xEDB8_8320 ^ (crc >> 1) } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn png(rgba: &[u8], size: u32) -> Vec<u8> {
    // Each row: filter byte 0, then the pixels.
    let row = size as usize * 4;
    let mut raw = Vec::with_capacity((row + 1) * size as usize);
    for line in rgba.chunks_exact(row) {
        raw.push(0);
        raw.extend_from_slice(line);
    }
    // zlib stream of stored deflate blocks (at most 65535 bytes each).
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, no interlace
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(size), Some(path)) = (args.next().and_then(|s| s.parse::<u32>().ok()), args.next()) else {
        eprintln!("usage: icon-png <size> <out.png>");
        std::process::exit(2);
    };
    std::fs::write(&path, png(&rider_core::icon::rgba(size), size)).expect("write PNG");
}

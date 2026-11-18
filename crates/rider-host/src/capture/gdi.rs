use super::{Capturer, Frame};
use anyhow::{Result, bail};
use rider_core::proto::MonitorInfo;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Gdi::*;

/// Fallback capture via BitBlt from the screen DC. Slower than DXGI and has no
/// change detection, so it is paced to ~30 fps and compares frames itself.
pub struct GdiCapturer {
    x: i32,
    y: i32,
    width: usize,
    height: usize,
    screen: HDC,
    mem: HDC,
    bitmap: HBITMAP,
    buf: Vec<u8>,
    prev_hash: u64,
    last: Instant,
}

unsafe impl Send for GdiCapturer {}

impl GdiCapturer {
    pub fn new(m: &MonitorInfo) -> Result<Self> {
        unsafe {
            let screen = GetDC(None);
            if screen.is_invalid() {
                bail!("GetDC failed");
            }
            let mem = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, m.width as i32, m.height as i32);
            SelectObject(mem, bitmap.into());
            Ok(Self {
                x: m.x,
                y: m.y,
                width: m.width as usize,
                height: m.height as usize,
                screen,
                mem,
                bitmap,
                buf: vec![0; m.width as usize * m.height as usize * 4],
                prev_hash: 0,
                last: Instant::now() - Duration::from_secs(1),
            })
        }
    }
}

impl Drop for GdiCapturer {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.mem);
            ReleaseDC(None, self.screen);
        }
    }
}

fn quick_hash(data: &[u8]) -> u64 {
    // FNV-style hash over every word; a typed character changes only a few pixels.
    let mut h = 0xcbf29ce484222325u64;
    for chunk in data.as_chunks::<8>().0 {
        h = (h ^ u64::from_le_bytes(*chunk)).wrapping_mul(0x100000001b3);
    }
    h
}

impl Capturer for GdiCapturer {
    fn backend(&self) -> &'static str {
        "GDI"
    }

    fn next_frame(&mut self, timeout_ms: u32) -> Result<Option<Frame<'_>>> {
        let period = Duration::from_millis(33);
        let wait = period.saturating_sub(self.last.elapsed());
        if wait > Duration::from_millis(timeout_ms as u64) {
            std::thread::sleep(Duration::from_millis(timeout_ms as u64));
            return Ok(None);
        }
        std::thread::sleep(wait);
        self.last = Instant::now();
        unsafe {
            BitBlt(self.mem, 0, 0, self.width as i32, self.height as i32, Some(self.screen), self.x, self.y, SRCCOPY)?;
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: self.width as i32,
                    biHeight: -(self.height as i32), // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let lines = GetDIBits(
                self.mem,
                self.bitmap,
                0,
                self.height as u32,
                Some(self.buf.as_mut_ptr() as _),
                &mut bmi,
                DIB_RGB_COLORS,
            );
            if lines == 0 {
                bail!("GetDIBits failed");
            }
        }
        let h = quick_hash(&self.buf);
        if h == self.prev_hash {
            return Ok(None);
        }
        self.prev_hash = h;
        Ok(Some(Frame { data: &self.buf, stride: self.width * 4, width: self.width, height: self.height }))
    }
}

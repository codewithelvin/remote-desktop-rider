//! Screen capture. DXGI Desktop Duplication first (GPU, only on change);
//! GDI BitBlt as a fallback for outputs DXGI cannot duplicate (some
//! DisplayLink / indirect display drivers, or remote sessions).

mod dxgi;
mod gdi;

use anyhow::Result;
use rider_core::proto::MonitorInfo;
use windows::Win32::Graphics::Dxgi::*;

/// A captured BGRA frame borrowed from the capturer.
pub struct Frame<'a> {
    pub data: &'a [u8],
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

pub trait Capturer: Send {
    /// Wait up to `timeout_ms` for a new frame. `Ok(None)` means the screen
    /// did not change.
    fn next_frame(&mut self, timeout_ms: u32) -> Result<Option<Frame<'_>>>;
    fn backend(&self) -> &'static str;
}

/// A monitor as seen by capture: protocol info plus where to find it in DXGI.
#[derive(Clone, Debug)]
pub struct Monitor {
    pub info: MonitorInfo,
    adapter: u32,
    output: u32,
}

/// Enumerate all monitors attached to the desktop, primary first.
pub fn monitors() -> Result<Vec<Monitor>> {
    let mut list = Vec::new();
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                let desc = output.GetDesc()?;
                if desc.AttachedToDesktop.as_bool() {
                    let r = desc.DesktopCoordinates;
                    let name = String::from_utf16_lossy(&desc.DeviceName)
                        .trim_end_matches('\0')
                        .trim_start_matches(r"\\.\")
                        .to_string();
                    list.push(Monitor {
                        info: MonitorInfo {
                            index: 0,
                            name,
                            x: r.left,
                            y: r.top,
                            width: (r.right - r.left) as u32,
                            height: (r.bottom - r.top) as u32,
                            primary: r.left == 0 && r.top == 0,
                        },
                        adapter: a,
                        output: o,
                    });
                }
                o += 1;
            }
            a += 1;
        }
    }
    // Some drivers report the same output under several adapters.
    list.sort_by_key(|m| (!m.info.primary, m.info.x, m.info.y));
    list.dedup_by(|a, b| a.info.x == b.info.x && a.info.y == b.info.y && a.info.width == b.info.width);
    for (i, m) in list.iter_mut().enumerate() {
        m.info.index = i as u32;
    }
    if list.is_empty() {
        anyhow::bail!("no monitors attached to the desktop");
    }
    Ok(list)
}

pub fn open(m: &Monitor) -> Result<Box<dyn Capturer>> {
    match dxgi::DxgiCapturer::new(m.adapter, m.output) {
        Ok(c) => Ok(Box::new(c)),
        Err(e) => {
            tracing::warn!("DXGI duplication failed for {} ({e:#}); using GDI capture", m.info.name);
            Ok(Box::new(gdi::GdiCapturer::new(&m.info)?))
        }
    }
}

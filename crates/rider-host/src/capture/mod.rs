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

/// Enumerate all monitors attached to the desktop, primary first, named as
/// Windows Settings names them ("DELL U2720Q", "Built-in display").
pub fn monitors() -> Result<Vec<Monitor>> {
    let names = friendly_names();
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
                    let device = String::from_utf16_lossy(&desc.DeviceName).trim_end_matches('\0').to_string();
                    let name =
                        names.get(&device).cloned().unwrap_or_else(|| device.trim_start_matches(r"\\.\").to_string());
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
    // Two of the same model: "DELL P2419H", "DELL P2419H (2)".
    for i in 1..list.len() {
        let same = list[..i].iter().filter(|m| m.info.name == list[i].info.name).count();
        if same > 0 {
            list[i].info.name = format!("{} ({})", list[i].info.name, same + 1);
        }
    }
    if list.is_empty() {
        anyhow::bail!("no monitors attached to the desktop");
    }
    Ok(list)
}

/// The monitor's own name for each GDI device (`\\.\DISPLAY1` -> "DELL U2720Q"),
/// read from the display configuration: the names Windows Settings shows. A
/// laptop's panel often has none: "Built-in display". Empty if Windows can't
/// tell (the device names are used then).
fn friendly_names() -> std::collections::HashMap<String, String> {
    use windows::Win32::Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_MODE_INFO,
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL,
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
        DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
        QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
    };
    let mut names = std::collections::HashMap::new();
    let text = |w: &[u16]| String::from_utf16_lossy(w).trim_end_matches('\0').trim().to_string();
    // SAFETY: the arrays are sized by GetDisplayConfigBufferSizes; each request
    // packet is the struct its header's type and size describe.
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).is_err() {
            return names;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None)
            .is_err()
        {
            return names;
        }
        for path in &paths[..np as usize] {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            source.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            source.header.adapterId = path.sourceInfo.adapterId;
            source.header.id = path.sourceInfo.id;
            let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            target.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            target.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            target.header.adapterId = path.targetInfo.adapterId;
            target.header.id = path.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0
                || DisplayConfigGetDeviceInfo(&mut target.header) != 0
            {
                continue;
            }
            let builtin = [
                DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL,
                DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED,
                DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED,
            ]
            .contains(&target.outputTechnology);
            let name = match text(&target.monitorFriendlyDeviceName) {
                n if !n.is_empty() => n,
                _ if builtin => "Built-in display".to_string(),
                _ => continue,
            };
            names.insert(text(&source.viewGdiDeviceName), name);
        }
    }
    names
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

#[cfg(test)]
mod tests {
    /// Needs a signed-in desktop: `cargo test -p rider-host -- --ignored monitors_have_names --nocapture`.
    #[test]
    #[ignore]
    fn monitors_have_names() {
        let list = super::monitors().unwrap();
        for m in &list {
            println!(
                "{} {} {}x{} primary={}",
                m.info.index + 1,
                m.info.name,
                m.info.width,
                m.info.height,
                m.info.primary
            );
            assert!(!m.info.name.is_empty());
        }
    }
}

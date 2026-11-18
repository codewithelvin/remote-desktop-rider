use super::{Capturer, Frame};
use anyhow::{Context, Result};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::Interface;

pub struct DxgiCapturer {
    adapter_idx: u32,
    output_idx: u32,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    dupl: Option<IDXGIOutputDuplication>,
    staging: Option<ID3D11Texture2D>,
    width: usize,
    height: usize,
    /// Staging texture currently mapped for the CPU (the last returned frame
    /// borrows its memory). Unmapped at the next call or on drop: no extra copy.
    mapped: bool,
    has_frame: bool,
}

// Used only from the dedicated video thread.
unsafe impl Send for DxgiCapturer {}

impl DxgiCapturer {
    pub fn new(adapter_idx: u32, output_idx: u32) -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let adapter = factory.EnumAdapters1(adapter_idx)?;
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .context("D3D11CreateDevice")?;
            let mut c = Self {
                adapter_idx,
                output_idx,
                device: device.unwrap(),
                context: context.unwrap(),
                dupl: None,
                staging: None,
                width: 0,
                height: 0,
                mapped: false,
                has_frame: false,
            };
            c.duplicate()?;
            Ok(c)
        }
    }

    fn duplicate(&mut self) -> Result<()> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let output = factory.EnumAdapters1(self.adapter_idx)?.EnumOutputs(self.output_idx)?;
            let output1: IDXGIOutput1 = output.cast()?;
            let dupl = output1.DuplicateOutput(&self.device).context("DuplicateOutput")?;
            let desc = dupl.GetDesc();
            self.width = desc.ModeDesc.Width as usize;
            self.height = desc.ModeDesc.Height as usize;
            self.unmap();
            self.dupl = Some(dupl);
            self.staging = None;
            Ok(())
        }
    }

    fn unmap(&mut self) {
        if self.mapped {
            if let Some(s) = &self.staging {
                // SAFETY: `s` was mapped by us on this thread and the previous
                // frame borrow ended (we hold `&mut self`).
                unsafe { self.context.Unmap(s, 0) };
            }
            self.mapped = false;
        }
    }

    fn staging(&mut self, src: &ID3D11Texture2D) -> Result<ID3D11Texture2D> {
        if let Some(s) = &self.staging {
            return Ok(s.clone());
        }
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            src.GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            desc.MipLevels = 1;
            desc.ArraySize = 1;
            desc.SampleDesc = DXGI_SAMPLE_DESC { Count: 1, Quality: 0 };
            let mut tex = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut tex))?;
            let tex = tex.unwrap();
            self.staging = Some(tex.clone());
            Ok(tex)
        }
    }
}

impl Capturer for DxgiCapturer {
    fn backend(&self) -> &'static str {
        "DXGI"
    }

    fn next_frame(&mut self, timeout_ms: u32) -> Result<Option<Frame<'_>>> {
        self.unmap();
        if self.dupl.is_none() {
            // Lost earlier (desktop switch, mode change); try again.
            if self.duplicate().is_err() {
                std::thread::sleep(std::time::Duration::from_millis(100));
                return Ok(None);
            }
        }
        let dupl = self.dupl.clone().unwrap();
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res = None;
            match dupl.AcquireNextFrame(timeout_ms, &mut info, &mut res) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST || e.code() == DXGI_ERROR_INVALID_CALL => {
                    tracing::info!("desktop duplication lost ({:?}); re-creating", e.code());
                    self.dupl = None;
                    return Ok(None);
                }
                Err(e) => return Err(e.into()),
            }
            // Only the mouse moved: the client draws its own cursor, so skip.
            if info.LastPresentTime == 0 && self.has_frame {
                dupl.ReleaseFrame()?;
                return Ok(None);
            }
            let tex: ID3D11Texture2D = res.unwrap().cast()?;
            let staging = self.staging(&tex)?;
            self.context.CopyResource(&staging, &tex);
            dupl.ReleaseFrame()?;

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            self.mapped = true;
            self.has_frame = true;
            let stride = mapped.RowPitch as usize;
            // SAFETY: the mapping covers RowPitch * height bytes and stays valid
            // until `unmap`, which needs `&mut self`, so it outlives this borrow.
            let data = std::slice::from_raw_parts(mapped.pData as *const u8, stride * self.height);
            Ok(Some(Frame { data, stride, width: self.width, height: self.height }))
        }
    }
}

impl Drop for DxgiCapturer {
    fn drop(&mut self) {
        self.unmap();
    }
}

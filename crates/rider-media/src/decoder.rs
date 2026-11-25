use crate::mf::*;
use anyhow::{Context, Result};
use rider_core::pixels::Nv12Frame;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::Interface;

pub struct H264Decoder {
    mft: IMFTransform,
    in_id: u32,
    out_id: u32,
    out_info: MFT_OUTPUT_STREAM_INFO,
    stride: usize,
    alloc_height: usize,
    width: usize,
    height: usize,
    time: i64,
}

unsafe impl Send for H264Decoder {}

impl H264Decoder {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        init_thread()?;
        unsafe {
            let mft: IMFTransform = CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)?;
            if let Ok(c) = mft.cast::<ICodecAPI>() {
                let _ = c.SetValue(&CODECAPI_AVLowLatencyMode, &variant_bool(true));
            }
            if let Ok(attrs) = mft.GetAttributes() {
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let (in_id, out_id) = stream_ids(&mft);
            let inp = video_type(&MFVideoFormat_H264, width, height, 60)?;
            mft.SetInputType(in_id, &inp, 0).context("decoder SetInputType(H264)")?;
            let mut d = Self {
                mft,
                in_id,
                out_id,
                out_info: Default::default(),
                stride: width as usize,
                alloc_height: height as usize,
                width: width as usize,
                height: height as usize,
                time: 0,
            };
            d.negotiate_output()?;
            d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(d)
        }
    }

    fn negotiate_output(&mut self) -> Result<()> {
        unsafe {
            let mut i = 0;
            loop {
                let t = self.mft.GetOutputAvailableType(self.out_id, i).context("decoder has no NV12 output")?;
                if t.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                    self.mft.SetOutputType(self.out_id, &t, 0)?;
                    if let Ok(size) = t.GetUINT64(&MF_MT_FRAME_SIZE) {
                        let (w, h) = unpack(size);
                        self.stride = w as usize;
                        self.alloc_height = h as usize;
                    }
                    if let Ok(stride) = t.GetUINT32(&MF_MT_DEFAULT_STRIDE) {
                        self.stride = (stride as i32).unsigned_abs() as usize;
                    }
                    break;
                }
                i += 1;
            }
            self.out_info = self.mft.GetOutputStreamInfo(self.out_id)?;
        }
        Ok(())
    }

    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<Nv12Frame>> {
        self.time += 166_666;
        let sample = sample_from_bytes(data, self.time, 166_666)?;
        let mut frames = Vec::new();
        unsafe {
            if let Err(e) = self.mft.ProcessInput(self.in_id, &sample, 0) {
                if e.code() != MF_E_NOTACCEPTING {
                    return Err(e.into());
                }
                self.drain(&mut frames)?;
                self.mft.ProcessInput(self.in_id, &sample, 0)?;
            }
        }
        self.drain(&mut frames)?;
        Ok(frames)
    }

    fn drain(&mut self, frames: &mut Vec<Nv12Frame>) -> Result<()> {
        loop {
            let alloc = if provides_samples(&self.out_info) {
                None
            } else {
                Some(self.out_info.cbSize.max((self.stride * self.alloc_height * 3 / 2) as u32))
            };
            match process_output(&self.mft, self.out_id, alloc)? {
                Output::Sample(s) => {
                    let data = sample_bytes(&s)?;
                    if data.len() >= self.stride * self.alloc_height * 3 / 2 {
                        frames.push(Nv12Frame {
                            data,
                            stride: self.stride,
                            alloc_height: self.alloc_height,
                            width: self.width.min(self.stride),
                            height: self.height.min(self.alloc_height),
                        });
                    }
                }
                Output::StreamChange => self.negotiate_output()?,
                Output::NeedMoreInput => return Ok(()),
            }
        }
    }
}

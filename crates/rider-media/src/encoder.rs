use crate::mf::{
    Output, init_thread, process_output, provides_samples, sample_bytes, sample_from_bytes, stream_ids, variant_bool,
    variant_u32, video_type,
};
use anyhow::{Context, Result, bail};
use std::time::{Duration, Instant};
use windows::Win32::Media::MediaFoundation::{
    CLSID_MSH264EncoderMFT, CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode,
    CODECAPI_AVEncMPVDefaultBPictureCount, CODECAPI_AVEncMPVGOPSize, CODECAPI_AVEncVideoForceKeyFrame,
    CODECAPI_AVLowLatencyMode, ICodecAPI, IMFActivate, IMFMediaEventGenerator, IMFSample, IMFTransform,
    METransformHaveOutput, METransformNeedInput, MF_E_NO_EVENTS_AVAILABLE, MF_E_NOTACCEPTING, MF_EVENT_FLAG_NO_WAIT,
    MF_LOW_LATENCY, MF_MT_AVG_BITRATE, MF_MT_MPEG2_PROFILE, MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK,
    MFMediaType_Video, MFSampleExtension_CleanPoint, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG, MFT_ENUM_FLAG_HARDWARE,
    MFT_ENUM_FLAG_SORTANDFILTER, MFT_FRIENDLY_NAME_Attribute, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_END_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_OUTPUT_STREAM_INFO, MFT_REGISTER_TYPE_INFO, MFTEnumEx, MFVideoFormat_H264, MFVideoFormat_NV12,
    eAVEncCommonRateControlMode_CBR, eAVEncH264VProfile_Main,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::core::{Interface, PWSTR};

pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub keyframe: bool,
    pub time_100ns: i64,
}

pub struct H264Encoder {
    mft: IMFTransform,
    codec: Option<ICodecAPI>,
    events: Option<IMFMediaEventGenerator>,
    pending_need_input: u32,
    in_id: u32,
    out_id: u32,
    out_info: MFT_OUTPUT_STREAM_INFO,
    pub width: u32,
    pub height: u32,
    fps: u32,
    pub name: String,
    pub hardware: bool,
}

unsafe impl Send for H264Encoder {}

impl H264Encoder {
    pub fn new(width: u32, height: u32, fps: u32, bitrate_bps: u32, allow_hardware: bool) -> Result<Self> {
        init_thread()?;
        if allow_hardware {
            match Self::try_hardware(width, height, fps, bitrate_bps) {
                Ok(e) => return Ok(e),
                Err(e) => tracing::warn!("hardware H.264 encoder unavailable, using software: {e:#}"),
            }
        }
        let mft: IMFTransform = unsafe { CoCreateInstance(&CLSID_MSH264EncoderMFT, None, CLSCTX_INPROC_SERVER)? };
        Self::configure(mft, "Microsoft H.264 (software)".into(), false, width, height, fps, bitrate_bps)
    }

    fn try_hardware(width: u32, height: u32, fps: u32, bitrate_bps: u32) -> Result<Self> {
        unsafe {
            let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
            let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
            let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFTEnumEx(
                MFT_CATEGORY_VIDEO_ENCODER,
                MFT_ENUM_FLAG(MFT_ENUM_FLAG_HARDWARE.0 | MFT_ENUM_FLAG_SORTANDFILTER.0),
                Some(&raw const input),
                Some(&raw const output),
                &raw mut list,
                &raw mut count,
            )?;
            let activates: Vec<IMFActivate> = if list.is_null() {
                Vec::new()
            } else {
                let v =
                    std::slice::from_raw_parts_mut(list, count as usize).iter_mut().filter_map(Option::take).collect();
                CoTaskMemFree(Some(list as _));
                v
            };
            let mut last_err = anyhow::anyhow!("no hardware H.264 encoder found");
            for act in activates {
                let name = friendly_name(&act).unwrap_or_else(|| "hardware encoder".into());
                let attempt = act
                    .ActivateObject::<IMFTransform>()
                    .map_err(anyhow::Error::from)
                    .and_then(|mft| Self::configure(mft, name.clone(), true, width, height, fps, bitrate_bps));
                match attempt {
                    Ok(e) => return Ok(e),
                    Err(e) => {
                        tracing::warn!("{name}: {e:#}");
                        let _ = act.ShutdownObject();
                        last_err = e;
                    }
                }
            }
            Err(last_err)
        }
    }

    fn configure(
        mft: IMFTransform,
        name: String,
        hardware: bool,
        width: u32,
        height: u32,
        fps: u32,
        bitrate_bps: u32,
    ) -> Result<Self> {
        unsafe {
            let mut events = None;
            if let Ok(attrs) = mft.GetAttributes() {
                if attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) == 1 {
                    attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
                    events = Some(mft.cast::<IMFMediaEventGenerator>()?);
                }
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let codec = mft.cast::<ICodecAPI>().ok();
            if let Some(c) = &codec {
                apply_low_latency(c, fps, bitrate_bps);
            }
            let (in_id, out_id) = stream_ids(&mft);

            let out = video_type(&MFVideoFormat_H264, width, height, fps)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate_bps)?;
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)?;
            mft.SetOutputType(out_id, &out, 0).context("SetOutputType(H264)")?;
            let inp = video_type(&MFVideoFormat_NV12, width, height, fps)?;
            mft.SetInputType(in_id, &inp, 0).context("SetInputType(NV12)")?;

            if let Some(c) = &codec {
                apply_low_latency(c, fps, bitrate_bps);
            }

            let out_info = mft.GetOutputStreamInfo(out_id)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

            tracing::info!("H.264 encoder: {name} ({width}x{height}@{fps}, {} kbps)", bitrate_bps / 1000);
            Ok(Self {
                mft,
                codec,
                events,
                pending_need_input: 0,
                in_id,
                out_id,
                out_info,
                width,
                height,
                fps,
                name,
                hardware,
            })
        }
    }

    pub fn set_bitrate(&mut self, bps: u32) {
        if let Some(c) = &self.codec {
            unsafe {
                let _ = c.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &variant_u32(bps));
            }
        }
    }

    pub fn encode(&mut self, nv12: &[u8], time_100ns: i64, force_keyframe: bool) -> Result<Vec<EncodedFrame>> {
        if force_keyframe && let Some(c) = &self.codec {
            unsafe {
                let _ = c.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &variant_u32(1));
            }
        }
        let sample = sample_from_bytes(nv12, time_100ns, 10_000_000 / i64::from(self.fps))?;
        let mut out = Vec::new();
        if self.events.is_some() {
            self.encode_async(&sample, &mut out)?;
        } else {
            self.encode_sync(&sample, &mut out)?;
        }
        Ok(out)
    }

    fn encode_sync(&mut self, sample: &IMFSample, out: &mut Vec<EncodedFrame>) -> Result<()> {
        unsafe {
            if let Err(e) = self.mft.ProcessInput(self.in_id, sample, 0) {
                if e.code() != MF_E_NOTACCEPTING {
                    return Err(e.into());
                }
                self.drain(out)?;
                self.mft.ProcessInput(self.in_id, sample, 0)?;
            }
        }
        self.drain(out)
    }

    fn drain(&mut self, out: &mut Vec<EncodedFrame>) -> Result<()> {
        loop {
            match self.pull()? {
                Some(f) => out.push(f),
                None => return Ok(()),
            }
        }
    }

    fn encode_async(&mut self, sample: &IMFSample, out: &mut Vec<EncodedFrame>) -> Result<()> {
        let deadline = Instant::now() + Duration::from_millis(500);
        while self.pending_need_input == 0 {
            if !self.poll_event(out)? {
                if Instant::now() > deadline {
                    bail!("hardware encoder stalled (no input request)");
                }
                std::thread::sleep(Duration::from_micros(500));
            }
        }
        self.pending_need_input -= 1;
        unsafe { self.mft.ProcessInput(self.in_id, sample, 0)? };

        let before = out.len();
        let deadline = Instant::now() + Duration::from_millis(40);
        while out.len() == before && Instant::now() < deadline {
            if !self.poll_event(out)? {
                std::thread::sleep(Duration::from_micros(250));
            }
        }
        while self.poll_event(out)? {}
        Ok(())
    }

    fn poll_event(&mut self, out: &mut Vec<EncodedFrame>) -> Result<bool> {
        let generator = self.events.as_ref().expect("async encoder");
        let ev = match unsafe { generator.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
            Ok(ev) => ev,
            Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        let kind = unsafe { ev.GetType()? } as i32;
        if kind == METransformNeedInput.0 {
            self.pending_need_input += 1;
        } else if kind == METransformHaveOutput.0
            && let Some(f) = self.pull()?
        {
            out.push(f);
        }
        Ok(true)
    }

    fn pull(&mut self) -> Result<Option<EncodedFrame>> {
        let alloc = if provides_samples(&self.out_info) {
            None
        } else {
            Some(self.out_info.cbSize.max(self.width * self.height * 3 / 2))
        };
        match process_output(&self.mft, self.out_id, alloc)? {
            Output::Sample(s) => unsafe {
                let keyframe = s.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) == 1;
                let time_100ns = s.GetSampleTime().unwrap_or(0);
                Ok(Some(EncodedFrame { data: sample_bytes(&s)?, keyframe, time_100ns }))
            },
            Output::NeedMoreInput => Ok(None),
            Output::StreamChange => unsafe {
                let t = self.mft.GetOutputAvailableType(self.out_id, 0)?;
                self.mft.SetOutputType(self.out_id, &t, 0)?;
                self.out_info = self.mft.GetOutputStreamInfo(self.out_id)?;
                Ok(None)
            },
        }
    }
}

impl Drop for H264Encoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}

fn apply_low_latency(c: &ICodecAPI, fps: u32, bitrate_bps: u32) {
    unsafe {
        let _ = c.SetValue(&CODECAPI_AVLowLatencyMode, &variant_bool(true));
        let _ =
            c.SetValue(&CODECAPI_AVEncCommonRateControlMode, &variant_u32(eAVEncCommonRateControlMode_CBR.0 as u32));
        let _ = c.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &variant_u32(bitrate_bps));
        let _ = c.SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &variant_u32(0));
        let _ = c.SetValue(&CODECAPI_AVEncMPVGOPSize, &variant_u32(fps * 10));
    }
}

fn friendly_name(act: &IMFActivate) -> Option<String> {
    unsafe {
        let mut p = PWSTR::null();
        let mut len = 0u32;
        act.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &raw mut p, &raw mut len).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
}

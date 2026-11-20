//! Media Foundation helpers shared by the encoder and decoder.

use anyhow::Result;
use std::mem::ManuallyDrop;
use std::sync::Once;
use windows::Win32::Foundation::VARIANT_TRUE;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL, VT_UI4};
use windows::core::GUID;

static MF_INIT: Once = Once::new();

/// Initialise COM for the current thread and Media Foundation for the process.
pub fn init_thread() -> Result<()> {
    unsafe {
        // S_FALSE (already initialised) is fine.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut res = Ok(());
    MF_INIT.call_once(|| unsafe {
        res = MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(Into::into);
    });
    res
}

pub(crate) fn variant_u32(v: u32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_UI4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { ulVal: v },
            }),
        },
    }
}

pub(crate) fn variant_bool(v: bool) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_BOOL,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { boolVal: if v { VARIANT_TRUE } else { Default::default() } },
            }),
        },
    }
}

pub(crate) fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

pub(crate) fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

pub(crate) fn video_type(subtype: &GUID, width: u32, height: u32, fps: u32) -> Result<IMFMediaType> {
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
        t.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        Ok(t)
    }
}

/// Wrap a byte slice in a new single-buffer `IMFSample`.
pub(crate) fn sample_from_bytes(data: &[u8], time_100ns: i64, duration_100ns: i64) -> Result<IMFSample> {
    unsafe {
        let buf = MFCreateMemoryBuffer(data.len() as u32)?;
        let mut ptr = std::ptr::null_mut();
        buf.Lock(&mut ptr, None, None)?;
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
        buf.Unlock()?;
        buf.SetCurrentLength(data.len() as u32)?;
        let sample = MFCreateSample()?;
        sample.AddBuffer(&buf)?;
        sample.SetSampleTime(time_100ns)?;
        sample.SetSampleDuration(duration_100ns)?;
        Ok(sample)
    }
}

pub(crate) fn empty_sample(size: u32) -> Result<IMFSample> {
    unsafe {
        let buf = MFCreateMemoryBuffer(size)?;
        let sample = MFCreateSample()?;
        sample.AddBuffer(&buf)?;
        Ok(sample)
    }
}

/// Copy the contents of a sample into a `Vec`.
pub(crate) fn sample_bytes(sample: &IMFSample) -> Result<Vec<u8>> {
    unsafe {
        let buf = sample.ConvertToContiguousBuffer()?;
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        buf.Lock(&mut ptr, None, Some(&mut len))?;
        let v = std::slice::from_raw_parts(ptr, len as usize).to_vec();
        buf.Unlock()?;
        Ok(v)
    }
}

pub(crate) enum Output {
    Sample(IMFSample),
    NeedMoreInput,
    StreamChange,
}

/// One `ProcessOutput` call on stream `stream_id`.
pub(crate) fn process_output(mft: &IMFTransform, stream_id: u32, alloc: Option<u32>) -> Result<Output> {
    unsafe {
        let sample = match alloc {
            Some(size) => Some(empty_sample(size)?),
            None => None,
        };
        let mut bufs = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: stream_id,
            pSample: ManuallyDrop::new(sample),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        }];
        let mut status = 0u32;
        let r = mft.ProcessOutput(0, &mut bufs, &mut status);
        let sample = ManuallyDrop::take(&mut bufs[0].pSample);
        drop(ManuallyDrop::take(&mut bufs[0].pEvents));
        match r {
            Ok(()) => sample.map(Output::Sample).ok_or_else(|| anyhow::anyhow!("MFT returned no sample")),
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(Output::NeedMoreInput),
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => Ok(Output::StreamChange),
            Err(e) => Err(e.into()),
        }
    }
}

pub(crate) fn stream_ids(mft: &IMFTransform) -> (u32, u32) {
    let mut i = [0u32];
    let mut o = [0u32];
    unsafe {
        match mft.GetStreamIDs(&mut i, &mut o) {
            Ok(()) => (i[0], o[0]),
            Err(_) => (0, 0), // E_NOTIMPL: fixed stream ids starting at 0
        }
    }
}

pub(crate) fn provides_samples(info: &MFT_OUTPUT_STREAM_INFO) -> bool {
    let mask = (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32;
    info.dwFlags & mask != 0
}

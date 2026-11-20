//! H.264 for the host (encoder) and the viewer (decoder).
//!
//! Windows: Media Foundation. The encoder prefers hardware MFTs (Intel
//! QuickSync, NVIDIA NVENC, AMD AMF) and falls back to the Microsoft software
//! encoder. Both ends run in low-latency mode: no B-frames, one frame in and
//! one frame out.
//!
//! Other systems (the macOS viewer): a portable software decoder (OpenH264);
//! see `soft`. The host side is Windows-only for now (docs/MACOS.md).

#[cfg(windows)]
pub mod decoder;
#[cfg(windows)]
pub mod encoder;
#[cfg(windows)]
mod mf;
#[cfg(any(not(windows), test))]
pub mod soft;

#[cfg(windows)]
pub use decoder::H264Decoder;
#[cfg(windows)]
pub use encoder::{EncodedFrame, H264Encoder};
#[cfg(windows)]
pub use mf::init_thread;

#[cfg(not(windows))]
pub use soft::H264Decoder;

/// Per-thread setup before using a decoder (nothing to do here).
#[cfg(not(windows))]
pub fn init_thread() -> anyhow::Result<()> {
    Ok(())
}

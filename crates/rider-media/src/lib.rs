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

#[cfg(not(windows))]
pub fn init_thread() -> anyhow::Result<()> {
    Ok(())
}

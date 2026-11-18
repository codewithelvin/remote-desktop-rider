//! Wire protocol.
//!
//! One QUIC connection carries:
//! * **Control stream** (first bidirectional stream, opened by the client):
//!   length-prefixed postcard messages, reliable and ordered. Used for
//!   handshake, keys and buttons, clipboard and monitor selection.
//! * **Video**: one unidirectional host→client stream *per frame*. Frames do
//!   not block each other (no head-of-line blocking), and the host resets a
//!   frame that is too old instead of retransmitting stale pixels.
//! * **Datagrams**: unreliable, for mouse motion where only the latest position matters.
//! * **File transfer**: one extra bidirectional stream per file.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const PROTOCOL_VERSION: u32 = 1;
const MAX_CONTROL_MSG: usize = 4 * 1024 * 1024;
pub const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MonitorInfo {
    pub index: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ClientMsg {
    Hello {
        version: u32,
        device_name: String,
        pairing: bool,
    },
    PairConfirm {
        accepted: bool,
    },
    Auth {
        pin: String,
    },
    SelectMonitor {
        index: u32,
    },
    RequestKeyframe,
    Clipboard {
        text: String,
    },
    Ping {
        client_us: u64,
    },
    Key {
        scancode: u16,
        extended: bool,
        down: bool,
    },
    MouseButton {
        button: MouseButton,
        down: bool,
    },
    /// Wheel deltas in Windows units (120 = one notch).
    MouseWheel {
        dx: i32,
        dy: i32,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum HostMsg {
    HelloAck {
        version: u32,
        host_name: String,
    },
    /// Sent during pairing once the host operator has answered.
    PairResult {
        accepted: bool,
    },
    AuthResult {
        ok: bool,
        message: String,
    },
    Monitors {
        list: Vec<MonitorInfo>,
        active: u32,
    },
    Clipboard {
        text: String,
    },
    Pong {
        client_us: u64,
        host_us: u64,
    },
    /// Sent once per second so the client can show what the host is doing.
    Stats {
        fps: f32,
        bitrate_kbps: u32,
        encode_ms: f32,
        encoder: String,
    },
    Bye {
        reason: String,
    },
}

/// Mouse motion sent as a QUIC datagram. Coordinates are normalised to
/// 0..=65535 across the currently streamed monitor.
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct MouseMove {
    pub seq: u32,
    pub x: u16,
    pub y: u16,
}

/// Prefix of every video frame stream; the H.264 Annex-B payload follows.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FrameHeader {
    pub seq: u64,
    pub capture_us: u64,
    pub encode_us: u32,
    pub keyframe: bool,
    pub width: u32,
    pub height: u32,
}

/// Prefix of a file-transfer stream (client → host); raw bytes follow.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileHeader {
    pub name: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileResult {
    pub ok: bool,
    pub message: String,
}

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>> {
    Ok(postcard::to_stdvec(msg)?)
}

pub fn decode<T: DeserializeOwned>(buf: &[u8]) -> Result<T> {
    Ok(postcard::from_bytes(buf)?)
}

/// Write one length-prefixed message.
pub async fn write_msg<T: Serialize>(s: &mut quinn::SendStream, msg: &T) -> Result<()> {
    let body = encode(msg)?;
    let mut buf = Vec::with_capacity(body.len() + 4);
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(&body);
    s.write_all(&buf).await?;
    Ok(())
}

/// Read one length-prefixed message; `Ok(None)` on a clean end of stream.
pub async fn read_msg<T: DeserializeOwned>(s: &mut quinn::RecvStream) -> Result<Option<T>> {
    let mut len = [0u8; 4];
    match s.read_exact(&mut len).await {
        Ok(()) => {}
        Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_CONTROL_MSG {
        bail!("control message too large ({len} bytes)");
    }
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).await?;
    Ok(Some(decode(&body)?))
}

/// Split a received frame stream into header and payload.
pub fn split_frame(buf: &[u8]) -> Result<(FrameHeader, &[u8])> {
    if buf.len() < 4 {
        bail!("short frame");
    }
    let hlen = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if buf.len() < 4 + hlen {
        bail!("truncated frame header");
    }
    let header = decode(&buf[4..4 + hlen])?;
    Ok((header, &buf[4 + hlen..]))
}

pub fn frame_prefix(h: &FrameHeader) -> Result<Vec<u8>> {
    let body = encode(h)?;
    let mut buf = Vec::with_capacity(body.len() + 4);
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(&body);
    Ok(buf)
}

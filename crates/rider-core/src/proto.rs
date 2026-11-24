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
    /// Ctrl+Alt+Del on the host (cannot be sent as normal keystrokes).
    SendSas,
    /// Encode at most this many lines (e.g. 720), 0 = full resolution.
    /// Only sent to hosts that speak `tls::ALPN_V2`.
    SetQuality {
        max_height: u32,
    },
    /// Lock the host (Win+L cannot be sent: Windows reserves it locally).
    /// Only sent to hosts that speak `tls::ALPN_V2`.
    LockScreen,
    /// A button at a position (as `MouseMove`). Motion travels as datagrams
    /// and buttons on this stream, so a press could overtake the last move
    /// and land beside a title bar or window border; this one cannot.
    /// `seq`: the last `MouseMove` sent, so older ones are not replayed after.
    /// Only sent to hosts that speak `tls::ALPN_V3`.
    MouseButtonAt {
        button: MouseButton,
        down: bool,
        x: u16,
        y: u16,
        seq: u32,
    },
}

/// Quality choices offered to the user: (max height, label); 0 = automatic.
pub const QUALITIES: [(u32, &str); 4] = [(0, "Auto"), (1080, "1080p"), (720, "720p"), (480, "480p")];

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
    /// The host pointer's shape changed: a `CursorShape` as a number, so a
    /// shape added later reads as an arrow instead of breaking the session.
    /// Only sent to clients that speak `tls::ALPN_V3`.
    Cursor {
        shape: u8,
    },
    /// What the person at the host allows. Sent at the start and whenever it
    /// changes. Only sent to clients that speak `tls::ALPN_V4`; the host
    /// enforces it for every client.
    Permissions(Permissions),
}

/// What the viewer may do, decided by the person at the controlled computer.
/// Seeing the screen is always allowed.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    /// Mouse, keyboard, Ctrl+Alt+Del and lock.
    pub control: bool,
    pub clipboard: bool,
    /// Send files to the controlled computer.
    pub files: bool,
}

/// The three choices offered to the person at the controlled computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Full,
    Standard,
    ViewOnly,
}

impl Access {
    pub const ALL: [Access; 3] = [Access::Full, Access::Standard, Access::ViewOnly];

    pub fn permissions(self) -> Permissions {
        match self {
            Access::Full => Permissions { control: true, clipboard: true, files: true },
            Access::Standard => Permissions { control: true, clipboard: true, files: false },
            Access::ViewOnly => Permissions { control: false, clipboard: false, files: false },
        }
    }

    /// Name used on the command line and in the app's JSON lines.
    pub fn code(self) -> &'static str {
        match self {
            Access::Full => "full",
            Access::Standard => "standard",
            Access::ViewOnly => "view",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Access::ALL.into_iter().find(|a| a.code() == s)
    }
}

impl Permissions {
    /// The named level these permissions match, if any.
    pub fn access(self) -> Option<Access> {
        Access::ALL.into_iter().find(|a| a.permissions() == self)
    }

    /// As a byte (for an atomic): bit 0 control, 1 clipboard, 2 files.
    pub fn bits(self) -> u8 {
        self.control as u8 | (self.clipboard as u8) << 1 | (self.files as u8) << 2
    }

    pub fn from_bits(b: u8) -> Self {
        Permissions { control: b & 1 != 0, clipboard: b & 2 != 0, files: b & 4 != 0 }
    }
}

/// Pointer shapes the viewer can show (values are on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CursorShape {
    Arrow = 0,
    Text = 1,
    Wait = 2,
    Progress = 3,
    Crosshair = 4,
    Hand = 5,
    ResizeWE = 6,
    ResizeNS = 7,
    ResizeNWSE = 8,
    ResizeNESW = 9,
    Move = 10,
    NotAllowed = 11,
    Help = 12,
}

impl CursorShape {
    /// Unknown values (from a newer host) are shown as an arrow.
    pub fn from_u8(v: u8) -> Self {
        use CursorShape::*;
        [
            Arrow, Text, Wait, Progress, Crosshair, Hand, ResizeWE, ResizeNS, ResizeNWSE, ResizeNESW, Move, NotAllowed,
            Help,
        ]
        .get(v as usize)
        .copied()
        .unwrap_or(Arrow)
    }
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
    match read_body(s).await? {
        Some(body) => Ok(Some(decode(&body)?)),
        None => Ok(None),
    }
}

/// Like [`read_msg`], but skips messages this version does not know (a
/// newer peer's additions) instead of failing: each message is length
/// prefixed, so the next one is found either way. For control streams.
pub async fn read_known<T: DeserializeOwned>(s: &mut quinn::RecvStream) -> Result<Option<T>> {
    loop {
        let Some(body) = read_body(s).await? else { return Ok(None) };
        match decode(&body) {
            Ok(m) => return Ok(Some(m)),
            Err(e) => tracing::debug!("skipped an unknown control message ({} bytes): {e}", body.len()),
        }
    }
}

async fn read_body(s: &mut quinn::RecvStream) -> Result<Option<Vec<u8>>> {
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
    Ok(Some(body))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_shapes_round_trip_and_unknown_is_arrow() {
        for v in 0..=12u8 {
            assert_eq!(CursorShape::from_u8(v) as u8, v);
        }
        assert_eq!(CursorShape::from_u8(13), CursorShape::Arrow);
        assert_eq!(CursorShape::from_u8(255), CursorShape::Arrow);
    }

    #[test]
    fn older_messages_keep_their_wire_form() {
        // New variants go at the end: what older peers send decodes the same.
        assert_eq!(encode(&ClientMsg::LockScreen).unwrap(), [12]);
        assert_eq!(encode(&HostMsg::Bye { reason: String::new() }).unwrap(), [7, 0]);
        let at = ClientMsg::MouseButtonAt { button: MouseButton::Left, down: true, x: 1, y: 2, seq: 3 };
        assert_eq!(encode(&at).unwrap()[0], 13);
        assert_eq!(encode(&HostMsg::Cursor { shape: 6 }).unwrap(), [8, 6]);
        let p = Access::Standard.permissions();
        assert_eq!(encode(&HostMsg::Permissions(p)).unwrap(), [9, 1, 1, 0]);
    }

    #[test]
    fn access_levels() {
        for a in Access::ALL {
            assert_eq!(Access::from_code(a.code()), Some(a));
            assert_eq!(a.permissions().access(), Some(a));
            assert_eq!(Permissions::from_bits(a.permissions().bits()), a.permissions());
        }
        assert_eq!(Access::from_code("everything"), None);
        let view = Access::ViewOnly.permissions();
        assert!(!view.control && !view.clipboard && !view.files);
        // Control without clipboard is no named level.
        assert_eq!(Permissions { control: true, clipboard: false, files: false }.access(), None);
    }

    #[test]
    fn unknown_messages_from_newer_peers_fail_to_decode() {
        // What `read_known` relies on: a variant past the end is an error,
        // not a misread message.
        assert!(decode::<HostMsg>(&[200, 1, 2]).is_err());
        assert!(decode::<ClientMsg>(&[99]).is_err());
    }
}

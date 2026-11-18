//! Shared building blocks for Remote Desktop Rider: device identity,
//! pinned mutual-TLS over QUIC, the wire protocol and pixel conversion.

pub mod clipboard;
pub mod icon;
pub mod identity;
pub mod pixels;
pub mod proto;
pub mod rendezvous;
pub mod sas;
pub mod tls;

use std::time::{SystemTime, UNIX_EPOCH};

/// UDP port the host listens on by default.
pub const DEFAULT_PORT: u16 = 47800;

/// Wall-clock microseconds since the Unix epoch. Used for latency estimates;
/// host/client clock offset is corrected with ping/pong.
pub fn now_us() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0)
}

/// Per-user application data directory, e.g. `%APPDATA%\RemoteDesktopRider\<role>`.
pub fn data_dir(role: &str) -> anyhow::Result<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("APPDATA is not set"))?;
    let dir = base.join("RemoteDesktopRider").join(role);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Read a secret as one line from stdin (for `--pin-stdin`, scripts and
/// automation). The trailing newline is stripped; an empty secret is an error.
pub fn secret_from_stdin() -> anyhow::Result<String> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let secret = line.trim_end_matches(['\r', '\n']).to_owned();
    anyhow::ensure!(!secret.is_empty(), "no PIN on stdin");
    Ok(secret)
}

/// This machine's name, used as a default device name.
pub fn machine_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".into())
}

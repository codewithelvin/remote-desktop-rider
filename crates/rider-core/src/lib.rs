//! Shared building blocks for Remote Desktop Rider: device identity,
//! pinned mutual-TLS over QUIC, the wire protocol and pixel conversion.

pub mod clipboard;
pub mod company;
pub mod diag;
#[cfg(windows)]
pub mod dpi;
pub mod https;
pub mod i18n;
pub mod icon;
pub mod identity;
pub mod pixels;
pub mod proto;
pub mod rendezvous;
pub mod sas;
pub mod session_files;
pub mod tls;
pub mod transfer;
pub mod tunnel;
#[cfg(windows)]
pub mod update;

use std::time::{SystemTime, UNIX_EPOCH};

/// UDP port the host listens on by default.
pub const DEFAULT_PORT: u16 = 47800;

/// Wall-clock microseconds since the Unix epoch. Used for latency estimates;
/// host/client clock offset is corrected with ping/pong.
pub fn now_us() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0)
}

/// Per-user application data directory for `role`:
/// * Windows: `%APPDATA%\RemoteDesktopRider\<role>`
/// * macOS: `~/Library/Application Support/RemoteDesktopRider/<role>`
/// * Linux and others: `$XDG_CONFIG_HOME` (or `~/.config`)`/RemoteDesktopRider/<role>`
pub fn data_dir(role: &str) -> anyhow::Result<std::path::PathBuf> {
    let dir = app_dir()?.join(role);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// `…/RemoteDesktopRider`, the per-user base of [`data_dir`] (not created).
pub fn app_dir() -> anyhow::Result<std::path::PathBuf> {
    use std::path::PathBuf;
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(windows) {
        var("APPDATA").ok_or_else(|| anyhow::anyhow!("APPDATA is not set"))?
    } else {
        let home = || var("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"));
        if cfg!(target_os = "macos") {
            home()?.join("Library").join("Application Support")
        } else {
            match var("XDG_CONFIG_HOME") {
                Some(x) => x,
                None => home()?.join(".config"),
            }
        }
    };
    Ok(base.join("RemoteDesktopRider"))
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
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").ok();
    #[cfg(unix)]
    let name = {
        let mut buf = [0u8; 256];
        // SAFETY: the buffer is valid for its length; gethostname NUL-terminates
        // on success (we also stop at the first NUL ourselves).
        let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        // "mac-mini.local" → "mac-mini"
        ok.then(|| String::from_utf8_lossy(&buf[..len]).split('.').next().unwrap_or_default().to_owned())
    };
    #[cfg(not(any(windows, unix)))]
    let name: Option<String> = None;
    name.filter(|n| !n.is_empty()).unwrap_or_else(|| "unknown".into())
}

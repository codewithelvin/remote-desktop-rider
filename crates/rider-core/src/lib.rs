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
pub mod sharp;
pub mod tls;
pub mod transfer;
pub mod tunnel;
#[cfg(windows)]
pub mod update;

use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_PORT: u16 = 47800;

pub fn now_us() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64)
}

pub fn data_dir(role: &str) -> anyhow::Result<std::path::PathBuf> {
    let dir = app_dir()?.join(role);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

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

pub fn secret_from_stdin() -> anyhow::Result<String> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let secret = line.trim_end_matches(['\r', '\n']).to_owned();
    anyhow::ensure!(!secret.is_empty(), "no PIN on stdin");
    Ok(secret)
}

pub fn machine_name() -> String {
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").ok();
    #[cfg(unix)]
    let name = {
        let mut buf = [0u8; 256];
        let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        ok.then(|| String::from_utf8_lossy(&buf[..len]).split('.').next().unwrap_or_default().to_owned())
    };
    #[cfg(not(any(windows, unix)))]
    let name: Option<String> = None;
    name.filter(|n| !n.is_empty()).unwrap_or_else(|| "unknown".into())
}

//! System plumbing: the command-line tools installed next to the app, and
//! (Windows) elevation, autostart and the unattended-access service state.
//! macOS and Linux get the same interface; there this device only controls
//! other computers, so the host-side answers are "off".

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};

pub use platform::*;

/// One of the command-line tools shipped next to this executable
/// (`rider-client.exe` on Windows, `rider-client` elsewhere).
pub fn tool(exe: &str) -> Command {
    let exe = if cfg!(windows) { exe } else { exe.trim_end_matches(".exe") };
    let path =
        std::env::current_exe().ok().and_then(|p| p.parent().map(|dir| dir.join(exe))).unwrap_or_else(|| exe.into());
    let mut cmd = Command::new(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// Keeps console tools from flashing a console window.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // Text the tool shows (errors, the session window) in the app's language.
    cmd.env("RIDER_LANG", rider_core::i18n::lang().code());
    cmd
}

/// Run a tool to completion. `Ok(stdout)` on success, otherwise the error it printed.
#[cfg_attr(not(windows), allow(dead_code))] // Manage only
pub fn run(exe: &str, args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    let mut child = tool(exe)
        .args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| rider_core::trf!("cannot start {exe}: {e}", "{exe} başladıla bilmir: {e}"))?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        use std::io::Write;
        let _ = writeln!(pipe, "{input}");
    }
    let mut err = String::new();
    let stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        if let Some(mut s) = stderr {
            let _ = s.read_to_string(&mut err);
        }
        err
    });
    let mut out = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut out);
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let err = reader.join().unwrap_or_default();
    if status.success() { Ok(out) } else { Err(error_text(&err)) }
}

/// The meaningful part of a tool's stderr: the anyhow error report, without log lines.
/// Shown when a tool exits without saying why (e.g. ended from Task Manager).
pub fn no_details() -> &'static str {
    rider_core::i18n::tr("It stopped unexpectedly.", "Gözlənilmədən dayandı.")
}

pub fn error_text(stderr: &str) -> String {
    let report = stderr.find("Error: ").map_or(stderr, |i| &stderr[i + 7..]);
    let text: Vec<&str> = report
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && l != &"Caused by:" && !l.contains(" INFO ") && !l.contains(" WARN "))
        .collect();
    if text.is_empty() { no_details().into() } else { text.join("\n") }
}

/// Forward each line of a pipe to `f` on a background thread.
pub fn lines(pipe: impl Read + Send + 'static, mut f: impl FnMut(String) + Send + 'static) {
    std::thread::spawn(move || {
        for line in BufReader::new(pipe).lines() {
            match line {
                Ok(l) => f(l),
                Err(_) => break,
            }
        }
    });
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(not(windows), allow(dead_code))] // always Off off Windows
pub enum RemoteAccess {
    Off,
    Stopped,
    On,
}

#[cfg(windows)]
mod platform {
    use super::RemoteAccess;
    use anyhow::{Result, bail};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_DWORD, REG_SZ, REG_VALUE_TYPE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegDeleteKeyValueW,
        RegGetValueW, RegSetKeyValueW,
    };
    use windows::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR, w};

    const SERVICE_NAME: &str = "RemoteDesktopRider";

    pub fn is_elevated() -> bool {
        // SAFETY: no arguments; reads the current process token.
        unsafe { IsUserAnAdmin().as_bool() }
    }

    /// Start this app again with administrator rights (one UAC prompt).
    pub fn relaunch_elevated(args: &str) -> Result<()> {
        let exe = std::env::current_exe()?;
        // SAFETY: all strings are valid NUL-terminated HSTRINGs that outlive the call.
        let r = unsafe {
            ShellExecuteW(
                None,
                w!("runas"),
                &HSTRING::from(exe.as_os_str()),
                &HSTRING::from(args),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        // Values above 32 mean success (documented ShellExecute convention).
        if r.0 as isize <= 32 {
            bail!("administrator rights were not granted");
        }
        Ok(())
    }

    /// Whether the unattended-access service is installed and running
    /// (works without administrator rights).
    pub fn remote_access() -> RemoteAccess {
        use windows_service::service::{ServiceAccess, ServiceState};
        use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
        let Ok(mgr) = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT) else {
            return RemoteAccess::Off;
        };
        let Ok(svc) = mgr.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) else {
            return RemoteAccess::Off;
        };
        match svc.query_status() {
            Ok(s) if s.current_state == ServiceState::Running => RemoteAccess::On,
            _ => RemoteAccess::Stopped,
        }
    }

    const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
    const APP_KEY: PCWSTR = w!(r"Software\RemoteDesktopRider");
    const VALUE: PCWSTR = w!("RemoteDesktopRider");

    /// Whether the app starts (into the tray) when the user signs in.
    pub fn autostart() -> bool {
        // SAFETY: existence check only; no output buffers.
        unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE, RRF_RT_REG_SZ, None, None, None) }.is_ok()
    }

    pub fn set_autostart(on: bool) -> Result<()> {
        if on {
            let cmd = format!("\"{}\" --tray", std::env::current_exe()?.display());
            set_value(RUN_KEY, VALUE, REG_SZ, &wide_bytes(&cmd))
        } else {
            // SAFETY: valid key and value names.
            match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE) } {
                ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
                e => bail!("cannot change the startup setting: {e:?}"),
            }
        }
    }

    /// A per-user yes/no remembered by the app (e.g. a hint already shown).
    /// Installed copies start at sign-in unless the user turned that off: enabled
    /// once per user on first start, and kept pointing at the current executable.
    /// Running from the installed location (`%ProgramFiles%\RemoteDesktopRider`).
    pub fn installed() -> bool {
        std::env::current_exe()
            .ok()
            .and_then(|exe| {
                let pf = std::env::var_os("ProgramFiles")?;
                let dir = std::path::Path::new(&pf).join("RemoteDesktopRider");
                Some(exe.parent()?.to_string_lossy().eq_ignore_ascii_case(&dir.to_string_lossy()))
            })
            .unwrap_or(false)
    }

    pub fn default_autostart() {
        let first = "AutostartDefaulted";
        if installed() && !flag(first) {
            set_flag(first);
            let _ = set_autostart(true);
        } else if autostart() {
            let _ = set_autostart(true);
        }
    }

    pub fn flag(name: &str) -> bool {
        let name = HSTRING::from(name);
        // SAFETY: existence check only.
        unsafe { RegGetValueW(HKEY_CURRENT_USER, APP_KEY, &name, RRF_RT_REG_DWORD, None, None, None) }.is_ok()
    }

    pub fn set_flag(name: &str) {
        let name = HSTRING::from(name);
        let _ = set_value(APP_KEY, PCWSTR(name.as_ptr()), REG_DWORD, &1u32.to_le_bytes());
    }

    pub fn clear_flag(name: &str) {
        let name = HSTRING::from(name);
        // SAFETY: valid key and value names; a missing value is fine.
        let _ = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, APP_KEY, &name) };
    }

    fn set_value(key: PCWSTR, name: PCWSTR, kind: REG_VALUE_TYPE, data: &[u8]) -> Result<()> {
        // SAFETY: `data` is valid for its length during the call.
        unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, key, name, kind.0, Some(data.as_ptr().cast()), data.len() as u32) }
            .ok()?;
        Ok(())
    }

    fn wide_bytes(s: &str) -> Vec<u8> {
        s.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect()
    }

    /// True for the first instance of the main window in this user session. The
    /// mutex is deliberately kept until the process exits.
    pub fn first_instance() -> bool {
        use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
        use windows::Win32::System::Threading::CreateMutexW;
        // SAFETY: named mutex creation; the handle is intentionally leaked.
        unsafe {
            let created = CreateMutexW(None, false, w!(r"Local\RemoteDesktopRider.App"));
            created.is_ok() && GetLastError() != ERROR_ALREADY_EXISTS
        }
    }

    /// Stop the main window shaking when dragged between monitors with
    /// different scaling (see `rider_core::dpi`). Cheap after the first call.
    pub fn smooth_monitor_moves(frame: &eframe::Frame) {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        static DONE: std::sync::Once = std::sync::Once::new();
        if DONE.is_completed() {
            return;
        }
        if let Ok(h) = frame.window_handle()
            && let RawWindowHandle::Win32(h) = h.as_raw()
        {
            DONE.call_once(|| {
                rider_core::dpi::follow_windows(windows::Win32::Foundation::HWND(h.hwnd.get() as *mut _))
            });
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::RemoteAccess;
    use anyhow::{Result, bail};

    pub fn is_elevated() -> bool {
        false
    }

    pub fn relaunch_elevated(_args: &str) -> Result<()> {
        bail!("only on Windows")
    }

    /// This device cannot be controlled yet on macOS and Linux.
    pub fn remote_access() -> RemoteAccess {
        RemoteAccess::Off
    }

    /// No tray icon to start into; starting at sign-in comes with it.
    pub fn autostart() -> bool {
        false
    }

    pub fn set_autostart(_on: bool) -> Result<()> {
        bail!("not available on this system yet")
    }

    pub fn default_autostart() {}

    fn flag_file(name: &str) -> Option<std::path::PathBuf> {
        Some(rider_core::app_dir().ok()?.join("flags").join(name))
    }

    /// A per-user yes/no remembered by the app (e.g. a hint already shown).
    pub fn flag(name: &str) -> bool {
        flag_file(name).is_some_and(|f| f.exists())
    }

    pub fn set_flag(name: &str) {
        if let Some(f) = flag_file(name) {
            let _ = f.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(f, b"1");
        }
    }

    pub fn first_instance() -> bool {
        true
    }

    pub fn smooth_monitor_moves(_frame: &eframe::Frame) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_text_strips_logs_and_headers() {
        let stderr =
            "2026-11-18T10:00:00Z  INFO rider: starting\nError: authentication failed\n\nCaused by:\n    wrong PIN\n";
        assert_eq!(error_text(stderr), "authentication failed\nwrong PIN");
        assert_eq!(error_text(""), no_details());
    }
}

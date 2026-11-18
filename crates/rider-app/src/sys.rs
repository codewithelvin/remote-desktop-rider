//! Windows plumbing: elevation, the command-line tools installed next to the
//! app, and the unattended-access service state.

use anyhow::{Result, bail};
use std::io::{BufRead, BufReader, Read};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use windows::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

/// Keeps console tools from flashing a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
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

/// One of the command-line tools shipped next to this executable.
pub fn tool(exe: &str) -> Command {
    let path =
        std::env::current_exe().ok().and_then(|p| p.parent().map(|dir| dir.join(exe))).unwrap_or_else(|| exe.into());
    let mut cmd = Command::new(path);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Run a tool to completion. `Ok(stdout)` on success, otherwise the error it printed.
pub fn run(exe: &str, args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    let mut child = tool(exe)
        .args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start {exe}: {e}"))?;
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
pub fn error_text(stderr: &str) -> String {
    let report = stderr.find("Error: ").map_or(stderr, |i| &stderr[i + 7..]);
    let text: Vec<&str> = report
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && l != &"Caused by:" && !l.contains(" INFO ") && !l.contains(" WARN "))
        .collect();
    if text.is_empty() { "failed (no details)".into() } else { text.join("\n") }
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
pub enum RemoteAccess {
    Off,
    Stopped,
    On,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_text_strips_logs_and_headers() {
        let stderr =
            "2026-11-18T10:00:00Z  INFO rider: starting\nError: authentication failed\n\nCaused by:\n    wrong PIN\n";
        assert_eq!(error_text(stderr), "authentication failed\nwrong PIN");
        assert_eq!(error_text(""), "failed (no details)");
    }
}

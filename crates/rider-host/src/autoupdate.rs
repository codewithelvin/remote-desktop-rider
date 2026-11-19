//! Unattended updates by the service: a computer that is only reached
//! remotely has nobody to click "Install update", so the service (SYSTEM)
//! checks the GitHub releases itself and installs a newer version when nobody
//! is connected. The installer stops this service, replaces the programs and
//! starts it again; the service then reopens the tray app for the signed-in
//! user if it was open before.
//!
//! - Turned on by default; `rider-host auto-update off` (or Manage) disables it.
//! - Never while a session is active: the agent marks sessions through
//!   [`SESSION_EVENT`], and the check is repeated right before installing.
//! - The installer is downloaded into a folder only SYSTEM and Administrators
//!   can write, and checked against the SHA-256 GitHub publishes for it, so
//!   nothing else can swap what runs as SYSTEM.

use crate::config::HostConfig;
use anyhow::{Context, Result};
use rider_core::update;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::RemoteDesktop::{
    WTS_CURRENT_SERVER_HANDLE, WTS_PROCESS_INFOW, WTSEnumerateProcessesW, WTSFreeMemory, WTSQueryUserToken,
};
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateEventW, EVENT_MODIFY_STATE, OpenEventW, PROCESS_INFORMATION, ResetEvent,
    STARTUPINFOW, SetEvent, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR, w};

/// Set while a remote session is active (manual reset). Created by the
/// service, so only SYSTEM and Administrators can change it.
pub const SESSION_EVENT: PCWSTR = w!(r"Global\RemoteDesktopRider-Session");

const FIRST_CHECK: Duration = Duration::from_secs(2 * 60);
const EVERY: Duration = Duration::from_secs(3600);
/// After a failed or postponed attempt.
const RETRY: Duration = Duration::from_secs(30 * 60);
const APP_EXE: &str = "RemoteDesktopRider.exe";

/// Agent side: a session started or ended (no-op without the service).
pub fn mark_session(active: bool) {
    // SAFETY: the handle is closed after use.
    unsafe {
        if let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, SESSION_EVENT) {
            let _ = if active { SetEvent(event) } else { ResetEvent(event) };
            let _ = CloseHandle(event);
        }
    }
}

/// Service side: the session flag and the background checker.
pub struct Updater {
    event: HANDLE,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Updater {
    pub fn start() -> Result<Self> {
        // SAFETY: manual-reset, initially clear; default DACL (SYSTEM + Administrators).
        let event = unsafe { CreateEventW(None, true, false, SESSION_EVENT)? };
        let stop = Arc::new(AtomicBool::new(false));
        let (ev, st) = (event.0 as usize, stop.clone());
        let thread = std::thread::Builder::new()
            .name("autoupdate".into())
            .spawn(move || run(HANDLE(ev as *mut _), &st))
            .context("cannot start the update checker")?;
        Ok(Self { event, stop, thread: Some(thread) })
    }

    /// A new agent starts with no session (the old one may have died mid-session).
    pub fn reset_session(&self) {
        // SAFETY: our own event handle.
        unsafe {
            let _ = ResetEvent(self.event);
        }
    }
}

impl Drop for Updater {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        // SAFETY: created in `start`, closed once, after the thread using it ended.
        unsafe {
            let _ = CloseHandle(self.event);
        }
    }
}

fn session_active(event: HANDLE) -> bool {
    // SAFETY: valid event handle; zero timeout.
    unsafe { WaitForSingleObject(event, 0) == WAIT_OBJECT_0 }
}

fn run(event: HANDLE, stop: &AtomicBool) {
    let mut next = Instant::now() + FIRST_CHECK;
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_secs(1));
        if Instant::now() < next {
            continue;
        }
        next = Instant::now() + EVERY;
        match HostConfig::load() {
            Ok(cfg) if !cfg.auto_update => continue,
            Err(e) => {
                tracing::warn!("auto-update: cannot read the settings: {e:#}");
                continue;
            }
            Ok(_) => {}
        }
        if session_active(event) {
            tracing::info!("auto-update: a session is active, checking again later");
            next = Instant::now() + RETRY;
            continue;
        }
        match check_and_install(event, stop) {
            Ok(true) => return, // the installer is about to stop this service
            Ok(false) => {}
            Err(e) => {
                tracing::warn!("auto-update: {e:#}");
                next = Instant::now() + RETRY;
            }
        }
    }
}

/// `%ProgramData%\RemoteDesktopRider\updates`, writable only by SYSTEM and Administrators.
fn updates_dir() -> Result<PathBuf> {
    let base = HostConfig::dir()?.parent().context("state dir has no parent")?.join("updates");
    std::fs::create_dir_all(&base)?;
    crate::system::secure_dir(&base)?;
    Ok(base)
}

/// Marker: the app was open in this console session before an update.
fn relaunch_marker() -> Result<PathBuf> {
    Ok(updates_dir()?.join("reopen-app"))
}

fn check_and_install(event: HANDLE, stop: &AtomicBool) -> Result<bool> {
    // An installer still running at the last service start could not be removed then.
    remove_installers();
    let Some(release) = update::newer()? else {
        tracing::debug!("auto-update: up to date ({})", update::Version::current());
        return Ok(false);
    };
    tracing::info!("auto-update: version {} is available, downloading", release.version);
    let dir = updates_dir()?;
    let file = dir.join(&release.file_name);
    update::download(&release, &file, &mut |_, _| {})?;
    if stop.load(Ordering::SeqCst) || session_active(event) {
        tracing::info!("auto-update: postponed (stopping or a session started)");
        let _ = std::fs::remove_file(&file);
        anyhow::bail!("postponed");
    }
    // The installer closes the tray app in every session; remember where it
    // was open so the restarted service can bring it back.
    // SAFETY: no arguments.
    let console = unsafe { windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId() };
    let marker = relaunch_marker()?;
    if app_running_in(console) {
        std::fs::write(&marker, console.to_string())?;
    } else {
        let _ = std::fs::remove_file(&marker);
    }
    tracing::info!("auto-update: installing {}", release.version);
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    std::process::Command::new(&file)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
        .arg(format!("/LOG={}", dir.join("install.log").display()))
        .creation_flags(DETACHED_PROCESS)
        .spawn()
        .context("cannot start the installer")?;
    Ok(true)
}

/// Is the tray app running in `session`?
fn app_running_in(session: u32) -> bool {
    let mut list: *mut WTS_PROCESS_INFOW = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: WTS allocates `list` (freed below); entries are valid for `count`.
    unsafe {
        if WTSEnumerateProcessesW(Some(WTS_CURRENT_SERVER_HANDLE), 0, 1, &mut list, &mut count).is_err() {
            return false;
        }
        let found = std::slice::from_raw_parts(list, count as usize).iter().any(|p| {
            p.SessionId == session && p.pProcessName.to_string().is_ok_and(|n| n.eq_ignore_ascii_case(APP_EXE))
        });
        WTSFreeMemory(list.cast());
        found
    }
}

/// Installers of earlier updates are no longer needed (the log stays).
fn remove_installers() {
    if let Ok(dir) = updates_dir()
        && let Ok(entries) = std::fs::read_dir(&dir)
    {
        for e in entries.flatten() {
            if e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// After an update: reopen the tray app for the user it was closed for.
/// Called by the service when it starts.
pub fn reopen_app_after_update() {
    remove_installers();
    let Ok(marker) = relaunch_marker() else { return };
    let Ok(text) = std::fs::read_to_string(&marker) else { return };
    let _ = std::fs::remove_file(&marker);
    let Ok(session) = text.trim().parse::<u32>() else { return };
    // SAFETY: no arguments.
    if unsafe { windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId() } != session {
        return; // someone else signed in meanwhile; the app starts at their sign-in
    }
    let Some(exe) = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join(APP_EXE))) else { return };
    match launch_as_user(session, &format!("\"{}\" --tray", exe.display())) {
        Ok(()) => tracing::info!("auto-update: reopened the app in session {session}"),
        Err(e) => tracing::warn!("auto-update: cannot reopen the app: {e:#}"),
    }
}

/// Start `cmd` as the user signed in to `session`, on their desktop.
fn launch_as_user(session: u32, cmd: &str) -> Result<()> {
    // SAFETY: handles and the environment block are released on every path;
    // the wide strings outlive the call that uses them.
    unsafe {
        let mut token = HANDLE::default();
        WTSQueryUserToken(session, &mut token).context("no user signed in")?;
        let mut env = std::ptr::null_mut();
        let have_env = CreateEnvironmentBlock(&mut env, Some(token), false).is_ok();
        let mut desktop: Vec<u16> = "winsta0\\default\0".encode_utf16().collect();
        let si = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop.as_mut_ptr()),
            ..Default::default()
        };
        let mut line: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
        let mut pi = PROCESS_INFORMATION::default();
        let r = windows::Win32::System::Threading::CreateProcessAsUserW(
            Some(token),
            PCWSTR::null(),
            Some(PWSTR(line.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT,
            have_env.then_some(env as *const _),
            PCWSTR::null(),
            &si,
            &mut pi,
        );
        if have_env {
            let _ = DestroyEnvironmentBlock(env);
        }
        let _ = CloseHandle(token);
        r?;
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);
        Ok(())
    }
}

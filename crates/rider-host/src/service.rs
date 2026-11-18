//! Unattended access: a Windows service supervising the host agent.
//!
//! Services live in session 0, which has no access to the user's screen. The
//! service therefore launches `rider-host agent` as SYSTEM *inside the
//! active console session* and relaunches it whenever that session changes
//! (logon, logoff, fast user switching) or the agent exits. Running as SYSTEM
//! lets the agent capture and control the login screen, lock screen and UAC
//! prompts. The agent is placed in a kill-on-close job, so it can never
//! outlive the service.

use crate::system;
use anyhow::{Context, Result, bail};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Security::{
    DuplicateTokenEx, SecurityImpersonation, SetTokenInformation, TOKEN_ALL_ACCESS, TokenPrimary, TokenSessionId,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateEventW, CreateProcessAsUserW, GetCurrentProcess, OpenProcessToken, PROCESS_INFORMATION,
    STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};
use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept, ServiceErrorControl,
    ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState,
    ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

pub const SERVICE_NAME: &str = "RemoteDesktopRider";
const DISPLAY_NAME: &str = "Remote Desktop Rider";
const FIREWALL_RULE: &str = "Remote Desktop Rider (UDP)";
const NO_SESSION: u32 = u32::MAX;
const RESPAWN_DELAY: Duration = Duration::from_secs(3);

fn install_dir() -> Result<PathBuf> {
    let pf = std::env::var_os("ProgramFiles").context("ProgramFiles is not set")?;
    Ok(PathBuf::from(pf).join("RemoteDesktopRider"))
}

fn manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
    ServiceManager::local_computer(None::<&str>, access).context("opening the service manager")
}

fn wait_for(service: &windows_service::service::Service, state: ServiceState) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while service.query_status()?.current_state != state {
        if Instant::now() > deadline {
            bail!("service did not reach {state:?} in time");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

/// Install (or upgrade) and start the service.
pub fn install(port: u16) -> Result<()> {
    system::require_elevated()?;
    let mgr = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
    let access = ServiceAccess::QUERY_STATUS
        | ServiceAccess::START
        | ServiceAccess::STOP
        | ServiceAccess::CHANGE_CONFIG
        | ServiceAccess::DELETE;

    // Stop a running older version so its binary can be replaced.
    let existing = mgr.open_service(SERVICE_NAME, access).ok();
    if let Some(s) = &existing
        && s.query_status()?.current_state != ServiceState::Stopped
    {
        let _ = s.stop();
        wait_for(s, ServiceState::Stopped)?;
    }

    // A SYSTEM service must run a binary that only administrators can
    // modify, otherwise any user could swap it and gain SYSTEM.
    let dir = install_dir()?;
    std::fs::create_dir_all(&dir)?;
    let exe = dir.join("rider-host.exe");
    let current = std::env::current_exe()?;
    if current != exe {
        replace_binary(&current, &exe)?;
    }

    let info = ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: DISPLAY_NAME.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.clone(),
        launch_arguments: vec!["service".into(), "run".into()],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let service = match existing {
        Some(s) => {
            s.change_config(&info)?;
            s
        }
        None => mgr.create_service(&info, access)?,
    };
    service.set_description("Unattended remote access for paired Remote Desktop Rider devices.")?;
    let restart = ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(5) };
    service.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 3600)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![restart.clone(), restart.clone(), restart]),
    })?;

    firewall_allow(&exe, port)?;
    if system::enable_software_sas()? {
        std::fs::write(crate::config::HostConfig::dir()?.join("sas-policy-set"), b"1")?;
    }
    service.start(&[] as &[&OsStr])?;
    wait_for(&service, ServiceState::Running)?;
    println!("Service installed and running.");
    println!("  binary:   {}", exe.display());
    println!("  firewall: inbound UDP {port} allowed for this binary");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    system::require_elevated()?;
    let mgr = manager(ServiceManagerAccess::CONNECT)?;
    match mgr.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE) {
        Ok(s) => {
            if s.query_status()?.current_state != ServiceState::Stopped {
                let _ = s.stop();
                wait_for(&s, ServiceState::Stopped)?;
            }
            s.delete()?;
        }
        Err(_) => println!("Service was not installed."),
    }
    firewall_remove();
    let marker = crate::config::HostConfig::dir()?.join("sas-policy-set");
    if marker.exists() {
        system::remove_software_sas();
        let _ = std::fs::remove_file(marker);
    }
    if let Ok(dir) = install_dir() {
        let _ = std::fs::remove_file(dir.join("rider-host.exe"));
        let _ = std::fs::remove_dir(dir);
    }
    println!("Service removed. Pairings and keys are kept in {}.", crate::config::HostConfig::dir()?.display());
    Ok(())
}

pub fn set_running(run: bool) -> Result<()> {
    system::require_elevated()?;
    let mgr = manager(ServiceManagerAccess::CONNECT)?;
    let s = mgr
        .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::START | ServiceAccess::STOP)
        .context("service is not installed (run `rider-host service install`)")?;
    if run {
        s.start(&[] as &[&OsStr])?;
        wait_for(&s, ServiceState::Running)
    } else {
        s.stop()?;
        wait_for(&s, ServiceState::Stopped)
    }
}

/// `Some(true)` running, `Some(false)` installed but not running, `None` not installed.
pub fn status() -> Option<bool> {
    let mgr = manager(ServiceManagerAccess::CONNECT).ok()?;
    let s = mgr.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS).ok()?;
    Some(s.query_status().ok()?.current_state == ServiceState::Running)
}

/// Stops the service for the lifetime of the guard (it holds the UDP port),
/// restarting it on drop. Used while pairing interactively.
pub struct Paused(bool);

impl Paused {
    pub fn service() -> Result<Self> {
        if status() == Some(true) {
            println!("Pausing the background service while pairing...");
            set_running(false)?;
            return Ok(Self(true));
        }
        Ok(Self(false))
    }
}

impl Drop for Paused {
    fn drop(&mut self) {
        if self.0 {
            match set_running(true) {
                Ok(()) => println!("Background service resumed."),
                Err(e) => eprintln!("Could not restart the service: {e:#}"),
            }
        }
    }
}

/// Copy `src` over `dst`, which may still be locked for a moment by exiting
/// service/agent processes. Falls back to renaming the old file aside, which
/// Windows allows even while it is running; leftovers are removed next time.
fn replace_binary(src: &Path, dst: &Path) -> Result<()> {
    if let Some(dir) = dst.parent()
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().ends_with(".old") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match std::fs::copy(src, dst) {
            Ok(_) => return Ok(()),
            Err(e) if e.raw_os_error() == Some(32) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(e) if e.raw_os_error() == Some(32) => {
                let aside = dst.with_extension(format!("{}.old", std::process::id()));
                std::fs::rename(dst, &aside).with_context(|| format!("{} is locked: {e}", dst.display()))?;
                std::fs::copy(src, dst).with_context(|| format!("copying binary to {}", dst.display()))?;
                return Ok(());
            }
            Err(e) => return Err(e).with_context(|| format!("copying binary to {}", dst.display())),
        }
    }
}

fn firewall_allow(exe: &Path, port: u16) -> Result<()> {
    firewall_remove();
    let status = std::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "add", "rule"])
        .arg(format!("name={FIREWALL_RULE}"))
        .args(["dir=in", "action=allow", "protocol=UDP", "profile=any"])
        .arg(format!("localport={port}"))
        .arg(format!("program={}", exe.display()))
        .stdout(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        bail!("adding the firewall rule failed");
    }
    Ok(())
}

fn firewall_remove() {
    let _ = std::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "delete", "rule"])
        .arg(format!("name={FIREWALL_RULE}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

// ---------------------------------------------------------------------------
// Service process (runs as LocalSystem in session 0).

windows_service::define_windows_service!(ffi_service_main, service_main);

/// Entry point for `rider-host service run`, invoked by the SCM only.
pub fn dispatch() -> Result<()> {
    windows_service::service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("not started by the service manager (use `rider-host service install`)")
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        tracing::error!("service failed: {e:#}");
    }
}

fn run_service() -> Result<()> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let status = service_control_handler::register(SERVICE_NAME, handler)?;
    let report = |state, accept| {
        status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(5),
            process_id: None,
        })
    };
    report(ServiceState::Running, ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN)?;
    tracing::info!("service started");
    let result = supervise(&stop_rx);
    report(ServiceState::Stopped, ServiceControlAccept::empty())?;
    tracing::info!("service stopped");
    result
}

struct Agent {
    process: HANDLE,
    session: u32,
}

impl Agent {
    fn alive(&self) -> bool {
        // SAFETY: valid process handle owned by `self`.
        unsafe { WaitForSingleObject(self.process, 0) == WAIT_TIMEOUT }
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        // SAFETY: valid process handle owned by `self`, closed exactly once.
        unsafe {
            let _ = TerminateProcess(self.process, 0);
            // Termination is asynchronous; wait so the binary is unlocked
            // by the time the service reports Stopped (upgrades replace it).
            let _ = WaitForSingleObject(self.process, 5000);
            let _ = CloseHandle(self.process);
        }
    }
}

fn supervise(stop: &mpsc::Receiver<()>) -> Result<()> {
    let job = kill_on_close_job()?;
    // Auto-reset; the default DACL of a SYSTEM process admits only SYSTEM and Administrators.
    // SAFETY: valid name, no security attributes.
    let sas = unsafe { CreateEventW(None, false, false, system::SAS_EVENT)? };
    let exe = std::env::current_exe()?;
    let mut agent: Option<Agent> = None;
    let mut last_spawn = Instant::now() - RESPAWN_DELAY;
    loop {
        // SAFETY: no arguments.
        let session = unsafe { WTSGetActiveConsoleSessionId() };
        let healthy = agent.as_ref().is_some_and(|a| a.alive() && a.session == session);
        if session != NO_SESSION && !healthy && last_spawn.elapsed() >= RESPAWN_DELAY {
            if let Some(old) = agent.take() {
                tracing::info!("restarting agent (session {} -> {session})", old.session);
            }
            last_spawn = Instant::now();
            match spawn_agent(&exe, session, job) {
                Ok(process) => {
                    tracing::info!("agent started in session {session}");
                    agent = Some(Agent { process, session });
                }
                Err(e) => tracing::error!("cannot start agent in session {session}: {e:#}"),
            }
        }
        // Wake promptly for Ctrl+Alt+Del requests from the agent.
        // SAFETY: valid event handle owned by this function.
        if unsafe { WaitForSingleObject(sas, 500) } == WAIT_OBJECT_0 {
            match system::send_sas_direct() {
                Ok(()) => tracing::info!("Ctrl+Alt+Del sent"),
                Err(e) => tracing::warn!("Ctrl+Alt+Del failed: {e:#}"),
            }
        }
        match stop.try_recv() {
            Err(TryRecvError::Empty) => {}
            _ => break,
        }
    }
    drop(agent);
    // SAFETY: handles created above, closed once.
    unsafe {
        let _ = CloseHandle(sas);
        let _ = CloseHandle(job);
    }
    Ok(())
}

fn kill_on_close_job() -> Result<HANDLE> {
    // SAFETY: the info struct is fully initialised and sized correctly.
    unsafe {
        let job = CreateJobObjectW(None, PCWSTR::null())?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )?;
        Ok(job)
    }
}

/// Launch `exe agent` as SYSTEM in `session` on the interactive window station.
fn spawn_agent(exe: &Path, session: u32, job: HANDLE) -> Result<HANDLE> {
    // SAFETY: every handle is closed on all paths; the wide strings outlive
    // the CreateProcessAsUserW call.
    unsafe {
        let mut own = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut own)?;
        let mut token = HANDLE::default();
        let dup = DuplicateTokenEx(own, TOKEN_ALL_ACCESS, None, SecurityImpersonation, TokenPrimary, &mut token);
        let _ = CloseHandle(own);
        dup?;
        let result = (|| {
            SetTokenInformation(token, TokenSessionId, &session as *const u32 as _, 4)?;
            let mut desktop: Vec<u16> = "winsta0\\default\0".encode_utf16().collect();
            let si = STARTUPINFOW {
                cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                lpDesktop: PWSTR(desktop.as_mut_ptr()),
                ..Default::default()
            };
            let mut cmd: Vec<u16> = format!("\"{}\" agent\0", exe.display()).encode_utf16().collect();
            let mut pi = PROCESS_INFORMATION::default();
            CreateProcessAsUserW(
                Some(token),
                PCWSTR::null(),
                Some(PWSTR(cmd.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_NO_WINDOW,
                None,
                PCWSTR::null(),
                &si,
                &mut pi,
            )?;
            let _ = CloseHandle(pi.hThread);
            if let Err(e) = AssignProcessToJobObject(job, pi.hProcess) {
                let _ = TerminateProcess(pi.hProcess, 1);
                let _ = CloseHandle(pi.hProcess);
                return Err(e.into());
            }
            anyhow::Ok(pi.hProcess)
        })();
        let _ = CloseHandle(token);
        result
    }
}

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

pub fn install(port: u16) -> Result<()> {
    system::require_elevated()?;
    let mgr = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
    let access = ServiceAccess::QUERY_STATUS
        | ServiceAccess::START
        | ServiceAccess::STOP
        | ServiceAccess::CHANGE_CONFIG
        | ServiceAccess::DELETE;

    let existing = mgr.open_service(SERVICE_NAME, access).ok();
    if let Some(s) = &existing
        && s.query_status()?.current_state != ServiceState::Stopped
    {
        let _ = s.stop();
        wait_for(s, ServiceState::Stopped)?;
    }

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
        account_name: None,
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

pub fn status() -> Option<bool> {
    let mgr = manager(ServiceManagerAccess::CONNECT).ok()?;
    let s = mgr.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS).ok()?;
    Some(s.query_status().ok()?.current_state == ServiceState::Running)
}

pub struct Paused(bool);

impl Paused {
    pub fn service() -> Result<Self> {
        if status() == Some(true) {
            println!("Pausing the background service while pairing...");
            set_running(false)?;
            resume_when_gone(std::process::id());
            return Ok(Self(true));
        }
        Ok(Self(false))
    }
}

fn resume_when_gone(pid: u32) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let spawn = |flags: u32| -> std::io::Result<()> {
        std::process::Command::new(std::env::current_exe()?)
            .args(["service", "resume-after", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(flags | DETACHED_PROCESS | CREATE_NO_WINDOW.0)
            .spawn()
            .map(drop)
    };
    if let Err(e) = spawn(CREATE_BREAKAWAY_FROM_JOB).or_else(|_| spawn(0)) {
        tracing::warn!("cannot start the service watchdog: {e}");
    }
}

pub fn resume_after(pid: u32) -> Result<()> {
    use windows::Win32::System::Threading::{INFINITE, OpenProcess, PROCESS_SYNCHRONIZE};
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            WaitForSingleObject(h, INFINITE);
            let _ = CloseHandle(h);
        }
    }
    if status() == Some(false) {
        tracing::info!("pairing ended without restarting the service: starting it");
        set_running(true)?;
    }
    Ok(())
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

windows_service::define_windows_service!(ffi_service_main, service_main);

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
    crate::autoupdate::reopen_app_after_update();
    let updater =
        crate::autoupdate::Updater::start().inspect_err(|e| tracing::warn!("auto-update unavailable: {e:#}")).ok();
    let result = supervise(&stop_rx, updater.as_ref());
    drop(updater);
    report(ServiceState::Stopped, ServiceControlAccept::empty())?;
    tracing::info!("service stopped");
    result
}

struct Agent {
    process: HANDLE,
    session: u32,
    job: HANDLE,
}

impl Agent {
    fn alive(&self) -> bool {
        unsafe { WaitForSingleObject(self.process, 0) == WAIT_TIMEOUT }
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        unsafe {
            let _ = TerminateProcess(self.process, 0);
            let _ = WaitForSingleObject(self.process, 5000);
            let _ = CloseHandle(self.process);
            let _ = CloseHandle(self.job);
        }
    }
}

fn supervise(stop: &mpsc::Receiver<()>, updater: Option<&crate::autoupdate::Updater>) -> Result<()> {
    let sas = unsafe { CreateEventW(None, false, false, system::SAS_EVENT)? };
    let exe = std::env::current_exe()?;
    let mut agent: Option<Agent> = None;
    let mut last_spawn = Instant::now() - RESPAWN_DELAY;
    loop {
        let session = unsafe { WTSGetActiveConsoleSessionId() };
        let healthy = agent.as_ref().is_some_and(|a| a.alive() && a.session == session);
        if session != NO_SESSION && !healthy && last_spawn.elapsed() >= RESPAWN_DELAY {
            if let Some(old) = agent.take() {
                tracing::info!("restarting agent (session {} -> {session})", old.session);
            }
            last_spawn = Instant::now();
            if let Some(u) = updater {
                u.reset_session();
            }
            match spawn_agent(&exe, session) {
                Ok((process, job)) => {
                    tracing::info!("agent started in session {session}");
                    agent = Some(Agent { process, session, job });
                }
                Err(e) => tracing::error!("cannot start agent in session {session}: {e:#}"),
            }
        }
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
    unsafe {
        let _ = CloseHandle(sas);
    }
    Ok(())
}

fn kill_on_close_job() -> Result<HANDLE> {
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

fn spawn_agent(exe: &Path, session: u32) -> Result<(HANDLE, HANDLE)> {
    let job = kill_on_close_job().context("job")?;
    match own_token_in(session).and_then(|token| start_agent(exe, token, job)) {
        Ok(process) => Ok((process, job)),
        Err(e) => {
            unsafe {
                let _ = CloseHandle(job);
            }
            Err(e)
        }
    }
}

fn own_token_in(session: u32) -> Result<HANDLE> {
    unsafe {
        let mut own = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut own).context("own token")?;
        let mut token = HANDLE::default();
        let dup = DuplicateTokenEx(own, TOKEN_ALL_ACCESS, None, SecurityImpersonation, TokenPrimary, &mut token);
        let _ = CloseHandle(own);
        dup.context("copy of own token")?;
        if let Err(e) = SetTokenInformation(token, TokenSessionId, &session as *const u32 as _, 4) {
            let _ = CloseHandle(token);
            return Err(anyhow::Error::from(e).context("token into the session"));
        }
        Ok(token)
    }
}

fn start_agent(exe: &Path, token: HANDLE, job: HANDLE) -> Result<HANDLE> {
    unsafe {
        let result = (|| {
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
            )
            .context("start the agent")?;
            let _ = CloseHandle(pi.hThread);
            if let Err(e) = AssignProcessToJobObject(job, pi.hProcess) {
                let _ = TerminateProcess(pi.hProcess, 1);
                let _ = CloseHandle(pi.hProcess);
                return Err(anyhow::Error::from(e).context("into the job"));
            }
            anyhow::Ok(pi.hProcess)
        })();
        let _ = CloseHandle(token);
        result
    }
}

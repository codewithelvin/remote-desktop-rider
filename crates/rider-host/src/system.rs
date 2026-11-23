//! Windows system integration: elevation, protected storage, the input
//! desktop (login screen / UAC), the logged-in user's profile, and Ctrl+Alt+Del.

use anyhow::{Context, Result, bail};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, GENERIC_ALL, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
    SetNamedSecurityInfoW,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, ImpersonateLoggedOnUser, IsWellKnownSid,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, RevertToSelf,
    WinBuiltinAdministratorsSid, WinLocalSystemSid,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, REG_DWORD, RRF_RT_REG_DWORD, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, HDESK, OpenInputDesktop, SetThreadDesktop,
};
use windows::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
use windows::Win32::UI::Shell::{GetUserProfileDirectoryW, IsUserAnAdmin};
use windows::core::{BOOL, HSTRING, PWSTR, s, w};

/// True for an elevated administrator or SYSTEM.
pub fn is_elevated() -> bool {
    // SAFETY: no arguments; reads the current process token.
    unsafe { IsUserAnAdmin().as_bool() }
}

pub fn require_elevated() -> Result<()> {
    if !is_elevated() {
        bail!("this command needs administrator rights. Open an elevated terminal (Run as administrator).");
    }
    Ok(())
}

/// Owner SYSTEM/Administrators only; children inherit. Protected = parent
/// ACEs (e.g. "Users: read" from ProgramData) are dropped.
const PROTECTED_SDDL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";

/// Create `path` (and parents) and lock it down to SYSTEM + Administrators.
///
/// Refuses directories owned by anyone else: a standard user could have
/// pre-created the folder ("squatting") to plant a trust store or key.
pub fn secure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
    let wide = HSTRING::from(path.as_os_str());
    // SAFETY: all out-pointers are valid locals; buffers allocated by the OS
    // are released with LocalFree exactly once.
    unsafe {
        let mut owner = PSID::default();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        let err = GetNamedSecurityInfoW(
            &wide,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            &mut sd,
        );
        if err != ERROR_SUCCESS {
            bail!("reading security of {}: {err:?}", path.display());
        }
        let trusted_owner = IsWellKnownSid(owner, WinLocalSystemSid).as_bool()
            || IsWellKnownSid(owner, WinBuiltinAdministratorsSid).as_bool();
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        if !trusted_owner {
            bail!(
                "{} is not owned by SYSTEM or Administrators and cannot be trusted. Delete it and run again.",
                path.display()
            );
        }

        let mut sd = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            &HSTRING::from(PROTECTED_SDDL),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )?;
        let mut present = BOOL(0);
        let mut defaulted = BOOL(0);
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let result = GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted).and_then(|()| {
            SetNamedSecurityInfoW(
                &wide,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(dacl),
                None,
            )
            .ok()
        });
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        result.with_context(|| format!("protecting {}", path.display()))?;
    }
    Ok(())
}

thread_local! {
    static THREAD_DESKTOP: Cell<isize> = const { Cell::new(0) };
}

/// Move the calling thread onto the desktop currently receiving input
/// (Default, or Winlogon for the login screen / UAC / lock screen).
///
/// Capture and SendInput only work on the input desktop. Only a SYSTEM
/// process can open Winlogon; for normal users this is a no-op there.
/// The thread must not own windows or hooks.
pub fn follow_input_desktop() -> bool {
    // SAFETY: the handle is kept open while it is the thread's desktop and
    // the previously attached handle is closed only after switching away.
    unsafe {
        let Ok(desk) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(GENERIC_ALL.0)) else {
            return false;
        };
        if SetThreadDesktop(desk).is_err() {
            let _ = CloseDesktop(desk);
            return false;
        }
        let previous = THREAD_DESKTOP.with(|d| d.replace(desk.0 as isize));
        if previous != 0 {
            let _ = CloseDesktop(HDESK(previous as _));
        }
        true
    }
}

/// `Downloads\Rider` of the user logged on at the console.
///
/// When running as the SYSTEM agent this resolves the interactive user's
/// profile; with nobody logged in, incoming files are refused.
pub fn incoming_files_dir() -> Result<PathBuf> {
    // SAFETY: token handle closed below; the buffer length comes from the first call.
    let profile = unsafe {
        let mut token = HANDLE::default();
        if WTSQueryUserToken(WTSGetActiveConsoleSessionId(), &mut token).is_ok() {
            let mut len = 0u32;
            let _ = GetUserProfileDirectoryW(token, None, &mut len);
            let mut buf = vec![0u16; len as usize];
            let r = GetUserProfileDirectoryW(token, Some(PWSTR(buf.as_mut_ptr())), &mut len);
            let _ = CloseHandle(token);
            r?;
            PathBuf::from(String::from_utf16_lossy(&buf[..len.saturating_sub(1) as usize]))
        } else {
            // Not SYSTEM (interactive host): our own profile is the user's.
            if is_system_profile() {
                bail!("no user is logged in on the host");
            }
            PathBuf::from(std::env::var_os("USERPROFILE").context("USERPROFILE not set")?)
        }
    };
    Ok(profile.join("Downloads").join("Rider"))
}

/// Run `f` on this thread as the signed-in console user when this process is
/// SYSTEM (the service's agent); otherwise it already is that user.
///
/// For files in the user's own folders: the user can turn any of them into a
/// junction to e.g. `C:\Windows`, so creating or deleting files there as
/// SYSTEM would let a remote session (or local user) write anywhere. As the
/// user, such a write gets only the user's rights.
pub fn as_console_user<T>(f: impl FnOnce() -> T) -> Result<T> {
    let mut token = HANDLE::default();
    // SAFETY: plain query into a local; fails unless we hold SeTcbPrivilege (SYSTEM).
    if unsafe { WTSQueryUserToken(WTSGetActiveConsoleSessionId(), &mut token) }.is_err() {
        if is_system_profile() {
            bail!("no user is signed in on the host");
        }
        return Ok(f());
    }
    /// Reverts the thread and closes the token, also if `f` panics.
    struct Guard(HANDLE);
    impl Drop for Guard {
        fn drop(&mut self) {
            // SAFETY: undoes the impersonation below on this thread; closes our token.
            unsafe {
                let _ = RevertToSelf();
                let _ = CloseHandle(self.0);
            }
        }
    }
    let guard = Guard(token);
    // SAFETY: a valid primary token of the console user; reverted by `guard`.
    unsafe { ImpersonateLoggedOnUser(guard.0) }.context("acting as the signed-in user")?;
    Ok(f())
}

fn is_system_profile() -> bool {
    std::env::var_os("USERPROFILE").is_some_and(|p| p.to_string_lossy().to_lowercase().contains("systemprofile"))
}

/// Named event the service waits on to generate Ctrl+Alt+Del for the agent.
/// Created by the service (SYSTEM), so only SYSTEM and Administrators can signal it.
pub const SAS_EVENT: windows::core::PCWSTR = w!(r"Global\RemoteDesktopRider-SAS");

/// Ctrl+Alt+Del (secure attention sequence) on the host.
///
/// Windows honours `SendSAS` only when called from a service, so the agent
/// asks the service through [`SAS_EVENT`]; without the service it is tried
/// directly.
pub fn send_sas() -> Result<()> {
    // SAFETY: valid event name; the handle is closed after use.
    unsafe {
        if let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, SAS_EVENT) {
            let r = SetEvent(event);
            let _ = CloseHandle(event);
            return r.context("signalling the service");
        }
    }
    send_sas_direct()
}

/// Call `SendSAS` in this process (effective only inside a service).
pub fn send_sas_direct() -> Result<()> {
    // SAFETY: SendSAS has signature `void SendSAS(BOOL AsUser)`; the symbol is
    // looked up by name from the system's sas.dll.
    unsafe {
        let lib = LoadLibraryW(w!("sas.dll")).context("sas.dll not available")?;
        let f = GetProcAddress(lib, s!("SendSAS")).context("SendSAS not found")?;
        let send: extern "system" fn(BOOL) = std::mem::transmute(f);
        send(BOOL(0));
    }
    Ok(())
}

const POLICY_KEY: windows::core::PCWSTR = w!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System");
const SAS_VALUE: windows::core::PCWSTR = w!("SoftwareSASGeneration");

/// Allow services to generate Ctrl+Alt+Del (policy value 1) if no policy is
/// configured. Returns true if we created the value (so uninstall removes it).
pub fn enable_software_sas() -> Result<bool> {
    // SAFETY: valid registry key/value names and a 4-byte DWORD buffer.
    unsafe {
        let mut data = 0u32;
        let mut size = 4u32;
        let existing = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            POLICY_KEY,
            SAS_VALUE,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as _),
            Some(&mut size),
        );
        if existing == ERROR_SUCCESS {
            return Ok(false); // respect an existing policy
        }
        let one = 1u32;
        let err =
            RegSetKeyValueW(HKEY_LOCAL_MACHINE, POLICY_KEY, SAS_VALUE, REG_DWORD.0, Some(&one as *const u32 as _), 4);
        if err != ERROR_SUCCESS {
            bail!("setting SoftwareSASGeneration policy: {err:?}");
        }
        Ok(true)
    }
}

pub fn remove_software_sas() {
    // SAFETY: valid registry key/value names.
    unsafe {
        let _ = RegDeleteKeyValueW(HKEY_LOCAL_MACHINE, POLICY_KEY, SAS_VALUE);
    }
}

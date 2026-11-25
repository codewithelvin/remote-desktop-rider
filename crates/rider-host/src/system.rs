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

pub fn is_elevated() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}

pub fn require_elevated() -> Result<()> {
    if !is_elevated() {
        bail!("this command needs administrator rights. Open an elevated terminal (Run as administrator).");
    }
    Ok(())
}

const PROTECTED_SDDL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";

pub fn secure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
    let wide = HSTRING::from(path.as_os_str());
    unsafe {
        let mut owner = PSID::default();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        let err = GetNamedSecurityInfoW(
            &wide,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&raw mut owner),
            None,
            None,
            None,
            &raw mut sd,
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
            &raw mut sd,
            None,
        )?;
        let mut present = BOOL(0);
        let mut defaulted = BOOL(0);
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let result =
            GetSecurityDescriptorDacl(sd, &raw mut present, &raw mut dacl, &raw mut defaulted).and_then(|()| {
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

pub fn follow_input_desktop() -> bool {
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

pub fn incoming_files_dir() -> Result<PathBuf> {
    let profile = unsafe {
        let mut token = HANDLE::default();
        if WTSQueryUserToken(WTSGetActiveConsoleSessionId(), &raw mut token).is_ok() {
            let mut len = 0u32;
            let _ = GetUserProfileDirectoryW(token, None, &raw mut len);
            let mut buf = vec![0u16; len as usize];
            let r = GetUserProfileDirectoryW(token, Some(PWSTR(buf.as_mut_ptr())), &raw mut len);
            let _ = CloseHandle(token);
            r?;
            PathBuf::from(String::from_utf16_lossy(&buf[..len.saturating_sub(1) as usize]))
        } else {
            if is_system_profile() {
                bail!("no user is logged in on the host");
            }
            PathBuf::from(std::env::var_os("USERPROFILE").context("USERPROFILE not set")?)
        }
    };
    Ok(profile.join("Downloads").join("Rider"))
}

struct Impersonation(HANDLE);

impl Drop for Impersonation {
    fn drop(&mut self) {
        unsafe {
            let _ = RevertToSelf();
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn as_console_user<T>(f: impl FnOnce() -> T) -> Result<T> {
    let mut token = HANDLE::default();
    if unsafe { WTSQueryUserToken(WTSGetActiveConsoleSessionId(), &raw mut token) }.is_err() {
        if is_system_profile() {
            bail!("no user is signed in on the host");
        }
        return Ok(f());
    }
    let guard = Impersonation(token);
    unsafe { ImpersonateLoggedOnUser(guard.0) }.context("acting as the signed-in user")?;
    Ok(f())
}

pub(crate) fn is_system_profile() -> bool {
    std::env::var_os("USERPROFILE").is_some_and(|p| p.to_string_lossy().to_lowercase().contains("systemprofile"))
}

pub const SAS_EVENT: windows::core::PCWSTR = w!(r"Global\RemoteDesktopRider-SAS");

pub fn send_sas() -> Result<()> {
    unsafe {
        if let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, SAS_EVENT) {
            let r = SetEvent(event);
            let _ = CloseHandle(event);
            return r.context("signalling the service");
        }
    }
    send_sas_direct()
}

pub fn send_sas_direct() -> Result<()> {
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

pub fn enable_software_sas() -> Result<bool> {
    unsafe {
        let mut data = 0u32;
        let mut size = 4u32;
        let existing = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            POLICY_KEY,
            SAS_VALUE,
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut data).cast()),
            Some(&raw mut size),
        );
        if existing == ERROR_SUCCESS {
            return Ok(false);
        }
        let one = 1u32;
        let err =
            RegSetKeyValueW(HKEY_LOCAL_MACHINE, POLICY_KEY, SAS_VALUE, REG_DWORD.0, Some((&raw const one).cast()), 4);
        if err != ERROR_SUCCESS {
            bail!("setting SoftwareSASGeneration policy: {err:?}");
        }
        Ok(true)
    }
}

pub fn remove_software_sas() {
    unsafe {
        let _ = RegDeleteKeyValueW(HKEY_LOCAL_MACHINE, POLICY_KEY, SAS_VALUE);
    }
}

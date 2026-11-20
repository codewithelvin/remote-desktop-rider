//! Notification-area (tray) icon, in plain Win32: a hidden window on the UI
//! thread receives the icon's clicks (the event loop already dispatches its
//! messages), shows the menu, and keeps the tooltip's remote-access state fresh.
//! Closing the main window only hides it; Quit in the menu exits.

use crate::sys::{self, RemoteAccess};
use anyhow::{Result, bail};
use rider_core::i18n::tr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, w};

const CLASS: PCWSTR = w!("RemoteDesktopRider.Tray");
/// Title of the main window (also what a second instance looks for).
pub const MAIN_TITLE: &str = "Remote Desktop Rider";
const WM_TRAY: u32 = WM_APP + 1;
/// Posted by a second instance: show the window.
const WM_SHOW: u32 = WM_APP + 2;
/// Posted by the installer before replacing files: exit (0x8003, keep in sync
/// with packaging/windows/RemoteDesktopRider.iss).
const WM_QUIT_APP: u32 = WM_APP + 3;
const ID_OPEN: usize = 1;
const ID_MANAGE: usize = 2;
const ID_QUIT: usize = 3;
const ID_UPDATE: usize = 4;
/// The user clicked one of the icon's notifications.
const NIN_BALLOONUSERCLICK: u32 = WM_USER + 5;
const TIMER_STATUS: usize = 1;

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
/// The tray window, for announcements from other threads.
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
/// A newer version found by the update check ("0.1.8").
static UPDATE: Mutex<Option<String>> = Mutex::new(None);
/// "Update to ..." was chosen in the menu; the UI picks this up.
static INSTALL_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Announce a newer version: one notification per version, and an "Update
/// to ..." item in the menu. Callable from any thread.
pub fn announce_update(version: &str) {
    {
        let mut u = UPDATE.lock().unwrap();
        if u.as_deref() == Some(version) {
            return;
        }
        *u = Some(version.to_owned());
    }
    let hwnd = TRAY_HWND.load(Ordering::Relaxed);
    if hwnd == 0 {
        return;
    }
    let mut nid = base(HWND(hwnd as *mut _));
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO;
    copy(
        &mut nid.szInfoTitle,
        &rider_core::trf!("Remote Desktop Rider {version} is available", "Remote Desktop Rider {version} mövcuddur"),
    );
    copy(
        &mut nid.szInfo,
        tr(
            "Click here or choose Update in the tray menu to install it.",
            "Quraşdırmaq üçün bura klikləyin və ya bildiriş sahəsindəki menyudan yeniləməni seçin.",
        ),
    );
    // SAFETY: nid is fully initialised and refers to our icon; Shell_NotifyIconW
    // may be called from any thread.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Whether "Update to ..." was chosen since the last call.
pub fn take_install_request() -> bool {
    INSTALL_REQUESTED.swap(false, Ordering::Relaxed)
}

fn request_install() {
    INSTALL_REQUESTED.store(true, Ordering::Relaxed);
    show_main(); // shows the download progress
}

pub struct Tray {
    hwnd: HWND,
}

impl std::fmt::Debug for Tray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tray")
    }
}

impl Tray {
    /// Call on the UI thread (its message loop delivers the icon's events).
    pub fn create() -> Result<Self> {
        // SAFETY: plain Win32 calls with valid, 'static class/title strings;
        // the window lives on this thread until `Drop`.
        unsafe {
            let hinst = GetModuleHandleW(None)?;
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst.into(),
                lpszClassName: CLASS,
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                bail!("cannot register the tray window class");
            }
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS,
                w!("Remote Desktop Rider tray"),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinst.into()),
                None,
            )?;
            TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
            TRAY_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
            add_icon(hwnd);
            SetTimer(Some(hwnd), TIMER_STATUS, 5000, None);
            Ok(Self { hwnd })
        }
    }

    /// A one-off notification from the icon.
    pub fn notify(&self, title: &str, text: &str) {
        let mut nid = base(self.hwnd);
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_INFO;
        copy(&mut nid.szInfoTitle, title);
        copy(&mut nid.szInfo, text);
        // SAFETY: nid is fully initialised and refers to our icon.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        remove_icon(self.hwnd);
        // SAFETY: our own window, destroyed on the thread that created it.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// If another instance runs, bring its window up and return true.
pub fn activate_other_instance() -> bool {
    // SAFETY: FindWindowW/PostMessageW with valid strings; no pointers kept.
    unsafe {
        let Ok(other) = FindWindowW(CLASS, None) else { return false };
        // Let the other process take the foreground (we got it from the user).
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        PostMessageW(Some(other), WM_SHOW, WPARAM(0), LPARAM(0)).is_ok()
    }
}

pub fn hide_main() {
    if let Some(h) = main_window() {
        // SAFETY: a top-level window of this thread.
        unsafe {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
}

/// For a hidden start: eframe shows the window after its first frame, so it
/// starts off-screen; hide it and centre it on the work area for later.
pub fn hide_main_centered() {
    let Some(h) = main_window() else { return };
    // SAFETY: a top-level window of this thread; out-params are local.
    unsafe {
        let mut win = RECT::default();
        let mut work = RECT::default();
        let _ = GetWindowRect(h, &mut win);
        let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut work as *mut RECT as *mut _), Default::default());
        let x = work.left + ((work.right - work.left) - (win.right - win.left)).max(0) / 2;
        let y = work.top + ((work.bottom - work.top) - (win.bottom - win.top)).max(0) / 2;
        let _ = SetWindowPos(h, None, x, y, 0, 0, SWP_HIDEWINDOW | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

pub fn show_main() {
    if let Some(h) = main_window() {
        // SAFETY: a top-level window of this thread.
        unsafe {
            let _ = ShowWindow(h, if IsIconic(h).as_bool() { SW_RESTORE } else { SW_SHOW });
            let _ = SetForegroundWindow(h);
        }
    }
}

/// The app's main window: the top-level window of this (UI) thread titled
/// [`MAIN_TITLE`]; hidden windows included.
fn main_window() -> Option<HWND> {
    unsafe extern "system" fn each(h: HWND, found: LPARAM) -> BOOL {
        let mut buf = [0u16; 64];
        // SAFETY: buf is writable for its length; `found` points to the
        // Option below, alive for the whole enumeration.
        unsafe {
            let n = GetWindowTextW(h, &mut buf) as usize;
            if String::from_utf16_lossy(&buf[..n]) == MAIN_TITLE {
                *(found.0 as *mut Option<HWND>) = Some(h);
                return BOOL(0);
            }
        }
        BOOL(1)
    }
    let mut found: Option<HWND> = None;
    // SAFETY: the callback only writes through the pointer we pass.
    unsafe {
        let _ = EnumThreadWindows(GetCurrentThreadId(), Some(each), LPARAM(&mut found as *mut _ as isize));
    }
    found
}

fn base(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: 1, ..Default::default() }
}

/// NUL-terminated UTF-16 for menu items.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn copy<const N: usize>(dst: &mut [u16; N], s: &str) {
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(N - 1).chain(std::iter::once(0))) {
        *d = c;
    }
}

fn status_text() -> &'static str {
    match sys::remote_access() {
        RemoteAccess::On => tr("Remote access: on", "Uzaqdan giriş: aktivdir"),
        RemoteAccess::Stopped => tr("Remote access: stopped", "Uzaqdan giriş: dayandırılıb"),
        RemoteAccess::Off => tr("Remote access: off", "Uzaqdan giriş: söndürülüb"),
    }
}

fn tooltip(nid: &mut NOTIFYICONDATAW) {
    nid.uFlags |= NIF_TIP;
    copy(&mut nid.szTip, &format!("Remote Desktop Rider\n{}", status_text()));
}

fn add_icon(hwnd: HWND) {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE;
    nid.uCallbackMessage = WM_TRAY;
    // SAFETY: icon resource 1 is embedded by build.rs; the stock icon is the fallback.
    nid.hIcon = unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        match LoadImageW(
            Some(hinst.into()),
            PCWSTR(std::ptr::without_provenance(1)), /* MAKEINTRESOURCE(1) */
            IMAGE_ICON,
            cx,
            cy,
            LR_DEFAULTCOLOR,
        ) {
            Ok(h) => HICON(h.0),
            Err(_) => LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
        }
    };
    tooltip(&mut nid);
    // SAFETY: nid is fully initialised.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

fn remove_icon(hwnd: HWND) {
    // SAFETY: deleting by window + id needs nothing else.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &base(hwnd));
    }
}

fn menu(hwnd: HWND) {
    // SAFETY: the menu is created, shown and destroyed here; strings are 'static
    // or outlive the calls.
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return };
        let status: Vec<u16> = status_text().encode_utf16().chain(std::iter::once(0)).collect();
        let update: Option<Vec<u16>> = UPDATE.lock().unwrap().as_ref().map(|v| {
            rider_core::trf!("Update to {v}", "{v} versiyasına yenilə")
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect()
        });
        let open = wide(tr("Open Remote Desktop Rider", "Remote Desktop Rider-i aç"));
        let manage = wide(tr("Manage this computer…", "Bu kompüteri idarə et…"));
        let quit_text = wide(tr("Quit", "Çıxış"));
        let _ = AppendMenuW(m, MF_STRING, ID_OPEN, PCWSTR(open.as_ptr()));
        if let Some(text) = &update {
            let _ = AppendMenuW(m, MF_STRING, ID_UPDATE, PCWSTR(text.as_ptr()));
        }
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(m, MF_STRING | MF_GRAYED, 0, PCWSTR(status.as_ptr()));
        let _ = AppendMenuW(m, MF_STRING, ID_MANAGE, PCWSTR(manage.as_ptr()));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(m, MF_STRING, ID_QUIT, PCWSTR(quit_text.as_ptr()));
        let _ = SetMenuDefaultItem(m, ID_OPEN as u32, 0);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Without this the menu does not close when clicking elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None).0 as usize;
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(m);
        match cmd {
            ID_OPEN => show_main(),
            ID_MANAGE => {
                let _ = sys::relaunch_elevated("--manage");
            }
            ID_QUIT => quit(hwnd),
            ID_UPDATE => request_install(),
            _ => {}
        }
    }
}

fn quit(hwnd: HWND) -> ! {
    // Settings are saved as they change and sessions are separate processes:
    // nothing else to wind down.
    remove_icon(hwnd);
    std::process::exit(0)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            match lparam.0 as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => show_main(),
                WM_RBUTTONUP | WM_CONTEXTMENU => menu(hwnd),
                NIN_BALLOONUSERCLICK if UPDATE.lock().unwrap().is_some() => request_install(),
                _ => {}
            }
            LRESULT(0)
        }
        WM_SHOW => {
            show_main();
            LRESULT(0)
        }
        WM_QUIT_APP => quit(hwnd),
        WM_TIMER => {
            let mut nid = base(hwnd);
            tooltip(&mut nid);
            // SAFETY: nid is fully initialised.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
            }
            LRESULT(0)
        }
        // Explorer restarted: the icon must be added again.
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            add_icon(hwnd);
            LRESULT(0)
        }
        // SAFETY: default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

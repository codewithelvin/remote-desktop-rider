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
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DestroyWindow, EnumThreadWindows, FindWindowW, GetCursorPos, GetSystemMetrics, GetWindowRect, GetWindowTextW,
    HICON, IDI_APPLICATION, IMAGE_ICON, IsIconic, LR_DEFAULTCOLOR, LoadIconW, LoadImageW, MF_GRAYED, MF_SEPARATOR,
    MF_STRING, PostMessageW, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON, SM_CYSMICON, SPI_GETWORKAREA,
    SW_HIDE, SW_RESTORE, SW_SHOW, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetForegroundWindow, SetMenuDefaultItem, SetTimer, SetWindowPos, ShowWindow,
    SystemParametersInfoW, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WINDOW_EX_STYLE, WM_APP, WM_CONTEXTMENU,
    WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_TIMER, WM_USER, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{BOOL, PCWSTR, w};

const CLASS: PCWSTR = w!("RemoteDesktopRider.Tray");
pub const MAIN_TITLE: &str = "Remote Desktop Rider";
const WM_TRAY: u32 = WM_APP + 1;
const WM_SHOW: u32 = WM_APP + 2;
const WM_QUIT_APP: u32 = WM_APP + 3;
const ID_OPEN: usize = 1;
const ID_MANAGE: usize = 2;
const ID_QUIT: usize = 3;
const ID_UPDATE: usize = 4;
const NIN_BALLOONUSERCLICK: u32 = WM_USER + 5;
const TIMER_STATUS: usize = 1;

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
static UPDATE: Mutex<Option<String>> = Mutex::new(None);
static INSTALL_REQUESTED: AtomicBool = AtomicBool::new(false);

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
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &raw const nid);
    }
}

pub fn take_install_request() -> bool {
    INSTALL_REQUESTED.swap(false, Ordering::Relaxed)
}

fn request_install() {
    INSTALL_REQUESTED.store(true, Ordering::Relaxed);
    show_main();
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
    pub fn create() -> Result<Self> {
        unsafe {
            let hinst = GetModuleHandleW(None)?;
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst.into(),
                lpszClassName: CLASS,
                ..Default::default()
            };
            if RegisterClassW(&raw const wc) == 0 {
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

    pub fn notify(&self, title: &str, text: &str) {
        let mut nid = base(self.hwnd);
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_INFO;
        copy(&mut nid.szInfoTitle, title);
        copy(&mut nid.szInfo, text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &raw const nid);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        remove_icon(self.hwnd);
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

pub fn activate_other_instance() -> bool {
    unsafe {
        let Ok(other) = FindWindowW(CLASS, None) else { return false };
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        PostMessageW(Some(other), WM_SHOW, WPARAM(0), LPARAM(0)).is_ok()
    }
}

pub fn hide_main() {
    if let Some(h) = main_window() {
        unsafe {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
}

pub fn hide_main_centered() {
    let Some(h) = main_window() else { return };
    unsafe {
        let mut win = RECT::default();
        let mut work = RECT::default();
        let _ = GetWindowRect(h, &raw mut win);
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&raw mut work).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
        );
        let x = work.left + ((work.right - work.left) - (win.right - win.left)).max(0) / 2;
        let y = work.top + ((work.bottom - work.top) - (win.bottom - win.top)).max(0) / 2;
        let _ = SetWindowPos(h, None, x, y, 0, 0, SWP_HIDEWINDOW | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

pub fn show_main() {
    if let Some(h) = main_window() {
        unsafe {
            let _ = ShowWindow(h, if IsIconic(h).as_bool() { SW_RESTORE } else { SW_SHOW });
            let _ = SetForegroundWindow(h);
        }
    }
}

fn main_window() -> Option<HWND> {
    unsafe extern "system" fn each(h: HWND, found: LPARAM) -> BOOL {
        let mut buf = [0u16; 64];
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
    unsafe {
        let _ = EnumThreadWindows(GetCurrentThreadId(), Some(each), LPARAM(&raw mut found as isize));
    }
    found
}

fn base(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: 1, ..Default::default() }
}

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
    nid.hIcon = unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        match LoadImageW(
            Some(hinst.into()),
            PCWSTR(std::ptr::without_provenance(1)),
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
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &raw const nid);
    }
}

fn remove_icon(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &base(hwnd));
    }
}

fn menu(hwnd: HWND) {
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
        let _ = GetCursorPos(&raw mut pt);
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
            unsafe {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &raw const nid);
            }
            LRESULT(0)
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            add_icon(hwnd);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

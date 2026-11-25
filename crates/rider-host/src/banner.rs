use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DT_CENTER, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FillRect,
    GetDC, GetTextExtentPoint32W, PAINTSTRUCT, ReleaseDC, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    GetSystemMetrics, GetWindowRect, HC_ACTION, IDC_HAND, LLMHF_INJECTED, LWA_ALPHA, LoadCursorW, MSG, MSLLHOOKSTRUCT,
    PostMessageW, PostQuitMessage, RegisterClassW, SM_CXSCREEN, SW_SHOWNOACTIVATE, SetLayeredWindowAttributes,
    SetWindowDisplayAffinity, SetWindowsHookExW, ShowWindow, TranslateMessage, UnhookWindowsHookEx,
    WDA_EXCLUDEFROMCAPTURE, WH_MOUSE_LL, WM_CLOSE, WM_DESTROY, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_PAINT, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE,
};
use windows::core::w;

static TEXT: Mutex<Vec<u16>> = Mutex::new(Vec::new());
static HWND_VAL: AtomicIsize = AtomicIsize::new(0);
pub static DISCONNECT_REQUESTED: AtomicBool = AtomicBool::new(false);

const WIDTH: i32 = 520;
const HEIGHT: i32 = 34;

pub struct Banner {
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Banner {
    pub fn show(text: &str) -> Self {
        DISCONNECT_REQUESTED.store(false, Ordering::SeqCst);
        *TEXT.lock().unwrap() = text.encode_utf16().collect();
        let thread = std::thread::Builder::new().name("rider-banner".into()).spawn(run).ok();
        Self { thread }
    }
}

impl Drop for Banner {
    fn drop(&mut self) {
        let h = HWND_VAL.swap(0, Ordering::SeqCst);
        if h != 0 {
            unsafe {
                let _ = PostMessageW(Some(HWND(h as _)), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run() {
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else { return };
        let class = w!("RiderSessionBanner");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&raw const wc);
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let width = {
            let text = TEXT.lock().unwrap().clone();
            let dc = GetDC(None);
            let mut size = SIZE::default();
            let _ = GetTextExtentPoint32W(dc, &text, &raw mut size);
            ReleaseDC(None, dc);
            (size.cx + 48).clamp(WIDTH, screen_w.max(WIDTH))
        };
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
            class,
            w!("Remote Desktop Rider"),
            WS_POPUP | WS_VISIBLE,
            (screen_w - width) / 2,
            0,
            width,
            HEIGHT,
            None,
            None,
            Some(instance.into()),
            None,
        ) else {
            return;
        };
        HWND_VAL.store(hwnd.0 as isize, Ordering::SeqCst);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(instance.into()), 0);
        if let Err(e) = &hook {
            tracing::warn!("banner mouse hook: {e}; the banner cannot be clicked");
        }

        let mut msg = MSG::default();
        while GetMessageW(&raw mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
        if let Ok(h) = hook {
            let _ = UnhookWindowsHookEx(h);
        }
    }
}

fn local_click_on(flags: u32, pt: POINT, banner: RECT) -> bool {
    flags & LLMHF_INJECTED == 0
        && pt.x >= banner.left
        && pt.x < banner.right
        && pt.y >= banner.top
        && pt.y < banner.bottom
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let msg = wparam.0 as u32;
    if code == HC_ACTION as i32 && (msg == WM_LBUTTONDOWN || msg == WM_LBUTTONUP) {
        let m = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let h = HWND_VAL.load(Ordering::SeqCst);
        let mut rc = RECT::default();
        if h != 0 && unsafe { GetWindowRect(HWND(h as _), &raw mut rc) }.is_ok() && local_click_on(m.flags, m.pt, rc) {
            if msg == WM_LBUTTONUP {
                DISCONNECT_REQUESTED.store(true, Ordering::SeqCst);
            }
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
fn rect() -> Option<RECT> {
    let h = HWND_VAL.load(Ordering::SeqCst);
    let mut rc = RECT::default();
    (h != 0 && unsafe { GetWindowRect(HWND(h as _), &raw mut rc) }.is_ok()).then_some(rc)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &raw mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &raw mut rc);
                let brush = CreateSolidBrush(COLORREF(0x002A_2AC8));
                FillRect(hdc, &raw const rc, brush);
                let _ = DeleteObject(brush.into());
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, COLORREF(0x00FF_FFFF));
                let mut text = TEXT.lock().unwrap().clone();
                DrawTextW(hdc, &mut text, &raw mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                let _ = EndPaint(hwnd, &raw const ps);
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::time::{Duration, Instant};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT, SendInput,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetCursorPos, GetWindowThreadProcessId, IsWindowVisible, LLMHF_LOWER_IL_INJECTED,
        PostThreadMessageW, SW_SHOW, SetCursorPos, WM_QUIT,
    };

    #[test]
    fn only_local_clicks_on_the_banner_count() {
        let rc = RECT { left: 100, top: 0, right: 620, bottom: 34 };
        let on = POINT { x: 300, y: 10 };
        assert!(local_click_on(0, on, rc), "the person at this computer clicks it");
        assert!(!local_click_on(LLMHF_INJECTED, on, rc), "the remote viewer's click passes through");
        assert!(!local_click_on(LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED, on, rc));
        assert!(!local_click_on(0, POINT { x: 99, y: 10 }, rc), "left of it");
        assert!(!local_click_on(0, POINT { x: 620, y: 10 }, rc), "right of it");
        assert!(!local_click_on(0, POINT { x: 300, y: 34 }, rc), "below it");
    }

    static UNDER_CLICKS: AtomicU32 = AtomicU32::new(0);

    unsafe extern "system" fn under_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_LBUTTONUP {
            UNDER_CLICKS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(3);
        while !ok() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    #[ignore = "needs an interactive desktop; moves the pointer"]
    fn remote_click_passes_through_to_the_window_underneath() {
        let under = std::thread::spawn(|| unsafe {
            let instance = GetModuleHandleW(None).unwrap();
            let class = w!("RiderBannerTestUnder");
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(under_proc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            });
            let screen_w = GetSystemMetrics(SM_CXSCREEN);
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                class,
                w!("under"),
                WS_POPUP | WS_VISIBLE,
                screen_w / 2 - 500,
                0,
                1000,
                200,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .unwrap();
            let _ = ShowWindow(hwnd, SW_SHOW);
            let mut msg = MSG::default();
            while GetMessageW(&raw mut msg, None, 0, 0).as_bool() {
                DispatchMessageW(&raw const msg);
            }
        });
        std::thread::sleep(Duration::from_millis(300));

        let banner = Banner::show("test banner — click here to disconnect");
        wait_for("the banner", || rect().is_some());
        std::thread::sleep(Duration::from_millis(300));
        let rc = rect().unwrap();
        unsafe {
            assert!(IsWindowVisible(HWND(HWND_VAL.load(Ordering::SeqCst) as _)).as_bool(), "banner shown");
        }

        let mut before = POINT::default();
        unsafe {
            let _ = GetCursorPos(&raw mut before);
            let _ = SetCursorPos(i32::midpoint(rc.left, rc.right), i32::midpoint(rc.top, rc.bottom));
        }
        std::thread::sleep(Duration::from_millis(100));
        let click = |flags| INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: flags, ..Default::default() } },
        };
        let sent =
            unsafe { SendInput(&[click(MOUSEEVENTF_LEFTDOWN), click(MOUSEEVENTF_LEFTUP)], size_of::<INPUT>() as i32) };
        assert_eq!(sent, 2);
        wait_for("the click to reach the window underneath", || UNDER_CLICKS.load(Ordering::SeqCst) == 1);
        std::thread::sleep(Duration::from_millis(400));
        unsafe {
            let _ = SetCursorPos(before.x, before.y);
        }
        assert!(!DISCONNECT_REQUESTED.load(Ordering::SeqCst), "a remote click must not end the session");

        drop(banner);
        unsafe {
            let h = FindWindowW(w!("RiderBannerTestUnder"), None).unwrap();
            let tid = GetWindowThreadProcessId(h, None);
            let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
            let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        let _ = under.join();
    }
}

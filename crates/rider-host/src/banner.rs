//! Always-on-top banner shown on the host while a remote session is active,
//! so the person at the machine always knows someone is connected. Clicking it
//! ends the session. It is excluded from screen capture, so the remote viewer
//! does not see it covering their work.
//!
//! Since the viewer cannot see it, the viewer's clicks must not hit it either:
//! it sits at the top centre, where browser and editor tabs are, and a remote
//! click on a tab there used to end the session. So the window is click-through
//! (layered + transparent) and a low-level mouse hook watches for clicks on
//! it: only clicks by the person at this computer count. Clicks the session
//! injects (SendInput, flagged LLMHF_INJECTED) pass to the window underneath.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

static TEXT: Mutex<Vec<u16>> = Mutex::new(Vec::new());
static HWND_VAL: AtomicIsize = AtomicIsize::new(0);
/// Set when the local user clicks the banner to kick the remote side out.
pub static DISCONNECT_REQUESTED: AtomicBool = AtomicBool::new(false);

const WIDTH: i32 = 520;
const HEIGHT: i32 = 34;

pub struct Banner {
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Banner {
    pub fn show(text: String) -> Self {
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
        RegisterClassW(&wc);
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        // Wide enough for the text (it differs by language), at least WIDTH.
        let width = {
            let text = TEXT.lock().unwrap().clone();
            let dc = GetDC(None);
            let mut size = windows::Win32::Foundation::SIZE::default();
            let _ = GetTextExtentPoint32W(dc, &text, &mut size);
            ReleaseDC(None, dc);
            (size.cx + 48).clamp(WIDTH, screen_w.max(WIDTH))
        };
        let Ok(hwnd) = CreateWindowExW(
            // Layered + transparent: clicks pass through (see the module docs).
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
        // A layered window is invisible until it has its alpha.
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        // Hide from Desktop Duplication / BitBlt so the viewer's picture stays clean.
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        // Local clicks on it. The hook runs on this thread (it pumps messages
        // and does nothing else, so the hook always answers in time).
        let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(instance.into()), 0);
        if let Err(e) = &hook {
            tracing::warn!("banner mouse hook: {e}; the banner cannot be clicked");
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if let Ok(h) = hook {
            let _ = UnhookWindowsHookEx(h);
        }
    }
}

/// Is this a click by the person at this computer (not one the session
/// injected) inside the banner?
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
        // SAFETY: for WH_MOUSE_LL with HC_ACTION, lparam points to an MSLLHOOKSTRUCT.
        let m = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let h = HWND_VAL.load(Ordering::SeqCst);
        let mut rc = RECT::default();
        // SAFETY: plain query; fails harmlessly if the window is gone.
        if h != 0 && unsafe { GetWindowRect(HWND(h as _), &mut rc) }.is_ok() && local_click_on(m.flags, m.pt, rc) {
            if msg == WM_LBUTTONUP {
                DISCONNECT_REQUESTED.store(true, Ordering::SeqCst);
            }
            // The click was for the banner, not for the window under it.
            return LRESULT(1);
        }
    }
    // SAFETY: pass everything else on.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Visible on screen right now (for the test below).
#[cfg(test)]
fn rect() -> Option<RECT> {
    let h = HWND_VAL.load(Ordering::SeqCst);
    let mut rc = RECT::default();
    // SAFETY: plain query.
    (h != 0 && unsafe { GetWindowRect(HWND(h as _), &mut rc) }.is_ok()).then_some(rc)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let brush = CreateSolidBrush(COLORREF(0x002A2AC8)); // BGR: red
                FillRect(hdc, &rc, brush);
                let _ = DeleteObject(brush.into());
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, COLORREF(0x00FFFFFF));
                let mut text = TEXT.lock().unwrap().clone();
                DrawTextW(hdc, &mut text, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                let _ = EndPaint(hwnd, &ps);
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
        // SAFETY: default handling.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(3);
        while !ok() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A remote click where the (invisible to the viewer) banner is must reach
    /// the window underneath (a browser tab, say) and not end the session.
    /// Needs an interactive desktop and moves the real pointer for a moment:
    /// `cargo test -p rider-host banner -- --ignored --test-threads=1`.
    #[test]
    #[ignore = "needs an interactive desktop; moves the pointer"]
    fn remote_click_passes_through_to_the_window_underneath() {
        // The window underneath: topmost, wide, at the top of the screen like
        // a maximized browser's tab strip. Created before the banner, so the
        // banner is above it.
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
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                DispatchMessageW(&msg);
            }
        });
        std::thread::sleep(Duration::from_millis(300));

        let banner = Banner::show("test banner — click here to disconnect".into());
        wait_for("the banner", || rect().is_some());
        std::thread::sleep(Duration::from_millis(300));
        let rc = rect().unwrap();
        unsafe {
            assert!(IsWindowVisible(HWND(HWND_VAL.load(Ordering::SeqCst) as _)).as_bool(), "banner shown");
        }

        let mut before = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut before);
            let _ = SetCursorPos((rc.left + rc.right) / 2, (rc.top + rc.bottom) / 2);
        }
        std::thread::sleep(Duration::from_millis(100));
        let click = |flags| INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: flags, ..Default::default() } },
        };
        // What the host does with a remote click (input.rs): SendInput.
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
        // End the window underneath (its thread quits with WM_QUIT via the class's
        // DefWindowProc on WM_CLOSE -> no PostQuitMessage, so post it directly).
        unsafe {
            let h = FindWindowW(w!("RiderBannerTestUnder"), None).unwrap();
            let tid = GetWindowThreadProcessId(h, None);
            let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
            let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        let _ = under.join();
    }
}

//! Always-on-top banner shown on the host while a remote session is active,
//! so the person at the machine always knows someone is connected. Clicking it
//! ends the session. It is excluded from screen capture, so the remote viewer
//! does not see it covering their work.

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
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!("Remote Desktop Rider"),
            WS_POPUP | WS_VISIBLE,
            (screen_w - WIDTH) / 2,
            0,
            WIDTH,
            HEIGHT,
            None,
            None,
            Some(instance.into()),
            None,
        ) else {
            return;
        };
        HWND_VAL.store(hwnd.0 as isize, Ordering::SeqCst);
        // Hide from Desktop Duplication / BitBlt so the viewer's picture stays clean.
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
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
            WM_LBUTTONUP => {
                DISCONNECT_REQUESTED.store(true, Ordering::SeqCst);
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

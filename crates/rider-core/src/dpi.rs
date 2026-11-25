use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetWindowLongW, IsZoomed, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, WM_DPICHANGED, WM_NCDESTROY,
    WS_CAPTION,
};

const SUBCLASS_ID: usize = 0x5244_5244;

pub fn follow_windows(hwnd: HWND) {
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(proc), SUBCLASS_ID, 0);
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, id: usize, _data: usize) -> LRESULT {
    unsafe {
        match msg {
            WM_DPICHANGED => {
                let suggested = *(lp.0 as *const RECT);
                let result = DefSubclassProc(hwnd, msg, wp, lp);
                let captioned = GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CAPTION.0 == WS_CAPTION.0;
                if captioned && !IsZoomed(hwnd).as_bool() {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        suggested.left,
                        suggested.top,
                        suggested.right - suggested.left,
                        suggested.bottom - suggested.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                result
            }
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(hwnd, Some(proc), id);
                DefSubclassProc(hwnd, msg, wp, lp)
            }
            _ => DefSubclassProc(hwnd, msg, wp, lp),
        }
    }
}

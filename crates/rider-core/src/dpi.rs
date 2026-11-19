//! Smooth dragging between monitors with different display scaling.
//!
//! When a window crosses into a monitor with another scale (e.g. 125% and
//! 100%), Windows sends `WM_DPICHANGED` with the rectangle the window should
//! take so that its own drag loop and the new size agree. winit computes its
//! own size and position instead, so the two keep correcting each other and
//! the window shakes at the monitor edge until the mouse is well past it.
//! [`follow_windows`] applies Windows' rectangle after winit has handled the
//! scale change, as Microsoft recommends for per-monitor DPI aware windows.

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetWindowLongW, IsZoomed, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, WM_DPICHANGED, WM_NCDESTROY,
    WS_CAPTION,
};

const SUBCLASS_ID: usize = 0x5244_5244; // "RDRD"

/// Install on `hwnd`. Must be called on the thread that owns the window;
/// calling it again for the same window is harmless.
pub fn follow_windows(hwnd: HWND) {
    // SAFETY: plain subclassing of our own window on its thread; the
    // procedure keeps no state.
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(proc), SUBCLASS_ID, 0);
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, id: usize, _data: usize) -> LRESULT {
    // SAFETY (whole function): Windows passes a valid RECT pointer with
    // WM_DPICHANGED; the other calls only take our own window handle.
    unsafe {
        match msg {
            WM_DPICHANGED => {
                let suggested = *(lp.0 as *const RECT);
                // winit updates its scale factor (and tells the UI) first.
                let result = DefSubclassProc(hwnd, msg, wp, lp);
                // Maximized and fullscreen (captionless) windows keep the size winit chose.
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

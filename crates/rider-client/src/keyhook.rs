//! System keys for the remote computer. Windows handles the Win key, Alt+Tab,
//! Alt+Esc and Ctrl+Esc in the shell before a window sees them, so Win+R,
//! Win+P, Win+E, Alt+Tab... would act on this computer. A low-level keyboard
//! hook catches them while the session window is in front, sends them to the host and keeps them
//! from this computer. Other keys go through the window as usual, so the host
//! sees e.g. Win down, P down/up, Win up = Win+P.
//!
//! Win+L and Ctrl+Alt+Del cannot be intercepted by any program: they always
//! act here (use the toolbar's Ctrl+Alt+Del for the remote one).
//!
//! The hook runs on its own thread with nothing else to do: Windows skips a
//! low-level hook that answers late (and may drop it), which a thread busy
//! decoding and drawing frames could not promise.
//!
//! The viewer must not register for raw keyboard input (winit's device
//! events): with that on, the hook is not called while the window is in front.

use rider_core::proto::ClientMsg;
use std::collections::HashSet;
use std::sync::Mutex;
use std::thread::JoinHandle;
use tokio::sync::mpsc::UnboundedSender;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_LWIN, VK_RWIN, VK_TAB};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_ALTDOWN, LLKHF_EXTENDED,
    LLKHF_UP, MSG, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_QUIT,
};

struct State {
    /// The session window (an HWND; kept as a number so State is Send).
    window: usize,
    out: UnboundedSender<ClientMsg>,
    /// Keys sent down and not yet up, released when focus moves away.
    down: HashSet<(u16, bool)>,
    ctrl: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// The hook thread; stops and removes the hook when dropped.
pub struct KeyHook {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl KeyHook {
    pub fn install(window: HWND, out: UnboundedSender<ClientMsg>) -> Option<Self> {
        *STATE.lock().ok()? = Some(State { window: window.0 as usize, out, down: HashSet::new(), ctrl: false });
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("keyhook".into())
            .spawn(move || {
                // SAFETY: the hook is installed and removed on this thread,
                // which pumps messages (GetMessageW) for as long as it lives.
                unsafe {
                    // A global hook needs a module handle (ERROR_HOOK_NEEDS_HMOD otherwise).
                    let hook = GetModuleHandleW(None)
                        .and_then(|m| SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), Some(m.into()), 0));
                    let hook = match hook {
                        Ok(h) => h,
                        Err(e) => {
                            tracing::warn!("keyboard hook: {e}");
                            let _ = tx.send(None);
                            return;
                        }
                    };
                    let _ = tx.send(Some(GetCurrentThreadId()));
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {}
                    let _ = UnhookWindowsHookEx(hook);
                }
            })
            .ok()?;
        match rx.recv() {
            Ok(Some(thread_id)) => Some(Self { thread_id, thread: Some(thread) }),
            _ => {
                let _ = thread.join();
                None
            }
        }
    }

    /// Focus left the session window: release what the remote side holds.
    pub fn release(&self) {
        if let Ok(mut s) = STATE.lock()
            && let Some(st) = s.as_mut()
        {
            for (scancode, extended) in st.down.drain() {
                let _ = st.out.send(ClientMsg::Key { scancode, extended, down: false });
            }
        }
    }
}

impl Drop for KeyHook {
    fn drop(&mut self) {
        self.release();
        // SAFETY: plain message to our own thread, which then unhooks and exits.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Ok(mut s) = STATE.lock() {
            *s = None;
        }
    }
}

/// Should this key event go to the host instead of this computer's shell?
fn system_key(vk: u32, alt: bool, ctrl: bool) -> bool {
    vk == VK_LWIN.0 as u32
        || vk == VK_RWIN.0 as u32
        || (alt && (vk == VK_TAB.0 as u32 || vk == VK_ESCAPE.0 as u32))
        || (ctrl && vk == VK_ESCAPE.0 as u32)
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for WH_KEYBOARD_LL with HC_ACTION, lparam points to a KBDLLHOOKSTRUCT.
        let k = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let taken = (|| {
            let mut s = STATE.lock().ok()?;
            let st = s.as_mut()?;
            const VK_CONTROL_L: u32 = 0xA2;
            const VK_CONTROL_R: u32 = 0xA3;
            let up = k.flags.0 & LLKHF_UP.0 != 0;
            if k.vkCode == VK_CONTROL_L || k.vkCode == VK_CONTROL_R {
                st.ctrl = !up;
            }
            // SAFETY: plain query.
            if unsafe { GetForegroundWindow() }.0 as usize != st.window {
                return None;
            }
            let alt = k.flags.0 & LLKHF_ALTDOWN.0 != 0;
            let key = (k.scanCode as u16 & 0xFF, k.flags.0 & LLKHF_EXTENDED.0 != 0);
            // Ups of keys we sent down are always ours (Alt may be up already).
            if !system_key(k.vkCode, alt, st.ctrl) && !(up && st.down.contains(&key)) {
                return None;
            }
            if up {
                st.down.remove(&key);
            } else {
                st.down.insert(key);
            }
            let _ = st.out.send(ClientMsg::Key { scancode: key.0, extended: key.1, down: !up });
            Some(())
        })();
        if taken.is_some() {
            return LRESULT(1);
        }
    }
    // SAFETY: pass everything else on.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_keys_are_taken() {
        assert!(system_key(VK_LWIN.0 as u32, false, false));
        assert!(system_key(VK_RWIN.0 as u32, false, false));
        assert!(system_key(VK_TAB.0 as u32, true, false));
        assert!(system_key(VK_ESCAPE.0 as u32, false, true));
        assert!(!system_key(VK_TAB.0 as u32, false, false));
        assert!(!system_key(0x50, false, false)); // P alone goes through the window
    }
}

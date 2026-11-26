use rider_core::proto::ClientMsg;
use std::collections::HashSet;
use std::sync::Mutex;
use std::thread::JoinHandle;
use tokio::sync::mpsc::UnboundedSender;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_F4, VK_LWIN, VK_RWIN, VK_TAB};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_ALTDOWN, LLKHF_EXTENDED,
    LLKHF_UP, MSG, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_QUIT,
};

struct State {
    window: usize,
    out: UnboundedSender<ClientMsg>,
    down: HashSet<(u16, bool)>,
    ctrl: bool,
    enabled: bool,
}

const VK_CONTROL_L: u32 = 0xA2;
const VK_CONTROL_R: u32 = 0xA3;

static STATE: Mutex<Option<State>> = Mutex::new(None);

pub struct KeyHook {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl KeyHook {
    pub fn install(window: HWND, out: UnboundedSender<ClientMsg>) -> Option<Self> {
        *STATE.lock().ok()? =
            Some(State { window: window.0 as usize, out, down: HashSet::new(), ctrl: false, enabled: true });
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("keyhook".into())
            .spawn(move || unsafe {
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
                while GetMessageW(&raw mut msg, None, 0, 0).0 > 0 {}
                let _ = UnhookWindowsHookEx(hook);
            })
            .ok()?;
        if let Ok(Some(thread_id)) = rx.recv() {
            Some(Self { thread_id, thread: Some(thread) })
        } else {
            let _ = thread.join();
            None
        }
    }

    #[expect(clippy::unused_self, reason = "the hook state is process-global; only the owner of the hook changes it")]
    pub fn retarget(&self, out: UnboundedSender<ClientMsg>) {
        if let Ok(mut s) = STATE.lock()
            && let Some(st) = s.as_mut()
        {
            st.out = out;
            st.down.clear();
        }
    }

    pub fn set_enabled(&self, on: bool) {
        if !on {
            self.release();
        }
        if let Ok(mut s) = STATE.lock()
            && let Some(st) = s.as_mut()
        {
            st.enabled = on;
        }
    }

    #[expect(clippy::unused_self, reason = "the hook state is process-global; only the owner of the hook changes it")]
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

fn system_key(vk: u32, alt: bool, ctrl: bool) -> bool {
    vk == u32::from(VK_LWIN.0)
        || vk == u32::from(VK_RWIN.0)
        || (alt && (vk == u32::from(VK_TAB.0) || vk == u32::from(VK_ESCAPE.0) || vk == u32::from(VK_F4.0)))
        || (ctrl && vk == u32::from(VK_ESCAPE.0))
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let k = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let taken = (|| {
            let mut s = STATE.lock().ok()?;
            let st = s.as_mut()?;
            let up = k.flags.0 & LLKHF_UP.0 != 0;
            if k.vkCode == VK_CONTROL_L || k.vkCode == VK_CONTROL_R {
                st.ctrl = !up;
            }
            if !st.enabled || unsafe { GetForegroundWindow() }.0 as usize != st.window {
                return None;
            }
            let alt = k.flags.0 & LLKHF_ALTDOWN.0 != 0;
            let key = (k.scanCode as u16 & 0xFF, k.flags.0 & LLKHF_EXTENDED.0 != 0);
            if !system_key(k.vkCode, alt, st.ctrl) && (!up || !st.down.contains(&key)) {
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
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_keys_are_taken() {
        assert!(system_key(u32::from(VK_LWIN.0), false, false));
        assert!(system_key(u32::from(VK_RWIN.0), false, false));
        assert!(system_key(u32::from(VK_TAB.0), true, false));
        assert!(system_key(u32::from(VK_ESCAPE.0), false, true));
        assert!(system_key(u32::from(VK_F4.0), true, false));
        assert!(!system_key(u32::from(VK_F4.0), false, false));
        assert!(!system_key(u32::from(VK_TAB.0), false, false));
        assert!(!system_key(0x50, false, false));
    }
}

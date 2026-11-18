//! Input injection with SendInput. Keys are sent as hardware scan codes, so
//! the host's own keyboard layout applies (same as a physical keyboard).

use rider_core::proto::{MonitorInfo, MouseButton};
use std::collections::HashSet;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const XBUTTON1: u32 = 1;
const XBUTTON2: u32 = 2;

pub struct Injector {
    monitor: MonitorInfo,
    last_seq: u32,
    /// Keys/buttons currently held by the remote side; released on disconnect.
    held_keys: HashSet<(u16, bool)>,
    held_buttons: HashSet<u8>,
}

fn send(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn button_flags(b: MouseButton, down: bool) -> (MOUSE_EVENT_FLAGS, u32) {
    match (b, down) {
        (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (MouseButton::Back, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
        (MouseButton::Back, false) => (MOUSEEVENTF_XUP, XBUTTON1),
        (MouseButton::Forward, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
        (MouseButton::Forward, false) => (MOUSEEVENTF_XUP, XBUTTON2),
    }
}

impl Injector {
    pub fn new(monitor: MonitorInfo) -> Self {
        Self { monitor, last_seq: 0, held_keys: HashSet::new(), held_buttons: HashSet::new() }
    }

    pub fn set_monitor(&mut self, m: MonitorInfo) {
        self.monitor = m;
    }

    /// Absolute move; `x`/`y` are 0..=65535 across the streamed monitor.
    /// Datagrams can arrive out of order, so stale positions are dropped.
    pub fn mouse_move(&mut self, seq: u32, x: u16, y: u16) {
        if seq.wrapping_sub(self.last_seq) > u32::MAX / 2 && self.last_seq != 0 {
            return;
        }
        self.last_seq = seq;
        let m = &self.monitor;
        let px = m.x as i64 + (x as i64 * (m.width as i64 - 1)) / 65535;
        let py = m.y as i64 + (y as i64 * (m.height as i64 - 1)) / 65535;
        unsafe {
            let vx = GetSystemMetrics(SM_XVIRTUALSCREEN) as i64;
            let vy = GetSystemMetrics(SM_YVIRTUALSCREEN) as i64;
            let vw = (GetSystemMetrics(SM_CXVIRTUALSCREEN) as i64 - 1).max(1);
            let vh = (GetSystemMetrics(SM_CYVIRTUALSCREEN) as i64 - 1).max(1);
            let nx = ((px - vx) * 65535 / vw) as i32;
            let ny = ((py - vy) * 65535 / vh) as i32;
            send(&[mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, nx, ny, 0)]);
        }
    }

    pub fn mouse_button(&mut self, b: MouseButton, down: bool) {
        let id = b as u8;
        if down {
            self.held_buttons.insert(id);
        } else {
            self.held_buttons.remove(&id);
        }
        let (flags, data) = button_flags(b, down);
        send(&[mouse(flags, 0, 0, data)]);
    }

    pub fn wheel(&mut self, dx: i32, dy: i32) {
        if dy != 0 {
            send(&[mouse(MOUSEEVENTF_WHEEL, 0, 0, dy as u32)]);
        }
        if dx != 0 {
            send(&[mouse(MOUSEEVENTF_HWHEEL, 0, 0, dx as u32)]);
        }
    }

    pub fn key(&mut self, scancode: u16, extended: bool, down: bool) {
        if down {
            self.held_keys.insert((scancode, extended));
        } else {
            self.held_keys.remove(&(scancode, extended));
        }
        send(&[key_input(scancode, extended, down)]);
    }

    /// Release everything the remote side was holding (avoids stuck Ctrl/Shift).
    pub fn release_all(&mut self) {
        for (sc, ext) in std::mem::take(&mut self.held_keys) {
            send(&[key_input(sc, ext, false)]);
        }
        for id in std::mem::take(&mut self.held_buttons) {
            let b =
                [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::Back, MouseButton::Forward]
                    [id as usize];
            let (flags, data) = button_flags(b, false);
            send(&[mouse(flags, 0, 0, data)]);
        }
    }
}

fn key_input(scancode: u16, extended: bool, down: bool) -> INPUT {
    let mut flags = KEYEVENTF_SCANCODE;
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: scancode, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        self.release_all();
    }
}

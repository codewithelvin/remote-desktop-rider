use rider_core::proto::{MonitorInfo, MouseButton};
use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

const XBUTTON1: u32 = 1;
const XBUTTON2: u32 = 2;

struct Injector {
    monitor: MonitorInfo,
    last_seq: u32,
    held_keys: HashSet<(u16, bool)>,
    held_buttons: HashSet<u8>,
}

pub enum InputEvent {
    Move { seq: u32, x: u16, y: u16 },
    Button { button: MouseButton, down: bool },
    ButtonAt { button: MouseButton, down: bool, x: u16, y: u16, seq: u32 },
    Wheel { dx: i32, dy: i32 },
    Key { scancode: u16, extended: bool, down: bool },
    Monitor(MonitorInfo),
    ReleaseAll,
}

#[derive(Clone)]
pub struct InputHandle(Sender<InputEvent>);

impl InputHandle {
    pub fn send(&self, ev: InputEvent) {
        let _ = self.0.send(ev);
    }
}

pub fn spawn(monitor: MonitorInfo) -> InputHandle {
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("rider-input".into())
        .spawn(move || run(rx, Injector::new(monitor)))
        .expect("spawn input thread");
    InputHandle(tx)
}

const DESKTOP_RECHECK: Duration = Duration::from_millis(250);

fn run(rx: Receiver<InputEvent>, mut inj: Injector) {
    crate::system::follow_input_desktop();
    let mut checked = Instant::now();
    for ev in rx {
        if checked.elapsed() >= DESKTOP_RECHECK {
            crate::system::follow_input_desktop();
            checked = Instant::now();
        }
        match ev {
            InputEvent::Move { seq, x, y } => inj.mouse_move(seq, x, y),
            InputEvent::Button { button, down } => inj.mouse_button(button, down, None),
            InputEvent::ButtonAt { button, down, x, y, seq } => {
                if inj.newer(seq) {
                    inj.last_seq = seq;
                }
                inj.mouse_button(button, down, Some((x, y)));
            }
            InputEvent::Wheel { dx, dy } => Injector::wheel(dx, dy),
            InputEvent::Key { scancode, extended, down } => inj.key(scancode, extended, down),
            InputEvent::Monitor(m) => inj.monitor = m,
            InputEvent::ReleaseAll => inj.release_all(),
        }
    }
}

fn send(inputs: &[INPUT]) {
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    if sent == 0 && crate::system::follow_input_desktop() {
        unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
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
    fn new(monitor: MonitorInfo) -> Self {
        Self { monitor, last_seq: 0, held_keys: HashSet::new(), held_buttons: HashSet::new() }
    }

    fn mouse_move(&mut self, seq: u32, x: u16, y: u16) {
        if !self.newer(seq) {
            return;
        }
        self.last_seq = seq;
        send(&[self.move_to(x, y)]);
    }

    fn newer(&self, seq: u32) -> bool {
        self.last_seq == 0 || seq.wrapping_sub(self.last_seq) <= u32::MAX / 2
    }

    fn move_to(&self, x: u16, y: u16) -> INPUT {
        let m = &self.monitor;
        let px = i64::from(m.x) + (i64::from(x) * (i64::from(m.width) - 1)) / 65535;
        let py = i64::from(m.y) + (i64::from(y) * (i64::from(m.height) - 1)) / 65535;
        let (vx, vy, vw, vh) = unsafe {
            (
                i64::from(GetSystemMetrics(SM_XVIRTUALSCREEN)),
                i64::from(GetSystemMetrics(SM_YVIRTUALSCREEN)),
                (i64::from(GetSystemMetrics(SM_CXVIRTUALSCREEN)) - 1).max(1),
                (i64::from(GetSystemMetrics(SM_CYVIRTUALSCREEN)) - 1).max(1),
            )
        };
        let nx = ((px - vx) * 65535 / vw) as i32;
        let ny = ((py - vy) * 65535 / vh) as i32;
        mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, nx, ny, 0)
    }

    fn mouse_button(&mut self, b: MouseButton, down: bool, at: Option<(u16, u16)>) {
        let id = b as u8;
        if down {
            self.held_buttons.insert(id);
        } else {
            self.held_buttons.remove(&id);
        }
        let (flags, data) = button_flags(b, down);
        let press = mouse(flags, 0, 0, data);
        match at {
            Some((x, y)) => send(&[self.move_to(x, y), press]),
            None => send(&[press]),
        }
    }

    fn wheel(dx: i32, dy: i32) {
        if dy != 0 {
            send(&[mouse(MOUSEEVENTF_WHEEL, 0, 0, dy as u32)]);
        }
        if dx != 0 {
            send(&[mouse(MOUSEEVENTF_HWHEEL, 0, 0, dx as u32)]);
        }
    }

    fn key(&mut self, scancode: u16, extended: bool, down: bool) {
        if down {
            self.held_keys.insert((scancode, extended));
        } else {
            self.held_keys.remove(&(scancode, extended));
        }
        send(&[key_input(scancode, extended, down)]);
    }

    fn release_all(&mut self) {
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

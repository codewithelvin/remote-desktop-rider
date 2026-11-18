//! Viewer window: draws the newest frame, captures input, shows live stats.
//!
//! The mouse cursor is drawn locally by Windows (the host does not render
//! it into the video), so pointer movement feels instant whatever the latency.
//!
//! Local shortcuts (not sent to the host):
//!   Ctrl+Alt+Shift+M  next monitor
//!   Ctrl+Alt+Shift+F  toggle fullscreen
//!   Ctrl+Alt+Shift+Q  disconnect
//!   Ctrl+Alt+End      Ctrl+Alt+Del on the host (login screen, lock screen)
//!   Drag and drop a file onto the window to send it to the host.

use crate::net::{self, NetHandle, Session, Shared, UiEvent};
use anyhow::Result;
use rider_core::pixels::{Nv12Frame, nv12_to_xrgb_fit};
use rider_core::proto::{ClientMsg, MonitorInfo, MouseButton};
use std::collections::HashSet;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::platform::scancode::PhysicalKeyExtScancode;
use winit::window::{Fullscreen, Window, WindowId};

pub fn run(rt: tokio::runtime::Runtime, session: Session, host_name: String) -> Result<()> {
    let event_loop = EventLoop::<UiEvent>::with_user_event().build()?;
    let shared = Arc::new(Shared::default());
    let net = net::start(&rt, session, event_loop.create_proxy(), shared.clone());
    let mut app = App {
        net,
        shared,
        host_name,
        window: None,
        surface: None,
        current: None,
        image_rect: (0, 0, 0, 0),
        modifiers: ModifiersState::empty(),
        held_keys: HashSet::new(),
        monitors: Vec::new(),
        active_monitor: 0,
        notice: None,
        next_title: Instant::now(),
        closed: None,
        last_frames: 0,
    };
    event_loop.run_app(&mut app)?;
    if let Some(reason) = app.closed {
        println!("Disconnected: {reason}");
    }
    rt.shutdown_timeout(Duration::from_secs(1));
    Ok(())
}

type Surface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

struct App {
    net: NetHandle,
    shared: Arc<Shared>,
    host_name: String,
    window: Option<Rc<Window>>,
    surface: Option<(softbuffer::Context<Rc<Window>>, Surface)>,
    current: Option<Nv12Frame>,
    /// Where the remote image is drawn inside the window (x, y, w, h).
    image_rect: (usize, usize, usize, usize),
    modifiers: ModifiersState,
    held_keys: HashSet<(u16, bool)>,
    monitors: Vec<MonitorInfo>,
    active_monitor: u32,
    notice: Option<(String, Instant)>,
    next_title: Instant,
    closed: Option<String>,
    last_frames: u32,
}

impl App {
    fn redraw(&mut self) {
        let (Some(window), Some((_, surface))) = (&self.window, &mut self.surface) else { return };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else { return };
        match &self.current {
            Some(f) => self.image_rect = nv12_to_xrgb_fit(f, &mut buf, w.get() as usize, h.get() as usize),
            None => buf.fill(0x0010_1014),
        }
        let _ = buf.present();
    }

    fn update_title(&mut self) {
        let Some(window) = &self.window else { return };
        let s = &self.shared;
        let frames = s.frames.load(Ordering::Relaxed);
        let fps = frames.wrapping_sub(self.last_frames);
        self.last_frames = frames;
        let mut title = format!(
            "{} — {fps} fps · RTT {:.0} ms · latency ~{:.0} ms · decode {:.1} ms · {}",
            self.host_name,
            s.rtt_us.load(Ordering::Relaxed) as f64 / 1000.0,
            s.latency_us.load(Ordering::Relaxed) as f64 / 1000.0,
            s.decode_us.load(Ordering::Relaxed) as f64 / 1000.0,
            s.host_stats.lock().unwrap(),
        );
        if self.monitors.len() > 1 {
            title.push_str(&format!(" · monitor {}/{}", self.active_monitor + 1, self.monitors.len()));
        }
        if let Some((n, t)) = &self.notice
            && t.elapsed() < Duration::from_secs(6)
        {
            title = format!("{n} — {title}");
        }
        window.set_title(&title);
    }

    fn release_keys(&mut self) {
        for (scancode, extended) in self.held_keys.drain() {
            self.net.send(ClientMsg::Key { scancode, extended, down: false });
        }
    }

    /// Returns true if the key press was a local shortcut.
    fn shortcut(&mut self, el: &ActiveEventLoop, event: &KeyEvent) -> bool {
        let key = event.physical_key;
        let m = self.modifiers;
        // Ctrl+Alt+End = Ctrl+Alt+Del on the host (same convention as RDP); the
        // real Ctrl+Alt+Del is always captured by the local machine.
        // Match by meaning too: the numpad End key (NumLock off) counts, as on laptops.
        let end = key == PhysicalKey::Code(KeyCode::End) || event.logical_key == Key::Named(NamedKey::End);
        if m.control_key() && m.alt_key() && end {
            self.release_keys();
            self.net.send(ClientMsg::SendSas);
            return true;
        }
        if !(m.control_key() && m.alt_key() && m.shift_key()) {
            return false;
        }
        match key {
            PhysicalKey::Code(KeyCode::KeyM) if self.monitors.len() > 1 => {
                let next = (self.active_monitor + 1) % self.monitors.len() as u32;
                self.net.send(ClientMsg::SelectMonitor { index: next });
            }
            PhysicalKey::Code(KeyCode::KeyF) => {
                if let Some(w) = &self.window {
                    let fs = if w.fullscreen().is_some() { None } else { Some(Fullscreen::Borderless(None)) };
                    w.set_fullscreen(fs);
                }
            }
            PhysicalKey::Code(KeyCode::KeyQ) => {
                self.release_keys();
                self.closed = Some("disconnected by you".into());
                self.net.conn.close(0u32.into(), b"bye");
                el.exit();
            }
            _ => return false,
        }
        true
    }
}

impl ApplicationHandler<UiEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(format!("{} — connecting...", self.host_name))
            .with_inner_size(LogicalSize::new(1280.0, 760.0));
        let window = match el.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                self.closed = Some(format!("cannot create window: {e}"));
                el.exit();
                return;
            }
        };
        match softbuffer::Context::new(window.clone())
            .and_then(|ctx| softbuffer::Surface::new(&ctx, window.clone()).map(|s| (ctx, s)))
        {
            Ok(pair) => self.surface = Some(pair),
            Err(e) => {
                self.closed = Some(format!("cannot create drawing surface: {e}"));
                el.exit();
                return;
            }
        }
        self.window = Some(window);
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: UiEvent) {
        match ev {
            UiEvent::NewFrame => {
                if let Some(f) = self.shared.latest.lock().unwrap().take() {
                    self.current = Some(f);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            UiEvent::Monitors(list, active) => {
                self.monitors = list;
                self.active_monitor = active;
            }
            UiEvent::Notice(n) => {
                tracing::info!("{n}");
                self.notice = Some((n, Instant::now()));
                self.update_title();
            }
            UiEvent::Closed(reason) => {
                self.closed.get_or_insert(reason);
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.release_keys();
                self.net.conn.close(0u32.into(), b"bye");
                self.closed.get_or_insert("window closed".into());
                el.exit();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(_) => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::Focused(false) => self.release_keys(),
            WindowEvent::CursorMoved { position, .. } => {
                let (ox, oy, w, h) = self.image_rect;
                if w > 1 && h > 1 {
                    let x = ((position.x - ox as f64) / (w - 1) as f64).clamp(0.0, 1.0);
                    let y = ((position.y - oy as f64) / (h - 1) as f64).clamp(0.0, 1.0);
                    self.net.mouse_move((x * 65535.0) as u16, (y * 65535.0) as u16);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    winit::event::MouseButton::Left => MouseButton::Left,
                    winit::event::MouseButton::Right => MouseButton::Right,
                    winit::event::MouseButton::Middle => MouseButton::Middle,
                    winit::event::MouseButton::Back => MouseButton::Back,
                    winit::event::MouseButton::Forward => MouseButton::Forward,
                    winit::event::MouseButton::Other(_) => return,
                };
                self.net.send(ClientMsg::MouseButton { button: b, down: state == ElementState::Pressed });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => ((x * 120.0) as i32, (y * 120.0) as i32),
                    MouseScrollDelta::PixelDelta(p) => (p.x as i32, p.y as i32),
                };
                self.net.send(ClientMsg::MouseWheel { dx, dy });
            }
            WindowEvent::KeyboardInput { event, is_synthetic, .. } => {
                if is_synthetic {
                    return;
                }
                let down = event.state == ElementState::Pressed;
                if down && self.shortcut(el, &event) {
                    return;
                }
                let Some(sc) = event.physical_key.to_scancode() else { return };
                let scancode = (sc & 0xFF) as u16;
                let extended = (sc & 0xFF00) == 0xE000;
                if down {
                    self.held_keys.insert((scancode, extended));
                } else {
                    self.held_keys.remove(&(scancode, extended));
                }
                self.net.send(ClientMsg::Key { scancode, extended, down });
            }
            WindowEvent::DroppedFile(path) => self.net.send_file(path),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if Instant::now() >= self.next_title {
            self.update_title();
            self.next_title = Instant::now() + Duration::from_secs(1);
        }
        el.set_control_flow(ControlFlow::WaitUntil(self.next_title));
    }
}

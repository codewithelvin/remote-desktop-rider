//! Viewer window: draws the newest frame, captures input, shows live stats.
//! A toolbar at the top centre (see `toolbar`) offers the actions below.
//!
//! The mouse cursor is drawn locally by Windows (the host does not render
//! it into the video), so pointer movement feels instant whatever the latency.
//!
//! Local shortcuts (not sent to the host):
//!   Ctrl+Alt+Shift+M  next monitor
//!   Ctrl+Alt+Shift+F  toggle fullscreen
//!   Ctrl+Alt+Shift+Q  disconnect
//!   Ctrl+Alt+Shift+L  lock the remote computer (Win+L always locks this one)
//!   Ctrl+Alt+End      Ctrl+Alt+Del on the host (login screen, lock screen)
//!   Win key combinations, Alt+Tab, Alt+Esc, Ctrl+Esc, Alt+F4 go to the host (keyhook)
//!   Drag and drop a file onto the window to send it to the host.

use crate::diag::{self, Diag};
use crate::keyhook::KeyHook;
use crate::net::{self, NetHandle, Session, Shared, UiEvent};
use crate::toolbar::{Action, Extras, Toolbar};
use anyhow::Result;
use rider_core::pixels::{Nv12Frame, nv12_to_xrgb_fit};
use rider_core::proto::{ClientMsg, MonitorInfo, MouseButton, QUALITIES};
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
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Fullscreen, Icon, Window, WindowId};

/// `report_to`: where to send this session's connection diagnostics.
pub fn run(
    rt: tokio::runtime::Runtime,
    session: Session,
    host_name: String,
    report_to: Option<diag::Target>,
) -> Result<()> {
    let event_loop = EventLoop::<UiEvent>::with_user_event().build()?;
    let shared = Arc::new(Shared::default());
    let diag = report_to.map(|t| Diag::new(t, &session.conn, session.relayed, rt.handle().clone()));
    // Hosts that speak protocol 2 take the quality setting (and lock).
    let v2 = rider_core::tls::speaks_v2(&session.conn);
    let prefs = crate::config::ClientConfig::load().map(|c| c.viewer).unwrap_or_default();
    let net = net::start(&rt, session, event_loop.create_proxy(), shared.clone());
    let quality = if v2 { prefs.quality } else { 0 };
    if quality != 0 {
        net.send(ClientMsg::SetQuality { max_height: quality });
    }
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
        toolbar: Toolbar::new(),
        toolbar_shown: true,
        keyhook: None,
        diag,
        started: Instant::now(),
        v2,
        quality,
        pinned: prefs.pin_toolbar,
    };
    app.refresh_extras();
    event_loop.run_app(&mut app)?;
    if let Some(d) = app.diag.take() {
        d.finish(&rt, app.closed.as_deref().unwrap_or("closed by the user"));
    }
    if let Some(reason) = app.closed {
        println!("Disconnected: {reason}");
    }
    rt.shutdown_timeout(Duration::from_secs(1));
    Ok(())
}

type Surface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

fn hwnd(window: &Window) -> Option<windows::Win32::Foundation::HWND> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(windows::Win32::Foundation::HWND(h.hwnd.get() as *mut _)),
        _ => None,
    }
}

fn app_icon(size: u32) -> Option<Icon> {
    Icon::from_rgba(rider_core::icon::rgba(size), size, size).ok()
}

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
    toolbar: Toolbar,
    /// Whether the last drawn frame showed the full toolbar.
    toolbar_shown: bool,
    /// Win key, Alt+Tab... to the host while this window is in front.
    keyhook: Option<KeyHook>,
    diag: Option<Diag>,
    started: Instant,
    /// The host speaks protocol 2 (quality setting, lock).
    v2: bool,
    /// Max lines asked of the host, 0 = automatic.
    quality: u32,
    pinned: bool,
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
        let (w, h) = (w.get() as usize, h.get() as usize);
        match &self.current {
            Some(f) => self.image_rect = nv12_to_xrgb_fit(f, &mut buf, w, h),
            None => buf.fill(0x0010_1014),
        }
        let monitors = (self.monitors.len() as u32, self.active_monitor);
        self.toolbar.set_state(window.scale_factor(), window.fullscreen().is_some(), monitors);
        self.toolbar.draw(&mut buf, w, h);
        self.toolbar_shown = self.toolbar.visible();
        let _ = buf.present();
    }

    fn update_title(&mut self) {
        let Some(window) = &self.window else { return };
        let s = &self.shared;
        let frames = s.frames.load(Ordering::Relaxed);
        let fps = frames.wrapping_sub(self.last_frames);
        self.last_frames = frames;
        let mut title = format!(
            "{} — {} · {fps} fps · RTT {:.0} ms · latency ~{:.0} ms · decode {:.1} ms · {}",
            self.host_name,
            elapsed(self.started.elapsed()),
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
            self.act(el, Action::CtrlAltDel);
            return true;
        }
        if !(m.control_key() && m.alt_key() && m.shift_key()) {
            return false;
        }
        let action = match key {
            PhysicalKey::Code(KeyCode::KeyM) if self.monitors.len() > 1 => {
                Action::Monitor((self.active_monitor + 1) % self.monitors.len() as u32)
            }
            PhysicalKey::Code(KeyCode::KeyF) => Action::Fullscreen,
            PhysicalKey::Code(KeyCode::KeyQ) => Action::Disconnect,
            // Win+L itself always locks this computer: Windows reserves it.
            PhysicalKey::Code(KeyCode::KeyL) => {
                self.lock_remote();
                return true;
            }
            _ => return false,
        };
        self.act(el, action);
        true
    }

    fn act(&mut self, el: &ActiveEventLoop, action: Action) {
        match action {
            Action::Fullscreen => {
                if let Some(w) = &self.window {
                    let fs = if w.fullscreen().is_some() { None } else { Some(Fullscreen::Borderless(None)) };
                    w.set_fullscreen(fs);
                    w.request_redraw();
                }
            }
            Action::CtrlAltDel => {
                self.release_keys();
                self.net.send(ClientMsg::SendSas);
            }
            Action::Monitor(index) => {
                if index != self.active_monitor && (index as usize) < self.monitors.len() {
                    self.net.send(ClientMsg::SelectMonitor { index });
                }
            }
            Action::Quality => self.quality_menu(),
            Action::Pin => {
                self.pinned = !self.pinned;
                let pinned = self.pinned;
                crate::config::update_viewer(|v| v.pin_toolbar = pinned);
                self.refresh_extras();
                self.redraw_soon();
            }
            Action::Elapsed => {}
            Action::Disconnect => {
                self.release_keys();
                self.closed = Some("disconnected by you".into());
                self.net.conn.close(0u32.into(), b"bye");
                el.exit();
            }
        }
    }

    /// Time, quality and pin on the toolbar; true if the bar must be redrawn.
    fn refresh_extras(&mut self) -> bool {
        let label = QUALITIES.iter().find(|q| q.0 == self.quality).map_or("Auto", |q| q.1);
        self.toolbar.set_extras(Extras {
            elapsed: elapsed(self.started.elapsed()),
            quality: self.v2.then(|| label.to_owned()),
            pinned: self.pinned,
        })
    }

    /// The quality dropdown, under its toolbar button.
    fn quality_menu(&mut self) {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::ClientToScreen;
        use windows::Win32::UI::WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, DestroyMenu, MF_CHECKED, MF_STRING, MF_UNCHECKED, TPM_LEFTALIGN,
            TPM_RETURNCMD, TPM_TOPALIGN, TrackPopupMenu,
        };
        let Some(h) = self.window.as_deref().and_then(hwnd) else { return };
        let Some((x0, _, bar_h)) = self.toolbar.button_rect(Action::Quality) else { return };
        let mut pt = POINT { x: x0 as i32, y: bar_h as i32 };
        // SAFETY: a popup menu owned and destroyed here; strings outlive the calls.
        let cmd = unsafe {
            let _ = ClientToScreen(h, &mut pt);
            let Ok(menu) = CreatePopupMenu() else { return };
            let texts: Vec<Vec<u16>> = QUALITIES
                .iter()
                .map(|&(lines, label)| {
                    let text = if lines == 0 { "Auto (adapts to the connection)" } else { label };
                    text.encode_utf16().chain(std::iter::once(0)).collect()
                })
                .collect();
            for (i, (&(lines, _), text)) in QUALITIES.iter().zip(&texts).enumerate() {
                let check = if lines == self.quality { MF_CHECKED } else { MF_UNCHECKED };
                let _ = AppendMenuW(menu, MF_STRING | check, i + 1, windows::core::PCWSTR(text.as_ptr()));
            }
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN, pt.x, pt.y, None, h, None);
            let _ = DestroyMenu(menu);
            cmd.0 as usize
        };
        let Some(&(lines, _)) = cmd.checked_sub(1).and_then(|i| QUALITIES.get(i)) else { return };
        if lines != self.quality {
            self.quality = lines;
            self.net.send(ClientMsg::SetQuality { max_height: lines });
            crate::config::update_viewer(|v| v.quality = lines);
            self.refresh_extras();
            self.redraw_soon();
        }
    }

    /// Lock the remote computer (what Win+L would do there).
    fn lock_remote(&mut self) {
        self.release_keys();
        if self.v2 {
            self.net.send(ClientMsg::LockScreen);
        } else {
            self.notice = Some(("Locking needs a newer version on the remote computer".into(), Instant::now()));
            self.update_title();
        }
    }

    fn redraw_soon(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

impl ApplicationHandler<UiEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(format!("{} — connecting...", self.host_name))
            .with_inner_size(LogicalSize::new(1280.0, 760.0))
            .with_window_icon(app_icon(32))
            .with_taskbar_icon(app_icon(64));
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
        // Unused, and raw keyboard input would stop the keyboard hook (keyhook.rs).
        el.listen_device_events(winit::event_loop::DeviceEvents::Never);
        if let Some(h) = hwnd(&window) {
            rider_core::dpi::follow_windows(h); // no shaking when dragged across monitors
        }
        self.keyhook = hwnd(&window).and_then(|h| KeyHook::install(h, self.net.ctrl.clone()));
        if self.keyhook.is_none() {
            tracing::warn!("Win key and Alt+Tab will act on this computer (keyboard hook unavailable)");
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
                self.redraw_soon();
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
            WindowEvent::Focused(false) => {
                self.release_keys();
                if let Some(h) = &self.keyhook {
                    h.release();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.toolbar.pointer(position.x, position.y) {
                    self.redraw_soon();
                }
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
                // Clicks on the toolbar stay here.
                if b == MouseButton::Left {
                    if state == ElementState::Pressed && self.toolbar.press() {
                        return;
                    }
                    if state == ElementState::Released {
                        let (taken, action) = self.toolbar.release();
                        if let Some(a) = action {
                            self.act(el, a);
                        }
                        if taken {
                            return;
                        }
                    }
                }
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
            WindowEvent::CursorLeft { .. } => {
                if self.toolbar.leave() {
                    self.redraw_soon();
                }
            }
            WindowEvent::DroppedFile(path) => self.net.send_file(path),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if Instant::now() >= self.next_title {
            self.update_title();
            self.next_title = Instant::now() + Duration::from_secs(1);
            if let Some(d) = &mut self.diag {
                d.tick(&self.net.conn, &self.shared);
            }
            if self.refresh_extras() {
                self.redraw_soon();
            }
        }
        // Hide (or show) the toolbar on time even when no frames arrive.
        if self.toolbar.visible() != self.toolbar_shown {
            self.redraw_soon();
        }
        let wake = self.toolbar.deadline().map_or(self.next_title, |t| t.min(self.next_title));
        el.set_control_flow(ControlFlow::WaitUntil(wake));
    }
}

/// Session time as "m:ss", or "h:mm:ss" from an hour on.
fn elapsed(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

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
//!
//! When the host goes silent the picture dims under "Connection lost,
//! reconnecting" (see `overlay`). QUIC keeps retrying the same connection;
//! if it dies, a new one is made with `Reconnect` until `RECONNECT_FOR` runs out.

use crate::diag::{self, Diag};
#[cfg(windows)]
use crate::keyhook::KeyHook;
use crate::net::{self, NetHandle, Session, Shared, UiEvent};
use crate::overlay::Overlay;
use crate::toolbar::{Action, Extras, Toolbar};
use anyhow::Result;
use rider_core::i18n::tr;
use rider_core::pixels::{Nv12Frame, nv12_to_xrgb_fit};
use rider_core::proto::{Access, ClientMsg, CursorShape, MonitorInfo, MouseButton, Permissions, QUALITIES};
use std::collections::HashSet;
use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
#[cfg(windows)]
use winit::platform::scancode::PhysicalKeyExtScancode;
#[cfg(windows)]
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{CursorIcon, Fullscreen, Icon, Window, WindowId};

/// Silence from the host (it answers a ping every second) that counts as lost.
const SILENT: Duration = Duration::from_secs(3);
/// How long to keep making new connections after one died.
const RECONNECT_FOR: Duration = Duration::from_secs(120);
/// One reconnect attempt at most (finding the host, the relay, the PIN).
const ATTEMPT: Duration = Duration::from_secs(30);

/// Makes a new authenticated session with the same host.
pub type Reconnect = Box<dyn Fn() -> Pin<Box<dyn Future<Output = Result<Session>> + Send>> + Send + Sync>;

/// `report_to`: where to send this session's connection diagnostics.
/// `reconnect`: how to get a new session if the connection dies (`None`:
/// the session ends with it, e.g. quick connect, which needs a new "Allow").
pub fn run(
    rt: tokio::runtime::Runtime,
    session: Session,
    host_name: String,
    report_to: Option<diag::Target>,
    reconnect: Option<Reconnect>,
) -> Result<()> {
    let event_loop = EventLoop::<UiEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let shared = Arc::new(Shared::default());
    let diag = report_to.map(|t| Diag::new(t, &session.conn, session.relayed, rt.handle().clone()));
    // Hosts that speak protocol 2 take the quality setting (and lock).
    let v2 = rider_core::tls::speaks_v2(&session.conn);
    let v3 = rider_core::tls::speaks_v3(&session.conn);
    let prefs = crate::config::ClientConfig::load().map(|c| c.viewer).unwrap_or_default();
    let net = net::start(rt.handle(), session, proxy.clone(), shared.clone());
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
        #[cfg(windows)]
        keyhook: None,
        diag,
        started: Instant::now(),
        v2,
        v3,
        quality,
        pointer: None,
        remote_cursor: CursorShape::Arrow,
        shown_cursor: CursorShape::Arrow,
        pinned: prefs.pin_toolbar,
        rt: rt.handle().clone(),
        proxy,
        reconnect: reconnect.map(Arc::new),
        reconnecting: None,
        overlay: Overlay::default(),
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

#[cfg(windows)]
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
    #[cfg(windows)]
    keyhook: Option<KeyHook>,
    diag: Option<Diag>,
    started: Instant,
    /// The host speaks protocol 2 (quality setting, lock).
    v2: bool,
    /// The host speaks protocol 3 (cursor shapes, clicks with position).
    v3: bool,
    /// Last pointer position sent (0..=65535 across the remote screen).
    pointer: Option<(u16, u16)>,
    /// The host pointer's shape, and the one this window shows.
    remote_cursor: CursorShape,
    shown_cursor: CursorShape,
    /// Max lines asked of the host, 0 = automatic.
    quality: u32,
    pinned: bool,
    rt: tokio::runtime::Handle,
    proxy: EventLoopProxy<UiEvent>,
    reconnect: Option<Arc<Reconnect>>,
    /// Why the last connection was lost, while new ones are being tried.
    reconnecting: Option<String>,
    overlay: Overlay,
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
        self.overlay.draw(&mut buf, w, h, window.scale_factor());
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
            "{} — {} · {fps} fps · RTT {:.0} ms · {} ~{:.0} ms · {} {:.1} ms · {}",
            self.host_name,
            elapsed(self.started.elapsed()),
            s.rtt_us.load(Ordering::Relaxed) as f64 / 1000.0,
            tr("latency", "gecikmə"),
            s.latency_us.load(Ordering::Relaxed) as f64 / 1000.0,
            tr("decode", "dekodlama"),
            s.decode_us.load(Ordering::Relaxed) as f64 / 1000.0,
            s.host_stats.lock().unwrap(),
        );
        if self.monitors.len() > 1 {
            title.push_str(&format!(" · monitor {}/{}", self.active_monitor + 1, self.monitors.len()));
        }
        // Not full access: always said, not only in the passing notice.
        if let Some(level) = access_label(self.shared.permissions()) {
            title = format!("{} [{level}]{}", self.host_name, &title[self.host_name.len()..]);
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
            Action::CtrlAltDel if !self.shared.permissions().control => {}
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
            Action::Elapsed | Action::Access => {}
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
        let label = QUALITIES
            .iter()
            .find(|q| q.0 == self.quality)
            .map_or(tr("Auto", "Avto"), |q| if q.0 == 0 { tr("Auto", "Avto") } else { q.1 });
        let p = self.shared.permissions();
        self.toolbar.set_extras(Extras {
            elapsed: elapsed(self.started.elapsed()),
            quality: self.v2.then(|| label.to_owned()),
            pinned: self.pinned,
            access: access_label(p),
            no_control: !p.control,
        })
    }

    /// The quality dropdown, under its toolbar button.
    #[cfg(windows)]
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
                    let text = if lines == 0 {
                        tr("Auto (adapts to the connection)", "Avto (bağlantıya uyğunlaşır)")
                    } else {
                        label
                    };
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
        self.set_quality(lines);
    }

    /// Without a native popup menu: each click picks the next quality.
    #[cfg(not(windows))]
    fn quality_menu(&mut self) {
        let i = QUALITIES.iter().position(|q| q.0 == self.quality).map_or(0, |i| (i + 1) % QUALITIES.len());
        self.set_quality(QUALITIES[i].0);
    }

    fn set_quality(&mut self, lines: u32) {
        if lines != self.quality {
            self.quality = lines;
            self.net.send(ClientMsg::SetQuality { max_height: lines });
            crate::config::update_viewer(|v| v.quality = lines);
            self.refresh_extras();
            self.redraw_soon();
        }
    }

    /// Lock the remote computer (what Win+L would do there).
    /// The person at the host changed what this side may do.
    fn permissions_changed(&mut self, p: Permissions) {
        if !p.control {
            // The host has let go of them already; nothing more to send.
            self.held_keys.clear();
        }
        #[cfg(windows)]
        if let Some(h) = &self.keyhook {
            h.set_enabled(p.control);
        }
        let text = match p.access() {
            Some(Access::Full) => {
                tr("The person at that computer allowed full access", "O kompüterdəki şəxs tam girişə icazə verdi")
            }
            Some(Access::Standard) => tr(
                "Standard access: control and clipboard, no files",
                "Standart giriş: idarəetmə və mübadilə buferi, fayllar olmadan",
            ),
            _ if !p.control => tr(
                "View only: the person at that computer has not allowed control",
                "Yalnız baxış: o kompüterdəki şəxs idarəetməyə icazə verməyib",
            ),
            _ => tr("The access level changed", "Giriş səviyyəsi dəyişdi"),
        };
        self.notice = Some((text.into(), Instant::now()));
        self.update_title();
        self.refresh_extras();
        self.redraw_soon();
    }

    fn lock_remote(&mut self) {
        if !self.shared.permissions().control {
            return;
        }
        self.release_keys();
        if self.v2 {
            self.net.send(ClientMsg::LockScreen);
        } else {
            self.notice = Some((
                tr(
                    "Locking needs a newer version on the remote computer",
                    "Kilidləmə üçün uzaq kompüterdə daha yeni versiya lazımdır",
                )
                .into(),
                Instant::now(),
            ));
            self.update_title();
        }
    }

    /// Show or update "Connection lost" while the host is silent or a new
    /// connection is being made; hide it once the host is heard again.
    fn update_overlay(&mut self) {
        let silent = self.shared.silent_for();
        let lines = (self.reconnecting.is_some() || silent >= SILENT).then(|| {
            (
                tr("Connection lost", "Bağlantı kəsildi").to_owned(),
                format!("{} {}", tr("Reconnecting…", "Yenidən qoşulur…"), elapsed(silent)),
            )
        });
        if self.overlay.set(lines) {
            self.redraw_soon();
        }
    }

    /// The connection died: try new ones in the background.
    fn start_reconnect(&mut self, reason: String) -> bool {
        let Some(reconnect) = self.reconnect.clone() else { return false };
        tracing::info!("{reason}; reconnecting");
        self.reconnecting = Some(reason.clone());
        self.held_keys.clear();
        let proxy = self.proxy.clone();
        self.rt.spawn(async move {
            let deadline = tokio::time::Instant::now() + RECONNECT_FOR;
            let mut wait = Duration::from_secs(1);
            let last = loop {
                let error = match tokio::time::timeout(ATTEMPT, reconnect()).await {
                    Ok(Ok(session)) => {
                        let _ = proxy.send_event(UiEvent::Reconnected(session));
                        return;
                    }
                    Ok(Err(e)) => format!("{e:#}"),
                    Err(_) => "timed out".to_owned(),
                };
                tracing::info!("reconnect: {error}");
                // A refused PIN or device will not get better; retrying could lock it out.
                let fatal = ["authentication failed", "host refused", "not paired"].iter().any(|m| error.contains(m));
                if fatal || tokio::time::Instant::now() + wait >= deadline {
                    break error;
                }
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(Duration::from_secs(5));
            };
            let _ = proxy.send_event(UiEvent::ReconnectFailed(format!("{reason}; could not reconnect: {last}")));
        });
        true
    }

    /// A new session after a lost one: same window, same settings.
    fn resume(&mut self, session: Session) {
        let lost = self.reconnecting.take().unwrap_or_default();
        if let Some(d) = &mut self.diag {
            d.restart(&session.conn, session.relayed, &format!("{lost}; reconnected"));
        }
        self.v2 = rider_core::tls::speaks_v2(&session.conn);
        self.v3 = rider_core::tls::speaks_v3(&session.conn);
        self.remote_cursor = CursorShape::Arrow;
        self.update_cursor();
        self.net = net::start(&self.rt, session, self.proxy.clone(), self.shared.clone());
        #[cfg(windows)]
        if let Some(h) = &self.keyhook {
            h.retarget(self.net.ctrl.clone());
        }
        if self.v2 && self.quality != 0 {
            self.net.send(ClientMsg::SetQuality { max_height: self.quality });
        }
        self.reconnecting = None;
        self.notice = Some((tr("Reconnected", "Yenidən qoşuldu").into(), Instant::now()));
        self.update_title();
        self.update_overlay();
        self.refresh_extras();
        self.redraw_soon();
    }

    /// The host's pointer shape, or an arrow over the toolbar.
    fn update_cursor(&mut self) {
        let want = if self.toolbar.hovering() { CursorShape::Arrow } else { self.remote_cursor };
        if want == self.shown_cursor {
            return;
        }
        self.shown_cursor = want;
        if let Some(w) = &self.window {
            w.set_cursor(cursor_icon(want));
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
            .with_title(format!("{} — {}", self.host_name, tr("connecting…", "qoşulur…")))
            .with_inner_size(LogicalSize::new(1280.0, 760.0))
            .with_window_icon(app_icon(32));
        #[cfg(windows)]
        let attrs = attrs.with_taskbar_icon(app_icon(64));
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
        #[cfg(windows)]
        {
            if let Some(h) = hwnd(&window) {
                rider_core::dpi::follow_windows(h); // no shaking when dragged across monitors
            }
            self.keyhook = hwnd(&window).and_then(|h| KeyHook::install(h, self.net.ctrl.clone()));
            if self.keyhook.is_none() {
                tracing::warn!("Win key and Alt+Tab will act on this computer (keyboard hook unavailable)");
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
                self.redraw_soon();
            }
            UiEvent::Cursor(shape) => {
                self.remote_cursor = shape;
                self.update_cursor();
            }
            UiEvent::Permissions(p) => self.permissions_changed(p),
            UiEvent::Notice(n) => {
                tracing::info!("{n}");
                self.notice = Some((n, Instant::now()));
                self.update_title();
            }
            // A connection replaced by a reconnect has nothing more to say.
            UiEvent::Closed { conn, .. } if conn != self.net.conn.stable_id() => {}
            UiEvent::Closed { reason, lost, .. } => {
                if !(lost && self.closed.is_none() && self.start_reconnect(reason.clone())) {
                    self.closed.get_or_insert(reason);
                    el.exit();
                }
            }
            UiEvent::Reconnected(session) => self.resume(session),
            UiEvent::ReconnectFailed(reason) => {
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
                #[cfg(windows)]
                if let Some(h) = &self.keyhook {
                    h.release();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.toolbar.pointer(position.x, position.y) {
                    self.redraw_soon();
                }
                self.update_cursor();
                let (ox, oy, w, h) = self.image_rect;
                if w > 1 && h > 1 {
                    let x = ((position.x - ox as f64) / (w - 1) as f64).clamp(0.0, 1.0);
                    let y = ((position.y - oy as f64) / (h - 1) as f64).clamp(0.0, 1.0);
                    let at = ((x * 65535.0) as u16, (y * 65535.0) as u16);
                    self.pointer = Some(at);
                    if self.shared.permissions().control {
                        self.net.mouse_move(at.0, at.1);
                    }
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
                if !self.shared.permissions().control {
                    return;
                }
                let down = state == ElementState::Pressed;
                self.net.send(match self.pointer.filter(|_| self.v3) {
                    Some((x, y)) => ClientMsg::MouseButtonAt { button: b, down, x, y, seq: self.net.mouse_seq() },
                    None => ClientMsg::MouseButton { button: b, down },
                });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => ((x * 120.0) as i32, (y * 120.0) as i32),
                    MouseScrollDelta::PixelDelta(p) => (p.x as i32, p.y as i32),
                };
                if self.shared.permissions().control {
                    self.net.send(ClientMsg::MouseWheel { dx, dy });
                }
            }
            WindowEvent::KeyboardInput { event, is_synthetic, .. } => {
                if is_synthetic {
                    return;
                }
                let down = event.state == ElementState::Pressed;
                if down && self.shortcut(el, &event) {
                    return;
                }
                if !self.shared.permissions().control {
                    return;
                }
                let Some((scancode, extended)) = host_scancode(event.physical_key) else { return };
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
                self.update_cursor();
            }
            WindowEvent::DroppedFile(path) => self.net.send_file(path, self.shared.permissions().files),
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
            self.update_overlay();
        }
        // Hide (or show) the toolbar on time even when no frames arrive.
        if self.toolbar.expire_confirm() || self.toolbar.visible() != self.toolbar_shown {
            self.redraw_soon();
        }
        let wake = self.toolbar.deadline().map_or(self.next_title, |t| t.min(self.next_title));
        el.set_control_flow(ControlFlow::WaitUntil(wake));
    }
}

/// The toolbar's access label: none for full access.
fn access_label(p: Permissions) -> Option<String> {
    match p.access() {
        Some(Access::Full) => None,
        Some(Access::Standard) => Some(tr("Standard access", "Standart giriş").into()),
        _ if !p.control => Some(tr("View only", "Yalnız baxış").into()),
        _ => Some(tr("Limited access", "Məhdud giriş").into()),
    }
}

/// The Windows scancode (and extended flag) the host expects for a key.
#[cfg(windows)]
fn host_scancode(key: PhysicalKey) -> Option<(u16, bool)> {
    let sc = key.to_scancode()?;
    Some(((sc & 0xFF) as u16, (sc & 0xFF00) == 0xE000))
}

#[cfg(not(windows))]
fn host_scancode(key: PhysicalKey) -> Option<(u16, bool)> {
    match key {
        PhysicalKey::Code(code) => crate::keymap::scancode(code),
        PhysicalKey::Unidentified(_) => None,
    }
}

fn cursor_icon(shape: CursorShape) -> CursorIcon {
    match shape {
        CursorShape::Arrow => CursorIcon::Default,
        CursorShape::Text => CursorIcon::Text,
        CursorShape::Wait => CursorIcon::Wait,
        CursorShape::Progress => CursorIcon::Progress,
        CursorShape::Crosshair => CursorIcon::Crosshair,
        CursorShape::Hand => CursorIcon::Pointer,
        CursorShape::ResizeWE => CursorIcon::EwResize,
        CursorShape::ResizeNS => CursorIcon::NsResize,
        CursorShape::ResizeNWSE => CursorIcon::NwseResize,
        CursorShape::ResizeNESW => CursorIcon::NeswResize,
        CursorShape::Move => CursorIcon::Move,
        CursorShape::NotAllowed => CursorIcon::NotAllowed,
        CursorShape::Help => CursorIcon::Help,
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

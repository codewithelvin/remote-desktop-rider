//! Viewer window: draws the newest frame, captures input, shows live stats.
//! A toolbar at the top centre (see `toolbar`) offers the actions below.
//!
//! The mouse cursor is drawn locally by Windows (the host does not render
//! it into the video), so pointer movement feels instant whatever the latency.
//!
//! Local shortcuts (not sent to the host):
//!   Alt+1 … Alt+9     show that remote monitor (when it has more than one)
//!   Ctrl+Alt+Shift+M  next monitor
//!   Ctrl+Alt+Shift+F  toggle fullscreen
//!   Ctrl+Alt+Shift+Q  disconnect
//!   Ctrl+Alt+Shift+C  open or close the chat
//!   Ctrl+Alt+Shift+L  lock the remote computer (Win+L always locks this one)
//!   Ctrl+Alt+End      Ctrl+Alt+Del on the host (login screen, lock screen)
//!   Win key combinations, Alt+Tab, Alt+Esc, Ctrl+Esc, Alt+F4 go to the host (keyhook)
//!   Drag and drop a file onto the window to send it to the host.
//!
//! Chat (hosts with protocol 5 and someone at that computer): the toolbar's
//! Chat button opens a panel at the right edge (see `chat`); the remote
//! screen fits into the space left of it. Files dropped on the window go
//! into the chat.
//!
//! When the host goes silent the picture dims under "Connection lost,
//! reconnecting" (see `overlay`). QUIC keeps retrying the same connection;
//! if it dies, a new one is made with `Reconnect` until `RECONNECT_FOR` runs out.

use crate::badge::Badge;
use crate::chat::ChatPanel;
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
use rider_core::trf;
use rider_ui::chat::Action as ChatAction;
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
        badge: Badge::default(),
        chat: None,
        chat_ready: false,
        scratch: Vec::new(),
        pointer_px: (-1.0, -1.0),
        hook_on: true,
    };
    app.refresh_extras();
    event_loop.run_app(&mut app)?;
    // Nothing of the chat outlives the window.
    app.chat = None;
    app.shared.end_files();
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
    /// The remote monitor shown, named in the corner (more than one).
    badge: Badge,
    /// Created with the window.
    chat: Option<ChatPanel>,
    /// Someone at the host sees the chat (`HostMsg::ChatReady`).
    chat_ready: bool,
    /// The remote screen, toolbar and overlay while the chat takes the
    /// right edge (copied next to it).
    scratch: Vec<u32>,
    /// Last pointer position in the window (physical px).
    pointer_px: (f64, f64),
    /// Whether the keyboard hook currently forwards keys.
    hook_on: bool,
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
        let panel = self.chat.as_ref().map_or(0, |c| c.width(w, window.scale_factor()));
        // The remote screen, overlay and toolbar fill the area left of the chat.
        let vw = w - panel;
        let mut scratch = std::mem::take(&mut self.scratch);
        let area: &mut [u32] = if panel == 0 {
            &mut buf
        } else {
            scratch.resize(vw * h, 0);
            &mut scratch
        };
        match &self.current {
            Some(f) => self.image_rect = nv12_to_xrgb_fit(f, area, vw, h),
            None => area.fill(0x0010_1014),
        }
        self.overlay.draw(area, vw, h, window.scale_factor());
        let monitors = (self.monitors.len() as u32, self.active_monitor);
        self.toolbar.set_state(window.scale_factor(), window.fullscreen().is_some(), monitors);
        let label = match self.monitors.get(self.active_monitor as usize) {
            Some(m) if self.monitors.len() > 1 => crate::toolbar::monitor_label(m.index, Some(&m.name)),
            _ => String::new(),
        };
        self.badge.set(label);
        self.badge.draw(area, vw, h, window.scale_factor(), self.pointer_px);
        self.toolbar.draw(area, vw, h);
        self.toolbar_shown = self.toolbar.visible();
        if panel > 0 {
            for y in 0..h {
                buf[y * w..y * w + vw].copy_from_slice(&scratch[y * vw..(y + 1) * vw]);
            }
            if let Some(c) = &mut self.chat {
                c.draw(window, &mut buf, w, h);
            }
        }
        self.scratch = scratch;
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
            let name = self.monitors.get(self.active_monitor as usize).map_or("", |m| m.name.as_str());
            let name = if name.is_empty() || name.starts_with("DISPLAY") { tr("monitor", "monitor") } else { name };
            title.push_str(&format!(" · {name} {}/{}", self.active_monitor + 1, self.monitors.len()));
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
        // Alt+1 … Alt+9: show that remote monitor (only those that exist, so
        // Alt+digit still reaches the host otherwise; not with AltGr, which
        // is Ctrl+Alt, nor the numpad, where Alt+digits types characters).
        if m.alt_key()
            && !m.control_key()
            && !m.shift_key()
            && !m.super_key()
            && let Some(n) = digit(key)
            && self.monitors.len() > 1
            && n <= self.monitors.len()
        {
            // The host already has Alt down: with nothing else pressed, its
            // release would open the menu bar of the program there. A Ctrl
            // tap in between makes it an Alt combination, like any other.
            if self.shared.permissions().control {
                for down in [true, false] {
                    self.net.send(ClientMsg::Key { scancode: 0x1D, extended: false, down });
                }
            }
            self.act(el, Action::Monitor(n as u32 - 1));
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
            PhysicalKey::Code(KeyCode::KeyC) if self.chat_available() => Action::Chat,
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
            Action::Chat => {
                if let Some(c) = &mut self.chat {
                    let open = !c.is_open();
                    c.set_open(open);
                }
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
        let chat = self.chat.as_ref().filter(|_| self.chat_available());
        self.toolbar.set_extras(Extras {
            elapsed: elapsed(self.started.elapsed()),
            quality: self.v2.then(|| label.to_owned()),
            pinned: self.pinned,
            access: access_label(p),
            no_control: !p.control,
            chat: chat.map(|c| c.chat.unread),
            chat_open: chat.is_some_and(ChatPanel::is_open),
            monitor_names: self.monitors.iter().map(|m| m.name.clone()).collect(),
        })
    }

    /// The host speaks protocol 5 and someone there sees the chat.
    fn chat_available(&self) -> bool {
        self.chat_ready && self.net.chat()
    }

    /// Carry out what was done in the chat panel.
    fn chat_actions(&mut self) {
        let Some(c) = &mut self.chat else { return };
        let actions = c.take_actions();
        for a in actions {
            match a {
                ChatAction::Send(text) => {
                    if let Some(c) = &mut self.chat {
                        c.chat.text(true, &text);
                        c.changed();
                    }
                    self.net.send(ClientMsg::Chat { text });
                }
                ChatAction::Attach => {
                    let proxy = self.proxy.clone();
                    std::thread::spawn(move || {
                        let files = rider_ui::files::pick_files();
                        if !files.is_empty() {
                            let _ = proxy.send_event(UiEvent::Attach(files));
                        }
                    });
                }
                ChatAction::Cancel(id) => self.shared.cancel(id),
                ChatAction::Open(id) => {
                    if let Some(path) = self.chat.as_ref().and_then(|c| c.file_path(id))
                        && let Err(e) = rider_ui::files::open(path)
                    {
                        self.show_notice(format!("{}: {e}", path.display()));
                    }
                }
                ChatAction::SaveAs(id) => {
                    let Some(c) = &self.chat else { continue };
                    let (Some(path), Some(f)) = (c.file_path(id).cloned(), c.chat.file(id)) else { continue };
                    let (name, proxy) = (f.name.clone(), self.proxy.clone());
                    std::thread::spawn(move || {
                        let msg = match rider_ui::files::save_copy(&path, &name) {
                            Ok(Some(to)) => format!("{} {}", tr("Saved to", "Saxlanıldı:"), to.display()),
                            Ok(None) => return,
                            Err(e) => format!("{name}: {e}"),
                        };
                        let _ = proxy.send_event(UiEvent::Notice(msg));
                    });
                }
                ChatAction::Close => {
                    if let Some(c) = &mut self.chat {
                        c.set_open(false);
                    }
                    self.refresh_extras();
                }
            }
        }
        self.redraw_soon();
    }

    fn show_notice(&mut self, text: String) {
        self.notice = Some((text, Instant::now()));
        self.update_title();
    }

    /// Something arrived for the chat; with the panel closed, say so.
    fn chat_event(&mut self, notice: Option<String>) {
        let Some(c) = &mut self.chat else { return };
        c.changed();
        let open = c.is_open();
        if !open && let Some(n) = notice {
            self.show_notice(n);
            self.toolbar.reveal();
        }
        self.refresh_extras();
        self.redraw_soon();
    }

    /// Keys go to the chat while its message box has focus: the keyboard
    /// hook must not send Win or Alt+Tab to the host then.
    fn update_hook(&mut self) {
        let typing = self.chat.as_ref().is_some_and(ChatPanel::typing);
        let on = self.shared.permissions().control && !typing;
        if on == self.hook_on {
            return;
        }
        self.hook_on = on;
        if typing {
            self.release_keys();
        }
        #[cfg(windows)]
        if let Some(h) = &self.keyhook {
            h.set_enabled(on);
        }
    }

    /// Whether the pointer is over the chat panel.
    fn over_chat(&self, x: f64) -> bool {
        let (Some(c), Some(w)) = (&self.chat, &self.window) else { return false };
        c.contains(x, w.inner_size().width as usize, w.scale_factor())
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
        self.hook_on = !p.control; // makes update_hook apply the new state
        self.update_hook();
        if let Some(c) = &mut self.chat {
            c.chat.files_allowed = p.files;
            c.changed();
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

    /// The host's pointer shape, an arrow over the toolbar, or the chat's.
    fn update_cursor(&mut self) {
        if self.over_chat(self.pointer_px.0) {
            if let (Some(c), Some(w)) = (&self.chat, &self.window) {
                w.set_cursor(c.cursor());
            }
            // Set again when the pointer goes back to the remote screen.
            self.shown_cursor = CursorShape::NotAllowed;
            if self.remote_cursor == CursorShape::NotAllowed {
                self.shown_cursor = CursorShape::Arrow;
            }
            return;
        }
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
        let mut chat = ChatPanel::new(&window, &self.host_name);
        chat.ready = self.chat_ready;
        chat.chat.files_allowed = self.shared.permissions().files;
        self.chat = Some(chat);
        self.window = Some(window);
        self.refresh_extras();
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
            UiEvent::Chat(text) => {
                let preview: String = text.chars().take(60).collect();
                if let Some(c) = &mut self.chat {
                    c.chat.text(false, &text);
                }
                let notice = format!("{}: {preview}", self.host_name);
                self.chat_event(Some(notice));
            }
            UiEvent::ChatReady(ready) => {
                self.chat_ready = ready;
                if let Some(c) = &mut self.chat {
                    c.ready = ready;
                    if !ready && c.is_open() {
                        c.chat.notice(tr(
                            "Nobody at that computer has the chat open now",
                            "O kompüterdə indi söhbət açıq deyil",
                        ));
                    }
                }
                self.chat_event(None);
            }
            UiEvent::File { id, mine, name, size } => {
                let notice = (!mine).then(|| trf!("{} sent {name}", "{} {name} göndərdi", self.host_name));
                if let Some(c) = &mut self.chat {
                    c.chat.file_started(id, mine, &name, size);
                }
                self.chat_event(notice);
            }
            UiEvent::FileProgress { id, done } => {
                if let Some(c) = &mut self.chat {
                    c.chat.file_progress(id, done);
                    if c.is_open() {
                        c.changed();
                        self.redraw_soon();
                    }
                }
            }
            UiEvent::FileDone { id, path } => {
                let name = self.chat.as_ref().and_then(|c| c.chat.file(id)).map(|f| f.name.clone());
                if let Some(c) = &mut self.chat {
                    c.chat.file_done(id);
                    if let Some(p) = path {
                        c.set_file_path(id, p);
                    }
                }
                // Without the chat (older host) the title says how it went.
                let notice =
                    (!self.chat_available()).then(|| trf!("{} sent", "{} göndərildi", name.unwrap_or_default()));
                self.chat_event(notice);
            }
            UiEvent::FileFailed { id, reason } => {
                let name = self.chat.as_ref().and_then(|c| c.chat.file(id)).map(|f| f.name.clone());
                if let Some(c) = &mut self.chat {
                    c.chat.file_failed(id, &reason);
                }
                self.chat_event(Some(format!("{}: {reason}", name.unwrap_or_default())));
            }
            UiEvent::Attach(files) => {
                for f in files {
                    self.net.send_file(f, &self.shared);
                }
            }
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
        if let WindowEvent::CursorMoved { position, .. } = &event {
            let (was, now) =
                (self.badge.under(self.pointer_px.0, self.pointer_px.1), self.badge.under(position.x, position.y));
            self.pointer_px = (position.x, position.y);
            if was != now {
                self.redraw_soon(); // the corner label steps aside, or comes back
            }
        }
        let to_chat = match &event {
            WindowEvent::CursorMoved { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } => {
                self.over_chat(self.pointer_px.0)
            }
            WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_) => {
                self.chat.as_ref().is_some_and(ChatPanel::typing)
            }
            _ => false,
        };
        if let (Some(c), Some(w)) = (&mut self.chat, &self.window) {
            // Pointer events always (egui sees the pointer leave, and a click
            // on the remote screen takes the focus from the message box).
            if c.on_event(w, &event) {
                w.request_redraw();
            }
        }
        if to_chat {
            if matches!(event, WindowEvent::CursorMoved { .. }) {
                if self.toolbar.leave() {
                    self.redraw_soon();
                }
                self.update_cursor();
            }
            self.update_hook();
            return;
        }
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
            WindowEvent::DroppedFile(path) => {
                // Into the chat, where its progress shows.
                if self.chat_available()
                    && let Some(c) = &mut self.chat
                {
                    c.set_open(true);
                    self.refresh_extras();
                    self.redraw_soon();
                }
                self.net.send_file(path, &self.shared);
            }
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
        self.chat_actions();
        self.update_hook();
        if self.chat.as_ref().is_some_and(ChatPanel::due) {
            self.redraw_soon();
        }
        let mut wake = self.toolbar.deadline().map_or(self.next_title, |t| t.min(self.next_title));
        if let Some(t) = self.chat.as_ref().and_then(ChatPanel::deadline) {
            wake = wake.min(t);
        }
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

/// The digit of a top-row number key (1-9).
fn digit(key: PhysicalKey) -> Option<usize> {
    let keys = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    keys.iter().position(|&k| key == PhysicalKey::Code(k)).map(|i| i + 1)
}

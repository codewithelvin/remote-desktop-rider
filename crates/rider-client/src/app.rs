use crate::badge::Badge;
use crate::chat::{ChatPanel, Page};
use crate::diag::{self, Diag};
#[cfg(windows)]
use crate::keyhook::KeyHook;
use crate::net::{self, NetHandle, Session, Shared, UiEvent};
use crate::overlay::Overlay;
use crate::toolbar::{Action, Extras, Toolbar};
use anyhow::Result;
use rider_core::i18n::tr;
use rider_core::pixels::{Nv12Frame, nv12_to_xrgb_fit, xrgb_fit};
use rider_core::proto::{Access, ClientMsg, CursorShape, MonitorInfo, MouseButton, Permissions, QUALITIES};
use rider_core::trf;
use rider_ui::chat::Action as ChatAction;
use std::collections::HashSet;
use std::fmt::Write as _;
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

const SILENT: Duration = Duration::from_secs(3);
const RECONNECT_FOR: Duration = Duration::from_secs(120);
const RESTART_RECONNECT_FOR: Duration = Duration::from_secs(600);
const ATTEMPT: Duration = Duration::from_secs(30);

pub type Reconnect = Box<dyn Fn() -> Pin<Box<dyn Future<Output = Result<Session>> + Send>> + Send + Sync>;

pub fn run(
    rt: tokio::runtime::Runtime,
    session: Session,
    host_name: String,
    report_to: Option<diag::Target>,
    reconnect: Option<Reconnect>,
    forwards: Vec<crate::forward::Spec>,
) -> Result<()> {
    let event_loop = EventLoop::<UiEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let shared = Arc::new(Shared::default());
    let diag = report_to.map(|t| Diag::new(t, &session.conn, session.relayed, rt.handle().clone()));
    let v2 = rider_core::tls::speaks_v2(&session.conn);
    let v3 = rider_core::tls::speaks_v3(&session.conn);
    let v9 = rider_core::tls::speaks_v9(&session.conn);
    let prefs = crate::config::ClientConfig::load().map_or_default(|c| c.viewer);
    let net = net::start(rt.handle(), session, proxy.clone(), &shared);
    let forwards = start_forwards(&rt, forwards, &shared, &proxy, &net.conn)?;
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
        v9,
        quality,
        pointer: None,
        remote_cursor: CursorShape::Arrow,
        shown_cursor: CursorShape::Arrow,
        pinned: prefs.pin_toolbar,
        rt: rt.handle().clone(),
        proxy,
        reconnect: reconnect.map(Arc::new),
        reconnecting: None,
        restarting: false,
        overlay: Overlay::default(),
        badge: Badge::default(),
        chat: None,
        chat_ready: false,
        host_tunnels: false,
        scratch: Vec::new(),
        sharp_buf: Vec::new(),
        pointer_px: (-1.0, -1.0),
        hook_on: true,
        forwards,
    };
    app.refresh_extras();
    event_loop.run_app(&mut app)?;
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
    toolbar_shown: bool,
    #[cfg(windows)]
    keyhook: Option<KeyHook>,
    diag: Option<Diag>,
    started: Instant,
    v2: bool,
    v3: bool,
    v9: bool,
    restarting: bool,
    pointer: Option<(u16, u16)>,
    remote_cursor: CursorShape,
    shown_cursor: CursorShape,
    quality: u32,
    pinned: bool,
    rt: tokio::runtime::Handle,
    proxy: EventLoopProxy<UiEvent>,
    reconnect: Option<Arc<Reconnect>>,
    reconnecting: Option<String>,
    overlay: Overlay,
    badge: Badge,
    chat: Option<ChatPanel>,
    chat_ready: bool,
    host_tunnels: bool,
    scratch: Vec<u32>,
    sharp_buf: Vec<u32>,
    pointer_px: (f64, f64),
    hook_on: bool,
    forwards: Vec<crate::forward::Listening>,
}

fn start_forwards(
    rt: &tokio::runtime::Runtime,
    specs: Vec<crate::forward::Spec>,
    shared: &Arc<Shared>,
    proxy: &EventLoopProxy<UiEvent>,
    conn: &quinn::Connection,
) -> Result<Vec<crate::forward::Listening>> {
    if !specs.is_empty() && !rider_core::tls::speaks_v7(conn) {
        eprintln!(
            "{}",
            tr(
                "The other computer runs an older version without TCP tunnels: update it.",
                "Digər kompüterdə TCP tunelləri olmayan köhnə versiya var: onu yeniləyin."
            )
        );
    }
    let mut open = Vec::with_capacity(specs.len());
    for spec in specs {
        let l = rt.block_on(crate::forward::listen(spec, shared.clone(), proxy.clone()))?;
        println!("{}", rider_core::trf!("Tunnel: {} -> {}", "Tunel: {} -> {}", l.local, l.target));
        open.push(l);
    }
    Ok(open)
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
        let vw = w - panel;
        let mut scratch = std::mem::take(&mut self.scratch);
        let area: &mut [u32] = if panel == 0 {
            &mut buf
        } else {
            scratch.resize(vw * h, 0);
            &mut scratch
        };
        match &self.current {
            Some(f) => self.image_rect = draw_frame(f, &self.shared, &mut self.sharp_buf, area, vw, h),
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
            let _ = write!(title, " · {name} {}/{}", self.active_monitor + 1, self.monitors.len());
        }
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

    fn shortcut(&mut self, el: &ActiveEventLoop, event: &KeyEvent) -> bool {
        let key = event.physical_key;
        let m = self.modifiers;
        let end = key == PhysicalKey::Code(KeyCode::End) || event.logical_key == Key::Named(NamedKey::End);
        if m.control_key() && m.alt_key() && end {
            self.act(el, Action::CtrlAltDel);
            return true;
        }
        if m.alt_key()
            && !m.control_key()
            && !m.shift_key()
            && !m.super_key()
            && let Some(n) = digit(key)
            && self.monitors.len() > 1
            && n <= self.monitors.len()
        {
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
            Action::Chat => self.toggle_page(Page::Chat),
            Action::Tunnels => self.toggle_page(Page::Tunnels),
            Action::Elapsed | Action::Access => {}
            Action::Restart => self.restart_remote(),
            Action::Disconnect => {
                self.release_keys();
                self.closed = Some("disconnected by you".into());
                self.net.conn.close(0u32.into(), b"bye");
                el.exit();
            }
        }
    }

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
            chat_open: chat.is_some_and(|c| c.is_open() && c.page() == Page::Chat),
            tunnels: self.tunnels_available().then_some(self.forwards.len()),
            tunnels_open: self.chat.as_ref().is_some_and(|c| c.is_open() && c.page() == Page::Tunnels),
            restart: self.restart_available(),
            monitor_names: self.monitors.iter().map(|m| m.name.clone()).collect(),
        })
    }

    fn host_tunnels_changed(&mut self, on: bool) {
        self.host_tunnels = on;
        self.refresh_extras();
        self.redraw_soon();
    }

    fn tunnels_available(&self) -> bool {
        let conn = &self.net.conn;
        let host_allows =
            if rider_core::tls::speaks_v8(conn) { self.host_tunnels } else { rider_core::tls::speaks_v7(conn) };
        host_allows && self.shared.permissions().is_full()
    }

    fn toggle_page(&mut self, page: Page) {
        if let Some(c) = &mut self.chat {
            let open = !(c.is_open() && c.page() == page);
            c.show_page(page);
            c.set_open(open);
        }
        self.refresh_extras();
        self.redraw_soon();
    }

    fn tunnel_actions(&mut self) {
        let Some(c) = &mut self.chat else { return };
        let actions = c.take_tunnel_actions();
        if actions.is_empty() {
            return;
        }
        for a in actions {
            match a {
                rider_ui::tunnels::Action::Add(text) => self.open_tunnel(&text),
                rider_ui::tunnels::Action::Close(i) if i < self.forwards.len() => {
                    self.forwards.remove(i);
                }
                rider_ui::tunnels::Action::Close(_) => {}
                rider_ui::tunnels::Action::Hide => {
                    if let Some(c) = &mut self.chat {
                        c.set_open(false);
                    }
                }
            }
        }
        self.sync_tunnels();
        self.refresh_extras();
        self.redraw_soon();
    }

    fn open_tunnel(&mut self, text: &str) {
        let opened = text
            .parse::<crate::forward::Spec>()
            .and_then(|spec| self.rt.block_on(crate::forward::listen(spec, self.shared.clone(), self.proxy.clone())));
        let error = match opened {
            Ok(l) => {
                self.forwards.push(l);
                None
            }
            Err(e) => Some(format!("{e:#}")),
        };
        if let Some(c) = &mut self.chat {
            c.tunnels.error = error;
        }
    }

    fn sync_tunnels(&mut self) {
        if let Some(c) = &mut self.chat {
            c.tunnels.open = self
                .forwards
                .iter()
                .map(|f| rider_ui::tunnels::Open { local: f.local.to_string(), target: f.target.clone() })
                .collect();
            c.changed();
        }
    }

    fn chat_available(&self) -> bool {
        self.chat_ready && self.net.chat()
    }

    fn chat_actions(&mut self) {
        let Some(c) = &mut self.chat else { return };
        let actions = c.take_actions();
        if actions.is_empty() {
            return;
        }
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

    fn over_chat(&self, x: f64) -> bool {
        let (Some(c), Some(w)) = (&self.chat, &self.window) else { return false };
        c.contains(x, w.inner_size().width as usize, w.scale_factor())
    }

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
        let cmd = unsafe {
            let _ = ClientToScreen(h, &raw mut pt);
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

    fn permissions_changed(&mut self, p: Permissions) {
        if !p.control {
            self.held_keys.clear();
        }
        self.hook_on = !p.control;
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

    fn restart_available(&self) -> bool {
        self.v9 && self.shared.permissions().is_full() && !self.restarting
    }

    fn chat_received(&mut self, text: &str) {
        let preview: String = text.chars().take(60).collect();
        if let Some(c) = &mut self.chat {
            c.chat.text(false, text);
        }
        let notice = format!("{}: {preview}", self.host_name);
        self.chat_event(Some(notice));
    }

    fn restart_remote(&mut self) {
        if !self.restart_available() {
            return;
        }
        self.release_keys();
        self.net.send(ClientMsg::Restart);
    }

    fn restart_event(&mut self, announced: Option<u32>) {
        self.restarting = announced.is_some();
        let text = match announced {
            Some(0) => tr("The remote computer is restarting", "Uzaq kompüter yenidən başlayır").to_owned(),
            Some(secs) => rider_core::trf!(
                "The remote computer restarts in {secs} s unless someone there cancels",
                "Orada kimsə ləğv etməsə, uzaq kompüter {secs} san. sonra yenidən başlayacaq"
            ),
            None => {
                tr("The restart was cancelled on the remote computer", "Yenidən başlatma uzaq kompüterdə ləğv edildi")
                    .to_owned()
            }
        };
        self.notice = Some((text, Instant::now()));
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

    fn update_overlay(&mut self) {
        let silent = self.shared.silent_for();
        let lines = (self.reconnecting.is_some() || silent >= SILENT).then(|| {
            let title = if self.restarting {
                tr("The remote computer is restarting", "Uzaq kompüter yenidən başlayır")
            } else {
                tr("Connection lost", "Bağlantı kəsildi")
            };
            (title.to_owned(), format!("{} {}", tr("Reconnecting…", "Yenidən qoşulur…"), elapsed(silent)))
        });
        if self.overlay.set(lines) {
            self.redraw_soon();
        }
    }

    fn start_reconnect(&mut self, reason: String) -> bool {
        let Some(reconnect) = self.reconnect.clone() else { return false };
        tracing::info!("{reason}; reconnecting");
        self.reconnecting = Some(reason.clone());
        self.held_keys.clear();
        let proxy = self.proxy.clone();
        let patience = if self.restarting { RESTART_RECONNECT_FOR } else { RECONNECT_FOR };
        self.rt.spawn(async move {
            let deadline = tokio::time::Instant::now() + patience;
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

    fn resume(&mut self, session: Session) {
        let lost = self.reconnecting.take().unwrap_or_default();
        tracing::info!("reconnected after: {lost}");
        if let Some(d) = &mut self.diag {
            d.restart(&session.conn, session.relayed, &format!("{lost}; reconnected"));
        }
        self.v2 = rider_core::tls::speaks_v2(&session.conn);
        self.v3 = rider_core::tls::speaks_v3(&session.conn);
        self.v9 = rider_core::tls::speaks_v9(&session.conn);
        self.restarting = false;
        self.remote_cursor = CursorShape::Arrow;
        self.update_cursor();
        self.net = net::start(&self.rt, session, self.proxy.clone(), &self.shared);
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

    fn update_cursor(&mut self) {
        if self.over_chat(self.pointer_px.0) {
            if let (Some(c), Some(w)) = (&self.chat, &self.window) {
                w.set_cursor(c.cursor());
            }
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

    fn show_frame(&mut self) {
        if let Some(f) = self.shared.latest.lock().unwrap().take() {
            self.current = Some(f);
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }

    fn redraw_soon(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

impl App {
    fn route_to_chat(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::CursorMoved { position, .. } = event {
            let (was, now) =
                (self.badge.under(self.pointer_px.0, self.pointer_px.1), self.badge.under(position.x, position.y));
            self.pointer_px = (position.x, position.y);
            if was != now {
                self.redraw_soon();
            }
        }
        let to_chat = match event {
            WindowEvent::CursorMoved { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } => {
                self.over_chat(self.pointer_px.0)
            }
            WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_) => {
                self.chat.as_ref().is_some_and(ChatPanel::typing)
            }
            _ => false,
        };
        if let (Some(c), Some(w)) = (&mut self.chat, &self.window)
            && c.on_event(w, event)
        {
            w.request_redraw();
        }
        if to_chat {
            if matches!(event, WindowEvent::CursorMoved { .. }) {
                if self.toolbar.leave() {
                    self.redraw_soon();
                }
                self.update_cursor();
            }
            self.update_hook();
        }
        to_chat
    }

    fn pointer_moved(&mut self, px: f64, py: f64) {
        if self.toolbar.pointer(px, py) {
            self.redraw_soon();
        }
        self.update_cursor();
        let (ox, oy, w, h) = self.image_rect;
        if w > 1 && h > 1 {
            let x = ((px - ox as f64) / (w - 1) as f64).clamp(0.0, 1.0);
            let y = ((py - oy as f64) / (h - 1) as f64).clamp(0.0, 1.0);
            let at = ((x * 65535.0) as u16, (y * 65535.0) as u16);
            self.pointer = Some(at);
            if self.shared.permissions().control {
                self.net.mouse_move(at.0, at.1);
            }
        }
    }

    fn toolbar_takes_click(&mut self, el: &ActiveEventLoop, state: ElementState) -> bool {
        if state == ElementState::Pressed {
            return self.toolbar.press();
        }
        let (taken, action) = self.toolbar.release();
        if let Some(a) = action {
            self.act(el, a);
        }
        taken
    }

    fn mouse_input(&mut self, el: &ActiveEventLoop, state: ElementState, button: winit::event::MouseButton) {
        let Some(b) = remote_button(button) else { return };
        if b == MouseButton::Left && self.toolbar_takes_click(el, state) {
            return;
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

    fn mouse_wheel(&self, delta: MouseScrollDelta) {
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => ((x * 120.0) as i32, (y * 120.0) as i32),
            MouseScrollDelta::PixelDelta(p) => (p.x as i32, p.y as i32),
        };
        if self.shared.permissions().control {
            self.net.send(ClientMsg::MouseWheel { dx, dy });
        }
    }

    fn key_input(&mut self, el: &ActiveEventLoop, event: &KeyEvent) {
        let down = event.state == ElementState::Pressed;
        if down && self.shortcut(el, event) {
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

    fn dropped_file(&mut self, path: std::path::PathBuf) {
        if self.chat_available()
            && let Some(c) = &mut self.chat
        {
            c.set_open(true);
            self.refresh_extras();
            self.redraw_soon();
        }
        self.net.send_file(path, &self.shared);
    }
}

fn remote_button(button: winit::event::MouseButton) -> Option<MouseButton> {
    use winit::event::MouseButton as W;
    Some(match button {
        W::Left => MouseButton::Left,
        W::Right => MouseButton::Right,
        W::Middle => MouseButton::Middle,
        W::Back => MouseButton::Back,
        W::Forward => MouseButton::Forward,
        W::Other(_) => return None,
    })
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
        el.listen_device_events(winit::event_loop::DeviceEvents::Never);
        #[cfg(windows)]
        {
            if let Some(h) = hwnd(&window) {
                rider_core::dpi::follow_windows(h);
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
        self.sync_tunnels();
        self.window = Some(window);
        self.refresh_extras();
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: UiEvent) {
        match ev {
            UiEvent::NewFrame => self.show_frame(),
            UiEvent::Sharpened => {
                if let Some(w) = &self.window {
                    w.request_redraw();
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
            UiEvent::Chat(text) => self.chat_received(&text),
            UiEvent::Tunnels(on) => self.host_tunnels_changed(on),
            UiEvent::Restarting(seconds) => self.restart_event(Some(seconds)),
            UiEvent::RestartCancelled => self.restart_event(None),
            UiEvent::WakeAddresses(macs) => remember_wake(self.host_name.clone(), macs),
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
            UiEvent::Closed { conn, .. } if conn != self.net.conn.stable_id() => {}
            UiEvent::Closed { reason, lost, .. } => {
                let lost = lost || self.restarting;
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
        if self.route_to_chat(&event) {
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
            WindowEvent::CursorMoved { position, .. } => self.pointer_moved(position.x, position.y),
            WindowEvent::MouseInput { state, button, .. } => self.mouse_input(el, state, button),
            WindowEvent::MouseWheel { delta, .. } => self.mouse_wheel(delta),
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => self.key_input(el, &event),
            WindowEvent::CursorLeft { .. } => {
                if self.toolbar.leave() {
                    self.redraw_soon();
                }
                self.update_cursor();
            }
            WindowEvent::DroppedFile(path) => self.dropped_file(path),
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
        if self.toolbar.expire_confirm() || self.toolbar.visible() != self.toolbar_shown {
            self.redraw_soon();
        }
        self.chat_actions();
        self.tunnel_actions();
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

fn draw_frame(
    f: &Nv12Frame,
    shared: &Shared,
    full: &mut Vec<u32>,
    area: &mut [u32],
    vw: usize,
    h: usize,
) -> (usize, usize, usize, usize) {
    let overlay = shared.sharp.lock().unwrap();
    if !overlay.covers(f.width, f.height) {
        return nv12_to_xrgb_fit(f, area, vw, h);
    }
    full.resize(f.width * f.height, 0);
    nv12_to_xrgb_fit(f, full, f.width, f.height);
    overlay.paint(full);
    drop(overlay);
    xrgb_fit(full, f.width, f.height, area, vw, h)
}

fn access_label(p: Permissions) -> Option<String> {
    match p.access() {
        Some(Access::Full) => None,
        Some(Access::Standard) => Some(tr("Standard access", "Standart giriş").into()),
        _ if !p.control => Some(tr("View only", "Yalnız baxış").into()),
        _ => Some(tr("Limited access", "Məhdud giriş").into()),
    }
}

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

fn elapsed(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

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

fn remember_wake(host: String, macs: Vec<rider_core::wake::Mac>) {
    std::thread::spawn(move || {
        let saved = crate::config::ClientConfig::load().and_then(|mut cfg| {
            if cfg.remember_wake(&host, &macs) {
                cfg.save()?;
            }
            Ok(())
        });
        if let Err(e) = saved {
            tracing::warn!("cannot save the wake addresses of {host}: {e:#}");
        }
    });
}

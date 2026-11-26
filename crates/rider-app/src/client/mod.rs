mod dialogs;
mod pages;
mod session;

use crate::pins;
use crate::sys::{self, RemoteAccess};
use crate::tray::{self, Tray};
use crate::updates::{self, Updater};
use crate::widgets::{self, ACCENT, AMBER, Bus, GREEN, GREY, Tx, icon};
use eframe::egui::{self, Align, Key, Layout, RichText, ScrollArea, TextEdit, TextureHandle, Ui, vec2};
use rider_client::PairPrompt;
use rider_client::config::{ClientConfig, KnownHost};
use rider_core::i18n::tr;
use rider_core::identity::{Identity, fp_from_hex};
use rider_core::proto::Access;
use rider_core::trf;
use rider_ui::chat::{Action as ChatAction, Chat};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

fn config_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(ClientConfig::path().ok()?).ok()?.modified().ok()
}

const POLL_EVERY: Duration = Duration::from_secs(15);
const RELEASES: &str = "https://github.com/codewithelvin/remote-desktop-rider/releases/latest";

fn host_side() -> bool {
    cfg!(windows) && std::env::var_os("RIDER_PORTABLE_UI").is_none()
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Computers,
    ThisComputer,
    Enterprise,
    Settings,
}

enum Msg {
    Online(Result<Vec<(String, bool)>, String>),
    PairPrompt(PairPrompt, oneshot::Sender<bool>),
    PairDone(Result<KnownHost, String>),
    Session { id: u64, update: SessionUpdate },
    Quick { id: u64, update: QuickUpdate },
    Support { id: u64, event: SupportEvent },
    SupportFiles { id: u64, files: Vec<std::path::PathBuf> },
    SupportNotice { id: u64, text: String },
}

enum SessionUpdate {
    Connected(String),
    Stderr(String),
    Exited(bool),
}

enum AddStage {
    Input { error: Option<String> },
    Connecting,
    Confirm { prompt: PairPrompt, answer: oneshot::Sender<bool> },
    Waiting { host: String },
    Done { name: String },
}

struct AddDialog {
    target: String,
    name: String,
    stage: AddStage,
}

struct ConnectDialog {
    fingerprint: String,
    name: String,
    pin: String,
    remember: bool,
    error: Option<String>,
}

struct Session {
    id: u64,
    host: String,
    connected: Option<String>,
    stderr: String,
    finished: Option<bool>,
    remember: Option<String>,
    saved_pin: bool,
}

enum QuickUpdate {
    Waiting(String),
    Code(String),
    Connected { host: String, route: String },
    Stderr(String),
    Exited,
}

#[derive(Clone)]
enum QuickStage {
    Input { error: Option<String> },
    Connecting,
    Waiting { host: String, code: Option<String> },
    Connected { host: String, route: String },
}

struct QuickDialog {
    id: u64,
    code: String,
    stage: QuickStage,
    stderr: String,
    child: Option<Arc<Mutex<Child>>>,
}

impl QuickDialog {
    fn stop(&mut self) {
        if let Some(c) = self.child.take() {
            let _ = c.lock().unwrap().kill();
        }
    }
}

enum SupportEvent {
    Code(Option<String>),
    ServerError(String),
    Request { device: String, code: String },
    Started(String),
    Ended,
    Cancelled,
    Declined,
    Expired,
    Exited(String),
    Chat(String),
    File { id: u64, mine: bool, name: String, size: u64 },
    FileProgress { id: u64, done: u64 },
    FileDone { id: u64, path: Option<std::path::PathBuf> },
    FileFailed { id: u64, reason: String },
    Tunnel(crate::tunnel_ask::TunnelEvent),
}

#[derive(Clone)]
enum SupportStage {
    Waiting,
    Asking { device: String, code: String },
    Active { device: String },
    Ended,
    Expired,
    Failed(String),
}

struct Support {
    id: u64,
    stdin: Option<ChildStdin>,
    access: Access,
    code: Option<String>,
    server_error: Option<String>,
    note: Option<String>,
    stage: SupportStage,
    chat: Chat,
    files: HashMap<u64, std::path::PathBuf>,
    tunnel: Option<crate::tunnel_ask::TunnelAsk>,
    tunnels_seen: HashSet<String>,
}

impl Support {
    fn new(id: u64, stdin: Option<ChildStdin>, stage: SupportStage) -> Self {
        let mut chat = Chat::new(tr("The person helping you", "Sizə kömək edən şəxs"));
        chat.closable = false;
        chat.heading = false;
        Self {
            id,
            stdin,
            access: Access::Full,
            code: None,
            server_error: None,
            note: None,
            stage,
            chat,
            files: HashMap::new(),
            tunnel: None,
            tunnels_seen: HashSet::new(),
        }
    }

    fn send(&mut self, line: &str) {
        if let Some(s) = &mut self.stdin {
            let _ = writeln!(s, "{line}");
        }
    }

    fn send_json(&mut self, word: &str, arg: &str) {
        let line = format!("{word} {}", serde_json::Value::from(arg));
        self.send(&line);
    }
}

pub struct ClientApp {
    rt: tokio::runtime::Runtime,
    bus: Bus<Msg>,
    tray: Option<Tray>,
    logo: Option<TextureHandle>,
    identity: Option<Arc<Identity>>,
    cfg: ClientConfig,
    fatal: Option<String>,
    tab: Tab,
    online: HashMap<String, bool>,
    polled: Option<Instant>,
    polling: bool,
    server_error: Option<String>,
    add: Option<AddDialog>,
    connect: Option<ConnectDialog>,
    quick: Option<QuickDialog>,
    support: Option<Support>,
    remove: Option<String>,
    saved_pins: HashSet<String>,
    sessions: Vec<Session>,
    next_id: u64,
    remote: RemoteAccess,
    remote_checked: Instant,
    manage_error: Option<String>,
    settings_msg: Option<Result<String, String>>,
    page_since: f64,
    cfg_mtime: Option<std::time::SystemTime>,
    autostart: bool,
    hide_in: u8,
    updater: Updater,
    server_edit: Option<(String, String)>,
    server_msg: Option<Result<String, String>>,
    server_check: Option<std::sync::mpsc::Receiver<(Result<rider_core::rendezvous::ServerRef, String>, bool)>>,
    enterprise: crate::enterprise::Enterprise,
}

impl ClientApp {
    pub fn new(ctx: &egui::Context, tray: Option<Tray>, hidden: bool) -> Self {
        let rt =
            tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime");
        let loaded = ClientConfig::load().and_then(|cfg| {
            let id = Identity::load_or_create(&ClientConfig::dir()?)?;
            Ok((cfg, id))
        });
        let (cfg, identity, fatal) = match loaded {
            Ok((cfg, id)) => (cfg, Some(Arc::new(id)), None),
            Err(e) => (
                ClientConfig::default(),
                None,
                Some(trf!(
                    "Cannot load this device's settings: {e:#}",
                    "Bu cihazın ayarlarını yükləmək mümkün olmadı: {e:#}"
                )),
            ),
        };
        let logo = egui::ColorImage::from_rgba_unmultiplied([64, 64], &rider_core::icon::rgba(64));
        let mut app = Self {
            rt,
            bus: Bus::new(ctx),
            tray,
            logo: Some(ctx.load_texture("logo", logo, egui::TextureOptions::LINEAR)),
            identity,
            cfg,
            fatal,
            tab: Tab::Computers,
            online: HashMap::new(),
            polled: None,
            polling: false,
            server_error: None,
            add: None,
            connect: None,
            quick: None,
            support: None,
            remove: None,
            saved_pins: HashSet::new(),
            sessions: Vec::new(),
            next_id: 0,
            remote: sys::remote_access(),
            remote_checked: Instant::now(),
            manage_error: None,
            settings_msg: None,
            page_since: 0.0,
            cfg_mtime: config_mtime(),
            autostart: sys::autostart(),
            hide_in: if hidden { 2 } else { 0 },
            updater: Updater::new(ctx),
            server_edit: None,
            server_msg: None,
            server_check: None,
            enterprise: crate::enterprise::Enterprise::new(ctx),
        };
        app.refresh_saved_pins();
        app.preview();
        app
    }

    fn preview(&mut self) {
        match std::env::var("RIDER_PREVIEW").as_deref() {
            Ok("this") => self.tab = Tab::ThisComputer,
            Ok("settings") => self.tab = Tab::Settings,
            Ok("enterprise-sign-in") => self.tab = Tab::Enterprise,
            Ok("enterprise-sign-in-server") => {
                self.tab = Tab::Enterprise;
                self.cfg.server = rider_core::rendezvous::ServerRef::new("rdr.example.com", &"ab".repeat(32)).ok();
            }
            Ok(which) if which.starts_with("enterprise") => {
                self.tab = Tab::Enterprise;
                self.cfg.enterprise = Some(rider_client::config::EnterpriseAdmin {
                    company: "Northwind".into(),
                    join_secret: String::new(),
                });
                match which {
                    "enterprise-many" => self.enterprise.preview_many(1500),
                    other => {
                        self.enterprise.preview_state(other.trim_start_matches("enterprise").trim_start_matches('-'));
                    }
                }
            }
            Ok("add") => self.open_add(),
            Ok("quick") => self.open_quick(),
            Ok(s) if s.starts_with("update-") => self.updater.preview(&s["update-".len()..]),
            Ok("connect") => {
                if let Some(h) = self.cfg.hosts.first() {
                    self.connect = Some(ConnectDialog {
                        fingerprint: h.fingerprint.clone(),
                        name: h.name.clone(),
                        pin: String::new(),
                        remember: true,
                        error: None,
                    });
                }
            }
            _ => {}
        }
    }

    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn refresh_saved_pins(&mut self) {
        self.saved_pins = self
            .cfg
            .hosts
            .iter()
            .filter(|h| pins::load(&h.fingerprint).is_some())
            .map(|h| h.fingerprint.clone())
            .collect();
    }

    fn connect_to(&mut self, fingerprint: String, name: String) {
        match pins::load(&fingerprint) {
            Some(pin) => {
                let d = ConnectDialog { fingerprint, name, pin, remember: false, error: None };
                self.start_session(&d, true);
            }
            None => {
                self.connect =
                    Some(ConnectDialog { fingerprint, name, pin: String::new(), remember: true, error: None });
            }
        }
    }

    fn forget_pin(&mut self, fingerprint: &str) {
        if let Err(e) = pins::forget(fingerprint) {
            self.server_error = Some(format!("{e:#}"));
        }
        self.saved_pins.remove(fingerprint);
    }

    fn watch_config(&mut self) {
        let m = config_mtime();
        if m != self.cfg_mtime {
            self.cfg_mtime = m;
            self.reload();
            self.server_error = None;
        }
    }

    fn reload(&mut self) {
        if let Ok(cfg) = ClientConfig::load() {
            self.cfg = cfg;
        }
        self.polled = None;
    }

    fn edit(&mut self, change: impl FnOnce(&mut ClientConfig)) -> Result<(), String> {
        let mut cfg = ClientConfig::load().map_err(|e| format!("{e:#}"))?;
        change(&mut cfg);
        cfg.save().map_err(|e| trf!("cannot save: {e:#}", "yadda saxlamaq mümkün olmadı: {e:#}"))?;
        self.cfg = cfg;
        self.polled = None;
        Ok(())
    }

    fn handle_messages(&mut self) {
        let msgs: Vec<Msg> = self.bus.drain().collect();
        for msg in msgs {
            match msg {
                Msg::Online(r) => {
                    self.polling = false;
                    match r {
                        Ok(list) => {
                            self.online = list.into_iter().collect();
                            self.server_error = None;
                        }
                        Err(e) => {
                            self.online.clear();
                            self.server_error = Some(e);
                        }
                    }
                }
                Msg::PairPrompt(prompt, answer) => {
                    if let Some(d) = &mut self.add {
                        d.stage = AddStage::Confirm { prompt, answer };
                    }
                }
                Msg::PairDone(r) => {
                    self.reload();
                    if let Some(d) = &mut self.add {
                        d.stage = match r {
                            Ok(host) => AddStage::Done { name: host.name },
                            Err(e) => AddStage::Input { error: Some(e) },
                        };
                    }
                }
                Msg::Session { id, update } => self.session_update(id, update),
                Msg::Quick { id, update } => self.quick_update(id, update),
                Msg::Support { id, event } => self.support_event(id, event),
                Msg::SupportFiles { id, files } => {
                    if let Some(s) = self.support.as_mut().filter(|s| s.id == id) {
                        for f in files {
                            s.send_json("send-file", &f.to_string_lossy());
                        }
                    }
                }
                Msg::SupportNotice { id, text } => {
                    if let Some(s) = self.support.as_mut().filter(|s| s.id == id) {
                        s.chat.notice(text);
                    }
                }
            }
        }
        self.sessions.retain(|s| s.finished != Some(true));
    }

    fn session_update(&mut self, id: u64, update: SessionUpdate) {
        let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) else { return };
        match update {
            SessionUpdate::Connected(route) => {
                s.connected = Some(route);
                if let Some(pin) = s.remember.take() {
                    let host = s.host.clone();
                    match pins::save(&host, &pin) {
                        Ok(()) => {
                            self.saved_pins.insert(host);
                        }
                        Err(e) => {
                            self.server_error = Some(trf!(
                                "Cannot remember the PIN: {e:#}",
                                "PIN kodu yadda saxlamaq mümkün olmadı: {e:#}"
                            ));
                        }
                    }
                }
                self.reload();
            }
            SessionUpdate::Stderr(line) => {
                s.stderr.push_str(&line);
                s.stderr.push('\n');
            }
            SessionUpdate::Exited(ok) => {
                s.finished = Some(ok);
                let rejected = s.stderr.contains("wrong PIN") || s.stderr.contains("authentication failed");
                if !ok && s.saved_pin && s.connected.is_none() && rejected {
                    let host = s.host.clone();
                    self.sessions.retain(|s| s.id != id);
                    self.forget_pin(&host);
                    let name = self.cfg.hosts.iter().find(|h| h.fingerprint == host).map(|h| h.name.clone());
                    self.connect = Some(ConnectDialog {
                        fingerprint: host,
                        name: name.unwrap_or_default(),
                        pin: String::new(),
                        remember: true,
                        error: Some(
                            tr(
                                "The saved PIN no longer works. Enter the current one.",
                                "Yadda saxlanılmış PIN kod artıq işləmir. Cari PIN kodu daxil edin.",
                            )
                            .into(),
                        ),
                    });
                }
                self.reload();
            }
        }
    }

    fn quick_update(&mut self, id: u64, update: QuickUpdate) {
        let Some(q) = self.quick.as_mut().filter(|q| q.id == id) else { return };
        match update {
            QuickUpdate::Waiting(host) => q.stage = QuickStage::Waiting { host, code: None },
            QuickUpdate::Code(code) => {
                if let QuickStage::Waiting { code: c, .. } = &mut q.stage {
                    *c = Some(code);
                }
            }
            QuickUpdate::Connected { host, route } => q.stage = QuickStage::Connected { host, route },
            QuickUpdate::Stderr(line) => {
                q.stderr.push_str(&line);
                q.stderr.push('\n');
            }
            QuickUpdate::Exited => {
                q.child = None;
                if matches!(q.stage, QuickStage::Connected { .. }) {
                    self.quick = None;
                } else {
                    q.stage = QuickStage::Input { error: Some(sys::error_text(&q.stderr)) };
                    q.stderr.clear();
                }
            }
        }
    }

    fn support_event(&mut self, id: u64, event: SupportEvent) {
        let Some(s) = self.support.as_mut().filter(|s| s.id == id) else { return };
        match event {
            SupportEvent::Code(code) => {
                s.code = code;
                s.server_error = None;
            }
            SupportEvent::ServerError(e) => s.server_error = Some(e),
            SupportEvent::Request { device, code } => {
                s.note = None;
                s.stage = SupportStage::Asking { device, code };
            }
            SupportEvent::Started(device) => {
                s.chat.peer.clone_from(&device);
                s.chat.files_allowed = s.access.permissions().files;
                s.stage = SupportStage::Active { device };
            }
            SupportEvent::Chat(text) => {
                s.chat.text(false, &text);
                let title = trf!("Message from {}", "{} mesaj göndərdi", s.chat.peer);
                let preview: String = text.chars().take(120).collect();
                self.attention(&title, &preview);
            }
            SupportEvent::File { id, mine, name, size } => {
                s.chat.file_started(id, mine, &name, size);
                if !mine {
                    let title = trf!("{} is sending a file", "{} fayl göndərir", s.chat.peer);
                    self.attention(&title, &name);
                }
            }
            SupportEvent::FileProgress { id, done } => s.chat.file_progress(id, done),
            SupportEvent::FileDone { id, path } => {
                s.chat.file_done(id);
                if let Some(p) = path {
                    s.files.insert(id, p);
                }
            }
            SupportEvent::FileFailed { id, reason } => s.chat.file_failed(id, reason),
            SupportEvent::Tunnel(t) => {
                use crate::tunnel_ask::{TunnelAsk, TunnelEvent, opened_notice};
                match t {
                    TunnelEvent::Request { id, target, seconds } => {
                        let title = trf!("{} asks for a TCP tunnel", "{} TCP tuneli istəyir", s.chat.peer);
                        s.tunnel = Some(TunnelAsk::new(id, target.clone(), seconds));
                        self.attention(&title, &target);
                    }
                    TunnelEvent::Expired(id) => {
                        if s.tunnel.as_ref().is_some_and(|a| a.id == id) {
                            s.tunnel = None;
                        }
                    }
                    TunnelEvent::Opened(target) => {
                        if s.tunnels_seen.insert(target.clone()) {
                            s.chat.notice(opened_notice(&target));
                        }
                    }
                }
            }
            SupportEvent::Ended => s.stage = SupportStage::Ended,
            SupportEvent::Cancelled => {
                s.stage = SupportStage::Waiting;
                s.note = Some(tr("The other side gave up.", "Qarşı tərəf qoşulmadan imtina etdi.").into());
            }
            SupportEvent::Declined => {
                s.stage = SupportStage::Waiting;
                s.note = Some(
                    tr(
                        "Declined. The code still works until it expires.",
                        "İmtina edildi. Kod müddəti bitənə qədər qüvvədədir.",
                    )
                    .into(),
                );
            }
            SupportEvent::Expired => s.stage = SupportStage::Expired,
            SupportEvent::Exited(stderr) => {
                s.stdin = None;
                if matches!(s.stage, SupportStage::Waiting | SupportStage::Asking { .. } | SupportStage::Active { .. })
                {
                    s.stage = SupportStage::Failed(sys::error_text(&stderr));
                }
            }
        }
    }

    fn attention(&self, title: &str, text: &str) {
        let ctx = self.bus.ctx();
        if ctx.input(|i| i.focused) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        if let Some(tray) = &self.tray {
            tray.notify(title, text);
        }
        ctx.request_repaint();
    }

    fn poll_online(&mut self) {
        let (Some(server), Some(identity)) = (self.cfg.server.clone(), self.identity.clone()) else {
            self.online.clear();
            return;
        };
        if self.polling || self.cfg.hosts.is_empty() || self.polled.is_some_and(|t| t.elapsed() < POLL_EVERY) {
            return;
        }
        let hosts: Vec<(String, _)> = self
            .cfg
            .hosts
            .iter()
            .filter_map(|h| Some((h.fingerprint.clone(), fp_from_hex(&h.fingerprint).ok()?)))
            .collect();
        self.polling = true;
        self.polled = Some(Instant::now());
        let tx = self.bus.tx();
        self.rt.spawn(async move {
            let fps: Vec<_> = hosts.iter().map(|(_, fp)| *fp).collect();
            let r = rider_client::online(&identity, &server, &fps)
                .await
                .map(|online| hosts.into_iter().map(|(hex, _)| hex).zip(online).collect())
                .map_err(|e| format!("{e:#}"));
            tx.send(Msg::Online(r));
        });
    }

    fn start_pairing(&mut self) {
        let (Some(d), Some(identity)) = (&mut self.add, self.identity.clone()) else { return };
        let target = d.target.trim().to_owned();
        if target.is_empty() {
            d.stage = AddStage::Input {
                error: Some(
                    tr(
                        "Enter the pairing code or address shown on the other computer.",
                        "Digər kompüterdə göstərilən cütləşdirmə kodunu və ya ünvanı daxil edin.",
                    )
                    .into(),
                ),
            };
            return;
        }
        let alias = Some(d.name.trim().to_owned()).filter(|n| !n.is_empty());
        d.stage = AddStage::Connecting;
        let tx = self.bus.tx();
        self.rt.spawn(async move {
            let result = async {
                let mut cfg = ClientConfig::load()?;
                let ask = tx.clone();
                rider_client::pair(&mut cfg, &identity, &target, alias, async move |p: &PairPrompt| {
                    let (answer, rx) = oneshot::channel();
                    ask.send(Msg::PairPrompt(p.clone(), answer));
                    rx.await.unwrap_or(false)
                })
                .await
            }
            .await;
            tx.send(Msg::PairDone(result.map_err(|e| format!("{e:#}"))));
        });
    }

    fn start_session(&mut self, d: &ConnectDialog, saved_pin: bool) {
        self.sessions.retain(|s| s.finished.is_none() || s.host != d.fingerprint);
        let id = self.next_id();
        let mut session = Session {
            id,
            host: d.fingerprint.clone(),
            connected: None,
            stderr: String::new(),
            finished: None,
            remember: d.remember.then(|| d.pin.clone()),
            saved_pin,
        };
        let spawned = sys::tool("rider-client.exe")
            .args(["connect", &d.fingerprint, "--pin-stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                session.stderr =
                    trf!("Error: cannot start rider-client.exe: {e}", "Error: rider-client.exe başladıla bilmir: {e}");
                session.finished = Some(false);
                self.sessions.push(session);
                return;
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = writeln!(stdin, "{}", d.pin);
        }
        let tx: Tx<Msg> = self.bus.tx();
        if let Some(out) = child.stdout.take() {
            let tx = tx.clone();
            sys::lines(out, move |line| {
                if let Some((_, route)) = parse_connected(&line) {
                    tx.send(Msg::Session { id, update: SessionUpdate::Connected(route) });
                }
            });
        }
        if let Some(err) = child.stderr.take() {
            let tx = tx.clone();
            sys::lines(err, move |line| tx.send(Msg::Session { id, update: SessionUpdate::Stderr(line) }));
        }
        std::thread::spawn(move || {
            let ok = child.wait().is_ok_and(|s| s.success());
            tx.send(Msg::Session { id, update: SessionUpdate::Exited(ok) });
        });
        self.sessions.push(session);
    }

    fn start_quick(&mut self) {
        let id = self.next_id();
        let Some(q) = &mut self.quick else { return };
        q.id = id;
        q.stderr.clear();
        let spawned = sys::tool("rider-client.exe")
            .args(["quick", q.code.trim()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                q.stage = QuickStage::Input {
                    error: Some(trf!("cannot start rider-client.exe: {e}", "rider-client.exe başladıla bilmir: {e}")),
                };
                return;
            }
        };
        q.stage = QuickStage::Connecting;
        let tx: Tx<Msg> = self.bus.tx();
        if let Some(out) = child.stdout.take() {
            let tx = tx.clone();
            sys::lines(out, move |line| {
                let update = if let Some((host, route)) = parse_connected(&line) {
                    QuickUpdate::Connected { host, route }
                } else if let Some(rest) = line.strip_prefix("Waiting for \"") {
                    QuickUpdate::Waiting(rest.split('"').next().unwrap_or_default().to_owned())
                } else if let Some(code) = line.strip_prefix("Verification code: ") {
                    QuickUpdate::Code(code.trim().to_owned())
                } else {
                    return;
                };
                tx.send(Msg::Quick { id, update });
            });
        }
        if let Some(err) = child.stderr.take() {
            let tx = tx.clone();
            sys::lines(err, move |line| tx.send(Msg::Quick { id, update: QuickUpdate::Stderr(line) }));
        }
        let child = Arc::new(Mutex::new(child));
        q.child = Some(child.clone());
        std::thread::spawn(move || {
            loop {
                if !matches!(child.lock().unwrap().try_wait(), Ok(None)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            std::thread::sleep(Duration::from_millis(150));
            tx.send(Msg::Quick { id, update: QuickUpdate::Exited });
        });
    }

    fn start_support(&mut self) {
        let Some(server) = self.cfg.server.clone() else { return };
        let id = self.next_id();
        let spawned = sys::tool("rider-host.exe")
            .args(["--json", "support", &server.addr, &server.fingerprint])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                let stage = SupportStage::Failed(trf!(
                    "cannot start rider-host.exe: {e}",
                    "rider-host.exe başladıla bilmir: {e}"
                ));
                self.support = Some(Support::new(id, None, stage));
                return;
            }
        };
        let stdin = child.stdin.take();
        let tx: Tx<Msg> = self.bus.tx();
        if let Some(out) = child.stdout.take() {
            let tx = tx.clone();
            sys::lines(out, move |line| {
                if let Some(event) = parse_support_event(&line) {
                    tx.send(Msg::Support { id, event });
                }
            });
        }
        let stderr = child.stderr.take();
        std::thread::spawn(move || {
            let mut err = String::new();
            if let Some(mut s) = stderr {
                use std::io::Read;
                let _ = s.read_to_string(&mut err);
            }
            let _ = child.wait();
            tx.send(Msg::Support { id, event: SupportEvent::Exited(err) });
        });
        self.support = Some(Support::new(id, stdin, SupportStage::Waiting));
    }

    fn open_add(&mut self) {
        self.add =
            Some(AddDialog { target: String::new(), name: String::new(), stage: AddStage::Input { error: None } });
    }

    fn open_quick(&mut self) {
        self.quick = Some(QuickDialog {
            id: 0,
            code: String::new(),
            stage: QuickStage::Input { error: None },
            stderr: String::new(),
            child: None,
        });
    }
}

fn parse_connected(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("Connected to \"")?;
    let (name, tail) = rest.rsplit_once("\" (")?;
    Some((name.to_owned(), tail.trim_end_matches(").").to_owned()))
}

fn parse_support_event(line: &str) -> Option<SupportEvent> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned();
    let n = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or_default();
    if let Some(t) = crate::tunnel_ask::parse(&v) {
        return Some(SupportEvent::Tunnel(t));
    }
    Some(match v.get("event")?.as_str()? {
        "registered" => SupportEvent::Code(v.get("code").and_then(|c| c.as_str()).map(str::to_owned)),
        "server_error" => SupportEvent::ServerError(s("message")),
        "support_request" => SupportEvent::Request { device: s("device"), code: s("code") },
        "support_started" => SupportEvent::Started(s("device")),
        "support_ended" => SupportEvent::Ended,
        "support_cancelled" => SupportEvent::Cancelled,
        "declined" => SupportEvent::Declined,
        "expired" => SupportEvent::Expired,
        "chat" => SupportEvent::Chat(s("text")),
        "file" => SupportEvent::File {
            id: n("id"),
            mine: v.get("dir").and_then(|d| d.as_str()) == Some("out"),
            name: s("name"),
            size: n("size"),
        },
        "file_progress" => SupportEvent::FileProgress { id: n("id"), done: n("done") },
        "file_done" => SupportEvent::FileDone {
            id: n("id"),
            path: v.get("path").and_then(|p| p.as_str()).map(std::path::PathBuf::from),
        },
        "file_failed" => SupportEvent::FileFailed { id: n("id"), reason: s("reason") },
        _ => return None,
    })
}

impl eframe::App for ClientApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        sys::smooth_monitor_moves(frame);
        let ctx = ui.ctx().clone();
        if self.hide_in > 0 {
            self.hide_in -= 1;
            if self.hide_in == 0 {
                tray::hide_main_centered();
            } else {
                ctx.request_repaint();
            }
        }
        self.close_to_tray(&ctx);
        self.handle_messages();
        self.watch_config();
        self.poll_online();
        if tray::take_install_request() {
            self.updater.install_offered();
        }
        ctx.request_repaint_after(Duration::from_secs(2));

        let side = if ctx.global_style().visuals.dark_mode {
            egui::Color32::from_rgb(30, 31, 36)
        } else {
            egui::Color32::from_rgb(232, 234, 239)
        };
        if self.session_page(ui) {
            return;
        }
        egui::Panel::left("nav")
            .exact_size(220.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(side).inner_margin(egui::Margin::symmetric(10, 0)))
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.global_style()).inner_margin(egui::Margin::symmetric(28, 22)))
            .show(ui, |ui| {
                if let Some(e) = &self.fatal {
                    widgets::error(ui, e);
                    return;
                }
                widgets::appear(ui, self.page_since);
                self.update_banner(ui);
                match self.tab {
                    Tab::Computers => self.computers(ui),
                    Tab::ThisComputer => self.this_computer(ui),
                    Tab::Enterprise => self.enterprise_page(ui),
                    Tab::Settings => self.settings(ui),
                }
            });
        self.add_dialog(&ctx);
        self.connect_dialog(&ctx);
        self.quick_dialog(&ctx);
        self.support_dialog(&ctx);
    }
}

fn progress_line(ui: &mut Ui, fraction: Option<f32>) {
    ui.add_space(2.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.0, ui.visuals().widgets.inactive.weak_bg_fill);
    let w = rect.width();
    let (x0, x1) = if let Some(f) = fraction {
        (0.0, w * f.clamp(0.0, 1.0))
    } else {
        let t = ui.input(|i| i.time) as f32;
        let pos = (t / 1.4).fract() * (w * 1.3) - w * 0.3;
        ui.ctx().request_repaint();
        (pos.max(0.0), (pos + w * 0.3).min(w))
    };
    if x1 > x0 {
        let bar = egui::Rect::from_min_max(
            egui::pos2(rect.left() + x0, rect.top()),
            egui::pos2(rect.left() + x1, rect.bottom()),
        );
        p.rect_filled(bar, 2.0, ACCENT);
    }
    ui.add_space(2.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_viewer_lines() {
        assert_eq!(
            parse_connected("Connected to \"Office (2)\" (direct 1.2.3.4:47800)."),
            Some(("Office (2)".into(), "direct 1.2.3.4:47800".into()))
        );
        assert!(parse_connected("Waiting for \"x\" to allow the connection.").is_none());
    }

    #[test]
    fn parses_support_events() {
        assert!(matches!(
            parse_support_event(r#"{"event":"registered","server":"s","public":"1.2.3.4:5","code":"SF6-CK6"}"#),
            Some(SupportEvent::Code(Some(c))) if c == "SF6-CK6"
        ));
        assert!(matches!(
            parse_support_event(r#"{"event":"support_request","device":"Laptop","from":"1.2.3.4:5","code":"123 456"}"#),
            Some(SupportEvent::Request { device, code }) if device == "Laptop" && code == "123 456"
        ));
        assert!(matches!(parse_support_event(r#"{"event":"expired"}"#), Some(SupportEvent::Expired)));
        assert!(parse_support_event("not json").is_none());
    }
}

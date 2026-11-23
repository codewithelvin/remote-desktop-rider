//! Main window (no administrator rights needed): the computers this device
//! can control, adding one, connecting, quick connect by one-time code,
//! getting help with a one-time code, and this device's settings.

use crate::pins;
use crate::sys::{self, RemoteAccess};
use crate::tray::{self, Tray};
use crate::updates::{self, Updater};
use crate::widgets::{self, ACCENT, AMBER, Bus, GREEN, GREY, Tx, icon};
use eframe::egui::{self, Align, Id, Key, Layout, Modal, RichText, ScrollArea, TextEdit, TextureHandle, Ui, vec2};
use rider_client::PairPrompt;
use rider_client::config::{ClientConfig, KnownHost};
use rider_core::i18n::tr;
use rider_core::identity::{Identity, fp_from_hex};
use rider_core::trf;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

fn config_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(ClientConfig::path().ok()?).ok()?.modified().ok()
}

/// How often the server is asked which computers are online.
const POLL_EVERY: Duration = Duration::from_secs(15);
/// Where new versions are published.
const RELEASES: &str = "https://github.com/codewithelvin/remote-desktop-rider/releases/latest";

/// This device can also be controlled (Windows: the This computer tab,
/// Manage, startup, self-update). On macOS and Linux the app only controls
/// other computers. `RIDER_PORTABLE_UI=1` shows that layout on Windows
/// (screenshots, checks).
fn host_side() -> bool {
    cfg!(windows) && std::env::var_os("RIDER_PORTABLE_UI").is_none()
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Computers,
    ThisComputer,
    Settings,
}

enum Msg {
    Online(Result<Vec<(String, bool)>, String>),
    PairPrompt(PairPrompt, oneshot::Sender<bool>),
    PairDone(Result<KnownHost, String>),
    Session { id: u64, update: SessionUpdate },
    Quick { id: u64, update: QuickUpdate },
    Support { id: u64, event: SupportEvent },
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
    /// Save the PIN (encrypted for this Windows user) once it worked.
    remember: bool,
    error: Option<String>,
}

struct Session {
    id: u64,
    /// Fingerprint of the computer.
    host: String,
    connected: Option<String>,
    stderr: String,
    finished: Option<bool>,
    /// PIN to remember once connected.
    remember: Option<String>,
    /// Started with the remembered PIN (no dialog).
    saved_pin: bool,
}

// ------------------------------------------------- quick connect (helper side)

/// Lines from `rider-client quick` (its stdout is meant to be read like this).
enum QuickUpdate {
    Waiting(String),
    Code(String),
    Connected { host: String, route: String },
    Stderr(String),
    Exited,
}

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

// ------------------------------------- one-time code (the person being helped)

/// Events from `rider-host --json support`.
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
    code: Option<String>,
    server_error: Option<String>,
    /// A short note under the code, e.g. after declining someone.
    note: Option<String>,
    stage: SupportStage,
}

impl Support {
    fn send(&mut self, line: &str) {
        if let Some(s) = &mut self.stdin {
            let _ = writeln!(s, "{line}");
        }
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
    /// Computers with a remembered PIN.
    saved_pins: HashSet<String>,
    sessions: Vec<Session>,
    next_id: u64,
    remote: RemoteAccess,
    remote_checked: Instant,
    manage_error: Option<String>,
    settings_msg: Option<Result<String, String>>,
    /// When the page shown last changed (egui time), for its fade-in.
    page_since: f64,
    /// Seen modification time of client.toml: Manage (another process) may change it.
    cfg_mtime: Option<std::time::SystemTime>,
    autostart: bool,
    /// Frames left before hiding a window started with `--tray`.
    hide_in: u8,
    updater: Updater,
    /// Server address and fingerprint being edited (Settings, without Manage).
    server_edit: Option<(String, String)>,
    server_msg: Option<Result<String, String>>,
    /// Checking a server in the background: the result, and whether its
    /// name was verified over HTTPS (rather than a typed fingerprint).
    server_check: Option<std::sync::mpsc::Receiver<(Result<rider_core::rendezvous::ServerRef, String>, bool)>>,
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
        };
        app.refresh_saved_pins();
        app.preview();
        app
    }

    /// `RIDER_PREVIEW=this|settings|add|quick|connect`: start on that page or
    /// dialog, to look at every view (both themes and languages).
    fn preview(&mut self) {
        match std::env::var("RIDER_PREVIEW").as_deref() {
            Ok("this") => self.tab = Tab::ThisComputer,
            Ok("settings") => self.tab = Tab::Settings,
            Ok("add") => self.open_add(),
            Ok("quick") => self.open_quick(),
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

    /// Connect: straight away with a remembered PIN, otherwise ask for it.
    fn connect_to(&mut self, fingerprint: String, name: String) {
        match pins::load(&fingerprint) {
            Some(pin) => {
                let d = ConnectDialog { fingerprint, name, pin, remember: false, error: None };
                self.start_session(d, true);
            }
            None => {
                self.connect =
                    Some(ConnectDialog { fingerprint, name, pin: String::new(), remember: true, error: None })
            }
        }
    }

    fn forget_pin(&mut self, fingerprint: &str) {
        if let Err(e) = pins::forget(fingerprint) {
            self.server_error = Some(format!("{e:#}"));
        }
        self.saved_pins.remove(fingerprint);
    }

    /// Pick up changes saved by other processes (Manage sets the server).
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
        self.polled = None; // refresh online state for the new list
    }

    /// Change the saved configuration. Viewer processes write to the same file
    /// (last connected), so start from what is on disk, never from our copy.
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
            }
        }
        // Finished sessions without an error disappear on their own.
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
                            ))
                        }
                    }
                }
                self.reload(); // the viewer just saved the last-connected time
            }
            SessionUpdate::Stderr(line) => {
                s.stderr.push_str(&line);
                s.stderr.push('\n');
            }
            SessionUpdate::Exited(ok) => {
                s.finished = Some(ok);
                // The remembered PIN no longer works (changed on that
                // computer): forget it and ask. (The host's messages are
                // protocol text, always English.)
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
                self.reload(); // last-connected time changed
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
                    // The session ran and is over.
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
            SupportEvent::Started(device) => s.stage = SupportStage::Active { device },
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

    fn start_session(&mut self, d: ConnectDialog, saved_pin: bool) {
        // A new attempt replaces the error of the previous one.
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

    /// Quick connect: run `rider-client quick <code>`; it opens the session
    /// window once the person there allows it.
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
            // Polled so Cancel can kill the process meanwhile.
            loop {
                if !matches!(child.lock().unwrap().try_wait(), Ok(None)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            // Let the stderr reader deliver the error first.
            std::thread::sleep(Duration::from_millis(150));
            tx.send(Msg::Quick { id, update: QuickUpdate::Exited });
        });
    }

    /// Get a one-time code: run `rider-host --json support` as this user
    /// (no administrator rights). It ends with the session or after 10 minutes.
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
                self.support = Some(Support {
                    id,
                    stdin: None,
                    code: None,
                    server_error: None,
                    note: None,
                    stage: SupportStage::Failed(trf!(
                        "cannot start rider-host.exe: {e}",
                        "rider-host.exe başladıla bilmir: {e}"
                    )),
                });
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
        self.support =
            Some(Support { id, stdin, code: None, server_error: None, note: None, stage: SupportStage::Waiting });
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

    // ---------------------------------------------------------------- views

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new((logo.id(), vec2(28.0, 28.0))));
            }
            let room = ui.available_width() - 2.0;
            ui.label(widgets::title_fit(ui, "Remote Desktop Rider", 15.0, room));
        });
        ui.add_space(18.0);
        for (tab, ic, text) in [
            (Tab::Computers, icon::PC, tr("Computers", "Kompüterlər")),
            (Tab::ThisComputer, icon::HOME, tr("This computer", "Bu kompüter")),
            (Tab::Settings, icon::SETTINGS, tr("Settings", "Parametrlər")),
        ] {
            if tab == Tab::ThisComputer && !host_side() {
                continue;
            }
            if widgets::nav_item(ui, ic, text, self.tab == tab) && self.tab != tab {
                self.tab = tab;
                self.page_since = ui.input(|i| i.time);
            }
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(12.0);
            widgets::hint(ui, &format!("   {} {}", tr("Version", "Versiya"), env!("CARGO_PKG_VERSION")));
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let (color, text) = match (&self.cfg.server, &self.server_error) {
                    (None, _) => (None, tr("Local network only", "Yalnız yerli şəbəkə")),
                    (Some(_), Some(_)) => (Some(AMBER), tr("Server offline", "Server əlçatmazdır")),
                    (Some(_), None) if self.polled.is_none() && !self.cfg.hosts.is_empty() => {
                        (Some(GREY), tr("Server…", "Server…"))
                    }
                    (Some(_), None) => (Some(GREEN), tr("Server online", "Server aktivdir")),
                };
                widgets::dot(ui, color);
                // Wraps if a wide font still makes it too long.
                ui.add(egui::Label::new(egui::RichText::new(text).weak()).wrap());
            });
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                widgets::language_switch(ui);
            });
        });
    }

    fn computers(&mut self, ui: &mut Ui) {
        let has_hosts = !self.cfg.hosts.is_empty();
        let mut add = false;
        let mut quick = false;
        widgets::page_header(
            ui,
            tr("Computers", "Kompüterlər"),
            tr("Computers this device can control.", "Bu cihazın idarə edə bildiyi kompüterlər."),
            |ui| {
                if has_hosts {
                    add = widgets::primary(
                        ui,
                        true,
                        &format!("{}   {}", icon::ADD, tr("Add computer", "Kompüter əlavə et")),
                    )
                    .clicked();
                }
                quick = ui
                    .button(format!("{}   {}", icon::PLAY, tr("Quick connect", "Sürətli qoşulma")))
                    .on_hover_text(tr(
                        "One-time session with the code another computer shows: no pairing, no PIN.",
                        "Digər kompüterin göstərdiyi kodla birdəfəlik seans: cütləşdirmə və PIN kod tələb olunmur.",
                    ))
                    .clicked();
            },
        );
        if let Some(e) = &self.server_error {
            widgets::card(ui, |ui| {
                widgets::error(
                    ui,
                    &trf!(
                        "The server can't be reached, so online status is unknown. {e}",
                        "Serverə qoşulmaq mümkün deyil, ona görə onlayn vəziyyət məlum deyil. {e}"
                    ),
                );
            });
            ui.add_space(8.0);
        }

        if !has_hosts {
            widgets::card(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(18.0);
                    ui.label(RichText::new(icon::PC).size(44.0).color(ACCENT));
                    ui.label(widgets::title(tr("No computers yet", "Hələ kompüter yoxdur"), 18.0));
                    widgets::hint(
                        ui,
                        tr(
                            "On the computer you want to control, open Remote Desktop Rider, go to\n\
                             This computer > Manage this computer, and click Add a device.\n\
                             For one-time help, use Quick connect instead.",
                            "İdarə etmək istədiyiniz kompüterdə Remote Desktop Rider-i açın,\n\
                             «Bu kompüter» > «Bu kompüteri idarə et» bölməsində «Cihaz əlavə et» düyməsini klikləyin.\n\
                             Birdəfəlik kömək üçün isə «Sürətli qoşulma»dan istifadə edin.",
                        ),
                    );
                    ui.add_space(4.0);
                    add = widgets::primary(
                        ui,
                        true,
                        &format!("{}   {}", icon::ADD, tr("Add computer", "Kompüter əlavə et")),
                    )
                    .clicked()
                        || add;
                    ui.add_space(18.0);
                });
            });
        }
        if add {
            self.open_add();
        }
        if quick {
            self.open_quick();
        }

        let mut hosts = self.cfg.hosts.clone();
        hosts.sort_by_key(|h| h.name.to_lowercase());
        ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            for h in &hosts {
                self.host_card(ui, h);
                ui.add_space(8.0);
            }
        });
    }

    fn host_card(&mut self, ui: &mut Ui, h: &KnownHost) {
        let online = self.online.get(&h.fingerprint).copied();
        let session = self
            .sessions
            .iter()
            .find(|s| s.host == h.fingerprint)
            .map(|s| (s.id, s.finished, s.connected.clone(), sys::error_text(&s.stderr)));
        let in_session = session.as_ref().is_some_and(|s| s.1.is_none());
        let mut dismiss = None;
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::avatar(ui, icon::PC, online.map(|o| if o { GREEN } else { GREY }));
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.label(widgets::title(&h.name, 16.0));
                    ui.horizontal(|ui| {
                        match online {
                            Some(true) => widgets::pill(ui, tr("Online", "Onlayn"), GREEN),
                            Some(false) => widgets::pill(ui, tr("Offline", "Oflayn"), GREY),
                            None if h.addr.is_empty() => {}
                            None => widgets::hint(ui, &h.addr),
                        }
                        let last = h.last_connected.map_or_else(
                            || tr("Never connected", "Heç vaxt qoşulmayıb").to_owned(),
                            |t| {
                                let when = widgets::ago(t);
                                trf!("Last connected {when}", "Son qoşulma: {when}")
                            },
                        );
                        widgets::hint(ui, &last);
                    });
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.remove.as_deref() == Some(h.fingerprint.as_str()) {
                        if widgets::danger(ui, tr("Remove", "Sil")).clicked() {
                            if let Err(e) = self.edit(|cfg| {
                                cfg.forget(&h.fingerprint);
                            }) {
                                self.server_error = Some(e);
                            }
                            self.forget_pin(&h.fingerprint);
                            self.remove = None;
                        }
                        if ui.button(tr("Cancel", "Ləğv et")).clicked() {
                            self.remove = None;
                        }
                        ui.label(tr("Remove from this list?", "Siyahıdan silinsin?"));
                    } else {
                        if widgets::icon_button(ui, icon::DELETE, tr("Remove from this list", "Siyahıdan sil"))
                            .clicked()
                        {
                            self.remove = Some(h.fingerprint.clone());
                        }
                        let label = if in_session { tr("In session", "Seansdadır") } else { tr("Connect", "Qoşul") };
                        // The server says the computer is offline: nothing to
                        // connect to until it comes back (checked every 15 s).
                        // Unknown status (no server, server unreachable) keeps
                        // Connect: a direct address on the LAN may still work.
                        let offline = online == Some(false);
                        let connect = widgets::primary(ui, !in_session && !offline, label);
                        if offline {
                            connect.on_disabled_hover_text(tr(
                                "This computer is offline. Connect becomes available as soon as it is back online.",
                                "Bu kompüter oflayndır. Onlayn olan kimi «Qoşul» düyməsi aktivləşəcək.",
                            ));
                        } else if connect.clicked() {
                            self.connect_to(h.fingerprint.clone(), h.name.clone());
                        }
                        if self.saved_pins.contains(&h.fingerprint)
                            && widgets::icon_button(
                                ui,
                                icon::KEY,
                                tr(
                                    "PIN remembered on this device. Click to forget it.",
                                    "PIN kod bu cihazda yadda saxlanılıb. Silmək üçün klikləyin.",
                                ),
                            )
                            .clicked()
                        {
                            self.forget_pin(&h.fingerprint);
                        }
                    }
                });
            });
            if let Some((id, finished, connected, error)) = &session {
                ui.add_space(2.0);
                ui.separator();
                ui.horizontal(|ui| match (finished, connected) {
                    (None, None) => {
                        ui.spinner();
                        ui.label(tr("Connecting…", "Qoşulur…"));
                    }
                    (None, Some(route)) => {
                        widgets::dot(ui, Some(GREEN));
                        ui.label(trf!(
                            "Connected ({route}). The session is open in its own window.",
                            "Qoşulub ({route}). Seans ayrıca pəncərədə açıqdır."
                        ));
                    }
                    (Some(_), _) => {
                        widgets::error(ui, error);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button(tr("Dismiss", "Bağla")).clicked() {
                                dismiss = Some(*id);
                            }
                        });
                    }
                });
            }
        });
        if let Some(id) = dismiss {
            self.sessions.retain(|s| s.id != id);
        }
    }

    fn this_computer(&mut self, ui: &mut Ui) {
        if self.remote_checked.elapsed() > Duration::from_secs(3) {
            self.remote = sys::remote_access();
            self.remote_checked = Instant::now();
        }
        widgets::page_header(ui, tr("This computer", "Bu kompüter"), &format!("\"{}\"", self.cfg.device_name), |_| {});
        ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            self.help_card(ui);
            ui.add_space(10.0);
            widgets::card(ui, |ui| {
                widgets::section(ui, icon::SHIELD, tr("Remote access", "Uzaqdan giriş"));
                ui.horizontal(|ui| {
                    let (color, state, text) = match self.remote {
                        RemoteAccess::On => (
                            GREEN,
                            tr("On", "Aktiv"),
                            tr(
                                "Devices you added can control this computer, even at the sign-in screen.",
                                "Əlavə etdiyiniz cihazlar bu kompüteri, hətta giriş ekranında da idarə edə bilər.",
                            ),
                        ),
                        RemoteAccess::Stopped => (
                            AMBER,
                            tr("Stopped", "Dayandırılıb"),
                            tr("Remote access is set up but not running.", "Uzaqdan giriş quraşdırılıb, lakin işləmir."),
                        ),
                        RemoteAccess::Off => (
                            GREY,
                            tr("Off", "Söndürülüb"),
                            tr("Nobody can connect to this computer.", "Heç kim bu kompüterə qoşula bilməz."),
                        ),
                    };
                    widgets::pill(ui, state, color);
                    ui.add(egui::Label::new(text).wrap());
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if widgets::primary(
                        ui,
                        true,
                        &format!("{}   {}", icon::SHIELD, tr("Manage this computer…", "Bu kompüteri idarə et…")),
                    )
                    .clicked()
                    {
                        self.manage_error = sys::relaunch_elevated("--manage").err().map(|e| format!("{e:#}"));
                    }
                    widgets::hint(ui, tr("Windows asks for permission once.", "Windows bir dəfə icazə istəyəcək."));
                });
                if let Some(e) = &self.manage_error {
                    widgets::error(ui, e);
                }
            });
            ui.add_space(10.0);
            widgets::card(ui, |ui| {
                widgets::section(
                    ui,
                    icon::INFO,
                    tr(
                        "Let another device control this computer",
                        "Başqa cihazın bu kompüteri idarə etməsinə icazə verin",
                    ),
                );
                for (n, step) in [
                    tr("Click Manage this computer above.", "Yuxarıda «Bu kompüteri idarə et» düyməsini klikləyin."),
                    tr("Set a PIN and turn on remote access.", "PIN kod təyin edin və uzaqdan girişi aktivləşdirin."),
                    tr(
                        "Click Add a device, then on the other device click Add computer and enter the code shown.",
                        "«Cihaz əlavə et» düyməsini klikləyin, sonra digər cihazda «Kompüter əlavə et» seçib göstərilən kodu daxil edin.",
                    ),
                ]
                .iter()
                .enumerate()
                {
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new(format!("{}.", n + 1)).color(ACCENT).strong());
                        ui.add(egui::Label::new(*step).wrap());
                    });
                }
            });
        });
    }

    /// "Get help now": a one-time code for someone to connect once.
    fn help_card(&mut self, ui: &mut Ui) {
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::KEY, tr("Get help now (one-time)", "İndi kömək alın (birdəfəlik)"));
            ui.add(
                egui::Label::new(tr(
                    "Let someone see and control this screen once, without a PIN or adding their device. \
                     You approve the connection, and nothing is kept afterwards.",
                    "Kiminsə bu ekranı bir dəfə görməsinə və idarə etməsinə icazə verin: PIN kod və cihaz əlavə \
                     etmək lazım deyil. Qoşulmanı siz təsdiqləyirsiniz, seansdan sonra heç nə saxlanılmır.",
                ))
                .wrap(),
            );
            let has_server = self.cfg.server.is_some();
            ui.horizontal(|ui| {
                let busy = self.support.is_some();
                if widgets::primary(ui, has_server && !busy, tr("Get a one-time code", "Birdəfəlik kod al")).clicked()
                {
                    self.start_support();
                }
                if !has_server {
                    widgets::hint(
                        ui,
                        tr(
                            "Needs a server: set it in Manage this computer → Server.",
                            "Server tələb olunur: onu «Bu kompüteri idarə et» → «Server» bölməsində təyin edin.",
                        ),
                    );
                }
            });
        });
    }

    fn settings(&mut self, ui: &mut Ui) {
        widgets::page_header(ui, tr("Settings", "Parametrlər"), "", |_| {});
        ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            widgets::card(ui, |ui| {
                widgets::section(ui, icon::PC, tr("This device", "Bu cihaz"));
                egui::Grid::new("device").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                    widgets::hint(ui, tr("Name", "Ad"));
                    ui.label(&self.cfg.device_name);
                    ui.end_row();
                    widgets::hint(ui, tr("Fingerprint", "Barmaq izi"));
                    let fp = self
                        .identity
                        .as_ref()
                        .map(|i| rider_core::identity::fp_short(&i.fingerprint()))
                        .unwrap_or_default();
                    ui.horizontal(|ui| widgets::copyable(ui, &fp, RichText::new(&fp).monospace()));
                    ui.end_row();
                });
            });
            ui.add_space(10.0);
            widgets::card(ui, |ui| {
                widgets::section(ui, icon::GLOBE, tr("Language", "Dil"));
                ui.horizontal(|ui| {
                    widgets::language_switch(ui);
                    ui.label(rider_core::i18n::lang().native_name());
                });
                widgets::hint(
                    ui,
                    if host_side() {
                        tr(
                            "English or Azerbaijani. Applies right away, also to session windows and Manage this computer.",
                            "İngilis və ya Azərbaycan dili. Dərhal tətbiq olunur: seans pəncərələri və «Bu kompüteri idarə et» də bu dildə açılır.",
                        )
                    } else {
                        tr(
                            "English or Azerbaijani. Applies right away, also to session windows.",
                            "İngilis və ya Azərbaycan dili. Dərhal tətbiq olunur: seans pəncərələri də bu dildə açılır.",
                        )
                    },
                );
            });
            ui.add_space(10.0);
            self.server_card(ui);
            ui.add_space(10.0);
            self.updates_card(ui);
            if !host_side() {
                return; // no tray to start into (yet)
            }
            ui.add_space(10.0);
            widgets::card(ui, |ui| {
                widgets::section(ui, icon::PLAY, tr("Startup", "Avtomatik başlatma"));
                if ui
                    .checkbox(
                        &mut self.autostart,
                        tr("Start Remote Desktop Rider when I sign in", "Sistemə daxil olduqda Remote Desktop Rider-i başlat"),
                    )
                    .changed()
                    && let Err(e) = sys::set_autostart(self.autostart)
                {
                    self.autostart = sys::autostart();
                    self.settings_msg = Some(Err(format!("{e:#}")));
                }
                widgets::hint(
                    ui,
                    tr(
                        "Closing the window keeps the app in the notification area. Use Quit in its menu to exit.",
                        "Pəncərə bağlandıqda proqram bildiriş sahəsində qalır. Tam çıxmaq üçün onun menyusunda «Çıxış» seçin.",
                    ),
                );
                if let Some(Err(e)) = &self.settings_msg {
                    widgets::error(ui, e);
                }
            });
        });
    }

    /// "Version x is available" above every page while there is one.
    fn update_banner(&mut self, ui: &mut Ui) {
        use updates::State;
        let state = self.updater.state();
        let release = match &state {
            State::Available(r)
            | State::Downloading { release: r, .. }
            | State::Installing(r)
            | State::Failed { release: Some(r), .. } => r.clone(),
            _ => return,
        };
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::DOWNLOAD).size(18.0).color(ACCENT));
                let version = &release.version;
                ui.label(
                    RichText::new(trf!("Version {version} is available", "{version} versiyası mövcuddur")).strong(),
                );
                ui.hyperlink_to(tr("What's new", "Yeniliklər"), &release.page);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| match &state {
                    State::Downloading { done, .. } => {
                        let f = *done as f32 / release.size.max(1) as f32;
                        let pct = f * 100.0;
                        ui.add(
                            egui::ProgressBar::new(f)
                                .desired_width(220.0)
                                .text(trf!("Downloading… {pct:.0}%", "Yüklənir… {pct:.0}%")),
                        );
                    }
                    State::Installing(_) => {
                        widgets::hint(
                            ui,
                            tr("Installing… the app reopens by itself.", "Quraşdırılır… proqram özü yenidən açılacaq."),
                        );
                        ui.spinner();
                    }
                    _ => {
                        let label = if matches!(state, State::Failed { .. }) {
                            tr("Try again", "Yenidən cəhd et")
                        } else {
                            tr("Install update", "Yeniləməni quraşdır")
                        };
                        if widgets::primary(ui, true, label).clicked() {
                            self.updater.install(release.clone());
                        }
                    }
                });
            });
            if let State::Failed { error, .. } = &state {
                widgets::error(ui, error);
            } else if matches!(state, State::Available(_)) {
                widgets::hint(
                    ui,
                    tr(
                        "Windows asks for permission once. Open sessions from this device end while it installs.",
                        "Windows bir dəfə icazə istəyəcək. Quraşdırma zamanı bu cihazdan açılmış seanslar bağlanacaq.",
                    ),
                );
            }
        });
        ui.add_space(12.0);
    }

    fn updates_card(&mut self, ui: &mut Ui) {
        use updates::State;
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::DOWNLOAD, tr("Updates", "Yeniləmələr"));
            ui.horizontal(|ui| {
                widgets::hint(ui, tr("Version", "Versiya"));
                ui.label(updates::current().to_string());
            });
            if !host_side() {
                ui.hyperlink_to(
                    tr(
                        "Get new versions from the releases page",
                        "Yeni versiyaları buraxılışlar səhifəsindən yükləyin",
                    ),
                    RELEASES,
                );
                return;
            }
            if !self.updater.installed {
                widgets::hint(
                    ui,
                    tr(
                        "Updates apply to the installed app (Program Files); this copy runs from elsewhere.",
                        "Yeniləmələr quraşdırılmış proqrama (Program Files) tətbiq olunur; bu nüsxə başqa qovluqdan işləyir.",
                    ),
                );
                return;
            }
            let mut auto = self.updater.auto;
            if ui.checkbox(&mut auto, tr("Check for updates automatically", "Yeniləmələri avtomatik yoxla")).changed()
            {
                self.updater.set_auto(auto);
            }
            ui.horizontal(|ui| {
                let state = self.updater.state();
                let checking = matches!(state, State::Checking { .. });
                if ui.add_enabled(!checking, egui::Button::new(tr("Check now", "İndi yoxla"))).clicked() {
                    self.updater.check();
                }
                match state {
                    State::Checking { manual: true } => {
                        ui.spinner();
                    }
                    State::UpToDate => {
                        widgets::success(ui, tr("You have the latest version.", "Ən son versiya quraşdırılıb."))
                    }
                    State::Failed { error, release: None } => widgets::error(ui, &error),
                    State::Available(r) => {
                        let version = &r.version;
                        widgets::hint(
                            ui,
                            &trf!(
                                "Version {version} is available (see the top).",
                                "{version} versiyası mövcuddur (yuxarıya baxın)."
                            ),
                        )
                    }
                    _ => {}
                }
            });
            widgets::hint(
                ui,
                tr(
                    "Updates come from this project's GitHub releases and are checked against their published SHA-256.",
                    "Yeniləmələr layihənin GitHub buraxılışlarından yüklənir və dərc olunmuş SHA-256 ilə yoxlanılır.",
                ),
            );
        });
    }

    /// The server is set in one place, Manage, for both directions; this
    /// only shows it.
    fn server_card(&mut self, ui: &mut Ui) {
        if !host_side() {
            return self.server_editor(ui);
        }
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::GLOBE, tr("Server", "Server"));
            match &self.cfg.server {
                Some(s) => {
                    egui::Grid::new("server").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                        widgets::hint(ui, tr("Address", "Ünvan"));
                        ui.label(&s.addr);
                        ui.end_row();
                        widgets::hint(ui, tr("Fingerprint", "Barmaq izi"));
                        ui.label(RichText::new(widgets::short_fp(&s.fingerprint)).monospace());
                        ui.end_row();
                    });
                }
                None => widgets::hint(
                    ui,
                    tr(
                        "Not set: only computers on this network can be reached.",
                        "Təyin edilməyib: yalnız bu şəbəkədəki kompüterlərə qoşulmaq mümkündür.",
                    ),
                ),
            }
            ui.horizontal(|ui| {
                if ui
                    .button(format!(
                        "{}   {}",
                        icon::SHIELD,
                        tr("Change in Manage this computer…", "«Bu kompüteri idarə et» bölməsində dəyiş…")
                    ))
                    .clicked()
                {
                    self.manage_error = sys::relaunch_elevated("--manage").err().map(|e| format!("{e:#}"));
                }
                widgets::hint(ui, tr("One setting for both directions.", "Hər iki istiqamət üçün bir ayar."));
            });
            if let Some(e) = &self.manage_error {
                widgets::error(ui, e);
            }
        });
    }

    /// Without Manage (macOS, Linux) the server is set here. Only the name is
    /// needed: the fingerprint is fetched over HTTPS (Advanced: type it).
    fn server_editor(&mut self, ui: &mut Ui) {
        use rider_core::rendezvous::ServerRef;
        let saved = self.cfg.server.as_ref().map(|s| (s.addr.clone(), s.fingerprint.clone())).unwrap_or_default();
        // A finished check: save what it found.
        let checked = self.server_check.as_ref().and_then(|rx| rx.try_recv().ok());
        if checked.is_some() {
            self.server_check = None;
        }
        let checking = self.server_check.is_some();
        if checking {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        let edit = self.server_edit.get_or_insert_with(|| saved.clone());
        let (mut save, mut clear, mut typed) = (false, false, false);
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::GLOBE, tr("Server", "Server"));
            egui::Grid::new("server_edit").num_columns(2).spacing([24.0, 8.0]).show(ui, |ui| {
                widgets::hint(ui, tr("Address", "Ünvan"));
                typed |= ui
                    .add(TextEdit::singleline(&mut edit.0).hint_text("rider.example.com").desired_width(f32::INFINITY))
                    .changed();
                ui.end_row();
            });
            egui::CollapsingHeader::new(tr("Advanced", "Ətraflı")).id_salt("server_advanced").show(ui, |ui| {
                widgets::hint(
                    ui,
                    tr(
                        "Fingerprint: only for a server without a name (an IP address). Otherwise it is checked over HTTPS by itself.",
                        "Barmaq izi: yalnız adı olmayan server (IP ünvanı) üçün. Əks halda o, HTTPS ilə avtomatik yoxlanılır.",
                    ),
                );
                typed |= ui
                    .add(
                        TextEdit::singleline(&mut edit.1)
                            .hint_text("SHA-256")
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY),
                    )
                    .changed();
            });
            // An old message no longer fits what is being typed.
            if typed {
                self.server_msg = None;
            }
            ui.horizontal(|ui| {
                let changed = (edit.0.trim(), edit.1.trim()) != (saved.0.as_str(), saved.1.as_str());
                let ok = changed && !edit.0.trim().is_empty() && !checking;
                save = widgets::primary(ui, ok, tr("Save", "Yadda saxla")).clicked();
                if self.cfg.server.is_some() {
                    clear = ui.add_enabled(!checking, egui::Button::new(tr("Remove", "Sil"))).clicked();
                }
                if checking {
                    ui.spinner();
                    widgets::hint(ui, tr("Checking the server…", "Server yoxlanılır…"));
                }
            });
            widgets::hint(
                ui,
                tr(
                    "Type the server's name, e.g. rider.example.com. Without a server, only computers on this network can be reached.",
                    "Serverin adını yazın, məsələn, rider.example.com. Server olmadan yalnız bu şəbəkədəki kompüterlərə qoşulmaq mümkündür.",
                ),
            );
            match &self.server_msg {
                Some(Ok(m)) => widgets::success(ui, m),
                Some(Err(e)) => widgets::error(ui, e),
                None => {}
            }
        });
        if save {
            // A typed fingerprint is used as given; otherwise the name is checked over HTTPS.
            let (addr, fp) = (edit.0.trim().to_owned(), edit.1.trim().to_owned());
            let typed = (!fp.is_empty() && fp != saved.1).then_some(fp);
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let verified = typed.is_none();
                let r = match typed {
                    Some(fp) => ServerRef::new(&addr, &fp),
                    None => ServerRef::discover(&addr),
                };
                let _ = tx.send((r.map_err(|e| format!("{e:#}")), verified));
                ctx.request_repaint();
            });
            self.server_check = Some(rx);
            self.server_msg = None;
            return;
        }
        let (result, verified) = match (clear, checked) {
            (true, _) => (Ok(None), false),
            (false, Some((r, verified))) => (r.map(Some), verified),
            (false, None) => return,
        };
        let result = result.and_then(|server| {
            let mut cfg = ClientConfig::load().map_err(|e| format!("{e:#}"))?;
            cfg.server = server.clone();
            cfg.save().map_err(|e| format!("{e:#}"))?;
            Ok(server)
        });
        self.server_msg = Some(match result {
            Ok(server) => {
                self.server_edit =
                    Some(server.as_ref().map(|s| (s.addr.clone(), s.fingerprint.clone())).unwrap_or_default());
                self.cfg.server = server;
                self.cfg_mtime = config_mtime();
                self.server_error = None;
                self.polled = None;
                Ok(if clear {
                    tr("Server removed.", "Server silindi.")
                } else if verified {
                    tr("Saved. The server was verified over HTTPS.", "Yadda saxlanıldı. Server HTTPS ilə yoxlanıldı.")
                } else {
                    tr("Saved.", "Yadda saxlanıldı.")
                }
                .into())
            }
            Err(e) => Err(e),
        });
    }

    fn add_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.add else { return };
        let mut start = false;
        let mut close = false;
        let mut reply = None;
        let shape = (std::mem::discriminant(&d.stage), matches!(&d.stage, AddStage::Input { error: Some(_) }));
        widgets::modal(ctx, "add-computer", shape).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(widgets::title(tr("Add a computer", "Kompüter əlavə et"), 19.0));
            ui.add_space(4.0);
            match &d.stage {
                AddStage::Input { error } => {
                    ui.label(tr(
                        "Code or address shown on the other computer",
                        "Digər kompüterdə göstərilən kod və ya ünvan",
                    ));
                    let r = ui.add(
                        TextEdit::singleline(&mut d.target)
                            .hint_text(tr("e.g. SF6-CK6 or 192.168.1.20", "məs., SF6-CK6 və ya 192.168.1.20"))
                            .font(egui::FontId::monospace(18.0))
                            .desired_width(f32::INFINITY),
                    );
                    if d.target.is_empty() {
                        r.request_focus();
                    }
                    ui.label(tr("Name in your list (optional)", "Siyahıdakı adı (istəyə bağlı)"));
                    ui.add(
                        TextEdit::singleline(&mut d.name)
                            .hint_text(tr("e.g. Office PC", "məs., Ofis kompüteri"))
                            .desired_width(f32::INFINITY),
                    );
                    if let Some(e) = error {
                        widgets::error(ui, e);
                    }
                    widgets::dialog_buttons(ui, |ui| {
                        start = widgets::primary(ui, !d.target.trim().is_empty(), tr("Continue", "Davam et")).clicked()
                            || (!d.target.trim().is_empty() && ui.input(|i| i.key_pressed(Key::Enter)));
                        close =
                            ui.button(tr("Cancel", "Ləğv et")).clicked() || ui.input(|i| i.key_pressed(Key::Escape));
                    });
                }
                AddStage::Connecting => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tr("Connecting…", "Qoşulur…"));
                    });
                    widgets::dialog_buttons(ui, |ui| close = ui.button(tr("Cancel", "Ləğv et")).clicked());
                }
                AddStage::Confirm { prompt, .. } => {
                    let (host, ip) = (&prompt.host_name, prompt.remote.ip());
                    ui.add(
                        egui::Label::new(trf!(
                            "\"{host}\" ({ip}) shows a verification code:",
                            "\"{host}\" ({ip}) təsdiq kodu göstərir:"
                        ))
                        .wrap(),
                    );
                    widgets::big_code(ui, &prompt.code);
                    widgets::hint(
                        ui,
                        tr(
                            "Only continue if it shows exactly this code. Otherwise someone may be listening in.",
                            "Yalnız orada məhz bu kod göstərilirsə davam edin. Əks halda əlaqəni kimsə dinləyə bilər.",
                        ),
                    );
                    widgets::dialog_buttons(ui, |ui| {
                        if widgets::primary(ui, true, tr("Yes, it matches", "Bəli, eynidir")).clicked() {
                            reply = Some(true);
                        }
                        if ui.button(tr("No", "Xeyr")).clicked() {
                            reply = Some(false);
                        }
                    });
                }
                AddStage::Waiting { host } => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(trf!(
                            "Waiting for someone at \"{host}\" to confirm…",
                            "\"{host}\" tərəfində təsdiq gözlənilir…"
                        ));
                    });
                    widgets::dialog_buttons(ui, |ui| close = ui.button(tr("Cancel", "Ləğv et")).clicked());
                }
                AddStage::Done { name } => {
                    widgets::success(ui, &trf!("\"{name}\" was added.", "\"{name}\" əlavə edildi."));
                    widgets::hint(
                        ui,
                        tr(
                            "Connect to it any time from your list, with its PIN.",
                            "Ona istənilən vaxt siyahınızdan PIN kodu ilə qoşula bilərsiniz.",
                        ),
                    );
                    widgets::dialog_buttons(ui, |ui| {
                        close = widgets::primary(ui, true, tr("Done", "Hazırdır")).clicked()
                            || ui.input(|i| i.key_pressed(Key::Enter));
                    });
                }
            }
        });
        if let Some(yes) = reply
            && let AddStage::Confirm { prompt, answer } =
                std::mem::replace(&mut d.stage, AddStage::Input { error: None })
        {
            let _ = answer.send(yes);
            d.stage = if yes { AddStage::Waiting { host: prompt.host_name } } else { AddStage::Connecting };
        }
        if close {
            // A pairing still running sees its question dropped and gives up.
            self.add = None;
        } else if start {
            self.start_pairing();
        }
    }

    fn connect_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.connect else { return };
        let mut go = false;
        let mut close = false;
        widgets::modal(ctx, "connect", d.error.is_some()).show(ctx, |ui| {
            ui.set_width(380.0);
            let name = &d.name;
            ui.label(widgets::title(trf!("Connect to \"{name}\"", "\"{name}\" kompüterinə qoşulma"), 19.0));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::KEY).color(ACCENT));
                ui.label(tr("PIN of that computer", "Həmin kompüterin PIN kodu"));
            });
            let r = ui.add(TextEdit::singleline(&mut d.pin).password(true).desired_width(f32::INFINITY));
            r.request_focus();
            if let Some(e) = &d.error {
                widgets::error(ui, e);
            }
            ui.add_space(2.0);
            ui.checkbox(&mut d.remember, tr("Remember PIN on this device", "PIN kodu bu cihazda yadda saxla"));
            widgets::hint(
                ui,
                tr(
                    "Next time Connect opens the session directly. Anyone who can sign in to your account on this device could too.",
                    "Növbəti dəfə «Qoşul» seansı birbaşa açacaq. Bu cihazda hesabınıza daxil ola bilən hər kəs də bunu edə bilər.",
                ),
            );
            widgets::dialog_buttons(ui, |ui| {
                go = widgets::primary(ui, !d.pin.is_empty(), tr("Connect", "Qoşul")).clicked()
                    || (!d.pin.is_empty() && ui.input(|i| i.key_pressed(Key::Enter)));
                close = ui.button(tr("Cancel", "Ləğv et")).clicked() || ui.input(|i| i.key_pressed(Key::Escape));
            });
        });
        if go {
            if let Some(d) = self.connect.take() {
                self.start_session(d, false);
            }
        } else if close {
            self.connect = None;
        }
    }

    /// Quick connect (helper side): enter the code, wait for Allow.
    fn quick_dialog(&mut self, ctx: &egui::Context) {
        let has_server = self.cfg.server.is_some();
        let Some(q) = &mut self.quick else { return };
        let mut go = false;
        let mut close = false;
        let shape = (std::mem::discriminant(&q.stage), matches!(&q.stage, QuickStage::Input { error: Some(_) }));
        widgets::modal(ctx, "quick-connect", shape).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(widgets::title(tr("Quick connect", "Sürətli qoşulma"), 19.0));
            ui.add_space(4.0);
            match &q.stage {
                QuickStage::Input { error } => {
                    if !has_server {
                        widgets::error(
                            ui,
                            if host_side() {
                                tr(
                                    "Quick connect needs a server: set it in This computer → Manage this computer → Server.",
                                    "Sürətli qoşulma üçün server lazımdır: onu «Bu kompüter» → «Bu kompüteri idarə et» → «Server» bölməsində təyin edin.",
                                )
                            } else {
                                tr(
                                    "Quick connect needs a server: set it in Settings → Server.",
                                    "Sürətli qoşulma üçün server lazımdır: onu «Parametrlər» → «Server» bölməsində təyin edin.",
                                )
                            },
                        );
                    }
                    ui.label(tr(
                        "One-time code shown on the other computer",
                        "Digər kompüterdə göstərilən birdəfəlik kod",
                    ));
                    let r = ui.add(
                        TextEdit::singleline(&mut q.code)
                            .hint_text(tr("e.g. SF6-CK6", "məs., SF6-CK6"))
                            .font(egui::FontId::monospace(18.0))
                            .desired_width(f32::INFINITY),
                    );
                    if q.code.is_empty() {
                        r.request_focus();
                    }
                    widgets::hint(
                        ui,
                        tr(
                            "The person there clicks Get a one-time code under This computer, then allows your connection.",
                            "Orada olan şəxs «Bu kompüter» bölməsində «Birdəfəlik kod al» düyməsini klikləyir, sonra qoşulmanıza icazə verir.",
                        ),
                    );
                    if let Some(e) = error {
                        widgets::error(ui, e);
                    }
                    let ok = has_server && !q.code.trim().is_empty();
                    widgets::dialog_buttons(ui, |ui| {
                        go = widgets::primary(ui, ok, tr("Connect", "Qoşul")).clicked()
                            || (ok && ui.input(|i| i.key_pressed(Key::Enter)));
                        close =
                            ui.button(tr("Cancel", "Ləğv et")).clicked() || ui.input(|i| i.key_pressed(Key::Escape));
                    });
                }
                QuickStage::Connecting => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tr("Connecting…", "Qoşulur…"));
                    });
                    widgets::dialog_buttons(ui, |ui| close = ui.button(tr("Cancel", "Ləğv et")).clicked());
                }
                QuickStage::Waiting { host, code } => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.add(
                            egui::Label::new(trf!(
                                "Waiting for the person at \"{host}\" to allow the connection…",
                                "\"{host}\" kompüterindəki şəxsin icazəsi gözlənilir…"
                            ))
                            .wrap(),
                        );
                    });
                    if let Some(code) = code {
                        widgets::hint(
                            ui,
                            tr(
                                "Verification code (they see the same one):",
                                "Təsdiq kodu (qarşı tərəf də eyni kodu görür):",
                            ),
                        );
                        widgets::big_code(ui, code);
                    }
                    widgets::dialog_buttons(ui, |ui| close = ui.button(tr("Cancel", "Ləğv et")).clicked());
                }
                QuickStage::Connected { host, route } => {
                    widgets::success(
                        ui,
                        &trf!(
                            "Connected to \"{host}\" ({route}). The session is open in its own window.",
                            "\"{host}\" kompüterinə qoşuldunuz ({route}). Seans ayrıca pəncərədə açıqdır."
                        ),
                    );
                    widgets::hint(
                        ui,
                        tr(
                            "It ends when you disconnect or the other side clicks its red bar. The code no longer works.",
                            "Seans siz bağlantını kəsdikdə və ya qarşı tərəf qırmızı zolağa kliklədikdə bitir. Kod artıq etibarsızdır.",
                        ),
                    );
                    widgets::dialog_buttons(ui, |ui| {
                        // Hiding the dialog leaves the session running.
                        if widgets::primary(ui, true, tr("OK", "Oldu")).clicked() {
                            close = true;
                        }
                    });
                }
            }
        });
        if go {
            self.start_quick();
        } else if close
            && let Some(mut q) = self.quick.take()
            && !matches!(q.stage, QuickStage::Connected { .. })
        {
            q.stop();
        }
    }

    /// Get help (the person being helped): the code, then Allow / Decline.
    fn support_dialog(&mut self, ctx: &egui::Context) {
        let Some(s) = &mut self.support else { return };
        let mut close = false;
        let mut answer = None;
        let mut again = false;
        Modal::new(Id::unique("support")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(widgets::title(tr("One-time code", "Birdəfəlik kod"), 19.0));
            ui.add_space(4.0);
            match s.stage.clone() {
                SupportStage::Waiting => {
                    match &s.code {
                        Some(code) => {
                            ui.label(tr(
                                "Tell this code to the person who is helping you:",
                                "Bu kodu sizə kömək edən şəxsə bildirin:",
                            ));
                            widgets::big_code(ui, code);
                            widgets::hint(
                                ui,
                                tr(
                                    "They enter it under Quick connect. It works once and expires in 10 minutes.",
                                    "O, kodu «Sürətli qoşulma» bölməsində daxil edir. Kod bir dəfə işləyir və 10 dəqiqəyə qüvvədən düşür.",
                                ),
                            );
                        }
                        None if s.server_error.is_none() => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(tr("Getting a code from the server…", "Serverdən kod alınır…"));
                            });
                        }
                        None => {}
                    }
                    if let Some(e) = &s.server_error {
                        widgets::error(ui, &format!("{}: {e}", tr("Server", "Server")));
                    }
                    if let Some(n) = &s.note {
                        widgets::hint(ui, n);
                    }
                    if s.code.is_some() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            widgets::hint(ui, tr("Waiting for the connection…", "Qoşulma gözlənilir…"));
                        });
                    }
                    widgets::hint(
                        ui,
                        tr(
                            "In this kind of session, windows running as administrator and UAC prompts can't be controlled.",
                            "Bu cür seansda administrator hüquqları ilə işləyən pəncərələri və UAC sorğularını idarə etmək mümkün deyil.",
                        ),
                    );
                    widgets::dialog_buttons(ui, |ui| close = ui.button(tr("Cancel", "Ləğv et")).clicked());
                }
                SupportStage::Asking { device, code } => {
                    ui.add(
                        egui::Label::new(
                            RichText::new(trf!(
                                "\"{device}\" wants to see and control this screen.",
                                "\"{device}\" bu ekranı görmək və idarə etmək istəyir."
                            ))
                            .strong(),
                        )
                        .wrap(),
                    );
                    widgets::hint(
                        ui,
                        tr(
                            "Verification code (the helper sees the same one):",
                            "Təsdiq kodu (kömək edən şəxs də eyni kodu görür):",
                        ),
                    );
                    widgets::big_code(ui, &code);
                    widgets::hint(
                        ui,
                        tr(
                            "Allow only if you know who this is. You can end the session at any time by clicking the red bar at the top of the screen.",
                            "Yalnız kim olduğunu bilirsinizsə icazə verin. Seansı istənilən vaxt ekranın yuxarısındakı qırmızı zolağa klikləməklə bitirə bilərsiniz.",
                        ),
                    );
                    widgets::dialog_buttons(ui, |ui| {
                        if widgets::primary(ui, true, tr("Allow", "İcazə ver")).clicked() {
                            answer = Some(true);
                        }
                        if ui.button(tr("Decline", "İmtina et")).clicked() {
                            answer = Some(false);
                        }
                    });
                }
                SupportStage::Active { device } => {
                    ui.horizontal(|ui| {
                        widgets::dot(ui, Some(GREEN));
                        ui.add(
                            egui::Label::new(trf!(
                                "\"{device}\" is connected and can control this computer.",
                                "\"{device}\" qoşulub və bu kompüteri idarə edə bilər."
                            ))
                            .wrap(),
                        );
                    });
                    widgets::hint(
                        ui,
                        tr(
                            "To end it, click End session here or the red bar at the top of the screen.",
                            "Bitirmək üçün burada «Seansı bitir» düyməsini və ya ekranın yuxarısındakı qırmızı zolağı klikləyin.",
                        ),
                    );
                    widgets::dialog_buttons(ui, |ui| {
                        close = widgets::danger(ui, tr("End session", "Seansı bitir")).clicked();
                    });
                }
                SupportStage::Ended => {
                    widgets::success(
                        ui,
                        tr(
                            "The session has ended. The code no longer works.",
                            "Seans başa çatdı. Kod artıq etibarsızdır.",
                        ),
                    );
                    widgets::dialog_buttons(ui, |ui| close = widgets::primary(ui, true, tr("Close", "Bağla")).clicked());
                }
                SupportStage::Expired => {
                    widgets::hint(
                        ui,
                        tr("The code expired without being used.", "Kod istifadə edilmədən qüvvədən düşdü."),
                    );
                    widgets::dialog_buttons(ui, |ui| {
                        again = widgets::primary(ui, true, tr("Get a new code", "Yeni kod al")).clicked();
                        close = ui.button(tr("Close", "Bağla")).clicked();
                    });
                }
                SupportStage::Failed(e) => {
                    widgets::error(ui, &e);
                    widgets::dialog_buttons(ui, |ui| close = ui.button(tr("Close", "Bağla")).clicked());
                }
            }
        });
        if let Some(yes) = answer {
            s.send(if yes { "y" } else { "n" });
        }
        if close || again {
            // Ends the one-time host (and a running session) at once.
            s.send("quit");
            self.support = None;
        }
        if again {
            self.start_support();
        }
    }

    /// Closing the window keeps the app in the tray (told once).
    fn close_to_tray(&mut self, ctx: &egui::Context) {
        let Some(tray) = &self.tray else { return };
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        tray::hide_main();
        let hint = "TrayHintShown";
        if !sys::flag(hint) {
            sys::set_flag(hint);
            tray.notify(
                tr("Remote Desktop Rider is still running", "Remote Desktop Rider işləməyə davam edir"),
                tr(
                    "It stays here in the notification area. Right-click the icon and choose Quit to exit.",
                    "Proqram bildiriş sahəsində qalır. Çıxmaq üçün nişanın üzərində sağ klikləyib «Çıxış» seçin.",
                ),
            );
        }
    }
}

/// `Connected to "name" (direct 1.2.3.4:47800).` → (name, route).
fn parse_connected(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("Connected to \"")?;
    let (name, tail) = rest.rsplit_once("\" (")?;
    Some((name.to_owned(), tail.trim_end_matches(").").to_owned()))
}

fn parse_support_event(line: &str) -> Option<SupportEvent> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned();
    Some(match v.get("event")?.as_str()? {
        "registered" => SupportEvent::Code(v.get("code").and_then(|c| c.as_str()).map(str::to_owned)),
        "server_error" => SupportEvent::ServerError(s("message")),
        "support_request" => SupportEvent::Request { device: s("device"), code: s("code") },
        "support_started" => SupportEvent::Started(s("device")),
        "support_ended" => SupportEvent::Ended,
        "support_cancelled" => SupportEvent::Cancelled,
        "declined" => SupportEvent::Declined,
        "expired" => SupportEvent::Expired,
        _ => return None,
    })
}

impl eframe::App for ClientApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        crate::sys::smooth_monitor_moves(frame);
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
        if crate::tray::take_install_request() {
            self.updater.install_offered();
        }
        // Keep timers (online polling, service state) ticking while shown.
        ctx.request_repaint_after(Duration::from_secs(2));

        let side = if ctx.global_style().visuals.dark_mode {
            egui::Color32::from_rgb(30, 31, 36)
        } else {
            egui::Color32::from_rgb(232, 234, 239)
        };
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
                    Tab::Settings => self.settings(ui),
                }
            });
        self.add_dialog(&ctx);
        self.connect_dialog(&ctx);
        self.quick_dialog(&ctx);
        self.support_dialog(&ctx);
    }
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

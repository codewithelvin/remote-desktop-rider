//! Main window (no administrator rights needed): the computers this device
//! can control, adding one, connecting, and this device's settings.

use crate::pins;
use crate::sys::{self, RemoteAccess};
use crate::tray::{self, Tray};
use crate::widgets::{self, ACCENT, AMBER, Bus, GREEN, GREY, Tx, icon};
use eframe::egui::{self, Align, Id, Key, Layout, Modal, RichText, ScrollArea, TextEdit, TextureHandle, Ui, vec2};
use rider_client::PairPrompt;
use rider_client::config::{ClientConfig, KnownHost};
use rider_core::identity::{Identity, fp_from_hex};
use rider_core::rendezvous::ServerRef;
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use windows::core::w;

/// How often the server is asked which computers are online.
const POLL_EVERY: Duration = Duration::from_secs(15);

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
    remove: Option<String>,
    /// Computers with a remembered PIN.
    saved_pins: HashSet<String>,
    sessions: Vec<Session>,
    next_session: u64,
    remote: RemoteAccess,
    remote_checked: Instant,
    manage_error: Option<String>,
    server_addr: String,
    server_fp: String,
    settings_msg: Option<Result<String, String>>,
    autostart: bool,
    /// Frames left before hiding a window started with `--tray`.
    hide_in: u8,
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
            Err(e) => (ClientConfig::default(), None, Some(format!("Cannot load this device's settings: {e:#}"))),
        };
        let (server_addr, server_fp) =
            cfg.server.as_ref().map(|s| (s.addr.clone(), s.fingerprint.clone())).unwrap_or_default();
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
            remove: None,
            saved_pins: HashSet::new(),
            sessions: Vec::new(),
            next_session: 0,
            remote: sys::remote_access(),
            remote_checked: Instant::now(),
            manage_error: None,
            server_addr,
            server_fp,
            settings_msg: None,
            autostart: sys::autostart(),
            hide_in: if hidden { 2 } else { 0 },
        };
        app.refresh_saved_pins();
        app
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
        cfg.save().map_err(|e| format!("cannot save: {e:#}"))?;
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
                Msg::Session { id, update } => {
                    let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) else { continue };
                    match update {
                        SessionUpdate::Connected(route) => {
                            s.connected = Some(route);
                            if let Some(pin) = s.remember.take() {
                                let host = s.host.clone();
                                match pins::save(&host, &pin) {
                                    Ok(()) => {
                                        self.saved_pins.insert(host);
                                    }
                                    Err(e) => self.server_error = Some(format!("Cannot remember the PIN: {e:#}")),
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
                            // The remembered PIN no longer works (changed on
                            // that computer): forget it and ask.
                            let rejected = s.stderr.contains("wrong PIN") || s.stderr.contains("authentication failed");
                            if !ok && s.saved_pin && s.connected.is_none() && rejected {
                                let host = s.host.clone();
                                self.sessions.retain(|s| s.id != id);
                                self.forget_pin(&host);
                                let name =
                                    self.cfg.hosts.iter().find(|h| h.fingerprint == host).map(|h| h.name.clone());
                                self.connect = Some(ConnectDialog {
                                    fingerprint: host,
                                    name: name.unwrap_or_default(),
                                    pin: String::new(),
                                    remember: true,
                                    error: Some("The saved PIN no longer works. Enter the current one.".into()),
                                });
                            }
                            self.reload(); // last-connected time changed
                        }
                    }
                }
            }
        }
        // Finished sessions without an error disappear on their own.
        self.sessions.retain(|s| s.finished != Some(true));
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
                error: Some("Enter the pairing code or address shown on the other computer.".into()),
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
        self.next_session += 1;
        let id = self.next_session;
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
                session.stderr = format!("Error: cannot start rider-client.exe: {e}");
                session.finished = Some(false);
                self.sessions.push(session);
                return;
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = writeln!(stdin, "{}", d.pin);
        }
        let tx: Tx<Msg> = self.bus.tx();
        if let Some(out) = child.stdout.take() {
            let tx = tx.clone();
            sys::lines(out, move |line| {
                // `Connected to "name" (direct 1.2.3.4:47800).`
                if line.starts_with("Connected to ") {
                    let route = line.rsplit_once(" (").map_or("", |(_, r)| r.trim_end_matches(").")).to_owned();
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

    fn open_add(&mut self) {
        self.add =
            Some(AddDialog { target: String::new(), name: String::new(), stage: AddStage::Input { error: None } });
    }

    // ---------------------------------------------------------------- views

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new((logo.id(), vec2(28.0, 28.0))));
            }
            ui.label(widgets::title("Remote Desktop Rider", 15.0));
        });
        ui.add_space(18.0);
        for (tab, ic, text) in [
            (Tab::Computers, icon::PC, "Computers"),
            (Tab::ThisComputer, icon::HOME, "This computer"),
            (Tab::Settings, icon::SETTINGS, "Settings"),
        ] {
            if widgets::nav_item(ui, ic, text, self.tab == tab) {
                self.tab = tab;
            }
        }
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(12.0);
            widgets::hint(ui, &format!("   Version {}", env!("CARGO_PKG_VERSION")));
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let (color, text) = match (&self.cfg.server, &self.server_error) {
                    (None, _) => (None, "No server: this network only"),
                    (Some(_), Some(_)) => (Some(AMBER), "Server unreachable"),
                    (Some(_), None) if self.polled.is_none() && !self.cfg.hosts.is_empty() => (Some(GREY), "Server…"),
                    (Some(_), None) => (Some(GREEN), "Server connected"),
                };
                widgets::dot(ui, color);
                widgets::hint(ui, text);
            });
        });
    }

    fn computers(&mut self, ui: &mut Ui) {
        let has_hosts = !self.cfg.hosts.is_empty();
        let mut add = false;
        widgets::page_header(ui, "Computers", "Computers this device can control.", |ui| {
            if has_hosts {
                add = widgets::primary(ui, true, &format!("{}   Add computer", icon::ADD)).clicked();
            }
        });
        if let Some(e) = &self.server_error {
            widgets::card(ui, |ui| {
                widgets::error(ui, &format!("The server can't be reached, so online status is unknown. {e}"));
            });
            ui.add_space(8.0);
        }

        if !has_hosts {
            widgets::card(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(18.0);
                    ui.label(RichText::new(icon::PC).size(44.0).color(ACCENT));
                    ui.label(widgets::title("No computers yet", 18.0));
                    widgets::hint(
                        ui,
                        "On the computer you want to control, open Remote Desktop Rider, go to\n\
                         This computer > Manage this computer, and click Add a device.",
                    );
                    ui.add_space(4.0);
                    add = widgets::primary(ui, true, &format!("{}   Add computer", icon::ADD)).clicked();
                    ui.add_space(18.0);
                });
            });
        }
        if add {
            self.open_add();
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
                            Some(true) => widgets::pill(ui, "Online", GREEN),
                            Some(false) => widgets::pill(ui, "Offline", GREY),
                            None if h.addr.is_empty() => {}
                            None => widgets::hint(ui, &h.addr),
                        }
                        let last = h
                            .last_connected
                            .map_or("Never connected".into(), |t| format!("Last connected {}", widgets::ago(t)));
                        widgets::hint(ui, &last);
                    });
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.remove.as_deref() == Some(h.fingerprint.as_str()) {
                        if widgets::danger(ui, "Remove").clicked() {
                            if let Err(e) = self.edit(|cfg| {
                                cfg.forget(&h.fingerprint);
                            }) {
                                self.server_error = Some(e);
                            }
                            self.forget_pin(&h.fingerprint);
                            self.remove = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.remove = None;
                        }
                        ui.label("Remove from this list?");
                    } else {
                        if widgets::icon_button(ui, icon::DELETE, "Remove from this list").clicked() {
                            self.remove = Some(h.fingerprint.clone());
                        }
                        let label = if in_session { "In session" } else { "Connect" };
                        if widgets::primary(ui, !in_session, label).clicked() {
                            self.connect_to(h.fingerprint.clone(), h.name.clone());
                        }
                        if self.saved_pins.contains(&h.fingerprint)
                            && widgets::icon_button(ui, icon::KEY, "PIN remembered on this device. Click to forget it.")
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
                        ui.label("Connecting…");
                    }
                    (None, Some(route)) => {
                        widgets::dot(ui, Some(GREEN));
                        ui.label(format!("Connected ({route}). The session is open in its own window."));
                    }
                    (Some(_), _) => {
                        widgets::error(ui, error);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("Dismiss").clicked() {
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
        widgets::page_header(ui, "This computer", &format!("\"{}\"", self.cfg.device_name), |_| {});
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::SHIELD, "Remote access");
            ui.horizontal(|ui| {
                let (color, state, text) = match self.remote {
                    RemoteAccess::On => {
                        (GREEN, "On", "Devices you added can control this computer, even at the sign-in screen.")
                    }
                    RemoteAccess::Stopped => (AMBER, "Stopped", "Remote access is set up but not running."),
                    RemoteAccess::Off => (GREY, "Off", "Nobody can connect to this computer."),
                };
                widgets::pill(ui, state, color);
                ui.label(text);
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if widgets::primary(ui, true, &format!("{}   Manage this computer…", icon::SHIELD)).clicked() {
                    self.manage_error = sys::relaunch_elevated("--manage").err().map(|e| format!("{e:#}"));
                }
                widgets::hint(ui, "Windows asks for permission once.");
            });
            if let Some(e) = &self.manage_error {
                widgets::error(ui, e);
            }
        });
        ui.add_space(10.0);
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::INFO, "Let another device control this computer");
            for (n, step) in [
                "Click Manage this computer above.",
                "Set a PIN and turn on remote access.",
                "Click Add a device, then on the other device click Add computer and enter the code shown.",
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
    }

    fn settings(&mut self, ui: &mut Ui) {
        widgets::page_header(ui, "Settings", "", |_| {});
        ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            widgets::card(ui, |ui| {
                widgets::section(ui, icon::PC, "This device");
                egui::Grid::new("device").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                    widgets::hint(ui, "Name");
                    ui.label(&self.cfg.device_name);
                    ui.end_row();
                    widgets::hint(ui, "Fingerprint");
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
            self.server_card(ui);
            ui.add_space(10.0);
            widgets::card(ui, |ui| {
                widgets::section(ui, icon::PLAY, "Startup");
                if ui.checkbox(&mut self.autostart, "Start Remote Desktop Rider when I sign in").changed()
                    && let Err(e) = sys::set_autostart(self.autostart)
                {
                    self.autostart = sys::autostart();
                    self.settings_msg = Some(Err(format!("{e:#}")));
                }
                widgets::hint(
                    ui,
                    "Closing the window keeps the app in the notification area. Use Quit in its menu to exit.",
                );
            });
        });
    }

    fn server_card(&mut self, ui: &mut Ui) {
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::GLOBE, "Server");
            widgets::hint(
                ui,
                "Your rider-server lets you reach computers on other networks. `rider-server info` prints both values.",
            );
            egui::Grid::new("server").num_columns(2).spacing([24.0, 8.0]).show(ui, |ui| {
                widgets::hint(ui, "Address");
                ui.add(TextEdit::singleline(&mut self.server_addr).hint_text("rider.example.com").desired_width(360.0));
                ui.end_row();
                widgets::hint(ui, "Fingerprint");
                ui.add(
                    TextEdit::singleline(&mut self.server_fp)
                        .hint_text("64 hex characters")
                        .desired_width(360.0)
                        .font(egui::TextStyle::Monospace),
                );
                ui.end_row();
            });
            ui.horizontal(|ui| {
                let changed = self.cfg.server.as_ref().map_or(!self.server_addr.trim().is_empty(), |s| {
                    s.addr != self.server_addr.trim() || s.fingerprint != self.server_fp.trim()
                });
                if widgets::primary(ui, changed, "Save").clicked() {
                    self.settings_msg = Some(match ServerRef::new(self.server_addr.trim(), self.server_fp.trim()) {
                        Ok(s) => self.edit(|cfg| cfg.server = Some(s)).map(|()| "Saved.".into()),
                        Err(e) => Err(format!("{e:#}")),
                    });
                }
                if self.cfg.server.is_some() && ui.button("Don't use a server").clicked() {
                    self.server_addr.clear();
                    self.server_fp.clear();
                    self.settings_msg = Some(self.edit(|cfg| cfg.server = None).map(|()| "Server removed.".into()));
                }
                match &self.settings_msg {
                    Some(Ok(m)) => widgets::success(ui, m),
                    Some(Err(e)) => widgets::error(ui, e),
                    None => {}
                }
            });
        });
    }

    // -------------------------------------------------------------- dialogs

    fn add_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.add else { return };
        let mut start = false;
        let mut close = false;
        let mut reply = None;
        Modal::new(Id::unique("add-computer")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(widgets::title("Add a computer", 19.0));
            ui.add_space(4.0);
            match &d.stage {
                AddStage::Input { error } => {
                    ui.label("Code or address shown on the other computer");
                    let r = ui.add(
                        TextEdit::singleline(&mut d.target)
                            .hint_text("e.g. SF6-CK6 or 192.168.1.20")
                            .font(egui::FontId::monospace(18.0))
                            .desired_width(f32::INFINITY),
                    );
                    if d.target.is_empty() {
                        r.request_focus();
                    }
                    ui.label("Name in your list (optional)");
                    ui.add(TextEdit::singleline(&mut d.name).hint_text("e.g. Office PC").desired_width(f32::INFINITY));
                    if let Some(e) = error {
                        widgets::error(ui, e);
                    }
                    widgets::dialog_buttons(ui, |ui| {
                        start = widgets::primary(ui, !d.target.trim().is_empty(), "Continue").clicked()
                            || (!d.target.trim().is_empty() && ui.input(|i| i.key_pressed(Key::Enter)));
                        close = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
                    });
                }
                AddStage::Connecting => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Connecting…");
                    });
                    widgets::dialog_buttons(ui, |ui| close = ui.button("Cancel").clicked());
                }
                AddStage::Confirm { prompt, .. } => {
                    ui.label(format!("\"{}\" ({}) shows a verification code:", prompt.host_name, prompt.remote.ip()));
                    widgets::big_code(ui, &prompt.code);
                    widgets::hint(
                        ui,
                        "Only continue if it shows exactly this code. Otherwise someone may be listening in.",
                    );
                    widgets::dialog_buttons(ui, |ui| {
                        if widgets::primary(ui, true, "Yes, it matches").clicked() {
                            reply = Some(true);
                        }
                        if ui.button("No").clicked() {
                            reply = Some(false);
                        }
                    });
                }
                AddStage::Waiting { host } => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Waiting for someone at \"{host}\" to confirm…"));
                    });
                    widgets::dialog_buttons(ui, |ui| close = ui.button("Cancel").clicked());
                }
                AddStage::Done { name } => {
                    widgets::success(ui, &format!("\"{name}\" was added."));
                    widgets::hint(ui, "Connect to it any time from your list, with its PIN.");
                    widgets::dialog_buttons(ui, |ui| {
                        close = widgets::primary(ui, true, "Done").clicked() || ui.input(|i| i.key_pressed(Key::Enter));
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
        Modal::new(Id::unique("connect")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(widgets::title(format!("Connect to \"{}\"", d.name), 19.0));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::KEY).color(ACCENT));
                ui.label("PIN of that computer");
            });
            let r = ui.add(TextEdit::singleline(&mut d.pin).password(true).desired_width(f32::INFINITY));
            r.request_focus();
            if let Some(e) = &d.error {
                widgets::error(ui, e);
            }
            ui.add_space(2.0);
            ui.checkbox(&mut d.remember, "Remember PIN on this device");
            widgets::hint(
                ui,
                "Next time Connect opens the session directly. Anyone who can use your Windows account here could too.",
            );
            widgets::dialog_buttons(ui, |ui| {
                go = widgets::primary(ui, !d.pin.is_empty(), "Connect").clicked()
                    || (!d.pin.is_empty() && ui.input(|i| i.key_pressed(Key::Enter)));
                close = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
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

    /// Closing the window keeps the app in the tray (told once).
    fn close_to_tray(&mut self, ctx: &egui::Context) {
        let Some(tray) = &self.tray else { return };
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        tray::hide_main();
        let hint = w!("TrayHintShown");
        if !sys::flag(hint) {
            sys::set_flag(hint);
            tray.notify(
                "Remote Desktop Rider is still running",
                "It stays here in the notification area. Right-click the icon and choose Quit to exit.",
            );
        }
    }
}

impl eframe::App for ClientApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
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
        self.poll_online();
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
                match self.tab {
                    Tab::Computers => self.computers(ui),
                    Tab::ThisComputer => self.this_computer(ui),
                    Tab::Settings => self.settings(ui),
                }
            });
        self.add_dialog(&ctx);
        self.connect_dialog(&ctx);
    }
}

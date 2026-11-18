//! Main window (no administrator rights needed): the computers this device
//! can control, adding one, connecting, and this device's settings.

use crate::sys::{self, RemoteAccess};
use crate::widgets::{self, Bus, GREEN, GREY, Tx};
use eframe::egui::{self, Align, Id, Key, Layout, Modal, RichText, TextEdit, Ui};
use rider_client::PairPrompt;
use rider_client::config::{ClientConfig, KnownHost};
use rider_core::identity::{Identity, fp_from_hex};
use rider_core::rendezvous::ServerRef;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

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
}

struct Session {
    id: u64,
    name: String,
    status: String,
    stderr: String,
    finished: Option<bool>,
}

pub struct ClientApp {
    rt: tokio::runtime::Runtime,
    bus: Bus<Msg>,
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
    sessions: Vec<Session>,
    next_session: u64,
    remote: RemoteAccess,
    remote_checked: Instant,
    manage_error: Option<String>,
    server_addr: String,
    server_fp: String,
    settings_msg: Option<Result<String, String>>,
}

impl ClientApp {
    pub fn new(ctx: &egui::Context) -> Self {
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
        Self {
            rt,
            bus: Bus::new(ctx),
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
            sessions: Vec::new(),
            next_session: 0,
            remote: sys::remote_access(),
            remote_checked: Instant::now(),
            manage_error: None,
            server_addr,
            server_fp,
            settings_msg: None,
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
                        SessionUpdate::Connected(line) => {
                            s.status = line;
                            self.reload(); // the viewer just saved the last-connected time
                        }
                        SessionUpdate::Stderr(line) => {
                            s.stderr.push_str(&line);
                            s.stderr.push('\n');
                        }
                        SessionUpdate::Exited(ok) => {
                            s.finished = Some(ok);
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

    fn start_session(&mut self, d: ConnectDialog) {
        // A new attempt replaces the error of the previous one.
        self.sessions.retain(|s| s.finished.is_none() || s.name != d.name);
        self.next_session += 1;
        let id = self.next_session;
        let mut session =
            Session { id, name: d.name.clone(), status: "Connecting…".into(), stderr: String::new(), finished: None };
        let spawned = sys::tool("rider-client.exe")
            .args(["connect", &d.fingerprint, "--pin-stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                session.stderr = format!("cannot start rider-client.exe: {e}");
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
                if let Some(rest) = line.strip_prefix("Connected to ") {
                    tx.send(Msg::Session { id, update: SessionUpdate::Connected(format!("Connected {rest}")) });
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

    // ---------------------------------------------------------------- views

    fn computers(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            widgets::heading(ui, "Computers you can control");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("+  Add computer").clicked() {
                    self.add = Some(AddDialog {
                        target: String::new(),
                        name: String::new(),
                        stage: AddStage::Input { error: None },
                    });
                }
            });
        });
        if let Some(e) = &self.server_error {
            widgets::error(ui, &format!("Server unreachable, online status unknown: {e}"));
        } else if self.cfg.server.is_none() {
            widgets::hint(ui, "No server set (Settings): computers are reachable on this network only.");
        }
        ui.separator();

        if self.cfg.hosts.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No computers yet").size(18.0));
                widgets::hint(
                    ui,
                    "On the computer you want to control, open Remote Desktop Rider, go to This computer \
                     and click Add a device. Then click \"Add computer\" here and enter the code it shows.",
                );
            });
        }

        let mut hosts = self.cfg.hosts.clone();
        hosts.sort_by_key(|h| h.name.to_lowercase());
        egui::ScrollArea::vertical().auto_shrink([false, true]).max_height(ui.available_height() - 120.0).show(
            ui,
            |ui| {
                for h in &hosts {
                    self.host_row(ui, h);
                }
            },
        );

        if !self.sessions.is_empty() {
            ui.separator();
            let mut dismiss = None;
            for s in &self.sessions {
                ui.horizontal(|ui| match s.finished {
                    None => {
                        ui.spinner();
                        ui.label(format!("{}: {}", s.name, s.status));
                    }
                    Some(_) => {
                        widgets::error(ui, &format!("{}: {}", s.name, sys::error_text(&s.stderr)));
                        if ui.small_button("Dismiss").clicked() {
                            dismiss = Some(s.id);
                        }
                    }
                });
            }
            if let Some(id) = dismiss {
                self.sessions.retain(|s| s.id != id);
            }
        }
    }

    fn host_row(&mut self, ui: &mut Ui, h: &KnownHost) {
        let online = self.online.get(&h.fingerprint).copied();
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                widgets::dot(ui, online.map(|o| if o { GREEN } else { GREY }));
                ui.vertical(|ui| {
                    ui.label(RichText::new(&h.name).strong().size(15.0));
                    let state = match online {
                        Some(true) => "online",
                        Some(false) => "offline",
                        None if h.addr.is_empty() => "via server",
                        None => h.addr.as_str(),
                    };
                    let last = h
                        .last_connected
                        .map_or("never connected".into(), |t| format!("last connected {}", widgets::ago(t)));
                    widgets::hint(ui, &format!("{state} · {last}"));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.remove.as_deref() == Some(h.fingerprint.as_str()) {
                        if ui.button("Cancel").clicked() {
                            self.remove = None;
                        }
                        if ui.button(RichText::new("Remove").color(ui.visuals().error_fg_color)).clicked() {
                            if let Err(e) = self.edit(|cfg| {
                                cfg.forget(&h.fingerprint);
                            }) {
                                self.server_error = Some(e);
                            }
                            self.remove = None;
                        }
                        ui.label("Remove this computer?");
                    } else {
                        if ui.button("Remove…").clicked() {
                            self.remove = Some(h.fingerprint.clone());
                        }
                        let busy = self.sessions.iter().any(|s| s.name == h.name && s.finished.is_none());
                        if ui.add_enabled(!busy, egui::Button::new(RichText::new("Connect").strong())).clicked() {
                            self.connect = Some(ConnectDialog {
                                fingerprint: h.fingerprint.clone(),
                                name: h.name.clone(),
                                pin: String::new(),
                            });
                        }
                    }
                });
            });
        });
    }

    fn this_computer(&mut self, ui: &mut Ui) {
        if self.remote_checked.elapsed() > Duration::from_secs(3) {
            self.remote = sys::remote_access();
            self.remote_checked = Instant::now();
        }
        widgets::heading(ui, "Remote access to this computer");
        ui.horizontal(|ui| {
            let (color, text) = match self.remote {
                RemoteAccess::On => {
                    (Some(GREEN), "On: paired devices can control this computer, even at the login screen.")
                }
                RemoteAccess::Stopped => (Some(GREY), "Installed but stopped."),
                RemoteAccess::Off => (None, "Off: nobody can connect to this computer."),
            };
            widgets::dot(ui, color);
            ui.label(text);
        });
        ui.add_space(8.0);
        widgets::hint(
            ui,
            "Allowing remote access, setting the PIN and adding devices need administrator rights. \
             Windows asks once when you open the settings.",
        );
        if ui.button("Manage this computer…").clicked() {
            self.manage_error = sys::relaunch_elevated("--manage").err().map(|e| format!("{e:#}"));
        }
        if let Some(e) = &self.manage_error {
            widgets::error(ui, e);
        }
    }

    fn settings(&mut self, ui: &mut Ui) {
        widgets::heading(ui, "This device");
        egui::Grid::new("device").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            ui.label("Name");
            ui.label(&self.cfg.device_name);
            ui.end_row();
            ui.label("Fingerprint");
            let fp =
                self.identity.as_ref().map(|i| rider_core::identity::fp_short(&i.fingerprint())).unwrap_or_default();
            ui.monospace(fp);
            ui.end_row();
        });

        widgets::heading(ui, "Server");
        widgets::hint(
            ui,
            "Your rider-server lets you reach computers on other networks (e.g. home and work). \
             `rider-server info` prints both values.",
        );
        egui::Grid::new("server").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            ui.label("Address");
            ui.add(TextEdit::singleline(&mut self.server_addr).hint_text("rider.example.com").desired_width(320.0));
            ui.end_row();
            ui.label("Fingerprint");
            ui.add(
                TextEdit::singleline(&mut self.server_fp)
                    .hint_text("64 hex characters")
                    .desired_width(320.0)
                    .font(egui::TextStyle::Monospace),
            );
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
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
        });
        match &self.settings_msg {
            Some(Ok(m)) => {
                ui.label(m);
            }
            Some(Err(e)) => widgets::error(ui, e),
            None => {}
        }
        ui.add_space(12.0);
        widgets::hint(ui, &format!("Remote Desktop Rider {}", env!("CARGO_PKG_VERSION")));
    }

    // -------------------------------------------------------------- dialogs

    fn add_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.add else { return };
        let mut start = false;
        let mut close = false;
        let mut reply = None;
        Modal::new(Id::unique("add-computer")).show(ctx, |ui| {
            ui.set_width(420.0);
            widgets::heading(ui, "Add a computer");
            match &d.stage {
                AddStage::Input { error } => {
                    ui.label("Pairing code or address shown on the other computer:");
                    let r = ui.add(
                        TextEdit::singleline(&mut d.target)
                            .hint_text("e.g. SF6-CK6 or 192.168.1.20")
                            .desired_width(f32::INFINITY),
                    );
                    if d.target.is_empty() {
                        r.request_focus();
                    }
                    ui.label("Name (optional):");
                    ui.add(TextEdit::singleline(&mut d.name).hint_text("e.g. Office PC").desired_width(f32::INFINITY));
                    if let Some(e) = error {
                        widgets::error(ui, e);
                    }
                    ui.horizontal(|ui| {
                        start = ui.button(RichText::new("Continue").strong()).clicked()
                            || ui.input(|i| i.key_pressed(Key::Enter));
                        close = ui.button("Cancel").clicked();
                    });
                }
                AddStage::Connecting => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Connecting…");
                    });
                }
                AddStage::Confirm { prompt, .. } => {
                    ui.label(format!("\"{}\" ({}) shows a verification code.", prompt.host_name, prompt.remote.ip()));
                    ui.vertical_centered(|ui| ui.label(RichText::new(&prompt.code).monospace().size(32.0).strong()));
                    ui.label("Does it show exactly this code? If not, someone may be intercepting the connection.");
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Yes, it matches").strong()).clicked() {
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
                }
                AddStage::Done { name } => {
                    ui.label(format!("\"{name}\" was added. Connect to it any time from the list."));
                    close = ui.button("Done").clicked() || ui.input(|i| i.key_pressed(Key::Enter));
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
            ui.set_width(340.0);
            widgets::heading(ui, &format!("Connect to \"{}\"", d.name));
            ui.label("PIN of that computer:");
            let r = ui.add(TextEdit::singleline(&mut d.pin).password(true).desired_width(f32::INFINITY));
            r.request_focus();
            ui.horizontal(|ui| {
                go = ui.add_enabled(!d.pin.is_empty(), egui::Button::new(RichText::new("Connect").strong())).clicked()
                    || (!d.pin.is_empty() && ui.input(|i| i.key_pressed(Key::Enter)));
                close = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
            });
        });
        if go {
            if let Some(d) = self.connect.take() {
                self.start_session(d);
            }
        } else if close {
            self.connect = None;
        }
    }
}

impl eframe::App for ClientApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_messages();
        self.poll_online();
        // Keep timers (online polling, service state) ticking while idle.
        ctx.request_repaint_after(Duration::from_secs(2));

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Remote Desktop Rider").strong().size(17.0));
                ui.add_space(16.0);
                ui.selectable_value(&mut self.tab, Tab::Computers, "Computers");
                ui.selectable_value(&mut self.tab, Tab::ThisComputer, "This computer");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
            });
            ui.add_space(2.0);
        });
        egui::CentralPanel::default().show(ui, |ui| {
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

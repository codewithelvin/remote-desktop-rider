//! "This computer" settings window. Runs elevated (`--manage`) and drives
//! `rider-host --json` for everything: the CLI stays the single place where
//! host state is changed.

use crate::sys;
use crate::widgets::{self, ACCENT, AMBER, Bus, GREEN, GREY, Tx, icon};
use eframe::egui::{self, Align, Key, Layout, RichText, TextEdit, Ui};
use rider_core::rendezvous::ServerRef;
use serde::Deserialize;
use std::io::Write;
use std::process::{ChildStdin, Stdio};

const HOST: &str = "rider-host.exe";

#[derive(Deserialize, Clone)]
struct Info {
    name: String,
    fingerprint: String,
    port: u16,
    pin_set: bool,
    server: Option<ServerRef>,
    /// `None`: service not installed; `Some(running)`.
    service: Option<bool>,
}

#[derive(Deserialize, Clone)]
struct Device {
    name: String,
    fingerprint: String,
    /// Unix seconds, as text (older hosts may omit it).
    #[serde(default)]
    paired_at: String,
}

enum Msg {
    Loaded(Result<(Info, Vec<Device>), String>),
    Done(Result<String, String>),
    Pair(PairEvent),
}

enum PairEvent {
    Listening { addresses: Vec<String> },
    Code(Option<String>),
    ServerError(String),
    Request { device: String, from: String, code: String },
    Paired(String),
    Declined(String),
    Exited { stderr: String },
}

#[derive(PartialEq)]
enum PairStage {
    Waiting,
    Asking { device: String, from: String, code: String },
    Confirming,
    Paired(String),
    Failed(String),
}

struct Pairing {
    stdin: Option<ChildStdin>,
    addresses: Vec<String>,
    code: Option<String>,
    server_error: Option<String>,
    stage: PairStage,
}

impl Pairing {
    fn send(&mut self, line: &str) {
        if let Some(s) = &mut self.stdin {
            let _ = writeln!(s, "{line}");
        }
    }
}

pub struct HostApp {
    bus: Bus<Msg>,
    info: Option<Info>,
    devices: Vec<Device>,
    load_error: Option<String>,
    busy: Option<&'static str>,
    result: Option<Result<String, String>>,
    pin: [String; 2],
    pin_open: bool,
    server_addr: String,
    server_fp: String,
    pairing: Option<Pairing>,
    revoke: Option<String>,
}

impl HostApp {
    pub fn new(ctx: &egui::Context) -> Self {
        let app = Self {
            bus: Bus::new(ctx),
            info: None,
            devices: Vec::new(),
            load_error: None,
            busy: Some("Loading…"),
            result: None,
            pin: Default::default(),
            pin_open: false,
            server_addr: String::new(),
            server_fp: String::new(),
            pairing: None,
            revoke: None,
        };
        app.load();
        app
    }

    fn load(&self) {
        let tx = self.bus.tx();
        std::thread::spawn(move || {
            let r = (|| {
                let info: Info = serde_json::from_str(&sys::run(HOST, &["--json", "info"], None)?)
                    .map_err(|e| format!("unexpected output from {HOST}: {e}"))?;
                let devices: Vec<Device> = serde_json::from_str(&sys::run(HOST, &["--json", "clients"], None)?)
                    .map_err(|e| format!("unexpected output from {HOST}: {e}"))?;
                Ok((info, devices))
            })();
            tx.send(Msg::Loaded(r));
        });
    }

    /// Run one `rider-host` command in the background, then reload.
    fn run(&mut self, label: &'static str, args: Vec<String>, stdin: Option<String>, ok: &'static str) {
        self.busy = Some(label);
        self.result = None;
        let tx = self.bus.tx();
        std::thread::spawn(move || {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let r = sys::run(HOST, &args, stdin.as_deref()).map(|_| ok.to_owned());
            tx.send(Msg::Done(r));
        });
    }

    fn start_pairing(&mut self) {
        self.result = None;
        let spawned = sys::tool(HOST)
            .args(["--json", "pair"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                self.result = Some(Err(format!("cannot start {HOST}: {e}")));
                return;
            }
        };
        let stdin = child.stdin.take();
        let tx: Tx<Msg> = self.bus.tx();
        if let Some(out) = child.stdout.take() {
            let tx = tx.clone();
            sys::lines(out, move |line| {
                if let Some(ev) = parse_event(&line) {
                    tx.send(Msg::Pair(ev));
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
            tx.send(Msg::Pair(PairEvent::Exited { stderr: err }));
        });
        self.pairing =
            Some(Pairing { stdin, addresses: Vec::new(), code: None, server_error: None, stage: PairStage::Waiting });
    }

    fn handle_messages(&mut self) {
        let msgs: Vec<Msg> = self.bus.drain().collect();
        for msg in msgs {
            match msg {
                Msg::Loaded(r) => {
                    self.busy = None;
                    match r {
                        Ok((info, devices)) => {
                            if let Some(s) = &info.server {
                                self.server_addr.clone_from(&s.addr);
                                self.server_fp.clone_from(&s.fingerprint);
                            }
                            self.info = Some(info);
                            self.devices = devices;
                            self.load_error = None;
                        }
                        Err(e) => self.load_error = Some(e),
                    }
                }
                Msg::Done(r) => {
                    if r.is_ok() {
                        self.pin_open = false;
                        self.pin = Default::default();
                    }
                    self.result = Some(r);
                    self.busy = Some("Loading…");
                    self.load();
                }
                Msg::Pair(ev) => self.pair_event(ev),
            }
        }
    }

    fn pair_event(&mut self, ev: PairEvent) {
        let Some(p) = &mut self.pairing else { return };
        match ev {
            PairEvent::Listening { addresses } => p.addresses = addresses,
            PairEvent::Code(code) => {
                p.code = code;
                p.server_error = None;
            }
            PairEvent::ServerError(e) => p.server_error = Some(e),
            PairEvent::Request { device, from, code } => p.stage = PairStage::Asking { device, from, code },
            PairEvent::Paired(device) => p.stage = PairStage::Paired(device),
            PairEvent::Declined(by) => p.stage = PairStage::Failed(format!("Pairing declined by {by}.")),
            PairEvent::Exited { stderr } => {
                p.stdin = None;
                if matches!(p.stage, PairStage::Waiting | PairStage::Asking { .. } | PairStage::Confirming) {
                    let why = sys::error_text(&stderr);
                    p.stage = PairStage::Failed(if why == sys::NO_DETAILS { "Pairing stopped.".into() } else { why });
                }
                self.load();
            }
        }
    }

    // ---------------------------------------------------------------- views

    fn idle(&self) -> bool {
        self.busy.is_none() && self.pairing.is_none()
    }

    fn remote_access(&mut self, ui: &mut Ui, info: &Info) {
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::SHIELD, "Remote access");
            ui.horizontal(|ui| {
                let (color, state, text) = match info.service {
                    Some(true) => (
                        GREEN,
                        "On",
                        "Devices you added can control this computer, also at the sign-in screen and after a restart.",
                    ),
                    Some(false) => (AMBER, "Stopped", "Set up but not running."),
                    None => (GREY, "Off", "Nobody can connect to this computer."),
                };
                widgets::pill(ui, state, color);
                ui.label(text);
            });
            let idle = self.idle();
            ui.horizontal(|ui| match info.service {
                Some(running) => {
                    if !running && widgets::primary(ui, idle, "Start").clicked() {
                        self.run("Starting…", args(&["service", "start"]), None, "Remote access started.");
                    }
                    if ui.add_enabled(idle, egui::Button::new("Turn off remote access")).clicked() {
                        self.run(
                            "Turning off…",
                            args(&["service", "uninstall"]),
                            None,
                            "Remote access is off. Your devices are kept.",
                        );
                    }
                }
                None => {
                    if widgets::primary(ui, idle && info.pin_set, "Turn on remote access").clicked() {
                        self.run("Turning on…", args(&["service", "install"]), None, "Remote access is on.");
                    }
                    if !info.pin_set {
                        widgets::hint(ui, "Set a PIN first.");
                    }
                }
            });
        });
    }

    fn pin_section(&mut self, ui: &mut Ui, info: &Info) {
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::KEY, "PIN");
            widgets::hint(ui, "Asked on every connection, also from devices you added. At least 6 characters.");
            if !self.pin_open {
                ui.horizontal(|ui| {
                    if info.pin_set {
                        widgets::pill(ui, "Set", GREEN);
                    } else {
                        widgets::pill(ui, "Not set", AMBER);
                    }
                    let label = if info.pin_set { "Change PIN…" } else { "Set PIN…" };
                    if ui.add_enabled(self.busy.is_none(), egui::Button::new(label)).clicked() {
                        self.pin_open = true;
                    }
                });
                return;
            }
            egui::Grid::new("pin").num_columns(2).spacing([24.0, 8.0]).show(ui, |ui| {
                widgets::hint(ui, "New PIN");
                let r = ui.add(TextEdit::singleline(&mut self.pin[0]).password(true).desired_width(240.0));
                if self.pin[0].is_empty() && self.pin[1].is_empty() {
                    r.request_focus();
                }
                ui.end_row();
                widgets::hint(ui, "Repeat");
                ui.add(TextEdit::singleline(&mut self.pin[1]).password(true).desired_width(240.0));
                ui.end_row();
            });
            let problem = if self.pin[0].chars().count() < 6 {
                Some("At least 6 characters.")
            } else if self.pin[0] != self.pin[1] {
                Some("The two entries differ.")
            } else {
                None
            };
            ui.horizontal(|ui| {
                let ok = problem.is_none() && self.busy.is_none();
                if widgets::primary(ui, ok, "Save PIN").clicked() || (ok && ui.input(|i| i.key_pressed(Key::Enter))) {
                    let pin = self.pin[0].clone();
                    self.run("Saving PIN…", args(&["set-pin", "--pin-stdin"]), Some(pin), "PIN saved.");
                }
                if ui.button("Cancel").clicked() {
                    self.pin_open = false;
                    self.pin = Default::default();
                }
                if let Some(p) = problem
                    && !self.pin[0].is_empty()
                {
                    widgets::hint(ui, p);
                }
            });
        });
    }

    fn devices_section(&mut self, ui: &mut Ui, info: &Info) {
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::section(ui, icon::DEVICES, "Devices that can control this computer");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let ok = self.idle() && info.pin_set;
                    let add = widgets::primary(ui, ok, &format!("{}   Add a device", icon::ADD));
                    if !info.pin_set {
                        add.on_disabled_hover_text("Set a PIN first.");
                    } else if add.clicked() {
                        self.start_pairing();
                    }
                });
            });
            if self.devices.is_empty() {
                widgets::hint(ui, "None yet. Click Add a device, then enter the code it shows on the other device.");
            }
            let devices = self.devices.clone();
            for d in &devices {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::PC).size(16.0).color(ACCENT));
                    ui.vertical(|ui| {
                        ui.label(widgets::title(&d.name, 14.5));
                        let added = d.paired_at.parse::<u64>().map(|t| format!("  ·  added {}", widgets::ago(t)));
                        widgets::hint(
                            ui,
                            &format!("{}{}", widgets::short_fp(&d.fingerprint), added.unwrap_or_default()),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.revoke.as_deref() == Some(d.fingerprint.as_str()) {
                            if widgets::danger(ui, "Remove").clicked() {
                                self.revoke = None;
                                self.run(
                                    "Removing…",
                                    vec!["revoke".into(), d.fingerprint.clone()],
                                    None,
                                    "Device removed.",
                                );
                            }
                            if ui.button("Cancel").clicked() {
                                self.revoke = None;
                            }
                            ui.label("It will no longer be able to connect.");
                        } else if self.busy.is_none()
                            && widgets::icon_button(ui, icon::DELETE, "Remove this device").clicked()
                        {
                            self.revoke = Some(d.fingerprint.clone());
                        }
                    });
                });
            }
        });
    }

    fn server_section(&mut self, ui: &mut Ui, info: &Info) {
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::GLOBE, "Server");
            widgets::hint(
                ui,
                "Lets your devices reach this computer from other networks. `rider-server info` prints both values.",
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
                let idle = self.busy.is_none();
                let changed = info.server.as_ref().map_or(!self.server_addr.trim().is_empty(), |s| {
                    s.addr != self.server_addr.trim() || s.fingerprint != self.server_fp.trim()
                });
                if widgets::primary(ui, idle && changed, "Save").clicked() {
                    match ServerRef::new(self.server_addr.trim(), self.server_fp.trim()) {
                        Ok(s) => {
                            self.run("Saving…", vec!["set-server".into(), s.addr, s.fingerprint], None, "Server saved.")
                        }
                        Err(e) => self.result = Some(Err(format!("{e:#}"))),
                    }
                }
                if info.server.is_some() && ui.add_enabled(idle, egui::Button::new("Don't use a server")).clicked() {
                    self.run("Saving…", args(&["set-server", "--off"]), None, "Server removed.");
                }
            });
        });
    }

    fn pairing_window(&mut self, ctx: &egui::Context, info: &Info) {
        let Some(p) = &mut self.pairing else { return };
        let mut close = false;
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("pairing")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(widgets::title("Add a device", 19.0));
            ui.add_space(4.0);
            match &p.stage {
                PairStage::Waiting => {
                    ui.label("On the other device, click Add computer and enter:");
                    match &p.code {
                        Some(code) => widgets::big_code(ui, code),
                        None if info.server.is_some() && p.server_error.is_none() => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Getting a code from the server…");
                            });
                        }
                        None => {}
                    }
                    if let Some(e) = &p.server_error {
                        widgets::error(ui, &format!("Server: {e}"));
                    }
                    if !p.addresses.is_empty() {
                        let on_lan = if p.code.is_some() {
                            "On the same network, an address also works:"
                        } else {
                            "On the same network, enter one of these addresses:"
                        };
                        widgets::hint(ui, on_lan);
                        let list: Vec<String> = p
                            .addresses
                            .iter()
                            .map(|a| a.trim_end_matches(&format!(":{}", info.port)).to_owned())
                            .collect();
                        ui.label(RichText::new(list.join("    ")).monospace());
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        widgets::hint(ui, "Waiting up to 10 minutes. Remote sessions are paused meanwhile.");
                    });
                    widgets::dialog_buttons(ui, |ui| close = ui.button("Cancel").clicked());
                }
                PairStage::Asking { device, from, code } => {
                    ui.label(format!("\"{device}\" ({from}) wants to be added. It shows this verification code:"));
                    widgets::big_code(ui, code);
                    widgets::hint(
                        ui,
                        "Allow only if the code is exactly the same and you trust that device to control this computer.",
                    );
                    widgets::dialog_buttons(ui, |ui| {
                        if widgets::primary(ui, true, "Yes, allow").clicked() {
                            answer = Some(true);
                        }
                        if ui.button("No").clicked() {
                            answer = Some(false);
                        }
                    });
                }
                PairStage::Confirming => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Waiting for the other device…");
                    });
                }
                PairStage::Paired(device) => {
                    widgets::success(ui, &format!("\"{device}\" was added."));
                    widgets::hint(ui, "It can connect to this computer with the PIN.");
                    widgets::dialog_buttons(ui, |ui| {
                        close = widgets::primary(ui, true, "Done").clicked() || ui.input(|i| i.key_pressed(Key::Enter));
                    });
                }
                PairStage::Failed(e) => {
                    widgets::error(ui, e);
                    widgets::dialog_buttons(ui, |ui| close = ui.button("Close").clicked());
                }
            }
        });
        if let Some(yes) = answer {
            p.send(if yes { "y" } else { "n" });
            p.stage = PairStage::Confirming;
        }
        if close {
            // Ends pairing; the host resumes the background service on exit.
            p.send("quit");
            self.pairing = None;
        }
    }
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_owned()).collect()
}

fn parse_event(line: &str) -> Option<PairEvent> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned();
    Some(match v.get("event")?.as_str()? {
        "listening" => PairEvent::Listening {
            addresses: v.get("addresses")?.as_array()?.iter().filter_map(|a| a.as_str().map(str::to_owned)).collect(),
        },
        "registered" => PairEvent::Code(v.get("code").and_then(|c| c.as_str()).map(str::to_owned)),
        "server_error" => PairEvent::ServerError(s("message")),
        "request" => PairEvent::Request { device: s("device"), from: s("from"), code: s("code") },
        "paired" => PairEvent::Paired(s("device")),
        "declined" => PairEvent::Declined(s("by")),
        _ => return None,
    })
}

impl eframe::App for HostApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_messages();
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.global_style()).inner_margin(egui::Margin::symmetric(28, 22)))
            .show(ui, |ui| {
                let subtitle = self
                    .info
                    .as_ref()
                    .map_or(String::new(), |i| format!("\"{}\"  ·  {}", i.name, widgets::short_fp(&i.fingerprint)));
                widgets::page_header(ui, "This computer", &subtitle, |ui| {
                    if let Some(label) = self.busy {
                        ui.label(label);
                        ui.spinner();
                    } else {
                        match &self.result {
                            Some(Ok(m)) => widgets::success(ui, m),
                            Some(Err(e)) => widgets::error(ui, e),
                            None => {}
                        }
                    }
                });
                if let Some(e) = &self.load_error {
                    widgets::card(ui, |ui| widgets::error(ui, e));
                    return;
                }
                let Some(info) = self.info.clone() else { return };
                egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                    self.remote_access(ui, &info);
                    ui.add_space(10.0);
                    self.pin_section(ui, &info);
                    ui.add_space(10.0);
                    self.devices_section(ui, &info);
                    ui.add_space(10.0);
                    self.server_section(ui, &info);
                });
            });
        if let Some(info) = self.info.clone() {
            self.pairing_window(&ctx, &info);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_events() {
        assert!(matches!(
            parse_event(r#"{"event":"registered","server":"s","public":"1.2.3.4:5","code":"SF6-CK6"}"#),
            Some(PairEvent::Code(Some(c))) if c == "SF6-CK6"
        ));
        assert!(matches!(
            parse_event(r#"{"event":"request","device":"PC","from":"1.2.3.4:5","code":"039 472"}"#),
            Some(PairEvent::Request { code, .. }) if code == "039 472"
        ));
        assert!(parse_event("Pausing the background service while pairing...").is_none());
        assert!(parse_event(r#"{"event":"unknown"}"#).is_none());
    }
}

use crate::sys;
use crate::widgets::{self, ACCENT, AMBER, Bus, GREEN, GREY, Tx, icon};
use eframe::egui::{self, Align, Key, Layout, RichText, TextEdit, Ui};
use rider_client::config::ClientConfig;
use rider_core::i18n::tr;
use rider_core::rendezvous::ServerRef;
use rider_core::trf;
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
    #[serde(default = "yes")]
    auto_update: bool,
    service: Option<bool>,
    #[serde(default)]
    language: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize, Clone)]
struct Device {
    name: String,
    fingerprint: String,
    #[serde(default)]
    paired_at: String,
}

enum Msg {
    Loaded(Result<(Info, Vec<Device>), String>),
    Done(Result<String, String>),
    Invite(Result<(String, u32), String>),
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
    server_invite: String,
    invite_code: Option<(String, std::time::Instant)>,
    invite_scroll: bool,
    pairing: Option<Pairing>,
    revoke: Option<String>,
}

const INVITE_PREVIEW: std::time::Duration = std::time::Duration::from_secs(600);

impl HostApp {
    pub fn new(ctx: &egui::Context) -> Self {
        let mut app = Self {
            bus: Bus::new(ctx),
            info: None,
            devices: Vec::new(),
            load_error: None,
            busy: Some(tr("Loading…", "Yüklənir…")),
            result: None,
            pin: Default::default(),
            pin_open: false,
            server_addr: String::new(),
            server_fp: String::new(),
            server_invite: String::new(),
            invite_code: None,
            invite_scroll: false,
            pairing: None,
            revoke: None,
        };
        if std::env::var_os("RIDER_PREVIEW").is_some() {
            app.preview();
        } else {
            app.load();
        }
        app
    }

    fn preview(&mut self) {
        let fp = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        self.busy = None;
        self.info = Some(Info {
            name: "OFFICE-PC".into(),
            fingerprint: fp.into(),
            port: rider_core::DEFAULT_PORT,
            pin_set: true,
            server: ServerRef::new("rider.example.com", fp).ok(),
            auto_update: true,
            service: Some(true),
            language: Some(rider_core::i18n::lang().code().into()),
        });
        let now = rider_core::now_us() / 1_000_000;
        self.devices = vec![
            Device { name: "Laptop".into(), fingerprint: fp.into(), paired_at: (now - 3 * 86_400).to_string() },
            Device { name: "Home PC".into(), fingerprint: fp.into(), paired_at: (now - 7200).to_string() },
        ];
        self.server_addr = "rider.example.com".into();
        self.server_fp = fp.into();
        self.pin_open = std::env::var("RIDER_PREVIEW").as_deref() == Ok("pin");
        if std::env::var("RIDER_PREVIEW").as_deref() == Ok("invite") {
            self.invite_code = Some(("K7P-2QX".into(), std::time::Instant::now() + INVITE_PREVIEW));
            self.invite_scroll = true;
        }
    }

    fn load(&self) {
        let tx = self.bus.tx();
        std::thread::spawn(move || {
            let r = (|| {
                let unexpected = |e: serde_json::Error| {
                    trf!("unexpected output from {HOST}: {e}", "{HOST} gözlənilməz cavab verdi: {e}")
                };
                let info: Info =
                    serde_json::from_str(&sys::run(HOST, &["--json", "info"], None)?).map_err(unexpected)?;
                let devices: Vec<Device> =
                    serde_json::from_str(&sys::run(HOST, &["--json", "clients"], None)?).map_err(unexpected)?;
                Ok((info, devices))
            })();
            tx.send(Msg::Loaded(r));
        });
    }

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

    fn sync_banner_language(&mut self) {
        if std::env::var_os("RIDER_PREVIEW").is_some() {
            return;
        }
        let want = rider_core::i18n::lang().code();
        let Some(info) = &mut self.info else { return };
        if info.language.as_deref() == Some(want) {
            return;
        }
        info.language = Some(want.to_owned());
        std::thread::spawn(move || {
            let _ = sys::run(HOST, &["language", want], None);
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
                self.result = Some(Err(trf!("cannot start {HOST}: {e}", "{HOST} başladıla bilmir: {e}")));
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
                            match &info.server {
                                Some(s) => {
                                    self.server_addr.clone_from(&s.addr);
                                    self.server_fp.clone_from(&s.fingerprint);
                                    let _ = sync_client_server(Some(s));
                                }
                                None if self.server_addr.is_empty() => {
                                    if let Some(s) = ClientConfig::load().ok().and_then(|c| c.server) {
                                        self.server_addr = s.addr;
                                        self.server_fp = s.fingerprint;
                                    }
                                }
                                None => {}
                            }
                            self.info = Some(info);
                            self.devices = devices;
                            self.load_error = None;
                            self.sync_banner_language();
                        }
                        Err(e) => self.load_error = Some(e),
                    }
                }
                Msg::Done(r) => {
                    if r.is_ok() {
                        self.pin_open = false;
                        self.pin = Default::default();
                        self.server_invite.clear();
                    }
                    self.result = Some(r);
                    self.busy = Some(tr("Loading…", "Yüklənir…"));
                    self.load();
                }
                Msg::Invite(r) => {
                    self.busy = None;
                    match r {
                        Ok((code, minutes)) => {
                            let until =
                                std::time::Instant::now() + std::time::Duration::from_secs(u64::from(minutes) * 60);
                            self.invite_code = Some((code, until));
                            self.invite_scroll = true;
                        }
                        Err(e) => self.result = Some(Err(e)),
                    }
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
            PairEvent::Declined(by) => {
                p.stage = PairStage::Failed(if by == "this computer" {
                    tr("Pairing was declined on this computer.", "Cütləşdirmə bu kompüterdə rədd edildi.").into()
                } else {
                    tr("Pairing was declined on the other device.", "Cütləşdirmə digər cihazda rədd edildi.").into()
                });
            }
            PairEvent::Exited { stderr } => {
                p.stdin = None;
                if matches!(p.stage, PairStage::Waiting | PairStage::Asking { .. } | PairStage::Confirming) {
                    let why = sys::error_text(&stderr);
                    p.stage = PairStage::Failed(if why == sys::no_details() {
                        tr("Pairing stopped.", "Cütləşdirmə dayandırıldı.").into()
                    } else {
                        why
                    });
                }
                self.load();
            }
        }
    }

    fn idle(&self) -> bool {
        self.busy.is_none() && self.pairing.is_none()
    }

    fn remote_access(&mut self, ui: &mut Ui, info: &Info) {
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::SHIELD, tr("Remote access", "Uzaqdan giriş"));
            ui.horizontal(|ui| {
                let (color, state, text) = match info.service {
                    Some(true) => (
                        GREEN,
                        tr("On", "Aktiv"),
                        tr(
                            "Devices you added can control this computer, also at the sign-in screen and after a restart.",
                            "Əlavə etdiyiniz cihazlar bu kompüteri idarə edə bilər, o cümlədən giriş ekranında və yenidən başladıldıqdan sonra.",
                        ),
                    ),
                    Some(false) => {
                        (AMBER, tr("Stopped", "Dayandırılıb"), tr("Set up but not running.", "Quraşdırılıb, lakin işləmir."))
                    }
                    None => (
                        GREY,
                        tr("Off", "Söndürülüb"),
                        tr("Nobody can connect to this computer.", "Heç kim bu kompüterə qoşula bilməz."),
                    ),
                };
                widgets::pill(ui, state, color);
                ui.add(egui::Label::new(text).wrap());
            });
            let idle = self.idle();
            ui.horizontal(|ui| {
                if let Some(running) = info.service {
                    if !running && widgets::primary(ui, idle, tr("Start", "Başlat")).clicked() {
                        self.run(
                            tr("Starting…", "Başladılır…"),
                            args(&["service", "start"]),
                            None,
                            tr("Remote access started.", "Uzaqdan giriş başladıldı."),
                        );
                    }
                    if ui
                        .add_enabled(idle, egui::Button::new(tr("Turn off remote access", "Uzaqdan girişi söndür")))
                        .clicked()
                    {
                        self.run(
                            tr("Turning off…", "Söndürülür…"),
                            args(&["service", "uninstall"]),
                            None,
                            tr(
                                "Remote access is off. Your devices are kept.",
                                "Uzaqdan giriş söndürüldü. Cihazlarınız saxlanılıb.",
                            ),
                        );
                    }
                } else {
                    if widgets::primary(
                        ui,
                        idle && info.pin_set,
                        tr("Turn on remote access", "Uzaqdan girişi aktivləşdir"),
                    )
                    .clicked()
                    {
                        self.run(
                            tr("Turning on…", "Aktivləşdirilir…"),
                            args(&["service", "install"]),
                            None,
                            tr("Remote access is on.", "Uzaqdan giriş aktivdir."),
                        );
                    }
                    if !info.pin_set {
                        widgets::hint(ui, tr("Set a PIN first.", "Əvvəlcə PIN kod təyin edin."));
                    }
                }
            });
            if info.service.is_some() {
                let mut auto = info.auto_update;
                if ui
                    .add_enabled(
                        idle,
                        egui::Checkbox::new(
                            &mut auto,
                            tr(
                                "Install updates automatically when nobody is connected",
                                "Heç kim qoşulu olmadıqda yeniləmələri avtomatik quraşdır",
                            ),
                        ),
                    )
                    .changed()
                {
                    let (state, done) = if auto {
                        ("on", tr("Automatic updates are on.", "Avtomatik yeniləmələr aktivdir."))
                    } else {
                        ("off", tr("Automatic updates are off.", "Avtomatik yeniləmələr söndürülüb."))
                    };
                    self.run(tr("Saving…", "Yadda saxlanılır…"), args(&["auto-update", state]), None, done);
                }
                widgets::hint(
                    ui,
                    tr(
                        "Keeps a computer you only reach remotely up to date: it updates itself and is back online within a minute.",
                        "Yalnız uzaqdan işlədiyiniz kompüteri aktual saxlayır: o, özünü yeniləyir və bir dəqiqə ərzində yenidən əlçatan olur.",
                    ),
                );
            }
        });
    }

    fn pin_section(&mut self, ui: &mut Ui, info: &Info) {
        widgets::card(ui, |ui| {
            widgets::section(ui, icon::KEY, tr("PIN", "PIN kod"));
            widgets::hint(
                ui,
                tr(
                    "Asked on every connection, also from devices you added. At least 6 characters.",
                    "Hər qoşulmada, o cümlədən əlavə etdiyiniz cihazlardan da soruşulur. Ən azı 6 simvol.",
                ),
            );
            if !self.pin_open {
                ui.horizontal(|ui| {
                    if info.pin_set {
                        widgets::pill(ui, tr("Set", "Təyin edilib"), GREEN);
                    } else {
                        widgets::pill(ui, tr("Not set", "Təyin edilməyib"), AMBER);
                    }
                    let label = if info.pin_set {
                        tr("Change PIN…", "PIN kodu dəyiş…")
                    } else {
                        tr("Set PIN…", "PIN kod təyin et…")
                    };
                    if ui.add_enabled(self.busy.is_none(), egui::Button::new(label)).clicked() {
                        self.pin_open = true;
                    }
                });
                return;
            }
            egui::Grid::new("pin").num_columns(2).spacing([24.0, 8.0]).show(ui, |ui| {
                widgets::hint(ui, tr("New PIN", "Yeni PIN kod"));
                let r = ui.add(TextEdit::singleline(&mut self.pin[0]).password(true).desired_width(240.0));
                if self.pin[0].is_empty() && self.pin[1].is_empty() {
                    r.request_focus();
                }
                ui.end_row();
                widgets::hint(ui, tr("Repeat", "Təkrar daxil edin"));
                ui.add(TextEdit::singleline(&mut self.pin[1]).password(true).desired_width(240.0));
                ui.end_row();
            });
            let problem = if self.pin[0].chars().count() < 6 {
                Some(tr("At least 6 characters.", "Ən azı 6 simvol."))
            } else if self.pin[0] != self.pin[1] {
                Some(tr("The two entries differ.", "Daxil edilən kodlar üst-üstə düşmür."))
            } else {
                None
            };
            ui.horizontal(|ui| {
                let ok = problem.is_none() && self.busy.is_none();
                if widgets::primary(ui, ok, tr("Save PIN", "PIN kodu yadda saxla")).clicked()
                    || (ok && ui.input(|i| i.key_pressed(Key::Enter)))
                {
                    let pin = self.pin[0].clone();
                    self.run(
                        tr("Saving PIN…", "PIN kod yadda saxlanılır…"),
                        args(&["set-pin", "--pin-stdin"]),
                        Some(pin),
                        tr("PIN saved.", "PIN kod yadda saxlanıldı."),
                    );
                }
                if ui.button(tr("Cancel", "Ləğv et")).clicked() {
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
                widgets::section(
                    ui,
                    icon::DEVICES,
                    tr("Devices that can control this computer", "Bu kompüteri idarə edə bilən cihazlar"),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let ok = self.idle() && info.pin_set;
                    let add =
                        widgets::primary(ui, ok, &format!("{}   {}", icon::ADD, tr("Add a device", "Cihaz əlavə et")));
                    if !info.pin_set {
                        add.on_disabled_hover_text(tr("Set a PIN first.", "Əvvəlcə PIN kod təyin edin."));
                    } else if add.clicked() {
                        self.start_pairing();
                    }
                });
            });
            if self.devices.is_empty() {
                widgets::hint(
                    ui,
                    tr(
                        "None yet. Click Add a device, then enter the code it shows on the other device.",
                        "Hələ heç biri yoxdur. «Cihaz əlavə et» düyməsini klikləyin, sonra göstərilən kodu digər cihazda daxil edin.",
                    ),
                );
            }
            let devices = self.devices.clone();
            for d in &devices {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::PC).size(16.0).color(ACCENT));
                    ui.vertical(|ui| {
                        ui.label(widgets::title(&d.name, 14.5));
                        let added = d.paired_at.parse::<u64>().map(|t| {
                            let when = widgets::ago(t);
                            trf!("  ·  added {when}", "  ·  əlavə edilib: {when}")
                        });
                        widgets::hint(
                            ui,
                            &format!("{}{}", widgets::short_fp(&d.fingerprint), added.unwrap_or_default()),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.revoke.as_deref() == Some(d.fingerprint.as_str()) {
                            if widgets::danger(ui, tr("Remove", "Sil")).clicked() {
                                self.revoke = None;
                                self.run(
                                    tr("Removing…", "Silinir…"),
                                    vec!["revoke".into(), d.fingerprint.clone()],
                                    None,
                                    tr("Device removed.", "Cihaz silindi."),
                                );
                            }
                            if ui.button(tr("Cancel", "Ləğv et")).clicked() {
                                self.revoke = None;
                            }
                            ui.label(tr("It will no longer be able to connect.", "O, artıq qoşula bilməyəcək."));
                        } else if self.busy.is_none()
                            && widgets::icon_button(ui, icon::DELETE, tr("Remove this device", "Bu cihazı sil"))
                                .clicked()
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
            widgets::section(ui, icon::GLOBE, tr("Server", "Server"));
            widgets::hint(
                ui,
                tr(
                    "One setting for both directions: your devices reach this computer, and this computer reaches \
                     yours, from any network. Type the server's name; it is checked over HTTPS.",
                    "Hər iki istiqamət üçün bir ayar: cihazlarınız bu kompüterə, bu kompüter də sizin cihazlarınıza \
                     istənilən şəbəkədən qoşulur. Serverin adını yazın; o, HTTPS ilə yoxlanılır.",
                ),
            );
            let mut typed = false;
            egui::Grid::new("server").num_columns(2).spacing([24.0, 8.0]).show(ui, |ui| {
                widgets::hint(ui, tr("Address", "Ünvan"));
                typed |= ui
                    .add(
                        TextEdit::singleline(&mut self.server_addr).hint_text("rider.example.com").desired_width(360.0),
                    )
                    .changed();
                ui.end_row();
                widgets::hint(ui, tr("Invite code", "Dəvət kodu"));
                typed |= ui
                    .add(
                        TextEdit::singleline(&mut self.server_invite)
                            .hint_text(tr(
                                "only for a new computer, e.g. K7P-2QX",
                                "yalnız yeni kompüter üçün, məs. K7P-2QX",
                            ))
                            .desired_width(360.0),
                    )
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
                        TextEdit::singleline(&mut self.server_fp)
                            .hint_text(tr("64 hex characters", "64 onaltılıq simvol"))
                            .desired_width(360.0)
                            .font(egui::TextStyle::Monospace),
                    )
                    .changed();
            });
            if typed && self.busy.is_none() {
                self.result = None;
            }
            ui.horizontal(|ui| {
                let idle = self.busy.is_none();
                let (addr, fp, invite) =
                    (self.server_addr.trim(), self.server_fp.trim(), self.server_invite.trim().to_owned());
                let saved_fp = info.server.as_ref().map_or("", |s| s.fingerprint.as_str());
                let typed = !fp.is_empty() && fp != saved_fp;
                let changed = info.server.as_ref().is_none_or(|s| s.addr != addr) || typed || !invite.is_empty();
                if widgets::primary(ui, idle && changed && !addr.is_empty(), tr("Save", "Yadda saxla")).clicked() {
                    let mut a = vec!["set-server".to_owned(), addr.to_owned()];
                    if typed {
                        a.push(fp.to_owned());
                    }
                    if !invite.is_empty() {
                        a.extend(["--invite".to_owned(), invite]);
                    }
                    self.run(
                        tr("Checking the server…", "Server yoxlanılır…"),
                        a,
                        None,
                        tr("Server saved.", "Server yadda saxlanıldı."),
                    );
                }
                if info.server.is_some() {
                    if ui
                        .add_enabled(idle, egui::Button::new(tr("Get an invite code", "Dəvət kodu al")))
                        .on_hover_text(tr(
                            "A one-time code that lets a new computer use this server (valid 10 minutes).                              To let a device control this computer, use Add a device instead.",
                            "Yeni kompüterin bu serverdən istifadə etməsi üçün birdəfəlik kod (10 dəqiqə etibarlıdır).                              Cihazın bu kompüteri idarə etməsi üçün isə «Cihaz əlavə et» düyməsindən istifadə edin.",
                        ))
                        .clicked()
                    {
                        self.invite();
                    }
                    if ui
                        .add_enabled(idle, egui::Button::new(tr("Don't use a server", "Serverdən istifadə etmə")))
                        .clicked()
                    {
                        let _ = sync_client_server(None);
                        self.invite_code = None;
                        self.run(
                            tr("Saving…", "Yadda saxlanılır…"),
                            args(&["set-server", "--off"]),
                            None,
                            tr("Server removed.", "Server ayarı silindi."),
                        );
                    }
                }
            });
            self.invite_block(ui, info);
        });
    }

    fn invite_block(&mut self, ui: &mut Ui, info: &Info) {
        let Some((_, until)) = &self.invite_code else { return };
        let left = until.saturating_duration_since(std::time::Instant::now()).as_secs();
        let Some(server) = info.server.as_ref().filter(|_| left > 0) else {
            self.invite_code = None;
            return;
        };
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(widgets::title(tr("Invite code for a new computer", "Yeni kompüter üçün dəvət kodu"), 15.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button(tr("Hide", "Gizlət")).clicked() {
                    self.invite_code = None;
                }
            });
        });
        let Some((code, _)) = &self.invite_code else { return };
        ui.add_space(4.0);
        widgets::big_code(ui, code);
        ui.add_space(6.0);
        ui.label(trf!(
            "On the new computer, open This computer → Manage this computer → Server. Enter {} as the address \
             and this code as the invite code, then click Save.",
            "Yeni kompüterdə «Bu kompüter» → «Bu kompüteri idarə et» → «Server» bölməsini açın. Ünvana {}, \
             «Dəvət kodu» xanasına bu kodu yazın və «Yadda saxla» düyməsini basın.",
            server.addr
        ));
        widgets::hint(
            ui,
            &trf!(
                "Works once. Expires in {}:{:02}.",
                "Bir dəfə işləyir. {}:{:02} sonra etibarsız olur.",
                left / 60,
                left % 60
            ),
        );
        if std::mem::take(&mut self.invite_scroll) {
            ui.scroll_to_cursor(Some(Align::Max));
        }
    }

    fn invite(&mut self) {
        self.busy = Some(tr("Asking the server…", "Serverdən soruşulur…"));
        self.result = None;
        self.invite_code = None;
        let tx = self.bus.tx();
        std::thread::spawn(move || {
            #[derive(serde::Deserialize)]
            struct Invite {
                code: String,
                minutes: u32,
            }
            let r = sys::run(HOST, &["--json", "invite"], None).and_then(|out| {
                let i: Invite = serde_json::from_str(out.trim()).map_err(|e| e.to_string())?;
                Ok((i.code, i.minutes))
            });
            tx.send(Msg::Invite(r));
        });
    }

    fn pairing_window(&mut self, ctx: &egui::Context, info: &Info) {
        let Some(p) = &mut self.pairing else { return };
        let mut close = false;
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("pairing")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(widgets::title(tr("Add a device", "Cihaz əlavə et"), 19.0));
            ui.add_space(4.0);
            match &p.stage {
                PairStage::Waiting => {
                    ui.label(tr(
                        "On the other device, click Add computer and enter:",
                        "Digər cihazda «Kompüter əlavə et» düyməsini klikləyin və bu kodu daxil edin:",
                    ));
                    match &p.code {
                        Some(code) => widgets::big_code(ui, code),
                        None if info.server.is_some() && p.server_error.is_none() => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(tr("Getting a code from the server…", "Serverdən kod alınır…"));
                            });
                        }
                        None if info.server.is_none() => widgets::error(
                            ui,
                            tr(
                                "No server is set for this computer, so it gets no code and only devices on the \
                                 same network can add it. Cancel and set the Server on this page to add devices \
                                 from anywhere.",
                                "Bu kompüter üçün server təyin edilməyib, ona görə kod alınmır və onu yalnız eyni \
                                 şəbəkədəki cihazlar əlavə edə bilər. İstənilən yerdən cihaz əlavə etmək üçün \
                                 ləğv edin və bu səhifədə serveri təyin edin.",
                            ),
                        ),
                        None => {}
                    }
                    if let Some(e) = &p.server_error {
                        widgets::error(ui, &format!("{}: {e}", tr("Server", "Server")));
                    }
                    if !p.addresses.is_empty() {
                        let on_lan = if p.code.is_some() {
                            tr("On the same network, an address also works:", "Eyni şəbəkədə ünvan da işləyir:")
                        } else {
                            tr(
                                "On the same network, enter one of these addresses:",
                                "Eyni şəbəkədə bu ünvanlardan birini daxil edin:",
                            )
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
                        widgets::hint(
                            ui,
                            tr(
                                "Waiting up to 10 minutes. Remote sessions are paused meanwhile.",
                                "10 dəqiqəyədək gözlənilir. Bu müddətdə uzaq seanslar dayandırılır.",
                            ),
                        );
                    });
                    widgets::dialog_buttons(ui, |ui| close = ui.button(tr("Cancel", "Ləğv et")).clicked());
                }
                PairStage::Asking { device, from, code } => {
                    ui.add(
                        egui::Label::new(trf!(
                            "\"{device}\" ({from}) wants to be added. It shows this verification code:",
                            "\"{device}\" ({from}) əlavə olunmaq istəyir. Orada bu təsdiq kodu göstərilir:"
                        ))
                        .wrap(),
                    );
                    widgets::big_code(ui, code);
                    widgets::hint(
                        ui,
                        tr(
                            "Allow only if the code is exactly the same and you trust that device to control this computer.",
                            "Yalnız kod tam eynidirsə və bu cihazın kompüterinizi idarə etməsinə etibar edirsinizsə, icazə verin.",
                        ),
                    );
                    widgets::dialog_buttons(ui, |ui| {
                        if widgets::primary(ui, true, tr("Yes, allow", "Bəli, icazə ver")).clicked() {
                            answer = Some(true);
                        }
                        if ui.button(tr("No", "Xeyr")).clicked() {
                            answer = Some(false);
                        }
                    });
                }
                PairStage::Confirming => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tr("Waiting for the other device…", "Digər cihaz gözlənilir…"));
                    });
                }
                PairStage::Paired(device) => {
                    widgets::success(ui, &trf!("\"{device}\" was added.", "\"{device}\" əlavə edildi."));
                    widgets::hint(
                        ui,
                        tr("It can connect to this computer with the PIN.", "O, bu kompüterə PIN kodla qoşula bilər."),
                    );
                    widgets::dialog_buttons(ui, |ui| {
                        close = widgets::primary(ui, true, tr("Done", "Hazırdır")).clicked()
                            || ui.input(|i| i.key_pressed(Key::Enter));
                    });
                }
                PairStage::Failed(e) => {
                    widgets::error(ui, e);
                    widgets::dialog_buttons(ui, |ui| close = ui.button(tr("Close", "Bağla")).clicked());
                }
            }
        });
        if let Some(yes) = answer {
            p.send(if yes { "y" } else { "n" });
            p.stage = PairStage::Confirming;
        }
        if close {
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
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        sys::smooth_monitor_moves(frame);
        let ctx = ui.ctx().clone();
        self.handle_messages();
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.global_style()).inner_margin(egui::Margin::symmetric(28, 22)))
            .show(ui, |ui| {
                let subtitle = self
                    .info
                    .as_ref()
                    .map_or(String::new(), |i| format!("\"{}\"  ·  {}", i.name, widgets::short_fp(&i.fingerprint)));
                let mut lang_changed = false;
                widgets::page_header(ui, tr("This computer", "Bu kompüter"), &subtitle, |ui| {
                    lang_changed = widgets::language_switch(ui).is_some();
                });
                if lang_changed {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(window_title().into()));
                    self.sync_banner_language();
                }
                if let Some(label) = self.busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(label);
                    });
                } else {
                    match &self.result {
                        Some(Ok(m)) => widgets::success(ui, m),
                        Some(Err(e)) => widgets::error(ui, e),
                        None => {}
                    }
                }
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

pub fn window_title() -> &'static str {
    tr("Remote Desktop Rider: this computer (administrator)", "Remote Desktop Rider: bu kompüter (administrator)")
}

fn sync_client_server(server: Option<&ServerRef>) -> anyhow::Result<()> {
    let mut cfg = ClientConfig::load()?;
    if cfg.server.as_ref() != server {
        cfg.server = server.cloned();
        cfg.save()?;
    }
    Ok(())
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

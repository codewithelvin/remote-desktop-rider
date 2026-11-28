use crate::widgets::{self, GREEN, icon};
use eframe::egui::{self, Align, Layout, RichText, Ui};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use rider_core::i18n::tr;
use rider_core::trf;
use rider_ui::chat::{Action, Chat};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    SHOW_WINDOW_CMD, SW_MINIMIZE, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, SW_SHOWNORMAL, ShowWindow,
};

pub const TITLE: &str = "Chat - Remote Desktop Rider";

#[derive(Debug, PartialEq)]
enum Event {
    Session { device: String, files: bool },
    Request { company: String, seconds: u64 },
    Chat(String),
    File { id: u64, mine: bool, name: String, size: u64 },
    FileProgress { id: u64, done: u64 },
    FileDone { id: u64, path: Option<PathBuf> },
    FileFailed { id: u64, reason: String },
    Notice(String),
    Tunnel(crate::tunnel_ask::TunnelEvent),
    Restart(crate::restart_ask::RestartEvent),
}

pub struct Panel {
    events: mpsc::Receiver<Event>,
    tx: mpsc::Sender<Event>,
    device: Option<String>,
    chat: Chat,
    files: HashMap<u64, PathBuf>,
    request: Option<(String, std::time::Instant, f32)>,
    tunnel: Option<crate::tunnel_ask::TunnelAsk>,
    restart: Option<crate::restart_ask::RestartWarning>,
    tunnels_seen: std::collections::HashSet<String>,
}

impl Panel {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        if let Ok(h) = cc.window_handle()
            && let RawWindowHandle::Win32(h) = h.as_raw()
        {
            WINDOW.store(h.hwnd.get(), std::sync::atomic::Ordering::Relaxed);
            show(SW_SHOWMINNOACTIVE);
        }
        let ctx = &cc.egui_ctx;
        let (tx, events) = mpsc::channel();
        let (input_tx, ctx2) = (tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut input = std::io::stdin().lock();
            let mut bytes = Vec::new();
            loop {
                bytes.clear();
                match input.read_until(b'\n', &mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if let Some(ev) = parse(&String::from_utf8_lossy(&bytes)) {
                            if matches!(ev, Event::Chat(_) | Event::File { mine: false, .. }) {
                                show(SW_SHOWNOACTIVATE);
                            }
                            if matches!(
                                ev,
                                Event::Request { .. }
                                    | Event::Tunnel(crate::tunnel_ask::TunnelEvent::Request { .. })
                                    | Event::Restart(crate::restart_ask::RestartEvent::Warning(_))
                            ) {
                                show(if quiet() { SW_SHOWNOACTIVATE } else { SW_SHOWNORMAL });
                            }
                            let _ = input_tx.send(ev);
                            ctx2.request_repaint();
                        }
                    }
                }
            }
            std::process::exit(0);
        });
        let mut chat = Chat::new("");
        chat.closable = false;
        chat.heading = false;
        Self {
            events,
            tx,
            device: None,
            chat,
            files: HashMap::new(),
            request: None,
            tunnel: None,
            restart: None,
            tunnels_seen: std::collections::HashSet::new(),
        }
    }

    fn send(line: &str) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }

    fn send_json(word: &str, arg: &str) {
        Self::send(&format!("{word} {}", serde_json::Value::from(arg)));
    }

    fn handle_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Session { device, files } => {
                    self.chat.peer.clone_from(&device);
                    self.chat.files_allowed = files;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(trf!(
                        "Chat with {device}",
                        "{device} ilə söhbət"
                    )));
                    self.device = Some(device);
                }
                Event::Chat(text) => {
                    self.chat.text(false, &text);
                    attention(ctx);
                }
                Event::File { id, mine, name, size } => {
                    self.chat.file_started(id, mine, &name, size);
                    if !mine {
                        attention(ctx);
                    }
                }
                Event::FileProgress { id, done } => self.chat.file_progress(id, done),
                Event::FileDone { id, path } => {
                    self.chat.file_done(id);
                    if let Some(p) = path {
                        self.files.insert(id, p);
                    }
                }
                Event::FileFailed { id, reason } => self.chat.file_failed(id, reason),
                Event::Notice(text) => self.chat.notice(text),
                Event::Tunnel(t) => self.tunnel_event(t, ctx),
                Event::Restart(crate::restart_ask::RestartEvent::Warning(seconds)) => {
                    self.restart = Some(crate::restart_ask::RestartWarning::new(seconds));
                    if !quiet() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    attention(ctx);
                }
                Event::Restart(crate::restart_ask::RestartEvent::Cancelled) => {
                    self.restart = None;
                    self.chat.notice(tr("The restart was cancelled.", "Yenidən başlatma ləğv edildi."));
                }
                Event::Request { company, seconds } => {
                    let seconds = seconds.clamp(5, 300);
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
                    self.request = Some((company, until, seconds as f32));
                    if !quiet() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                            egui::UserAttentionType::Critical,
                        ));
                    }
                }
            }
        }
    }

    fn header(&mut self, ui: &mut Ui) {
        let dark = ui.visuals().dark_mode;
        let bar = if dark { egui::Color32::from_rgb(30, 31, 36) } else { egui::Color32::from_rgb(232, 234, 239) };
        let mut end = false;
        egui::Panel::top("session")
            .resizable(false)
            .show_separator_line(true)
            .frame(egui::Frame::new().fill(bar).inner_margin(egui::Margin::symmetric(12, 7)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        end = widgets::danger(ui, tr("End session", "Seansı bitir")).clicked();
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            widgets::dot(ui, Some(GREEN));
                            let who = match &self.device {
                                Some(d) => trf!("\"{d}\" is connected", "\"{d}\" qoşulub"),
                                None => tr("Connecting…", "Qoşulur…").into(),
                            };
                            ui.add(egui::Label::new(widgets::title(&who, 13.5)).truncate()).on_hover_text(tr(
                                "A paired device sees and controls this computer.",
                                "Cütləşdirilmiş cihaz bu kompüteri görür və idarə edir.",
                            ));
                        });
                    });
                });
            });
        if end {
            Self::send("end");
        }
    }

    fn act(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Send(text) => {
                    self.chat.text(true, &text);
                    Self::send_json("chat", &text);
                }
                Action::Attach => {
                    std::thread::spawn(|| {
                        for f in rider_ui::files::pick_files() {
                            Self::send_json("send-file", &f.to_string_lossy());
                        }
                    });
                }
                Action::Cancel(id) => Self::send(&format!("cancel-file {id}")),
                Action::Open(id) => {
                    if let Some(p) = self.files.get(&id)
                        && let Err(e) = rider_ui::files::open(p)
                    {
                        let text = format!("{}: {e}", p.display());
                        self.chat.notice(text);
                    }
                }
                Action::SaveAs(id) => {
                    let (Some(path), Some(f)) = (self.files.get(&id).cloned(), self.chat.file(id)) else { continue };
                    let (name, tx) = (f.name.clone(), self.tx.clone());
                    std::thread::spawn(move || {
                        let text = match rider_ui::files::save_copy(&path, &name) {
                            Ok(Some(to)) => format!("{} {}", tr("Saved to", "Saxlanıldı:"), to.display()),
                            Ok(None) => return,
                            Err(e) => format!("{name}: {e}"),
                        };
                        let _ = tx.send(Event::Notice(text));
                    });
                }
                Action::Close => {}
            }
        }
    }
}

impl Panel {
    fn tunnel_event(&mut self, t: crate::tunnel_ask::TunnelEvent, ctx: &egui::Context) {
        use crate::tunnel_ask::{TunnelAsk, TunnelEvent, opened_notice};
        match t {
            TunnelEvent::Request { id, target, seconds } => {
                self.tunnel = Some(TunnelAsk::new(id, target, seconds));
                if !quiet() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                attention(ctx);
            }
            TunnelEvent::Expired(id) => {
                if self.tunnel.as_ref().is_some_and(|a| a.id == id) {
                    self.tunnel = None;
                }
            }
            TunnelEvent::Opened(target) => {
                if self.tunnels_seen.insert(target.clone()) {
                    self.chat.notice(opened_notice(&target));
                }
            }
        }
    }

    fn tunnel_banner(&mut self, ui: &mut Ui) {
        if self.tunnel.as_ref().is_some_and(crate::tunnel_ask::TunnelAsk::expired) {
            self.tunnel = None;
        }
        let device = self.device.clone().unwrap_or_default();
        let answer = self.tunnel.as_ref().and_then(|a| crate::tunnel_ask::banner(ui, a, &device).map(|c| a.reply(c)));
        if let Some(line) = answer {
            Self::send(&line);
            self.tunnel = None;
        }
    }

    fn restart_banner(&mut self, ui: &mut Ui) {
        let Some(warning) = &self.restart else { return };
        let device = self.device.clone().unwrap_or_default();
        if crate::restart_ask::banner(ui, warning, &device) {
            Self::send(crate::restart_ask::CANCEL);
        }
    }

    fn answer(&mut self, yes: bool) {
        self.request = None;
        Self::send(if yes { "accept" } else { "decline" });
        if yes {
            show(SW_MINIMIZE);
        }
    }

    fn request_page(&mut self, ui: &mut Ui) {
        let Some((company, until, total)) = self.request.clone() else { return };
        let left = until.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            self.answer(false);
            return;
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        let device = self.device.clone().unwrap_or_default();
        let mut choice = None;
        egui::CentralPanel::default().show(ui, |ui| {
            const CONTENT_H: f32 = 330.0;
            const BUTTON: egui::Vec2 = egui::vec2(124.0, 34.0);
            const GAP: f32 = 10.0;
            ui.add_space(((ui.available_height() - CONTENT_H) / 2.0).max(12.0));
            ui.vertical_centered(|ui| {
                ui.set_max_width(320.0);
                widgets::avatar(ui, icon::COMPANY, None);
                ui.add_space(14.0);
                ui.label(widgets::title(tr("Remote access request", "Uzaqdan giriş sorğusu"), 17.0));
                ui.add_space(14.0);
                ui.label(widgets::title(&device, 15.0));
                ui.label(RichText::new(trf!("Admin of {company}", "{company} admini")).weak());
                ui.add_space(12.0);
                ui.label(tr("wants to see and control this computer.", "bu kompüteri görmək və idarə etmək istəyir."));
                ui.add_space(6.0);
                widgets::hint(
                    ui,
                    tr(
                        "Nothing happens unless you accept. You can end the session at any time.",
                        "Siz qəbul etməsəniz, heç nə baş vermir. Seansı istənilən vaxt bitirə bilərsiniz.",
                    ),
                );
                ui.add_space(18.0);
                let row = egui::vec2(BUTTON.x * 2.0 + GAP, BUTTON.y);
                ui.allocate_ui_with_layout(row, Layout::left_to_right(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    let accept = egui::Button::new(
                        RichText::new(tr("Accept", "Qəbul et")).family(widgets::semibold()).color(egui::Color32::WHITE),
                    )
                    .fill(widgets::ACCENT);
                    if ui.add_sized(BUTTON, accept).clicked() {
                        choice = Some(true);
                    }
                    if ui.add_sized(BUTTON, egui::Button::new(tr("Decline", "Rədd et"))).clicked() {
                        choice = Some(false);
                    }
                });
                ui.add_space(18.0);
                let (bar, _) = ui.allocate_exact_size(egui::vec2(row.x, 4.0), egui::Sense::hover());
                let p = ui.painter();
                p.rect_filled(bar, 2.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
                let part = (left.as_secs_f32() / total).clamp(0.0, 1.0);
                let done = egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * part, bar.height()));
                let color = if left.as_secs() <= 10 { widgets::AMBER } else { widgets::ACCENT };
                p.rect_filled(done, 2.0, color);
                ui.add_space(6.0);
                let secs = left.as_secs() + 1;
                widgets::hint(ui, &trf!("Declines by itself in {secs} s", "{secs} san. sonra avtomatik rədd ediləcək"));
            });
        });
        if let Some(yes) = choice {
            self.answer(yes);
        }
    }
}

fn quiet() -> bool {
    std::env::var_os("RIDER_PREVIEW").is_some()
}

fn attention(ctx: &egui::Context) {
    if ctx.input(|i| i.focused) {
        return;
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
}

static WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

fn show(how: SHOW_WINDOW_CMD) {
    let h = WINDOW.load(std::sync::atomic::Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = ShowWindow(HWND(h as *mut _), how);
        }
    }
}

impl eframe::App for Panel {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_events(&ctx);
        if self.request.is_some() {
            if ctx.input(|i| i.viewport().close_requested()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.answer(false);
            }
            self.request_page(ui);
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            show(SW_MINIMIZE);
        }
        self.header(ui);
        self.restart_banner(ui);
        self.tunnel_banner(ui);
        let mut actions = Vec::new();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(ui.visuals().panel_fill))
            .show(ui, |ui| actions = self.chat.show(ui));
        self.act(actions);
    }
}

fn parse(line: &str) -> Option<Event> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_owned();
    let n = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or_default();
    if let Some(t) = crate::tunnel_ask::parse(&v) {
        return Some(Event::Tunnel(t));
    }
    if let Some(r) = crate::restart_ask::parse(&v) {
        return Some(Event::Restart(r));
    }
    Some(match v.get("event")?.as_str()? {
        "session" => Event::Session {
            device: s("device"),
            files: v.get("files").and_then(serde_json::Value::as_bool).unwrap_or(false),
        },
        "chat" => Event::Chat(s("text")),
        "request" => Event::Request { company: s("company"), seconds: n("seconds") },
        "file" => Event::File {
            id: n("id"),
            mine: v.get("dir").and_then(|d| d.as_str()) == Some("out"),
            name: s("name"),
            size: n("size"),
        },
        "file_progress" => Event::FileProgress { id: n("id"), done: n("done") },
        "file_done" => Event::FileDone { id: n("id"), path: v.get("path").and_then(|p| p.as_str()).map(PathBuf::from) },
        "file_failed" => Event::FileFailed { id: n("id"), reason: s("reason") },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_events() {
        assert_eq!(
            parse(r#"{"event":"session","device":"Laptop","files":true}"#),
            Some(Event::Session { device: "Laptop".into(), files: true })
        );
        assert_eq!(parse(r#"{"event":"chat","text":"hi"}"#), Some(Event::Chat("hi".into())));
        assert_eq!(
            parse(r#"{"event":"file","id":2,"dir":"in","name":"a.txt","size":5}"#),
            Some(Event::File { id: 2, mine: false, name: "a.txt".into(), size: 5 })
        );
        assert_eq!(
            parse(r#"{"event":"file_done","id":2,"path":"C:\\x\\a.txt"}"#),
            Some(Event::FileDone { id: 2, path: Some(PathBuf::from(r"C:\x\a.txt")) })
        );
        assert_eq!(
            parse(r#"{"event":"request","company":"Northwind","seconds":60}"#),
            Some(Event::Request { company: "Northwind".into(), seconds: 60 })
        );
        assert_eq!(parse("garbage"), None);
    }
}

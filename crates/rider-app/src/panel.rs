//! `RemoteDesktopRider --session-panel`: the chat window of a paired session,
//! started by the host's service for the person signed in at this computer
//! (see rider-host's `panel`). It waits in the taskbar, minimized, and comes
//! up when the viewer writes or sends a file.
//!
//! Input (stdin): JSON events from the host, `session` first, then `chat`,
//! `file`, `file_progress`, `file_done`, `file_failed`. Output (stdout):
//! `chat "<text>"`, `send-file "<path>"`, `cancel-file <id>`, `end`. When the
//! input closes the session is over and the window goes.

use crate::widgets::{self, GREEN};
use eframe::egui::{self, Align, Layout, Ui};
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
    SHOW_WINDOW_CMD, SW_MINIMIZE, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, ShowWindow,
};

pub const TITLE: &str = "Chat - Remote Desktop Rider";

/// What arrives from the host.
#[derive(Debug, PartialEq)]
enum Event {
    Session {
        device: String,
        files: bool,
    },
    Chat(String),
    File {
        id: u64,
        mine: bool,
        name: String,
        size: u64,
    },
    FileProgress {
        id: u64,
        done: u64,
    },
    FileDone {
        id: u64,
        path: Option<PathBuf>,
    },
    FileFailed {
        id: u64,
        reason: String,
    },
    /// A note to show (e.g. where a file was saved).
    Notice(String),
}

pub struct Panel {
    events: mpsc::Receiver<Event>,
    tx: mpsc::Sender<Event>,
    device: Option<String>,
    chat: Chat,
    /// Where each received file is.
    files: HashMap<u64, PathBuf>,
}

impl Panel {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // The window is created hidden: show it minimized, waiting in the
        // taskbar until there is something to read. Before the input thread
        // starts, so a first message can bring it up.
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
                            // From here: a minimized window does not run the
                            // app (no frames) until it is shown.
                            if matches!(ev, Event::Chat(_) | Event::File { mine: false, .. }) {
                                show(SW_SHOWNOACTIVATE);
                            }
                            let _ = input_tx.send(ev);
                            ctx2.request_repaint();
                        }
                    }
                }
            }
            // The session is over. Nothing here is kept, so the window just
            // goes (also while minimized, when the app runs no frames).
            std::process::exit(0);
        });
        let mut chat = Chat::new("");
        chat.closable = false;
        chat.heading = false;
        Self { events, tx, device: None, chat, files: HashMap::new() }
    }

    /// One line to the host.
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

/// Something new from the viewer: flash the window (the input thread has
/// already brought it up, without taking the keyboard).
fn attention(ctx: &egui::Context) {
    if ctx.input(|i| i.focused) {
        return;
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
}

/// This process's window (there is one), once it exists.
static WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

fn show(how: SHOW_WINDOW_CMD) {
    let h = WINDOW.load(std::sync::atomic::Ordering::Relaxed);
    if h != 0 {
        // SAFETY: our own top-level window, alive while the app runs.
        unsafe {
            let _ = ShowWindow(HWND(h as *mut _), how);
        }
    }
}

impl eframe::App for Panel {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_events(&ctx);
        // The chat stays for the whole session: closing only puts it away.
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            show(SW_MINIMIZE);
        }
        self.header(ui);
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
    Some(match v.get("event")?.as_str()? {
        "session" => Event::Session {
            device: s("device"),
            files: v.get("files").and_then(serde_json::Value::as_bool).unwrap_or(false),
        },
        "chat" => Event::Chat(s("text")),
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
        assert_eq!(parse("garbage"), None);
    }
}

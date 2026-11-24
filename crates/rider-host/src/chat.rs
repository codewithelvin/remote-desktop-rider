//! Chat on the host side (protocol 5): messages and files between the
//! viewer and the person at this computer, who sees them in the app (JSON
//! mode, see `ui`). Without the app there is nobody to chat with: the viewer
//! is told so (`HostMsg::ChatReady(false)`) and files dropped on the viewer
//! go to Downloads\Rider as before.
//!
//! Events to the app, one JSON line each:
//!   {"event":"chat","text":"…"}
//!   {"event":"file","id":1,"dir":"in"|"out","name":"a.pdf","size":123}
//!   {"event":"file_progress","id":1,"done":45}
//!   {"event":"file_done","id":1,"path":"C:\\…"}     (path: received files)
//!   {"event":"file_failed","id":1,"reason":"…"}
//!
//! Received files are kept in this session's private temp folder
//! (`rider_core::session_files`) and deleted when the session ends.

use crate::ui::{self, ChatCmd};
use rider_core::proto::{HostMsg, MAX_CHAT, Permissions};
use rider_core::session_files::SessionFiles;
use rider_core::transfer::{self, Cancel};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;

/// One session's chat.
pub struct Chat {
    conn: quinn::Connection,
    out: UnboundedSender<HostMsg>,
    /// The viewer speaks protocol 5.
    v5: bool,
    next_id: AtomicU64,
    cancels: Mutex<HashMap<u64, Cancel>>,
    /// Created on the first received file.
    files: Mutex<Option<Arc<SessionFiles>>>,
}

/// Is someone here to read the chat (the app runs this host)?
pub fn available() -> bool {
    ui::json()
}

impl Chat {
    pub fn new(conn: quinn::Connection, out: UnboundedSender<HostMsg>, v5: bool) -> Arc<Self> {
        if v5 {
            let _ = out.send(HostMsg::ChatReady(available()));
        }
        Arc::new(Self { conn, out, v5, next_id: AtomicU64::new(1), cancels: Mutex::default(), files: Mutex::default() })
    }

    /// A message from the viewer.
    pub fn text_from_viewer(&self, text: &str) {
        let text: String = text.chars().take(MAX_CHAT).collect();
        tracing::info!("chat message from the viewer ({} chars)", text.chars().count());
        ui::emit(json!({ "event": "chat", "text": text }));
    }

    /// Carry out the app's chat commands until the session ends.
    pub async fn run(self: Arc<Self>, perms: impl Fn() -> Permissions + Send + 'static) {
        if !available() {
            return;
        }
        let mut rx = ui::chat_commands().lock().await;
        // Commands typed before this session started belong to no one.
        while rx.try_recv().is_ok() {}
        while let Some(cmd) = rx.recv().await {
            match cmd {
                ChatCmd::Text(text) if self.v5 => {
                    let text: String = text.chars().take(MAX_CHAT).collect();
                    let _ = self.out.send(HostMsg::Chat { text });
                }
                ChatCmd::Text(_) => {}
                ChatCmd::SendFile(path) => self.clone().send_file(path, perms().files),
                ChatCmd::Cancel(id) => {
                    if let Some(c) = self.cancels.lock().unwrap().get(&id) {
                        c.store(true, Ordering::Relaxed);
                    }
                }
            }
        }
    }

    /// The session is over: stop transfers and delete the received files.
    pub fn end(&self) {
        for c in self.cancels.lock().unwrap().values() {
            c.store(true, Ordering::Relaxed);
        }
        self.files.lock().unwrap().take();
    }

    fn new_transfer(&self) -> (u64, Cancel) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let cancel = Cancel::default();
        self.cancels.lock().unwrap().insert(id, cancel.clone());
        (id, cancel)
    }

    fn end_transfer(&self, id: u64) {
        self.cancels.lock().unwrap().remove(&id);
    }

    fn send_file(self: Arc<Self>, path: std::path::PathBuf, allowed: bool) {
        let (id, cancel) = self.new_transfer();
        let name = path.file_name().map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
        ui::emit(json!({ "event": "file", "id": id, "dir": "out", "name": name, "size": size }));
        let refuse = if !self.v5 {
            Some("the other side's version cannot receive files from here")
        } else if !allowed {
            Some("files are not allowed in this session")
        } else {
            None
        };
        if let Some(reason) = refuse {
            ui::emit(json!({ "event": "file_failed", "id": id, "reason": reason }));
            self.end_transfer(id);
            return;
        }
        tokio::spawn(async move {
            let mut report = Throttle::new(size);
            let result = transfer::send(&self.conn, &path, &cancel, |done| {
                if report.due(done) {
                    ui::emit(json!({ "event": "file_progress", "id": id, "done": done }));
                }
            })
            .await;
            match result {
                Ok(r) if r.ok => {
                    tracing::info!("sent {name} ({size} bytes) to the viewer");
                    ui::emit(json!({ "event": "file_done", "id": id }));
                }
                Ok(r) => ui::emit(json!({ "event": "file_failed", "id": id, "reason": r.message })),
                Err(e) => ui::emit(json!({ "event": "file_failed", "id": id, "reason": format!("{e:#}") })),
            }
            self.end_transfer(id);
        });
    }

    /// A file from the viewer, shown in the chat and kept until the session ends.
    pub async fn receive_file(self: Arc<Self>, send: quinn::SendStream, recv: quinn::RecvStream) {
        let files = {
            let mut f = self.files.lock().unwrap();
            if f.is_none() {
                match SessionFiles::create() {
                    Ok(s) => *f = Some(Arc::new(s)),
                    Err(e) => tracing::warn!("no folder for received files: {e:#}"),
                }
            }
            f.clone()
        };
        let Some(files) = files else {
            let _ = transfer::refuse(send, recv, "the other computer cannot store files now").await;
            return;
        };
        let (id, cancel) = self.new_transfer();
        let announced = AtomicU64::new(0);
        let mut report = Throttle::new(0);
        let result = transfer::receive(
            send,
            recv,
            |name, size| {
                announced.store(size, Ordering::Relaxed);
                ui::emit(json!({ "event": "file", "id": id, "dir": "in", "name": name, "size": size }));
            },
            move |name| files.incoming(name),
            &cancel,
            |done| {
                report.size = announced.load(Ordering::Relaxed);
                if report.due(done) {
                    ui::emit(json!({ "event": "file_progress", "id": id, "done": done }));
                }
            },
        )
        .await;
        match result {
            Ok(r) => {
                tracing::info!("received {} ({} bytes) in the chat", r.name, r.size);
                ui::emit(json!({ "event": "file_done", "id": id, "path": r.path }));
            }
            Err(e) => ui::emit(json!({ "event": "file_failed", "id": id, "reason": format!("{e:#}") })),
        }
        self.end_transfer(id);
    }
}

/// Progress reports: about every 2% (and at the end), not every chunk.
struct Throttle {
    size: u64,
    next: u64,
}

impl Throttle {
    fn new(size: u64) -> Self {
        Self { size, next: 0 }
    }

    fn due(&mut self, done: u64) -> bool {
        if done < self.next && done < self.size {
            return false;
        }
        self.next = done + (self.size / 50).max(256 * 1024);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_reported_in_steps() {
        let mut t = Throttle::new(100 * 1024 * 1024);
        let chunk = 256 * 1024;
        let reports = (1..=400u64).filter(|i| t.due(i * chunk)).count();
        assert!((45..=55).contains(&reports), "{reports}");
        // The last chunk is always reported.
        let mut t = Throttle::new(1000);
        assert!(t.due(500));
        assert!(t.due(1000));
    }
}

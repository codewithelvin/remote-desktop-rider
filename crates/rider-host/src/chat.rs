//! Chat on the host side (protocol 5): messages and files between the
//! viewer and the person at this computer. A one-time session chats in the
//! app that runs this host (JSON mode, see `ui`); a paired session in a
//! chat window started for the signed-in user (`panel`). With neither
//! (nobody signed in, an old viewer) the viewer is told nobody is there
//! (`HostMsg::ChatReady(false)`) and files dropped on the viewer go to
//! Downloads\Rider as before.
//!
//! Events, one JSON line each:
//!   {"event":"chat","text":"…"}
//!   {"event":"file","id":1,"dir":"in"|"out","name":"a.pdf","size":123}
//!   {"event":"file_progress","id":1,"done":45}
//!   {"event":"file_done","id":1,"path":"C:\\…"}     (path: received files)
//!   {"event":"file_failed","id":1,"reason":"…"}
//!
//! One-time sessions keep received files in the session's private temp
//! folder (`rider_core::session_files`), deleted when the session ends.
//! Paired sessions are the owner's own devices: their files go to
//! Downloads\Rider and stay, written as the signed-in user.

use crate::panel::Panel;
use crate::ui::{self, ChatCmd};
use anyhow::Result;
use rider_core::proto::{HostMsg, MAX_CHAT, Permissions};
use rider_core::session_files::SessionFiles;
use rider_core::transfer::{self, Cancel};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// Where the person at this computer chats.
pub enum Link {
    /// The app running this host (one-time sessions).
    App,
    /// The paired session's chat window, and what is typed in it.
    Panel(Panel, Mutex<Option<UnboundedReceiver<ChatCmd>>>),
    /// Nobody.
    None,
}

impl Link {
    pub fn panel((panel, cmds): (Panel, UnboundedReceiver<ChatCmd>)) -> Self {
        Self::Panel(panel, Mutex::new(Some(cmds)))
    }
}

/// One session's chat.
pub struct Chat {
    conn: quinn::Connection,
    out: UnboundedSender<HostMsg>,
    /// The viewer speaks protocol 5.
    v5: bool,
    link: Link,
    /// Someone here sees the chat (a window can close during the session).
    ready: AtomicBool,
    next_id: AtomicU64,
    cancels: Mutex<HashMap<u64, Cancel>>,
    /// One-time sessions: created on the first received file.
    files: Mutex<Option<Arc<SessionFiles>>>,
}

impl Chat {
    pub fn new(conn: quinn::Connection, out: UnboundedSender<HostMsg>, v5: bool, link: Link) -> Arc<Self> {
        let ready = !matches!(link, Link::None);
        if v5 {
            let _ = out.send(HostMsg::ChatReady(ready));
        }
        Arc::new(Self {
            conn,
            out,
            v5,
            link,
            ready: AtomicBool::new(ready),
            next_id: AtomicU64::new(1),
            cancels: Mutex::default(),
            files: Mutex::default(),
        })
    }

    /// Is someone here to read the chat?
    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    fn emit(&self, event: serde_json::Value) {
        match &self.link {
            Link::App => ui::emit(event),
            Link::Panel(p, _) => p.emit(event),
            Link::None => {}
        }
    }

    /// A message from the viewer.
    pub fn text_from_viewer(&self, text: &str) {
        let text: String = text.chars().take(MAX_CHAT).collect();
        tracing::info!("chat message from the viewer ({} chars)", text.chars().count());
        self.emit(json!({ "event": "chat", "text": text }));
    }

    /// Carry out the chat commands from this computer until the session ends.
    pub async fn run(self: Arc<Self>, perms: impl Fn() -> Permissions + Send + 'static) {
        match &self.link {
            Link::App => {
                let mut rx = ui::chat_commands().lock().await;
                // Commands typed before this session started belong to no one.
                while rx.try_recv().is_ok() {}
                while let Some(cmd) = rx.recv().await {
                    self.clone().command(cmd, &perms);
                }
            }
            Link::Panel(_, cmds) => {
                let Some(mut rx) = cmds.lock().unwrap().take() else { return };
                while let Some(cmd) = rx.recv().await {
                    self.clone().command(cmd, &perms);
                }
                // The window is gone (closed, or it failed).
                tracing::info!("the chat window closed");
                self.ready.store(false, Ordering::Relaxed);
                if self.v5 {
                    let _ = self.out.send(HostMsg::ChatReady(false));
                }
            }
            Link::None => {}
        }
    }

    fn command(self: Arc<Self>, cmd: ChatCmd, perms: &impl Fn() -> Permissions) {
        match cmd {
            ChatCmd::Text(text) if self.v5 => {
                let text: String = text.chars().take(MAX_CHAT).collect();
                let _ = self.out.send(HostMsg::Chat { text });
            }
            ChatCmd::Text(_) => {}
            ChatCmd::SendFile(path) => self.send_file(path, perms().files),
            ChatCmd::Cancel(id) => {
                if let Some(c) = self.cancels.lock().unwrap().get(&id) {
                    c.store(true, Ordering::Relaxed);
                }
            }
            ChatCmd::End => {
                tracing::info!("session ended from the chat window");
                crate::banner::DISCONNECT_REQUESTED.store(true, Ordering::SeqCst);
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

    fn failed(&self, id: u64, reason: String) {
        self.emit(json!({ "event": "file_failed", "id": id, "reason": reason }));
        self.end_transfer(id);
    }

    fn send_file(self: Arc<Self>, path: PathBuf, allowed: bool) {
        let (id, cancel) = self.new_transfer();
        let name = path.file_name().map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
        tokio::spawn(async move {
            // Opened with the signed-in user's rights: the service's agent
            // could read any file, the person at this computer may not.
            let opened = tokio::task::spawn_blocking({
                let path = path.clone();
                move || crate::system::as_console_user(|| std::fs::File::open(&path))
            })
            .await;
            let file = match opened {
                Ok(Ok(Ok(f))) => Some(f),
                _ => None,
            };
            let size = file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len());
            self.emit(json!({ "event": "file", "id": id, "dir": "out", "name": name, "size": size }));
            let file = match (file, self.v5, allowed) {
                (_, false, _) => {
                    return self.failed(id, "the other side's version cannot receive files from here".into());
                }
                (_, _, false) => return self.failed(id, "files are not allowed in this session".into()),
                (None, ..) => return self.failed(id, format!("cannot open {}", path.display())),
                (Some(f), ..) => f,
            };
            let mut report = Throttle::new(size);
            let result = transfer::send_open(&self.conn, file, name.clone(), &cancel, |done| {
                if report.due(done) {
                    self.emit(json!({ "event": "file_progress", "id": id, "done": done }));
                }
            })
            .await;
            match result {
                Ok(r) if r.ok => {
                    tracing::info!("sent {name} ({size} bytes) to the viewer");
                    self.emit(json!({ "event": "file_done", "id": id }));
                    self.end_transfer(id);
                }
                Ok(r) => self.failed(id, r.message),
                Err(e) => self.failed(id, format!("{e:#}")),
            }
        });
    }

    /// A file from the viewer, shown in the chat.
    pub async fn receive_file(self: Arc<Self>, send: quinn::SendStream, recv: quinn::RecvStream) {
        let paired = matches!(self.link, Link::Panel(..));
        let session_files = if paired {
            None
        } else {
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
            if files.is_none() {
                let _ = transfer::refuse(send, recv, "the other computer cannot store files now").await;
                return;
            }
            files
        };
        let downloads = if paired {
            match crate::system::incoming_files_dir() {
                Ok(d) => Some(d),
                Err(e) => {
                    let _ = transfer::refuse(send, recv, &format!("{e:#}")).await;
                    return;
                }
            }
        } else {
            None
        };
        let create = move |name: &str| -> Result<(PathBuf, std::fs::File)> {
            match (&session_files, &downloads) {
                (Some(files), _) => files.incoming(name),
                (None, Some(dir)) => {
                    let name = name.to_owned();
                    crate::system::as_console_user(|| crate::session::create_incoming(dir, &name))?
                }
                (None, None) => anyhow::bail!("nowhere to store files"),
            }
        };
        // In the user's own folder, a partial file goes as that user too.
        let discard = |p: PathBuf| {
            let _ = crate::system::as_console_user(|| std::fs::remove_file(p));
        };
        let (id, cancel) = self.new_transfer();
        let announced = AtomicU64::new(0);
        let mut report = Throttle::new(0);
        let result = transfer::receive_with(
            send,
            recv,
            |name, size| {
                announced.store(size, Ordering::Relaxed);
                self.emit(json!({ "event": "file", "id": id, "dir": "in", "name": name, "size": size }));
            },
            create,
            discard,
            &cancel,
            |done| {
                report.size = announced.load(Ordering::Relaxed);
                if report.due(done) {
                    self.emit(json!({ "event": "file_progress", "id": id, "done": done }));
                }
            },
        )
        .await;
        match result {
            Ok(r) => {
                tracing::info!("received {} ({} bytes) in the chat", r.name, r.size);
                self.emit(json!({ "event": "file_done", "id": id, "path": r.path }));
                self.end_transfer(id);
            }
            Err(e) => self.failed(id, format!("{e:#}")),
        }
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

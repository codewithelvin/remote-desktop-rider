use crate::panel::Panel;
use crate::ui::{self, ChatCmd};
use anyhow::Result;
use rider_core::proto::{FileHeader, HostMsg, MAX_CHAT, Permissions};
use rider_core::session_files::SessionFiles;
use rider_core::transfer::{self, Cancel};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

const TUNNEL_ANSWER_TIME: Duration = Duration::from_secs(60);

pub enum Link {
    App,
    Panel(Panel, Mutex<Option<UnboundedReceiver<ChatCmd>>>),
    None,
}

impl Link {
    pub fn panel((panel, cmds): (Panel, UnboundedReceiver<ChatCmd>)) -> Self {
        Self::Panel(panel, Mutex::new(Some(cmds)))
    }
}

pub struct Chat {
    conn: quinn::Connection,
    out: UnboundedSender<HostMsg>,
    v5: bool,
    link: Link,
    ready: AtomicBool,
    next_id: AtomicU64,
    cancels: Mutex<HashMap<u64, Cancel>>,
    files: Mutex<Option<Arc<SessionFiles>>>,
    tunnel_asks: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    restart_cancel: Mutex<Option<oneshot::Sender<()>>>,
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
            tunnel_asks: Mutex::default(),
            restart_cancel: Mutex::default(),
        })
    }

    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    fn emit(&self, event: &serde_json::Value) {
        match &self.link {
            Link::App => ui::emit(event),
            Link::Panel(p, _) => p.emit(event),
            Link::None => {}
        }
    }

    pub async fn ask_tunnel(&self, target: &str) -> Option<bool> {
        if !self.ready() {
            return None;
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.tunnel_asks.lock().unwrap().insert(id, tx);
        self.emit(
            &json!({ "event": "tunnel_request", "id": id, "target": target, "seconds": TUNNEL_ANSWER_TIME.as_secs() }),
        );
        let answer = tokio::time::timeout(TUNNEL_ANSWER_TIME, rx).await;
        self.tunnel_asks.lock().unwrap().remove(&id);
        match answer {
            Ok(Ok(allow)) => Some(allow),
            Ok(Err(_)) => None,
            Err(_) => {
                self.emit(&json!({ "event": "tunnel_request_expired", "id": id }));
                Some(false)
            }
        }
    }

    pub async fn warn_restart(&self, warning: Duration) -> Option<bool> {
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.restart_cancel.lock().unwrap();
            if pending.is_some() {
                return None;
            }
            *pending = Some(tx);
        }
        self.emit(&json!({ "event": "restart", "seconds": warning.as_secs() }));
        let cancelled = matches!(tokio::time::timeout(warning, rx).await, Ok(Ok(())));
        self.restart_cancel.lock().unwrap().take();
        if cancelled {
            self.emit(&json!({ "event": "restart_cancelled" }));
        }
        Some(!cancelled)
    }

    pub fn tunnel_event(&self, event: &str, target: &str) {
        self.emit(&json!({ "event": event, "target": target }));
    }

    pub fn text_from_viewer(&self, text: &str) {
        let text: String = text.chars().take(MAX_CHAT).collect();
        tracing::info!("chat message from the viewer ({} chars)", text.chars().count());
        self.emit(&json!({ "event": "chat", "text": text }));
    }

    pub async fn run(self: Arc<Self>, perms: impl Fn() -> Permissions + Send + 'static) {
        match &self.link {
            Link::App => {
                let mut rx = ui::chat_commands().lock().await;
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
            ChatCmd::SendFile(path) => self.send_file(path, perms().files),
            ChatCmd::Cancel(id) => {
                if let Some(c) = self.cancels.lock().unwrap().get(&id) {
                    c.store(true, Ordering::Relaxed);
                }
            }
            ChatCmd::Tunnel { id, allow } => {
                if let Some(tx) = self.tunnel_asks.lock().unwrap().remove(&id) {
                    let _ = tx.send(allow);
                }
            }
            ChatCmd::CancelRestart => {
                if let Some(tx) = self.restart_cancel.lock().unwrap().take() {
                    tracing::info!("restart cancelled on the host");
                    let _ = tx.send(());
                }
            }
            ChatCmd::Text(_) | ChatCmd::Answer(_) => {}
            ChatCmd::End => {
                tracing::info!("session ended from the chat window");
                crate::banner::DISCONNECT_REQUESTED.store(true, Ordering::SeqCst);
            }
        }
    }

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

    fn failed(&self, id: u64, reason: &str) {
        self.emit(&json!({ "event": "file_failed", "id": id, "reason": reason }));
        self.end_transfer(id);
    }

    fn send_file(self: Arc<Self>, path: PathBuf, allowed: bool) {
        let (id, cancel) = self.new_transfer();
        let name = path.file_name().map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
        tokio::spawn(async move {
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
            self.emit(&json!({ "event": "file", "id": id, "dir": "out", "name": name, "size": size }));
            let file = match (file, self.v5, allowed) {
                (_, false, _) => {
                    return self.failed(id, "the other side's version cannot receive files from here");
                }
                (_, _, false) => return self.failed(id, "files are not allowed in this session"),
                (None, ..) => return self.failed(id, &format!("cannot open {}", path.display())),
                (Some(f), ..) => f,
            };
            let mut report = Throttle::new(size);
            let result = transfer::send_open(&self.conn, file, name.clone(), &cancel, |done| {
                if report.due(done) {
                    self.emit(&json!({ "event": "file_progress", "id": id, "done": done }));
                }
            })
            .await;
            match result {
                Ok(r) if r.ok => {
                    tracing::info!("sent {name} ({size} bytes) to the viewer");
                    self.emit(&json!({ "event": "file_done", "id": id }));
                    self.end_transfer(id);
                }
                Ok(r) => self.failed(id, &r.message),
                Err(e) => self.failed(id, &format!("{e:#}")),
            }
        });
    }

    pub async fn receive_file(self: Arc<Self>, header: FileHeader, send: quinn::SendStream, recv: quinn::RecvStream) {
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
        let discard = |p: PathBuf| {
            let _ = crate::system::as_console_user(|| std::fs::remove_file(p));
        };
        let (id, cancel) = self.new_transfer();
        let announced = AtomicU64::new(0);
        let mut report = Throttle::new(0);
        let result = transfer::receive_with(
            header,
            send,
            recv,
            |name, size| {
                announced.store(size, Ordering::Relaxed);
                self.emit(&json!({ "event": "file", "id": id, "dir": "in", "name": name, "size": size }));
            },
            create,
            discard,
            &cancel,
            |done| {
                report.size = announced.load(Ordering::Relaxed);
                if report.due(done) {
                    self.emit(&json!({ "event": "file_progress", "id": id, "done": done }));
                }
            },
        )
        .await;
        match result {
            Ok(r) => {
                tracing::info!("received {} ({} bytes) in the chat", r.name, r.size);
                self.emit(&json!({ "event": "file_done", "id": id, "path": r.path }));
                self.end_transfer(id);
            }
            Err(e) => self.failed(id, &format!("{e:#}")),
        }
    }
}

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
        let mut t = Throttle::new(1000);
        assert!(t.due(500));
        assert!(t.due(1000));
    }
}

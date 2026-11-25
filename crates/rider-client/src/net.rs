use crate::video::{self, IncomingFrame};
use rider_core::i18n::tr;
use rider_core::pixels::Nv12Frame;
use rider_core::proto::{
    ClientMsg, CursorShape, HostMsg, MAX_CHAT, MAX_FRAME_BYTES, MonitorInfo, MouseMove, Permissions, encode,
    read_known, split_frame, write_msg,
};
use rider_core::session_files::SessionFiles;
use rider_core::transfer::{self, Cancel};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedSender};
use winit::event_loop::EventLoopProxy;

#[derive(Debug)]
pub struct Session {
    pub conn: quinn::Connection,
    pub send: quinn::SendStream,
    pub recv: quinn::RecvStream,
    pub relayed: bool,
}

const MAX_FRAMES_READING: usize = 64;

#[derive(Debug)]
pub enum UiEvent {
    NewFrame,
    Monitors(Vec<MonitorInfo>, u32),
    Cursor(CursorShape),
    Permissions(Permissions),
    Chat(String),
    ChatReady(bool),
    File { id: u64, mine: bool, name: String, size: u64 },
    FileProgress { id: u64, done: u64 },
    FileDone { id: u64, path: Option<PathBuf> },
    FileFailed { id: u64, reason: String },
    Attach(Vec<PathBuf>),
    Notice(String),
    Closed { conn: usize, reason: String, lost: bool },
    Reconnected(Session),
    ReconnectFailed(String),
}

#[derive(Default)]
pub struct Shared {
    pub latest: Mutex<Option<Nv12Frame>>,
    pub rtt_us: AtomicU64,
    pub clock_offset_us: AtomicI64,
    best_rtt_us: AtomicU64,
    pub latency_us: AtomicU64,
    pub decode_us: AtomicU64,
    pub frames: AtomicU32,
    pub host_stats: Mutex<String>,
    pub last_rx_us: AtomicU64,
    denied: AtomicU8,
    next_file: AtomicU64,
    cancels: Mutex<HashMap<u64, Cancel>>,
    received: Mutex<Option<Arc<SessionFiles>>>,
}

impl Shared {
    pub fn silent_for(&self) -> Duration {
        Duration::from_micros(rider_core::now_us().saturating_sub(self.last_rx_us.load(Ordering::Relaxed)))
    }

    fn heard(&self) {
        self.last_rx_us.store(rider_core::now_us(), Ordering::Relaxed);
    }

    pub fn permissions(&self) -> Permissions {
        Permissions::from_bits(!self.denied.load(Ordering::Relaxed) & 0b111)
    }

    fn set_permissions(&self, p: Permissions) {
        self.denied.store(!p.bits() & 0b111, Ordering::Relaxed);
    }

    fn new_transfer(&self) -> (u64, Cancel) {
        let id = self.next_file.fetch_add(1, Ordering::Relaxed) + 1;
        let cancel = Cancel::default();
        self.cancels.lock().unwrap().insert(id, cancel.clone());
        (id, cancel)
    }

    fn end_transfer(&self, id: u64) {
        self.cancels.lock().unwrap().remove(&id);
    }

    pub fn cancel(&self, id: u64) {
        if let Some(c) = self.cancels.lock().unwrap().get(&id) {
            c.store(true, Ordering::Relaxed);
        }
    }

    pub fn end_files(&self) {
        for c in self.cancels.lock().unwrap().values() {
            c.store(true, Ordering::Relaxed);
        }
        self.received.lock().unwrap().take();
    }

    fn received_files(&self) -> anyhow::Result<Arc<SessionFiles>> {
        let mut r = self.received.lock().unwrap();
        if r.is_none() {
            *r = Some(Arc::new(SessionFiles::create()?));
        }
        Ok(r.clone().expect("just set"))
    }
}

struct Throttle {
    size: u64,
    next: u64,
}

impl Throttle {
    fn due(&mut self, done: u64) -> bool {
        if done < self.next && done < self.size {
            return false;
        }
        self.next = done + (self.size / 50).max(256 * 1024);
        true
    }
}

#[derive(Clone)]
pub struct NetHandle {
    pub conn: quinn::Connection,
    pub ctrl: UnboundedSender<ClientMsg>,
    rt: tokio::runtime::Handle,
    proxy: EventLoopProxy<UiEvent>,
    mouse_seq: Arc<AtomicU32>,
}

impl NetHandle {
    pub fn send(&self, msg: ClientMsg) {
        let _ = self.ctrl.send(msg);
    }

    pub fn mouse_move(&self, x: u16, y: u16) {
        let seq = self.mouse_seq.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        if let Ok(b) = encode(&MouseMove { seq, x, y }) {
            let _ = self.conn.send_datagram(b.into());
        }
    }

    pub fn mouse_seq(&self) -> u32 {
        self.mouse_seq.load(Ordering::Relaxed)
    }

    pub fn chat(&self) -> bool {
        rider_core::tls::speaks_v5(&self.conn)
    }

    pub fn send_file(&self, path: PathBuf, shared: &Arc<Shared>) {
        let (id, cancel) = shared.new_transfer();
        let name = path.file_name().map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
        let _ = self.proxy.send_event(UiEvent::File { id, mine: true, name, size });
        if !shared.permissions().files {
            let reason = tr(
                "The person at that computer does not allow files",
                "O kompüterdəki şəxs fayl göndərməyə icazə vermir",
            );
            let _ = self.proxy.send_event(UiEvent::FileFailed { id, reason: reason.into() });
            shared.end_transfer(id);
            return;
        }
        let (conn, proxy, shared) = (self.conn.clone(), self.proxy.clone(), shared.clone());
        self.rt.spawn(async move {
            let mut report = Throttle { size, next: 0 };
            let p = proxy.clone();
            let result = transfer::send(&conn, &path, &cancel, |done| {
                if report.due(done) {
                    let _ = p.send_event(UiEvent::FileProgress { id, done });
                }
            })
            .await;
            let ev = match result {
                Ok(r) if r.ok => UiEvent::FileDone { id, path: None },
                Ok(r) => UiEvent::FileFailed { id, reason: r.message },
                Err(e) => UiEvent::FileFailed { id, reason: format!("{e:#}") },
            };
            let _ = proxy.send_event(ev);
            shared.end_transfer(id);
        });
    }
}

async fn receive_file(
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    proxy: EventLoopProxy<UiEvent>,
    shared: Arc<Shared>,
) {
    if !shared.permissions().files {
        let _ = transfer::refuse(send, recv, "files are not allowed in this session").await;
        return;
    }
    let files = match shared.received_files() {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("no folder for received files: {e:#}");
            let _ = transfer::refuse(send, recv, "the viewer cannot store files now").await;
            return;
        }
    };
    let (id, cancel) = shared.new_transfer();
    let announced = AtomicU64::new(0);
    let mut report = Throttle { size: 0, next: 0 };
    let result = transfer::receive(
        send,
        recv,
        |name, size| {
            announced.store(size, Ordering::Relaxed);
            let _ = proxy.send_event(UiEvent::File { id, mine: false, name: name.into(), size });
        },
        move |name| files.incoming(name),
        &cancel,
        |done| {
            report.size = announced.load(Ordering::Relaxed);
            if report.due(done) {
                let _ = proxy.send_event(UiEvent::FileProgress { id, done });
            }
        },
    )
    .await;
    let ev = match result {
        Ok(r) => UiEvent::FileDone { id, path: Some(r.path) },
        Err(e) => UiEvent::FileFailed { id, reason: format!("{e:#}") },
    };
    let _ = proxy.send_event(ev);
    shared.end_transfer(id);
}

pub fn start(
    rt: &tokio::runtime::Handle,
    s: Session,
    proxy: EventLoopProxy<UiEvent>,
    shared: &Arc<Shared>,
) -> NetHandle {
    let Session { conn, send, recv, .. } = s;
    let id = conn.stable_id();
    shared.best_rtt_us.store(0, Ordering::Relaxed);
    shared.heard();
    let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel::<ClientMsg>();

    rt.spawn(async move {
        let mut send = send;
        while let Some(m) = ctrl_rx.recv().await {
            if write_msg(&mut send, &m).await.is_err() {
                break;
            }
        }
    });

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (clip_local_tx, mut clip_local_rx) = mpsc::unbounded_channel::<String>();
    let (clip_remote_tx, clip_remote_rx) = std::sync::mpsc::channel::<String>();
    rider_core::clipboard::spawn(clip_local_tx, clip_remote_rx, stop.clone());
    {
        let ctrl = ctrl_tx.clone();
        let shared = shared.clone();
        rt.spawn(async move {
            while let Some(text) = clip_local_rx.recv().await {
                if shared.permissions().clipboard {
                    let _ = ctrl.send(ClientMsg::Clipboard { text });
                }
            }
        });
    }

    {
        let proxy = proxy.clone();
        let shared = shared.clone();
        let conn = conn.clone();
        let stop = stop.clone();
        rt.spawn(async move {
            let mut recv = recv;
            let (reason, lost) = loop {
                let msg = read_known::<HostMsg>(&mut recv).await;
                shared.heard();
                match msg {
                    Ok(Some(HostMsg::Monitors { list, active })) => {
                        let _ = proxy.send_event(UiEvent::Monitors(list, active));
                    }
                    Ok(Some(HostMsg::Clipboard { text })) => {
                        let _ = clip_remote_tx.send(text);
                    }
                    Ok(Some(HostMsg::Permissions(p))) => {
                        shared.set_permissions(p);
                        let _ = proxy.send_event(UiEvent::Permissions(p));
                    }
                    Ok(Some(HostMsg::Chat { text })) => {
                        let text: String = text.chars().take(MAX_CHAT).collect();
                        let _ = proxy.send_event(UiEvent::Chat(text));
                    }
                    Ok(Some(HostMsg::ChatReady(ready))) => {
                        let _ = proxy.send_event(UiEvent::ChatReady(ready));
                    }
                    Ok(Some(HostMsg::Pong { client_us, host_us })) => {
                        let now = rider_core::now_us();
                        let rtt = now.saturating_sub(client_us);
                        shared.rtt_us.store(rtt, Ordering::Relaxed);
                        let best = shared.best_rtt_us.load(Ordering::Relaxed);
                        if best == 0 || rtt <= best + 2_000 {
                            if best == 0 || rtt < best {
                                shared.best_rtt_us.store(rtt, Ordering::Relaxed);
                            }
                            let offset = host_us as i64 - (client_us + rtt / 2) as i64;
                            shared.clock_offset_us.store(offset, Ordering::Relaxed);
                        }
                    }
                    Ok(Some(HostMsg::Stats { fps, bitrate_kbps, encode_ms, encoder })) => {
                        *shared.host_stats.lock().unwrap() = format!(
                            "host {fps:.0} fps, {:.1} Mbps, enc {encode_ms:.1} ms ({encoder})",
                            bitrate_kbps as f32 / 1000.0
                        );
                    }
                    Ok(Some(HostMsg::Cursor { shape })) => {
                        let _ = proxy.send_event(UiEvent::Cursor(CursorShape::from_u8(shape)));
                    }
                    Ok(Some(HostMsg::Bye { reason })) => break (reason, false),
                    Ok(Some(_)) => {}
                    Ok(None) => break ("host closed the session".into(), false),
                    Err(e) => {
                        use quinn::ConnectionError as E;
                        let lost =
                            matches!(conn.close_reason(), None | Some(E::TimedOut | E::Reset | E::TransportError(_)));
                        break (format!("connection lost: {e}"), lost);
                    }
                }
            };
            stop.store(true, Ordering::Relaxed);
            conn.close(0u32.into(), b"bye");
            let _ = proxy.send_event(UiEvent::Closed { conn: id, reason, lost });
        });
    }

    {
        let ctrl = ctrl_tx.clone();
        rt.spawn(async move {
            let mut t = tokio::time::interval(Duration::from_millis(1000));
            loop {
                t.tick().await;
                if ctrl.send(ClientMsg::Ping { client_us: rider_core::now_us() }).is_err() {
                    break;
                }
            }
        });
    }

    let (frame_tx, frame_rx) = std::sync::mpsc::channel::<IncomingFrame>();
    video::spawn_decoder(id, frame_rx, ctrl_tx.clone(), shared.clone(), proxy.clone());
    {
        let conn = conn.clone();
        let shared = shared.clone();
        let reading = Arc::new(tokio::sync::Semaphore::new(MAX_FRAMES_READING));
        rt.spawn(async move {
            while let Ok(mut s) = conn.accept_uni().await {
                shared.heard();
                let tx = frame_tx.clone();
                let Ok(permit) = reading.clone().acquire_owned().await else { break };
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Ok(buf) = s.read_to_end(MAX_FRAME_BYTES).await
                        && let Ok((header, payload)) = split_frame(&buf)
                    {
                        let _ = tx.send(IncomingFrame { header, data: payload.to_vec() });
                    }
                });
            }
        });
    }

    if rider_core::tls::speaks_v5(&conn) {
        let (conn, proxy, shared) = (conn.clone(), proxy.clone(), shared.clone());
        rt.spawn(async move {
            while let Ok((s, r)) = conn.accept_bi().await {
                tokio::spawn(receive_file(s, r, proxy.clone(), shared.clone()));
            }
        });
    }

    NetHandle { conn, ctrl: ctrl_tx, rt: rt.clone(), proxy, mouse_seq: Arc::new(AtomicU32::new(0)) }
}

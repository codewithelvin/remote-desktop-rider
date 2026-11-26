use crate::video::{self, IncomingFrame};
use rider_core::clipboard::Clip;
use rider_core::i18n::tr;
use rider_core::pixels::Nv12Frame;
use rider_core::proto::{
    BulkHeader, ClientMsg, CursorShape, FileHeader, HostMsg, ImageHeader, MAX_CHAT, MAX_FRAME_BYTES, MonitorInfo,
    MouseMove, Permissions, encode, read_known, split_frame, split_frame_dirty, write_msg,
};
use rider_core::session_files::SessionFiles;
use rider_core::transfer::{self, Cancel};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU32, AtomicU64, Ordering};
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
    Sharpened,
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
    pub sharp: Mutex<rider_core::sharp::Overlay>,
    sharp_on: AtomicBool,
}

impl Shared {
    pub fn silent_for(&self) -> Duration {
        Duration::from_micros(rider_core::now_us().saturating_sub(self.last_rx_us.load(Ordering::Relaxed)))
    }

    fn pong(&self, client_us: u64, host_us: u64) {
        let rtt = rider_core::now_us().saturating_sub(client_us);
        self.rtt_us.store(rtt, Ordering::Relaxed);
        let best = self.best_rtt_us.load(Ordering::Relaxed);
        if best == 0 || rtt <= best + 2_000 {
            if best == 0 || rtt < best {
                self.best_rtt_us.store(rtt, Ordering::Relaxed);
            }
            let offset = host_us as i64 - (client_us + rtt / 2) as i64;
            self.clock_offset_us.store(offset, Ordering::Relaxed);
        }
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

    pub fn sharp_lost(&self, seq: u64, ctrl: &UnboundedSender<ClientMsg>) {
        self.sharp.lock().unwrap().lost(seq);
        if self.sharp_on.load(Ordering::Relaxed) {
            let _ = ctrl.send(ClientMsg::SharpLost);
        }
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
    header: FileHeader,
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
        header,
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
    *shared.sharp.lock().unwrap() = rider_core::sharp::Overlay::new();
    shared.sharp_on.store(rider_core::tls::speaks_v6(&conn), Ordering::Relaxed);
    let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel::<ClientMsg>();
    rt.spawn(write_control(send, ctrl_rx));

    let stop = Arc::new(AtomicBool::new(false));
    let (clip_local_tx, clip_local_rx) = mpsc::unbounded_channel::<Clip>();
    let (clip_remote_tx, clip_remote_rx) = std::sync::mpsc::channel::<Clip>();
    rider_core::clipboard::spawn(clip_local_tx, clip_remote_rx, stop.clone());
    rt.spawn(forward_clipboard(clip_local_rx, ctrl_tx.clone(), shared.clone(), conn.clone()));

    let reader = HostReader {
        conn: conn.clone(),
        proxy: proxy.clone(),
        shared: shared.clone(),
        clipboard: clip_remote_tx.clone(),
    };
    rt.spawn(reader.run(recv, id, stop));
    rt.spawn(ping(ctrl_tx.clone()));

    let (frame_tx, frame_rx) = std::sync::mpsc::channel::<IncomingFrame>();
    video::spawn_decoder(id, frame_rx, ctrl_tx.clone(), shared.clone(), proxy.clone());
    rt.spawn(read_frames(conn.clone(), shared.clone(), frame_tx));
    if rider_core::tls::speaks_v5(&conn) {
        rt.spawn(accept_streams(conn.clone(), proxy.clone(), shared.clone(), clip_remote_tx));
    }

    NetHandle { conn, ctrl: ctrl_tx, rt: rt.clone(), proxy, mouse_seq: Arc::new(AtomicU32::new(0)) }
}

async fn write_control(mut send: quinn::SendStream, mut ctrl: mpsc::UnboundedReceiver<ClientMsg>) {
    while let Some(m) = ctrl.recv().await {
        if write_msg(&mut send, &m).await.is_err() {
            break;
        }
    }
}

async fn forward_clipboard(
    mut local: mpsc::UnboundedReceiver<Clip>,
    ctrl: UnboundedSender<ClientMsg>,
    shared: Arc<Shared>,
    conn: quinn::Connection,
) {
    let images = rider_core::tls::speaks_v6(&conn);
    while let Some(clip) = local.recv().await {
        if !shared.permissions().clipboard {
            continue;
        }
        match clip {
            Clip::Text(text) => {
                let _ = ctrl.send(ClientMsg::Clipboard { text });
            }
            Clip::Image(image) if images => {
                let conn = conn.clone();
                tokio::spawn(async move {
                    if let Err(e) = transfer::send_image(&conn, &image).await {
                        tracing::warn!("clipboard image not sent: {e:#}");
                    }
                });
            }
            Clip::Image(_) => {}
        }
    }
}

async fn ping(ctrl: UnboundedSender<ClientMsg>) {
    let mut t = tokio::time::interval(Duration::from_millis(1000));
    loop {
        t.tick().await;
        if ctrl.send(ClientMsg::Ping { client_us: rider_core::now_us() }).is_err() {
            break;
        }
    }
}

async fn read_frames(conn: quinn::Connection, shared: Arc<Shared>, frames: std::sync::mpsc::Sender<IncomingFrame>) {
    let reading = Arc::new(tokio::sync::Semaphore::new(MAX_FRAMES_READING));
    let dirty_lists = rider_core::tls::speaks_v6(&conn);
    while let Ok(mut s) = conn.accept_uni().await {
        shared.heard();
        let tx = frames.clone();
        let Ok(permit) = reading.clone().acquire_owned().await else { break };
        tokio::spawn(async move {
            let _permit = permit;
            let Ok(buf) = s.read_to_end(MAX_FRAME_BYTES).await else { return };
            let frame = if dirty_lists {
                split_frame_dirty(&buf).map(|(header, dirty, payload)| IncomingFrame {
                    header,
                    dirty: dirty.to_vec(),
                    data: payload.to_vec(),
                })
            } else {
                split_frame(&buf).map(|(header, payload)| IncomingFrame {
                    header,
                    dirty: Vec::new(),
                    data: payload.to_vec(),
                })
            };
            if let Ok(f) = frame {
                let _ = tx.send(f);
            }
        });
    }
}

async fn accept_streams(
    conn: quinn::Connection,
    proxy: EventLoopProxy<UiEvent>,
    shared: Arc<Shared>,
    clipboard: std::sync::mpsc::Sender<Clip>,
) {
    while let Ok((s, mut r)) = conn.accept_bi().await {
        let (conn, proxy, shared, clipboard) = (conn.clone(), proxy.clone(), shared.clone(), clipboard.clone());
        tokio::spawn(async move {
            match transfer::read_header(&conn, &mut r).await {
                Ok(BulkHeader::File(header)) => receive_file(header, s, r, proxy, shared).await,
                Ok(BulkHeader::ClipboardImage(header)) => receive_image(header, r, &shared, &clipboard).await,
                Ok(BulkHeader::SharpRefresh(header)) => receive_refresh(header, r, &shared, &proxy).await,
                Err(e) => tracing::warn!("incoming stream: {e:#}"),
            }
        });
    }
}

async fn receive_refresh(
    header: rider_core::sharp::RefreshHeader,
    r: quinn::RecvStream,
    shared: &Arc<Shared>,
    proxy: &EventLoopProxy<UiEvent>,
) {
    let tiles = match transfer::receive_refresh(r, header).await {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!("sharp refresh not received: {e:#}");
            return;
        }
    };
    let shared = shared.clone();
    let applied = tokio::task::spawn_blocking(move || {
        let (w, h) = (header.width as usize, header.height as usize);
        let grid = rider_core::sharp::Grid::new(w, h);
        let decoded: Vec<_> = tiles.iter().filter_map(|t| rider_core::sharp::decode_tile(grid, t).ok()).collect();
        let mut overlay = shared.sharp.lock().unwrap();
        decoded.iter().filter(|(i, px)| overlay.insert(header.seq, w, h, *i, px)).count()
    })
    .await
    .unwrap_or(0);
    if applied > 0 {
        let _ = proxy.send_event(UiEvent::Sharpened);
    }
}

async fn receive_image(
    header: ImageHeader,
    mut r: quinn::RecvStream,
    shared: &Shared,
    clipboard: &std::sync::mpsc::Sender<Clip>,
) {
    if !shared.permissions().clipboard {
        let _ = r.stop(0u32.into());
        return;
    }
    match transfer::receive_image(r, header).await {
        Ok(image) => {
            let _ = clipboard.send(Clip::Image(image));
        }
        Err(e) => tracing::warn!("clipboard image not received: {e:#}"),
    }
}

struct HostReader {
    conn: quinn::Connection,
    proxy: EventLoopProxy<UiEvent>,
    shared: Arc<Shared>,
    clipboard: std::sync::mpsc::Sender<Clip>,
}

impl HostReader {
    async fn run(self, mut recv: quinn::RecvStream, id: usize, stop: Arc<AtomicBool>) {
        let (reason, lost) = loop {
            let msg = read_known::<HostMsg>(&mut recv).await;
            self.shared.heard();
            match msg {
                Ok(Some(HostMsg::Bye { reason })) => break (reason, false),
                Ok(Some(m)) => self.handle(m),
                Ok(None) => break ("host closed the session".into(), false),
                Err(e) => {
                    use quinn::ConnectionError as E;
                    let lost =
                        matches!(self.conn.close_reason(), None | Some(E::TimedOut | E::Reset | E::TransportError(_)));
                    break (format!("connection lost: {e}"), lost);
                }
            }
        };
        stop.store(true, Ordering::Relaxed);
        self.conn.close(0u32.into(), b"bye");
        let _ = self.proxy.send_event(UiEvent::Closed { conn: id, reason, lost });
    }

    fn handle(&self, msg: HostMsg) {
        let event = match msg {
            HostMsg::Monitors { list, active } => UiEvent::Monitors(list, active),
            HostMsg::Clipboard { text } => {
                let _ = self.clipboard.send(Clip::Text(text));
                return;
            }
            HostMsg::Permissions(p) => {
                self.shared.set_permissions(p);
                UiEvent::Permissions(p)
            }
            HostMsg::Chat { text } => UiEvent::Chat(text.chars().take(MAX_CHAT).collect()),
            HostMsg::ChatReady(ready) => UiEvent::ChatReady(ready),
            HostMsg::Pong { client_us, host_us } => {
                self.shared.pong(client_us, host_us);
                return;
            }
            HostMsg::Stats { fps, bitrate_kbps, encode_ms, encoder } => {
                *self.shared.host_stats.lock().unwrap() = format!(
                    "host {fps:.0} fps, {:.1} Mbps, enc {encode_ms:.1} ms ({encoder})",
                    bitrate_kbps as f32 / 1000.0
                );
                return;
            }
            HostMsg::Cursor { shape } => UiEvent::Cursor(CursorShape::from_u8(shape)),
            _ => return,
        };
        let _ = self.proxy.send_event(event);
    }
}

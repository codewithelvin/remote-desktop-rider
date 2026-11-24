//! Client side of a live session: control stream, frame streams, mouse
//! datagrams, clipboard, file upload and clock sync.

use crate::video::{self, IncomingFrame};
use rider_core::pixels::Nv12Frame;
use rider_core::proto::*;
use rider_core::trf;
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
    /// Through the rendezvous server's relay rather than a direct path.
    pub relayed: bool,
}

/// Far above the host's in-flight window (at most 16 frames).
const MAX_FRAMES_READING: usize = 64;

#[derive(Debug)]
pub enum UiEvent {
    NewFrame,
    Monitors(Vec<MonitorInfo>, u32),
    /// The host pointer's shape.
    Cursor(CursorShape),
    /// What the person at the host allows (hosts with protocol 4).
    Permissions(Permissions),
    Notice(String),
    /// The session with connection `conn` (its `stable_id`) ended. `lost`:
    /// the network failed (worth reconnecting), not a deliberate end.
    Closed {
        conn: usize,
        reason: String,
        lost: bool,
    },
    /// A new session after a lost connection.
    Reconnected(Session),
    /// Reconnecting failed for good.
    ReconnectFailed(String),
}

/// State shared between the network, decoder and UI threads.
#[derive(Default)]
pub struct Shared {
    /// Most recent decoded picture. The UI always shows the newest one and
    /// silently drops any it did not get to: never a queue.
    pub latest: Mutex<Option<Nv12Frame>>,
    pub rtt_us: AtomicU64,
    /// host_clock - client_clock, in µs (from the lowest-RTT ping sample).
    pub clock_offset_us: AtomicI64,
    best_rtt_us: AtomicU64,
    pub latency_us: AtomicU64,
    pub decode_us: AtomicU64,
    pub frames: AtomicU32,
    pub host_stats: Mutex<String>,
    /// When the host was last heard from (`rider_core::now_us`).
    pub last_rx_us: AtomicU64,
    /// What the host does *not* allow (`Permissions::bits`, inverted so the
    /// default, also for hosts that never say, is everything allowed).
    denied: AtomicU8,
}

impl Shared {
    /// How long the host has been silent.
    pub fn silent_for(&self) -> Duration {
        Duration::from_micros(rider_core::now_us().saturating_sub(self.last_rx_us.load(Ordering::Relaxed)))
    }

    fn heard(&self) {
        self.last_rx_us.store(rider_core::now_us(), Ordering::Relaxed);
    }

    /// What the person at the host currently allows.
    pub fn permissions(&self) -> Permissions {
        Permissions::from_bits(!self.denied.load(Ordering::Relaxed) & 0b111)
    }

    fn set_permissions(&self, p: Permissions) {
        self.denied.store(!p.bits() & 0b111, Ordering::Relaxed);
    }
}

/// What the UI uses to talk to the host.
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

    /// Mouse motion goes out as an unreliable datagram: a lost position is
    /// superseded by the next one anyway.
    pub fn mouse_move(&self, x: u16, y: u16) {
        let seq = self.mouse_seq.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        if let Ok(b) = encode(&MouseMove { seq, x, y }) {
            let _ = self.conn.send_datagram(b.into());
        }
    }

    /// The sequence number of the last mouse move sent.
    pub fn mouse_seq(&self) -> u32 {
        self.mouse_seq.load(Ordering::Relaxed)
    }

    pub fn send_file(&self, path: PathBuf, allowed: bool) {
        let conn = self.conn.clone();
        let proxy = self.proxy.clone();
        self.rt.spawn(async move {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if !allowed {
                let _ = proxy.send_event(UiEvent::Notice(trf!(
                    "{name}: the person at that computer does not allow files",
                    "{name}: o kompüterdəki şəxs fayl göndərməyə icazə vermir"
                )));
                return;
            }
            let _ = proxy.send_event(UiEvent::Notice(format!("sending {name}...")));
            let msg = match upload(&conn, &path).await {
                Ok(r) if r.ok => format!("{name} saved on host: {}", r.message),
                Ok(r) => format!("{name}: {}", r.message),
                Err(e) => format!("{name}: {e:#}"),
            };
            let _ = proxy.send_event(UiEvent::Notice(msg));
        });
    }
}

async fn upload(conn: &quinn::Connection, path: &std::path::Path) -> anyhow::Result<FileResult> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
    let (mut send, mut recv) = conn.open_bi().await?;
    let _ = send.set_priority(-2_000_000_000); // bulk data yields to video and input
    write_msg(&mut send, &FileHeader { name, size }).await?;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        send.write_all(&buf[..n]).await?;
    }
    send.finish()?;
    read_msg::<FileResult>(&mut recv).await?.ok_or_else(|| anyhow::anyhow!("host closed the transfer"))
}

pub fn start(
    rt: &tokio::runtime::Handle,
    s: Session,
    proxy: EventLoopProxy<UiEvent>,
    shared: Arc<Shared>,
) -> NetHandle {
    let Session { conn, send, recv, .. } = s;
    let id = conn.stable_id();
    // A new connection: fresh clock samples, and the host counts as heard.
    shared.best_rtt_us.store(0, Ordering::Relaxed);
    shared.heard();
    let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel::<ClientMsg>();

    // Control writer.
    rt.spawn(async move {
        let mut send = send;
        while let Some(m) = ctrl_rx.recv().await {
            if write_msg(&mut send, &m).await.is_err() {
                break;
            }
        }
    });

    // Clipboard sync.
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

    // Control reader.
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
                    Ok(Some(HostMsg::Pong { client_us, host_us })) => {
                        let now = rider_core::now_us();
                        let rtt = now.saturating_sub(client_us);
                        shared.rtt_us.store(rtt, Ordering::Relaxed);
                        // Only trust offsets from near-minimum-RTT samples: queueing
                        // delay makes the one-way time asymmetric.
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
                        // Timeouts and resets are the network; a close by either
                        // program is deliberate.
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

    // Clock sync / RTT.
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

    // Frames: one uni stream each, read concurrently, decoded in order. A
    // bounded number at once: a misbehaving host could otherwise make this
    // side buffer up to MAX_FRAME_BYTES for each of a thousand open streams.
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
                    // A reset (stale frame abandoned by the host) just errors here;
                    // the decoder notices the gap and asks for a keyframe.
                    if let Ok(buf) = s.read_to_end(MAX_FRAME_BYTES).await
                        && let Ok((header, payload)) = split_frame(&buf)
                    {
                        let _ = tx.send(IncomingFrame { header, data: payload.to_vec() });
                    }
                });
            }
        });
    }

    NetHandle { conn, ctrl: ctrl_tx, rt: rt.clone(), proxy, mouse_seq: Arc::new(AtomicU32::new(0)) }
}

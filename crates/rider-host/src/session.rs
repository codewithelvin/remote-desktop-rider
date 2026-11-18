//! One client connection: pairing (optional) → PIN → live session.

use crate::banner::{self, Banner};
use crate::capture;
use crate::config::HostConfig;
use crate::input::{self, InputEvent};
use crate::video::{self, VideoCmd, VideoConfig, VideoPacket, VideoStats};
use anyhow::{Context, Result, bail};
use rider_core::identity::{Fingerprint, Identity, fp_short};
use rider_core::proto::*;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Frames not fully delivered within this time are abandoned (stream reset),
/// so the link never wastes capacity retransmitting stale pixels.
const FRAME_DEADLINE: Duration = Duration::from_millis(500);
const MIN_BITRATE: u32 = 300_000;
const START_BITRATE: u32 = 4_000_000;
const PAIRING_WINDOW: Duration = Duration::from_secs(600);

pub struct HostState {
    pub config: Mutex<HostConfig>,
    /// Modification time of the loaded config file; used for hot reload.
    config_mtime: Mutex<Option<std::time::SystemTime>>,
    pub identity: Identity,
    pairing_until: Mutex<Option<Instant>>,
    /// Signalled after a successful pairing (`rider-host pair` then exits).
    pub paired: tokio::sync::Notify,
    busy: AtomicBool,
    auth: Mutex<AuthGuard>,
}

#[derive(Default)]
struct AuthGuard {
    failures: u32,
    locked_until: Option<Instant>,
}

impl HostState {
    pub fn new(config: HostConfig, identity: Identity, pairing: bool) -> Self {
        Self {
            config: Mutex::new(config),
            config_mtime: Mutex::new(config_mtime()),
            identity,
            pairing_until: Mutex::new(pairing.then(|| Instant::now() + PAIRING_WINDOW)),
            paired: tokio::sync::Notify::new(),
            busy: AtomicBool::new(false),
            auth: Mutex::new(AuthGuard::default()),
        }
    }

    /// Reload the config if it changed on disk, so `set-pin`, `revoke` and
    /// other management commands apply to a running host immediately.
    pub(crate) fn refresh(&self) {
        let now = config_mtime();
        let mut seen = self.config_mtime.lock().unwrap();
        if now != *seen {
            match HostConfig::load() {
                Ok(c) => {
                    *self.config.lock().unwrap() = c;
                    *seen = now;
                    tracing::info!("configuration reloaded");
                }
                Err(e) => tracing::warn!("ignoring unreadable config change: {e:#}"),
            }
        }
    }

    pub fn pairing_open(&self) -> bool {
        self.pairing_until.lock().unwrap().is_some_and(|t| Instant::now() < t)
    }

    fn close_pairing(&self) {
        *self.pairing_until.lock().unwrap() = None;
    }
}

/// TLS-level gate: only paired clients (or anyone, while pairing is open).
#[derive(Debug)]
pub struct HostTrust(pub Arc<HostState>);

impl std::fmt::Debug for HostState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostState")
    }
}

fn config_mtime() -> Option<std::time::SystemTime> {
    HostConfig::path().ok().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok())
}

impl rider_core::tls::ClientTrust for HostTrust {
    fn is_allowed(&self, fp: &Fingerprint) -> bool {
        self.0.refresh();
        self.0.pairing_open() || self.0.config.lock().unwrap().trusted().contains(fp)
    }
}

pub async fn handle(state: Arc<HostState>, incoming: quinn::Incoming) {
    let remote = incoming.remote_address();
    let conn = match incoming.await {
        Ok(c) => c,
        Err(e) => {
            tracing::info!("{remote}: handshake rejected: {e}");
            return;
        }
    };
    // Wait for the control stream before counting the session: a client
    // racing several network paths closes the losers before opening one, so
    // they never make the host look busy.
    let streams = match tokio::time::timeout(Duration::from_secs(10), conn.accept_bi()).await {
        Ok(Ok(s)) => s,
        _ => {
            tracing::debug!("{remote}: no control stream (path not chosen by the client)");
            return;
        }
    };
    if state.busy.swap(true, Ordering::SeqCst) {
        conn.close(1u32.into(), b"host is busy with another session");
        return;
    }
    // Keeps the service from installing an update during the session.
    crate::autoupdate::mark_session(true);
    let result = run(&state, &conn, streams).await;
    crate::autoupdate::mark_session(false);
    state.busy.store(false, Ordering::SeqCst);
    match result {
        Ok(()) => tracing::info!("{remote}: session ended"),
        // The client closing the connection normally (e.g. after pairing) is not a fault.
        Err(e) if matches!(conn.close_reason(), Some(quinn::ConnectionError::ApplicationClosed(_))) => {
            tracing::info!("{remote}: closed by client ({e:#})")
        }
        Err(e) => tracing::warn!("{remote}: {e:#}"),
    }
    conn.close(0u32.into(), b"bye");
}

async fn run(
    state: &Arc<HostState>,
    conn: &quinn::Connection,
    (mut send, mut recv): (quinn::SendStream, quinn::RecvStream),
) -> Result<()> {
    let client_fp = rider_core::tls::peer_fingerprint(conn)?;
    let _ = send.set_priority(1); // control beats video

    let Some(ClientMsg::Hello { version, device_name, pairing }) = read_msg(&mut recv).await? else {
        bail!("expected Hello");
    };
    if version != PROTOCOL_VERSION {
        bail!("protocol version mismatch (client {version}, host {PROTOCOL_VERSION})");
    }
    let host_name = state.config.lock().unwrap().name.clone();
    write_msg(&mut send, &HostMsg::HelloAck { version: PROTOCOL_VERSION, host_name }).await?;

    let known_name = state.config.lock().unwrap().client_name(&client_fp);
    let name = if pairing {
        pair(state, conn, &mut send, &mut recv, &client_fp, &device_name).await?;
        device_name
    } else {
        match known_name {
            Some(n) => n,
            None => {
                write_msg(&mut send, &HostMsg::Bye { reason: "this device is not paired".into() }).await?;
                bail!("unpaired device {} tried to connect", fp_short(&client_fp));
            }
        }
    };

    authenticate(state, &mut send, &mut recv, &name).await?;
    tracing::info!("session started with {name} ({}) from {}", fp_short(&client_fp), conn.remote_address());
    stream(state, conn, send, recv, name).await
}

async fn pair(
    state: &Arc<HostState>,
    conn: &quinn::Connection,
    send: &mut quinn::SendStream,
    recv: &mut quinn::RecvStream,
    client_fp: &Fingerprint,
    device_name: &str,
) -> Result<()> {
    if !state.pairing_open() {
        write_msg(send, &HostMsg::Bye { reason: "pairing is not enabled on this host".into() }).await?;
        bail!("pairing request while pairing is closed");
    }
    let code = rider_core::sas::compute(conn, &state.identity.fingerprint(), client_fp)?;
    if crate::ui::json() {
        crate::ui::emit(serde_json::json!({
            "event": "request", "device": device_name, "from": conn.remote_address(), "code": code,
        }));
        return finish_pairing(state, send, recv, client_fp, device_name, crate::ui::answer()).await;
    }
    println!();
    println!("  ┌──────────────────────────────────────────────────────┐");
    println!("  │  PAIRING REQUEST from \"{device_name}\"");
    println!("  │  from {}", conn.remote_address());
    println!("  │");
    println!("  │  Verification code:   {code}");
    println!("  │");
    println!("  │  Confirm the client shows the SAME code (by phone or");
    println!("  │  in person). A different code means someone is");
    println!("  │  intercepting the connection - answer N.");
    println!("  └──────────────────────────────────────────────────────┘");
    print!("  Codes match and you trust this device? [y/N] ");
    use std::io::Write;
    let _ = std::io::stdout().flush();

    let operator = async {
        tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            matches!(line.trim(), "y" | "Y" | "yes")
        })
        .await
        .unwrap_or(false)
    };
    finish_pairing(state, send, recv, client_fp, device_name, operator).await
}

/// Wait for both the operator and the client to confirm, then trust the client.
async fn finish_pairing(
    state: &Arc<HostState>,
    send: &mut quinn::SendStream,
    recv: &mut quinn::RecvStream,
    client_fp: &Fingerprint,
    device_name: &str,
    operator: impl std::future::Future<Output = bool>,
) -> Result<()> {
    let client = async {
        match read_msg::<ClientMsg>(recv).await {
            Ok(Some(ClientMsg::PairConfirm { accepted })) => accepted,
            _ => false,
        }
    };
    let (operator_ok, client_ok) = tokio::join!(operator, client);
    let accepted = operator_ok && client_ok;
    write_msg(send, &HostMsg::PairResult { accepted }).await?;
    if !accepted {
        let by = if operator_ok { "the other device" } else { "this computer" };
        crate::ui::emit(serde_json::json!({ "event": "declined", "by": by }));
        bail!("pairing declined");
    }
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.add_trusted(device_name, client_fp);
        cfg.save()?;
    }
    state.close_pairing();
    *state.config_mtime.lock().unwrap() = config_mtime();
    state.paired.notify_one();
    crate::ui::emit(
        serde_json::json!({ "event": "paired", "device": device_name, "fingerprint": rider_core::identity::fp_hex(client_fp) }),
    );
    if crate::ui::json() {
        return Ok(());
    }
    println!("  Paired with \"{device_name}\" ({}). Pairing window closed.", fp_short(client_fp));
    Ok(())
}

async fn authenticate(
    state: &Arc<HostState>,
    send: &mut quinn::SendStream,
    recv: &mut quinn::RecvStream,
    name: &str,
) -> Result<()> {
    for _ in 0..3 {
        let Some(ClientMsg::Auth { pin }) = read_msg(recv).await? else { bail!("expected Auth") };
        let locked = state.auth.lock().unwrap().locked_until.is_some_and(|t| Instant::now() < t);
        if locked {
            write_msg(send, &HostMsg::AuthResult { ok: false, message: "too many wrong PINs; try again later".into() })
                .await?;
            bail!("PIN entry locked");
        }
        // Argon2 is deliberately slow; keep it off the async executor.
        let cfg = state.config.lock().unwrap().clone();
        let ok = tokio::task::spawn_blocking(move || cfg.verify_pin(&pin)).await?;
        if ok {
            state.auth.lock().unwrap().failures = 0;
            write_msg(send, &HostMsg::AuthResult { ok: true, message: String::new() }).await?;
            return Ok(());
        }
        let delay = {
            let mut g = state.auth.lock().unwrap();
            g.failures += 1;
            if g.failures >= 10 {
                g.locked_until = Some(Instant::now() + Duration::from_secs(15 * 60));
            }
            Duration::from_secs(1 << g.failures.min(5))
        };
        tracing::warn!("wrong PIN from {name}");
        tokio::time::sleep(delay).await;
        write_msg(send, &HostMsg::AuthResult { ok: false, message: "wrong PIN".into() }).await?;
    }
    bail!("too many wrong PINs")
}

async fn stream(
    state: &Arc<HostState>,
    conn: &quinn::Connection,
    send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    name: String,
) -> Result<()> {
    let cfg = state.config.lock().unwrap().clone();
    let monitors = capture::monitors()?;
    let list: Vec<MonitorInfo> = monitors.iter().map(|m| m.info.clone()).collect();
    let _banner = Banner::show(format!("●  {name} is viewing this screen  —  click here to disconnect"));

    // Control writer: everything sent to the client goes through this channel.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<HostMsg>();
    let writer = tokio::spawn(async move {
        let mut send = send;
        while let Some(msg) = out_rx.recv().await {
            if write_msg(&mut send, &msg).await.is_err() {
                break;
            }
        }
    });
    let _ = out_tx.send(HostMsg::Monitors { list: list.clone(), active: 0 });

    // Video pipeline.
    let stop = Arc::new(AtomicBool::new(false));
    let inflight = Arc::new(AtomicUsize::new(0));
    let stats = Arc::new(VideoStats::default());
    let expired = Arc::new(AtomicU32::new(0));
    let (vtx, mut vrx) = mpsc::channel::<VideoPacket>(4);
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<VideoCmd>();
    let max_bitrate = cfg.max_bitrate_kbps.max(500) * 1000;
    let video_thread = video::spawn(video::Pipeline {
        monitors: monitors.clone(),
        start_monitor: 0,
        cfg: VideoConfig {
            max_fps: cfg.max_fps,
            start_bitrate_bps: START_BITRATE.min(max_bitrate),
            hardware: cfg.hardware_encoder,
        },
        tx: vtx,
        cmds: cmd_rx,
        inflight: inflight.clone(),
        stats: stats.clone(),
        stop: stop.clone(),
    });

    // Completes when the video thread exits (its channel closes).
    let mut sender = {
        let conn = conn.clone();
        let inflight = inflight.clone();
        let expired = expired.clone();
        tokio::spawn(async move {
            while let Some(p) = vrx.recv().await {
                let conn = conn.clone();
                let inflight = inflight.clone();
                let expired = expired.clone();
                tokio::spawn(async move {
                    if !send_frame(&conn, p).await {
                        expired.fetch_add(1, Ordering::Relaxed);
                    }
                    inflight.fetch_sub(1, Ordering::AcqRel);
                });
            }
        })
    };

    // Input.
    let input = input::spawn(list[0].clone());
    let mouse = {
        let conn = conn.clone();
        let input = input.clone();
        tokio::spawn(async move {
            while let Ok(d) = conn.read_datagram().await {
                if let Ok(m) = decode::<MouseMove>(&d) {
                    input.send(InputEvent::Move { seq: m.seq, x: m.x, y: m.y });
                }
            }
        })
    };

    // Clipboard.
    let (clip_local_tx, mut clip_local_rx) = mpsc::unbounded_channel::<String>();
    let (clip_remote_tx, clip_remote_rx) = std::sync::mpsc::channel::<String>();
    rider_core::clipboard::spawn(clip_local_tx, clip_remote_rx, stop.clone());
    let clip_fwd = {
        let out_tx = out_tx.clone();
        tokio::spawn(async move {
            while let Some(text) = clip_local_rx.recv().await {
                let _ = out_tx.send(HostMsg::Clipboard { text });
            }
        })
    };

    // Incoming files.
    let files = {
        let conn = conn.clone();
        tokio::spawn(async move {
            while let Ok((s, r)) = conn.accept_bi().await {
                tokio::spawn(async move {
                    if let Err(e) = receive_file(s, r).await {
                        tracing::warn!("file transfer failed: {e:#}");
                    }
                });
            }
        })
    };

    // Adaptive bitrate + stats.
    let ticker = {
        let conn = conn.clone();
        let stats = stats.clone();
        let cmd_tx = cmd_tx.clone();
        let out_tx = out_tx.clone();
        let expired = expired.clone();
        tokio::spawn(async move {
            let mut bitrate = START_BITRATE.min(max_bitrate);
            let mut last = conn.stats().path;
            let mut tick = tokio::time::interval(Duration::from_millis(500));
            let mut n = 0u32;
            let (mut fr_acc, mut by_acc, mut enc_acc) = (0u32, 0u64, 0u64);
            loop {
                tick.tick().await;
                n += 1;
                let path = conn.stats().path;
                let frames = stats.frames.swap(0, Ordering::Relaxed);
                let bytes = stats.bytes.swap(0, Ordering::Relaxed);
                let enc_us = stats.encode_us.swap(0, Ordering::Relaxed);
                let skips = stats.congestion_skips.swap(0, Ordering::Relaxed);
                let exp = expired.swap(0, Ordering::Relaxed);
                let sent = path.sent_packets.saturating_sub(last.sent_packets).max(1);
                let lost = path.lost_packets.saturating_sub(last.lost_packets);
                last = path;
                let loss = lost as f64 / sent as f64;
                let queueing = path.rtt > path.min_rtt + (path.min_rtt / 2).max(Duration::from_millis(25));

                let new = if exp > 0 || skips > 2 || queueing || loss > 0.08 {
                    (bitrate as f64 * 0.75) as u32
                } else if frames > 5 && loss < 0.02 {
                    (bitrate as f64 * 1.08) as u32 + 50_000
                } else {
                    bitrate
                }
                .clamp(MIN_BITRATE, max_bitrate);
                if new.abs_diff(bitrate) > bitrate / 20 {
                    bitrate = new;
                    let _ = cmd_tx.send(VideoCmd::SetBitrate(bitrate));
                }

                fr_acc += frames;
                by_acc += bytes;
                enc_acc += enc_us;
                if n.is_multiple_of(2) {
                    let encoder = stats.encoder.lock().unwrap().clone();
                    let _ = out_tx.send(HostMsg::Stats {
                        fps: fr_acc as f32,
                        bitrate_kbps: (by_acc * 8 / 1000) as u32,
                        encode_ms: if fr_acc > 0 { enc_acc as f32 / fr_acc as f32 / 1000.0 } else { 0.0 },
                        encoder,
                    });
                    (fr_acc, by_acc, enc_acc) = (0, 0, 0);
                }
            }
        })
    };

    // Control messages from the client.
    let control = async {
        while let Some(msg) = read_msg::<ClientMsg>(&mut recv).await? {
            match msg {
                ClientMsg::Ping { client_us } => {
                    let _ = out_tx.send(HostMsg::Pong { client_us, host_us: rider_core::now_us() });
                }
                ClientMsg::Key { scancode, extended, down } => input.send(InputEvent::Key { scancode, extended, down }),
                ClientMsg::MouseButton { button, down } => input.send(InputEvent::Button { button, down }),
                ClientMsg::MouseWheel { dx, dy } => input.send(InputEvent::Wheel { dx, dy }),
                ClientMsg::SendSas => {
                    tracing::info!("Ctrl+Alt+Del requested by {name}");
                    if let Err(e) = tokio::task::spawn_blocking(crate::system::send_sas).await? {
                        tracing::warn!("Ctrl+Alt+Del failed: {e:#}");
                    }
                }
                ClientMsg::RequestKeyframe => {
                    let _ = cmd_tx.send(VideoCmd::Keyframe);
                }
                ClientMsg::SelectMonitor { index } => {
                    if let Some(m) = list.get(index as usize) {
                        input.send(InputEvent::Monitor(m.clone()));
                        let _ = cmd_tx.send(VideoCmd::SelectMonitor(index as usize));
                        let _ = out_tx.send(HostMsg::Monitors { list: list.clone(), active: index });
                    }
                }
                ClientMsg::Clipboard { text } => {
                    let _ = clip_remote_tx.send(text);
                }
                ClientMsg::Hello { .. } | ClientMsg::Auth { .. } | ClientMsg::PairConfirm { .. } => {}
            }
        }
        anyhow::Ok(())
    };
    let kicked = async {
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if banner::DISCONNECT_REQUESTED.load(Ordering::SeqCst) {
                return;
            }
        }
    };

    let result = tokio::select! {
        r = control => r,
        e = conn.closed() => { tracing::info!("connection closed: {e}"); Ok(()) }
        _ = &mut sender => {
            let why = stats.failure.lock().unwrap().clone().unwrap_or_else(|| "video stopped".into());
            let _ = out_tx.send(HostMsg::Bye { reason: format!("host video failed: {why}") });
            tokio::time::sleep(Duration::from_millis(200)).await;
            Err(anyhow::anyhow!("video pipeline stopped: {why}"))
        }
        _ = kicked => {
            tracing::info!("disconnected by the local user");
            let _ = out_tx.send(HostMsg::Bye { reason: "the host user ended the session".into() });
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(())
        }
    };

    stop.store(true, Ordering::SeqCst);
    for t in [sender, mouse, clip_fwd, files, ticker, writer] {
        t.abort();
    }
    drop(input); // the input thread releases held keys once the mouse task is gone too
    let _ = tokio::task::spawn_blocking(move || video_thread.join()).await;
    result
}

/// Send one frame on its own stream. Returns false if it missed its deadline.
async fn send_frame(conn: &quinn::Connection, p: VideoPacket) -> bool {
    let Ok(mut s) = conn.open_uni().await else { return false };
    // Older frames first; all video ranks below the control stream.
    let _ = s.set_priority(-((p.header.seq % 1_000_000_000) as i32));
    let Ok(prefix) = frame_prefix(&p.header) else { return false };
    let delivered = tokio::time::timeout(FRAME_DEADLINE, async {
        s.write_all(&prefix).await.ok()?;
        s.write_all(&p.data).await.ok()?;
        s.finish().ok()?;
        s.stopped().await.ok()
    })
    .await;
    match delivered {
        Ok(Some(_)) => true,
        _ => {
            let _ = s.reset(0u32.into());
            false
        }
    }
}

async fn receive_file(mut send: quinn::SendStream, mut recv: quinn::RecvStream) -> Result<()> {
    let header: FileHeader = read_msg(&mut recv).await?.context("missing file header")?;
    let name = std::path::Path::new(&header.name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty() && *n != "." && *n != "..")
        .context("invalid file name")?
        .to_string();
    let dir = crate::system::incoming_files_dir()?;
    tokio::fs::create_dir_all(&dir).await?;
    let mut path = dir.join(&name);
    let mut i = 1;
    while path.exists() {
        let p = std::path::Path::new(&name);
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
        let ext = p.extension().and_then(|s| s.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
        path = dir.join(format!("{stem} ({i}){ext}"));
        i += 1;
    }
    tracing::info!("receiving {} ({} bytes)", path.display(), header.size);
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::File::create(&path).await?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    while let Some(n) = recv.read(&mut buf).await? {
        total += n as u64;
        if total > header.size {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            bail!("sender exceeded announced size");
        }
        file.write_all(&buf[..n]).await?;
    }
    file.flush().await?;
    let ok = total == header.size;
    if !ok {
        drop(file);
        let _ = tokio::fs::remove_file(&path).await;
    }
    let message = if ok { path.display().to_string() } else { "transfer incomplete".into() };
    tracing::info!("file {}: {message}", if ok { "saved" } else { "failed" });
    write_msg(&mut send, &FileResult { ok, message }).await?;
    let _ = send.finish();
    Ok(())
}

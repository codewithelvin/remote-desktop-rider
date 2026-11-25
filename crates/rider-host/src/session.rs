//! One client connection: pairing (optional) → PIN → live session.

use crate::banner::{self, Banner};
use crate::capture;
use crate::config::HostConfig;
use crate::input::{self, InputEvent};
use crate::video::{self, VideoCmd, VideoConfig, VideoPacket, VideoStats};
use anyhow::{Context, Result, bail};
use rider_core::diag::{self, Recorder, Role, Route, Sample, u16_of};
use rider_core::identity::{Fingerprint, Identity, fp_short};
use rider_core::proto::*;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Frames not fully delivered within this time are abandoned (stream reset),
/// so the link never wastes capacity retransmitting stale pixels.
const FRAME_DEADLINE: Duration = Duration::from_millis(500);
const MIN_BITRATE: u32 = 300_000;
const START_BITRATE: u32 = 4_000_000;
const PAIRING_WINDOW: Duration = Duration::from_secs(600);
/// The client's first message must come at once; a PIN is typed by a person.
/// Without limits a connection that never speaks would keep the host busy.
const HELLO_WAIT: Duration = Duration::from_secs(10);
const PIN_WAIT: Duration = Duration::from_secs(120);

pub struct HostState {
    pub config: Mutex<HostConfig>,
    /// Modification time of the loaded config file; used for hot reload.
    config_mtime: Mutex<Option<std::time::SystemTime>>,
    pub identity: Identity,
    pairing_until: Mutex<Option<Instant>>,
    /// One-time support session (quick connect): no PIN, no saved trust, the
    /// person at this computer allows the connection. Config lives in memory.
    pub support: bool,
    /// Support mode: signalled when the one session has ended.
    pub done: tokio::sync::Notify,
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
            support: false,
            done: tokio::sync::Notify::new(),
            paired: tokio::sync::Notify::new(),
            busy: AtomicBool::new(false),
            auth: Mutex::new(AuthGuard::default()),
        }
    }

    /// A one-time support host: its code (the server's pairing code) stays
    /// valid for 10 minutes or until a connection is allowed.
    pub fn support(config: HostConfig, identity: Identity) -> Self {
        Self {
            config: Mutex::new(config),
            config_mtime: Mutex::new(None),
            support: true,
            ..Self::new(HostConfig::default(), identity, true)
        }
    }

    /// Reload the config if it changed on disk, so `set-pin`, `revoke` and
    /// other management commands apply to a running host immediately.
    pub(crate) fn refresh(&self) {
        if self.support {
            return; // in memory only; the machine's host config is not ours
        }
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

    /// The pairing / support window ran out (rather than being closed by use).
    pub fn pairing_expired(&self) -> bool {
        self.pairing_until.lock().unwrap().is_some_and(|t| Instant::now() >= t)
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
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

/// `tunnel`: arrived through the HTTPS tunnel (for diagnostics).
pub async fn handle(state: Arc<HostState>, incoming: quinn::Incoming, tunnel: bool) {
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
    let result = run(&state, &conn, streams, tunnel).await;
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
    tunnel: bool,
) -> Result<()> {
    let client_fp = rider_core::tls::peer_fingerprint(conn)?;
    let _ = send.set_priority(1); // control beats video

    let hello = tokio::time::timeout(HELLO_WAIT, read_msg(&mut recv)).await.context("no Hello in time")??;
    let Some(ClientMsg::Hello { version, device_name, pairing }) = hello else {
        bail!("expected Hello");
    };
    if version != PROTOCOL_VERSION {
        bail!("protocol version mismatch (client {version}, host {PROTOCOL_VERSION})");
    }
    let host_name = state.config.lock().unwrap().name.clone();
    write_msg(&mut send, &HostMsg::HelloAck { version: PROTOCOL_VERSION, host_name }).await?;

    if state.support {
        let access = support_allow(state, conn, &mut send, &client_fp, &device_name).await?;
        tracing::info!("support session started with {device_name} from {} ({})", conn.remote_address(), access.code());
        let result = stream(state, conn, send, recv, device_name, tunnel, access).await;
        crate::ui::emit(serde_json::json!({ "event": "support_ended" }));
        state.done.notify_one();
        return result;
    }

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
    stream(state, conn, send, recv, name, tunnel, Access::Full).await
}

/// One-time support: the person at this computer sees who wants to connect
/// (and the verification code the helper sees) and allows, with an access
/// level, or declines. No PIN.
async fn support_allow(
    state: &Arc<HostState>,
    conn: &quinn::Connection,
    send: &mut quinn::SendStream,
    client_fp: &Fingerprint,
    device_name: &str,
) -> Result<Access> {
    if !state.pairing_open() {
        write_msg(send, &HostMsg::Bye { reason: "this code has expired or was already used".into() }).await?;
        bail!("support request after the code expired");
    }
    let code = rider_core::sas::compute(conn, &state.identity.fingerprint(), client_fp)?;
    crate::ui::emit(serde_json::json!({
        "event": "support_request", "device": device_name, "from": conn.remote_address(), "code": code,
    }));
    let answer = async {
        if crate::ui::json() {
            return crate::ui::answer_access().await;
        }
        println!();
        println!("  \"{device_name}\" ({}) wants to see and control this screen.", conn.remote_address());
        println!("  Verification code: {code} (the helper sees the same code)");
        print!("  Allow? [y = full access, s = standard (no files), v = view only, N = no] ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            match line.trim() {
                "y" | "Y" | "yes" => Some(Access::Full),
                "s" | "S" => Some(Access::Standard),
                "v" | "V" => Some(Access::ViewOnly),
                _ => None,
            }
        })
        .await
        .unwrap_or(None)
    };
    let allowed = tokio::select! {
        a = answer => a,
        _ = conn.closed() => {
            crate::ui::emit(serde_json::json!({ "event": "support_cancelled", "device": device_name }));
            bail!("{device_name} gave up before an answer");
        }
    };
    let Some(access) = allowed else {
        write_msg(send, &HostMsg::AuthResult { ok: false, message: "the person at that computer declined".into() })
            .await?;
        crate::ui::emit(serde_json::json!({ "event": "declined", "by": "this computer" }));
        // Let the answer arrive: the helper closes once it has read it.
        let _ = tokio::time::timeout(Duration::from_secs(3), conn.closed()).await;
        bail!("support request from {device_name} declined");
    };
    // The code is used up: the next registration update withdraws it.
    state.close_pairing();
    write_msg(send, &HostMsg::AuthResult { ok: true, message: String::new() }).await?;
    crate::ui::emit(serde_json::json!({ "event": "support_started", "device": device_name, "access": access.code() }));
    Ok(access)
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
        let msg = tokio::time::timeout(PIN_WAIT, read_msg(recv)).await.context("no PIN in time")??;
        let Some(ClientMsg::Auth { pin }) = msg else { bail!("expected Auth") };
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
    tunnel: bool,
    access: Access,
) -> Result<()> {
    let cfg = state.config.lock().unwrap().clone();
    // What the viewer may do (`Permissions::bits`); checked on every input,
    // clipboard and file, changed live by the person at this computer.
    let perms = Arc::new(AtomicU8::new(access.permissions().bits()));
    let allows = |perms: &AtomicU8| Permissions::from_bits(perms.load(Ordering::Relaxed));
    let v4 = rider_core::tls::speaks_v4(conn);
    let v5 = rider_core::tls::speaks_v5(conn);
    let recorder = Arc::new(Mutex::new(recorder(state, conn, tunnel).await));
    let monitors = capture::monitors()?;
    let list: Vec<MonitorInfo> = monitors.iter().map(|m| m.info.clone()).collect();
    let _banner = Banner::show(rider_core::trf!(
        "●  {name} is viewing this screen  —  click here to disconnect",
        "●  {name} bu ekranı izləyir  —  əlaqəni kəsmək üçün bura klikləyin"
    ));

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
    if v4 {
        let _ = out_tx.send(HostMsg::Permissions(access.permissions()));
    }
    let link = chat_link(state, v5, &name, access).await;
    let chat = crate::chat::Chat::new(conn.clone(), out_tx.clone(), v5, link);
    let chat_task = {
        let perms = perms.clone();
        tokio::spawn(chat.clone().run(move || allows(&perms)))
    };

    // Video pipeline.
    let stop = Arc::new(AtomicBool::new(false));
    let inflight = Arc::new(AtomicUsize::new(0));
    let window = Arc::new(AtomicUsize::new(video::inflight_window(conn.rtt(), cfg.max_fps)));
    let stats = Arc::new(VideoStats::default());
    let expired = Arc::new(AtomicU32::new(0));
    let (vtx, mut vrx) = mpsc::channel::<VideoPacket>(4);
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<VideoCmd>();
    // The client's quality choice: max lines, 0 = automatic.
    let quality = Arc::new(AtomicU32::new(0));
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
        window: window.clone(),
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
        let perms = perms.clone();
        tokio::spawn(async move {
            while let Ok(d) = conn.read_datagram().await {
                if let Ok(m) = decode::<MouseMove>(&d)
                    && allows(&perms).control
                {
                    input.send(InputEvent::Move { seq: m.seq, x: m.x, y: m.y });
                }
            }
        })
    };

    // Access changes by the person at this computer (the app's `access` lines).
    let access_task = {
        let (perms, out_tx, input) = (perms.clone(), out_tx.clone(), input.clone());
        let mut changes = crate::ui::access_changes();
        tokio::spawn(async move {
            while changes.changed().await.is_ok() {
                let Some(access) = *changes.borrow_and_update() else { continue };
                let new = access.permissions();
                let old = Permissions::from_bits(perms.swap(new.bits(), Ordering::Relaxed));
                if old.control && !new.control {
                    input.send(InputEvent::ReleaseAll);
                }
                tracing::info!("access changed to {}", access.code());
                crate::ui::emit(serde_json::json!({ "event": "access", "access": access.code() }));
                if v4 {
                    let _ = out_tx.send(HostMsg::Permissions(new));
                }
            }
        })
    };

    // Pointer shapes, for clients that show them.
    if rider_core::tls::speaks_v3(conn) {
        crate::cursor::spawn(out_tx.clone(), stop.clone());
    }

    // Clipboard.
    let (clip_local_tx, mut clip_local_rx) = mpsc::unbounded_channel::<String>();
    let (clip_remote_tx, clip_remote_rx) = std::sync::mpsc::channel::<String>();
    rider_core::clipboard::spawn(clip_local_tx, clip_remote_rx, stop.clone());
    let clip_fwd = {
        let out_tx = out_tx.clone();
        let perms = perms.clone();
        tokio::spawn(async move {
            while let Some(text) = clip_local_rx.recv().await {
                if allows(&perms).clipboard {
                    let _ = out_tx.send(HostMsg::Clipboard { text });
                }
            }
        })
    };

    // Incoming files: into the chat when someone here sees it, else to
    // Downloads\Rider.
    let files = {
        let (conn, perms, chat) = (conn.clone(), perms.clone(), chat.clone());
        tokio::spawn(async move {
            while let Ok((s, r)) = conn.accept_bi().await {
                if !allows(&perms).files {
                    let refused =
                        rider_core::transfer::refuse(s, r, "the person at that computer does not allow files");
                    if let Ok(size) = refused.await {
                        tracing::info!("refused a file ({size} bytes): not allowed at this access level");
                    }
                } else if chat.ready() {
                    tokio::spawn(chat.clone().receive_file(s, r));
                } else {
                    tokio::spawn(async move {
                        if let Err(e) = receive_file(s, r).await {
                            tracing::warn!("file transfer failed: {e:#}");
                        }
                    });
                }
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
        let (state, recorder, quality) = (state.clone(), recorder.clone(), quality.clone());
        let max_fps = cfg.max_fps;
        tokio::spawn(async move {
            let mut bitrate = START_BITRATE.min(max_bitrate);
            // Highest recent send rate (bps), decaying: bounds the target.
            let mut peak = 0u32;
            // Diagnostics totals over one sample interval.
            let mut d = Sample::default();
            let (mut d_bytes, mut d_frames, mut d_enc, mut d_sent, mut d_lost) = (0u64, 0u32, 0u64, 0u64, 0u64);
            let per_sample = (diag::SAMPLE_EVERY.as_millis() / 500) as u32;
            let mut last = conn.stats().path;
            let mut auto = AutoResolution::default();
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
                d_bytes += bytes;
                d_frames += frames;
                d_enc += enc_us;
                d_sent += sent;
                d_lost += lost;
                d.dropped = d.dropped.saturating_add(u16_of(skips + exp));
                let queueing = path.rtt > path.min_rtt + (path.min_rtt / 2).max(Duration::from_millis(25));
                // Sized from the path's base delay, so queueing does not widen it.
                window.store(video::inflight_window(path.min_rtt, max_fps), Ordering::Relaxed);

                peak = ((peak as f64 * PEAK_DECAY) as u32).max(u32::try_from(bytes * 8 * 2).unwrap_or(u32::MAX));
                let congested = exp > 0 || skips > 2 || queueing || loss > 0.08;
                let new = next_bitrate(bitrate, max_bitrate, peak, congested, frames, loss);
                if new.abs_diff(bitrate) > bitrate / 20 {
                    bitrate = new;
                    let _ = cmd_tx.send(VideoCmd::SetBitrate(bitrate));
                }
                if let Some(h) = auto.update(quality.load(Ordering::Relaxed), bitrate) {
                    let _ = cmd_tx.send(VideoCmd::MaxHeight(h));
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

                if n.is_multiple_of(per_sample)
                    && let Some(r) = recorder.lock().unwrap().as_mut()
                {
                    let secs = diag::SAMPLE_EVERY.as_secs().max(1);
                    r.set_encoder(&stats.encoder.lock().unwrap());
                    let sample = Sample {
                        rtt_ms: u16_of(path.rtt.as_millis()),
                        min_rtt_ms: u16_of(path.min_rtt.as_millis()),
                        loss_permille: u16_of(d_lost * 1000 / d_sent.max(1)),
                        cwnd_kb: u32::try_from(path.cwnd / 1024).unwrap_or(u32::MAX),
                        fps: u16_of(u64::from(d_frames) / secs),
                        kbps: u32::try_from(d_bytes * 8 / 1000 / secs).unwrap_or(u32::MAX),
                        target_kbps: bitrate / 1000,
                        frame_ms: if d_frames > 0 { d_enc as f32 / d_frames as f32 / 1000.0 } else { 0.0 },
                        ..d
                    };
                    if let Some(report) = r.push(sample) {
                        send_report(&state, report, tunnel);
                    }
                    d = Sample::default();
                    (d_bytes, d_frames, d_enc, d_sent, d_lost) = (0, 0, 0, 0, 0);
                }
            }
        })
    };

    // Control messages from the client.
    let control = async {
        while let Some(msg) = read_known::<ClientMsg>(&mut recv).await? {
            let p = allows(&perms);
            let needs_control = matches!(
                msg,
                ClientMsg::Key { .. }
                    | ClientMsg::MouseButton { .. }
                    | ClientMsg::MouseButtonAt { .. }
                    | ClientMsg::MouseWheel { .. }
                    | ClientMsg::SendSas
                    | ClientMsg::LockScreen
            );
            if (needs_control && !p.control) || (matches!(msg, ClientMsg::Clipboard { .. }) && !p.clipboard) {
                continue;
            }
            match msg {
                ClientMsg::Ping { client_us } => {
                    let _ = out_tx.send(HostMsg::Pong { client_us, host_us: rider_core::now_us() });
                }
                ClientMsg::Key { scancode, extended, down } => input.send(InputEvent::Key { scancode, extended, down }),
                ClientMsg::MouseButton { button, down } => input.send(InputEvent::Button { button, down }),
                ClientMsg::MouseButtonAt { button, down, x, y, seq } => {
                    input.send(InputEvent::ButtonAt { button, down, x, y, seq })
                }
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
                ClientMsg::SetQuality { max_height } => {
                    tracing::info!(
                        "quality: {}",
                        if max_height == 0 { "auto".into() } else { format!("{max_height}p") }
                    );
                    quality.store(max_height, Ordering::Relaxed);
                    // Auto starts again from full resolution.
                    let _ = cmd_tx.send(VideoCmd::MaxHeight(max_height));
                }
                ClientMsg::LockScreen => {
                    tracing::info!("locking at the client's request");
                    // SAFETY: no arguments; fails harmlessly if nobody is signed in.
                    if let Err(e) = unsafe { windows::Win32::System::Shutdown::LockWorkStation() } {
                        tracing::warn!("cannot lock: {e}");
                    }
                }
                ClientMsg::Chat { text } => chat.text_from_viewer(&text),
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

    let mut ended = String::new();
    let result = tokio::select! {
        r = control => r,
        e = conn.closed() => { tracing::info!("connection closed: {e}"); ended = e.to_string(); Ok(()) }
        _ = &mut sender => {
            let why = stats.failure.lock().unwrap().clone().unwrap_or_else(|| "video stopped".into());
            let _ = out_tx.send(HostMsg::Bye { reason: format!("host video failed: {why}") });
            tokio::time::sleep(Duration::from_millis(200)).await;
            Err(anyhow::anyhow!("video pipeline stopped: {why}"))
        }
        _ = kicked => {
            tracing::info!("disconnected by the local user");
            ended = "ended by the host user".into();
            let _ = out_tx.send(HostMsg::Bye { reason: "the host user ended the session".into() });
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(())
        }
    };

    stop.store(true, Ordering::SeqCst);
    for t in [sender, mouse, access_task, chat_task, clip_fwd, files, ticker, writer] {
        t.abort();
    }
    chat.end();
    if let Some(r) = recorder.lock().unwrap().take() {
        if ended.is_empty() {
            ended = match &result {
                Ok(()) => "closed by the client".into(),
                Err(e) => format!("{e:#}"),
            };
        }
        send_report(state, r.finish(&ended), tunnel);
    }
    drop(input); // the input thread releases held keys once the mouse task is gone too
    let _ = tokio::task::spawn_blocking(move || video_thread.join()).await;
    result
}

/// Where the person at this computer chats in this session: the app for a
/// one-time session, a chat window for a paired one (viewers that chat only).
async fn chat_link(state: &HostState, v5: bool, device: &str, access: Access) -> crate::chat::Link {
    use crate::chat::Link;
    if state.support {
        return Link::App;
    }
    if !v5 {
        return Link::None;
    }
    let (device, files) = (device.to_owned(), access.permissions().files);
    match tokio::task::spawn_blocking(move || crate::panel::Panel::launch(&device, files)).await {
        Ok(Ok(panel)) => {
            tracing::info!("chat window started");
            Link::panel(panel)
        }
        Ok(Err(e)) => {
            tracing::info!("no chat window: {e:#}");
            Link::None
        }
        Err(e) => {
            tracing::warn!("no chat window: {e}");
            Link::None
        }
    }
}

/// Per 500 ms tick: the recent send-rate peak halves in about 11 s.
const PEAK_DECAY: f64 = 0.97;

/// The adaptive bitrate's next target (bps), every 500 ms.
///
/// Down by a quarter on any congestion sign; otherwise up 8% whenever frames
/// went out cleanly, even a few (a calm screen at 7 fps must recover too: a
/// rule needing 10+ fps left a long session at the minimum for hours). It
/// never runs far ahead of what the picture uses (`peak`, bps): at most 4x,
/// at least `START_BITRATE`, so a burst after a calm stretch does not flood
/// the path.
fn next_bitrate(bitrate: u32, max: u32, peak: u32, congested: bool, frames: u32, loss: f64) -> u32 {
    let ceiling = peak.saturating_mul(4).max(START_BITRATE);
    let new = if congested {
        (bitrate as f64 * 0.75) as u32
    } else if frames > 0 && loss < 0.02 {
        ((bitrate as f64 * 1.08) as u32 + 50_000).min(ceiling.max(bitrate))
    } else {
        bitrate
    };
    // A calm screen lowers the target only gently, towards the ceiling.
    let new = if new > ceiling { new.min(((bitrate as f64 * 0.95) as u32).max(ceiling)) } else { new };
    new.clamp(MIN_BITRATE, max.max(MIN_BITRATE))
}

/// "Auto" quality: lower the resolution when the adaptive bitrate stays low
/// (fewer pixels at the same bitrate look sharper and move smoother), and
/// raise it again once the connection has room. Hysteresis keeps it from
/// flapping: down after a few seconds, up only after a long good stretch.
#[derive(Default)]
struct AutoResolution {
    /// Current automatic cap (0 = full resolution).
    current: u32,
    /// A different cap the bitrate asks for, and since when.
    pending: Option<(u32, Instant)>,
    /// The user's choice at the last update.
    choice: u32,
}

impl AutoResolution {
    const DOWN_AFTER: Duration = Duration::from_secs(4);
    const UP_AFTER: Duration = Duration::from_secs(20);

    /// The cap for this bitrate, ignoring history.
    fn wanted(bitrate: u32) -> u32 {
        match bitrate {
            ..900_000 => 480,
            900_000..2_200_000 => 720,
            _ => 0,
        }
    }

    /// Lines as a comparable size (0 = full = largest).
    fn size(cap: u32) -> u32 {
        if cap == 0 { u32::MAX } else { cap }
    }

    /// Called twice a second; returns a new cap to apply.
    fn update(&mut self, choice: u32, bitrate: u32) -> Option<u32> {
        if choice != self.choice {
            // A fixed choice was made (applied by the control loop), or Auto
            // was picked again and restarts from full resolution.
            (self.choice, self.current, self.pending) = (choice, 0, None);
            return None;
        }
        if choice != 0 {
            return None;
        }
        let want = Self::wanted(bitrate);
        if want == self.current {
            self.pending = None;
            return None;
        }
        let since = match self.pending {
            Some((p, t)) if p == want => t,
            _ => {
                self.pending = Some((want, Instant::now()));
                return None;
            }
        };
        let wait = if Self::size(want) < Self::size(self.current) { Self::DOWN_AFTER } else { Self::UP_AFTER };
        if since.elapsed() < wait {
            return None;
        }
        self.current = want;
        self.pending = None;
        Some(want)
    }
}

/// Diagnostics for this session, when a server is configured to send them to.
async fn recorder(state: &HostState, conn: &quinn::Connection, tunnel: bool) -> Option<Recorder> {
    let server = state.config.lock().unwrap().server.clone()?;
    // Sessions from the server's address came through its relay.
    let server_ip = tokio::time::timeout(Duration::from_secs(3), server.resolve()).await.ok()?.ok()?.ip();
    let route = if tunnel || conn.remote_address().ip() == server_ip { Route::Relay } else { Route::Direct };
    Some(Recorder::new(diag::session_id(conn), Role::Host, route, tunnel))
}

/// Send a diagnostics report in the background.
fn send_report(state: &Arc<HostState>, report: diag::Report, tunnel: bool) {
    let Some(server) = state.config.lock().unwrap().server.clone() else { return };
    let state = state.clone();
    tokio::spawn(async move {
        if tunnel {
            rider_core::rendezvous::mark_tunnel(); // UDP to the server does not get through
        }
        if let Err(e) = rider_core::rendezvous::send_report(&state.identity, &server, report).await {
            tracing::debug!("diagnostics report not sent: {e:#}");
        }
    });
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

/// Create a new file for `name` in `dir` ("name (2).ext" if taken). Runs as
/// the signed-in user (see `system::as_console_user`).
pub(crate) fn create_incoming(dir: &std::path::Path, name: &str) -> Result<(std::path::PathBuf, std::fs::File)> {
    std::fs::create_dir_all(dir)?;
    let p = std::path::Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = p.extension().and_then(|s| s.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
    for i in 1..1000 {
        let path = if i == 1 { dir.join(name) } else { dir.join(format!("{stem} ({i}){ext}")) };
        // create_new: never opens (or truncates) something already there.
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => return Ok((path, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
        }
    }
    bail!("too many files named {name}")
}

/// A file dropped on the viewer while nobody here has the chat open: saved
/// to Downloads\\Rider as the signed-in user.
async fn receive_file(send: quinn::SendStream, recv: quinn::RecvStream) -> Result<()> {
    let dir = crate::system::incoming_files_dir()?;
    let create = move |name: &str| {
        let name = name.to_owned();
        crate::system::as_console_user(|| create_incoming(&dir, &name))?
    };
    let got = rider_core::transfer::receive(send, recv, |_, _| {}, create, &Default::default(), |_| {}).await?;
    tracing::info!("file saved: {} ({} bytes)", got.path.display(), got.size);
    Ok(())
}

#[cfg(test)]
mod auto_resolution_tests {
    use super::*;

    #[test]
    fn bitrate_recovers_at_low_frame_rates() {
        // 7 fps = 3 frames per tick: the old rule (frames > 5) never grew.
        let mut b = MIN_BITRATE;
        for _ in 0..40 {
            b = next_bitrate(b, 20_000_000, 600_000, false, 3, 0.0);
        }
        assert!(b >= START_BITRATE, "back up within 20 s: {b}");
    }

    #[test]
    fn bitrate_follows_use_and_backs_off() {
        // Calm screen: no higher than START_BITRATE.
        let mut b = START_BITRATE;
        for _ in 0..100 {
            b = next_bitrate(b, 20_000_000, 300_000, false, 2, 0.0);
        }
        assert_eq!(b, START_BITRATE);
        // Motion that uses it: room to grow, up to 4x the peak.
        for _ in 0..100 {
            b = next_bitrate(b, 20_000_000, 3_500_000, false, 8, 0.0);
        }
        assert_eq!(b, 14_000_000);
        // Calm again: lowered gently (5% a tick), not dropped at once.
        let calm = next_bitrate(b, 20_000_000, 300_000, false, 2, 0.0);
        assert_eq!(calm, 13_300_000);
        // Congestion: a quarter down, never below the minimum.
        assert_eq!(next_bitrate(b, 20_000_000, 3_500_000, true, 8, 0.0), 10_500_000);
        assert_eq!(next_bitrate(MIN_BITRATE, 20_000_000, 0, true, 0, 0.5), MIN_BITRATE);
        // The configured maximum wins.
        assert_eq!(next_bitrate(9_000_000, 8_000_000, 5_000_000, false, 8, 0.0), 8_000_000);
    }

    #[test]
    fn steps_down_quickly_up_slowly_and_respects_choice() {
        let mut a = AutoResolution::default();
        assert_eq!(a.update(0, 5_000_000), None, "full resolution already");
        assert_eq!(a.update(0, 600_000), None, "not at once");
        a.pending = a.pending.map(|(p, t)| (p, t - AutoResolution::DOWN_AFTER));
        assert_eq!(a.update(0, 600_000), Some(480));
        assert_eq!(a.update(0, 3_000_000), None);
        a.pending = a.pending.map(|(p, t)| (p, t - AutoResolution::DOWN_AFTER));
        assert_eq!(a.update(0, 3_000_000), None, "going up waits longer");
        a.pending = a.pending.map(|(p, t)| (p, t - AutoResolution::UP_AFTER));
        assert_eq!(a.update(0, 3_000_000), Some(0));
        // A fixed choice turns the automatic steps off.
        assert_eq!(a.update(720, 100_000), None);
        assert_eq!(a.update(720, 100_000), None);
        assert!(a.pending.is_none());
    }
}

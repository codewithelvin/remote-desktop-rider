use super::rate::{START_BITRATE, Ticker};
use super::{HostState, OpenPanel, create_incoming};
use crate::banner::{self, Banner};
use crate::capture;
use crate::chat::Chat;
use crate::input::{self, InputEvent, InputHandle};
use crate::video::{self, RefreshJob, SharpSink, VideoCmd, VideoConfig, VideoPacket, VideoStats};
use anyhow::Result;
use rider_core::clipboard::Clip;
use rider_core::diag::{self, Recorder, Role, Route};
use rider_core::proto::{
    Access, BulkHeader, ClientMsg, FileHeader, HostMsg, ImageHeader, MonitorInfo, MouseMove, Permissions, decode,
    frame_prefix, frame_prefix_dirty, read_known, write_msg,
};
use rider_core::transfer;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const FRAME_DEADLINE: Duration = Duration::from_millis(500);
const GOODBYE_GRACE: Duration = Duration::from_millis(200);

pub(super) struct Viewer {
    pub name: String,
    pub tunnel: bool,
    pub access: Access,
    pub panel: Option<OpenPanel>,
}

type Perms = Arc<AtomicU8>;
type Outbox = mpsc::UnboundedSender<HostMsg>;

fn allows(perms: &AtomicU8) -> Permissions {
    Permissions::from_bits(perms.load(Ordering::Relaxed))
}

struct Video {
    thread: std::thread::JoinHandle<()>,
    cmds: std::sync::mpsc::Sender<VideoCmd>,
    sender: JoinHandle<()>,
    stats: Arc<VideoStats>,
    window: Arc<AtomicUsize>,
    expired: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
}

struct Control<'a> {
    name: &'a str,
    perms: &'a AtomicU8,
    out: &'a Outbox,
    input: &'a InputHandle,
    video: &'a std::sync::mpsc::Sender<VideoCmd>,
    clipboard: &'a std::sync::mpsc::Sender<Clip>,
    quality: &'a AtomicU32,
    monitors: &'a [MonitorInfo],
    chat: &'a Chat,
}

pub(super) async fn stream(
    state: &Arc<HostState>,
    conn: &quinn::Connection,
    send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    viewer: Viewer,
) -> Result<()> {
    let Viewer { name, tunnel, access, panel } = viewer;
    let cfg = state.config.lock().unwrap().clone();
    let perms: Perms = Arc::new(AtomicU8::new(access.permissions().bits()));
    let v4 = rider_core::tls::speaks_v4(conn);
    let v5 = rider_core::tls::speaks_v5(conn);
    let recorder = Arc::new(Mutex::new(recorder(state, conn, tunnel).await));
    let monitors = capture::monitors()?;
    let list: Vec<MonitorInfo> = monitors.iter().map(|m| m.info.clone()).collect();
    let _banner = Banner::show(&rider_core::trf!(
        "●  {name} is viewing this screen  —  click here to disconnect",
        "●  {name} bu ekranı izləyir  —  əlaqəni kəsmək üçün bura klikləyin"
    ));

    let (out_tx, out_rx) = mpsc::unbounded_channel::<HostMsg>();
    let writer = spawn_writer(send, out_rx);
    let _ = out_tx.send(HostMsg::Monitors { list: list.clone(), active: 0 });
    if v4 {
        let _ = out_tx.send(HostMsg::Permissions(access.permissions()));
    }
    let link = match panel {
        Some(p) if v5 => crate::chat::Link::panel(p),
        _ => chat_link(state, v5, &name, access).await,
    };
    let chat = Chat::new(conn.clone(), out_tx.clone(), v5, link);
    let chat_task = {
        let perms = perms.clone();
        tokio::spawn(chat.clone().run(move || allows(&perms)))
    };

    let max_bitrate = cfg.max_bitrate_kbps.max(500) * 1000;
    let mut video = start_video(conn, monitors, &cfg, max_bitrate);
    let input = input::spawn(list[0].clone());
    let mouse = spawn_mouse(conn.clone(), input.clone(), perms.clone());
    let access_task = spawn_access_watch(perms.clone(), out_tx.clone(), input.clone(), v4);
    if rider_core::tls::speaks_v3(conn) {
        crate::cursor::spawn(out_tx.clone(), video.stop.clone());
    }
    let (clip_local_tx, clip_local_rx) = mpsc::unbounded_channel::<Clip>();
    let (clip_remote_tx, clip_remote_rx) = std::sync::mpsc::channel::<Clip>();
    rider_core::clipboard::spawn(clip_local_tx, clip_remote_rx, video.stop.clone());
    let clip_fwd = spawn_clipboard_forward(clip_local_rx, out_tx.clone(), perms.clone(), conn.clone());
    let files = spawn_bulk_receiver(conn.clone(), perms.clone(), chat.clone(), clip_remote_tx.clone());
    let quality = Arc::new(AtomicU32::new(0));
    let ticker = tokio::spawn(
        Ticker {
            conn: conn.clone(),
            stats: video.stats.clone(),
            cmd_tx: video.cmds.clone(),
            out_tx: out_tx.clone(),
            expired: video.expired.clone(),
            window: video.window.clone(),
            quality: quality.clone(),
            recorder: recorder.clone(),
            state: state.clone(),
            tunnel,
            max_fps: cfg.max_fps,
            max_bitrate,
        }
        .run(),
    );

    let control = Control {
        name: &name,
        perms: &perms,
        out: &out_tx,
        input: &input,
        video: &video.cmds,
        clipboard: &clip_remote_tx,
        quality: &quality,
        monitors: &list,
        chat: &chat,
    };
    let mut ended = String::new();
    let result = tokio::select! {
        r = control.run(&mut recv) => r,
        e = conn.closed() => { tracing::info!("connection closed: {e}"); ended = e.to_string(); Ok(()) }
        _ = &mut video.sender => {
            let why = video.stats.failure.lock().unwrap().clone().unwrap_or_else(|| "video stopped".into());
            say_goodbye(&out_tx, format!("host video failed: {why}")).await;
            Err(anyhow::anyhow!("video pipeline stopped: {why}"))
        }
        () = disconnect_requested() => {
            tracing::info!("disconnected by the local user");
            ended = "ended by the host user".into();
            say_goodbye(&out_tx, "the host user ended the session".into()).await;
            Ok(())
        }
    };

    video.stop.store(true, Ordering::SeqCst);
    for t in [video.sender, mouse, access_task, chat_task, clip_fwd, files, ticker, writer] {
        t.abort();
    }
    chat.end();
    report_end(state, &recorder, ended, &result, tunnel);
    drop(input);
    let thread = video.thread;
    let _ = tokio::task::spawn_blocking(move || thread.join()).await;
    result
}

fn report_end(
    state: &Arc<HostState>,
    recorder: &Mutex<Option<Recorder>>,
    ended: String,
    result: &Result<()>,
    tunnel: bool,
) {
    let Some(r) = recorder.lock().unwrap().take() else { return };
    let ended = match result {
        _ if !ended.is_empty() => ended,
        Ok(()) => "closed by the client".into(),
        Err(e) => format!("{e:#}"),
    };
    send_report(state, r.finish(&ended), tunnel);
}

async fn say_goodbye(out: &Outbox, reason: String) {
    let _ = out.send(HostMsg::Bye { reason });
    tokio::time::sleep(GOODBYE_GRACE).await;
}

async fn disconnect_requested() {
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if banner::DISCONNECT_REQUESTED.load(Ordering::SeqCst) {
            return;
        }
    }
}

fn spawn_writer(mut send: quinn::SendStream, mut out: mpsc::UnboundedReceiver<HostMsg>) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(msg) = out.recv().await {
            if write_msg(&mut send, &msg).await.is_err() {
                break;
            }
        }
    })
}

fn start_video(
    conn: &quinn::Connection,
    monitors: Vec<capture::Monitor>,
    cfg: &crate::config::HostConfig,
    max_bitrate: u32,
) -> Video {
    let stop = Arc::new(AtomicBool::new(false));
    let inflight = Arc::new(AtomicUsize::new(0));
    let window = Arc::new(AtomicUsize::new(video::inflight_window(conn.rtt(), cfg.max_fps)));
    let stats = Arc::new(VideoStats::default());
    let expired = Arc::new(AtomicU32::new(0));
    let (vtx, vrx) = mpsc::channel::<VideoPacket>(4);
    let (cmds, cmd_rx) = std::sync::mpsc::channel::<VideoCmd>();
    let sharp = rider_core::tls::speaks_v6(conn).then(|| spawn_refresher(conn.clone(), cmds.clone()));
    let thread = video::spawn(video::Pipeline {
        monitors,
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
        sharp,
    });
    let sender = spawn_frame_sender(conn.clone(), vrx, inflight, expired.clone());
    Video { thread, cmds, sender, stats, window, expired, stop }
}

fn spawn_refresher(conn: quinn::Connection, cmds: std::sync::mpsc::Sender<VideoCmd>) -> SharpSink {
    let (jobs, mut rx) = mpsc::unbounded_channel::<RefreshJob>();
    let busy = Arc::new(AtomicBool::new(false));
    let flag = busy.clone();
    tokio::spawn(async move {
        while let Some(job) = rx.recv().await {
            let count = job.tiles.len();
            let sent = async {
                let RefreshJob { header, grid, tiles } = job;
                let pngs = tokio::task::spawn_blocking(move || {
                    tiles
                        .iter()
                        .map(|(i, rgb)| rider_core::sharp::encode_tile(grid, *i, rgb))
                        .collect::<Result<Vec<_>>>()
                })
                .await??;
                transfer::send_refresh(&conn, header, &pngs).await
            }
            .await;
            match sent {
                Ok(()) => tracing::debug!("sharp refresh: {count} tiles"),
                Err(e) => {
                    tracing::debug!("sharp refresh not delivered: {e:#}");
                    let _ = cmds.send(VideoCmd::SharpLost);
                }
            }
            flag.store(false, Ordering::Release);
        }
    });
    SharpSink { jobs, busy }
}

fn spawn_frame_sender(
    conn: quinn::Connection,
    mut frames: mpsc::Receiver<VideoPacket>,
    inflight: Arc<AtomicUsize>,
    expired: Arc<AtomicU32>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(p) = frames.recv().await {
            let (conn, inflight, expired) = (conn.clone(), inflight.clone(), expired.clone());
            tokio::spawn(async move {
                if !send_frame(&conn, p).await {
                    expired.fetch_add(1, Ordering::Relaxed);
                }
                inflight.fetch_sub(1, Ordering::AcqRel);
            });
        }
    })
}

fn spawn_mouse(conn: quinn::Connection, input: InputHandle, perms: Perms) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Ok(d) = conn.read_datagram().await {
            if let Ok(m) = decode::<MouseMove>(&d)
                && allows(&perms).control
            {
                input.send(InputEvent::Move { seq: m.seq, x: m.x, y: m.y });
            }
        }
    })
}

fn spawn_access_watch(perms: Perms, out: Outbox, input: InputHandle, v4: bool) -> JoinHandle<()> {
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
            crate::ui::emit(&serde_json::json!({ "event": "access", "access": access.code() }));
            if v4 {
                let _ = out.send(HostMsg::Permissions(new));
            }
        }
    })
}

fn spawn_clipboard_forward(
    mut local: mpsc::UnboundedReceiver<Clip>,
    out: Outbox,
    perms: Perms,
    conn: quinn::Connection,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let images = rider_core::tls::speaks_v6(&conn);
        while let Some(clip) = local.recv().await {
            if !allows(&perms).clipboard {
                continue;
            }
            match clip {
                Clip::Text(text) => {
                    let _ = out.send(HostMsg::Clipboard { text });
                }
                Clip::Image(image) if images => {
                    let conn = conn.clone();
                    tokio::spawn(async move {
                        match transfer::send_image(&conn, &image).await {
                            Ok(()) => tracing::info!("clipboard image sent ({} bytes)", image.png.len()),
                            Err(e) => tracing::warn!("clipboard image not sent: {e:#}"),
                        }
                    });
                }
                Clip::Image(_) => {}
            }
        }
    })
}

fn spawn_bulk_receiver(
    conn: quinn::Connection,
    perms: Perms,
    chat: Arc<Chat>,
    clipboard: std::sync::mpsc::Sender<Clip>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Ok((s, mut r)) = conn.accept_bi().await {
            let (conn, perms, chat, clipboard) = (conn.clone(), perms.clone(), chat.clone(), clipboard.clone());
            tokio::spawn(async move {
                match transfer::read_header(&conn, &mut r).await {
                    Ok(BulkHeader::File(header)) => incoming_file(header, s, r, &perms, chat).await,
                    Ok(BulkHeader::ClipboardImage(header)) => incoming_image(header, r, &perms, &clipboard).await,
                    Ok(BulkHeader::SharpRefresh(_)) => {
                        let _ = r.stop(0u32.into());
                    }
                    Err(e) => tracing::warn!("incoming stream: {e:#}"),
                }
            });
        }
    })
}

async fn incoming_file(
    header: FileHeader,
    s: quinn::SendStream,
    r: quinn::RecvStream,
    perms: &AtomicU8,
    chat: Arc<Chat>,
) {
    if !allows(perms).files {
        if transfer::refuse(s, r, "the person at that computer does not allow files").await.is_ok() {
            tracing::info!("refused a file ({} bytes): not allowed at this access level", header.size);
        }
    } else if chat.ready() {
        chat.receive_file(header, s, r).await;
    } else if let Err(e) = receive_file(header, s, r).await {
        tracing::warn!("file transfer failed: {e:#}");
    }
}

async fn incoming_image(
    header: ImageHeader,
    mut r: quinn::RecvStream,
    perms: &AtomicU8,
    clipboard: &std::sync::mpsc::Sender<Clip>,
) {
    if !allows(perms).clipboard {
        let _ = r.stop(0u32.into());
        tracing::info!("refused a clipboard image: not allowed at this access level");
        return;
    }
    match transfer::receive_image(r, header).await {
        Ok(image) => {
            tracing::info!("clipboard image received ({}x{})", image.width, image.height);
            let _ = clipboard.send(Clip::Image(image));
        }
        Err(e) => tracing::warn!("clipboard image not received: {e:#}"),
    }
}

impl Control<'_> {
    async fn run(&self, recv: &mut quinn::RecvStream) -> Result<()> {
        while let Some(msg) = read_known::<ClientMsg>(recv).await? {
            if self.permitted(&msg) {
                self.handle(msg).await?;
            }
        }
        Ok(())
    }

    fn permitted(&self, msg: &ClientMsg) -> bool {
        let p = allows(self.perms);
        let needs_control = matches!(
            msg,
            ClientMsg::Key { .. }
                | ClientMsg::MouseButton { .. }
                | ClientMsg::MouseButtonAt { .. }
                | ClientMsg::MouseWheel { .. }
                | ClientMsg::SendSas
                | ClientMsg::LockScreen
        );
        let clipboard = matches!(msg, ClientMsg::Clipboard { .. });
        (!needs_control || p.control) && (!clipboard || p.clipboard)
    }

    async fn handle(&self, msg: ClientMsg) -> Result<()> {
        match msg {
            ClientMsg::Ping { client_us } => {
                let _ = self.out.send(HostMsg::Pong { client_us, host_us: rider_core::now_us() });
            }
            ClientMsg::Key { scancode, extended, down } => {
                self.input.send(InputEvent::Key { scancode, extended, down });
            }
            ClientMsg::MouseButton { button, down } => self.input.send(InputEvent::Button { button, down }),
            ClientMsg::MouseButtonAt { button, down, x, y, seq } => {
                self.input.send(InputEvent::ButtonAt { button, down, x, y, seq });
            }
            ClientMsg::MouseWheel { dx, dy } => self.input.send(InputEvent::Wheel { dx, dy }),
            ClientMsg::SendSas => {
                tracing::info!("Ctrl+Alt+Del requested by {}", self.name);
                if let Err(e) = tokio::task::spawn_blocking(crate::system::send_sas).await? {
                    tracing::warn!("Ctrl+Alt+Del failed: {e:#}");
                }
            }
            ClientMsg::RequestKeyframe => {
                let _ = self.video.send(VideoCmd::Keyframe);
            }
            ClientMsg::SelectMonitor { index } => self.select_monitor(index),
            ClientMsg::Clipboard { text } => {
                let _ = self.clipboard.send(Clip::Text(text));
            }
            ClientMsg::SetQuality { max_height } => self.set_quality(max_height),
            ClientMsg::LockScreen => lock_screen(),
            ClientMsg::Chat { text } => self.chat.text_from_viewer(&text),
            ClientMsg::SharpLost => {
                let _ = self.video.send(VideoCmd::SharpLost);
            }
            ClientMsg::Hello { .. } | ClientMsg::Auth { .. } | ClientMsg::PairConfirm { .. } => {}
        }
        Ok(())
    }

    fn select_monitor(&self, index: u32) {
        if let Some(m) = self.monitors.get(index as usize) {
            self.input.send(InputEvent::Monitor(m.clone()));
            let _ = self.video.send(VideoCmd::SelectMonitor(index as usize));
            let _ = self.out.send(HostMsg::Monitors { list: self.monitors.to_vec(), active: index });
        }
    }

    fn set_quality(&self, max_height: u32) {
        tracing::info!("quality: {}", if max_height == 0 { "auto".into() } else { format!("{max_height}p") });
        self.quality.store(max_height, Ordering::Relaxed);
        let _ = self.video.send(VideoCmd::MaxHeight(max_height));
    }
}

fn lock_screen() {
    tracing::info!("locking at the client's request");
    if let Err(e) = unsafe { windows::Win32::System::Shutdown::LockWorkStation() } {
        tracing::warn!("cannot lock: {e}");
    }
}

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

async fn recorder(state: &HostState, conn: &quinn::Connection, tunnel: bool) -> Option<Recorder> {
    let server = state.config.lock().unwrap().server.clone()?;
    let server_ip = tokio::time::timeout(Duration::from_secs(3), server.resolve()).await.ok()?.ok()?.ip();
    let route = if tunnel || conn.remote_address().ip() == server_ip { Route::Relay } else { Route::Direct };
    Some(Recorder::new(diag::session_id(conn), Role::Host, route, tunnel))
}

pub(super) fn send_report(state: &Arc<HostState>, report: diag::Report, tunnel: bool) {
    let Some(server) = state.config.lock().unwrap().server.clone() else { return };
    let state = state.clone();
    tokio::spawn(async move {
        if tunnel {
            rider_core::rendezvous::mark_tunnel();
        }
        if let Err(e) = rider_core::rendezvous::send_report(&state.identity, &server, report).await {
            tracing::debug!("diagnostics report not sent: {e:#}");
        }
    });
}

async fn send_frame(conn: &quinn::Connection, p: VideoPacket) -> bool {
    let Ok(mut s) = conn.open_uni().await else { return false };
    let _ = s.set_priority(-((p.header.seq % 1_000_000_000) as i32));
    let prefix = if rider_core::tls::speaks_v6(conn) {
        frame_prefix_dirty(&p.header, &p.dirty)
    } else {
        frame_prefix(&p.header)
    };
    let Ok(prefix) = prefix else { return false };
    let delivered = tokio::time::timeout(FRAME_DEADLINE, async {
        s.write_all(&prefix).await.ok()?;
        s.write_all(&p.data).await.ok()?;
        s.finish().ok()?;
        s.stopped().await.ok()
    })
    .await;
    if let Ok(Some(_)) = delivered {
        true
    } else {
        let _ = s.reset(0u32.into());
        false
    }
}

async fn receive_file(header: FileHeader, send: quinn::SendStream, recv: quinn::RecvStream) -> Result<()> {
    let dir = crate::system::incoming_files_dir()?;
    let create = move |name: &str| {
        let name = name.to_owned();
        crate::system::as_console_user(|| create_incoming(&dir, &name))?
    };
    let got = transfer::receive(header, send, recv, |_, _| {}, create, &Arc::default(), |_| {}).await?;
    tracing::info!("file saved: {} ({} bytes)", got.path.display(), got.size);
    Ok(())
}

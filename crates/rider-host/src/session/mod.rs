mod forward;
mod rate;
mod stream;

use crate::config::HostConfig;
use anyhow::{Context, Result, bail};
use rider_core::identity::{Fingerprint, Identity, fp_short};
use rider_core::proto::{Access, ClientMsg, HostMsg, PROTOCOL_VERSION, read_msg, write_msg};
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use stream::{Viewer, stream};
use tokio::sync::mpsc;

const NOBODY: &str = "nobody is signed in at that computer to accept";
const PAIRING_WINDOW: Duration = Duration::from_secs(600);
const ACCEPT_WAIT: Duration = Duration::from_secs(60);
const HELLO_WAIT: Duration = Duration::from_secs(10);
const PIN_WAIT: Duration = Duration::from_secs(120);

pub struct HostState {
    pub config: Mutex<HostConfig>,
    config_mtime: Mutex<Option<std::time::SystemTime>>,
    pub identity: Identity,
    pairing_until: Mutex<Option<Instant>>,
    pub support: bool,
    pub done: tokio::sync::Notify,
    pub paired: tokio::sync::Notify,
    busy: AtomicBool,
    auth: Mutex<AuthGuard>,
    pub company_unattended: AtomicBool,
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
            company_unattended: AtomicBool::new(false),
        }
    }

    pub fn support(config: HostConfig, identity: Identity) -> Self {
        Self {
            config: Mutex::new(config),
            config_mtime: Mutex::new(None),
            support: true,
            ..Self::new(HostConfig::default(), identity, true)
        }
    }

    pub(crate) fn refresh(&self) {
        if self.support {
            return;
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

    pub fn pairing_expired(&self) -> bool {
        self.pairing_until.lock().unwrap().is_some_and(|t| Instant::now() >= t)
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }
}

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

pub async fn handle(state: Arc<HostState>, incoming: quinn::Incoming, tunnel: bool) {
    let remote = incoming.remote_address();
    let conn = match incoming.await {
        Ok(c) => c,
        Err(e) => {
            tracing::info!("{remote}: handshake rejected: {e}");
            return;
        }
    };
    let Ok(Ok(streams)) = tokio::time::timeout(Duration::from_secs(10), conn.accept_bi()).await else {
        tracing::debug!("{remote}: no control stream (path not chosen by the client)");
        return;
    };
    if state.busy.swap(true, Ordering::SeqCst) {
        conn.close(1u32.into(), b"host is busy with another session");
        return;
    }
    crate::autoupdate::mark_session(true);
    let result = run(&state, &conn, streams, tunnel).await;
    crate::autoupdate::mark_session(false);
    state.busy.store(false, Ordering::SeqCst);
    match result {
        Ok(()) => tracing::info!("{remote}: session ended"),
        Err(e) if matches!(conn.close_reason(), Some(quinn::ConnectionError::ApplicationClosed(_))) => {
            tracing::info!("{remote}: closed by client ({e:#})");
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
    let _ = send.set_priority(1);

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
        let result = stream(state, conn, send, recv, Viewer { name: device_name, tunnel, access, panel: None }).await;
        crate::ui::emit(&serde_json::json!({ "event": "support_ended" }));
        state.done.notify_one();
        return result;
    }

    let (known_name, admin) = {
        let cfg = state.config.lock().unwrap();
        (cfg.client_name(&client_fp), cfg.company_admin())
    };
    if !pairing && known_name.is_none() && admin == Some(client_fp) {
        let panel = company_allow(state, conn, &mut send, &device_name).await?;
        tracing::info!("company session started with {device_name} from {}", conn.remote_address());
        return stream(state, conn, send, recv, Viewer { name: device_name, tunnel, access: Access::Full, panel })
            .await;
    }
    let name = if pairing {
        pair(state, conn, &mut send, &mut recv, &client_fp, &device_name).await?;
        device_name
    } else if let Some(n) = known_name {
        n
    } else {
        write_msg(&mut send, &HostMsg::Bye { reason: "this device is not paired".into() }).await?;
        bail!("unpaired device {} tried to connect", fp_short(&client_fp));
    };

    authenticate(state, &mut send, &mut recv, &name).await?;
    tracing::info!("session started with {name} ({}) from {}", fp_short(&client_fp), conn.remote_address());
    stream(state, conn, send, recv, Viewer { name, tunnel, access: Access::Full, panel: None }).await
}

type OpenPanel = (crate::panel::Panel, mpsc::UnboundedReceiver<crate::ui::ChatCmd>);

async fn company_allow(
    state: &Arc<HostState>,
    conn: &quinn::Connection,
    send: &mut quinn::SendStream,
    device_name: &str,
) -> Result<Option<OpenPanel>> {
    let (company, unattended) = {
        let cfg = state.config.lock().unwrap();
        let c = cfg.company.clone().unwrap_or_default();
        (c.name, c.unattended && state.company_unattended.load(Ordering::Relaxed))
    };
    let refuse = async |send: &mut quinn::SendStream, why: &str| -> Result<()> {
        write_msg(send, &HostMsg::AuthResult { ok: false, message: why.into() }).await?;
        let _ = tokio::time::timeout(Duration::from_secs(3), conn.closed()).await;
        bail!("company admin {device_name}: {why}")
    };
    if unattended {
        tracing::info!("company admin {device_name}: unattended, nobody asked");
        write_msg(send, &HostMsg::AuthResult { ok: true, message: String::new() }).await?;
        return Ok(None);
    }
    match crate::company::console_user() {
        (None, _) => return refuse(send, NOBODY).await.map(|()| None),
        (Some(_), true) => {
            return refuse(send, "that computer is locked: the person there is away").await.map(|()| None);
        }
        (Some(_), false) => {}
    }
    let (device, files) = (device_name.to_owned(), Access::Full.permissions().files);
    let launched = tokio::task::spawn_blocking(move || crate::panel::Panel::launch(&device, files)).await?;
    let (panel, mut answers) = match launched {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("no window to ask in: {e:#}");
            return refuse(send, NOBODY).await.map(|()| None);
        }
    };
    panel.ask(&company, ACCEPT_WAIT.as_secs());
    let answer = async {
        while let Some(cmd) = answers.recv().await {
            if let crate::ui::ChatCmd::Answer(yes) = cmd {
                return Some(yes);
            }
        }
        None
    };
    let accepted = tokio::select! {
        a = tokio::time::timeout(ACCEPT_WAIT, answer) => a.ok().flatten(),
        _ = conn.closed() => bail!("{device_name} gave up before an answer"),
    };
    match accepted {
        Some(true) => {
            write_msg(send, &HostMsg::AuthResult { ok: true, message: String::new() }).await?;
            Ok(Some((panel, answers)))
        }
        Some(false) => refuse(send, "the person at that computer declined").await.map(|()| None),
        None => refuse(send, "the person at that computer did not answer").await.map(|()| None),
    }
}

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
    crate::ui::emit(&serde_json::json!({
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
            crate::ui::emit(&serde_json::json!({ "event": "support_cancelled", "device": device_name }));
            bail!("{device_name} gave up before an answer");
        }
    };
    let Some(access) = allowed else {
        write_msg(send, &HostMsg::AuthResult { ok: false, message: "the person at that computer declined".into() })
            .await?;
        crate::ui::emit(&serde_json::json!({ "event": "declined", "by": "this computer" }));
        let _ = tokio::time::timeout(Duration::from_secs(3), conn.closed()).await;
        bail!("support request from {device_name} declined");
    };
    state.close_pairing();
    write_msg(send, &HostMsg::AuthResult { ok: true, message: String::new() }).await?;
    crate::ui::emit(&serde_json::json!({ "event": "support_started", "device": device_name, "access": access.code() }));
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
        crate::ui::emit(&serde_json::json!({
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

async fn finish_pairing(
    state: &Arc<HostState>,
    send: &mut quinn::SendStream,
    recv: &mut quinn::RecvStream,
    client_fp: &Fingerprint,
    device_name: &str,
    operator: impl Future<Output = bool>,
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
        crate::ui::emit(&serde_json::json!({ "event": "declined", "by": by }));
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
        &serde_json::json!({ "event": "paired", "device": device_name, "fingerprint": rider_core::identity::fp_hex(client_fp) }),
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
                g.locked_until = Some(Instant::now() + Duration::from_mins(15));
            }
            Duration::from_secs(1 << g.failures.min(5))
        };
        tracing::warn!("wrong PIN from {name}");
        tokio::time::sleep(delay).await;
        write_msg(send, &HostMsg::AuthResult { ok: false, message: "wrong PIN".into() }).await?;
    }
    bail!("too many wrong PINs")
}

pub(crate) fn create_incoming(dir: &std::path::Path, name: &str) -> Result<(std::path::PathBuf, std::fs::File)> {
    std::fs::create_dir_all(dir)?;
    let p = std::path::Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = p.extension().and_then(|s| s.to_str()).map_or_default(|e| format!(".{e}"));
    for i in 1..1000 {
        let path = if i == 1 { dir.join(name) } else { dir.join(format!("{stem} ({i}){ext}")) };
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => return Ok((path, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
        }
    }
    bail!("too many files named {name}")
}

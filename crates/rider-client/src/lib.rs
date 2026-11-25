//! Remote Desktop Rider client library: pairing, connecting (direct or via
//! the rendezvous server), host status, and the viewer window. Used by the
//! `rider-client` CLI and the desktop app.

pub mod app;
mod badge;
mod chat;
pub mod config;
pub mod diag;
#[cfg(windows)]
mod keyhook;
#[cfg(any(not(windows), test))]
mod keymap;
pub mod net;
mod overlay;
mod toolbar;
mod video;

use anyhow::{Context, Result, bail};
use config::{ClientConfig, KnownHost};
use rider_core::i18n::tr;
use rider_core::identity::{Fingerprint, Identity, fp_from_hex, fp_hex};
use rider_core::proto::*;
use rider_core::rendezvous::{self, Path, PeerMsg, ServerMsg, ServerRef, Target};
use std::net::SocketAddr;

/// How long the rendezvous server gets before a known direct address is tried too.
const SERVER_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

pub async fn resolve(addr: &str) -> Result<SocketAddr> {
    rendezvous::resolve(addr, rider_core::DEFAULT_PORT).await
}

/// Reach a host through the rendezvous server: get its candidate addresses,
/// punch NAT holes and race direct paths against the relay.
///
/// `pinned`: the host key we expect (`None` only while pairing by code; then
/// the key the server names is pinned and the verification code check catches
/// a lying server). `direct`: a known direct address to try as well.
/// `support`: a one-time support session (quick connect), not a paired one.
pub async fn via_server(
    identity: &Identity,
    server: &ServerRef,
    target: Target,
    pinned: Option<Fingerprint>,
    direct: Option<SocketAddr>,
    support: bool,
) -> Result<(quinn::Connection, Path)> {
    let (socket, rv, mut send, mut recv) = rendezvous::reach_server(identity, server).await?;
    let local = rendezvous::local_candidates(socket.port());
    write_msg(&mut send, &PeerMsg::Connect { version: rendezvous::VERSION, target, local }).await?;
    let reply = tokio::time::timeout(std::time::Duration::from_secs(10), read_msg::<ServerMsg>(&mut recv))
        .await
        .context("rendezvous server did not answer")??;
    rv.close(0u32.into(), b"done");
    let (host, mut candidates, relay) = match reply {
        Some(ServerMsg::Ready { host, candidates, relay }) => (host, candidates, relay),
        Some(ServerMsg::Error { message }) => bail!("{}", rendezvous::explain(&message)),
        _ => bail!("unexpected reply from the rendezvous server"),
    };
    if pinned.is_some_and(|p| p != host) {
        bail!("the rendezvous server offered a different host key; refusing");
    }
    if let Some(d) = direct.filter(|d| !candidates.contains(d)) {
        candidates.insert(0, d);
    }
    // Through the tunnel only the relay is reachable. Diagnostics: exercise
    // the relay even when a direct path would work.
    if socket.is_tunnel() || std::env::var_os("RIDER_FORCE_RELAY").is_some() {
        candidates.clear();
    }
    let cfg = if support {
        rider_core::tls::support_client_config(identity, Some(host))?
    } else {
        rider_core::tls::client_config(identity, Some(pinned.unwrap_or(host)))?
    };
    let (result, ()) = tokio::join!(
        rendezvous::race(&socket.endpoint, cfg, &candidates, relay.map(|r| r.addr)),
        socket.punch(&candidates, relay.as_ref()),
    );
    result
}

pub async fn open(identity: &Identity, addr: &str, pinned: Option<Fingerprint>) -> Result<quinn::Connection> {
    let sa = resolve(addr).await?;
    let mut endpoint = quinn::Endpoint::client(SocketAddr::from(([0, 0, 0, 0], 0)))?;
    endpoint.set_default_client_config(rider_core::tls::client_config(identity, pinned)?);
    let conn =
        tokio::time::timeout(std::time::Duration::from_secs(10), endpoint.connect(sa, rider_core::tls::SERVER_NAME)?)
            .await
            .context("timed out connecting (is the host running and UDP reachable?)")?
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(conn)
}

pub async fn hello(
    conn: &quinn::Connection,
    device_name: &str,
    pairing: bool,
) -> Result<(quinn::SendStream, quinn::RecvStream, String)> {
    let (mut send, mut recv) = conn.open_bi().await?;
    let _ = send.set_priority(1);
    write_msg(&mut send, &ClientMsg::Hello { version: PROTOCOL_VERSION, device_name: device_name.into(), pairing })
        .await?;
    match read_msg::<HostMsg>(&mut recv).await? {
        Some(HostMsg::HelloAck { version, host_name }) if version == PROTOCOL_VERSION => Ok((send, recv, host_name)),
        Some(HostMsg::HelloAck { version, .. }) => bail!("host speaks protocol {version}, we speak {PROTOCOL_VERSION}"),
        Some(HostMsg::Bye { reason }) => bail!("host refused: {reason}"),
        _ => bail!("unexpected reply from host"),
    }
}

/// What the user must compare with the host before confirming a pairing.
#[derive(Debug, Clone)]
pub struct PairPrompt {
    pub host_name: String,
    pub remote: SocketAddr,
    /// Six-digit verification code, formatted `123 456`.
    pub code: String,
}

/// Pair with a host by pairing code (via the server) or address.
///
/// `confirm` shows the verification code to the user and returns whether
/// they confirmed that the host shows the same one. On success the host is
/// saved in `cfg` and returned.
pub async fn pair(
    cfg: &mut ClientConfig,
    identity: &Identity,
    target: &str,
    alias: Option<String>,
    confirm: impl AsyncFnOnce(&PairPrompt) -> bool,
) -> Result<KnownHost> {
    let hint = rider_core::i18n::tr(
        "On the other computer, open This computer → Manage this computer → Add a device and enter the code it \
         shows, e.g. SF6-CK6. Codes expire after 10 minutes.",
        "Digər kompüterdə «Bu kompüter» → «Bu kompüteri idarə et» → «Cihaz əlavə et» düyməsini basın və göstərilən \
         kodu daxil edin, məs. SF6-CK6. Kodlar 10 dəqiqə etibarlıdır.",
    );
    let (conn, addr) = match (rendezvous::normalize_code(target), &cfg.server) {
        (Some(code), Some(server)) => {
            let (conn, path) = via_server(identity, server, Target::Code(code), None, None, false)
                .await
                .map_err(|e| anyhow::anyhow!("{e:#}\n{hint}"))?;
            // Remember a direct address only; relay ports are per session.
            let addr = match path {
                Path::Direct(a) => a.to_string(),
                Path::Relay(_) => String::new(),
            };
            (conn, addr)
        }
        (Some(_), None) => bail!("pairing codes need a rendezvous server: run `rider-client set-server` first"),
        (None, _) => {
            let conn = open(identity, target, None).await.map_err(|e| {
                // A computer name only resolves on the local network; from
                // elsewhere the pairing code is the way.
                let name_only = !target.contains(['.', ':']) && target.chars().any(|c| c.is_ascii_alphabetic());
                if name_only {
                    anyhow::anyhow!("\"{target}\" is not reachable by name from here.\n{hint}")
                } else {
                    anyhow::anyhow!("{e:#}\n{hint}")
                }
            })?;
            (conn, target.to_owned())
        }
    };
    let host_fp = rider_core::tls::peer_fingerprint(&conn)?;
    let (mut send, mut recv, host_name) = hello(&conn, &cfg.device_name, true).await?;
    let code = rider_core::sas::compute(&conn, &host_fp, &identity.fingerprint())?;
    let accepted = confirm(&PairPrompt { host_name: host_name.clone(), remote: conn.remote_address(), code }).await;
    write_msg(&mut send, &ClientMsg::PairConfirm { accepted }).await?;
    if !accepted {
        bail!("pairing cancelled");
    }
    match read_msg::<HostMsg>(&mut recv).await? {
        Some(HostMsg::PairResult { accepted: true }) => {}
        Some(HostMsg::PairResult { accepted: false }) => bail!("the host operator declined pairing"),
        Some(HostMsg::Bye { reason }) => bail!("host refused: {reason}"),
        _ => bail!("connection lost during pairing"),
    }
    let name = alias.unwrap_or(host_name);
    let host = KnownHost { name, addr, fingerprint: fp_hex(&host_fp), last_connected: None };
    cfg.upsert(host.clone());
    cfg.save()?;
    conn.close(0u32.into(), b"paired");
    Ok(host)
}

/// `piped`: PIN from `--pin-stdin`, tried once; otherwise up to three prompts.
/// Returns the authenticated session and the route it took ("direct 1.2.3.4:47800").
pub async fn connect(
    cfg: &ClientConfig,
    identity: &Identity,
    host: &KnownHost,
    mut piped: Option<String>,
) -> Result<(net::Session, String)> {
    let attempts = if piped.is_some() { 1 } else { 3 };
    let pinned = fp_from_hex(&host.fingerprint)?;
    let (conn, route, relayed) = match &cfg.server {
        Some(server) => {
            let direct = if host.addr.is_empty() { None } else { resolve(&host.addr).await.ok() };
            let via = via_server(identity, server, Target::Host(pinned), Some(pinned), direct, false);
            let (conn, path) = match direct {
                None => via.await?,
                Some(addr) => {
                    // Prefer the server (NAT traversal, relay), but don't make the
                    // user wait out a dead server when a direct address is known.
                    tokio::pin!(via);
                    let first = tokio::select! {
                        r = &mut via => Some(r),
                        _ = tokio::time::sleep(SERVER_GRACE) => None,
                    };
                    match first {
                        Some(Ok(done)) => done,
                        Some(Err(e)) => {
                            eprintln!("Via server: {e:#}. Trying {addr} directly...");
                            (open(identity, &host.addr, Some(pinned)).await?, Path::Direct(addr))
                        }
                        // Server slow: race it against the known address.
                        None => tokio::select! {
                            r = &mut via => r?,
                            // A failed direct attempt just leaves the server path running.
                            Ok(c) = open(identity, &host.addr, Some(pinned)) => (c, Path::Direct(addr)),
                        },
                    }
                }
            };
            (conn, path.to_string(), matches!(path, Path::Relay(_)))
        }
        None if host.addr.is_empty() => {
            bail!("\"{}\" is only reachable via a rendezvous server: run `rider-client set-server`", host.name)
        }
        None => (open(identity, &host.addr, Some(pinned)).await?, format!("direct {}", host.addr), false),
    };
    let (mut send, mut recv, _) = hello(&conn, &cfg.device_name, false).await?;
    for attempt in 1..=attempts {
        let pin = match piped.take() {
            Some(pin) => pin,
            None => tokio::task::spawn_blocking(|| rpassword::prompt_password("PIN: ")).await??,
        };
        write_msg(&mut send, &ClientMsg::Auth { pin }).await?;
        match read_msg::<HostMsg>(&mut recv).await? {
            Some(HostMsg::AuthResult { ok: true, .. }) => {
                return Ok((net::Session { conn, send, recv, relayed }, route));
            }
            Some(HostMsg::AuthResult { ok: false, message }) => {
                eprintln!("{message}");
                if attempt == attempts || message.contains("later") {
                    break;
                }
            }
            Some(HostMsg::Bye { reason }) => bail!("host refused: {reason}"),
            _ => bail!("connection lost during authentication"),
        }
    }
    bail!("authentication failed")
}

/// Shown to the helper while the person at the other computer decides.
#[derive(Debug, Clone)]
pub struct QuickPrompt {
    pub host_name: String,
    /// The same six digits the other computer shows next to Allow.
    pub code: String,
}

/// Quick connect: a one-time session by the code the other computer shows
/// ("Get a one-time code"). No pairing and no PIN: the person there allows
/// it. Nothing is saved. Returns the session, its route and the host's name.
pub async fn quick(
    cfg: &ClientConfig,
    identity: &Identity,
    code: &str,
    waiting: impl FnOnce(&QuickPrompt),
) -> Result<(net::Session, String, String)> {
    let Some(server) = &cfg.server else {
        bail!(tr(
            "quick connect needs a server: set it in This computer → Manage → Server",
            "sürətli qoşulma üçün server lazımdır: onu «Bu kompüter» → «İdarə et» → «Server» bölməsində təyin edin"
        ));
    };
    let Some(code) = rendezvous::normalize_code(code) else {
        bail!(tr(
            "that is not a valid code: it has 6 letters and digits, like SF6-CK6",
            "kod düzgün deyil: o, 6 hərf və rəqəmdən ibarətdir, məsələn, SF6-CK6"
        ));
    };
    let (conn, path) = via_server(identity, server, Target::Code(code), None, None, true).await.map_err(|e| {
        let text = format!("{e:#}");
        // A pairing code's host does not speak the support protocol.
        if text.contains("no_application_protocol") || text.contains("error 120") {
            anyhow::anyhow!(tr(
                "this code is for adding a computer (Add computer), not for quick connect",
                "bu kod sürətli qoşulma üçün deyil, kompüter əlavə etmək («Kompüter əlavə et») üçündür"
            ))
        } else if text.contains("offline or this device is not allowed") {
            anyhow::anyhow!(tr(
                "no computer is waiting with this code: it may have expired (10 minutes) or been used",
                "bu kodla gözləyən kompüter yoxdur: kodun müddəti bitib (10 dəqiqə) və ya o artıq istifadə olunub"
            ))
        } else {
            e
        }
    })?;
    let host_fp = rider_core::tls::peer_fingerprint(&conn)?;
    let (send, mut recv, host_name) = hello(&conn, &cfg.device_name, false).await?;
    let code = rider_core::sas::compute(&conn, &host_fp, &identity.fingerprint())?;
    waiting(&QuickPrompt { host_name: host_name.clone(), code });
    match read_msg::<HostMsg>(&mut recv).await? {
        Some(HostMsg::AuthResult { ok: true, .. }) => {}
        // The host's text is protocol English; this one the helper should read in their language.
        Some(HostMsg::AuthResult { ok: false, message }) if message.contains("declined") => {
            bail!(tr("the person at that computer declined", "həmin kompüterdəki şəxs imtina etdi"))
        }
        Some(HostMsg::AuthResult { ok: false, message }) => bail!("{message}"),
        Some(HostMsg::Bye { reason }) => bail!("{reason}"),
        _ => bail!(tr("the other computer closed the connection", "digər kompüter bağlantını bağladı")),
    }
    let relayed = matches!(path, Path::Relay(_));
    Ok((net::Session { conn, send, recv, relayed }, path.to_string(), host_name))
}

/// Enterprise mode: the company's admin connects to one of its computers.
/// No PIN: the person there accepts or declines (unless the computer is
/// unattended). `waiting(host name)` once it is reached and asking.
/// Returns the session, its route and the computer's name.
/// `cancel`: the admin gave up while the person there is still being asked;
/// the connection is closed properly, so their window goes away at once.
pub async fn company_connect(
    cfg: &ClientConfig,
    identity: &Identity,
    host: Fingerprint,
    waiting: impl FnOnce(&str),
    cancel: impl std::future::Future<Output = ()>,
) -> Result<(net::Session, String, String)> {
    let Some(server) = &cfg.server else {
        bail!(tr("Enterprise mode needs the company's server", "Müəssisə rejimi üçün şirkətin serveri lazımdır"));
    };
    let (conn, path) = via_server(identity, server, Target::Host(host), Some(host), None, false).await?;
    let (send, mut recv, host_name) = hello(&conn, &cfg.device_name, false).await?;
    waiting(&host_name);
    let answer = tokio::select! {
        answer = read_msg::<HostMsg>(&mut recv) => answer?,
        () = cancel => {
            conn.close(0u32.into(), b"cancelled");
            // Time for the close to go out before this process ends.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            bail!(tr("cancelled", "ləğv edildi"));
        }
    };
    match answer {
        Some(HostMsg::AuthResult { ok: true, .. }) => {}
        Some(HostMsg::AuthResult { ok: false, message }) if message.contains("declined") => {
            bail!(tr("the person at that computer declined", "həmin kompüterdəki şəxs imtina etdi"))
        }
        Some(HostMsg::AuthResult { ok: false, message }) if message.contains("did not answer") => {
            bail!(tr("nobody answered at that computer", "həmin kompüterdə heç kim cavab vermədi"))
        }
        Some(HostMsg::AuthResult { ok: false, message }) if message.contains("is locked") => {
            bail!(tr(
                "that computer is locked: the person there is away",
                "həmin kompüter kilidlidir: oradakı şəxs yerində deyil"
            ))
        }
        Some(HostMsg::AuthResult { ok: false, message }) if message.contains("nobody is signed in") => {
            bail!(tr(
                "nobody is signed in at that computer to accept",
                "həmin kompüterdə qəbul etmək üçün heç kim daxil olmayıb"
            ))
        }
        Some(HostMsg::AuthResult { ok: false, message }) => bail!("{message}"),
        Some(HostMsg::Bye { reason }) => bail!("{reason}"),
        _ => bail!(tr("the other computer closed the connection", "digər kompüter bağlantını bağladı")),
    }
    let relayed = matches!(path, Path::Relay(_));
    Ok((net::Session { conn, send, recv, relayed }, path.to_string(), host_name))
}

/// Ask the rendezvous server which of `hosts` are online (for this device).
pub async fn online(identity: &Identity, server: &ServerRef, hosts: &[Fingerprint]) -> Result<Vec<bool>> {
    let (_socket, conn, mut send, mut recv) = rendezvous::reach_server(identity, server).await?;
    write_msg(&mut send, &PeerMsg::Status { version: rendezvous::VERSION, hosts: hosts.to_vec() }).await?;
    let reply = tokio::time::timeout(std::time::Duration::from_secs(5), read_msg::<ServerMsg>(&mut recv))
        .await
        .context("rendezvous server did not answer")??;
    conn.close(0u32.into(), b"done");
    match reply {
        Some(ServerMsg::Online { online }) if online.len() == hosts.len() => Ok(online),
        Some(ServerMsg::Error { message }) => bail!("{}", rendezvous::explain(&message)),
        _ => bail!("unexpected reply from the rendezvous server"),
    }
}

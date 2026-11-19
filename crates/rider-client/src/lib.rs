//! Remote Desktop Rider client library: pairing, connecting (direct or via
//! the rendezvous server), host status, and the viewer window. Used by the
//! `rider-client` CLI and the desktop app.

pub mod app;
pub mod config;
pub mod diag;
mod keyhook;
pub mod net;
mod toolbar;
mod video;

use anyhow::{Context, Result, bail};
use config::{ClientConfig, KnownHost};
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
pub async fn via_server(
    identity: &Identity,
    server: &ServerRef,
    target: Target,
    pinned: Option<Fingerprint>,
    direct: Option<SocketAddr>,
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
        Some(ServerMsg::Error { message }) => bail!("{message}"),
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
    let cfg = rider_core::tls::client_config(identity, Some(pinned.unwrap_or(host)))?;
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
    let hint = "On the other computer, open This computer → Manage → Add a device (or run `rider-host pair`) \
                and enter the code it shows, e.g. SF6-CK6.";
    let (conn, addr) = match (rendezvous::normalize_code(target), &cfg.server) {
        (Some(code), Some(server)) => {
            let (conn, path) = via_server(identity, server, Target::Code(code), None, None)
                .await
                .map_err(|e| anyhow::anyhow!("{e:#}\n{hint} Codes expire after 10 minutes."))?;
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
            let via = via_server(identity, server, Target::Host(pinned), Some(pinned), direct);
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
        Some(ServerMsg::Error { message }) => bail!("{message}"),
        _ => bail!("unexpected reply from the rendezvous server"),
    }
}

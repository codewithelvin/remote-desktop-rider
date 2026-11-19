//! Keeps the host registered with the rendezvous server so paired clients
//! can reach it from anywhere, and opens NAT paths when one connects.
//!
//! The host has two UDP sockets: the fixed port (47800, direct connections on
//! the LAN and by address) and a random high port. Some networks (company
//! firewalls) drop UDP from unusual fixed ports while ordinary high ports get
//! through, so when the server does not answer on one socket the next attempt
//! uses the other; whichever works is kept. When neither gets an answer, the
//! host registers through the HTTPS tunnel instead (see `rider_core::tunnel`)
//! and accepts sessions on it, which then always go through the relay.

use crate::session::HostState;
use anyhow::{Result, bail};
use rider_core::identity::fp_short;
use rider_core::proto::{read_msg, write_msg};
use rider_core::rendezvous::{PeerMsg, PeerSocket, ServerMsg, ServerRef, VERSION, local_candidates};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

/// How often the allow-list, pairing state and LAN addresses are re-checked.
const UPDATE_INTERVAL: Duration = Duration::from_secs(2);
const SERVER_CHANGED: &str = "server setting changed";

fn configured(state: &HostState) -> Option<ServerRef> {
    state.refresh();
    state.config.lock().unwrap().server.clone()
}

/// Run forever, reconnecting with exponential backoff (1 s .. 30 s). The
/// server comes from the live config: `set-server` applies without restart.
/// `interactive`: print the pairing code to the console (not for the agent).
/// `accept`: TLS for sessions arriving through the HTTPS tunnel.
pub async fn run(state: Arc<HostState>, sockets: Vec<Arc<PeerSocket>>, accept: quinn::ServerConfig, interactive: bool) {
    let mut current = 0;
    let mut backoff = Duration::from_secs(1);
    let mut last_error = String::new();
    // Consecutive "no answer" results, across sockets.
    let mut silent_failures = 0;
    let mut last_udp_error = String::new();
    // Set once no UDP socket got an answer: register through the tunnel.
    let mut via_tunnel = false;
    let mut tunnel: Option<(ServerRef, Arc<PeerSocket>)> = None;
    loop {
        let Some(server) = configured(&state) else {
            tokio::time::sleep(UPDATE_INTERVAL).await;
            continue;
        };
        let started = std::time::Instant::now();
        let socket = if via_tunnel {
            open_tunnel(&state, &mut tunnel, &server, &accept).await
        } else {
            Ok(sockets[current].clone())
        };
        let result = match socket {
            Ok(socket) => session(&state, &socket, &server, interactive).await,
            Err(e) => Err(e),
        };
        if let Err(e) = result {
            let mut msg = format!("{e:#}");
            if msg == SERVER_CHANGED {
                backoff = Duration::from_secs(1);
                continue;
            }
            // No answer at all: maybe this port is filtered; try the other socket now,
            // then the tunnel, and only report the failure once all have had their turn.
            if !via_tunnel && msg.contains("did not answer") {
                silent_failures += 1;
                backoff = Duration::from_secs(1);
                let from = sockets[current].port();
                current = (current + 1) % sockets.len();
                if silent_failures < sockets.len() {
                    tracing::info!(
                        "rendezvous server {}: no answer from UDP {from}, trying UDP {}",
                        server.addr,
                        sockets[current].port()
                    );
                } else {
                    tracing::info!("rendezvous server {}: no answer over UDP, trying the HTTPS tunnel", server.addr);
                    via_tunnel = true;
                    silent_failures = 0;
                    last_udp_error.clone_from(&msg);
                }
                continue;
            }
            if via_tunnel {
                // Next time start over with UDP: the network may have changed.
                via_tunnel = false;
                if !last_udp_error.is_empty() {
                    msg = format!("{last_udp_error}; HTTPS tunnel: {msg}");
                }
            }
            // Log each distinct failure once instead of every retry.
            if msg != last_error {
                tracing::warn!("rendezvous server {}: {msg}", server.addr);
                crate::ui::emit(serde_json::json!({ "event": "server_error", "server": server.addr, "message": msg }));
                last_error = msg;
            }
        }
        if started.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(1); // was connected for a while: retry fast
            silent_failures = 0;
            last_udp_error.clear();
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

/// The tunnel for `server`, reused while it is open; a new one otherwise.
/// Each tunnel accepts sessions until it closes or is replaced and idle.
async fn open_tunnel(
    state: &Arc<HostState>,
    tunnel: &mut Option<(ServerRef, Arc<PeerSocket>)>,
    server: &ServerRef,
    accept: &quinn::ServerConfig,
) -> Result<Arc<PeerSocket>> {
    if let Some((s, t)) = tunnel
        && s == server
        && t.alive()
    {
        return Ok(t.clone());
    }
    *tunnel = None; // retire the old one (its sessions keep running)
    let socket = Arc::new(PeerSocket::tunnel(server, Some(accept.clone())).await?);
    tokio::spawn(accept_sessions(state.clone(), Arc::downgrade(&socket), socket.endpoint.clone()));
    *tunnel = Some((server.clone(), socket.clone()));
    Ok(socket)
}

/// Accept sessions on a tunnel endpoint. Once the rendezvous loop has let go
/// of the tunnel and no session uses it any more, close it.
async fn accept_sessions(state: Arc<HostState>, socket: std::sync::Weak<PeerSocket>, endpoint: quinn::Endpoint) {
    let mut check = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break };
                tokio::spawn(crate::session::handle(state.clone(), incoming));
            }
            _ = check.tick() => {
                if socket.strong_count() == 0 && endpoint.open_connections() == 0 {
                    break;
                }
            }
        }
    }
    endpoint.close(0u32.into(), b"tunnel closed");
}

#[derive(PartialEq)]
struct Registration {
    local: Vec<SocketAddr>,
    allowed: Vec<rider_core::identity::Fingerprint>,
    pairing: bool,
}

fn current(state: &HostState, port: u16) -> Registration {
    state.refresh();
    let mut allowed = state.config.lock().unwrap().trusted();
    allowed.sort_unstable();
    Registration { local: local_candidates(port), allowed, pairing: state.pairing_open() }
}

async fn session(
    state: &Arc<HostState>,
    socket: &Arc<PeerSocket>,
    server: &ServerRef,
    interactive: bool,
) -> Result<()> {
    let (conn, mut send, mut recv) = socket.connect_server(&state.identity, server).await?;
    let mut reg = current(state, socket.port());
    let register = |r: &Registration| PeerMsg::Register {
        version: VERSION,
        local: r.local.clone(),
        allowed: r.allowed.clone(),
        pairing: r.pairing,
    };
    write_msg(&mut send, &register(&reg)).await?;
    let mut shown_code: Option<String> = None;
    let mut tick = tokio::time::interval(UPDATE_INTERVAL);
    loop {
        tokio::select! {
            msg = read_msg::<ServerMsg>(&mut recv) => match msg? {
                Some(ServerMsg::Registered { public, pairing_code }) => {
                    tracing::info!("registered with {} as {public}", server.addr);
                    crate::ui::emit(serde_json::json!({
                        "event": "registered", "server": server.addr, "public": public, "code": pairing_code,
                    }));
                    if interactive && !crate::ui::json() && pairing_code.is_some() && pairing_code != shown_code {
                        let code = pairing_code.as_deref().unwrap_or_default();
                        println!("  Pairing code (via {}):  {code}", server.addr);
                        println!("    On the client run:  rider-client pair {code}");
                    }
                    shown_code = pairing_code;
                }
                Some(ServerMsg::Offer { client, candidates, relay }) => {
                    tracing::info!("connection request from {} via server", fp_short(&client));
                    let socket = socket.clone();
                    tokio::spawn(async move { socket.punch(&candidates, relay.as_ref()).await });
                }
                Some(ServerMsg::Error { message }) => bail!("{message}"),
                Some(ServerMsg::Ready { .. } | ServerMsg::Online { .. }) => {}
                None => bail!("server closed the connection"),
            },
            _ = tick.tick() => {
                if configured(state).as_ref() != Some(server) {
                    conn.close(0u32.into(), b"reconfigured");
                    bail!(SERVER_CHANGED);
                }
                let now = current(state, socket.port());
                if now != reg {
                    write_msg(&mut send, &register(&now)).await?;
                    reg = now;
                }
            }
            e = conn.closed() => bail!("{e}"),
        }
    }
}

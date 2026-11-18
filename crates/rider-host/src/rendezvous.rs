//! Keeps the host registered with the rendezvous server so paired clients
//! can reach it from anywhere, and opens NAT paths when one connects.

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
pub async fn run(state: Arc<HostState>, socket: Arc<PeerSocket>, interactive: bool) {
    let mut backoff = Duration::from_secs(1);
    let mut last_error = String::new();
    loop {
        let Some(server) = configured(&state) else {
            tokio::time::sleep(UPDATE_INTERVAL).await;
            continue;
        };
        let started = std::time::Instant::now();
        if let Err(e) = session(&state, &socket, &server, interactive).await {
            let msg = format!("{e:#}");
            if msg == SERVER_CHANGED {
                backoff = Duration::from_secs(1);
                continue;
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
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
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

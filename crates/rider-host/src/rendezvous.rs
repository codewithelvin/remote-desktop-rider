use crate::session::HostState;
use anyhow::{Result, bail};
use rider_core::identity::fp_short;
use rider_core::proto::{read_msg, write_msg};
use rider_core::rendezvous::{PeerMsg, PeerSocket, ServerMsg, ServerRef, VERSION, local_candidates};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const UPDATE_INTERVAL: Duration = Duration::from_secs(2);
const PLACE_INTERVAL: Duration = Duration::from_secs(15);
const SERVER_CHANGED: &str = "server setting changed";

fn configured(state: &HostState) -> Option<ServerRef> {
    state.refresh();
    state.config.lock().unwrap().server.clone()
}

pub async fn run(state: Arc<HostState>, sockets: Vec<Arc<PeerSocket>>, accept: quinn::ServerConfig, interactive: bool) {
    let mut current = 0;
    let mut backoff = Duration::from_secs(1);
    let mut last_error = String::new();
    let mut silent_failures = 0;
    let mut last_udp_error = String::new();
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
                via_tunnel = false;
                if !last_udp_error.is_empty() {
                    msg = format!("{last_udp_error}; HTTPS tunnel: {msg}");
                }
            }
            if msg != last_error {
                tracing::warn!("rendezvous server {}: {msg}", server.addr);
                crate::ui::emit(&serde_json::json!({ "event": "server_error", "server": server.addr, "message": msg }));
                last_error = msg;
            }
        }
        if started.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(1);
            silent_failures = 0;
            last_udp_error.clear();
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

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
    *tunnel = None;
    let socket = Arc::new(PeerSocket::tunnel(server, Some(accept.clone())).await?);
    tokio::spawn(accept_sessions(state.clone(), Arc::downgrade(&socket), socket.endpoint.clone()));
    *tunnel = Some((server.clone(), socket.clone()));
    Ok(socket)
}

async fn accept_sessions(state: Arc<HostState>, socket: std::sync::Weak<PeerSocket>, endpoint: quinn::Endpoint) {
    let mut check = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break };
                tokio::spawn(crate::session::handle(state.clone(), incoming, true));
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

struct Registrar<'a> {
    state: &'a Arc<HostState>,
    socket: &'a Arc<PeerSocket>,
    server: &'a ServerRef,
    interactive: bool,
    send: quinn::SendStream,
    reg: Registration,
    info: Option<rider_core::company::HostInfo>,
    place: Option<rider_core::company::Whereabouts>,
    takes_place: bool,
    shown_code: Option<String>,
}

async fn session(
    state: &Arc<HostState>,
    socket: &Arc<PeerSocket>,
    server: &ServerRef,
    interactive: bool,
) -> Result<()> {
    let (conn, send, mut recv) = socket.connect_server(&state.identity, server).await?;
    let mut r = Registrar {
        state,
        socket,
        server,
        interactive,
        send,
        reg: current(state, socket.port()),
        info: host_info(state),
        place: None,
        takes_place: false,
        shown_code: None,
    };
    r.register().await?;
    r.send_info().await?;
    let mut tick = tokio::time::interval(UPDATE_INTERVAL);
    let mut place_tick = tokio::time::interval(PLACE_INTERVAL);
    loop {
        tokio::select! {
            msg = read_msg::<ServerMsg>(&mut recv) => match msg? {
                Some(msg) => r.on_message(msg).await?,
                None => bail!("server closed the connection"),
            },
            _ = tick.tick() => {
                if configured(state).as_ref() != Some(server) {
                    conn.close(0u32.into(), b"reconfigured");
                    bail!(SERVER_CHANGED);
                }
                r.refresh().await?;
            }
            _ = place_tick.tick(), if r.takes_place => r.report_place(false).await?,
            e = conn.closed() => bail!("{e}"),
        }
    }
}

fn host_info(state: &HostState) -> Option<rider_core::company::HostInfo> {
    let cfg = state.config.lock().unwrap();
    cfg.company.as_ref().map(|c| crate::company::info(&cfg, Some(c)))
}

impl Registrar<'_> {
    async fn register(&mut self) -> Result<()> {
        let r = &self.reg;
        let msg = PeerMsg::Register {
            version: VERSION,
            local: r.local.clone(),
            allowed: r.allowed.clone(),
            pairing: r.pairing,
        };
        write_msg(&mut self.send, &msg).await
    }

    async fn send_info(&mut self) -> Result<()> {
        if let Some(i) = &self.info {
            write_msg(&mut self.send, &PeerMsg::HostInfo { version: VERSION, info: i.clone() }).await?;
        }
        Ok(())
    }

    async fn refresh(&mut self) -> Result<()> {
        let now = current(self.state, self.socket.port());
        if now != self.reg {
            self.reg = now;
            self.register().await?;
        }
        let now = host_info(self.state);
        if now != self.info {
            self.info = now;
            self.send_info().await?;
        }
        Ok(())
    }

    async fn report_place(&mut self, always: bool) -> Result<()> {
        let now = tokio::task::spawn_blocking(crate::company::whereabouts).await?;
        if always || self.place.as_ref() != Some(&now) {
            tracing::info!("place: {:?} ({})", now.place, now.via);
            write_msg(&mut self.send, &PeerMsg::Place { version: VERSION, place: now.clone() }).await?;
            self.place = Some(now);
        }
        Ok(())
    }

    fn set_policy(&self, unattended: bool) {
        tracing::info!("the company admin's unattended switch: {}", if unattended { "on" } else { "off" });
        self.state.company_unattended.store(unattended, std::sync::atomic::Ordering::Relaxed);
    }

    async fn on_message(&mut self, msg: ServerMsg) -> Result<()> {
        match msg {
            ServerMsg::Registered { public, pairing_code } => self.on_registered(public, pairing_code),
            ServerMsg::Offer { client, candidates, relay } => {
                tracing::info!("connection request from {} via server", fp_short(&client));
                let socket = self.socket.clone();
                tokio::spawn(async move { socket.punch(&candidates, relay.as_ref()).await });
            }
            ServerMsg::CompanyPolicy { unattended } => self.set_policy(unattended),
            ServerMsg::CompanyPolicy2 { unattended } => {
                self.set_policy(unattended);
                if !self.takes_place {
                    self.takes_place = true;
                    self.report_place(true).await?;
                }
            }
            ServerMsg::Error { message } => bail!("{message}"),
            ServerMsg::Ready { .. }
            | ServerMsg::Online { .. }
            | ServerMsg::Joined
            | ServerMsg::InviteCode { .. }
            | ServerMsg::Company { .. }
            | ServerMsg::Directory(_)
            | ServerMsg::Directory2 { .. }
            | ServerMsg::Enrolled { .. } => {}
        }
        Ok(())
    }

    fn on_registered(&mut self, public: SocketAddr, pairing_code: Option<String>) {
        let server = &self.server.addr;
        tracing::info!("registered with {server} as {public}");
        crate::ui::emit(&serde_json::json!({
            "event": "registered", "server": server, "public": public, "code": pairing_code,
        }));
        if self.interactive && !crate::ui::json() && pairing_code.is_some() && pairing_code != self.shown_code {
            let code = pairing_code.as_deref().unwrap_or_default();
            if self.state.support {
                println!("  One-time code (valid 10 minutes):  {code}");
                println!("    The helper runs:  rider-client quick {code}");
            } else {
                println!("  Pairing code (via {server}):  {code}");
                println!("    On the client run:  rider-client pair {code}");
            }
        }
        self.shown_code = pairing_code;
    }
}

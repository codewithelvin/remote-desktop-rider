//! Remote Desktop Rider rendezvous server (self-hosted, e.g. on a small VPS).
//!
//! Hosts keep a connection open and register which client keys may reach
//! them. When a client asks for a host, the server tells both sides each
//! other's addresses so they can connect directly through their NATs, and
//! opens a relay port as a fallback. It never sees screen or keyboard data:
//! sessions are end-to-end encrypted with keys pinned at pairing.

mod relay;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use relay::RelayPool;
use rider_core::identity::{Fingerprint, Identity, fp_hex, fp_short};
use rider_core::proto::{read_msg, write_msg};
use rider_core::rendezvous::{CODE_ALPHABET, PeerMsg, Relay, ServerMsg, Target, VERSION};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const CODE_LIFETIME: Duration = Duration::from_secs(600);
/// Connect requests allowed per source IP per minute (brute-force / abuse guard).
const CONNECTS_PER_MINUTE: u32 = 30;

#[derive(Parser)]
#[command(name = "rider-server", version, about = "Remote Desktop Rider rendezvous and relay server")]
struct Cli {
    /// Directory for the server key (created if missing).
    #[arg(long, global = true, default_value = "rider-server-data")]
    state_dir: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the server.
    Run {
        #[arg(long, default_value = "0.0.0.0:47900")]
        listen: SocketAddr,
        /// Public IP peers use to reach relay ports (needed behind NAT, e.g. some clouds).
        #[arg(long)]
        public_ip: Option<IpAddr>,
        /// UDP port range for relayed sessions, e.g. 47901-47999 (open it in the firewall).
        #[arg(long, default_value = "47901-47999", value_parser = parse_range)]
        relay_ports: std::ops::RangeInclusive<u16>,
    },
    /// Print the server fingerprint and the commands to configure hosts and clients.
    Info,
}

fn parse_range(s: &str) -> Result<std::ops::RangeInclusive<u16>, String> {
    let (a, b) = s.split_once('-').ok_or("expected FIRST-LAST")?;
    let (a, b): (u16, u16) = (a.parse().map_err(|_| "bad port")?, b.parse().map_err(|_| "bad port")?);
    if a == 0 || a > b {
        return Err("invalid range".into());
    }
    Ok(a..=b)
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,quinn=warn".into()),
        )
        .init();
    let cli = Cli::parse();
    std::fs::create_dir_all(&cli.state_dir)?;
    let identity = Identity::load_or_create(&cli.state_dir)?;
    match cli.cmd {
        Cmd::Info => print_setup(&identity, None),
        Cmd::Run { listen, public_ip, relay_ports } => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(serve(identity, listen, public_ip, relay_ports))?,
    }
    Ok(())
}

fn print_setup(identity: &Identity, listen: Option<SocketAddr>) {
    let fp = fp_hex(&identity.fingerprint());
    let port = listen.map(|l| l.port()).unwrap_or(rider_core::rendezvous::DEFAULT_SERVER_PORT);
    println!("Server fingerprint: {fp}");
    println!("Configure every host and client with:");
    println!("  rider-host   set-server <this-server-address>:{port} {fp}");
    println!("  rider-client set-server <this-server-address>:{port} {fp}");
}

struct HostEntry {
    conn_id: usize,
    tx: mpsc::UnboundedSender<ServerMsg>,
    public: SocketAddr,
    local: Vec<SocketAddr>,
    allowed: HashSet<Fingerprint>,
    pairing: bool,
}

struct Server {
    hosts: Mutex<HashMap<Fingerprint, HostEntry>>,
    codes: Mutex<HashMap<String, (Fingerprint, Instant)>>,
    rate: Mutex<HashMap<IpAddr, (Instant, u32)>>,
    relays: Arc<RelayPool>,
}

async fn serve(
    identity: Identity,
    listen: SocketAddr,
    public_ip: Option<IpAddr>,
    relay_ports: std::ops::RangeInclusive<u16>,
) -> Result<()> {
    let cfg = rider_core::tls::server_config_alpn(
        &identity,
        Arc::new(rider_core::tls::AnyClient),
        rider_core::tls::RENDEZVOUS_ALPN,
    )?;
    let endpoint = quinn::Endpoint::server(cfg, listen).with_context(|| format!("cannot listen on UDP {listen}"))?;
    let server = Arc::new(Server {
        hosts: Mutex::default(),
        codes: Mutex::default(),
        rate: Mutex::default(),
        relays: RelayPool::new(public_ip, relay_ports.clone()),
    });
    tracing::info!(
        "rendezvous on UDP {listen}, relay ports {}-{}{}",
        relay_ports.start(),
        relay_ports.end(),
        public_ip.map(|ip| format!(", public IP {ip}")).unwrap_or_default()
    );
    print_setup(&identity, Some(listen));

    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break };
                let server = server.clone();
                tokio::spawn(async move {
                    let remote = incoming.remote_address();
                    match incoming.await {
                        Ok(conn) => {
                            if let Err(e) = handle(&server, &conn).await {
                                tracing::debug!("{remote}: {e:#}");
                            }
                        }
                        Err(e) => tracing::debug!("{remote}: handshake failed: {e}"),
                    }
                });
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    endpoint.close(0u32.into(), b"server shutting down");
    Ok(())
}

async fn handle(server: &Arc<Server>, conn: &quinn::Connection) -> Result<()> {
    let fp = rider_core::tls::peer_fingerprint(conn)?;
    let (mut send, mut recv) = tokio::time::timeout(Duration::from_secs(10), conn.accept_bi()).await??;
    match read_msg::<PeerMsg>(&mut recv).await? {
        Some(PeerMsg::Register { version, local, allowed, pairing }) => {
            check_version(version, &mut send).await?;
            serve_host(server, conn, fp, send, recv, local, allowed, pairing).await
        }
        Some(PeerMsg::Connect { version, target, local }) => {
            check_version(version, &mut send).await?;
            let reply = connect(server, conn, fp, target, local).await;
            write_msg(&mut send, &reply).await?;
            let _ = send.finish();
            // Give the reply time to arrive; the client closes the connection.
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
            Ok(())
        }
        None => Ok(()),
    }
}

async fn check_version(version: u32, send: &mut quinn::SendStream) -> Result<()> {
    if version != VERSION {
        let message = format!("server speaks rendezvous protocol {VERSION}, you speak {version}; update");
        write_msg(send, &ServerMsg::Error { message: message.clone() }).await?;
        bail!(message);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn serve_host(
    server: &Arc<Server>,
    conn: &quinn::Connection,
    fp: Fingerprint,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    local: Vec<SocketAddr>,
    allowed: Vec<Fingerprint>,
    pairing: bool,
) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMsg>();
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if write_msg(&mut send, &msg).await.is_err() {
                break;
            }
        }
    });
    let conn_id = conn.stable_id();
    let public = conn.remote_address();
    server.hosts.lock().unwrap().insert(
        fp,
        HostEntry { conn_id, tx: tx.clone(), public, local, allowed: allowed.into_iter().collect(), pairing },
    );
    tracing::info!("host {} online from {public}", fp_short(&fp));
    let _ = tx.send(ServerMsg::Registered { public, pairing_code: server.update_code(fp, pairing) });

    // Further Register messages update the allow-list / pairing state.
    let result = async {
        while let Some(msg) = read_msg::<PeerMsg>(&mut recv).await? {
            if let PeerMsg::Register { local, allowed, pairing, .. } = msg {
                if let Some(h) = server.hosts.lock().unwrap().get_mut(&fp).filter(|h| h.conn_id == conn_id) {
                    h.local = local;
                    h.allowed = allowed.into_iter().collect();
                    h.pairing = pairing;
                }
                let _ = tx.send(ServerMsg::Registered { public, pairing_code: server.update_code(fp, pairing) });
            }
        }
        anyhow::Ok(())
    }
    .await;

    // Remove only our own entry: the host may already have reconnected.
    let mut hosts = server.hosts.lock().unwrap();
    if hosts.get(&fp).is_some_and(|h| h.conn_id == conn_id) {
        hosts.remove(&fp);
        server.codes.lock().unwrap().retain(|_, (owner, _)| *owner != fp);
        tracing::info!("host {} offline", fp_short(&fp));
    }
    drop(hosts);
    writer.abort();
    result
}

impl Server {
    /// Issue (or keep) a pairing code while `pairing` is on; revoke it when off.
    fn update_code(&self, fp: Fingerprint, pairing: bool) -> Option<String> {
        let mut codes = self.codes.lock().unwrap();
        let now = Instant::now();
        codes.retain(|_, (owner, issued)| now.duration_since(*issued) < CODE_LIFETIME && (pairing || *owner != fp));
        if !pairing {
            return None;
        }
        if let Some((code, _)) = codes.iter().find(|(_, (owner, _))| *owner == fp) {
            return Some(code.clone());
        }
        let code = loop {
            let c = new_code()?;
            if !codes.contains_key(&c) {
                break c;
            }
        };
        codes.insert(code.clone(), (fp, now));
        Some(code)
    }

    fn rate_limited(&self, ip: IpAddr) -> bool {
        let mut rate = self.rate.lock().unwrap();
        let now = Instant::now();
        if rate.len() > 10_000 {
            rate.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(60));
        }
        let entry = rate.entry(ip).or_insert((now, 0));
        if now.duration_since(entry.0) >= Duration::from_secs(60) {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 > CONNECTS_PER_MINUTE
    }
}

fn new_code() -> Option<String> {
    let mut out = String::with_capacity(7);
    let mut byte = [0u8; 1];
    while out.len() < 7 {
        if out.len() == 3 {
            out.push('-');
        }
        // Rejection sampling keeps every character equally likely.
        loop {
            getrandom::fill(&mut byte).ok()?;
            let limit = 256 - 256 % CODE_ALPHABET.len();
            if (byte[0] as usize) < limit {
                out.push(CODE_ALPHABET[byte[0] as usize % CODE_ALPHABET.len()] as char);
                break;
            }
        }
    }
    Some(out)
}

async fn connect(
    server: &Arc<Server>,
    conn: &quinn::Connection,
    client: Fingerprint,
    target: Target,
    mut local: Vec<SocketAddr>,
) -> ServerMsg {
    // One generic answer for "offline", "unknown" and "not allowed", so the
    // server does not reveal which hosts exist to devices that are not paired.
    let refuse = || ServerMsg::Error { message: "host is offline or this device is not allowed to reach it".into() };
    if server.rate_limited(conn.remote_address().ip()) {
        return ServerMsg::Error { message: "too many requests; try again in a minute".into() };
    }
    let host_fp = match target {
        Target::Host(fp) => fp,
        Target::Code(code) => {
            let Some(code) = rider_core::rendezvous::normalize_code(&code) else { return refuse() };
            match server.codes.lock().unwrap().get(&code) {
                Some((fp, issued)) if issued.elapsed() < CODE_LIFETIME => *fp,
                _ => return refuse(),
            }
        }
    };
    let (host_tx, mut host_candidates) = {
        let hosts = server.hosts.lock().unwrap();
        let Some(h) = hosts.get(&host_fp) else { return refuse() };
        if !(h.allowed.contains(&client) || h.pairing) {
            return refuse();
        }
        let mut c = h.local.clone();
        c.push(h.public);
        (h.tx.clone(), c)
    };
    local.push(conn.remote_address());
    local.truncate(9);
    host_candidates.truncate(9);

    let relay = server.relays.open(conn.local_ip()).await.map(|(addr, token)| Relay { addr, token });
    if host_tx.send(ServerMsg::Offer { client, candidates: local, relay }).is_err() {
        return refuse();
    }
    tracing::info!(
        "{} -> host {}{}",
        fp_short(&client),
        fp_short(&host_fp),
        relay.map(|r| format!(" (relay :{})", r.addr.port())).unwrap_or_default()
    );
    ServerMsg::Ready { host: host_fp, candidates: host_candidates, relay }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_well_formed_and_normalize() {
        for _ in 0..200 {
            let c = new_code().unwrap();
            assert_eq!(rider_core::rendezvous::normalize_code(&c).as_deref(), Some(c.as_str()));
        }
    }

    #[test]
    fn rate_limit_trips_after_quota() {
        let s = Server {
            hosts: Mutex::default(),
            codes: Mutex::default(),
            rate: Mutex::default(),
            relays: RelayPool::new(None, 1..=1),
        };
        let ip: IpAddr = "198.51.100.1".parse().unwrap();
        assert!((0..CONNECTS_PER_MINUTE).all(|_| !s.rate_limited(ip)));
        assert!(s.rate_limited(ip));
    }

    #[test]
    fn pairing_code_lifecycle() {
        let s = Server {
            hosts: Mutex::default(),
            codes: Mutex::default(),
            rate: Mutex::default(),
            relays: RelayPool::new(None, 1..=1),
        };
        let fp = [1u8; 32];
        let code = s.update_code(fp, true).unwrap();
        assert_eq!(s.update_code(fp, true).as_deref(), Some(code.as_str()), "stable while pairing");
        assert_eq!(s.update_code(fp, false), None);
        assert!(s.codes.lock().unwrap().is_empty(), "revoked when pairing ends");
    }
}

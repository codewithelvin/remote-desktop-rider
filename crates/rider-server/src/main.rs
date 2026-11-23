//! Remote Desktop Rider rendezvous server (self-hosted, e.g. on a small VPS).
//!
//! Hosts keep a connection open and register which client keys may reach
//! them. When a client asks for a host, the server tells both sides each
//! other's addresses so they can connect directly through their NATs, and
//! opens a relay port as a fallback. It never sees screen or keyboard data:
//! sessions are end-to-end encrypted with keys pinned at pairing.

mod members;
mod relay;
mod tunnel;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use members::Members;
use relay::RelayPool;
use rider_core::identity::{Fingerprint, Identity, fp_hex, fp_short};
use rider_core::proto::{read_msg, write_msg};
use rider_core::rendezvous::{CODE_ALPHABET, PeerMsg, Relay, ServerInfo, ServerMsg, Target, VERSION};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const CODE_LIFETIME: Duration = Duration::from_secs(600);
/// Connect requests allowed per source IP per minute (brute-force / abuse guard).
const CONNECTS_PER_MINUTE: u32 = 30;
/// Diagnostics reports accepted per device per hour.
const REPORTS_PER_HOUR: u32 = 30;

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
        /// Also accept the HTTPS tunnel (for networks that block UDP) on this
        /// loopback TCP address, behind a TLS web server, e.g. 127.0.0.1:47900.
        #[arg(long)]
        tunnel_listen: Option<SocketAddr>,
        /// Personal server: only member computers can be reached (and the
        /// devices they trust can reach any computer). New computers join
        /// with an invite code. Without it every computer that registers is
        /// recorded as a member, so switching this on later keeps them.
        #[arg(long)]
        invite_only: bool,
    },
    /// Print the server fingerprint and the commands to configure hosts and clients.
    Info,
    /// A one-time invite code for a new computer (valid 10 minutes).
    Invite,
    /// List member computers, or remove one.
    Members {
        /// Remove the member whose fingerprint starts with this (8+ characters).
        #[arg(long)]
        remove: Option<String>,
    },
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
        Cmd::Invite => {
            let code = Members::new(&cli.state_dir, true).create_invite()?;
            println!("Invite code: {code}");
            println!("Valid for 10 minutes, for one computer: enter it next to the server address.");
        }
        Cmd::Members { remove: Some(prefix) } => {
            let n = Members::new(&cli.state_dir, true).remove(&prefix)?;
            println!("Removed {n} member(s).");
        }
        Cmd::Members { remove: None } => {
            let list = Members::new(&cli.state_dir, true).list();
            if list.is_empty() {
                println!("No member computers yet.");
            }
            let mut list: Vec<_> = list.into_iter().collect();
            list.sort_by_key(|(_, t)| *t);
            for (fp, since) in list {
                println!("{}  member since {since} (unix time)", fp_hex(&fp));
            }
        }
        Cmd::Run { listen, public_ip, relay_ports, tunnel_listen, invite_only } => {
            let members = Members::new(&cli.state_dir, invite_only);
            tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(serve(
                identity,
                listen,
                public_ip,
                relay_ports,
                tunnel_listen,
                members,
            ))?
        }
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
    reports: Mutex<HashMap<Fingerprint, (Instant, u32)>>,
    relays: Arc<RelayPool>,
    tunnels: Arc<tunnel::Peers>,
    members: Members,
}

async fn serve(
    identity: Identity,
    listen: SocketAddr,
    public_ip: Option<IpAddr>,
    relay_ports: std::ops::RangeInclusive<u16>,
    tunnel_listen: Option<SocketAddr>,
    members: Members,
) -> Result<()> {
    let cfg = rider_core::tls::server_config_alpn(
        &identity,
        Arc::new(rider_core::tls::AnyClient),
        &[rider_core::tls::RENDEZVOUS_ALPN],
    )?;
    let endpoint = quinn::Endpoint::server(cfg, listen).with_context(|| format!("cannot listen on UDP {listen}"))?;
    let server = Arc::new(Server {
        hosts: Mutex::default(),
        codes: Mutex::default(),
        rate: Mutex::default(),
        reports: Mutex::default(),
        relays: RelayPool::new(public_ip, relay_ports.clone()),
        tunnels: Arc::default(),
        members,
    });
    if server.members.invite_only {
        tracing::info!("invite only: {} member computer(s)", server.members.list().len());
    }
    tracing::info!(
        "rendezvous on UDP {listen}, relay ports {}-{}{}",
        relay_ports.start(),
        relay_ports.end(),
        public_ip.map(|ip| format!(", public IP {ip}")).unwrap_or_default()
    );
    print_setup(&identity, Some(listen));
    if let Some(addr) = tunnel_listen {
        if public_ip.is_none() {
            tracing::warn!("--tunnel-listen without --public-ip: tunneled peers may get unusable relay addresses");
        }
        // Published over HTTPS, so devices only need the server's name.
        let info = ServerInfo { fingerprint: fp_hex(&identity.fingerprint()), port: listen.port() };
        let info = Arc::new(serde_json::to_string(&info)?);
        tokio::spawn(tunnel::serve(tunnel::bind(addr).await?, server.tunnels.clone(), info));
    }

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
        Some(PeerMsg::Status { version, mut hosts }) => {
            check_version(version, &mut send).await?;
            hosts.truncate(256);
            let reply = if server.rate_limited(conn.remote_address()) {
                ServerMsg::Error { message: "too many requests; try again in a minute".into() }
            } else {
                ServerMsg::Online { online: server.online(&fp, &hosts) }
            };
            write_msg(&mut send, &reply).await?;
            let _ = send.finish();
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
            Ok(())
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
        Some(PeerMsg::Join { version, code }) => {
            check_version(version, &mut send).await?;
            let reply = if server.rate_limited(conn.remote_address()) {
                ServerMsg::Error { message: "too many requests; try again in a minute".into() }
            } else {
                match server.members.join(fp, &code) {
                    Ok(()) => {
                        tracing::info!("{} joined with an invite", fp_short(&fp));
                        ServerMsg::Joined
                    }
                    Err(e) => ServerMsg::Error { message: format!("{e:#}") },
                }
            };
            write_msg(&mut send, &reply).await?;
            let _ = send.finish();
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
            Ok(())
        }
        Some(PeerMsg::Invite { version }) => {
            check_version(version, &mut send).await?;
            let reply = if !server.members.contains(&fp) {
                ServerMsg::Error { message: "only member computers can invite others".into() }
            } else if server.rate_limited(conn.remote_address()) {
                ServerMsg::Error { message: "too many requests; try again in a minute".into() }
            } else {
                match server.members.create_invite() {
                    Ok(code) => {
                        tracing::info!("{} created an invite", fp_short(&fp));
                        ServerMsg::InviteCode { code, minutes: (members::INVITE_LIFETIME.as_secs() / 60) as u32 }
                    }
                    Err(e) => ServerMsg::Error { message: format!("{e:#}") },
                }
            };
            write_msg(&mut send, &reply).await?;
            let _ = send.finish();
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
            Ok(())
        }
        Some(PeerMsg::Report { version, report }) => {
            // Only devices this server knows (registered hosts and the clients
            // they allow), within a quota; reports are metrics only.
            if version == VERSION && server.known(&fp) && !server.reports_limited(fp) {
                let report = serde_json::to_string(&report.bounded())?;
                tracing::info!(target: "diag", "{} {report}", fp_short(&fp));
            }
            conn.close(0u32.into(), b"ok");
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
    // An open server remembers who uses it, for turning on --invite-only.
    if !server.members.invite_only
        && let Err(e) = server.members.add(fp)
    {
        tracing::warn!("cannot record member: {e:#}");
    }
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

    /// Online state of `hosts` as seen by `client`: a host that does not
    /// allow this client reads as offline, so nothing leaks to strangers.
    fn online(&self, client: &Fingerprint, hosts: &[Fingerprint]) -> Vec<bool> {
        let registered = self.hosts.lock().unwrap();
        hosts.iter().map(|fp| registered.get(fp).is_some_and(|h| h.allowed.contains(client))).collect()
    }

    /// A registered member host, or a client some registered member allows.
    fn known(&self, fp: &Fingerprint) -> bool {
        let hosts = self.hosts.lock().unwrap();
        hosts.contains_key(fp) && self.members.admitted(fp) || self.trusted_by_member(&hosts, fp)
    }

    /// Invite only: sessions to a member computer, or from a device a member
    /// trusts (the owner helping any computer by quick connect).
    fn may_reach(&self, hosts: &HashMap<Fingerprint, HostEntry>, host: &Fingerprint, client: &Fingerprint) -> bool {
        self.members.admitted(host) || self.trusted_by_member(hosts, client)
    }

    /// `client` is allowed by a registered member host (the owner's devices).
    fn trusted_by_member(&self, hosts: &HashMap<Fingerprint, HostEntry>, client: &Fingerprint) -> bool {
        hosts.iter().any(|(fp, h)| h.allowed.contains(client) && self.members.admitted(fp))
    }

    fn reports_limited(&self, fp: Fingerprint) -> bool {
        let mut reports = self.reports.lock().unwrap();
        let now = Instant::now();
        reports.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(3600));
        let entry = reports.entry(fp).or_insert((now, 0));
        entry.1 += 1;
        entry.1 > REPORTS_PER_HOUR
    }

    /// Per source address; tunneled peers count under their real address.
    fn rate_limited(&self, from: SocketAddr) -> bool {
        let ip = if from.ip().is_loopback() { self.tunnels.real(from).unwrap_or(from.ip()) } else { from.ip() };
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
    if server.rate_limited(conn.remote_address()) {
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
        if !server.may_reach(&hosts, &host_fp, &client) {
            return refuse();
        }
        let mut c = h.local.clone();
        // A tunneled peer's address is the bridge's loopback socket: not a candidate.
        c.extend(Some(h.public).filter(|a| !a.ip().is_loopback()));
        (h.tx.clone(), c)
    };
    local.extend(Some(conn.remote_address()).filter(|a| !a.ip().is_loopback()));
    local.truncate(9);
    host_candidates.truncate(9);

    // Each peer gets its own token (see relay.rs).
    let relays = server.relays.open(conn.local_ip()).await;
    let host_relay = relays.map(|(addr, [token, _])| Relay { addr, token });
    let relay = relays.map(|(addr, [_, token])| Relay { addr, token });
    if host_tx.send(ServerMsg::Offer { client, candidates: local, relay: host_relay }).is_err() {
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
            reports: Mutex::default(),
            relays: RelayPool::new(None, 1..=1),
            tunnels: Arc::default(),
            members: Members::new(&std::env::temp_dir(), false),
        };
        let from: SocketAddr = "198.51.100.1:5000".parse().unwrap();
        assert!((0..CONNECTS_PER_MINUTE).all(|_| !s.rate_limited(from)));
        assert!(s.rate_limited(from));
        // Tunneled peers count under their real address, not loopback.
        let tunneled: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        s.tunnels.insert_for_test(tunneled, from.ip());
        assert!(s.rate_limited(tunneled), "same quota as the real address");
        let other: SocketAddr = "127.0.0.1:40001".parse().unwrap();
        assert!(!s.rate_limited(other));
    }

    #[test]
    fn invite_only_serves_members_and_their_devices() {
        let dir = std::env::temp_dir().join(format!("rider-gate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Server {
            hosts: Mutex::default(),
            codes: Mutex::default(),
            rate: Mutex::default(),
            reports: Mutex::default(),
            relays: RelayPool::new(None, 1..=1),
            tunnels: Arc::default(),
            members: Members::new(&dir, true),
        };
        let (member, laptop, stranger_host, stranger_pc, support) =
            ([1u8; 32], [2u8; 32], [3u8; 32], [4u8; 32], [5u8; 32]);
        s.members.add(member).unwrap();
        let entry = |allowed: &[Fingerprint]| HostEntry {
            conn_id: 0,
            tx: mpsc::unbounded_channel().0,
            public: "198.51.100.1:1".parse().unwrap(),
            local: Vec::new(),
            allowed: allowed.iter().copied().collect(),
            pairing: false,
        };
        let mut hosts = s.hosts.lock().unwrap();
        hosts.insert(member, entry(&[laptop]));
        hosts.insert(stranger_host, entry(&[stranger_pc]));
        hosts.insert(support, entry(&[]));
        assert!(s.may_reach(&hosts, &member, &laptop), "own device to own computer");
        assert!(s.may_reach(&hosts, &member, &stranger_pc), "pairing a new device with a member");
        assert!(s.may_reach(&hosts, &support, &laptop), "quick-connect help from an own device");
        assert!(!s.may_reach(&hosts, &stranger_host, &stranger_pc), "strangers cannot use the server");
        assert!(!s.may_reach(&hosts, &support, &stranger_pc));
        drop(hosts);
        assert!(s.known(&member) && s.known(&laptop), "reports from the owner's devices");
        assert!(!s.known(&stranger_pc) && !s.known(&stranger_host), "not from strangers");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pairing_code_lifecycle() {
        let s = Server {
            hosts: Mutex::default(),
            codes: Mutex::default(),
            rate: Mutex::default(),
            reports: Mutex::default(),
            relays: RelayPool::new(None, 1..=1),
            tunnels: Arc::default(),
            members: Members::new(&std::env::temp_dir(), false),
        };
        let fp = [1u8; 32];
        let code = s.update_code(fp, true).unwrap();
        assert_eq!(s.update_code(fp, true).as_deref(), Some(code.as_str()), "stable while pairing");
        assert_eq!(s.update_code(fp, false), None);
        assert!(s.codes.lock().unwrap().is_empty(), "revoked when pairing ends");
    }
}

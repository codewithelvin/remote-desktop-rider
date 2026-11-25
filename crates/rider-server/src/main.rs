mod company;
mod members;
mod relay;
mod tunnel;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use members::Members;
use relay::RelayPool;
use rider_core::company::DirEdit;
use rider_core::identity::{Fingerprint, Identity, fp_hex, fp_short};
use rider_core::proto::{read_msg, write_msg};
use rider_core::rendezvous::{CODE_ALPHABET, PeerMsg, Relay, ServerInfo, ServerMsg, Target, VERSION, refusal};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const CODE_LIFETIME: Duration = Duration::from_secs(600);
const CONNECTS_PER_MINUTE: u32 = 30;
const REPORTS_PER_HOUR: u32 = 30;

#[derive(Parser)]
#[command(name = "rider-server", version, about = "Remote Desktop Rider rendezvous and relay server")]
struct Cli {
    #[arg(long, global = true, default_value = "rider-server-data")]
    state_dir: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Run {
        #[arg(long, default_value = "0.0.0.0:47900")]
        listen: SocketAddr,
        #[arg(long)]
        public_ip: Option<IpAddr>,
        #[arg(long, default_value = "47901-47999", value_parser = parse_range)]
        relay_ports: std::ops::RangeInclusive<u16>,
        #[arg(long)]
        tunnel_listen: Option<SocketAddr>,
        #[arg(long)]
        invite_only: bool,
    },
    Info,
    Invite,
    Members {
        #[arg(long)]
        remove: Option<String>,
    },
    Company {
        #[command(subcommand)]
        action: Option<CompanyCmd>,
    },
}

#[derive(Subcommand)]
enum CompanyCmd {
    Init {
        #[arg(long)]
        name: String,
    },
    SetupCode,
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
        Cmd::Company { action } => company_command(&cli.state_dir, action)?,
        Cmd::Run { listen, public_ip, relay_ports, tunnel_listen, invite_only } => {
            let members = Members::new(&cli.state_dir, invite_only);
            let company = company::Company::load(&cli.state_dir)?;
            tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(serve(
                identity,
                listen,
                public_ip,
                relay_ports,
                tunnel_listen,
                members,
                company,
            ))?
        }
    }
    Ok(())
}

fn company_command(dir: &std::path::Path, action: Option<CompanyCmd>) -> Result<()> {
    let print_code = |code: &str| {
        println!("Admin setup code: {code}");
        println!("Valid 24 hours, once: in the app, Enterprise mode > Sign in, with this server's address.");
    };
    match action {
        Some(CompanyCmd::Init { name }) => {
            let code = company::Company::init(dir, &name)?;
            println!("Company \"{}\" created.", name.trim());
            print_code(&code);
        }
        Some(CompanyCmd::SetupCode) => print_code(&company::Company::new_setup_code(dir)?),
        None => match company::Company::load(dir)?.summary() {
            None => println!("No company on this server. Create one: rider-server company init --name <name>"),
            Some((name, admin, computers)) => {
                println!("Company:   {name}");
                println!("Admin:     {}", admin.as_deref().unwrap_or("none yet (rider-server company setup-code)"));
                println!("Computers: {computers}");
            }
        },
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
    company: company::Company,
}

async fn serve(
    identity: Identity,
    listen: SocketAddr,
    public_ip: Option<IpAddr>,
    relay_ports: std::ops::RangeInclusive<u16>,
    tunnel_listen: Option<SocketAddr>,
    members: Members,
    company: company::Company,
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
        company,
    });
    if let Some((name, _, n)) = server.company.summary() {
        tracing::info!("company \"{name}\": {n} computer(s)");
    }
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
        let info = ServerInfo { fingerprint: fp_hex(&identity.fingerprint()), port: listen.port() };
        let info = Arc::new(serde_json::to_string(&info)?);
        tokio::spawn(tunnel::serve(tunnel::bind(addr).await?, server.tunnels.clone(), info));
    }

    {
        let server = server.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(3));
            loop {
                tick.tick().await;
                let company = server.clone();
                let saved = tokio::task::spawn_blocking(move || company.company.flush()).await;
                if let Ok(Err(e)) = saved {
                    tracing::warn!("cannot save the company: {e:#}");
                }
            }
        });
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
            _ = stop_signal() => break,
        }
    }
    endpoint.close(0u32.into(), b"server shutting down");
    if let Err(e) = server.company.flush() {
        tracing::warn!("cannot save the company: {e:#}");
    }
    Ok(())
}

async fn stop_signal() {
    #[cfg(unix)]
    {
        let Ok(mut term) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
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
                ServerMsg::Error { message: refusal::BUSY.into() }
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
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
            Ok(())
        }
        Some(PeerMsg::Join { version, code }) => {
            check_version(version, &mut send).await?;
            let reply = if server.rate_limited(conn.remote_address()) {
                ServerMsg::Error { message: refusal::BUSY.into() }
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
                ServerMsg::Error { message: refusal::BUSY.into() }
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
            if version == VERSION && server.known(&fp) && !server.reports_limited(fp) {
                let report = serde_json::to_string(&report.bounded())?;
                tracing::info!(target: "diag", "{} {report}", fp_short(&fp));
            }
            conn.close(0u32.into(), b"ok");
            Ok(())
        }
        Some(
            msg @ (PeerMsg::AdminClaim { .. }
            | PeerMsg::Directory { .. }
            | PeerMsg::DirectoryEdit { .. }
            | PeerMsg::Directory2 { .. }
            | PeerMsg::Enroll { .. }),
        ) => {
            let reply = company_request(server, conn, fp, msg);
            write_msg(&mut send, &reply).await?;
            let _ = send.finish();
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
            Ok(())
        }
        Some(PeerMsg::HostInfo { .. } | PeerMsg::Place { .. }) | None => Ok(()),
    }
}

fn policy(server: &Server, fp: &Fingerprint, unattended: bool) -> ServerMsg {
    if server.company.takes_place(fp) {
        ServerMsg::CompanyPolicy2 { unattended }
    } else {
        ServerMsg::CompanyPolicy { unattended }
    }
}

fn company_request(server: &Server, conn: &quinn::Connection, fp: Fingerprint, msg: PeerMsg) -> ServerMsg {
    let error = |message: String| ServerMsg::Error { message };
    let version = match &msg {
        PeerMsg::AdminClaim { version, .. }
        | PeerMsg::Directory { version }
        | PeerMsg::DirectoryEdit { version, .. }
        | PeerMsg::Directory2 { version }
        | PeerMsg::Enroll { version, .. } => *version,
        _ => return error("unexpected request".into()),
    };
    if version != VERSION {
        return error(format!("server speaks rendezvous protocol {VERSION}, you speak {version}; update"));
    }
    let directory = |server: &Server| {
        let online: HashSet<Fingerprint> = server.hosts.lock().unwrap().keys().copied().collect();
        server
            .company
            .directory(&online)
            .map_or_else(|| error("this server has no company".into()), ServerMsg::Directory)
    };
    match msg {
        PeerMsg::AdminClaim { .. } | PeerMsg::Enroll { .. } if server.rate_limited(conn.remote_address()) => {
            error(refusal::BUSY.into())
        }
        PeerMsg::AdminClaim { code, .. } => match server.company.claim(&fp, &code) {
            Ok((company, join_secret)) => {
                tracing::info!("{} is now the admin of \"{company}\"", fp_short(&fp));
                ServerMsg::Company { company, join_secret }
            }
            Err(e) => error(format!("{e:#}")),
        },
        PeerMsg::Enroll { secret, info, .. } => match server.company.enroll(&fp, &secret, info) {
            Ok(company) => {
                tracing::info!("{} joined the company", fp_short(&fp));
                if let Err(e) = server.members.add(fp) {
                    tracing::warn!("cannot record member: {e:#}");
                }
                ServerMsg::Enrolled { company }
            }
            Err(e) => error(format!("{e:#}")),
        },
        _ if !server.company.is_admin(&fp) => error("only the company's admin can see its computers".into()),
        PeerMsg::Directory { .. } => directory(server),
        PeerMsg::Directory2 { .. } => {
            let online: HashSet<Fingerprint> = server.hosts.lock().unwrap().keys().copied().collect();
            match server.company.directory(&online) {
                Some(directory) => ServerMsg::Directory2 { directory, places: server.company.places(&online) },
                None => error("this server has no company".into()),
            }
        }
        PeerMsg::DirectoryEdit { edit, .. } => {
            let switched = match &edit {
                DirEdit::Unattended { computer, on } => Some((*computer, *on)),
                _ => None,
            };
            match server.company.edit(edit) {
                Ok(()) => {
                    if let Some((computer, unattended)) = switched
                        && let Some(h) = server.hosts.lock().unwrap().get(&computer)
                    {
                        let _ = h.tx.send(policy(server, &computer, unattended));
                    }
                    directory(server)
                }
                Err(e) => error(format!("{e:#}")),
            }
        }
        _ => error("unexpected request".into()),
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

    let result = async {
        while let Some(msg) = read_msg::<PeerMsg>(&mut recv).await? {
            match msg {
                PeerMsg::Register { local, allowed, pairing, .. } => {
                    if let Some(h) = server.hosts.lock().unwrap().get_mut(&fp).filter(|h| h.conn_id == conn_id) {
                        h.local = local;
                        h.allowed = allowed.into_iter().collect();
                        h.pairing = pairing;
                    }
                    let _ = tx.send(ServerMsg::Registered { public, pairing_code: server.update_code(fp, pairing) });
                }
                PeerMsg::HostInfo { info, .. } => {
                    server.company.update_info(&fp, info);
                    let _ = tx.send(policy(server, &fp, server.company.unattended_on(&fp)));
                }
                PeerMsg::Place { place, .. } => server.company.set_place(&fp, place),
                _ => {}
            }
        }
        anyhow::Ok(())
    }
    .await;

    let mut hosts = server.hosts.lock().unwrap();
    if hosts.get(&fp).is_some_and(|h| h.conn_id == conn_id) {
        hosts.remove(&fp);
        server.codes.lock().unwrap().retain(|_, (owner, _)| *owner != fp);
        tracing::info!("host {} offline", fp_short(&fp));
    }
    drop(hosts);
    server.company.seen(&fp);
    writer.abort();
    result
}

impl Server {
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

    fn online(&self, client: &Fingerprint, hosts: &[Fingerprint]) -> Vec<bool> {
        let registered = self.hosts.lock().unwrap();
        hosts.iter().map(|fp| registered.get(fp).is_some_and(|h| h.allowed.contains(client))).collect()
    }

    fn known(&self, fp: &Fingerprint) -> bool {
        let hosts = self.hosts.lock().unwrap();
        hosts.contains_key(fp) && self.members.admitted(fp) || self.trusted_by_member(&hosts, fp)
    }

    fn may_reach(&self, hosts: &HashMap<Fingerprint, HostEntry>, host: &Fingerprint, client: &Fingerprint) -> bool {
        self.members.admitted(host) || self.trusted_by_member(hosts, client)
    }

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
    let refuse = || ServerMsg::Error { message: refusal::UNREACHABLE.into() };
    if server.rate_limited(conn.remote_address()) {
        return ServerMsg::Error { message: refusal::BUSY.into() };
    }
    let host_fp = match target {
        Target::Host(fp) => fp,
        Target::Code(code) => {
            let Some(code) = rider_core::rendezvous::normalize_code(&code) else { return refuse() };
            match server.codes.lock().unwrap().get(&code) {
                Some((fp, issued)) if issued.elapsed() < CODE_LIFETIME => *fp,
                _ if server.members.is_invite(&code) => {
                    return ServerMsg::Error { message: refusal::INVITE_NOT_PAIRING.into() };
                }
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
        c.extend(Some(h.public).filter(|a| !a.ip().is_loopback()));
        (h.tx.clone(), c)
    };
    local.extend(Some(conn.remote_address()).filter(|a| !a.ip().is_loopback()));
    local.truncate(9);
    host_candidates.truncate(9);

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
            company: company::Company::none(),
        };
        let from: SocketAddr = "198.51.100.1:5000".parse().unwrap();
        assert!((0..CONNECTS_PER_MINUTE).all(|_| !s.rate_limited(from)));
        assert!(s.rate_limited(from));
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
            company: company::Company::none(),
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
            company: company::Company::none(),
        };
        let fp = [1u8; 32];
        let code = s.update_code(fp, true).unwrap();
        assert_eq!(s.update_code(fp, true).as_deref(), Some(code.as_str()), "stable while pairing");
        assert_eq!(s.update_code(fp, false), None);
        assert!(s.codes.lock().unwrap().is_empty(), "revoked when pairing ends");
    }

    #[test]
    fn enterprise_over_the_wire() {
        use rider_core::company::{DirEdit, HostInfo};
        use rider_core::rendezvous::{ServerRef, admin_claim, directory, enroll};
        let dir = std::env::temp_dir().join(format!("rider-enterprise-wire-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let identity = Identity::load_or_create(&dir).unwrap();
        let code = company::Company::init(&dir, "Northwind").unwrap();
        let port = 47_000 + (std::process::id() % 800) as u16;
        let server_ref = ServerRef::new(&format!("127.0.0.1:{port}"), &fp_hex(&identity.fingerprint())).unwrap();
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let members = Members::new(&dir, false);
            let company = company::Company::load(&dir).unwrap();
            let listen: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
            tokio::spawn(serve(identity, listen, None, 48_000..=48_010, None, members, company));
            tokio::time::sleep(Duration::from_millis(300)).await;

            let admin = Identity::ephemeral().unwrap();
            let pc = Identity::ephemeral().unwrap();
            let stranger = Identity::ephemeral().unwrap();
            assert!(directory(&admin, &server_ref, None).await.is_err(), "not the admin yet");
            let (name, secret) = admin_claim(&admin, &server_ref, &code).await.unwrap();
            assert_eq!(name, "Northwind");
            assert!(admin_claim(&stranger, &server_ref, &code).await.is_err(), "a code works once");

            let info = HostInfo { name: "PC-310-07".into(), version: "0.5.0".into(), ..Default::default() };
            assert!(enroll(&pc, &server_ref, "wrong", info.clone()).await.is_err());
            assert_eq!(enroll(&pc, &server_ref, &secret, info).await.unwrap(), "Northwind");

            assert!(directory(&stranger, &server_ref, None).await.is_err(), "only the admin sees the list");
            assert!(directory(&pc, &server_ref, None).await.is_err(), "a company computer is not the admin");
            let edit = DirEdit::AddFloor { name: "Mine".into() };
            assert!(directory(&stranger, &server_ref, Some(edit.clone())).await.is_err(), "nor changes it");
            assert!(directory(&pc, &server_ref, Some(edit)).await.is_err(), "nor changes it");
            let forged = HostInfo { name: "FORGED".into(), ..Default::default() };
            assert!(enroll(&stranger, &server_ref, "", forged).await.is_err(), "no secret, no joining");
            let d = directory(&admin, &server_ref, Some(DirEdit::AddFloor { name: "Floor 3".into() })).await.unwrap();
            let floor = d.floors[0].id;
            let d = directory(&admin, &server_ref, Some(DirEdit::AddRoom { floor, name: "310".into() })).await.unwrap();
            let room = d.floors[0].rooms[0].id;
            assert_eq!(d.room_counts(None), (0, 1), "the new computer waits in Unassigned (offline)");
            let edit = DirEdit::Place { computers: vec![pc.fingerprint()], room: Some(room) };
            let d = directory(&admin, &server_ref, Some(edit)).await.unwrap();
            assert_eq!(d.computers[0].room, Some(room));
            assert_eq!(d.computers[0].info.name, "PC-310-07");
            assert!(d.computers.iter().all(|c| c.info.name != "FORGED"));
            let ghost = DirEdit::Place { computers: vec![stranger.fingerprint()], room: None };
            assert!(directory(&admin, &server_ref, Some(ghost)).await.is_err(), "not a company computer");
        });
        drop(rt);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn enterprise_at_scale() {
        use rider_core::company::{DirEdit, HostInfo};
        use rider_core::rendezvous::{ServerRef, admin_claim, directory};
        let dir = std::env::temp_dir().join(format!("rider-enterprise-scale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let identity = Identity::load_or_create(&dir).unwrap();
        let code = company::Company::init(&dir, "Big").unwrap();
        let long = "\u{1F600}".repeat(100);
        let secret = {
            let c = company::Company::load(&dir).unwrap();
            let (_, secret) = c.claim(&[1; 32], &company::Company::new_setup_code(&dir).unwrap()).unwrap();
            let info =
                HostInfo { name: long.clone(), user: Some(long.clone()), version: long.clone(), ..Default::default() };
            c.fill(company::MAX_COMPUTERS as u32 - 1, &info);
            c.enroll(&[8; 32], &secret, info).unwrap();
            let mut fp = [9u8; 32];
            fp[31] = 8;
            assert!(c.enroll(&fp, &secret, HostInfo::default()).is_err(), "no more than the limit");
            secret
        };
        let _ = secret;
        let port = 47_000 + ((std::process::id() + 400) % 800) as u16;
        let server_ref = ServerRef::new(&format!("127.0.0.1:{port}"), &fp_hex(&identity.fingerprint())).unwrap();
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let members = Members::new(&dir, false);
            let company = company::Company::load(&dir).unwrap();
            let listen: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
            tokio::spawn(serve(identity, listen, None, 48_020..=48_030, None, members, company));
            tokio::time::sleep(Duration::from_millis(300)).await;
            let admin = Identity::ephemeral().unwrap();
            assert!(admin_claim(&admin, &server_ref, &code).await.is_err());
            let fresh = company::Company::new_setup_code(&dir).unwrap();
            admin_claim(&admin, &server_ref, &fresh).await.unwrap();

            let started = std::time::Instant::now();
            let d = directory(&admin, &server_ref, Some(DirEdit::AddFloor { name: "F".into() })).await.unwrap();
            eprintln!("{} computers listed in {:?}", d.computers.len(), started.elapsed());
            assert_eq!(d.computers.len(), company::MAX_COMPUTERS);
            let floor = d.floors[0].id;
            let d = directory(&admin, &server_ref, Some(DirEdit::AddRoom { floor, name: "R".into() })).await.unwrap();
            let room = d.floors[0].rooms[0].id;
            let all: Vec<_> = d.computers.iter().map(|c| c.fingerprint).collect();
            let started = std::time::Instant::now();
            let d = directory(&admin, &server_ref, Some(DirEdit::Place { computers: all, room: Some(room) }))
                .await
                .unwrap();
            eprintln!("all moved in {:?}", started.elapsed());
            assert_eq!(d.room_counts(Some(room)), (0, company::MAX_COMPUTERS));
        });
        drop(rt);
        let kept = company::Company::load(&dir).unwrap().directory(&Default::default()).unwrap();
        assert!(kept.computers.iter().all(|c| c.room.is_some()), "kept on disk");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

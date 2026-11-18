//! Remote Desktop Rider - client (the viewer you control a host from).

mod app;
mod config;
mod net;
mod video;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use config::{ClientConfig, KnownHost};
use rider_core::identity::{Identity, fp_from_hex, fp_hex, fp_short};
use rider_core::proto::*;
use rider_core::rendezvous::{self, Path, PeerMsg, PeerSocket, ServerMsg, ServerRef, Target};
use std::io::Write;
use std::net::SocketAddr;

/// How long the rendezvous server gets before a known direct address is tried too.
const SERVER_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Parser)]
#[command(name = "rider-client", version, about = "Remote Desktop Rider client")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// One-time pairing with a host that is running `rider-host pair`.
    Pair {
        /// The pairing code the host shows (e.g. 7K3-QX9, needs `set-server`),
        /// or the host's address, e.g. 192.168.1.20 or office-pc.example.com:47800.
        addr: String,
        /// Name to save the host under (defaults to the host's own name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Connect to a paired host by name or address.
    Connect {
        host: String,
        /// Read the PIN from one line of stdin instead of prompting.
        #[arg(long)]
        pin_stdin: bool,
    },
    /// List paired hosts.
    List,
    /// Forget a paired host.
    Forget { host: String },
    /// Show this device's identity.
    Info,
    /// Use a rendezvous server (rider-server) to reach hosts on other networks.
    SetServer {
        /// Server address, e.g. rider.example.com or 203.0.113.7:47900.
        #[arg(required_unless_present = "off")]
        addr: Option<String>,
        /// Server fingerprint, printed by `rider-server info`.
        #[arg(required_unless_present = "off")]
        fingerprint: Option<String>,
        /// Stop using a server (direct addresses only).
        #[arg(long, conflicts_with_all = ["addr", "fingerprint"])]
        off: bool,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,quinn=warn".into()),
        )
        .init();
    let cli = Cli::parse();
    let mut cfg = ClientConfig::load()?;
    let identity = Identity::load_or_create(&ClientConfig::dir()?)?;

    match cli.cmd {
        Cmd::List => {
            if cfg.hosts.is_empty() {
                println!("No paired hosts. Use `rider-client pair <address>`.");
            }
            for h in &cfg.hosts {
                let fp = fp_from_hex(&h.fingerprint).map(|f| fp_short(&f)).unwrap_or_default();
                let addr = if h.addr.is_empty() { "(via server)" } else { h.addr.as_str() };
                println!("{:<20} {:<28} {fp}", h.name, addr);
            }
        }
        Cmd::Forget { host } => {
            let before = cfg.hosts.len();
            let k = host.to_lowercase();
            cfg.hosts.retain(|h| h.name.to_lowercase() != k && h.addr != host);
            cfg.save()?;
            println!("Removed {} host(s).", before - cfg.hosts.len());
        }
        Cmd::Info => {
            println!("Device name:  {}", cfg.device_name);
            println!("Fingerprint:  {}", fp_short(&identity.fingerprint()));
            println!(
                "Server:       {}",
                cfg.server.as_ref().map_or("none (direct addresses only)", |s| s.addr.as_str())
            );
            println!("Config dir:   {}", ClientConfig::dir()?.display());
        }
        Cmd::SetServer { addr, fingerprint, off } => {
            cfg.server = match (off, addr, fingerprint) {
                (true, ..) => None,
                (false, Some(a), Some(f)) => Some(ServerRef::new(&a, &f)?),
                _ => bail!("give the server address and fingerprint, or --off"),
            };
            cfg.save()?;
            match &cfg.server {
                Some(s) => println!("Rendezvous server set to {}.", s.addr),
                None => println!("Rendezvous server disabled."),
            }
        }
        Cmd::Pair { addr, name } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(pair(&mut cfg, &identity, &addr, name))?;
        }
        Cmd::Connect { host, pin_stdin } => {
            let known = cfg
                .find(&host)
                .cloned()
                .with_context(|| format!("\"{host}\" is not paired. Run `rider-client pair <address>` first."))?;
            let rt = tokio::runtime::Runtime::new()?;
            let piped = if pin_stdin { Some(rider_core::secret_from_stdin()?) } else { None };
            let session = rt.block_on(connect(&cfg, &identity, &known, piped))?;
            app::run(rt, session, known.name)?;
        }
    }
    Ok(())
}

async fn resolve(addr: &str) -> Result<SocketAddr> {
    rendezvous::resolve(addr, rider_core::DEFAULT_PORT).await
}

/// Reach a host through the rendezvous server: get its candidate addresses,
/// punch NAT holes and race direct paths against the relay.
///
/// `pinned`: the host key we expect (`None` only while pairing by code; then
/// the key the server names is pinned and the verification code check catches
/// a lying server). `direct`: a known direct address to try as well.
async fn via_server(
    identity: &Identity,
    server: &ServerRef,
    target: Target,
    pinned: Option<rider_core::identity::Fingerprint>,
    direct: Option<SocketAddr>,
) -> Result<(quinn::Connection, Path)> {
    let socket = PeerSocket::bind(SocketAddr::from(([0, 0, 0, 0], 0)), None)?;
    let (rv, mut send, mut recv) = socket.connect_server(identity, server).await?;
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
    // Diagnostics: exercise the relay even when a direct path would work.
    if std::env::var_os("RIDER_FORCE_RELAY").is_some() {
        candidates.clear();
    }
    let cfg = rider_core::tls::client_config(identity, Some(pinned.unwrap_or(host)))?;
    let (result, ()) = tokio::join!(
        rendezvous::race(&socket.endpoint, cfg, &candidates, relay.map(|r| r.addr)),
        socket.punch(&candidates, relay.as_ref()),
    );
    result
}

async fn open(
    identity: &Identity,
    addr: &str,
    pinned: Option<rider_core::identity::Fingerprint>,
) -> Result<quinn::Connection> {
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

async fn hello(
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

async fn pair(cfg: &mut ClientConfig, identity: &Identity, target: &str, alias: Option<String>) -> Result<()> {
    let hint = "Is `rider-host pair` running on the host? Pairing must be enabled there.";
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
            let conn = open(identity, target, None).await.map_err(|e| anyhow::anyhow!("{e:#}\n{hint}"))?;
            (conn, target.to_owned())
        }
    };
    let host_fp = rider_core::tls::peer_fingerprint(&conn)?;
    let (mut send, mut recv, host_name) = hello(&conn, &cfg.device_name, true).await?;
    let code = rider_core::sas::compute(&conn, &host_fp, &identity.fingerprint())?;

    println!();
    println!("  Pairing with \"{host_name}\" at {}", conn.remote_address());
    println!();
    println!("  Verification code:   {code}");
    println!();
    println!("  The host screen shows a code too. They must be IDENTICAL.");
    println!("  If they differ, someone may be intercepting: answer N.");
    print!("  Codes match? [y/N] ");
    std::io::stdout().flush()?;
    let accepted = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        matches!(line.trim(), "y" | "Y" | "yes")
    })
    .await?;
    write_msg(&mut send, &ClientMsg::PairConfirm { accepted }).await?;
    if !accepted {
        bail!("pairing cancelled");
    }
    println!("  Waiting for the host operator to confirm...");
    match read_msg::<HostMsg>(&mut recv).await? {
        Some(HostMsg::PairResult { accepted: true }) => {}
        Some(HostMsg::PairResult { accepted: false }) => bail!("the host operator declined pairing"),
        Some(HostMsg::Bye { reason }) => bail!("host refused: {reason}"),
        _ => bail!("connection lost during pairing"),
    }
    let name = alias.unwrap_or(host_name);
    cfg.upsert(KnownHost { name: name.clone(), addr, fingerprint: fp_hex(&host_fp) });
    cfg.save()?;
    conn.close(0u32.into(), b"paired");
    println!("  Paired with \"{name}\" ({}).", fp_short(&host_fp));
    println!("  Connect any time with:  rider-client connect \"{name}\"");
    Ok(())
}

/// `piped`: PIN from `--pin-stdin`, tried once; otherwise up to three prompts.
async fn connect(
    cfg: &ClientConfig,
    identity: &Identity,
    host: &KnownHost,
    mut piped: Option<String>,
) -> Result<net::Session> {
    let attempts = if piped.is_some() { 1 } else { 3 };
    let pinned = fp_from_hex(&host.fingerprint)?;
    let conn = match &cfg.server {
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
            println!("Connected to \"{}\" ({path}).", host.name);
            conn
        }
        None if host.addr.is_empty() => {
            bail!("\"{}\" is only reachable via a rendezvous server: run `rider-client set-server`", host.name)
        }
        None => open(identity, &host.addr, Some(pinned)).await?,
    };
    let (mut send, mut recv, _) = hello(&conn, &cfg.device_name, false).await?;
    for attempt in 1..=attempts {
        let pin = match piped.take() {
            Some(pin) => pin,
            None => tokio::task::spawn_blocking(|| rpassword::prompt_password("PIN: ")).await??,
        };
        write_msg(&mut send, &ClientMsg::Auth { pin }).await?;
        match read_msg::<HostMsg>(&mut recv).await? {
            Some(HostMsg::AuthResult { ok: true, .. }) => return Ok(net::Session { conn, send, recv }),
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

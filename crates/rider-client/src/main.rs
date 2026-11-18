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
use std::io::Write;
use std::net::SocketAddr;

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
        /// Host address, e.g. 192.168.1.20 or office-pc.example.com:47800
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
                println!("{:<20} {:<28} {fp}", h.name, h.addr);
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
            println!("Config dir:   {}", ClientConfig::dir()?.display());
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
    let with_port =
        if addr.parse::<SocketAddr>().is_ok() || addr.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) {
            addr.to_string()
        } else {
            format!("{addr}:{}", rider_core::DEFAULT_PORT)
        };
    tokio::net::lookup_host(&with_port).await?.find(|a| a.is_ipv4()).with_context(|| format!("cannot resolve {addr}"))
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

async fn pair(cfg: &mut ClientConfig, identity: &Identity, addr: &str, alias: Option<String>) -> Result<()> {
    let conn = open(identity, addr, None).await.map_err(|e| {
        anyhow::anyhow!("{e:#}\nIs `rider-host pair` running on the host? Pairing must be enabled there.")
    })?;
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
    cfg.upsert(KnownHost { name: name.clone(), addr: addr.to_string(), fingerprint: fp_hex(&host_fp) });
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
    let conn = open(identity, &host.addr, Some(pinned)).await?;
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

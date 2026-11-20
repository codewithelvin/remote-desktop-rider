//! Remote Desktop Rider - client CLI (the viewer you control a host from).

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rider_client::config::ClientConfig;
use rider_core::identity::{Identity, fp_from_hex, fp_short};
use rider_core::rendezvous::ServerRef;
use std::io::Write;

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
    /// Quick connect: a one-time session by the code the other computer shows
    /// under "Get a one-time code". No pairing or PIN; the person there allows it.
    Quick {
        /// The code, e.g. SF6-CK6.
        code: String,
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
            let removed = cfg.forget(&host);
            cfg.save()?;
            println!("Removed {removed} host(s).");
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
            let host = rt.block_on(rider_client::pair(&mut cfg, &identity, &addr, name, async |p| {
                println!();
                println!("  Pairing with \"{}\" at {}", p.host_name, p.remote);
                println!();
                println!("  Verification code:   {}", p.code);
                println!();
                println!("  The host screen shows a code too. They must be IDENTICAL.");
                println!("  If they differ, someone may be intercepting: answer N.");
                print!("  Codes match? [y/N] ");
                let _ = std::io::stdout().flush();
                let ok = tokio::task::spawn_blocking(|| {
                    let mut line = String::new();
                    let _ = std::io::stdin().read_line(&mut line);
                    matches!(line.trim(), "y" | "Y" | "yes")
                })
                .await
                .unwrap_or(false);
                if ok {
                    println!("  Waiting for the host operator to confirm...");
                }
                ok
            }))?;
            println!("  Paired with \"{}\" ({}).", host.name, fp_short(&fp_from_hex(&host.fingerprint)?));
            println!("  Connect any time with:  rider-client connect \"{}\"", host.name);
        }
        Cmd::Quick { code } => {
            let rt = tokio::runtime::Runtime::new()?;
            let (session, route, host_name) = rt.block_on(rider_client::quick(&cfg, &identity, &code, |p| {
                // The desktop app reads these two lines.
                println!("Waiting for \"{}\" to allow the connection.", p.host_name);
                println!("Verification code: {}", p.code);
            }))?;
            println!("Connected to \"{host_name}\" ({route}).");
            let report_to =
                cfg.server.clone().map(|server| rider_client::diag::Target { identity: identity.clone(), server });
            rider_client::app::run(rt, session, host_name, report_to)?;
        }
        Cmd::Connect { host, pin_stdin } => {
            let known = cfg
                .find(&host)
                .cloned()
                .with_context(|| format!("\"{host}\" is not paired. Run `rider-client pair <address>` first."))?;
            let rt = tokio::runtime::Runtime::new()?;
            let piped = if pin_stdin { Some(rider_core::secret_from_stdin()?) } else { None };
            let (session, route) = rt.block_on(rider_client::connect(&cfg, &identity, &known, piped))?;
            cfg.touch(&known.fingerprint);
            if let Err(e) = cfg.save() {
                tracing::warn!("cannot save last-connected time: {e:#}");
            }
            // After saving: the desktop app reloads its list when it sees this line.
            println!("Connected to \"{}\" ({route}).", known.name);
            let report_to =
                cfg.server.clone().map(|server| rider_client::diag::Target { identity: identity.clone(), server });
            rider_client::app::run(rt, session, known.name, report_to)?;
        }
    }
    Ok(())
}

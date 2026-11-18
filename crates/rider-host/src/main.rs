//! Remote Desktop Rider - host agent (the machine being controlled).
//!
//! Modes:
//! * interactive: `rider-host run` / `rider-host pair` in an elevated terminal;
//! * unattended: `rider-host service install` registers a Windows service that
//!   keeps a SYSTEM agent (`rider-host agent`) running in the console session.

mod autoupdate;
mod banner;
mod capture;
mod config;
mod input;
mod rendezvous;
mod service;
mod session;
mod system;
mod ui;
mod video;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use config::HostConfig;
use rider_core::identity::{Identity, fp_from_hex, fp_short};
use session::{HostState, HostTrust};
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "rider-host", version, about = "Remote Desktop Rider host agent (run as administrator)")]
struct Cli {
    /// Machine-readable output for the desktop app (JSON lines; answers on stdin).
    #[arg(long, global = true, hide = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Accept connections from paired devices in this terminal (default).
    Run {
        /// Also accept ONE new device for pairing (window closes after 10 minutes).
        #[arg(long)]
        pair: bool,
    },
    /// Pair one new device, then exit. Pauses the background service meanwhile.
    Pair,
    /// Set or change the session PIN every connection must provide.
    SetPin {
        /// Read the PIN from one line of stdin instead of prompting.
        #[arg(long)]
        pin_stdin: bool,
    },
    /// List paired devices.
    Clients,
    /// Remove a paired device by name or fingerprint prefix.
    Revoke { device: String },
    /// Show this host's identity and settings.
    Info,
    /// Use a rendezvous server (rider-server) so clients can connect from other networks.
    SetServer {
        /// Server address, e.g. rider.example.com or 203.0.113.7:47900.
        #[arg(required_unless_present = "off")]
        addr: Option<String>,
        /// Server fingerprint, printed by `rider-server info`.
        #[arg(required_unless_present = "off")]
        fingerprint: Option<String>,
        /// Stop using a server (LAN / direct connections only).
        #[arg(long, conflicts_with_all = ["addr", "fingerprint"])]
        off: bool,
    },
    /// Let the service install new releases by itself when nobody is connected.
    AutoUpdate {
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
    /// Manage the unattended-access Windows service.
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
    /// Internal: the session agent launched by the service.
    #[command(hide = true)]
    Agent,
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// Install (or upgrade) and start the service. Works at the login screen and after reboots.
    Install,
    /// Stop and remove the service (pairings are kept).
    Uninstall,
    Start,
    Stop,
    Status,
    /// Internal: entry point used by the Service Control Manager.
    #[command(hide = true)]
    Run,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.json {
        ui::enable();
    }
    let cmd = cli.cmd.unwrap_or(Cmd::Run { pair: false });
    let headless = match &cmd {
        Cmd::Agent => Some("agent"),
        Cmd::Service { action: ServiceCmd::Run } => Some("service"),
        _ => None,
    };

    // The host's key and trust store are machine-wide and admin-only.
    system::require_elevated()?;
    let dir = HostConfig::init_dir()?;
    init_logging(headless.map(|name| dir.join("logs").join(format!("{name}.log"))))?;

    // Physical pixels everywhere: capture, monitor rects and SendInput must agree.
    // SAFETY: called once at startup, before any window is created.
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

    let mut cfg = HostConfig::load()?;
    match cmd {
        Cmd::Service { action } => match action {
            ServiceCmd::Run => service::dispatch()?,
            ServiceCmd::Install => {
                if !cfg.has_pin() {
                    bail!("set a PIN first: `rider-host set-pin`");
                }
                Identity::load_or_create(&dir)?;
                service::install(cfg.port)?;
            }
            ServiceCmd::Uninstall => service::uninstall()?,
            ServiceCmd::Start => service::set_running(true)?,
            ServiceCmd::Stop => service::set_running(false)?,
            ServiceCmd::Status => println!("{}", service_status()),
        },
        Cmd::SetPin { pin_stdin } => {
            let pin = if pin_stdin {
                rider_core::secret_from_stdin()?
            } else {
                let pin = rpassword::prompt_password("New PIN (min 6 characters): ")?;
                if rpassword::prompt_password("Repeat PIN: ")? != pin {
                    bail!("PINs do not match");
                }
                pin
            };
            if pin.chars().count() < 6 {
                bail!("PIN must be at least 6 characters");
            }
            cfg.set_pin(&pin)?;
            cfg.save()?;
            println!("PIN saved (a running host picks it up immediately).");
        }
        Cmd::Clients if ui::json() => {
            let list: Vec<_> = cfg
                .trusted_clients
                .iter()
                .map(|c| serde_json::json!({ "name": c.name, "fingerprint": c.fingerprint, "paired_at": c.paired_at }))
                .collect();
            println!("{}", serde_json::Value::Array(list));
        }
        Cmd::Clients => {
            if cfg.trusted_clients.is_empty() {
                println!("No paired devices. Run `rider-host pair` to add one.");
            }
            for c in &cfg.trusted_clients {
                let fp = fp_from_hex(&c.fingerprint).map(|f| fp_short(&f)).unwrap_or_default();
                println!("{:<24} {fp}", c.name);
            }
        }
        Cmd::Revoke { device } => {
            let needle = device.to_lowercase().replace('-', "");
            let before = cfg.trusted_clients.len();
            cfg.trusted_clients.retain(|c| c.name.to_lowercase() != needle && !c.fingerprint.starts_with(&needle));
            let removed = before - cfg.trusted_clients.len();
            cfg.save()?;
            println!("Removed {removed} device(s).");
        }
        Cmd::SetServer { addr, fingerprint, off } => {
            cfg.server = match (off, addr, fingerprint) {
                (true, ..) => None,
                (false, Some(a), Some(f)) => Some(rider_core::rendezvous::ServerRef::new(&a, &f)?),
                _ => bail!("give the server address and fingerprint, or --off"),
            };
            cfg.save()?;
            match &cfg.server {
                Some(s) => println!("Rendezvous server set to {} (a running host switches within seconds).", s.addr),
                None => println!("Rendezvous server disabled."),
            }
        }
        Cmd::AutoUpdate { state } => {
            cfg.auto_update = state == "on";
            cfg.save()?;
            println!(
                "Automatic updates {}.",
                if cfg.auto_update { "on: installed when nobody is connected" } else { "off" }
            );
        }
        Cmd::Info if ui::json() => {
            let identity = Identity::load_or_create(&dir)?;
            let info = serde_json::json!({
                "name": cfg.name,
                "fingerprint": rider_core::identity::fp_hex(&identity.fingerprint()),
                "port": cfg.port,
                "pin_set": cfg.has_pin(),
                "paired": cfg.trusted_clients.len(),
                "server": cfg.server,
                "auto_update": cfg.auto_update,
                "service": service::status(),
            });
            println!("{info}");
        }
        Cmd::Info => {
            let identity = Identity::load_or_create(&dir)?;
            println!("Host name:    {}", cfg.name);
            println!("Fingerprint:  {}", fp_short(&identity.fingerprint()));
            println!("Port (UDP):   {}", cfg.port);
            println!("PIN set:      {}", if cfg.has_pin() { "yes" } else { "NO - run `rider-host set-pin`" });
            println!("Paired:       {} device(s)", cfg.trusted_clients.len());
            println!("Server:       {}", cfg.server.as_ref().map_or("none (direct/LAN only)", |s| s.addr.as_str()));
            println!("Service:      {}", service_status());
            println!("State dir:    {}", dir.display());
        }
        Cmd::Pair => {
            let _paused = service::Paused::service()?;
            serve(cfg, Identity::load_or_create(&dir)?, Mode::PairOnce)?;
        }
        Cmd::Run { pair } => {
            if service::status() == Some(true) {
                bail!(
                    "the background service is running and already accepts connections.\n\
                     Use `rider-host pair` to add a device, or `rider-host service stop` first."
                );
            }
            let mode = if pair { Mode::Interactive { pairing: true } } else { Mode::Interactive { pairing: false } };
            serve(cfg, Identity::load_or_create(&dir)?, mode)?;
        }
        Cmd::Agent => {
            if !cfg.has_pin() {
                tracing::error!("no PIN set; the agent will not accept connections. Run `rider-host set-pin`.");
                // Avoid a tight restart loop in the supervisor.
                std::thread::sleep(std::time::Duration::from_secs(30));
                return Ok(());
            }
            serve(cfg, Identity::load_or_create(&dir)?, Mode::Agent)?;
        }
    }
    Ok(())
}

fn service_status() -> &'static str {
    match service::status() {
        Some(true) => "installed, running",
        Some(false) => "installed, stopped",
        None => "not installed",
    }
}

/// Console logging for interactive use; a file (capped at 10 MB) for the
/// service and agent, which have no console.
fn init_logging(file: Option<std::path::PathBuf>) -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,quinn=warn".into());
    match file {
        // JSON mode: stdout carries events for the app, so logs go to stderr.
        None if ui::json() => {
            tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false).with_writer(std::io::stderr).init()
        }
        None => tracing_subscriber::fmt().with_env_filter(filter).init(),
        Some(path) => {
            std::fs::create_dir_all(path.parent().context("log path has no parent")?)?;
            let truncate = std::fs::metadata(&path).is_ok_and(|m| m.len() > 10 * 1024 * 1024);
            let f = std::fs::OpenOptions::new()
                .create(true)
                .append(!truncate)
                .write(true)
                .truncate(truncate)
                .open(&path)?;
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(f))
                .init();
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Interactive {
        pairing: bool,
    },
    /// Pair one device and exit.
    PairOnce,
    /// Headless SYSTEM agent launched by the service.
    Agent,
}

fn serve(cfg: HostConfig, identity: Identity, mode: Mode) -> Result<()> {
    if !cfg.has_pin() {
        bail!("no PIN set. Run `rider-host set-pin` first - every session must provide it.");
    }
    let pairing = matches!(mode, Mode::PairOnce | Mode::Interactive { pairing: true });
    let port = cfg.port;
    let name = cfg.name.clone();
    let fp = fp_short(&identity.fingerprint());
    let server = cfg.server.clone();
    let state = Arc::new(HostState::new(cfg, identity, pairing));

    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async move {
        let trust = Arc::new(HostTrust(state.clone()));
        let server_cfg = rider_core::tls::server_config(&state.identity, trust)?;
        // One UDP socket for clients AND the rendezvous server: NAT hole
        // punching only works if both use the same mapping.
        let socket = Arc::new(
            rider_core::rendezvous::PeerSocket::bind(SocketAddr::from(([0, 0, 0, 0], port)), Some(server_cfg))
                .with_context(|| format!("cannot listen on UDP {port} (is another host running?)"))?,
        );
        let endpoint = socket.endpoint.clone();
        tokio::spawn(rendezvous::run(state.clone(), socket.clone(), mode != Mode::Agent));
        let quit = if ui::json() { ui::start_input() } else { Arc::new(tokio::sync::Notify::new()) };

        tracing::info!("host \"{name}\" listening on UDP {port}, fingerprint {fp}");
        ui::emit(serde_json::json!({
            "event": "listening",
            "name": name,
            "port": port,
            "fingerprint": fp,
            "addresses": rider_core::rendezvous::local_candidates(port),
            "server": server.as_ref().map(|s| &s.addr),
            "pairing": pairing,
        }));
        if mode != Mode::Agent && !ui::json() {
            println!("Remote Desktop Rider host \"{name}\"");
            println!("  listening on UDP {port}");
            println!("  host fingerprint {fp}");
            match &server {
                Some(s) => println!("  reachable from anywhere via {}", s.addr),
                None => println!("  no rendezvous server: reachable on this network only (`rider-host set-server`)"),
            }
            if pairing {
                println!("  PAIRING OPEN for 10 minutes. On the client run:");
                println!("    rider-client pair <this-ip>:{port}");
            }
            println!("Press Ctrl+C to stop.");
        }

        loop {
            tokio::select! {
                incoming = endpoint.accept() => {
                    let Some(incoming) = incoming else { break };
                    tokio::spawn(session::handle(state.clone(), incoming));
                }
                _ = state.paired.notified(), if mode == Mode::PairOnce => {
                    // Let the client receive the result before closing.
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    break;
                }
                _ = tokio::signal::ctrl_c() => break,
                _ = quit.notified() => break,
            }
        }
        endpoint.close(0u32.into(), b"host shutting down");
        endpoint.wait_idle().await;
        anyhow::Ok(())
    })
}

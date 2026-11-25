mod autoupdate;
mod banner;
mod capture;
mod chat;
mod company;
mod config;
mod cursor;
mod input;
mod panel;
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
    #[arg(long, global = true, hide = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    Run {
        #[arg(long)]
        pair: bool,
    },
    Pair,
    SetPin {
        #[arg(long)]
        pin_stdin: bool,
    },
    Clients,
    Revoke {
        device: String,
    },
    Info,
    SetServer {
        #[arg(required_unless_present = "off")]
        addr: Option<String>,
        fingerprint: Option<String>,
        #[arg(long)]
        invite: Option<String>,
        #[arg(long, conflicts_with_all = ["addr", "fingerprint", "invite"])]
        off: bool,
    },
    Invite,
    Enroll {
        token: String,
        #[arg(long)]
        unattended: bool,
    },
    LeaveCompany,
    Support {
        server: String,
        fingerprint: String,
    },
    Language {
        #[arg(value_parser = ["en", "az"])]
        lang: String,
    },
    AutoUpdate {
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
    #[command(hide = true)]
    Agent,
}

#[derive(Subcommand)]
enum ServiceCmd {
    Install,
    Uninstall,
    Start,
    Stop,
    Status,
    #[command(hide = true)]
    Run,
    #[command(hide = true)]
    ResumeAfter {
        pid: u32,
    },
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

    if let Cmd::Support { server, fingerprint } = cmd {
        init_logging(None)?;
        dpi_aware();
        let cfg = HostConfig {
            server: Some(rider_core::rendezvous::ServerRef::new(&server, &fingerprint)?),
            port: 0,
            auto_update: false,
            ..HostConfig::default()
        };
        return serve(cfg, Identity::ephemeral()?, Mode::Support);
    }

    system::require_elevated()?;
    let dir = HostConfig::init_dir()?;
    init_logging(headless.map(|name| dir.join("logs").join(format!("{name}.log"))))?;
    dpi_aware();

    let mut cfg = HostConfig::load()?;
    if let Some(lang) = cfg.language.as_deref().and_then(rider_core::i18n::Lang::from_code) {
        rider_core::i18n::set_current(lang);
    }
    match cmd {
        Cmd::Service { action } => match action {
            ServiceCmd::Run => service::dispatch()?,
            ServiceCmd::Install => {
                if !cfg.can_accept() {
                    bail!("set a PIN first: `rider-host set-pin`");
                }
                Identity::load_or_create(&dir)?;
                service::install(cfg.port)?;
            }
            ServiceCmd::Uninstall => service::uninstall()?,
            ServiceCmd::Start => service::set_running(true)?,
            ServiceCmd::Stop => service::set_running(false)?,
            ServiceCmd::Status => println!("{}", service_status()),
            ServiceCmd::ResumeAfter { pid } => service::resume_after(pid)?,
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
                let fp = fp_from_hex(&c.fingerprint).map_or_default(|f| fp_short(&f));
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
        Cmd::SetServer { addr, fingerprint, invite, off } => {
            use rider_core::rendezvous::{self, ServerRef};
            cfg.server = match (off, addr) {
                (true, _) => None,
                (false, Some(a)) => {
                    let server = match fingerprint {
                        Some(f) => ServerRef::new(&a, &f)?,
                        None => ServerRef::discover(&a)?,
                    };
                    if let Some(code) = invite {
                        let identity = Identity::load_or_create(&dir)?;
                        tokio::runtime::Runtime::new()?.block_on(rendezvous::join(&identity, &server, &code))?;
                        println!("Joined {} with the invite code.", server.addr);
                    }
                    Some(server)
                }
                _ => bail!("give the server address, or --off"),
            };
            cfg.save()?;
            match &cfg.server {
                Some(s) => println!("Rendezvous server set to {} (a running host switches within seconds).", s.addr),
                None => println!("Rendezvous server disabled."),
            }
        }
        Cmd::Invite => {
            let server = cfg.server.clone().context("no server is set")?;
            let identity = Identity::load_or_create(&dir)?;
            let (code, minutes) =
                tokio::runtime::Runtime::new()?.block_on(rider_core::rendezvous::invite(&identity, &server))?;
            if ui::json() {
                println!("{}", serde_json::json!({ "code": code, "minutes": minutes }));
            } else {
                println!("Invite code: {code} (valid {minutes} minutes, for one computer)");
            }
        }
        Cmd::Enroll { token, unattended } => {
            let identity = Identity::load_or_create(&dir)?;
            let company = company::enroll(&mut cfg, &identity, &token, unattended)?;
            println!("Joined \"{company}\": the admin sees this computer under Unassigned.");
        }
        Cmd::LeaveCompany => match cfg.company.take() {
            Some(c) => {
                cfg.save()?;
                println!("Left \"{}\": its admin can no longer connect.", c.name);
            }
            None => println!("This computer is not in a company."),
        },
        Cmd::Support { .. } => unreachable!("handled above"),
        Cmd::Language { lang } => {
            cfg.language = Some(lang);
            cfg.save()?;
            println!("Banner language saved (applies from the next session).");
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
                "language": cfg.language,
                "service": service::status(),
            });
            println!("{info}");
        }
        Cmd::Info => {
            let identity = Identity::load_or_create(&dir)?;
            println!("Host name:    {}", cfg.name);
            println!("Fingerprint:  {}", fp_short(&identity.fingerprint()));
            println!("Port (UDP):   {}", cfg.port);
            let pin = match (cfg.has_pin(), cfg.company.is_some()) {
                (true, _) => "yes",
                (false, true) => "no (not needed: the company's admin is accepted, not a PIN)",
                (false, false) => "NO - run `rider-host set-pin`",
            };
            println!("PIN set:      {pin}");
            println!("Paired:       {} device(s)", cfg.trusted_clients.len());
            println!("Server:       {}", cfg.server.as_ref().map_or("none (direct/LAN only)", |s| s.addr.as_str()));
            if let Some(c) = &cfg.company {
                let unattended = if c.unattended { ", unattended-capable" } else { "" };
                println!("Company:      {}{unattended}", c.name);
            }
            println!("Service:      {}", service_status());
            println!("State dir:    {}", dir.display());
            for m in capture::monitors().unwrap_or_default() {
                let primary = if m.info.primary { ", primary" } else { "" };
                println!(
                    "Monitor {}:    {} ({}x{}{primary})",
                    m.info.index + 1,
                    m.info.name,
                    m.info.width,
                    m.info.height
                );
            }
            let w = company::whereabouts();
            let via = if w.via.is_empty() { String::new() } else { format!(" (via {})", w.via) };
            println!("Place:        {:?}{via}", w.place);
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
            if !cfg.can_accept() {
                tracing::error!("no PIN set; the agent will not accept connections. Run `rider-host set-pin`.");
                std::thread::sleep(std::time::Duration::from_secs(30));
                return Ok(());
            }
            serve(cfg, Identity::load_or_create(&dir)?, Mode::Agent)?;
        }
    }
    Ok(())
}

fn dpi_aware() {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }
}

fn service_status() -> &'static str {
    match service::status() {
        Some(true) => "installed, running",
        Some(false) => "installed, stopped",
        None => "not installed",
    }
}

fn init_logging(file: Option<std::path::PathBuf>) -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,quinn=warn".into());
    match file {
        None if ui::json() => {
            tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false).with_writer(std::io::stderr).init();
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
    Interactive { pairing: bool },
    PairOnce,
    Agent,
    Support,
}

fn serve(cfg: HostConfig, identity: Identity, mode: Mode) -> Result<()> {
    let support = mode == Mode::Support;
    if !cfg.can_accept() && !support {
        bail!("no PIN set. Run `rider-host set-pin` first - every session must provide it.");
    }
    let pairing = matches!(mode, Mode::PairOnce | Mode::Interactive { pairing: true } | Mode::Support);
    let port = cfg.port;
    let name = cfg.name.clone();
    let fp = fp_short(&identity.fingerprint());
    let server = cfg.server.clone();
    let state =
        Arc::new(if support { HostState::support(cfg, identity) } else { HostState::new(cfg, identity, pairing) });

    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async move {
        let trust = Arc::new(HostTrust(state.clone()));
        let server_cfg = if support {
            rider_core::tls::support_server_config(&state.identity, trust)?
        } else {
            rider_core::tls::server_config(&state.identity, trust)?
        };
        let socket = Arc::new(
            rider_core::rendezvous::PeerSocket::bind(SocketAddr::from(([0, 0, 0, 0], port)), Some(server_cfg.clone()))
                .with_context(|| format!("cannot listen on UDP {port} (is another host running?)"))?,
        );
        let endpoint = socket.endpoint.clone();
        let mut sockets = vec![socket.clone()];
        match rider_core::rendezvous::PeerSocket::bind(SocketAddr::from(([0, 0, 0, 0], 0)), Some(server_cfg.clone())) {
            Ok(s) => sockets.push(Arc::new(s)),
            Err(e) => tracing::warn!("no second UDP socket: {e:#}"),
        }
        let wan = sockets.get(1).map(|s| s.endpoint.clone());
        tokio::spawn(rendezvous::run(state.clone(), sockets, server_cfg, mode != Mode::Agent));
        let quit = if ui::json() { ui::start_input() } else { Arc::new(tokio::sync::Notify::new()) };

        tracing::info!("host \"{name}\" listening on UDP {port}, fingerprint {fp}");
        ui::emit(&serde_json::json!({
            "event": "listening",
            "name": name,
            "port": port,
            "fingerprint": fp,
            "addresses": rider_core::rendezvous::local_candidates(port),
            "server": server.as_ref().map(|s| &s.addr),
            "pairing": pairing,
            "support": support,
        }));
        if support && !ui::json() {
            println!("Waiting for the one-time code from {}…", server.as_ref().map_or("", |s| s.addr.as_str()));
        } else if mode != Mode::Agent && !ui::json() {
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

        let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            tokio::select! {
                () = state.done.notified(), if support => {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    break;
                }
                _ = tick.tick(), if support => {
                    if state.pairing_expired() && !state.is_busy() {
                        ui::emit(&serde_json::json!({ "event": "expired" }));
                        if !ui::json() {
                            println!("The code expired.");
                        }
                        break;
                    }
                }
                incoming = endpoint.accept() => {
                    let Some(incoming) = incoming else { break };
                    tokio::spawn(session::handle(state.clone(), incoming, false));
                }
                Some(incoming) = async { wan.as_ref()?.accept().await }, if wan.is_some() => {
                    tokio::spawn(session::handle(state.clone(), incoming, false));
                }
                () = state.paired.notified(), if mode == Mode::PairOnce => {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    break;
                }
                _ = tokio::signal::ctrl_c() => break,
                () = quit.notified() => {
                    if support && state.is_busy() {
                        banner::DISCONNECT_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), state.done.notified()).await;
                    }
                    break;
                }
            }
        }
        endpoint.close(0u32.into(), b"host shutting down");
        if let Some(w) = &wan {
            w.close(0u32.into(), b"host shutting down");
        }
        endpoint.wait_idle().await;
        anyhow::Ok(())
    })
}

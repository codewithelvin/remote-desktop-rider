mod autoupdate;
mod banner;
mod capture;
mod chat;
mod commands;
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
use rider_core::identity::{Identity, fp_short};
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
    Tunnels {
        #[arg(value_parser = ["ask", "allow", "off"])]
        policy: String,
    },
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
    #[command(hide = true)]
    Agent,
}

#[derive(Subcommand)]
pub enum ServiceCmd {
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
    if let Cmd::Support { server, fingerprint } = cmd {
        return support(&server, &fingerprint);
    }
    let headless = match &cmd {
        Cmd::Agent => Some("agent"),
        Cmd::Service { action: ServiceCmd::Run } => Some("service"),
        _ => None,
    };
    system::require_elevated()?;
    let dir = HostConfig::init_dir()?;
    init_logging(headless.map(|name| dir.join("logs").join(format!("{name}.log"))))?;
    dpi_aware();
    let cfg = HostConfig::load()?;
    if let Some(lang) = cfg.language.as_deref().and_then(rider_core::i18n::Lang::from_code) {
        rider_core::i18n::set_current(lang);
    }
    dispatch(cmd, cfg, &dir)
}

fn support(server: &str, fingerprint: &str) -> Result<()> {
    init_logging(None)?;
    dpi_aware();
    let cfg = HostConfig {
        server: Some(rider_core::rendezvous::ServerRef::new(server, fingerprint)?),
        port: 0,
        auto_update: false,
        ..HostConfig::default()
    };
    serve(cfg, Identity::ephemeral()?, Mode::Support)
}

fn dispatch(cmd: Cmd, mut cfg: HostConfig, dir: &std::path::Path) -> Result<()> {
    match cmd {
        Cmd::Service { action } => commands::service(&cfg, dir, &action),
        Cmd::SetPin { pin_stdin } => commands::set_pin(&mut cfg, pin_stdin),
        Cmd::Clients => {
            commands::clients(&cfg);
            Ok(())
        }
        Cmd::Revoke { device } => commands::revoke(&mut cfg, &device),
        Cmd::SetServer { addr, fingerprint, invite, off } => {
            commands::set_server(&mut cfg, dir, commands::ServerChoice { addr, fingerprint, invite, off })
        }
        Cmd::Invite => commands::invite(&cfg, dir),
        Cmd::Enroll { token, unattended } => commands::enroll(&mut cfg, dir, &token, unattended),
        Cmd::LeaveCompany => commands::leave_company(&mut cfg),
        Cmd::Support { server, fingerprint } => support(&server, &fingerprint),
        Cmd::Language { lang } => commands::language(&mut cfg, lang),
        Cmd::AutoUpdate { state } => commands::auto_update(&mut cfg, &state),
        Cmd::Tunnels { policy } => commands::tunnels(&mut cfg, &policy),
        Cmd::Info => commands::info(&cfg, dir),
        Cmd::Pair => commands::pair(cfg, dir),
        Cmd::Run { pair } => commands::run(cfg, dir, pair),
        Cmd::Agent => commands::agent(cfg, dir),
    }
}

fn dpi_aware() {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
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
pub enum Mode {
    Interactive { pairing: bool },
    PairOnce,
    Agent,
    Support,
}

fn announce(
    name: &str,
    port: u16,
    fp: &str,
    server: Option<&rider_core::rendezvous::ServerRef>,
    pairing: bool,
    mode: Mode,
) {
    let support = mode == Mode::Support;
    tracing::info!("host \"{name}\" listening on UDP {port}, fingerprint {fp}");
    ui::emit(&serde_json::json!({
        "event": "listening",
        "name": name,
        "port": port,
        "fingerprint": fp,
        "addresses": rider_core::rendezvous::local_candidates(port),
        "server": server.map(|s| &s.addr),
        "pairing": pairing,
        "support": support,
    }));
    if support && !ui::json() {
        println!("Waiting for the one-time code from {}…", server.map_or("", |s| s.addr.as_str()));
    } else if mode != Mode::Agent && !ui::json() {
        println!("Remote Desktop Rider host \"{name}\"");
        println!("  listening on UDP {port}");
        println!("  host fingerprint {fp}");
        match server {
            Some(s) => println!("  reachable from anywhere via {}", s.addr),
            None => println!("  no rendezvous server: reachable on this network only (`rider-host set-server`)"),
        }
        if pairing {
            println!("  PAIRING OPEN for 10 minutes. On the client run:");
            println!("    rider-client pair <this-ip>:{port}");
        }
        println!("Press Ctrl+C to stop.");
    }
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

        announce(&name, port, &fp, server.as_ref(), pairing, mode);

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

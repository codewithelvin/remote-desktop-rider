//! Remote Desktop Rider - host agent (the machine being controlled).

mod banner;
mod capture;
mod config;
mod input;
mod session;
mod video;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use config::HostConfig;
use rider_core::identity::{Identity, fp_from_hex, fp_short};
use session::{HostState, HostTrust};
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "rider-host", version, about = "Remote Desktop Rider host agent")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Accept connections from paired devices (default).
    Run {
        /// Also accept ONE new device for pairing (window closes after 10 minutes).
        #[arg(long)]
        pair: bool,
    },
    /// Same as `run --pair`.
    Pair,
    /// Set or change the session PIN every connection must provide.
    SetPin,
    /// List paired devices.
    Clients,
    /// Remove a paired device by name or fingerprint prefix.
    Revoke { device: String },
    /// Show this host's identity and settings.
    Info,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,quinn=warn".into()),
        )
        .init();
    // Physical pixels everywhere: capture, monitor rects and SendInput must agree.
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

    let cli = Cli::parse();
    let mut cfg = HostConfig::load()?;
    let identity = Identity::load_or_create(&HostConfig::dir()?)?;

    match cli.cmd.unwrap_or(Cmd::Run { pair: false }) {
        Cmd::SetPin => {
            let pin = rpassword::prompt_password("New PIN (min 6 characters): ")?;
            if pin.chars().count() < 6 {
                bail!("PIN must be at least 6 characters");
            }
            if rpassword::prompt_password("Repeat PIN: ")? != pin {
                bail!("PINs do not match");
            }
            cfg.set_pin(&pin)?;
            cfg.save()?;
            println!("PIN saved.");
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
        Cmd::Info => {
            println!("Host name:    {}", cfg.name);
            println!("Fingerprint:  {}", fp_short(&identity.fingerprint()));
            println!("Port (UDP):   {}", cfg.port);
            println!("PIN set:      {}", if cfg.has_pin() { "yes" } else { "NO - run `rider-host set-pin`" });
            println!("Paired:       {} device(s)", cfg.trusted_clients.len());
            println!("Config dir:   {}", HostConfig::dir()?.display());
        }
        Cmd::Pair => serve(cfg, identity, true)?,
        Cmd::Run { pair } => serve(cfg, identity, pair)?,
    }
    Ok(())
}

fn serve(cfg: HostConfig, identity: Identity, pairing: bool) -> Result<()> {
    if !cfg.has_pin() {
        bail!("no PIN set. Run `rider-host set-pin` first - every session must provide it.");
    }
    let port = cfg.port;
    let name = cfg.name.clone();
    let fp = fp_short(&identity.fingerprint());
    let state = Arc::new(HostState::new(cfg, identity, pairing));

    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async move {
        let trust = Arc::new(HostTrust(state.clone()));
        let server_cfg = rider_core::tls::server_config(&state.identity, trust)?;
        let endpoint = quinn::Endpoint::server(server_cfg, SocketAddr::from(([0, 0, 0, 0], port)))?;

        println!("Remote Desktop Rider host \"{name}\"");
        println!("  listening on UDP {port}");
        println!("  host fingerprint {fp}");
        if pairing {
            println!("  PAIRING OPEN for 10 minutes. On the client run:");
            println!("    rider-client pair <this-ip>:{port}");
        }
        println!("Press Ctrl+C to stop.");

        loop {
            tokio::select! {
                incoming = endpoint.accept() => {
                    let Some(incoming) = incoming else { break };
                    tokio::spawn(session::handle(state.clone(), incoming));
                }
                _ = tokio::signal::ctrl_c() => break,
            }
        }
        endpoint.close(0u32.into(), b"host shutting down");
        anyhow::Ok(())
    })
}

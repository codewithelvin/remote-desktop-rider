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
    Pair {
        addr: String,
        #[arg(long)]
        name: Option<String>,
    },
    Connect {
        host: String,
        #[arg(long)]
        pin_stdin: bool,
    },
    Quick {
        code: String,
    },
    CompanyConnect {
        fingerprint: String,
    },
    Company {
        #[command(subcommand)]
        action: CompanyCmd,
    },
    List,
    Forget {
        host: String,
    },
    Info,
    SetServer {
        #[arg(required_unless_present = "off")]
        addr: Option<String>,
        fingerprint: Option<String>,
        #[arg(long, conflicts_with_all = ["addr", "fingerprint"])]
        off: bool,
    },
}

#[derive(Subcommand)]
enum CompanyCmd {
    SignIn {
        code: String,
    },
    Token,
    List,
    Unattended {
        fingerprint: String,
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
}

fn company(cfg: &mut ClientConfig, identity: &Identity, action: CompanyCmd) -> Result<()> {
    use rider_core::company::{DirEdit, JoinToken};
    let server = cfg.server.clone().context("no server: run `rider-client set-server` first")?;
    let rt = tokio::runtime::Runtime::new()?;
    match action {
        CompanyCmd::SignIn { code } => {
            let (company, join_secret) = rt.block_on(rider_core::rendezvous::admin_claim(identity, &server, &code))?;
            println!("Signed in: this device is the admin of \"{company}\".");
            cfg.enterprise = Some(rider_client::config::EnterpriseAdmin { company, join_secret });
            cfg.save()?;
        }
        CompanyCmd::Token => {
            let admin = cfg.enterprise.as_ref().context("not signed in: `rider-client company sign-in <code>`")?;
            let token = JoinToken {
                server: server.addr.clone(),
                server_fingerprint: fp_from_hex(&server.fingerprint)?,
                admin: identity.fingerprint(),
                secret: admin.join_secret.clone(),
            };
            println!("{}", token.encode());
        }
        CompanyCmd::List => {
            let dir = rt.block_on(rider_core::rendezvous::directory(identity, &server, None))?;
            let list: Vec<_> = dir
                .computers
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "fingerprint": rider_core::identity::fp_hex(&c.fingerprint),
                        "name": c.info.name,
                        "user": c.info.user,
                        "online": c.online,
                        "room": c.room,
                        "unattended": c.unattended(),
                    })
                })
                .collect();
            println!("{}", serde_json::Value::Array(list));
        }
        CompanyCmd::Unattended { fingerprint, state } => {
            let edit = DirEdit::Unattended { computer: fp_from_hex(&fingerprint)?, on: state == "on" };
            rt.block_on(rider_core::rendezvous::directory(identity, &server, Some(edit)))?;
            println!("Unattended {state}.");
        }
    }
    Ok(())
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
                (false, Some(a), None) => Some(ServerRef::discover(&a)?),
                _ => bail!("give the server address, or --off"),
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
                println!("Waiting for \"{}\" to allow the connection.", p.host_name);
                println!("Verification code: {}", p.code);
            }))?;
            println!("Connected to \"{host_name}\" ({route}).");
            let report_to =
                cfg.server.clone().map(|server| rider_client::diag::Target { identity: identity.clone(), server });
            rider_client::app::run(rt, session, host_name, report_to, None)?;
        }
        Cmd::Company { action } => company(&mut cfg, &identity, action)?,
        Cmd::CompanyConnect { fingerprint } => {
            let host = fp_from_hex(&fingerprint)?;
            let rt = tokio::runtime::Runtime::new()?;
            let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
            std::thread::spawn(move || {
                for line in std::io::stdin().lines() {
                    if line.is_ok_and(|l| l.trim() == "cancel") {
                        let _ = cancel_tx.send(());
                        return;
                    }
                }
            });
            let cancel = async move {
                if cancel_rx.await.is_err() {
                    std::future::pending::<()>().await;
                }
            };
            let (session, route, host_name) = rt.block_on(rider_client::company_connect(
                &cfg,
                &identity,
                host,
                |name| {
                    println!("Waiting for \"{name}\" to accept.");
                },
                cancel,
            ))?;
            println!("Connected to \"{host_name}\" ({route}).");
            let report_to =
                cfg.server.clone().map(|server| rider_client::diag::Target { identity: identity.clone(), server });
            rider_client::app::run(rt, session, host_name, report_to, None)?;
        }
        Cmd::Connect { host, pin_stdin } => {
            let known = cfg
                .find(&host)
                .cloned()
                .with_context(|| format!("\"{host}\" is not paired. Run `rider-client pair <address>` first."))?;
            let rt = tokio::runtime::Runtime::new()?;
            let piped = if pin_stdin { Some(rider_core::secret_from_stdin()?) } else { None };
            let reconnect = piped.clone().map(|pin| {
                let (cfg, identity, known) = (cfg.clone(), identity.clone(), known.clone());
                Box::new(move || {
                    let (cfg, identity, known, pin) = (cfg.clone(), identity.clone(), known.clone(), pin.clone());
                    Box::pin(async move { Ok(rider_client::connect(&cfg, &identity, &known, Some(pin)).await?.0) })
                        as std::pin::Pin<Box<dyn std::future::Future<Output = Result<_>> + Send>>
                }) as rider_client::app::Reconnect
            });
            let (session, route) = rt.block_on(rider_client::connect(&cfg, &identity, &known, piped))?;
            cfg.touch(&known.fingerprint);
            if let Err(e) = cfg.save() {
                tracing::warn!("cannot save last-connected time: {e:#}");
            }
            println!("Connected to \"{}\" ({route}).", known.name);
            let report_to =
                cfg.server.clone().map(|server| rider_client::diag::Target { identity: identity.clone(), server });
            rider_client::app::run(rt, session, known.name, report_to, reconnect)?;
        }
    }
    Ok(())
}

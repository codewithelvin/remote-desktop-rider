use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rider_client::PairPrompt;
use rider_client::app::Reconnect;
use rider_client::config::{ClientConfig, KnownHost};
use rider_client::forward::Spec;
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
        #[arg(long = "forward", value_name = "[LOCAL_PORT:]HOST:PORT")]
        forwards: Vec<Spec>,
    },
    Quick {
        code: String,
        #[arg(long = "forward", value_name = "[LOCAL_PORT:]HOST:PORT")]
        forwards: Vec<Spec>,
    },
    CompanyConnect {
        fingerprint: String,
        #[arg(long = "forward", value_name = "[LOCAL_PORT:]HOST:PORT")]
        forwards: Vec<Spec>,
    },
    Company {
        #[command(subcommand)]
        action: CompanyCmd,
    },
    List,
    Forget {
        host: String,
    },
    Wake {
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
        Cmd::List => list(&cfg),
        Cmd::Forget { host } => {
            let removed = cfg.forget(&host);
            cfg.save()?;
            println!("Removed {removed} host(s).");
        }
        Cmd::Wake { host } => wake(&cfg, &identity, &host)?,
        Cmd::Info => info(&cfg, &identity)?,
        Cmd::SetServer { addr, fingerprint, off } => set_server(&mut cfg, addr, fingerprint, off)?,
        Cmd::Pair { addr, name } => pair(&mut cfg, &identity, &addr, name)?,
        Cmd::Quick { code, forwards } => quick(&cfg, &identity, &code, forwards)?,
        Cmd::Company { action } => company(&mut cfg, &identity, action)?,
        Cmd::CompanyConnect { fingerprint, forwards } => company_connect(&cfg, &identity, &fingerprint, forwards)?,
        Cmd::Connect { host, pin_stdin, forwards } => connect(&mut cfg, &identity, &host, pin_stdin, forwards)?,
    }
    Ok(())
}

fn list(cfg: &ClientConfig) {
    if cfg.hosts.is_empty() {
        println!("No paired hosts. Use `rider-client pair <address>`.");
    }
    for h in &cfg.hosts {
        let fp = fp_from_hex(&h.fingerprint).map_or_default(|f| fp_short(&f));
        let addr = if h.addr.is_empty() { "(via server)" } else { h.addr.as_str() };
        println!("{:<20} {:<28} {fp}", h.name, addr);
    }
}

fn info(cfg: &ClientConfig, identity: &Identity) -> Result<()> {
    println!("Device name:  {}", cfg.device_name);
    println!("Fingerprint:  {}", fp_short(&identity.fingerprint()));
    println!("Server:       {}", cfg.server.as_ref().map_or("none (direct addresses only)", |s| s.addr.as_str()));
    println!("Config dir:   {}", ClientConfig::dir()?.display());
    Ok(())
}

fn set_server(cfg: &mut ClientConfig, addr: Option<String>, fingerprint: Option<String>, off: bool) -> Result<()> {
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
    Ok(())
}

async fn confirm_code(p: &PairPrompt) -> bool {
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
}

fn pair(cfg: &mut ClientConfig, identity: &Identity, addr: &str, name: Option<String>) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    let host = rt.block_on(rider_client::pair(cfg, identity, addr, name, async |p| confirm_code(p).await))?;
    println!("  Paired with \"{}\" ({}).", host.name, fp_short(&fp_from_hex(&host.fingerprint)?));
    println!("  Connect any time with:  rider-client connect \"{}\"", host.name);
    Ok(())
}

fn report_target(cfg: &ClientConfig, identity: &Identity) -> Option<rider_client::diag::Target> {
    cfg.server.clone().map(|server| rider_client::diag::Target { identity: identity.clone(), server })
}

fn quick(cfg: &ClientConfig, identity: &Identity, code: &str, forwards: Vec<Spec>) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    let (session, route, host_name) = rt.block_on(rider_client::quick(cfg, identity, code, |p| {
        println!("Waiting for \"{}\" to allow the connection.", p.host_name);
        println!("Verification code: {}", p.code);
    }))?;
    println!("Connected to \"{host_name}\" ({route}).");
    rider_client::app::run(rt, session, host_name, report_target(cfg, identity), None, forwards)
}

fn cancel_on_stdin() -> impl Future<Output = ()> {
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            if line.is_ok_and(|l| l.trim() == "cancel") {
                let _ = cancel_tx.send(());
                return;
            }
        }
    });
    async move {
        if cancel_rx.await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

fn company_connect(cfg: &ClientConfig, identity: &Identity, fingerprint: &str, forwards: Vec<Spec>) -> Result<()> {
    let host = fp_from_hex(fingerprint)?;
    let rt = tokio::runtime::Runtime::new()?;
    let waiting = |name: &str| println!("Waiting for \"{name}\" to accept.");
    let (session, route, host_name) =
        rt.block_on(rider_client::company_connect(cfg, identity, host, waiting, cancel_on_stdin()))?;
    println!("Connected to \"{host_name}\" ({route}).");
    rider_client::app::run(rt, session, host_name, report_target(cfg, identity), None, forwards)
}

fn reconnect_with(cfg: &ClientConfig, identity: &Identity, known: &KnownHost, pin: String) -> Reconnect {
    let (cfg, identity, known) = (cfg.clone(), identity.clone(), known.clone());
    Box::new(move || {
        let (cfg, identity, known, pin) = (cfg.clone(), identity.clone(), known.clone(), pin.clone());
        Box::pin(async move { Ok(rider_client::connect(&cfg, &identity, &known, Some(pin)).await?.0) })
            as std::pin::Pin<Box<dyn Future<Output = Result<_>> + Send>>
    })
}

fn wake(cfg: &ClientConfig, identity: &Identity, host: &str) -> Result<()> {
    let known = cfg.find(host).with_context(|| format!("\"{host}\" is not paired"))?;
    let fp = fp_from_hex(&known.fingerprint)?;
    let rt = tokio::runtime::Runtime::new()?;
    let outcome = rt.block_on(rider_client::wake(identity, cfg.server.as_ref(), fp, &known.wake_macs()))?;
    match outcome {
        rider_client::WakeOutcome::Helpers(n) => {
            println!("Wake-up sent here and through {n} computer(s) on its network.");
        }
        rider_client::WakeOutcome::ServerTooOld => {
            println!("Wake-up sent on this network only: the server is too old to pass it on.");
        }
        rider_client::WakeOutcome::NoServer => println!("Wake-up sent on this network."),
    }
    Ok(())
}

fn connect(
    cfg: &mut ClientConfig,
    identity: &Identity,
    host: &str,
    pin_stdin: bool,
    forwards: Vec<Spec>,
) -> Result<()> {
    let known = cfg
        .find(host)
        .cloned()
        .with_context(|| format!("\"{host}\" is not paired. Run `rider-client pair <address>` first."))?;
    let rt = tokio::runtime::Runtime::new()?;
    let piped = if pin_stdin { Some(rider_core::secret_from_stdin()?) } else { None };
    let reconnect = piped.clone().map(|pin| reconnect_with(cfg, identity, &known, pin));
    let (session, route) = rt.block_on(rider_client::connect(cfg, identity, &known, piped))?;
    cfg.touch(&known.fingerprint);
    if let Err(e) = cfg.save() {
        tracing::warn!("cannot save last-connected time: {e:#}");
    }
    println!("Connected to \"{}\" ({route}).", known.name);
    rider_client::app::run(rt, session, known.name, report_target(cfg, identity), reconnect, forwards)
}

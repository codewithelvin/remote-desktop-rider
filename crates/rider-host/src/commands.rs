use crate::config::HostConfig;
use crate::{Mode, ServiceCmd, capture, company, serve, service, ui};
use anyhow::{Context, Result, bail};
use rider_core::identity::{Identity, fp_from_hex, fp_short};
use rider_core::rendezvous::{self, ServerRef};
use std::path::Path;

pub fn service(cfg: &HostConfig, dir: &Path, action: &ServiceCmd) -> Result<()> {
    match action {
        ServiceCmd::Run => service::dispatch()?,
        ServiceCmd::Install => {
            if !cfg.can_accept() {
                bail!("set a PIN first: `rider-host set-pin`");
            }
            Identity::load_or_create(dir)?;
            service::install(cfg.port)?;
        }
        ServiceCmd::Uninstall => service::uninstall()?,
        ServiceCmd::Start => service::set_running(true)?,
        ServiceCmd::Stop => service::set_running(false)?,
        ServiceCmd::Status => println!("{}", service_status()),
        ServiceCmd::ResumeAfter { pid } => service::resume_after(*pid)?,
    }
    Ok(())
}

pub fn service_status() -> &'static str {
    match service::status() {
        Some(true) => "installed, running",
        Some(false) => "installed, stopped",
        None => "not installed",
    }
}

pub fn set_pin(cfg: &mut HostConfig, pin_stdin: bool) -> Result<()> {
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
    Ok(())
}

pub fn clients(cfg: &HostConfig) {
    if ui::json() {
        let list: Vec<_> = cfg
            .trusted_clients
            .iter()
            .map(|c| serde_json::json!({ "name": c.name, "fingerprint": c.fingerprint, "paired_at": c.paired_at }))
            .collect();
        println!("{}", serde_json::Value::Array(list));
        return;
    }
    if cfg.trusted_clients.is_empty() {
        println!("No paired devices. Run `rider-host pair` to add one.");
    }
    for c in &cfg.trusted_clients {
        let fp = fp_from_hex(&c.fingerprint).map_or_default(|f| fp_short(&f));
        println!("{:<24} {fp}", c.name);
    }
}

pub fn revoke(cfg: &mut HostConfig, device: &str) -> Result<()> {
    let needle = device.to_lowercase().replace('-', "");
    let before = cfg.trusted_clients.len();
    cfg.trusted_clients.retain(|c| c.name.to_lowercase() != needle && !c.fingerprint.starts_with(&needle));
    let removed = before - cfg.trusted_clients.len();
    cfg.save()?;
    println!("Removed {removed} device(s).");
    Ok(())
}

pub struct ServerChoice {
    pub addr: Option<String>,
    pub fingerprint: Option<String>,
    pub invite: Option<String>,
    pub off: bool,
}

pub fn set_server(cfg: &mut HostConfig, dir: &Path, choice: ServerChoice) -> Result<()> {
    cfg.server = match (choice.off, choice.addr) {
        (true, _) => None,
        (false, Some(a)) => {
            let server = match choice.fingerprint {
                Some(f) => ServerRef::new(&a, &f)?,
                None => ServerRef::discover(&a)?,
            };
            if let Some(code) = choice.invite {
                let identity = Identity::load_or_create(dir)?;
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
    Ok(())
}

pub fn invite(cfg: &HostConfig, dir: &Path) -> Result<()> {
    let server = cfg.server.clone().context("no server is set")?;
    let identity = Identity::load_or_create(dir)?;
    let (code, minutes) = tokio::runtime::Runtime::new()?.block_on(rendezvous::invite(&identity, &server))?;
    if ui::json() {
        println!("{}", serde_json::json!({ "code": code, "minutes": minutes }));
    } else {
        println!("Invite code: {code} (valid {minutes} minutes, for one computer)");
    }
    Ok(())
}

pub fn enroll(cfg: &mut HostConfig, dir: &Path, token: &str, unattended: bool) -> Result<()> {
    let identity = Identity::load_or_create(dir)?;
    let company = company::enroll(cfg, &identity, token, unattended)?;
    println!("Joined \"{company}\": the admin sees this computer under Unassigned.");
    Ok(())
}

pub fn leave_company(cfg: &mut HostConfig) -> Result<()> {
    match cfg.company.take() {
        Some(c) => {
            cfg.save()?;
            println!("Left \"{}\": its admin can no longer connect.", c.name);
        }
        None => println!("This computer is not in a company."),
    }
    Ok(())
}

pub fn language(cfg: &mut HostConfig, lang: String) -> Result<()> {
    cfg.language = Some(lang);
    cfg.save()?;
    println!("Banner language saved (applies from the next session).");
    Ok(())
}

pub fn auto_update(cfg: &mut HostConfig, state: &str) -> Result<()> {
    cfg.auto_update = state == "on";
    cfg.save()?;
    let how = if cfg.auto_update { "on: installed when nobody is connected" } else { "off" };
    println!("Automatic updates {how}.");
    Ok(())
}

pub fn tunnels(cfg: &mut HostConfig, policy: &str) -> Result<()> {
    cfg.tunnels = match policy {
        "allow" => crate::config::TunnelPolicy::Allow,
        "off" => crate::config::TunnelPolicy::Off,
        _ => crate::config::TunnelPolicy::Ask,
    };
    cfg.save()?;
    let how = match cfg.tunnels {
        crate::config::TunnelPolicy::Ask => rider_core::i18n::tr(
            "the person at this computer approves each new address",
            "bu kompüterdəki şəxs hər yeni ünvanı təsdiqləyir",
        ),
        crate::config::TunnelPolicy::Allow => rider_core::i18n::tr(
            "allowed without asking, in Full access sessions only",
            "soruşmadan icazə verilir, yalnız Tam giriş sessiyalarında",
        ),
        crate::config::TunnelPolicy::Off => rider_core::i18n::tr("off", "söndürülüb"),
    };
    println!("{}: {how}.", rider_core::i18n::tr("TCP tunnels", "TCP tunelləri"));
    Ok(())
}

pub fn info(cfg: &HostConfig, dir: &Path) -> Result<()> {
    let identity = Identity::load_or_create(dir)?;
    if ui::json() {
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
        return Ok(());
    }
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
    print_monitors();
    let w = company::whereabouts();
    let via = if w.via.is_empty() { String::new() } else { format!(" (via {})", w.via) };
    println!("Place:        {:?}{via}", w.place);
    Ok(())
}

fn print_monitors() {
    for m in capture::monitors().unwrap_or_default() {
        let primary = if m.info.primary { ", primary" } else { "" };
        let (n, name, w, h) = (m.info.index + 1, &m.info.name, m.info.width, m.info.height);
        println!("Monitor {n}:    {name} ({w}x{h}{primary})");
    }
}

pub fn pair(cfg: HostConfig, dir: &Path) -> Result<()> {
    let _paused = service::Paused::service()?;
    serve(cfg, Identity::load_or_create(dir)?, Mode::PairOnce)
}

pub fn run(cfg: HostConfig, dir: &Path, pairing: bool) -> Result<()> {
    if service::status() == Some(true) {
        bail!(
            "the background service is running and already accepts connections.\n\
             Use `rider-host pair` to add a device, or `rider-host service stop` first."
        );
    }
    serve(cfg, Identity::load_or_create(dir)?, Mode::Interactive { pairing })
}

pub fn agent(cfg: HostConfig, dir: &Path) -> Result<()> {
    if !cfg.can_accept() {
        tracing::error!("no PIN set; the agent will not accept connections. Run `rider-host set-pin`.");
        std::thread::sleep(std::time::Duration::from_secs(30));
        return Ok(());
    }
    serve(cfg, Identity::load_or_create(dir)?, Mode::Agent)
}

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
pub struct KnownHost {
    pub name: String,
    /// Direct address (LAN / port-forward), or empty if only reachable via the server.
    #[serde(default)]
    pub addr: String,
    /// Pinned SHA-256 of the host certificate (hex).
    pub fingerprint: String,
    /// Unix time (seconds) of the last successful connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_connected: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ClientConfig {
    /// How this device introduces itself to hosts.
    pub device_name: String,
    #[serde(default)]
    pub hosts: Vec<KnownHost>,
    /// Rendezvous server for reaching hosts on other networks.
    #[serde(default)]
    pub server: Option<rider_core::rendezvous::ServerRef>,
    /// Session window preferences, remembered between sessions.
    #[serde(default)]
    pub viewer: ViewerPrefs,
    /// Enterprise mode: this app is the admin of a company on `server`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enterprise: Option<EnterpriseAdmin>,
}

/// What the admin's app keeps after signing in with the setup code.
#[derive(Serialize, Deserialize, Clone)]
pub struct EnterpriseAdmin {
    pub company: String,
    /// Goes into the company installer's join token.
    pub join_secret: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ViewerPrefs {
    /// Keep the session toolbar open.
    #[serde(default)]
    pub pin_toolbar: bool,
    /// Picture quality: at most this many lines, 0 = automatic.
    #[serde(default)]
    pub quality: u32,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            device_name: rider_core::machine_name(),
            hosts: Vec::new(),
            server: None,
            viewer: ViewerPrefs::default(),
            enterprise: None,
        }
    }
}

/// Change the viewer preferences in the saved config (re-read first: the
/// desktop app may have changed other settings meanwhile).
pub fn update_viewer(f: impl FnOnce(&mut ViewerPrefs)) {
    let saved = ClientConfig::load().and_then(|mut c| {
        f(&mut c.viewer);
        c.save()
    });
    if let Err(e) = saved {
        tracing::warn!("cannot save viewer settings: {e:#}");
    }
}

impl ClientConfig {
    pub fn dir() -> Result<PathBuf> {
        rider_core::data_dir("client")
    }

    pub fn path() -> Result<PathBuf> {
        Ok(Self::dir()?.join("client.toml"))
    }

    pub fn load() -> Result<Self> {
        let p = Self::path()?;
        if !p.exists() {
            let c = Self::default();
            c.save()?;
            return Ok(c);
        }
        toml::from_str(&std::fs::read_to_string(&p)?).with_context(|| format!("parsing {}", p.display()))
    }

    pub fn save(&self) -> Result<()> {
        let p = Self::path()?;
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }

    pub fn find(&self, key: &str) -> Option<&KnownHost> {
        let k = key.to_lowercase();
        self.hosts
            .iter()
            .find(|h| h.name.to_lowercase() == k || h.fingerprint == k || (!h.addr.is_empty() && h.addr == key))
    }

    /// Add or replace a host (same name or same key).
    pub fn upsert(&mut self, host: KnownHost) {
        let name = host.name.to_lowercase();
        self.hosts.retain(|h| h.name.to_lowercase() != name && h.fingerprint != host.fingerprint);
        self.hosts.push(host);
    }

    /// Record a successful connection to the host with this fingerprint.
    pub fn touch(&mut self, fingerprint: &str) {
        let now = rider_core::now_us() / 1_000_000;
        if let Some(h) = self.hosts.iter_mut().find(|h| h.fingerprint == fingerprint) {
            h.last_connected = Some(now);
        }
    }

    /// Remove a host by name (case-insensitive), address or fingerprint. Returns how many were removed.
    pub fn forget(&mut self, key: &str) -> usize {
        let k = key.to_lowercase();
        let before = self.hosts.len();
        self.hosts.retain(|h| h.name.to_lowercase() != k && h.addr != key && h.fingerprint != k);
        before - self.hosts.len()
    }
}

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
pub struct KnownHost {
    pub name: String,
    pub addr: String,
    /// Pinned SHA-256 of the host certificate (hex).
    pub fingerprint: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ClientConfig {
    /// How this device introduces itself to hosts.
    pub device_name: String,
    #[serde(default)]
    pub hosts: Vec<KnownHost>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self { device_name: rider_core::machine_name(), hosts: Vec::new() }
    }
}

impl ClientConfig {
    pub fn dir() -> Result<PathBuf> {
        rider_core::data_dir("client")
    }

    fn path() -> Result<PathBuf> {
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
        self.hosts.iter().find(|h| h.name.to_lowercase() == k || h.addr == key)
    }

    pub fn upsert(&mut self, host: KnownHost) {
        self.hosts.retain(|h| h.name.to_lowercase() != host.name.to_lowercase());
        self.hosts.push(host);
    }
}

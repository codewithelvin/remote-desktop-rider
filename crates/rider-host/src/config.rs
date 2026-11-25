//! Host configuration: listening port, PIN hash and the paired-client trust store.

use anyhow::{Context, Result};
use argon2::Argon2;
use rider_core::identity::{Fingerprint, fp_from_hex, fp_hex};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct TrustedClient {
    pub name: String,
    pub fingerprint: String,
    pub paired_at: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct HostConfig {
    pub name: String,
    pub port: u16,
    /// Upper bound for the adaptive bitrate, in kbit/s.
    pub max_bitrate_kbps: u32,
    pub max_fps: u32,
    pub hardware_encoder: bool,
    /// `salt_hex:hash_hex` (Argon2id), or empty if no PIN is set.
    pub pin_hash: String,
    #[serde(default)]
    pub trusted_clients: Vec<TrustedClient>,
    /// Rendezvous server for reaching this host from other networks.
    #[serde(default)]
    pub server: Option<rider_core::rendezvous::ServerRef>,
    /// Let the service install new releases by itself when nobody is connected.
    #[serde(default = "yes")]
    pub auto_update: bool,
    /// Language of the session banner (`en`, `az`); the service has no user
    /// language of its own. Set by Manage.
    #[serde(default)]
    pub language: Option<String>,
    /// Enterprise mode: this computer belongs to a company (`rider-host enroll`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company: Option<CompanyLink>,
}

/// The company this computer joined.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct CompanyLink {
    pub name: String,
    /// The admin app's key (hex), pinned from the join token.
    pub admin: String,
    /// Installed as unattended-capable (the admin must also switch it on).
    #[serde(default)]
    pub unattended: bool,
}

fn yes() -> bool {
    true
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            name: rider_core::machine_name(),
            port: rider_core::DEFAULT_PORT,
            max_bitrate_kbps: 20_000,
            max_fps: 60,
            hardware_encoder: true,
            pin_hash: String::new(),
            trusted_clients: Vec::new(),
            server: None,
            auto_update: true,
            language: None,
            company: None,
        }
    }
}

impl HostConfig {
    /// Machine-wide state: `%ProgramData%\RemoteDesktopRider\host`, shared by
    /// the service agent (SYSTEM) and elevated management commands.
    pub fn dir() -> Result<PathBuf> {
        let base = std::env::var_os("ProgramData").context("ProgramData is not set")?;
        Ok(PathBuf::from(base).join("RemoteDesktopRider").join("host"))
    }

    /// Create the state directory and restrict it to SYSTEM + Administrators
    /// (it holds the host's private key and the trust store). Call once at startup.
    pub fn init_dir() -> Result<PathBuf> {
        let dir = Self::dir()?;
        crate::system::secure_dir(dir.parent().context("state dir has no parent")?)?;
        crate::system::secure_dir(&dir)?;
        Ok(dir)
    }

    pub fn path() -> Result<PathBuf> {
        Ok(Self::dir()?.join("host.toml"))
    }

    pub fn load() -> Result<Self> {
        let p = Self::path()?;
        if !p.exists() {
            let c = Self::default();
            c.save()?;
            return Ok(c);
        }
        let s = std::fs::read_to_string(&p)?;
        toml::from_str(&s).with_context(|| format!("parsing {}", p.display()))
    }

    pub fn save(&self) -> Result<()> {
        let p = Self::path()?;
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }

    pub fn has_pin(&self) -> bool {
        !self.pin_hash.is_empty()
    }

    /// Whether anyone could connect: a PIN for paired devices, or the
    /// company's admin (no PIN: the person here accepts). A company computer
    /// installed by Group Policy has no PIN, and needs none.
    pub fn can_accept(&self) -> bool {
        self.has_pin() || self.company.is_some()
    }

    pub fn set_pin(&mut self, pin: &str) -> Result<()> {
        let mut salt = [0u8; 16];
        getrandom::fill(&mut salt).map_err(|e| anyhow::anyhow!("rng: {e}"))?;
        let hash = hash_pin(pin, &salt)?;
        self.pin_hash = format!("{}:{}", hex::encode(salt), hex::encode(hash));
        Ok(())
    }

    pub fn verify_pin(&self, pin: &str) -> bool {
        let Some((salt, hash)) = self.pin_hash.split_once(':') else { return false };
        let (Ok(salt), Ok(expected)) = (hex::decode(salt), hex::decode(hash)) else { return false };
        match hash_pin(pin, &salt) {
            // constant-time comparison
            Ok(h) => h.len() == expected.len() && h.iter().zip(&expected).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0,
            Err(_) => false,
        }
    }

    /// Devices that may connect: the paired ones and the company's admin
    /// (who is asked for no PIN; the person here accepts instead).
    pub fn trusted(&self) -> Vec<Fingerprint> {
        let paired = self.trusted_clients.iter().filter_map(|c| fp_from_hex(&c.fingerprint).ok());
        paired.chain(self.company_admin()).collect()
    }

    /// The company's admin, pinned when this computer joined.
    pub fn company_admin(&self) -> Option<Fingerprint> {
        self.company.as_ref().and_then(|c| fp_from_hex(&c.admin).ok())
    }

    pub fn client_name(&self, fp: &Fingerprint) -> Option<String> {
        let hex = fp_hex(fp);
        self.trusted_clients.iter().find(|c| c.fingerprint == hex).map(|c| c.name.clone())
    }

    pub fn add_trusted(&mut self, name: &str, fp: &Fingerprint) {
        let hex = fp_hex(fp);
        self.trusted_clients.retain(|c| c.fingerprint != hex);
        self.trusted_clients.push(TrustedClient {
            name: name.to_string(),
            fingerprint: hex,
            paired_at: format!("{}", rider_core::now_us() / 1_000_000),
        });
    }
}

fn hash_pin(pin: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    Argon2::default().hash_password_into(pin.as_bytes(), salt, &mut out).map_err(|e| anyhow::anyhow!("argon2: {e}"))?;
    Ok(out)
}

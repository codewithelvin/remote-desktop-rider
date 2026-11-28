use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
pub struct KnownHost {
    pub name: String,
    #[serde(default)]
    pub addr: String,
    pub fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_connected: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wake: Vec<String>,
}

impl KnownHost {
    pub fn wake_macs(&self) -> Vec<rider_core::wake::Mac> {
        rider_core::wake::usable(&self.wake.iter().filter_map(|m| m.parse().ok()).collect::<Vec<_>>())
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ClientConfig {
    pub device_name: String,
    #[serde(default)]
    pub hosts: Vec<KnownHost>,
    #[serde(default)]
    pub server: Option<rider_core::rendezvous::ServerRef>,
    #[serde(default)]
    pub viewer: ViewerPrefs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enterprise: Option<EnterpriseAdmin>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct EnterpriseAdmin {
    pub company: String,
    pub join_secret: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ViewerPrefs {
    #[serde(default)]
    pub pin_toolbar: bool,
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

    pub fn upsert(&mut self, host: KnownHost) {
        let name = host.name.to_lowercase();
        self.hosts.retain(|h| h.name.to_lowercase() != name && h.fingerprint != host.fingerprint);
        self.hosts.push(host);
    }

    pub fn touch(&mut self, fingerprint: &str) {
        let now = rider_core::now_us() / 1_000_000;
        if let Some(h) = self.hosts.iter_mut().find(|h| h.fingerprint == fingerprint) {
            h.last_connected = Some(now);
        }
    }

    pub fn remember_wake(&mut self, name: &str, macs: &[rider_core::wake::Mac]) -> bool {
        let wake: Vec<String> = rider_core::wake::usable(macs).iter().map(ToString::to_string).collect();
        let k = name.to_lowercase();
        match self.hosts.iter_mut().find(|h| h.name.to_lowercase() == k) {
            Some(h) if h.wake != wake => {
                h.wake = wake;
                true
            }
            _ => false,
        }
    }

    pub fn forget(&mut self, key: &str) -> usize {
        let k = key.to_lowercase();
        let before = self.hosts.len();
        self.hosts.retain(|h| h.name.to_lowercase() != k && h.addr != key && h.fingerprint != k);
        before - self.hosts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rider_core::wake::Mac;

    #[test]
    fn wake_addresses_are_saved_by_name_and_read_back() {
        let mut cfg = ClientConfig::default();
        cfg.upsert(KnownHost {
            name: "Office PC".into(),
            addr: String::new(),
            fingerprint: "ab".repeat(32),
            last_connected: None,
            wake: Vec::new(),
        });
        let mac = Mac([0x08, 0, 0x27, 0x5F, 0xA2, 0x96]);
        let multicast = Mac([0x01, 0, 0x5E, 0, 0, 1]);
        assert!(cfg.remember_wake("office pc", &[mac, multicast, mac]));
        assert!(!cfg.remember_wake("Office PC", &[mac]), "unchanged addresses are not saved again");
        assert!(!cfg.remember_wake("Someone else", &[mac]), "only a paired computer is updated");
        let host = cfg.find("Office PC").unwrap();
        assert_eq!(host.wake, ["08-00-27-5F-A2-96"]);
        assert_eq!(host.wake_macs(), [mac]);
    }

    #[test]
    fn a_hand_edited_bad_address_is_ignored() {
        let host = KnownHost {
            name: "pc".into(),
            addr: String::new(),
            fingerprint: String::new(),
            last_connected: None,
            wake: vec!["not a mac".into(), "08-00-27-5F-A2-96".into()],
        };
        assert_eq!(host.wake_macs(), [Mac([0x08, 0, 0x27, 0x5F, 0xA2, 0x96])]);
    }
}

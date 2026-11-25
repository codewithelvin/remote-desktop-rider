use crate::identity::{Fingerprint, fp_hex};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct HostInfo {
    pub name: String,
    pub user: Option<String>,
    pub locked: bool,
    pub version: String,
    pub unattended: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Room {
    pub id: u32,
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Floor {
    pub id: u32,
    pub name: String,
    pub rooms: Vec<Room>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Computer {
    pub fingerprint: Fingerprint,
    pub info: HostInfo,
    pub room: Option<u32>,
    pub unattended_on: bool,
    pub online: bool,
    pub last_seen: u64,
    pub joined: u64,
}

impl Computer {
    pub fn unattended(&self) -> bool {
        self.info.unattended && self.unattended_on
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct Directory {
    pub company: String,
    pub floors: Vec<Floor>,
    pub computers: Vec<Computer>,
}

impl Directory {
    pub fn floor(&self, id: u32) -> Option<&Floor> {
        self.floors.iter().find(|f| f.id == id)
    }

    pub fn room(&self, id: u32) -> Option<(&Floor, &Room)> {
        self.floors.iter().find_map(|f| f.rooms.iter().find(|r| r.id == id).map(|r| (f, r)))
    }

    pub fn in_room(&self, room: Option<u32>) -> impl Iterator<Item = &Computer> {
        self.computers.iter().filter(move |c| c.room == room)
    }

    pub fn floor_counts(&self, floor: &Floor) -> (usize, usize) {
        let ids: Vec<u32> = floor.rooms.iter().map(|r| r.id).collect();
        self.counts(|c| c.room.is_some_and(|r| ids.contains(&r)))
    }

    pub fn room_counts(&self, room: Option<u32>) -> (usize, usize) {
        self.counts(|c| c.room == room)
    }

    fn counts(&self, f: impl Fn(&Computer) -> bool) -> (usize, usize) {
        self.computers.iter().filter(|c| f(c)).fold((0, 0), |(on, all), c| (on + c.online as usize, all + 1))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum DirEdit {
    AddFloor { name: String },
    AddRoom { floor: u32, name: String },
    Rename { id: u32, name: String },
    Delete { id: u32 },
    Place { computers: Vec<Fingerprint>, room: Option<u32> },
    Unattended { computer: Fingerprint, on: bool },
    Forget { computer: Fingerprint },
}

pub const MAX_NAME: usize = 60;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct JoinToken {
    pub server: String,
    pub server_fingerprint: Fingerprint,
    pub admin: Fingerprint,
    pub secret: String,
}

const TOKEN_PREFIX: &str = "rdr1.";

impl JoinToken {
    pub fn encode(&self) -> String {
        format!("{TOKEN_PREFIX}{}", hex::encode(postcard::to_allocvec(self).unwrap_or_default()))
    }

    pub fn decode(text: &str) -> Result<Self> {
        let hex_part = text.trim().strip_prefix(TOKEN_PREFIX).context("not a company join token (rdr1.…)")?;
        let bytes = hex::decode(hex_part).context("the join token is damaged")?;
        let token: Self = postcard::from_bytes(&bytes).context("the join token is damaged")?;
        if token.server.is_empty() || token.secret.is_empty() {
            bail!("the join token is incomplete");
        }
        Ok(token)
    }

    pub fn server_fingerprint_hex(&self) -> String {
        fp_hex(&self.server_fingerprint)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Place {
    #[default]
    Unknown,
    Office,
    Remote,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct Whereabouts {
    pub place: Place,
    pub via: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetLink {
    pub network: String,
    pub adapter: String,
    pub domain: bool,
    pub vpn: bool,
}

pub fn is_vpn_adapter(description: &str, if_type: u32) -> bool {
    const NAMES: [&str; 12] = [
        "pangp",
        "globalprotect",
        "vpn",
        "anyconnect",
        "fortinet",
        "fortissl",
        "wireguard",
        "wintun",
        "openvpn",
        "tap-windows",
        "juniper",
        "pulse secure",
    ];
    let d = description.to_ascii_lowercase();
    if_type == 23 || if_type == 131 || NAMES.iter().any(|n| d.contains(n))
}

impl Whereabouts {
    pub fn decide(links: &[NetLink], domain_joined: bool) -> Self {
        if let Some(l) = links.iter().find(|l| l.domain && !l.vpn) {
            return Self { place: Place::Office, via: l.network.clone() };
        }
        if let Some(l) = links.iter().find(|l| l.vpn) {
            return Self { place: Place::Remote, via: l.adapter.clone() };
        }
        if domain_joined {
            return Self { place: Place::Remote, via: String::new() };
        }
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_or_remote() {
        let link = |network: &str, adapter: &str, domain, vpn| NetLink {
            network: network.into(),
            adapter: adapter.into(),
            domain,
            vpn,
        };
        let lan = link("corp.example.com", "Intel(R) Wi-Fi 6 AX201", true, false);
        let gp = link("corp.example.com", "PANGP Virtual Ethernet Adapter", true, true);
        let home = link("HomeWiFi", "Intel(R) Wi-Fi 6 AX201", false, false);
        let decide = |links: &[NetLink], joined| Whereabouts::decide(links, joined);
        assert_eq!(decide(std::slice::from_ref(&lan), true).place, Place::Office);
        let both = decide(&[gp.clone(), lan], true);
        assert_eq!((both.place, both.via.as_str()), (Place::Office, "corp.example.com"));
        let away = decide(&[home.clone(), gp], true);
        assert_eq!((away.place, away.via.as_str()), (Place::Remote, "PANGP Virtual Ethernet Adapter"));
        assert_eq!(decide(std::slice::from_ref(&home), true).place, Place::Remote);
        assert_eq!(decide(&[home], false).place, Place::Unknown);
        assert_eq!(decide(&[], false).place, Place::Unknown);
    }

    #[test]
    fn vpn_adapters() {
        assert!(is_vpn_adapter("PANGP Virtual Ethernet Adapter", 6));
        assert!(is_vpn_adapter("PANGP Virtual Ethernet Adapter Secure", 6));
        assert!(is_vpn_adapter("Cisco AnyConnect Secure Mobility Client Virtual Miniport Adapter", 6));
        assert!(is_vpn_adapter("WAN Miniport (PPTP)", 23));
        assert!(!is_vpn_adapter("Intel(R) Ethernet Connection (13) I219-LM", 6));
        assert!(!is_vpn_adapter("Hyper-V Virtual Ethernet Adapter", 6));
        assert!(!is_vpn_adapter("VirtualBox Host-Only Ethernet Adapter", 6));
    }

    #[test]
    fn join_token_round_trip() {
        let t = JoinToken {
            server: "rdm.example.com".into(),
            server_fingerprint: [7; 32],
            admin: [9; 32],
            secret: "abc123".into(),
        };
        let text = t.encode();
        assert!(text.starts_with("rdr1."));
        assert_eq!(JoinToken::decode(&format!("  {text}\n")).unwrap(), t);
        assert!(JoinToken::decode("rdr1.zz").is_err());
        assert!(JoinToken::decode("hello").is_err());
    }

    #[test]
    fn counts_by_floor_and_room() {
        let pc = |room, online| Computer {
            fingerprint: [0; 32],
            info: HostInfo::default(),
            room,
            unattended_on: false,
            online,
            last_seen: 0,
            joined: 0,
        };
        let d = Directory {
            company: "C".into(),
            floors: vec![Floor {
                id: 1,
                name: "Floor 3".into(),
                rooms: vec![Room { id: 2, name: "310".into() }, Room { id: 3, name: "312".into() }],
            }],
            computers: vec![pc(Some(2), true), pc(Some(2), false), pc(Some(3), true), pc(None, true)],
        };
        assert_eq!(d.floor_counts(&d.floors[0]), (2, 3));
        assert_eq!(d.room_counts(Some(2)), (1, 2));
        assert_eq!(d.room_counts(None), (1, 1));
        assert_eq!(d.room(3).map(|(f, r)| (f.id, r.name.as_str())), Some((1, "312")));
    }
}

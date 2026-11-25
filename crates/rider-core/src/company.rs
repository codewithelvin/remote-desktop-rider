//! Enterprise mode: one company's computers on its own server, looked after
//! by one admin (see docs/ROADMAP.md, Milestone 7).
//!
//! * The server keeps the company directory: floors, rooms and the
//!   computers that joined (`rider-server company init` creates it and
//!   prints the admin's one-time setup code).
//! * The admin's app claims the company with that code; from then on its
//!   own key is the admin. It reads and edits the [`Directory`].
//! * A computer joins with a [`JoinToken`] (built into the company
//!   installer): it pins the server and the admin's key, then enrolls.

use crate::identity::{Fingerprint, fp_hex};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// What a computer tells the server about itself, for the admin's console.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct HostInfo {
    /// Computer name.
    pub name: String,
    /// Who is signed in at the console, if anyone.
    pub user: Option<String>,
    /// The screen is locked.
    pub locked: bool,
    /// RDR version, e.g. "0.5.3".
    pub version: String,
    /// Installed as unattended-capable (admin may connect without asking,
    /// once the admin also switches it on).
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

/// One computer of the company, as the admin sees it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Computer {
    pub fingerprint: Fingerprint,
    pub info: HostInfo,
    /// Its room; `None` = Unassigned.
    pub room: Option<u32>,
    /// The admin's switch for connecting without asking (works only if
    /// `info.unattended` too).
    pub unattended_on: bool,
    pub online: bool,
    /// Unix time it was last connected to the server.
    pub last_seen: u64,
    /// Unix time it joined.
    pub joined: u64,
}

impl Computer {
    /// The admin may connect without anyone accepting.
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

    /// The floor and room with this room id.
    pub fn room(&self, id: u32) -> Option<(&Floor, &Room)> {
        self.floors.iter().find_map(|f| f.rooms.iter().find(|r| r.id == id).map(|r| (f, r)))
    }

    /// Computers in `room` (`None`: unassigned).
    pub fn in_room(&self, room: Option<u32>) -> impl Iterator<Item = &Computer> {
        self.computers.iter().filter(move |c| c.room == room)
    }

    /// (online, total) on a floor.
    pub fn floor_counts(&self, floor: &Floor) -> (usize, usize) {
        let ids: Vec<u32> = floor.rooms.iter().map(|r| r.id).collect();
        self.counts(|c| c.room.is_some_and(|r| ids.contains(&r)))
    }

    /// (online, total) in a room (`None`: unassigned).
    pub fn room_counts(&self, room: Option<u32>) -> (usize, usize) {
        self.counts(|c| c.room == room)
    }

    fn counts(&self, f: impl Fn(&Computer) -> bool) -> (usize, usize) {
        self.computers.iter().filter(|c| f(c)).fold((0, 0), |(on, all), c| (on + c.online as usize, all + 1))
    }
}

/// A change to the directory by the admin.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum DirEdit {
    AddFloor {
        name: String,
    },
    AddRoom {
        floor: u32,
        name: String,
    },
    /// A floor or a room.
    Rename {
        id: u32,
        name: String,
    },
    /// An empty floor or room.
    Delete {
        id: u32,
    },
    /// Computers into a room, or back to Unassigned (`None`): one, or
    /// a whole selection at once.
    Place {
        computers: Vec<Fingerprint>,
        room: Option<u32>,
    },
    Unattended {
        computer: Fingerprint,
        on: bool,
    },
    /// Take a computer off the list (it can join again with the token).
    Forget {
        computer: Fingerprint,
    },
}

/// Longest floor, room or company name.
pub const MAX_NAME: usize = 60;

/// What a computer needs to join the company: the server it pins, the
/// admin's key it pins, and the company's join secret. Written as text
/// (`rdr1.` + hex) for installers and scripts.
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

/// Where a company computer is: in the office, or outside it (working from
/// home over the company VPN). Sent by 0.6+ computers in `PeerMsg::Place`,
/// only to servers that said they take it (`ServerMsg::CompanyPolicy2`).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Place {
    /// Cannot tell (not on a domain, no VPN): shown as nothing.
    #[default]
    Unknown,
    Office,
    Remote,
}

/// A computer's place, and what told it: the office network's name, or the
/// VPN adapter it reaches the company through.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct Whereabouts {
    pub place: Place,
    pub via: String,
}

/// One connected network as Windows sees it (Network List Manager), with
/// the adapter it is on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetLink {
    /// The network's name, e.g. "corp.example.com".
    pub network: String,
    /// The adapter's description, e.g. "PANGP Virtual Ethernet Adapter".
    pub adapter: String,
    /// Windows found the computer's domain controller on it.
    pub domain: bool,
    /// The adapter is a VPN's (see [`is_vpn_adapter`]).
    pub vpn: bool,
}

/// A VPN's virtual adapter, by its description or interface type (PPP 23,
/// tunnel 131). Hyper-V's and VirtualBox's adapters are not: they carry the
/// computer's own network.
pub fn is_vpn_adapter(description: &str, if_type: u32) -> bool {
    const NAMES: [&str; 12] = [
        "pangp", // Palo Alto GlobalProtect: "PANGP Virtual Ethernet Adapter"
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
    /// Decide from the connected networks. A domain-joined computer in the
    /// office finds its domain on its own Wi-Fi or Ethernet (even with the
    /// VPN connected too); outside, only through the VPN or not at all.
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
        // In the office, with or without GlobalProtect connected too.
        assert_eq!(decide(&[lan.clone()], true).place, Place::Office);
        let both = decide(&[gp.clone(), lan], true);
        assert_eq!((both.place, both.via.as_str()), (Place::Office, "corp.example.com"));
        // At home: the domain only through the VPN.
        let away = decide(&[home.clone(), gp], true);
        assert_eq!((away.place, away.via.as_str()), (Place::Remote, "PANGP Virtual Ethernet Adapter"));
        // At home without the VPN: a domain computer is still not in the office.
        assert_eq!(decide(&[home.clone()], true).place, Place::Remote);
        // Not on a domain, no VPN: cannot tell.
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

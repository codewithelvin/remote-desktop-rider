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

#[cfg(test)]
mod tests {
    use super::*;

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

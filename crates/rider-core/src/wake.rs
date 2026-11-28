use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const MAX_MACS: usize = 8;

const SYNC: [u8; 6] = [0xFF; 6];
const MAC_REPEATS: usize = 16;
const PACKET_LEN: usize = SYNC.len() + MAC_REPEATS * 6;
const DISCARD_PORT: u16 = 9;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mac(pub [u8; 6]);

impl Mac {
    pub fn usable(self) -> bool {
        let multicast = self.0[0] & 1 == 1;
        !multicast && self.0 != [0; 6]
    }

    pub fn magic_packet(self) -> [u8; PACKET_LEN] {
        let mut packet = [0u8; PACKET_LEN];
        packet[..SYNC.len()].copy_from_slice(&SYNC);
        for copy in packet[SYNC.len()..].as_chunks_mut::<6>().0 {
            *copy = self.0;
        }
        packet
    }
}

impl std::fmt::Display for Mac {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02X}-{b:02X}-{c:02X}-{d:02X}-{e:02X}-{g:02X}")
    }
}

impl std::str::FromStr for Mac {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let parts: Vec<&str> = s.split(['-', ':']).collect();
        if parts.len() != 6 {
            bail!("a MAC address has six parts: {s}");
        }
        let mut mac = [0u8; 6];
        for (byte, part) in mac.iter_mut().zip(parts) {
            if part.len() != 2 {
                bail!("each part of a MAC address is two hex digits: {s}");
            }
            *byte = u8::from_str_radix(part, 16)?;
        }
        Ok(Mac(mac))
    }
}

pub fn usable(macs: &[Mac]) -> Vec<Mac> {
    let mut out: Vec<Mac> = Vec::new();
    for &mac in macs.iter().filter(|m| m.usable()) {
        if !out.contains(&mac) && out.len() < MAX_MACS {
            out.push(mac);
        }
    }
    out
}

pub fn send(macs: &[Mac], broadcasts: &[Ipv4Addr]) -> Result<usize> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_broadcast(true)?;
    let mut sent = 0;
    let mut last_err = None;
    for mac in usable(macs) {
        let packet = mac.magic_packet();
        for &ip in broadcasts.iter().chain(std::iter::once(&Ipv4Addr::BROADCAST)) {
            match socket.send_to(&packet, SocketAddrV4::new(ip, DISCARD_PORT)) {
                Ok(_) => sent += 1,
                Err(e) => last_err = Some(e),
            }
        }
    }
    match last_err {
        Some(e) if sent == 0 => Err(e.into()),
        _ => Ok(sent),
    }
}

pub fn directed_broadcast(ip: Ipv4Addr, prefix_len: u8) -> Option<Ipv4Addr> {
    if !(1..=30).contains(&prefix_len) || ip.is_loopback() || ip.is_link_local() {
        return None;
    }
    let host_bits = u32::MAX >> prefix_len;
    Some(Ipv4Addr::from(u32::from(ip) | host_bits))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: Mac = Mac([0x08, 0x00, 0x27, 0x12, 0x34, 0x56]);

    #[test]
    fn magic_packet_is_sync_then_sixteen_copies() {
        let p = MAC.magic_packet();
        assert_eq!(p.len(), 102);
        assert_eq!(p[..6], [0xFF; 6]);
        assert!(p[6..].as_chunks::<6>().0.iter().all(|c| *c == MAC.0));
        assert_eq!(p[6..].as_chunks::<6>().0.len(), 16);
    }

    #[test]
    fn mac_text_round_trips() {
        assert_eq!(MAC.to_string(), "08-00-27-12-34-56");
        assert_eq!("08:00:27:12:34:56".parse::<Mac>().unwrap(), MAC);
        assert_eq!("08-00-27-12-34-56".parse::<Mac>().unwrap(), MAC);
        assert!("08-00-27-12-34".parse::<Mac>().is_err());
        assert!("08-00-27-12-34-5G".parse::<Mac>().is_err());
        assert!("080-0-27-12-34-56".parse::<Mac>().is_err());
    }

    #[test]
    fn only_unicast_nonzero_macs_are_kept_once() {
        let multicast = Mac([0x01, 0, 0x5E, 0, 0, 1]);
        let zero = Mac([0; 6]);
        let many: Vec<Mac> = (0..20).map(|i| Mac([0x02, 0, 0, 0, 0, i])).collect();
        assert_eq!(usable(&[MAC, multicast, zero, MAC]), vec![MAC]);
        assert_eq!(usable(&many).len(), MAX_MACS);
    }

    #[test]
    fn directed_broadcast_of_common_subnets() {
        let ip = Ipv4Addr::new(192, 168, 1, 23);
        assert_eq!(directed_broadcast(ip, 24), Some(Ipv4Addr::new(192, 168, 1, 255)));
        assert_eq!(directed_broadcast(Ipv4Addr::new(10, 1, 2, 3), 16), Some(Ipv4Addr::new(10, 1, 255, 255)));
        assert_eq!(directed_broadcast(ip, 32), None);
        assert_eq!(directed_broadcast(ip, 0), None);
        assert_eq!(directed_broadcast(Ipv4Addr::LOCALHOST, 8), None);
        assert_eq!(directed_broadcast(Ipv4Addr::new(169, 254, 1, 1), 16), None);
    }
}

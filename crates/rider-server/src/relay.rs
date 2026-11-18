//! UDP relay of last resort for peers that cannot reach each other directly
//! (symmetric NAT, strict firewalls).
//!
//! Each session gets its own UDP port and a random 16-byte token known only
//! to the two peers (delivered over their TLS connections to the server).
//! The first two distinct addresses that present the token are paired, and
//! from then on datagrams are forwarded between them unchanged. The payload
//! is end-to-end encrypted QUIC: the relay cannot read or alter sessions.

use rider_core::rendezvous::{BIND_MAGIC, Token};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::UdpSocket;

/// Peers must bind within this time; established relays expire after the same idle time.
const BIND_TIMEOUT: Duration = Duration::from_secs(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct RelayPool {
    public_ip: Option<IpAddr>,
    free: Mutex<Vec<u16>>,
}

impl RelayPool {
    pub fn new(public_ip: Option<IpAddr>, ports: std::ops::RangeInclusive<u16>) -> Arc<Self> {
        Arc::new(Self { public_ip, free: Mutex::new(ports.rev().collect()) })
    }

    /// Open a relay for one session. `ip` is the server address peers reach
    /// (used when no public IP was configured). `None` if exhausted.
    pub async fn open(self: &Arc<Self>, ip: Option<IpAddr>) -> Option<(SocketAddr, Token)> {
        let ip = self.public_ip.or(ip).filter(|ip| !ip.is_unspecified())?;
        let mut token = Token::default();
        getrandom::fill(&mut token).ok()?;
        loop {
            let port = self.free.lock().unwrap().pop()?;
            match UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], port))).await {
                Ok(socket) => {
                    let pool = self.clone();
                    tokio::spawn(async move {
                        run(socket, token).await;
                        pool.free.lock().unwrap().insert(0, port);
                    });
                    return Some((SocketAddr::new(ip, port), token));
                }
                Err(e) => tracing::warn!("relay port {port} unavailable: {e}"),
            }
        }
    }
}

async fn run(socket: UdpSocket, token: Token) {
    let port = socket.local_addr().map(|a| a.port()).unwrap_or(0);
    let (mut a, mut b): (Option<SocketAddr>, Option<SocketAddr>) = (None, None);
    let mut forwarded = 0u64;
    let mut buf = vec![0u8; 65_536];
    loop {
        let timeout = if b.is_some() { IDLE_TIMEOUT } else { BIND_TIMEOUT };
        let (n, src) = match tokio::time::timeout(timeout, socket.recv_from(&mut buf)).await {
            Err(_) => break,
            Ok(Err(_)) => continue, // e.g. ICMP-induced errors; keep serving
            Ok(Ok(r)) => r,
        };
        let data = &buf[..n];
        if n == 24 && &data[..8] == BIND_MAGIC {
            if data[8..] == token {
                match a {
                    None => a = Some(src),
                    Some(first) if first != src && b.is_none() => {
                        b = Some(src);
                        tracing::info!("relay :{port} connected {first} <-> {src}");
                    }
                    Some(_) => {}
                }
            }
            continue;
        }
        let to = if Some(src) == a {
            b
        } else if Some(src) == b {
            a
        } else {
            None // unknown sender: drop
        };
        if let Some(to) = to {
            let _ = socket.send_to(data, to).await;
            forwarded += n as u64;
        }
    }
    tracing::info!("relay :{port} closed after forwarding {} KiB", forwarded / 1024);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rider_core::rendezvous::Relay;

    #[tokio::test]
    async fn forwards_between_bound_peers_only() {
        let pool = RelayPool::new(Some("127.0.0.1".parse().unwrap()), 48_950..=48_960);
        let (addr, token) = pool.open(None).await.expect("relay");
        let relay = Relay { addr, token };
        let p1 = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let p2 = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let stranger = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        // Wrong token is ignored, so the stranger never becomes a peer.
        let mut bad = relay.bind_datagram();
        bad[23] ^= 1;
        stranger.send_to(&bad, addr).await.unwrap();
        p1.send_to(&relay.bind_datagram(), addr).await.unwrap();
        p2.send_to(&relay.bind_datagram(), addr).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        p1.send_to(b"hello", addr).await.unwrap();
        let mut buf = [0u8; 16];
        let n = tokio::time::timeout(Duration::from_secs(2), p2.recv(&mut buf)).await.unwrap().unwrap();
        assert_eq!(&buf[..n], b"hello");
        p2.send_to(b"back", addr).await.unwrap();
        let n = tokio::time::timeout(Duration::from_secs(2), p1.recv(&mut buf)).await.unwrap().unwrap();
        assert_eq!(&buf[..n], b"back");

        stranger.send_to(b"inject", addr).await.unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(200), p1.recv(&mut buf)).await.is_err());
    }
}

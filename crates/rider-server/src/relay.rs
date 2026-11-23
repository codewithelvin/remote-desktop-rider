//! UDP relay of last resort for peers that cannot reach each other directly
//! (symmetric NAT, strict firewalls).
//!
//! Each session gets its own UDP port and two random 16-byte tokens, one per
//! peer, each delivered only to its peer over its TLS connection to the
//! server. A token claims its peer's side once: the first address presenting
//! it. From then on datagrams are forwarded between the two sides unchanged.
//! The payload is end-to-end encrypted QUIC: the relay cannot read or alter
//! sessions.
//!
//! Bind datagrams travel in clear, so someone on one peer's path can see that
//! peer's token. With one shared token they could claim the other side before
//! the other peer arrived and block the session; a token only claims its own
//! side, which its peer has already taken by the time anyone saw it.

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
    /// (used when no public IP was configured). Returns the address and the
    /// two peers' tokens; `None` if exhausted.
    pub async fn open(self: &Arc<Self>, ip: Option<IpAddr>) -> Option<(SocketAddr, [Token; 2])> {
        let ip = self.public_ip.or(ip).filter(|ip| !ip.is_unspecified())?;
        let mut tokens = [Token::default(); 2];
        for t in &mut tokens {
            getrandom::fill(t).ok()?;
        }
        loop {
            let port = self.free.lock().unwrap().pop()?;
            match UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], port))).await {
                Ok(socket) => {
                    let pool = self.clone();
                    tokio::spawn(async move {
                        run(socket, tokens).await;
                        pool.free.lock().unwrap().insert(0, port);
                    });
                    return Some((SocketAddr::new(ip, port), tokens));
                }
                Err(e) => tracing::warn!("relay port {port} unavailable: {e}"),
            }
        }
    }
}

async fn run(socket: UdpSocket, tokens: [Token; 2]) {
    let port = socket.local_addr().map(|a| a.port()).unwrap_or(0);
    // The two sides, each claimed once by its own token.
    let mut sides: [Option<SocketAddr>; 2] = [None, None];
    // Bytes a -> b and b -> a, and when the pair was made.
    let (mut ab, mut ba) = (0u64, 0u64);
    let mut since = None;
    let mut buf = vec![0u8; 65_536];
    loop {
        let [a, b] = sides;
        let timeout = if since.is_some() { IDLE_TIMEOUT } else { BIND_TIMEOUT };
        let (n, src) = match tokio::time::timeout(timeout, socket.recv_from(&mut buf)).await {
            Err(_) => break,
            Ok(Err(_)) => continue, // e.g. ICMP-induced errors; keep serving
            Ok(Ok(r)) => r,
        };
        let data = &buf[..n];
        if n == 24 && &data[..8] == BIND_MAGIC {
            let side = tokens.iter().position(|t| data[8..] == *t);
            // One address cannot be both sides.
            if let Some(i) = side
                && sides[i].is_none()
                && sides[1 - i] != Some(src)
            {
                sides[i] = Some(src);
                if let [Some(a), Some(b)] = sides {
                    since = Some(std::time::Instant::now());
                    tracing::info!("relay :{port} connected {a} <-> {b}");
                }
            }
            continue;
        }
        let (to, count) = if a == Some(src) {
            (b, &mut ab)
        } else if b == Some(src) {
            (a, &mut ba)
        } else {
            continue; // unknown sender: drop
        };
        if let Some(to) = to {
            let _ = socket.send_to(data, to).await;
            *count += n as u64;
        }
    }
    let secs = since.map_or(0, |t| t.elapsed().saturating_sub(IDLE_TIMEOUT).as_secs());
    tracing::info!("relay :{port} closed: {secs} s, {} KiB host->client, {} KiB client->host", ab / 1024, ba / 1024);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rider_core::rendezvous::Relay;

    #[tokio::test]
    async fn forwards_between_bound_peers_only() {
        let pool = RelayPool::new(Some("127.0.0.1".parse().unwrap()), 48_950..=48_960);
        let (addr, [t1, t2]) = pool.open(None).await.expect("relay");
        assert_ne!(t1, t2);
        let (r1, r2) = (Relay { addr, token: t1 }, Relay { addr, token: t2 });
        let p1 = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let p2 = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let stranger = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        // Wrong token is ignored, so the stranger never becomes a peer.
        let mut bad = r2.bind_datagram();
        bad[23] ^= 1;
        stranger.send_to(&bad, addr).await.unwrap();
        p1.send_to(&r1.bind_datagram(), addr).await.unwrap();
        // Someone who saw p1's bind cannot take the other side with it.
        stranger.send_to(&r1.bind_datagram(), addr).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        p2.send_to(&r2.bind_datagram(), addr).await.unwrap();
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

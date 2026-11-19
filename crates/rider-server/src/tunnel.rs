//! Server end of the HTTPS tunnel (see `rider_core::tunnel`): peers on
//! networks that block UDP send their datagrams inside a WebSocket, and this
//! bridge forwards them to the server's own UDP ports on loopback.
//!
//! It listens on plain TCP on loopback; the web server in front of it (nginx)
//! terminates TLS on 443 and passes the client's address in `X-Real-IP`.
//! Each WebSocket gets its own loopback UDP socket, so to the rendezvous
//! service and the relays a tunneled peer looks like one ordinary UDP peer.

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use rider_core::tunnel::{MAX_DATAGRAM, PATH, PORTS, decode, encode};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Bytes, Message};

/// Open tunnels per client address, and new ones per client address per minute.
const MAX_OPEN_PER_IP: u32 = 16;
const NEW_PER_MINUTE: u32 = 60;
/// Open tunnels in total (each holds a TCP connection and a UDP socket).
const MAX_OPEN: usize = 2000;
/// A tunnel with no traffic either way is closed after this long. QUIC keeps
/// live connections busy far more often than that.
const IDLE: Duration = Duration::from_secs(120);
const HANDSHAKE: Duration = Duration::from_secs(10);

/// Which client address each tunnel's loopback UDP socket stands for, so
/// the rendezvous service can rate-limit tunneled peers like direct ones.
#[derive(Default)]
pub struct Peers(Mutex<HashMap<SocketAddr, IpAddr>>);

impl Peers {
    /// The real client address behind `addr`, if it is one of our tunnels.
    pub fn real(&self, addr: SocketAddr) -> Option<IpAddr> {
        self.0.lock().unwrap().get(&addr).copied()
    }

    #[cfg(test)]
    pub fn insert_for_test(&self, addr: SocketAddr, ip: IpAddr) {
        self.0.lock().unwrap().insert(addr, ip);
    }
}

/// Removes a tunnel from [`Peers`] when it closes.
struct Registered<'a>(&'a Peers, SocketAddr);

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        self.0.0.lock().unwrap().remove(&self.1);
    }
}

#[derive(Default)]
struct Limits {
    /// Per client address: open tunnels, start of the minute, tunnels opened in it.
    by_ip: Mutex<HashMap<IpAddr, (u32, Instant, u32)>>,
}

impl Limits {
    fn admit(self: &Arc<Self>, ip: IpAddr) -> Option<Slot> {
        let mut by_ip = self.by_ip.lock().unwrap();
        if by_ip.len() >= MAX_OPEN && !by_ip.contains_key(&ip) {
            return None;
        }
        let now = Instant::now();
        let (open, minute, new) = by_ip.entry(ip).or_insert((0, now, 0));
        if now.duration_since(*minute) >= Duration::from_secs(60) {
            (*minute, *new) = (now, 0);
        }
        if *open >= MAX_OPEN_PER_IP || *new >= NEW_PER_MINUTE {
            return None;
        }
        *open += 1;
        *new += 1;
        Some(Slot { limits: self.clone(), ip })
    }
}

/// One admitted tunnel; frees its place when dropped.
struct Slot {
    limits: Arc<Limits>,
    ip: IpAddr,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut by_ip = self.limits.by_ip.lock().unwrap();
        if let Some((open, minute, _)) = by_ip.get_mut(&self.ip) {
            *open -= 1;
            if *open == 0 && minute.elapsed() >= Duration::from_secs(60) {
                by_ip.remove(&self.ip);
            }
        }
    }
}

pub async fn bind(listen: SocketAddr) -> Result<TcpListener> {
    let listener = TcpListener::bind(listen).await.with_context(|| format!("cannot listen on TCP {listen}"))?;
    tracing::info!("HTTPS tunnel bridge on TCP {listen}{PATH}");
    Ok(listener)
}

pub async fn serve(listener: TcpListener, peers: Arc<Peers>) {
    let limits = Arc::<Limits>::default();
    loop {
        let (tcp, peer) = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                // Out of file descriptors and the like: back off instead of spinning.
                tracing::warn!("tunnel accept: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let (limits, peers) = (limits.clone(), peers.clone());
        tokio::spawn(async move {
            if let Err(e) = handle(tcp, peer, &limits, &peers).await {
                tracing::debug!("tunnel {peer}: {e:#}");
            }
        });
    }
}

async fn handle(tcp: TcpStream, peer: SocketAddr, limits: &Arc<Limits>, peers: &Peers) -> Result<()> {
    let _ = tcp.set_nodelay(true);
    let mut client = peer.ip();
    // The signature is tungstenite's callback type.
    #[allow(clippy::result_large_err)]
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        if req.uri().path() != PATH {
            let mut e = ErrorResponse::new(None);
            *e.status_mut() = StatusCode::NOT_FOUND;
            return Err(e);
        }
        // Trust the proxy's header only from the proxy itself.
        if peer.ip().is_loopback()
            && let Some(ip) = req.headers().get("x-real-ip").and_then(|v| v.to_str().ok()?.trim().parse().ok())
        {
            client = ip;
        }
        Ok(resp)
    };
    let config =
        WebSocketConfig::default().max_message_size(Some(2 + MAX_DATAGRAM)).max_frame_size(Some(2 + MAX_DATAGRAM));
    let mut ws =
        tokio::time::timeout(HANDSHAKE, tokio_tungstenite::accept_hdr_async_with_config(tcp, check, Some(config)))
            .await
            .context("handshake timed out")??;
    let Some(_slot) = limits.admit(client) else {
        let _ = ws.close(None).await;
        bail!("{client}: too many tunnels");
    };
    let udp = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let local = udp.local_addr()?;
    peers.0.lock().unwrap().insert(local, client);
    let _registered = Registered(peers, local);
    tracing::debug!("tunnel open for {client}");

    let mut buf = vec![0u8; MAX_DATAGRAM + 1];
    loop {
        tokio::select! {
            msg = tokio::time::timeout(IDLE, ws.next()) => match msg {
                Ok(Some(Ok(Message::Binary(frame)))) => {
                    // Malformed frames and ports outside the server's own are dropped.
                    if let Some((port, datagram)) = decode(&frame) {
                        let _ = udp.send_to(datagram, (Ipv4Addr::LOCALHOST, port)).await;
                    }
                }
                Ok(Some(Ok(Message::Close(_)) | Err(_)) | None) => break,
                Ok(Some(Ok(_))) => {} // ping / pong (answered by the library) / text
                Err(_) => break,      // idle
            },
            r = udp.recv_from(&mut buf) => {
                let Ok((n, from)) = r else { continue };
                if from.ip().is_loopback() && PORTS.contains(&from.port()) && n <= MAX_DATAGRAM {
                    ws.send(Message::Binary(Bytes::from(encode(from.port(), &buf[..n])))).await?;
                }
            }
        }
    }
    tracing::debug!("tunnel closed for {client}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_per_ip() {
        let limits = Arc::<Limits>::default();
        let ip: IpAddr = "198.51.100.7".parse().unwrap();
        let slots: Vec<_> = (0..MAX_OPEN_PER_IP).map_while(|_| limits.admit(ip)).collect();
        assert_eq!(slots.len() as u32, MAX_OPEN_PER_IP);
        assert!(limits.admit(ip).is_none(), "concurrent cap");
        drop(slots);
        assert!(limits.admit(ip).is_some(), "slots are released");
        assert!(limits.admit("198.51.100.8".parse().unwrap()).is_some(), "others unaffected");
    }
}

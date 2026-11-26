use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use rider_core::rendezvous::INFO_PATH;
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

const MAX_OPEN_PER_IP: u32 = 16;
const NEW_PER_MINUTE: u32 = 60;
const MAX_OPEN: usize = 2000;
const IDLE: Duration = Duration::from_secs(120);
const HANDSHAKE: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct Peers(Mutex<HashMap<SocketAddr, IpAddr>>);

impl Peers {
    pub fn real(&self, addr: SocketAddr) -> Option<IpAddr> {
        self.0.lock().unwrap().get(&addr).copied()
    }

    #[cfg(test)]
    pub fn insert_for_test(&self, addr: SocketAddr, ip: IpAddr) {
        self.0.lock().unwrap().insert(addr, ip);
    }
}

struct Registered<'a>(&'a Peers, SocketAddr);

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        self.0.0.lock().unwrap().remove(&self.1);
    }
}

#[derive(Default)]
struct Limits {
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

pub async fn serve(listener: TcpListener, peers: Arc<Peers>, info: Arc<String>) {
    let limits = Arc::<Limits>::default();
    loop {
        let (tcp, peer) = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("tunnel accept: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let (limits, peers, info) = (limits.clone(), peers.clone(), info.clone());
        tokio::spawn(async move {
            if let Ok(true) = tokio::time::timeout(HANDSHAKE, serve_info(&tcp, &info)).await {
                return;
            }
            if let Err(e) = handle(tcp, peer, &limits, &peers).await {
                tracing::debug!("tunnel {peer}: {e:#}");
            }
        });
    }
}

async fn serve_info(tcp: &TcpStream, info: &str) -> bool {
    let request = format!("GET {INFO_PATH} ");
    let mut head = [0u8; 64];
    let n = loop {
        let Ok(n) = tcp.peek(&mut head).await else { return false };
        if n == 0 || n >= request.len() || head[..n].contains(&b'\n') {
            break n;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let line = &head[..n];
    if !(line.starts_with(request.as_bytes()) || line.starts_with(format!("GET {INFO_PATH}?").as_bytes())) {
        return false;
    }
    let mut buf = [0u8; 4096];
    let mut seen = Vec::new();
    while !seen.windows(4).any(|w| w == b"\r\n\r\n") && seen.len() < 16 * 1024 {
        tcp.readable().await.ok();
        match tcp.try_read(&mut buf) {
            Ok(0) => return true,
            Ok(n) => seen.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return true,
        }
    }
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         Connection: close\r\n\r\n{info}",
        info.len()
    );
    let mut written = 0;
    while written < reply.len() {
        tcp.writable().await.ok();
        match tcp.try_write(&reply.as_bytes()[written..]) {
            Ok(n) => written += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }
    }
    true
}

async fn handle(tcp: TcpStream, peer: SocketAddr, limits: &Arc<Limits>, peers: &Peers) -> Result<()> {
    let _ = tcp.set_nodelay(true);
    let mut client = peer.ip();
    #[allow(clippy::result_large_err)]
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        if req.uri().path() != PATH {
            let mut e = ErrorResponse::new(None);
            *e.status_mut() = StatusCode::NOT_FOUND;
            return Err(e);
        }
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
                    if let Some((port, datagram)) = decode(&frame) {
                        let _ = udp.send_to(datagram, (Ipv4Addr::LOCALHOST, port)).await;
                    }
                }
                Ok(Some(Ok(Message::Close(_)) | Err(_)) | None) | Err(_) => break,
                Ok(Some(Ok(_))) => {}
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

//! Reaching a host across the internet: rendezvous, NAT hole punching and
//! relay fallback through a self-hosted `rider-server`.
//!
//! ```text
//!  client ──(1) Connect{host fp}──▶ server ◀──(0) Register{allowed}── host
//!         ◀─(2) Ready{candidates, relay}── ──(2) Offer{candidates, relay}─▶
//!  (3) both send small "punch" datagrams to each other's candidates (LAN
//!      addresses + the public address the server observed) from the SAME
//!      UDP socket QUIC uses, opening their NAT mappings;
//!  (4) the client races QUIC connections: direct candidates first, the
//!      relay shortly after; the first completed handshake wins.
//! ```
//!
//! The server never sees session keys: the end-to-end TLS keys are pinned at
//! pairing, so a malicious server or relay can at most refuse service.

use crate::identity::{Fingerprint, Identity, fp_from_hex};
use crate::tls;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

/// UDP port of the rendezvous server by default.
pub const DEFAULT_SERVER_PORT: u16 = 47900;
pub const VERSION: u32 = 1;

/// Hole-punch datagram. Deliberately tiny: QUIC endpoints drop it as noise.
pub const PUNCH: &[u8] = b"RDR-PUNCH";
/// Relay registration datagram: magic + 16-byte session token.
pub const BIND_MAGIC: &[u8; 8] = b"RDRBIND1";
pub type Token = [u8; 16];

/// How long the client waits for a direct path before also trying the relay.
const RELAY_DELAY: Duration = Duration::from_millis(1200);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);

/// Where the rendezvous server is and which key it must present.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ServerRef {
    /// `host[:port]`, e.g. `rider.example.com` or `203.0.113.7:47900`.
    pub addr: String,
    /// Hex SHA-256 of the server certificate (printed by `rider-server info`).
    pub fingerprint: String,
}

impl ServerRef {
    pub fn new(addr: &str, fingerprint: &str) -> Result<Self> {
        let fp = fp_from_hex(fingerprint).context("invalid server fingerprint")?;
        Ok(Self { addr: addr.trim().to_owned(), fingerprint: crate::identity::fp_hex(&fp) })
    }

    pub fn fingerprint(&self) -> Result<Fingerprint> {
        fp_from_hex(&self.fingerprint)
    }

    pub async fn resolve(&self) -> Result<SocketAddr> {
        resolve(&self.addr, DEFAULT_SERVER_PORT).await
    }
}

/// Resolve `host[:port]` to an IPv4 socket address.
pub async fn resolve(addr: &str, default_port: u16) -> Result<SocketAddr> {
    let has_port =
        addr.parse::<SocketAddr>().is_ok() || addr.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok());
    let with_port = if has_port { addr.to_owned() } else { format!("{addr}:{default_port}") };
    tokio::net::lookup_host(&with_port)
        .await
        .with_context(|| format!("cannot resolve {addr}"))?
        .find(SocketAddr::is_ipv4)
        .with_context(|| format!("{addr} has no IPv4 address"))
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Target {
    Host(Fingerprint),
    /// One-time pairing code shown by a host in pairing mode, e.g. `7K3-QX9`.
    Code(String),
}

/// Peer → server (first bidirectional stream, length-prefixed postcard).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum PeerMsg {
    /// Host: announce availability. Sent again whenever `allowed` or `pairing` changes.
    Register { version: u32, local: Vec<SocketAddr>, allowed: Vec<Fingerprint>, pairing: bool },
    /// Client: ask to be connected to a host.
    Connect { version: u32, target: Target, local: Vec<SocketAddr> },
    /// Client: which of these hosts are online? Only hosts that allow this
    /// client can show as online; the rest always read as offline.
    Status { version: u32, hosts: Vec<Fingerprint> },
}

/// Server → peer.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ServerMsg {
    /// To a host: registration accepted. `public` is the address the server saw.
    Registered {
        public: SocketAddr,
        pairing_code: Option<String>,
    },
    /// To a host: a permitted client wants to connect.
    Offer {
        client: Fingerprint,
        candidates: Vec<SocketAddr>,
        relay: Option<Relay>,
    },
    /// To a client: the host was told; try these paths.
    Ready {
        host: Fingerprint,
        candidates: Vec<SocketAddr>,
        relay: Option<Relay>,
    },
    Error {
        message: String,
    },
    /// To a client: answer to `Status`, in the same order.
    Online {
        online: Vec<bool>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Relay {
    pub addr: SocketAddr,
    pub token: Token,
}

impl Relay {
    pub fn bind_datagram(&self) -> [u8; 24] {
        let mut d = [0u8; 24];
        d[..8].copy_from_slice(BIND_MAGIC);
        d[8..].copy_from_slice(&self.token);
        d
    }
}

/// Pairing codes: 6 characters without look-alikes (0/O, 1/I/L), shown as `ABC-DEF`.
pub const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";

/// Canonical form of a user-typed pairing code, or `None` if it is not one.
pub fn normalize_code(s: &str) -> Option<String> {
    let raw: String = s.chars().filter(|c| !matches!(c, '-' | ' ')).collect::<String>().to_ascii_uppercase();
    (raw.len() == 6 && raw.bytes().all(|b| CODE_ALPHABET.contains(&b))).then(|| format!("{}-{}", &raw[..3], &raw[3..]))
}

/// A QUIC endpoint plus raw access to the same UDP socket.
///
/// Hole punching and relay registration only work if they leave from the
/// exact socket (and therefore NAT mapping) that QUIC then uses.
pub struct PeerSocket {
    pub endpoint: quinn::Endpoint,
    raw: Arc<UdpSocket>,
}

impl PeerSocket {
    /// Must be called inside a Tokio runtime.
    pub fn bind(addr: SocketAddr, server: Option<quinn::ServerConfig>) -> Result<Self> {
        let socket = UdpSocket::bind(addr).with_context(|| format!("cannot bind UDP {addr}"))?;
        let raw = Arc::new(socket.try_clone()?);
        let runtime = quinn::default_runtime().context("no async runtime")?;
        let endpoint = quinn::Endpoint::new(quinn::EndpointConfig::default(), server, socket, runtime)?;
        Ok(Self { endpoint, raw })
    }

    pub fn port(&self) -> u16 {
        self.endpoint.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    /// Open NAT mappings toward `targets` and register with the relay.
    /// Repeated a few times because any single datagram may be lost.
    pub async fn punch(&self, targets: &[SocketAddr], relay: Option<&Relay>) {
        let bind = relay.map(|r| (r.addr, r.bind_datagram()));
        for _ in 0..5 {
            for t in targets {
                let _ = self.raw.send_to(PUNCH, t);
            }
            if let Some((addr, d)) = &bind {
                let _ = self.raw.send_to(d, addr);
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    /// Connect to the rendezvous server and open the control stream.
    pub async fn connect_server(
        &self,
        identity: &Identity,
        server: &ServerRef,
    ) -> Result<(quinn::Connection, quinn::SendStream, quinn::RecvStream)> {
        let addr = server.resolve().await?;
        let cfg = tls::client_config_alpn(identity, Some(server.fingerprint()?), tls::RENDEZVOUS_ALPN)?;
        let conn =
            tokio::time::timeout(Duration::from_secs(10), self.endpoint.connect_with(cfg, addr, tls::SERVER_NAME)?)
                .await
                .with_context(|| format!("rendezvous server {} did not answer", server.addr))?
                .with_context(|| format!("rendezvous server {}", server.addr))?;
        let (send, recv) = conn.open_bi().await?;
        Ok((conn, send, recv))
    }
}

/// Which path a connection took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    Direct(SocketAddr),
    Relay(SocketAddr),
}

impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Path::Direct(a) => write!(f, "direct {a}"),
            Path::Relay(a) => write!(f, "relayed via {a}"),
        }
    }
}

/// Race QUIC handshakes over all paths; the first to complete wins and the
/// others are closed. Direct candidates start at once, the relay a little
/// later so it is only used when no direct path works.
pub async fn race(
    endpoint: &quinn::Endpoint,
    cfg: quinn::ClientConfig,
    direct: &[SocketAddr],
    relay: Option<SocketAddr>,
) -> Result<(quinn::Connection, Path)> {
    let mut attempts = tokio::task::JoinSet::new();
    let paths = direct
        .iter()
        .map(|&a| (Path::Direct(a), Duration::ZERO))
        .chain(relay.map(|a| (Path::Relay(a), if direct.is_empty() { Duration::ZERO } else { RELAY_DELAY })));
    for (path, delay) in paths {
        let (endpoint, cfg) = (endpoint.clone(), cfg.clone());
        attempts.spawn(async move {
            tokio::time::sleep(delay).await;
            let addr = match path {
                Path::Direct(a) | Path::Relay(a) => a,
            };
            let result = match endpoint.connect_with(cfg, addr, tls::SERVER_NAME) {
                Ok(connecting) => connecting.await.map_err(anyhow::Error::from),
                Err(e) => Err(e.into()),
            };
            (path, result)
        });
    }
    if attempts.is_empty() {
        bail!("no path to the host");
    }

    let mut last_err = None;
    let winner = tokio::time::timeout(CONNECT_TIMEOUT, async {
        while let Some(joined) = attempts.join_next().await {
            match joined {
                Ok((path, Ok(conn))) => return Some((conn, path)),
                Ok((path, Err(e))) => last_err = Some(anyhow::anyhow!("{path}: {e}")),
                Err(_) => {}
            }
        }
        None
    })
    .await
    .ok()
    .flatten();

    // Close losers that completed in the meantime; abort the rest.
    attempts.abort_all();
    while let Some(joined) = attempts.join_next().await {
        if let Ok((_, Ok(conn))) = joined {
            conn.close(0u32.into(), b"another path won");
        }
    }
    winner.ok_or_else(|| last_err.unwrap_or_else(|| anyhow::anyhow!("timed out")).context("no path to the host worked"))
}

/// This machine's IPv4 addresses (up, non-loopback) with `port`: candidates
/// for peers on the same LAN.
pub fn local_candidates(port: u16) -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = local_ipv4().into_iter().map(|ip| SocketAddr::from((ip, port))).collect();
    out.truncate(8);
    out
}

#[cfg(windows)]
fn local_ipv4() -> Vec<std::net::Ipv4Addr> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
        IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = 16 * 1024u32;
    let mut buf: Vec<u64> = Vec::new(); // u64 for alignment of the structs
    for _ in 0..3 {
        buf.resize((size as usize).div_ceil(8), 0);
        // SAFETY: `buf` is at least `size` bytes and suitably aligned.
        let err =
            unsafe { GetAdaptersAddresses(AF_INET.0 as u32, flags, None, Some(buf.as_mut_ptr().cast()), &mut size) };
        match err {
            0 => break,
            111 => continue, // ERROR_BUFFER_OVERFLOW: `size` now holds the required length
            _ => return Vec::new(),
        }
    }
    let mut ips = Vec::new();
    // SAFETY: the OS filled `buf` with a linked list of adapters that lives
    // as long as `buf`; every pointer is checked for null before use.
    unsafe {
        let mut adapter = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !adapter.is_null() {
            let a = &*adapter;
            if a.OperStatus == IfOperStatusUp && a.IfType != IF_TYPE_SOFTWARE_LOOPBACK {
                let mut unicast = a.FirstUnicastAddress;
                while !unicast.is_null() {
                    let sa = (*unicast).Address.lpSockaddr;
                    if !sa.is_null() && (*sa).sa_family == AF_INET {
                        let sin = &*(sa as *const SOCKADDR_IN);
                        let ip = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.S_un.S_addr));
                        if !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() {
                            ips.push(ip);
                        }
                    }
                    unicast = (*unicast).Next;
                }
            }
            adapter = a.Next;
        }
    }
    ips
}

#[cfg(not(windows))]
fn local_ipv4() -> Vec<std::net::Ipv4Addr> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_codes() {
        assert_eq!(normalize_code("7k3-qx9").as_deref(), Some("7K3-QX9"));
        assert_eq!(normalize_code("7K3QX9").as_deref(), Some("7K3-QX9"));
        assert_eq!(normalize_code("7K3-QX0"), None, "0 is not in the alphabet");
        assert_eq!(normalize_code("192.168.1.2"), None);
        assert_eq!(normalize_code("office-pc"), None);
    }

    #[test]
    fn bind_datagram_layout() {
        let r = Relay { addr: "1.2.3.4:5".parse().unwrap(), token: [7; 16] };
        let d = r.bind_datagram();
        assert_eq!(&d[..8], BIND_MAGIC);
        assert_eq!(&d[8..], &[7; 16]);
    }

    #[test]
    fn local_candidates_have_port() {
        assert!(local_candidates(4242).iter().all(|a| a.port() == 4242 && !a.ip().is_loopback()));
    }
}

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

    /// The server called `addr` (a name, optionally `:port`), its
    /// fingerprint fetched over HTTPS from the same name and checked like a
    /// browser checks a site: users type only the name. Blocking.
    pub fn discover(addr: &str) -> Result<Self> {
        let addr = addr.trim();
        let (host, port) = match addr.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() => (h, Some(p)),
            _ => (addr, None),
        };
        if host.is_empty() || host.parse::<std::net::IpAddr>().is_ok() || !host.contains('.') {
            bail!(crate::i18n::tr(
                "Use the server's name (like rider.example.com): only a name can be checked over HTTPS. For an IP address, enter the fingerprint under Advanced.",
                "Serverin adını yazın (məsələn, rider.example.com): HTTPS ilə yalnız ad yoxlanıla bilər. IP ünvanı üçün barmaq izini «Ətraflı» bölməsində daxil edin."
            ));
        }
        // A name that does not exist gets a plain message, without the
        // system's technical one.
        if std::net::ToSocketAddrs::to_socket_addrs(&(host, 443)).is_err() {
            bail!(crate::trf!(
                "No server named {host} was found. Is the name spelled right?",
                "{host} adlı server tapılmadı. Ad düzgün yazılıb?"
            ));
        }
        let body = crate::https::get(host, INFO_PATH, 4096).map_err(|e| {
            anyhow::anyhow!(crate::trf!(
                "Could not check {host} over HTTPS ({e:#}). Is it a Remote Desktop Rider server?",
                "{host} HTTPS ilə yoxlanıla bilmədi ({e:#}). Bu Remote Desktop Rider serveridir?"
            ))
        })?;
        let info: ServerInfo = serde_json::from_slice(&body).context("unexpected answer from the server")?;
        let addr = match port {
            Some(_) => addr.to_owned(),
            None if info.port != DEFAULT_SERVER_PORT => format!("{host}:{}", info.port),
            None => host.to_owned(),
        };
        Self::new(&addr, &info.fingerprint)
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
    /// Host or client: connection diagnostics for one session (no reply).
    Report { version: u32, report: crate::diag::Report },
    /// A new computer joins a server that only accepts invited computers,
    /// with an invite code (`rider-server invite`, or Invite from a member).
    Join { version: u32, code: String },
    /// A member computer asks for an invite code for another one.
    Invite { version: u32 },
    // Enterprise mode (`company`). Older servers close the connection on these.
    /// An app becomes the company's admin with the one-time setup code.
    AdminClaim { version: u32, code: String },
    /// The admin: the company directory, please.
    Directory { version: u32 },
    /// The admin changes the directory; answered with the new directory.
    DirectoryEdit { version: u32, edit: crate::company::DirEdit },
    /// A computer joins the company with the join token's secret.
    Enroll { version: u32, secret: String, info: crate::company::HostInfo },
    /// A company computer, on its `Register` connection: what the admin's
    /// console shows about it (sent again when it changes).
    HostInfo { version: u32, info: crate::company::HostInfo },
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
    /// Answer to `Join`: this computer is now a member.
    Joined,
    /// Answer to `Invite`: a one-time code, valid for `minutes`.
    InviteCode {
        code: String,
        minutes: u32,
    },
    /// Answer to `AdminClaim`: this app is now the admin of `company`;
    /// `join_secret` goes into the company installer's join token.
    Company {
        company: String,
        join_secret: String,
    },
    /// Answer to `Directory` / `DirectoryEdit`.
    Directory(crate::company::Directory),
    /// Answer to `Enroll`: the computer is on the company's list.
    Enrolled {
        company: String,
    },
    /// To a company computer (on its `Register` connection): the admin's
    /// switch for connecting without anyone accepting.
    CompanyPolicy {
        unattended: bool,
    },
}

/// HTTPS path where a server publishes its fingerprint (the web front end
/// passes `/rider-tunnel...` to the server's tunnel bridge).
/// The server's refusals. The server sends them in English (as older
/// versions did); `explain` shows them in the user's language.
pub mod refusal {
    pub const UNREACHABLE: &str = "host is offline or this device is not allowed to reach it";
    pub const BUSY: &str = "too many requests; try again in a minute";
    pub const INVITE_NOT_PAIRING: &str = "this is a server invite code, not a pairing code";
    pub const BAD_INVITE: &str = "this invite code is not valid: it may have expired (10 minutes) or been used";
}

/// A message from the server in the user's language (unknown ones as sent).
pub fn explain(message: &str) -> String {
    use crate::i18n::tr;
    match message {
        refusal::UNREACHABLE => tr(
            "The computer is offline, or this device may not reach it.",
            "Kompüter oflayndır və ya bu cihazın ona qoşulmağa icazəsi yoxdur.",
        ),
        refusal::BUSY => {
            tr("Too many attempts. Try again in a minute.", "Çox cəhd edildi. Bir dəqiqə sonra yenidən cəhd edin.")
        }
        refusal::INVITE_NOT_PAIRING => tr(
            "This is a server invite code: it goes in the Server box of a new computer. To add a computer here,              use the code from its Add a device button.",
            "Bu, server dəvət kodudur: o, yeni kompüterin «Server» bölməsinə yazılır. Kompüteri buraya əlavə              etmək üçün onun «Cihaz əlavə et» düyməsinin göstərdiyi kodu istifadə edin.",
        ),
        refusal::BAD_INVITE => tr(
            "This invite code is not valid: it has expired (10 minutes) or was already used.",
            "Bu dəvət kodu etibarsızdır: vaxtı bitib (10 dəqiqə) və ya artıq istifadə olunub.",
        ),
        other => return other.to_owned(),
    }
    .to_owned()
}

pub const INFO_PATH: &str = "/rider-tunnel/info";

/// What a server publishes at [`INFO_PATH`]. Not secret: the same key is
/// shown to anyone who connects. HTTPS makes sure it is the real server's.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub fingerprint: String,
    /// The rendezvous UDP port.
    pub port: u16,
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

/// Until when this process goes straight to the HTTPS tunnel: the network
/// did not let UDP to the server through recently.
static TUNNEL_UNTIL: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
/// How long to stay on the tunnel before trying UDP again.
const TUNNEL_STICKY: Duration = Duration::from_secs(600);

/// Whether this process recently had to reach the server through the tunnel.
pub fn tunnel_recent() -> bool {
    TUNNEL_UNTIL.lock().unwrap().is_some_and(|t| std::time::Instant::now() < t)
}

/// Note that UDP to the server does not get through from here.
pub fn mark_tunnel() {
    *TUNNEL_UNTIL.lock().unwrap() = Some(std::time::Instant::now() + TUNNEL_STICKY);
}

/// One request to the server and its answer.
async fn ask(identity: &Identity, server: &ServerRef, msg: &PeerMsg) -> Result<ServerMsg> {
    let (_socket, conn, mut send, mut recv) = reach_server(identity, server).await?;
    crate::proto::write_msg(&mut send, msg).await?;
    let reply = tokio::time::timeout(Duration::from_secs(10), crate::proto::read_msg::<ServerMsg>(&mut recv))
        .await
        .context("the server did not answer")??;
    conn.close(0u32.into(), b"done");
    match reply {
        Some(ServerMsg::Error { message }) => bail!("{}", explain(&message)),
        Some(r) => Ok(r),
        // An older server closes on messages it does not know.
        None => bail!("the server is too old for this: update rider-server"),
    }
}

/// Join a server that only accepts invited computers.
pub async fn join(identity: &Identity, server: &ServerRef, code: &str) -> Result<()> {
    let code = normalize_code(code).context(crate::i18n::tr(
        "that is not an invite code: it has 6 letters and digits, like K7P-2QX",
        "bu dəvət kodu deyil: o, 6 hərf və rəqəmdən ibarətdir, məsələn, K7P-2QX",
    ))?;
    match ask(identity, server, &PeerMsg::Join { version: VERSION, code }).await? {
        ServerMsg::Joined => Ok(()),
        _ => bail!("unexpected reply from the rendezvous server"),
    }
}

/// An invite code for another computer: (code, minutes valid).
pub async fn invite(identity: &Identity, server: &ServerRef) -> Result<(String, u32)> {
    match ask(identity, server, &PeerMsg::Invite { version: VERSION }).await? {
        ServerMsg::InviteCode { code, minutes } => Ok((code, minutes)),
        _ => bail!("unexpected reply from the rendezvous server"),
    }
}

/// Enterprise mode: claim the company with the server's one-time setup code
/// (`rider-server company init`). Returns (company name, join secret).
pub async fn admin_claim(identity: &Identity, server: &ServerRef, code: &str) -> Result<(String, String)> {
    let code = code.trim().to_ascii_uppercase();
    match ask(identity, server, &PeerMsg::AdminClaim { version: VERSION, code }).await? {
        ServerMsg::Company { company, join_secret } => Ok((company, join_secret)),
        _ => bail!("unexpected reply from the rendezvous server"),
    }
}

/// Enterprise mode, admin only: the company directory, after `edit` if given.
pub async fn directory(
    identity: &Identity,
    server: &ServerRef,
    edit: Option<crate::company::DirEdit>,
) -> Result<crate::company::Directory> {
    let msg = match edit {
        Some(edit) => PeerMsg::DirectoryEdit { version: VERSION, edit },
        None => PeerMsg::Directory { version: VERSION },
    };
    match ask(identity, server, &msg).await? {
        ServerMsg::Directory(d) => Ok(d),
        _ => bail!("unexpected reply from the rendezvous server"),
    }
}

/// Enterprise mode: this computer joins the company. Returns its name.
pub async fn enroll(
    identity: &Identity,
    server: &ServerRef,
    secret: &str,
    info: crate::company::HostInfo,
) -> Result<String> {
    match ask(identity, server, &PeerMsg::Enroll { version: VERSION, secret: secret.into(), info }).await? {
        ServerMsg::Enrolled { company } => Ok(company),
        _ => bail!("unexpected reply from the rendezvous server"),
    }
}

/// A fresh socket connected to the rendezvous server: UDP, or the HTTPS
/// tunnel on networks where the server does not answer over UDP.
pub async fn reach_server(
    identity: &Identity,
    server: &ServerRef,
) -> Result<(PeerSocket, quinn::Connection, quinn::SendStream, quinn::RecvStream)> {
    #[cfg(windows)]
    if tunnel_recent() {
        let socket = PeerSocket::tunnel(server, None).await?;
        let (conn, send, recv) = socket.connect_server(identity, server).await?;
        mark_tunnel();
        return Ok((socket, conn, send, recv));
    }
    let socket = PeerSocket::bind(SocketAddr::from(([0, 0, 0, 0], 0)), None)?;
    match socket.connect_server(identity, server).await {
        Ok((conn, send, recv)) => Ok((socket, conn, send, recv)),
        #[cfg(windows)]
        Err(e) if format!("{e:#}").contains("did not answer") => {
            tracing::info!("{e:#}; trying the HTTPS tunnel");
            let socket = PeerSocket::tunnel(server, None).await?;
            let (conn, send, recv) = socket.connect_server(identity, server).await?;
            mark_tunnel();
            Ok((socket, conn, send, recv))
        }
        Err(e) => Err(e),
    }
}

/// Send one diagnostics report to the server. Best effort: the caller only
/// logs failures (an older server simply drops the connection).
pub async fn send_report(identity: &Identity, server: &ServerRef, report: crate::diag::Report) -> Result<()> {
    let (_socket, conn, mut send, _recv) = reach_server(identity, server).await?;
    crate::proto::write_msg(&mut send, &PeerMsg::Report { version: VERSION, report: report.bounded() }).await?;
    send.finish()?;
    // The server closes the connection once it has the report.
    let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
    Ok(())
}

/// A QUIC endpoint plus raw access to the same UDP socket.
///
/// Hole punching and relay registration only work if they leave from the
/// exact socket (and therefore NAT mapping) that QUIC then uses.
pub struct PeerSocket {
    pub endpoint: quinn::Endpoint,
    raw: Raw,
}

/// Where raw datagrams (punches, relay registration) go out.
enum Raw {
    Udp(Arc<UdpSocket>),
    #[cfg(windows)]
    Tunnel(Arc<crate::tunnel::TunnelSocket>),
}

impl Raw {
    fn send_to(&self, data: &[u8], to: &SocketAddr) {
        match self {
            Raw::Udp(s) => {
                let _ = s.send_to(data, to);
            }
            #[cfg(windows)]
            Raw::Tunnel(t) => t.send_to(data, *to),
        }
    }
}

impl PeerSocket {
    /// Must be called inside a Tokio runtime.
    pub fn bind(addr: SocketAddr, server: Option<quinn::ServerConfig>) -> Result<Self> {
        let socket = UdpSocket::bind(addr).with_context(|| format!("cannot bind UDP {addr}"))?;
        let raw = Arc::new(socket.try_clone()?);
        let runtime = quinn::default_runtime().context("no async runtime")?;
        let endpoint = quinn::Endpoint::new(quinn::EndpointConfig::default(), server, socket, runtime)?;
        Ok(Self { endpoint, raw: Raw::Udp(raw) })
    }

    /// Fallback for networks that block UDP: reach `server` (and its relays)
    /// through the HTTPS tunnel instead. Sessions then always use the relay.
    #[cfg(windows)]
    pub async fn tunnel(server: &ServerRef, accept: Option<quinn::ServerConfig>) -> Result<Self> {
        let ip = server.resolve().await?.ip();
        let host =
            server.addr.rsplit_once(':').filter(|(_, p)| p.parse::<u16>().is_ok()).map_or(&*server.addr, |(h, _)| h);
        let socket = crate::tunnel::TunnelSocket::connect(host, ip).await?;
        let runtime = quinn::default_runtime().context("no async runtime")?;
        let endpoint = quinn::Endpoint::new_with_abstract_socket(
            quinn::EndpointConfig::default(),
            accept,
            socket.clone(),
            runtime,
        )?;
        Ok(Self { endpoint, raw: Raw::Tunnel(socket) })
    }

    /// Through the HTTPS tunnel rather than UDP.
    pub fn is_tunnel(&self) -> bool {
        !matches!(self.raw, Raw::Udp(_))
    }

    /// False once a tunnel has closed (a UDP socket is always usable).
    pub fn alive(&self) -> bool {
        match &self.raw {
            Raw::Udp(_) => true,
            #[cfg(windows)]
            Raw::Tunnel(t) => t.alive(),
        }
    }

    /// Local UDP port; 0 for the tunnel (no local addresses to offer).
    pub fn port(&self) -> u16 {
        self.endpoint.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    /// Open NAT mappings toward `targets` and register with the relay.
    /// Repeated a few times because any single datagram may be lost.
    pub async fn punch(&self, targets: &[SocketAddr], relay: Option<&Relay>) {
        let bind = relay.map(|r| (r.addr, r.bind_datagram()));
        for _ in 0..5 {
            for t in targets {
                self.raw.send_to(PUNCH, t);
            }
            if let Some((addr, d)) = &bind {
                self.raw.send_to(d, addr);
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
        let cfg = tls::client_config_alpn(identity, Some(server.fingerprint()?), &[tls::RENDEZVOUS_ALPN])?;
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
    if port == 0 {
        return Vec::new(); // the HTTPS tunnel has no local address
    }
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

/// macOS and Linux: interfaces that are up, via `getifaddrs`.
#[cfg(unix)]
fn local_ipv4() -> Vec<std::net::Ipv4Addr> {
    let mut ips = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs allocates a linked list we only read and then free
    // with freeifaddrs; every pointer is checked for null before use.
    unsafe {
        if libc::getifaddrs(&mut list) != 0 {
            return ips;
        }
        let mut cur = list;
        while !cur.is_null() {
            let ifa = &*cur;
            let up = ifa.ifa_flags & (libc::IFF_UP as libc::c_uint) != 0;
            let loopback = ifa.ifa_flags & (libc::IFF_LOOPBACK as libc::c_uint) != 0;
            if up && !loopback && !ifa.ifa_addr.is_null() && i32::from((*ifa.ifa_addr).sa_family) == libc::AF_INET {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let ip = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                if !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() && !ips.contains(&ip) {
                    ips.push(ip);
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(list);
    }
    ips
}

#[cfg(not(any(windows, unix)))]
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

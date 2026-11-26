use crate::i18n::tr;
use crate::proto::{BulkHeader, read_msg, write_msg};
use crate::trf;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_TARGET_LEN: usize = 260;
const STREAM_PRIORITY: i32 = 0;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ForwardHeader {
    pub target: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ForwardReply {
    Open,
    Refused(Refusal),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    NeedsFullAccess,
    TurnedOff,
    Declined,
    NobodyToAsk,
    NotLocal,
    BadAddress(String),
    CannotConnect(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NeedsFullAccess => {
                f.write_str(tr("TCP tunnels need Full access", "TCP tunelləri üçün Tam giriş lazımdır"))
            }
            Refusal::TurnedOff => {
                f.write_str(tr("TCP tunnels are turned off on that computer", "o kompüterdə TCP tunelləri söndürülüb"))
            }
            Refusal::Declined => f.write_str(tr(
                "the person at that computer declined the tunnel",
                "o kompüterdəki şəxs tuneli rədd etdi",
            )),
            Refusal::NobodyToAsk => f.write_str(tr(
                "nobody at that computer can approve the tunnel",
                "o kompüterdə tuneli təsdiqləyəcək heç kim yoxdur",
            )),
            Refusal::NotLocal => f.write_str(tr(
                "only that computer and its local network can be reached",
                "yalnız o kompüterə və onun yerli şəbəkəsinə qoşulmaq olar",
            )),
            Refusal::BadAddress(target) => write!(f, "{}", trf!("cannot find {target}", "{target} tapılmadı")),
            Refusal::CannotConnect(target) => {
                write!(f, "{}", trf!("cannot connect to {target}", "{target} ünvanına qoşulmaq olmur"))
            }
        }
    }
}

pub fn split_target(target: &str) -> Result<(&str, u16)> {
    if target.len() > MAX_TARGET_LEN {
        bail!(tr("the address is too long", "ünvan çox uzundur"));
    }
    let (host, port) = target.rsplit_once(':').context(tr(
        "give the address as host:port, e.g. 192.168.1.20:443",
        "ünvanı host:port kimi yazın, məsələn 192.168.1.20:443",
    ))?;
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    let port: u16 =
        port.parse().ok().filter(|&p| p != 0).context(tr("the port must be 1-65535", "port 1-65535 olmalıdır"))?;
    if host.is_empty() || host.contains(char::is_whitespace) {
        bail!(tr("missing host name or address", "host adı və ya ünvan yoxdur"));
    }
    Ok((host, port))
}

pub fn reachable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => private_v4(v4),
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or_else(|| private_v6(v6), private_v4),
    }
}

fn private_v4(ip: Ipv4Addr) -> bool {
    ip.is_loopback() || ip.is_private() || ip.is_link_local()
}

fn private_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    ip.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
}

pub async fn resolve(target: &str) -> Result<SocketAddr, Refusal> {
    let (host, port) = split_target(target).map_err(|_| Refusal::BadAddress(target.to_owned()))?;
    let found: Vec<SocketAddr> =
        tokio::net::lookup_host((host, port)).await.map_err(|_| Refusal::BadAddress(target.to_owned()))?.collect();
    if found.is_empty() {
        return Err(Refusal::BadAddress(target.to_owned()));
    }
    found.into_iter().find(|a| reachable(a.ip())).ok_or(Refusal::NotLocal)
}

pub async fn open(conn: &quinn::Connection, target: &str) -> Result<(quinn::SendStream, quinn::RecvStream)> {
    if !crate::tls::speaks_v7(conn) {
        bail!(tr(
            "the other computer runs an older version without TCP tunnels: update it",
            "digər kompüterdə TCP tunelləri olmayan köhnə versiya var: onu yeniləyin"
        ));
    }
    split_target(target)?;
    let (mut send, mut recv) = conn.open_bi().await?;
    let _ = send.set_priority(STREAM_PRIORITY);
    write_msg(&mut send, &BulkHeader::Forward(ForwardHeader { target: target.to_owned() })).await?;
    match read_msg::<ForwardReply>(&mut recv)
        .await?
        .context(tr("the other computer closed the tunnel", "digər kompüter tuneli bağladı"))?
    {
        ForwardReply::Open => Ok((send, recv)),
        ForwardReply::Refused(reason) => bail!("{reason}"),
    }
}

pub async fn refuse(mut send: quinn::SendStream, reason: Refusal) -> Result<()> {
    write_msg(&mut send, &ForwardReply::Refused(reason)).await?;
    send.finish()?;
    let _ = send.stopped().await;
    Ok(())
}

pub async fn accept(mut send: quinn::SendStream, recv: quinn::RecvStream, tcp: tokio::net::TcpStream) -> Result<()> {
    write_msg(&mut send, &ForwardReply::Open).await?;
    let _ = tcp.set_nodelay(true);
    let (r, w) = tcp.into_split();
    pipe(recv, send, r, w).await
}

pub async fn pipe<R, W>(
    mut from_peer: quinn::RecvStream,
    mut to_peer: quinn::SendStream,
    mut local_r: R,
    mut local_w: W,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let up = async {
        let n = tokio::io::copy(&mut local_r, &mut to_peer).await;
        let _ = to_peer.finish();
        n
    };
    let down = async {
        let n = tokio::io::copy(&mut from_peer, &mut local_w).await;
        let _ = local_w.shutdown().await;
        n
    };
    let (a, b) = tokio::join!(up, down);
    a.context("sending")?;
    b.context("receiving")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_host_and_port() {
        assert_eq!(split_target("192.168.1.20:443").unwrap(), ("192.168.1.20", 443));
        assert_eq!(split_target("nas.local:80").unwrap(), ("nas.local", 80));
        assert_eq!(split_target("[fe80::1]:22").unwrap(), ("fe80::1", 22));
        assert!(split_target("192.168.1.20").is_err());
        assert!(split_target("192.168.1.20:0").is_err());
        assert!(split_target("192.168.1.20:70000").is_err());
        assert!(split_target(":80").is_err());
        assert!(split_target(&format!("{}:80", "a".repeat(MAX_TARGET_LEN))).is_err());
    }

    #[test]
    fn only_this_computer_and_its_local_network() {
        for ok in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.1.20",
            "169.254.10.1",
            "::1",
            "fd00::5",
            "fe80::1",
            "::ffff:192.168.1.1",
        ] {
            assert!(reachable(ok.parse().unwrap()), "{ok} should be reachable");
        }
        for no in [
            "8.8.8.8",
            "1.1.1.1",
            "172.32.0.1",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "2001:4860::8888",
            "::",
            "::ffff:8.8.8.8",
            "51.210.43.4",
        ] {
            assert!(!reachable(no.parse().unwrap()), "{no} must not be reachable");
        }
    }

    #[tokio::test]
    async fn public_names_and_addresses_are_refused() {
        assert_eq!(resolve("8.8.8.8:53").await, Err(Refusal::NotLocal));
        assert!(matches!(resolve("no-such-host.invalid:80").await, Err(Refusal::BadAddress(_))));
        assert_eq!(resolve("127.0.0.1:8080").await.unwrap(), "127.0.0.1:8080".parse().unwrap());
        assert_eq!(resolve("localhost:8080").await.unwrap().port(), 8080);
    }
}

use crate::net::{Shared, UiEvent};
use anyhow::{Context, Result};
use rider_core::forward;
use rider_core::i18n::tr;
use rider_core::trf;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use winit::event_loop::EventLoopProxy;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub local_port: u16,
    pub target: String,
}

impl std::str::FromStr for Spec {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let (local_port, target) = match s.split_once(':') {
            Some((port, rest)) if rest.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) => {
                (port.parse().context(tr("the local port must be 0-65535", "yerli port 0-65535 olmalıdır"))?, rest)
            }
            _ => (0, s),
        };
        forward::split_target(target)?;
        Ok(Self { local_port, target: target.to_owned() })
    }
}

pub struct Listening {
    pub local: SocketAddr,
    pub target: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn listen(spec: Spec, shared: Arc<Shared>, proxy: EventLoopProxy<UiEvent>) -> Result<Listening> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, spec.local_port)).await.with_context(|| {
        trf!("cannot use local port {}", "{} yerli portundan istifadə etmək olmur", spec.local_port)
    })?;
    let local = listener.local_addr()?;
    let target = spec.target.clone();
    let task = tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let (target, shared, proxy) = (target.clone(), shared.clone(), proxy.clone());
            tokio::spawn(async move {
                if let Err(e) = carry(tcp, &target, &shared).await {
                    let _ = proxy.send_event(UiEvent::Notice(format!("{target}: {e:#}")));
                }
            });
        }
    });
    Ok(Listening { local, target: spec.target, task })
}

async fn carry(tcp: TcpStream, target: &str, shared: &Shared) -> Result<()> {
    let conn = shared.conn.lock().unwrap().clone().context(tr("not connected", "qoşulmayıb"))?;
    let (send, recv) = forward::open(&conn, target).await?;
    let _ = tcp.set_nodelay(true);
    let (r, w) = tcp.into_split();
    forward::pipe(recv, send, r, w).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_name_an_optional_local_port_and_a_target() {
        let s: Spec = "8080:192.168.1.20:80".parse().unwrap();
        assert_eq!(s, Spec { local_port: 8080, target: "192.168.1.20:80".into() });
        let s: Spec = "nas.local:443".parse().unwrap();
        assert_eq!(s, Spec { local_port: 0, target: "nas.local:443".into() });
        let s: Spec = "2222:[fe80::1]:22".parse().unwrap();
        assert_eq!(s, Spec { local_port: 2222, target: "[fe80::1]:22".into() });
        assert!("192.168.1.20".parse::<Spec>().is_err());
        assert!("99999:192.168.1.20:80".parse::<Spec>().is_err());
    }
}

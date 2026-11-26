use crate::chat::Chat;
use crate::config::TunnelPolicy;
use rider_core::forward::{self, CONNECT_TIMEOUT, ForwardHeader, Refusal};
use rider_core::proto::Permissions;
use std::collections::HashSet;
use std::sync::Mutex;

pub struct Forwards {
    policy: TunnelPolicy,
    allowed: Mutex<HashSet<String>>,
    declined: Mutex<HashSet<String>>,
    asking: tokio::sync::Mutex<()>,
}

impl Forwards {
    pub fn new(policy: TunnelPolicy) -> Self {
        Self { policy, allowed: Mutex::default(), declined: Mutex::default(), asking: tokio::sync::Mutex::new(()) }
    }

    async fn permit(&self, target: &str, perms: Permissions, chat: &Chat) -> Result<(), Refusal> {
        if !perms.is_full() {
            return Err(Refusal::NeedsFullAccess);
        }
        match self.policy {
            TunnelPolicy::Off => return Err(Refusal::TurnedOff),
            TunnelPolicy::Allow => return Ok(()),
            TunnelPolicy::Ask => {}
        }
        let key = target.to_ascii_lowercase();
        let _one_at_a_time = self.asking.lock().await;
        if self.allowed.lock().unwrap().contains(&key) {
            return Ok(());
        }
        if self.declined.lock().unwrap().contains(&key) {
            return Err(Refusal::Declined);
        }
        match chat.ask_tunnel(target).await {
            Some(true) => {
                self.allowed.lock().unwrap().insert(key);
                Ok(())
            }
            Some(false) => {
                self.declined.lock().unwrap().insert(key);
                Err(Refusal::Declined)
            }
            None => Err(Refusal::NobodyToAsk),
        }
    }
}

pub async fn incoming(
    header: ForwardHeader,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    perms: Permissions,
    chat: &Chat,
    forwards: &Forwards,
) {
    let target = header.target;
    let opened = async {
        let addr = forward::resolve(&target).await?;
        forwards.permit(&target, perms, chat).await?;
        let tcp = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(addr))
            .await
            .ok()
            .and_then(Result::ok)
            .ok_or_else(|| Refusal::CannotConnect(target.clone()))?;
        Ok::<_, Refusal>((addr, tcp))
    };
    match opened.await {
        Ok((addr, tcp)) => {
            tracing::info!("TCP tunnel to {addr} opened");
            chat.tunnel_event("tunnel_open", &target);
            if let Err(e) = forward::accept(send, recv, tcp).await {
                tracing::info!("TCP tunnel to {addr} ended: {e:#}");
            }
            chat.tunnel_event("tunnel_closed", &target);
        }
        Err(refusal) => {
            tracing::info!("TCP tunnel to {target} refused: {refusal:?}");
            let _ = forward::refuse(send, refusal).await;
        }
    }
}

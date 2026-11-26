pub mod flow;

pub const PATH: &str = "/rider-tunnel";
pub const PORTS: std::ops::RangeInclusive<u16> = 47900..=47999;
pub const MAX_DATAGRAM: usize = 2048;
pub const MAX_QUEUE_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

pub fn stale(queued: std::time::Instant, now: std::time::Instant) -> bool {
    now.saturating_duration_since(queued) > MAX_QUEUE_DELAY
}

pub fn encode(port: u16, datagram: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(2 + datagram.len());
    f.extend_from_slice(&port.to_be_bytes());
    f.extend_from_slice(datagram);
    f
}

pub fn decode(frame: &[u8]) -> Option<(u16, &[u8])> {
    let (port, data) = frame.split_first_chunk::<2>()?;
    let port = u16::from_be_bytes(*port);
    (PORTS.contains(&port) && data.len() <= MAX_DATAGRAM).then_some((port, data))
}

#[cfg(windows)]
pub use client::TunnelSocket;

#[cfg(windows)]
mod client {
    use super::flow::{Flow, decode_ack};
    use super::{MAX_DATAGRAM, PATH, PORTS, decode, encode};
    use anyhow::{Context, Result, bail};
    use quinn::udp::{RecvMeta, Transmit};
    use quinn::{AsyncUdpSocket, UdpPoller};
    use std::ffi::c_void;
    use std::fmt;
    use std::io::{self, IoSliceMut};
    use std::net::{IpAddr, SocketAddr};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::task::{Context as TaskContext, Poll};
    use std::time::Instant;
    use tokio::sync::mpsc;
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_AUTOLOGON_SECURITY_LEVEL_LOW, WINHTTP_FLAG_SECURE,
        WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_OPTION_UPGRADE_TO_WEB_SOCKET, WINHTTP_QUERY_FLAG_NUMBER,
        WINHTTP_QUERY_STATUS_CODE, WINHTTP_WEB_SOCKET_BINARY_MESSAGE_BUFFER_TYPE, WINHTTP_WEB_SOCKET_BUFFER_TYPE,
        WINHTTP_WEB_SOCKET_CLOSE_BUFFER_TYPE, WINHTTP_WEB_SOCKET_SUCCESS_CLOSE_STATUS, WinHttpCloseHandle,
        WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReceiveResponse,
        WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts, WinHttpWebSocketClose,
        WinHttpWebSocketCompleteUpgrade, WinHttpWebSocketReceive, WinHttpWebSocketSend,
    };
    use windows::core::{HSTRING, PCWSTR, w};

    const SEND_QUEUE: usize = 128;

    struct Handle(*mut c_void);
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}

    impl Handle {
        fn new(h: *mut c_void, what: &str) -> Result<Self> {
            if h.is_null() {
                return Err(windows::core::Error::from_thread()).context(what.to_owned());
            }
            Ok(Self(h))
        }
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }

    struct Ws {
        socket: Handle,
        _session: Handle,
    }

    impl Ws {
        fn connect(host: &str) -> Result<Self> {
            let agent = HSTRING::from(format!("RemoteDesktopRider/{}", env!("CARGO_PKG_VERSION")));
            unsafe {
                let session = Handle::new(
                    WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0),
                    "WinHttpOpen",
                )?;
                WinHttpSetTimeouts(session.0, 15_000, 15_000, 15_000, 15_000)?;
                let conn = Handle::new(WinHttpConnect(session.0, &HSTRING::from(host), 443, 0), "connect")?;
                let req = Handle::new(
                    WinHttpOpenRequest(
                        conn.0,
                        w!("GET"),
                        &HSTRING::from(PATH),
                        PCWSTR::null(),
                        PCWSTR::null(),
                        std::ptr::null(),
                        WINHTTP_FLAG_SECURE,
                    ),
                    "request",
                )?;
                let low = WINHTTP_AUTOLOGON_SECURITY_LEVEL_LOW.to_ne_bytes();
                let _ = WinHttpSetOption(Some(req.0), WINHTTP_OPTION_AUTOLOGON_POLICY, Some(&low));
                WinHttpSetOption(Some(req.0), WINHTTP_OPTION_UPGRADE_TO_WEB_SOCKET, None)?;
                WinHttpSendRequest(req.0, None, None, 0, 0, 0).context("cannot connect")?;
                WinHttpReceiveResponse(req.0, std::ptr::null_mut()).context("no answer")?;
                let mut status = 0u32;
                let mut len = size_of::<u32>() as u32;
                WinHttpQueryHeaders(
                    req.0,
                    WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                    PCWSTR::null(),
                    Some((&raw mut status).cast()),
                    &raw mut len,
                    std::ptr::null_mut(),
                )?;
                if status != 101 {
                    bail!("the server does not offer the HTTPS tunnel (HTTP {status})");
                }
                let socket = Handle::new(WinHttpWebSocketCompleteUpgrade(req.0, None), "WebSocket upgrade")?;
                drop(req);
                drop(conn);
                Ok(Self { socket, _session: session })
            }
        }

        fn send(&self, frame: &[u8]) -> Result<()> {
            let e = unsafe {
                WinHttpWebSocketSend(self.socket.0, WINHTTP_WEB_SOCKET_BINARY_MESSAGE_BUFFER_TYPE, Some(frame))
            };
            if e != 0 {
                bail!("tunnel send failed ({e})");
            }
            Ok(())
        }

        fn recv(&self, buf: &mut [u8]) -> Result<Option<usize>> {
            let mut filled = 0usize;
            loop {
                let mut n = 0u32;
                let mut kind = WINHTTP_WEB_SOCKET_BUFFER_TYPE::default();
                let e = unsafe {
                    WinHttpWebSocketReceive(
                        self.socket.0,
                        buf[filled..].as_mut_ptr().cast(),
                        (buf.len() - filled) as u32,
                        &raw mut n,
                        &raw mut kind,
                    )
                };
                if e != 0 {
                    bail!("tunnel receive failed ({e})");
                }
                filled += n as usize;
                match kind {
                    WINHTTP_WEB_SOCKET_CLOSE_BUFFER_TYPE => return Ok(None),
                    WINHTTP_WEB_SOCKET_BUFFER_TYPE(1 | 3) if filled < buf.len() => {}
                    _ => return Ok(Some(filled)),
                }
            }
        }

        fn close(&self) {
            unsafe {
                let _ = WinHttpWebSocketClose(self.socket.0, WINHTTP_WEB_SOCKET_SUCCESS_CLOSE_STATUS.0 as u16, None, 0);
            }
        }
    }

    pub struct TunnelSocket {
        server_ip: IpAddr,
        tx: mpsc::Sender<(Instant, Vec<u8>)>,
        rx: Mutex<mpsc::UnboundedReceiver<(u16, Vec<u8>)>>,
        alive: Arc<AtomicBool>,
        ws: Arc<Ws>,
    }

    impl fmt::Debug for TunnelSocket {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("TunnelSocket").field("server_ip", &self.server_ip).finish_non_exhaustive()
        }
    }

    impl TunnelSocket {
        pub async fn connect(host: &str, server_ip: IpAddr) -> Result<Arc<Self>> {
            let h = host.to_owned();
            let ws = Arc::new(
                tokio::task::spawn_blocking(move || Ws::connect(&h))
                    .await
                    .context("tunnel task failed")?
                    .with_context(|| format!("HTTPS tunnel to {host}"))?,
            );
            let alive = Arc::new(AtomicBool::new(true));
            let (tx, mut out) = mpsc::channel::<(Instant, Vec<u8>)>(SEND_QUEUE);
            let (inc, rx) = mpsc::unbounded_channel();

            let flow = Arc::new((Mutex::new(Flow::default()), Condvar::new()));
            let (w, a, f) = (ws.clone(), alive.clone(), flow.clone());
            std::thread::Builder::new().name("tunnel-send".into()).spawn(move || {
                while let Some((queued, frame)) = out.blocking_recv() {
                    if admit(&f, queued, frame.len(), out.is_empty()) && w.send(&frame).is_err() {
                        break;
                    }
                }
                a.store(false, Ordering::SeqCst);
                w.close();
            })?;
            let (r, a, f) = (ws.clone(), alive.clone(), flow);
            std::thread::Builder::new().name("tunnel-recv".into()).spawn(move || {
                let mut buf = vec![0u8; 2 + MAX_DATAGRAM];
                while let Ok(Some(n)) = r.recv(&mut buf) {
                    if let Some(total) = decode_ack(&buf[..n]) {
                        f.0.lock().unwrap().acked(total, Instant::now());
                        f.1.notify_one();
                    } else if let Some((port, data)) = decode(&buf[..n])
                        && inc.send((port, data.to_vec())).is_err()
                    {
                        break;
                    }
                }
                a.store(false, Ordering::SeqCst);
            })?;
            Ok(Arc::new(Self { server_ip, tx, rx: Mutex::new(rx), alive, ws }))
        }

        pub fn alive(&self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }

        pub fn send_to(&self, datagram: &[u8], to: SocketAddr) {
            let ours = to.ip() == self.server_ip || to.ip().is_loopback();
            if ours && PORTS.contains(&to.port()) && datagram.len() <= MAX_DATAGRAM {
                let _ = self.tx.try_send((Instant::now(), encode(to.port(), datagram)));
            }
        }
    }

    fn admit(flow: &(Mutex<Flow>, Condvar), queued: Instant, len: usize, nothing_behind: bool) -> bool {
        let mut f = flow.0.lock().unwrap();
        let mut held = false;
        loop {
            let now = Instant::now();
            let waited = now.saturating_duration_since(queued);
            if waited > f.max_wait() {
                return false;
            }
            if f.may_send() {
                f.sent(len, now, nothing_behind && !held);
                return true;
            }
            held = true;
            let left = f.max_wait().saturating_sub(waited);
            f = flow.1.wait_timeout(f, left).unwrap().0;
        }
    }

    impl Drop for TunnelSocket {
        fn drop(&mut self) {
            self.ws.close();
        }
    }

    #[derive(Debug)]
    struct AlwaysWritable;

    impl UdpPoller for AlwaysWritable {
        fn poll_writable(self: Pin<&mut Self>, _: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncUdpSocket for TunnelSocket {
        fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
            Box::pin(AlwaysWritable)
        }

        fn try_send(&self, t: &Transmit<'_>) -> io::Result<()> {
            self.send_to(t.contents, t.destination);
            Ok(())
        }

        fn poll_recv(
            &self,
            cx: &mut TaskContext<'_>,
            bufs: &mut [IoSliceMut<'_>],
            meta: &mut [RecvMeta],
        ) -> Poll<io::Result<usize>> {
            let mut rx = self.rx.lock().unwrap();
            match rx.poll_recv(cx) {
                Poll::Ready(Some((port, data))) => {
                    let n = data.len().min(bufs[0].len());
                    bufs[0][..n].copy_from_slice(&data[..n]);
                    meta[0] = RecvMeta {
                        addr: SocketAddr::new(self.server_ip, port),
                        len: n,
                        stride: n,
                        ecn: None,
                        dst_ip: None,
                    };
                    Poll::Ready(Ok(1))
                }
                Poll::Ready(None) | Poll::Pending => Poll::Pending,
            }
        }

        fn local_addr(&self) -> io::Result<SocketAddr> {
            Ok(SocketAddr::from(([0, 0, 0, 0], 0)))
        }

        fn may_fragment(&self) -> bool {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames() {
        let f = encode(47900, b"hello");
        assert_eq!(decode(&f), Some((47900, &b"hello"[..])));
        assert_eq!(decode(&encode(47999, b"")), Some((47999, &b""[..])));
        assert_eq!(decode(&encode(53, b"x")), None);
        assert_eq!(decode(&encode(48000, b"x")), None);
        assert_eq!(decode(&[0xBB]), None);
        assert_eq!(decode(&encode(47900, &vec![0; MAX_DATAGRAM + 1])), None);
    }

    #[test]
    fn packets_waiting_too_long_are_stale() {
        let t = std::time::Instant::now();
        assert!(!stale(t, t));
        assert!(!stale(t, t + MAX_QUEUE_DELAY));
        assert!(stale(t, t + MAX_QUEUE_DELAY + std::time::Duration::from_millis(1)));
        assert!(!stale(t + std::time::Duration::from_secs(1), t));
    }
}

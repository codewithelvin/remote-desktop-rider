//! One small HTTPS GET, with the certificate checked against the system's
//! trusted roots (as a browser would). Used to learn a rendezvous server's
//! fingerprint from its web address, so users only type the server's name.
//!
//! Windows: WinHTTP (system proxy and certificate store). Elsewhere: rustls
//! with the system's root certificates.

use anyhow::Result;

/// `GET https://{host}{path}`; the body, at most `max` bytes.
#[cfg(windows)]
pub fn get(host: &str, path: &str, max: usize) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    crate::update::http::get(&format!("https://{host}{path}"), "application/json", max as u64, &mut |chunk| {
        body.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok(body)
}

/// `GET https://{host}{path}`; the body, at most `max` bytes.
#[cfg(not(windows))]
pub fn get(host: &str, path: &str, max: usize) -> Result<Vec<u8>> {
    use anyhow::{Context, bail};
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::sync::Arc;
    use std::time::Duration;

    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        bail!("no trusted certificates found on this system");
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.to_owned()).context("invalid server name")?;
    let conn = rustls::ClientConnection::new(Arc::new(config), name)?;

    let addr = (host, 443).to_socket_addrs()?.next().with_context(|| format!("cannot resolve {host}"))?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(15)).context("cannot connect")?;
    tcp.set_read_timeout(Some(Duration::from_secs(15)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(15)))?;
    let mut tls = rustls::StreamOwned::new(conn, tcp);
    // HTTP/1.0: a plain body that ends with the connection, never chunked.
    let request = format!(
        "GET {path} HTTP/1.0\r\nHost: {host}\r\nAccept: application/json\r\nUser-Agent: RemoteDesktopRider/{}\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    );
    tls.write_all(request.as_bytes())?;
    let mut reply = Vec::new();
    match tls.by_ref().take((max + 16 * 1024) as u64).read_to_end(&mut reply) {
        Ok(_) => {}
        // Servers often close without TLS close_notify after the body.
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && !reply.is_empty() => {}
        Err(e) => return Err(e).context("reading the reply"),
    }
    let split = reply.windows(4).position(|w| w == b"\r\n\r\n").context("malformed reply")?;
    let status = String::from_utf8_lossy(&reply[..split]);
    let code = status.split_whitespace().nth(1).unwrap_or("");
    if code != "200" {
        bail!("server answered HTTP {code}");
    }
    let body = reply[split + 4..].to_vec();
    if body.len() > max {
        bail!("reply too large");
    }
    Ok(body)
}

//! QUIC + TLS 1.3 configuration with certificate pinning on both sides.
//!
//! * The client pins the host's fingerprint (learned during pairing).
//! * The host only completes the handshake for client fingerprints in its
//!   trust store, unless a pairing window is open. Unpaired devices never get
//!   past TLS, so they cannot reach the application protocol at all.
//!
//! Signatures are still verified normally, so presenting a pinned certificate
//! without owning its private key fails the handshake.

use crate::identity::{Fingerprint, Identity, fingerprint};
use anyhow::Result;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};
use std::sync::Arc;
use std::time::Duration;

pub const ALPN: &[u8] = b"rider/1";
/// Server name used in the handshake. Ignored by our verifiers (we pin keys, not names).
pub const SERVER_NAME: &str = "rider";

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Decides, at handshake time, whether a client certificate is acceptable.
pub trait ClientTrust: Send + Sync + std::fmt::Debug {
    fn is_allowed(&self, fp: &Fingerprint) -> bool;
}

#[derive(Debug)]
struct PinnedClientVerifier {
    trust: Arc<dyn ClientTrust>,
    provider: Arc<CryptoProvider>,
}

impl ClientCertVerifier for PinnedClientVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn client_auth_mandatory(&self) -> bool {
        true
    }
    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        if self.trust.is_allowed(&fingerprint(end_entity)) {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(rustls::Error::General("device is not paired with this host".into()))
        }
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// `expected == None` is only used while pairing: any host key is accepted
/// and the humans then compare the short authentication string.
#[derive(Debug)]
struct PinnedServerVerifier {
    expected: Option<Fingerprint>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self.expected {
            Some(exp) if fingerprint(end_entity) != exp => {
                Err(rustls::Error::General("HOST FINGERPRINT MISMATCH - possible man-in-the-middle attack".into()))
            }
            _ => Ok(ServerCertVerified::assertion()),
        }
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Transport tuned for interactive streaming over bad links: BBR congestion
/// control (does not collapse on random loss like loss-based Cubic), frequent
/// keep-alives, and large windows for high bandwidth-delay paths.
fn transport() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
    t.keep_alive_interval(Some(Duration::from_secs(2)));
    t.max_idle_timeout(Some(Duration::from_secs(20).try_into().expect("valid idle timeout")));
    t.max_concurrent_uni_streams(1024u32.into());
    t.max_concurrent_bidi_streams(32u32.into());
    t.stream_receive_window((16u32 * 1024 * 1024).into());
    t.receive_window((64u32 * 1024 * 1024).into());
    t.send_window(64 * 1024 * 1024);
    t.datagram_receive_buffer_size(Some(1024 * 1024));
    Arc::new(t)
}

pub fn server_config(id: &Identity, trust: Arc<dyn ClientTrust>) -> Result<quinn::ServerConfig> {
    let provider = provider();
    let verifier = Arc::new(PinnedClientVerifier { trust, provider: provider.clone() });
    let mut crypto = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![id.cert.clone()], id.private_key())?;
    crypto.alpn_protocols = vec![ALPN.to_vec()];
    let qsc = quinn::crypto::rustls::QuicServerConfig::try_from(crypto)?;
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(qsc));
    cfg.transport_config(transport());
    Ok(cfg)
}

pub fn client_config(id: &Identity, expected_host: Option<Fingerprint>) -> Result<quinn::ClientConfig> {
    let provider = provider();
    let verifier = Arc::new(PinnedServerVerifier { expected: expected_host, provider: provider.clone() });
    let mut crypto = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![id.cert.clone()], id.private_key())?;
    crypto.alpn_protocols = vec![ALPN.to_vec()];
    let qcc = quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(qcc));
    cfg.transport_config(transport());
    Ok(cfg)
}

/// Fingerprint of the certificate the peer presented on an established connection.
pub fn peer_fingerprint(conn: &quinn::Connection) -> Result<Fingerprint> {
    let any = conn.peer_identity().ok_or_else(|| anyhow::anyhow!("peer sent no certificate"))?;
    let certs =
        any.downcast::<Vec<CertificateDer<'static>>>().map_err(|_| anyhow::anyhow!("unexpected peer identity type"))?;
    let cert = certs.first().ok_or_else(|| anyhow::anyhow!("empty peer certificate chain"))?;
    Ok(fingerprint(cert))
}

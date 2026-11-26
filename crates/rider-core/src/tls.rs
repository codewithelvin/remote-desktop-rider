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
pub const ALPN_V2: &[u8] = b"rider/2";
pub const ALPN_V3: &[u8] = b"rider/3";
pub const ALPN_V4: &[u8] = b"rider/4";
pub const ALPN_V5: &[u8] = b"rider/5";
pub const ALPN_V6: &[u8] = b"rider/6";
pub const ALPN_V7: &[u8] = b"rider/7";
const SESSION_ALPNS: &[&[u8]] = &[ALPN_V7, ALPN_V6, ALPN_V5, ALPN_V4, ALPN_V3, ALPN_V2, ALPN];
pub const RENDEZVOUS_ALPN: &[u8] = b"rider-rv/1";
pub const SUPPORT_ALPN: &[u8] = b"rider-support/1";
pub const SUPPORT_ALPN_V2: &[u8] = b"rider-support/2";
pub const SUPPORT_ALPN_V3: &[u8] = b"rider-support/3";
pub const SUPPORT_ALPN_V4: &[u8] = b"rider-support/4";
pub const SUPPORT_ALPN_V5: &[u8] = b"rider-support/5";
pub const SUPPORT_ALPN_V6: &[u8] = b"rider-support/6";
const SUPPORT_ALPNS: &[&[u8]] =
    &[SUPPORT_ALPN_V6, SUPPORT_ALPN_V5, SUPPORT_ALPN_V4, SUPPORT_ALPN_V3, SUPPORT_ALPN_V2, SUPPORT_ALPN];
pub const SERVER_NAME: &str = "rider";

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

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

#[derive(Debug)]
pub struct AnyClient;

impl ClientTrust for AnyClient {
    fn is_allowed(&self, _: &Fingerprint) -> bool {
        true
    }
}

pub fn server_config(id: &Identity, trust: Arc<dyn ClientTrust>) -> Result<quinn::ServerConfig> {
    server_config_alpn(id, trust, SESSION_ALPNS)
}

pub fn client_config(id: &Identity, expected_host: Option<Fingerprint>) -> Result<quinn::ClientConfig> {
    client_config_alpn(id, expected_host, SESSION_ALPNS)
}

pub fn server_config_alpn(id: &Identity, trust: Arc<dyn ClientTrust>, alpn: &[&[u8]]) -> Result<quinn::ServerConfig> {
    let provider = provider();
    let verifier = Arc::new(PinnedClientVerifier { trust, provider: provider.clone() });
    let mut crypto = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![id.cert.clone()], id.private_key())?;
    crypto.alpn_protocols = alpn.iter().map(|a| a.to_vec()).collect();
    let qsc = quinn::crypto::rustls::QuicServerConfig::try_from(crypto)?;
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(qsc));
    cfg.transport_config(transport());
    Ok(cfg)
}

pub fn client_config_alpn(
    id: &Identity,
    expected_host: Option<Fingerprint>,
    alpn: &[&[u8]],
) -> Result<quinn::ClientConfig> {
    let provider = provider();
    let verifier = Arc::new(PinnedServerVerifier { expected: expected_host, provider: provider.clone() });
    let mut crypto = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![id.cert.clone()], id.private_key())?;
    crypto.alpn_protocols = alpn.iter().map(|a| a.to_vec()).collect();
    let qcc = quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(qcc));
    cfg.transport_config(transport());
    Ok(cfg)
}

pub fn speaks_v2(conn: &quinn::Connection) -> bool {
    protocol(conn).is_some_and(|p| {
        [
            ALPN_V7,
            ALPN_V6,
            ALPN_V5,
            ALPN_V4,
            ALPN_V3,
            ALPN_V2,
            SUPPORT_ALPN_V6,
            SUPPORT_ALPN_V5,
            SUPPORT_ALPN_V4,
            SUPPORT_ALPN_V3,
            SUPPORT_ALPN_V2,
            SUPPORT_ALPN,
        ]
        .contains(&p.as_slice())
    })
}

pub fn speaks_v3(conn: &quinn::Connection) -> bool {
    protocol(conn).is_some_and(|p| {
        [
            ALPN_V7,
            ALPN_V6,
            ALPN_V5,
            ALPN_V4,
            ALPN_V3,
            SUPPORT_ALPN_V6,
            SUPPORT_ALPN_V5,
            SUPPORT_ALPN_V4,
            SUPPORT_ALPN_V3,
            SUPPORT_ALPN_V2,
        ]
        .contains(&p.as_slice())
    })
}

pub fn speaks_v4(conn: &quinn::Connection) -> bool {
    protocol(conn).is_some_and(|p| {
        [ALPN_V7, ALPN_V6, ALPN_V5, ALPN_V4, SUPPORT_ALPN_V6, SUPPORT_ALPN_V5, SUPPORT_ALPN_V4, SUPPORT_ALPN_V3]
            .contains(&p.as_slice())
    })
}

pub fn speaks_v5(conn: &quinn::Connection) -> bool {
    protocol(conn).is_some_and(|p| {
        [ALPN_V7, ALPN_V6, ALPN_V5, SUPPORT_ALPN_V6, SUPPORT_ALPN_V5, SUPPORT_ALPN_V4].contains(&p.as_slice())
    })
}

pub fn speaks_v6(conn: &quinn::Connection) -> bool {
    protocol(conn).is_some_and(|p| [ALPN_V7, ALPN_V6, SUPPORT_ALPN_V6, SUPPORT_ALPN_V5].contains(&p.as_slice()))
}

pub fn speaks_v7(conn: &quinn::Connection) -> bool {
    protocol(conn).is_some_and(|p| p == ALPN_V7 || p == SUPPORT_ALPN_V6)
}

fn protocol(conn: &quinn::Connection) -> Option<Vec<u8>> {
    conn.handshake_data()?.downcast::<quinn::crypto::rustls::HandshakeData>().ok()?.protocol
}

pub fn peer_fingerprint(conn: &quinn::Connection) -> Result<Fingerprint> {
    let any = conn.peer_identity().ok_or_else(|| anyhow::anyhow!("peer sent no certificate"))?;
    let certs =
        any.downcast::<Vec<CertificateDer<'static>>>().map_err(|_| anyhow::anyhow!("unexpected peer identity type"))?;
    let cert = certs.first().ok_or_else(|| anyhow::anyhow!("empty peer certificate chain"))?;
    Ok(fingerprint(cert))
}

pub fn support_server_config(id: &Identity, trust: Arc<dyn ClientTrust>) -> Result<quinn::ServerConfig> {
    server_config_alpn(id, trust, SUPPORT_ALPNS)
}

pub fn support_client_config(id: &Identity, expected_host: Option<Fingerprint>) -> Result<quinn::ClientConfig> {
    client_config_alpn(id, expected_host, SUPPORT_ALPNS)
}

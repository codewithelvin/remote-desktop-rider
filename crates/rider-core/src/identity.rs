//! Persistent device identity.
//!
//! Every host and client owns a long-lived Ed25519 key with a self-signed
//! certificate. The SHA-256 of that certificate (its *fingerprint*) is the
//! device's identity: peers pin it during pairing and reject anything else
//! afterwards. No certificate authority (and no server) is ever trusted.

use anyhow::{Context, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use std::path::Path;

pub type Fingerprint = [u8; 32];

#[derive(Clone)]
pub struct Identity {
    pub cert: CertificateDer<'static>,
    key_pkcs8: Vec<u8>,
}

impl Identity {
    /// Load the identity stored in `dir`, creating a fresh one on first run.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let cert_path = dir.join("identity.cert.der");
        let key_path = dir.join("identity.key.der");
        if cert_path.exists() && key_path.exists() {
            let cert = std::fs::read(&cert_path).context("reading identity certificate")?;
            let key = std::fs::read(&key_path).context("reading identity key")?;
            return Ok(Self { cert: CertificateDer::from(cert), key_pkcs8: key });
        }
        let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519)?;
        let params = rcgen::CertificateParams::new(vec!["rider".to_string()])?;
        let cert = params.self_signed(&key_pair)?;
        let id = Self { cert: cert.der().clone(), key_pkcs8: key_pair.serialize_der() };
        std::fs::write(&key_path, &id.key_pkcs8).context("writing identity key")?;
        std::fs::write(&cert_path, id.cert.as_ref()).context("writing identity certificate")?;
        Ok(id)
    }

    pub fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_pkcs8.clone()))
    }

    pub fn fingerprint(&self) -> Fingerprint {
        fingerprint(&self.cert)
    }
}

pub fn fingerprint(cert: &CertificateDer<'_>) -> Fingerprint {
    Sha256::digest(cert.as_ref()).into()
}

/// Full hex form, stored in config files.
pub fn fp_hex(fp: &Fingerprint) -> String {
    hex::encode(fp)
}

pub fn fp_from_hex(s: &str) -> Result<Fingerprint> {
    let v = hex::decode(s.trim()).context("fingerprint is not hex")?;
    v.try_into().map_err(|_| anyhow::anyhow!("fingerprint must be 32 bytes"))
}

/// Short human-readable form, e.g. `3FA2-91C0-77DE-0B14`.
pub fn fp_short(fp: &Fingerprint) -> String {
    fp[..8].chunks(2).map(|c| format!("{:02X}{:02X}", c[0], c[1])).collect::<Vec<_>>().join("-")
}

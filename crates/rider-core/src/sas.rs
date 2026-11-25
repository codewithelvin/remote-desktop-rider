use crate::identity::Fingerprint;
use sha2::{Digest, Sha256};

const EXPORTER_LABEL: &[u8] = b"EXPORTER-rider-pairing-v1";

pub fn compute(conn: &quinn::Connection, host_fp: &Fingerprint, client_fp: &Fingerprint) -> anyhow::Result<String> {
    let mut exported = [0u8; 32];
    conn.export_keying_material(&mut exported, EXPORTER_LABEL, b"")
        .map_err(|_| anyhow::anyhow!("TLS exporter unavailable"))?;
    let mut h = Sha256::new();
    h.update(b"rider-sas-v1");
    h.update(host_fp);
    h.update(client_fp);
    h.update(exported);
    let d = h.finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    Ok(format!("{:03} {:03}", n / 1000, n % 1000))
}

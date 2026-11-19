//! Connection diagnostics: what a session's network and pipeline did, so
//! slow paths can be found and tuned.
//!
//! Each side samples a few numbers every [`SAMPLE_EVERY`] and sends them to
//! the rendezvous server when the session ends (and every [`FLUSH_EVERY`]
//! during long ones), over its own authenticated connection. Reports hold
//! metrics only: no screen content, input, addresses, names or keys. Both
//! sides derive the same `session` number from the session's TLS keys, so the
//! server can pair host and client reports while it means nothing to anyone
//! else.

use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const SAMPLE_EVERY: Duration = Duration::from_secs(5);
pub const FLUSH_EVERY: Duration = Duration::from_secs(600);
/// Samples per report: [`FLUSH_EVERY`] / [`SAMPLE_EVERY`].
pub const MAX_SAMPLES: usize = 120;
const EXPORTER_LABEL: &[u8] = b"EXPORTER-rider-diagnostics";

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Client,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Direct,
    Relay,
}

/// One [`SAMPLE_EVERY`] interval. Host: `frame_ms` is encode time, `kbps` the
/// video actually sent, `target_kbps` the adaptive bitrate, `dropped` frames
/// skipped or expired by congestion. Client: `frame_ms` is decode time,
/// `latency_ms` capture-to-display, `fps` frames shown.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq)]
pub struct Sample {
    /// Seconds since the session started.
    pub t: u32,
    pub rtt_ms: u16,
    pub min_rtt_ms: u16,
    /// Lost / sent packets, in thousandths.
    pub loss_permille: u16,
    pub cwnd_kb: u32,
    pub fps: u16,
    pub kbps: u32,
    pub target_kbps: u32,
    pub frame_ms: f32,
    pub latency_ms: u16,
    pub dropped: u16,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Report {
    pub session: u64,
    pub role: Role,
    pub version: String,
    pub route: Route,
    /// This side reached the server through the HTTPS tunnel.
    pub tunnel: bool,
    /// Host: the video encoder in use.
    pub encoder: String,
    /// Report number within the session (long sessions send several).
    pub part: u16,
    /// Set on the session's last report: how it ended.
    pub ended: Option<String>,
    pub samples: Vec<Sample>,
}

impl Report {
    /// Clip to sane sizes (the server applies the same to what it receives).
    pub fn bounded(mut self) -> Self {
        self.samples.truncate(MAX_SAMPLES);
        for s in [&mut self.version, &mut self.encoder] {
            truncate(s, 40);
        }
        if let Some(e) = &mut self.ended {
            truncate(e, 120);
        }
        self
    }
}

fn truncate(s: &mut String, max: usize) {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
}

/// The same number on both ends of `conn`, unpredictable to anyone else.
pub fn session_id(conn: &quinn::Connection) -> u64 {
    let mut b = [0u8; 8];
    match conn.export_keying_material(&mut b, EXPORTER_LABEL, b"") {
        Ok(()) => u64::from_le_bytes(b),
        Err(_) => 0,
    }
}

/// Collects samples for one session side and cuts them into reports.
pub struct Recorder {
    report: Report,
    started: std::time::Instant,
    last_flush: std::time::Instant,
}

impl Recorder {
    pub fn new(session: u64, role: Role, route: Route, tunnel: bool) -> Self {
        let now = std::time::Instant::now();
        Self {
            report: Report {
                session,
                role,
                version: env!("CARGO_PKG_VERSION").into(),
                route,
                tunnel,
                encoder: String::new(),
                part: 0,
                ended: None,
                samples: Vec::with_capacity(MAX_SAMPLES),
            },
            started: now,
            last_flush: now,
        }
    }

    pub fn set_encoder(&mut self, encoder: &str) {
        if self.report.encoder != encoder {
            encoder.clone_into(&mut self.report.encoder);
        }
    }

    /// Add a sample; returns a report to send when one is due.
    pub fn push(&mut self, mut s: Sample) -> Option<Report> {
        s.t = self.started.elapsed().as_secs() as u32;
        self.report.samples.push(s);
        let due = self.report.samples.len() >= MAX_SAMPLES || self.last_flush.elapsed() >= FLUSH_EVERY;
        due.then(|| self.cut(None))
    }

    /// The final report.
    pub fn finish(mut self, ended: &str) -> Report {
        self.cut(Some(ended.into()))
    }

    fn cut(&mut self, ended: Option<String>) -> Report {
        self.last_flush = std::time::Instant::now();
        let samples = std::mem::replace(&mut self.report.samples, Vec::with_capacity(MAX_SAMPLES));
        let r = Report { samples, ended, ..self.report.clone() };
        self.report.part = self.report.part.saturating_add(1);
        r.bounded()
    }
}

/// Clamp to a u16 field.
pub fn u16_of(v: impl TryInto<u16>) -> u16 {
    v.try_into().unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_cuts_reports() {
        let mut r = Recorder::new(7, Role::Host, Route::Relay, true);
        r.set_encoder("NVENC H.264");
        let mut sent = 0;
        for _ in 0..MAX_SAMPLES * 2 + 3 {
            if let Some(rep) = r.push(Sample::default()) {
                assert_eq!(rep.samples.len(), MAX_SAMPLES);
                assert_eq!(rep.part, sent);
                assert!(rep.ended.is_none());
                sent += 1;
            }
        }
        let last = r.finish("closed by client");
        assert_eq!((sent, last.part, last.samples.len()), (2, 2, 3));
        assert_eq!(last.encoder, "NVENC H.264");
        assert_eq!(last.ended.as_deref(), Some("closed by client"));
    }

    #[test]
    fn bounded_clips_strings_on_char_boundaries() {
        let mut r = Recorder::new(1, Role::Client, Route::Direct, false).finish(&"é".repeat(200));
        r.encoder = "x".repeat(100);
        let r = r.bounded();
        assert!(r.ended.unwrap().len() <= 120);
        assert_eq!(r.encoder.len(), 40);
    }
}

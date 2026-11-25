use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const SAMPLE_EVERY: Duration = Duration::from_secs(5);
pub const FLUSH_EVERY: Duration = Duration::from_secs(600);
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

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq)]
pub struct Sample {
    pub t: u32,
    pub rtt_ms: u16,
    pub min_rtt_ms: u16,
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
    pub tunnel: bool,
    pub encoder: String,
    pub part: u16,
    pub ended: Option<String>,
    pub samples: Vec<Sample>,
}

impl Report {
    #[must_use]
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

pub fn session_id(conn: &quinn::Connection) -> u64 {
    let mut b = [0u8; 8];
    match conn.export_keying_material(&mut b, EXPORTER_LABEL, b"") {
        Ok(()) => u64::from_le_bytes(b),
        Err(_) => 0,
    }
}

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

    pub fn push(&mut self, mut s: Sample) -> Option<Report> {
        s.t = self.started.elapsed().as_secs() as u32;
        self.report.samples.push(s);
        let due = self.report.samples.len() >= MAX_SAMPLES || self.last_flush.elapsed() >= FLUSH_EVERY;
        due.then(|| self.cut(None))
    }

    pub fn finish(mut self, ended: &str) -> Report {
        self.cut(Some(ended.into()))
    }

    fn cut(&mut self, ended: Option<String>) -> Report {
        self.last_flush = std::time::Instant::now();
        let samples = std::mem::replace(&mut self.report.samples, Vec::with_capacity(MAX_SAMPLES));
        let r = Report { ended, samples, ..self.report.clone() };
        self.report.part = self.report.part.saturating_add(1);
        r.bounded()
    }
}

pub fn loss_permille(lost: u64, sent: u64) -> u16 {
    u16_of(lost * 1000 / sent.max(lost).max(1))
}

pub fn u16_of(v: impl TryInto<u16>) -> u16 {
    v.try_into().unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loss_never_passes_all_of_it() {
        assert_eq!(loss_permille(0, 0), 0);
        assert_eq!(loss_permille(5, 1000), 5);
        assert_eq!(loss_permille(11, 3), 1000);
    }

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

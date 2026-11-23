//! Client side of the connection diagnostics (see `rider_core::diag`):
//! what this end saw of a session, sent to the rendezvous server.

use crate::net::Shared;
use rider_core::diag::{self, Recorder, Report, Role, Route, Sample, u16_of};
use rider_core::identity::Identity;
use rider_core::rendezvous::{self, ServerRef};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Where reports go.
pub struct Target {
    pub identity: Identity,
    pub server: ServerRef,
}

pub struct Diag {
    target: Arc<Target>,
    recorder: Recorder,
    rt: tokio::runtime::Handle,
    next: Instant,
    /// Counters at the previous sample.
    frames: u32,
    sent: u64,
    lost: u64,
    rx_bytes: u64,
}

impl Diag {
    pub fn new(target: Target, conn: &quinn::Connection, relayed: bool, rt: tokio::runtime::Handle) -> Self {
        let route = if relayed { Route::Relay } else { Route::Direct };
        let tunnel = rendezvous::tunnel_recent();
        Self {
            target: Arc::new(target),
            recorder: Recorder::new(diag::session_id(conn), Role::Client, route, tunnel),
            rt,
            next: Instant::now() + diag::SAMPLE_EVERY,
            frames: 0,
            sent: 0,
            lost: 0,
            rx_bytes: 0,
        }
    }

    /// Take a sample when one is due (call at least once a second).
    pub fn tick(&mut self, conn: &quinn::Connection, shared: &Shared) {
        if Instant::now() < self.next {
            return;
        }
        self.next += diag::SAMPLE_EVERY;
        let stats = conn.stats();
        let secs = diag::SAMPLE_EVERY.as_secs().max(1);
        let frames = shared.frames.load(Ordering::Relaxed);
        let sent = stats.path.sent_packets.saturating_sub(self.sent);
        let lost = stats.path.lost_packets.saturating_sub(self.lost);
        let rx = stats.udp_rx.bytes.saturating_sub(self.rx_bytes);
        let sample = Sample {
            rtt_ms: u16_of(shared.rtt_us.load(Ordering::Relaxed) / 1000),
            min_rtt_ms: u16_of(stats.path.min_rtt.as_millis()),
            loss_permille: u16_of(lost * 1000 / sent.max(1)),
            cwnd_kb: u32::try_from(stats.path.cwnd / 1024).unwrap_or(u32::MAX),
            fps: u16_of(u64::from(frames.wrapping_sub(self.frames)) / secs),
            kbps: u32::try_from(rx * 8 / 1000 / secs).unwrap_or(u32::MAX),
            frame_ms: shared.decode_us.load(Ordering::Relaxed) as f32 / 1000.0,
            latency_ms: u16_of(shared.latency_us.load(Ordering::Relaxed) / 1000),
            ..Sample::default()
        };
        (self.frames, self.sent, self.lost, self.rx_bytes) =
            (frames, stats.path.sent_packets, stats.path.lost_packets, stats.udp_rx.bytes);
        if let Some(report) = self.recorder.push(sample) {
            let target = self.target.clone();
            self.rt.spawn(async move { send(&target, report).await });
        }
    }

    /// A new connection after a lost one: report the old session (in the
    /// background) and record the new one from zero.
    pub fn restart(&mut self, conn: &quinn::Connection, relayed: bool, ended: &str) {
        let route = if relayed { Route::Relay } else { Route::Direct };
        let fresh = Recorder::new(diag::session_id(conn), Role::Client, route, rendezvous::tunnel_recent());
        let report = std::mem::replace(&mut self.recorder, fresh).finish(ended);
        let target = self.target.clone();
        self.rt.spawn(async move { send(&target, report).await });
        self.next = Instant::now() + diag::SAMPLE_EVERY;
        (self.sent, self.lost, self.rx_bytes) = (0, 0, 0);
    }

    /// Send the last report; waits a few seconds at most.
    pub fn finish(self, rt: &tokio::runtime::Runtime, ended: &str) {
        let report = self.recorder.finish(ended);
        // The timeout must be created inside the runtime (it needs its timer);
        // building it outside panicked at the end of every reported session.
        let _ = rt.block_on(async { tokio::time::timeout(Duration::from_secs(8), send(&self.target, report)).await });
    }
}

async fn send(target: &Target, report: Report) {
    if let Err(e) = rendezvous::send_report(&target.identity, &target.server, report).await {
        tracing::debug!("diagnostics report not sent: {e:#}");
    }
}

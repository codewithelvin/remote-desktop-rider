use super::HostState;
use super::stream::send_report;
use crate::video::{self, VideoCmd, VideoStats};
use rider_core::diag::{self, Recorder, Sample, u16_of};
use rider_core::proto::HostMsg;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub(super) const MIN_BITRATE: u32 = 300_000;
pub(super) const START_BITRATE: u32 = 4_000_000;
const PEAK_DECAY: f64 = 0.97;
const TICK: Duration = Duration::from_millis(500);

pub(super) struct Ticker {
    pub conn: quinn::Connection,
    pub stats: Arc<VideoStats>,
    pub cmd_tx: std::sync::mpsc::Sender<VideoCmd>,
    pub out_tx: mpsc::UnboundedSender<HostMsg>,
    pub expired: Arc<AtomicU32>,
    pub window: Arc<AtomicUsize>,
    pub quality: Arc<AtomicU32>,
    pub recorder: Arc<Mutex<Option<Recorder>>>,
    pub state: Arc<HostState>,
    pub tunnel: bool,
    pub max_fps: u32,
    pub max_bitrate: u32,
}

#[derive(Default)]
struct Totals {
    frames: u32,
    bytes: u64,
    encode_us: u64,
}

#[derive(Default)]
struct DiagTotals {
    sample: Sample,
    work: Totals,
    sent: u64,
    lost: u64,
}

struct Tick {
    path: quinn::PathStats,
    work: Totals,
    skips: u32,
    expired: u32,
    sent: u64,
    lost: u64,
}

impl Tick {
    fn loss(&self) -> f64 {
        self.lost as f64 / self.sent as f64
    }

    fn queueing(&self) -> bool {
        let p = &self.path;
        p.rtt > p.min_rtt + (p.min_rtt / 2).max(Duration::from_millis(25))
    }
}

struct Control {
    bitrate: u32,
    peak: u32,
    auto: AutoResolution,
    shown: Totals,
    diag: DiagTotals,
    last: quinn::PathStats,
    n: u32,
}

impl Ticker {
    pub(super) async fn run(self) {
        let mut c = Control {
            bitrate: START_BITRATE.min(self.max_bitrate),
            peak: 0,
            auto: AutoResolution::default(),
            shown: Totals::default(),
            diag: DiagTotals::default(),
            last: self.conn.stats().path,
            n: 0,
        };
        let per_sample = (diag::SAMPLE_EVERY.as_millis() / TICK.as_millis()) as u32;
        let mut interval = tokio::time::interval(TICK);
        loop {
            interval.tick().await;
            c.n += 1;
            let t = self.measure(&mut c.last);
            self.adapt(&mut c, &t);
            c.shown.add(&t.work);
            if c.n.is_multiple_of(2) {
                self.report_stats(&mut c.shown);
            }
            c.diag.add(&t);
            if c.n.is_multiple_of(per_sample) {
                self.record(&mut c.diag, &t.path, c.bitrate);
            }
        }
    }

    fn measure(&self, last: &mut quinn::PathStats) -> Tick {
        let path = self.conn.stats().path;
        let s = &self.stats;
        let tick = Tick {
            work: Totals {
                frames: s.frames.swap(0, Ordering::Relaxed),
                bytes: s.bytes.swap(0, Ordering::Relaxed),
                encode_us: s.encode_us.swap(0, Ordering::Relaxed),
            },
            skips: s.congestion_skips.swap(0, Ordering::Relaxed),
            expired: self.expired.swap(0, Ordering::Relaxed),
            sent: path.sent_packets.saturating_sub(last.sent_packets).max(1),
            lost: path.lost_packets.saturating_sub(last.lost_packets),
            path,
        };
        *last = path;
        tick
    }

    fn adapt(&self, c: &mut Control, t: &Tick) {
        self.window.store(video::inflight_window(t.path.min_rtt, self.max_fps), Ordering::Relaxed);
        let used = u32::try_from(t.work.bytes * 8 * 2).unwrap_or(u32::MAX);
        c.peak = ((f64::from(c.peak) * PEAK_DECAY) as u32).max(used);
        let congested = t.expired > 0 || t.skips > 2 || t.queueing() || t.loss() > 0.08;
        let new = next_bitrate(c.bitrate, self.max_bitrate, c.peak, congested, t.work.frames, t.loss());
        if new.abs_diff(c.bitrate) > c.bitrate / 20 {
            c.bitrate = new;
            let _ = self.cmd_tx.send(VideoCmd::SetBitrate(new));
        }
        if let Some(h) = c.auto.update(self.quality.load(Ordering::Relaxed), c.bitrate) {
            let _ = self.cmd_tx.send(VideoCmd::MaxHeight(h));
        }
    }

    fn report_stats(&self, shown: &mut Totals) {
        let encoder = self.stats.encoder.lock().unwrap().clone();
        let _ = self.out_tx.send(HostMsg::Stats {
            fps: shown.frames as f32,
            bitrate_kbps: (shown.bytes * 8 / 1000) as u32,
            encode_ms: shown.frame_ms(),
            encoder,
        });
        *shown = Totals::default();
    }

    fn record(&self, d: &mut DiagTotals, path: &quinn::PathStats, bitrate: u32) {
        let mut recorder = self.recorder.lock().unwrap();
        let Some(r) = recorder.as_mut() else { return };
        let secs = diag::SAMPLE_EVERY.as_secs().max(1);
        r.set_encoder(&self.stats.encoder.lock().unwrap());
        let sample = Sample {
            rtt_ms: u16_of(path.rtt.as_millis()),
            min_rtt_ms: u16_of(path.min_rtt.as_millis()),
            loss_permille: diag::loss_permille(d.lost, d.sent),
            cwnd_kb: u32::try_from(path.cwnd / 1024).unwrap_or(u32::MAX),
            fps: u16_of(u64::from(d.work.frames) / secs),
            kbps: u32::try_from(d.work.bytes * 8 / 1000 / secs).unwrap_or(u32::MAX),
            target_kbps: bitrate / 1000,
            frame_ms: d.work.frame_ms(),
            ..d.sample
        };
        if let Some(report) = r.push(sample) {
            send_report(&self.state, report, self.tunnel);
        }
        *d = DiagTotals::default();
    }
}

impl Totals {
    fn add(&mut self, o: &Totals) {
        self.frames += o.frames;
        self.bytes += o.bytes;
        self.encode_us += o.encode_us;
    }

    fn frame_ms(&self) -> f32 {
        if self.frames > 0 { self.encode_us as f32 / self.frames as f32 / 1000.0 } else { 0.0 }
    }
}

impl DiagTotals {
    fn add(&mut self, t: &Tick) {
        self.work.add(&t.work);
        self.sent += t.sent;
        self.lost += t.lost;
        self.sample.dropped = self.sample.dropped.saturating_add(u16_of(t.skips + t.expired));
    }
}

fn next_bitrate(bitrate: u32, max: u32, peak: u32, congested: bool, frames: u32, loss: f64) -> u32 {
    let ceiling = peak.saturating_mul(4).max(START_BITRATE);
    let new = if congested {
        (f64::from(bitrate) * 0.75) as u32
    } else if frames > 0 && loss < 0.02 {
        ((f64::from(bitrate) * 1.08) as u32 + 50_000).min(ceiling.max(bitrate))
    } else {
        bitrate
    };
    let new = if new > ceiling { new.min(((f64::from(bitrate) * 0.95) as u32).max(ceiling)) } else { new };
    new.clamp(MIN_BITRATE, max.max(MIN_BITRATE))
}

#[derive(Default)]
struct AutoResolution {
    current: u32,
    pending: Option<(u32, Instant)>,
    choice: u32,
}

impl AutoResolution {
    const DOWN_AFTER: Duration = Duration::from_secs(4);
    const UP_AFTER: Duration = Duration::from_secs(20);

    fn wanted(bitrate: u32) -> u32 {
        match bitrate {
            ..900_000 => 480,
            900_000..2_200_000 => 720,
            _ => 0,
        }
    }

    fn size(cap: u32) -> u32 {
        if cap == 0 { u32::MAX } else { cap }
    }

    fn update(&mut self, choice: u32, bitrate: u32) -> Option<u32> {
        if choice != self.choice {
            (self.choice, self.current, self.pending) = (choice, 0, None);
            return None;
        }
        if choice != 0 {
            return None;
        }
        let want = Self::wanted(bitrate);
        if want == self.current {
            self.pending = None;
            return None;
        }
        let since = match self.pending {
            Some((p, t)) if p == want => t,
            _ => {
                self.pending = Some((want, Instant::now()));
                return None;
            }
        };
        let wait = if Self::size(want) < Self::size(self.current) { Self::DOWN_AFTER } else { Self::UP_AFTER };
        if since.elapsed() < wait {
            return None;
        }
        self.current = want;
        self.pending = None;
        Some(want)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrate_recovers_at_low_frame_rates() {
        let mut b = MIN_BITRATE;
        for _ in 0..40 {
            b = next_bitrate(b, 20_000_000, 600_000, false, 3, 0.0);
        }
        assert!(b >= START_BITRATE, "back up within 20 s: {b}");
    }

    #[test]
    fn bitrate_follows_use_and_backs_off() {
        let mut b = START_BITRATE;
        for _ in 0..100 {
            b = next_bitrate(b, 20_000_000, 300_000, false, 2, 0.0);
        }
        assert_eq!(b, START_BITRATE);
        for _ in 0..100 {
            b = next_bitrate(b, 20_000_000, 3_500_000, false, 8, 0.0);
        }
        assert_eq!(b, 14_000_000);
        let calm = next_bitrate(b, 20_000_000, 300_000, false, 2, 0.0);
        assert_eq!(calm, 13_300_000);
        assert_eq!(next_bitrate(b, 20_000_000, 3_500_000, true, 8, 0.0), 10_500_000);
        assert_eq!(next_bitrate(MIN_BITRATE, 20_000_000, 0, true, 0, 0.5), MIN_BITRATE);
        assert_eq!(next_bitrate(9_000_000, 8_000_000, 5_000_000, false, 8, 0.0), 8_000_000);
    }

    #[test]
    fn steps_down_quickly_up_slowly_and_respects_choice() {
        let mut a = AutoResolution::default();
        assert_eq!(a.update(0, 5_000_000), None, "full resolution already");
        assert_eq!(a.update(0, 600_000), None, "not at once");
        a.pending = a.pending.map(|(p, t)| (p, t.checked_sub(AutoResolution::DOWN_AFTER).unwrap()));
        assert_eq!(a.update(0, 600_000), Some(480));
        assert_eq!(a.update(0, 3_000_000), None);
        a.pending = a.pending.map(|(p, t)| (p, t.checked_sub(AutoResolution::DOWN_AFTER).unwrap()));
        assert_eq!(a.update(0, 3_000_000), None, "going up waits longer");
        a.pending = a.pending.map(|(p, t)| (p, t.checked_sub(AutoResolution::UP_AFTER).unwrap()));
        assert_eq!(a.update(0, 3_000_000), Some(0));
        assert_eq!(a.update(720, 100_000), None);
        assert_eq!(a.update(720, 100_000), None);
        assert!(a.pending.is_none());
    }
}

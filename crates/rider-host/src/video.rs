use crate::capture::{self, Monitor};
use rider_core::pixels::bgra_to_nv12;
use rider_core::proto::FrameHeader;
use rider_core::sharp::{Grid, RefreshHeader, Tracker};
use rider_media::H264Encoder;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MIN_INFLIGHT: usize = 3;
const MAX_INFLIGHT: usize = 16;
const REFINE_FRAMES: u32 = 6;
const REFINE_INTERVAL: Duration = Duration::from_millis(120);
const MIN_KEYFRAME_GAP: Duration = Duration::from_millis(250);
const STILL: Duration = Duration::from_millis(500);

#[derive(Clone, Copy)]
pub enum VideoCmd {
    SelectMonitor(usize),
    Keyframe,
    SetBitrate(u32),
    MaxHeight(u32),
    SharpLost,
}

pub struct VideoPacket {
    pub header: FrameHeader,
    pub dirty: Vec<u8>,
    pub data: Vec<u8>,
}

pub struct RefreshJob {
    pub header: RefreshHeader,
    pub grid: Grid,
    pub tiles: Vec<(usize, Vec<u8>)>,
}

pub struct SharpSink {
    pub jobs: tokio::sync::mpsc::UnboundedSender<RefreshJob>,
    pub busy: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct VideoStats {
    pub frames: AtomicU32,
    pub bytes: AtomicU64,
    pub encode_us: AtomicU64,
    pub congestion_skips: AtomicU32,
    pub encoder: Mutex<String>,
    pub failure: Mutex<Option<String>>,
}

pub struct VideoConfig {
    pub max_fps: u32,
    pub start_bitrate_bps: u32,
    pub hardware: bool,
}

pub struct Pipeline {
    pub monitors: Vec<Monitor>,
    pub start_monitor: usize,
    pub cfg: VideoConfig,
    pub tx: tokio::sync::mpsc::Sender<VideoPacket>,
    pub cmds: Receiver<VideoCmd>,
    pub inflight: Arc<AtomicUsize>,
    pub window: Arc<AtomicUsize>,
    pub stats: Arc<VideoStats>,
    pub stop: Arc<AtomicBool>,
    pub sharp: Option<SharpSink>,
}

pub fn inflight_window(rtt: Duration, max_fps: u32) -> usize {
    let interval = Duration::from_secs(1) / max_fps.max(1);
    let frames = (rtt + Duration::from_millis(40)).as_nanos().div_ceil(interval.as_nanos().max(1));
    (frames as usize + 1).clamp(MIN_INFLIGHT, MAX_INFLIGHT)
}

pub fn spawn(p: Pipeline) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("rider-video".into())
        .spawn(move || {
            let stats = p.stats.clone();
            if let Err(e) = run(p) {
                tracing::error!("video pipeline stopped: {e:#}");
                *stats.failure.lock().unwrap() = Some(format!("{e:#}"));
            }
        })
        .expect("spawn video thread")
}

#[derive(Default)]
struct Picture {
    nv12: Vec<u8>,
    bgra: Vec<u8>,
    keep_bgra: bool,
    scaler: rider_core::pixels::Downscaler,
    width: usize,
    height: usize,
}

impl Picture {
    fn convert(&mut self, f: &capture::Frame<'_>, max_height: usize) {
        let (sw, sh) = (f.width & !1, f.height & !1);
        if max_height > 0 && sh > max_height {
            self.height = max_height & !1;
            self.width = (sw * self.height / sh) & !1;
            let small = self.scaler.scale(f.data, f.stride, sw, sh, self.width, self.height);
            bgra_to_nv12(small, self.width * 4, self.width, self.height, &mut self.nv12);
            if self.keep_bgra {
                self.bgra.clear();
                self.bgra.extend_from_slice(&small[..self.width * self.height * 4]);
            }
        } else {
            (self.width, self.height) = (sw, sh);
            bgra_to_nv12(f.data, f.stride, self.width, self.height, &mut self.nv12);
            if self.keep_bgra {
                self.bgra.clear();
                for row in f.data.chunks(f.stride).take(self.height) {
                    self.bgra.extend_from_slice(&row[..self.width * 4]);
                }
            }
        }
    }
}

enum Grab {
    Fresh,
    Unchanged,
    Reopened,
}

struct Streamer {
    p: Pipeline,
    interval: Duration,
    epoch: Instant,
    monitor: usize,
    cap: Box<dyn capture::Capturer>,
    enc: Option<H264Encoder>,
    bitrate: u32,
    max_height: usize,
    picture: Picture,
    capture_us: u64,
    have_frame: bool,
    force_kf: bool,
    last_kf: Instant,
    refine_left: u32,
    last_encode: Instant,
    seq: u64,
    congested: bool,
    tracker: Option<Tracker>,
    dirty: Vec<u8>,
    last_fresh: Instant,
}

fn run(p: Pipeline) -> anyhow::Result<()> {
    rider_media::init_thread()?;
    let mut s = Streamer::new(p)?;
    while !s.p.stop.load(Ordering::Relaxed) {
        while let Ok(cmd) = s.p.cmds.try_recv() {
            s.apply(cmd)?;
        }
        if s.no_room() {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        s.pace();
        let fresh = match s.grab()? {
            Grab::Fresh => true,
            Grab::Unchanged => false,
            Grab::Reopened => continue,
        };
        if s.have_frame && s.due(fresh) {
            if !s.encode_and_send()? {
                return Ok(());
            }
        } else if !fresh {
            s.refresh_if_still();
        }
    }
    Ok(())
}

impl Streamer {
    fn new(p: Pipeline) -> anyhow::Result<Self> {
        let interval = Duration::from_secs(1) / p.cfg.max_fps.max(1);
        let monitor = p.start_monitor.min(p.monitors.len() - 1);
        let cap = capture::open(&p.monitors[monitor])?;
        tracing::info!("capturing {} via {}", p.monitors[monitor].info.name, cap.backend());
        let tracker = p.sharp.as_ref().map(|_| Tracker::new());
        let picture = Picture { keep_bgra: tracker.is_some(), ..Picture::default() };
        Ok(Self {
            interval,
            epoch: Instant::now(),
            monitor,
            cap,
            enc: None,
            bitrate: p.cfg.start_bitrate_bps,
            max_height: 0,
            picture,
            capture_us: 0,
            have_frame: false,
            force_kf: true,
            last_kf: Instant::now().checked_sub(MIN_KEYFRAME_GAP).unwrap(),
            refine_left: 0,
            last_encode: Instant::now().checked_sub(interval).unwrap(),
            seq: 0,
            congested: false,
            tracker,
            dirty: Vec::new(),
            last_fresh: Instant::now(),
            p,
        })
    }

    fn reopen(&mut self) -> anyhow::Result<()> {
        self.cap = capture::open(&self.p.monitors[self.monitor])?;
        Ok(())
    }

    fn apply(&mut self, cmd: VideoCmd) -> anyhow::Result<()> {
        match cmd {
            VideoCmd::SelectMonitor(i) if i < self.p.monitors.len() && i != self.monitor => {
                self.monitor = i;
                self.reopen()?;
                tracing::info!("switched to {} via {}", self.p.monitors[i].info.name, self.cap.backend());
                self.enc = None;
                self.have_frame = false;
                self.force_kf = true;
            }
            VideoCmd::Keyframe => self.force_kf = true,
            VideoCmd::MaxHeight(m) if m as usize != self.max_height => {
                self.max_height = m as usize;
                self.reopen()?;
                tracing::info!("max height {}", if m == 0 { "full".into() } else { m.to_string() });
                self.have_frame = false;
                self.force_kf = true;
            }
            VideoCmd::SelectMonitor(_) | VideoCmd::MaxHeight(_) => {}
            VideoCmd::SharpLost => {
                if let Some(t) = self.tracker.as_mut() {
                    t.lost();
                }
            }
            VideoCmd::SetBitrate(b) => {
                self.bitrate = b;
                if let Some(e) = self.enc.as_mut() {
                    e.set_bitrate(b);
                }
            }
        }
        Ok(())
    }

    fn no_room(&mut self) -> bool {
        let full = self.p.inflight.load(Ordering::Acquire) >= self.p.window.load(Ordering::Relaxed);
        if full && !self.congested {
            self.p.stats.congestion_skips.fetch_add(1, Ordering::Relaxed);
        }
        self.congested = full;
        full
    }

    fn pace(&self) {
        let since = self.last_encode.elapsed();
        if since < self.interval {
            std::thread::sleep(self.interval.checked_sub(since).unwrap());
        }
    }

    fn grab(&mut self) -> anyhow::Result<Grab> {
        match self.cap.next_frame(50) {
            Ok(Some(f)) => {
                self.picture.convert(&f, self.max_height);
                self.capture_us = rider_core::now_us();
                self.have_frame = true;
                self.refine_left = REFINE_FRAMES;
                self.last_fresh = Instant::now();
                if let Some(t) = self.tracker.as_mut() {
                    let dirty = t.update(&self.picture.bgra, self.picture.width, self.picture.height);
                    if self.dirty.len() == dirty.len() {
                        self.dirty.iter_mut().zip(&dirty).for_each(|(a, b)| *a |= b);
                    } else {
                        self.dirty = dirty;
                    }
                }
                Ok(Grab::Fresh)
            }
            Ok(None) => Ok(Grab::Unchanged),
            Err(e) => {
                tracing::warn!("capture error: {e:#}; reopening");
                std::thread::sleep(Duration::from_millis(250));
                self.reopen()?;
                Ok(Grab::Reopened)
            }
        }
    }

    fn keyframe_due(&self) -> bool {
        self.force_kf && self.last_kf.elapsed() >= MIN_KEYFRAME_GAP
    }

    fn due(&mut self, fresh: bool) -> bool {
        if fresh {
            return true;
        }
        let refine_due = self.refine_left > 0 && self.last_encode.elapsed() >= REFINE_INTERVAL;
        if !(refine_due || self.keyframe_due()) {
            return false;
        }
        self.refine_left = self.refine_left.saturating_sub(1);
        self.capture_us = rider_core::now_us();
        true
    }

    fn encode(&mut self) -> anyhow::Result<(Vec<rider_media::EncodedFrame>, u32, bool)> {
        let (w, h) = (self.picture.width, self.picture.height);
        let enc = match &mut self.enc {
            Some(e) if e.width as usize == w && e.height as usize == h => e,
            slot => {
                drop(slot.take());
                let cfg = &self.p.cfg;
                let e = H264Encoder::new(w as u32, h as u32, cfg.max_fps, self.bitrate, cfg.hardware)?;
                self.p.stats.encoder.lock().unwrap().clone_from(&e.name);
                self.force_kf = true;
                slot.insert(e)
            }
        };
        let want_kf = self.force_kf && self.last_kf.elapsed() >= MIN_KEYFRAME_GAP;
        let t0 = Instant::now();
        let time_100ns = (self.epoch.elapsed().as_nanos() / 100) as i64;
        let packets = enc.encode(&self.picture.nv12, time_100ns, want_kf)?;
        Ok((packets, t0.elapsed().as_micros() as u32, want_kf))
    }

    fn refresh_if_still(&mut self) {
        let (Some(t), Some(sink)) = (self.tracker.as_mut(), self.p.sharp.as_ref()) else { return };
        if self.seq == 0 || self.last_fresh.elapsed() < STILL || sink.busy.load(Ordering::Acquire) {
            return;
        }
        let unsent = t.unsent();
        if unsent.is_empty() {
            return;
        }
        let grid = t.grid();
        let header = RefreshHeader { seq: self.seq - 1, width: grid.width as u32, height: grid.height as u32, size: 0 };
        let tiles = t.take(&unsent);
        sink.busy.store(true, Ordering::Release);
        if sink.jobs.send(RefreshJob { header, grid, tiles }).is_err() {
            sink.busy.store(false, Ordering::Release);
            t.lost();
        }
    }

    fn encode_and_send(&mut self) -> anyhow::Result<bool> {
        let (packets, encode_us, keyframe) = self.encode()?;
        self.last_encode = Instant::now();
        if keyframe {
            self.force_kf = false;
            self.last_kf = Instant::now();
        }
        let stats = &self.p.stats;
        stats.encode_us.fetch_add(u64::from(encode_us), Ordering::Relaxed);
        for p in packets {
            stats.frames.fetch_add(1, Ordering::Relaxed);
            stats.bytes.fetch_add(p.data.len() as u64, Ordering::Relaxed);
            let header = FrameHeader {
                seq: self.seq,
                capture_us: self.capture_us,
                encode_us,
                keyframe: p.keyframe,
                width: self.picture.width as u32,
                height: self.picture.height as u32,
            };
            self.seq += 1;
            self.p.inflight.fetch_add(1, Ordering::AcqRel);
            let dirty = std::mem::take(&mut self.dirty);
            if self.p.tx.blocking_send(VideoPacket { header, dirty, data: p.data }).is_err() {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_covers_a_round_trip() {
        let ms = Duration::from_millis;
        assert_eq!(inflight_window(ms(2), 60), 4, "LAN: delayed ack + 1");
        assert_eq!(inflight_window(ms(2), 15), MIN_INFLIGHT);
        assert_eq!(inflight_window(ms(150), 60), 13);
        assert_eq!(inflight_window(ms(150), 30), 7);
        assert_eq!(inflight_window(ms(900), 60), MAX_INFLIGHT, "capped");
    }
}

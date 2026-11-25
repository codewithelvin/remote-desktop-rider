use crate::capture::{self, Monitor};
use rider_core::pixels::bgra_to_nv12;
use rider_core::proto::FrameHeader;
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

pub enum VideoCmd {
    SelectMonitor(usize),
    Keyframe,
    SetBitrate(u32),
    MaxHeight(u32),
}

pub struct VideoPacket {
    pub header: FrameHeader,
    pub data: Vec<u8>,
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

fn run(p: Pipeline) -> anyhow::Result<()> {
    let Pipeline { monitors, start_monitor, cfg, tx, cmds, inflight, window, stats, stop } = p;
    rider_media::init_thread()?;
    let interval = Duration::from_secs(1) / cfg.max_fps.max(1);
    let epoch = Instant::now();
    let mut monitor = start_monitor.min(monitors.len() - 1);
    let mut cap = capture::open(&monitors[monitor])?;
    tracing::info!("capturing {} via {}", monitors[monitor].info.name, cap.backend());
    let mut enc: Option<H264Encoder> = None;
    let mut bitrate = cfg.start_bitrate_bps;
    let mut nv12 = Vec::new();
    let mut max_height = 0usize;
    let mut scaler = rider_core::pixels::Downscaler::default();
    let (mut w, mut h) = (0usize, 0usize);
    let mut capture_us = 0u64;
    let mut have_frame = false;
    let mut force_kf = true;
    let mut last_kf = Instant::now().checked_sub(MIN_KEYFRAME_GAP).unwrap();
    let mut refine_left = 0u32;
    let mut last_encode = Instant::now().checked_sub(interval).unwrap();
    let mut seq = 0u64;
    let mut congested = false;

    while !stop.load(Ordering::Relaxed) {
        while let Ok(cmd) = cmds.try_recv() {
            match cmd {
                VideoCmd::SelectMonitor(i) if i < monitors.len() && i != monitor => {
                    monitor = i;
                    cap = capture::open(&monitors[monitor])?;
                    tracing::info!("switched to {} via {}", monitors[monitor].info.name, cap.backend());
                    enc = None;
                    have_frame = false;
                    force_kf = true;
                }
                VideoCmd::Keyframe => force_kf = true,
                VideoCmd::MaxHeight(m) if m as usize != max_height => {
                    max_height = m as usize;
                    cap = capture::open(&monitors[monitor])?;
                    tracing::info!("max height {}", if m == 0 { "full".into() } else { m.to_string() });
                    have_frame = false;
                    force_kf = true;
                }
                VideoCmd::SelectMonitor(_) | VideoCmd::MaxHeight(_) => {}
                VideoCmd::SetBitrate(b) => {
                    bitrate = b;
                    if let Some(e) = enc.as_mut() {
                        e.set_bitrate(b);
                    }
                }
            }
        }

        if inflight.load(Ordering::Acquire) >= window.load(Ordering::Relaxed) {
            if !congested {
                stats.congestion_skips.fetch_add(1, Ordering::Relaxed);
                congested = true;
            }
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        congested = false;

        let since = last_encode.elapsed();
        if since < interval {
            std::thread::sleep(interval.checked_sub(since).unwrap());
        }

        let fresh = match cap.next_frame(50) {
            Ok(Some(f)) => {
                let (sw, sh) = (f.width & !1, f.height & !1);
                if max_height > 0 && sh > max_height {
                    h = max_height & !1;
                    w = (sw * h / sh) & !1;
                    let small = scaler.scale(f.data, f.stride, sw, sh, w, h);
                    bgra_to_nv12(small, w * 4, w, h, &mut nv12);
                } else {
                    (w, h) = (sw, sh);
                    bgra_to_nv12(f.data, f.stride, w, h, &mut nv12);
                }
                capture_us = rider_core::now_us();
                have_frame = true;
                refine_left = REFINE_FRAMES;
                true
            }
            Ok(None) => false,
            Err(e) => {
                tracing::warn!("capture error: {e:#}; reopening");
                std::thread::sleep(Duration::from_millis(250));
                cap = capture::open(&monitors[monitor])?;
                continue;
            }
        };
        if !have_frame {
            continue;
        }
        let keyframe_due = force_kf && last_kf.elapsed() >= MIN_KEYFRAME_GAP;
        if !fresh {
            let refine_due = refine_left > 0 && last_encode.elapsed() >= REFINE_INTERVAL;
            if !(refine_due || keyframe_due) {
                continue;
            }
            refine_left = refine_left.saturating_sub(1);
            capture_us = rider_core::now_us();
        }

        if enc.as_ref().is_none_or(|e| e.width as usize != w || e.height as usize != h) {
            drop(enc.take());
            let e = H264Encoder::new(w as u32, h as u32, cfg.max_fps, bitrate, cfg.hardware)?;
            stats.encoder.lock().unwrap().clone_from(&e.name);
            enc = Some(e);
            force_kf = true;
        }
        let want_kf = force_kf && last_kf.elapsed() >= MIN_KEYFRAME_GAP;
        let t0 = Instant::now();
        let time_100ns = (epoch.elapsed().as_nanos() / 100) as i64;
        let packets = enc.as_mut().unwrap().encode(&nv12, time_100ns, want_kf)?;
        let encode_us = t0.elapsed().as_micros() as u32;
        last_encode = Instant::now();
        if want_kf {
            force_kf = false;
            last_kf = Instant::now();
        }
        stats.encode_us.fetch_add(u64::from(encode_us), Ordering::Relaxed);

        for p in packets {
            stats.frames.fetch_add(1, Ordering::Relaxed);
            stats.bytes.fetch_add(p.data.len() as u64, Ordering::Relaxed);
            let header =
                FrameHeader { seq, capture_us, encode_us, keyframe: p.keyframe, width: w as u32, height: h as u32 };
            seq += 1;
            inflight.fetch_add(1, Ordering::AcqRel);
            if tx.blocking_send(VideoPacket { header, data: p.data }).is_err() {
                return Ok(());
            }
        }
    }
    Ok(())
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

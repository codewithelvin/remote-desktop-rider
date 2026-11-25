use crate::net::{Shared, UiEvent};
use rider_core::proto::{ClientMsg, FrameHeader};
use rider_media::H264Decoder;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use winit::event_loop::EventLoopProxy;

const GAP_TIMEOUT: Duration = Duration::from_millis(80);
const MAX_PENDING: usize = 6;
const KEYFRAME_RETRY: Duration = Duration::from_millis(300);

pub struct IncomingFrame {
    pub header: FrameHeader,
    pub data: Vec<u8>,
}

pub fn spawn_decoder(
    conn: usize,
    rx: Receiver<IncomingFrame>,
    ctrl: UnboundedSender<ClientMsg>,
    shared: Arc<Shared>,
    proxy: EventLoopProxy<UiEvent>,
) {
    std::thread::Builder::new()
        .name("rider-decode".into())
        .spawn(move || {
            if let Err(e) = run(&rx, &ctrl, &shared, &proxy) {
                let _ =
                    proxy.send_event(UiEvent::Closed { conn, reason: format!("decoder failed: {e:#}"), lost: false });
            }
        })
        .expect("spawn decoder thread");
}

fn run(
    rx: &Receiver<IncomingFrame>,
    ctrl: &UnboundedSender<ClientMsg>,
    shared: &Shared,
    proxy: &EventLoopProxy<UiEvent>,
) -> anyhow::Result<()> {
    rider_media::init_thread()?;
    let mut decoder: Option<H264Decoder> = None;
    let mut dims = (0u32, 0u32);
    let mut pending: BTreeMap<u64, (IncomingFrame, Instant)> = BTreeMap::new();
    let mut next: Option<u64> = None;
    let mut need_keyframe = true;
    let mut last_kf_request = Instant::now().checked_sub(KEYFRAME_RETRY).unwrap();

    loop {
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(f) => {
                if next.is_none_or(|n| f.header.seq >= n) {
                    pending.insert(f.header.seq, (f, Instant::now()));
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }

        loop {
            if need_keyframe {
                let Some(seq) = pending.iter().find(|(_, (f, _))| f.header.keyframe).map(|(s, _)| *s) else {
                    if last_kf_request.elapsed() >= KEYFRAME_RETRY {
                        let _ = ctrl.send(ClientMsg::RequestKeyframe);
                        last_kf_request = Instant::now();
                    }
                    while pending.len() > MAX_PENDING {
                        pending.pop_first();
                    }
                    break;
                };
                pending = pending.split_off(&seq);
                need_keyframe = false;
                next = Some(seq);
            }
            let n = next.unwrap();
            if let Some((f, _)) = pending.remove(&n) {
                next = Some(n + 1);
                if !decode(&mut decoder, &mut dims, &f, shared, proxy) {
                    decoder = None;
                    need_keyframe = true;
                }
                continue;
            }
            let stalled = pending.first_key_value().is_some_and(|(_, (_, t))| t.elapsed() > GAP_TIMEOUT)
                || pending.len() > MAX_PENDING;
            if stalled {
                tracing::debug!("frame {n} lost; requesting keyframe");
                need_keyframe = true;
                continue;
            }
            break;
        }
    }
}

fn decode(
    decoder: &mut Option<H264Decoder>,
    dims: &mut (u32, u32),
    f: &IncomingFrame,
    shared: &Shared,
    proxy: &EventLoopProxy<UiEvent>,
) -> bool {
    let h = &f.header;
    if decoder.is_none() || *dims != (h.width, h.height) {
        match H264Decoder::new(h.width, h.height) {
            Ok(d) => {
                *decoder = Some(d);
                *dims = (h.width, h.height);
            }
            Err(e) => {
                tracing::error!("cannot create decoder: {e:#}");
                return false;
            }
        }
    }
    let t0 = Instant::now();
    let frames = match decoder.as_mut().unwrap().decode(&f.data) {
        Ok(fr) => fr,
        Err(e) => {
            tracing::warn!("decode error: {e:#}");
            return false;
        }
    };
    let decode_us = t0.elapsed().as_micros() as u64;
    if let Some(mut pic) = frames.into_iter().last() {
        pic.width = pic.width.min(h.width as usize);
        pic.height = pic.height.min(h.height as usize);
        let offset = shared.clock_offset_us.load(Ordering::Relaxed);
        let now_host_clock = rider_core::now_us() as i64 + offset;
        let latency = (now_host_clock - h.capture_us as i64).max(0) as u64;
        shared.latency_us.store(latency, Ordering::Relaxed);
        shared.decode_us.store(decode_us, Ordering::Relaxed);
        shared.frames.fetch_add(1, Ordering::Relaxed);
        *shared.latest.lock().unwrap() = Some(pic);
        let _ = proxy.send_event(UiEvent::NewFrame);
    }
    true
}

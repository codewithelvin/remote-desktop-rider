use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const ACK_PORT: u16 = 0;
pub const ACK_EVERY_BYTES: u64 = 4096;
pub const ACK_EVERY: Duration = Duration::from_millis(20);
const TARGET_QUEUE: Duration = Duration::from_millis(100);
const PACKET: u64 = 2 + 1200;
const MIN_WINDOW: u64 = 4 * PACKET;
const START_WINDOW: u64 = 10 * PACKET;
const BANDWIDTH_ROUNDS: u32 = 10;
const RATE_SPAN: Duration = Duration::from_millis(500);
const RTT_MEMORY: Duration = Duration::from_secs(10);
const MAX_PENDING: usize = 8192;
const PROBE_GIVE_UP: Duration = Duration::from_secs(2);

pub fn encode_ack(total: u64) -> [u8; 10] {
    let mut f = [0u8; 10];
    f[..2].copy_from_slice(&ACK_PORT.to_be_bytes());
    f[2..].copy_from_slice(&total.to_be_bytes());
    f
}

pub fn decode_ack(frame: &[u8]) -> Option<u64> {
    let (port, total) = frame.split_first_chunk::<2>()?;
    let total: &[u8; 8] = total.try_into().ok()?;
    (u16::from_be_bytes(*port) == ACK_PORT).then(|| u64::from_be_bytes(*total))
}

#[derive(Default)]
pub struct Flow {
    sent: u64,
    acked: u64,
    active: bool,
    app_limited_until: u64,
    pending: VecDeque<(u64, Instant)>,
    deliveries: VecDeque<(Instant, u64)>,
    rates: VecDeque<(Instant, f64)>,
    recent_rate: Option<f64>,
    latest_rtt: Duration,
    min_rtt: Option<(Duration, Instant)>,
    probe: Option<Probe>,
}

struct Probe {
    since: Instant,
    from: Option<u64>,
    drains: bool,
}

impl Flow {
    pub fn sent(&mut self, bytes: usize, now: Instant, app_limited: bool) {
        if let Some(p) = &mut self.probe
            && p.from.is_none()
            && self.sent - self.acked <= MIN_WINDOW
        {
            p.from = Some(self.sent);
        }
        self.sent += bytes as u64;
        if app_limited {
            self.app_limited_until = self.sent;
        }
        if self.active && self.pending.len() < MAX_PENDING {
            self.pending.push_back((self.sent, now));
        }
    }

    pub fn acked(&mut self, total: u64, now: Instant) {
        self.active = true;
        let total = total.min(self.sent);
        if total < self.acked {
            return;
        }
        let app_limited = total <= self.app_limited_until;
        self.acked = total;
        if self.probe.is_none() && self.min_rtt.is_some_and(|(_, at)| now.saturating_duration_since(at) > RTT_MEMORY) {
            let quiet = self.app_limited_until >= self.sent;
            self.probe = Some(Probe { since: now, from: quiet.then_some(self.sent), drains: !quiet });
        }
        while let Some(&(end, at)) = self.pending.front() {
            if end > total {
                break;
            }
            self.pending.pop_front();
            self.rtt_sample(end, now.saturating_duration_since(at), now);
        }
        self.delivery_sample(total, now, app_limited);
    }

    fn rtt_sample(&mut self, end: u64, rtt: Duration, now: Instant) {
        let probed = self.probe.as_ref().is_some_and(|p| {
            p.from.is_some_and(|from| end > from) || now.saturating_duration_since(p.since) > PROBE_GIVE_UP
        });
        if probed {
            self.probe = None;
        }
        self.latest_rtt = rtt;
        if probed || self.min_rtt.is_none_or(|(min, _)| rtt <= min) {
            self.min_rtt = Some((rtt, now));
        }
    }

    fn delivery_sample(&mut self, total: u64, now: Instant, app_limited: bool) {
        let rtt = self.min_rtt.map_or(Duration::ZERO, |(rtt, _)| rtt);
        let span = rtt.max(RATE_SPAN);
        self.deliveries.push_back((now, total));
        while self.deliveries.len() > 2 && self.deliveries.get(1).is_some_and(|(t, _)| now - *t >= span) {
            self.deliveries.pop_front();
        }
        let (Some(&(t0, a0)), Some(&(t1, a1))) = (self.deliveries.front(), self.deliveries.back()) else { return };
        let elapsed = t1.saturating_duration_since(t0);
        if elapsed < span {
            return;
        }
        let rate = (a1 - a0) as f64 / elapsed.as_secs_f64();
        self.recent_rate = Some(rate);
        if app_limited && self.bandwidth().is_some_and(|best| rate <= best) {
            return;
        }
        let memory = span * BANDWIDTH_ROUNDS;
        self.rates.push_back((now, rate));
        while self.rates.len() > 1 && self.rates.front().is_some_and(|(t, _)| now - *t > memory) {
            self.rates.pop_front();
        }
    }

    fn bandwidth(&self) -> Option<f64> {
        self.rates.iter().map(|&(_, r)| r).reduce(f64::max)
    }

    pub fn in_flight(&self) -> u64 {
        self.sent - self.acked
    }

    pub fn window(&self) -> u64 {
        if self.probe.as_ref().is_some_and(|p| p.drains) {
            return MIN_WINDOW;
        }
        let Some((rtt, _)) = self.min_rtt else { return START_WINDOW };
        let rate = if self.queue_building() { self.recent_rate } else { self.bandwidth() };
        let Some(rate) = rate else { return START_WINDOW };
        ((rate * (rtt + TARGET_QUEUE).as_secs_f64()) as u64).max(MIN_WINDOW)
    }

    fn queue_building(&self) -> bool {
        self.min_rtt.is_some_and(|(rtt, _)| self.latest_rtt > rtt + TARGET_QUEUE)
    }

    pub fn max_wait(&self) -> Duration {
        match self.min_rtt {
            Some((rtt, _)) if !self.queue_building() => rtt + TARGET_QUEUE,
            _ => TARGET_QUEUE,
        }
    }

    pub fn may_send(&self) -> bool {
        !self.active || self.in_flight() < self.window()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn acks_round_trip_and_are_not_data() {
        assert_eq!(decode_ack(&encode_ack(0)), Some(0));
        assert_eq!(decode_ack(&encode_ack(123_456_789)), Some(123_456_789));
        assert_eq!(decode_ack(&encode_ack(7)[..9]), None);
        assert_eq!(decode_ack(&super::super::encode(47900, &[0; 8])), None);
        assert_eq!(super::super::decode(&encode_ack(5)), None);
    }

    #[test]
    fn without_acks_nothing_is_held_back() {
        let mut f = Flow::default();
        let t = Instant::now();
        for _ in 0..10_000 {
            f.sent(1200, t, false);
        }
        assert!(f.may_send());
    }

    struct Link {
        rate: f64,
        delay: Duration,
        queue: VecDeque<(usize, Instant)>,
        credit: f64,
        acks: VecDeque<(Instant, u64)>,
        received: u64,
        last_ack: (u64, Instant),
        worst_wait: Duration,
    }

    impl Link {
        fn step(&mut self, now: Instant, dt: Duration, flow: &mut Flow, measure: bool) {
            self.credit += self.rate * dt.as_secs_f64();
            while let Some(&(len, at)) = self.queue.front() {
                if self.credit < len as f64 {
                    break;
                }
                self.credit -= len as f64;
                self.queue.pop_front();
                if measure {
                    self.worst_wait = self.worst_wait.max(now - at);
                }
                self.received += len as u64;
                if self.received - self.last_ack.0 >= ACK_EVERY_BYTES || now - self.last_ack.1 >= ACK_EVERY {
                    self.last_ack = (self.received, now);
                    self.acks.push_back((now + self.delay, self.received));
                }
            }
            if self.queue.is_empty() {
                self.credit = self.credit.min(1200.0);
            }
            while self.acks.front().is_some_and(|(t, _)| *t <= now) {
                let (_, total) = self.acks.pop_front().unwrap();
                flow.acked(total, now);
            }
        }
    }

    struct Outcome {
        worst_wait: Duration,
        use_of_link: f64,
        dropped: f64,
    }

    fn steady(ms: u64) -> f64 {
        let _ = ms;
        1_000_000.0 / 8.0 / 1000.0
    }

    fn run(rates: &[(f64, u64)], rtt: Duration, offer: fn(u64) -> f64) -> Outcome {
        let start = Instant::now();
        let dt = Duration::from_millis(1);
        let mut flow = Flow::default();
        let mut link = Link {
            rate: rates[0].0,
            delay: rtt,
            queue: VecDeque::new(),
            credit: 0.0,
            acks: VecDeque::new(),
            received: 0,
            last_ack: (0, start),
            worst_wait: Duration::ZERO,
        };
        flow.acked(0, start);
        let mut waiting: VecDeque<Instant> = VecDeque::new();
        let (mut now, mut offered, mut sent_late, mut packets, mut dropped) = (start, 0.0, 0u64, 0u64, 0u64);
        let total_ms: u64 = rates.iter().map(|r| r.1).sum();
        let mut phase_end = 0;
        for (rate, ms) in rates {
            link.rate = *rate;
            phase_end += ms;
            while (now - start).as_millis() < u128::from(phase_end) {
                now += dt;
                let elapsed = (now - start).as_millis() as u64;
                offered += offer(elapsed);
                while offered >= 1200.0 {
                    offered -= 1200.0;
                    waiting.push_back(now);
                    if elapsed >= total_ms / 2 {
                        packets += 1;
                    }
                }
                while let Some(&queued) = waiting.front() {
                    if now - queued > flow.max_wait() {
                        waiting.pop_front();
                        if elapsed >= total_ms / 2 {
                            dropped += 1;
                        }
                    } else if flow.may_send() {
                        waiting.pop_front();
                        flow.sent(1200, now, waiting.is_empty());
                        link.queue.push_back((1200, now));
                        if elapsed >= total_ms / 2 {
                            sent_late += 1200;
                        }
                    } else {
                        break;
                    }
                }
                let measure = elapsed >= total_ms / 2 + 2000;
                link.step(now, dt, &mut flow, measure);
            }
        }
        let late_secs = (total_ms as f64 / 2.0) / 1000.0;
        Outcome {
            worst_wait: link.worst_wait,
            use_of_link: (sent_late as f64 / late_secs / link.rate).min(1.0),
            dropped: dropped as f64 / packets.max(1) as f64,
        }
    }

    #[test]
    fn a_slow_link_keeps_a_short_queue_and_stays_busy() {
        let o = run(&[(300_000.0 / 8.0, 20_000)], Duration::from_millis(150), steady);
        assert!(o.worst_wait < Duration::from_millis(400), "queue wait {:?}", o.worst_wait);
        assert!(o.use_of_link > 0.8, "link use {}", o.use_of_link);
    }

    #[test]
    fn a_sudden_drop_in_bandwidth_recovers_a_short_queue() {
        let o = run(&[(8_000_000.0 / 8.0, 10_000), (300_000.0 / 8.0, 20_000)], Duration::from_millis(150), steady);
        assert!(o.worst_wait < Duration::from_millis(400), "queue wait {:?}", o.worst_wait);
        assert!(o.use_of_link > 0.8, "link use {}", o.use_of_link);
    }

    #[test]
    fn a_fast_link_is_not_held_back() {
        let o = run(&[(20_000_000.0 / 8.0, 10_000)], Duration::from_millis(150), steady);
        assert!(o.worst_wait < Duration::from_millis(400), "queue wait {:?}", o.worst_wait);
        assert!(o.dropped < 0.001, "dropped {}", o.dropped);
    }

    fn encoder_backing_off_after_the_drop(ms: u64) -> f64 {
        const LINK_DROPS_AT_MS: u64 = 10_000;
        const ENCODER_REACTS_MS: u64 = 3000;
        let kbps = if ms < LINK_DROPS_AT_MS + ENCODER_REACTS_MS { 4000.0 } else { 250.0 };
        kbps * 1000.0 / 8.0 / 1000.0
    }

    #[test]
    fn a_drop_while_the_encoder_backs_off_still_drains_the_queue() {
        let o = run(
            &[(8_000_000.0 / 8.0, 10_000), (300_000.0 / 8.0, 20_000)],
            Duration::from_millis(160),
            encoder_backing_off_after_the_drop,
        );
        assert!(o.worst_wait < Duration::from_millis(400), "queue wait {:?}", o.worst_wait);
    }

    fn video_with_keyframes(ms: u64) -> f64 {
        const KEYFRAME_BYTES: f64 = 60_000.0;
        const KEYFRAME_EVERY_MS: u64 = 2000;
        let steady = 300_000.0 / 8.0 / 1000.0;
        if ms.is_multiple_of(KEYFRAME_EVERY_MS) { steady + KEYFRAME_BYTES } else { steady }
    }

    #[test]
    fn keyframes_on_a_distant_fast_link_are_not_dropped() {
        let o = run(&[(20_000_000.0 / 8.0, 20_000)], Duration::from_millis(160), video_with_keyframes);
        assert!(o.dropped == 0.0, "dropped {:.2} %", o.dropped * 100.0);
        assert!(o.worst_wait < Duration::from_millis(400), "queue wait {:?}", o.worst_wait);
    }
}

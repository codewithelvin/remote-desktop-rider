use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const ACK_PORT: u16 = 0;
pub const ACK_EVERY_BYTES: u64 = 4096;
pub const ACK_EVERY: Duration = Duration::from_millis(20);
const TARGET_QUEUE: Duration = Duration::from_millis(100);
const PACKET: u64 = 2 + 1200;
const MIN_WINDOW: u64 = 4 * PACKET;
const START_WINDOW: u64 = 10 * PACKET;
const RATE_SPAN: Duration = Duration::from_millis(500);
const MIN_RATE_SPAN: Duration = Duration::from_millis(50);
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
    pending: VecDeque<(u64, Instant)>,
    deliveries: VecDeque<(Instant, u64)>,
    min_rtt: Option<(Duration, Instant)>,
    probe: Option<Probe>,
}

struct Probe {
    since: Instant,
    from: Option<u64>,
}

impl Flow {
    pub fn sent(&mut self, bytes: usize, now: Instant) {
        if let Some(p) = &mut self.probe
            && p.from.is_none()
            && self.sent - self.acked <= MIN_WINDOW
        {
            p.from = Some(self.sent);
        }
        self.sent += bytes as u64;
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
        self.acked = total;
        if self.probe.is_none() && self.min_rtt.is_some_and(|(_, at)| now.saturating_duration_since(at) > RTT_MEMORY) {
            self.probe = Some(Probe { since: now, from: None });
        }
        while let Some(&(end, at)) = self.pending.front() {
            if end > total {
                break;
            }
            self.pending.pop_front();
            self.rtt_sample(end, now.saturating_duration_since(at), now);
        }
        self.deliveries.push_back((now, total));
        while self.deliveries.len() > 2 && self.deliveries.front().is_some_and(|(t, _)| now - *t > RATE_SPAN) {
            self.deliveries.pop_front();
        }
    }

    fn rtt_sample(&mut self, end: u64, rtt: Duration, now: Instant) {
        let probed = self.probe.as_ref().is_some_and(|p| {
            p.from.is_some_and(|from| end > from) || now.saturating_duration_since(p.since) > PROBE_GIVE_UP
        });
        if probed {
            self.probe = None;
        }
        if probed || self.min_rtt.is_none_or(|(min, _)| rtt <= min) {
            self.min_rtt = Some((rtt, now));
        }
    }

    pub fn in_flight(&self) -> u64 {
        self.sent - self.acked
    }

    pub fn window(&self) -> u64 {
        if self.probe.is_some() {
            return MIN_WINDOW;
        }
        let (Some((rtt, _)), Some(&(t0, a0)), Some(&(t1, a1))) =
            (self.min_rtt, self.deliveries.front(), self.deliveries.back())
        else {
            return START_WINDOW;
        };
        let span = t1.saturating_duration_since(t0);
        if span < MIN_RATE_SPAN {
            return START_WINDOW;
        }
        let rate = (a1 - a0) as f64 / span.as_secs_f64();
        ((rate * (rtt + TARGET_QUEUE).as_secs_f64()) as u64).max(MIN_WINDOW)
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
            f.sent(1200, t);
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

    fn run(rates: &[(f64, u64)]) -> (Duration, f64) {
        let start = Instant::now();
        let dt = Duration::from_millis(1);
        let mut flow = Flow::default();
        let mut link = Link {
            rate: rates[0].0,
            delay: Duration::from_millis(150),
            queue: VecDeque::new(),
            credit: 0.0,
            acks: VecDeque::new(),
            received: 0,
            last_ack: (0, start),
            worst_wait: Duration::ZERO,
        };
        flow.acked(0, start);
        let (mut now, mut offered, mut sent_late) = (start, 0.0, 0u64);
        let total_ms: u64 = rates.iter().map(|r| r.1).sum();
        let mut phase_end = 0;
        for (rate, ms) in rates {
            link.rate = *rate;
            phase_end += ms;
            while (now - start).as_millis() < u128::from(phase_end) {
                now += dt;
                offered += 1_000_000.0 * dt.as_secs_f64();
                while offered >= 1200.0 {
                    offered -= 1200.0;
                    if flow.may_send() {
                        flow.sent(1200, now);
                        link.queue.push_back((1200, now));
                        if (now - start).as_millis() >= u128::from(total_ms) / 2 {
                            sent_late += 1200;
                        }
                    }
                }
                let measure = (now - start).as_millis() >= u128::from(total_ms) / 2 + 2000;
                link.step(now, dt, &mut flow, measure);
            }
        }
        let late_secs = (total_ms as f64 / 2.0) / 1000.0;
        (link.worst_wait, sent_late as f64 / late_secs / link.rate)
    }

    #[test]
    fn a_slow_link_keeps_a_short_queue_and_stays_busy() {
        let (wait, use_of_link) = run(&[(300_000.0 / 8.0, 20_000)]);
        assert!(wait < Duration::from_millis(400), "queue wait {wait:?}");
        assert!(use_of_link > 0.8, "link use {use_of_link}");
    }

    #[test]
    fn a_sudden_drop_in_bandwidth_recovers_a_short_queue() {
        let (wait, use_of_link) = run(&[(8_000_000.0 / 8.0, 10_000), (300_000.0 / 8.0, 20_000)]);
        assert!(wait < Duration::from_millis(400), "queue wait {wait:?}");
        assert!(use_of_link > 0.8, "link use {use_of_link}");
    }

    #[test]
    fn a_fast_link_is_not_held_back() {
        let (wait, use_of_link) = run(&[(20_000_000.0 / 8.0, 10_000)]);
        assert!(wait < Duration::from_millis(400), "queue wait {wait:?}");
        assert!(use_of_link > 0.0);
    }
}

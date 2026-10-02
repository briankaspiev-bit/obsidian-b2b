//! Clocks.
//!
//! * [`SessionClock`]: monotonic microseconds, epoch-anchored so separate
//!   processes on one host agree (handy for the bench's ground truth). An
//!   artificial offset can be added to simulate two machines whose clocks differ.
//! * [`ClockSync`]: NTP-style four-timestamp exchange with min-RTT filtering.
//!   Used for telemetry (true one-way delay) and for aligning recordings, *not*
//!   for playout timing — playout is anchored on packet arrivals so a sync
//!   error can never move the monitor delay.
//! * [`LineFit`]: least-squares line, used for drift estimation.

use std::collections::VecDeque;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug)]
pub struct SessionClock {
    base_instant: Instant,
    base_epoch_us: i64,
    offset_us: i64,
}

impl SessionClock {
    pub fn new(offset_us: i64) -> Self {
        let base_instant = Instant::now();
        let base_epoch_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros() as i64;
        SessionClock {
            base_instant,
            base_epoch_us,
            offset_us,
        }
    }

    pub fn now_us(&self) -> i64 {
        self.base_epoch_us + self.base_instant.elapsed().as_micros() as i64 + self.offset_us
    }

    /// Convert a session-clock time back to an `Instant` (for sleeping until a deadline).
    pub fn instant_at(&self, t_us: i64) -> Instant {
        let rel = t_us - self.base_epoch_us - self.offset_us;
        if rel >= 0 {
            self.base_instant + std::time::Duration::from_micros(rel as u64)
        } else {
            self.base_instant
        }
    }

    pub fn offset_us(&self) -> i64 {
        self.offset_us
    }
}

/// Sleep until `deadline`, finishing with a short spin for sub-100 µs accuracy.
pub fn sleep_until(deadline: Instant) {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        let left = deadline - now;
        if left > std::time::Duration::from_micros(200) {
            std::thread::sleep(left - std::time::Duration::from_micros(120));
        } else {
            std::hint::spin_loop();
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SyncSample {
    pub rtt_us: i64,
    /// Estimated (peer clock - local clock).
    pub offset_us: i64,
    pub at_us: i64,
}

/// NTP-style offset estimate between this peer and the remote peer.
pub struct ClockSync {
    window: VecDeque<SyncSample>,
    cap: usize,
    pub all_rtts: Vec<i64>,
}

impl ClockSync {
    pub fn new(cap: usize) -> Self {
        ClockSync {
            window: VecDeque::new(),
            cap,
            all_rtts: Vec::new(),
        }
    }

    /// t0: local send, t1: remote receive, t2: remote send, t3: local receive.
    pub fn add(&mut self, t0: i64, t1: i64, t2: i64, t3: i64) -> SyncSample {
        let rtt = (t3 - t0) - (t2 - t1);
        let offset = ((t1 - t0) + (t2 - t3)) / 2;
        let s = SyncSample {
            rtt_us: rtt,
            offset_us: offset,
            at_us: t3,
        };
        self.window.push_back(s);
        while self.window.len() > self.cap {
            self.window.pop_front();
        }
        self.all_rtts.push(rtt);
        s
    }

    /// Offset from the lowest-RTT sample in the window (least queueing, least asymmetry).
    pub fn offset_us(&self) -> Option<i64> {
        self.best().map(|s| s.offset_us)
    }

    pub fn best(&self) -> Option<SyncSample> {
        self.window.iter().min_by_key(|s| s.rtt_us).copied()
    }

    pub fn min_rtt_us(&self) -> Option<i64> {
        self.best().map(|s| s.rtt_us)
    }
}

/// Least-squares fit of y = a + b x.
#[derive(Default, Clone, Debug)]
pub struct LineFit {
    n: f64,
    sx: f64,
    sy: f64,
    sxx: f64,
    sxy: f64,
}

impl LineFit {
    pub fn add(&mut self, x: f64, y: f64) {
        self.n += 1.0;
        self.sx += x;
        self.sy += y;
        self.sxx += x * x;
        self.sxy += x * y;
    }

    pub fn len(&self) -> usize {
        self.n as usize
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0.0
    }

    /// (intercept, slope). Slope is 0 when the x spread is degenerate.
    pub fn solve(&self) -> Option<(f64, f64)> {
        if self.n < 1.0 {
            return None;
        }
        let d = self.n * self.sxx - self.sx * self.sx;
        if self.n < 2.0 || d.abs() < 1e-12 {
            return Some((self.sy / self.n, 0.0));
        }
        let b = (self.n * self.sxy - self.sx * self.sy) / d;
        let a = (self.sy - b * self.sx) / self.n;
        Some((a, b))
    }
}

pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_recovers_offset_with_symmetric_delay() {
        let mut cs = ClockSync::new(32);
        let true_offset = 12_345; // remote = local + offset
        for (i, q) in [3_000i64, 500, 9_000, 0, 4_000].iter().enumerate() {
            let t0 = i as i64 * 100_000;
            let one_way = 20_000;
            let t1 = t0 + one_way + q + true_offset;
            let t2 = t1 + 50;
            let t3 = t2 - true_offset + one_way;
            cs.add(t0, t1, t2, t3);
        }
        assert_eq!(cs.offset_us().unwrap(), true_offset);
        assert_eq!(cs.min_rtt_us().unwrap(), 40_000);
    }

    #[test]
    fn line_fit() {
        let mut f = LineFit::default();
        for i in 0..10 {
            f.add(i as f64, 3.0 + 0.5 * i as f64);
        }
        let (a, b) = f.solve().unwrap();
        assert!((a - 3.0).abs() < 1e-9 && (b - 0.5).abs() < 1e-9);
    }
}

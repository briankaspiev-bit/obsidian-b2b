//! Round-trip measurements with the engine's own wire format
//! (`obsidian_protocol::Packet::Ping/Pong`) and clock-sync filter
//! (`obsidian_clock::ClockSync`). Fed by the booth link (`link.rs`), which
//! sends the pings and answers the other booth's.

use obsidian_clock::ClockSync;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkResult {
    pub pings_sent: u32,
    pub pongs_received: u32,
    pub loss_pct: f64,
    /// Median round trip.
    pub rtt_ms: f64,
    pub rtt_min_ms: f64,
    /// p99 round trip minus the minimum: how much the path wobbles.
    pub jitter_ms: f64,
    /// The other booth's clock minus ours, from the lowest-RTT exchange.
    pub clock_offset_ms: Option<f64>,
    /// True if the other booth answered at all.
    pub reached: bool,
}

/// Counts pings out and pongs back over one measuring window.
pub struct Probe {
    sent: u32,
    received: u32,
    sync: ClockSync,
}

impl Default for Probe {
    fn default() -> Self {
        Probe { sent: 0, received: 0, sync: ClockSync::new(512) }
    }
}

impl Probe {
    pub fn sent(&mut self) {
        self.sent += 1;
    }

    /// t0: our send, t1/t2: their receive/send, t3: our receive (microseconds).
    pub fn pong(&mut self, t0: i64, t1: i64, t2: i64, t3: i64) {
        self.received += 1;
        self.sync.add(t0, t1, t2, t3);
    }

    pub fn summary(&self) -> NetworkResult {
        let mut rtts: Vec<f64> = self.sync.all_rtts.iter().map(|&us| us as f64 / 1000.0).collect();
        rtts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let pick = |p: f64| if rtts.is_empty() { 0.0 } else { obsidian_clock::percentile(&rtts, p) };
        let min = rtts.first().copied().unwrap_or(0.0);
        let received = self.received.min(self.sent);
        NetworkResult {
            pings_sent: self.sent,
            pongs_received: received,
            loss_pct: if self.sent == 0 {
                100.0
            } else {
                100.0 * f64::from(self.sent - received) / f64::from(self.sent)
            },
            rtt_ms: pick(50.0),
            rtt_min_ms: min,
            jitter_ms: (pick(99.0) - min).max(0.0),
            clock_offset_ms: self.sync.offset_us().map(|us| us as f64 / 1000.0),
            reached: received > 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_reports_median_jitter_and_loss() {
        let mut p = Probe::default();
        for i in 0..10 {
            p.sent();
            if i < 8 {
                // RTT 20 ms, plus 10 ms on one of them.
                let extra = if i == 3 { 10_000 } else { 0 };
                p.pong(0, 5_000, 5_000, 20_000 + extra);
            }
        }
        let r = p.summary();
        assert!(r.reached);
        assert_eq!(r.loss_pct, 20.0);
        assert_eq!(r.rtt_ms, 20.0);
        assert_eq!(r.jitter_ms, 10.0);
    }

    #[test]
    fn nothing_back_is_all_loss() {
        let mut p = Probe::default();
        p.sent();
        let r = p.summary();
        assert!(!r.reached);
        assert_eq!(r.loss_pct, 100.0);
    }
}

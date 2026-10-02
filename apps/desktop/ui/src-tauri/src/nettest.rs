//! Booth Check network test: a short ping/pong exchange with the other booth
//! on the same UDP socket the session will use, with the engine's own wire
//! format (`obsidian_protocol::Packet::Ping/Pong`) and clock-sync filter
//! (`obsidian_clock::ClockSync`).
//!
//! Both booths run this at the same time; each one answers the other's pings
//! while measuring its own, so neither side needs to go first.

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use obsidian_clock::ClockSync;
use obsidian_protocol::Packet;
use serde::Serialize;

/// How often a probe goes out.
pub const PING_INTERVAL: Duration = Duration::from_millis(20);

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

/// Microseconds since `start`, the local clock for this test.
fn now_us(start: Instant) -> i64 {
    start.elapsed().as_micros() as i64
}

/// Probe `peer` for `duration`, answering its probes too. Pongs still in
/// flight when the time is up get a short grace period so they don't count as
/// lost.
pub fn run(sock: &UdpSocket, peer: SocketAddr, duration: Duration) -> std::io::Result<NetworkResult> {
    let start = Instant::now();
    let grace = Duration::from_millis(400);
    let mut sync = ClockSync::new(256);
    let mut buf = vec![0u8; 2048];
    let mut out = Vec::with_capacity(64);
    let mut next_ping = Instant::now();
    let mut sent: u32 = 0;
    let mut received: u32 = 0;

    loop {
        let now = Instant::now();
        let elapsed = now - start;
        if elapsed >= duration + grace {
            break;
        }
        if elapsed < duration && now >= next_ping {
            out.clear();
            Packet::Ping { id: sent, t0_us: now_us(start) }.encode(&mut out);
            sock.send_to(&out, peer)?;
            sent += 1;
            next_ping += PING_INTERVAL;
        }
        let wait = next_ping.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
        sock.set_read_timeout(Some(wait))?;
        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            // Windows reports an ICMP "port unreachable" from an earlier send as
            // a reset on the next receive. The other side just isn't up yet.
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => continue,
            Err(e) => return Err(e),
        };
        let t_recv = now_us(start);
        match Packet::decode(&buf[..n]) {
            Ok(Packet::Ping { id, t0_us }) => {
                out.clear();
                Packet::Pong { id, t0_us, t1_us: t_recv, t2_us: now_us(start) }.encode(&mut out);
                sock.send_to(&out, from)?;
            }
            Ok(Packet::Pong { t0_us, t1_us, t2_us, .. }) => {
                received += 1;
                sync.add(t0_us, t1_us, t2_us, t_recv);
            }
            // Media or anything else is not part of the test.
            _ => {}
        }
    }

    Ok(summarize(sent, received, &sync))
}

fn summarize(sent: u32, received: u32, sync: &ClockSync) -> NetworkResult {
    let mut rtts: Vec<f64> = sync.all_rtts.iter().map(|&us| us as f64 / 1000.0).collect();
    rtts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pick = |p: f64| if rtts.is_empty() { 0.0 } else { obsidian_clock::percentile(&rtts, p) };
    let min = rtts.first().copied().unwrap_or(0.0);
    let received = received.min(sent);
    NetworkResult {
        pings_sent: sent,
        pongs_received: received,
        loss_pct: if sent == 0 { 100.0 } else { 100.0 * f64::from(sent - received) / f64::from(sent) },
        rtt_ms: pick(50.0),
        rtt_min_ms: min,
        jitter_ms: (pick(99.0) - min).max(0.0),
        clock_offset_ms: sync.offset_us().map(|us| us as f64 / 1000.0),
        reached: received > 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn two_booths_on_loopback_reach_each_other() {
        let a = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").unwrap();
        let (a_addr, b_addr) = (a.local_addr().unwrap(), b.local_addr().unwrap());
        let d = Duration::from_millis(600);
        let tb = thread::spawn(move || run(&b, a_addr, d).unwrap());
        let ra = run(&a, b_addr, d).unwrap();
        let rb = tb.join().unwrap();
        for r in [&ra, &rb] {
            assert!(r.reached);
            assert!(r.pings_sent >= 25, "{r:?}");
            assert!(r.loss_pct < 5.0, "{r:?}");
            assert!(r.rtt_ms < 20.0, "{r:?}");
            // Same machine, same clock: the offset is close to the start-time gap.
            assert!(r.clock_offset_ms.unwrap().abs() < 50.0, "{r:?}");
        }
    }

    #[test]
    fn nobody_home_is_all_loss() {
        let a = UdpSocket::bind("127.0.0.1:0").unwrap();
        // Bound but never read: pings vanish, no pongs come back.
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        let r = run(&a, silent.local_addr().unwrap(), Duration::from_millis(200)).unwrap();
        assert!(!r.reached);
        assert_eq!(r.loss_pct, 100.0);
    }
}

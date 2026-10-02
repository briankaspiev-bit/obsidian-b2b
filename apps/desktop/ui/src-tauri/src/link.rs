//! The booth link: everything that crosses the network between the two apps
//! before the engine's live audio exists.
//!
//! After the rendezvous service pairs the booths, this thread owns the UDP
//! socket and:
//! * sends a heartbeat ping every 100 ms with the engine's wire format and
//!   answers the other booth's, giving a live round trip, jitter and loss;
//! * carries the UI's coordination messages (ready, take over, handoff,
//!   leave...) as small JSON packets;
//! * carries each booth's input level, so the other side's meter moves with
//!   the real mixer;
//! * runs Booth Check's network test on demand.
//!
//! Coordination packets start with their own magic (`0x0B 0x43`), so the
//! engine (`0x0B5D`) and the rendezvous service (`0x0B52`) ignore them and
//! the relay forwards them like media. When the engine's live mode takes the
//! socket, these messages move into the engine's protocol.

use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use obsidian_protocol::Packet;
use serde::Serialize;

use crate::nettest::{NetworkResult, Probe};

pub const MAGIC: [u8; 2] = [0x0B, 0x43];
const KIND_CONTROL: u8 = 1;
const KIND_LEVEL: u8 = 2;
const KIND_BYE: u8 = 3;
/// Keeps coordination messages in one unfragmented datagram.
pub const MAX_CONTROL_BYTES: usize = 1200;

const HEARTBEAT: Duration = Duration::from_millis(100);
const STATUS_EVERY: Duration = Duration::from_millis(500);
/// A fresh window for the live numbers, so they follow the network.
const WINDOW: Duration = Duration::from_secs(3);
/// No packet for this long and the other booth counts as reconnecting.
pub const SILENCE_RECONNECTING: Duration = Duration::from_millis(1500);
/// ...and for this long, as gone.
pub const SILENCE_LEFT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PeerState {
    Connected,
    Reconnecting,
    Left,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LinkStatus {
    pub state: PeerState,
    pub rtt_ms: f64,
    pub jitter_ms: f64,
    pub loss_pct: f64,
    pub relay: bool,
}

/// Where the link reports what it hears. The Tauri app forwards these to the
/// UI as events; tests collect them.
pub trait LinkSink: Send + 'static {
    fn control(&self, json: String);
    fn status(&self, status: LinkStatus);
    fn remote_level(&self, level: (f32, f32));
}

enum Cmd {
    Control(String),
    Level((f32, f32)),
    Test(Duration, Sender<NetworkResult>),
    Stop,
}

pub struct BoothLink {
    tx: Sender<Cmd>,
    join: Option<JoinHandle<()>>,
}

impl BoothLink {
    pub fn start(sock: UdpSocket, peer: SocketAddr, relay: bool, sink: impl LinkSink) -> std::io::Result<Self> {
        sock.set_read_timeout(Some(Duration::from_millis(10)))?;
        let (tx, rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("booth-link".into())
            .spawn(move || run(sock, peer, relay, rx, sink))?;
        Ok(BoothLink { tx, join: Some(join) })
    }

    pub fn send_control(&self, json: String) -> Result<(), String> {
        if json.len() > MAX_CONTROL_BYTES {
            return Err(format!("control message too large ({} bytes)", json.len()));
        }
        self.tx.send(Cmd::Control(json)).map_err(|_| "link closed".to_string())
    }

    pub fn set_local_level(&self, level: (f32, f32)) {
        let _ = self.tx.send(Cmd::Level(level));
    }

    /// Measures for `duration` and blocks until the result is in.
    pub fn network_test(&self, duration: Duration) -> Option<NetworkResult> {
        self.tester().run(duration)
    }

    /// Lets a test run without holding on to the link itself.
    pub fn tester(&self) -> Tester {
        Tester(self.tx.clone())
    }
}

pub struct Tester(Sender<Cmd>);

impl Tester {
    pub fn run(&self, duration: Duration) -> Option<NetworkResult> {
        let (tx, rx) = mpsc::channel();
        self.0.send(Cmd::Test(duration, tx)).ok()?;
        rx.recv().ok()
    }
}

impl Drop for BoothLink {
    /// Says goodbye so the other booth shows "left" at once, not after a timeout.
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn frame(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(3 + body.len());
    v.extend_from_slice(&MAGIC);
    v.push(kind);
    v.extend_from_slice(body);
    v
}

fn level_body((l, r): (f32, f32)) -> [u8; 8] {
    let mut b = [0u8; 8];
    b[..4].copy_from_slice(&l.to_le_bytes());
    b[4..].copy_from_slice(&r.to_le_bytes());
    b
}

fn run(sock: UdpSocket, peer: SocketAddr, relay: bool, rx: Receiver<Cmd>, sink: impl LinkSink) {
    let start = Instant::now();
    let now_us = || start.elapsed().as_micros() as i64;
    let mut buf = vec![0u8; 2048];
    let mut out = Vec::with_capacity(64);
    let mut ping_id: u32 = 0;
    let mut next_beat = Instant::now();
    let mut next_status = Instant::now() + STATUS_EVERY;
    let mut last_heard: Option<Instant> = None;
    let mut window = Probe::default();
    let mut window_started = Instant::now();
    let mut live = Probe::default().summary();
    let mut test: Option<(Instant, Probe, Sender<NetworkResult>)> = None;
    let mut local_level: Option<(f32, f32)> = None;
    let mut said_bye = false;
    let mut last_state: Option<PeerState> = None;

    loop {
        // Commands from the app.
        loop {
            match rx.try_recv() {
                Ok(Cmd::Control(json)) => {
                    let _ = sock.send_to(&frame(KIND_CONTROL, json.as_bytes()), peer);
                }
                Ok(Cmd::Level(l)) => local_level = Some(l),
                Ok(Cmd::Test(d, reply)) => test = Some((Instant::now() + d, Probe::default(), reply)),
                Ok(Cmd::Stop) | Err(mpsc::TryRecvError::Disconnected) => {
                    // Twice, in case one is lost.
                    for _ in 0..2 {
                        let _ = sock.send_to(&frame(KIND_BYE, &[]), peer);
                    }
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }

        let now = Instant::now();
        if now >= next_beat {
            out.clear();
            Packet::Ping { id: ping_id, t0_us: now_us() }.encode(&mut out);
            ping_id = ping_id.wrapping_add(1);
            let _ = sock.send_to(&out, peer);
            window.sent();
            if let Some((_, p, _)) = test.as_mut() {
                p.sent();
            }
            if let Some(l) = local_level.take() {
                let _ = sock.send_to(&frame(KIND_LEVEL, &level_body(l)), peer);
            }
            next_beat += HEARTBEAT;
            if next_beat < now {
                next_beat = now + HEARTBEAT;
            }
        }

        if test.as_ref().is_some_and(|(until, _, _)| now >= *until + Duration::from_millis(300)) {
            let (_, p, reply) = test.take().unwrap();
            let _ = reply.send(p.summary());
        }

        if now >= next_status {
            next_status += STATUS_EVERY;
            if now - window_started >= WINDOW {
                live = window.summary();
                window = Probe::default();
                window_started = now;
            }
            let silence = last_heard.map_or(now - start, |t| now - t);
            let state = if said_bye || silence >= SILENCE_LEFT {
                PeerState::Left
            } else if last_heard.is_some() && silence < SILENCE_RECONNECTING {
                PeerState::Connected
            } else {
                PeerState::Reconnecting
            };
            if state != PeerState::Connected && state != PeerState::Left && last_state.is_none() {
                // Still waiting for the first packet: not news yet.
            } else {
                sink.status(LinkStatus {
                    state,
                    rtt_ms: live.rtt_ms,
                    jitter_ms: live.jitter_ms,
                    loss_pct: if state == PeerState::Connected { live.loss_pct } else { 100.0 },
                    relay,
                });
                last_state = Some(state);
            }
        }

        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        // Windows reports an ICMP "port unreachable" from an
                        // earlier send as a reset on the next receive.
                        | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                continue
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
        };
        let pkt = &buf[..n];
        let t_recv = now_us();

        if pkt.len() >= 3 && pkt[..2] == MAGIC {
            last_heard = Some(Instant::now());
            match pkt[2] {
                KIND_CONTROL => {
                    if let Ok(s) = std::str::from_utf8(&pkt[3..]) {
                        sink.control(s.to_owned());
                    }
                }
                KIND_LEVEL if pkt.len() >= 11 => {
                    let l = f32::from_le_bytes(pkt[3..7].try_into().unwrap());
                    let r = f32::from_le_bytes(pkt[7..11].try_into().unwrap());
                    sink.remote_level((l, r));
                }
                KIND_BYE => said_bye = true,
                _ => {}
            }
            continue;
        }

        match Packet::decode(pkt) {
            Ok(Packet::Ping { id, t0_us }) => {
                last_heard = Some(Instant::now());
                said_bye = false;
                out.clear();
                Packet::Pong { id, t0_us, t1_us: t_recv, t2_us: now_us() }.encode(&mut out);
                let _ = sock.send_to(&out, from);
            }
            Ok(Packet::Pong { t0_us, t1_us, t2_us, .. }) => {
                last_heard = Some(Instant::now());
                window.pong(t0_us, t1_us, t2_us, t_recv);
                if let Some((_, p, _)) = test.as_mut() {
                    p.pong(t0_us, t1_us, t2_us, t_recv);
                }
            }
            // Late rendezvous probes, stray media: not ours.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Collect {
        controls: Arc<Mutex<Vec<String>>>,
        statuses: Arc<Mutex<Vec<LinkStatus>>>,
        levels: Arc<Mutex<Vec<(f32, f32)>>>,
    }

    impl LinkSink for Collect {
        fn control(&self, json: String) {
            self.controls.lock().unwrap().push(json);
        }
        fn status(&self, s: LinkStatus) {
            self.statuses.lock().unwrap().push(s);
        }
        fn remote_level(&self, l: (f32, f32)) {
            self.levels.lock().unwrap().push(l);
        }
    }

    fn pair() -> (BoothLink, Collect, BoothLink, Collect) {
        let a = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").unwrap();
        let (aa, ba) = (a.local_addr().unwrap(), b.local_addr().unwrap());
        let (ca, cb) = (Collect::default(), Collect::default());
        let la = BoothLink::start(a, ba, false, ca.clone()).unwrap();
        let lb = BoothLink::start(b, aa, false, cb.clone()).unwrap();
        (la, ca, lb, cb)
    }

    #[test]
    fn booths_exchange_messages_levels_and_measure() {
        let (la, ca, lb, cb) = pair();
        la.send_control(r#"{"type":"boothReady","ready":true}"#.into()).unwrap();
        lb.set_local_level((-12.0, -13.5));
        let r = la.network_test(Duration::from_millis(800)).unwrap();
        assert!(r.reached, "{r:?}");
        assert!(r.pings_sent >= 7, "{r:?}");
        assert!(r.loss_pct < 15.0, "{r:?}");
        assert!(r.rtt_ms < 20.0, "{r:?}");
        assert_eq!(cb.controls.lock().unwrap().as_slice(), [r#"{"type":"boothReady","ready":true}"#]);
        assert_eq!(ca.levels.lock().unwrap().first(), Some(&(-12.0, -13.5)));
        assert!(ca.statuses.lock().unwrap().iter().any(|s| s.state == PeerState::Connected));
        drop(lb);
        thread::sleep(Duration::from_millis(700));
        assert_eq!(ca.statuses.lock().unwrap().last().unwrap().state, PeerState::Left);
        drop(la);
    }

    /// Answers pings by hand for `answer_for`, then goes quiet without a goodbye.
    fn flaky_peer(sock: UdpSocket, answer_for: Duration) -> JoinHandle<()> {
        thread::spawn(move || {
            sock.set_read_timeout(Some(Duration::from_millis(10))).unwrap();
            let until = Instant::now() + answer_for;
            let mut buf = [0u8; 2048];
            let mut out = Vec::new();
            while Instant::now() < until {
                if let Ok((n, from)) = sock.recv_from(&mut buf) {
                    if let Ok(Packet::Ping { id, t0_us }) = Packet::decode(&buf[..n]) {
                        out.clear();
                        Packet::Pong { id, t0_us, t1_us: 0, t2_us: 0 }.encode(&mut out);
                        sock.send_to(&out, from).unwrap();
                    }
                }
            }
        })
    }

    #[test]
    fn silence_reads_as_reconnecting() {
        let a = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").unwrap();
        let ca = Collect::default();
        let la = BoothLink::start(a, b.local_addr().unwrap(), false, ca.clone()).unwrap();
        flaky_peer(b, Duration::from_millis(700)).join().unwrap();
        thread::sleep(SILENCE_RECONNECTING + Duration::from_millis(700));
        let states: Vec<PeerState> = ca.statuses.lock().unwrap().iter().map(|s| s.state).collect();
        assert_eq!(states.first(), Some(&PeerState::Connected), "{states:?}");
        assert_eq!(states.last(), Some(&PeerState::Reconnecting), "{states:?}");
        drop(la);
    }

    #[test]
    fn nobody_home_reports_nothing_yet() {
        let a = UdpSocket::bind("127.0.0.1:0").unwrap();
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        let ca = Collect::default();
        let la = BoothLink::start(a, silent.local_addr().unwrap(), false, ca.clone()).unwrap();
        let r = la.network_test(Duration::from_millis(300)).unwrap();
        assert!(!r.reached);
        assert_eq!(r.loss_pct, 100.0);
        // Never heard the other booth: no status, so the UI keeps "waiting".
        assert!(ca.statuses.lock().unwrap().is_empty());
    }
}

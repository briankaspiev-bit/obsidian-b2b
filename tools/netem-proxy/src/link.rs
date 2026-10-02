//! The proxy's forwarding threads, usable in-process (e.g. the solo-practice ghost
//! DJ talks to the real engine through a simulated NYC-London path on localhost).

use crate::{DirStats, Impairer, Profile};
use obsidian_clock::sleep_until;
use std::collections::BinaryHeap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

struct Item {
    at: Instant,
    seq: u64,
    data: Vec<u8>,
}
impl PartialEq for Item {
    fn eq(&self, o: &Self) -> bool {
        self.at == o.at && self.seq == o.seq
    }
}
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        // min-heap
        o.at.cmp(&self.at).then(o.seq.cmp(&self.seq))
    }
}

type Queue = Arc<(Mutex<BinaryHeap<Item>>, Condvar)>;

/// Forward datagrams arriving on `rx` to `dest` (sent from `tx`), impaired by `imp`,
/// until `stop` is set. Returns (receive thread → stats, send thread).
pub fn direction(
    name: &str,
    rx: UdpSocket,
    tx: UdpSocket,
    dest: SocketAddr,
    mut imp: Impairer,
    t0: Instant,
    stop: Arc<AtomicBool>,
) -> (JoinHandle<DirStats>, JoinHandle<()>) {
    let q: Queue = Arc::new((Mutex::new(BinaryHeap::new()), Condvar::new()));
    let q2 = q.clone();
    let stop2 = stop.clone();
    let recv = std::thread::Builder::new()
        .name(format!("{name}-rx"))
        .spawn(move || {
            let mut buf = vec![0u8; 65_536];
            let mut seq = 0u64;
            rx.set_read_timeout(Some(Duration::from_millis(50)))
                .unwrap();
            while !stop2.load(Ordering::Relaxed) {
                let Ok((n, _)) = rx.recv_from(&mut buf) else {
                    continue;
                };
                let now = (Instant::now() - t0).as_secs_f64();
                if let Some(at) = imp.process(now) {
                    let at = t0 + Duration::from_secs_f64(at);
                    seq += 1;
                    let (m, c) = &*q2;
                    m.lock().unwrap().push(Item {
                        at,
                        seq,
                        data: buf[..n].to_vec(),
                    });
                    c.notify_one();
                }
            }
            imp.stats
        })
        .unwrap();
    let send = std::thread::Builder::new()
        .name(format!("{name}-tx"))
        .spawn(move || {
            let (m, c) = &*q;
            loop {
                let mut g = m.lock().unwrap();
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                match g.peek().map(|i| i.at) {
                    None => {
                        let _ = c.wait_timeout(g, Duration::from_millis(20)).unwrap();
                    }
                    Some(at) => {
                        let now = Instant::now();
                        if at <= now {
                            let it = g.pop().unwrap();
                            drop(g);
                            let _ = tx.send_to(&it.data, dest);
                        } else if at - now > Duration::from_micros(800) {
                            let _ = c
                                .wait_timeout(g, at - now - Duration::from_micros(500))
                                .unwrap();
                        } else {
                            drop(g);
                            sleep_until(at);
                        }
                    }
                }
            }
        })
        .unwrap();
    (recv, send)
}

/// A two-way impaired path on localhost between peers A and B.
/// A sends to [`Link::a_side`], B sends to [`Link::b_side`].
pub struct Link {
    pub a_side: SocketAddr,
    pub b_side: SocketAddr,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    stats: Vec<JoinHandle<DirStats>>,
}

impl Link {
    pub fn start(
        a_peer: SocketAddr,
        b_peer: SocketAddr,
        a_to_b: Profile,
        b_to_a: Profile,
        seed: u64,
    ) -> std::io::Result<Link> {
        let sa = UdpSocket::bind("127.0.0.1:0")?;
        let sb = UdpSocket::bind("127.0.0.1:0")?;
        let (a_side, b_side) = (sa.local_addr()?, sb.local_addr()?);
        let stop = Arc::new(AtomicBool::new(false));
        let t0 = Instant::now();
        let (r1, s1) = direction(
            "ab",
            sa.try_clone()?,
            sb.try_clone()?,
            b_peer,
            Impairer::new(a_to_b, seed),
            t0,
            stop.clone(),
        );
        let (r2, s2) = direction(
            "ba",
            sb,
            sa,
            a_peer,
            Impairer::new(b_to_a, seed.wrapping_add(1000)),
            t0,
            stop.clone(),
        );
        Ok(Link {
            a_side,
            b_side,
            stop,
            threads: vec![s1, s2],
            stats: vec![r1, r2],
        })
    }

    /// Stop and return (A→B, B→A) stats.
    pub fn stop(self) -> (DirStats, DirStats) {
        self.stop.store(true, Ordering::Relaxed);
        let mut st = self.stats.into_iter().map(|h| h.join().unwrap());
        let r = (st.next().unwrap(), st.next().unwrap());
        for t in self.threads {
            let _ = t.join();
        }
        r
    }
}

/// The lab profiles shipped in tools/netem-profiles/profiles.json, built in.
pub fn builtin_profiles() -> Vec<Profile> {
    serde_json::from_str(include_str!("../../netem-profiles/profiles.json"))
        .expect("built-in profiles parse")
}

pub fn builtin_profile(name: &str) -> Option<Profile> {
    builtin_profiles().into_iter().find(|p| p.name == name)
}

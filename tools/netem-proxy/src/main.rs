//! `obsidian-netem`: sits between peer A and peer B and impairs each direction.
//!
//!   A --(sends to a_listen)--> proxy --(from b_listen)--> B.peer_addr
//!   B --(sends to b_listen)--> proxy --(from a_listen)--> A.peer_addr
//!
//! Profiles live in tools/netem-profiles/profiles.json (same names as the
//! report's lab rig: same-city, nyc-lon, nyc-tyo, bad-wifi, route-change).
//! Equivalent `tc netem` scripts for a real Linux router box are in
//! tools/netem-profiles/tc/.

use anyhow::{Context, Result};
use clap::Parser;
use obsidian_clock::sleep_until;
use obsidian_netem_proxy::{DirStats, Impairer, Profile};
use std::collections::BinaryHeap;
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    a_listen: SocketAddr,
    #[arg(long)]
    a_peer: SocketAddr,
    #[arg(long)]
    b_listen: SocketAddr,
    #[arg(long)]
    b_peer: SocketAddr,
    #[arg(long)]
    profiles: PathBuf,
    #[arg(long)]
    profile: String,
    /// Profile for B→A if different (asymmetric paths).
    #[arg(long)]
    profile_ba: Option<String>,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Exit after this many seconds and write stats.
    #[arg(long)]
    duration: f64,
    #[arg(long)]
    stats: Option<PathBuf>,
}

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

fn direction(
    name: &'static str,
    rx: UdpSocket,
    tx: UdpSocket,
    dest: SocketAddr,
    mut imp: Impairer,
    t0: Instant,
    end: Instant,
) -> (
    std::thread::JoinHandle<DirStats>,
    std::thread::JoinHandle<()>,
) {
    let q: Queue = Arc::new((Mutex::new(BinaryHeap::new()), Condvar::new()));
    let q2 = q.clone();
    let recv = std::thread::Builder::new()
        .name(format!("{name}-rx"))
        .spawn(move || {
            let mut buf = vec![0u8; 65_536];
            let mut seq = 0u64;
            rx.set_read_timeout(Some(Duration::from_millis(50)))
                .unwrap();
            while Instant::now() < end {
                let Ok((n, _)) = rx.recv_from(&mut buf) else {
                    continue;
                };
                let now = (Instant::now() - t0).as_secs_f64();
                match imp.process(now) {
                    None => {}
                    Some(at) => {
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
                if Instant::now() > end + Duration::from_millis(500) {
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

fn main() -> Result<()> {
    let a = Args::parse();
    let all: Vec<Profile> =
        serde_json::from_str(&std::fs::read_to_string(&a.profiles).context("read profiles")?)?;
    let find = |n: &str| {
        all.iter()
            .find(|p| p.name == n)
            .cloned()
            .with_context(|| format!("no profile {n}"))
    };
    let pab = find(&a.profile)?;
    let pba = find(a.profile_ba.as_deref().unwrap_or(&a.profile))?;
    let sa = UdpSocket::bind(a.a_listen)?;
    let sb = UdpSocket::bind(a.b_listen)?;
    let t0 = Instant::now();
    let end = t0 + Duration::from_secs_f64(a.duration);
    let mk = |p: Profile, seed: u64| Impairer::new(p, seed);
    let (ab_rx, ab_tx) = direction(
        "ab",
        sa.try_clone()?,
        sb.try_clone()?,
        a.b_peer,
        mk(pab, a.seed),
        t0,
        end,
    );
    let (ba_rx, ba_tx) = direction(
        "ba",
        sb,
        sa,
        a.a_peer,
        mk(pba, a.seed.wrapping_add(1000)),
        t0,
        end,
    );
    let s_ab = ab_rx.join().unwrap();
    let s_ba = ba_rx.join().unwrap();
    ab_tx.join().unwrap();
    ba_tx.join().unwrap();
    let out = serde_json::json!({ "a_to_b": s_ab, "b_to_a": s_ba });
    if let Some(p) = a.stats {
        std::fs::write(p, serde_json::to_string_pretty(&out)?)?;
    }
    eprintln!("[netem] {}", out);
    Ok(())
}

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
use obsidian_netem_proxy::link::direction;
use obsidian_netem_proxy::{Impairer, Profile};
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
    let stop = Arc::new(AtomicBool::new(false));
    let mk = |p: Profile, seed: u64| Impairer::new(p, seed);
    let (ab_rx, ab_tx) = direction(
        "ab",
        sa.try_clone()?,
        sb.try_clone()?,
        a.b_peer,
        mk(pab, a.seed),
        t0,
        stop.clone(),
    );
    let (ba_rx, ba_tx) = direction(
        "ba",
        sb,
        sa,
        a.a_peer,
        mk(pba, a.seed.wrapping_add(1000)),
        t0,
        stop.clone(),
    );
    std::thread::sleep(Duration::from_secs_f64(a.duration));
    stop.store(true, Ordering::Relaxed);
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

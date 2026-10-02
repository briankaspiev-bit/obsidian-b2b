//! Network impairment model shared by the UDP proxy (`obsidian-netem`) and the
//! bench's virtual-time simulator. Time is plain seconds (f64) so the same model
//! runs in real time or simulated time.

use rand::{Rng, SeedableRng};
use rand_distr::{Distribution, Normal};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct GilbertElliott {
    /// P(good → bad) per packet.
    pub p_gb: f64,
    /// P(bad → good) per packet.
    pub p_bg: f64,
    pub loss_good: f64,
    pub loss_bad: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Spikes {
    /// Probability per packet that a stall starts (Wi-Fi retries, scans, power save).
    pub prob: f64,
    pub min_ms: f64,
    pub max_ms: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Step {
    pub at_s: f64,
    pub delay_ms: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub delay_ms: f64,
    #[serde(default)]
    pub jitter_ms: f64,
    #[serde(default)]
    pub loss: Option<GilbertElliott>,
    #[serde(default)]
    pub spikes: Option<Spikes>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

#[derive(Debug, Default, Serialize, Clone)]
pub struct DirStats {
    pub received: u64,
    pub dropped: u64,
    pub delivered: u64,
    pub stalls: u64,
    pub mean_delay_ms: f64,
    pub max_delay_ms: f64,
}

pub struct Impairer {
    p: Profile,
    rng: rand::rngs::StdRng,
    bad: bool,
    stall_until: Option<f64>,
    last_deliver: Option<f64>,
    pub stats: DirStats,
}

impl Impairer {
    pub fn new(p: Profile, seed: u64) -> Self {
        Impairer {
            p,
            rng: rand::rngs::StdRng::seed_from_u64(seed),
            bad: false,
            stall_until: None,
            last_deliver: None,
            stats: DirStats::default(),
        }
    }

    /// `now` = seconds since the path came up. Returns delivery time, or None if dropped.
    pub fn process(&mut self, now: f64) -> Option<f64> {
        self.stats.received += 1;
        if let Some(ge) = &self.p.loss {
            self.bad = if self.bad {
                self.rng.gen::<f64>() >= ge.p_bg
            } else {
                self.rng.gen::<f64>() < ge.p_gb
            };
            let pl = if self.bad { ge.loss_bad } else { ge.loss_good };
            if self.rng.gen::<f64>() < pl {
                self.stats.dropped += 1;
                return None;
            }
        }
        let mut base = self.p.delay_ms;
        for s in &self.p.steps {
            if now >= s.at_s {
                base = s.delay_ms;
            }
        }
        let j = if self.p.jitter_ms > 0.0 {
            Normal::new(0.0, self.p.jitter_ms)
                .unwrap()
                .sample(&mut self.rng)
                .max(-2.0 * self.p.jitter_ms)
        } else {
            0.0
        };
        let mut at = now + (base + j).max(0.0) / 1e3;
        if let Some(sp) = &self.p.spikes {
            if self.stall_until.map(|s| s < now).unwrap_or(true) && self.rng.gen::<f64>() < sp.prob
            {
                let len = self.rng.gen_range(sp.min_ms..=sp.max_ms);
                self.stall_until = Some(now + len / 1e3);
                self.stats.stalls += 1;
            }
            if let Some(s) = self.stall_until {
                if s > now {
                    at = at.max(s + base / 1e3);
                }
            }
        }
        // FIFO: a path does not reorder.
        if let Some(l) = self.last_deliver {
            at = at.max(l);
        }
        self.last_deliver = Some(at);
        let dms = (at - now) * 1e3;
        self.stats.delivered += 1;
        self.stats.mean_delay_ms += (dms - self.stats.mean_delay_ms) / self.stats.delivered as f64;
        self.stats.max_delay_ms = self.stats.max_delay_ms.max(dms);
        Some(at)
    }
}

pub fn load_profiles(path: &std::path::Path) -> anyhow::Result<Vec<Profile>> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

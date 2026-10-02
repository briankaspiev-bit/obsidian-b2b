//! Fixed-delay playout buffer (the "booth delay contract", report §F.4).
//!
//! # How the delay is anchored
//!
//! For every primary frame that arrives we compute
//! `y = arrival_us - x`, where `x` is the frame's position on the sender's
//! sample timeline expressed in µs. `y` is the one-way delay plus an unknown
//! constant (clock offset). Its *lower envelope* is the "fastest the network
//! can deliver" line. We fit a line through per-2-second minima of `y`:
//! the slope is the sender-vs-receiver audio clock drift (ppm), the intercept
//! is the propagation floor.
//!
//! Playout time of sender position `x` is then
//! `T(x) = c + (x - x0) * (1 + slope)`, with `c` chosen at start as
//! `envelope + margin`, where `margin` = p99.5 of the jitter measured during
//! the network test plus a safety pad. That mapping is held fixed:
//!
//! * jitter inside the margin is absorbed silently;
//! * packets later than the margin are concealed (redundant copy → Opus FEC → PLC);
//! * clock drift is followed by a small resampling ratio change (ASRC), with a
//!   deadband so measurement noise never moves the delay;
//! * sustained lateness triggers a *re-anchor*: the delay is raised once, by
//!   jumping during silence or by a slow audible-safe slew otherwise.
//!
//! The clock-sync estimate is never used for playout, so a sync error cannot
//! wander the monitor delay; it is only used for telemetry.

use obsidian_clock::{percentile, LineFit};
use obsidian_codec::{CodecConfig, CodecError, DecodeKind, Decoder, CHANNELS, SAMPLE_RATE};
use obsidian_protocol::MediaPacket;
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};

/// Envelope bucket length (sender µs). Per-bucket minima feed the drift fit.
const BUCKET_US: f64 = 2e6;
/// Lateness is counted in quarter-second windows of arrival time.
const LATE_WIN_US: i64 = 250_000;
/// Buckets averaged for the delay level when no drift slope is established.
const LEVEL_BUCKETS: usize = 10;
const US_PER_SAMPLE: f64 = 1e6 / SAMPLE_RATE as f64;

#[derive(Debug, Clone)]
pub struct PlayoutConfig {
    /// Length of the network test before playout starts (µs of sender time).
    pub network_test_us: i64,
    /// Jitter percentile the margin covers.
    pub margin_percentile: f64,
    /// Fixed safety pad added on top of the measured jitter.
    pub safety_us: i64,
    /// Never set a margin smaller than this (covers one frame of sender pacing).
    pub min_margin_us: i64,
    /// Max ASRC deviation for drift tracking.
    pub asrc_max_ppm: f64,
    /// Start correcting when |error| exceeds this; stop below `deadband_exit_us`.
    pub deadband_us: f64,
    pub deadband_exit_us: f64,
    /// Max rate deviation while slewing a re-anchor through music (0.5% ≈ 8.6 cents).
    pub slew_max_ppm: f64,
    /// Errors bigger than this are applied as a jump with a crossfade, not a slew.
    pub jump_threshold_us: f64,
    /// Re-anchor when more than this fraction of frames were late within one second.
    pub late_fraction_trigger: f64,
    /// Seconds of decoded audio kept behind the read head (needed for rewinds).
    pub history_secs: f64,
    /// Below this RMS (linear) the remote stream counts as silent.
    pub silence_rms: f32,
    /// Allow lowering the delay after a route improves, if stable this long (µs).
    pub lower_after_us: i64,
    /// Output block length (µs). A block needs all of its samples at its start, so this
    /// is part of the budget.
    pub output_block_us: i64,
    /// Extra wait so redundant copies can still arrive in time: the largest redundancy
    /// offset × frame duration. Loss recovery costs exactly this much latency.
    pub recovery_us: i64,
}

impl Default for PlayoutConfig {
    fn default() -> Self {
        PlayoutConfig {
            network_test_us: 10_000_000,
            margin_percentile: 99.5,
            safety_us: 2_000,
            min_margin_us: 3_000,
            asrc_max_ppm: 500.0,
            deadband_us: 1_000.0,
            deadband_exit_us: 100.0,
            slew_max_ppm: 5_000.0,
            jump_threshold_us: 30_000.0,
            late_fraction_trigger: 0.01,
            history_secs: 4.0,
            silence_rms: 0.001,
            lower_after_us: 120_000_000,
            output_block_us: 5_000,
            recovery_us: 0,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PlayoutStats {
    pub packets: u64,
    pub frames_decoded: u64,
    pub frames_normal: u64,
    pub frames_from_redundancy: u64,
    pub frames_fec: u64,
    pub frames_plc: u64,
    /// Primary packets that arrived after their frame had already been played/concealed.
    pub late_packets: u64,
    pub duplicate_frames: u64,
    pub reanchors: Vec<ReanchorEvent>,
    pub initial_margin_us: i64,
    pub drift_ppm_estimate: f64,
    pub asrc_corrections: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReanchorEvent {
    pub at_out_us: i64,
    pub reason: String,
    pub delta_us: i64,
    pub method: String,
}

/// Per-render-block info for telemetry.
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockInfo {
    /// Sender sample position at the first output sample.
    pub read_pos: f64,
    /// Sender clock capture time of `read_pos`, if known.
    pub capture_tx_us: Option<i64>,
    pub concealed_frames: u32,
    pub ratio: f64,
    pub silent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Measuring,
    Playing,
}

struct Envelope {
    /// (bucket index, min y) for BUCKET_US buckets of sender time.
    buckets: VecDeque<(i64, f64)>,
    cur_bucket: Option<(i64, f64)>,
    cap: usize,
    /// Recent raw (x, y) points, for margin percentiles.
    recent: VecDeque<(f64, f64)>,
    recent_us: f64,
}

impl Envelope {
    fn new() -> Self {
        Envelope {
            buckets: VecDeque::new(),
            cur_bucket: None,
            cap: 90,
            recent: VecDeque::new(),
            recent_us: 10e6,
        }
    }

    fn add(&mut self, x: f64, y: f64) {
        let b = (x / BUCKET_US).floor() as i64;
        match &mut self.cur_bucket {
            Some((cb, m)) if *cb == b => *m = m.min(y),
            Some((cb, _)) if b < *cb => {}
            _ => {
                if let Some(done) = self.cur_bucket.take() {
                    self.buckets.push_back(done);
                    while self.buckets.len() > self.cap {
                        self.buckets.pop_front();
                    }
                }
                self.cur_bucket = Some((b, y));
            }
        }
        self.recent.push_back((x, y));
        while let Some(&(x0, _)) = self.recent.front() {
            if x - x0 > self.recent_us {
                self.recent.pop_front();
            } else {
                break;
            }
        }
    }

    fn reset_to_recent(&mut self, keep_us: f64) {
        let last_x = self.recent.back().map(|p| p.0).unwrap_or(0.0);
        self.recent.retain(|p| last_x - p.0 <= keep_us);
        self.buckets.clear();
        self.cur_bucket = None;
        let pts: Vec<_> = self.recent.drain(..).collect();
        for (x, y) in pts {
            self.add(x, y);
        }
    }

    /// Returns (slope, envelope value at x). The slope is used only when it is
    /// statistically clear of zero (> 3 standard errors) over at least 15 buckets;
    /// bucket minima are noisy and a wrong slope would walk the delay.
    fn fit(&self, x: f64) -> Option<(f64, f64)> {
        let mut pts: Vec<(f64, f64)> = self
            .buckets
            .iter()
            .map(|&(b, m)| ((b as f64 + 0.5) * BUCKET_US, m))
            .collect();
        if pts.is_empty() {
            let (b, m) = self.cur_bucket?;
            pts.push(((b as f64 + 0.5) * BUCKET_US, m));
        }
        let x_ref = pts.last().unwrap().0;
        let n = pts.len() as f64;
        let mean_y = pts.iter().map(|p| p.1).sum::<f64>() / n;
        if pts.len() >= 15 {
            let mut f = LineFit::default();
            for &(px, py) in &pts {
                f.add(px - x_ref, py);
            }
            let (a, b) = f.solve()?;
            let mean_x = pts.iter().map(|p| p.0 - x_ref).sum::<f64>() / n;
            let sxx: f64 = pts.iter().map(|p| (p.0 - x_ref - mean_x).powi(2)).sum();
            let rss: f64 = pts
                .iter()
                .map(|p| (p.1 - (a + b * (p.0 - x_ref))).powi(2))
                .sum();
            let se = (rss / (n - 2.0) / sxx).sqrt();
            let rms = (rss / n).sqrt();
            // A clock drift is a clean line. A route change is a step, which a line fits
            // badly: refuse to read a step as drift.
            if b.abs() > 3.0 * se && rms < 1_500.0 && b.abs() < 300e-6 {
                let b = b.clamp(-1e-3, 1e-3);
                // Level through the mean point so a clamped slope stays centred.
                let a = mean_y - b * mean_x;
                return Some((b, a + b * (x - x_ref)));
            }
        }
        // No clear slope: level from the most recent buckets only, so real drift that
        // is not yet significant does not leave the level lagging behind.
        let k = pts.len().min(LEVEL_BUCKETS);
        let recent = pts[pts.len() - k..].iter().map(|p| p.1).sum::<f64>() / k as f64;
        Some((0.0, recent))
    }

    /// Standard error of the level estimate (µs): bucket-minimum scatter / √k.
    /// Noisy paths (big jitter) get a wider deadband, so noise never moves the delay.
    fn level_se(&self) -> f64 {
        let m: Vec<f64> = self.buckets.iter().map(|b| b.1).collect();
        if m.len() < 4 {
            return 0.0;
        }
        let d: Vec<f64> = m.windows(2).map(|w| w[1] - w[0]).collect();
        let var = d.iter().map(|v| v * v).sum::<f64>() / d.len() as f64 / 2.0;
        var.sqrt() / (m.len().min(LEVEL_BUCKETS) as f64).sqrt()
    }

    /// Residuals (y - envelope) of recent points within `window_us` of the newest.
    fn residuals(&self, window_us: f64) -> Vec<f64> {
        let Some(&(last_x, _)) = self.recent.back() else {
            return vec![];
        };
        let Some((slope, env_last)) = self.fit(last_x) else {
            return vec![];
        };
        let mut r: Vec<f64> = self
            .recent
            .iter()
            .filter(|p| last_x - p.0 <= window_us)
            .map(|&(x, y)| y - (env_last + slope * (x - last_x)))
            .collect();
        r.sort_by(|a, b| a.partial_cmp(b).unwrap());
        r
    }
}

struct Pending {
    payload: Vec<u8>,
    redundant: bool,
}

pub struct PlayoutBuffer {
    cfg: PlayoutConfig,
    codec: CodecConfig,
    decoder: Decoder,
    fs: usize,
    phase: Phase,
    first_x: Option<f64>,
    pending: BTreeMap<u64, Pending>,
    capture: BTreeMap<u64, i64>,
    env: Envelope,

    // decoded timeline (interleaved stereo), first sample = dec_start_frame * fs
    decoded: VecDeque<f32>,
    dec_start_frame: u64,
    next_decode_frame: u64,

    // locked mapping T(x) = c + (x - x0) * (1 + slope)
    x0: f64,
    c: f64,
    slope: f64,
    margin_us: f64,
    lower_candidate_since: Option<i64>,

    read_pos: f64,
    correcting: bool,
    // late tracking
    late_window: VecDeque<(i64, u32, u32)>, // (second, late, total)
    drift_streak: (i8, u32),
    out_scale: f64,
    last_reanchor_us: Option<i64>,
    force_jump: bool,
    slewing: bool,
    pub stats: PlayoutStats,
    recent_rms: f32,
    last_out_us: i64,
}

impl PlayoutBuffer {
    pub fn new(cfg: PlayoutConfig, codec: CodecConfig) -> Result<Self, CodecError> {
        Ok(PlayoutBuffer {
            decoder: Decoder::new(&codec)?,
            fs: codec.frame_samples,
            codec,
            cfg,
            phase: Phase::Measuring,
            first_x: None,
            pending: BTreeMap::new(),
            capture: BTreeMap::new(),
            env: Envelope::new(),
            decoded: VecDeque::new(),
            dec_start_frame: 0,
            next_decode_frame: 0,
            x0: 0.0,
            c: 0.0,
            slope: 0.0,
            margin_us: 0.0,
            lower_candidate_since: None,
            read_pos: 0.0,
            correcting: false,
            late_window: VecDeque::new(),
            drift_streak: (0, 0),
            out_scale: 1.0,
            last_reanchor_us: None,
            force_jump: false,
            slewing: false,
            stats: PlayoutStats::default(),
            recent_rms: 0.0,
            last_out_us: 0,
        })
    }

    pub fn is_playing(&self) -> bool {
        self.phase == Phase::Playing
    }

    /// Current planned delay above the arrival envelope (µs).
    pub fn margin_us(&self) -> f64 {
        self.margin_us
    }

    /// Feed one received media packet.
    pub fn push(&mut self, pkt: &MediaPacket, arrival_us: i64) {
        self.stats.packets += 1;
        let fs = self.fs as f64;
        let x = pkt.primary.index as f64 * fs * US_PER_SAMPLE;
        if self.first_x.is_none() {
            self.first_x = Some(x);
        }
        self.env.add(x, arrival_us as f64 - x);

        let sec = arrival_us / LATE_WIN_US;
        match self.late_window.back_mut() {
            Some(w) if w.0 == sec => w.2 += 1,
            _ => self.late_window.push_back((sec, 0, 1)),
        }
        while self.late_window.len() > 12 {
            self.late_window.pop_front();
        }

        for (i, f) in std::iter::once(&pkt.primary)
            .chain(pkt.redundant.iter())
            .enumerate()
        {
            self.capture.entry(f.index).or_insert(f.capture_us);
            if self.phase == Phase::Playing && f.index < self.next_decode_frame {
                if i == 0 {
                    self.stats.late_packets += 1;
                    if let Some(w) = self.late_window.back_mut() {
                        w.1 += 1;
                    }
                }
                continue;
            }
            if self.pending.contains_key(&f.index) {
                self.stats.duplicate_frames += (i == 0) as u64;
                continue;
            }
            self.pending.insert(
                f.index,
                Pending {
                    payload: f.payload.clone(),
                    redundant: i > 0,
                },
            );
        }
    }

    /// True once the network test window has been observed.
    pub fn ready_to_start(&self) -> bool {
        match (self.first_x, self.env.recent.back()) {
            (Some(fx), Some(&(lx, _))) => lx - fx >= self.cfg.network_test_us as f64,
            _ => false,
        }
    }

    /// Network-test summary: (p50, p99, p99.9, max) of jitter above the envelope, µs.
    pub fn jitter_summary(&self) -> (f64, f64, f64, f64) {
        let r = self.env.residuals(f64::MAX);
        (
            percentile(&r, 50.0),
            percentile(&r, 99.0),
            percentile(&r, 99.9),
            r.last().copied().unwrap_or(f64::NAN),
        )
    }

    /// Lock the delay and begin playout at local output time `now_us`.
    pub fn start(&mut self, now_us: i64, extra_us: f64) {
        let r = self.env.residuals(f64::MAX);
        let jitter = percentile(&r, self.cfg.margin_percentile).max(0.0);
        self.margin_us = self.margin_for(jitter) + extra_us;
        self.stats.initial_margin_us = self.margin_us as i64;
        let last_x = self.env.recent.back().unwrap().0;
        let (slope, env) = self.env.fit(last_x).unwrap();
        self.slope = slope;
        self.x0 = last_x;
        self.c = env + last_x + self.margin_us;
        self.phase = Phase::Playing;
        self.read_pos = self.desired_pos(now_us as f64);
        let f = (self.read_pos / self.fs as f64).floor() as i64 - 2;
        let f = f.max(0) as u64;
        self.dec_start_frame = f;
        self.next_decode_frame = f;
        self.decoded.clear();
        self.pending.retain(|&k, _| k >= f);
        self.last_out_us = now_us;
    }

    /// Change the planned delay by `delta_us` (e.g. beat-quantized monitoring).
    pub fn add_delay(&mut self, delta_us: f64, at_out_us: i64, reason: &str) {
        self.margin_us += delta_us;
        self.c += delta_us;
        let method = if delta_us.abs() >= self.cfg.jump_threshold_us {
            "jump"
        } else {
            "slew"
        };
        self.stats.reanchors.push(ReanchorEvent {
            at_out_us,
            reason: reason.to_string(),
            delta_us: delta_us as i64,
            method: method.into(),
        });
    }

    fn margin_for(&self, jitter_us: f64) -> f64 {
        (jitter_us + self.cfg.safety_us as f64).max(self.cfg.min_margin_us as f64)
            + (self.cfg.output_block_us + self.cfg.recovery_us) as f64
    }

    /// Error of the output device clock (ppm, + = fast). Each output sample then spans
    /// slightly less real time, and the resampling ratio is scaled to match.
    pub fn set_output_ppm(&mut self, ppm: f64) {
        self.out_scale = 1.0 / (1.0 + ppm * 1e-6);
    }

    fn desired_pos(&self, t_us: f64) -> f64 {
        let x = self.x0 + (t_us - self.c) / (1.0 + self.slope);
        x / US_PER_SAMPLE
    }

    fn decode_next(&mut self) -> Result<DecodeKind, CodecError> {
        let k = self.next_decode_frame;
        let (pcm, kind) = if let Some(p) = self.pending.remove(&k) {
            if p.redundant {
                self.stats.frames_from_redundancy += 1;
            } else {
                self.stats.frames_normal += 1;
            }
            (self.decoder.decode(&p.payload)?, DecodeKind::Normal)
        } else if let Some(next) = self.pending.get(&(k + 1)) {
            match self.decoder.decode_fec(&next.payload)? {
                Some(pcm) => {
                    self.stats.frames_fec += 1;
                    (pcm, DecodeKind::Fec)
                }
                None => {
                    self.stats.frames_plc += 1;
                    (self.decoder.conceal()?, DecodeKind::Plc)
                }
            }
        } else {
            self.stats.frames_plc += 1;
            (self.decoder.conceal()?, DecodeKind::Plc)
        };
        self.stats.frames_decoded += 1;
        self.decoded.extend(pcm);
        self.next_decode_frame += 1;
        Ok(kind)
    }

    fn sample(&self, i: i64, ch: usize) -> f32 {
        let start = (self.dec_start_frame * self.fs as u64) as i64;
        let idx = i - start;
        if idx < 0 {
            return 0.0;
        }
        self.decoded
            .get(idx as usize * CHANNELS + ch)
            .copied()
            .unwrap_or(0.0)
    }

    fn ensure_decoded(&mut self, pos: f64, concealed: &mut u32) -> Result<(), CodecError> {
        let need = ((pos.floor() as i64 + 3).max(0) as u64) / self.fs as u64;
        if need + 1 < self.dec_start_frame {
            return Ok(());
        }
        if self.next_decode_frame < self.dec_start_frame {
            self.next_decode_frame = self.dec_start_frame;
        }
        // Jumping far ahead: skip straight there instead of concealing a long gap.
        if need > self.next_decode_frame + 200 {
            let skip_to = need - 2;
            self.decoded.clear();
            self.dec_start_frame = skip_to;
            self.next_decode_frame = skip_to;
            self.pending.retain(|&k, _| k >= skip_to);
        }
        while self.next_decode_frame <= need {
            if self.decode_next()? != DecodeKind::Normal {
                *concealed += 1;
            }
        }
        Ok(())
    }

    fn interp(&self, p: f64) -> (f32, f32) {
        let i = p.floor() as i64;
        let t = (p - i as f64) as f32;
        if t == 0.0 {
            return (self.sample(i, 0), self.sample(i, 1));
        }
        let mut out = [0f32; 2];
        for ch in 0..2 {
            let y0 = self.sample(i - 1, ch);
            let y1 = self.sample(i, ch);
            let y2 = self.sample(i + 1, ch);
            let y3 = self.sample(i + 2, ch);
            // Catmull-Rom
            let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
            let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
            let c = -0.5 * y0 + 0.5 * y2;
            out[ch] = ((a * t + b) * t + c) * t + y1;
        }
        (out[0], out[1])
    }

    fn capture_tx_at(&self, pos: f64) -> Option<i64> {
        let k = (pos / self.fs as f64).floor().max(0.0) as u64;
        let base = self.capture.get(&k)?;
        let frac = pos - (k * self.fs as u64) as f64;
        Some(base + (frac * US_PER_SAMPLE).round() as i64)
    }

    fn maintenance(&mut self, now_us: i64) {
        // Drift tracking: compare the locked mapping with the live envelope.
        let x_now = self.read_pos * US_PER_SAMPLE;
        if let Some((slope, env)) = self.env.fit(x_now) {
            self.stats.drift_ppm_estimate = -slope * 1e6;
            let target_c_now = env + x_now + self.margin_us;
            let locked_now = self.c + (x_now - self.x0) * (1.0 + self.slope);
            let diff = target_c_now - locked_now;
            // Re-base the mapping on the new slope without moving the current point.
            if (slope - self.slope).abs() > 5e-6 {
                self.c = locked_now;
                self.x0 = x_now;
                self.slope = slope;
            }
            let deadband = self
                .cfg
                .deadband_us
                .max(3.0 * self.env.level_se())
                .min(4_000.0);
            if diff.abs() > deadband && diff.abs() < 5_000.0 {
                // Small, drift-sized mismatch: let the ASRC follow it, but only once it has
                // persisted for 5 s in the same direction (bucket-minimum noise is not drift).
                let sign = diff.signum() as i8;
                if self.drift_streak.0 == sign {
                    self.drift_streak.1 += 1;
                } else {
                    self.drift_streak = (sign, 1);
                }
                if self.drift_streak.1 >= 5 {
                    self.c += diff;
                    self.stats.asrc_corrections += 1;
                    self.drift_streak = (0, 0);
                }
                self.lower_candidate_since = None;
            } else if diff <= -5_000.0 {
                // The route got faster. Only follow if it stays that way.
                let since = *self.lower_candidate_since.get_or_insert(now_us);
                if now_us - since > self.cfg.lower_after_us {
                    self.c += diff;
                    self.stats.reanchors.push(ReanchorEvent {
                        at_out_us: now_us,
                        reason: "route improved, stable".into(),
                        delta_us: diff as i64,
                        method: "slew".into(),
                    });
                    self.lower_candidate_since = None;
                }
            } else {
                self.lower_candidate_since = None;
                self.drift_streak = (0, 0);
            }
        }

        // Sustained lateness → raise the delay once.
    }

    /// Lateness check, every quarter second. Windows are keyed by arrival time.
    fn lateness_check(&mut self, now_us: i64) {
        let cur = now_us / LATE_WIN_US;
        let sum = |from: i64, to: i64| {
            let (mut l, mut t) = (0u32, 0u32);
            for w in self.late_window.iter().filter(|w| w.0 >= from && w.0 < to) {
                l += w.1;
                t += w.2;
            }
            (l, t, if t > 20 { l as f64 / t as f64 } else { 0.0 })
        };
        let q = sum(cur - 1, cur); // last quarter second
        let s1 = sum(cur - 4, cur); // last second
        let s2 = sum(cur - 8, cur - 4); // the second before
        let trig = self.cfg.late_fraction_trigger;
        // Broken (a route change): a fifth of the last quarter second late. Otherwise
        // sustained: two seconds in a row over the trigger, or one second badly over it.
        let sustained = q.2 > 0.2 || (s1.2 > trig && s2.2 > trig) || s1.2 > 5.0 * trig;
        let (late, total) = if q.2 > 0.2 { (q.0, q.1) } else { (s1.0, s1.1) };
        let p1 = Some((late, total, 0.0));
        // Don't stack a second re-anchor on one that is still settling.
        let settling = self
            .last_reanchor_us
            .map(|t| now_us - t < 6_000_000)
            .unwrap_or(false);
        if let (true, false, Some((late, total, _))) = (sustained, settling, p1) {
            {
                // Lateness has lasted ≥ 2 s, so the last 0.9 s is all on the new path:
                // rebuild the envelope from it alone (mixing in old-path packets would
                // count the step as jitter and raise the delay twice).
                // For a sudden break only the last ~0.4 s is on the new path.
                self.env
                    .reset_to_recent(if q.2 > 0.2 { 400_000.0 } else { 900_000.0 });
                self.last_reanchor_us = Some(now_us);
                let r = self.env.residuals(f64::MAX);
                let jitter = percentile(&r, self.cfg.margin_percentile).max(0.0);
                let new_margin = self.margin_for(jitter);
                let x_now = self.read_pos * US_PER_SAMPLE;
                if let Some((slope, env)) = self.env.fit(x_now) {
                    let new_c_now = env + x_now + new_margin.max(self.margin_us);
                    let locked_now = self.c + (x_now - self.x0) * (1.0 + self.slope);
                    let delta = (new_c_now - locked_now).max(0.0);
                    self.c = locked_now + delta;
                    self.x0 = x_now;
                    self.slope = slope;
                    self.margin_us = new_margin.max(self.margin_us);
                    // If a big share of packets is already late the audio is broken anyway:
                    // jump now rather than slewing through seconds of concealment.
                    let broken = late as f64 / total as f64 > 0.2;
                    self.force_jump = broken;
                    let method = if broken
                        || self.recent_rms < self.cfg.silence_rms
                        || delta >= self.cfg.jump_threshold_us
                    {
                        "jump"
                    } else {
                        "slew"
                    };
                    self.stats.reanchors.push(ReanchorEvent {
                        at_out_us: now_us,
                        reason: format!("{late}/{total} recent packets late"),
                        delta_us: delta as i64,
                        method: method.into(),
                    });
                }
                self.late_window.clear();
            }
        }
    }

    /// Render `n` output samples (per channel) starting at local output time `out_us`.
    /// Output is interleaved stereo.
    pub fn render(
        &mut self,
        out_us: i64,
        n: usize,
        out: &mut Vec<f32>,
    ) -> Result<BlockInfo, CodecError> {
        out.clear();
        if self.phase != Phase::Playing {
            out.resize(n * CHANNELS, 0.0);
            return Ok(BlockInfo {
                silent: true,
                ratio: 1.0,
                ..Default::default()
            });
        }
        if out_us / 1_000_000 != self.last_out_us / 1_000_000 {
            self.maintenance(out_us);
        }
        if out_us / LATE_WIN_US != self.last_out_us / LATE_WIN_US {
            self.lateness_check(out_us);
        }
        self.last_out_us = out_us;

        let desired = self.desired_pos(out_us as f64);
        let err_us = (desired - self.read_pos) * US_PER_SAMPLE;
        let base_ratio = self.out_scale / (1.0 + self.slope);
        let mut concealed = 0u32;

        // Big change: jump with a crossfade over this block.
        let force = std::mem::take(&mut self.force_jump) && err_us.abs() > self.cfg.deadband_us;
        let jump_from = if force
            || err_us.abs() >= self.cfg.jump_threshold_us
            || (err_us.abs() > self.cfg.deadband_us && self.recent_rms < self.cfg.silence_rms)
        {
            let from = self.read_pos;
            self.read_pos = desired;
            self.correcting = false;
            Some(from)
        } else {
            None
        };

        let mut ratio = base_ratio;
        if jump_from.is_none() {
            if err_us.abs() > self.cfg.deadband_us {
                self.correcting = true;
            } else if err_us.abs() < self.cfg.deadband_exit_us {
                self.correcting = false;
            }
            if err_us.abs() > 5_000.0 {
                // A planned change (re-anchor) is being slewed in: finish it briskly.
                self.slewing = true;
            } else if err_us.abs() < self.cfg.deadband_exit_us {
                self.slewing = false;
            }
            if self.correcting {
                // Drift: close the error over ~4 s within the ASRC range.
                // Planned change: close it over ~1 s within the slew range.
                let (lim, tau_s) = if self.slewing {
                    (self.cfg.slew_max_ppm, 1.0)
                } else {
                    (self.cfg.asrc_max_ppm, 4.0)
                };
                let ppm = (err_us / tau_s).clamp(-lim, lim);
                ratio = base_ratio * (1.0 + ppm * 1e-6);
            }
        }

        let info_pos = self.read_pos;
        let end = self.read_pos + ratio * n as f64;
        self.ensure_decoded(end, &mut concealed)?;
        if let Some(from) = jump_from {
            self.ensure_decoded(from + ratio * n as f64, &mut concealed)?;
        }
        let mut energy = 0f32;
        for i in 0..n {
            let p = self.read_pos + ratio * i as f64;
            let (mut l, mut r) = self.interp(p);
            if let Some(from) = jump_from {
                let g = i as f32 / n as f32;
                let (ol, or) = self.interp(from + ratio * i as f64);
                l = l * g + ol * (1.0 - g);
                r = r * g + or * (1.0 - g);
            }
            energy += l * l + r * r;
            out.push(l);
            out.push(r);
        }
        self.read_pos = end;
        let rms = (energy / (2 * n) as f32).sqrt();
        self.recent_rms = 0.7 * self.recent_rms + 0.3 * rms;

        // Prune history.
        let keep_from = (self.read_pos - self.cfg.history_secs * SAMPLE_RATE as f64).max(0.0)
            as u64
            / self.fs as u64;
        while self.dec_start_frame < keep_from && self.decoded.len() >= self.fs * CHANNELS {
            self.decoded.drain(..self.fs * CHANNELS);
            self.dec_start_frame += 1;
        }
        let cap_keep = keep_from.saturating_sub(4);
        while let Some((&k, _)) = self.capture.first_key_value() {
            if k < cap_keep {
                self.capture.pop_first();
            } else {
                break;
            }
        }

        Ok(BlockInfo {
            read_pos: info_pos,
            capture_tx_us: self.capture_tx_at(info_pos),
            concealed_frames: concealed,
            ratio,
            silent: rms < self.cfg.silence_rms,
        })
    }

    pub fn codec(&self) -> &CodecConfig {
        &self.codec
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obsidian_codec::Encoder;
    use obsidian_protocol::{CodecId, Frame};
    use rand::{Rng, SeedableRng};

    struct Sim {
        delays: Vec<f64>,
        stats: PlayoutStats,
    }

    /// Simulated time: sender clock skew `ppm`, jitter σ, random loss, redundancy offsets.
    fn simulate(ppm: f64, jitter_us: f64, loss: f64, redundancy: &[u64], secs: f64) -> Sim {
        let codec = CodecConfig {
            codec: CodecId::Pcm16,
            ..Default::default()
        };
        let mut enc = Encoder::new(&codec).unwrap();
        let recovery_us = redundancy
            .iter()
            .max()
            .map(|&o| o as i64 * 5_000)
            .unwrap_or(0);
        let cfg = PlayoutConfig {
            network_test_us: 3_000_000,
            recovery_us,
            ..Default::default()
        };
        let mut pb = PlayoutBuffer::new(cfg, codec.clone()).unwrap();
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let frame_us = 5_000.0 / (1.0 + ppm * 1e-6);
        let mut payloads: Vec<Vec<u8>> = Vec::new();
        // Events: (arrival, packet)
        let mut arrivals = Vec::new();
        let n_frames = (secs * 1e6 / frame_us) as u64;
        let mut last_arrival = 0f64;
        for k in 0..n_frames {
            let cap = 1_000_000.0 + k as f64 * frame_us;
            let pcm: Vec<f32> = (0..480)
                .map(|i| ((k * 240 + i / 2) as f32 * 0.01).sin() * 0.3)
                .collect();
            payloads.push(enc.encode(&pcm).unwrap());
            let mk = |idx: u64| Frame {
                index: idx,
                capture_us: (1_000_000.0 + idx as f64 * frame_us) as i64,
                payload: payloads[idx as usize].clone(),
            };
            let pkt = MediaPacket {
                stream_id: 1,
                seq: k as u32,
                codec: CodecId::Pcm16,
                frame_samples: 240,
                primary: mk(k),
                redundant: redundancy
                    .iter()
                    .filter(|&&o| o <= k)
                    .map(|&o| mk(k - o))
                    .collect(),
            };
            if rng.gen::<f64>() < loss {
                continue;
            }
            let j: f64 = rng.gen::<f64>() * jitter_us;
            let a = (cap + 5_000.0 + 20_000.0 + j).max(last_arrival);
            last_arrival = a;
            arrivals.push((a as i64, pkt));
        }
        let mut delays = Vec::new();
        let mut out = Vec::new();
        let mut ai = 0;
        let mut t = 1_000_000i64;
        let end = (1e6 + secs * 1e6) as i64 - 200_000;
        while t < end {
            while ai < arrivals.len() && arrivals[ai].0 <= t {
                pb.push(&arrivals[ai].1, arrivals[ai].0);
                ai += 1;
            }
            if !pb.is_playing() && pb.ready_to_start() {
                pb.start(t, 0.0);
            }
            let info = pb.render(t, 240, &mut out).unwrap();
            if pb.is_playing() {
                if let Some(c) = info.capture_tx_us {
                    delays.push((t - c) as f64);
                }
            }
            t += 5_000;
        }
        Sim {
            delays,
            stats: pb.stats.clone(),
        }
    }

    fn spread(d: &[f64]) -> f64 {
        let lo = d.iter().cloned().fold(f64::MAX, f64::min);
        let hi = d.iter().cloned().fold(f64::MIN, f64::max);
        hi - lo
    }

    #[test]
    fn constant_delay_under_jitter() {
        let s = simulate(0.0, 8_000.0, 0.0, &[], 30.0);
        assert!(s.delays.len() > 4000);
        assert!(
            spread(&s.delays) < 100.0,
            "delay spread {} µs",
            spread(&s.delays)
        );
        assert_eq!(s.stats.frames_plc, 0, "{:?}", s.stats);
        // margin ≈ p99.5 of uniform[0,8ms] + 2 ms safety
        assert!(s.stats.initial_margin_us > 14_000 && s.stats.initial_margin_us < 15_500);
    }

    #[test]
    fn follows_clock_drift_without_wandering() {
        let s = simulate(150.0, 2_000.0, 0.0, &[], 90.0);
        let tail = &s.delays[s.delays.len() / 3..];
        let last = &s.delays[s.delays.len() * 2 / 3..];
        eprintln!(
            "drift: tail spread {} µs, last-third spread {} µs, est {} ppm, corrections {}",
            spread(tail),
            spread(last),
            s.stats.drift_ppm_estimate,
            s.stats.asrc_corrections
        );
        // Report target: ±2 ms.
        assert!(spread(tail) < 4_000.0, "delay spread {} µs", spread(tail));
        assert!(
            spread(last) < 1_500.0,
            "late delay spread {} µs",
            spread(last)
        );
        assert!(
            (s.stats.drift_ppm_estimate - 150.0).abs() < 30.0,
            "{}",
            s.stats.drift_ppm_estimate
        );
        assert_eq!(s.stats.frames_plc, 0, "{:?}", s.stats);
    }

    #[test]
    fn redundancy_recovers_random_loss() {
        let none = simulate(0.0, 2_000.0, 0.02, &[], 20.0);
        let red = simulate(0.0, 2_000.0, 0.02, &[1], 20.0);
        assert!(none.stats.frames_plc > 30);
        assert!(
            red.stats.frames_plc * 10 < none.stats.frames_plc,
            "{} vs {}",
            red.stats.frames_plc,
            none.stats.frames_plc
        );
        assert!(red.stats.frames_from_redundancy > 30);
    }
}

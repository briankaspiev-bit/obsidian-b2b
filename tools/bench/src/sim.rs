//! Virtual-time simulator: one direction of the booth link (sender → impaired path →
//! playout buffer) run as fast as the CPU allows, with the real Opus codec, the real
//! playout buffer and the same impairment model as the UDP proxy.
//!
//! Why: the real-time bench runs on whatever host it gets. A cloud VM has scheduler
//! stalls of 20-40 ms (hypervisor steal) that look exactly like network jitter. The
//! simulator has no host noise, is reproducible from a seed, and can run the
//! report's 60-minute criteria in about a minute.

use crate::gen;
use obsidian_clock::percentile;
use obsidian_codec::{CodecConfig, Decoder, Encoder};
use obsidian_jitter::{PlayoutBuffer, PlayoutConfig};
use obsidian_netem_proxy::{Impairer, Profile};
use obsidian_protocol::{Frame, MediaPacket};
use serde::Serialize;
use std::collections::VecDeque;

#[derive(Clone)]
pub struct SimCase {
    pub name: String,
    pub profile: Profile,
    pub minutes: f64,
    pub redundancy: Vec<u64>,
    pub codec: CodecConfig,
    pub tx_ppm: f64,
    pub rx_ppm: f64,
    pub seed: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimResult {
    pub name: String,
    pub profile: String,
    pub minutes: f64,
    pub redundancy: Vec<u64>,
    pub tx_ppm: f64,
    pub rx_ppm: f64,
    pub delay_p50_ms: f64,
    pub delay_initial_ms: f64,
    pub delay_final_ms: f64,
    pub steady_max_wander_ms: f64,
    pub unplanned_excursion_blocks: usize,
    pub reanchors: Vec<String>,
    pub frames: u64,
    pub recovered_redundancy: u64,
    pub concealed_plc: u64,
    pub late_packets: u64,
    pub lost_on_path: u64,
    pub glitch_events: usize,
    pub glitches_per_30min: f64,
    pub drift_ppm_estimate: f64,
    pub send_kbps: f64,
}

struct RefRing {
    start_frame: u64,
    buf: VecDeque<f32>,
    fs: usize,
}

impl RefRing {
    fn get(&self, i: i64, ch: usize) -> f32 {
        let idx = i - (self.start_frame * self.fs as u64) as i64;
        if idx < 0 {
            return 0.0;
        }
        self.buf.get(idx as usize * 2 + ch).copied().unwrap_or(0.0)
    }
    fn interp(&self, p: f64, ch: usize) -> f32 {
        let i = p.floor() as i64;
        let t = (p - i as f64) as f32;
        if t == 0.0 {
            return self.get(i, ch);
        }
        let (y0, y1, y2, y3) = (
            self.get(i - 1, ch),
            self.get(i, ch),
            self.get(i + 1, ch),
            self.get(i + 2, ch),
        );
        let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
        let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c = -0.5 * y0 + 0.5 * y2;
        ((a * t + b) * t + c) * t + y1
    }
}

pub fn run(case: &SimCase) -> anyhow::Result<SimResult> {
    let program = gen::track(&gen::dj_a());
    let plen = program.len() / 2;
    let fs = case.codec.frame_samples;
    let frame_us = fs as f64 * 1e6 / 48_000.0;
    let tx_period = frame_us / (1.0 + case.tx_ppm * 1e-6);
    let rx_period = frame_us / (1.0 + case.rx_ppm * 1e-6);
    let max_red = case.redundancy.iter().max().copied().unwrap_or(0);

    let mut enc = Encoder::new(&case.codec)?;
    let mut ref_dec = Decoder::new(&case.codec)?;
    let cfg = PlayoutConfig {
        recovery_us: max_red as i64 * frame_us as i64,
        output_block_us: frame_us as i64,
        ..Default::default()
    };
    let mut pb = PlayoutBuffer::new(cfg, case.codec.clone())?;
    pb.set_output_ppm(case.rx_ppm);
    let mut path = Impairer::new(case.profile.clone(), case.seed);

    let t0 = 1_000_000.0f64;
    let end = t0 + case.minutes * 60e6;
    let mut history: VecDeque<Frame> = VecDeque::new();
    let mut inflight: VecDeque<(f64, MediaPacket)> = VecDeque::new();
    let mut refr = RefRing {
        start_frame: 0,
        buf: VecDeque::new(),
        fs,
    };
    let mut k: u64 = 0;
    let mut bytes: u64 = 0;
    let mut pcm = Vec::with_capacity(fs * 2);
    let mut out = Vec::with_capacity(fs * 2);

    let mut delays: Vec<(f64, f32)> = Vec::new();
    let mut event_times: Vec<(f64, bool)> = Vec::new(); // (time, is_jump)
    let mut damaged: Vec<u64> = Vec::new();
    let mut seen_events = 0;
    let mut j: u64 = 0;
    loop {
        let t = t0 + j as f64 * rx_period;
        if t >= end {
            break;
        }
        // Sender: everything captured and ready by t.
        while t0 + (k + 1) as f64 * tx_period <= t {
            let capture = t0 + k as f64 * tx_period;
            pcm.clear();
            for i in 0..fs {
                let idx = ((k as usize * fs) + i) % plen;
                pcm.push(program[idx * 2]);
                pcm.push(program[idx * 2 + 1]);
            }
            let payload = enc.encode(&pcm)?;
            refr.buf.extend(ref_dec.decode(&payload)?);
            let primary = Frame {
                index: k,
                capture_us: capture as i64,
                payload,
            };
            let redundant = case
                .redundancy
                .iter()
                .filter_map(|&o| history.iter().rev().find(|f| f.index + o == k).cloned())
                .collect::<Vec<_>>();
            bytes += 28
                + 30
                + primary.payload.len() as u64
                + redundant
                    .iter()
                    .map(|f| 18 + f.payload.len() as u64)
                    .sum::<u64>();
            let pkt = MediaPacket {
                stream_id: 1,
                seq: k as u32,
                codec: case.codec.codec,
                frame_samples: fs as u16,
                primary: primary.clone(),
                redundant,
            };
            let send_s = (capture + tx_period - t0) / 1e6;
            if let Some(at) = path.process(send_s) {
                inflight.push_back((t0 + at * 1e6, pkt));
            }
            history.push_back(primary);
            while history.len() > max_red.max(1) as usize {
                history.pop_front();
            }
            k += 1;
        }
        while inflight.front().map(|p| p.0 <= t).unwrap_or(false) {
            let (at, pkt) = inflight.pop_front().unwrap();
            pb.push(&pkt, at as i64);
        }
        j += 1;
        if !pb.is_playing() {
            if pb.ready_to_start() {
                pb.start(t as i64, 0.0);
            } else {
                continue;
            }
        }
        let info = pb.render(t as i64, fs, &mut out)?;
        while pb.stats.reanchors.len() > seen_events {
            let e = &pb.stats.reanchors[seen_events];
            event_times.push((t, e.method == "jump"));
            seen_events += 1;
        }
        let capture = t0 + info.read_pos / fs as f64 * tx_period;
        delays.push((t, ((t - capture) / 1e3) as f32));
        let near = event_times
            .last()
            .map(|(et, _)| t - et < 20_000.0)
            .unwrap_or(false);
        let (mut s, mut e) = (0f64, 0f64);
        for i in 0..fs {
            let p = info.read_pos + info.ratio * i as f64;
            for ch in 0..2 {
                let want = refr.interp(p, ch) as f64;
                let got = out[i * 2 + ch] as f64;
                s += want * want;
                e += (want - got) * (want - got);
            }
        }
        if !near && (s / (2 * fs) as f64).sqrt() > 1e-3 && 10.0 * (s / (e + 1e-20)).log10() < 20.0 {
            damaged.push(j);
        }
        // Prune the reference ring behind the read head (keep 5 s for rewinds).
        let keep_from = ((info.read_pos - 5.0 * 48_000.0).max(0.0) as u64) / fs as u64;
        while refr.start_frame < keep_from && refr.buf.len() >= fs * 2 {
            refr.buf.drain(..fs * 2);
            refr.start_frame += 1;
        }
    }

    let mut glitch_events = 0;
    let mut last: Option<u64> = None;
    for &b in &damaged {
        if last.map(|l| b - l > 10).unwrap_or(true) {
            glitch_events += 1;
        }
        last = Some(b);
    }
    // Steady-state wander per segment between planned changes.
    let mut bounds: Vec<f64> = event_times.iter().map(|e| e.0).collect();
    bounds.push(f64::MAX);
    let (mut wander, mut excursions) = (0f64, 0usize);
    let mut seg_start = f64::MIN;
    for (bi, &b) in bounds.iter().enumerate() {
        let settle = if bi == 0 {
            0.0
        } else if event_times[bi - 1].1 {
            100_000.0
        } else {
            8_000_000.0
        };
        let mut seg: Vec<f64> = delays
            .iter()
            .filter(|(t, _)| *t >= seg_start + settle && *t < b)
            .map(|v| v.1 as f64)
            .collect();
        if seg.len() > 200 {
            let raw = seg.clone();
            seg.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = percentile(&seg, 50.0);
            for v in raw {
                let d = (v - med).abs();
                wander = wander.max(d);
                excursions += (d > 2.0) as usize;
            }
        }
        seg_start = b;
    }
    let mut all: Vec<f64> = delays.iter().map(|d| d.1 as f64).collect();
    all.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let played_min = delays.len() as f64 * frame_us / 60e6;
    Ok(SimResult {
        name: case.name.clone(),
        profile: case.profile.name.clone(),
        minutes: case.minutes,
        redundancy: case.redundancy.clone(),
        tx_ppm: case.tx_ppm,
        rx_ppm: case.rx_ppm,
        delay_p50_ms: percentile(&all, 50.0),
        delay_initial_ms: delays.first().map(|d| d.1 as f64).unwrap_or(f64::NAN),
        delay_final_ms: delays.last().map(|d| d.1 as f64).unwrap_or(f64::NAN),
        steady_max_wander_ms: wander,
        unplanned_excursion_blocks: excursions,
        reanchors: pb
            .stats
            .reanchors
            .iter()
            .map(|e| {
                format!(
                    "{:+.1} ms {} ({})",
                    e.delta_us as f64 / 1e3,
                    e.method,
                    e.reason
                )
            })
            .collect(),
        frames: pb.stats.frames_decoded,
        recovered_redundancy: pb.stats.frames_from_redundancy,
        concealed_plc: pb.stats.frames_plc,
        late_packets: pb.stats.late_packets,
        lost_on_path: path.stats.dropped,
        glitch_events,
        glitches_per_30min: glitch_events as f64 / played_min * 30.0,
        drift_ppm_estimate: pb.stats.drift_ppm_estimate,
        send_kbps: bytes as f64 * 8.0 / (k as f64 * tx_period / 1e6) / 1e3,
    })
}

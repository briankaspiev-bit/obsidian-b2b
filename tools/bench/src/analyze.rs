//! Offline scoring of one direction (sender Y → receiver X).
//!
//! Ground truth comes from files the peers wrote, never from the engine's own
//! estimates:
//! * Y/sent.wav is exactly what Y encoded, indexed by Y's sample position.
//! * A clean (lossless) encode→decode of sent.wav is the reference: with no
//!   loss, the live decoder produces it bit-for-bit, so any difference in X's
//!   monitor output is network damage (concealment), not codec quality.
//! * X/blocks.csv says which sender position each output block played, and the
//!   capture timestamp; the bench knows each peer's simulated clock offset, so
//!   it computes the true end-to-end delay itself.

use anyhow::{Context, Result};
use obsidian_align::{onset_envelope, phase_lag};
use obsidian_codec::{CodecConfig, CodecId, Decoder, Encoder};
use obsidian_engine::read_wav;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct DirResult {
    pub direction: String,
    pub blocks: usize,
    // delay
    pub delay_p50_ms: f64,
    pub delay_min_ms: f64,
    pub delay_max_ms: f64,
    /// Largest deviation from the segment median while no planned change was in progress.
    pub steady_max_wander_ms: f64,
    /// Steady-state blocks more than 2 ms from their segment median.
    pub unplanned_excursion_blocks: usize,
    pub planned_changes: usize,
    pub booth_margin_ms: f64,
    // loss handling
    pub frames: u64,
    pub recovered_redundancy: u64,
    pub recovered_fec: u64,
    pub concealed_plc: u64,
    pub late_packets: u64,
    // audible damage
    pub damaged_blocks: usize,
    pub glitch_events: usize,
    pub glitches_per_10min: f64,
    pub worst_block_snr_db: f64,
    pub monitor_seconds: f64,
    // codec
    pub codec_snr_db: f64,
    pub codec_delay_samples: usize,
    // clocks
    pub clock_sync_error_us: Option<i64>,
    pub drift_ppm_estimate: f64,
    pub asrc_corrections: u64,
    // beat alignment of the remote as heard by X, last 20 s
    pub beat_residual_ms: Option<f64>,
    pub send_kbps: f64,
    pub cpu_percent: Option<f64>,
}

pub fn codec_from_report(r: &Value) -> CodecConfig {
    let codec = if r["codec"].as_str() == Some("Pcm16") {
        CodecId::Pcm16
    } else {
        CodecId::Opus
    };
    CodecConfig {
        codec,
        bitrate: r["bitrate"].as_i64().unwrap_or(256_000) as i32,
        frame_samples: r["frame_samples"].as_u64().unwrap_or(240) as usize,
        ..Default::default()
    }
}

fn reference_decode(sent: &[f32], codec: &CodecConfig) -> Result<Vec<f32>> {
    let mut enc = Encoder::new(codec)?;
    let mut dec = Decoder::new(codec)?;
    let fl = codec.frame_len();
    let mut out = Vec::with_capacity(sent.len());
    for f in sent.chunks_exact(fl) {
        out.extend(dec.decode(&enc.encode(f)?)?);
    }
    Ok(out)
}

fn snr_db(sig: f64, err: f64) -> f64 {
    10.0 * ((sig + 1e-20) / (err + 1e-20)).log10()
}

fn codec_quality(sent: &[f32], reference: &[f32]) -> (f64, usize) {
    // Find codec delay on a 2 s excerpt, then SNR over everything.
    // Excerpt: the loudest 2 s (a DJ is silent before dropping in or after fading out).
    let frames = sent.len() / 2;
    let start = (0..frames.saturating_sub(48_000 * 3) / 48_000)
        .map(|sec| sec * 48_000)
        .max_by(|&a, &b| {
            let e = |s: usize| {
                sent[s * 2..(s + 96_000) * 2]
                    .iter()
                    .map(|v| v * v)
                    .sum::<f32>()
            };
            e(a).partial_cmp(&e(b)).unwrap()
        })
        .unwrap_or(0);
    let len = 48_000 * 2;
    if reference.len() < (start + len + 2000) * 2 {
        return (f64::NAN, 0);
    }
    let mut best = (f64::MIN, 0usize);
    for d in 0..1000 {
        let mut c = 0f64;
        for i in start..start + len {
            c += sent[i * 2] as f64 * reference[(i + d) * 2] as f64;
        }
        if c > best.0 {
            best = (c, d);
        }
    }
    let d = best.1;
    let (mut s, mut e) = (0f64, 0f64);
    for i in 48_000..(reference.len() / 2 - d - 1) {
        for ch in 0..2 {
            let x = sent[i * 2 + ch] as f64;
            let y = reference[(i + d) * 2 + ch] as f64;
            s += x * x;
            e += (x - y) * (x - y);
        }
    }
    (snr_db(s, e), d)
}

fn sample(buf: &[f32], i: i64, ch: usize) -> f32 {
    if i < 0 {
        return 0.0;
    }
    buf.get(i as usize * 2 + ch).copied().unwrap_or(0.0)
}

fn interp(buf: &[f32], p: f64, ch: usize) -> f32 {
    let i = p.floor() as i64;
    let t = (p - i as f64) as f32;
    if t == 0.0 {
        return sample(buf, i, ch);
    }
    let (y0, y1, y2, y3) = (
        sample(buf, i - 1, ch),
        sample(buf, i, ch),
        sample(buf, i + 1, ch),
        sample(buf, i + 2, ch),
    );
    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c = -0.5 * y0 + 0.5 * y2;
    ((a * t + b) * t + c) * t + y1
}

struct Row {
    out_us: i64,
    read_pos: f64,
    capture_tx: Option<i64>,
    offset_est: Option<i64>,
    ratio: f64,
}

fn read_blocks(p: &Path) -> Result<Vec<Row>> {
    let s = std::fs::read_to_string(p)?;
    let mut rows = Vec::new();
    for line in s.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        rows.push(Row {
            out_us: f[0].parse()?,
            read_pos: f[1].parse()?,
            capture_tx: f[2].parse().ok(),
            offset_est: f[3].parse().ok(),
            ratio: f[6].parse()?,
        });
    }
    Ok(rows)
}

pub fn analyze_direction(label: &str, rx_dir: &Path, tx_dir: &Path, bpm: f64) -> Result<DirResult> {
    let rx: Value = serde_json::from_str(
        &std::fs::read_to_string(rx_dir.join("report.json")).context("rx report")?,
    )?;
    let tx: Value = serde_json::from_str(
        &std::fs::read_to_string(tx_dir.join("report.json")).context("tx report")?,
    )?;
    let codec = codec_from_report(&tx);
    let sent = read_wav(&tx_dir.join("sent.wav"))?;
    let reference = reference_decode(&sent, &codec)?;
    let (codec_snr_db, codec_delay) = if codec.codec == CodecId::Pcm16 {
        let (mut s, mut e) = (0f64, 0f64);
        for (x, y) in sent.iter().zip(&reference) {
            s += (*x as f64).powi(2);
            e += (*x as f64 - *y as f64).powi(2);
        }
        (snr_db(s, e), 0)
    } else {
        codec_quality(&sent, &reference)
    };
    let monitor = read_wav(&rx_dir.join("monitor.wav"))?;
    let rows = read_blocks(&rx_dir.join("blocks.csv"))?;
    let n = codec.frame_samples;

    let off_rx = rx["clock_offset_param_us"].as_i64().unwrap_or(0);
    let off_tx = tx["clock_offset_param_us"].as_i64().unwrap_or(0);
    let true_offset = off_tx - off_rx;

    // Planned changes (re-anchors, beat quantization).
    let events: Vec<(i64, String)> = rx["playout"]["reanchors"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| {
                    (
                        e["at_out_us"].as_i64().unwrap_or(0),
                        e["method"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    // ---- audio damage ----
    let mut damaged = Vec::new();
    let mut worst = f64::INFINITY;
    for (j, r) in rows.iter().enumerate() {
        if (j + 1) * n * 2 > monitor.len() {
            break;
        }
        let near_event = events
            .iter()
            .any(|(t, _)| r.out_us >= *t - 6_000 && r.out_us <= *t + 16_000);
        let (mut s, mut e) = (0f64, 0f64);
        for i in 0..n {
            let p = r.read_pos + r.ratio * i as f64;
            for ch in 0..2 {
                let want = interp(&reference, p, ch) as f64;
                let got = monitor[(j * n + i) * 2 + ch] as f64;
                s += want * want;
                e += (want - got) * (want - got);
            }
        }
        let rms = (s / (2 * n) as f64).sqrt();
        if rms < 1e-3 || near_event {
            continue;
        }
        let snr = snr_db(s, e);
        worst = worst.min(snr);
        if snr < 20.0 {
            damaged.push(j);
        }
    }
    let mut glitch_events = 0;
    let mut last: Option<usize> = None;
    for &j in &damaged {
        if last.map(|l| j - l > 10).unwrap_or(true) {
            glitch_events += 1;
        }
        last = Some(j);
    }
    let monitor_seconds = rows.len() as f64 * n as f64 / 48_000.0;

    // ---- true delay ----
    let mut series: Vec<(i64, f64)> = Vec::new();
    for r in &rows {
        if let Some(c) = r.capture_tx {
            let d = (r.out_us - off_rx) - (c - off_tx);
            series.push((r.out_us, d as f64 / 1e3));
        }
    }
    let mut sorted: Vec<f64> = series.iter().map(|v| v.1).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = obsidian_clock::percentile(&sorted, 50.0);
    // Steady state: exclude 100 ms after jumps, 8 s after slews.
    let mut steady_max = 0f64;
    let mut excursions = 0usize;
    let mut bounds: Vec<i64> = events.iter().map(|e| e.0).collect();
    bounds.push(i64::MAX);
    let mut seg_start = i64::MIN;
    for (bi, &b) in bounds.iter().enumerate() {
        let settle = if bi == 0 {
            0
        } else if events[bi - 1].1 == "jump" {
            100_000
        } else {
            8_000_000
        };
        let seg: Vec<f64> = series
            .iter()
            .filter(|(t, _)| *t >= seg_start + settle && *t < b)
            .map(|v| v.1)
            .collect();
        if seg.len() > 200 {
            let mut s = seg.clone();
            s.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = obsidian_clock::percentile(&s, 50.0);
            for v in &seg {
                let dev = (v - med).abs();
                steady_max = steady_max.max(dev);
                if dev > 2.0 {
                    excursions += 1;
                }
            }
        }
        seg_start = b;
    }

    // ---- clock sync error ----
    let clock_sync_error_us = rows
        .iter()
        .rev()
        .find_map(|r| r.offset_est)
        .map(|o| o - true_offset);

    // ---- beat residual: remote as heard vs our own program as heard, last 20 s ----
    let beat_residual_ms = (|| -> Option<f64> {
        let rx_sent = read_wav(&rx_dir.join("sent.wav")).ok()?;
        let start_local = rx["start_at_us"].as_i64()? + off_rx;
        let skew = rx["skew_ppm"].as_f64().unwrap_or(0.0);
        let take = (20.0 * 48_000.0 / n as f64) as usize;
        if rows.len() < take + 10 {
            return None;
        }
        let first = rows.len() - take;
        let mut local = Vec::with_capacity(take * n * 2);
        let mut remote = Vec::with_capacity(take * n * 2);
        for (j, r) in rows.iter().enumerate().skip(first) {
            let lp =
                ((r.out_us - start_local) as f64 * (1.0 + skew * 1e-6) * 48_000.0 / 1e6) as usize;
            for i in 0..n {
                local.push(*rx_sent.get((lp + i) * 2)?);
                local.push(*rx_sent.get((lp + i) * 2 + 1)?);
                remote.push(*monitor.get((j * n + i) * 2)?);
                remote.push(*monitor.get((j * n + i) * 2 + 1)?);
            }
        }
        let le = onset_envelope(&local, 48);
        let re = onset_envelope(&remote, 48);
        if le.iter().all(|v| *v == 0.0) || re.iter().all(|v| *v == 0.0) {
            return None;
        }
        let period = 60.0 / bpm * 1000.0;
        let (lag, _) = phase_lag(&le, &re, period)?;
        Some(if lag > period / 2.0 {
            lag - period
        } else {
            lag
        })
    })();

    let cpu = rx["cpu_seconds"]
        .as_f64()
        .map(|c| c / (monitor_seconds + 10.0) * 100.0);
    Ok(DirResult {
        direction: label.to_string(),
        blocks: rows.len(),
        delay_p50_ms: p50,
        delay_min_ms: sorted.first().copied().unwrap_or(f64::NAN),
        delay_max_ms: sorted.last().copied().unwrap_or(f64::NAN),
        steady_max_wander_ms: steady_max,
        unplanned_excursion_blocks: excursions,
        planned_changes: events.len(),
        booth_margin_ms: rx["booth_margin_ms"].as_f64().unwrap_or(f64::NAN),
        frames: rx["playout"]["frames_decoded"].as_u64().unwrap_or(0),
        recovered_redundancy: rx["playout"]["frames_from_redundancy"]
            .as_u64()
            .unwrap_or(0),
        recovered_fec: rx["playout"]["frames_fec"].as_u64().unwrap_or(0),
        concealed_plc: rx["playout"]["frames_plc"].as_u64().unwrap_or(0),
        late_packets: rx["playout"]["late_packets"].as_u64().unwrap_or(0),
        damaged_blocks: damaged.len(),
        glitch_events,
        glitches_per_10min: glitch_events as f64 / monitor_seconds * 600.0,
        worst_block_snr_db: worst,
        monitor_seconds,
        codec_snr_db,
        codec_delay_samples: codec_delay,
        clock_sync_error_us,
        drift_ppm_estimate: rx["playout"]["drift_ppm_estimate"].as_f64().unwrap_or(0.0),
        asrc_corrections: rx["playout"]["asrc_corrections"].as_u64().unwrap_or(0),
        beat_residual_ms,
        send_kbps: tx["send_kbps"].as_f64().unwrap_or(f64::NAN),
        cpu_percent: cpu,
    })
}

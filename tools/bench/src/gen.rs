//! Deterministic test music: a 125 BPM club loop per DJ, so beat alignment,
//! transients (where dropouts are most audible) and full-band content are all
//! exercised. Not real music; good enough to measure the plumbing.

use rand::{Rng, SeedableRng};
use std::f32::consts::PI;

const FS: f32 = 48_000.0;

pub struct TrackSpec {
    pub seed: u64,
    pub bpm: f32,
    pub bars: usize,
    /// Bass root (Hz).
    pub root: f32,
    /// Chord intervals in semitones for the four 4-bar sections.
    pub chords: [[f32; 3]; 4],
    pub arp: bool,
}

fn semis(f: f32, s: f32) -> f32 {
    f * 2f32.powf(s / 12.0)
}

fn add(buf: &mut [f32], at: usize, l: f32, r: f32) {
    if at * 2 + 1 < buf.len() {
        buf[at * 2] += l;
        buf[at * 2 + 1] += r;
    }
}

/// Interleaved stereo, peak-normalised to about -3 dBFS. The first kick is at sample 0.
pub fn track(spec: &TrackSpec) -> Vec<f32> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(spec.seed);
    let beat = 60.0 / spec.bpm;
    let beat_n = (beat * FS).round() as usize;
    let total_beats = spec.bars * 4;
    let n = total_beats * beat_n;
    let mut buf = vec![0f32; n * 2];

    for b in 0..total_beats {
        let t0 = b * beat_n;
        // Kick
        let mut ph = 0f32;
        for i in 0..(0.4 * FS) as usize {
            let t = i as f32 / FS;
            let f = 48.0 + 110.0 * (-t * 35.0).exp();
            ph += 2.0 * PI * f / FS;
            let click = if i < 48 {
                (1.0 - i as f32 / 48.0) * 0.4
            } else {
                0.0
            };
            let s = (ph.sin() * (-t * 9.0).exp() + click) * 0.9;
            add(&mut buf, t0 + i, s, s);
        }
        // Clap on 2 and 4
        if b % 2 == 1 {
            let mut lp = 0f32;
            for i in 0..(0.18 * FS) as usize {
                let t = i as f32 / FS;
                let noise: f32 = rng.gen_range(-1.0..1.0);
                lp = lp + 0.35 * (noise - lp);
                let s = (noise - lp) * (-t * 22.0).exp() * 0.35;
                add(&mut buf, t0 + i, s * 0.9, s);
            }
        }
        // Hats: 16ths closed, offbeat open
        for k in 0..4 {
            let at = t0 + k * beat_n / 4;
            let open = k == 2;
            let len = if open { 0.15 } else { 0.035 };
            let mut prev = 0f32;
            let pan = if k % 2 == 0 { 0.8 } else { 1.0 };
            for i in 0..(len * FS) as usize {
                let t = i as f32 / FS;
                let noise: f32 = rng.gen_range(-1.0..1.0);
                let hp = noise - prev;
                prev = noise;
                let s = hp * (-t / (len * 0.35)).exp() * if open { 0.12 } else { 0.08 };
                add(&mut buf, at + i, s * pan, s * (1.8 - pan));
            }
        }
        // Bass: offbeat 8th notes
        let section = (b / 16) % 4;
        let bass_f = semis(spec.root, spec.chords[section][0]);
        let at = t0 + beat_n / 2;
        let mut lp = 0f32;
        for i in 0..(beat * 0.45 * FS) as usize {
            let t = i as f32 / FS;
            let saw = 2.0 * ((bass_f * t).fract()) - 1.0;
            lp += 0.06 * (saw - lp);
            let env = (1.0 - (-t * 200.0).exp()) * (-t * 4.0).exp();
            let s = lp * env * 0.5;
            add(&mut buf, at + i, s, s);
        }
        // Optional arp (16ths) for variety between DJs
        if spec.arp {
            for k in 0..4 {
                let at = t0 + k * beat_n / 4;
                let note = spec.chords[section][(b * 4 + k) % 3] + 24.0;
                let f = semis(spec.root, note);
                for i in 0..(0.09 * FS) as usize {
                    let t = i as f32 / FS;
                    let s = (2.0 * PI * f * t).sin()
                        * (2.0 * PI * f * 2.0 * t).sin().abs()
                        * (-t * 30.0).exp()
                        * 0.10;
                    let pan = if k % 2 == 0 { 0.6 } else { 1.4 };
                    add(&mut buf, at + i, s * pan, s * (2.0 - pan));
                }
            }
        }
    }
    // Pad: detuned saws per 4-bar section, gently low-passed, wide stereo.
    let sec_n = 16 * beat_n;
    let (mut lpl, mut lpr) = (0f32, 0f32);
    for i in 0..n {
        let section = (i / sec_n) % 4;
        let t = i as f32 / FS;
        let mut l = 0f32;
        let mut r = 0f32;
        for &iv in &spec.chords[section] {
            let f = semis(spec.root * 4.0, iv);
            l += 2.0 * ((f * 1.003 * t).fract()) - 1.0;
            r += 2.0 * ((f * 0.997 * t).fract()) - 1.0;
        }
        let cutoff = 0.03 + 0.02 * (2.0 * PI * t / 8.0).sin();
        lpl += cutoff * (l - lpl);
        lpr += cutoff * (r - lpr);
        add(&mut buf, i, lpl * 0.06, lpr * 0.06);
    }
    let peak = buf.iter().fold(0f32, |m, v| m.max(v.abs()));
    let g = 0.7 / peak.max(1e-6);
    buf.iter_mut().for_each(|v| *v *= g);
    buf
}

pub fn write_wav(path: &std::path::Path, stereo: &[f32]) -> anyhow::Result<()> {
    let mut w = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        },
    )?;
    for &s in stereo {
        w.write_sample((s.clamp(-1.0, 1.0) * 8_388_607.0) as i32)?;
    }
    w.finalize()?;
    Ok(())
}

pub fn dj_a() -> TrackSpec {
    TrackSpec {
        seed: 1,
        bpm: 125.0,
        bars: 32,
        root: 55.0,
        chords: [
            [0.0, 3.0, 7.0],
            [-4.0, 0.0, 3.0],
            [-2.0, 2.0, 5.0],
            [-5.0, -2.0, 2.0],
        ],
        arp: false,
    }
}

pub fn dj_b() -> TrackSpec {
    TrackSpec {
        seed: 2,
        bpm: 125.0,
        bars: 32,
        root: 49.0,
        chords: [
            [0.0, 4.0, 7.0],
            [5.0, 9.0, 12.0],
            [-3.0, 0.0, 4.0],
            [7.0, 11.0, 14.0],
        ],
        arp: true,
    }
}

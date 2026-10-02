//! A one-track player standing in for a DJ deck when there is no DJ gear:
//! play/pause, pitch, nudge, and SYNC (match tempo and beat phase to the partner
//! as heard). The beat grid comes from the file's own audio.

use obsidian_align::{beat_phase, estimate_period, onset_envelope};
use serde::Serialize;
use std::sync::Arc;

pub const HOP: usize = 48; // 1 ms onset hop, same as the engine's beat tools
pub const RATE: f64 = 48_000.0;
const XFADE: usize = 240;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct BeatGrid {
    /// Beat period in samples at the file's own speed.
    pub period: f64,
    /// Position (samples) of a beat, as the onset tracker sees it.
    pub offset: f64,
    pub bpm: f64,
}

/// Autocorrelation peak near `m` beats, for a period accurate to ~1/m of a hop.
pub fn refine_period(env: &[f32], p0: f64, m: usize) -> f64 {
    let centre = p0 * m as f64;
    let (lo, hi) = (
        (centre - m as f64 * 2.0).floor() as usize,
        (centre + m as f64 * 2.0).ceil() as usize,
    );
    if lo < 3 || env.len() < hi * 2 {
        return p0;
    }
    let mean = env.iter().sum::<f32>() / env.len() as f32;
    let ac = |lag: usize| -> f64 {
        env.iter()
            .zip(&env[lag..])
            .map(|(a, b)| ((a - mean) * (b - mean)) as f64)
            .sum::<f64>()
            / (env.len() - lag) as f64
    };
    let vals: Vec<f64> = (lo..=hi).map(ac).collect();
    let Some((i, _)) = vals
        .iter()
        .enumerate()
        .skip(1)
        .take(vals.len().saturating_sub(2))
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
    else {
        return p0;
    };
    let (a, b, c) = (vals[i - 1], vals[i], vals[i + 1]);
    let d = a - 2.0 * b + c;
    let off = if d.abs() > 1e-12 {
        0.5 * (a - c) / d
    } else {
        0.0
    };
    let p = (lo + i) as f64 + off.clamp(-0.5, 0.5);
    let r = p / m as f64;
    if (r - p0).abs() < 1.0 {
        r
    } else {
        p0
    }
}

/// Tempo and beat positions of a track, from its first 90 s.
pub fn analyze(program: &[f32]) -> Option<BeatGrid> {
    let n = (program.len() / 2).min(90 * 48_000);
    let env = onset_envelope(&program[..n * 2], HOP);
    let hop_s = HOP as f64 / RATE;
    let p0 = estimate_period(&env, hop_s, 70.0, 180.0)?;
    let p = refine_period(&env, p0, 16);
    let ph = beat_phase(&env, p)?;
    Some(BeatGrid {
        period: p * HOP as f64,
        offset: ph * HOP as f64,
        bpm: 60.0 / (p * hop_s),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct DeckStatus {
    pub title: String,
    pub playing: bool,
    pub track_bpm: Option<f64>,
    pub bpm: Option<f64>,
    pub pitch_pct: f64,
    pub sync: bool,
    pub sync_err_ms: Option<f64>,
    pub pos_s: f64,
    pub len_s: f64,
    pub gain: f32,
}

#[derive(Clone)]
pub struct Deck {
    pub title: String,
    program: Arc<Vec<f32>>,
    frames: usize,
    pub grid: Option<BeatGrid>,
    pub pos: f64,
    pub playing: bool,
    /// Playback speed (1.0 = the file's own tempo). Pitch changes with it, like vinyl.
    pub pitch: f64,
    bend: f64,
    bend_left: usize,
    /// Automatic gain (the ghost's fades); the DJ's fader is applied outside.
    pub gain: f32,
    ramp: Option<(f32, f32)>,
    pub sync: bool,
    /// Sync has matched tempo and phase at least once since it was switched on.
    pub sync_locked: bool,
    pub sync_err_ms: Option<f64>,
    xfade_from: Option<(f64, usize)>,
}

impl Deck {
    pub fn new(title: String, program: Arc<Vec<f32>>) -> Self {
        let grid = analyze(&program);
        let frames = program.len() / 2;
        let mut d = Deck {
            title,
            program,
            frames,
            grid,
            pos: 0.0,
            playing: false,
            pitch: 1.0,
            bend: 0.0,
            bend_left: 0,
            gain: 1.0,
            ramp: None,
            sync: false,
            sync_locked: false,
            sync_err_ms: None,
            xfade_from: None,
        };
        d.cue();
        d
    }

    /// Back to the first beat of the track.
    pub fn cue(&mut self) {
        self.pos = self
            .grid
            .map(|g| g.offset.rem_euclid(g.period))
            .unwrap_or(0.0);
        self.xfade_from = None;
    }

    fn sample(&self, p: f64, ch: usize) -> f32 {
        let n = self.frames as i64;
        if n == 0 {
            return 0.0;
        }
        let i = p.floor() as i64;
        let t = (p - i as f64) as f32;
        let g = |k: i64| self.program[(k.rem_euclid(n) as usize) * 2 + ch];
        let (y0, y1, y2, y3) = (g(i - 1), g(i), g(i + 1), g(i + 2));
        let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
        let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c = -0.5 * y0 + 0.5 * y2;
        ((a * t + b) * t + c) * t + y1
    }

    pub fn render(&mut self, n: usize, out: &mut Vec<f32>) {
        out.clear();
        for _ in 0..n {
            if !self.playing {
                out.push(0.0);
                out.push(0.0);
                continue;
            }
            if let Some((target, step)) = self.ramp {
                self.gain += step;
                if (step >= 0.0 && self.gain >= target) || (step < 0.0 && self.gain <= target) {
                    self.gain = target;
                    self.ramp = None;
                }
            }
            let (mut l, mut r) = (self.sample(self.pos, 0), self.sample(self.pos, 1));
            if let Some((old, left)) = self.xfade_from {
                let w = left as f32 / XFADE as f32;
                l = l * (1.0 - w) + self.sample(old, 0) * w;
                r = r * (1.0 - w) + self.sample(old, 1) * w;
                self.xfade_from = if left > 1 {
                    Some((old + self.pitch, left - 1))
                } else {
                    None
                };
            }
            out.push(l * self.gain);
            out.push(r * self.gain);
            let step = if self.bend_left > 0 {
                self.bend_left -= 1;
                self.pitch * (1.0 + self.bend)
            } else {
                self.pitch
            };
            self.pos += step;
            if self.pos >= self.frames as f64 {
                self.pos -= self.frames as f64;
            }
        }
    }

    /// Move the play head by `samples` with a short crossfade (a sync jump).
    pub fn jump(&mut self, samples: f64) {
        let old = self.pos;
        self.pos = (self.pos + samples).rem_euclid(self.frames.max(1) as f64);
        if self.playing && self.gain > 0.0 {
            self.xfade_from = Some((old, XFADE));
        }
    }

    /// Shift the beat by `ms` (positive = earlier/ahead) over `over_s` seconds,
    /// like pushing a platter.
    pub fn nudge(&mut self, ms: f64, over_s: f64) {
        let n = (over_s * RATE).max(1.0);
        self.bend = ms / 1e3 / over_s;
        self.bend_left = n as usize;
    }

    pub fn ramp_gain(&mut self, target: f32, secs: f64) {
        let n = (secs * RATE).max(1.0) as f32;
        let step = (target - self.gain) / n;
        if step == 0.0 {
            self.ramp = None;
        } else {
            self.ramp = Some((target, step));
        }
    }

    pub fn ramping(&self) -> bool {
        self.ramp.is_some()
    }

    /// Seconds of output until this deck's next beat (from the next sample it renders).
    pub fn time_to_next_beat(&self) -> Option<f64> {
        let g = self.grid?;
        let phase = ((self.pos - g.offset) / g.period).rem_euclid(1.0);
        let rem = if phase < 1e-9 { 0.0 } else { 1.0 - phase };
        Some(rem * g.period / (self.pitch * RATE))
    }

    pub fn status(&self) -> DeckStatus {
        DeckStatus {
            title: self.title.clone(),
            playing: self.playing,
            track_bpm: self.grid.map(|g| g.bpm),
            bpm: self.grid.map(|g| g.bpm * self.pitch),
            pitch_pct: (self.pitch - 1.0) * 100.0,
            sync: self.sync,
            sync_err_ms: self.sync_err_ms,
            pos_s: self.pos / RATE,
            len_s: self.frames as f64 / RATE,
            gain: self.gain,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_test_track_tempo_and_beats() {
        let t = obsidian_testaudio::track(&obsidian_testaudio::dj_a());
        let g = analyze(&t).expect("grid");
        assert!((g.bpm - 125.0).abs() < 0.05, "bpm {}", g.bpm);
        // Kicks are at multiples of the period from sample 0 (plus the onset filter's lag).
        let lag_ms = g.offset.rem_euclid(g.period) / 48.0;
        assert!(lag_ms < 15.0, "phase {lag_ms} ms");
    }
}

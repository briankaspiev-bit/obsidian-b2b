//! Beat-quantized monitoring (report §D.3).
//!
//! The leader hears the follower one round trip late. We add just enough
//! extra delay that the follower lands exactly on the leader's next beat.
//! No BPM metadata needed: we compare onset envelopes of the local audio and
//! the remote monitor, estimate the beat period from the local audio, and find
//! the phase lag of the remote within one beat.

/// Onset envelope: low-band (kick-heavy) energy flux, one value per `hop` samples.
/// Input is interleaved stereo at 48 kHz.
pub fn onset_envelope(stereo: &[f32], hop: usize) -> Vec<f32> {
    // Two cascaded one-pole low-passes at ~150 Hz.
    let a = (-2.0 * std::f32::consts::PI * 150.0 / 48_000.0).exp();
    let (mut s1, mut s2) = (0f32, 0f32);
    let frames = stereo.len() / 2;
    let mut env = Vec::with_capacity(frames / hop + 1);
    let mut prev = 0f32;
    let mut acc = 0f32;
    for i in 0..frames {
        let m = 0.5 * (stereo[2 * i] + stereo[2 * i + 1]);
        s1 = a * s1 + (1.0 - a) * m;
        s2 = a * s2 + (1.0 - a) * s1;
        acc += s2 * s2;
        if (i + 1) % hop == 0 {
            let e = (acc / hop as f32 + 1e-9).ln();
            env.push((e - prev).max(0.0));
            prev = e;
            acc = 0.0;
        }
    }
    env
}

fn parabolic(ym1: f32, y0: f32, yp1: f32) -> f64 {
    let d = ym1 - 2.0 * y0 + yp1;
    if d.abs() < 1e-12 {
        0.0
    } else {
        (0.5 * (ym1 - yp1) / d).clamp(-0.5, 0.5) as f64
    }
}

/// Beat period in hops from autocorrelation, searching `bpm_lo..bpm_hi`.
pub fn estimate_period(env: &[f32], hop_s: f64, bpm_lo: f64, bpm_hi: f64) -> Option<f64> {
    let lo = (60.0 / bpm_hi / hop_s).floor() as usize;
    let hi = (60.0 / bpm_lo / hop_s).ceil() as usize;
    if env.len() < hi * 4 || lo < 2 {
        return None;
    }
    let mean = env.iter().sum::<f32>() / env.len() as f32;
    let e: Vec<f32> = env.iter().map(|v| v - mean).collect();
    let ac = |lag: usize| -> f32 {
        e.iter().zip(&e[lag..]).map(|(a, b)| a * b).sum::<f32>() / (e.len() - lag) as f32
    };
    let vals: Vec<f32> = (lo - 1..=hi + 1).map(ac).collect();
    let (best_i, _) = vals[1..vals.len() - 1]
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())?;
    let i = best_i + 1;
    Some((lo - 1 + i) as f64 + parabolic(vals[i - 1], vals[i], vals[i + 1]))
}

/// How far (in hops, within [0, period)) the remote envelope lags the local one.
pub fn phase_lag(local: &[f32], remote: &[f32], period: f64) -> Option<(f64, f32)> {
    let n = local.len().min(remote.len());
    let p = period.round() as usize;
    if n < p * 4 || p < 3 {
        return None;
    }
    let ml = local[..n].iter().sum::<f32>() / n as f32;
    let mr = remote[..n].iter().sum::<f32>() / n as f32;
    let score = |lag: usize| -> f32 {
        let mut s = 0f32;
        for i in 0..n - lag {
            s += (local[i] - ml) * (remote[i + lag] - mr);
        }
        s / (n - lag) as f32
    };
    let scores: Vec<f32> = (0..p).map(score).collect();
    let (best, &peak) = scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())?;
    let ym1 = scores[(best + p - 1) % p];
    let yp1 = scores[(best + 1) % p];
    let lag = (best as f64 + parabolic(ym1, peak, yp1)).rem_euclid(period);
    // Confidence: peak vs. mean score.
    let mean = scores.iter().sum::<f32>() / p as f32;
    let spread = scores.iter().map(|s| (s - mean).abs()).sum::<f32>() / p as f32 + 1e-12;
    Some((lag, (peak - mean) / spread))
}

/// Extra delay that moves a remote lagging by `lag` onto the next grid line of `unit`.
pub fn quantize_extra(lag: f64, unit: f64) -> f64 {
    let r = lag.rem_euclid(unit);
    if r < 1e-9 {
        0.0
    } else {
        unit - r
    }
}

/// Theoretical helper: total monitor delay for a known round trip and tempo.
pub fn quantized_monitor_delay_ms(round_trip_ms: f64, bpm: f64, beats: f64) -> f64 {
    let unit = 60_000.0 / bpm * beats;
    round_trip_ms + quantize_extra(round_trip_ms, unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kicks(secs: f64, bpm: f64, offset_s: f64) -> Vec<f32> {
        let n = (secs * 48_000.0) as usize;
        let mut v = vec![0f32; n * 2];
        let beat = 60.0 / bpm;
        let mut t = offset_s;
        while t < secs {
            let s0 = (t * 48_000.0) as usize;
            for i in 0..9_600.min(n.saturating_sub(s0)) {
                let tt = i as f32 / 48_000.0;
                let f = 50.0 + 100.0 * (-tt * 30.0).exp();
                let x = (2.0 * std::f32::consts::PI * f * tt).sin() * (-tt * 12.0).exp() * 0.8;
                v[(s0 + i) * 2] = x;
                v[(s0 + i) * 2 + 1] = x;
            }
            t += beat;
        }
        v
    }

    #[test]
    fn finds_tempo_and_phase() {
        let hop = 48; // 1 ms
        let local = onset_envelope(&kicks(12.0, 125.0, 0.0), hop);
        let remote = onset_envelope(&kicks(12.0, 125.0, 0.083), hop);
        let period = estimate_period(&local, 0.001, 70.0, 180.0).unwrap();
        assert!((period - 480.0).abs() < 1.5, "period {period}");
        let (lag, conf) = phase_lag(&local, &remote, period).unwrap();
        assert!((lag - 83.0).abs() < 1.5, "lag {lag}");
        assert!(conf > 2.0);
        let extra = quantize_extra(lag, period);
        assert!((extra - 397.0).abs() < 2.0);
    }

    #[test]
    fn theory() {
        assert!((quantized_monitor_delay_ms(80.0, 125.0, 1.0) - 480.0).abs() < 1e-9);
        assert!((quantized_monitor_delay_ms(80.0, 125.0, 4.0) - 1920.0).abs() < 1e-9);
    }
}

/// Streaming version of [`onset_envelope`] (same output, fed block by block).
pub struct OnsetTracker {
    hop: usize,
    a: f32,
    s1: f32,
    s2: f32,
    prev: f32,
    acc: f32,
    count: usize,
    pub env: Vec<f32>,
}

impl OnsetTracker {
    pub fn new(hop: usize) -> Self {
        let a = (-2.0 * std::f32::consts::PI * 150.0 / 48_000.0).exp();
        OnsetTracker {
            hop,
            a,
            s1: 0.0,
            s2: 0.0,
            prev: 0.0,
            acc: 0.0,
            count: 0,
            env: Vec::new(),
        }
    }

    pub fn push(&mut self, stereo: &[f32]) {
        for fr in stereo.chunks_exact(2) {
            let m = 0.5 * (fr[0] + fr[1]);
            self.s1 = self.a * self.s1 + (1.0 - self.a) * m;
            self.s2 = self.a * self.s2 + (1.0 - self.a) * self.s1;
            self.acc += self.s2 * self.s2;
            self.count += 1;
            if self.count == self.hop {
                let e = (self.acc / self.hop as f32 + 1e-9).ln();
                self.env.push((e - self.prev).max(0.0));
                self.prev = e;
                self.acc = 0.0;
                self.count = 0;
            }
        }
    }
}

/// Phase (in hops, within [0, period)) of the beats in `env`, relative to index 0.
pub fn beat_phase(env: &[f32], period: f64) -> Option<f64> {
    let p = period.round() as usize;
    if env.len() < p * 3 || p < 3 {
        return None;
    }
    // Fold the envelope onto one period and take the peak.
    let mut fold = vec![0f32; p];
    for (i, v) in env.iter().enumerate() {
        let k = ((i as f64).rem_euclid(period)).floor() as usize % p;
        fold[k] += v;
    }
    let (best, &peak) = fold
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())?;
    let off = parabolic(fold[(best + p - 1) % p], peak, fold[(best + 1) % p]);
    Some((best as f64 + off).rem_euclid(period))
}

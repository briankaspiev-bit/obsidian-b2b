//! Correlation, envelopes, interpolation and line fitting.

use rustfft::{FftPlanner, num_complex::Complex32};
use serde::{Deserialize, Serialize};

/// Cross-correlation of a short `x` against a longer `y`:
/// `r[k] = sum_i x[i] * y[i + k]` for `k` in `0..=y.len() - x.len()`.
/// With `phat`, the cross-spectrum is whitened (GCC-PHAT), which gives a sharp
/// peak that is robust to codec colouring and level changes.
pub fn xcorr(x: &[f32], y: &[f32], phat: bool) -> Vec<f32> {
    assert!(y.len() >= x.len());
    let n = (x.len() + y.len()).next_power_of_two();
    let mut planner = FftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut xb: Vec<Complex32> = (0..n).map(|i| Complex32::new(if i < x.len() { x[i] } else { 0.0 }, 0.0)).collect();
    let mut yb: Vec<Complex32> = (0..n).map(|i| Complex32::new(if i < y.len() { y[i] } else { 0.0 }, 0.0)).collect();
    fwd.process(&mut xb);
    fwd.process(&mut yb);
    for i in 0..n {
        let mut r = xb[i].conj() * yb[i];
        if phat {
            let m = r.norm();
            r = if m > 1e-20 { r / m } else { Complex32::new(0.0, 0.0) };
        }
        yb[i] = r;
    }
    inv.process(&mut yb);
    let count = y.len() - x.len() + 1;
    (0..count).map(|k| yb[k].re / n as f32).collect()
}

/// Integer argmax refined with a parabola. Returns (fractional index, peak-to-rms ratio).
pub fn peak(r: &[f32]) -> (f64, f64) {
    let (mut k, mut best) = (0usize, f32::MIN);
    for (i, &v) in r.iter().enumerate() {
        if v > best {
            best = v;
            k = i;
        }
    }
    let mut pos = k as f64;
    if k > 0 && k + 1 < r.len() {
        let (a, b, c) = (r[k - 1] as f64, r[k] as f64, r[k + 1] as f64);
        let den = a - 2.0 * b + c;
        if den.abs() > 1e-20 {
            pos += (0.5 * (a - c) / den).clamp(-0.5, 0.5);
        }
    }
    let rms = (r.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / r.len() as f64).sqrt();
    (pos, if rms > 0.0 { best as f64 / rms } else { 0.0 })
}

pub fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()) as f32)
        .collect()
}

/// RMS level in dBFS, one value per `hop` samples over a `2 * hop` window.
pub fn env_db(x: &[f32], hop: usize) -> Vec<f32> {
    let frames = x.len() / hop;
    (0..frames)
        .map(|f| {
            let a = f * hop;
            let b = (a + 2 * hop).min(x.len());
            let e = x[a..b].iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / (b - a) as f64;
            (10.0 * (e + 1e-14).log10()) as f32
        })
        .collect()
}

pub fn rms_db(x: &[f32]) -> f32 {
    if x.is_empty() {
        return -140.0;
    }
    let e = x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len() as f64;
    (10.0 * (e + 1e-14).log10()) as f32
}

/// Onset-strength envelope at `sr / hop`: positive change of log energy, so it is
/// level-independent and peaks on transients (kicks, claps, hats).
pub fn onset_env(x: &[f32], hop: usize) -> Vec<f32> {
    let win = 4 * hop;
    let frames = x.len().saturating_sub(win) / hop;
    let mut le = Vec::with_capacity(frames);
    for f in 0..frames {
        let s = &x[f * hop..f * hop + win];
        let e = s.iter().map(|&v| v * v).sum::<f32>() / win as f32;
        le.push((e + 1e-7).ln());
    }
    let mut o = vec![0.0f32; frames];
    for i in 1..frames {
        o[i] = (le[i] - le[i - 1]).max(0.0);
    }
    o
}

/// Normalized cross-correlation of `x` against `y` at lags `0..=y.len()-x.len()` (direct form).
pub fn ncc_direct(x: &[f32], y: &[f32]) -> Vec<f32> {
    let mx = x.iter().sum::<f32>() / x.len() as f32;
    let xs: Vec<f32> = x.iter().map(|v| v - mx).collect();
    let nx = xs.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    (0..=y.len() - x.len())
        .map(|k| {
            let seg = &y[k..k + x.len()];
            let my = seg.iter().sum::<f32>() / seg.len() as f32;
            let mut dot = 0.0f32;
            let mut ny = 0.0f32;
            for (a, b) in xs.iter().zip(seg) {
                let bb = b - my;
                dot += a * bb;
                ny += bb * bb;
            }
            dot / (nx * ny.sqrt().max(1e-12))
        })
        .collect()
}

/// Kaiser-windowed sinc interpolator for fractional read positions.
pub struct Sinc {
    half: usize,
    phases: usize,
    table: Vec<f32>,
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..50 {
        term *= (x / (2.0 * k as f64)).powi(2);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

impl Sinc {
    pub fn new() -> Self {
        let (half, phases, beta) = (32usize, 512usize, 9.0f64);
        let taps = 2 * half;
        let mut table = Vec::with_capacity((phases + 1) * taps);
        let i0b = bessel_i0(beta);
        for p in 0..=phases {
            let frac = p as f64 / phases as f64;
            for j in 0..taps {
                let o = j as f64 - (half as f64 - 1.0);
                let t = frac - o;
                let s = if t.abs() < 1e-12 {
                    1.0
                } else {
                    (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t)
                };
                let z = t / half as f64;
                let w = if z.abs() >= 1.0 {
                    0.0
                } else {
                    bessel_i0(beta * (1.0 - z * z).sqrt()) / i0b
                };
                table.push((s * w) as f32);
            }
        }
        Sinc { half, phases, table }
    }

    #[inline]
    pub fn at(&self, x: &[f32], pos: f64) -> f32 {
        let fl = pos.floor();
        let i = fl as i64;
        let frac = pos - fl;
        if frac < 1e-9 {
            return if i >= 0 && (i as usize) < x.len() { x[i as usize] } else { 0.0 };
        }
        let taps = 2 * self.half;
        let pf = frac * self.phases as f64;
        let p0 = (pf.floor() as usize).min(self.phases - 1);
        let w = (pf - p0 as f64) as f32;
        let t0 = &self.table[p0 * taps..(p0 + 1) * taps];
        let t1 = &self.table[(p0 + 1) * taps..(p0 + 2) * taps];
        let base = i - (self.half as i64 - 1);
        let mut acc = 0.0f32;
        for j in 0..taps {
            let idx = base + j as i64;
            if idx >= 0 && (idx as usize) < x.len() {
                acc += x[idx as usize] * (t0[j] + w * (t1[j] - t0[j]));
            }
        }
        acc
    }
}

/// `y = y0 + slope * (x - x0)`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Line {
    pub x0: f64,
    pub y0: f64,
    pub slope: f64,
}

impl Line {
    pub fn at(&self, x: f64) -> f64 {
        self.y0 + self.slope * (x - self.x0)
    }
    pub fn inv(&self, y: f64) -> f64 {
        self.x0 + (y - self.y0) / self.slope
    }
}

pub struct Fit {
    pub line: Line,
    pub inliers: usize,
    pub rms: f64,
    pub max_abs: f64,
}

/// Least-squares line with iterative outlier rejection. With `fixed_slope`, only the
/// offset is fitted (median of residuals). `tol` is the minimum rejection threshold.
pub fn robust_line(pts: &[(f64, f64)], fixed_slope: Option<f64>, tol: f64) -> Option<Fit> {
    let mut keep: Vec<bool> = vec![true; pts.len()];
    let mut line = Line {
        x0: 0.0,
        y0: 0.0,
        slope: 1.0,
    };
    for _ in 0..8 {
        let sel: Vec<&(f64, f64)> = pts.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| p).collect();
        if sel.len() < 2 && !(fixed_slope.is_some() && sel.len() == 1) {
            return None;
        }
        let x0 = sel.iter().map(|p| p.0).sum::<f64>() / sel.len() as f64;
        line = match fixed_slope {
            Some(s) => {
                let mut r: Vec<f64> = sel.iter().map(|p| p.1 - s * (p.0 - x0)).collect();
                r.sort_by(|a, b| a.partial_cmp(b).unwrap());
                Line {
                    x0,
                    y0: r[r.len() / 2],
                    slope: s,
                }
            }
            None => {
                let y0 = sel.iter().map(|p| p.1).sum::<f64>() / sel.len() as f64;
                let sxx: f64 = sel.iter().map(|p| (p.0 - x0).powi(2)).sum();
                let sxy: f64 = sel.iter().map(|p| (p.0 - x0) * (p.1 - y0)).sum();
                Line {
                    x0,
                    y0,
                    slope: if sxx > 0.0 { sxy / sxx } else { 1.0 },
                }
            }
        };
        let res: Vec<f64> = pts.iter().map(|p| p.1 - line.at(p.0)).collect();
        let mut abs: Vec<f64> = res.iter().zip(&keep).filter(|(_, k)| **k).map(|(r, _)| r.abs()).collect();
        abs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mad = abs[abs.len() / 2];
        let thr = (4.0 * 1.4826 * mad).max(tol);
        let new: Vec<bool> = res.iter().map(|r| r.abs() <= thr).collect();
        if new == keep {
            break;
        }
        keep = new;
    }
    let inl: Vec<f64> = pts
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(p, _)| p.1 - line.at(p.0))
        .collect();
    if inl.is_empty() {
        return None;
    }
    let rms = (inl.iter().map(|r| r * r).sum::<f64>() / inl.len() as f64).sqrt();
    let max_abs = inl.iter().fold(0.0f64, |m, r| m.max(r.abs()));
    Some(Fit {
        line,
        inliers: inl.len(),
        rms,
        max_abs,
    })
}

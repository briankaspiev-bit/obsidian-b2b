//! FIFOs and the level-controlled resampler that bridges two audio clocks.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Interleaved stereo FIFO shared between an audio callback and an engine thread.
/// A plain mutex: the critical sections are a few memcpys.
pub struct AudioFifo {
    q: Mutex<VecDeque<f32>>,
    cap_frames: usize,
    pub overflows: AtomicU64,
}

impl AudioFifo {
    pub fn new(cap_frames: usize) -> Self {
        AudioFifo {
            q: Mutex::new(VecDeque::with_capacity(cap_frames * 2)),
            cap_frames,
            overflows: AtomicU64::new(0),
        }
    }

    pub fn push(&self, stereo: &[f32]) {
        let mut q = self.q.lock().unwrap();
        q.extend(stereo.iter().copied());
        let over = q.len().saturating_sub(self.cap_frames * 2);
        if over > 0 {
            q.drain(..over);
            self.overflows.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn len_frames(&self) -> usize {
        self.q.lock().unwrap().len() / 2
    }

    fn pop_into(&self, max_frames: usize, out: &mut VecDeque<f32>) -> usize {
        let mut q = self.q.lock().unwrap();
        let n = (q.len() / 2).min(max_frames);
        out.extend(q.drain(..n * 2));
        n
    }

    fn drop_frames(&self, n: usize) {
        let mut q = self.q.lock().unwrap();
        let n = (n * 2).min(q.len());
        q.drain(..n);
    }

    pub fn clear(&self) {
        self.q.lock().unwrap().clear();
    }
}

/// Pulls from a FIFO at `in_rate`, produces frames at `out_rate`, and nudges the
/// ratio (±0.2% max) so the FIFO level stays at `target_frames`.
pub struct AdaptiveResampler {
    nominal: f64,
    target: f64,
    buf: VecDeque<f32>,
    pos: f64,
    integ: f64,
    level_avg: Option<f64>,
    primed: bool,
    pub underruns: u64,
    pub ratio_ppm: f64,
    /// Set when the target was raised to fit the device's callback size.
    pub grown_to: Option<usize>,
}

impl AdaptiveResampler {
    pub fn new(in_rate: u32, out_rate: u32, target_frames: usize) -> Self {
        AdaptiveResampler {
            nominal: in_rate as f64 / out_rate as f64,
            target: target_frames as f64,
            buf: VecDeque::new(),
            pos: 1.0,
            integ: 0.0,
            level_avg: None,
            primed: false,
            underruns: 0,
            ratio_ppm: 0.0,
            grown_to: None,
        }
    }

    fn at(&self, i: usize, ch: usize) -> f32 {
        self.buf.get(i * 2 + ch).copied().unwrap_or(0.0)
    }

    /// Fill `out` (interleaved stereo, `out.len()/2` frames).
    pub fn pull(&mut self, fifo: &AudioFifo, out: &mut [f32]) {
        let frames = out.len() / 2;
        // The FIFO must hold more than one device callback plus one engine block, or
        // every big callback underruns and re-primes (silence forever). Sound cards
        // pick their own callback size, so grow the target to fit.
        let floor = frames as f64 * self.nominal * 1.5 + 480.0;
        if self.target < floor {
            self.target = floor;
            self.grown_to = Some(floor as usize);
        }
        let level = fifo.len_frames() as f64 + self.buf.len() as f64 / 2.0 - self.pos;
        // Wait for the FIFO to reach its target before starting (or after an underrun).
        if !self.primed {
            if level < self.target {
                out.iter_mut().for_each(|s| *s = 0.0);
                return;
            }
            self.primed = true;
            self.level_avg = Some(level);
        }
        // Way over target (a source that paused and burst back): skip to target.
        if level > self.target * 3.0 + 4_800.0 {
            fifo.drop_frames((level - self.target) as usize);
            self.level_avg = None;
        }
        let avg = self
            .level_avg
            .map(|a| 0.98 * a + 0.02 * level)
            .unwrap_or(level);
        self.level_avg = Some(avg);
        let err_s = (avg - self.target) / 48_000.0;
        self.integ = (self.integ + err_s * frames as f64 / 48_000.0).clamp(-0.05, 0.05);
        let corr = (0.02 * err_s + 0.002 * self.integ).clamp(-0.002, 0.002);
        self.ratio_ppm = corr * 1e6;
        let step = self.nominal * (1.0 + corr);

        let need = (self.pos + step * frames as f64).ceil() as usize + 3;
        let have = self.buf.len() / 2;
        if have < need {
            fifo.pop_into(need - have, &mut self.buf);
        }
        if self.buf.len() / 2 < need {
            // Underrun: emit silence, re-prime.
            self.underruns += 1;
            self.primed = false;
            out.iter_mut().for_each(|s| *s = 0.0);
            return;
        }
        for k in 0..frames {
            let p = self.pos + step * k as f64;
            let i = p.floor() as usize;
            let t = (p - i as f64) as f32;
            for ch in 0..2 {
                let y0 = self.at(i.saturating_sub(1), ch);
                let y1 = self.at(i, ch);
                let y2 = self.at(i + 1, ch);
                let y3 = self.at(i + 2, ch);
                let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
                let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
                let c = -0.5 * y0 + 0.5 * y2;
                out[k * 2 + ch] = ((a * t + b) * t + c) * t + y1;
            }
        }
        self.pos += step * frames as f64;
        // Keep one frame of history behind the read position.
        let consumed = (self.pos.floor() as usize).saturating_sub(1);
        self.buf.drain(..consumed * 2);
        self.pos -= consumed as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_a_drifting_44k1_device() {
        // Producer: 48 kHz at +150 ppm; consumer: 44.1 kHz device, 441-frame callbacks.
        let fifo = AudioFifo::new(48_000);
        let mut rs = AdaptiveResampler::new(48_000, 44_100, 960);
        let mut produced = 0f64;
        let mut out = vec![0f32; 441 * 2];
        let mut levels = Vec::new();
        for cb in 0..60_000 {
            // 10 ms of device time per callback; producer adds 48000*1.00015*0.01 frames.
            let want = ((cb + 1) as f64 * 480.0 * 1.000_15).floor() - produced;
            let n = want as usize;
            fifo.push(&vec![0.1f32; n * 2]);
            produced += n as f64;
            rs.pull(&fifo, &mut out);
            if cb > 30_000 {
                levels.push(fifo.len_frames());
            }
        }
        let lo = *levels.iter().min().unwrap() as f64;
        let hi = *levels.iter().max().unwrap() as f64;
        assert!(lo > 400.0 && hi < 1_600.0, "level {lo}..{hi}");
        assert!((rs.ratio_ppm - 150.0).abs() < 30.0, "ppm {}", rs.ratio_ppm);
        assert!(rs.underruns <= 1);
    }

    #[test]
    fn plays_through_callbacks_bigger_than_the_target() {
        // A Windows shared-mode device asking for 1056 frames per callback against a
        // 720-frame target used to underrun on every callback and stay silent.
        let fifo = AudioFifo::new(48_000);
        let mut rs = AdaptiveResampler::new(48_000, 48_000, 720);
        let mut out = vec![0f32; 1056 * 2];
        let mut produced = 0usize;
        let mut loud = 0;
        for cb in 0..2_000 {
            // Engine pushes 240-frame blocks at 48 kHz; 1056 frames = 22 ms per callback.
            let due = (cb + 1) * 1056;
            while produced + 240 <= due {
                fifo.push(&vec![0.5f32; 480]);
                produced += 240;
            }
            rs.pull(&fifo, &mut out);
            if cb > 100 && out.iter().all(|&v| (v - 0.5).abs() < 1e-3) {
                loud += 1;
            }
        }
        assert!(
            loud > 1_850,
            "only {loud} of 1899 callbacks had audio, {} underruns",
            rs.underruns
        );
    }
}

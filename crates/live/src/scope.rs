//! What a DJ screen draws: a waveform column per 5 ms block for both decks as this
//! DJ hears them, and where each deck's beats fall, all on the session's output
//! clock (milliseconds since the session started; column `i` is heard at `i * 5` ms).

use serde::Serialize;
use std::collections::VecDeque;

/// Columns kept for screens that poll late (60 s).
const KEEP: usize = 12_000;

/// Low / mid / high peaks of one 5 ms block, 0..255 on a square-root scale.
pub type Bands = [u8; 3];

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Column {
    pub you: Bands,
    pub partner: Bands,
}

/// A deck's beats on the output clock: `beat_ms + k * period_ms` for any whole `k`.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct BeatClock {
    pub period_ms: f64,
    pub beat_ms: f64,
    /// A beat that starts a bar (beat 1), when known.
    pub bar_ms: Option<f64>,
}

impl BeatClock {
    pub fn bpm(&self) -> f64 {
        60_000.0 / self.period_ms
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScopeChunk {
    /// Index of `cols[0]`; the next poll asks for `first + cols.len()`.
    pub first: u64,
    pub cols: Vec<Column>,
}

#[derive(Default)]
pub struct Scope {
    first: u64,
    cols: VecDeque<Column>,
}

impl Scope {
    pub fn push(&mut self, c: Column) {
        self.cols.push_back(c);
        while self.cols.len() > KEEP {
            self.cols.pop_front();
            self.first += 1;
        }
    }

    /// Columns from index `from` on (or the oldest kept, if `from` fell out).
    pub fn since(&self, from: u64) -> ScopeChunk {
        let first = from.max(self.first);
        let skip = (first - self.first) as usize;
        ScopeChunk {
            first,
            cols: self.cols.iter().skip(skip).copied().collect(),
        }
    }
}

/// Splits a stereo block into low (< 200 Hz), mid and high (> 2.5 kHz) peaks.
#[derive(Default)]
pub struct BandSplit {
    lo: f32,
    mid: f32,
}

impl BandSplit {
    pub fn column(&mut self, stereo: &[f32]) -> Bands {
        let a_lo = 1.0 - (-2.0 * std::f32::consts::PI * 200.0 / 48_000.0).exp();
        let a_mid = 1.0 - (-2.0 * std::f32::consts::PI * 2_500.0 / 48_000.0).exp();
        let mut pk = [0f32; 3];
        for fr in stereo.chunks_exact(2) {
            let m = 0.5 * (fr[0] + fr[1]);
            self.lo += a_lo * (m - self.lo);
            self.mid += a_mid * (m - self.mid);
            let bands = [self.lo, self.mid - self.lo, m - self.mid];
            for (p, b) in pk.iter_mut().zip(bands) {
                *p = p.max(b.abs());
            }
        }
        pk.map(|p| (p.min(1.0).sqrt() * 255.0).round() as u8)
    }
}

/// Peaks of a whole track, one column per 5 ms of the track at its own speed
/// (what a deck shows ahead of the play head).
pub fn track_wave(program: &[f32]) -> Vec<Bands> {
    let mut s = BandSplit::default();
    program.chunks(240 * 2).map(|b| s.column(b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_newest_columns_and_resumes_where_asked() {
        let mut s = Scope::default();
        for i in 0..(KEEP + 10) {
            s.push(Column {
                you: [(i % 256) as u8, 0, 0],
                partner: [0; 3],
            });
        }
        let all = s.since(0);
        assert_eq!(all.first, 10);
        assert_eq!(all.cols.len(), KEEP);
        let tail = s.since(KEEP as u64 + 5);
        assert_eq!(tail.first, KEEP as u64 + 5);
        assert_eq!(tail.cols.len(), 5);
        assert_eq!(tail.cols[0].you[0], ((KEEP + 5) % 256) as u8);
    }

    #[test]
    fn a_kick_lands_in_the_low_band_and_a_hat_in_the_high() {
        let kick: Vec<f32> = (0..480)
            .flat_map(|i| {
                let v = 0.8 * (2.0 * std::f32::consts::PI * 55.0 * i as f32 / 48_000.0).sin();
                [v, v]
            })
            .collect();
        let hat: Vec<f32> = (0..480)
            .flat_map(|i| {
                let v = if i % 2 == 0 { 0.5 } else { -0.5 };
                [v, v]
            })
            .collect();
        let k = BandSplit::default().column(&kick);
        let h = BandSplit::default().column(&hat);
        assert!(k[0] > k[2] * 2, "kick {k:?}");
        assert!(h[2] > h[0] * 2, "hat {h:?}");
    }
}

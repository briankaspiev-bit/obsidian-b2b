//! Score a merge against synthetic ground truth.

use crate::audio;
use crate::dsp::{self, Line};
use crate::session;
use crate::synth;
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
pub struct HandoffScore {
    pub index: usize,
    pub from: String,
    pub to: String,
    pub method: String,
    /// Error of the alignment map against truth, across the overlap.
    pub map_err_mean_ms: f64,
    pub map_err_max_abs_ms: f64,
    /// Error of the timestamps-only map at mid-overlap, for comparison.
    pub timestamp_err_ms: f64,
    /// Worst placement error found in the rendered stems: where the leader's audio actually
    /// sits relative to the follower's in the master, versus truth.
    pub rendered_err_ms: Option<f64>,
}

pub fn run(truth_path: &Path, report_path: &Path, stems: Option<(&Path, &Path)>) -> Result<Vec<HandoffScore>> {
    let truth = synth::load_truth(truth_path)?;
    let mut sess_ids = Vec::new();
    let placed = match stems {
        Some((session_path, dir)) => {
            let sess = session::load(session_path)?;
            let base = session_path.parent().unwrap_or(Path::new("."));
            let mut isos = Vec::new();
            let mut st = Vec::new();
            for d in &sess.djs {
                sess_ids.push(d.id.clone());
                isos.push(audio::read(&base.join(&d.iso.path))?.mono());
                st.push(audio::read(&dir.join(format!("stem_{}.wav", d.id)))?.mono());
            }
            Some((isos, st))
        }
        None => None,
    };
    let rep: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(report_path)?)?;
    let sr = truth.sample_rate as f64;
    let mut scores = Vec::new();
    for (i, h) in rep["handoffs"].as_array().context("report has no handoffs")?.iter().enumerate() {
        let from = h["from"].as_str().unwrap().to_string();
        let to = h["to"].as_str().unwrap().to_string();
        let map: Line = serde_json::from_value(h["map"].clone())?;
        let ts: Line = serde_json::from_value(h["timestamp_map"].clone())?;
        let n0 = h["follower_on_sample"].as_f64().unwrap();
        let n1 = h["follower_overlap_end_sample"].as_f64().unwrap();
        let errs: Vec<f64> = (0..=40)
            .map(|k| {
                let n = n0 + (n1 - n0) * k as f64 / 40.0;
                (map.at(n) - truth.map(&from, &to, n)) / sr * 1e3
            })
            .collect();
        let mid = 0.5 * (n0 + n1);
        let rendered = match &placed {
            Some((isos, stems)) => {
                let cf = h["follower_master_offset_samples"]
                    .as_f64()
                    .context("report too old: no follower_master_offset_samples")?;
                let li = sess_ids.iter().position(|d| *d == from).unwrap();
                let fi = sess_ids.iter().position(|d| *d == to).unwrap();
                let (ms, me) = (n0 + cf, n1 + cf);
                let mut worst = 0.0f64;
                for q in [0.2, 0.5, 0.8] {
                    let m = (ms + (me - ms) * q) as usize;
                    let n_exp = m as f64 - cf;
                    let n_meas = locate(&stems[fi], &isos[fi], m, n_exp, sr);
                    let m_meas = locate(&stems[li], &isos[li], m, map.at(n_exp), sr);
                    let e = (m_meas - truth.map(&from, &to, n_meas)) / sr * 1e3;
                    if e.abs() > worst.abs() {
                        worst = e;
                    }
                }
                Some(worst)
            }
            None => None,
        };
        scores.push(HandoffScore {
            index: i + 1,
            from,
            to,
            method: h["method"].as_str().unwrap_or("?").to_string(),
            map_err_mean_ms: errs.iter().sum::<f64>() / errs.len() as f64,
            map_err_max_abs_ms: errs.iter().fold(0.0f64, |m, e| m.max(e.abs())),
            timestamp_err_ms: (ts.at(mid) - truth.map(h["from"].as_str().unwrap(), h["to"].as_str().unwrap(), mid)) / sr * 1e3,
            rendered_err_ms: rendered,
        });
    }
    Ok(scores)
}

/// Which iso sample is playing at master sample `m + 0.5 s` in this stem (searched near
/// `guess` for `m`). The lag is a window average, so it is reported at the window centre.
fn locate(stem: &[f32], iso: &[f32], m: usize, guess: f64, sr: f64) -> f64 {
    let w = sr as usize;
    let s = (0.05 * sr) as usize;
    let x = &stem[m..m + w];
    let a = guess.round() as usize - s;
    let y = &iso[a..a + w + 2 * s];
    let (k, _) = dsp::peak(&dsp::xcorr(x, y, true));
    a as f64 + k + (w / 2) as f64
}

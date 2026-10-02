//! Offline master rebuild: align each handoff follower-anchored, then render one master.
//!
//! Model (feasibility report, sections E.1 and P): when F takes over from L, F mixed
//! against L *as heard at F*. So in the master, F's audio plays at its natural rate
//! and L is placed where F heard it. Each handoff therefore yields a map
//! `F iso sample -> L iso sample`, and L is read through that map for the overlap.
//! Placement offsets only ever change while a DJ is silent, so the master never jumps.

use crate::audio::{self, Audio};
use crate::dsp::{self, Line, Sinc};
use crate::session::{self, Session};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    /// Received stream if present and confident, else onsets, else timestamps.
    Auto,
    /// Correlate the follower's recording of the received stream against the leader's ISO.
    Received,
    /// Correlate onset envelopes of the two ISOs (beat-phase), seeded from timestamps.
    Onset,
    /// Session-clock timestamps and telemetry only, no audio analysis.
    Timestamps,
}

pub struct Opts {
    pub method: Method,
    pub gate_db: f32,
    pub target_lufs: Option<f64>,
    pub ceiling_dbtp: f64,
    pub onset_search_ms: f64,
    pub stems: Option<PathBuf>,
    pub bits: u16,
}

struct Dj {
    id: String,
    iso: Audio,
    mono: Vec<f32>,
    env: Vec<f32>,
    onset: Vec<f32>,
    /// (from id, mono received stream re-indexed onto this DJ's iso samples)
    recv: Option<(String, Vec<f32>)>,
    /// Session time (s) of iso sample 0.
    t0: f64,
    /// True samples per session-second.
    fs: f64,
    /// Capture + monitor latency in seconds.
    rt: f64,
}

const ONSET_HOP: usize = 16;

impl Dj {
    fn load(base: &Path, d: &session::Dj, sr: u32) -> Result<Dj> {
        let iso = audio::read(&base.join(&d.iso.path))?;
        if iso.sr != sr {
            bail!("{}: sample rate {} != session {}", d.iso.path, iso.sr, sr);
        }
        let mono = iso.mono();
        let recv = match &d.received {
            Some(r) => {
                let a = audio::read(&base.join(&r.path))?;
                if a.sr != sr {
                    bail!("{}: sample rate {} != session {}", r.path, a.sr, sr);
                }
                let src = a.mono();
                let mut v = vec![0.0f32; mono.len()];
                for (n, slot) in v.iter_mut().enumerate() {
                    let i = n as i64 - r.offset_samples;
                    if i >= 0 && (i as usize) < src.len() {
                        *slot = src[i as usize];
                    }
                }
                Some((r.from.clone(), v))
            }
            None => None,
        };
        Ok(Dj {
            id: d.id.clone(),
            env: dsp::env_db(&mono, sr as usize / 100),
            onset: dsp::onset_env(&mono, ONSET_HOP),
            mono,
            iso,
            recv,
            t0: d.iso.start_session_ms / 1000.0,
            fs: sr as f64 * (1.0 + d.clock_ppm * 1e-6),
            rt: d.monitor_roundtrip_ms / 1000.0,
        })
    }
}

#[derive(Serialize)]
pub struct HandoffReport {
    pub index: usize,
    pub from: String,
    pub to: String,
    pub method: Method,
    pub event_session_s: f64,
    /// Follower iso sample where the follower's audio starts.
    pub follower_on_sample: f64,
    /// Follower iso sample where the leader goes silent.
    pub follower_overlap_end_sample: f64,
    pub overlap_s: f64,
    /// Follower iso sample -> leader iso sample, as used for the render.
    pub map: Line,
    /// The same map from timestamps and telemetry alone.
    pub timestamp_map: Line,
    pub windows: usize,
    pub inliers: usize,
    pub fit_rms_ms: f64,
    pub fit_max_ms: f64,
    pub measured_drift_ppm: f64,
    /// How far the audio-derived alignment moved the timestamp guess, mid-overlap.
    pub audio_vs_timestamps_ms: f64,
    pub master_overlap_start_s: f64,
    pub master_overlap_end_s: f64,
    /// Master sample = follower iso sample + this, from this handoff on.
    pub follower_master_offset_samples: f64,
}

#[derive(Serialize)]
pub struct TurnReport {
    pub dj: String,
    pub master_start_s: f64,
    pub master_end_s: f64,
}

#[derive(Serialize)]
pub struct Report {
    pub tool: String,
    pub sample_rate: u32,
    pub master_s: f64,
    pub integrated_lufs_before: f64,
    pub gain_db: f64,
    pub integrated_lufs_after: f64,
    pub true_peak_dbtp_after: f64,
    pub turns: Vec<TurnReport>,
    pub handoffs: Vec<HandoffReport>,
    pub warnings: Vec<String>,
}

struct Est {
    map: Line,
    method: Method,
    windows: usize,
    inliers: usize,
    rms: f64,
    max: f64,
}

fn first_active(env: &[f32], from: usize, to: usize, thr: f32) -> Option<usize> {
    let mut i = from;
    while i < env.len().min(to) && env[i] > thr {
        i += 1; // still in the tail of something earlier
    }
    (i..env.len().min(to)).find(|&j| env[j] > thr)
}

fn silence_start(env: &[f32], from: usize, thr: f32, min_frames: usize) -> Option<usize> {
    let mut run = 0;
    for i in from..env.len() {
        if env[i] <= thr {
            run += 1;
            if run >= min_frames {
                return Some(i + 1 - run);
            }
        } else {
            run = 0;
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn est_received(f: &Dj, l: &Dj, recv: &[f32], d: f64, n0: f64, n1: f64, sr: f64, gate: f32) -> Option<Est> {
    let rt = f.rt * f.fs;
    // recv sample r (on F's clock) -> L iso sample
    let g0 = Line {
        x0: 0.0,
        y0: (f.t0 - d - l.t0) * l.fs,
        slope: l.fs / f.fs,
    };
    let w = (3.0 * sr) as usize;
    let hopw = (1.5 * sr) as usize;
    let s = (1.0 * sr) as usize;
    let hw = dsp::hann(w);
    let mut pts = Vec::new();
    let mut windows = 0;
    let mut r = (n0 - rt - 4.0 * sr).max(0.0) as usize;
    let end = (n1 - rt + 1.0 * sr).max(0.0) as usize;
    while r + w <= recv.len() && r < end {
        let x = &recv[r..r + w];
        let pm = g0.at(r as f64).round() as i64;
        let a = pm - s as i64;
        if dsp::rms_db(x) > gate && a >= 0 && (a as usize) + w + 2 * s <= l.mono.len() {
            windows += 1;
            let xw: Vec<f32> = x.iter().zip(&hw).map(|(v, h)| v * h).collect();
            let y = &l.mono[a as usize..a as usize + w + 2 * s];
            let (k, conf) = dsp::peak(&dsp::xcorr(&xw, y, true));
            if conf > 8.0 {
                // The lag is an average over the window, so it belongs to the window centre.
                let half = (w / 2) as f64;
                pts.push((r as f64 + half, a as f64 + k + half));
            }
        }
        r += hopw;
    }
    let fit = dsp::robust_line(&pts, None, 0.25e-3 * sr)?;
    if fit.inliers < 4 || fit.rms > 1e-3 * sr {
        return None;
    }
    let g = fit.line;
    Some(Est {
        map: Line {
            x0: g.x0 + rt,
            y0: g.y0,
            slope: g.slope,
        },
        method: Method::Received,
        windows,
        inliers: fit.inliers,
        rms: fit.rms,
        max: fit.max_abs,
    })
}

#[allow(clippy::too_many_arguments)]
fn est_onset(f: &Dj, l: &Dj, coarse: Line, n0: f64, n1: f64, sr: f64, search_ms: f64, gate: f32) -> Option<Est> {
    let osr = sr / ONSET_HOP as f64;
    let wf = (8.0 * osr) as usize;
    let hopf = (4.0 * osr) as usize;
    let sf = (search_ms * 1e-3 * osr) as usize;
    let mut pts = Vec::new();
    let mut windows = 0;
    let mut fi = ((n0 + 2.0 * sr) / ONSET_HOP as f64) as usize;
    let end = ((n1 - 1.0 * sr) / ONSET_HOP as f64).max(0.0) as usize;
    while fi + wf <= end.min(f.onset.len()) {
        let ns = (fi * ONSET_HOP) as f64;
        let pm = coarse.at(ns);
        let a = (pm / ONSET_HOP as f64).floor() as i64 - sf as i64;
        let audible = |x: &[f32], at: usize, len: usize| at + len <= x.len() && dsp::rms_db(&x[at..at + len]) > gate;
        let span = wf * ONSET_HOP;
        if a >= 0
            && (a as usize) + wf + 2 * sf <= l.onset.len()
            && audible(&f.mono, ns as usize, span)
            && audible(&l.mono, pm.max(0.0) as usize, span)
        {
            windows += 1;
            let x = &f.onset[fi..fi + wf];
            let y = &l.onset[a as usize..a as usize + wf + 2 * sf];
            let r = dsp::ncc_direct(x, y);
            let (k, _) = dsp::peak(&r);
            let pk = r[k.round() as usize];
            if pk > 0.15 {
                let half = (span / 2) as f64;
                pts.push((ns + half, (a as f64 + k) * ONSET_HOP as f64 + half));
            }
        }
        fi += hopf;
    }
    let fit = dsp::robust_line(&pts, Some(coarse.slope), 1e-3 * sr)?;
    if fit.inliers < 3 {
        return None;
    }
    Some(Est {
        map: fit.line,
        method: Method::Onset,
        windows,
        inliers: fit.inliers,
        rms: fit.rms,
        max: fit.max_abs,
    })
}

enum Kind {
    Nat { c: f64 },
    Warp { map: Line, c_f: f64 },
}

struct Piece {
    dj: usize,
    m0: i64,
    m1: i64,
    fade_in: bool,
    fade_out: bool,
    kind: Kind,
}

impl Piece {
    fn pos(&self, m: i64) -> f64 {
        match self.kind {
            Kind::Nat { c } => m as f64 - c,
            Kind::Warp { map, c_f } => map.at(m as f64 - c_f),
        }
    }
}

pub fn run(session_path: &Path, out: &Path, report_path: Option<&Path>, o: &Opts) -> Result<Report> {
    let s: Session = session::load(session_path)?;
    let base = session_path.parent().unwrap_or(Path::new("."));
    let sr = s.sample_rate as f64;
    let hop = s.sample_rate as usize / 100;
    let mut warnings = Vec::new();

    let djs: Vec<Dj> = s.djs.iter().map(|d| Dj::load(base, d, s.sample_rate)).collect::<Result<_>>()?;
    let idx = |id: &str| djs.iter().position(|d| d.id == id).with_context(|| format!("unknown dj '{id}'"));
    let nch = djs[0].iso.ch.len();
    if djs.iter().any(|d| d.iso.ch.len() != nch) {
        bail!("all ISO recordings must have the same channel count");
    }

    // Turn order from the event log.
    let mut evs: Vec<&session::Event> = s.events.iter().collect();
    evs.sort_by(|a, b| a.t_session_ms.partial_cmp(&b.t_session_ms).unwrap());
    let takeovers: Vec<(usize, usize, f64)> = evs
        .iter()
        .filter(|e| e.kind == "take_over")
        .map(|e| {
            Ok((
                idx(e.from.as_deref().context("take_over without from")?)?,
                idx(e.to.as_deref().context("take_over without to")?)?,
                e.t_session_ms / 1000.0,
            ))
        })
        .collect::<Result<_>>()?;
    let first = match evs.iter().find(|e| e.kind == "on_air") {
        Some(e) => idx(e.dj.as_deref().context("on_air without dj")?)?,
        None => takeovers.first().map(|t| t.0).context("no on_air or take_over events")?,
    };

    let pre = (0.5 * sr) as i64;
    let post = 0.5 * sr;
    let fade = (0.02 * sr) as i64;

    // First DJ: master sample 0 is half a second before their first sound.
    // Plain first sound: the opening DJ is often already playing at sample 0, so there is no
    // earlier tail to skip (first_active would skip their whole first run).
    let on0 = djs[first]
        .env
        .iter()
        .position(|&v| v > o.gate_db)
        .context("first DJ never makes a sound")?
        * hop;
    let mut c = vec![0.0f64; djs.len()];
    c[first] = (pre - on0 as i64) as f64;
    let mut pieces: Vec<Piece> = Vec::new();
    let mut cur = Piece {
        dj: first,
        m0: 0,
        m1: 0,
        fade_in: true,
        fade_out: false,
        kind: Kind::Nat { c: c[first] },
    };
    let mut reports = Vec::new();
    let mut turns = Vec::new();
    let mut turn_start = 0i64;
    let mut on_air = first;

    for (k, &(li, fi, te)) in takeovers.iter().enumerate() {
        if li != on_air {
            warnings.push(format!(
                "handoff {}: log says {} -> {}, but {} was on air",
                k + 1,
                djs[li].id,
                djs[fi].id,
                djs[on_air].id
            ));
        }
        let (l, f) = (&djs[li], &djs[fi]);
        let key = format!("{}->{}", l.id, f.id);
        let d = match s.telemetry.one_way_ms.get(&key) {
            Some(v) => v / 1000.0,
            None => {
                warnings.push(format!("handoff {}: no one_way_ms for {key}, assuming 100 ms", k + 1));
                0.1
            }
        };
        let coarse = Line {
            x0: 0.0,
            y0: (f.t0 - f.rt - d - l.t0) * l.fs,
            slope: l.fs / f.fs,
        };

        // Where the follower comes in and where the leader goes quiet.
        let ne = ((te - f.t0) * f.fs).max(0.0);
        let fe = ne as usize / hop;
        let f_on = first_active(&f.env, fe.saturating_sub(2000), fe + 6000, o.gate_db)
            .with_context(|| format!("handoff {}: no audio from {} near the take-over", k + 1, f.id))?
            * hop;
        let le = (coarse.at(ne).max(0.0) as usize) / hop;
        let l_end = match silence_start(&l.env, le, o.gate_db, 300) {
            Some(fr) => (fr * hop + 2 * hop) as f64,
            None => {
                warnings.push(format!("handoff {}: {} never went silent after the take-over", k + 1, l.id));
                l.mono.len() as f64
            }
        };
        let n_on = f_on as f64;
        let n_lend = coarse.inv(l_end);

        let recv = f.recv.as_ref().filter(|(from, _)| *from == l.id).map(|(_, v)| v.as_slice());
        let try_recv = || recv.and_then(|r| est_received(f, l, r, d, n_on, n_lend, sr, o.gate_db));
        let try_onset = || est_onset(f, l, coarse, n_on, n_lend, sr, o.onset_search_ms, o.gate_db);
        let ts = || Est {
            map: coarse,
            method: Method::Timestamps,
            windows: 0,
            inliers: 0,
            rms: 0.0,
            max: 0.0,
        };
        let est = match o.method {
            Method::Auto => try_recv().or_else(try_onset).unwrap_or_else(ts),
            Method::Received => try_recv().with_context(|| format!("handoff {}: received-stream alignment failed", k + 1))?,
            Method::Onset => try_onset().with_context(|| format!("handoff {}: onset alignment failed", k + 1))?,
            Method::Timestamps => ts(),
        };
        if o.method == Method::Auto && est.method != Method::Received {
            warnings.push(format!("handoff {}: fell back to {:?} alignment", k + 1, est.method));
        }
        let map = est.map;
        let n_lend = map.inv(l_end);

        // Follower placement: L stays continuous at the moment F's region begins.
        let n_f0 = n_on - pre as f64;
        c[fi] = map.at(n_f0) + c[li] - n_f0;
        let m_f0 = (n_f0 + c[fi]).round() as i64;
        let m_lend = (map.inv(l_end + post) + c[fi]).round() as i64;

        cur.m1 = m_f0;
        pieces.push(cur);
        pieces.push(Piece {
            dj: li,
            m0: m_f0,
            m1: m_lend,
            fade_in: false,
            fade_out: true,
            kind: Kind::Warp { map, c_f: c[fi] },
        });
        turns.push(TurnReport {
            dj: l.id.clone(),
            master_start_s: turn_start as f64 / sr,
            master_end_s: m_lend as f64 / sr,
        });
        cur = Piece {
            dj: fi,
            m0: m_f0,
            m1: 0,
            fade_in: true,
            fade_out: false,
            kind: Kind::Nat { c: c[fi] },
        };
        turn_start = m_f0;

        let mid = 0.5 * (n_on + n_lend);
        reports.push(HandoffReport {
            index: k + 1,
            from: l.id.clone(),
            to: f.id.clone(),
            method: est.method,
            event_session_s: te,
            follower_on_sample: n_on,
            follower_overlap_end_sample: n_lend,
            overlap_s: (n_lend - n_on) / sr,
            map,
            timestamp_map: coarse,
            windows: est.windows,
            inliers: est.inliers,
            fit_rms_ms: est.rms / sr * 1e3,
            fit_max_ms: est.max / sr * 1e3,
            measured_drift_ppm: (map.slope - 1.0) * 1e6,
            audio_vs_timestamps_ms: (map.at(mid) - coarse.at(mid)) / sr * 1e3,
            master_overlap_start_s: (n_on + c[fi]) / sr,
            master_overlap_end_s: (n_lend + c[fi]) / sr,
            follower_master_offset_samples: c[fi],
        });
        on_air = fi;
    }
    let last = &djs[on_air];
    let last_on = last.env.iter().rposition(|&v| v > o.gate_db).unwrap_or(0) * hop + 2 * hop;
    cur.m1 = (last_on as f64 + post + c[on_air]).round() as i64;
    cur.fade_out = true;
    turns.push(TurnReport {
        dj: last.id.clone(),
        master_start_s: turn_start as f64 / sr,
        master_end_s: cur.m1 as f64 / sr,
    });
    pieces.push(cur);

    // Render one stem per DJ.
    let len = pieces.iter().map(|p| p.m1).max().unwrap_or(0).max(1) as usize;
    let sinc = Sinc::new();
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let mut stems: Vec<Vec<Vec<f32>>> = vec![vec![vec![0.0f32; len]; nch]; djs.len()];
    for (di, stem) in stems.iter_mut().enumerate() {
        let mine: Vec<&Piece> = pieces.iter().filter(|p| p.dj == di).collect();
        for (ch, out_ch) in stem.iter_mut().enumerate() {
            let src = &djs[di].iso.ch[ch];
            let chunk = len.div_ceil(threads);
            std::thread::scope(|sc| {
                for (ci, buf) in out_ch.chunks_mut(chunk).enumerate() {
                    let (mine, sinc) = (&mine, &sinc);
                    sc.spawn(move || {
                        let base = (ci * chunk) as i64;
                        let top = base + buf.len() as i64;
                        for p in mine.iter() {
                            for m in p.m0.max(base)..p.m1.min(top) {
                                let mut g = 1.0f32;
                                if p.fade_in && m - p.m0 < fade {
                                    g = (0.5 - 0.5 * (std::f64::consts::PI * (m - p.m0) as f64 / fade as f64).cos()) as f32;
                                }
                                if p.fade_out && p.m1 - m < fade {
                                    g *= (0.5 - 0.5 * (std::f64::consts::PI * (p.m1 - m) as f64 / fade as f64).cos()) as f32;
                                }
                                buf[(m - base) as usize] += g * sinc.at(src, p.pos(m));
                            }
                        }
                    });
                }
            });
        }
    }

    let mut master = Audio {
        sr: s.sample_rate,
        ch: vec![vec![0.0f32; len]; nch],
    };
    for stem in &stems {
        for (mc, sc) in master.ch.iter_mut().zip(stem) {
            for (a, b) in mc.iter_mut().zip(sc) {
                *a += *b;
            }
        }
    }

    let (lufs_before, tp_before) = loudness(&master)?;
    let gain_db = match o.target_lufs {
        Some(t) => (t - lufs_before).min(o.ceiling_dbtp - tp_before),
        None => (o.ceiling_dbtp - tp_before).min(0.0),
    };
    let g = 10f64.powf(gain_db / 20.0) as f32;
    for c in master.ch.iter_mut() {
        for v in c.iter_mut() {
            *v *= g;
        }
    }
    let (lufs_after, tp_after) = loudness(&master)?;
    audio::write_wav(out, &master, o.bits)?;

    if let Some(dir) = &o.stems {
        std::fs::create_dir_all(dir)?;
        for (d, st) in djs.iter().zip(stems) {
            audio::write_wav(&dir.join(format!("stem_{}.wav", d.id)), &Audio { sr: s.sample_rate, ch: st }, 32)?;
        }
    }

    let report = Report {
        tool: format!("obsidian-merge {}", env!("CARGO_PKG_VERSION")),
        sample_rate: s.sample_rate,
        master_s: len as f64 / sr,
        integrated_lufs_before: lufs_before,
        gain_db,
        integrated_lufs_after: lufs_after,
        true_peak_dbtp_after: tp_after,
        turns,
        handoffs: reports,
        warnings,
    };
    if let Some(p) = report_path {
        std::fs::write(p, serde_json::to_string_pretty(&report)?)?;
    }
    Ok(report)
}

fn loudness(a: &Audio) -> Result<(f64, f64)> {
    let mut m = ebur128::EbuR128::new(a.ch.len() as u32, a.sr, ebur128::Mode::I | ebur128::Mode::TRUE_PEAK)?;
    let planes: Vec<&[f32]> = a.ch.iter().map(|c| c.as_slice()).collect();
    m.add_frames_planar_f32(&planes)?;
    let mut tp = 0.0f64;
    for c in 0..a.ch.len() {
        tp = tp.max(m.true_peak(c as u32)?);
    }
    Ok((m.loudness_global()?, 20.0 * (tp + 1e-12).log10()))
}

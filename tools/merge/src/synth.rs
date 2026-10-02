//! Synthetic B2B session generator with exact ground truth.
//!
//! Two simulated DJs play synthesized techno tracks on their own mixers. Every signal
//! is an analytic function of true time, so each recording can be sampled exactly on
//! its own drifting clock without resampling error. The follower starts each track on
//! a bar line of the leader's track *as heard through the network and monitor*.

use crate::audio::{self, Audio};
use crate::session::{self, Event, FileRef, Received, Session, Telemetry};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::f64::consts::PI;
use std::path::{Path, PathBuf};

pub struct SynthOpts {
    pub out: PathBuf,
    pub minutes: f64,
    pub handoffs: usize,
    pub seed: u64,
    pub bpm: f64,
    pub received: bool,
    pub human_err_ms: f64,
    pub ppm: [f64; 2],
    pub one_way_ms: [f64; 2],
    pub loss: f64,
}

#[derive(Serialize, Deserialize)]
pub struct TruthDj {
    pub id: String,
    pub t0: f64,
    pub ppm: f64,
    pub cap_ms: f64,
    pub mon_ms: f64,
}

#[derive(Serialize, Deserialize)]
pub struct TruthHandoff {
    pub from: String,
    pub to: String,
    pub follower_start_s: f64,
    pub human_err_ms: f64,
}

#[derive(Serialize, Deserialize)]
pub struct Truth {
    pub sample_rate: u32,
    pub djs: Vec<TruthDj>,
    pub one_way_ms: BTreeMap<String, f64>,
    pub handoffs: Vec<TruthHandoff>,
}

impl Truth {
    /// The exact follower-iso-sample -> leader-iso-sample map for a handoff.
    pub fn map(&self, from: &str, to: &str, n: f64) -> f64 {
        let l = self.djs.iter().find(|d| d.id == from).unwrap();
        let f = self.djs.iter().find(|d| d.id == to).unwrap();
        let sr = self.sample_rate as f64;
        let (fsl, fsf) = (sr * (1.0 + l.ppm * 1e-6), sr * (1.0 + f.ppm * 1e-6));
        let d = self.one_way_ms[&format!("{from}->{to}")] / 1000.0;
        (f.t0 + n / fsf - (f.cap_ms + f.mon_ms) / 1000.0 - d - l.t0) * fsl
    }
}

pub struct Rng(u64);

pub fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Rng {
    fn u(&mut self) -> f64 {
        self.0 = mix64(self.0);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.u()
    }
    fn normal(&mut self) -> f64 {
        let (u1, u2) = (self.u().max(1e-300), self.u());
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
    }
}

#[inline]
fn noise(seed: u64, a: i64, b: i64) -> f64 {
    let h = mix64(seed ^ mix64(a as u64 ^ mix64(b as u64)));
    (h >> 11) as f64 / (1u64 << 52) as f64 - 1.0
}

#[derive(Clone)]
struct Track {
    seed: u64,
    bpm: f64,
    root: f64,
    bass: [u8; 16],
    chords: [[f64; 3]; 4],
    hat: f64,
    kick_f0: f64,
    kick_decay: f64,
    pan: f64,
}

impl Track {
    fn random(r: &mut Rng, bpm: f64) -> Track {
        let mut bass = [0u8; 16];
        for (i, b) in bass.iter_mut().enumerate() {
            if i % 4 != 0 && r.u() < 0.55 {
                *b = 1 + (r.u() * 5.0) as u8;
            }
        }
        let root = r.range(38.0, 58.0);
        let mut chords = [[0.0; 3]; 4];
        for c in chords.iter_mut() {
            let base = root * 4.0 * [1.0, 1.122, 1.335, 1.498][(r.u() * 4.0) as usize];
            *c = [base, base * 1.189, base * 1.498];
        }
        Track {
            seed: mix64(r.0 ^ 0xABCD),
            bpm,
            root,
            bass,
            chords,
            hat: r.range(0.08, 0.2),
            kick_f0: r.range(110.0, 170.0),
            kick_decay: r.range(0.12, 0.22),
            pan: r.range(-0.3, 0.3),
        }
    }

    /// Sample at `u` seconds after the track's first downbeat.
    fn at(&self, u: f64) -> [f64; 2] {
        let t = 60.0 / self.bpm;
        let (mut l, mut r) = (0.0, 0.0);
        let b = (u / t).floor() as i64;
        for bb in [b, b - 1] {
            if bb < 0 {
                continue;
            }
            let v = u - bb as f64 * t;
            if v < 0.45 {
                let ph = 2.0 * PI * (48.0 * v + (self.kick_f0 - 48.0) * 0.025 * (1.0 - (-v / 0.025).exp()));
                let s = 0.7 * ph.sin() * (-v / self.kick_decay).exp() * (1.0 - (-v / 0.0008).exp());
                l += s;
                r += s;
            }
            let bp = bb.rem_euclid(4);
            if (bp == 1 || bp == 3) && v < 0.3 {
                let si = (v * 48000.0) as i64;
                let a = 0.25 * (-v / 0.07).exp();
                l += a * noise(self.seed ^ 1, bb, si);
                r += a * noise(self.seed ^ 2, bb, si);
            }
        }
        let ob = ((u - 0.5 * t) / t).floor() as i64;
        for oo in [ob, ob - 1] {
            if oo < 0 {
                continue;
            }
            let v = u - (oo as f64 + 0.5) * t;
            if (0.0..0.25).contains(&v) {
                let si = (v * 48000.0) as i64;
                let n = 0.5 * (noise(self.seed ^ 3, oo, si) - noise(self.seed ^ 3, oo, si - 1));
                let a = self.hat * (-v / 0.06).exp() * n;
                l += a * (1.0 - self.pan);
                r += a * (1.0 + self.pan);
            }
        }
        let q = t / 4.0;
        let st = (u / q).floor() as i64;
        let v = u - st as f64 * q;
        if st.rem_euclid(2) == 1 {
            let si = (v * 48000.0) as i64;
            let n = 0.5 * (noise(self.seed ^ 4, st, si) - noise(self.seed ^ 4, st, si - 1));
            let a = 0.05 * (-v / 0.015).exp() * n;
            l += a * (1.0 + self.pan);
            r += a * (1.0 - self.pan);
        }
        let p = self.bass[st.rem_euclid(16) as usize] as usize;
        if p > 0 {
            let f = self.root * [0.0, 1.0, 1.5, 2.0, 1.189, 0.75][p];
            let a = 0.25 * (-v / 0.1).exp() * (1.0 - (-v / 0.004).exp());
            let w = (2.0 * PI * f * v).sin() + 0.4 * (4.0 * PI * f * v).sin() + 0.2 * (6.0 * PI * f * v).sin();
            l += a * w;
            r += a * w;
        }
        let blk_len = 16.0 * t;
        let blk = (u / blk_len).floor() as i64;
        let vb = u - blk as f64 * blk_len;
        let mut e = (vb / 0.05).min((blk_len - vb) / 0.05).min(1.0);
        e = e * e * (3.0 - 2.0 * e);
        for (i, &f) in self.chords[blk.rem_euclid(4) as usize].iter().enumerate() {
            l += 0.035 * e * (2.0 * PI * f * 1.002 * u + i as f64).sin();
            r += 0.035 * e * (2.0 * PI * f * 0.998 * u + 2.0 * i as f64).sin();
        }
        [l, r]
    }
}

struct Turn {
    tr: Track,
    start: f64,
    fin: (f64, f64),
    fout: (f64, f64),
}

struct SDj {
    id: &'static str,
    name: &'static str,
    turns: Vec<Turn>,
    t0: f64,
    fs: f64,
    cap: f64,
    mon: f64,
    seed: u64,
}

impl SDj {
    /// This DJ's mixer output at true time `tau`.
    fn mixer(&self, tau: f64) -> [f64; 2] {
        let mut o = [0.0; 2];
        for tn in &self.turns {
            if tau >= tn.fin.0 && tau < tn.fout.1 {
                let g = if tau < tn.fin.1 {
                    (0.5 * PI * (tau - tn.fin.0) / (tn.fin.1 - tn.fin.0)).sin()
                } else if tau >= tn.fout.0 {
                    (0.5 * PI * (tau - tn.fout.0) / (tn.fout.1 - tn.fout.0)).cos()
                } else {
                    1.0
                };
                let s = tn.tr.at(tau - tn.start);
                o[0] += g * s[0];
                o[1] += g * s[1];
            }
        }
        // Interface noise floor and a little mains hum, always present.
        let fl = noise(self.seed, 7, (tau * 48000.0).floor() as i64) * 1.2e-4 + 2.5e-4 * (2.0 * PI * 50.0 * tau).sin();
        [o[0] + fl, o[1] + fl]
    }
}

fn render(n: usize, f: impl Fn(usize) -> [f64; 2] + Sync) -> [Vec<f32>; 2] {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let mut l = vec![0.0f32; n];
    let mut r = vec![0.0f32; n];
    let chunk = n.div_ceil(threads).max(1);
    std::thread::scope(|sc| {
        for (ci, (lc, rc)) in l.chunks_mut(chunk).zip(r.chunks_mut(chunk)).enumerate() {
            let f = &f;
            sc.spawn(move || {
                for i in 0..lc.len() {
                    let s = f(ci * chunk + i);
                    lc[i] = s[0] as f32;
                    rc[i] = s[1] as f32;
                }
            });
        }
    });
    [l, r]
}

pub fn run(o: &SynthOpts) -> Result<()> {
    std::fs::create_dir_all(&o.out)?;
    let sr = 48000u32;
    let mut rng = Rng(mix64(o.seed));
    let mut djs = [
        SDj {
            id: "A",
            name: "Val",
            turns: vec![],
            t0: 0.0,
            fs: sr as f64 * (1.0 + o.ppm[0] * 1e-6),
            cap: 4.2e-3,
            mon: 9.1e-3,
            seed: 11,
        },
        SDj {
            id: "B",
            name: "Dana",
            turns: vec![],
            t0: 2.3871,
            fs: sr as f64 * (1.0 + o.ppm[1] * 1e-6),
            cap: 6.9e-3,
            mon: 12.4e-3,
            seed: 22,
        },
    ];
    let d = [o.one_way_ms[0] / 1000.0, o.one_way_ms[1] / 1000.0]; // A->B, B->A
    let t = 60.0 / o.bpm;
    let bar = 4.0 * t;
    let turn_len = o.minutes * 60.0 / (o.handoffs + 1) as f64;

    let mut events = Vec::new();
    let mut truth_h = Vec::new();
    let start0 = 3.0;
    events.push(Event {
        t_session_ms: start0 * 1000.0 + rng.normal() * 15.0,
        kind: "on_air".into(),
        dj: Some("A".into()),
        from: None,
        to: None,
    });
    let mut cur = 0usize;
    let mut turn = Turn {
        tr: Track::random(&mut rng, o.bpm),
        start: start0,
        fin: (start0, start0 + 2.0 * bar),
        fout: (0.0, 0.0),
    };
    for _ in 0..o.handoffs {
        let f = 1 - cur;
        let hb = turn.start + (turn_len / bar).round() * bar;
        // What the follower hears of the leader is late by monitor + network + leader capture.
        let delay = djs[f].mon + d[cur] + djs[cur].cap;
        let he = o.human_err_ms * rng.normal();
        let fstart = hb + delay + he / 1000.0;
        turn.fout = (hb + 16.0 * bar, hb + 32.0 * bar);
        djs[cur].turns.push(turn);
        events.push(Event {
            t_session_ms: fstart * 1000.0 + rng.normal() * 15.0,
            kind: "take_over".into(),
            dj: None,
            from: Some(djs[cur].id.into()),
            to: Some(djs[f].id.into()),
        });
        truth_h.push(TruthHandoff {
            from: djs[cur].id.into(),
            to: djs[f].id.into(),
            follower_start_s: fstart,
            human_err_ms: he,
        });
        turn = Turn {
            tr: Track::random(&mut rng, o.bpm),
            start: fstart,
            fin: (fstart, fstart + 16.0 * bar),
            fout: (0.0, 0.0),
        };
        cur = f;
    }
    turn.fout = (turn.start + turn_len, turn.start + turn_len + 8.0 * bar);
    let end = turn.fout.1 + 4.0;
    djs[cur].turns.push(turn);

    let mut sdjs = Vec::new();
    for x in 0..2 {
        let me = &djs[x];
        let other = &djs[1 - x];
        let n = ((end - me.t0) * me.fs) as usize;
        eprintln!("synth: rendering {} ISO ({:.1} min)", me.id, n as f64 / sr as f64 / 60.0);
        let iso = render(n, |i| me.mixer(me.t0 + i as f64 / me.fs - me.cap));
        let iso_name = format!("{}_iso.wav", me.id.to_lowercase());
        audio::write_wav(&o.out.join(&iso_name), &Audio { sr, ch: iso.to_vec() }, 24)?;

        let received = if o.received {
            eprintln!("synth: rendering what {} heard from {}", me.id, other.id);
            // Sample n of this file is played to `me`'s monitor at the same instant iso sample n is captured.
            let dd = d[1 - x];
            let raw = render(n, |i| other.mixer(me.t0 + i as f64 / me.fs - dd - other.cap));
            let mono = degrade(raw, o.loss, &mut rng);
            let name = format!("{}_recv_from_{}.wav", me.id.to_lowercase(), other.id.to_lowercase());
            audio::write_wav(&o.out.join(&name), &Audio { sr, ch: vec![mono] }, 16)?;
            Some(Received {
                from: other.id.into(),
                path: name,
                offset_samples: 0,
            })
        } else {
            None
        };
        sdjs.push(session::Dj {
            id: me.id.into(),
            name: Some(me.name.into()),
            iso: FileRef {
                path: iso_name,
                start_session_ms: me.t0 * 1000.0 + rng.normal() * 2.0,
            },
            received,
            monitor_roundtrip_ms: (me.cap + me.mon) * 1000.0 + rng.normal() * 0.3,
            clock_ppm: (me.fs / sr as f64 - 1.0) * 1e6 + rng.normal() * 0.5,
        });
    }

    let mut tel = BTreeMap::new();
    tel.insert("A->B".to_string(), o.one_way_ms[0] + rng.normal() * 4.0);
    tel.insert("B->A".to_string(), o.one_way_ms[1] + rng.normal() * 4.0);
    let sess = Session {
        version: 1,
        sample_rate: sr,
        djs: sdjs,
        events,
        telemetry: Telemetry { one_way_ms: tel },
    };
    std::fs::write(o.out.join("session.json"), serde_json::to_string_pretty(&sess)?)?;

    let mut ow = BTreeMap::new();
    ow.insert("A->B".to_string(), o.one_way_ms[0]);
    ow.insert("B->A".to_string(), o.one_way_ms[1]);
    let truth = Truth {
        sample_rate: sr,
        djs: djs
            .iter()
            .map(|d| TruthDj {
                id: d.id.into(),
                t0: d.t0,
                ppm: (d.fs / sr as f64 - 1.0) * 1e6,
                cap_ms: d.cap * 1e3,
                mon_ms: d.mon * 1e3,
            })
            .collect(),
        one_way_ms: ow,
        handoffs: truth_h,
    };
    std::fs::write(o.out.join("truth.json"), serde_json::to_string_pretty(&truth)?)?;
    eprintln!("synth: wrote {}", o.out.display());
    Ok(())
}

/// Rough stand-in for a lossy network stream: band-limit, gain change, noise, and
/// 20 ms packets lost outright (zeroed, which is harsher than real concealment).
fn degrade(x: [Vec<f32>; 2], loss: f64, rng: &mut Rng) -> Vec<f32> {
    let a = 1.0 - (-2.0 * PI * 14000.0 / 48000.0).exp();
    let (mut y1, mut y2) = (0.0f64, 0.0f64);
    let mut out: Vec<f32> = x[0]
        .iter()
        .zip(&x[1])
        .map(|(l, r)| {
            let s = 0.5 * (*l as f64 + *r as f64);
            y1 += a * (s - y1);
            y2 += a * (y1 - y2);
            (0.7 * y2 + 0.003 * rng.normal()) as f32
        })
        .collect();
    for blk in out.chunks_mut(960) {
        if rng.u() < loss {
            blk.fill(0.0);
        }
    }
    out
}

pub fn load_truth(p: &Path) -> Result<Truth> {
    Ok(serde_json::from_str(&std::fs::read_to_string(p)?)?)
}

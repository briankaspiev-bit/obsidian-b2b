//! Booth engine session for one peer.
//!
//! A peer sends its own stereo program to the other peer and plays the other
//! peer's program on a "monitor" output at a fixed delay. With no audio devices
//! yet, input is a WAV file and the monitor output is a WAV file written in
//! real time by a paced output thread that stands in for the device callback.
//!
//! Threads:
//! * sender — paced by the (optionally skewed) capture clock: encode, packetize
//!   with redundancy, send; also sends clock-sync pings.
//! * receiver — UDP in: media into the playout buffer, ping/pong for clock sync.
//! * output — every 5 ms renders the monitor block, logs telemetry, runs
//!   beat-quantized monitoring and the simulated "follower DJ".

use anyhow::{Context, Result};
use obsidian_align::{beat_phase, estimate_period, phase_lag, quantize_extra, OnsetTracker};
use obsidian_clock::{percentile, sleep_until, ClockSync, SessionClock};
use obsidian_codec::{CodecConfig, Encoder, SAMPLE_RATE};
use obsidian_jitter::{PlayoutBuffer, PlayoutConfig, PlayoutStats};
use obsidian_protocol::{Frame, MediaPacket, Packet};
use serde::Serialize;
use std::collections::VecDeque;
use std::io::Write;
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use obsidian_codec;
pub use obsidian_jitter;

const HOP: usize = 48; // 1 ms onset hop

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum MonitorMode {
    /// Plain constant delay (what the follower uses).
    Plain,
    /// Beat-quantized: add delay so the remote lands on our next beat (what the leader uses).
    Beat,
}

#[derive(Debug, Clone)]
pub struct PeerConfig {
    pub name: String,
    pub bind: SocketAddr,
    pub peer: SocketAddr,
    /// Interleaved stereo 48 kHz program (the DJ's own master).
    pub program: Arc<Vec<f32>>,
    pub codec: CodecConfig,
    /// Redundancy offsets: each packet also carries frames k-o for each o.
    pub redundancy: Vec<u64>,
    pub playout: PlayoutConfig,
    /// Session length from `start_at_us` (seconds).
    pub duration_s: f64,
    /// Session-clock start time (epoch µs, *without* the simulated offset).
    pub start_at_us: i64,
    /// Simulated wall-clock offset of this machine (µs).
    pub clock_offset_us: i64,
    /// Simulated audio clock error of this machine (ppm, + = fast).
    pub skew_ppm: f64,
    pub monitor: MonitorMode,
    /// Tempo hint for beat modes. None = estimate from audio.
    pub bpm: Option<f64>,
    /// Simulated follower DJ: stay silent, then start the program on the remote's beat.
    pub follow: bool,
    /// Simulated outgoing DJ: this many seconds after the remote comes in, fade our
    /// program out over 4 s (the handoff). None = keep playing.
    pub fade_out_after_remote_s: Option<f64>,
    pub out_dir: PathBuf,
    pub stream_id: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct BeatEvent {
    pub at_out_us: i64,
    pub period_ms: f64,
    /// Remote lag behind our own beats, within one beat (−period/2, period/2].
    pub lag_ms: f64,
    pub confidence: f32,
    pub applied_extra_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FollowDrop {
    pub decided_at_out_us: i64,
    pub period_ms: f64,
    /// Output-clock time at which our track's first beat is heard locally.
    pub drop_at_out_us: i64,
    pub drop_sender_pos: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DelaySummary {
    pub blocks: usize,
    pub min_ms: f64,
    pub p50_ms: f64,
    pub max_ms: f64,
    pub std_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerReport {
    pub name: String,
    pub start_at_us: i64,
    pub clock_offset_param_us: i64,
    pub skew_ppm: f64,
    pub monitor: MonitorMode,
    pub codec: String,
    pub bitrate: i32,
    pub frame_samples: usize,
    pub redundancy: Vec<u64>,
    pub encoder_lookahead_samples: usize,
    pub playout_start_out_us: Option<i64>,
    pub network_test_jitter_ms: [f64; 4],
    pub booth_margin_ms: f64,
    pub clock_offset_est_us: Option<i64>,
    pub min_rtt_ms: Option<f64>,
    pub rtt_p50_ms: f64,
    pub rtt_p99_ms: f64,
    pub packets_sent: u64,
    pub bytes_sent: u64,
    pub send_kbps: f64,
    pub playout: PlayoutStats,
    pub delay_est: Option<DelaySummary>,
    /// Median capture→playout delay over the last 10 s of non-silent blocks (ms).
    pub delay_final_ms: Option<f64>,
    /// Sample index in sent.wav (the ISO) that monitor.wav sample 0 lines up with.
    pub monitor_offset_samples: Option<u64>,
    pub beat_events: Vec<BeatEvent>,
    pub follow_drop: Option<FollowDrop>,
    /// Output-clock time at which the simulated outgoing fade started.
    pub fade_out_at_out_us: Option<i64>,
    pub cpu_seconds: Option<f64>,
}

enum DiskMsg {
    Sent(Vec<f32>),
    Monitor(Vec<f32>, String),
}

/// What this DJ is playing locally, as a function of sender sample position.
struct LocalProgram {
    program: Arc<Vec<f32>>,
    /// Sample position at which the program starts (None = not yet).
    drop_pos: Option<f64>,
    /// (start sample, length) of the outgoing fade; silent afterwards.
    fade: Option<(u64, u64)>,
}

impl LocalProgram {
    fn frame(&self, pos: u64, n: usize, out: &mut Vec<f32>) {
        out.clear();
        let len = self.program.len() / 2;
        for i in 0..n as u64 {
            let p = pos + i;
            match self.drop_pos {
                Some(d) if (p as f64) >= d => {
                    let idx = ((p as f64 - d).floor() as usize) % len;
                    let g = match self.fade {
                        Some((s, l)) if p >= s => 1.0 - ((p - s) as f32 / l as f32).min(1.0),
                        _ => 1.0,
                    };
                    out.push(self.program[idx * 2] * g);
                    out.push(self.program[idx * 2 + 1] * g);
                }
                _ => {
                    out.push(0.0);
                    out.push(0.0);
                }
            }
        }
    }
}

fn cpu_seconds() -> Option<f64> {
    let s = std::fs::read_to_string("/proc/self/stat").ok()?;
    let rest = s.rsplit(')').next()?;
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ut: f64 = f.get(11)?.parse().ok()?;
    let st: f64 = f.get(12)?.parse().ok()?;
    Some((ut + st) / 100.0)
}

/// Bind `cfg.bind` and run the session.
pub fn run_peer(cfg: PeerConfig) -> Result<PeerReport> {
    let sock = UdpSocket::bind(cfg.bind).with_context(|| format!("bind {}", cfg.bind))?;
    run_peer_with_socket(cfg, sock)
}

/// Run the session on a socket someone else already set up, e.g. the room-code /
/// hole-punching client: after punching, the media must flow on that same socket
/// (same local port) to `cfg.peer`, the address the rendezvous returned
/// (`cfg.bind` is ignored). Packets from other addresses are still accepted, so a
/// relay can be swapped in by changing `cfg.peer`.
pub fn run_peer_with_socket(cfg: PeerConfig, sock: UdpSocket) -> Result<PeerReport> {
    std::fs::create_dir_all(&cfg.out_dir)?;
    let clock = SessionClock::new(cfg.clock_offset_us);
    let start_local = cfg.start_at_us + cfg.clock_offset_us; // start in this peer's clock
    let end_local = start_local + (cfg.duration_s * 1e6) as i64;
    sock.set_read_timeout(Some(Duration::from_millis(50)))?;
    let stop = Arc::new(AtomicBool::new(false));

    let mut playout_cfg = cfg.playout.clone();
    let frame_us = cfg.codec.frame_samples as i64 * 1_000_000 / SAMPLE_RATE as i64;
    playout_cfg.recovery_us = cfg.redundancy.iter().max().copied().unwrap_or(0) as i64 * frame_us;
    playout_cfg.output_block_us = frame_us;
    let pb = Arc::new(Mutex::new(PlayoutBuffer::new(
        playout_cfg,
        cfg.codec.clone(),
    )?));
    let sync = Arc::new(Mutex::new(ClockSync::new(64)));
    let local = Arc::new(Mutex::new(LocalProgram {
        program: cfg.program.clone(),
        drop_pos: if cfg.follow { None } else { Some(0.0) },
        fade: None,
    }));
    let sent_packets = Arc::new(AtomicU64::new(0));
    let sent_bytes = Arc::new(AtomicU64::new(0));

    // ---------------- disk writer (keeps file I/O off the real-time threads) ----------------
    let (wtx, wrx) = std::sync::mpsc::channel::<DiskMsg>();
    let writer = {
        let dir = cfg.out_dir.clone();
        std::thread::spawn(move || -> Result<()> {
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate: SAMPLE_RATE,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let mut sent = hound::WavWriter::create(dir.join("sent.wav"), spec)?;
            let mut mon = hound::WavWriter::create(dir.join("monitor.wav"), spec)?;
            let mut blocks =
                std::io::BufWriter::new(std::fs::File::create(dir.join("blocks.csv"))?);
            writeln!(
                blocks,
                "out_us,read_pos,capture_tx_us,offset_est_us,delay_est_us,concealed,ratio,silent"
            )?;
            for m in wrx {
                match m {
                    DiskMsg::Sent(v) => v.iter().try_for_each(|&x| sent.write_sample(x))?,
                    DiskMsg::Monitor(v, line) => {
                        v.iter().try_for_each(|&x| mon.write_sample(x))?;
                        writeln!(blocks, "{line}")?;
                    }
                }
            }
            sent.finalize()?;
            mon.finalize()?;
            blocks.flush()?;
            Ok(())
        })
    };

    // ---------------- receiver ----------------
    let rx = {
        let sock = sock.try_clone()?;
        let pb = pb.clone();
        let sync = sync.clone();
        let stop = stop.clone();
        std::thread::spawn(move || -> Result<()> {
            let mut buf = vec![0u8; 65_536];
            let mut out = Vec::with_capacity(64);
            while !stop.load(Ordering::Relaxed) {
                let (n, from) = match sock.recv_from(&mut buf) {
                    Ok(v) => v,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        continue
                    }
                    Err(_) => continue,
                };
                let now = clock.now_us();
                let Ok(pkt) = Packet::decode(&buf[..n]) else {
                    continue;
                };
                match pkt {
                    Packet::Media(m) => pb.lock().unwrap().push(&m, now),
                    Packet::Ping { id, t0_us } => {
                        let t2 = clock.now_us();
                        Packet::Pong {
                            id,
                            t0_us,
                            t1_us: now,
                            t2_us: t2,
                        }
                        .encode(&mut out);
                        let _ = sock.send_to(&out, from);
                    }
                    Packet::Pong {
                        t0_us,
                        t1_us,
                        t2_us,
                        ..
                    } => {
                        sync.lock().unwrap().add(t0_us, t1_us, t2_us, now);
                    }
                    Packet::Bye => {}
                }
            }
            Ok(())
        })
    };

    // ---------------- sender ----------------
    let mut encoder = Encoder::new(&cfg.codec)?;
    let lookahead = encoder.lookahead();
    let tx = {
        let sock = sock.try_clone()?;
        let cfg = cfg.clone();
        let local = local.clone();
        let sent_packets = sent_packets.clone();
        let sent_bytes = sent_bytes.clone();
        let wtx = wtx.clone();
        std::thread::spawn(move || -> Result<()> {
            let fs = cfg.codec.frame_samples;
            let period_us = frame_us as f64 / (1.0 + cfg.skew_ppm * 1e-6);
            let max_red = cfg.redundancy.iter().max().copied().unwrap_or(0) as usize;
            let mut history: VecDeque<Frame> = VecDeque::new();
            let mut pcm = Vec::with_capacity(fs * 2);
            let mut buf = Vec::with_capacity(2048);
            let mut ping_id = 0u32;
            let mut next_ping = start_local;
            let mut k: u64 = 0;
            loop {
                let capture_us = start_local + (k as f64 * period_us) as i64;
                let ready_us = start_local + ((k + 1) as f64 * period_us) as i64;
                if ready_us > end_local {
                    break;
                }
                sleep_until(clock.instant_at(ready_us));
                local.lock().unwrap().frame(k * fs as u64, fs, &mut pcm);
                let _ = wtx.send(DiskMsg::Sent(pcm.clone()));
                let payload = encoder.encode(&pcm)?;
                let primary = Frame {
                    index: k,
                    capture_us,
                    payload,
                };
                let redundant = cfg
                    .redundancy
                    .iter()
                    .filter_map(|&o| history.iter().rev().find(|f| f.index + o == k).cloned())
                    .collect();
                let pkt = Packet::Media(MediaPacket {
                    stream_id: cfg.stream_id,
                    seq: k as u32,
                    codec: cfg.codec.codec,
                    frame_samples: fs as u16,
                    primary: primary.clone(),
                    redundant,
                });
                pkt.encode(&mut buf);
                if sock.send_to(&buf, cfg.peer).is_ok() {
                    sent_packets.fetch_add(1, Ordering::Relaxed);
                    sent_bytes.fetch_add(buf.len() as u64 + 28, Ordering::Relaxed);
                    // + IPv4/UDP headers
                }
                history.push_back(primary);
                while history.len() > max_red.max(1) {
                    history.pop_front();
                }
                let now = clock.now_us();
                if now >= next_ping {
                    Packet::Ping {
                        id: ping_id,
                        t0_us: clock.now_us(),
                    }
                    .encode(&mut buf);
                    let _ = sock.send_to(&buf, cfg.peer);
                    ping_id += 1;
                    next_ping = now + 100_000;
                }
                k += 1;
            }
            Packet::Bye.encode(&mut buf);
            let _ = sock.send_to(&buf, cfg.peer);
            Ok(())
        })
    };

    // ---------------- output (stands in for the device callback) ----------------
    let n = cfg.codec.frame_samples;
    let mut out = Vec::with_capacity(n * 2);
    let mut local_block = Vec::with_capacity(n * 2);
    let mut local_onsets = OnsetTracker::new(HOP);
    let mut remote_onsets = OnsetTracker::new(HOP);
    let mut playout_start: Option<i64> = None;
    let mut net_jitter = [f64::NAN; 4];
    let mut delays = Vec::new();
    let mut beat_events = Vec::new();
    let mut fade_out_at: Option<i64> = None;
    let mut quant_total_ms = 0f64;
    let mut final_delays: Vec<(i64, f64)> = Vec::new();
    let mut follow_drop: Option<FollowDrop> = None;
    let mut next_beat_check: Option<i64> = None;
    let mut remote_active_since: Option<i64> = None;
    // The output device shares the capture device's clock (one interface does both),
    // so output blocks run on the same skewed grid as capture frames: block j plays
    // while frame j is captured. That keeps monitor.wav sample-locked to sent.wav.
    let block_period_us = frame_us as f64 / (1.0 + cfg.skew_ppm * 1e-6);
    pb.lock().unwrap().set_output_ppm(cfg.skew_ppm);
    let mut monitor_offset_samples: Option<u64> = None;
    let mut j: i64 = 0;
    loop {
        let t = start_local + (j as f64 * block_period_us).round() as i64;
        let block_index = j as u64;
        if t + frame_us > end_local - 200_000 {
            break;
        }
        sleep_until(clock.instant_at(t));
        j += 1;
        let mut pbl = pb.lock().unwrap();
        if playout_start.is_none() {
            if pbl.ready_to_start() {
                let s = pbl.jitter_summary();
                net_jitter = [s.0 / 1e3, s.1 / 1e3, s.2 / 1e3, s.3 / 1e3];
                pbl.start(t, 0.0);
                playout_start = Some(t);
                monitor_offset_samples = Some(block_index * n as u64);
            } else {
                continue;
            }
        }
        let info = pbl.render(t, n, &mut out)?;
        drop(pbl);
        let offset = sync.lock().unwrap().offset_us();
        let delay = match (info.capture_tx_us, offset) {
            (Some(c), Some(o)) => Some(t - (c - o)),
            _ => None,
        };
        let line = format!(
            "{},{:.3},{},{},{},{},{:.9},{}",
            t,
            info.read_pos,
            info.capture_tx_us
                .map(|v| v.to_string())
                .unwrap_or_default(),
            offset.map(|v| v.to_string()).unwrap_or_default(),
            delay.map(|v| v.to_string()).unwrap_or_default(),
            info.concealed_frames,
            info.ratio,
            info.silent as u8
        );
        let _ = wtx.send(DiskMsg::Monitor(out.clone(), line));
        if let Some(d) = delay {
            if !info.silent {
                delays.push(d as f64);
                final_delays.push((t, d as f64));
            }
        }

        // Local program as heard right now: our own sample position at time t.
        let local_pos = block_index * n as u64;
        local.lock().unwrap().frame(local_pos, n, &mut local_block);
        local_onsets.push(&local_block);
        remote_onsets.push(&out);
        if info.silent {
            remote_active_since = None;
        } else if remote_active_since.is_none() {
            remote_active_since = Some(t);
        }
        let hop_s = HOP as f64 / SAMPLE_RATE as f64;
        // Onset index of output time t (env[0] corresponds to playout start).
        let ps = playout_start.unwrap();
        let env_idx = |time: i64| ((time - ps) as f64 / 1e6 / hop_s) as usize;
        const WINDOW_US: i64 = 8_000_000;

        // Simulated follower DJ: beatmatch to the remote as heard, then drop on its beat.
        if cfg.follow && follow_drop.is_none() {
            if let Some(since) = remote_active_since {
                if t - since >= WINDOW_US {
                    let a = env_idx(t - WINDOW_US);
                    let renv = &remote_onsets.env[a..];
                    let period = match cfg.bpm {
                        Some(b) => 60.0 / b / hop_s,
                        None => estimate_period(renv, hop_s, 70.0, 180.0).unwrap_or(0.0),
                    };
                    if let Some(ph) = beat_phase(renv, period) {
                        // Remote beats are heard at out-times (t - WINDOW) + ph*hop + n*period.
                        let base = (t - WINDOW_US) as f64 + ph * hop_s * 1e6;
                        let per_us = period * hop_s * 1e6;
                        let next = base + ((t as f64 + 200_000.0 - base) / per_us).ceil() * per_us;
                        // Our sample pos heard at time `next`. Program's first kick is at its sample 0.
                        let pos = (next - start_local as f64)
                            * (1.0 + cfg.skew_ppm * 1e-6)
                            * SAMPLE_RATE as f64
                            / 1e6;
                        local.lock().unwrap().drop_pos = Some(pos);
                        follow_drop = Some(FollowDrop {
                            decided_at_out_us: t,
                            period_ms: per_us / 1e3,
                            drop_at_out_us: next as i64,
                            drop_sender_pos: pos,
                        });
                    }
                }
            }
        }

        // Simulated outgoing DJ: fade out once the incoming DJ has been on for a while.
        if let (Some(after), Some(since), None) = (
            cfg.fade_out_after_remote_s,
            remote_active_since,
            fade_out_at,
        ) {
            if t - since >= (after * 1e6) as i64 {
                let start = local_pos + n as u64;
                local.lock().unwrap().fade = Some((start, 4 * SAMPLE_RATE as u64));
                fade_out_at = Some(t);
            }
        }

        // Beat-quantized monitoring.
        if cfg.monitor == MonitorMode::Beat {
            let local_has_audio = fade_out_at.is_none()
                && local
                    .lock()
                    .unwrap()
                    .drop_pos
                    .map(|d| (local_pos as f64) > d + 48_000.0 * 8.0)
                    .unwrap_or(false);
            if let (Some(since), true) = (remote_active_since, local_has_audio) {
                let due = next_beat_check
                    .map(|c| t >= c)
                    .unwrap_or(t - since >= WINDOW_US);
                if due {
                    let a = env_idx(t - WINDOW_US);
                    let lenv = &local_onsets.env[a..];
                    let renv = &remote_onsets.env[a..remote_onsets.env.len().min(a + lenv.len())];
                    let period = match cfg.bpm {
                        Some(b) => Some(60.0 / b / hop_s),
                        None => estimate_period(lenv, hop_s, 70.0, 180.0),
                    };
                    if let Some(period) = period {
                        if let Some((lag, conf)) = phase_lag(lenv, renv, period) {
                            let signed = if lag > period / 2.0 {
                                lag - period
                            } else {
                                lag
                            };
                            let lag_ms = signed * hop_s * 1e3;
                            let per_ms = period * hop_s * 1e3;
                            let mut applied = 0.0;
                            if lag_ms.abs() > 3.0 && conf > 1.5 {
                                // First lock: delay to the next beat. Re-lock after a path
                                // change: nudge by the residual (either way), never add a beat.
                                applied = if quant_total_ms >= lag_ms && quant_total_ms > 0.0 {
                                    -lag_ms
                                } else {
                                    quantize_extra(lag, period) * hop_s * 1e3
                                };
                                quant_total_ms += applied;
                                pb.lock().unwrap().add_delay(
                                    applied * 1e3,
                                    t,
                                    "beat-quantized monitoring",
                                );
                            }
                            beat_events.push(BeatEvent {
                                at_out_us: t,
                                period_ms: per_ms,
                                lag_ms,
                                confidence: conf,
                                applied_extra_ms: applied,
                            });
                        }
                    }
                    next_beat_check = Some(t + WINDOW_US);
                }
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    tx.join().unwrap()?;
    rx.join().unwrap()?;
    drop(wtx);
    writer.join().unwrap()?;

    let pbl = pb.lock().unwrap();
    let syncl = sync.lock().unwrap();
    let mut rtts: Vec<f64> = syncl.all_rtts.iter().map(|&r| r as f64 / 1e3).collect();
    rtts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let delay_final_ms = final_delays.last().map(|&(t_end, _)| {
        let mut v: Vec<f64> = final_delays
            .iter()
            .filter(|(t, _)| t_end - t <= 10_000_000)
            .map(|x| x.1 / 1e3)
            .collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        percentile(&v, 50.0)
    });
    let delay_est = if delays.is_empty() {
        None
    } else {
        let mut d: Vec<f64> = delays.iter().map(|v| v / 1e3).collect();
        let mean = d.iter().sum::<f64>() / d.len() as f64;
        let std = (d.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / d.len() as f64).sqrt();
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        Some(DelaySummary {
            blocks: d.len(),
            min_ms: d[0],
            p50_ms: percentile(&d, 50.0),
            max_ms: *d.last().unwrap(),
            std_ms: std,
        })
    };
    let secs = cfg.duration_s;
    let report = PeerReport {
        name: cfg.name.clone(),
        start_at_us: cfg.start_at_us,
        clock_offset_param_us: cfg.clock_offset_us,
        skew_ppm: cfg.skew_ppm,
        monitor: cfg.monitor,
        codec: format!("{:?}", cfg.codec.codec),
        bitrate: cfg.codec.bitrate,
        frame_samples: cfg.codec.frame_samples,
        redundancy: cfg.redundancy.clone(),
        encoder_lookahead_samples: lookahead,
        playout_start_out_us: playout_start,
        network_test_jitter_ms: net_jitter,
        booth_margin_ms: pbl.margin_us() / 1e3,
        clock_offset_est_us: syncl.offset_us(),
        min_rtt_ms: syncl.min_rtt_us().map(|v| v as f64 / 1e3),
        rtt_p50_ms: percentile(&rtts, 50.0),
        rtt_p99_ms: percentile(&rtts, 99.0),
        packets_sent: sent_packets.load(Ordering::Relaxed),
        bytes_sent: sent_bytes.load(Ordering::Relaxed),
        send_kbps: sent_bytes.load(Ordering::Relaxed) as f64 * 8.0 / secs / 1e3,
        playout: pbl.stats.clone(),
        delay_est,
        delay_final_ms,
        monitor_offset_samples,
        beat_events,
        follow_drop,
        fade_out_at_out_us: fade_out_at,
        cpu_seconds: cpu_seconds(),
    };
    std::fs::write(
        cfg.out_dir.join("report.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    Ok(report)
}

/// Read a WAV as interleaved stereo f32 at 48 kHz (mono is duplicated).
pub fn read_wav(path: &std::path::Path) -> Result<Vec<f32>> {
    let mut r = hound::WavReader::open(path).with_context(|| format!("open {}", path.display()))?;
    let spec = r.spec();
    anyhow::ensure!(
        spec.sample_rate == SAMPLE_RATE,
        "{} must be 48 kHz (got {})",
        path.display(),
        spec.sample_rate
    );
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()?
        }
    };
    Ok(match spec.channels {
        1 => samples.iter().flat_map(|&s| [s, s]).collect(),
        2 => samples,
        c => samples
            .chunks_exact(c as usize)
            .flat_map(|f| [f[0], f[1]])
            .collect(),
    })
}

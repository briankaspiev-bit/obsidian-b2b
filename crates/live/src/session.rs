//! A live booth session on real (or simulated) sound cards.
//!
//! One media thread runs on the system clock every 5 ms, exactly like the bench:
//! it takes this DJ's next 5 ms (from the deck, or from captured system audio),
//! applies the fader, encodes and sends it; renders the partner's 5 ms from the
//! playout buffer; mixes what this DJ hears into the headphone FIFO; and runs the
//! booth logic (TAKE OVER, beat-quantized monitoring for whoever is on air, deck
//! SYNC for whoever is coming in, the ghost DJ's moves). Sound cards sit behind
//! clock-bridging FIFOs ([`obsidian_audio_io`]), so they never move the booth delay.

use crate::deck::{refine_period, Deck, DeckStatus, HOP};
use crate::ghost::{Ghost, GhostPlan};
use crate::scope::{track_wave, BandSplit, Bands, BeatClock, Column, Scope, ScopeChunk};
use anyhow::Result;
use obsidian_align::{beat_phase, estimate_period, phase_lag, quantize_extra, OnsetTracker};
use obsidian_audio_io::{AdaptiveResampler, AudioFifo};
use obsidian_clock::{sleep_until, ClockSync, SessionClock};
use obsidian_codec::{CodecConfig, Encoder, SAMPLE_RATE};
use obsidian_jitter::{PlayoutBuffer, PlayoutConfig};
use obsidian_protocol::{Frame, MediaPacket, Packet};
use serde::Serialize;
use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub enum LiveSource {
    /// A music file on the built-in deck (play, pitch, nudge, SYNC).
    Deck {
        title: String,
        program: Arc<Vec<f32>>,
    },
    /// Captured audio (system audio or an input), stereo at `rate`.
    Capture { fifo: Arc<AudioFifo>, rate: u32 },
}

pub struct LiveConfig {
    pub name: String,
    /// Where to send. `None` = wait for the partner to reach us first. Either way the
    /// session follows the address the partner's packets actually come from.
    pub peer: Option<SocketAddr>,
    pub codec: CodecConfig,
    pub redundancy: Vec<u64>,
    pub playout: PlayoutConfig,
    pub source: LiveSource,
    /// Headphones: 48 kHz stereo blocks are pushed here for the device to pull.
    pub sink: Option<Arc<AudioFifo>>,
    /// How long a block waits in the sink before it is heard (ms). With captured
    /// system audio, the DJ hears their own music directly but the partner through
    /// this FIFO, so beat checks compensate by this much.
    pub sink_latency_ms: f64,
    /// Write sent.wav / monitor.wav / report.json here.
    pub record_dir: Option<PathBuf>,
    pub start_on_air: bool,
    pub ghost: Option<GhostPlan>,
    /// Deck starts playing right away.
    pub autoplay: bool,
}

impl LiveConfig {
    pub fn new(name: &str, source: LiveSource) -> Self {
        LiveConfig {
            name: name.into(),
            peer: None,
            codec: CodecConfig::default(),
            redundancy: vec![1, 3],
            playout: PlayoutConfig::default(),
            source,
            sink: None,
            sink_latency_ms: 0.0,
            record_dir: None,
            start_on_air: false,
            ghost: None,
            autoplay: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cmd {
    TakeOver,
    DeckPlayPause,
    DeckCue,
    SyncToggle,
    /// Shift the deck's beat by this many ms (positive = earlier).
    Nudge(f64),
    /// Jump the deck by whole beats (negative = back).
    BeatJump(i32),
    /// Change the deck's speed by this many percent.
    Pitch(f64),
    /// Solo practice: bring the ghost DJ back in now.
    GhostComeBack,
    /// Coming in: cued and ready to take over (shown on the partner's screen).
    /// Cleared automatically on TAKE OVER.
    SetReady(bool),
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LiveStatus {
    /// "waiting for partner", "measuring the network", "live", "partner left".
    pub phase: String,
    pub partner_name: Option<String>,
    pub partner_addr: Option<String>,
    pub on_air: bool,
    pub partner_on_air: bool,
    pub ready: bool,
    pub partner_ready: bool,
    /// How late the partner reaches your ears: one-way network + buffer (ms, clock-sync estimate).
    pub booth_delay_ms: Option<f64>,
    pub rtt_ms: Option<f64>,
    /// Safety buffer the playout holds for jitter and loss recovery.
    pub margin_ms: Option<f64>,
    /// Extra delay added so the partner lands on your beat (on air only).
    pub beat_align_ms: f64,
    /// Wi-Fi shield: you asked the partner for wide redundancy (your incoming link drops
    /// bursts), and the partner asked you (yours outgoing does).
    pub shield: bool,
    pub partner_shield: bool,
    /// Over the last 10 s: frames that needed recovery, and how many were patched over (audible).
    pub recovered_10s: u64,
    pub concealed_10s: u64,
    pub concealed_total: u64,
    pub late_total: u64,
    pub reanchors: usize,
    pub fader: f32,
    pub partner_volume: f32,
    pub local_peak: f32,
    pub partner_peak: f32,
    pub partner_bpm: Option<f64>,
    pub deck: Option<DeckStatus>,
    /// Phase meter: where both beats are, as you hear them.
    pub beats: Option<BeatView>,
    /// Peak level every 10 ms over the last 3 s, newest last, as heard now (for scrolling
    /// waveforms). Same timeline for both, so kicks that line up sit at the same index.
    pub wave_you: Vec<f32>,
    pub wave_partner: Vec<f32>,
    pub ghost: Option<String>,
    pub send_kbps: f64,
    pub capture_underruns: u64,
    pub uptime_s: f64,
    /// Output clock at this status (ms since the session started; see [`crate::scope`]).
    pub now_ms: f64,
    /// Where your beats and the partner's beats fall (as you hear them), when there's a beat.
    pub you_beat: Option<BeatClock>,
    pub partner_beat: Option<BeatClock>,
    /// Newest last, "mm:ss text".
    pub events: Vec<String>,
}

/// The phase meter: both DJs' place in the bar as heard in the headphones.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BeatView {
    /// The partner's place in their bar: 0.0 = beat 1 ... 3.99.
    pub partner: f64,
    /// Your deck's place in its bar (no deck: None).
    pub you: Option<f64>,
    /// How far your beats run ahead of the partner's, in beats (negative = behind).
    /// Whole beats count only when `bars_known`; otherwise it is within ±0.5.
    pub ahead_beats: Option<f64>,
    pub ahead_ms: Option<f64>,
    /// Beat 1 is a real guess from both tracks' clap patterns (else just the nearest beat).
    pub bars_known: bool,
    pub beat_ms: f64,
}

/// The partner's beat grid as heard, refreshed every second.
#[derive(Clone, Copy)]
struct PartnerGrid {
    /// Output time of one of the partner's beats, and the beat length (µs).
    base_us: f64,
    per_us: f64,
    /// Which beat (counted from `base_us`) is a beat 1.
    down_k: f64,
    bars_known: bool,
}

impl PartnerGrid {
    fn pos(&self, t_out: f64) -> f64 {
        ((t_out - self.base_us) / self.per_us - self.down_k).rem_euclid(4.0)
    }
}

/// Peak level per 10 ms, last 3 s.
struct PeakWave {
    cur: f32,
    n: usize,
    v: VecDeque<f32>,
}

impl PeakWave {
    fn new() -> Self {
        PeakWave {
            cur: 0.0,
            n: 0,
            v: VecDeque::from(vec![0.0; 300]),
        }
    }
    fn push(&mut self, stereo: &[f32]) {
        for fr in stereo.chunks_exact(2) {
            self.cur = self.cur.max(fr[0].abs()).max(fr[1].abs());
            self.n += 1;
            if self.n == 480 {
                self.v.pop_front();
                self.v.push_back(self.cur);
                self.cur = 0.0;
                self.n = 0;
            }
        }
    }
}

pub struct LiveControls {
    fader: AtomicU32,
    partner_volume: AtomicU32,
    stop: AtomicBool,
    cmds: Mutex<Vec<Cmd>>,
    pub status: Mutex<LiveStatus>,
    scope: Mutex<Scope>,
    deck_wave: Mutex<Option<Arc<Vec<Bands>>>>,
}

impl Default for LiveControls {
    fn default() -> Self {
        LiveControls {
            fader: AtomicU32::new(1f32.to_bits()),
            partner_volume: AtomicU32::new(1f32.to_bits()),
            stop: AtomicBool::new(false),
            cmds: Mutex::new(Vec::new()),
            status: Mutex::new(LiveStatus::default()),
            scope: Mutex::new(Scope::default()),
            deck_wave: Mutex::new(None),
        }
    }
}

impl LiveControls {
    pub fn send(&self, c: Cmd) {
        self.cmds.lock().unwrap().push(c);
    }
    /// This DJ's channel volume, 0..1: what the partner (and the room) gets.
    pub fn set_fader(&self, v: f32) {
        self.fader
            .store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub fn fader(&self) -> f32 {
        f32::from_bits(self.fader.load(Ordering::Relaxed))
    }
    /// How loud the partner is in this DJ's headphones, 0..2.
    pub fn set_partner_volume(&self, v: f32) {
        self.partner_volume
            .store(v.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
    }
    pub fn partner_volume(&self) -> f32 {
        f32::from_bits(self.partner_volume.load(Ordering::Relaxed))
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
    pub fn status(&self) -> LiveStatus {
        self.status.lock().unwrap().clone()
    }
    /// Waveform columns from index `from` on (pass the previous chunk's end to stream).
    pub fn scope_since(&self, from: u64) -> ScopeChunk {
        self.scope.lock().unwrap().since(from)
    }
    /// The deck's whole track, 5 ms per column at its own speed (for the view ahead).
    pub fn deck_wave(&self) -> Option<Arc<Vec<Bands>>> {
        self.deck_wave.lock().unwrap().clone()
    }
}

/// Shared between the receiver and the media thread.
/// Redundancy for a lossy link (bad Wi-Fi): copies reach 16 frames (80 ms) back,
/// so a dropout that long is rebuilt instead of patched over.
const SHIELD: [u64; 6] = [1, 2, 4, 7, 11, 16];
/// Turn the shield on after this many dropouts too long for the normal copies within
/// `SHIELD_WINDOW_US`; off after `SHIELD_OFF_US` without one.
const SHIELD_BURSTS: usize = 2;
const SHIELD_WINDOW_US: i64 = 30_000_000;
const SHIELD_OFF_US: i64 = 120_000_000;

struct Remote {
    pb: PlayoutBuffer,
    /// Highest media seq seen on this stream, and when runs of lost packets longer
    /// than the normal redundancy covers happened (output clock, µs).
    last_seq: Option<u32>,
    long_gaps: VecDeque<i64>,
    /// The partner asks us to send shielded (wide) redundancy.
    partner_shield: bool,
    stream_id: Option<u32>,
    /// Bumped whenever the partner's stream (re)starts.
    generation: u64,
    addr: Option<SocketAddr>,
    last_packet_us: Option<i64>,
    state: Option<(u32, bool, u32, String)>,
    partner_ready: bool,
    left: bool,
}

/// Holds "active" through short gaps (breakdowns, a quiet intro).
struct Activity {
    since: Option<i64>,
    last_loud: i64,
}
impl Activity {
    fn new() -> Self {
        Activity {
            since: None,
            last_loud: i64::MIN / 2,
        }
    }
    fn update(&mut self, t: i64, loud: bool) {
        if loud {
            self.last_loud = t;
            self.since.get_or_insert(t);
        } else if t - self.last_loud > 2_000_000 {
            self.since = None;
        }
    }
    fn for_us(&self, t: i64) -> i64 {
        self.since.map(|s| t - s).unwrap_or(0)
    }
}

fn peak(v: &[f32]) -> f32 {
    v.iter().fold(0f32, |m, x| m.max(x.abs()))
}

fn rms(v: &[f32]) -> f32 {
    (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
}

#[derive(Serialize)]
struct LiveReport<'a> {
    name: &'a str,
    seconds: f64,
    final_status: &'a LiveStatus,
    playout: &'a obsidian_jitter::PlayoutStats,
    /// sent.wav and monitor.wav start on the same block, so this is 0.
    monitor_offset_samples: u64,
}

enum DiskMsg {
    Block(Vec<f32>, Vec<f32>),
}

/// Run until `ctl.stop()`. `sock` may come from the room-code client (already punched).
pub fn run_live(
    mut cfg: LiveConfig,
    sock: UdpSocket,
    ctl: Arc<LiveControls>,
) -> Result<LiveStatus> {
    let clock = SessionClock::new(0);
    let fs = cfg.codec.frame_samples;
    let frame_us = fs as i64 * 1_000_000 / SAMPLE_RATE as i64;
    let mut pcfg = cfg.playout.clone();
    pcfg.recovery_us = cfg.redundancy.iter().max().copied().unwrap_or(0) as i64 * frame_us;
    pcfg.output_block_us = frame_us;
    let tiebreak: u32 = rand::random();
    let stream_id: u32 = rand::random::<u32>() | 1;
    sock.set_read_timeout(Some(Duration::from_millis(50)))?;
    let remote = Arc::new(Mutex::new(Remote {
        pb: PlayoutBuffer::new(pcfg.clone(), cfg.codec.clone())?,
        last_seq: None,
        long_gaps: VecDeque::new(),
        partner_shield: false,
        stream_id: None,
        generation: 0,
        addr: cfg.peer,
        last_packet_us: None,
        state: None,
        partner_ready: false,
        left: false,
    }));
    let sync = Arc::new(Mutex::new(ClockSync::new(64)));
    let stop_rx = Arc::new(AtomicBool::new(false));
    let bytes_sent = Arc::new(AtomicU64::new(0));

    // Drop anything that queued up on the socket before we were ready: a burst of
    // stale packets would read as huge jitter and inflate the booth buffer.
    sock.set_nonblocking(true)?;
    let mut scratch = [0u8; 2048];
    while sock.recv_from(&mut scratch).is_ok() {}
    sock.set_nonblocking(false)?;
    sock.set_read_timeout(Some(Duration::from_millis(50)))?;

    // ---------------- receiver ----------------
    let rx = {
        let sock = sock.try_clone()?;
        let remote = remote.clone();
        let sync = sync.clone();
        let stop = stop_rx.clone();
        let (pcfg, codec) = (pcfg.clone(), cfg.codec.clone());
        let normal_cover = cfg.redundancy.iter().max().copied().unwrap_or(0) as u32;
        std::thread::Builder::new()
            .name("live-rx".into())
            .spawn(move || -> Result<()> {
                let mut buf = vec![0u8; 65_536];
                let mut out = Vec::with_capacity(64);
                while !stop.load(Ordering::Relaxed) {
                    let Ok((n, from)) = sock.recv_from(&mut buf) else {
                        continue; // timeouts, and Windows' "connection reset" when the partner isn't up yet
                    };
                    let now = clock.now_us();
                    let Ok(pkt) = Packet::decode(&buf[..n]) else {
                        continue;
                    };
                    let mut r = remote.lock().unwrap();
                    r.addr = Some(from);
                    r.last_packet_us = Some(now);
                    match pkt {
                        Packet::Media(m) => {
                            if r.stream_id != Some(m.stream_id) {
                                r.pb = PlayoutBuffer::new(pcfg.clone(), codec.clone())?;
                                r.stream_id = Some(m.stream_id);
                                r.generation += 1;
                                r.left = false;
                                r.last_seq = None;
                            }
                            if let Some(l) = r.last_seq {
                                let gap = m.seq.wrapping_sub(l).wrapping_sub(1);
                                // Lost in a row (ignore reordering and wraparound).
                                if gap > normal_cover && gap < 10_000 {
                                    r.long_gaps.push_back(now);
                                }
                            }
                            if r.last_seq
                                .map(|l| m.seq.wrapping_sub(l) < 1 << 31)
                                .unwrap_or(true)
                            {
                                r.last_seq = Some(m.seq);
                            }
                            r.pb.push(&m, now);
                        }
                        Packet::Ping { id, t0_us } => {
                            drop(r);
                            Packet::Pong {
                                id,
                                t0_us,
                                t1_us: now,
                                t2_us: clock.now_us(),
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
                            drop(r);
                            sync.lock().unwrap().add(t0_us, t1_us, t2_us, now);
                        }
                        Packet::State {
                            epoch,
                            on_air,
                            ready,
                            shield,
                            tiebreak,
                            name,
                        } => {
                            r.partner_ready = ready;
                            r.partner_shield = shield;
                            r.state = Some((epoch, on_air, tiebreak, name));
                        }
                        Packet::Bye => {
                            r.left = true;
                            r.stream_id = None;
                            r.state = None;
                        }
                    }
                }
                Ok(())
            })?
    };

    // ---------------- recordings ----------------
    let (wtx, writer) = match &cfg.record_dir {
        Some(dir) => {
            std::fs::create_dir_all(dir)?;
            let dir = dir.clone();
            let (tx, rxm) = std::sync::mpsc::channel::<DiskMsg>();
            let h = std::thread::spawn(move || -> Result<()> {
                let spec = hound::WavSpec {
                    channels: 2,
                    sample_rate: SAMPLE_RATE,
                    bits_per_sample: 32,
                    sample_format: hound::SampleFormat::Float,
                };
                let mut sent = hound::WavWriter::create(dir.join("sent.wav"), spec)?;
                let mut mon = hound::WavWriter::create(dir.join("monitor.wav"), spec)?;
                for DiskMsg::Block(a, b) in rxm {
                    a.iter().try_for_each(|&x| sent.write_sample(x))?;
                    b.iter().try_for_each(|&x| mon.write_sample(x))?;
                }
                sent.finalize()?;
                mon.finalize()?;
                Ok(())
            });
            (Some(tx), Some(h))
        }
        None => (None, None),
    };

    // ---------------- media thread state ----------------
    let mut encoder = Encoder::new(&cfg.codec)?;
    let mut history: VecDeque<Frame> = VecDeque::new();
    let max_red = cfg.redundancy.iter().max().copied().unwrap_or(0) as usize;
    let (mut deck, mut capture) = match std::mem::replace(
        &mut cfg.source,
        LiveSource::Capture {
            fifo: Arc::new(AudioFifo::new(1)),
            rate: 48_000,
        },
    ) {
        LiveSource::Deck { title, program } => {
            let mut d = Deck::new(title, program);
            d.playing = cfg.autoplay;
            (Some(d), None)
        }
        LiveSource::Capture { fifo, rate } => (
            None,
            Some((
                fifo.clone(),
                AdaptiveResampler::new(rate, SAMPLE_RATE, 20 * 48),
            )),
        ),
    };
    if let Some(d) = &deck {
        *ctl.deck_wave.lock().unwrap() = Some(Arc::new(track_wave(&d.program)));
    }
    let mut ghost = cfg.ghost.take().map(Ghost::new);
    let capture_shift_hops = if capture.is_some() {
        (cfg.sink_latency_ms * 48_000.0 / 1e3 / HOP as f64).round() as usize
    } else {
        0
    };

    let mut on_air = cfg.start_on_air;
    let mut epoch: u32 = on_air as u32;
    let mut state_burst = 3u32;
    let mut ready = false;
    let mut events: Vec<String> = Vec::new();
    let start = clock.now_us() + 20_000;
    let ev = |events: &mut Vec<String>, t: i64, s: String| {
        let secs = ((t - start).max(0) / 1_000_000) as i64;
        events.push(format!("{:02}:{:02} {s}", secs / 60, secs % 60));
        if events.len() > 50 {
            events.remove(0);
        }
    };
    if on_air {
        ev(&mut events, start, "You are on air".into());
    }

    let mut local = Vec::with_capacity(fs * 2);
    let mut remote_out = vec![0f32; fs * 2];
    let mut mix = Vec::with_capacity(fs * 2);
    let mut buf = Vec::with_capacity(2048);
    let mut local_on = OnsetTracker::new(HOP);
    let mut remote_on = OnsetTracker::new(HOP);
    let mut remote_hf = crate::deck::HfTracker::default();
    let mut local_act = Activity::new();
    let mut remote_act = Activity::new();
    let mut seen_gen = 0u64;
    let mut playing_gen: Option<u64> = None;
    let mut quant_total_ms = 0f64;
    let mut next_beat_check: Option<i64> = None;
    let mut next_sync: i64 = 0;
    let mut partner_bpm: Option<f64> = None;
    let mut partner_beat: Option<BeatClock> = None;
    let mut you_heard_beat: Option<BeatClock> = None;
    let (mut you_bands, mut partner_bands) = (BandSplit::default(), BandSplit::default());
    let mut pgrid: Option<PartnerGrid> = None;
    let mut want_shield = false;
    let mut next_shield_check = start;
    let mut wave_you = PeakWave::new();
    let mut wave_partner = PeakWave::new();
    let mut next_ping = start;
    let mut next_state = start;
    let mut ping_id = 0u32;
    let mut recent: VecDeque<(i64, u64, u64)> = VecDeque::new(); // (t, recovered, concealed) cumulative
    let mut seen_reanchors = 0usize;
    let mut partner_name: Option<String> = None;
    let mut partner_on_air = false;
    let mut last_status = 0i64;
    let mut local_peak = 0f32;
    let mut remote_peak = 0f32;
    let hop_s = HOP as f64 / SAMPLE_RATE as f64;
    const WINDOW_US: i64 = 8_000_000;
    let mut j: i64 = 0;

    while !ctl.stopped() {
        let t = start + j * frame_us;
        sleep_until(clock.instant_at(t));
        j += 1;
        let fader = ctl.fader();
        let pvol = ctl.partner_volume();
        let mut take_over_now = false;

        // ---- commands ----
        for c in std::mem::take(&mut *ctl.cmds.lock().unwrap()) {
            match c {
                Cmd::TakeOver => take_over_now = true,
                Cmd::SetReady(v) => {
                    if ready != v {
                        ready = v;
                        state_burst = 3;
                    }
                }
                Cmd::GhostComeBack => {
                    if let Some(g) = &mut ghost {
                        g.come_back = true;
                    }
                }
                c => {
                    if let Some(d) = &mut deck {
                        match c {
                            Cmd::DeckPlayPause => d.playing = !d.playing,
                            // Back to the first beat; keeps playing if it was.
                            Cmd::DeckCue => d.cue(),
                            Cmd::SyncToggle => {
                                d.sync = !d.sync;
                                d.sync_locked = false;
                                d.sync_err_ms = None;
                                next_sync = t;
                            }
                            Cmd::Nudge(ms) => d.nudge(ms, 0.25),
                            Cmd::BeatJump(n) => d.beat_jump(n),
                            Cmd::Pitch(p) => d.pitch = (d.pitch + p / 100.0).clamp(0.85, 1.15),
                            _ => {}
                        }
                    }
                }
            }
        }

        // ---- this DJ's next 5 ms ----
        if let Some(d) = &mut deck {
            d.render(fs, &mut local);
        } else if let Some((fifo, rs)) = &mut capture {
            local.resize(fs * 2, 0.0);
            rs.pull(fifo, &mut local);
        }
        for x in local.iter_mut() {
            *x *= fader;
        }
        local_peak = local_peak.max(peak(&local));

        let (peer, gen, rstate, left, last_pkt, partner_ready) = {
            let r = remote.lock().unwrap();
            (
                r.addr,
                r.generation,
                r.state.clone(),
                r.left,
                r.last_packet_us,
                r.partner_ready,
            )
        };
        if let Some(peer) = peer {
            let payload = encoder.encode(&local)?;
            let k = (j - 1) as u64;
            let primary = Frame {
                index: k,
                capture_us: t,
                payload,
            };
            let shielded = remote.lock().unwrap().partner_shield;
            let offsets: &[u64] = if shielded { &SHIELD } else { &cfg.redundancy };
            let redundant = offsets
                .iter()
                .filter_map(|&o| history.iter().rev().find(|f| f.index + o == k).cloned())
                .collect();
            Packet::Media(MediaPacket {
                stream_id,
                seq: k as u32,
                codec: cfg.codec.codec,
                frame_samples: fs as u16,
                primary: primary.clone(),
                redundant,
            })
            .encode(&mut buf);
            if sock.send_to(&buf, peer).is_ok() {
                bytes_sent.fetch_add(buf.len() as u64 + 28, Ordering::Relaxed);
            }
            history.push_back(primary);
            while history.len() > max_red.max(SHIELD[SHIELD.len() - 1] as usize) {
                history.pop_front();
            }
            if t >= next_ping {
                Packet::Ping {
                    id: ping_id,
                    t0_us: clock.now_us(),
                }
                .encode(&mut buf);
                let _ = sock.send_to(&buf, peer);
                ping_id += 1;
                next_ping = t + 200_000;
            }
        }

        // ---- handoff state ----
        if let Some((p_epoch, p_on_air, p_tb, name)) = rstate {
            if partner_name.as_deref() != Some(name.as_str()) {
                ev(&mut events, t, format!("Connected to {name}"));
                partner_name = Some(name);
            }
            if p_on_air && on_air && (p_epoch > epoch || (p_epoch == epoch && p_tb < tiebreak)) {
                on_air = false;
                epoch = p_epoch;
                state_burst = 3;
                ev(
                    &mut events,
                    t,
                    format!(
                        "{} took over, you're off air",
                        partner_name.as_deref().unwrap_or("Partner")
                    ),
                );
            } else if p_epoch > epoch {
                epoch = p_epoch;
            }
            if p_on_air != partner_on_air {
                partner_on_air = p_on_air;
                if p_on_air && !on_air && p_epoch >= epoch {
                    ev(
                        &mut events,
                        t,
                        format!("{} is on air", partner_name.as_deref().unwrap_or("Partner")),
                    );
                }
            }
        }
        if let Some(g) = &mut ghost {
            let d = deck.as_mut().expect("the ghost plays a deck");
            if g.step(t, d, on_air, remote_act.for_us(t), &mut events, start) {
                take_over_now = true;
            }
        }
        if take_over_now && !on_air {
            epoch = epoch.max(rstate_epoch(&remote)) + 1;
            on_air = true;
            ready = false;
            state_burst = 3;
            next_beat_check = Some(t + 2_000_000);
            ev(&mut events, t, "You took over: you're on air".into());
        }
        if let Some(peer) = peer {
            if state_burst > 0 || t >= next_state {
                Packet::State {
                    epoch,
                    on_air,
                    ready,
                    shield: want_shield,
                    tiebreak,
                    name: cfg.name.clone(),
                }
                .encode(&mut buf);
                let _ = sock.send_to(&buf, peer);
                state_burst = state_burst.saturating_sub(1);
                next_state = t + 250_000;
            }
        }

        // ---- partner's 5 ms ----
        if gen != seen_gen {
            seen_gen = gen;
            playing_gen = None;
            quant_total_ms = 0.0;
            next_beat_check = None;
            ev(
                &mut events,
                t,
                "Partner stream started, measuring the network (10 s)".into(),
            );
        }
        let mut info = None;
        {
            let mut r = remote.lock().unwrap();
            if playing_gen != Some(gen) && r.stream_id.is_some() && r.pb.ready_to_start() {
                r.pb.start(t, 0.0);
                playing_gen = Some(gen);
                ev(
                    &mut events,
                    t,
                    format!("Hearing partner, buffer {:.0} ms", r.pb.margin_us() / 1e3),
                );
            }
            if playing_gen == Some(gen) && r.stream_id.is_some() {
                info = Some(r.pb.render(t, fs, &mut remote_out)?);
                while r.pb.stats.reanchors.len() > seen_reanchors {
                    let e = &r.pb.stats.reanchors[seen_reanchors];
                    if e.reason != "beat-quantized monitoring" && e.reason != "wifi shield" {
                        ev(
                            &mut events,
                            t,
                            format!(
                                "Network got worse: delay {:+.0} ms ({})",
                                e.delta_us as f64 / 1e3,
                                e.method
                            ),
                        );
                    }
                    seen_reanchors += 1;
                }
            } else {
                remote_out.iter_mut().for_each(|x| *x = 0.0);
                seen_reanchors = r.pb.stats.reanchors.len();
            }
        }
        remote_peak = remote_peak.max(peak(&remote_out));
        ctl.scope.lock().unwrap().push(Column {
            you: you_bands.column(&local),
            partner: partner_bands.column(&remote_out),
        });

        // ---- headphones ----
        if let Some(sink) = &cfg.sink {
            mix.clear();
            let with_local = deck.is_some();
            for i in 0..fs * 2 {
                let v = remote_out[i] * pvol + if with_local { local[i] } else { 0.0 };
                mix.push(v);
            }
            sink.push(&mix);
        }
        if let Some(w) = &wtx {
            let _ = w.send(DiskMsg::Block(local.clone(), remote_out.clone()));
        }

        // ---- beat tools ----
        local_on.push(&local);
        remote_on.push(&remote_out);
        remote_hf.push(&remote_out);
        wave_you.push(&local);
        wave_partner.push(&remote_out);
        local_act.update(t, rms(&local) > 0.003);
        remote_act.update(t, rms(&remote_out) > 0.003);
        let end_idx = remote_on.env.len().min(local_on.env.len());
        let win = (WINDOW_US / 1000) as usize;

        // On air: delay the partner just enough to land on your beat.
        if on_air
            && local_act.for_us(t) >= WINDOW_US
            && remote_act.for_us(t) >= WINDOW_US
            && playing_gen.is_some()
            && end_idx > win + capture_shift_hops
        {
            let due = next_beat_check.map(|c| t >= c).unwrap_or(true);
            if due {
                let a = end_idx - win;
                // Captured audio is heard now; the partner is heard after the sink FIFO.
                let lenv = &local_on.env[a..end_idx];
                let renv = &remote_on.env[a - capture_shift_hops..end_idx - capture_shift_hops];
                if let Some(period) = estimate_period(lenv, hop_s, 70.0, 180.0) {
                    if let (Some((_, conf)), Some(pl), Some(pr)) = (
                        phase_lag(lenv, renv, period),
                        beat_phase(lenv, period),
                        beat_phase(renv, period),
                    ) {
                        // Compare where the kicks land (folded envelope peaks): cross-correlating
                        // two different tracks' envelopes is biased by their shapes (tens of ms).
                        let lag = (pr - pl).rem_euclid(period);
                        let signed = if lag > period / 2.0 {
                            lag - period
                        } else {
                            lag
                        };
                        let lag_ms = signed * hop_s * 1e3;
                        if lag_ms.abs() > 3.0 && conf > 1.5 {
                            let applied = if quant_total_ms >= lag_ms && quant_total_ms > 0.0 {
                                -lag_ms
                            } else {
                                quantize_extra(lag, period) * hop_s * 1e3
                            };
                            quant_total_ms += applied;
                            remote.lock().unwrap().pb.add_delay(
                                applied * 1e3,
                                t,
                                "beat-quantized monitoring",
                            );
                            ev(
                                &mut events,
                                t,
                                format!("Partner moved {applied:+.0} ms to land on your beat"),
                            );
                        }
                    }
                }
                next_beat_check = Some(t + WINDOW_US);
            }
        }

        // Partner tempo (for the screen) and deck SYNC while coming in.
        // Wi-Fi shield: dropouts longer than the normal copies cover ask the partner
        // for wider redundancy, and the buffer waits long enough for those copies.
        if t >= next_shield_check {
            next_shield_check = t + 1_000_000;
            let mut r = remote.lock().unwrap();
            while r
                .long_gaps
                .front()
                .map(|&g| t - g > SHIELD_OFF_US)
                .unwrap_or(false)
            {
                r.long_gaps.pop_front();
            }
            let recent = r
                .long_gaps
                .iter()
                .filter(|&&g| t - g <= SHIELD_WINDOW_US)
                .count();
            let was = want_shield;
            if !want_shield && recent >= SHIELD_BURSTS {
                want_shield = true;
            } else if want_shield && r.long_gaps.is_empty() {
                want_shield = false;
            }
            let base = cfg.redundancy.iter().max().copied().unwrap_or(0) as i64 * frame_us;
            let need = if want_shield {
                SHIELD[SHIELD.len() - 1] as i64 * frame_us
            } else {
                base
            };
            if r.pb.recovery_us() != need {
                r.pb.set_recovery(need, t, "wifi shield");
            }
            drop(r);
            if want_shield != was {
                state_burst = 3;
                ev(
                    &mut events,
                    t,
                    if want_shield {
                        format!(
                            "Dropouts on the link: Wi-Fi shield on (+{} ms delay, rebuilds gaps up to 80 ms)",
                            (SHIELD[SHIELD.len() - 1] as i64 * frame_us - base) / 1000
                        )
                    } else {
                        "Link steady again: Wi-Fi shield off".into()
                    },
                );
            }
        }

        if t >= next_sync {
            next_sync = t + 1_000_000;
            let w = 6000usize;
            if remote_act.for_us(t) >= 6_000_000 && remote_on.env.len() > w {
                let renv = &remote_on.env[remote_on.env.len() - w..];
                if let Some(p0) = estimate_period(renv, hop_s, 70.0, 180.0) {
                    let pr = refine_period(renv, p0, 4);
                    partner_bpm = Some(60.0 / (pr * hop_s));
                    partner_beat = heard_beat(renv, pr, t + frame_us - start);
                    if let Some(d) = deck
                        .as_mut()
                        .filter(|d| d.sync && d.playing && !on_air && d.grid.is_some())
                    {
                        let hf = &remote_hf.env[remote_hf.env.len().saturating_sub(w)..];
                        sync_step(d, renv, hf, pr, t + frame_us, &mut events, t, start);
                    }
                    let hf = &remote_hf.env[remote_hf.env.len().saturating_sub(w)..];
                    pgrid = partner_grid(deck.as_ref(), renv, hf, pr, t + frame_us);
                    // The bar guess SYNC uses, as a beat 1 on the screen's clock.
                    if let (Some(b), Some(g)) =
                        (partner_beat.as_mut(), pgrid.filter(|g| g.bars_known))
                    {
                        b.bar_ms = Some((g.base_us + g.down_k * g.per_us - start as f64) / 1e3);
                    }
                }
            } else {
                partner_bpm = None;
                partner_beat = None;
                pgrid = None;
            }
            // Captured music: find your beats the same way, from what was sent.
            if capture.is_some() {
                you_heard_beat = None;
                if local_act.for_us(t) >= 6_000_000 && local_on.env.len() > w {
                    let lenv = &local_on.env[local_on.env.len() - w..];
                    if let Some(p0) = estimate_period(lenv, hop_s, 70.0, 180.0) {
                        let pl = refine_period(lenv, p0, 4);
                        you_heard_beat = heard_beat(lenv, pl, t + frame_us - start);
                    }
                }
            }
        }

        // ---- status ----
        if t - last_status >= 50_000 {
            last_status = t;
            let r = remote.lock().unwrap();
            let s = &r.pb.stats;
            recent.push_back((t, s.frames_from_redundancy + s.frames_fec, s.frames_plc));
            while recent
                .front()
                .map(|f| t - f.0 > 10_000_000)
                .unwrap_or(false)
            {
                recent.pop_front();
            }
            let (r0, c0) = recent.front().map(|f| (f.1, f.2)).unwrap_or((0, 0));
            let off = sync.lock().unwrap().offset_us();
            let rtt = sync.lock().unwrap().min_rtt_us();
            let delay = match (info.and_then(|i| i.capture_tx_us), off) {
                (Some(c), Some(o)) => Some((t - (c - o)) as f64 / 1e3),
                _ => None,
            };
            let lost = last_pkt.map(|l| t - l > 3_000_000).unwrap_or(false);
            let phase = if left {
                "partner left"
            } else if peer.is_none() || last_pkt.is_none() {
                "waiting for partner"
            } else if lost {
                "partner not reachable"
            } else if playing_gen.is_none() {
                "measuring the network"
            } else {
                "live"
            };
            let mut st = ctl.status.lock().unwrap();
            *st = LiveStatus {
                phase: phase.into(),
                partner_name: partner_name.clone(),
                partner_addr: peer.map(|p| p.to_string()),
                on_air,
                partner_on_air,
                ready,
                partner_ready: partner_ready && !partner_on_air,
                booth_delay_ms: delay,
                rtt_ms: rtt.map(|v| v as f64 / 1e3),
                margin_ms: playing_gen.map(|_| r.pb.margin_us() / 1e3),
                beat_align_ms: quant_total_ms,
                shield: want_shield,
                partner_shield: r.partner_shield,
                recovered_10s: s.frames_from_redundancy + s.frames_fec - r0,
                concealed_10s: s.frames_plc - c0,
                concealed_total: s.frames_plc,
                late_total: s.late_packets,
                reanchors: s
                    .reanchors
                    .iter()
                    .filter(|e| {
                        e.reason != "beat-quantized monitoring" && e.reason != "wifi shield"
                    })
                    .count(),
                fader,
                partner_volume: pvol,
                local_peak,
                partner_peak: remote_peak,
                partner_bpm,
                deck: deck.as_ref().map(|d| d.status()),
                beats: pgrid.map(|g| beat_view(&g, deck.as_ref(), (t + frame_us) as f64)),
                wave_you: wave_you.v.iter().copied().collect(),
                wave_partner: wave_partner.v.iter().copied().collect(),
                ghost: ghost.as_ref().map(|g| g.describe()),
                send_kbps: bytes_sent.load(Ordering::Relaxed) as f64 * 8.0
                    / ((t - start).max(1) as f64 / 1e6)
                    / 1e3,
                capture_underruns: capture.as_ref().map(|c| c.1.underruns).unwrap_or(0),
                uptime_s: (t - start) as f64 / 1e6,
                now_ms: (t + frame_us - start) as f64 / 1e3,
                you_beat: match &deck {
                    Some(d) => deck_beat(d, t + frame_us - start),
                    None => you_heard_beat,
                },
                partner_beat,
                events: events.clone(),
            };
            local_peak = 0.0;
            remote_peak = 0.0;
        }
    }

    // ---------------- shut down ----------------
    if let Some(peer) = remote.lock().unwrap().addr {
        Packet::Bye.encode(&mut buf);
        for _ in 0..3 {
            let _ = sock.send_to(&buf, peer);
        }
    }
    stop_rx.store(true, Ordering::Relaxed);
    let _ = rx.join();
    drop(wtx);
    if let Some(h) = writer {
        h.join().unwrap()?;
    }
    let st = ctl.status();
    if let Some(dir) = &cfg.record_dir {
        let r = remote.lock().unwrap();
        let rep = LiveReport {
            name: &cfg.name,
            seconds: st.uptime_s,
            final_status: &st,
            playout: &r.pb.stats,
            monitor_offset_samples: 0,
        };
        std::fs::write(dir.join("report.json"), serde_json::to_string_pretty(&rep)?)?;
    }
    Ok(st)
}

/// Beats in an onset envelope (1 ms hops) that ends at output time `t_end_us`.
fn heard_beat(env: &[f32], period_hops: f64, t_end_us: i64) -> Option<BeatClock> {
    let ph = beat_phase(env, period_hops)?;
    let hop_ms = HOP as f64 / SAMPLE_RATE as f64 * 1e3;
    let t0_ms = t_end_us as f64 / 1e3 - env.len() as f64 * hop_ms;
    Some(BeatClock {
        period_ms: period_hops * hop_ms,
        beat_ms: t0_ms + ph * hop_ms,
        bar_ms: None,
    })
}

/// The deck's beats from its own grid; bars counted from the track's first beat.
fn deck_beat(d: &Deck, t_next_us: i64) -> Option<BeatClock> {
    let g = d.grid?;
    if !d.playing {
        return None;
    }
    let dt = d.time_to_next_beat()?;
    let period_ms = g.period / (d.pitch * SAMPLE_RATE as f64) * 1e3;
    let beat_ms = t_next_us as f64 / 1e3 + dt * 1e3;
    let ahead = d.pos + dt * d.pitch * SAMPLE_RATE as f64;
    let n = ((ahead - g.offset) / g.period).round() as i64;
    Some(BeatClock {
        period_ms,
        beat_ms,
        bar_ms: Some(beat_ms - n.rem_euclid(4) as f64 * period_ms),
    })
}

fn rstate_epoch(remote: &Mutex<Remote>) -> u32 {
    remote
        .lock()
        .unwrap()
        .state
        .as_ref()
        .map(|s| s.0)
        .unwrap_or(0)
}

/// One SYNC correction: match the deck's tempo to the partner as heard, then pull
/// its next beat onto the partner's beat grid. `renv` is the partner's onset
/// envelope ending at output time `t_end`; `pr` its beat period in hops.
/// Which shift (0-3 beats forward) of this deck's bar best matches the partner's
/// bar pattern, given that the deck's next beat lands on partner beat `k` (counted
/// from the beat at phase `ph` of `renv`). None when there is no clear winner.
fn bar_shift(d: &Deck, renv: &[f32], pr: f64, ph: f64, k: f64) -> Option<i32> {
    let c = bar_corr(d, renv, pr, ph, k)?;
    let (best, &cb) = c
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())?;
    // Only move when the winner is clearly better than staying put.
    if best != 0 && cb - c[0] < 0.05 {
        return Some(0);
    }
    Some(if best == 3 { -1 } else { best as i32 })
}

/// Bar-pattern correlation for shifting the deck 0-3 beats forward (see [`bar_shift`]).
fn bar_corr(d: &Deck, renv: &[f32], pr: f64, ph: f64, k: f64) -> Option<[f64; 4]> {
    // `renv` here is the partner's high-band envelope over the same window.
    let g = d.grid?;
    let own = d.bar.as_ref()?;
    let ph_own = g.period / HOP as f64; // own beat in file bins
    let remote = crate::deck::fold(renv, ph, 4.0 * pr);
    let dt = d.time_to_next_beat()?;
    let q = d.pos + dt * d.pitch * SAMPLE_RATE as f64;
    let b = ((q - g.offset) / g.period).round().rem_euclid(4.0);
    let kr = k.rem_euclid(4.0);
    let n = remote.len();
    let corr = |s: f64| -> f64 {
        let (mut xs, mut ys) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for tau in 0..n {
            let xo = ((b + s) * ph_own + tau as f64 * d.pitch).rem_euclid(own.len() as f64)
                as usize
                % own.len();
            let yr = ((kr * pr + tau as f64).rem_euclid(n as f64)) as usize % n;
            xs.push(own[xo] as f64);
            ys.push(remote[yr] as f64);
        }
        let mx = xs.iter().sum::<f64>() / n as f64;
        let my = ys.iter().sum::<f64>() / n as f64;
        let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
        for (x, y) in xs.iter().zip(&ys) {
            sxy += (x - mx) * (y - my);
            sxx += (x - mx).powi(2);
            syy += (y - my).powi(2);
        }
        sxy / (sxx * syy).sqrt().max(1e-12)
    };
    Some([corr(0.0), corr(1.0), corr(2.0), corr(3.0)])
}

/// The partner's beat grid as heard at output time `t_end` (`renv`/`rhf`: their onset
/// and high-band envelopes ending then; `pr`: beat in hops). Beat 1 comes from your
/// deck when its tempo is close (the same bar guess SYNC uses), else from the claps.
fn partner_grid(
    d: Option<&Deck>,
    renv: &[f32],
    rhf: &[f32],
    pr: f64,
    t_end: i64,
) -> Option<PartnerGrid> {
    let hop_us = HOP as f64 / SAMPLE_RATE as f64 * 1e6;
    let ph = beat_phase(renv, pr)?;
    let per_us = pr * hop_us;
    let base_us = t_end as f64 - renv.len() as f64 * hop_us + ph * hop_us;
    let beats_now = (t_end as f64 - base_us) / per_us;
    if let Some(d) = d {
        if let (Some(g), Some(own), Some(dt)) = (d.grid, d.beat_pos(), d.time_to_next_beat()) {
            let own_per_us = g.period / (d.pitch * SAMPLE_RATE as f64) * 1e6;
            if (own_per_us / per_us - 1.0).abs() < 0.12 {
                let own_next = t_end as f64 + dt * 1e6;
                let k = ((own_next - base_us) / per_us).round();
                // Your next beat lands this far before the partner's nearest one.
                let frac = (base_us + k * per_us - own_next) / per_us;
                if let Some(c) = bar_corr(d, rhf, pr, ph, k) {
                    let hi = c.iter().cloned().fold(f64::MIN, f64::max);
                    let lo = c.iter().cloned().fold(f64::MAX, f64::min);
                    // A real clap pattern to go by (claps on 2 and 4 can't tell 1 from 3:
                    // then staying put wins, as in SYNC's own bar guess).
                    if hi - lo >= 0.1 {
                        let best = (0..4).find(|&i| c[i] == hi).unwrap();
                        let best = if best != 0 && hi - c[0] < 0.05 {
                            0
                        } else {
                            best
                        };
                        // Shifting your track s beats forward would line the bars up,
                        // so you're s beats behind.
                        let s = [0.0, 1.0, 2.0, -1.0][best];
                        let ahead = frac - s;
                        return Some(PartnerGrid {
                            base_us,
                            per_us,
                            down_k: beats_now - (own - ahead),
                            bars_known: true,
                        });
                    }
                }
            }
        }
    }
    let fold = crate::deck::fold(rhf, ph, 4.0 * pr);
    Some(PartnerGrid {
        base_us,
        per_us,
        down_k: crate::deck::clap_downbeat(&fold, pr),
        bars_known: false,
    })
}

fn beat_view(g: &PartnerGrid, d: Option<&Deck>, t_out: f64) -> BeatView {
    let partner = g.pos(t_out);
    let you = d.and_then(|d| d.beat_pos());
    let ahead = you.map(|y| {
        if g.bars_known {
            (y - partner + 2.0).rem_euclid(4.0) - 2.0
        } else {
            (y - partner + 0.5).rem_euclid(1.0) - 0.5
        }
    });
    BeatView {
        partner,
        you,
        ahead_beats: ahead,
        ahead_ms: ahead.map(|a| a * g.per_us / 1e3),
        bars_known: g.bars_known,
        beat_ms: g.per_us / 1e3,
    }
}

#[allow(clippy::too_many_arguments)]
fn sync_step(
    d: &mut Deck,
    renv: &[f32],
    rhf: &[f32],
    pr: f64,
    t_end: i64,
    events: &mut Vec<String>,
    t: i64,
    start: i64,
) {
    let g = d.grid.unwrap();
    let hop_s = HOP as f64 / SAMPLE_RATE as f64;
    let pr_s = pr * hop_s;
    // Speed that makes this track's beat as long as the partner's; allow half/double time.
    let base = g.period / SAMPLE_RATE as f64 / pr_s;
    let target = [base, base * 2.0, base / 2.0]
        .into_iter()
        .min_by(|a, b| (a - 1.0).abs().partial_cmp(&(b - 1.0).abs()).unwrap())
        .unwrap();
    let say = |events: &mut Vec<String>, s: String| {
        let secs = ((t - start).max(0) / 1_000_000) as i64;
        events.push(format!("{:02}:{:02} {s}", secs / 60, secs % 60));
    };
    if (target - 1.0).abs() > 0.12 {
        d.sync = false;
        say(
            events,
            format!(
                "SYNC off: tempos too far apart ({:.1} vs {:.1} BPM)",
                g.bpm,
                60.0 / pr_s
            ),
        );
        return;
    }
    let Some(ph) = beat_phase(renv, pr) else {
        return;
    };
    let t0 = t_end as f64 - renv.len() as f64 * hop_s * 1e6;
    let per_us = pr_s * 1e6;
    let base_t = t0 + ph * hop_s * 1e6;
    if !d.sync_locked {
        d.pitch = target;
    } else {
        d.pitch = 0.7 * d.pitch + 0.3 * target;
    }
    let Some(dt) = d.time_to_next_beat() else {
        return;
    };
    let own = t_end as f64 + dt * 1e6;
    let k = ((own - base_t) / per_us).round();
    let mut e_us = own - (base_t + k * per_us);
    // Own beats may be every other partner beat at half/double time; any partner beat will do.
    e_us = (e_us + per_us / 2.0).rem_euclid(per_us) - per_us / 2.0;
    d.sync_err_ms = Some(e_us / 1e3);
    if !d.sync_locked {
        // First lock: jump straight onto the beat (with a short crossfade).
        d.jump(e_us / 1e6 * d.pitch * SAMPLE_RATE as f64);
        d.sync_locked = true;
        d.bar_checks_left = 3;
        // Beats line up now; also guess the bar, so claps and phrases line up too.
        if (target - base).abs() < 1e-9 {
            if let Some(s) = bar_shift(d, rhf, pr, ph, k) {
                if s != 0 {
                    d.beat_jump(s);
                    say(
                        events,
                        format!("SYNC moved your track {s} beat(s) to line up the bars"),
                    );
                }
            }
        }
        say(
            events,
            format!("SYNC locked to partner at {:.1} BPM", 60.0 / pr_s),
        );
    } else if e_us.abs() > 1500.0 {
        d.nudge(e_us / 1e3, 0.8);
    } else if d.bar_checks_left > 0 && (target - base).abs() < 1e-9 {
        d.bar_checks_left -= 1;
        if let Some(s) = bar_shift(d, rhf, pr, ph, k) {
            if s != 0 {
                d.beat_jump(s);
                say(
                    events,
                    format!("SYNC moved your track {s} beat(s) to line up the bars"),
                );
            }
        }
    }
}

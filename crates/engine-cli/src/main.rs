//! `obsidian-peer`: one side of a remote B2B, headless.
//!
//! Example (two terminals, no network impairment):
//!   obsidian-peer --name a --bind 127.0.0.1:9001 --peer 127.0.0.1:9002 --input a.wav --out out/a --monitor beat
//!   obsidian-peer --name b --bind 127.0.0.1:9002 --peer 127.0.0.1:9001 --input b.wav --out out/b --follow
//! Start both within a few seconds of each other, or pass the same --start-at.

use clap::Parser;
use obsidian_engine::obsidian_codec::{CodecConfig, CodecId};
use obsidian_engine::obsidian_jitter::PlayoutConfig;
use obsidian_engine::{read_wav, run_peer, MonitorMode, PeerConfig};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[arg(long, default_value = "peer")]
    name: String,
    #[arg(long)]
    bind: SocketAddr,
    #[arg(long)]
    peer: SocketAddr,
    /// 48 kHz WAV: this DJ's program (looped).
    #[arg(long)]
    input: PathBuf,
    /// Output folder: monitor.wav, sent.wav, blocks.csv, report.json.
    #[arg(long)]
    out: PathBuf,
    /// Session length in seconds (includes the network test).
    #[arg(long, default_value_t = 60.0)]
    duration: f64,
    /// Epoch milliseconds to start at (both peers use the same). Default: 2 s from now, rounded.
    #[arg(long)]
    start_at_ms: Option<i64>,
    /// opus | pcm16
    #[arg(long, default_value = "opus")]
    codec: String,
    #[arg(long, default_value_t = 256)]
    kbps: i32,
    /// Frame size in samples (120 = 2.5 ms, 240 = 5 ms, 480 = 10 ms).
    #[arg(long, default_value_t = 240)]
    frame: usize,
    /// Redundancy offsets, e.g. "1" or "1,3". Empty = none. Each offset k costs k×5 ms of
    /// latency and one more copy of the bitrate; 1,3 recovers single and double losses.
    #[arg(long, default_value = "1,3")]
    redundancy: String,
    #[arg(long, default_value_t = false)]
    opus_no_prediction: bool,
    /// Network test length before playout (seconds).
    #[arg(long, default_value_t = 10.0)]
    net_test: f64,
    /// Jitter percentile covered by the booth delay.
    #[arg(long, default_value_t = 99.5)]
    margin_pct: f64,
    #[arg(long, default_value_t = 2.0)]
    safety_ms: f64,
    /// plain | beat
    #[arg(long, default_value = "plain")]
    monitor: String,
    #[arg(long)]
    bpm: Option<f64>,
    /// Simulate the follower DJ: start the program on the remote's beat.
    #[arg(long, default_value_t = false)]
    follow: bool,
    /// Simulate the outgoing DJ: fade out this many seconds after the remote comes in.
    #[arg(long)]
    fade_out_after: Option<f64>,
    /// Simulated clock offset of this machine (ms).
    #[arg(long, default_value_t = 0.0)]
    clock_offset_ms: f64,
    /// Simulated audio clock error (ppm).
    #[arg(long, default_value_t = 0.0)]
    skew_ppm: f64,
    #[arg(long, default_value_t = 1)]
    stream_id: u32,
}

fn main() -> anyhow::Result<()> {
    let a = Args::parse();
    let program = Arc::new(read_wav(&a.input)?);
    let codec = CodecConfig {
        codec: match a.codec.as_str() {
            "opus" => CodecId::Opus,
            "pcm16" | "pcm" => CodecId::Pcm16,
            c => anyhow::bail!("unknown codec {c}"),
        },
        frame_samples: a.frame,
        bitrate: a.kbps * 1000,
        prediction_disabled: a.opus_no_prediction,
        ..Default::default()
    };
    let redundancy = a
        .redundancy
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().parse::<u64>())
        .collect::<Result<Vec<_>, _>>()?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;
    let start_at_ms = a.start_at_ms.unwrap_or((now_ms / 1000 + 2) * 1000);
    let cfg = PeerConfig {
        name: a.name,
        bind: a.bind,
        peer: a.peer,
        program,
        codec,
        redundancy,
        playout: PlayoutConfig {
            network_test_us: (a.net_test * 1e6) as i64,
            margin_percentile: a.margin_pct,
            safety_us: (a.safety_ms * 1e3) as i64,
            ..Default::default()
        },
        duration_s: a.duration,
        start_at_us: start_at_ms * 1000,
        clock_offset_us: (a.clock_offset_ms * 1e3) as i64,
        skew_ppm: a.skew_ppm,
        monitor: match a.monitor.as_str() {
            "plain" => MonitorMode::Plain,
            "beat" => MonitorMode::Beat,
            m => anyhow::bail!("unknown monitor mode {m}"),
        },
        bpm: a.bpm,
        follow: a.follow,
        fade_out_after_remote_s: a.fade_out_after,
        out_dir: a.out,
        stream_id: a.stream_id,
    };
    let report = run_peer(cfg)?;
    eprintln!(
        "[{}] booth margin {:.1} ms, delay p50 {:?} ms, plc {} / {} frames, late {}, reanchors {}",
        report.name,
        report.booth_margin_ms,
        report
            .delay_est
            .as_ref()
            .map(|d| (d.p50_ms * 10.0).round() / 10.0),
        report.playout.frames_plc,
        report.playout.frames_decoded,
        report.playout.late_packets,
        report.playout.reanchors.len()
    );
    Ok(())
}

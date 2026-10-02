mod audio;
mod check;
mod dsp;
mod merge;
mod session;
mod synth;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Rebuild one clean master from both DJs' local recordings after a remote B2B session.
#[derive(Parser)]
#[command(name = "obsidian-merge", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Align both recordings at every handoff and render the master.
    Merge {
        /// session.json written by the desktop engine (paths inside are relative to it).
        session: PathBuf,
        #[arg(short, long, default_value = "master.wav")]
        out: PathBuf,
        /// Per-handoff alignment report (JSON).
        #[arg(long)]
        report: Option<PathBuf>,
        /// Also write each DJ's placed contribution as a float WAV in this directory.
        #[arg(long)]
        stems: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = merge::Method::Auto)]
        method: merge::Method,
        /// Integrated loudness target. Omit with --no-normalize.
        #[arg(long, default_value_t = -14.0)]
        target_lufs: f64,
        #[arg(long)]
        no_normalize: bool,
        #[arg(long, default_value_t = -1.0)]
        ceiling_dbtp: f64,
        /// Below this level (dBFS RMS) a DJ counts as silent.
        #[arg(long, default_value_t = -60.0)]
        gate_db: f32,
        /// Onset search range around the timestamp guess. Keep under half a beat.
        #[arg(long, default_value_t = 200.0)]
        onset_search_ms: f64,
        /// Output bit depth: 16, 24, or 32 (float).
        #[arg(long, default_value_t = 24)]
        bits: u16,
    },
    /// Generate a synthetic two-DJ session with known offsets, drift and handoffs.
    Synth {
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long, default_value_t = 24.0)]
        minutes: f64,
        #[arg(long, default_value_t = 5)]
        handoffs: usize,
        #[arg(long, default_value_t = 7)]
        seed: u64,
        #[arg(long, default_value_t = 126.0)]
        bpm: f64,
        /// Leave out the received-stream recordings (tests the fallback path).
        #[arg(long)]
        no_received: bool,
        /// Std-dev of the follower's beatmatching error, ms.
        #[arg(long, default_value_t = 0.0)]
        human_err_ms: f64,
        #[arg(long, default_value_t = 37.0, allow_hyphen_values = true)]
        ppm_a: f64,
        #[arg(long, default_value_t = -52.0, allow_hyphen_values = true)]
        ppm_b: f64,
        #[arg(long, default_value_t = 87.3)]
        one_way_ab_ms: f64,
        #[arg(long, default_value_t = 112.6)]
        one_way_ba_ms: f64,
        /// Fraction of 20 ms packets lost in the received stream.
        #[arg(long, default_value_t = 0.01)]
        loss: f64,
    },
    /// Score a merge report (and stems) against a synthetic session's truth.json.
    Check {
        #[arg(long)]
        truth: PathBuf,
        #[arg(long)]
        report: PathBuf,
        /// Stems directory from `merge --stems`, to also verify the rendered placement.
        #[arg(long, requires = "session")]
        stems: Option<PathBuf>,
        /// The session.json that was merged (needed with --stems).
        #[arg(long)]
        session: Option<PathBuf>,
        /// Write scores as JSON here.
        #[arg(long)]
        json: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Merge {
            session,
            out,
            report,
            stems,
            method,
            target_lufs,
            no_normalize,
            ceiling_dbtp,
            gate_db,
            onset_search_ms,
            bits,
        } => {
            let o = merge::Opts {
                method,
                gate_db,
                target_lufs: if no_normalize { None } else { Some(target_lufs) },
                ceiling_dbtp,
                onset_search_ms,
                stems,
                bits,
            };
            let r = merge::run(&session, &out, report.as_deref(), &o)?;
            println!(
                "master: {} ({:.1} min, {:+.1} dB -> {:.1} LUFS, {:.1} dBTP)",
                out.display(),
                r.master_s / 60.0,
                r.gain_db,
                r.integrated_lufs_after,
                r.true_peak_dbtp_after
            );
            println!(
                "{:>2}  {:<7} {:<10} {:>8} {:>7} {:>9} {:>10} {:>11}",
                "#", "handoff", "method", "overlap", "fit", "rms(ms)", "drift(ppm)", "moved(ms)"
            );
            for h in &r.handoffs {
                println!(
                    "{:>2}  {:<7} {:<10} {:>7.1}s {:>3}/{:<3} {:>9.3} {:>10.1} {:>11.2}",
                    h.index,
                    format!("{}->{}", h.from, h.to),
                    format!("{:?}", h.method).to_lowercase(),
                    h.overlap_s,
                    h.inliers,
                    h.windows,
                    h.fit_rms_ms,
                    h.measured_drift_ppm,
                    h.audio_vs_timestamps_ms
                );
            }
            for w in &r.warnings {
                println!("warning: {w}");
            }
        }
        Cmd::Synth {
            out,
            minutes,
            handoffs,
            seed,
            bpm,
            no_received,
            human_err_ms,
            ppm_a,
            ppm_b,
            one_way_ab_ms,
            one_way_ba_ms,
            loss,
        } => {
            synth::run(&synth::SynthOpts {
                out,
                minutes,
                handoffs,
                seed,
                bpm,
                received: !no_received,
                human_err_ms,
                ppm: [ppm_a, ppm_b],
                one_way_ms: [one_way_ab_ms, one_way_ba_ms],
                loss,
            })?;
        }
        Cmd::Check {
            truth,
            report,
            stems,
            session,
            json,
        } => {
            let s = check::run(&truth, &report, session.as_deref().zip(stems.as_deref()))?;
            println!(
                "{:>2}  {:<7} {:<10} {:>13} {:>12} {:>15} {:>15}",
                "#", "handoff", "method", "map err mean", "map err max", "rendered err", "timestamps-only"
            );
            for h in &s {
                println!(
                    "{:>2}  {:<7} {:<10} {:>10.3} ms {:>9.3} ms {:>15} {:>12.2} ms",
                    h.index,
                    format!("{}->{}", h.from, h.to),
                    h.method,
                    h.map_err_mean_ms,
                    h.map_err_max_abs_ms,
                    h.rendered_err_ms.map(|v| format!("{v:.3} ms")).unwrap_or("-".into()),
                    h.timestamp_err_ms
                );
            }
            if let Some(p) = json {
                std::fs::write(p, serde_json::to_string_pretty(&s)?)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

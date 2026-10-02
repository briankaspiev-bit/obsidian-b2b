//! `obsidian-bench`: the lab rig from report §V.1, on one machine.
//!
//!   obsidian-bench gen  --out bench-out/audio
//!   obsidian-bench run  --out bench-out [--only nyc-lon,bad-wifi] [--duration 90] [--jobs 3]
//!
//! Each scenario starts the netem proxy and two peers (A = leader in
//! beat-quantized monitor mode, B = simulated follower who drops on A's beat),
//! then scores both directions and writes bench-out/summary.md.

mod analyze;
mod gen;
mod session;
mod sim;

use analyze::{analyze_direction, DirResult};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write the two test tracks (dj-a.wav, dj-b.wav).
    Gen {
        #[arg(long, default_value = "bench-out/audio")]
        out: PathBuf,
    },
    /// Run scenarios and score them.
    Run {
        #[arg(long, default_value = "bench-out")]
        out: PathBuf,
        /// Comma-separated scenario names. Default: all.
        #[arg(long)]
        only: Option<String>,
        /// Seconds per scenario (includes the 10 s network test).
        #[arg(long, default_value_t = 90.0)]
        duration: f64,
        /// Scenarios to run at once.
        #[arg(long, default_value_t = 2)]
        jobs: usize,
        /// Path to tools/netem-profiles/profiles.json.
        #[arg(long, default_value = "tools/netem-profiles/profiles.json")]
        profiles: PathBuf,
    },
    /// Re-score existing scenario folders.
    Analyze {
        #[arg(long, default_value = "bench-out")]
        out: PathBuf,
    },
    /// Virtual-time simulation of one direction, no host noise (default 60 min per case).
    Sim {
        #[arg(long, default_value = "bench-out")]
        out: PathBuf,
        #[arg(long, default_value_t = 60.0)]
        minutes: f64,
        #[arg(long)]
        only: Option<String>,
        #[arg(long, default_value = "tools/netem-profiles/profiles.json")]
        profiles: PathBuf,
        #[arg(long, default_value_t = 4)]
        jobs: usize,
    },
    /// List scenarios.
    List,
}

#[derive(Clone)]
struct Scenario {
    name: &'static str,
    profile: &'static str,
    what: &'static str,
    a: Vec<&'static str>,
    b: Vec<&'static str>,
}

fn scenarios() -> Vec<Scenario> {
    let s = |name, profile, what, a: &[&'static str], b: &[&'static str]| Scenario {
        name,
        profile,
        what,
        a: a.to_vec(),
        b: b.to_vec(),
    };
    vec![
        s(
            "clean",
            "clean",
            "Loopback baseline, Opus 256k, redundancy 1+3",
            &[],
            &[],
        ),
        s("same-city", "same-city", "8 ms, 1 ms jitter", &[], &[]),
        s(
            "nyc-lon",
            "nyc-lon",
            "38 ms, 4 ms jitter, 0.3% loss",
            &[],
            &[],
        ),
        s(
            "nyc-tyo",
            "nyc-tyo",
            "90 ms, 8 ms jitter, 0.5% loss",
            &[],
            &[],
        ),
        s(
            "bad-wifi",
            "bad-wifi",
            "Wi-Fi stalls + 2% bursty loss, redundancy 1+3",
            &[],
            &[],
        ),
        s(
            "bad-wifi-r0",
            "bad-wifi",
            "Same, no redundancy",
            &["--redundancy", ""],
            &["--redundancy", ""],
        ),
        s(
            "bad-wifi-r147",
            "bad-wifi",
            "Same, redundancy offsets 1, 4 and 7",
            &["--redundancy", "1,4,7"],
            &["--redundancy", "1,4,7"],
        ),
        s(
            "route-change",
            "route-change",
            "38 → 60 ms one-way step at 50 s",
            &[],
            &[],
        ),
        s(
            "drift-offset",
            "nyc-lon",
            "nyc-lon + B's clock 1.234 s off and B's audio clock +80 ppm fast",
            &[],
            &["--clock-offset-ms", "1234", "--skew-ppm", "80"],
        ),
        s(
            "pcm-same-city",
            "same-city",
            "Raw PCM16 (1.5 Mbps) instead of Opus",
            &["--codec", "pcm16"],
            &["--codec", "pcm16"],
        ),
        s(
            "handoff",
            "nyc-lon",
            "Full handoff for the merge tool: B drops on A's beat, A fades out 20 s later; B's clock +80 ppm",
            &["--fade-out-after", "20"],
            &["--skew-ppm", "80"],
        ),
    ]
}

fn bin(name: &str) -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let p = exe
        .parent()
        .context("exe dir")?
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    anyhow::ensure!(
        p.exists(),
        "{} not found; build the whole workspace",
        p.display()
    );
    Ok(p)
}

#[derive(Serialize)]
struct ScenarioResult {
    name: String,
    profile: String,
    what: String,
    a_to_b: Option<DirResult>,
    b_to_a: Option<DirResult>,
    a_beat_events: serde_json::Value,
    b_follow_drop: serde_json::Value,
    proxy: serde_json::Value,
    error: Option<String>,
}

fn launch(
    sc: &Scenario,
    idx: usize,
    out: &Path,
    audio: &Path,
    duration: f64,
    profiles: &Path,
) -> Result<Vec<Child>> {
    let dir = out.join(sc.name);
    std::fs::create_dir_all(&dir)?;
    let base = 21_000 + idx as u16 * 10;
    let (pa, pb, la, lb) = (base + 1, base + 2, base + 3, base + 4);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;
    let start_at = now_ms + 1_500;
    let log = |n: &str| -> Result<Stdio> { Ok(Stdio::from(std::fs::File::create(dir.join(n))?)) };
    let proxy = Command::new(bin("obsidian-netem")?)
        .args([
            "--a-listen",
            &format!("127.0.0.1:{la}"),
            "--a-peer",
            &format!("127.0.0.1:{pa}"),
        ])
        .args([
            "--b-listen",
            &format!("127.0.0.1:{lb}"),
            "--b-peer",
            &format!("127.0.0.1:{pb}"),
        ])
        .args([
            "--profiles",
            &profiles.to_string_lossy(),
            "--profile",
            sc.profile,
        ])
        .args([
            "--seed",
            &(idx as u64 + 1).to_string(),
            "--duration",
            &(duration + 3.0).to_string(),
        ])
        .args(["--stats", &dir.join("proxy.json").to_string_lossy()])
        .stderr(log("proxy.log")?)
        .spawn()?;
    let peer = |name: &str,
                bind: u16,
                to: u16,
                input: &str,
                extra: &[&str],
                role: &[&str]|
     -> Result<Child> {
        let mut c = Command::new(bin("obsidian-peer")?);
        c.args([
            "--name",
            name,
            "--bind",
            &format!("127.0.0.1:{bind}"),
            "--peer",
            &format!("127.0.0.1:{to}"),
        ])
        .args([
            "--input",
            &audio.join(input).to_string_lossy(),
            "--out",
            &dir.join(name).to_string_lossy(),
        ])
        .args([
            "--duration",
            &duration.to_string(),
            "--start-at-ms",
            &start_at.to_string(),
        ])
        .args(["--stream-id", if name == "a" { "1" } else { "2" }])
        .args(role)
        .args(extra)
        .stderr(log(&format!("{name}.log"))?);
        Ok(c.spawn()?)
    };
    let a = peer("a", pa, la, "dj-a.wav", &sc.a, &["--monitor", "beat"])?;
    let b = peer("b", pb, lb, "dj-b.wav", &sc.b, &["--follow"])?;
    Ok(vec![proxy, a, b])
}

fn score(sc_name: &str, profile: &str, what: &str, dir: &Path) -> ScenarioResult {
    if let Err(e) = session::write_session(dir) {
        eprintln!("{sc_name}: session.json not written: {e:#}");
    }
    let read = |p: PathBuf| -> serde_json::Value {
        std::fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Null)
    };
    let a_rep = read(dir.join("a/report.json"));
    let b_rep = read(dir.join("b/report.json"));
    let ab = analyze_direction("A→B", &dir.join("b"), &dir.join("a"), 125.0);
    let ba = analyze_direction("B→A", &dir.join("a"), &dir.join("b"), 125.0);
    let error = match (&ab, &ba) {
        (Err(e), _) | (_, Err(e)) => Some(format!("{e:#}")),
        _ => None,
    };
    ScenarioResult {
        name: sc_name.into(),
        profile: profile.into(),
        what: what.into(),
        a_to_b: ab.ok(),
        b_to_a: ba.ok(),
        a_beat_events: a_rep["beat_events"].clone(),
        b_follow_drop: b_rep["follow_drop"].clone(),
        proxy: read(dir.join("proxy.json")),
        error,
    }
}

fn f(v: f64, d: usize) -> String {
    if v.is_finite() {
        format!("{v:.d$}")
    } else {
        "–".into()
    }
}

fn summary(results: &[ScenarioResult]) -> String {
    let mut s = String::new();
    s.push_str("# Obsidian engine bench\n\n");
    s.push_str("A = leader (beat-quantized monitor), B = simulated follower (drops on A's beat as heard). Delay is true capture→monitor-output delay from the bench's own clock truth, not the engine's estimate.\n\n");
    s.push_str("| Scenario | Dir | Delay p50 ms | Booth margin ms | Steady wander ms | Unplanned >2 ms blocks | Planned changes | Redund. / FEC / PLC frames | Late pkts | Glitch events (per 10 min) | Worst block SNR dB | Beat residual ms | Send kbps |\n");
    s.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for r in results {
        for d in [&r.a_to_b, &r.b_to_a].into_iter().flatten() {
            s.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} / {} / {} | {} | {} ({}) | {} | {} | {} |\n",
                r.name,
                d.direction,
                f(d.delay_p50_ms, 1),
                f(d.booth_margin_ms, 1),
                f(d.steady_max_wander_ms, 2),
                d.unplanned_excursion_blocks,
                d.planned_changes,
                d.recovered_redundancy,
                d.recovered_fec,
                d.concealed_plc,
                d.late_packets,
                d.glitch_events,
                f(d.glitches_per_10min, 1),
                f(d.worst_block_snr_db, 1),
                d.beat_residual_ms.map(|v| f(v, 1)).unwrap_or("–".into()),
                f(d.send_kbps, 0),
            ));
        }
        if let Some(e) = &r.error {
            s.push_str(&format!(
                "| {} | error | {} |\n",
                r.name,
                e.replace('|', "/")
            ));
        }
    }
    s.push_str("\n## Clocks, codec, CPU\n\n| Scenario | Dir | Clock-sync error µs | Drift est ppm | ASRC corrections | Codec SNR dB (vs source) | Codec delay samples | CPU % (one peer) |\n|---|---|---|---|---|---|---|---|\n");
    for r in results {
        for d in [&r.a_to_b, &r.b_to_a].into_iter().flatten() {
            s.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
                r.name,
                d.direction,
                d.clock_sync_error_us
                    .map(|v| v.to_string())
                    .unwrap_or("–".into()),
                f(d.drift_ppm_estimate, 1),
                d.asrc_corrections,
                f(d.codec_snr_db, 1),
                d.codec_delay_samples,
                d.cpu_percent.map(|v| f(v, 1)).unwrap_or("–".into()),
            ));
        }
    }
    s.push_str("\n## Beat-quantized monitoring at A\n\n| Scenario | Lag before (ms) | Extra delay added (ms) | Later checks: lag (ms) |\n|---|---|---|---|\n");
    for r in results {
        if let Some(ev) = r.a_beat_events.as_array() {
            let first = ev.first();
            let later: Vec<String> = ev
                .iter()
                .skip(1)
                .map(|e| f(e["lag_ms"].as_f64().unwrap_or(f64::NAN), 1))
                .collect();
            s.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                r.name,
                first
                    .map(|e| f(e["lag_ms"].as_f64().unwrap_or(f64::NAN), 1))
                    .unwrap_or("–".into()),
                first
                    .map(|e| f(e["applied_extra_ms"].as_f64().unwrap_or(f64::NAN), 1))
                    .unwrap_or("–".into()),
                later.join(", ")
            ));
        }
    }
    s
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Gen { out } => {
            std::fs::create_dir_all(&out)?;
            gen::write_wav(&out.join("dj-a.wav"), &gen::track(&gen::dj_a()))?;
            gen::write_wav(&out.join("dj-b.wav"), &gen::track(&gen::dj_b()))?;
            eprintln!("wrote {}/dj-a.wav, dj-b.wav", out.display());
        }
        Cmd::Sim {
            out,
            minutes,
            only,
            profiles,
            jobs,
        } => {
            let all = obsidian_netem_proxy::load_profiles(&profiles)?;
            let prof = |n: &str| all.iter().find(|p| p.name == n).cloned().unwrap();
            let opus = obsidian_codec::CodecConfig::default();
            let case = |name: &str, p: &str, red: &[u64], tx: f64, rx: f64| sim::SimCase {
                name: name.into(),
                profile: prof(p),
                minutes,
                redundancy: red.to_vec(),
                codec: opus.clone(),
                tx_ppm: tx,
                rx_ppm: rx,
                seed: 42,
            };
            // Default redundancy is [1, 3]: each packet also carries the frames 1 and 3 back.
            let mut cases = vec![
                case("same-city", "same-city", &[1, 3], 0.0, 0.0),
                case("nyc-lon", "nyc-lon", &[1, 3], 0.0, 0.0),
                case("nyc-lon-r1", "nyc-lon", &[1], 0.0, 0.0),
                case("nyc-lon-r0", "nyc-lon", &[], 0.0, 0.0),
                case("nyc-tyo", "nyc-tyo", &[1, 3], 0.0, 0.0),
                case("nyc-tyo-r1", "nyc-tyo", &[1], 0.0, 0.0),
                case("bad-wifi", "bad-wifi", &[1, 3], 0.0, 0.0),
                case("bad-wifi-r147", "bad-wifi", &[1, 4, 7], 0.0, 0.0),
                case("route-change", "route-change", &[1, 3], 0.0, 0.0),
                case("drift+80/-50", "nyc-lon", &[1, 3], 80.0, -50.0),
            ];
            if let Some(o) = only {
                let w: Vec<&str> = o.split(',').collect();
                cases.retain(|c| w.contains(&c.name.as_str()));
            }
            let results = std::sync::Mutex::new(Vec::new());
            let next = std::sync::atomic::AtomicUsize::new(0);
            std::thread::scope(|sc| {
                for _ in 0..jobs.max(1) {
                    sc.spawn(|| loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(c) = cases.get(i) else { break };
                        eprintln!("▶ sim {} ({} min)", c.name, c.minutes);
                        match sim::run(c) {
                            Ok(r) => results.lock().unwrap().push((i, r)),
                            Err(e) => eprintln!("{}: {e:#}", c.name),
                        }
                    });
                }
            });
            let mut results = results.into_inner().unwrap();
            results.sort_by_key(|r| r.0);
            let results: Vec<sim::SimResult> = results.into_iter().map(|r| r.1).collect();
            let mut md = String::from("# Virtual-time simulation (no host noise)\n\n");
            md.push_str("| Case | Min | Redund. | Delay p50 ms | Steady wander ms | Unplanned >2 ms blocks | Re-anchors | Lost on path | Recovered | PLC | Late | Glitches (per 30 min) | Drift est ppm | kbps |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
            for r in &results {
                md.push_str(&format!(
                    "| {} | {} | {:?} | {} | {} | {} | {} | {} | {} | {} | {} | {} ({}) | {} | {} |\n",
                    r.name, r.minutes, r.redundancy, f(r.delay_p50_ms, 1), f(r.steady_max_wander_ms, 2), r.unplanned_excursion_blocks,
                    if r.reanchors.is_empty() { "none".to_string() } else { r.reanchors.join("; ") },
                    r.lost_on_path, r.recovered_redundancy, r.concealed_plc, r.late_packets, r.glitch_events, f(r.glitches_per_30min, 1),
                    f(r.drift_ppm_estimate, 1), f(r.send_kbps, 0)
                ));
            }
            std::fs::create_dir_all(&out)?;
            std::fs::write(out.join("sim.md"), &md)?;
            std::fs::write(
                out.join("sim.json"),
                serde_json::to_string_pretty(&results)?,
            )?;
            println!("{md}");
        }
        Cmd::List => {
            for s in scenarios() {
                println!("{:16} {:13} {}", s.name, s.profile, s.what);
            }
        }
        Cmd::Run {
            out,
            only,
            duration,
            jobs,
            profiles,
        } => {
            let audio = out.join("audio");
            if !audio.join("dj-a.wav").exists() {
                std::fs::create_dir_all(&audio)?;
                gen::write_wav(&audio.join("dj-a.wav"), &gen::track(&gen::dj_a()))?;
                gen::write_wav(&audio.join("dj-b.wav"), &gen::track(&gen::dj_b()))?;
            }
            let wanted: Option<Vec<String>> =
                only.map(|o| o.split(',').map(|s| s.trim().to_string()).collect());
            let list: Vec<Scenario> = scenarios()
                .into_iter()
                .filter(|s| {
                    wanted
                        .as_ref()
                        .map(|w| w.iter().any(|x| x == s.name))
                        .unwrap_or(true)
                })
                .collect();
            for (ci, chunk) in list.chunks(jobs.max(1)).enumerate() {
                let mut kids = Vec::new();
                for (i, sc) in chunk.iter().enumerate() {
                    eprintln!("▶ {} ({})", sc.name, sc.what);
                    kids.push(launch(
                        sc,
                        ci * jobs + i,
                        &out,
                        &audio,
                        duration,
                        &profiles,
                    )?);
                }
                for k in kids.iter_mut().flatten() {
                    k.wait()?;
                }
            }
            let mut results = Vec::new();
            for sc in &list {
                let r = score(sc.name, sc.profile, sc.what, &out.join(sc.name));
                std::fs::write(
                    out.join(sc.name).join("result.json"),
                    serde_json::to_string_pretty(&r)?,
                )?;
                results.push(r);
            }
            let md = summary(&results);
            std::fs::write(out.join("summary.md"), &md)?;
            println!("{md}");
        }
        Cmd::Analyze { out } => {
            let mut results = Vec::new();
            for sc in scenarios() {
                let dir = out.join(sc.name);
                if dir.join("a/report.json").exists() {
                    let r = score(sc.name, sc.profile, sc.what, &dir);
                    std::fs::write(dir.join("result.json"), serde_json::to_string_pretty(&r)?)?;
                    results.push(r);
                }
            }
            let md = summary(&results);
            std::fs::write(out.join("summary.md"), &md)?;
            println!("{md}");
        }
    }
    Ok(())
}

//! Robot DJ: joins a real Obsidian room as the other DJ, with no sound card, so
//! one person can test a set against a second location (a GitHub runner).
//!
//!   robot_dj --code K7QX-M2PD [--minutes 8] [--out robot-out]
//!
//! It speaks the desktop app's room protocol: pairs through the room server
//! (OBSIDIAN_SERVER), says hello, answers Booth Check, presses ready, and starts
//! the set with the same countdown. In the set it plays "Test music: Bells" on
//! the built-in deck. Whenever it has been off air for a while it plays, syncs,
//! shows READY and takes over; taking it back is up to you (SPACE).
//!
//! It writes what it heard from you (monitor.wav), what it sent (sent.wav), the
//! engine's report.json, a per-second timeline.csv and a plain summary.md.

use std::fmt::Write as _;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use obsidian_desktop::devices::test_track;
use obsidian_desktop::link::{BoothLink, LinkSink, LinkStatus};
use obsidian_live::{run_live, Cmd, LiveConfig, LiveControls, LiveSource, LiveStatus};
use obsidian_rendezvous::client::{join_room, ClientConfig, Path};
use obsidian_rendezvous::proto::normalize_code;

/// Same as the app's START_COUNTDOWN_MS: both sides start the set this long after
/// the second DJ presses ready.
const COUNTDOWN: Duration = Duration::from_millis(3000);
/// How long the robot waits in the booth for you to press ready.
const BOOTH_WAIT: Duration = Duration::from_secs(15 * 60);
/// Off air this long (s), then the robot takes over; it plays and syncs at a
/// third of it and shows READY at three quarters.
const CUE_TAKE_S: f64 = 60.0;
/// The level the robot reports in Booth Check (dBFS), like a playing deck.
const BOOTH_LEVEL_DB: f32 = -12.0;
/// Below this peak (about -50 dB) what arrives from you counts as silence.
const SILENT_PEAK: f32 = 0.003;

struct Args {
    code: String,
    name: String,
    minutes: f64,
    cue_s: f64,
    out: PathBuf,
}

fn args() -> Result<Args> {
    let mut a = Args {
        code: String::new(),
        name: "Robot DJ".into(),
        minutes: 8.0,
        cue_s: CUE_TAKE_S,
        out: PathBuf::from("robot-out"),
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().ok_or_else(|| anyhow!("{k} needs a value"));
        match k.as_str() {
            "--code" => a.code = v()?,
            "--name" => a.name = v()?,
            "--minutes" => a.minutes = v()?.parse().context("--minutes")?,
            "--out" => a.out = v()?.into(),
            "--cue-seconds" => a.cue_s = v()?.parse().context("--cue-seconds")?,
            _ => bail!("unknown argument {k}"),
        }
    }
    if a.code.trim().is_empty() {
        bail!("--code is required (the room code from Create room)");
    }
    a.minutes = a.minutes.clamp(1.0, 30.0);
    Ok(a)
}

/// What the booth link hears, passed to the main thread.
enum Heard {
    Control(serde_json::Value),
}

struct ChanSink(Sender<Heard>);

impl LinkSink for ChanSink {
    fn control(&self, json: String) {
        if let Ok(v) = serde_json::from_str(&json) {
            let _ = self.0.send(Heard::Control(v));
        }
    }
    fn status(&self, _: LinkStatus) {}
    fn remote_level(&self, _: (f32, f32)) {}
}

fn say(msg: &str) {
    println!("[robot] {msg}");
}

fn main() -> Result<()> {
    let a = args()?;
    std::fs::create_dir_all(&a.out)?;
    let server =
        std::env::var("OBSIDIAN_SERVER").context("set OBSIDIAN_SERVER to the room server")?;
    let addr: SocketAddr = server
        .to_socket_addrs()
        .context("room server address")?
        .next()
        .ok_or_else(|| anyhow!("room server address"))?;

    let code = normalize_code(&a.code);
    say(&format!("joining room {code}"));
    let conn = join_room(&ClientConfig::new(addr), &code, &a.name).map_err(|e| anyhow!("{e:?}"))?;
    let relay = conn.path == Path::Relay;
    say(&format!(
        "paired with {} ({}), {}",
        conn.peer_name,
        conn.peer_addr,
        if relay {
            "through the relay"
        } else {
            "directly"
        }
    ));

    // ---- Booth Check ----
    let (tx, rx) = mpsc::channel();
    let link = BoothLink::start(
        conn.socket.try_clone()?,
        conn.peer_addr,
        relay,
        ChanSink(tx),
    )?;
    let hello = serde_json::json!({ "t": "hello", "name": a.name, "city": "GitHub cloud" });
    link.send_control(hello.to_string())
        .map_err(|e| anyhow!(e))?;
    let (booth, start_at) = wait_for_ready(&link, &rx, &hello)?;
    say(&format!(
        "you pressed ready; the set starts in {} s",
        COUNTDOWN.as_secs()
    ));

    // ---- The set ----
    let sock = conn.socket.try_clone()?;
    let peer = conn.peer_addr;
    thread::sleep(start_at.saturating_duration_since(Instant::now()));
    link.hand_over();
    let (title, program) = test_track(1);
    let mut cfg = LiveConfig::new(&a.name, LiveSource::Deck { title, program });
    cfg.peer = Some(peer);
    // The room's creator opens on air, like the app.
    cfg.start_on_air = conn.is_host;
    cfg.autoplay = conn.is_host;
    cfg.record_dir = Some(a.out.clone());
    let ctl = Arc::new(LiveControls::default());
    let c2 = ctl.clone();
    let engine = thread::spawn(move || run_live(cfg, sock, c2));
    say("live");

    let samples = drive(&ctl, a.minutes, a.cue_s);
    ctl.stop();
    let final_status = engine.join().map_err(|_| anyhow!("engine panicked"))??;
    conn.leave();

    let summary = summarize(&a, relay, &booth, &samples, &final_status);
    std::fs::write(a.out.join("summary.md"), &summary)?;
    std::fs::write(a.out.join("timeline.csv"), timeline_csv(&samples))?;
    println!("{summary}");
    Ok(())
}

/// What Booth Check measured, for the summary.
#[derive(Default)]
struct BoothSeen {
    rtt_ms: Option<f64>,
    jitter_ms: Option<f64>,
    loss_pct: Option<f64>,
}

/// Keeps saying hello and ready until you press ready too.
/// Returns when the set starts: the countdown from your ready, as on your screen.
fn wait_for_ready(
    link: &BoothLink,
    rx: &Receiver<Heard>,
    hello: &serde_json::Value,
) -> Result<(BoothSeen, Instant)> {
    let ready = serde_json::json!({ "t": "boothReady", "ready": true }).to_string();
    let until = Instant::now() + BOOTH_WAIT;
    let mut seen = BoothSeen::default();
    let mut next_send = Instant::now();
    say("in Booth Check: run the check, then press I'M READY");
    // The robot's own Booth Check, measured while you run yours.
    let (ntx, nrx) = mpsc::channel();
    let tester = link.tester();
    thread::spawn(move || {
        let _ = ntx.send(tester.run(Duration::from_secs(4)));
    });
    loop {
        if Instant::now() > until {
            bail!(
                "nobody pressed ready within {} minutes",
                BOOTH_WAIT.as_secs() / 60
            );
        }
        if Instant::now() >= next_send {
            // Repeats are harmless: the app keeps its countdown once both are ready.
            link.send_control(hello.to_string())
                .map_err(|e| anyhow!(e))?;
            link.send_control(ready.clone()).map_err(|e| anyhow!(e))?;
            link.set_local_level((BOOTH_LEVEL_DB, BOOTH_LEVEL_DB));
            next_send = Instant::now() + Duration::from_millis(500);
        }
        if let Ok(Some(n)) = nrx.try_recv() {
            if n.reached {
                seen.rtt_ms = Some(n.rtt_ms);
                seen.jitter_ms = Some(n.jitter_ms);
                seen.loss_pct = Some(n.loss_pct);
                say(&format!(
                    "Booth Check: {:.0} ms round trip, {:.1} ms jitter, {:.1}% lost",
                    n.rtt_ms, n.jitter_ms, n.loss_pct
                ));
            }
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Heard::Control(v)) => match v["t"].as_str() {
                Some("boothReady") if v["ready"] == true => {
                    let start_at = Instant::now() + COUNTDOWN;
                    // Finish the robot's own measurement if it's still running, within the countdown.
                    if seen.rtt_ms.is_none() {
                        let left = start_at.saturating_duration_since(Instant::now()) / 2;
                        if let Ok(Some(n)) = nrx.recv_timeout(left) {
                            if n.reached {
                                seen.rtt_ms = Some(n.rtt_ms);
                                seen.jitter_ms = Some(n.jitter_ms);
                                seen.loss_pct = Some(n.loss_pct);
                            }
                        }
                    }
                    return Ok((seen, start_at));
                }
                Some("leave") | Some("end") => bail!("you left the room"),
                _ => {}
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => bail!("booth link closed"),
        }
    }
}

/// One second of the set, as the robot saw it.
struct Sample {
    t_s: f64,
    phase: String,
    on_air: bool,
    you_on_air: bool,
    rtt_ms: Option<f64>,
    margin_ms: Option<f64>,
    concealed_10s: u64,
    concealed_total: u64,
    late_total: u64,
    /// Peak of what arrived from you this second.
    your_peak: f32,
    shield: bool,
    /// The robot pressed TAKE OVER this second.
    took_over: bool,
}

/// Plays the set: cue, sync, ready, take over whenever off air for a while.
fn drive(ctl: &LiveControls, minutes: f64, cue_s: f64) -> Vec<Sample> {
    let start = Instant::now();
    let end = start + Duration::from_secs_f64(minutes * 60.0);
    let mut off_air_since = Some(0.0);
    let mut stage = 0; // 0 cueing, 1 playing+synced, 2 ready, 3 asked to take over
    let mut samples = Vec::new();
    let mut peak = 0f32;
    let mut next = start + Duration::from_secs(1);
    while Instant::now() < end {
        thread::sleep(Duration::from_millis(100));
        let st = ctl.status();
        peak = peak.max(st.partner_peak);
        if Instant::now() < next {
            continue;
        }
        next += Duration::from_secs(1);
        let t = start.elapsed().as_secs_f64();
        let mut took_over = false;
        if st.on_air {
            off_air_since = None;
            stage = 0;
        } else {
            let since = *off_air_since.get_or_insert(t);
            let off = t - since;
            let playing = st.deck.as_ref().map(|d| d.playing).unwrap_or(false);
            if stage == 0 && off >= cue_s / 3.0 && st.partner_on_air {
                if !playing {
                    ctl.send(Cmd::DeckPlayPause);
                }
                ctl.send(Cmd::SyncToggle);
                say("cueing: playing and synced to you");
                stage = 1;
            } else if stage == 1 && off >= cue_s * 0.75 {
                ctl.send(Cmd::SetReady(true));
                say("READY");
                stage = 2;
            } else if stage == 2 && off >= cue_s {
                ctl.send(Cmd::TakeOver);
                say("TAKE OVER: the robot is on air; take it back with SPACE");
                took_over = true;
                stage = 3;
            }
        }
        samples.push(Sample {
            t_s: t,
            phase: st.phase.clone(),
            on_air: st.on_air,
            you_on_air: st.partner_on_air,
            rtt_ms: st.rtt_ms,
            margin_ms: st.margin_ms,
            concealed_10s: st.concealed_10s,
            concealed_total: st.concealed_total,
            late_total: st.late_total,
            your_peak: peak,
            shield: st.shield,
            took_over,
        });
        peak = 0.0;
        if st.phase == "partner left" {
            say("you left the set");
            break;
        }
    }
    samples
}

fn db(peak: f32) -> f64 {
    if peak > 1e-6 {
        20.0 * (peak as f64).log10()
    } else {
        f64::NEG_INFINITY
    }
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    Some(v[v.len() / 2])
}

/// Seconds from each handoff until the other side was confirmed on air.
fn handoffs(s: &[Sample]) -> (Vec<f64>, Vec<f64>) {
    let (mut robot, mut you) = (Vec::new(), Vec::new());
    for (i, x) in s.iter().enumerate() {
        if x.took_over {
            if let Some(y) = s[i..].iter().find(|y| y.on_air && !y.you_on_air) {
                robot.push(y.t_s - x.t_s);
            }
        }
        if i > 0 && s[i - 1].on_air && !x.on_air && x.you_on_air {
            you.push(x.t_s);
        }
    }
    (robot, you)
}

fn summarize(a: &Args, relay: bool, booth: &BoothSeen, s: &[Sample], fin: &LiveStatus) -> String {
    let live: Vec<&Sample> = s.iter().filter(|x| x.phase == "live").collect();
    let rtt = median(live.iter().filter_map(|x| x.rtt_ms).collect());
    let heard_s = live.len() as f64;
    let frames = heard_s * 200.0; // 5 ms frames
    let patched = fin.concealed_total as f64;
    let patched_pct = if frames > 0.0 {
        100.0 * patched / frames
    } else {
        0.0
    };
    let worst_10s = live.iter().map(|x| x.concealed_10s).max().unwrap_or(0);
    let bad_seconds = live.iter().filter(|x| x.concealed_10s > 10).count();
    let you_air: Vec<&&Sample> = live.iter().filter(|x| x.you_on_air).collect();
    let level = median(
        you_air
            .iter()
            .map(|x| db(x.your_peak))
            .filter(|d| d.is_finite())
            .collect(),
    );
    let silent = you_air.iter().filter(|x| x.your_peak < SILENT_PEAK).count();
    let shield = live.iter().filter(|x| x.shield).count();
    let (robot_takes, your_takes) = handoffs(s);

    let sound = if heard_s < 10.0 {
        "not tested (the set barely ran)"
    } else if patched_pct < 0.1 && worst_10s <= 10 {
        "clean"
    } else if patched_pct < 0.5 {
        "mostly clean, a few small glitches"
    } else {
        "choppy"
    };

    let mut o = String::new();
    let _ = writeln!(o, "# Robot DJ test: {}", fmt_mins(heard_s));
    let _ = writeln!(o);
    let _ = writeln!(o, "**Your sound, as heard in the cloud: {sound}.**");
    let _ = writeln!(o);
    let _ = writeln!(o, "| | |");
    let _ = writeln!(o, "|---|---|");
    let _ = writeln!(
        o,
        "| Path | {} |",
        if relay { "through the relay" } else { "direct" }
    );
    if let (Some(r), Some(j), Some(l)) = (booth.rtt_ms, booth.jitter_ms, booth.loss_pct) {
        let _ = writeln!(
            o,
            "| Booth Check | {r:.0} ms round trip, {j:.1} ms jitter, {l:.1}% lost |"
        );
    }
    if let Some(r) = rtt {
        let _ = writeln!(o, "| Round trip in the set | {r:.0} ms (median) |");
    }
    let _ = writeln!(
        o,
        "| Patched-over sound (dropouts) | {patched:.0} of {frames:.0} frames, {patched_pct:.2}% · worst 10 s: {worst_10s} · seconds over 10: {bad_seconds} |"
    );
    let _ = writeln!(o, "| Late packets | {} |", fin.late_total);
    let _ = writeln!(o, "| Wi-Fi shield on | {shield} s |");
    match level {
        Some(l) => {
            let _ = writeln!(
                o,
                "| Your level while on air | {l:.0} dB peak (median) · silent {silent} of {} s |",
                you_air.len()
            );
        }
        None => {
            let _ = writeln!(o, "| Your level while on air | never heard you on air |");
        }
    }
    let _ = writeln!(
        o,
        "| Handoffs | robot took over {}× (confirmed in {}) · you took it back {}× |",
        robot_takes.len(),
        robot_takes
            .iter()
            .map(|d| format!("{d:.0} s"))
            .collect::<Vec<_>>()
            .join(", "),
        your_takes.len()
    );
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "Room code {}, robot name \"{}\". Files: monitor.wav is what reached the robot from you.",
        a.code, a.name
    );
    o
}

fn fmt_mins(s: f64) -> String {
    format!("{}:{:02} live", (s / 60.0) as u64, (s % 60.0) as u64)
}

fn timeline_csv(s: &[Sample]) -> String {
    let mut o = String::from("t_s,phase,robot_on_air,you_on_air,rtt_ms,margin_ms,concealed_10s,concealed_total,late_total,your_peak_db,shield,robot_took_over\n");
    for x in s {
        let _ = writeln!(
            o,
            "{:.0},{},{},{},{},{},{},{},{},{:.1},{},{}",
            x.t_s,
            x.phase,
            x.on_air,
            x.you_on_air,
            x.rtt_ms.map(|v| format!("{v:.1}")).unwrap_or_default(),
            x.margin_ms.map(|v| format!("{v:.1}")).unwrap_or_default(),
            x.concealed_10s,
            x.concealed_total,
            x.late_total,
            db(x.your_peak).max(-120.0),
            x.shield,
            x.took_over
        );
    }
    o
}

//! `obsidian-live`: one DJ's side of a remote B2B, on this laptop's sound card.
//!
//!   obsidian-live solo                         practise alone with a ghost DJ over a simulated NYC-London link
//!   obsidian-live host --music track.mp3      wait for a partner on UDP port 9000
//!   obsidian-live join --peer 203.0.113.7:9000 --system
//!   obsidian-live devices

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::{cursor, execute, queue, style, terminal};
use obsidian_audio_io::{device, file, sim::SimDevice, AudioFifo};
use obsidian_live::{
    run_live, Cmd, GhostPlan, GhostSession, LiveConfig, LiveControls, LiveSource, LiveStatus,
};
use std::io::Write;
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(
    name = "obsidian-live",
    version,
    about = "Remote B2B booth: play with a DJ in another city, or practise alone with a ghost DJ."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Practise alone: a ghost DJ plays through the real engine over a simulated long-distance link.
    Solo {
        #[command(flatten)]
        audio: AudioArgs,
        /// Simulated path: clean, same-city, nyc-lon, nyc-tyo, bad-wifi, route-change.
        #[arg(long, default_value = "nyc-lon")]
        path: String,
        /// The ghost's music (repeat for a playlist). Default: built-in 125 BPM test grooves.
        #[arg(long = "ghost-music")]
        ghost_music: Vec<PathBuf>,
        /// Seconds you play alone before the ghost comes back (0 = only when you press G).
        #[arg(long, default_value_t = 60.0)]
        ghost_return: f64,
    },
    /// Wait for your partner to connect (they run `join` with your address).
    Host {
        #[command(flatten)]
        audio: AudioArgs,
        /// UDP port to listen on. Forward it on your router if your partner can't reach you.
        #[arg(long, default_value_t = 9000)]
        port: u16,
    },
    /// Connect to a partner who is hosting.
    Join {
        #[command(flatten)]
        audio: AudioArgs,
        /// Partner's address, e.g. 203.0.113.7:9000.
        #[arg(long)]
        peer: SocketAddr,
        #[arg(long, default_value_t = 9000)]
        port: u16,
    },
    /// List sound cards.
    Devices,
    /// Play a test beep on an output, to check you can hear the app at all.
    Tone {
        /// Output (part of the name from `devices`). Default: as for a session.
        #[arg(long)]
        output: Option<String>,
        /// First channel of the pair (3 = channels 3/4).
        #[arg(long, default_value_t = 1)]
        output_channel: usize,
        #[arg(long, default_value_t = 10.0)]
        seconds: f64,
    },
}

#[derive(Args, Clone)]
struct AudioArgs {
    /// Your name, shown to your partner.
    #[arg(long, default_value = "DJ")]
    name: String,
    /// Play this music file on the built-in deck (mp3, wav, flac, aac/m4a, ogg).
    #[arg(long)]
    music: Option<PathBuf>,
    /// Send whatever this laptop is playing (Spotify, a browser, rekordbox...). Windows 10 2004+.
    #[arg(long)]
    system: bool,
    /// Send a sound card input instead (part of its name, e.g. "Line In").
    #[arg(long)]
    input: Option<String>,
    /// Headphones / speakers (part of the name). Default: the system default.
    #[arg(long)]
    output: Option<String>,
    /// Which output channels to use, as the first of a pair: 3 = channels 3/4
    /// (the headphone jack on many DJ controllers).
    #[arg(long, default_value_t = 1)]
    output_channel: usize,
    /// Start on air.
    #[arg(long)]
    on_air: bool,
    /// Save your side (sent.wav, monitor.wav, report.json) under this folder.
    #[arg(long, default_value = "recordings")]
    record: PathBuf,
    #[arg(long)]
    no_record: bool,
    /// Loss protection: copies of earlier packets (1,3 = any single or double loss, +15 ms).
    #[arg(long, default_value = "1,3")]
    redundancy: String,
    /// Testing: run without the screen for this many seconds, printing status lines.
    #[arg(long, hide = true)]
    headless: Option<f64>,
    /// Testing: a simulated 44.1 kHz sound card (+80 ppm) instead of a real one.
    #[arg(long, hide = true)]
    sim_output: bool,
    /// Testing: scripted keys, e.g. "20:sync,21:play,45:take".
    #[arg(long, hide = true)]
    script: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Command::Tone {
            output,
            output_channel,
            seconds,
        } => tone(output.as_deref(), output_channel, seconds),
        Command::Devices => {
            let l = device::list_devices()?;
            println!(
                "Outputs (default: {}):",
                l.default_output.as_deref().unwrap_or("none")
            );
            for d in l.outputs {
                println!("  {d}");
            }
            println!(
                "Inputs (default: {}):",
                l.default_input.as_deref().unwrap_or("none")
            );
            for d in l.inputs {
                println!("  {d}");
            }
            Ok(())
        }
        Command::Solo {
            audio,
            path,
            ghost_music,
            ghost_return,
        } => {
            let sock = UdpSocket::bind("127.0.0.1:0")?;
            let mut playlist = Vec::new();
            for p in &ghost_music {
                playlist.push((title_of(p), Arc::new(file::load_stereo_48k(p)?)));
            }
            if playlist.is_empty() {
                playlist.push((
                    "Ghost groove A (125 BPM)".into(),
                    Arc::new(obsidian_testaudio::track(&obsidian_testaudio::dj_a())),
                ));
                playlist.push((
                    "Ghost groove B (125 BPM)".into(),
                    Arc::new(obsidian_testaudio::track(&obsidian_testaudio::dj_b())),
                ));
            }
            let mut plan = GhostPlan::new(playlist);
            plan.return_after_s = (ghost_return > 0.0).then_some(ghost_return);
            let ghost = GhostSession::start(sock.local_addr()?, &path, plan, rand_seed())?;
            let title = format!("solo practice with Ghost DJ over a simulated {path} link");
            let r = session(
                audio,
                sock,
                Some(ghost.peer_for_user),
                title,
                Some(ghost.ctl.clone()),
            );
            ghost.stop()?;
            r
        }
        Command::Host { audio, port } => {
            let sock = UdpSocket::bind(("0.0.0.0", port))
                .with_context(|| format!("UDP port {port} is busy"))?;
            let title = format!(
                "hosting on UDP port {port}: your partner runs join --peer <your public IP>:{port}"
            );
            session(audio, sock, None, title, None)
        }
        Command::Join { audio, peer, port } => {
            let sock =
                UdpSocket::bind(("0.0.0.0", port)).or_else(|_| UdpSocket::bind("0.0.0.0:0"))?;
            let title = format!("joined {peer}");
            session(audio, sock, Some(peer), title, None)
        }
    }
}

fn tone(output: Option<&str>, output_channel: usize, seconds: f64) -> Result<()> {
    let sink = Arc::new(AudioFifo::new(48_000));
    let s = device::start_output(output, sink.clone(), 15.0, output_channel.max(1) - 1)?;
    println!("Playing a beep on: {} ({})", s.name, s.format);
    println!("You should hear half-second beeps. Ctrl+C to stop.");
    let t0 = Instant::now();
    let mut n: u64 = 0;
    let mut block = vec![0f32; 480];
    let mut next_print = 1.0;
    while t0.elapsed().as_secs_f64() < seconds {
        // Keep the FIFO fed 30 ms ahead in 5 ms blocks.
        while (n as f64) < (t0.elapsed().as_secs_f64() + 0.03) * 48_000.0 {
            for k in 0..240 {
                let i = n + k as u64;
                let on = (i / 24_000) % 2 == 0;
                let v = if on {
                    0.2 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin()
                } else {
                    0.0
                };
                block[k * 2] = v;
                block[k * 2 + 1] = v;
            }
            sink.push(&block);
            n += 240;
        }
        if t0.elapsed().as_secs_f64() >= next_print {
            next_print += 1.0;
            let st = &s.stats;
            println!(
                "  {:>2.0} s  callbacks {}  ({} frames each)  level {:.2}  underruns {}{}",
                t0.elapsed().as_secs_f64(),
                st.callbacks.load(std::sync::atomic::Ordering::Relaxed),
                st.frames.load(std::sync::atomic::Ordering::Relaxed),
                st.peak(),
                st.underruns.load(std::sync::atomic::Ordering::Relaxed),
                st.last_error
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|e| format!("  ERROR {e}"))
                    .unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    println!("Done. If the callbacks counted up and level was above 0 but you heard nothing, the sound went to a different jack or Windows has the app muted (Settings > Sound > Volume mixer).");
    Ok(())
}

fn rand_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
}

fn title_of(p: &std::path::Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

#[allow(dead_code)] // held only to keep the streams alive
enum Keep {
    Out(device::OutputStream),
    In(device::InputStream),
    Sim(SimDevice),
    #[cfg(windows)]
    Sys(obsidian_audio_io::winloop::SystemCapture),
}

fn session(
    a: AudioArgs,
    sock: UdpSocket,
    peer: Option<SocketAddr>,
    title: String,
    ghost: Option<Arc<LiveControls>>,
) -> Result<()> {
    let mut keep: Vec<Keep> = Vec::new();
    let sources = a.music.is_some() as u8 + a.system as u8 + a.input.is_some() as u8;
    if sources > 1 {
        bail!("pick one of --music, --system, --input");
    }
    let source = if a.system {
        #[cfg(windows)]
        {
            let fifo = Arc::new(AudioFifo::new(48_000));
            keep.push(Keep::Sys(obsidian_audio_io::winloop::SystemCapture::start(
                fifo.clone(),
            )?));
            LiveSource::Capture { fifo, rate: 48_000 }
        }
        #[cfg(not(windows))]
        bail!("--system needs Windows; use --music or --input here");
    } else if let Some(name) = &a.input {
        let fifo = Arc::new(AudioFifo::new(96_000));
        let s = device::start_input(Some(name), fifo.clone())?;
        let rate = s.rate;
        keep.push(Keep::In(s));
        LiveSource::Capture { fifo, rate }
    } else {
        let (title, program) = match &a.music {
            Some(p) => (
                title_of(p),
                file::load_stereo_48k(p).with_context(|| format!("open {}", p.display()))?,
            ),
            None => (
                "Test groove B (125 BPM)".to_string(),
                obsidian_testaudio::track(&obsidian_testaudio::dj_b()),
            ),
        };
        LiveSource::Deck {
            title,
            program: Arc::new(program),
        }
    };
    let capture_mode = matches!(source, LiveSource::Capture { .. });

    // Headphones: the engine pushes 48 kHz blocks, the device pulls through a clock-bridging resampler.
    let sink = Arc::new(AudioFifo::new(48_000));
    let target_ms = 15.0;
    let mut out_stats: Option<Arc<device::StreamStats>> = None;
    let out_name = if a.sim_output {
        keep.push(Keep::Sim(SimDevice::output(
            sink.clone(),
            44_100,
            80.0,
            441,
            target_ms,
            |_| {},
        )));
        "simulated 44.1 kHz sound card".to_string()
    } else {
        let s = device::start_output(
            a.output.as_deref(),
            sink.clone(),
            target_ms,
            a.output_channel.max(1) - 1,
        )?;
        let n = format!("{} ({})", s.name, s.format);
        out_stats = Some(s.stats.clone());
        keep.push(Keep::Out(s));
        n
    };

    let mut cfg = LiveConfig::new(&a.name, source);
    cfg.peer = peer;
    cfg.sink = Some(sink);
    cfg.sink_latency_ms = target_ms + 10.0;
    cfg.start_on_air = a.on_air;
    cfg.redundancy = a
        .redundancy
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().parse())
        .collect::<Result<_, _>>()
        .context("--redundancy")?;
    if !a.no_record {
        let stamp = chrono_stamp();
        cfg.record_dir = Some(a.record.join(stamp));
    }
    let record_dir = cfg.record_dir.clone();
    let ctl = Arc::new(LiveControls::default());
    let c2 = ctl.clone();
    let engine = std::thread::Builder::new()
        .name("live".into())
        .spawn(move || run_live(cfg, sock, c2))?;

    let script = parse_script(a.script.as_deref())?;
    let res = match a.headless {
        Some(secs) => headless(&ctl, ghost.as_deref(), secs, script),
        None => screen(
            &ctl,
            ghost.as_deref(),
            &title,
            &out_name,
            capture_mode,
            out_stats.as_deref(),
        ),
    };
    ctl.stop();
    let fin = engine.join().unwrap();
    drop(keep);
    res?;
    let st = fin?;
    println!(
        "Session over after {:.0} s. Patched-over frames: {}. Network re-syncs: {}.",
        st.uptime_s, st.concealed_total, st.reanchors
    );
    if let Some(d) = record_dir {
        println!("Your recordings: {}", d.display());
    }
    Ok(())
}

fn chrono_stamp() -> String {
    let s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("session-{s}")
}

fn parse_script(s: Option<&str>) -> Result<Vec<(f64, String)>> {
    let mut v = Vec::new();
    for item in s.unwrap_or("").split(',').filter(|x| !x.is_empty()) {
        let (t, c) = item
            .split_once(':')
            .context("script items are time:command")?;
        v.push((t.parse::<f64>()?, c.to_string()));
    }
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    Ok(v)
}

fn apply(ctl: &LiveControls, ghost: Option<&LiveControls>, c: &str) {
    match c {
        "take" => ctl.send(Cmd::TakeOver),
        "play" => ctl.send(Cmd::DeckPlayPause),
        "sync" => ctl.send(Cmd::SyncToggle),
        "cue" => ctl.send(Cmd::DeckCue),
        "jump" => ctl.send(Cmd::BeatJump(1)),
        "ready" => ctl.send(Cmd::SetReady(true)),
        "comeback" => {
            if let Some(g) = ghost {
                g.send(Cmd::GhostComeBack)
            }
        }
        c if c.starts_with("fader") => ctl.set_fader(c[5..].parse().unwrap_or(1.0)),
        c if c.starts_with("nudge") => ctl.send(Cmd::Nudge(c[5..].parse().unwrap_or(0.0))),
        _ => eprintln!("unknown script command {c}"),
    }
}

fn headless(
    ctl: &LiveControls,
    ghost: Option<&LiveControls>,
    secs: f64,
    mut script: Vec<(f64, String)>,
) -> Result<()> {
    let t0 = Instant::now();
    let mut seen = 0usize;
    let mut next_line = 0.0;
    while t0.elapsed().as_secs_f64() < secs {
        let el = t0.elapsed().as_secs_f64();
        while script.first().map(|s| s.0 <= el).unwrap_or(false) {
            let (_, c) = script.remove(0);
            apply(ctl, ghost, &c);
        }
        let st = ctl.status();
        for e in st.events.iter().skip(seen.min(st.events.len())) {
            println!("event   {e}");
        }
        seen = st.events.len();
        if let Some(g) = ghost {
            let gs = g.status();
            let _ = gs;
        }
        if el >= next_line {
            next_line += 5.0;
            let d = st.deck.as_ref();
            println!(
                "status  t={:>5.1} {:<22} on_air={} partner_on_air={} ready={} partner_ready={} delay={} buffer={} align={:+.0} rec10={} patched10={} late={} bpm={} deck_bpm={} sync_err={} ghost={}",
                el,
                st.phase,
                st.on_air,
                st.partner_on_air,
                st.ready,
                st.partner_ready,
                fmt_ms(st.booth_delay_ms),
                fmt_ms(st.margin_ms),
                st.beat_align_ms,
                st.recovered_10s,
                st.concealed_10s,
                st.late_total,
                st.partner_bpm.map(|b| format!("{b:.2}")).unwrap_or("-".into()),
                d.and_then(|d| d.bpm).map(|b| format!("{b:.2}")).unwrap_or("-".into()),
                d.and_then(|d| d.sync_err_ms).map(|e| format!("{e:+.1}ms")).unwrap_or("-".into()),
                ghost.map(|g| g.status().ghost.unwrap_or_default()).unwrap_or_default(),
            );
            if let Some(g) = ghost {
                let gs = g.status();
                println!(
                    "ghost   phase={} on_air={} delay={} align={:+.0} deck_bpm={} sync_err={} patched_total={}",
                    gs.phase,
                    gs.on_air,
                    fmt_ms(gs.booth_delay_ms),
                    gs.beat_align_ms,
                    gs.deck.as_ref().and_then(|d| d.bpm).map(|b| format!("{b:.2}")).unwrap_or("-".into()),
                    gs.deck.as_ref().and_then(|d| d.sync_err_ms).map(|e| format!("{e:+.1}ms")).unwrap_or("-".into()),
                    gs.concealed_total
                );
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn fmt_ms(v: Option<f64>) -> String {
    v.map(|v| format!("{v:.0}ms")).unwrap_or("-".into())
}

fn bar(v: f32, max: f32, w: usize) -> String {
    let n = ((v / max).clamp(0.0, 1.0) * w as f32).round() as usize;
    format!("{}{}", "█".repeat(n), "░".repeat(w - n))
}

fn meter(peak: f32) -> String {
    // -48..0 dBFS in 12 steps
    let db = 20.0 * peak.max(1e-6).log10();
    let n = (((db + 48.0) / 4.0).clamp(0.0, 12.0)) as usize;
    format!("{}{}", "▮".repeat(n), " ".repeat(12 - n))
}

fn screen(
    ctl: &LiveControls,
    ghost: Option<&LiveControls>,
    title: &str,
    out_name: &str,
    capture_mode: bool,
    out_stats: Option<&device::StreamStats>,
) -> Result<()> {
    let mut out = std::io::stdout();
    terminal::enable_raw_mode()?;
    execute!(out, terminal::EnterAlternateScreen, cursor::Hide)?;
    let r = (|| -> Result<()> {
        loop {
            while event::poll(Duration::from_millis(0))? {
                if let Event::Key(k) = event::read()? {
                    if k.kind != KeyEventKind::Press {
                        continue; // Windows reports releases too
                    }
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char(' ') | KeyCode::Char('t') => ctl.send(Cmd::TakeOver),
                        KeyCode::Char('p') => ctl.send(Cmd::DeckPlayPause),
                        KeyCode::Char('s') => ctl.send(Cmd::SyncToggle),
                        KeyCode::Char('c') => ctl.send(Cmd::DeckCue),
                        KeyCode::Char('r') => ctl.send(Cmd::SetReady(!ctl.status().ready)),
                        KeyCode::Char('[') => ctl.send(Cmd::BeatJump(-1)),
                        KeyCode::Char(']') => ctl.send(Cmd::BeatJump(1)),
                        KeyCode::Char(',') => ctl.send(Cmd::Nudge(-10.0)),
                        KeyCode::Char('.') => ctl.send(Cmd::Nudge(10.0)),
                        KeyCode::Char('-') => ctl.send(Cmd::Pitch(-0.1)),
                        KeyCode::Char('=') | KeyCode::Char('+') => ctl.send(Cmd::Pitch(0.1)),
                        KeyCode::Up => ctl.set_fader(ctl.fader() + 0.05),
                        KeyCode::Down => ctl.set_fader(ctl.fader() - 0.05),
                        KeyCode::Right => ctl.set_partner_volume(ctl.partner_volume() + 0.05),
                        KeyCode::Left => ctl.set_partner_volume(ctl.partner_volume() - 0.05),
                        KeyCode::Char('g') => {
                            if let Some(g) = ghost {
                                g.send(Cmd::GhostComeBack)
                            }
                        }
                        _ => {}
                    }
                }
            }
            let mut st = ctl.status();
            if let Some(g) = ghost {
                st.ghost = g.status().ghost;
            }
            draw(
                &mut out,
                &st,
                title,
                out_name,
                capture_mode,
                ghost.is_some(),
                out_stats,
            )?;
            std::thread::sleep(Duration::from_millis(80));
        }
    })();
    execute!(out, cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;
    r
}

fn draw(
    out: &mut std::io::Stdout,
    st: &LiveStatus,
    title: &str,
    out_name: &str,
    capture_mode: bool,
    solo: bool,
    out_stats: Option<&device::StreamStats>,
) -> Result<()> {
    let partner = st.partner_name.clone().unwrap_or_else(|| "Partner".into());
    let mm = |s: f64| format!("{:02}:{:02}", (s as u64) / 60, (s as u64) % 60);
    let air = if st.on_air {
        if st.partner_ready {
            format!("YOU ARE ON AIR · {partner} is READY to take over")
        } else {
            "YOU ARE ON AIR".to_string()
        }
    } else if st.partner_on_air && st.ready {
        format!("{partner} is on air · you're READY (SPACE to take over)")
    } else if st.partner_on_air {
        format!("{partner} is on air (you're coming in: R = ready, SPACE = take over)")
    } else {
        "Nobody is on air yet (SPACE to take the air)".to_string()
    };
    let mut lines = vec![
        format!(" OBSIDIAN LIVE  ·  {title}"),
        format!(" headphones: {out_name}    {}", mm(st.uptime_s)),
        out_stats
            .map(|o| {
                use std::sync::atomic::Ordering::Relaxed;
                format!(
                    " sound card: {} callbacks of {} frames · output level {} · underruns {}{}",
                    o.callbacks.load(Relaxed),
                    o.frames.load(Relaxed),
                    meter(o.peak()),
                    o.underruns.load(Relaxed),
                    o.last_error.lock().unwrap().as_ref().map(|e| format!(" · ERROR {e}")).unwrap_or_default()
                )
            })
            .unwrap_or_default(),
        String::new(),
        format!(" {air}"),
        String::new(),
        format!(
            " Link      {}  ·  partner reaches you {}  ·  buffer {}  ·  round trip {}{}",
            st.phase,
            fmt_ms(st.booth_delay_ms),
            fmt_ms(st.margin_ms),
            fmt_ms(st.rtt_ms),
            if st.beat_align_ms.abs() > 0.5 { format!("  ·  +{:.0} ms to land on your beat", st.beat_align_ms) } else { String::new() }
        ),
        format!(
            " Network   last 10 s: {} frames rescued, {} patched over  ·  total patched {}  ·  re-syncs {}  ·  {:.0} kbps up",
            st.recovered_10s, st.concealed_10s, st.concealed_total, st.reanchors, st.send_kbps
        ),
        String::new(),
        format!(" You       fader {} {:>3.0}%   {}", bar(st.fader, 1.0, 20), st.fader * 100.0, meter(st.local_peak)),
        format!(" {:<9} volume {} {:>3.0}%  {}", trunc(&partner, 9), bar(st.partner_volume, 2.0, 20), st.partner_volume * 100.0, meter(st.partner_peak)),
    ];
    if let Some(d) = &st.deck {
        lines.push(format!(
            " Deck      {}  {}  {}  pitch {:+.1}%  SYNC {}   {} / {}",
            trunc(&d.title, 28),
            if d.playing {
                "▶ playing"
            } else {
                "❚❚ paused"
            },
            d.bpm
                .map(|b| format!("{b:.1} BPM"))
                .unwrap_or("tempo ?".into()),
            d.pitch_pct,
            if d.sync {
                d.sync_err_ms
                    .map(|e| format!("on ({e:+.1} ms)"))
                    .unwrap_or("on (listening)".into())
            } else {
                "off".into()
            },
            mm(d.pos_s),
            mm(d.len_s)
        ));
    } else if capture_mode {
        lines.push(" Source    system audio / input (play your music as usual; it's sent with your fader applied)".into());
    }
    lines.push(format!(
        " {:<9} {}",
        "Tempo",
        st.partner_bpm
            .map(|b| format!("{partner} at {b:.1} BPM"))
            .unwrap_or("-".into())
    ));
    if solo {
        lines.push(format!(
            " Ghost     {}",
            st.ghost.clone().unwrap_or_default()
        ));
    }
    lines.push(String::new());
    lines.push(" What happened".into());
    for e in st.events.iter().rev().take(9).rev() {
        lines.push(format!("   {e}"));
    }
    lines.push(String::new());
    lines.push(
        " SPACE take over  ·  R ready  ·  ↑↓ your fader  ·  ←→ partner volume  ·  Q quit".into(),
    );
    let mut keys = String::new();
    if st.deck.is_some() {
        keys += " P play/pause  ·  S sync  ·  C cue  ·  [ ] beat jump  ·  , . nudge  ·  - = pitch";
    }
    if solo {
        keys += "  ·  G ghost comes back";
    }
    if !keys.is_empty() {
        lines.push(keys);
    }
    let (w, _) = terminal::size().unwrap_or((120, 40));
    queue!(out, cursor::MoveTo(0, 0))?;
    for l in lines {
        let l: String = l.chars().take(w as usize).collect();
        queue!(
            out,
            style::Print(l),
            terminal::Clear(terminal::ClearType::UntilNewLine),
            cursor::MoveToNextLine(1)
        )?;
    }
    queue!(out, terminal::Clear(terminal::ClearType::FromCursorDown))?;
    out.flush()?;
    Ok(())
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).collect::<String>() + "…"
    }
}

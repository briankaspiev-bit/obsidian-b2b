//! The set itself: hands the paired socket, the input opened in Booth Check
//! and the chosen headphones to the engine's live session (`obsidian-live`).
//! From here on TAKE OVER, levels and link health come from the engine.
//!
//! Practice is the same set against the engine's ghost DJ over a simulated
//! long-distance link, played on the built-in deck.

use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result};
use obsidian_live::scope::Bands;
use obsidian_live::{
    run_live, Cmd, GhostPlan, GhostSession, LiveConfig, LiveControls, LiveSource, LiveStatus,
    ScopeChunk,
};

use crate::devices::{self, Capture, Playback};

/// How far ahead the headphone FIFO runs (ms); the engine compensates for it.
const OUTPUT_TARGET_MS: f64 = 15.0;
/// The simulated path to the ghost DJ (a lab profile in crates/netem-proxy).
pub const PRACTICE_PATH: &str = "nyc-lon";

pub enum Source {
    /// The input picked in Booth Check (your mixer, or what the laptop plays).
    Capture(Capture),
    /// A track on the built-in deck.
    Deck {
        title: String,
        program: Arc<Vec<f32>>,
    },
}

pub struct LiveRun {
    ctl: Arc<LiveControls>,
    join: Option<JoinHandle<Result<LiveStatus>>>,
    pub record_dir: Option<PathBuf>,
    ghost: Option<GhostSession>,
    /// Next waveform column the screen hasn't had yet.
    scope_next: AtomicU64,
    // Kept open for the length of the set.
    _capture: Option<Capture>,
    _playback: Playback,
}

pub struct LiveSetup {
    pub name: String,
    pub source: Source,
    pub output_id: Option<String>,
    pub sock: UdpSocket,
    pub peer: SocketAddr,
    pub start_on_air: bool,
    pub record_dir: Option<PathBuf>,
}

/// The ghost's two built-in grooves; you get a third so you aren't mixing a track into itself.
fn ghost_playlist() -> Vec<(String, Arc<Vec<f32>>)> {
    vec![
        (
            "Ghost groove A (125 BPM)".into(),
            Arc::new(obsidian_testaudio::track(&obsidian_testaudio::dj_a())),
        ),
        (
            "Ghost groove B (125 BPM)".into(),
            Arc::new(obsidian_testaudio::track(&obsidian_testaudio::dj_b())),
        ),
    ]
}

/// Your deck's track: a music file, or the built-in test groove.
pub fn practice_track(path: Option<&Path>) -> Result<(String, Arc<Vec<f32>>)> {
    match path {
        Some(p) => {
            let title = p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Your track".into());
            let program = obsidian_audio_io::file::load_stereo_48k(p)
                .with_context(|| format!("Couldn't open {title}"))?;
            Ok((title, Arc::new(program)))
        }
        None => Ok((
            "Test groove B (125 BPM)".into(),
            Arc::new(obsidian_testaudio::track(&obsidian_testaudio::dj_b())),
        )),
    }
}

impl LiveRun {
    pub fn start(s: LiveSetup) -> Result<LiveRun> {
        let playback = devices::open_output(s.output_id.as_deref(), OUTPUT_TARGET_MS)?;
        let (source, capture) = match s.source {
            Source::Capture(c) => {
                // What queued up during the countdown is stale; start from now.
                c.fifo.clear();
                (
                    LiveSource::Capture {
                        fifo: c.fifo.clone(),
                        rate: c.rate,
                    },
                    Some(c),
                )
            }
            Source::Deck { title, program } => (LiveSource::Deck { title, program }, None),
        };
        let mut cfg = LiveConfig::new(&s.name, source);
        cfg.peer = Some(s.peer);
        cfg.sink = Some(playback.sink.clone());
        cfg.sink_latency_ms = OUTPUT_TARGET_MS + 10.0;
        cfg.start_on_air = s.start_on_air;
        cfg.record_dir = s.record_dir.clone();
        let ctl = Arc::new(LiveControls::default());
        let c2 = ctl.clone();
        let sock = s.sock;
        let join = thread::Builder::new()
            .name("live".into())
            .spawn(move || run_live(cfg, sock, c2))?;
        Ok(LiveRun {
            ctl,
            join: Some(join),
            record_dir: s.record_dir,
            ghost: None,
            scope_next: AtomicU64::new(0),
            _capture: capture,
            _playback: playback,
        })
    }

    /// You on the deck, the ghost DJ on air at the far end of a simulated link.
    pub fn practice(
        name: &str,
        track: (String, Arc<Vec<f32>>),
        output_id: Option<String>,
    ) -> Result<LiveRun> {
        let sock = UdpSocket::bind("127.0.0.1:0")?;
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        let ghost = GhostSession::start(
            sock.local_addr()?,
            PRACTICE_PATH,
            GhostPlan::new(ghost_playlist()),
            seed,
        )?;
        let setup = LiveSetup {
            name: name.into(),
            source: Source::Deck {
                title: track.0,
                program: track.1,
            },
            output_id,
            sock,
            peer: ghost.peer_for_user,
            start_on_air: false,
            record_dir: None,
        };
        match LiveRun::start(setup) {
            Ok(mut run) => {
                run.ghost = Some(ghost);
                Ok(run)
            }
            Err(e) => {
                let _ = ghost.stop();
                Err(e)
            }
        }
    }

    pub fn status(&self) -> LiveStatus {
        self.ctl.status()
    }

    /// What the ghost is up to, in practice.
    pub fn ghost_says(&self) -> Option<String> {
        self.ghost.as_ref().and_then(|g| g.ctl.status().ghost)
    }

    pub fn ghost_come_back(&self) {
        if let Some(g) = &self.ghost {
            g.ctl.send(Cmd::GhostComeBack);
        }
    }

    /// Waveform columns made since the last call.
    pub fn scope_new(&self) -> ScopeChunk {
        let c = self
            .ctl
            .scope_since(self.scope_next.load(Ordering::Relaxed));
        self.scope_next
            .store(c.first + c.cols.len() as u64, Ordering::Relaxed);
        c
    }

    pub fn deck_wave(&self) -> Option<Arc<Vec<Bands>>> {
        self.ctl.deck_wave()
    }

    /// Deck buttons: play, cue, sync, nudge (ms) and pitch (%).
    pub fn deck(&self, c: Cmd) {
        self.ctl.send(c);
    }

    pub fn take_over(&self) {
        self.ctl.send(Cmd::TakeOver);
    }

    /// READY while cueing; the partner sees it, TAKE OVER clears it.
    pub fn set_ready(&self, ready: bool) {
        self.ctl.send(Cmd::SetReady(ready));
    }

    pub fn set_fader(&self, v: f32) {
        self.ctl.set_fader(v);
    }

    pub fn set_partner_volume(&self, v: f32) {
        self.ctl.set_partner_volume(v);
    }

    /// Ends the set (the engine says goodbye to the partner and closes the recording).
    pub fn stop(mut self) -> Option<LiveStatus> {
        self.ctl.stop();
        let st = self
            .join
            .take()
            .and_then(|j| j.join().ok())
            .and_then(|r| r.ok());
        if let Some(g) = self.ghost.take() {
            let _ = g.stop();
        }
        st
    }
}

impl Drop for LiveRun {
    fn drop(&mut self) {
        self.ctl.stop();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        if let Some(g) = self.ghost.take() {
            let _ = g.stop();
        }
    }
}

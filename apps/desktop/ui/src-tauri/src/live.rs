//! The set itself: hands the paired socket, the input opened in Booth Check
//! and the chosen headphones to the engine's live session (`obsidian-live`).
//! From here on TAKE OVER, levels and link health come from the engine.

use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use anyhow::Result;
use obsidian_live::{run_live, Cmd, LiveConfig, LiveControls, LiveSource, LiveStatus};

use crate::devices::{self, Capture, Playback};

/// How far ahead the headphone FIFO runs (ms); the engine compensates for it.
const OUTPUT_TARGET_MS: f64 = 15.0;

pub struct LiveRun {
    ctl: Arc<LiveControls>,
    join: Option<JoinHandle<Result<LiveStatus>>>,
    pub record_dir: Option<PathBuf>,
    // Kept open for the length of the set.
    _capture: Capture,
    _playback: Playback,
}

pub struct LiveSetup {
    pub name: String,
    pub capture: Capture,
    pub output_id: Option<String>,
    pub sock: UdpSocket,
    pub peer: SocketAddr,
    pub start_on_air: bool,
    pub record_dir: Option<PathBuf>,
}

impl LiveRun {
    pub fn start(s: LiveSetup) -> Result<LiveRun> {
        let playback = devices::open_output(s.output_id.as_deref(), OUTPUT_TARGET_MS)?;
        // What queued up during the countdown is stale; start from now.
        s.capture.fifo.clear();
        let mut cfg = LiveConfig::new(
            &s.name,
            LiveSource::Capture { fifo: s.capture.fifo.clone(), rate: s.capture.rate },
        );
        cfg.peer = Some(s.peer);
        cfg.sink = Some(playback.sink.clone());
        cfg.sink_latency_ms = OUTPUT_TARGET_MS + 10.0;
        cfg.start_on_air = s.start_on_air;
        cfg.record_dir = s.record_dir.clone();
        let ctl = Arc::new(LiveControls::default());
        let c2 = ctl.clone();
        let sock = s.sock;
        let join = thread::Builder::new().name("live".into()).spawn(move || run_live(cfg, sock, c2))?;
        Ok(LiveRun { ctl, join: Some(join), record_dir: s.record_dir, _capture: s.capture, _playback: playback })
    }

    pub fn status(&self) -> LiveStatus {
        self.ctl.status()
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
        self.join.take().and_then(|j| j.join().ok()).and_then(|r| r.ok())
    }
}

impl Drop for LiveRun {
    fn drop(&mut self) {
        self.ctl.stop();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

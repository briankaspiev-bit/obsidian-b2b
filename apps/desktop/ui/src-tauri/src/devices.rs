//! Sound cards, through the engine's own audio layer (`obsidian-audio-io`), so
//! the input you meter in Booth Check is the very stream the live session
//! sends: it is opened once and handed over.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

use anyhow::{anyhow, Result};
use obsidian_audio_io::{device, AudioFifo};
use serde::Serialize;

/// The "What you send" choice that captures everything the laptop plays
/// (DJ software, a browser...) except Obsidian itself. Windows only.
pub const SYSTEM_AUDIO_ID: &str = "system-audio";

/// "What you send" choices for testing without a mixer: built-in music, looped,
/// sent exactly as a mixer would be. Two of them so each DJ can pick a different one.
pub const TEST_MUSIC: [(&str, &str); 2] = [("test-music-groove", "Test music: Groove"), ("test-music-bells", "Test music: Bells")];

/// Matches the UI's AudioDevice.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AudioDevice {
    pub id: String,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DeviceList {
    pub inputs: Vec<AudioDevice>,
    pub outputs: Vec<AudioDevice>,
}

fn entries(names: Vec<String>, default: Option<String>) -> Vec<AudioDevice> {
    let mut v: Vec<AudioDevice> = names
        .into_iter()
        .map(|n| {
            let is_default = Some(&n) == default.as_ref();
            AudioDevice {
                id: n.clone(),
                label: n,
                detail: if is_default { "System default".into() } else { String::new() },
            }
        })
        .collect();
    // Default first, so the UI's "first device" default is the system one.
    v.sort_by_key(|d| d.detail.is_empty());
    v
}

pub fn list() -> DeviceList {
    let mut out = DeviceList::default();
    if let Ok(d) = device::list_devices() {
        out.inputs = entries(d.inputs, d.default_input);
        out.outputs = entries(d.outputs, d.default_output);
    }
    if cfg!(windows) {
        out.inputs.insert(
            0,
            AudioDevice {
                id: SYSTEM_AUDIO_ID.into(),
                label: "Everything this laptop plays".into(),
                detail: "DJ software, music apps".into(),
            },
        );
    }
    for (id, label) in TEST_MUSIC {
        out.inputs.push(AudioDevice {
            id: id.into(),
            label: label.into(),
            detail: "No mixer needed".into(),
        });
    }
    out
}

pub fn to_dbfs(amp: f32) -> f32 {
    if amp <= 1e-6 {
        f32::NEG_INFINITY
    } else {
        20.0 * amp.log10()
    }
}

/// An open input. Its stream lives on a thread of its own (audio streams are
/// not `Send` on every platform) until this is dropped.
pub struct Capture {
    pub fifo: Arc<AudioFifo>,
    pub rate: u32,
    peak: Peak,
    _stop: mpsc::Sender<()>,
}

/// Where the input's latest peak (× 1e6) is kept.
enum Peak {
    Device(Arc<device::StreamStats>),
    /// Laptop audio or test music: peak × 1e6, kept by whoever fills the FIFO.
    System(Arc<AtomicU64>),
}

impl Capture {
    /// Peak of the latest audio callback, in dBFS.
    pub fn level_dbfs(&self) -> f32 {
        let micro = match &self.peak {
            Peak::Device(s) => s.peak_micro.load(Ordering::Relaxed),
            Peak::System(p) => p.load(Ordering::Relaxed),
        };
        to_dbfs(micro as f32 / 1e6)
    }
}

/// Headphones: the engine pushes 48 kHz blocks into `sink`.
pub struct Playback {
    pub sink: Arc<AudioFifo>,
    _stop: mpsc::Sender<()>,
}

/// Runs `open` on its own thread, keeps what it returns alive there until the
/// returned sender is dropped, and hands back the shareable parts.
fn hold<T, K: 'static>(name: &str, open: impl FnOnce() -> Result<(K, T)> + Send + 'static) -> Result<(T, mpsc::Sender<()>)>
where
    T: Send + 'static,
{
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<T>>();
    thread::Builder::new().name(name.into()).spawn(move || match open() {
        Ok((keep, parts)) => {
            let _ = ready_tx.send(Ok(parts));
            let _ = stop_rx.recv(); // blocks until the owner drops the sender
            drop(keep);
        }
        Err(e) => {
            let _ = ready_tx.send(Err(e));
        }
    })?;
    let parts = ready_rx.recv().map_err(|_| anyhow!("{name} thread died"))??;
    Ok((parts, stop_tx))
}

/// Opens `id` (a device name, or [`SYSTEM_AUDIO_ID`]) into a fresh FIFO.
pub fn open_input(id: &str) -> Result<Capture> {
    let id = id.to_owned();
    let fifo = Arc::new(AudioFifo::new(96_000));
    let f = fifo.clone();
    let ((rate, peak), stop) = hold("capture", move || -> Result<(Box<dyn std::any::Any>, (u32, Peak))> {
        if let Some(i) = TEST_MUSIC.iter().position(|(t, _)| *t == id) {
            let s = TestMusic::start(test_track(i), f);
            let peak = Peak::System(s.peak_micro.clone());
            return Ok((Box::new(s), (48_000, peak)));
        }
        if id == SYSTEM_AUDIO_ID {
            #[cfg(windows)]
            {
                let s = obsidian_audio_io::winloop::SystemCapture::start(f)?;
                let peak = Peak::System(s.peak_micro.clone());
                return Ok((Box::new(s), (48_000, peak)));
            }
            #[cfg(not(windows))]
            return Err(anyhow!("Capturing the laptop's own audio needs Windows."));
        }
        let s = device::start_input(Some(&id), f)?;
        let parts = (s.rate, Peak::Device(s.stats.clone()));
        Ok((Box::new(s), parts))
    })?;
    Ok(Capture { fifo, rate, peak, _stop: stop })
}

/// Opens headphones `id` with a small FIFO for the engine to fill.
pub fn open_output(id: Option<&str>, target_ms: f64) -> Result<Playback> {
    let id = id.map(str::to_owned);
    let sink = Arc::new(AudioFifo::new(48_000));
    let s2 = sink.clone();
    let ((), stop) = hold("playback", move || -> Result<(Box<dyn std::any::Any>, ())> {
        let s = device::start_output(id.as_deref(), s2, target_ms, 0)?;
        Ok((Box::new(s), ()))
    })?;
    Ok(Playback { sink, _stop: stop })
}

fn test_track(i: usize) -> Arc<Vec<f32>> {
    if i == 0 {
        Arc::new(obsidian_testaudio::track(&obsidian_testaudio::dj_b()))
    } else {
        crate::live::ghost_playlist().swap_remove(0).1
    }
}

/// Plays a track into a FIFO in real time, looped, like a sound card would.
struct TestMusic {
    stop: Arc<std::sync::atomic::AtomicBool>,
    peak_micro: Arc<AtomicU64>,
}

impl TestMusic {
    fn start(track: Arc<Vec<f32>>, fifo: Arc<AudioFifo>) -> TestMusic {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let peak_micro = Arc::new(AtomicU64::new(0));
        let (st, pk) = (stop.clone(), peak_micro.clone());
        let _ = thread::Builder::new().name("test-music".into()).spawn(move || {
            const BLOCK: usize = 480; // 10 ms of 48 kHz frames
            let start = std::time::Instant::now();
            let mut pos = 0usize;
            let mut sent = 0u64;
            while !st.load(Ordering::Relaxed) {
                let mut block = Vec::with_capacity(BLOCK * 2);
                while block.len() < BLOCK * 2 {
                    let take = (BLOCK * 2 - block.len()).min(track.len() - pos);
                    block.extend_from_slice(&track[pos..pos + take]);
                    pos = (pos + take) % track.len();
                }
                let peak = block.iter().fold(0f32, |m, v| m.max(v.abs()));
                pk.store((peak * 1e6) as u64, Ordering::Relaxed);
                fifo.push(&block);
                sent += BLOCK as u64;
                let due = start + std::time::Duration::from_micros(sent * 1_000_000 / 48_000);
                if let Some(wait) = due.checked_duration_since(std::time::Instant::now()) {
                    thread::sleep(wait);
                }
            }
        });
        TestMusic { stop, peak_micro }
    }
}

impl Drop for TestMusic {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbfs() {
        assert!((to_dbfs(0.5) - -6.02).abs() < 0.05);
        assert_eq!(to_dbfs(0.0), f32::NEG_INFINITY);
    }

    #[test]
    fn default_device_comes_first() {
        let v = entries(vec!["A".into(), "B".into()], Some("B".into()));
        assert_eq!(v[0].id, "B");
        assert_eq!(v[0].detail, "System default");
    }

    #[test]
    fn listing_never_panics_without_devices() {
        // CI machines have no sound card; an empty list is fine.
        let _ = list();
    }

    #[test]
    fn test_music_plays_in_real_time_with_a_level() {
        let list = list();
        for (id, _) in TEST_MUSIC {
            assert!(list.inputs.iter().any(|d| d.id == id), "{id} offered");
            let c = open_input(id).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(500));
            let frames = c.fifo.len_frames();
            assert!((19_000..=30_000).contains(&frames), "{id}: {frames} frames in 0.5 s");
            assert!(c.level_dbfs() > -30.0, "{id}: level {}", c.level_dbfs());
        }
    }
}

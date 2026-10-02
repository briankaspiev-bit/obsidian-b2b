//! Windows "system audio" capture: everything the laptop is playing (Spotify, a
//! browser, rekordbox, ...) except this app's own output, so the remote DJ we
//! monitor never feeds back into what we send. Uses WASAPI process loopback in
//! EXCLUDE mode on our own process id (Windows 10 2004 / build 19041 or newer).

use crate::AudioFifo;
use anyhow::{anyhow, Result};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use wasapi::{initialize_mta, AudioClient, Direction, SampleType, StreamMode, WaveFormat};

pub struct SystemCapture {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    /// Peak of the last packet × 1e6.
    pub peak_micro: Arc<AtomicU64>,
}

impl SystemCapture {
    /// Start capturing into `fifo` as 48 kHz stereo f32 (Windows converts for us).
    pub fn start(fifo: Arc<AudioFifo>) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicU64::new(0));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let (s2, p2) = (stop.clone(), peak.clone());
        let handle = std::thread::Builder::new()
            .name("system-capture".into())
            .spawn(move || {
                if let Err(e) = capture(fifo, &s2, &p2, &ready_tx) {
                    let _ = ready_tx.send(Err(e.to_string()));
                }
            })?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(SystemCapture {
                stop,
                handle: Some(handle),
                peak_micro: peak,
            }),
            Ok(Err(e)) => Err(anyhow!(
                "system audio capture failed: {e} (needs Windows 10 version 2004 or newer)"
            )),
            Err(_) => Err(anyhow!("system audio capture thread died")),
        }
    }

    pub fn peak(&self) -> f32 {
        self.peak_micro.load(Ordering::Relaxed) as f32 / 1e6
    }
}

impl Drop for SystemCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn capture(
    fifo: Arc<AudioFifo>,
    stop: &AtomicBool,
    peak: &AtomicU64,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = initialize_mta().ok();
    let fmt = WaveFormat::new(32, 32, &SampleType::Float, 48_000, 2, None);
    let mut client = AudioClient::new_application_loopback_client(std::process::id(), false)?;
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: 0,
    };
    client.initialize_client(&fmt, &Direction::Capture, &mode)?;
    let event = client.set_get_eventhandle()?;
    let cap = client.get_audiocaptureclient()?;
    client.start_stream()?;
    let _ = ready.send(Ok(()));
    let mut bytes: VecDeque<u8> = VecDeque::new();
    let mut samples: Vec<f32> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        // Process loopback only signals while something is playing; time out quietly.
        let _ = event.wait_for_event(100);
        loop {
            let n = cap.get_next_packet_size()?.unwrap_or(0);
            if n == 0 {
                break;
            }
            cap.read_from_device_to_deque(&mut bytes)?;
        }
        if bytes.len() >= 8 {
            let whole = bytes.len() / 8 * 8;
            samples.clear();
            let mut pk = 0f32;
            let mut b = [0u8; 4];
            for _ in 0..whole / 4 {
                for x in b.iter_mut() {
                    *x = bytes.pop_front().unwrap();
                }
                let v = f32::from_le_bytes(b);
                pk = pk.max(v.abs());
                samples.push(v);
            }
            fifo.push(&samples);
            peak.store((pk * 1e6) as u64, Ordering::Relaxed);
        }
    }
    let _ = client.stop_stream();
    Ok(())
}

//! A simulated sound card: a thread that consumes (or produces) audio at its own
//! nominal rate with a clock error, in callbacks of a fixed size. Used to test the
//! live pipeline where no real device exists (CI, cloud machines).

use crate::{AdaptiveResampler, AudioFifo, ENGINE_RATE};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct SimDevice {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<AdaptiveResampler>>,
}

impl SimDevice {
    /// Output: pull from `fifo` at `rate` × (1 + ppm), `period` frames per callback.
    /// Every pulled block is also passed to `tap` (e.g. to record what was "played").
    pub fn output(
        fifo: Arc<AudioFifo>,
        rate: u32,
        ppm: f64,
        period: usize,
        target_ms: f64,
        mut tap: impl FnMut(&[f32]) + Send + 'static,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut rs = AdaptiveResampler::new(ENGINE_RATE, rate, (target_ms * 48.0) as usize);
            let mut buf = vec![0f32; period * 2];
            let dt = period as f64 / (rate as f64 * (1.0 + ppm * 1e-6));
            let t0 = Instant::now();
            let mut k = 0u64;
            while !s2.load(Ordering::Relaxed) {
                k += 1;
                let due = t0 + Duration::from_secs_f64(k as f64 * dt);
                if let Some(d) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(d);
                }
                rs.pull(&fifo, &mut buf);
                tap(&buf);
            }
            rs
        });
        SimDevice {
            stop,
            handle: Some(handle),
        }
    }

    pub fn stop(mut self) -> AdaptiveResampler {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.take().unwrap().join().unwrap()
    }
}

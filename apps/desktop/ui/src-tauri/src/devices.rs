//! Audio devices through cpal (WASAPI on Windows) and a level meter on the
//! chosen input, so Booth Check's YOUR SEND meter shows the real mixer.
//!
//! The engine thread owns device I/O for the session itself; this is only
//! enumeration and metering before the session starts.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;

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

fn describe(dev: &cpal::Device, input: bool, is_default: bool) -> Option<AudioDevice> {
    let name = dev.name().ok()?;
    let cfg = if input { dev.default_input_config() } else { dev.default_output_config() }.ok()?;
    let mut detail = format!(
        "{} ch · {} kHz",
        cfg.channels(),
        f64::from(cfg.sample_rate().0) / 1000.0
    );
    if is_default {
        detail.push_str(" · system default");
    }
    // cpal has no stable device id; the name is what WASAPI shows and is
    // unique enough per machine for a picker.
    Some(AudioDevice { id: name.clone(), label: name, detail })
}

/// Default device first, so the UI's "first device" default is the system one.
pub fn list() -> DeviceList {
    let host = cpal::default_host();
    let def_in = host.default_input_device().and_then(|d| d.name().ok());
    let def_out = host.default_output_device().and_then(|d| d.name().ok());
    let mut out = DeviceList::default();
    if let Ok(devs) = host.input_devices() {
        out.inputs = devs
            .filter_map(|d| {
                let is_def = d.name().ok() == def_in;
                describe(&d, true, is_def)
            })
            .collect();
    }
    if let Ok(devs) = host.output_devices() {
        out.outputs = devs
            .filter_map(|d| {
                let is_def = d.name().ok() == def_out;
                describe(&d, false, is_def)
            })
            .collect();
    }
    out.inputs.sort_by_key(|d| !d.detail.ends_with("system default"));
    out.outputs.sort_by_key(|d| !d.detail.ends_with("system default"));
    out
}

/// Peak level per channel since the last read, as linear amplitude bits.
#[derive(Default)]
pub struct Peaks {
    left: AtomicU32,
    right: AtomicU32,
}

impl Peaks {
    fn raise(slot: &AtomicU32, v: f32) {
        let mut cur = slot.load(Ordering::Relaxed);
        while v > f32::from_bits(cur) {
            match slot.compare_exchange_weak(cur, v.to_bits(), Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(now) => cur = now,
            }
        }
    }

    /// dBFS per channel since the last call; -inf is silence.
    pub fn take_dbfs(&self) -> (f32, f32) {
        let l = f32::from_bits(self.left.swap(0, Ordering::Relaxed));
        let r = f32::from_bits(self.right.swap(0, Ordering::Relaxed));
        (to_dbfs(l), to_dbfs(r))
    }

    fn feed(&self, frames: impl Iterator<Item = (f32, f32)>) {
        let (mut l, mut r) = (0f32, 0f32);
        for (a, b) in frames {
            l = l.max(a.abs());
            r = r.max(b.abs());
        }
        Self::raise(&self.left, l);
        Self::raise(&self.right, r);
    }
}

pub fn to_dbfs(amp: f32) -> f32 {
    if amp <= 1e-6 {
        f32::NEG_INFINITY
    } else {
        20.0 * amp.log10()
    }
}

/// A running input meter. Dropping it closes the device.
pub struct InputMeter {
    pub peaks: Arc<Peaks>,
    stop: Option<mpsc::Sender<()>>,
}

impl Drop for InputMeter {
    fn drop(&mut self) {
        self.stop.take();
    }
}

fn find_input(id: &str) -> Result<cpal::Device> {
    let host = cpal::default_host();
    host.input_devices()?
        .find(|d| d.name().map(|n| n == id).unwrap_or(false))
        .ok_or_else(|| anyhow!("Input \"{id}\" is not connected"))
}

/// Open `id` and meter it. Channels 1-2 are the stereo pair (a mono input is
/// shown on both sides).
pub fn meter_input(id: &str) -> Result<InputMeter> {
    let id = id.to_owned();
    let peaks = Arc::new(Peaks::default());
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
    let p = peaks.clone();
    // cpal streams are not Send on every platform, so one thread owns it.
    thread::Builder::new().name("input-meter".into()).spawn(move || {
        let stream = (|| -> Result<cpal::Stream> {
            let dev = find_input(&id)?;
            let cfg = dev.default_input_config().context("reading the input format")?;
            let ch = usize::from(cfg.channels()).max(1);
            let err = |e| eprintln!("input meter: {e}");
            let s = match cfg.sample_format() {
                cpal::SampleFormat::F32 => dev.build_input_stream(
                    &cfg.into(),
                    move |d: &[f32], _: &_| p.feed(d.chunks(ch).map(|f| (f[0], *f.get(1).unwrap_or(&f[0])))),
                    err,
                    None,
                ),
                cpal::SampleFormat::I16 => dev.build_input_stream(
                    &cfg.into(),
                    move |d: &[i16], _: &_| {
                        let s = |v: i16| f32::from(v) / 32768.0;
                        p.feed(d.chunks(ch).map(|f| (s(f[0]), s(*f.get(1).unwrap_or(&f[0])))))
                    },
                    err,
                    None,
                ),
                cpal::SampleFormat::I32 => dev.build_input_stream(
                    &cfg.into(),
                    move |d: &[i32], _: &_| {
                        let s = |v: i32| v as f32 / 2_147_483_648.0;
                        p.feed(d.chunks(ch).map(|f| (s(f[0]), s(*f.get(1).unwrap_or(&f[0])))))
                    },
                    err,
                    None,
                ),
                other => return Err(anyhow!("Unsupported input format {other:?}")),
            }?;
            s.play()?;
            Ok(s)
        })();
        match stream {
            Ok(s) => {
                let _ = ready_tx.send(Ok(()));
                // Blocks until the meter is dropped.
                let _ = stop_rx.recv();
                drop(s);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        }
    })?;
    ready_rx.recv().map_err(|_| anyhow!("input meter thread died"))??;
    Ok(InputMeter { peaks, stop: Some(stop_tx) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_hold_until_read() {
        let p = Peaks::default();
        p.feed([(0.5, -0.25), (0.1, 0.1)].into_iter());
        p.feed([(0.05, 0.0)].into_iter());
        let (l, r) = p.take_dbfs();
        assert!((l - -6.02).abs() < 0.05, "{l}");
        assert!((r - -12.04).abs() < 0.05, "{r}");
        assert_eq!(p.take_dbfs(), (f32::NEG_INFINITY, f32::NEG_INFINITY));
    }

    #[test]
    fn listing_never_panics_without_devices() {
        // CI machines have no sound card; an empty list is fine.
        let _ = list();
    }
}

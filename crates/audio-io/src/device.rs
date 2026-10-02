//! Sound cards through cpal (WASAPI on Windows, Core Audio on macOS, ALSA on Linux).

use crate::{AdaptiveResampler, AudioFifo, ENGINE_RATE};
use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub struct DeviceList {
    pub outputs: Vec<String>,
    pub inputs: Vec<String>,
    pub default_output: Option<String>,
    pub default_input: Option<String>,
}

pub fn list_devices() -> Result<DeviceList> {
    let host = cpal::default_host();
    Ok(DeviceList {
        outputs: host.output_devices()?.map(|d| d.to_string()).collect(),
        inputs: host.input_devices()?.map(|d| d.to_string()).collect(),
        default_output: host.default_output_device().map(|d| d.to_string()),
        default_input: host.default_input_device().map(|d| d.to_string()),
    })
}

fn is_display(name: &str) -> bool {
    let n = name.to_lowercase();
    [
        "display audio",
        "hdmi",
        "displayport",
        "nvidia high definition",
        "amd high definition",
    ]
    .iter()
    .any(|k| n.contains(k))
}

/// A headphone output if there is one; else the system default, unless that is a
/// monitor's HDMI/DisplayPort audio (a common
/// Windows default on laptops plugged into a screen): then the first other output,
/// which is usually the laptop's own speakers / headphone jack.
fn default_output(host: &cpal::Host) -> Result<cpal::Device> {
    // A headphone endpoint wins: on many laptops (Realtek, e.g. MSI) the headphone
    // jack is its own "2nd output" device and the default "Speakers" stays silent
    // when headphones are plugged in.
    if let Some(d) = host.output_devices()?.find(|d| {
        let n = d.to_string().to_lowercase();
        n.contains("headphone") || n.contains("2nd output")
    }) {
        return Ok(d);
    }
    let def = host
        .default_output_device()
        .context("no default output device")?;
    if !is_display(&def.to_string()) {
        return Ok(def);
    }
    Ok(host
        .output_devices()?
        .find(|d| !is_display(&d.to_string()))
        .unwrap_or(def))
}

fn find(devs: impl Iterator<Item = cpal::Device>, name: &str) -> Option<cpal::Device> {
    let want = name.to_lowercase();
    devs.into_iter()
        .find(|d| d.to_string().to_lowercase().contains(&want))
}

/// Live numbers from a running stream, for the status screen.
#[derive(Default)]
pub struct StreamStats {
    pub callbacks: AtomicU64,
    pub underruns: AtomicU64,
    /// Resampler correction in ppm × 1000 (as bits of an i64).
    pub ratio_ppm_milli: AtomicU64,
    /// Peak level of the last callback × 1e6.
    pub peak_micro: AtomicU64,
    /// Frames per callback (last seen).
    pub frames: AtomicU64,
    /// The last error the sound card reported, if any.
    pub last_error: Mutex<Option<String>>,
}

impl StreamStats {
    pub fn ratio_ppm(&self) -> f64 {
        self.ratio_ppm_milli.load(Ordering::Relaxed) as i64 as f64 / 1000.0
    }
    pub fn peak(&self) -> f32 {
        self.peak_micro.load(Ordering::Relaxed) as f32 / 1e6
    }
}

pub struct OutputStream {
    _stream: cpal::Stream,
    pub name: String,
    pub rate: u32,
    pub channels: u16,
    /// e.g. "f32, 2 ch, 48000 Hz".
    pub format: String,
    pub stats: Arc<StreamStats>,
}

/// Play the engine's 48 kHz FIFO on a device. `target_ms` is the FIFO depth the
/// resampler holds (the price of bridging two clocks; 10-20 ms is plenty).
/// `first_channel` (0-based) picks the output pair, e.g. 2 for the headphone
/// jack (channels 3/4) of a DJ controller's sound card.
pub fn start_output(
    name: Option<&str>,
    fifo: Arc<AudioFifo>,
    target_ms: f64,
    first_channel: usize,
) -> Result<OutputStream> {
    let host = cpal::default_host();
    let device = match name {
        Some(n) => find(host.output_devices()?, n)
            .ok_or_else(|| anyhow!("no output device matching \"{n}\""))?,
        None => default_output(&host)?,
    };
    let cfg = device
        .default_output_config()
        .context("output device config")?;
    let rate = cfg.sample_rate();
    let channels = cfg.channels();
    let format = format!("{}, {} ch, {} Hz", cfg.sample_format(), channels, rate);
    let stats = Arc::new(StreamStats::default());
    let rs = Mutex::new(AdaptiveResampler::new(
        ENGINE_RATE,
        rate,
        (target_ms * 48.0) as usize,
    ));
    let stream = match cfg.sample_format() {
        SampleFormat::F32 => {
            build_out::<f32>(&device, cfg.into(), fifo, rs, stats.clone(), first_channel)?
        }
        SampleFormat::I16 => {
            build_out::<i16>(&device, cfg.into(), fifo, rs, stats.clone(), first_channel)?
        }
        SampleFormat::I32 => {
            build_out::<i32>(&device, cfg.into(), fifo, rs, stats.clone(), first_channel)?
        }
        SampleFormat::U16 => {
            build_out::<u16>(&device, cfg.into(), fifo, rs, stats.clone(), first_channel)?
        }
        f => anyhow::bail!("unsupported output sample format {f}"),
    };
    stream.play()?;
    Ok(OutputStream {
        _stream: stream,
        name: device.to_string(),
        rate,
        channels,
        format,
        stats,
    })
}

fn build_out<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    fifo: Arc<AudioFifo>,
    rs: Mutex<AdaptiveResampler>,
    stats: Arc<StreamStats>,
    first_channel: usize,
) -> Result<cpal::Stream> {
    let ch = config.channels as usize;
    let first = if first_channel + 2 <= ch {
        first_channel
    } else {
        0
    };
    let mut stereo: Vec<f32> = Vec::new();
    let err_stats = stats.clone();
    let s = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let frames = data.len() / ch;
            stereo.resize(frames * 2, 0.0);
            let mut r = rs.lock().unwrap();
            r.pull(&fifo, &mut stereo);
            let mut peak = 0f32;
            for (f, o) in data.chunks_mut(ch).enumerate() {
                for (c, s) in o.iter_mut().enumerate() {
                    let v = if c >= first && c < first + 2 {
                        stereo[f * 2 + c - first]
                    } else {
                        0.0
                    };
                    peak = peak.max(v.abs());
                    *s = T::from_sample(v.clamp(-1.0, 1.0));
                }
            }
            stats.callbacks.fetch_add(1, Ordering::Relaxed);
            stats.frames.store(frames as u64, Ordering::Relaxed);
            stats.underruns.store(r.underruns, Ordering::Relaxed);
            stats
                .ratio_ppm_milli
                .store((r.ratio_ppm * 1000.0) as i64 as u64, Ordering::Relaxed);
            stats
                .peak_micro
                .store((peak * 1e6) as u64, Ordering::Relaxed);
        },
        move |e| *err_stats.last_error.lock().unwrap() = Some(e.to_string()),
        None,
    )?;
    Ok(s)
}

pub struct InputStream {
    _stream: cpal::Stream,
    pub name: String,
    pub rate: u32,
    pub stats: Arc<StreamStats>,
}

/// Capture a device (mic / line in / audio-interface input) into a FIFO at the device's rate.
pub fn start_input(name: Option<&str>, fifo: Arc<AudioFifo>) -> Result<InputStream> {
    let host = cpal::default_host();
    let device = match name {
        Some(n) => find(host.input_devices()?, n)
            .ok_or_else(|| anyhow!("no input device matching \"{n}\""))?,
        None => host
            .default_input_device()
            .context("no default input device")?,
    };
    let cfg = device
        .default_input_config()
        .context("input device config")?;
    let rate = cfg.sample_rate();
    let stats = Arc::new(StreamStats::default());
    let stream = match cfg.sample_format() {
        SampleFormat::F32 => build_in::<f32>(&device, cfg.into(), fifo, stats.clone())?,
        SampleFormat::I16 => build_in::<i16>(&device, cfg.into(), fifo, stats.clone())?,
        SampleFormat::I32 => build_in::<i32>(&device, cfg.into(), fifo, stats.clone())?,
        SampleFormat::U16 => build_in::<u16>(&device, cfg.into(), fifo, stats.clone())?,
        f => anyhow::bail!("unsupported input sample format {f}"),
    };
    stream.play()?;
    Ok(InputStream {
        _stream: stream,
        name: device.to_string(),
        rate,
        stats,
    })
}

fn build_in<T: SizedSample>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    fifo: Arc<AudioFifo>,
    stats: Arc<StreamStats>,
) -> Result<cpal::Stream>
where
    f32: FromSample<T>,
{
    let ch = config.channels as usize;
    let mut stereo: Vec<f32> = Vec::new();
    let s = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            stereo.clear();
            let mut peak = 0f32;
            for f in data.chunks(ch) {
                let l: f32 = f[0].to_sample();
                let r: f32 = if ch > 1 { f[1].to_sample() } else { l };
                peak = peak.max(l.abs()).max(r.abs());
                stereo.push(l);
                stereo.push(r);
            }
            fifo.push(&stereo);
            stats.callbacks.fetch_add(1, Ordering::Relaxed);
            stats
                .peak_micro
                .store((peak * 1e6) as u64, Ordering::Relaxed);
        },
        |e| eprintln!("input stream: {e}"),
        None,
    )?;
    Ok(s)
}

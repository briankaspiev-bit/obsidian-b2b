//! WAV/FLAC input, WAV output. Everything is held as planar f32 in memory.

use anyhow::{Context, Result, bail};
use std::path::Path;

pub struct Audio {
    pub sr: u32,
    /// Planar channels, all the same length.
    pub ch: Vec<Vec<f32>>,
}

impl Audio {
    pub fn len(&self) -> usize {
        self.ch.first().map(|c| c.len()).unwrap_or(0)
    }

    pub fn mono(&self) -> Vec<f32> {
        let n = self.ch.len() as f32;
        (0..self.len()).map(|i| self.ch.iter().map(|c| c[i]).sum::<f32>() / n).collect()
    }
}

pub fn read(path: &Path) -> Result<Audio> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "wav" => read_wav(path),
        "flac" => read_flac(path),
        _ => bail!("{}: unsupported audio format (use .wav or .flac)", path.display()),
    }
    .with_context(|| format!("reading {}", path.display()))
}

fn read_wav(path: &Path) -> Result<Audio> {
    let mut r = hound::WavReader::open(path)?;
    let spec = r.spec();
    let nch = spec.channels as usize;
    let mut ch = vec![Vec::with_capacity(r.duration() as usize); nch];
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for (i, s) in r.samples::<f32>().enumerate() {
                ch[i % nch].push(s?);
            }
        }
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            for (i, s) in r.samples::<i32>().enumerate() {
                ch[i % nch].push(s? as f32 * scale);
            }
        }
    }
    Ok(Audio { sr: spec.sample_rate, ch })
}

fn read_flac(path: &Path) -> Result<Audio> {
    let mut r = claxon::FlacReader::open(path)?;
    let info = r.streaminfo();
    let nch = info.channels as usize;
    let scale = 1.0 / (1u64 << (info.bits_per_sample - 1)) as f32;
    let mut ch = vec![Vec::new(); nch];
    for (i, s) in r.samples().enumerate() {
        ch[i % nch].push(s? as f32 * scale);
    }
    Ok(Audio { sr: info.sample_rate, ch })
}

/// `bits` = 16 or 24 for integer PCM with TPDF dither, 32 for float.
pub fn write_wav(path: &Path, a: &Audio, bits: u16) -> Result<()> {
    let spec = hound::WavSpec {
        channels: a.ch.len() as u16,
        sample_rate: a.sr,
        bits_per_sample: bits,
        sample_format: if bits == 32 {
            hound::SampleFormat::Float
        } else {
            hound::SampleFormat::Int
        },
    };
    let mut w = hound::WavWriter::create(path, spec).with_context(|| format!("creating {}", path.display()))?;
    let n = a.len();
    if bits == 32 {
        for i in 0..n {
            for c in &a.ch {
                w.write_sample(c[i])?;
            }
        }
    } else {
        let full = ((1u64 << (bits - 1)) - 1) as f32;
        let mut rng = 0x9E37_79B9_7F4A_7C15u64;
        let mut uni = move || {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((rng >> 40) as f32) / (1u64 << 24) as f32
        };
        for i in 0..n {
            for c in &a.ch {
                let d = uni() - uni(); // TPDF, +-1 LSB
                let v = (c[i] * full + d).round().clamp(-full - 1.0, full);
                w.write_sample(v as i32)?;
            }
        }
    }
    w.finalize()?;
    Ok(())
}

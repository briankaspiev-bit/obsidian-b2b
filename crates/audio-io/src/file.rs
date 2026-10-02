//! Load a music file (MP3, AAC/M4A, FLAC, WAV, AIFF, OGG) as 48 kHz interleaved stereo.

use anyhow::{Context, Result};
use std::path::Path;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

pub fn load_stereo_48k(path: &Path) -> Result<Vec<f32>> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .with_context(|| format!("{}: unsupported or unreadable audio file", path.display()))?;
    let track = format
        .default_track(TrackType::Audio)
        .context("no audio track")?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .context("no codec parameters")?
        .audio()
        .context("not audio")?
        .clone();
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())?;

    let mut interleaved: Vec<f32> = Vec::new();
    let mut rate = 0u32;
    let mut channels = 0usize;
    let mut tmp: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(e.into()),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                rate = buf.spec().rate();
                channels = buf.spec().channels().count();
                tmp.resize(buf.samples_interleaved(), 0.0);
                buf.copy_to_slice_interleaved(&mut tmp);
                interleaved.extend_from_slice(&tmp);
            }
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::ensure!(
        rate > 0 && channels > 0 && !interleaved.is_empty(),
        "{}: no audio decoded",
        path.display()
    );
    let stereo: Vec<f32> = match channels {
        1 => interleaved.iter().flat_map(|&s| [s, s]).collect(),
        2 => interleaved,
        c => interleaved
            .chunks_exact(c)
            .flat_map(|f| [f[0], f[1]])
            .collect(),
    };
    Ok(if rate == 48_000 {
        stereo
    } else {
        resample_offline(&stereo, rate, 48_000)
    })
}

/// Windowed-sinc (Blackman, 32 zero crossings) offline resampler for whole files.
pub fn resample_offline(stereo: &[f32], from: u32, to: u32) -> Vec<f32> {
    let n_in = stereo.len() / 2;
    let ratio = to as f64 / from as f64;
    let n_out = (n_in as f64 * ratio).floor() as usize;
    let cutoff = ratio.min(1.0) * 0.95;
    const ZC: i64 = 32;
    let half = (ZC as f64 / cutoff).ceil() as i64;
    let mut out = vec![0f32; n_out * 2];
    for (j, o) in out.chunks_exact_mut(2).enumerate() {
        let x = j as f64 / ratio;
        let c = x.floor() as i64;
        let (mut l, mut r, mut wsum) = (0f64, 0f64, 0f64);
        for i in (c - half + 1)..=(c + half) {
            if i < 0 || i as usize >= n_in {
                continue;
            }
            let d = x - i as f64;
            let a = std::f64::consts::PI * d * cutoff;
            let sinc = if a.abs() < 1e-9 { 1.0 } else { a.sin() / a };
            let t = (d / half as f64).clamp(-1.0, 1.0);
            let w = 0.42
                + 0.5 * (std::f64::consts::PI * t).cos()
                + 0.08 * (2.0 * std::f64::consts::PI * t).cos();
            let k = sinc * w;
            l += stereo[i as usize * 2] as f64 * k;
            r += stereo[i as usize * 2 + 1] as f64 * k;
            wsum += k;
        }
        if wsum.abs() > 1e-9 {
            o[0] = (l / wsum) as f32;
            o[1] = (r / wsum) as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_a_tone_cleanly() {
        let n = 44_100;
        let x: Vec<f32> = (0..n)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 44_100.0).sin() * 0.5;
                [s, s]
            })
            .collect();
        let y = resample_offline(&x, 44_100, 48_000);
        assert_eq!(y.len() / 2, 48_000);
        let mut err = 0f64;
        for j in 1000..47_000 {
            let want = (2.0 * std::f64::consts::PI * 1000.0 * j as f64 / 48_000.0).sin() * 0.5;
            err = err.max((y[j * 2] as f64 - want).abs());
        }
        assert!(err < 2e-3, "max err {err}");
    }
}

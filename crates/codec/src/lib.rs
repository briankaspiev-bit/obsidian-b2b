//! Codec layer: 48 kHz stereo, fixed frame size (default 5 ms = 240 samples).
//!
//! Audio is interleaved stereo `f32` throughout the engine.

pub use obsidian_protocol::CodecId;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;

#[derive(Debug, Clone)]
pub struct CodecConfig {
    pub codec: CodecId,
    /// Samples per channel per frame. Opus allows 120, 240, 480, 960.
    pub frame_samples: usize,
    /// Opus bitrate in bits/s (CBR).
    pub bitrate: i32,
    /// Opus in-band FEC. Kept as a switch so the bench can show it is inert in CELT mode.
    pub inband_fec: bool,
    /// Opus "prediction disabled": independent frames, better recovery after loss, costs quality.
    pub prediction_disabled: bool,
    pub complexity: i32,
}

impl Default for CodecConfig {
    fn default() -> Self {
        CodecConfig {
            codec: CodecId::Opus,
            frame_samples: 240,
            bitrate: 256_000,
            inband_fec: false,
            prediction_disabled: false,
            complexity: 10,
        }
    }
}

impl CodecConfig {
    pub fn frame_len(&self) -> usize {
        self.frame_samples * CHANNELS
    }
}

#[derive(Debug)]
pub struct CodecError(pub String);
impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CodecError {}
impl From<opus::Error> for CodecError {
    fn from(e: opus::Error) -> Self {
        CodecError(e.to_string())
    }
}

pub struct Encoder {
    cfg: CodecConfig,
    opus: Option<opus::Encoder>,
    buf: Vec<u8>,
}

impl Encoder {
    pub fn new(cfg: &CodecConfig) -> Result<Self, CodecError> {
        let opus = match cfg.codec {
            CodecId::Opus => {
                let mut e = opus::Encoder::new(
                    SAMPLE_RATE,
                    opus::Channels::Stereo,
                    opus::Application::LowDelay,
                )?;
                e.set_bitrate(opus::Bitrate::Bits(cfg.bitrate))?;
                e.set_vbr(false)?;
                e.set_signal(opus::Signal::Music)?;
                e.set_complexity(cfg.complexity)?;
                e.set_inband_fec(cfg.inband_fec)?;
                e.set_packet_loss_perc(if cfg.inband_fec { 5 } else { 0 })?;
                e.set_prediction_disabled(cfg.prediction_disabled)?;
                Some(e)
            }
            CodecId::Pcm16 => None,
        };
        Ok(Encoder {
            cfg: cfg.clone(),
            opus,
            buf: vec![0u8; 4000],
        })
    }

    /// Encoder algorithmic delay in samples (Opus lookahead; 0 for PCM).
    pub fn lookahead(&mut self) -> usize {
        match &mut self.opus {
            Some(e) => e.get_lookahead().unwrap_or(0).max(0) as usize,
            None => 0,
        }
    }

    pub fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>, CodecError> {
        assert_eq!(
            pcm.len(),
            self.cfg.frame_len(),
            "encode expects exactly one frame"
        );
        match &mut self.opus {
            Some(e) => {
                let n = e.encode_float(pcm, &mut self.buf)?;
                Ok(self.buf[..n].to_vec())
            }
            None => {
                let mut out = Vec::with_capacity(pcm.len() * 2);
                for &s in pcm {
                    let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                    out.extend_from_slice(&v.to_le_bytes());
                }
                Ok(out)
            }
        }
    }
}

/// How a decoded frame was produced. The jitter buffer counts these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeKind {
    Normal,
    /// Rebuilt from Opus in-band FEC carried in the *next* packet.
    Fec,
    /// Packet-loss concealment (no data at all for this frame).
    Plc,
}

pub struct Decoder {
    cfg: CodecConfig,
    opus: Option<opus::Decoder>,
    last: Vec<f32>,
}

impl Decoder {
    pub fn new(cfg: &CodecConfig) -> Result<Self, CodecError> {
        let opus = match cfg.codec {
            CodecId::Opus => Some(opus::Decoder::new(SAMPLE_RATE, opus::Channels::Stereo)?),
            CodecId::Pcm16 => None,
        };
        Ok(Decoder {
            cfg: cfg.clone(),
            opus,
            last: vec![0.0; cfg.frame_len()],
        })
    }

    pub fn decode(&mut self, payload: &[u8]) -> Result<Vec<f32>, CodecError> {
        let n = self.cfg.frame_len();
        let mut out = vec![0f32; n];
        match &mut self.opus {
            Some(d) => {
                let got = d.decode_float(payload, &mut out, false)?;
                out.truncate(got * CHANNELS);
                out.resize(n, 0.0);
            }
            None => {
                for (i, c) in payload.chunks_exact(2).take(n).enumerate() {
                    out[i] = i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0;
                }
            }
        }
        self.last.copy_from_slice(&out);
        Ok(out)
    }

    /// Recover the frame *before* `next_payload` from its in-band FEC.
    /// Returns None when the codec has no FEC (PCM) — caller should conceal.
    pub fn decode_fec(&mut self, next_payload: &[u8]) -> Result<Option<Vec<f32>>, CodecError> {
        let n = self.cfg.frame_len();
        match &mut self.opus {
            // CELT frames carry no FEC; asking for it just runs PLC. Report that honestly.
            Some(_) if !self.cfg.inband_fec => Ok(None),
            Some(d) => {
                let mut out = vec![0f32; n];
                let got = d.decode_float(next_payload, &mut out, true)?;
                out.truncate(got * CHANNELS);
                out.resize(n, 0.0);
                self.last.copy_from_slice(&out);
                Ok(Some(out))
            }
            None => Ok(None),
        }
    }

    /// Conceal one missing frame.
    pub fn conceal(&mut self) -> Result<Vec<f32>, CodecError> {
        let n = self.cfg.frame_len();
        match &mut self.opus {
            Some(d) => {
                let mut out = vec![0f32; n];
                let got = d.decode_float(&[], &mut out, false)?;
                out.truncate(got * CHANNELS);
                out.resize(n, 0.0);
                Ok(out)
            }
            None => {
                // PCM: fade the last frame out (cheap PLC, keeps clicks down).
                let mut out = self.last.clone();
                let frames = n / CHANNELS;
                for i in 0..frames {
                    let g = 1.0 - i as f32 / frames as f32;
                    out[i * 2] *= g;
                    out[i * 2 + 1] *= g;
                }
                self.last.iter_mut().for_each(|s| *s = 0.0);
                Ok(out)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frames: usize, start: usize) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = (start + i) as f32 / SAMPLE_RATE as f32;
            let s = 0.3 * (2.0 * std::f32::consts::PI * 440.0 * t).sin();
            v.push(s);
            v.push(s);
        }
        v
    }

    #[test]
    fn opus_roundtrip_and_plc() {
        let cfg = CodecConfig::default();
        let mut enc = Encoder::new(&cfg).unwrap();
        let mut dec = Decoder::new(&cfg).unwrap();
        for k in 0..50 {
            let pkt = enc.encode(&tone(240, k * 240)).unwrap();
            // 256 kbps CBR at 5 ms = 160 bytes.
            assert_eq!(pkt.len(), 160);
            let out = dec.decode(&pkt).unwrap();
            assert_eq!(out.len(), 480);
        }
        assert_eq!(dec.conceal().unwrap().len(), 480);
    }

    #[test]
    fn pcm_roundtrip() {
        let cfg = CodecConfig {
            codec: CodecId::Pcm16,
            ..Default::default()
        };
        let mut enc = Encoder::new(&cfg).unwrap();
        let mut dec = Decoder::new(&cfg).unwrap();
        let x = tone(240, 0);
        let y = dec.decode(&enc.encode(&x).unwrap()).unwrap();
        for (a, b) in x.iter().zip(&y) {
            assert!((a - b).abs() < 1e-4);
        }
    }
}

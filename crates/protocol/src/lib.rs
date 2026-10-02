//! Wire format for the booth engine.
//!
//! Everything travels over one UDP flow per peer pair. Every datagram starts
//! with a 4-byte header: magic (u16), version (u8), kind (u8). All integers are
//! big-endian.
//!
//! Media packets carry one primary codec frame plus zero or more *redundant*
//! copies of earlier frames (see [`MediaPacket::redundant`]). Redundancy is the
//! loss-recovery tool for music: Opus in-band FEC only exists in SILK/hybrid
//! modes, and 5 ms frames at 192+ kbps always run CELT, so in-band FEC does
//! nothing on this link.
//!
//! Not yet here: AEAD encryption and ICE. The header leaves room for both
//! (version bump), see docs/engine-status.md.

pub const MAGIC: u16 = 0x0B5D;
pub const VERSION: u8 = 1;

const KIND_MEDIA: u8 = 1;
const KIND_PING: u8 = 2;
const KIND_PONG: u8 = 3;
const KIND_BYE: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CodecId {
    Opus = 1,
    Pcm16 = 2,
}

impl CodecId {
    fn from_u8(v: u8) -> Result<Self, DecodeError> {
        match v {
            1 => Ok(CodecId::Opus),
            2 => Ok(CodecId::Pcm16),
            _ => Err(DecodeError::BadCodec(v)),
        }
    }
}

/// One codec frame inside a media packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Frame index on the sender's sample timeline (`sample_pos / frame_samples`).
    pub index: u64,
    /// Sender session-clock time (µs) at which the first sample of this frame was captured.
    pub capture_us: i64,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPacket {
    pub stream_id: u32,
    /// Packet sequence number (one per packet, wraps).
    pub seq: u32,
    pub codec: CodecId,
    pub frame_samples: u16,
    /// The newest frame.
    pub primary: Frame,
    /// Copies of earlier frames, e.g. offsets 1 and 4 back.
    pub redundant: Vec<Frame>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    Media(MediaPacket),
    /// NTP-style clock probe. `t0_us` is the sender's clock at transmit.
    Ping {
        id: u32,
        t0_us: i64,
    },
    /// Reply: echoes t0, adds receive time t1 and transmit time t2 (replier's clock).
    Pong {
        id: u32,
        t0_us: i64,
        t1_us: i64,
        t2_us: i64,
    },
    Bye,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    TooShort,
    BadMagic,
    BadVersion(u8),
    BadKind(u8),
    BadCodec(u8),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for DecodeError {}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.pos + n > self.buf.len() {
            return Err(DecodeError::TooShort);
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64, DecodeError> {
        Ok(self.u64()? as i64)
    }
}

fn put_frame(out: &mut Vec<u8>, f: &Frame) {
    out.extend_from_slice(&f.index.to_be_bytes());
    out.extend_from_slice(&f.capture_us.to_be_bytes());
    out.extend_from_slice(&(f.payload.len() as u16).to_be_bytes());
    out.extend_from_slice(&f.payload);
}

fn get_frame(r: &mut Reader<'_>) -> Result<Frame, DecodeError> {
    let index = r.u64()?;
    let capture_us = r.i64()?;
    let len = r.u16()? as usize;
    let payload = r.take(len)?.to_vec();
    Ok(Frame {
        index,
        capture_us,
        payload,
    })
}

impl Packet {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.clear();
        out.extend_from_slice(&MAGIC.to_be_bytes());
        out.push(VERSION);
        match self {
            Packet::Media(m) => {
                out.push(KIND_MEDIA);
                out.extend_from_slice(&m.stream_id.to_be_bytes());
                out.extend_from_slice(&m.seq.to_be_bytes());
                out.push(m.codec as u8);
                out.extend_from_slice(&m.frame_samples.to_be_bytes());
                out.push(m.redundant.len() as u8);
                put_frame(out, &m.primary);
                for f in &m.redundant {
                    put_frame(out, f);
                }
            }
            Packet::Ping { id, t0_us } => {
                out.push(KIND_PING);
                out.extend_from_slice(&id.to_be_bytes());
                out.extend_from_slice(&t0_us.to_be_bytes());
            }
            Packet::Pong {
                id,
                t0_us,
                t1_us,
                t2_us,
            } => {
                out.push(KIND_PONG);
                out.extend_from_slice(&id.to_be_bytes());
                out.extend_from_slice(&t0_us.to_be_bytes());
                out.extend_from_slice(&t1_us.to_be_bytes());
                out.extend_from_slice(&t2_us.to_be_bytes());
            }
            Packet::Bye => out.push(KIND_BYE),
        }
    }

    pub fn decode(buf: &[u8]) -> Result<Packet, DecodeError> {
        let mut r = Reader { buf, pos: 0 };
        if r.u16()? != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        let v = r.u8()?;
        if v != VERSION {
            return Err(DecodeError::BadVersion(v));
        }
        match r.u8()? {
            KIND_MEDIA => {
                let stream_id = r.u32()?;
                let seq = r.u32()?;
                let codec = CodecId::from_u8(r.u8()?)?;
                let frame_samples = r.u16()?;
                let n_red = r.u8()? as usize;
                let primary = get_frame(&mut r)?;
                let mut redundant = Vec::with_capacity(n_red);
                for _ in 0..n_red {
                    redundant.push(get_frame(&mut r)?);
                }
                Ok(Packet::Media(MediaPacket {
                    stream_id,
                    seq,
                    codec,
                    frame_samples,
                    primary,
                    redundant,
                }))
            }
            KIND_PING => Ok(Packet::Ping {
                id: r.u32()?,
                t0_us: r.i64()?,
            }),
            KIND_PONG => Ok(Packet::Pong {
                id: r.u32()?,
                t0_us: r.i64()?,
                t1_us: r.i64()?,
                t2_us: r.i64()?,
            }),
            KIND_BYE => Ok(Packet::Bye),
            k => Err(DecodeError::BadKind(k)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_roundtrip() {
        let p = Packet::Media(MediaPacket {
            stream_id: 7,
            seq: 42,
            codec: CodecId::Opus,
            frame_samples: 240,
            primary: Frame {
                index: 100,
                capture_us: -5,
                payload: vec![1, 2, 3],
            },
            redundant: vec![Frame {
                index: 99,
                capture_us: -5005,
                payload: vec![9; 160],
            }],
        });
        let mut buf = Vec::new();
        p.encode(&mut buf);
        assert_eq!(Packet::decode(&buf).unwrap(), p);
    }

    #[test]
    fn control_roundtrip_and_garbage() {
        let mut buf = Vec::new();
        for p in [
            Packet::Ping { id: 1, t0_us: 2 },
            Packet::Pong {
                id: 1,
                t0_us: 2,
                t1_us: 3,
                t2_us: 4,
            },
            Packet::Bye,
        ] {
            p.encode(&mut buf);
            assert_eq!(Packet::decode(&buf).unwrap(), p);
        }
        assert!(Packet::decode(&[0, 1, 2]).is_err());
        buf[0] ^= 0xff;
        assert_eq!(Packet::decode(&buf), Err(DecodeError::BadMagic));
    }
}

//! Wire format for rendezvous control packets.
//!
//! Every control packet (client <-> server, and peer <-> peer punch probes)
//! starts with [`MAGIC`] then [`VERSION`] then a one-byte message type. Any UDP
//! packet that reaches the server *without* this magic is treated as media and,
//! when the sender belongs to a relayed room, forwarded blindly to the other
//! member. The engine's own packets start with `0x0B 0x5D`, so the two never
//! collide.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub const MAGIC: [u8; 2] = [0x0B, 0x52];
pub const VERSION: u8 = 1;

/// Room codes avoid look-alike characters (0/O, 1/I/L) so they survive being
/// read out over a phone call.
pub const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
/// Shown to people as `XXXX-XXXX`; see [`format_code`].
pub const CODE_LEN: usize = 8;
/// Longest display name kept, in bytes.
pub const MAX_NAME_LEN: usize = 64;

pub type Token = [u8; 16];
pub type SessionId = [u8; 8];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathDecision {
    Pending,
    Direct,
    Relay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    RoomNotFound,
    RoomFull,
    RateLimited,
    BadToken,
    Unknown(u8),
}

impl ErrorCode {
    fn to_u8(self) -> u8 {
        match self {
            ErrorCode::RoomNotFound => 1,
            ErrorCode::RoomFull => 2,
            ErrorCode::RateLimited => 3,
            ErrorCode::BadToken => 4,
            ErrorCode::Unknown(v) => v,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => ErrorCode::RoomNotFound,
            2 => ErrorCode::RoomFull,
            3 => ErrorCode::RateLimited,
            4 => ErrorCode::BadToken,
            v => ErrorCode::Unknown(v),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    // client -> server
    Create {
        req: u32,
        /// Display name shown to the other DJ (truncated to 64 bytes).
        name: String,
        local: Option<SocketAddr>,
    },
    Join {
        req: u32,
        code: String,
        name: String,
        local: Option<SocketAddr>,
    },
    Poll {
        token: Token,
    },
    Report {
        token: Token,
        direct_ok: bool,
    },
    Leave {
        token: Token,
    },

    // server -> client
    Created {
        req: u32,
        code: String,
        token: Token,
        observed: SocketAddr,
    },
    Joined {
        req: u32,
        token: Token,
        observed: SocketAddr,
    },
    Waiting {
        observed: SocketAddr,
    },
    Peer {
        session: SessionId,
        is_host: bool,
        observed: SocketAddr,
        peer_public: SocketAddr,
        peer_local: Option<SocketAddr>,
        peer_name: String,
    },
    Decision {
        path: PathDecision,
    },
    Error {
        req: u32,
        code: ErrorCode,
    },

    // peer <-> peer
    Punch {
        session: SessionId,
        from_host: bool,
    },
    PunchAck {
        session: SessionId,
        from_host: bool,
    },
}

const T_CREATE: u8 = 0x01;
const T_JOIN: u8 = 0x02;
const T_POLL: u8 = 0x03;
const T_REPORT: u8 = 0x04;
const T_LEAVE: u8 = 0x05;
const T_CREATED: u8 = 0x81;
const T_JOINED: u8 = 0x82;
const T_WAITING: u8 = 0x83;
const T_PEER: u8 = 0x84;
const T_DECISION: u8 = 0x85;
const T_ERROR: u8 = 0x8F;
const T_PUNCH: u8 = 0x10;
const T_PUNCH_ACK: u8 = 0x11;

pub fn is_control(packet: &[u8]) -> bool {
    packet.len() >= 4 && packet[..2] == MAGIC
}

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer(Vec::with_capacity(64));
        w.bytes(&MAGIC);
        w.u8(VERSION);
        match self {
            Msg::Create { req, name, local } => {
                w.u8(T_CREATE);
                w.u32(*req);
                w.str(name);
                w.opt_addr(*local);
            }
            Msg::Join {
                req,
                code,
                name,
                local,
            } => {
                w.u8(T_JOIN);
                w.u32(*req);
                w.str(code);
                w.str(name);
                w.opt_addr(*local);
            }
            Msg::Poll { token } => {
                w.u8(T_POLL);
                w.bytes(token);
            }
            Msg::Report { token, direct_ok } => {
                w.u8(T_REPORT);
                w.bytes(token);
                w.u8(*direct_ok as u8);
            }
            Msg::Leave { token } => {
                w.u8(T_LEAVE);
                w.bytes(token);
            }
            Msg::Created {
                req,
                code,
                token,
                observed,
            } => {
                w.u8(T_CREATED);
                w.u32(*req);
                w.str(code);
                w.bytes(token);
                w.addr(*observed);
            }
            Msg::Joined {
                req,
                token,
                observed,
            } => {
                w.u8(T_JOINED);
                w.u32(*req);
                w.bytes(token);
                w.addr(*observed);
            }
            Msg::Waiting { observed } => {
                w.u8(T_WAITING);
                w.addr(*observed);
            }
            Msg::Peer {
                session,
                is_host,
                observed,
                peer_public,
                peer_local,
                peer_name,
            } => {
                w.u8(T_PEER);
                w.bytes(session);
                w.u8(*is_host as u8);
                w.addr(*observed);
                w.addr(*peer_public);
                w.opt_addr(*peer_local);
                w.str(peer_name);
            }
            Msg::Decision { path } => {
                w.u8(T_DECISION);
                w.u8(match path {
                    PathDecision::Pending => 0,
                    PathDecision::Direct => 1,
                    PathDecision::Relay => 2,
                });
            }
            Msg::Error { req, code } => {
                w.u8(T_ERROR);
                w.u32(*req);
                w.u8(code.to_u8());
            }
            Msg::Punch { session, from_host } => {
                w.u8(T_PUNCH);
                w.bytes(session);
                w.u8(*from_host as u8);
            }
            Msg::PunchAck { session, from_host } => {
                w.u8(T_PUNCH_ACK);
                w.bytes(session);
                w.u8(*from_host as u8);
            }
        }
        w.0
    }

    /// Returns `None` for anything that is not a well-formed control packet of
    /// our version. Never panics on hostile input.
    pub fn decode(packet: &[u8]) -> Option<Msg> {
        if !is_control(packet) || packet[2] != VERSION {
            return None;
        }
        let mut r = Reader(&packet[4..]);
        let msg = match packet[3] {
            T_CREATE => Msg::Create {
                req: r.u32()?,
                name: r.str()?,
                local: r.opt_addr()?,
            },
            T_JOIN => Msg::Join {
                req: r.u32()?,
                code: r.str()?,
                name: r.str()?,
                local: r.opt_addr()?,
            },
            T_POLL => Msg::Poll { token: r.array()? },
            T_REPORT => Msg::Report {
                token: r.array()?,
                direct_ok: r.u8()? != 0,
            },
            T_LEAVE => Msg::Leave { token: r.array()? },
            T_CREATED => Msg::Created {
                req: r.u32()?,
                code: r.str()?,
                token: r.array()?,
                observed: r.addr()?,
            },
            T_JOINED => Msg::Joined {
                req: r.u32()?,
                token: r.array()?,
                observed: r.addr()?,
            },
            T_WAITING => Msg::Waiting {
                observed: r.addr()?,
            },
            T_PEER => Msg::Peer {
                session: r.array()?,
                is_host: r.u8()? != 0,
                observed: r.addr()?,
                peer_public: r.addr()?,
                peer_local: r.opt_addr()?,
                peer_name: r.str()?,
            },
            T_DECISION => Msg::Decision {
                path: match r.u8()? {
                    0 => PathDecision::Pending,
                    1 => PathDecision::Direct,
                    2 => PathDecision::Relay,
                    _ => return None,
                },
            },
            T_ERROR => Msg::Error {
                req: r.u32()?,
                code: ErrorCode::from_u8(r.u8()?),
            },
            T_PUNCH => Msg::Punch {
                session: r.array()?,
                from_host: r.u8()? != 0,
            },
            T_PUNCH_ACK => Msg::PunchAck {
                session: r.array()?,
                from_host: r.u8()? != 0,
            },
            _ => return None,
        };
        Some(msg)
    }
}

/// Uppercases and strips spaces/dashes so "abc-123" and "ABC123" match.
pub fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// `ABCD2345` -> `ABCD-2345`, the form the app shows.
pub fn format_code(code: &str) -> String {
    let code = normalize_code(code);
    if code.len() == CODE_LEN {
        format!("{}-{}", &code[..CODE_LEN / 2], &code[CODE_LEN / 2..])
    } else {
        code
    }
}

/// Cuts a name to [`MAX_NAME_LEN`] bytes without splitting a character.
pub fn clamp_name(name: &str) -> String {
    let mut end = name.len().min(MAX_NAME_LEN);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].trim().to_string()
}

pub fn is_valid_code(code: &str) -> bool {
    code.len() == CODE_LEN && code.bytes().all(|b| CODE_ALPHABET.contains(&b))
}

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    fn str(&mut self, s: &str) {
        let b = s.as_bytes();
        self.u8(b.len().min(255) as u8);
        self.bytes(&b[..b.len().min(255)]);
    }
    fn addr(&mut self, a: SocketAddr) {
        match a.ip() {
            IpAddr::V4(ip) => {
                self.u8(4);
                self.bytes(&ip.octets());
            }
            IpAddr::V6(ip) => {
                self.u8(6);
                self.bytes(&ip.octets());
            }
        }
        self.bytes(&a.port().to_be_bytes());
    }
    fn opt_addr(&mut self, a: Option<SocketAddr>) {
        match a {
            Some(a) => self.addr(a),
            None => self.u8(0),
        }
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Some(head)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.array()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.array()?))
    }
    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn addr_tagged(&mut self, tag: u8) -> Option<SocketAddr> {
        let ip = match tag {
            4 => IpAddr::V4(Ipv4Addr::from(self.array::<4>()?)),
            6 => IpAddr::V6(Ipv6Addr::from(self.array::<16>()?)),
            _ => return None,
        };
        Some(SocketAddr::new(ip, self.u16()?))
    }
    fn addr(&mut self) -> Option<SocketAddr> {
        let tag = self.u8()?;
        self.addr_tagged(tag)
    }
    /// Outer `None` = malformed, inner `None` = absent.
    fn opt_addr(&mut self) -> Option<Option<SocketAddr>> {
        match self.u8()? {
            0 => Some(None),
            tag => Some(Some(self.addr_tagged(tag)?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_every_message() {
        let a: SocketAddr = "203.0.113.7:40000".parse().unwrap();
        let b: SocketAddr = "[2001:db8::1]:5000".parse().unwrap();
        let msgs = vec![
            Msg::Create {
                name: "Val".into(),
                req: 7,
                local: Some(a),
            },
            Msg::Create {
                name: "Val".into(),
                req: 7,
                local: None,
            },
            Msg::Join {
                name: "Val".into(),
                req: 9,
                code: "ABC234".into(),
                local: Some(b),
            },
            Msg::Poll { token: [3; 16] },
            Msg::Report {
                token: [4; 16],
                direct_ok: true,
            },
            Msg::Leave { token: [5; 16] },
            Msg::Created {
                req: 1,
                code: "ZZZ999".into(),
                token: [6; 16],
                observed: a,
            },
            Msg::Joined {
                req: 2,
                token: [7; 16],
                observed: b,
            },
            Msg::Waiting { observed: a },
            Msg::Peer {
                peer_name: "Dana".into(),
                session: [1; 8],
                is_host: true,
                observed: a,
                peer_public: b,
                peer_local: None,
            },
            Msg::Decision {
                path: PathDecision::Relay,
            },
            Msg::Error {
                req: 3,
                code: ErrorCode::RoomFull,
            },
            Msg::Punch {
                session: [2; 8],
                from_host: false,
            },
            Msg::PunchAck {
                session: [2; 8],
                from_host: true,
            },
        ];
        for m in msgs {
            assert_eq!(Msg::decode(&m.encode()), Some(m));
        }
    }

    #[test]
    fn rejects_garbage_and_media() {
        assert_eq!(Msg::decode(&[]), None);
        assert_eq!(Msg::decode(&[0x0B, 0x5D, 1, 1, 0, 0]), None);
        let mut p = Msg::Poll { token: [0; 16] }.encode();
        p.truncate(10);
        assert_eq!(Msg::decode(&p), None);
        let mut p = Msg::Poll { token: [0; 16] }.encode();
        p[2] = 99;
        assert_eq!(Msg::decode(&p), None);
    }

    #[test]
    fn codes_normalize() {
        assert_eq!(normalize_code(" abcd-23xy "), "ABCD23XY");
        assert!(is_valid_code("ABCD23XY"));
        assert!(!is_valid_code("ABCD0OXY"));
        assert!(!is_valid_code("ABCD23X"));
        assert_eq!(format_code("abcd23xy"), "ABCD-23XY");
    }

    #[test]
    fn names_are_clamped_on_char_boundaries() {
        assert_eq!(clamp_name("  Val "), "Val");
        let long = "é".repeat(40); // 80 bytes
        let c = clamp_name(&long);
        assert_eq!(c.len(), 64);
        assert!(c.chars().all(|ch| ch == 'é'));
    }
}

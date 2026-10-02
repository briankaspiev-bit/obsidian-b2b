//! The rendezvous server: one UDP port that does room codes, address
//! discovery (the server reports back the public address it saw, like STUN),
//! path arbitration, and blind relaying when direct UDP fails.
//!
//! State lives in memory. A restart drops live rooms, which is acceptable for
//! private two-person sessions: both apps simply create a new room.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::proto::{
    clamp_name, is_control, is_valid_code, normalize_code, ErrorCode, Msg, PathDecision, SessionId,
    Token, CODE_ALPHABET, CODE_LEN,
};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// How long a room waits for its second DJ.
    pub unjoined_ttl: Duration,
    /// How long a joined room may sit undecided or a decided room may sit idle.
    pub idle_ttl: Duration,
    /// Relayed rooms are kept while media flows; dropped after this much silence.
    pub relay_idle_ttl: Duration,
    pub max_rooms: usize,
    pub creates_per_ip_per_minute: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            unjoined_ttl: Duration::from_secs(15 * 60),
            idle_ttl: Duration::from_secs(120),
            relay_idle_ttl: Duration::from_secs(60),
            max_rooms: 10_000,
            creates_per_ip_per_minute: 30,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub rooms_created: u64,
    pub rooms_joined: u64,
    pub direct_decisions: u64,
    pub relay_decisions: u64,
    pub relayed_packets: u64,
    pub relayed_bytes: u64,
    pub dropped_packets: u64,
}

struct Member {
    token: Token,
    name: String,
    addr: SocketAddr,
    local: Option<SocketAddr>,
    report: Option<bool>,
}

struct Room {
    session: SessionId,
    created_req: (SocketAddr, u32),
    host: Member,
    guest: Option<Member>,
    decision: PathDecision,
    last_activity: Instant,
}

impl Room {
    fn member(&mut self, token: &Token) -> Option<(&mut Member, bool)> {
        if &self.host.token == token {
            return Some((&mut self.host, true));
        }
        match &mut self.guest {
            Some(g) if &g.token == token => Some((g, false)),
            _ => None,
        }
    }
}

pub struct Server {
    cfg: ServerConfig,
    rooms: HashMap<String, Room>,
    by_token: HashMap<Token, String>,
    /// Relayed media: packet from key is forwarded to (value.0); value.1 is the room code.
    relay: HashMap<SocketAddr, (SocketAddr, String)>,
    rate: HashMap<IpAddr, (Instant, u32)>,
    pub stats: Stats,
}

impl Server {
    pub fn new(cfg: ServerConfig) -> Self {
        Server {
            cfg,
            rooms: HashMap::new(),
            by_token: HashMap::new(),
            relay: HashMap::new(),
            rate: HashMap::new(),
            stats: Stats::default(),
        }
    }

    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// Handles one inbound datagram. `send` is called for every datagram the
    /// server wants to emit (replies, or a forwarded media packet).
    pub fn handle(
        &mut self,
        now: Instant,
        src: SocketAddr,
        packet: &[u8],
        send: &mut dyn FnMut(&[u8], SocketAddr),
    ) {
        if !is_control(packet) {
            self.forward(now, src, packet, send);
            return;
        }
        let Some(msg) = Msg::decode(packet) else {
            self.stats.dropped_packets += 1;
            return;
        };
        let reply = match msg {
            Msg::Create { req, name, local } => self.create(now, src, req, &name, local),
            Msg::Join {
                req,
                code,
                name,
                local,
            } => self.join(now, src, req, &code, &name, local),
            Msg::Poll { token } => self.poll(now, src, token),
            Msg::Report { token, direct_ok } => self.report(now, src, token, direct_ok),
            Msg::Leave { token } => {
                if let Some(code) = self.by_token.get(&token).cloned() {
                    self.remove_room(&code);
                }
                None
            }
            // Server-to-client or peer-to-peer types are never valid here.
            _ => None,
        };
        if let Some(reply) = reply {
            send(&reply.encode(), src);
        }
    }

    fn forward(
        &mut self,
        now: Instant,
        src: SocketAddr,
        packet: &[u8],
        send: &mut dyn FnMut(&[u8], SocketAddr),
    ) {
        let Some((dst, code)) = self.relay.get(&src) else {
            self.stats.dropped_packets += 1;
            return;
        };
        send(packet, *dst);
        self.stats.relayed_packets += 1;
        self.stats.relayed_bytes += packet.len() as u64;
        if let Some(room) = self.rooms.get_mut(code) {
            room.last_activity = now;
        }
    }

    fn create(
        &mut self,
        now: Instant,
        src: SocketAddr,
        req: u32,
        name: &str,
        local: Option<SocketAddr>,
    ) -> Option<Msg> {
        // A retransmitted Create (our reply was lost) gets the same room back.
        if let Some((code, room)) = self.rooms.iter().find(|(_, r)| r.created_req == (src, req)) {
            return Some(Msg::Created {
                req,
                code: code.clone(),
                token: room.host.token,
                observed: src,
            });
        }
        if self.rooms.len() >= self.cfg.max_rooms || !self.allow_create(now, src.ip()) {
            return Some(Msg::Error {
                req,
                code: ErrorCode::RateLimited,
            });
        }
        let code = loop {
            let c = random_code();
            if !self.rooms.contains_key(&c) {
                break c;
            }
        };
        let token: Token = random_bytes();
        self.rooms.insert(
            code.clone(),
            Room {
                session: random_bytes(),
                created_req: (src, req),
                host: Member {
                    token,
                    name: clamp_name(name),
                    addr: src,
                    local,
                    report: None,
                },
                guest: None,
                decision: PathDecision::Pending,
                last_activity: now,
            },
        );
        self.by_token.insert(token, code.clone());
        self.stats.rooms_created += 1;
        Some(Msg::Created {
            req,
            code,
            token,
            observed: src,
        })
    }

    fn join(
        &mut self,
        now: Instant,
        src: SocketAddr,
        req: u32,
        code: &str,
        name: &str,
        local: Option<SocketAddr>,
    ) -> Option<Msg> {
        let code = normalize_code(code);
        if !is_valid_code(&code) {
            return Some(Msg::Error {
                req,
                code: ErrorCode::RoomNotFound,
            });
        }
        let Some(room) = self.rooms.get_mut(&code) else {
            return Some(Msg::Error {
                req,
                code: ErrorCode::RoomNotFound,
            });
        };
        if let Some(g) = &room.guest {
            // Retransmitted Join from the same socket: answer again.
            if g.addr == src {
                return Some(Msg::Joined {
                    req,
                    token: g.token,
                    observed: src,
                });
            }
            return Some(Msg::Error {
                req,
                code: ErrorCode::RoomFull,
            });
        }
        let token: Token = random_bytes();
        room.guest = Some(Member {
            token,
            name: clamp_name(name),
            addr: src,
            local,
            report: None,
        });
        room.last_activity = now;
        self.by_token.insert(token, code);
        self.stats.rooms_joined += 1;
        Some(Msg::Joined {
            req,
            token,
            observed: src,
        })
    }

    fn poll(&mut self, now: Instant, src: SocketAddr, token: Token) -> Option<Msg> {
        let Some(code) = self.by_token.get(&token) else {
            return Some(Msg::Error {
                req: 0,
                code: ErrorCode::BadToken,
            });
        };
        let room = self.rooms.get_mut(code)?;
        let session = room.session;
        let (me, is_host) = room.member(&token)?;
        // The mapping can move (Wi-Fi roam, NAT rebinding); always trust the latest.
        me.addr = src;
        room.last_activity = now;
        let other = if is_host {
            room.guest.as_ref()
        } else {
            Some(&room.host)
        };
        Some(match other {
            None => Msg::Waiting { observed: src },
            Some(o) => Msg::Peer {
                session,
                is_host,
                observed: src,
                peer_public: o.addr,
                peer_local: o.local,
                peer_name: o.name.clone(),
            },
        })
    }

    fn report(
        &mut self,
        now: Instant,
        src: SocketAddr,
        token: Token,
        direct_ok: bool,
    ) -> Option<Msg> {
        let Some(code) = self.by_token.get(&token).cloned() else {
            return Some(Msg::Error {
                req: 0,
                code: ErrorCode::BadToken,
            });
        };
        let room = self.rooms.get_mut(&code)?;
        room.guest.as_ref()?;
        let (me, _) = room.member(&token)?;
        me.addr = src;
        if me.report.is_none() {
            me.report = Some(direct_ok);
        }
        room.last_activity = now;

        if room.decision == PathDecision::Pending {
            let host = room.host.report;
            let guest = room.guest.as_ref().and_then(|g| g.report);
            room.decision = match (host, guest) {
                // One side failing is enough: both must agree on one path.
                (Some(false), _) | (_, Some(false)) => PathDecision::Relay,
                (Some(true), Some(true)) => PathDecision::Direct,
                _ => PathDecision::Pending,
            };
            match room.decision {
                PathDecision::Direct => self.stats.direct_decisions += 1,
                PathDecision::Relay => self.stats.relay_decisions += 1,
                PathDecision::Pending => {}
            }
        }
        if room.decision == PathDecision::Relay {
            // (Re)install forwarding from the members' latest addresses; this
            // is the same socket their media will come from.
            let host = room.host.addr;
            let guest = room.guest.as_ref()?.addr;
            self.relay.retain(|_, (_, c)| c != &code);
            self.relay.insert(host, (guest, code.clone()));
            self.relay.insert(guest, (host, code));
        }
        Some(Msg::Decision {
            path: room.decision,
        })
    }

    fn allow_create(&mut self, now: Instant, ip: IpAddr) -> bool {
        let entry = self.rate.entry(ip).or_insert((now, 0));
        if now.duration_since(entry.0) >= Duration::from_secs(60) {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= self.cfg.creates_per_ip_per_minute
    }

    fn remove_room(&mut self, code: &str) {
        if let Some(room) = self.rooms.remove(code) {
            self.by_token.remove(&room.host.token);
            if let Some(g) = room.guest {
                self.by_token.remove(&g.token);
            }
            self.relay.retain(|_, (_, c)| c != code);
        }
    }

    /// Drops expired rooms. Call about once a second.
    pub fn sweep(&mut self, now: Instant) {
        let cfg = &self.cfg;
        let expired: Vec<String> = self
            .rooms
            .iter()
            .filter(|(_, r)| {
                let idle = now.duration_since(r.last_activity);
                let ttl = match (&r.guest, r.decision) {
                    (None, _) => cfg.unjoined_ttl,
                    (Some(_), PathDecision::Relay) => cfg.relay_idle_ttl,
                    _ => cfg.idle_ttl,
                };
                idle > ttl
            })
            .map(|(c, _)| c.clone())
            .collect();
        for code in expired {
            self.remove_room(&code);
        }
        self.rate
            .retain(|_, (t, _)| now.duration_since(*t) < Duration::from_secs(60));
    }
}

/// Runs the server on an already-bound socket until an I/O error occurs.
pub fn run(socket: UdpSocket, cfg: ServerConfig, log: bool) -> io::Result<()> {
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    let mut server = Server::new(cfg);
    let mut buf = [0u8; 2048];
    let mut last_sweep = Instant::now();
    let mut last_log = Instant::now();
    loop {
        match socket.recv_from(&mut buf) {
            Ok((n, src)) => {
                let now = Instant::now();
                server.handle(now, src, &buf[..n], &mut |data, dst| {
                    // A full send buffer or an unreachable peer must not stop the loop.
                    let _ = socket.send_to(data, dst);
                });
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            // Windows reports ICMP port-unreachable as a recv error on UDP sockets.
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
            Err(e) => return Err(e),
        }
        let now = Instant::now();
        if now.duration_since(last_sweep) >= Duration::from_secs(1) {
            server.sweep(now);
            last_sweep = now;
        }
        if log && now.duration_since(last_log) >= Duration::from_secs(60) {
            let s = server.stats;
            eprintln!(
                "rooms={} created={} joined={} direct={} relay={} relayed_pkts={} relayed_mb={:.1} dropped={}",
                server.room_count(),
                s.rooms_created,
                s.rooms_joined,
                s.direct_decisions,
                s.relay_decisions,
                s.relayed_packets,
                s.relayed_bytes as f64 / 1e6,
                s.dropped_packets
            );
            last_log = now;
        }
    }
}

pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("OS random source unavailable");
    b
}

fn random_code() -> String {
    // Rejection sampling keeps every character equally likely.
    let mut out = String::with_capacity(CODE_LEN);
    while out.len() < CODE_LEN {
        let [b] = random_bytes::<1>();
        let limit = 256 - (256 % CODE_ALPHABET.len());
        if (b as usize) < limit {
            out.push(CODE_ALPHABET[b as usize % CODE_ALPHABET.len()] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn call(s: &mut Server, now: Instant, src: SocketAddr, m: Msg) -> Vec<(Msg, SocketAddr)> {
        let mut out = Vec::new();
        s.handle(now, src, &m.encode(), &mut |d, a| {
            out.push((Msg::decode(d).unwrap(), a))
        });
        out
    }

    fn setup(s: &mut Server, now: Instant, a: SocketAddr, b: SocketAddr) -> (Token, Token, String) {
        let r = call(
            s,
            now,
            a,
            Msg::Create {
                name: "Val".into(),
                req: 1,
                local: None,
            },
        );
        let Msg::Created {
            code, token: ta, ..
        } = r[0].0.clone()
        else {
            panic!("{r:?}")
        };
        let r = call(
            s,
            now,
            b,
            Msg::Join {
                name: "Val".into(),
                req: 1,
                code: code.to_lowercase(),
                local: None,
            },
        );
        let Msg::Joined { token: tb, .. } = r[0].0.clone() else {
            panic!("{r:?}")
        };
        (ta, tb, code)
    }

    #[test]
    fn create_join_poll_exchanges_addresses() {
        let mut s = Server::new(ServerConfig::default());
        let now = Instant::now();
        let (a, b) = (addr("1.1.1.1:1000"), addr("2.2.2.2:2000"));
        let r = call(
            &mut s,
            now,
            a,
            Msg::Create {
                name: "Val".into(),
                req: 1,
                local: None,
            },
        );
        let Msg::Created {
            code,
            token: ta,
            observed,
            ..
        } = r[0].0.clone()
        else {
            panic!()
        };
        assert_eq!(observed, a);
        assert!(is_valid_code(&code));
        // Retransmit returns the same room.
        let r = call(
            &mut s,
            now,
            a,
            Msg::Create {
                name: "Val".into(),
                req: 1,
                local: None,
            },
        );
        assert!(matches!(&r[0].0, Msg::Created { code: c, .. } if c == &code));
        assert_eq!(s.room_count(), 1);

        assert!(matches!(
            call(&mut s, now, a, Msg::Poll { token: ta })[0].0,
            Msg::Waiting { .. }
        ));
        let local = Some(addr("192.168.1.5:2000"));
        let r = call(
            &mut s,
            now,
            b,
            Msg::Join {
                name: "Val".into(),
                req: 5,
                code: code.clone(),
                local,
            },
        );
        let Msg::Joined { token: tb, .. } = r[0].0.clone() else {
            panic!()
        };
        let r = call(&mut s, now, a, Msg::Poll { token: ta });
        let Msg::Peer {
            peer_name,
            peer_public,
            peer_local,
            is_host,
            ..
        } = r[0].0.clone()
        else {
            panic!()
        };
        assert_eq!((peer_public, peer_local, is_host), (b, local, true));
        assert_eq!(peer_name, "Val");
        let r = call(&mut s, now, b, Msg::Poll { token: tb });
        assert!(
            matches!(r[0].0, Msg::Peer { peer_public, is_host: false, .. } if peer_public == a)
        );

        // A third DJ is refused.
        let r = call(
            &mut s,
            now,
            addr("3.3.3.3:3"),
            Msg::Join {
                name: "Val".into(),
                req: 1,
                code,
                local: None,
            },
        );
        assert!(matches!(
            r[0].0,
            Msg::Error {
                code: ErrorCode::RoomFull,
                ..
            }
        ));
    }

    #[test]
    fn unknown_code_is_not_found() {
        let mut s = Server::new(ServerConfig::default());
        let r = call(
            &mut s,
            Instant::now(),
            addr("1.1.1.1:1"),
            Msg::Join {
                name: "Val".into(),
                req: 2,
                code: "ZZZZZZZZ".into(),
                local: None,
            },
        );
        assert_eq!(
            r[0].0,
            Msg::Error {
                req: 2,
                code: ErrorCode::RoomNotFound
            }
        );
    }

    #[test]
    fn both_direct_means_direct_and_no_relay() {
        let mut s = Server::new(ServerConfig::default());
        let now = Instant::now();
        let (a, b) = (addr("1.1.1.1:1000"), addr("2.2.2.2:2000"));
        let (ta, tb, _) = setup(&mut s, now, a, b);
        let r = call(
            &mut s,
            now,
            a,
            Msg::Report {
                token: ta,
                direct_ok: true,
            },
        );
        assert_eq!(
            r[0].0,
            Msg::Decision {
                path: PathDecision::Pending
            }
        );
        let r = call(
            &mut s,
            now,
            b,
            Msg::Report {
                token: tb,
                direct_ok: true,
            },
        );
        assert_eq!(
            r[0].0,
            Msg::Decision {
                path: PathDecision::Direct
            }
        );
        let mut out = 0;
        s.handle(now, a, &[0x0B, 0x5D, 1, 2, 3], &mut |_, _| out += 1);
        assert_eq!(out, 0, "no relaying for a direct room");
    }

    #[test]
    fn one_failure_means_relay_and_media_is_forwarded() {
        let mut s = Server::new(ServerConfig::default());
        let now = Instant::now();
        let (a, b) = (addr("1.1.1.1:1000"), addr("2.2.2.2:2000"));
        let (ta, tb, _) = setup(&mut s, now, a, b);
        assert_eq!(
            call(
                &mut s,
                now,
                a,
                Msg::Report {
                    token: ta,
                    direct_ok: true
                }
            )[0]
            .0,
            Msg::Decision {
                path: PathDecision::Pending
            }
        );
        assert_eq!(
            call(
                &mut s,
                now,
                b,
                Msg::Report {
                    token: tb,
                    direct_ok: false
                }
            )[0]
            .0,
            Msg::Decision {
                path: PathDecision::Relay
            }
        );
        // The host keeps asking and learns the outcome too.
        assert_eq!(
            call(
                &mut s,
                now,
                a,
                Msg::Report {
                    token: ta,
                    direct_ok: true
                }
            )[0]
            .0,
            Msg::Decision {
                path: PathDecision::Relay
            }
        );

        let media = [0x0B, 0x5D, 9, 9, 9];
        let mut out = Vec::new();
        s.handle(now, a, &media, &mut |d, to| out.push((d.to_vec(), to)));
        s.handle(now, b, &media, &mut |d, to| out.push((d.to_vec(), to)));
        s.handle(now, addr("6.6.6.6:6"), &media, &mut |d, to| {
            out.push((d.to_vec(), to))
        });
        assert_eq!(out, vec![(media.to_vec(), b), (media.to_vec(), a)]);
        assert_eq!(s.stats.relayed_packets, 2);
        assert_eq!(s.stats.dropped_packets, 1);
    }

    #[test]
    fn rooms_expire_and_relay_stops() {
        let cfg = ServerConfig::default();
        let mut s = Server::new(cfg.clone());
        let now = Instant::now();
        let (a, b) = (addr("1.1.1.1:1000"), addr("2.2.2.2:2000"));
        let (ta, tb, _) = setup(&mut s, now, a, b);
        call(
            &mut s,
            now,
            a,
            Msg::Report {
                token: ta,
                direct_ok: false,
            },
        );
        call(
            &mut s,
            now,
            b,
            Msg::Report {
                token: tb,
                direct_ok: false,
            },
        );
        // Media keeps a relayed room alive.
        let later = now + cfg.relay_idle_ttl - Duration::from_secs(1);
        s.handle(later, a, &[0, 0, 0, 0], &mut |_, _| {});
        s.sweep(later + Duration::from_secs(2));
        assert_eq!(s.room_count(), 1);
        s.sweep(later + cfg.relay_idle_ttl + Duration::from_secs(1));
        assert_eq!(s.room_count(), 0);
        let mut out = 0;
        s.handle(now, a, &[0, 0, 0, 0], &mut |_, _| out += 1);
        assert_eq!(out, 0);
    }

    #[test]
    fn create_is_rate_limited_per_ip() {
        let mut s = Server::new(ServerConfig {
            creates_per_ip_per_minute: 2,
            ..Default::default()
        });
        let now = Instant::now();
        for (i, port) in [1u16, 2, 3].iter().enumerate() {
            let r = call(
                &mut s,
                now,
                addr(&format!("9.9.9.9:{port}")),
                Msg::Create {
                    name: "Val".into(),
                    req: i as u32,
                    local: None,
                },
            );
            assert_eq!(
                matches!(
                    r[0].0,
                    Msg::Error {
                        code: ErrorCode::RateLimited,
                        ..
                    }
                ),
                i == 2
            );
        }
    }
}

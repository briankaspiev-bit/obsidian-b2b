//! Client side: create or join a room, punch a hole to the other DJ, and hand
//! back a UDP socket plus the address to send media to.
//!
//! Everything happens on the one socket the engine will later use for media,
//! so the public address the server observed, the punched NAT mapping and the
//! relay registration all belong to that socket.
//!
//! ```no_run
//! use obsidian_rendezvous::client::{ClientConfig, create_room};
//! let cfg = ClientConfig::new("rendezvous.example.com:3478".parse().unwrap());
//! let host = create_room(&cfg, "Val").unwrap();
//! println!("room code: {}", host.code());
//! let link = host.wait_for_guest(std::time::Duration::from_secs(600)).unwrap();
//! // obsidian_engine::run_peer_with_socket(PeerConfig { peer: link.peer_addr, .. }, link.socket)
//! ```

use std::fmt;
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::proto::{ErrorCode, Msg, PathDecision, SessionId, Token};
use crate::server::random_bytes;

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub server: SocketAddr,
    /// How long to try direct UDP before falling back to the relay.
    pub punch_timeout: Duration,
    /// Skip hole punching and always use the relay (testing, or networks known to block it).
    pub force_relay: bool,
}

impl ClientConfig {
    pub fn new(server: SocketAddr) -> Self {
        ClientConfig {
            server,
            punch_timeout: Duration::from_secs(3),
            force_relay: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    /// Straight to the other laptop.
    Direct,
    /// Through the rendezvous server's relay.
    Relay,
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The server did not answer at all (wrong address, server down, UDP blocked).
    ServerUnreachable,
    RoomNotFound,
    RoomFull,
    RateLimited,
    /// Nobody joined (host) or the handshake stalled.
    Timeout,
    Protocol(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "network error: {e}"),
            Error::ServerUnreachable => write!(f, "could not reach the rendezvous server"),
            Error::RoomNotFound => write!(f, "no room with that code (it may have expired)"),
            Error::RoomFull => write!(f, "that room already has two DJs"),
            Error::RateLimited => write!(
                f,
                "too many rooms created from this network, try again in a minute"
            ),
            Error::Timeout => write!(f, "timed out"),
            Error::Protocol(m) => write!(f, "protocol error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<ErrorCode> for Error {
    fn from(c: ErrorCode) -> Self {
        match c {
            ErrorCode::RoomNotFound => Error::RoomNotFound,
            ErrorCode::RoomFull => Error::RoomFull,
            ErrorCode::RateLimited => Error::RateLimited,
            ErrorCode::BadToken => Error::Protocol("server forgot our room"),
            ErrorCode::Unknown(_) => Error::Protocol("unknown error code"),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A connected pair. Give `socket` to the engine and send media to `peer_addr`.
#[derive(Debug)]
pub struct Connection {
    pub socket: UdpSocket,
    /// The other DJ's address on a direct path, or the relay's address.
    pub peer_addr: SocketAddr,
    pub path: Path,
    /// True for the DJ who created the room.
    pub is_host: bool,
    /// The other DJ's display name.
    pub peer_name: String,
    /// Our address as the internet sees it.
    pub public_addr: SocketAddr,
    /// Shared by both peers; usable as a key-derivation input or log tag.
    pub session: SessionId,
    server: SocketAddr,
    token: Token,
}

impl Connection {
    /// Tells the server we are done so a relayed room is freed immediately
    /// instead of after its idle timeout. Best effort.
    pub fn leave(&self) {
        let _ = self
            .socket
            .send_to(&Msg::Leave { token: self.token }.encode(), self.server);
    }
}

/// A created room waiting for the second DJ.
pub struct HostRoom {
    socket: UdpSocket,
    cfg: ClientConfig,
    code: String,
    token: Token,
}

impl HostRoom {
    /// Raw code, e.g. `ABCD2345`. Show it with [`crate::proto::format_code`].
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Blocks until the guest joins and a path is chosen.
    pub fn wait_for_guest(self, timeout: Duration) -> Result<Connection> {
        connect(self.socket, &self.cfg, self.token, timeout)
    }
}

/// Binds a fresh socket and creates a room.
/// `name` is shown to the other DJ.
pub fn create_room(cfg: &ClientConfig, name: &str) -> Result<HostRoom> {
    create_room_with_socket(cfg, name, bind_for(cfg.server)?)
}

pub fn create_room_with_socket(
    cfg: &ClientConfig,
    name: &str,
    socket: UdpSocket,
) -> Result<HostRoom> {
    let local = local_candidate(&socket, cfg.server);
    let req = u32::from_be_bytes(random_bytes());
    let msg = Msg::Create {
        req,
        name: name.to_string(),
        local,
    };
    let reply = request(&socket, cfg.server, &msg, |m| match m {
        Msg::Created { req: r, .. } | Msg::Error { req: r, .. } => *r == req,
        _ => false,
    })?;
    match reply {
        Msg::Created { code, token, .. } => Ok(HostRoom {
            socket,
            cfg: cfg.clone(),
            code,
            token,
        }),
        Msg::Error { code, .. } => Err(code.into()),
        _ => unreachable!(),
    }
}

/// Binds a fresh socket, joins the room and connects to its host.
/// Accepts the code with or without its dash, in any case.
pub fn join_room(cfg: &ClientConfig, code: &str, name: &str) -> Result<Connection> {
    join_room_with_socket(cfg, code, name, bind_for(cfg.server)?)
}

pub fn join_room_with_socket(
    cfg: &ClientConfig,
    code: &str,
    name: &str,
    socket: UdpSocket,
) -> Result<Connection> {
    let local = local_candidate(&socket, cfg.server);
    let req = u32::from_be_bytes(random_bytes());
    let msg = Msg::Join {
        req,
        code: code.to_string(),
        name: name.to_string(),
        local,
    };
    let reply = request(&socket, cfg.server, &msg, |m| match m {
        Msg::Joined { req: r, .. } | Msg::Error { req: r, .. } => *r == req,
        _ => false,
    })?;
    match reply {
        Msg::Joined { token, .. } => connect(socket, cfg, token, Duration::from_secs(30)),
        Msg::Error { code, .. } => Err(code.into()),
        _ => unreachable!(),
    }
}

fn bind_for(server: SocketAddr) -> io::Result<UdpSocket> {
    let any: SocketAddr = match server {
        SocketAddr::V4(_) => "0.0.0.0:0".parse().unwrap(),
        SocketAddr::V6(_) => "[::]:0".parse().unwrap(),
    };
    UdpSocket::bind(any)
}

/// Our LAN address + the socket's port, so two DJs on the same network can
/// talk directly even when their router does not support hairpinning.
fn local_candidate(socket: &UdpSocket, server: SocketAddr) -> Option<SocketAddr> {
    let port = socket.local_addr().ok()?.port();
    // Connecting a throwaway UDP socket sends nothing; it only asks the OS
    // which interface would route to the server.
    let probe = bind_for(server).ok()?;
    probe.connect(server).ok()?;
    let ip = probe.local_addr().ok()?.ip();
    if ip.is_unspecified() {
        return None;
    }
    Some(SocketAddr::new(ip, port))
}

const RETRY: Duration = Duration::from_millis(250);
const SERVER_ATTEMPTS: u32 = 20;

/// Sends `msg` to the server until a matching reply arrives.
fn request(
    socket: &UdpSocket,
    server: SocketAddr,
    msg: &Msg,
    matches: impl Fn(&Msg) -> bool,
) -> Result<Msg> {
    let packet = msg.encode();
    let mut buf = [0u8; 2048];
    for _ in 0..SERVER_ATTEMPTS {
        socket.send_to(&packet, server)?;
        let deadline = Instant::now() + RETRY;
        while let Some((m, from)) = recv_until(socket, &mut buf, deadline)? {
            if from == server && matches(&m) {
                return Ok(m);
            }
        }
    }
    Err(Error::ServerUnreachable)
}

/// Receives control messages until `deadline`; returns `None` on timeout.
/// Non-control packets are discarded.
fn recv_until(
    socket: &UdpSocket,
    buf: &mut [u8],
    deadline: Instant,
) -> Result<Option<(Msg, SocketAddr)>> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(None);
        }
        socket.set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
        match socket.recv_from(buf) {
            Ok((n, from)) => {
                if let Some(m) = Msg::decode(&buf[..n]) {
                    return Ok(Some((m, from)));
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None)
            }
            // ICMP unreachable from a dead candidate (reported on Windows and Linux).
            Err(e)
                if e.kind() == io::ErrorKind::ConnectionReset
                    || e.kind() == io::ErrorKind::ConnectionRefused => {}
            Err(e) => return Err(e.into()),
        }
    }
}

struct PeerInfo {
    session: SessionId,
    is_host: bool,
    observed: SocketAddr,
    candidates: Vec<SocketAddr>,
    peer_name: String,
}

fn connect(
    socket: UdpSocket,
    cfg: &ClientConfig,
    token: Token,
    wait: Duration,
) -> Result<Connection> {
    let peer = wait_for_peer(&socket, cfg.server, token, wait)?;
    let direct = if cfg.force_relay {
        None
    } else {
        punch(&socket, &peer, cfg.punch_timeout)?
    };
    let decision = arbitrate(&socket, cfg.server, token, &peer, direct.is_some())?;
    drain(&socket)?;
    let (path, peer_addr) = match (decision, direct) {
        (PathDecision::Direct, Some(addr)) => (Path::Direct, addr),
        (PathDecision::Relay, _) => (Path::Relay, cfg.server),
        _ => {
            return Err(Error::Protocol(
                "server chose direct but we have no direct path",
            ))
        }
    };
    Ok(Connection {
        socket,
        peer_addr,
        path,
        is_host: peer.is_host,
        peer_name: peer.peer_name,
        public_addr: peer.observed,
        session: peer.session,
        server: cfg.server,
        token,
    })
}

fn wait_for_peer(
    socket: &UdpSocket,
    server: SocketAddr,
    token: Token,
    wait: Duration,
) -> Result<PeerInfo> {
    let poll = Msg::Poll { token }.encode();
    let give_up = Instant::now() + wait;
    let mut buf = [0u8; 2048];
    let mut last_reply = Instant::now();
    while Instant::now() < give_up {
        socket.send_to(&poll, server)?;
        let deadline = Instant::now() + RETRY;
        while let Some((m, from)) = recv_until(socket, &mut buf, deadline)? {
            if from != server {
                continue;
            }
            last_reply = Instant::now();
            match m {
                Msg::Peer {
                    session,
                    is_host,
                    observed,
                    peer_public,
                    peer_local,
                    peer_name,
                } => {
                    let mut candidates = vec![peer_public];
                    // Only worth trying a LAN address when it differs from the public one.
                    if let Some(l) = peer_local.filter(|l| *l != peer_public && is_private(l.ip()))
                    {
                        candidates.push(l);
                    }
                    return Ok(PeerInfo {
                        session,
                        is_host,
                        observed,
                        candidates,
                        peer_name,
                    });
                }
                Msg::Error { code, .. } => return Err(code.into()),
                _ => {}
            }
        }
        if last_reply.elapsed() > RETRY * SERVER_ATTEMPTS {
            return Err(Error::ServerUnreachable);
        }
        // Pace polls; the inner loop already waited RETRY.
    }
    Err(Error::Timeout)
}

const PUNCH_INTERVAL: Duration = Duration::from_millis(20);

/// Sprays probes at every candidate and returns the address that answered.
///
/// A path counts as working only once we get a `PunchAck`: that proves our
/// probe reached the peer *and* its reply reached us.
fn punch(socket: &UdpSocket, peer: &PeerInfo, timeout: Duration) -> Result<Option<SocketAddr>> {
    let probe = Msg::Punch {
        session: peer.session,
        from_host: peer.is_host,
    }
    .encode();
    let ack = Msg::PunchAck {
        session: peer.session,
        from_host: peer.is_host,
    }
    .encode();
    let give_up = Instant::now() + timeout;
    let mut buf = [0u8; 2048];
    while Instant::now() < give_up {
        for c in &peer.candidates {
            // Unroutable candidates (e.g. a LAN address from another network) just fail.
            let _ = socket.send_to(&probe, *c);
        }
        let deadline = (Instant::now() + PUNCH_INTERVAL).min(give_up);
        while let Some((m, from)) = recv_until(socket, &mut buf, deadline)? {
            match m {
                Msg::Punch { session, from_host }
                    if session == peer.session && from_host != peer.is_host =>
                {
                    // Reply to wherever it really came from; with port-changing
                    // NATs that may not be any candidate we were given.
                    socket.send_to(&ack, from)?;
                }
                Msg::PunchAck { session, from_host }
                    if session == peer.session && from_host != peer.is_host =>
                {
                    // Linger briefly so the peer also gets acks for its probes.
                    answer_punches(socket, peer, &ack, Duration::from_millis(100))?;
                    return Ok(Some(from));
                }
                _ => {}
            }
        }
    }
    Ok(None)
}

fn answer_punches(
    socket: &UdpSocket,
    peer: &PeerInfo,
    ack: &[u8],
    for_how_long: Duration,
) -> Result<()> {
    let until = Instant::now() + for_how_long;
    let mut buf = [0u8; 2048];
    while let Some((m, from)) = recv_until(socket, &mut buf, until)? {
        if let Msg::Punch { session, from_host } = m {
            if session == peer.session && from_host != peer.is_host {
                socket.send_to(ack, from)?;
            }
        }
    }
    Ok(())
}

/// Tells the server whether direct worked and waits for the joint decision,
/// answering the peer's late probes meanwhile.
fn arbitrate(
    socket: &UdpSocket,
    server: SocketAddr,
    token: Token,
    peer: &PeerInfo,
    direct_ok: bool,
) -> Result<PathDecision> {
    let report = Msg::Report { token, direct_ok }.encode();
    let ack = Msg::PunchAck {
        session: peer.session,
        from_host: peer.is_host,
    }
    .encode();
    // The other side may still be punching for up to its own timeout.
    let give_up = Instant::now() + Duration::from_secs(20);
    let mut buf = [0u8; 2048];
    while Instant::now() < give_up {
        socket.send_to(&report, server)?;
        let deadline = Instant::now() + Duration::from_millis(100);
        while let Some((m, from)) = recv_until(socket, &mut buf, deadline)? {
            match m {
                Msg::Decision { path } if from == server && path != PathDecision::Pending => {
                    return Ok(path)
                }
                Msg::Error { code, .. } if from == server => return Err(code.into()),
                Msg::Punch { session, from_host }
                    if session == peer.session && from_host != peer.is_host =>
                {
                    socket.send_to(&ack, from)?;
                }
                _ => {}
            }
        }
    }
    Err(Error::Timeout)
}

/// Discards queued handshake packets so the engine starts on a clean socket.
/// Probes still in flight can arrive later; the engine must ignore packets
/// that start with [`crate::proto::MAGIC`].
fn drain(socket: &UdpSocket) -> io::Result<()> {
    socket.set_nonblocking(true)?;
    let mut buf = [0u8; 2048];
    while socket.recv_from(&mut buf).is_ok() {}
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(None)
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local() || v4.is_loopback(),
        // fc00::/7 unique local
        IpAddr::V6(v6) => (v6.segments()[0] & 0xfe00) == 0xfc00 || v6.is_loopback(),
    }
}

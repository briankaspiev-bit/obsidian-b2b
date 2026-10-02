//! End-to-end on loopback: real sockets, real server loop, both clients.
//! NAT behavior is covered by `nat/run-nat-tests.sh`.

use std::net::{SocketAddr, UdpSocket};
use std::thread;
use std::time::Duration;

use obsidian_rendezvous::client::{create_room, join_room, ClientConfig, Connection, Error, Path};
use obsidian_rendezvous::proto::is_control;
use obsidian_rendezvous::server::{run, ServerConfig};

fn start_server() -> SocketAddr {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = sock.local_addr().unwrap();
    thread::spawn(move || run(sock, ServerConfig::default(), false));
    addr
}

fn pair(cfg: ClientConfig) -> (Connection, Connection) {
    let host = create_room(&cfg, "Val").unwrap();
    let code = host.code().to_lowercase();
    let guest_cfg = cfg.clone();
    let guest = thread::spawn(move || join_room(&guest_cfg, &code, "Dana").unwrap());
    let h = host.wait_for_guest(Duration::from_secs(10)).unwrap();
    (h, guest.join().unwrap())
}

/// Sends engine-shaped packets (magic 0x0B5D) both ways and checks they arrive.
fn exchange_media(a: &Connection, b: &Connection) {
    for s in [&a.socket, &b.socket] {
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    }
    a.socket
        .send_to(&[0x0B, 0x5D, 1, 2, 3], a.peer_addr)
        .unwrap();
    assert_eq!(recv_media(&b.socket), vec![0x0B, 0x5D, 1, 2, 3]);
    b.socket.send_to(&[0x0B, 0x5D, 4, 5], b.peer_addr).unwrap();
    assert_eq!(recv_media(&a.socket), vec![0x0B, 0x5D, 4, 5]);
}

/// Like the engine must: skip late rendezvous packets (magic 0x0B52).
fn recv_media(s: &UdpSocket) -> Vec<u8> {
    let mut buf = [0u8; 64];
    loop {
        let (n, _) = s.recv_from(&mut buf).unwrap();
        if !is_control(&buf[..n]) {
            return buf[..n].to_vec();
        }
    }
}

#[test]
fn connects_directly_when_nothing_blocks() {
    let server = start_server();
    let (h, g) = pair(ClientConfig::new(server));
    assert_eq!((h.path, g.path), (Path::Direct, Path::Direct));
    assert!(h.is_host && !g.is_host);
    assert_eq!(h.session, g.session);
    assert_eq!((h.peer_name.as_str(), g.peer_name.as_str()), ("Dana", "Val"));
    assert_eq!(h.peer_addr.port(), g.socket.local_addr().unwrap().port());
    exchange_media(&h, &g);
}

#[test]
fn falls_back_to_relay_and_forwards_media() {
    let server = start_server();
    let mut cfg = ClientConfig::new(server);
    cfg.force_relay = true;
    let (h, g) = pair(cfg);
    assert_eq!((h.path, g.path), (Path::Relay, Path::Relay));
    assert_eq!(h.peer_addr, server);
    exchange_media(&h, &g);
}

#[test]
fn one_side_failing_puts_both_on_relay() {
    let server = start_server();
    let host = create_room(&ClientConfig::new(server), "Val").unwrap();
    let code = host.code().to_string();
    let mut relay_cfg = ClientConfig::new(server);
    relay_cfg.force_relay = true;
    let guest = thread::spawn(move || join_room(&relay_cfg, &code, "Dana").unwrap());
    let h = host.wait_for_guest(Duration::from_secs(10)).unwrap();
    let g = guest.join().unwrap();
    assert_eq!((h.path, g.path), (Path::Relay, Path::Relay));
    exchange_media(&h, &g);
}

#[test]
fn bad_code_and_full_room_are_reported() {
    let server = start_server();
    let cfg = ClientConfig::new(server);
    assert!(matches!(
        join_room(&cfg, "ZZZZ-ZZZZ", "Dana"),
        Err(Error::RoomNotFound)
    ));
    let (_h, _g) = pair(cfg.clone());
    // The pair's room is full; a new room code is needed for a third DJ.
    let host = create_room(&cfg, "Val").unwrap();
    let code = host.code().to_string();
    let c2 = cfg.clone();
    let code2 = code.clone();
    let t = thread::spawn(move || join_room(&c2, &code2, "Dana"));
    let _first = host.wait_for_guest(Duration::from_secs(10)).unwrap();
    t.join().unwrap().unwrap();
    assert!(matches!(
        join_room(&cfg, &code, "Dana"),
        Err(Error::RoomFull)
    ));
}

#[test]
fn unreachable_server_is_reported() {
    // Bind and drop to get a port nobody listens on.
    let dead = UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let r = create_room(&ClientConfig::new(dead), "Val");
    assert!(matches!(r, Err(Error::ServerUnreachable)), "{:?}", r.err());
}

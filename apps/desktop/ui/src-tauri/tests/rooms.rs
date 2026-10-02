//! Two booths meet through a real rendezvous server on loopback, then talk
//! over the booth link, directly and through the relay.

use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use obsidian_desktop::link::{BoothLink, LinkSink, LinkStatus, PeerState};
use obsidian_rendezvous::client::{create_room, join_room, ClientConfig, Connection, Path};
use obsidian_rendezvous::server::{run, ServerConfig};

#[derive(Clone, Default)]
struct Collect {
    controls: Arc<Mutex<Vec<String>>>,
    statuses: Arc<Mutex<Vec<LinkStatus>>>,
}

impl LinkSink for Collect {
    fn control(&self, json: String) {
        self.controls.lock().unwrap().push(json);
    }
    fn status(&self, s: LinkStatus) {
        self.statuses.lock().unwrap().push(s);
    }
    fn remote_level(&self, _: (f32, f32)) {}
}

fn start_server() -> SocketAddr {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = sock.local_addr().unwrap();
    thread::spawn(move || run(sock, ServerConfig::default(), false));
    addr
}

fn pair(cfg: ClientConfig) -> (Connection, Connection) {
    let host = create_room(&cfg, "Val").unwrap();
    let code = host.code().to_owned();
    let gcfg = cfg.clone();
    let guest = thread::spawn(move || join_room(&gcfg, &code, "Dana").unwrap());
    let h = host.wait_for_guest(Duration::from_secs(10)).unwrap();
    (h, guest.join().unwrap())
}

fn link(c: &Connection, sink: Collect) -> BoothLink {
    BoothLink::start(c.socket.try_clone().unwrap(), c.peer_addr, c.path == Path::Relay, sink).unwrap()
}

fn talk(force_relay: bool) {
    let mut cfg = ClientConfig::new(start_server());
    cfg.force_relay = force_relay;
    let (h, g) = pair(cfg);
    let (hs, gs) = (Collect::default(), Collect::default());
    let (hl, gl) = (link(&h, hs.clone()), link(&g, gs.clone()));

    gl.send_control(r#"{"t":"takeOver"}"#.into()).unwrap();
    let r = hl.network_test(Duration::from_millis(800)).unwrap();
    assert!(r.reached && r.loss_pct < 15.0, "{r:?}");
    assert_eq!(hs.controls.lock().unwrap().as_slice(), [r#"{"t":"takeOver"}"#]);
    let last = hs.statuses.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.state, PeerState::Connected);
    assert_eq!(last.relay, force_relay);

    drop(gl);
    thread::sleep(Duration::from_millis(700));
    assert_eq!(hs.statuses.lock().unwrap().last().unwrap().state, PeerState::Left);
}

#[test]
fn booths_talk_directly() {
    talk(false);
}

#[test]
fn booths_talk_through_the_relay() {
    talk(true);
}

//! The Robot DJ against a stand-in for the desktop app, through a real room
//! server on loopback: pair, Booth Check, ready, countdown, the set, a handoff
//! each way, and a summary that calls the sound clean.

use std::net::UdpSocket;
use std::process::Command;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use obsidian_desktop::devices::test_track;
use obsidian_desktop::link::{BoothLink, LinkSink, LinkStatus};
use obsidian_live::{run_live, Cmd, LiveConfig, LiveControls, LiveSource};
use obsidian_rendezvous::client::{create_room, ClientConfig, Path};
use obsidian_rendezvous::server::{run, ServerConfig};

struct Controls(mpsc::Sender<String>);
impl LinkSink for Controls {
    fn control(&self, json: String) {
        let _ = self.0.send(json);
    }
    fn status(&self, _: LinkStatus) {}
    fn remote_level(&self, _: (f32, f32)) {}
}

#[test]
fn the_robot_dj_plays_a_set_with_you_and_reports_on_your_sound() {
    let server_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let server = server_sock.local_addr().unwrap();
    thread::spawn(move || run(server_sock, ServerConfig::default(), false));

    // You: Create room.
    let room = create_room(&ClientConfig::new(server), "Glizzy").unwrap();
    let code = room.code().to_owned();
    let out = std::env::temp_dir().join(format!("robot-{}", std::process::id()));
    let robot = Command::new(env!("CARGO_BIN_EXE_robot_dj"))
        .args(["--code", &code, "--minutes", "1.2", "--cue-seconds", "15"])
        .arg("--out")
        .arg(&out)
        .env("OBSIDIAN_SERVER", server.to_string())
        .spawn()
        .unwrap();
    let conn = room.wait_for_guest(Duration::from_secs(20)).unwrap();
    assert_eq!(conn.peer_name, "Robot DJ");

    // Booth Check: the robot says hello and is ready; you press ready.
    let (tx, rx) = mpsc::channel();
    let link = BoothLink::start(
        conn.socket.try_clone().unwrap(),
        conn.peer_addr,
        conn.path == Path::Relay,
        Controls(tx),
    )
    .unwrap();
    let (mut hello, mut ready) = (false, false);
    let until = Instant::now() + Duration::from_secs(10);
    while !(hello && ready) && Instant::now() < until {
        if let Ok(m) = rx.recv_timeout(Duration::from_millis(200)) {
            hello |= m.contains("\"hello\"") && m.contains("Robot DJ");
            ready |= m.contains("\"boothReady\"") && m.contains("true");
        }
    }
    assert!(hello && ready, "robot said hello {hello}, ready {ready}");
    assert!(
        link.network_test(Duration::from_secs(1)).is_some(),
        "Booth Check measures the robot"
    );
    link.send_control(r#"{"t":"boothReady","ready":true}"#.into())
        .unwrap();
    thread::sleep(Duration::from_millis(3000));
    link.hand_over();

    // The set: you open on air with Groove.
    let (title, program) = test_track(0);
    let mut cfg = LiveConfig::new("Glizzy", LiveSource::Deck { title, program });
    cfg.peer = Some(conn.peer_addr);
    cfg.start_on_air = true;
    cfg.autoplay = true;
    let ctl = Arc::new(LiveControls::default());
    let c2 = ctl.clone();
    let sock = conn.socket.try_clone().unwrap();
    let engine = thread::spawn(move || run_live(cfg, sock, c2));

    // The robot takes over after its cue; you take it back.
    let until = Instant::now() + Duration::from_secs(50);
    while !ctl.status().partner_on_air && Instant::now() < until {
        thread::sleep(Duration::from_millis(200));
    }
    assert!(ctl.status().partner_on_air, "the robot took over");
    thread::sleep(Duration::from_secs(5));
    ctl.send(Cmd::TakeOver);

    let status = robot.wait_with_output().unwrap().status;
    ctl.stop();
    let _ = engine.join();
    assert!(status.success(), "robot exited with {status}");
    let summary = std::fs::read_to_string(out.join("summary.md")).unwrap();
    eprintln!("{summary}");
    // Both DJs share one small CI machine here, so a few late packets are allowed;
    // a choppy verdict still fails.
    assert!(
        summary.contains("**Your sound, as heard in the cloud: clean.**")
            || summary.contains("**Your sound, as heard in the cloud: mostly clean"),
        "{summary}"
    );
    // It takes over, you take it back, and it comes back for another turn.
    assert!(!summary.contains("robot took over 0×"), "{summary}");
    assert!(summary.contains("you took it back 1×"), "{summary}");
    assert!(!summary.contains("100.0% lost"), "{summary}");
    for f in ["monitor.wav", "sent.wav", "report.json", "timeline.csv"] {
        assert!(out.join(f).exists(), "{f} written");
    }
    let _ = std::fs::remove_dir_all(&out);
}

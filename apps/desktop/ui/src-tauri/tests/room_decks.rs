//! A live room between two DJs with no mixer: each picked "Test music", so the
//! room plays it on the built-in deck. The opener is already playing; the other
//! DJ can play, SYNC and TAKE OVER, like in Practice.

use std::net::UdpSocket;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use obsidian_desktop::devices::{open_input, TEST_MUSIC};
use obsidian_desktop::live::Source;
use obsidian_live::{run_live, Cmd, LiveConfig, LiveControls, LiveSource, LiveStatus};

fn wait_for(ctl: &LiveControls, secs: u64, ok: impl Fn(&LiveStatus) -> bool) -> LiveStatus {
    let until = Instant::now() + Duration::from_secs(secs);
    loop {
        let st = ctl.status();
        if ok(&st) || Instant::now() > until {
            return st;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// The deck a room would play for "Test music" input `i`.
fn room_deck(i: usize) -> LiveSource {
    match Source::for_room(open_input(TEST_MUSIC[i].0).unwrap()) {
        Source::Deck { title, program } => LiveSource::Deck { title, program },
        Source::Capture(_) => panic!("Test music should play on the built-in deck"),
    }
}

#[test]
fn without_a_mixer_both_djs_mix_on_the_built_in_decks() {
    let (sa, sb) = (
        UdpSocket::bind("127.0.0.1:0").unwrap(),
        UdpSocket::bind("127.0.0.1:0").unwrap(),
    );
    let (aa, ab) = (sa.local_addr().unwrap(), sb.local_addr().unwrap());

    // Same settings LiveRun::start gives a room.
    let mut ca = LiveConfig::new("Glizzy", room_deck(0));
    ca.peer = Some(ab);
    ca.start_on_air = true;
    ca.autoplay = true;
    let mut cb = LiveConfig::new("Julio", room_deck(1));
    cb.peer = Some(aa);

    let (a, b) = (Arc::new(LiveControls::default()), Arc::new(LiveControls::default()));
    let (a2, b2) = (a.clone(), b.clone());
    let ja = thread::spawn(move || run_live(ca, sa, a2));
    let jb = thread::spawn(move || run_live(cb, sb, b2));

    // The opener's deck is playing on air; Julio hears it and is cued, not playing.
    let st = wait_for(&b, 25, |s| s.partner_on_air && s.partner_beat.is_some());
    assert!(st.partner_on_air, "Glizzy on air: {st:?}");
    let da = a.status().deck.expect("Glizzy has a deck");
    assert!(da.playing && da.title.contains("Groove"), "{da:?}");
    let db = st.deck.expect("Julio has a deck");
    assert!(!db.playing && db.title.contains("Bells"), "{db:?}");

    // Julio plays and syncs to what he hears.
    b.send(Cmd::DeckPlayPause);
    b.send(Cmd::SyncToggle);
    let st = wait_for(&b, 20, |s| {
        s.deck
            .as_ref()
            .map(|d| d.playing && d.sync && d.sync_err_ms.map(|e| e.abs() < 3.0).unwrap_or(false))
            .unwrap_or(false)
    });
    let d = st.deck.expect("deck");
    assert!(d.playing && d.sync, "Julio synced: {d:?}");

    // TAKE OVER: Julio goes on air, Glizzy comes off.
    b.send(Cmd::TakeOver);
    let st = wait_for(&b, 15, |s| s.on_air);
    assert!(st.on_air, "Julio took over: {st:?}");
    let st = wait_for(&a, 15, |s| !s.on_air && s.partner_on_air);
    assert!(!st.on_air && st.partner_on_air, "Glizzy handed over: {st:?}");

    a.stop();
    b.stop();
    ja.join().unwrap().unwrap();
    jb.join().unwrap().unwrap();
}

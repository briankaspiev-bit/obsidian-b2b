//! Practice against the ghost DJ, the way the app runs it but without a sound
//! card: the DJ screen gets waveform columns and both decks' beats, and once
//! SYNC locks your beats land on the ghost's (what the phase meter shows).

use std::net::UdpSocket;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use obsidian_desktop::live::{practice_track, PRACTICE_PATH};
use obsidian_live::{
    run_live, Cmd, GhostPlan, GhostSession, LiveConfig, LiveControls, LiveSource, LiveStatus,
};

fn ghost_plan() -> GhostPlan {
    let a = obsidian_testaudio::track(&obsidian_testaudio::dj_a());
    let mut plan = GhostPlan::new(vec![("Ghost groove A".into(), Arc::new(a))]);
    plan.return_after_s = None;
    plan
}

/// Your beat minus the partner's, folded into half a beat either side (ms).
fn phase_ms(st: &LiveStatus) -> Option<f64> {
    let (y, p) = (st.you_beat?, st.partner_beat?);
    let d = (y.beat_ms - p.beat_ms).rem_euclid(p.period_ms);
    Some(if d > p.period_ms / 2.0 {
        d - p.period_ms
    } else {
        d
    })
}

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

#[test]
fn the_dj_screen_sees_both_decks_beats_and_sync_lines_them_up() {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let ghost =
        GhostSession::start(sock.local_addr().unwrap(), PRACTICE_PATH, ghost_plan(), 7).unwrap();
    let (title, program) = practice_track(None).unwrap();
    let mut cfg = LiveConfig::new("Brian", LiveSource::Deck { title, program });
    cfg.peer = Some(ghost.peer_for_user);
    let ctl = Arc::new(LiveControls::default());
    let c2 = ctl.clone();
    let engine = thread::spawn(move || run_live(cfg, sock, c2));

    // The ghost is on air; we hear it and find its beat.
    let st = wait_for(&ctl, 25, |s| s.partner_beat.is_some() && s.partner_on_air);
    let p = st.partner_beat.expect("the ghost's beat");
    assert!((p.bpm() - 125.0).abs() < 1.0, "ghost at {:.1} BPM", p.bpm());
    assert!(st.you_beat.is_none(), "deck isn't playing yet");
    assert!(
        ctl.deck_wave().map(|w| w.len() > 1000).unwrap_or(false),
        "deck track waveform"
    );

    // Columns cover the whole session so far, one per 5 ms, with the ghost's kicks in them.
    let chunk = ctl.scope_since(0);
    let expect = st.now_ms / 5.0;
    assert!(
        (chunk.cols.len() as f64 - expect).abs() < 60.0,
        "{} columns at {:.0} ms",
        chunk.cols.len(),
        st.now_ms
    );
    let loud = chunk.cols.iter().filter(|c| c.partner[0] > 60).count();
    assert!(loud > 50, "partner low band loud in {loud} columns");

    // Play and SYNC: your beats come in on the ghost's.
    ctl.send(Cmd::DeckPlayPause);
    ctl.send(Cmd::SyncToggle);
    let st = wait_for(&ctl, 20, |s| {
        s.deck
            .as_ref()
            .map(|d| d.sync && d.sync_err_ms.map(|e| e.abs() < 3.0).unwrap_or(false))
            .unwrap_or(false)
    });
    thread::sleep(Duration::from_secs(2));
    let st2 = ctl.status();
    let y = st2.you_beat.expect("your beat");
    assert!((y.bpm() - 125.0).abs() < 1.0, "you at {:.1} BPM", y.bpm());
    assert!(y.bar_ms.is_some(), "your bars are counted from the track");
    let ph = phase_ms(&st2).expect("phase");
    assert!(
        ph.abs() < 12.0,
        "after SYNC your kick is {ph:+.1} ms off the ghost's (deck {:?})",
        st.deck
    );

    // A 100 ms nudge shows up on the phase meter.
    ctl.send(Cmd::SyncToggle);
    thread::sleep(Duration::from_millis(300));
    ctl.send(Cmd::Nudge(100.0));
    thread::sleep(Duration::from_secs(2));
    let moved = phase_ms(&ctl.status()).expect("phase after nudge");
    assert!(
        (moved - ph + 100.0).abs() < 15.0,
        "nudged 100 ms earlier: {ph:+.1} -> {moved:+.1}"
    );

    ctl.stop();
    engine.join().unwrap().unwrap();
    ghost.stop().unwrap();
}

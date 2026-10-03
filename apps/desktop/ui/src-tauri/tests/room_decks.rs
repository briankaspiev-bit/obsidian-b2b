//! A live room between two DJs with no mixer: each picked "Test music", so the
//! room plays it on the built-in deck. The opener is already playing; the other
//! DJ can play, SYNC and TAKE OVER, like in Practice.

use std::net::UdpSocket;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use obsidian_audio_io::{AdaptiveResampler, AudioFifo};
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

/// RMS of 1.5 s of what a DJ hears, with their fader and the partner slider set so.
fn heard(ctl: &LiveControls, sink: &AudioFifo, fader: f32, pvol: f32) -> f32 {
    ctl.set_fader(fader);
    ctl.set_partner_volume(pvol);
    thread::sleep(Duration::from_millis(300));
    sink.clear();
    thread::sleep(Duration::from_millis(1500));
    // Target just under what's queued, so the resampler reads it all as is.
    let mut rs = AdaptiveResampler::new(48_000, 48_000, sink.len_frames() - 1_000);
    let (mut out, mut block) = (Vec::new(), vec![0f32; 480 * 2]);
    while sink.len_frames() > 1_000 {
        rs.pull(sink, &mut block);
        out.extend_from_slice(&block);
    }
    (out.iter().map(|x| x * x).sum::<f32>() / out.len().max(1) as f32).sqrt()
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
    let (ha, hb) = (
        Arc::new(AudioFifo::new(48_000 * 10)),
        Arc::new(AudioFifo::new(48_000 * 10)),
    );
    ca.sink = Some(ha.clone());
    cb.sink = Some(hb.clone());

    let (a, b) = (
        Arc::new(LiveControls::default()),
        Arc::new(LiveControls::default()),
    );
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

    // TAKE OVER asks for the booth (Brian's friend's notes, 2026-10-03): Julio
    // stays off air until Glizzy, on air, lets him in.
    b.send(Cmd::TakeOver);
    let st = wait_for(&a, 5, |s| s.partner_ask_ms_left.is_some());
    let left = st.partner_ask_ms_left.expect("Glizzy sees Julio's ask");
    assert!(left > 5_000 && left <= 10_000, "the ask runs 10 s: {left}");
    let st = b.status();
    assert!(!st.on_air && st.ask_ms_left.is_some(), "Julio waits: {st:?}");
    a.send(Cmd::AnswerAsk(true));
    let st = wait_for(&b, 3, |s| s.on_air);
    assert!(st.on_air, "Julio took over once let in: {st:?}");
    assert_eq!(st.ask_ms_left, None);
    let st = wait_for(&a, 15, |s| !s.on_air && s.partner_on_air);
    assert!(
        !st.on_air && st.partner_on_air,
        "Glizzy handed over: {st:?}"
    );

    // Live test, 2026-10-03: after taking over, Julio heard nothing. Now the new
    // on-air DJ hears his own deck, and each fader/slider moves its own side.
    let julio_own = heard(&b, &hb, 1.0, 0.0);
    let julio_all = heard(&b, &hb, 1.0, 1.0);
    let julio_none = heard(&b, &hb, 0.0, 0.0);
    // His fader is his channel fader: it also sets what Glizzy gets. Back up.
    b.set_fader(1.0);
    // Glizzy, now cueing, still hears his own deck and Julio on air.
    let glizzy_own = heard(&a, &ha, 1.0, 0.0);
    let glizzy_julio = heard(&a, &ha, 0.0, 1.0);
    eprintln!(
        "after takeover: Julio own {julio_own:.3} all {julio_all:.3} none {julio_none:.4}; \
         Glizzy own {glizzy_own:.3} hears Julio {glizzy_julio:.3}"
    );
    assert!(
        julio_own > 0.05,
        "Julio hears his own deck on air: {julio_own:.3}"
    );
    assert!(
        julio_none < 0.01,
        "both sliders down is silence: {julio_none:.4}"
    );
    assert!(
        glizzy_own > 0.05,
        "Glizzy hears his own deck while cueing: {glizzy_own:.3}"
    );
    assert!(
        glizzy_julio > 0.05,
        "Glizzy hears Julio on air: {glizzy_julio:.3}"
    );
    // Both waveforms keep moving on each screen (yours is drawn after your fader).
    a.set_fader(1.0);
    thread::sleep(Duration::from_secs(1));
    let a_cols = a.scope_since(0);
    let recent = &a_cols.cols[a_cols.cols.len().saturating_sub(100)..];
    assert!(
        recent.iter().any(|c| c.you[0] > 30),
        "Glizzy's own waveform still draws"
    );
    assert!(
        recent.iter().any(|c| c.partner[0] > 30),
        "Julio's waveform draws on Glizzy's screen"
    );

    // Robot DJ set, 2026-10-03: Brian blended the robot's song out while on air,
    // and on its next turn it was still down in his ears. Now it's a shared
    // mixer: the DJ on air sets the blend; on a handoff the new DJ's fader takes
    // the level their song was already playing at (no jump to full blast), and
    // they bring it up themselves.
    b.set_partner_volume(0.5); // Julio, on air, has Glizzy's song at half
    a.set_partner_volume(0.0); // Glizzy, off air, can't turn Julio's song down
    a.set_fader(0.8); // and cues with his fader at 80%
    thread::sleep(Duration::from_millis(500));
    let st = a.status();
    assert_eq!(
        st.partner_volume, 1.0,
        "off air, the on-air song stays as its DJ set it"
    );
    assert!(
        heard(&a, &ha, 0.0, 0.0) > 0.05,
        "Glizzy still hears Julio on air"
    );
    a.set_fader(0.8);
    thread::sleep(Duration::from_millis(500));
    // Glizzy asks; Julio says not yet, and Glizzy stays off air.
    a.send(Cmd::TakeOver);
    wait_for(&b, 5, |s| s.partner_ask_ms_left.is_some());
    b.send(Cmd::AnswerAsk(false));
    let st = wait_for(&a, 5, |s| s.ask_denied);
    assert!(st.ask_denied && !st.on_air && st.ask_ms_left.is_none(), "not yet: {st:?}");
    thread::sleep(Duration::from_millis(500));
    assert!(b.status().on_air && b.status().partner_ask_ms_left.is_none());
    // He asks again and nobody answers: after 10 s he goes on air anyway.
    let asked = Instant::now();
    a.send(Cmd::TakeOver);
    let st = wait_for(&a, 15, |s| s.on_air && s.fader < 0.8);
    let waited = asked.elapsed().as_secs_f64();
    assert!((9.5..12.0).contains(&waited), "unanswered ask went through after {waited:.1} s");
    assert!(st.on_air, "Glizzy took it back: {st:?}");
    assert!(
        (st.fader - 0.4).abs() < 0.02,
        "Glizzy's song stays at the level it was playing (80% x 50%): {}",
        st.fader
    );
    assert_eq!(
        st.partner_volume, 1.0,
        "Glizzy starts hearing Julio as Julio sends it"
    );
    let st = wait_for(&b, 15, |s| !s.on_air);
    assert!(!st.on_air, "Julio handed over: {st:?}");
    assert_eq!(
        b.partner_volume(),
        1.0,
        "Julio's blend of Glizzy passed to Glizzy"
    );
    // Both levels show on both screens, so they can blend together.
    let st = wait_for(&b, 5, |s| (s.partner_fader - 0.4).abs() < 0.02);
    assert!(
        (st.partner_fader - 0.4).abs() < 0.02,
        "Julio sees Glizzy's fader: {}",
        st.partner_fader
    );
    // On air again, Glizzy brings himself up and can blend Julio out.
    a.set_fader(1.0);
    a.set_partner_volume(0.0);
    thread::sleep(Duration::from_millis(300));
    let st = a.status();
    assert_eq!(
        (st.fader, st.partner_volume),
        (1.0, 0.0),
        "the on-air DJ sets the blend"
    );
    let st = wait_for(&b, 5, |s| s.partner_fader == 1.0);
    assert_eq!(
        st.partner_fader, 1.0,
        "Julio sees Glizzy bring his fader up"
    );

    a.stop();
    b.stop();
    ja.join().unwrap().unwrap();
    jb.join().unwrap().unwrap();
}

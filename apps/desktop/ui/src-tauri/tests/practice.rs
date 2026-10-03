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

/// What reaches the headphones: your fader only moves your deck, and the
/// partner slider only moves the ghost.
#[test]
fn each_slider_moves_only_its_own_side_in_the_headphones() {
    use obsidian_audio_io::{AdaptiveResampler, AudioFifo};

    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let ghost =
        GhostSession::start(sock.local_addr().unwrap(), PRACTICE_PATH, ghost_plan(), 9).unwrap();
    let (title, program) = practice_track(None).unwrap();
    let sink = Arc::new(AudioFifo::new(48_000 * 10));
    let mut cfg = LiveConfig::new("Brian", LiveSource::Deck { title, program });
    cfg.peer = Some(ghost.peer_for_user);
    cfg.sink = Some(sink.clone());
    let ctl = Arc::new(LiveControls::default());
    let c2 = ctl.clone();
    let engine = thread::spawn(move || run_live(cfg, sock, c2));

    wait_for(&ctl, 25, |s| s.partner_on_air && s.partner_beat.is_some());
    ctl.send(Cmd::DeckPlayPause);

    // RMS of 1.5 s of headphone output at these slider settings.
    let level = |fader: f32, pvol: f32| {
        ctl.set_fader(fader);
        ctl.set_partner_volume(pvol);
        thread::sleep(Duration::from_millis(300));
        sink.clear();
        thread::sleep(Duration::from_millis(1500));
        // Target just under what's queued, so the resampler reads it all as is.
        let mut rs = AdaptiveResampler::new(48_000, 48_000, sink.len_frames() - 1_000);
        let mut out = Vec::new();
        let mut block = vec![0f32; 480 * 2];
        while sink.len_frames() > 1_000 {
            rs.pull(&sink, &mut block);
            out.extend_from_slice(&block);
        }
        (out.iter().map(|x| x * x).sum::<f32>() / out.len().max(1) as f32).sqrt()
    };
    // Off air, the Ghost is on air: you cue under it, and only the DJ on air
    // sets the blend, so your slider can't turn the Ghost down.
    let ghost_only = level(0.0, 1.0);
    let both = level(1.0, 1.0);
    let ghost_cant_mute = level(0.0, 0.0);

    // After you take over, the Ghost rides along under you and you set the blend.
    ctl.send(Cmd::TakeOver);
    wait_for(&ctl, 5, |s| s.on_air);
    let riding = level(0.0, 1.0);
    let riding_muted = level(0.0, 0.0);
    let riding_loud = level(0.0, 2.0);
    let you_only = level(1.0, 0.0);

    ctl.stop();
    engine.join().unwrap().unwrap();
    ghost.stop().unwrap();

    eprintln!(
        "off air: ghost {ghost_only:.3} both {both:.3} ghost slider down {ghost_cant_mute:.3}; \
         on air: ghost {riding:.3} muted {riding_muted:.4} x2 {riding_loud:.3} you {you_only:.3}"
    );
    assert!(ghost_only > 0.05, "you hear the Ghost on air: {ghost_only}");
    assert!(both > ghost_only, "both {both} louder than the Ghost alone");
    assert!(
        (ghost_cant_mute / ghost_only - 1.0).abs() < 0.3,
        "off air, your slider leaves the on-air Ghost alone: {ghost_only} -> {ghost_cant_mute}"
    );
    assert!(
        riding > 0.05,
        "the Ghost rides along after you take over: {riding}"
    );
    assert!(
        riding_muted < 0.001,
        "on air, you can blend the Ghost out: {riding_muted}"
    );
    assert!(
        (riding_loud / riding - 2.0).abs() < 0.3,
        "the blend scales only the Ghost: {riding} -> {riding_loud}"
    );
    assert!(
        you_only > 0.05,
        "you on air with the Ghost blended out: {you_only}"
    );
}

/// The ghost never plays your built-in groove, so its music stays tellable apart.
#[test]
fn the_ghosts_tracks_differ_from_your_built_in_groove() {
    let (_, yours) = practice_track(None).unwrap();
    for (title, t) in obsidian_desktop::live::ghost_playlist() {
        let n = yours.len().min(t.len());
        let diff: f32 = yours[..n]
            .iter()
            .zip(&t[..n])
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            / n as f32;
        assert!(diff > 0.02, "{title} too close to your groove ({diff})");
    }
}

//! Solo practice: a "ghost DJ" that plays through the real engine over a simulated
//! long-distance path, so one person can practise monitoring and handoffs alone.
//!
//! The ghost starts on air. When you take over, it lets you ride for a bit, fades
//! out over 8 bars and rests. After a while (or when you press G) it cues its next
//! track, SYNCs to you as it hears you, fades in, and takes the air back, which is
//! your cue to fade out. Then it all repeats.

use crate::deck::Deck;
use crate::session::{run_live, LiveConfig, LiveControls, LiveSource};
use anyhow::{Context, Result};
use obsidian_netem_proxy::{builtin_profile, Link, Profile};
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct GhostPlan {
    /// Tracks the ghost cycles through.
    pub playlist: Vec<(String, Arc<Vec<f32>>)>,
    /// After you take over, how long the ghost keeps playing before its fade.
    pub ride_s: f64,
    pub fade_s: f64,
    /// How long you play alone before the ghost comes back (None = only on G).
    pub return_after_s: Option<f64>,
    pub fade_in_s: f64,
    /// Once faded in, how long until it takes over.
    pub takeover_after_s: f64,
}

impl GhostPlan {
    pub fn new(playlist: Vec<(String, Arc<Vec<f32>>)>) -> Self {
        GhostPlan {
            playlist,
            ride_s: 15.0,
            fade_s: 15.0,
            return_after_s: Some(60.0),
            fade_in_s: 15.0,
            takeover_after_s: 15.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    OnAir,
    Riding { since: i64 },
    FadingOut,
    Resting { since: i64 },
    Syncing { since: i64 },
    FadingIn,
    Blended { since: i64 },
}

pub struct Ghost {
    plan: GhostPlan,
    /// Decks analysed up front (beat grids take a moment; never on the audio thread).
    decks: Vec<Deck>,
    state: State,
    track: usize,
    pub come_back: bool,
}

impl Ghost {
    pub fn new(plan: GhostPlan) -> Self {
        let decks = plan
            .playlist
            .iter()
            .map(|(t, p)| Deck::new(t.clone(), p.clone()))
            .collect();
        Ghost {
            plan,
            decks,
            state: State::OnAir,
            track: 0,
            come_back: false,
        }
    }

    pub fn describe(&self) -> String {
        match self.state {
            State::OnAir => "on air".into(),
            State::Riding { .. } => "still playing under you".into(),
            State::FadingOut => "fading out".into(),
            State::Resting { .. } => "resting (press G to bring it back)".into(),
            State::Syncing { .. } => "cueing its next track, syncing to you".into(),
            State::FadingIn => "fading in over your track".into(),
            State::Blended { .. } => "blended in, about to take over".into(),
        }
    }

    /// Advance one block. `deck` is the ghost's deck (loaded by the session with
    /// the first playlist track, playing). Returns true to take the air.
    pub fn step(
        &mut self,
        t: i64,
        deck: &mut Deck,
        on_air: bool,
        partner_active_us: i64,
        events: &mut Vec<String>,
        start: i64,
    ) -> bool {
        let say = |events: &mut Vec<String>, s: &str| {
            let secs = ((t - start).max(0) / 1_000_000) as i64;
            events.push(format!("{:02}:{:02} Ghost: {s}", secs / 60, secs % 60));
        };
        let us = |s: f64| (s * 1e6) as i64;
        match self.state {
            State::OnAir => {
                if !on_air {
                    // Follow the new leader while riding out, the way the incoming DJ followed us.
                    deck.sync = true;
                    deck.sync_locked = true;
                    self.state = State::Riding { since: t };
                    say(events, "you have the air; I'll ride along then fade out");
                }
            }
            State::Riding { since } => {
                if t - since >= us(self.plan.ride_s) {
                    deck.ramp_gain(0.0, self.plan.fade_s);
                    self.state = State::FadingOut;
                }
            }
            State::FadingOut => {
                if !deck.ramping() {
                    deck.playing = false;
                    self.state = State::Resting { since: t };
                    say(events, "faded out");
                }
            }
            State::Resting { since } => {
                let due = self.come_back
                    || self
                        .plan
                        .return_after_s
                        .map(|s| t - since >= us(s))
                        .unwrap_or(false);
                if due && partner_active_us >= 8_000_000 {
                    self.come_back = false;
                    self.track = (self.track + 1) % self.plan.playlist.len();
                    *deck = self.decks[self.track].clone();
                    deck.gain = 0.0;
                    deck.sync = true;
                    deck.playing = true;
                    self.state = State::Syncing { since: t };
                    say(events, "cueing my next track and syncing to you");
                } else if due && self.come_back && partner_active_us == 0 {
                    // Nothing to sync to: come straight back on air.
                    self.come_back = false;
                    deck.gain = 1.0;
                    deck.cue();
                    deck.playing = true;
                    self.state = State::OnAir;
                    say(events, "you're silent, so I'm taking the air");
                    return true;
                }
            }
            State::Syncing { since } => {
                let locked =
                    deck.sync_locked && deck.sync_err_ms.map(|e| e.abs() < 5.0).unwrap_or(false);
                if locked && t - since >= 3_000_000 {
                    deck.ramp_gain(1.0, self.plan.fade_in_s);
                    self.state = State::FadingIn;
                    say(events, "locked to your beat, fading in");
                } else if !deck.sync {
                    // SYNC gave up (tempos too far apart): rest again.
                    deck.playing = false;
                    self.state = State::Resting { since: t };
                }
            }
            State::FadingIn => {
                if !deck.ramping() {
                    self.state = State::Blended { since: t };
                }
            }
            State::Blended { since } => {
                if on_air {
                    self.state = State::OnAir;
                } else if t - since >= us(self.plan.takeover_after_s) || partner_active_us == 0 {
                    self.state = State::OnAir;
                    say(events, "taking over; your turn to fade out");
                    return true;
                }
            }
        }
        false
    }
}

/// A ghost DJ running in this process, reachable over a simulated path.
pub struct GhostSession {
    pub ctl: Arc<LiveControls>,
    /// Where the user's session should send (the near end of the simulated path).
    pub peer_for_user: SocketAddr,
    link: Option<Link>,
    thread: Option<std::thread::JoinHandle<Result<()>>>,
}

impl GhostSession {
    /// `user_addr`: the user's session socket (on 127.0.0.1). `path`: a lab profile
    /// name such as "nyc-lon".
    pub fn start(user_addr: SocketAddr, path: &str, plan: GhostPlan, seed: u64) -> Result<Self> {
        let profile: Profile =
            builtin_profile(path).with_context(|| format!("unknown path profile {path}"))?;
        let gsock = UdpSocket::bind("127.0.0.1:0")?;
        let gaddr = gsock.local_addr()?;
        let link = Link::start(user_addr, gaddr, profile.clone(), profile, seed)?;
        let (title, prog) = plan.playlist[0].clone();
        let mut cfg = LiveConfig::new(
            "Ghost DJ",
            LiveSource::Deck {
                title,
                program: prog,
            },
        );
        cfg.peer = Some(link.b_side);
        cfg.start_on_air = true;
        cfg.autoplay = true;
        cfg.ghost = Some(plan);
        let ctl = Arc::new(LiveControls::default());
        let c2 = ctl.clone();
        let thread = std::thread::Builder::new()
            .name("ghost".into())
            .spawn(move || run_live(cfg, gsock, c2).map(|_| ()))?;
        Ok(GhostSession {
            ctl,
            peer_for_user: link.a_side,
            link: Some(link),
            thread: Some(thread),
        })
    }

    pub fn stop(mut self) -> Result<()> {
        self.ctl.stop();
        if let Some(t) = self.thread.take() {
            t.join().unwrap()?;
        }
        if let Some(l) = self.link.take() {
            l.stop();
        }
        Ok(())
    }
}

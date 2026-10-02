//! Live booth sessions on real sound cards (the engine the desktop app drives).
//!
//! * [`session::run_live`] runs one DJ's side: send their audio, play the partner
//!   at a fixed delay, TAKE OVER, fader, SYNC; [`session::LiveControls`] is the
//!   handle a UI uses (commands in, [`session::LiveStatus`] out).
//! * [`ghost::GhostSession`] is the solo-practice partner.

pub mod deck;
pub mod ghost;
pub mod session;

pub use ghost::{GhostPlan, GhostSession};
pub use session::{run_live, Cmd, LiveConfig, LiveControls, LiveSource, LiveStatus};

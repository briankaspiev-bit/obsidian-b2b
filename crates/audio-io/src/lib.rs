//! Audio I/O for the live engine.
//!
//! The engine runs on the system clock at 48 kHz (5 ms blocks, exactly like the
//! bench). Real devices run on their own clocks, often at 44.1 kHz. Two FIFOs
//! bridge the gap:
//!
//! * output: the engine pushes 48 kHz blocks; the device callback pulls through an
//!   [`AdaptiveResampler`] whose ratio follows the FIFO level (an ASRC);
//! * capture: the device callback pushes frames at its own rate; the engine's sender
//!   pulls 48 kHz frames through the same kind of resampler.
//!
//! So a drifting or 44.1 kHz sound card never moves the booth delay.

pub mod device;
pub mod file;
pub mod resample;
pub mod sim;
#[cfg(windows)]
pub mod winloop;

pub use resample::{AdaptiveResampler, AudioFifo};

pub const ENGINE_RATE: u32 = 48_000;

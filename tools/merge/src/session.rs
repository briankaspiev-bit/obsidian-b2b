//! The session manifest the desktop engine writes next to the recordings.
//! See README.md for the field-by-field contract.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone)]
pub struct Session {
    pub version: u32,
    /// Nominal sample rate of every file.
    pub sample_rate: u32,
    pub djs: Vec<Dj>,
    pub events: Vec<Event>,
    #[serde(default)]
    pub telemetry: Telemetry,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Dj {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    /// This DJ's own program output, captured locally (pre-encode, lossless).
    pub iso: FileRef,
    /// The other DJ's stream as it was played to this DJ's monitor, written by the
    /// engine on the same sample clock as `iso` (sample `n` of both = same instant).
    #[serde(default)]
    pub received: Option<Received>,
    /// Capture input latency + monitor output latency, from the Booth Check loopback.
    #[serde(default)]
    pub monitor_roundtrip_ms: f64,
    /// This device's audio clock error versus the session clock, in ppm.
    #[serde(default)]
    pub clock_ppm: f64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FileRef {
    pub path: String,
    /// Session-clock time of sample 0.
    pub start_session_ms: f64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Received {
    pub from: String,
    pub path: String,
    /// Sample index in `iso` that sample 0 of this file lines up with.
    #[serde(default)]
    pub offset_samples: i64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Event {
    pub t_session_ms: f64,
    /// "on_air" (with `dj`) or "take_over" (with `from` / `to`). Others are ignored.
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dj: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Telemetry {
    /// Median one-way delay "A->B": capture on A to playout on B, in ms.
    #[serde(default)]
    pub one_way_ms: BTreeMap<String, f64>,
}

pub fn load(path: &Path) -> Result<Session> {
    let s = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))
}

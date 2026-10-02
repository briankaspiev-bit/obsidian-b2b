//! Writes `session.json` for the master merge tool (tools/merge, format in its README)
//! from the two peers' reports. In the product the engine writes this itself after the
//! control plane exchanges both sides' telemetry; the bench does it offline.
//!
//! Session clock = the shared start instant, in ms. Both peers start their capture at
//! that instant, so each ISO (`sent.wav`) has `start_session_ms = 0`. Each peer's
//! `monitor.wav` is the remote stream exactly as played to its monitor, on the same
//! sample clock as its ISO, starting at ISO sample `monitor_offset_samples`.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::Path;

pub fn write_session(dir: &Path) -> Result<()> {
    let read = |p: &str| -> Result<Value> {
        Ok(serde_json::from_str(
            &std::fs::read_to_string(dir.join(p)).with_context(|| p.to_string())?,
        )?)
    };
    let a = read("a/report.json")?;
    let b = read("b/report.json")?;
    let start_local = |r: &Value| {
        r["start_at_us"].as_i64().unwrap_or(0) + r["clock_offset_param_us"].as_i64().unwrap_or(0)
    };
    let to_session_ms = |r: &Value, t_us: i64| (t_us - start_local(r)) as f64 / 1e3;

    let mut events = vec![json!({ "t_session_ms": 0.0, "type": "on_air", "dj": "A" })];
    if let Some(t) = b["follow_drop"]["drop_at_out_us"].as_i64() {
        events.push(json!({ "t_session_ms": to_session_ms(&b, t), "type": "take_over", "from": "A", "to": "B" }));
    }
    let dj = |id: &str, name: &str, r: &Value, from: &str, ppm: f64| {
        json!({
            "id": id,
            "name": name,
            "iso": { "path": format!("{}/sent.wav", id.to_lowercase()), "start_session_ms": 0.0 },
            "received": {
                "from": from,
                "path": format!("{}/monitor.wav", id.to_lowercase()),
                "offset_samples": r["monitor_offset_samples"].as_u64().unwrap_or(0)
            },
            // No audio devices yet: file in, file out, so there is no device latency.
            // Booth Check will measure this once cpal I/O is in.
            "monitor_roundtrip_ms": 0.0,
            "clock_ppm": ppm
        })
    };
    // A is the session-clock reference; B's clock error is what A measured on B's stream.
    let b_ppm = a["playout"]["drift_ppm_estimate"].as_f64().unwrap_or(0.0);
    let session = json!({
        "version": 1,
        "sample_rate": 48000,
        "djs": [dj("A", "DJ A", &a, "B", 0.0), dj("B", "DJ B", &b, "A", b_ppm)],
        "events": events,
        "telemetry": { "one_way_ms": {
            "A->B": b["delay_final_ms"].as_f64(),
            "B->A": a["delay_final_ms"].as_f64()
        } }
    });
    std::fs::write(
        dir.join("session.json"),
        serde_json::to_string_pretty(&session)?,
    )?;
    Ok(())
}

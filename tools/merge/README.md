# obsidian-merge

Offline master rebuild for a remote B2B session (feasibility report, sections E.1 and P).
It takes both DJs' local recordings plus the session log, aligns every handoff the way
the incoming DJ actually heard it, and renders one clean master.

```
cargo build --release
./target/release/obsidian-merge merge session/session.json -o master.wav --report report.json
```

## How it aligns

When B takes over from A, B beatmatched against A **as heard at B's location**: late by
the network, the jitter buffer and B's monitor. So in the master, B plays at its natural
rate and A is placed where B heard it ("follower-anchored"). Each handoff gives a map
`follower sample -> leader sample`, and the leader is read through that map while the two
overlap. Placement only ever changes while a DJ is silent, so the master never jumps.

Methods, in the order `--method auto` tries them:

| Method | What it uses | Accuracy on synthetic sessions |
|---|---|---|
| `received` | The follower's recording of the stream they heard, correlated (GCC-PHAT) against the leader's ISO. Measures the real delay, drift and network path directly. | **≤ 0.42 ms**, and that residue is exactly the monitor-latency calibration error the test injects |
| `onset` | Onset envelopes of the two ISOs (different tracks, shared kick grid), seeded from timestamps. | 0.1 to 8 ms, depends on the two tracks' sounds |
| `timestamps` | Session clock stamps, clock ppm and one-way telemetry only. | As good as the telemetry (5 ms in the synthetic runs; real one-way estimates can be far worse) |

Rendering: Kaiser-windowed sinc interpolation for the sub-sample offsets and the ppm-level
warp of the outgoing DJ, 20 ms fades where each DJ's region starts and ends (always inside
silence, which also removes idle noise and hum), then EBU R128 loudness normalization
(default -14 LUFS, true peak ceiling -1 dBTP). The crossfade itself is the DJs' own, kept as
they played it.

## What the desktop engine must write

`session.json`, with paths relative to it:

```json
{
  "version": 1,
  "sample_rate": 48000,
  "djs": [
    {
      "id": "A", "name": "Val",
      "iso": { "path": "a_iso.wav", "start_session_ms": 2.3 },
      "received": { "from": "B", "path": "a_recv_from_b.wav", "offset_samples": 0 },
      "monitor_roundtrip_ms": 13.3,
      "clock_ppm": 37.0
    }
  ],
  "events": [
    { "t_session_ms": 3000, "type": "on_air", "dj": "A" },
    { "t_session_ms": 243100, "type": "take_over", "from": "A", "to": "B" }
  ],
  "telemetry": { "one_way_ms": { "A->B": 87.3, "B->A": 112.6 } }
}
```

- `iso`: this DJ's own program output, lossless (WAV or FLAC), and the session-clock time of sample 0.
- `received`: **the remote stream exactly as it was sent to this DJ's monitor, written on the
  same sample clock as `iso`** (sample n of both files = the same instant; use `offset_samples`
  if the files start at different points). This is the one engine requirement that makes the
  sub-millisecond result possible. Mono is fine.
- `monitor_roundtrip_ms`: capture input latency + monitor output latency, measured by the Booth
  Check loopback. Any error here lands 1:1 in the alignment.
- `clock_ppm`: the device clock's error against the session clock (the engine already measures
  drift). Used for the timestamp seed and the fallbacks.
- `telemetry.one_way_ms`: capture on one side to playout on the other, per direction.

Supported input: WAV (16/24/32-bit int, float) and FLAC. Output: WAV (24-bit by default).

## Testing without real recordings

```
obsidian-merge synth --out runs/s1                      # 24 min, 5 handoffs, 89 ppm drift, 1% loss
obsidian-merge merge runs/s1/session.json -o runs/s1/master.wav --report runs/s1/report.json --stems runs/s1/stems
obsidian-merge check --truth runs/s1/truth.json --report runs/s1/report.json --stems runs/s1/stems --session runs/s1/session.json
```

`synth` makes two simulated DJs playing synthesized techno on their own drifting clocks,
with network delay, packet loss, timestamp and calibration noise, and the follower starting
each track on the leader's bar line as heard. Every signal is an analytic function of time,
so `truth.json` holds the exact answer. `check` scores the alignment map and also
re-measures where the audio actually sits in the rendered stems.

`cargo test --release` runs the unit tests and a 3-minute end-to-end session.

## Known limits

- Whole files are held in memory. A 60-minute session peaked at 9 GB of RAM, so a 2-hour set
  will not fit on a 16 GB laptop yet. Streaming the render is the next step.
- Output is WAV only. FLAC/MP3 export is not wired up yet.
- A delay change in the middle of an overlap (jitter buffer re-anchor) is fitted as a straight
  line; the fit RMS in the report flags it.
- Onset fallback carries a track-dependent bias of several ms. Treat it as a rescue, not a
  target.
- Only tested on synthetic audio so far.

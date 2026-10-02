# Booth engine (Rust) — what exists and how to run it

First working core of the remote B2B audio engine from the feasibility report
(decision D2: custom Opus over UDP in Rust). Headless for now: WAV in, WAV out,
no audio devices, no UI wiring yet.

## Layout

| Path | What it does |
|---|---|
| `crates/protocol` | Wire format. One UDP flow per peer pair: media packets (primary frame + redundant copies of earlier frames, each with frame index and capture timestamp), clock-sync ping/pong, bye. |
| `crates/codec` | Opus in restricted-low-delay (CELT) mode, 48 kHz stereo, 5 ms frames, CBR 256 kbps by default; raw PCM16 as an alternative. |
| `crates/clock` | Session clock, NTP-style clock sync (min-RTT filtered), line fitting, precise sleep. |
| `crates/jitter` | The fixed-delay playout buffer: anchors on the packet-arrival envelope, holds a constant booth delay, recovers loss (redundancy → Opus FEC → PLC), tracks clock drift with a resampler (ASRC), re-anchors once on sustained lateness. |
| `crates/align` | Onset envelopes, tempo and beat-phase estimation, beat-quantized monitoring delay. |
| `crates/engine` | One peer's session: sender, receiver and output threads, monitor modes, simulated follower DJ, telemetry (`report.json`, `blocks.csv`). |
| `crates/engine-cli` | `obsidian-peer`, the headless peer. |
| `tools/netem-proxy` | `obsidian-netem`, a UDP impairment proxy (delay, jitter, bursty loss, Wi-Fi stalls, route changes). No root, works on Windows/macOS/Linux. |
| `tools/netem-profiles` | The lab profiles from the report (`profiles.json`) and the equivalent `tc netem` script for a real Linux router box. |
| `tools/bench` | `obsidian-bench`: generates test music; `run` drives both peers through the proxy in real time per scenario; `sim` runs one direction in virtual time (no host noise, 60 min in about a minute); both score from ground truth. Also writes `session.json` for the master merge tool. |

## Run it

```
cargo test --workspace --release
cargo build --release
./target/release/obsidian-bench list
./target/release/obsidian-bench run --out bench-out --duration 90 --jobs 1   # ~15 min, writes bench-out/summary.md
./target/release/obsidian-bench run --out bench-out --only nyc-lon,bad-wifi
./target/release/obsidian-bench sim --out bench-out --minutes 60                # virtual time, writes bench-out/sim.md
```

Each real-time scenario folder holds `a/` and `b/` (`sent.wav` = that DJ's own program, `monitor.wav` = the remote as played to their monitor, `blocks.csv`, `report.json`) and `session.json` for `tools/merge`:

```
obsidian-merge merge bench-out/handoff/session.json -o master.wav --report merge-report.json
```

Two peers by hand (e.g. two laptops on a LAN; use each other's IPs):

```
./target/release/obsidian-bench gen --out audio
./target/release/obsidian-peer --name a --bind 0.0.0.0:9001 --peer <B-ip>:9002 --input audio/dj-a.wav --out out/a --monitor beat --start-at-ms <same epoch ms on both>
./target/release/obsidian-peer --name b --bind 0.0.0.0:9002 --peer <A-ip>:9001 --input audio/dj-b.wav --out out/b --follow --start-at-ms <same>
```

## How the delay contract works

* **Anchor on arrivals, not clocks.** For each packet, `y = arrival − sender position`. Its lower envelope is the fastest the path delivers. Playout time of a sample = envelope + margin, where margin = p99.5 of the jitter seen in the 10 s network test + 2 ms safety + one output block + the redundancy depth. Clock-sync error can therefore never move the delay; sync is only telemetry and (later) recording alignment.
* **Hold it.** Jitter inside the margin is silent. Late packets are concealed. Drift between the two audio clocks is measured from the envelope slope (only used once it is statistically clear of noise) and followed with a ≤500 ppm resampling change, with a 1 ms deadband and a 5 s persistence rule so noise never moves the delay.
* **Re-anchor once, visibly.** Two consecutive seconds with >1% late packets (or one second with >5%) raise the delay to the new p99.5: a jump if the stream is silent, otherwise a ≤0.5% slew. Latency drops are ignored unless stable for 2 minutes.
* **Loss recovery costs latency, explicitly.** Each packet carries copies of earlier frames at chosen offsets (default: 1 and 3 back, so any single or double loss is recovered). The largest offset *k* adds *k* × 5 ms to the margin, and each offset adds one more copy of the bitrate (256 kbps → about 920 kbps on the wire with 1+3). Opus in-band FEC is wired but inert: it only exists in SILK/hybrid modes, and 5 ms music frames always run CELT. (The report listed "FEC on"; that does nothing here, redundancy is the real tool.)

## Beat-quantized monitoring

The leader (`--monitor beat`) compares onset envelopes of its own program and the remote monitor over 8 s, estimates the tempo from its own audio (no BPM metadata needed), measures how far the remote lags within one beat, and adds exactly enough delay that the remote lands on its next beat (a one-time jump with a 5 ms crossfade). It re-checks every 8 s.

The bench's follower (`--follow`) simulates a perfect DJ: it listens to the leader as heard, finds the beat phase, and starts its own track on the next beat. A real DJ does this by ear.

## Recordings for the master merge

Each peer writes the remote stream exactly as it played it to the monitor (`monitor.wav`) on the same sample clock as its own program (`sent.wav`): output blocks run on the same device clock as capture, and `report.json` gives `monitor_offset_samples`, the ISO sample that monitor sample 0 lines up with. `obsidian-bench` turns two reports into the merge tool's `session.json` (A = session-clock reference; B's `clock_ppm` is A's measured drift of B; `monitor_roundtrip_ms` is 0 until real devices and Booth Check exist).

## Live build

`obsidian-live` (`crates/live`, `crates/audio-io`) runs this engine on real sound cards with TAKE OVER, a fader, a SYNC deck and a solo-practice ghost DJ. See `docs/live.md`.

## Next: a Windows build two people can try over the internet

Goal: Brian and a friend each run one app on a Windows laptop, connect with a short code, play any audio (a music file or whatever the laptop is playing), hand off, and judge how it feels.

| Piece | Work | Notes |
|---|---|---|
| Audio I/O | `cpal` (WASAPI) replacing the WAV threads: input = a file player **or** WASAPI loopback of the laptop's own output; output = headphones/speakers. Lock-free ring between network and audio threads. | Loopback capture plus monitoring on the same device feeds back; the app must monitor on a different output (headphones on a USB dongle) or mute the remote in the loopback mix. That is the first thing to get right. |
| Connect by code | Tiny rendezvous service (HTTPS + WebSocket) that pairs two codes and swaps public IP:port; UDP hole punching via STUN; fallback UDP relay on a small cloud VM. | Many home routers punch fine; CGNAT/mobile hotspots need the relay. Relay adds a few ms if it sits near one side. |
| Handoff | TAKE OVER button: on-air state, beat-quantized monitor for the leader, event log; reuse the engine's monitor modes. | No DJ gear needed: the "mixer" is a volume fader per side in the app. |
| UI | Wire the existing Live Session screen (`apps/desktop/ui`) to the engine through Tauri commands; Booth Check shows measured delay, loss and jitter. | |
| Recording | Write `sent.wav`/`monitor.wav` + `session.json` per side, upload after, run `tools/merge`. | Already in the engine. |
| Security | AEAD (ChaCha20-Poly1305) on media; keys from the pairing exchange. | Needed before strangers use it, not for a two-friend test. |

What it does not need: ASIO, DJ software integration, accounts, audience.

## Still open in the engine

* Bad Wi-Fi is not solved: with 2% bursty loss and stalls, even three redundant copies leave hundreds of glitches per 30 min (see sim results). Options to test: deeper offsets sized to burst length, a low-bitrate backup stream, NACK retransmission with a bigger buffer. Until then Booth Check should push performers to Ethernet.
* Bar-quantized mode needs downbeat metadata (Ableton Link / Pro DJ Link); beats alone can't tell bar 1 from bar 3.
* When the delay re-anchors after the follower has beatmatched, the follower's own mix shifts by that amount (seen in the bench). A real DJ re-nudges; the UI should warn.
* Hour-long real-time runs on real laptops; the cloud VM used here stalls 20-40 ms on its own, so its real-time numbers are an upper bound on jitter.

# Remote B2B DJ — Technical Feasibility + MVP Architecture Report

Project: Obsidian B2B · Prepared for Brian · 2 October 2026 · v1

---

## How to read this report

Every load-bearing claim carries one of four labels:

| Label | Meaning |
|---|---|
| **[PROVEN]** | Shipped in existing products or well established in audio/networking practice. Low risk. |
| **[PLAUSIBLE]** | Sound in principle, with partial precedent. Expect it to work after normal engineering. |
| **[RISKY]** | Could work, but has a real chance of failing or degrading the experience. Needs an early spike. |
| **[UNKNOWN]** | Could not be verified from research. Must be answered by a prototype or direct test. |

Where competitor facts come from web research, sources are listed in the appendix. Anything not sourced there is engineering judgment.

---

## Executive summary

1. **The concept is technically viable.** Remote B2B with each DJ on their own gear is not science fiction: PairBeat (Windows + rekordbox, Opus over regional servers), MakingWaves Collab (bring-your-own-gear B2B streamed as one mix), the faraway.dj hardware box, and DIY rigs built from VBAN/ZeroTier/Parsec all exist. None of them combine a robust latency model, clean master output, and a social network. **[PROVEN that it can be done; UNPROVEN that it can be done *convincingly*.]**

2. **Your framing is almost right, with one important correction.** "Latency is measurable, manageable, compensatable" is correct, but compensation is not a single number you subtract. There are **two separate alignment problems**:
   - **Beat phase** (do the kicks line up?) — solvable with *beat-quantized monitoring* or a *shared beat grid*.
   - **Musical content alignment** (does the incoming track line up with the outgoing track's phrase?) — the incoming DJ lines up against what *they* heard, which is already late. So the "correct" combined mix only exists **from the incoming DJ's point of view**, and that point of view flips at every handoff.

   This means a symmetric "both DJs share one mixer" model is impossible over the internet. The product must be designed around a **leader / follower** model with an **elastic master** that re-anchors at each handoff. This is the core technical IP of the company. **[PLAUSIBLE — must be prototyped first.]**

3. **The DJs never need to hear each other "in real time."** B2B is turn-taking, not ensemble jamming. A constant, predictable delay of 60–150 ms (or delay rounded up to a whole beat) is workable. **Variable** delay is the enemy, not delay itself. The engine's #1 job is a *fixed delay contract*.

4. **Do not use WebRTC for the performer audio path.** Its jitter buffer (NetEQ) time-warps audio and varies delay to suit voice calls. Use a **custom UDP media path in Rust with Opus (or raw PCM), a fixed-target jitter buffer, adaptive resampling for clock drift, and ICE for NAT traversal**. Use WebRTC/LiveKit later for video and the audience, completely off the critical path.

5. **Capture is the biggest practical unknown, not networking.** Getting the DJ's master out of rekordbox/Serato/CDJs cleanly, without feedback, on Windows (where DJ software uses single-client ASIO drivers) is the riskiest piece. The universal fallback is a physical line into any USB audio interface; the convenient path is OS-level process audio capture (Core Audio taps on macOS 14.2+, WASAPI process loopback on Windows) which must be spiked immediately.

6. **Music rights are the single biggest business dependency.** Private two-person practice is low risk. Public livestreams, stored replays, and monetization each trigger different licenses, and replays in particular have no statutory shortcut. Plan Phase 1 as private-only with local recordings, Phase 2 as "broadcast to licensed platforms" (Twitch DJ Program, Mixcloud Live, etc.), and only later native public booths with direct label deals.

7. **First milestone:** two Macs, two DJs, real gear, generic audio-interface capture, custom Opus/UDP link, fixed delay, beat-quantized remote monitoring, local lossless recording on both sides, and an offline "master rebuild" that produces a clean combined mix. Pass/fail criteria are in section W and Z.

---

## A. Product interpretation

**What it is:** a connection layer that turns two (later N) existing DJ setups into one virtual booth, plus a platform where those booths become live, social, and archived objects.

**What it is not (yet):** a DJ application, a streaming catalogue, or a mixer replacement.

**Product layers, in dependency order:**

| Layer | Purpose | Moat contribution |
|---|---|---|
| 1. Booth Engine (desktop) | Capture, transport, latency model, monitoring, recording | Low-medium. Hard to build, but copyable over 1–2 years. |
| 2. Session object | Every B2B becomes a record: who, where, handoffs, tracks, recording | Medium. Data compounds. |
| 3. Identity + graph | DJ profiles, collaborators, B2B map, followers | High. Network effect. |
| 4. Live + spectators | Booths, chat, reactions, track IDs | High, but gated by licensing. |
| 5. Matchmaking (Open Decks) | Find compatible DJs instantly | Very high. Liquidity is the moat, and latency data is a unique input. |
| 6. Clubs / events / Pass the Decks | Persistent venues and signature formats | High. Community + brand. |
| 7. Content engine + AI | Clips, tracklists, artwork, recaps | Medium. Distribution flywheel. |

**Pushback on the thesis:**

- "Discord × Boiler Room × rekordbox × Twitch × matchmaking" is a good north star but a dangerous build list. The thing that has never been done *well* is layer 1 + layer 2. Everything above it is known product territory. If layer 1 is mediocre, nothing above it matters. If it is excellent, Open Decks is the most differentiated feature on the list, because matchmaking needs a reliable "any two DJs can connect in under a minute" primitive that nobody else has.
- **Open Decks has a hidden dependency on latency geography.** A match between New York and Tokyo is a different experience than New York and Toronto. Matchmaking must treat measured latency as a first-class compatibility variable, which you already listed. That makes latency telemetry strategically valuable, not just diagnostic.
- **Beatport Party Mode** (browser DJ app, up to 4 DJs, up to 100 spectators, claims "latency-free") solves the problem by having everyone play inside *Beatport's* software from *Beatport's* catalogue, so state is synced instead of audio. That's the opposite thesis. It shows demand exists, and it shows that "bring your own gear" is the gap.

---

## B. Core technical challenge

Stated precisely:

> Two DJs, each with an independent audio clock, an independent mixer, and their own speakers, connected by a network with 20–200 ms of one-way delay that varies over time and drops packets, must produce (1) a monitoring experience each of them can mix against and (2) a single combined master that sounds like one continuous set.

The sub-problems, ranked by how much they threaten the product:

| # | Problem | Why it's hard | Status |
|---|---|---|---|
| 1 | **Handoff alignment** (follower perspective) | The incoming DJ aligns to delayed audio, so the "true" mix exists only at their location. It moves every handoff. | **[PLAUSIBLE]** — elastic master design in C/E/F. Core IP. |
| 2 | **Capture without feedback** | DJ software and hardware were never designed to export a clean master to a third app while also receiving remote audio. | **[RISKY]** on Windows/ASIO, **[PLAUSIBLE]** on macOS. |
| 3 | **Constant delay under variable networks** | Internet jitter is bursty. Adapting the buffer changes delay, which breaks the musical contract. | **[PROVEN]** techniques (JackTrip, Jamulus), but tuning for this use case is new. |
| 4 | **Clock drift** | Two sound cards at "48 kHz" differ by 10–100 ppm: up to ~0.7 s of drift over 2 hours. | **[PROVEN]** — adaptive resampling. |
| 5 | **Packet loss** | A dropout in a club mix is far more noticeable than in a voice call. | **[PROVEN]** — Opus FEC/PLC + redundancy; tuning needed. |
| 6 | **NAT traversal** | Home routers, CGNAT, venue Wi-Fi. | **[PROVEN]** — ICE/STUN/TURN. |
| 7 | **Clean master recording** | Has to reflect the follower-anchored mix, survive dropouts, and be lossless. | **[PLAUSIBLE]** — dual local recordings + offline rebuild. |

---

## C. Proposed signal flow

### C.1 What each DJ sends

**Recommendation: each DJ sends their own stereo master (post-fader program output) plus a metadata/control stream. Nothing else in V1.**

| Candidate | Send in V1? | Reasoning |
|---|---|---|
| Master output (stereo) | **Yes** | It's what the other DJ and the audience need to hear. Universal across all software and hardware. |
| Individual decks | No | DJ software doesn't expose per-deck outputs without "external mixer" mode and multichannel interfaces. Huge setup burden. Reconsider only for a future "shared mixer" mode. |
| Cue / headphone signal | No | Remote cue is not needed for a B2B handoff (see D). Doubling streams doubles failure surface. |
| Stems | No | Not exposed by DJ software as outputs. Pure complexity. |
| MIDI / control | No (V1) | Not needed when each DJ mixes on their own mixer. Future "remote booth fader" (D.4) uses our own control messages, not raw MIDI. |
| Metadata | **Yes** | Session clock timestamps, level/silence state, "on air" state, handoff intent, BPM & beat phase when available, track ID when available. Tiny bandwidth, huge value. |

### C.2 End-to-end flow (V1, two DJs, peer-to-peer)

```
DJ A (New York)                                                     DJ B (London)
─────────────────                                                   ─────────────────
rekordbox / controller                                              Serato / CDJs + DJM
  │ master out (own decks only)                                       │ master out
  ▼                                                                   ▼
[CAPTURE]  interface input │ process tap │ mixer USB rec          [CAPTURE]
  │                                                                   │
  ├──► [LOCAL ISO RECORDER] FLAC, session-clock stamped               ├──► [LOCAL ISO RECORDER]
  │                                                                   │
  ▼                                                                   ▼
[ENCODE] Opus 48k stereo, 5 ms frames (or PCM)                     [ENCODE]
  │                                                                   │
  ▼          encrypted UDP, ICE direct or via relay                   ▼
[TRANSPORT] ═══════════════════════════════════════════════════► [TRANSPORT]
[TRANSPORT] ◄═══════════════════════════════════════════════════ [TRANSPORT]
  │                                                                   │
  ▼                                                                   ▼
[JITTER BUFFER — fixed target] → [ASRC drift correction]           (same)
  │                                                                   │
  ▼                                                                   ▼
[MONITOR ALIGNER] beat-quantized or follower-aligned delay          (same)
  │                                                                   │
  ▼                                                                   ▼
[MONITOR OUT] separate output → booth speakers / monitor           (same)
  (local DJ software continues to drive their own speakers + cue)
```

**Rules that make this safe:**

1. **The uplink only ever contains the local DJ's own decks.** Remote audio is never fed back into the capture path. That is the entire anti-feedback strategy (see D.3).
2. **Remote audio is played out by our app on a separate output path**, not injected into the DJ software.
3. **Every audio frame carries a session-clock timestamp and sample counter** so any device can reconstruct exact alignment later.
4. **Local lossless recording happens before encoding.** The network can lose packets; the recording never does.

### C.3 Where the "true" master lives

There is no single physical place where both DJs' audio is aligned for both of them. The master is assembled at a **mix point**:

- **V1 (PoC):** offline. Both ISO recordings are merged after the session using the handoff log. Zero real-time risk, best quality.
- **V1.5:** real time on one DJ's machine (the host), for a live preview/recording.
- **Phase 3 (Live):** real time on a **Booth Server** in a cloud region between the DJs, feeding the audience. This is where the elastic master runs (section E).

---

## D. Performer audio experience

### D.1 What DJ A needs to hear from DJ B to mix properly

To execute a handoff, the incoming DJ needs:

1. **The outgoing DJ's master, at a constant delay, through speakers or a monitor**, so they can beatmatch their cued track in headphones against it. This is exactly how DJs beatmatch against a club's room sound. **[PROVEN workflow]**
2. **Their own cue in headphones, untouched and zero-latency** from their own DJ software. **[PROVEN — we don't touch it]**
3. **A visual status:** "Alex is playing", "Alex is handing to you", remote level meter, remote BPM if known, and a bar/phrase counter when metadata exists.

The outgoing DJ needs:

1. To hear the incoming DJ's track arriving, in phase with their own, so they know when to cut their bass/fade out.
2. A clear "you're handing off" / "you're off air" state.

### D.2 Can a DJ beatmatch reliably against delayed remote audio?

**Yes, if the delay is constant. [PROVEN]**

The incoming DJ matches their track to what they hear. Their output is then correctly aligned *at their location*. The faraway.dj review reported ~200 ms (about half a beat at house tempo) as workable for experienced DJs, and the aniclover DIY rig reported beatmatching and EQ/fader transitions as "quite usable" at ~800 miles. What breaks beatmatching is delay that wanders, because the DJ chases a moving target.

What does **not** work without help is the *outgoing* DJ's experience. If B is aligned to A-as-heard-in-London, then back in New York, A hears B arrive one full round trip late (e.g. 80 ms ≈ 1/6 of a beat at 125 BPM). That sounds like a flam or trainwreck to A even though the mix is correct. Fix: **beat-quantized monitoring.**

### D.3 Beat-quantized monitoring (key technique)

Delay the remote audio further so it lands on the **next beat boundary** relative to the listener's own audio. At 125 BPM a beat is 480 ms. If B's audio arrives 80 ms off A's grid, add another 400 ms, and B lands exactly one beat late, in phase with A's kicks. To A, it sounds locked.

- Musical precedent: NINJAM uses interval-quantized latency for online jamming. **[PROVEN concept]**
- The offset can be measured without knowing BPM: cross-correlate the onset envelope of local audio against the received remote stream, find the phase offset within the beat, and lock it. **[PLAUSIBLE]**
- If BPM/beat phase metadata exists (Ableton Link, Pro DJ Link), alignment is exact and instant. **[PLAUSIBLE]**
- Cost: remote events (filter sweeps, cuts) are heard up to one beat later. For B2B turn-taking that's acceptable. **[PLAUSIBLE — validate with DJs]**

**Caution — phrase alignment:** quantizing to one beat aligns kicks, not phrases. If a follower drops their track "on the one" of a phrase *as they hear it*, they are aligning against audio that is late. That is fine for the follower's own mix (and the master if anchored at the follower — section E). It is **not** fine if the system tries to present a single absolute grid to everyone. This is why the master must be follower-anchored, and why a bar/phrase counter from metadata is a strong Phase 2 UX addition.

### D.4 Headphone cueing

- **V1: cueing stays 100% local.** Each DJ cues their own decks in their own headphones via their own software/mixer. We do not touch the cue path. **[PROVEN]**
- **Optional V1 setting: "remote in headphones"** — our app can mix remote audio into a second output that the DJ splits into headphones. Useful for DJs at home without monitors. Requires a second output device or a mixer with an aux/line channel.
- **CDJ + DJM setups:** the cleanest experience is to bring the remote DJ into a **spare line channel on the DJM**. Then the remote DJ is literally "a channel on your mixer" and can be cued like a deck. **Danger:** the DJM master now contains the remote signal, so the capture must use a pre-master source (or the remote channel must be cue-only, fader down). This is the #1 feedback trap. V1 should detect it automatically (correlate uplink against what we just played out; if correlated above threshold, show "Feedback detected — your mixer is sending Alex back to Alex") rather than try echo cancellation, which damages music. **[PLAUSIBLE detection; RISKY if users route this way without guidance]**

### D.5 What happens during a transition

**Recommended V1 model: "own mixer, explicit handoff."**

1. Leader A is on air. Follower B cues their next track, beatmatching against A on their monitor.
2. B taps **TAKE OVER** (or the app detects B's stream going from silence to signal while A plays).
3. B brings their fader up on their own mixer. The app marks B as the anchor for the master (section E).
4. A hears B arrive in phase (beat-quantized), cuts their bass, fades out on their own mixer.
5. A's stream goes silent. B is the sole leader. Roles flip.

**Why not a shared virtual crossfader in V1?** In a real B2B, the incoming DJ controls both channels on one mixer. Remotely, each DJ owns their own mixer, so a "booth fader" that lets B fade A down in the master is possible (our app applies the gain at the mix point and in each DJ's monitor) but adds UI and a control path that must be latency-aware. Treat it as a **Phase 2 experiment**, not an MVP requirement. **[PLAUSIBLE]**

**Hard cases to test explicitly:** quick 8/16-bar trades, long double-drop layering, tracks with no kicks (ambient intros) where onset-based alignment is weak, and a DJ who rides the pitch fader during the overlap.

### D.6 Should delay be compensated on the performer side or only on the master side?

**Both, differently:**

| Where | What compensation | Goal |
|---|---|---|
| Follower's monitor | None (plain constant delay) | They align to what they hear. |
| Leader's monitor | Beat-quantized delay | Follower sounds in phase to them. |
| Master / audience | Follower-anchored elastic alignment | The audience hears the mix exactly as the follower built it. |
| Recording | Offline rebuild from ISOs + handoff log | Studio-quality, perfectly aligned. |

Never delay a DJ's *own* audio in their own speakers or headphones. That would break their hands-to-ears loop.

---

## E. Audience / master audio architecture

### E.1 The follower-anchored master

Let the incoming (follower) DJ be **F** and the on-air (leader) DJ be **L**. F aligns their track to L as heard at F's location. So at the mix point, the master must play L delayed by exactly F's round trip relative to F's stream:

```
master(T) = L_arrived(T − d_L) + F_arrived(T − d_F),   with  d_L = d_F + RTT_F(via mix point)
```

When roles flip, the new leader (the old follower, still playing) needs *more* delay so the next follower can be anchored. The follower's delay can be changed freely **while their stream is silent** (between their turns), which is inaudible. The leader's delay can only be increased smoothly.

### E.2 The elastic master

- The audience path has a **headroom buffer** of a few seconds (audiences tolerate 2–10 s; HLS tolerates 10–30 s).
- Each handoff "spends" roughly one RTT of headroom.
- The engine **re-absorbs** it by imperceptibly adjusting the playback rate of the master during solo sections. Numbers: an 80 ms correction every ~3 minutes is a 0.04% rate change, about 0.05 BPM at 128 BPM, or under one cent of pitch if done by resampling. Inaudible. **[PLAUSIBLE — core prototype item]**
- Follower detection: stream goes from silence to signal while the other is non-silent, confirmed or overridden by the explicit TAKE OVER action.

This design keeps the audience mix musically correct without forcing DJs to use sync, and without ever audibly jumping time.

### E.3 Optional "Grid Lock" mode (shared beat grid)

If both DJs enable **Ableton Link** in their software (rekordbox Performance mode, Serato DJ Pro, Engine DJ, and Traktor have offered Link support in recent versions — **[PLAUSIBLE, verify per current version]**), our app can join the local Link session on each machine and lock both Link timelines to one global beat clock synchronized over the network (NTP-style clock sync, single-digit-millisecond accuracy). Then both DJs' decks are phase-locked to the same absolute grid regardless of latency.

- Monitoring: remote delay rounded to a beat or bar is exactly in phase. **[PLAUSIBLE]**
- Master: beat phase is automatically correct. Phrase alignment still follows the follower's ears, so the elastic master still applies, but corrections become whole beats/bars and easier to detect.
- Not available on standalone CDJs or vinyl. Many DJs dislike sync. **So Grid Lock is an enhancement, never a requirement.**
- Licensing: Ableton Link is dual-licensed (GPLv2+ or a proprietary license from Ableton). A closed-source app needs the proprietary license. **[UNKNOWN — confirm terms with Ableton]**

### E.4 Audience delivery (Phase 3+)

```
DJ A ─┐                                   ┌─► LiveKit / WebRTC SFU → in-app booth viewers (≈0.5–2 s)
      ├─► Booth Server (mix point, region) ┼─► HLS/LL-HLS → CDN → large audiences (3–30 s)
DJ B ─┘    elastic master + recorder       └─► RTMP restream → Twitch / YouTube / Mixcloud Live
```

- Audience never touches the performer path. They can be delayed by seconds.
- Video is captured from DJ cameras and sent via LiveKit, then **delayed to match the master audio** at the booth server or player.

---

## F. Latency model

### F.1 Budget (one-way, capture-to-ear)

| Stage | Typical | Best case | Notes |
|---|---|---|---|
| Capture buffer | 2.7–5 ms | 1.3 ms | 64–256 samples @ 48 kHz |
| Opus encode + algorithmic delay | 7.5 ms | 5 ms | 5 ms frames + 2.5 ms lookahead. PCM: ~0 ms |
| OS / network stack | 1–2 ms | <1 ms | |
| **Network propagation** | see F.2 | | dominated by distance |
| Jitter buffer target | 10–40 ms | 5 ms | set from measured jitter p99 |
| Decode + ASRC + output buffer | 4–8 ms | 3 ms | |
| **Total excluding network** | **~25–60 ms** | **~15 ms** | |

### F.2 Distance (rule of thumb: light in fiber ≈ 5 µs/km; real routes add 30–60%)

| Pair | Typical internet RTT | One-way network | Estimated one-way total | Feel |
|---|---|---|---|---|
| Same city | 5–20 ms | 3–10 ms | 20–50 ms | Near-booth |
| NYC ↔ LA | 60–75 ms | 30–38 ms | 55–90 ms | Very good |
| NYC ↔ London | 70–80 ms | 35–40 ms | 60–95 ms | Good |
| NYC ↔ Berlin | 85–100 ms | 43–50 ms | 70–105 ms | Good |
| NYC ↔ Tokyo | 160–190 ms | 80–95 ms | 105–150 ms | Workable with quantization |
| London ↔ Sydney | 250–290 ms | 125–145 ms | 150–200 ms | Edge case |

RTTs are typical public-internet ranges, not measurements. **[PLAUSIBLE — PoC must measure real ones.]**

### F.3 Acceptable latency targets

| Use | Target | Hard ceiling | Reasoning |
|---|---|---|---|
| DJ's own monitoring/cue | 0 added | 0 added | Never in our path. |
| Remote monitoring (beatmatch) | ≤ 100 ms one-way, **constant** | ~250 ms | Constancy matters more than size. Beat quantization hides phase. |
| Delay variation after jitter buffer | 0 ms unplanned | ± 2 ms | Any wander is perceived as the remote DJ "drifting". |
| Real-time ensemble jamming | 20–30 ms | — | Literature threshold for musicians playing *together*. We are deliberately **not** in this regime. |
| In-app audience | 0.5–3 s | 10 s | Comfortable with chat. |
| Broadcast (HLS/RTMP) | 3–30 s | — | Standard. |
| Video vs audio (audience) | ±45 ms lip-sync | — | Broadcast standards ballpark. |
| DJ-to-DJ camera | 150–400 ms fine | — | Social, not sync-critical. |

### F.4 The fixed delay contract

At session start, after the network test, the engine sets a **booth delay** per direction = measured one-way p50 + jitter p99 + safety margin. It holds that delay constant.

- Jitter spikes inside the margin: absorbed silently.
- Late packets beyond the margin: concealed (Opus PLC/FEC).
- Sustained degradation: the engine schedules a **re-anchor**: it raises the delay smoothly during a moment when that stream is silent, or with a slow rate change, and shows "Booth Sync: adjusting". It never wanders silently.
- Latency drop (route improves): ignore it unless large and stable for minutes, then lower the delay during silence. Stability beats minimality.

---

## G. Networking architecture

### G.1 Overview

```
                     ┌──────────────────────────────┐
                     │ Control plane (HTTPS + WSS)   │
                     │ rooms, invites, presence,     │
                     │ signaling, telemetry ingest    │
                     └──────────────┬───────────────┘
                 signaling / ICE    │      signaling / ICE
            ┌───────────────────────┴──────────────────────┐
            ▼                                               ▼
     Desktop A  ══════════ direct UDP (ICE) ═══════════  Desktop B
            ║                                               ║
            ╚════════ fallback: regional media relay ═══════╝
                       (our UDP relay, TURN-like)
```

### G.2 Components

- **Signaling:** WebSocket to the control plane. Exchanges ICE candidates, session keys, capabilities.
- **NAT traversal:** ICE (RFC 8445) with STUN for server-reflexive candidates. **[PROVEN]** Implementation via a Rust ICE library (e.g. the `webrtc-ice` crate from webrtc-rs, or libjuice via FFI). **[PLAUSIBLE — library choice to confirm in spike]**
- **Relay:** for symmetric NAT / CGNAT / locked-down networks. In WebRTC deployments a meaningful minority of sessions (commonly cited at roughly 10–20%) need TURN. **[PLAUSIBLE]** Options: run coturn and tunnel our UDP packets through standard TURN, or run our own lightweight UDP relay (simpler, lower overhead, also usable as a deliberate low-latency backbone path).
- **Path selection:** probe both direct and relay paths at session start, pick the lower **p99** latency, keep the other warm for instant failover.
- **Media transport:** our own packet format over UDP: sequence number, session-clock timestamp, sample counter, codec frame(s), redundancy, encrypted with an AEAD cipher (ChaCha20-Poly1305 / AES-GCM) using keys exchanged via a Noise or DTLS-style handshake.
- **Control channel peer-to-peer:** QUIC (via `quinn`) or a reliable stream over the same UDP flow for metadata, handoff events, clock sync pings, telemetry.

### G.3 Geography

- Distance mostly sets latency, and nothing can reduce it below physics.
- For far pairs (NYC–Tokyo), a relay on a premium backbone (cloud provider backbones, global accelerator products) can sometimes beat public-internet routing and jitter. **[PLAUSIBLE — measure, don't assume]**
- For Live mode, the booth server should sit near the geographic midpoint of the DJs' network paths, not near the audience. The audience CDN handles the audience.
- Matchmaking should use measured RTT (from lightweight background probes to regional anchors) as a compatibility input.

---

## H. Audio-device architecture

### H.1 Capture strategies (ranked)

| Tier | Method | Works with | Pros | Cons | Status |
|---|---|---|---|---|---|
| 1 | **Physical line into a USB audio interface** (booth/rec out of mixer or controller → interface input) | Everything: rekordbox, Serato, Engine, CDJs, vinyl | Universal, zero software conflict, no feedback if source is pre-remote | User needs a ~$50–150 interface and a cable | **[PROVEN]** |
| 2 | **Mixer USB record channel** (many Pioneer DJM and similar mixers expose master/rec as a USB input) | CDJ+DJM, many club mixers, some controllers | No extra hardware, digital | Varies by model, must catalogue | **[PLAUSIBLE — catalogue per model]** |
| 3 | **macOS Core Audio process tap** (macOS 14.2+) — capture rekordbox/Serato's output directly | Software DJs on modern macOS | No hardware, no driver install | Needs permission; behavior with DJ apps that output to controller devices must be verified | **[UNKNOWN — spike #1]** |
| 4 | **Windows WASAPI process loopback** | Software DJs on Windows *when the DJ app uses WASAPI/WDM* | No hardware | **Bypassed when the DJ app uses ASIO**, which is the norm for controllers | **[RISKY — spike #2]** |
| 5 | **Our own virtual audio device** (macOS AudioServerPlugIn; Windows virtual driver) | DJ software that can output master to a separate device | Clean, controllable | Driver signing, install friction, OS update breakage; on Windows a kernel driver is a major project | **[RISKY — defer]** |

**Recommendation:** ship tier 1 + 2 as the "always works" path in V1 and pursue tier 3 on macOS as the "no hardware" path. Treat Windows no-hardware capture as an open research item.

### H.2 Output (remote monitoring)

- Our app outputs remote audio to a **user-selected output device**, ideally different from the one the DJ software owns: laptop output → monitor speakers, a second interface, or the same multi-channel interface on spare channels.
- macOS Core Audio is multi-client, so sharing a device with the DJ app is possible. **[PROVEN]**
- **Windows ASIO drivers are often single-client.** If rekordbox owns the controller's ASIO driver, our app likely can't open it. Our app should use WASAPI on a different device. **[RISKY — catalogue common controllers]**

### H.3 Do we need ASIO? Do we need a virtual audio device?

- **ASIO:** not for V1. Our app can do capture/playback via WASAPI (exclusive or low-latency shared mode) on a separate interface with acceptable latency. ASIO support matters later for pro interfaces; the ASIO SDK has its own Steinberg license terms. **[UNKNOWN — verify current SDK license before shipping]**
- **Virtual device:** not for V1. Process taps + hardware capture cover the PoC. Build one only if spikes 1–2 fail and user research shows hardware capture is a dealbreaker.

### H.4 Audio engine rules

- Fixed internal format: 48 kHz, 32-bit float, stereo.
- Real-time audio threads never allocate, lock, or log. Lock-free ring buffers between the device callback and network threads.
- Every device has its own clock domain; adaptive resampling bridges them (section F/drift).
- Sample counters + session clock timestamps on every buffer.

---

## I. Recommended desktop technology

**Recommendation: Rust core (audio + network) with a Tauri shell and a web-tech UI.**

| Option | Verdict | Reasoning |
|---|---|---|
| **Rust + Tauri** | **Recommended** | Memory-safe real-time code, excellent networking ecosystem (tokio, quinn), `cpal` for cross-platform audio (CoreAudio, WASAPI, ASIO), Opus bindings, small binaries. Tauri gives a fast UI layer in TypeScript that can share components with the web app. |
| C++ + JUCE | Strong alternative | Most mature audio framework, but JUCE's licensing (AGPL or commercial tiers) costs money at scale, and networking/async in C++ is slower to build safely. Good fallback if a Rust audio blocker appears. |
| Electron | Not recommended for core | Fine as a UI shell, but heavier. Never put audio in the Electron/Node process. |
| Browser only | **Rejected for performers** | No reliable access to other apps' audio, no control over buffering, inconsistent device handling, background-tab throttling. Fine for spectators. |
| Native Swift/C# per platform | Not recommended | Doubles the engine. |

Structure the core so the UI is a thin client over a local IPC/command API. That lets us later run the engine headless (CLI for tests, CI, and a future "booth box" hardware device).

---

## J. Recommended backend stack

Keep V1's backend tiny. It only needs rooms, invites, signaling, relay, and telemetry.

| Concern | V1 choice | Later |
|---|---|---|
| Control plane API + signaling | **TypeScript (Node or Bun) with Fastify + WebSocket**, or Rust (axum) if the team is Rust-heavy | Split into services along the boundaries in Q |
| Database | **Postgres** | + read replicas, partitioning for telemetry |
| Ephemeral state / presence | Redis (or Postgres + in-memory in V1) | Redis cluster |
| Auth | Email magic link or OAuth via a hosted provider (Supabase Auth, Clerk, or similar) | Full identity service |
| Media relay | **Rust UDP relay**, stateless, deployed in 4–6 regions on VMs with good UDP performance | Anycast / global accelerator |
| STUN | Our relay nodes + public STUN for dev; coturn if standard TURN is used | |
| Object storage | S3-compatible (R2/S3) for recordings | + CDN |
| Telemetry | Session metrics to Postgres/ClickHouse; crash reports via Sentry | ClickHouse for analytics |
| Hosting | Control plane on a managed platform (Fly.io/Render/Railway); relays on raw VMs | Kubernetes only when needed |
| Video / audience (Phase 3) | **LiveKit** (open-source SFU or LiveKit Cloud) | |

---

## K. Recommended realtime protocol

| Path | Protocol | Why |
|---|---|---|
| **Performer audio** | **Custom RTP-like packets over UDP**, AEAD-encrypted, ICE for connectivity | Full control of buffering and delay. The JackTrip/Jamulus pattern. **[PROVEN pattern]** |
| Performer control/metadata | QUIC streams (`quinn`) peer-to-peer, or reliable messages over the same flow | Ordered, reliable, encrypted, low overhead |
| Clock sync | NTP-style ping exchange over the control channel, filtered (min-RTT) | ~1–5 ms accuracy is enough for alignment; sample-level comes from sample counters |
| Client ↔ control plane | HTTPS + WebSocket | Standard |
| Video (DJ cams) | WebRTC via LiveKit | Mature, video isn't sync-critical |
| Audience | WebRTC (in-app small/medium), LL-HLS (large), RTMP out (restream) | |

**Why not WebRTC for performer audio:** libwebrtc's NetEQ is designed for speech. It changes buffer depth adaptively and time-stretches audio (accelerate/expand) to manage delay, which directly violates the fixed delay contract and audibly warps music. Its audio processing (echo cancellation, gain control, noise suppression) must also be fully disabled. It is possible to fight it, but it's easier to own the path. **[PLAUSIBLE — we could validate by A/B in the PoC, but not required]**

**Why not QUIC for audio:** QUIC datagrams would work, but buy little over raw UDP + AEAD for a two-party stream, and congestion control must not throttle a constant-bitrate music stream. Use QUIC for control, raw UDP for media.

---

## L. Recommended codecs

| Use | Codec | Settings | License |
|---|---|---|---|
| Performer link (default) | **Opus**, CELT/music mode | 48 kHz stereo, 5 ms frames (2.5 ms option), 192–256 kbps CBR, in-band FEC on, plus optional packet redundancy | BSD + royalty-free patent grants. **[PROVEN]** |
| Performer link ("Studio" mode, good networks) | **Raw PCM** 24-bit (~2.3 Mbps) or 16-bit (~1.5 Mbps) | JackTrip-style | No codec licensing |
| Local ISO recording | **FLAC** (or WAV/RF64) 48 kHz / 24-bit | | BSD |
| Master archive | FLAC | | BSD |
| Audience / replay | AAC (HLS) and/or Opus (WebRTC) | 192–320 kbps | **AAC encoding has patent licensing considerations** — prefer OS-provided encoders or a cloud service that carries the license **[UNKNOWN — confirm]** |

Expected quality: Opus at 192–256 kbps stereo is near-transparent for most listeners on club music. **[PROVEN]** The perceived quality bottleneck will be dropouts, not codec artifacts.

---

## M. Peer-to-peer vs relay analysis

| Factor | Peer-to-peer | Relay | Hybrid (recommended) |
|---|---|---|---|
| Latency | Usually lowest | +1–15 ms if well placed; can be *lower* if backbone beats public routing | Best of both, measured |
| Connectivity | Fails for ~10–20% of networks | ~100% | ~100% |
| Cost | ~free | Bandwidth: 2 × 256 kbps per session is cheap | Pay only when needed |
| Privacy | E2EE trivially | E2EE still possible (relay forwards ciphertext) | Same |
| Live mode | Doesn't help audience | Natural place for mix point | Booth server becomes the relay |

**Recommendation:** V1 = ICE direct first, our UDP relay as fallback, choose by measured p99. Phase 3 Live mode = DJs send to a regional booth server (which mixes and feeds the audience), while DJ-to-DJ monitoring can remain direct if that path is faster.

---

## N. Windows / macOS support strategy

**Answer to Q20: build the PoC on macOS, compile and smoke-test Windows from day one, reach Windows parity before public beta.**

Reasoning:

- macOS Core Audio is multi-client and has process taps (macOS 14.2+), making both no-hardware capture and shared-device output much easier. **[PLAUSIBLE]**
- Windows is where the hardest unknowns are (single-client ASIO, loopback bypass). Those need their own spike, but should not block proving the core B2B experience.
- The Tier 1 capture path (interface input) works identically on both OSes via `cpal`, so the PoC *can* run on Windows if a test DJ only has a PC.
- PairBeat has shipped Windows + rekordbox, which suggests Windows capture is solvable. **[UNKNOWN how they capture]**
- Linux: not targeted, but the engine should build there for CI and servers.

Market split between Mac and Windows among target DJs: **[UNKNOWN]** — a quick survey of 30–50 target DJs should precede the Windows parity decision.

---

## O. rekordbox / Serato / Engine compatibility strategy

**Answer to Q21/Q22: stay software-agnostic by capturing audio, and add per-ecosystem integrations only for metadata.**

| Layer | Approach | rekordbox | Serato | Engine DJ | CDJs (standalone) |
|---|---|---|---|---|---|
| Audio | Generic capture (H.1) | ✓ | ✓ | ✓ (via mixer/interface) | ✓ (via DJM/interface) |
| Beat sync (optional Grid Lock) | Ableton Link | ✓ (Performance mode) **[PLAUSIBLE]** | ✓ **[PLAUSIBLE]** | ✓ **[PLAUSIBLE]** | ✗ |
| Now playing / track metadata | Integration | rekordbox has no official live API; Pro DJ Link sniffing works for CDJ networks (open-source libraries like beat-link and alphatheta-connect) **[PLAUSIBLE]** | Serato history files / Serato Live features **[UNKNOWN for live use]** | StageLinQ protocol (community-documented) **[PLAUSIBLE]** | Pro DJ Link **[PLAUSIBLE]** |
| Universal fallback | Audio fingerprinting of the captured stream (ACRCloud, Audible Magic, Shazam-style) | ✓ | ✓ | ✓ | ✓ |

**Notes:**

- Integrating with Pro DJ Link and StageLinQ means reverse-engineered or community-documented protocols. They may break, and vendor terms should be reviewed. **[RISKY for production reliance]**
- Do not depend on any DJ software vendor's cooperation for V1. Partnerships can come later from a position of traction.
- Streaming services inside DJ apps (Beatport/Beatsource Streaming, TIDAL, SoundCloud, etc.) generally license tracks for personal use. Capturing and transmitting them may breach those services' terms, and some may detect it. **[UNKNOWN — review each service's terms]**

---

## P. Recording architecture

**Answer to Q29/Q30: record twice, merge once.**

1. **Local ISO recordings (always, V1).** Each desktop records its own pre-encode capture to FLAC 48k/24, stamped with session clock and sample counters. Also records the received remote stream (for diagnostics) and the handoff/event log. **[PROVEN pattern — "double-ender" podcast tools like Riverside work this way]**
2. **Post-session master rebuild (V1).** Both ISOs are uploaded (or exchanged peer-to-peer, if the user chooses not to use our cloud) and merged:
   - Map both onto the session timeline using timestamps and drift data.
   - Apply follower-anchored alignment per handoff segment (E.1). Because each DJ's stream is silent between their turns, segment offsets are applied inside silences, so the rebuild never needs to stretch audio.
   - Refine each handoff offset with onset cross-correlation over the overlap region.
   - Loudness-normalize (EBU R128 target configurable), export FLAC + MP3/AAC.
   - **[PLAUSIBLE — this is the most valuable PoC deliverable because it proves the audience-side mix is achievable before building any real-time server]**
3. **Real-time master (V1.5 on host, Phase 3 on booth server).** Elastic master (E.2) recorded as it's broadcast. This is the "what the audience heard" archive; the rebuild remains the best-quality version.

**Consent:** recording requires both DJs to accept. A persistent "REC" indicator is visible to both. Recording state is logged on the session object.

---

## Q. Session / control-plane architecture

### Q.1 V1 services (one deployable)

```
control-plane (monolith, modular)
├─ auth        accounts, devices, tokens
├─ rooms       create room, invite codes, membership, room state machine
├─ signaling   WebSocket: ICE candidates, key exchange, capability negotiation
├─ sessions    session objects, participants, events (handoffs), timestamps
├─ telemetry   per-session network/audio metrics ingest
└─ recordings  upload URLs, rebuild job queue, assets
media-relay (separate, Rust, per region)
rebuild-worker (separate, Rust or Python + ffmpeg, job queue)
```

### Q.2 Room / session state machine

```
CREATED → WAITING_FOR_GUEST → BOTH_PRESENT → TESTING (network + audio check)
       → READY → LIVE (session running) ⇄ DEGRADED ⇄ RECONNECTING → ENDED
```

- **Invite codes:** short, high-entropy (e.g. 8–10 base32 chars), single-use by default, expire after 30 minutes, host must accept ("knock" flow) before audio flows.
- **Reconnect without destroying the performance (Q17):**
  - The audio engine keeps playing the last good remote buffer's concealment then fades remote to silence over ~200 ms. The local DJ's own sound is never interrupted (we're not in their path).
  - The session stays LIVE for a grace window (e.g. 60 s). ICE restarts; path switches to relay if direct failed.
  - On reconnect: clocks re-sync, booth delay re-established, remote fades back in on the next beat boundary. Recording continues locally throughout, so the rebuilt master has no gap from the performer side.
  - UI says "Alex reconnecting…" not "ICE failed".

### Q.3 Future boundaries (design now, build later)

The monolith's modules map to future services: Identity, Rooms/Booths, Sessions, Presence, Matchmaking, Social Graph, Media (relay + booth servers), Recording/Media Processing, Realtime Messaging (chat/reactions), Notifications, Search/Discovery, Analytics, Events/Clubs, Payments, Content/AI pipeline. Keep each module's tables and APIs separate from day one so they can be split without rewrites.

The **desktop engine depends only on**: auth token, room/signaling API, relay addresses, telemetry endpoint. It must work against a local dev control plane and in a "manual IP" mode for lab tests.

---

## R. MVP UI

Principles: nightlife, dark, calm, minimal during performance. Plain-language states with diagnostics behind a toggle.

**Screens:**

1. **Home:** `CREATE ROOM` · `JOIN ROOM` (code field).
2. **Room / setup:** both DJ cards (name, CONNECTED/WAITING), Audio Input [device], Audio Output (remote monitor) [device], live input meter, **Run Booth Check** button.
3. **Booth Check (≈20 s):** measures RTT, jitter, loss, path (direct/relay), clock drift, input level and a feedback test (plays a short inaudible-ish probe on the monitor out and checks it doesn't appear in the capture). Results in plain words with a fix for each failure ("Your input is too quiet — raise the REC/Booth out level").
4. **Ready:** `START SESSION` (both must press, or host starts).
5. **Session (performance mode):** big timer, two DJ names with ON AIR / CUEING / OFF states, `TAKE OVER` button, BOOTH SYNC, NETWORK, REC, `END SESSION`. Remote level meter. Optional bar counter when metadata exists.
6. **After session:** duration, handoff count, connection summary, "Master mix is being built" → download/share.

**Label mapping (internals → user words):**

| User sees | Excellent | Good | Fair | Poor |
|---|---|---|---|---|
| **Network** | loss < 0.1%, jitter p99 < 10 ms | loss < 0.5%, jitter p99 < 20 ms | loss < 2%, jitter p99 < 40 ms | worse |
| **Booth Sync** | Stable: delay unchanged, drift corrected | Adjusting: planned re-anchor in progress | Unstable: frequent concealment | Lost |
| **Remote DJ** | Connected / Reconnecting / Left | | | |

Diagnostics drawer: RTT, one-way estimates, buffer depth, concealed packets/min, drift ppm, path type, codec bitrate.

---

## S. Data model (V1, with room to grow)

```sql
users            (id, email, display_name, created_at)
dj_profiles      (user_id PK/FK, handle, city, country, genres[], software[], hardware[],
                  bpm_min, bpm_max, bio, avatar_url)                 -- light in V1
devices          (id, user_id, os, os_version, app_version, audio_capture_method, created_at)
rooms            (id, owner_id, mode: practice|live|open_decks, visibility, state,
                  created_at, ended_at)
room_invites     (id, room_id, code_hash, created_by, expires_at, max_uses, used_count,
                  revoked_at)
room_members     (room_id, user_id, role: host|guest|spectator, joined_at, left_at)
sessions         (id, room_id, started_at, ended_at, session_clock_epoch, status,
                  booth_delay_ms_a_to_b, booth_delay_ms_b_to_a, path_type, codec, bitrate)
session_participants (session_id, user_id, device_id, city, country, joined_at, left_at)
session_events   (id, session_id, t_session_ms, type: start|take_over|handoff_complete|
                  reconnect|re_anchor|rec_start|rec_stop|end, actor_user_id, payload jsonb)
session_segments (id, session_id, performer_user_id, start_ms, end_ms, anchor_offset_ms)
track_plays      (id, session_id, performer_user_id, t_start_ms, t_end_ms,
                  source: fingerprint|link|stagelinq|manual, title, artist, isrc, bpm, key,
                  confidence)                                        -- Phase 2
telemetry_samples(session_id, device_id, t, rtt_ms, jitter_p99_ms, loss_pct,
                  concealed_frames, buffer_ms, drift_ppm)            -- time-series store
recordings       (id, session_id, kind: iso_a|iso_b|realtime_master|rebuilt_master,
                  storage_key, format, duration_ms, sha256, consent_state, created_at)
consents         (id, session_id, user_id, kind: recording|broadcast, granted_at, revoked_at)
blocks           (blocker_id, blocked_id, created_at)
reports          (id, reporter_id, target_type, target_id, reason, created_at, status)
```

Later tables: follows, collaborations (derived edges for the B2B map), clubs, club_members, events, event_slots (Pass the Decks), booths (persistent), chat_messages, reactions, clips, matchmaking_tickets, subscriptions, payouts.

The **collaboration graph** (user ↔ user edges with counts, hours, cities) is derivable from `session_participants` and should be materialized early, because it powers profiles, the B2B map, and matchmaking.

---

## T. Major technical risks

| # | Risk | Likelihood | Impact | Mitigation | Label |
|---|---|---|---|---|---|
| 1 | Handoffs don't *feel* like B2B even with the latency model (DJs find it awkward) | Medium | **Fatal** | Test with real DJs in Milestone 1. Iterate on monitoring modes (plain vs beat-quantized vs bar-quantized), TAKE OVER UX, booth fader | **[UNKNOWN]** |
| 2 | Windows no-hardware capture impossible with ASIO-based DJ setups | High | High | Hardware capture path; mixer USB rec channels; virtual driver later; study PairBeat's approach | **[RISKY]** |
| 3 | macOS process taps don't capture DJ app output reliably | Medium | Medium | Hardware path; own HAL plugin (e.g. with an MIT-licensed helper library) | **[UNKNOWN]** |
| 4 | Feedback loops via user routing (remote into mixer channel) | High | Medium | Booth Check feedback probe, live correlation detector, guided setup per hardware type | **[PLAUSIBLE]** |
| 5 | Jitter bursts on home Wi-Fi cause dropouts | High | High | Strongly recommend Ethernet; Opus FEC + redundancy; margin in booth delay; show "Use Ethernet" proactively | **[PROVEN mitigations]** |
| 6 | Elastic master produces audible artifacts or wrong alignment | Medium | High (for Live) | Offline rebuild first; real-time later; conservative rate limits | **[PLAUSIBLE]** |
| 7 | Follower detection wrong in layering/double-drops | Medium | Medium | Explicit TAKE OVER as source of truth; auto-detect only suggests | **[PLAUSIBLE]** |
| 8 | Reverse-engineered metadata protocols break | Medium | Low-Medium | Fingerprinting fallback | **[RISKY]** |
| 9 | Music licensing blocks public/replay features | High | **Fatal for Live/Content** | Section U; staged approach; restream to licensed platforms | **[RISKY]** |
| 10 | Matchmaking liquidity (too few DJs online) | High | High (for Open Decks) | Scheduled open-decks hours, communities/clubs first, region bootstrapping | **[RISKY — business, not tech]** |
| 11 | Third-party license traps (GPL in closed app: Link, BlackHole, Jamulus code) | Medium | Medium | License review before adopting any dependency; don't copy GPL code | **[PLAUSIBLE]** |

---

## U. Music-rights / legal considerations

**This section is not legal advice. Get a music-tech lawyer before any public launch.** It separates the scenarios because they really do have different rules.

| Scenario | Rights touched | Risk | Notes |
|---|---|---|---|
| **Private practice** (2 DJs, private room, nothing stored by us) | Arguably a private transmission between acquaintances | **Low** | In the US, "public performance" means to the public or a substantial group outside a normal social circle. A private two-person session is generally not public. Similar concepts exist elsewhere. **[PLAUSIBLE — confirm per key market]** Encrypted, unrecorded-by-us sessions keep us close to a communications tool. |
| **Local recording** (stays on the DJ's machine) | Reproduction by the user | Low for us | Comparable to OBS/rekordbox's own recorder. Terms of service put responsibility on the user. |
| **Stored recordings on our servers** | Reproduction of sound recordings + compositions | **Medium-High** | Hosting copies of copyrighted mixes needs licensing or safe-harbor compliance. |
| **Public livestream (native)** | Public performance of compositions (PROs: ASCAP/BMI/SESAC/GMR in US, PRS UK, GEMA DE, SACEM FR…) + sound recordings (labels) + video sync issues | **High** | US statutory webcasting licenses (via SoundExchange) cover some *non-interactive audio* streams but have conditions DJ sets often break (e.g. limits on tracks per artist/album in a period, **no advance announcement of specific upcoming tracks** — which conflicts with "upcoming track" UI). Video streams generally fall outside statutory licenses. Outside the US there's usually no statutory route. |
| **Replay / on-demand** | Interactive → no statutory license | **Very high** | Needs direct deals with labels/distributors and publishers. Mixcloud built its business on such deals. |
| **Monetized streams, tips, subscriptions, tickets** | Same as above, plus commercial use increases scrutiny and damages exposure | **Very high** | Twitch's DJ Program (2024) is the reference model: platform licenses with labels, DJs share revenue. |
| **Track identification** | Fingerprinting itself is fine; displaying metadata is fine | Low | Fingerprint vendors also provide rights reporting needed for licensing deals. |
| **Clubs/events (ticketed virtual venues)** | Public performance + recording rights + possibly venue-style licenses | **High** | Promoters will expect the platform to handle rights. |
| **Audience track requests** | Increases "interactive" character | Medium | Could affect classification under statutory schemes. |
| **Streaming-service tracks inside DJ apps** | Service terms often restrict to personal use | **Unknown/High** | Capturing Beatport/TIDAL/SoundCloud streams and broadcasting may violate their terms. |

**Platform-law layer:**

- **US DMCA §512 safe harbor:** register a DMCA agent, notice-and-takedown, repeat-infringer policy. Required as soon as users can host/stream content publicly.
- **EU DSM Directive Article 17:** platforms giving public access to large amounts of user-uploaded copyrighted works must make best efforts to *obtain authorization* and prevent unavailability of notified works. Stricter than DMCA. Relevant for EU live/replay.
- **Recording consent:** voice talkback and camera recordings implicate consent laws (some US states require all-party consent); GDPR for EU users' personal data and recordings.

**Where licensing becomes a major business dependency:** the moment there are **public native streams, stored replays, or monetization**. That's Phase 3+. The staged plan:

1. **Phase 1–2:** private only, recordings local by default, opt-in cloud storage for private use, clear ToS. Low exposure.
2. **Phase 3:** "Go Live" = **restream to platforms that already hold licenses** (Twitch DJ Program, Mixcloud Live, possibly YouTube with Content ID consequences). In-app spectating initially for small private/invite-only audiences. Gather data on catalogue played.
3. **Phase 4+:** negotiate direct licenses (labels, distributors, PROs/CMOs) or partner with a licensed DJ-mix platform. Fingerprinting + rights reporting becomes core infra.

---

## V. Prototype test plan

### V.1 Lab rig

- Two machines on one LAN, linked through a network emulator: Linux `tc netem` box (or macOS Network Link Conditioner, or `clumsy` on Windows) between them.
- **Network profiles:** `same-city` (8 ms one-way, 1 ms jitter, 0% loss), `nyc-lon` (38 ms, 4 ms jitter, 0.3% loss), `nyc-tyo` (90 ms, 8 ms, 0.5%), `bad-wifi` (40 ms, 25 ms bursty jitter, 2% bursty loss), `route-change` (step from 38 → 60 ms mid-session).
- **Measurement tools (build these first):**
  - *Latency probe:* inject a click/impulse into A's capture, detect it in B's monitor output via loopback cable, measure end-to-end; repeat continuously to measure delay stability.
  - *Glitch detector:* send a known test signal (sine sweep + MLS), detect discontinuities on receive.
  - *Drift test:* 3-hour run, track drift ppm and alignment error.
  - *Quality metric:* ViSQOL or PEAQ-style objective scores on received audio vs source.

### V.2 Real-world pairs

NYC↔NYC, NYC↔LA, NYC↔London, NYC↔Berlin, NYC↔Tokyo (or the nearest real equivalents among available DJs). Home Ethernet first, then Wi-Fi.

### V.3 DJ experience tests

- 6–10 DJs of mixed skill, mixed setups: at least one rekordbox controller, one Serato controller, one CDJ+DJM, one Engine.
- Each pair plays 3 × 30-minute sets: (a) plain constant delay monitoring, (b) beat-quantized monitoring, (c) bar-quantized monitoring. Randomized order.
- Structured interview + 1–5 ratings: "Could you execute transitions?", "Did it feel like a B2B?", "Was the delay distracting?", "Would you do this weekly?".
- Record screen + audio for later review of trainwrecks.

### V.4 Listener test (the audience proof)

- Blind test: rebuilt remote-B2B masters vs locally recorded B2B mixes by the same DJs. 20+ listeners. Can they tell which is remote? Rate transition quality 1–5.

---

## W. Measurable success criteria (PoC)

| Area | Criterion |
|---|---|
| Latency | NYC↔London-equivalent one-way capture-to-monitor ≤ 95 ms p95 on Ethernet |
| Stability | Monitoring delay variance ≤ ±2 ms over 60 min; **zero unplanned delay changes** |
| Dropouts | ≤ 1 audible glitch per 30 min at 0.5% random loss; ≤ 1 per 10 min on `bad-wifi` |
| Drift | Alignment error between streams ≤ 2 ms after 3 hours |
| Audio quality | Opus 256 kbps objective score near-transparent vs source; no DJ reports "sounds compressed" |
| Recording | Rebuilt master handoff alignment error ≤ 5 ms (onset-measured) at every handoff; no gaps |
| Connectivity | Session established in ≥ 95% of attempts across test networks (incl. relay fallback); invite → ready ≤ 2 min for a first-time user with instructions |
| Reconnect | Network drop of ≤ 10 s recovers automatically without ending the session |
| Resource use | ≤ 10% CPU on an M1 MacBook Air with DJ software running; no added xruns in DJ software |
| **DJ experience (the real gate)** | ≥ 70% of test DJs rate "felt like a real B2B" ≥ 4/5 and ≥ 60% say they'd use it weekly |
| **Listener test** | Listeners identify remote vs local master no better than ~60% (near chance), transition ratings within 0.5 points |

If the DJ-experience and listener criteria pass, the company is technically viable. If they fail with good network numbers, the problem is the interaction model, not the plumbing, and that's the most important thing to learn early.

---

## X. Build phases

| Phase | Goal | Contents | Exit criteria |
|---|---|---|---|
| **0. Spikes** | Kill the biggest unknowns | (1) macOS process tap capture of rekordbox/Serato; (2) Windows capture with ASIO-based controllers; (3) Opus/UDP loopback with fixed jitter buffer + ASRC; (4) Ableton Link join + clock sync; (5) license review (Link, ASIO SDK, ICE lib) | Written results per spike; go/no-go on capture tiers |
| **1. Remote B2B PoC** | Prove the experience | Section Z | Section W |
| **2. Private beta (MVP)** | Usable by strangers | Tauri app, accounts, room/invite flow, Booth Check, reconnection, relay regions, Windows parity, crash/telemetry, cloud recording rebuild, guided setup per hardware type, TAKE OVER UX, optional booth fader experiment | 100+ DJs, weekly retention signal, setup success ≥ 90% |
| **3. Live booths** | Audience | Booth server + elastic master, LiveKit cams, restream to Twitch/Mixcloud/YouTube, small in-app audiences, chat/reactions, track ID via fingerprinting, session pages | Licensed-platform restreams working; first public events |
| **4. Network** | The moat | DJ profiles, session history, B2B map, follows, Open Decks matchmaking with latency-aware compatibility, discovery feed | Matchmaking liquidity in 2–3 regions/genres |
| **5. Clubs & formats** | Community + brand | Clubs, schedules, residents, moderators, Pass the Decks, N-DJ rotation | Promoters running recurring events |
| **6. Content + AI + money** | Distribution + revenue | Auto clips, tracklists, chapters, artwork, recaps; DJ Pro / Club tiers; tipping; licensing deals | Paid conversion, licensing in place |

---

## Y. Estimated engineering difficulty by subsystem

Relative difficulty (1 = routine, 5 = research-grade) and rough effort for one strong engineer. Effort numbers are judgment, not commitments.

| Subsystem | Difficulty | Rough effort | Notes |
|---|---|---|---|
| Audio capture — interface/generic | 2 | 1–2 wks | `cpal` |
| Audio capture — macOS process tap | 3 | 2–3 wks | Unknown API behavior with DJ apps |
| Audio capture — Windows no-hardware | **5** | 4–10+ wks | Possibly a driver |
| Opus/PCM UDP transport + encryption | 3 | 3–4 wks | |
| Fixed-target jitter buffer + PLC/FEC tuning | 4 | 3–5 wks | Tuning is the work |
| Clock sync + ASRC drift correction | 4 | 2–4 wks | Well known, easy to get subtly wrong |
| Beat-quantized monitoring (onset correlation) | 4 | 2–4 wks | Novel for this use |
| Follower-anchored offline master rebuild | 4 | 3–4 wks | Core IP |
| Real-time elastic master (booth server) | **5** | 6–10 wks | Core IP, Phase 3 |
| ICE/NAT traversal + relay | 3 | 2–4 wks | Library choice matters |
| Ableton Link Grid Lock | 3 | 2–3 wks | + licensing |
| Tauri app + MVP UI | 2 | 3–4 wks | |
| Control plane (rooms, invites, signaling) | 2 | 2–3 wks | |
| Feedback detection + Booth Check | 3 | 2 wks | |
| Metadata (Pro DJ Link / StageLinQ / fingerprinting) | 3 | 3–6 wks | |
| Live audience (LiveKit, HLS, restream) | 3 | 4–6 wks | Mostly integration |
| Social graph, profiles, discovery | 2 | ongoing | Standard product engineering |
| Matchmaking | 3 | 4–8 wks | Liquidity is the hard part |
| AI content pipeline | 3 | ongoing | Vendor-heavy |
| Licensing/rights infrastructure | 4 (non-technical) | ongoing | Business dependency |

---

## Z. Exact first engineering milestone

### "M1 — Remote B2B Proof of Concept"

**Setup**

- Two Apple Silicon Macs (macOS 14.2+), two DJs, their own gear (one rekordbox controller, one Serato or CDJ+DJM).
- Capture: **Tier 1 generic interface input** (booth/rec out → USB interface). Tier 3 process tap only if Spike 1 passed.
- Remote monitor output to a separate output device or channel pair.
- No accounts, no UI beyond a minimal window or CLI. Pairing by a tiny signaling server or manual IP:port, with STUN-based hole punching; relay optional.

**Engine scope**

1. Capture 48 kHz stereo, local FLAC ISO recording with session-clock timestamps.
2. Opus 5 ms frames, 256 kbps, FEC on; PCM mode as a toggle.
3. Encrypted UDP transport with sequence numbers, timestamps, sample counters.
4. Clock sync over the control channel; ASRC drift correction.
5. Fixed-target jitter buffer, set from a 20-second network test; PLC; no adaptive drift of delay.
6. Monitoring modes: plain constant delay, beat-quantized (onset correlation, with manual BPM entry as fallback), bar-quantized.
7. TAKE OVER hotkey that logs handoff events with session timestamps.
8. Telemetry log: RTT, jitter, loss, buffer depth, concealment, drift.
9. **Offline master rebuild tool**: takes both ISOs + event logs, produces an aligned FLAC master and an alignment report per handoff.

**Test protocol**

- Lab: all `tc netem` profiles in V.1 for 60 minutes each with test signals (latency probe, glitch detector, drift).
- Real: at least one cross-city pair (ideally ~NYC↔London distance) playing three 30-minute sets with the three monitoring modes.
- Listener blind test on the rebuilt masters.

**Done when:** the W criteria are met, or we have a written explanation of which one failed and why.

**What M1 deliberately excludes:** accounts, profiles, spectators, video, matchmaking, cloud recording, Windows polish, fancy UI.

---

## Answers to the 30 technical questions (quick index)

| # | Question | Short answer | See |
|---|---|---|---|
| 1 | Best architecture for 2-DJ B2B | Native Rust desktop engine, own-mixer leader/follower model, custom UDP+Opus, ICE + relay, follower-anchored master | C, E, G |
| 2–3 | What audio is sent | Each DJ's own stereo master + metadata. No decks/stems/cue/MIDI in V1 | C.1 |
| 4 | What A needs from B | B's master at constant delay on a monitor; own cue untouched; clear on-air state | D.1 |
| 5 | Feedback loops | Never send remote audio upstream; separate output path; probe + correlation detector | C.2, D.4 |
| 6 | Headphone cueing | Stays local; optional remote-in-headphones; DJM line channel for CDJ users with care | D.4 |
| 7 | Transitions | Follower beatmatches, takes over on own mixer; leader fades out; master anchors to follower | D.5, E.1 |
| 8 | Beatmatch vs delayed audio | Yes if delay is constant | D.2 |
| 9 | Compensate where | Leader monitor: beat-quantized; follower: plain; master: follower-anchored | D.6 |
| 10 | Global master clock | One session timeline for timestamps; each device keeps its own audio clock + ASRC; optional Link beat clock | F.4, E.3 |
| 11 | Server-authoritative master | Not for V1 (offline rebuild); yes for Live audience (booth server) | C.3, E |
| 12 | P2P vs relay | Hybrid, chosen by measured p99 | M |
| 13 | Geography | Sets the latency floor; quantized monitoring makes far pairs workable; booth server near the DJs' midpoint | F.2, G.3 |
| 14 | Latency changes mid-set | Fixed delay contract with planned re-anchors during silence or by slow rate change | F.4 |
| 15 | Packet loss | Opus FEC/PLC, redundancy, margin, Ethernet guidance | L, T |
| 16 | Clock drift | Adaptive resampling | B, H.4 |
| 17 | Reconnect | Grace window, ICE restart, fade out/in on beat, local recording continues | Q.2 |
| 18 | Achievable quality | Near-transparent Opus 192–256 kbps; PCM on good links | L |
| 19 | Acceptable latency | Monitoring ≤ 100 ms constant; audience seconds; video ±45 ms to audio | F.3 |
| 20 | Windows/macOS | macOS PoC, Windows compiled from day one, parity before public beta | N |
| 21 | Which ecosystem first | Generic capture first; rekordbox/Pro DJ Link metadata first among integrations | O |
| 22 | Software-agnostic capture | Yes via audio capture; metadata is where per-ecosystem work lives | O |
| 23 | Virtual audio device | Not for V1 | H.3 |
| 24 | ASIO | Not for V1; single-client ASIO is a Windows risk to design around | H.2, H.3 |
| 25 | Codec/library licensing | Opus/FLAC BSD; Link GPL-or-commercial; ASIO SDK, JUCE, AAC need review; avoid GPL code (BlackHole, Jamulus) in a closed app | L, T |
| 26–27 | NAT / STUN / TURN | ICE + STUN on our relays; own UDP relay or coturn; ~10–20% sessions relayed | G.2 |
| 28 | Video separate | Yes, fully off the critical path (LiveKit), delayed to match audio for audience | E.4 |
| 29–30 | Recording | Local ISOs on both sides + offline follower-anchored rebuild; real-time master at booth server later | P |

---

## Competitive frame

| Product | What it solves | What it leaves open |
|---|---|---|
| **PairBeat** | Windows desktop app capturing rekordbox audio, Opus, regional servers (Frankfurt, Amsterdam, London, New York, Santiago); free beta | Windows + rekordbox only; no audience, recording, or social layer found |
| **Beatport DJ Party Mode / B2B Mode** | Real-time multi-DJ sets inside Beatport's browser DJ app (up to 4 DJs, up to 100 spectators), catalogue + imported tracks | Requires their software and subscription; no "bring your own gear" |
| **MakingWaves Collab** | Remote B2B with your own gear, streamed as one mix; strong track ID, overlays, one-click publishing; from £15.99/mo | Positioned as a station/streaming toolkit, not a social network for DJs; no matchmaking found. Closest overall competitor; worth trialing in Phase 0 |
| **faraway.dj** (2021) | Hardware box, browser crossfader, Icecast/RTMP | ~200 ms latency, hardware cost, no visual feedback of the other DJ |
| **DIY (VBAN + ZeroTier + Parsec + OBS)** | Shows demand and that remote B2B is usable | Fragile, expert-only |
| **JackTrip / Jamulus** | Ultra-low-latency uncompressed/Opus audio for musicians; server mixing (Jamulus) | Built for ensemble jamming, not DJ handoffs; no DJ workflow; Jamulus is GPL |
| **Mixcloud / Mixcloud Live** | Licensed DJ mix hosting + live | Not collaborative |
| **Twitch (DJ Program)** | Licensed DJ livestreaming with label revenue share | Not collaborative; a likely restream partner |
| **Discord** | Social presence, voice | Voice codecs/processing unsuited to music; not DJ-native |
| **rekordbox / Serato / Engine** | The DJ software itself | No remote B2B across ecosystems |
| **OBS / LiveKit** | Broadcast tooling / media infra | Building blocks, not a product for DJs |
| **B2B Studio, RemoteBeats** | **[UNKNOWN]** — could not find reliable public information | Verify in Phase 0 |

The gap is real: nobody combines bring-your-own-gear, a trustworthy latency model, a clean master, and a DJ-native social network.

---

## Recommended repository structure

A single monorepo. Rust workspace for everything real-time, TypeScript workspace for UI and control plane, with the protocol defined once and generated for both.

```
obsidian/
├─ README.md
├─ Cargo.toml                    # Rust workspace
├─ package.json                  # pnpm/bun workspace for TS
├─ docs/
│  ├─ feasibility/               # this report
│  ├─ adr/                       # architecture decision records (one per decision)
│  ├─ audio-signal-flow.md
│  ├─ latency-model.md
│  └─ hardware-setup-guides/     # per controller / mixer routing guides
│
├─ crates/                       # Rust: the Booth Engine
│  ├─ engine/                    # orchestrates a session: capture → encode → send; recv → align → play
│  ├─ audio-io/                  # device enumeration + streams (cpal), RT-safe ring buffers
│  ├─ capture-macos/             # Core Audio process taps
│  ├─ capture-windows/           # WASAPI loopback / process loopback experiments
│  ├─ codec/                     # Opus + PCM framing, FEC/redundancy
│  ├─ transport/                 # UDP media packets, AEAD, ICE, relay client, path selection
│  ├─ jitter/                    # fixed-target jitter buffer, PLC hooks
│  ├─ clock/                     # session clock sync, drift estimation, ASRC
│  ├─ align/                     # onset detection, beat/bar quantized monitoring, follower anchoring
│  ├─ link-bridge/               # Ableton Link Grid Lock (feature-flagged; license-gated)
│  ├─ recorder/                  # FLAC ISO recording, event log
│  ├─ rebuild/                   # offline master rebuild (lib + CLI)
│  ├─ telemetry/                 # metrics collection + export
│  ├─ protocol/                  # wire formats + control messages (source of truth)
│  └─ engine-cli/                # headless engine for lab tests and CI
│
├─ apps/
│  ├─ desktop/                   # Tauri app
│  │  ├─ src-tauri/              # thin Rust layer: commands → engine
│  │  └─ ui/                     # TS/React UI (home, room, booth check, session)
│  └─ web/                       # Phase 3+: booth pages, profiles, discovery (empty in V1)
│
├─ services/
│  ├─ control-plane/             # TS: auth, rooms, invites, signaling, sessions, telemetry ingest
│  ├─ media-relay/               # Rust: stateless regional UDP relay
│  ├─ rebuild-worker/            # Rust: cloud master rebuild jobs (Phase 2)
│  └─ booth-server/              # Rust: real-time elastic master + audience feed (Phase 3)
│
├─ packages/
│  ├─ protocol-ts/               # generated TS types from crates/protocol
│  └─ ui-kit/                    # shared design system (dark, nightlife)
│
├─ tools/
│  ├─ netem-profiles/            # tc netem / NLC / clumsy profiles (same-city, nyc-lon, nyc-tyo, bad-wifi…)
│  ├─ latency-probe/             # impulse injection + detection
│  ├─ glitch-detector/           # discontinuity detection on test signals
│  ├─ drift-test/                # long-run alignment test
│  └─ listener-test/             # blind A/B test harness
│
├─ infra/                        # IaC for control plane, relays, storage
└─ .github/workflows/            # CI: Rust (macOS, Windows, Linux), TS, lab-sim tests
```

**Why this shape:** the engine is a library with a CLI from day one, so every hard problem can be tested headless in CI and under emulated networks before any UI exists. The desktop app and the web platform stay separable, as you wanted, and the services directory already mirrors the future service boundaries without building them.

---

## Appendix — research sources

- PairBeat — https://pairbeat.com/en/
- Beatport, "What is B2B Mode?" — https://support.beatport.com/hc/en-us/articles/26216499630100-What-is-B2B-Mode
- MusicTech, Beatport Party Mode (Oct 2022) — https://musictech.com/news/beatport-dj-party-mode-online-b2b-sets-beatsource/
- MakingWaves — https://makingwaves.live/
- Digital DJ Tips, faraway.dj review (Jul 2021) — https://www.digitaldjtips.com/i-just-played-b2b-with-a-dj-in-another-country-heres-how-it-went/
- aniclover/remote-dj-b2b (DIY rig) — https://github.com/aniclover/remote-dj-b2b
- DJ TechTools, "Will DJing back-to-back online finally be possible?" — https://djtechtools.com/2020/09/13/will-djing-back-to-back-online-finally-be-possible/
- TechCrunch, Twitch DJ Program (Jun 2024) — https://techcrunch.com/2024/06/06/twitch-djs-will-now-have-to-pay-music-labels-to-play-songs-in-livestreams/
- Mixmag, Twitch DJ revenue share — https://mixmag.net/read/twitch-djs-pay-revenue-to-record-labels-news
- JackTrip latency guidance — https://support.jacktrip.com/how-to-optimize-latency-when-using-jacktrip
- Cáceres & Chafe, "JackTrip: Under the Hood of an Engine for Network Audio" — https://ccrma.stanford.edu/groups/soundwire/publications/papers/2009-caceres_chafe-ICMC-jacktrip.pdf
- alphatheta-connect (Pro DJ Link library) — https://github.com/chrisle/alphatheta-connect
- Engine DJ community, StageLinQ API discussion — https://community.enginedj.com/t/stagelinq-protocol-api-availability-part-1/27578

Claims about OS audio APIs, codec licensing, Ableton Link licensing, DJ-software Link support, and statutory licensing conditions come from background knowledge and are labeled accordingly; each should be re-verified during Phase 0.

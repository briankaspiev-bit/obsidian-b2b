# Engine bench results — 2026-10-02

Measured on a 4-vCPU cloud Linux VM. Reproduce with `obsidian-bench sim` and `obsidian-bench run` (see `docs/engine.md`).

## Read this first

* **The simulator is the clean number.** It runs the real Opus codec, the real playout buffer and the same impairment model in virtual time, so nothing but the network model affects it. 60 minutes per case.
* **The real-time bench is an upper bound.** It runs two real peer processes and the UDP proxy on one VM. That VM stalls on its own: an idle 5 ms timer loop overshot by up to 41 ms (20 times in 30 s, hypervisor steal). Those stalls look exactly like network jitter, so they inflate booth margins (loopback "clean" got 128 ms) and cause most of the real-time glitches. A real laptop will be different; that is what the two-laptop test is for.
* Delay is the **true** capture → monitor-output delay, computed by the bench from ground truth (it knows each peer's simulated clock offset), not the engine's own estimate. Glitches are counted against a lossless decode of exactly what was sent: a "glitch event" is a run of 5 ms blocks that differ from it by more than −20 dB.
* Leader A's monitor delay includes **beat quantization** (one beat ≈ 480 ms at 125 BPM), so B→A rows show ~400 ms by design.

## Against the report's success criteria (section W)

| Criterion | Target | Result | Verdict |
|---|---|---|---|
| Delay stability | ±2 ms over 60 min, zero unplanned changes | ≤ 1.72 ms max deviation and 0 blocks > 2 ms off in every sim case; the one planned change is the route-change re-anchor (+22.8 ms, once); drift case +80/−50 ppm holds 1.36 ms | **Met in sim** |
| Latency NYC↔London | ≤ 95 ms p95 one-way | 75 ms in sim (38 ms path + 25 ms jitter buffer + 15 ms loss-recovery + 5 ms frame + block) | **Met in sim** (no device latency yet) |
| Dropouts at 0.5% random loss | ≤ 1 per 30 min | NYC↔Tokyo (0.5% loss, 8 ms jitter): 1 in 60 min with redundancy 1+3; 89 per 30 min with 1 copy only | **Met with 1+3** |
| Dropouts on bad Wi-Fi | ≤ 1 per 10 min | 760 per 30 min (1+3), 304 per 30 min (1+4+7) | **Far off.** Needs a different approach; Ethernet for performers until then |
| Drift | ≤ 2 ms after 3 h | Drift estimated to ±1 ppm (80.5 vs 80 true); 60 min run holds ±1.4 ms | Likely, 3 h run not done |
| Codec quality | near-transparent at 256 kbps | Opus CELT 256 kbps: 21.4 dB waveform SNR (not a perceptual score; PEAQ/ViSQOL not run), PCM16 82.8 dB | Unknown until a listening test |
| Beat-quantized monitoring | (not in W) | After lock, remote kicks land within 0.0–0.2 ms of the leader's on every clean-path run; 1–3 ms with the drifting clock | **Works** |
| Route change 38 → 60 ms | (not in W) | Detected and re-anchored once (+22.8 ms); 13 short concealments around the step, none after | OK |
| Merged master | ≤ 5 ms alignment at handoff | `tools/merge` on the bench handoff session: 0.025 ms fit RMS, 19/19 windows, drift measured −78.8 ppm (true 80) | **Met** for this input |


## Virtual-time simulation, 60 min per case

| Case | Min | Redund. | Delay p50 ms | Steady wander ms | Unplanned >2 ms blocks | Re-anchors | Lost on path | Recovered | PLC | Late | Glitches (per 30 min) | Drift est ppm | kbps |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| same-city | 60 | [1, 3] | 37.4 | 0.00 | 0 | none | 0 | 0 | 0 | 0 | 0 (0.0) | -0.0 | 918 |
| nyc-lon | 60 | [1, 3] | 75.4 | 0.00 | 0 | none | 2174 | 2172 | 0 | 0 | 0 (0.0) | -0.0 | 918 |
| nyc-lon-r1 | 60 | [1] | 65.4 | 0.00 | 0 | none | 2174 | 2165 | 7 | 0 | 7 (3.5) | -0.0 | 634 |
| nyc-lon-r0 | 60 | [] | 60.4 | 0.00 | 0 | none | 2174 | 0 | 2179 | 7 | 2074 (1039.9) | -0.0 | 349 |
| nyc-tyo | 60 | [1, 3] | 138.6 | 1.72 | 0 | none | 3574 | 3570 | 0 | 0 | 1 (0.5) | -0.0 | 918 |
| nyc-tyo-r1 | 60 | [1] | 128.6 | 1.72 | 0 | none | 3574 | 3548 | 92 | 70 | 89 (44.6) | -0.0 | 634 |
| bad-wifi | 60 | [1, 3] | 121.0 | 0.00 | 0 | none | 14384 | 6870 | 7612 | 119 | 1517 (760.6) | -0.0 | 918 |
| bad-wifi-r147 | 60 | [1, 4, 7] | 141.0 | 0.00 | 0 | none | 14384 | 11170 | 3193 | 0 | 606 (303.8) | -0.0 | 1203 |
| route-change | 60 | [1, 3] | 98.2 | 0.00 | 0 | +22.8 ms slew (2/199 recent packets late) | 2174 | 2172 | 15 | 15 | 13 (6.5) | -0.0 | 918 |
| drift+80/-50 | 60 | [1, 3] | 75.9 | 1.36 | 0 | none | 2175 | 2173 | 0 | 0 | 0 (0.0) | 80.5 | 918 |

Bad Wi-Fi = 2% loss in bursts averaging 25 ms plus 20–80 ms stalls every ~2.5 s. Redundancy offsets 1+3 recover any one or two lost packets in a row; longer bursts fall through to Opus concealment.

## Real-time bench, 90 s per scenario (host-noise caveat above)

These runs used the build just before the last two fixes (faster finish of a planned slew; no beat check while the leader fades out). The `handoff` rows are from the final build. Folder layout per scenario: `a/`, `b/`, `session.json`, `result.json`.

A = leader (beat-quantized monitor), B = simulated follower (drops on A's beat as heard). Delay is true capture→monitor-output delay from the bench's own clock truth, not the engine's estimate.

| Scenario | Dir | Delay p50 ms | Booth margin ms | Steady wander ms | Unplanned >2 ms blocks | Planned changes | Redund. / FEC / PLC frames | Late pkts | Glitch events (per 10 min) | Worst block SNR dB | Beat residual ms | Send kbps |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| clean | A→B | 55.8 | 128.4 | 4.19 | 876 | 2 | 0 / 0 / 22 | 22 | 6 (45.1) | -11.8 | 80.0 | 925 |
| clean | B→A | 421.7 | 416.6 | 0.00 | 0 | 2 | 0 / 0 / 10 | 10 | 0 (0.0) | 172.1 | -0.0 | 925 |
| same-city | A→B | 65.8 | 54.6 | 0.00 | 0 | 0 | 0 / 0 / 1 | 1 | 2 (15.0) | -3.3 | 0.6 | 925 |
| same-city | B→A | 410.1 | 398.9 | 0.00 | 0 | 1 | 0 / 0 / 0 | 0 | 0 (0.0) | 179.4 | 0.0 | 925 |
| nyc-lon | A→B | 99.2 | 63.9 | 0.00 | 0 | 0 | 44 / 0 / 8 | 8 | 5 (37.6) | -5.0 | 0.3 | 925 |
| nyc-lon | B→A | 376.6 | 341.3 | 0.00 | 0 | 1 | 50 / 0 / 1 | 1 | 1 (7.5) | 2.9 | 0.1 | 925 |
| nyc-tyo | A→B | 137.5 | 91.4 | 1.41 | 0 | 1 | 68 / 0 / 21 | 21 | 5 (37.6) | -9.0 | -1.7 | 925 |
| nyc-tyo | B→A | 337.4 | 254.1 | 0.00 | 0 | 1 | 81 / 0 / 0 | 0 | 0 (0.0) | 169.3 | -0.1 | 925 |
| bad-wifi | A→B | 131.6 | 95.3 | 0.98 | 0 | 1 | 173 / 0 / 183 | 33 | 40 (300.9) | -13.6 | 50.9 | 925 |
| bad-wifi | B→A | 394.5 | 358.7 | 0.00 | 0 | 1 | 172 / 0 / 161 | 0 | 31 (233.2) | -8.4 | -0.0 | 925 |
| bad-wifi-r0 | A→B | 113.7 | 77.3 | 2.75 | 274 | 1 | 0 / 0 / 373 | 23 | 78 (586.8) | -13.6 | -1.1 | 355 |
| bad-wifi-r0 | B→A | 108.6 | 323.8 | 1.61 | 0 | 2 | 0 / 0 / 324 | 12 | 29 (218.2) | -8.6 | 0.1 | 355 |
| bad-wifi-r147 | A→B | 154.9 | 118.4 | 1.12 | 0 | 1 | 260 / 0 / 37 | 11 | 13 (97.8) | -16.2 | 50.1 | 1209 |
| bad-wifi-r147 | B→A | 370.4 | 334.5 | 0.00 | 0 | 1 | 306 / 0 / 55 | 0 | 14 (105.3) | -7.0 | -0.0 | 1209 |
| route-change | A→B | 77.9 | 51.6 | 0.00 | 0 | 1 | 36 / 0 / 14 | 14 | 6 (45.1) | -6.4 | 31.9 | 925 |
| route-change | B→A | 397.8 | 362.6 | 0.00 | 0 | 2 | 45 / 0 / 7 | 7 | 0 (0.0) | 171.4 | 0.1 | 925 |
| drift-offset | A→B | 76.5 | 41.2 | 0.03 | 0 | 0 | 52 / 0 / 13 | 13 | 3 (22.6) | -13.6 | 4.6 | 925 |
| drift-offset | B→A | 398.8 | 368.7 | 2.35 | 877 | 3 | 40 / 0 / 7 | 7 | 0 (0.0) | 61.0 | -2.8 | 925 |
| pcm-same-city | A→B | 48.2 | 225.7 | 0.00 | 0 | 1 | 0 / 0 / 42 | 42 | 3 (22.6) | -2.8 | 189.3 | 4764 |
| pcm-same-city | B→A | 432.4 | 421.4 | 0.00 | 0 | 1 | 0 / 0 / 0 | 0 | 0 (0.0) | 178.2 | -0.1 | 4764 |
| handoff | A→B | 108.0 | 72.5 | 0.00 | 0 | 0 | 48 / 0 / 0 | 0 | 0 (0.0) | 64.2 | – | 925 |
| handoff | B→A | 365.7 | 330.2 | 2.06 | 176 | 1 | 57 / 0 / 0 | 0 | 0 (0.0) | 62.4 | – | 925 |

## Clocks, codec, CPU

| Scenario | Dir | Clock-sync error µs | Drift est ppm | ASRC corrections | Codec SNR dB (vs source) | Codec delay samples | CPU % (one peer) |
|---|---|---|---|---|---|---|---|
| clean | A→B | -22 | -0.0 | 0 | 21.4 | 120 | 6.9 |
| clean | B→A | 14 | -0.4 | 0 | 21.5 | 120 | 7.0 |
| same-city | A→B | 325 | -0.0 | 0 | 21.4 | 120 | 6.9 |
| same-city | B→A | -87 | -0.0 | 0 | 21.5 | 120 | 7.1 |
| nyc-lon | A→B | 3730 | -0.0 | 0 | 21.4 | 120 | 6.9 |
| nyc-lon | B→A | 1669 | -0.0 | 0 | 21.5 | 120 | 7.0 |
| nyc-tyo | A→B | -1868 | -0.0 | 3 | 21.4 | 120 | 6.8 |
| nyc-tyo | B→A | 694 | -0.0 | 0 | 21.5 | 120 | 6.9 |
| bad-wifi | A→B | -4756 | -0.0 | 1 | 21.4 | 120 | 6.8 |
| bad-wifi | B→A | 3131 | -0.0 | 0 | 21.5 | 120 | 7.0 |
| bad-wifi-r0 | A→B | 113 | -0.0 | 0 | 21.4 | 120 | 6.4 |
| bad-wifi-r0 | B→A | -457 | -0.0 | 1 | 21.5 | 120 | 6.9 |
| bad-wifi-r147 | A→B | 2076 | -0.0 | 1 | 21.4 | 120 | 7.0 |
| bad-wifi-r147 | B→A | -1790 | -0.0 | 0 | 21.5 | 120 | 7.1 |
| route-change | A→B | -496 | -0.0 | 0 | 21.4 | 120 | 8.1 |
| route-change | B→A | -375 | -0.0 | 0 | 21.5 | 120 | 8.3 |
| drift-offset | A→B | 2889 | -0.0 | 0 | 21.4 | 120 | 8.3 |
| drift-offset | B→A | 1545 | 83.4 | 1 | 21.5 | 120 | 8.3 |
| pcm-same-city | A→B | 908 | -0.0 | 0 | 82.8 | 0 | 5.4 |
| pcm-same-city | B→A | 56 | -0.0 | 0 | 82.8 | 0 | 5.4 |
| handoff | A→B | -376 | -0.0 | 0 | 21.4 | 120 | 6.9 |
| handoff | B→A | 1573 | 80.9 | 1 | 21.5 | 120 | 7.4 |

## Beat-quantized monitoring at A

| Scenario | Lag before (ms) | Extra delay added (ms) | Later checks: lag (ms) |
|---|---|---|---|
| clean | 115.9 | 364.1 | -0.0, -0.0, -0.0, -0.0, -0.0, -0.0, -0.0 |
| same-city | 137.1 | 342.9 | 0.1, 0.1, 0.1, 0.0, 0.1, 0.1, 0.0 |
| nyc-lon | 196.1 | 283.9 | 0.1, 0.1, 0.1, 0.0, 0.1, 0.1, 0.1 |
| nyc-tyo | -199.2 | 199.2 | -0.0, -0.0, -0.0, -0.1, -0.0, -0.0, -0.0 |
| bad-wifi | 206.5 | 273.5 | -0.0, 0.0, -0.0, -0.0, -0.0, -0.0, -0.0 |
| bad-wifi-r0 | 228.2 | 251.8 | 0.1, 0.1, 0.0 |
| bad-wifi-r147 | -227.1 | 227.1 | -0.0, -0.0, -0.0, -0.0, -0.0, -0.0, -0.0 |
| route-change | 160.1 | 319.9 | 0.1, 0.1, 0.1, 0.0, 0.1, 0.1, 0.1 |
| drift-offset | 159.0 | 321.0 | -0.3, -3.0, -4.0, -2.9, -1.8, -2.2, -3.0 |
| pcm-same-city | 95.7 | 384.3 | -0.0, -0.0, -0.1, -0.1, -0.0, -0.1, -0.1 |
| handoff | 214.3 | 265.7 | -0.2 |

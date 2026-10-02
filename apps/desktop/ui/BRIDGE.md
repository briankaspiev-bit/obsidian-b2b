# Desktop bridge: UI ↔ Rust

The UI's `TauriSessionEngine` (`src/session/tauriEngine.ts`) talks to the
Rust side (`src-tauri/src/lib.rs`) only through these. Types are in
`src/session/bridge.ts`.

## Commands

| Command | Args | Returns |
|---|---|---|
| `engine_info` | | `{ version, roomServer: string \| null, liveAudio: boolean }` |
| `list_devices` | | `{ inputs, outputs }`, each `{ id, label, detail }`, system default first |
| `start_input_meter` / `stop_input_meter` | `id` | opens the input and starts `local-level` |
| `create_room` | `name` | the code, `"K7QX-M2PD"`; the guest arriving is a `room-event` |
| `join_room` | `code`, `name` | `{ code, peerName, isHost, relay }` or a plain-language error |
| `leave_room` | | says goodbye to the other booth and the server |
| `send_control` | `json` (≤1200 bytes) | sent to the other app as `peer-control` |
| `select_output` | `id` | headphones for the set |
| `start_live` | `startOnAir` | the engine's live session takes the socket, the metered input and the headphones |
| `live_take_over` | | TAKE OVER, through the engine |
| `live_set_ready` | `ready` | READY while cueing; TAKE OVER clears it |
| `live_set_fader` / `live_set_partner_volume` | `value` (0–1 / 0–2) | |
| `stop_live` | | ends the set (or practice); the folder its recordings went to |
| `load_track` | raw body: the file; header `x-name`: its name, percent-encoded | the path it was saved to, for `start_practice` |
| `start_practice` | `name`, `track` (path or null for the built-in groove) | `{ partnerName, path, track }`: your deck against the engine's ghost DJ over a simulated `nyc-lon` link |
| `deck_command` | `action`: `playPause` \| `cue` \| `sync` \| `nudge` (ms, + is earlier) \| `pitch` (%), `value` | the built-in deck |
| `ghost_come_back` | | practice: the ghost cues its next track now |
| `deck_wave` | | raw bytes: your deck's whole track, low/mid/high per 5 ms (empty without a deck) |
| `run_network_test` | `seconds` | `{ pingsSent, pongsReceived, lossPct, rttMs, rttMinMs, jitterMs, clockOffsetMs, reached }` |

## Events

| Event | Payload |
|---|---|
| `room-event` | `{ kind: "paired", code, peerName, isHost, relay }` or `{ kind: "error", message }` |
| `peer-control` | the JSON string the other app sent |
| `link-status` | `{ state: "connected" \| "reconnecting" \| "left", rttMs, jitterMs, lossPct, relay }`, every 500 ms once the other booth was heard |
| `local-level` / `remote-level` | `{ left, right }` in dBFS, `null` = silence (before the set) |
| `live-status` | `obsidian_live::LiveStatus` (snake_case) plus `ghost_says`, about 15 times a second during the set |
| `live-scope` | `{ first, cols: [{ you: [lo, mid, hi], partner: [lo, mid, hi] }] }`: waveform columns made since the last event, one per 5 ms; column `i` is heard at `i * 5` ms on the output clock |

## On the wire

After pairing, `link.rs` owns the UDP socket the rendezvous service handed
over. It sends a heartbeat ping every 100 ms in the engine's own format
(`obsidian_protocol::Packet::Ping/Pong`, magic `0x0B5D`) and answers the
other side's. Coordination and levels use their own magic, `0x0B 0x43`,
then a kind byte: `1` JSON message, `2` level (two little-endian f32),
`3` goodbye. The relay forwards them like media. No packet for 1.5 s reads
as reconnecting, a goodbye or 30 s as left.

Peer messages (`src/session/peer.ts`): `hello {name, city}`,
`boothReady {ready}`, `markReady`, `cancelReady`, `takeOver`,
`emergencyTakeOver`, `end`, `leave`. Both apps run the same reducers and
apply the other's moves as the remote DJ's; both run the handoff timer.

At the end of the countdown the link steps aside without a goodbye
(`BoothLink::hand_over`) and `obsidian_live::run_live` takes the same
socket. From then on the engine's own `State` packet carries who is on air;
the screen follows `live-status` (`on_air` / `partner_on_air`), and TAKE OVER
is `live_take_over`.

## The DJ view

`live-status` carries `now_ms` (the engine's output clock), `you_beat` and
`partner_beat` (`{ period_ms, beat_ms, bar_ms }`: beats at `beat_ms + k *
period_ms` on the same clock; `bar_ms` a beat 1 when known), `deck` (title,
playing, bpm, pitch, SYNC, position) and the fader values. With
`live-scope`, that is everything `src/live-session/DeckView.tsx` draws: both
decks' waveforms on one play head with beat grids, and the phase meter
(`phaseOf` in `src/session/deck.ts`: your kick against theirs in ms, and bar
offset once both bars are known). Partner bars are not known yet.

# Obsidian desktop app

The Windows desktop app: a Tauri shell (`src-tauri/`, Rust) around the
React + TypeScript UI (`src/`). Screens: **Home** (your DJ name, create or
join a room), **Booth Check** (devices, network test, ready) and **Live
Session** (who's on air, TAKE OVER, handoffs).

```
npm install
npm run dev            # UI only, in a browser, on the demo engine
npm test               # reducers, peer messages, two apps talking (fake bridge)
npm run build:preview  # one self-contained HTML file in dist-preview/

npx tauri dev          # the desktop app (needs Rust; on Linux, webkit2gtk-4.1 + ALSA dev packages)
npx tauri build        # installer in src-tauri/target/release/bundle/
cd src-tauri && cargo test   # devices, booth link, two booths through a real room server
```

CI (`.github/workflows/desktop.yml`) runs all tests and builds the Windows
installer; download it from the run's **Artifacts**.

## Where rooms are made

The app needs the rendezvous server from `services/rendezvous`. It reads its
address from `OBSIDIAN_SERVER` (`host:3478`) at run time, or from
`OBSIDIAN_DEFAULT_SERVER` baked in at build time (CI uses the repository
variable `OBSIDIAN_SERVER`). Without one, Home explains that and offers the
demo.

To try two copies on one machine:

```
cd services/rendezvous && cargo run --bin rendezvous-server     # udp 0.0.0.0:3478
OBSIDIAN_SERVER=127.0.0.1:3478 npx tauri dev                     # then a second copy of the built app
```

## What's real and what isn't yet

| | Real today | Waits for |
|---|---|---|
| Room codes, pairing, direct or relayed path | yes (`services/rendezvous`) | a deployed server |
| Devices and your send meter | yes (cpal / WASAPI) | |
| Round trip, jitter, loss, clock match | yes (engine's ping/pong and clock-sync, `link.rs`) | |
| The other DJ's meter | yes: their mixer's level, sent over the link | |
| Hearing the other DJ, sending yours, recording | yes: at the end of the countdown the engine's live session (`crates/live`) takes the connection, your input and headphones; recordings go to Music\\Obsidian | |
| TAKE OVER, emergency take over | yes, through the engine (its `State` packet) | |
| The other DJ's READY during the set | no: shows on your screen only | a ready flag in the engine's `State` |
| "Everything this laptop plays" as your send | yes, Windows 10 2004+ (system audio minus Obsidian itself) | |

See `BRIDGE.md` for the commands, events and peer messages.

## Layout

- `src/session/` — no UI. `types.ts`, the pure `roomReducer.ts` and
  `sessionReducer.ts`, `engine.ts` (the `SessionEngine` interface every
  screen talks to), `tauriEngine.ts` (the real one), `bridge.ts` (typed Tauri
  calls), `peer.ts` (messages between the two apps, Booth Check scoring),
  `mockEngine.ts` (the demo).
- `src/app/` — `Root.tsx` picks the engine (real in the app, demo in a
  browser or on request), `App.tsx` picks the screen.
- `src/home/`, `src/booth/`, `src/live-session/` — the screens.
- `src-tauri/src/` — `lib.rs` (commands, events), `devices.rs`, `link.rs`
  (booth link), `nettest.rs`, `live.rs` (starts the engine's live session).

TAKE OVER only changes coordination state and logs events. It never mutes,
cuts or reroutes anyone's audio.

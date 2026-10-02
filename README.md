# Obsidian B2B

Remote back-to-back (B2B) DJ sessions: two DJs in different cities play one set together, with a social space where audiences can watch live.

Early stage. What is here today:

| Path | What it is |
|---|---|
| `apps/desktop/ui/` | Windows desktop app: Tauri shell (`src-tauri/`) around the React UI (Home, Booth Check, Live Session). Real room codes, devices and booth link; demo engine one click away. See its README. |
| `notes/` | Implementation notes. |
| `previews/` | Self-contained HTML build of the UI preview. |
| `docs/remote-b2b-feasibility-report.md` | Feasibility report: latency model, engine choice, rights, first milestone. |
| `crates/` | Rust booth engine: wire format, Opus codec, clock sync, fixed-delay jitter buffer, beat alignment, peer session, `obsidian-peer` CLI. See `docs/engine.md`. |
| `crates/live`, `crates/audio-io` | `obsidian-live`: the engine on real sound cards (Windows build), music-file deck or system audio, TAKE OVER, fader, SYNC, and solo practice with a ghost DJ. How to run it: `docs/live.md`. |
| `tools/netem-proxy`, `tools/netem-profiles`, `tools/bench` | Bad-network test bench: UDP impairment proxy, lab profiles, real-time and virtual-time benches. Results in `docs/engine-results.md`. |

Coming next: real audio devices and connect-by-code so two people can try a remote B2B on two Windows laptops (plan in `docs/engine.md`).

## Run the UI

```
cd apps/desktop/ui
npm install
npm run dev
npm test
```

## Run the engine bench

```
cargo test --workspace --release
cargo run --release -p obsidian-bench -- sim --minutes 60      # virtual time, about a minute per case
cargo run --release -p obsidian-bench -- run --jobs 1          # real-time, two peers + proxy, ~17 min
```

# Obsidian B2B

Remote back-to-back (B2B) DJ sessions: two DJs in different cities play one set together, with a social space where audiences can watch live.

Early stage. What is here today:

| Path | What it is |
|---|---|
| `apps/desktop/ui/` | Desktop app UI (React + TypeScript + Vite): Home, Booth Check and Live Session screens, running on a mock engine. See its README. |
| `notes/` | Implementation notes. |
| `previews/` | Self-contained HTML build of the UI preview. |
| `docs/remote-b2b-feasibility-report.md` | Feasibility report: latency model, engine choice, rights, first milestone. |

Coming next (per the feasibility report's layout): `crates/` for the Rust real-time audio engine, and `tools/` for benchmarks and the master-mix merge tool.

## Run the UI

```
cd apps/desktop/ui
npm install
npm run dev
npm test
```

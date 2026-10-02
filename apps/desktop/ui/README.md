# Obsidian B2B desktop UI

React + TypeScript UI for the desktop app (to sit inside the Tauri shell at
`apps/desktop/ui`, per the feasibility report's repo layout). Today it holds
one screen: **Live Session**.

```
npm install
npm run dev            # local dev server
npm run typecheck
npm test               # session state machine tests
npm run build:preview  # one self-contained HTML file in dist-preview/
```

## Layout

- `src/session/` — no UI. `types.ts` (domain), `sessionReducer.ts` (pure
  handoff state machine + event log), `engine.ts` (the `SessionEngine`
  interface the UI talks to), `mockEngine.ts` (**mock only**), `useSession.ts`
  (React hooks over the engine).
- `src/live-session/` — the screen and its parts.
- `src/styles/` — tokens (shared palette with the clickable mockup) and the
  screen's stylesheet.

## Wiring the real engine

Implement `SessionEngine` over the Rust engine's Tauri commands/events and
provide it in `main.tsx` instead of `MockSessionEngine`. Then delete
`mockEngine.ts` and `MockControls.tsx`. Set `VITE_HIDE_MOCK_CONTROLS=true`
to hide the mock panel before then.

TAKE OVER only changes coordination state and logs events. It never mutes,
cuts or reroutes anyone's audio.

# Live Session screen: implementation report

Date: 2026-10-02. Code: `code/obsidian/apps/desktop/ui/` in the project files.
Live preview: https://claude.ai/artifact/LaaHPJyMAGPZGFmx5GVAFi

## Starting point

- No app codebase existed (no GitHub repo linked). The only Live Session screen was the
  clickable design mockup (https://claude.ai/artifact/GqWkBEQoCFAbddKX7krCiS).
- So this is a new React 19 + TypeScript + Vite package at `apps/desktop/ui`, the
  path the feasibility report's monorepo layout gives the Tauri UI.
- Kept from the mockup: palette (#0A0A0D, #FF6A3D, #6CC4FF, greys #9A9AA8/#1A1A21/#34343F,
  ok green #7FE0A8, amber #FFC24D), Space Grotesk + JetBrains Mono, "Brian K" / "Maya",
  section R status words (Booth Sync, Network, Remote DJ, Recording).

## FILES CHANGED (all new)

| File | Purpose |
| --- | --- |
| `package.json`, `tsconfig.json`, `vite.config.ts`, `index.html`, `.gitignore`, `README.md` | Package setup; `build:preview` makes a one-file HTML preview |
| `src/main.tsx` | Mounts the screen with the mock engine (the only place the mock is chosen) |
| `src/session/types.ts` | Domain types, split into session, link (network/recording) and level state |
| `src/session/sessionReducer.ts` | Pure handoff state machine + event log |
| `src/session/sessionReducer.test.ts` | 7 tests for the state machine (plus 7 for the room) |
| `src/session/engine.ts` | `SessionEngine` interface the UI depends on; `HANDOFF_DURATION_MS` |
| `src/session/mockEngine.ts` | **Mock** engine (tracks, levels, health, diagnostics, dropout) |
| `src/session/useSession.ts` | React hooks over the engine (`useSyncExternalStore`) |
| `src/lib/format.ts` | Timer and remaining-time formatting |
| `src/live-session/*.tsx`, `labels.ts` | The screen and its components (below) |
| `src/styles/tokens.css`, `live-session.css` | Tokens and the screen's styles |

## COMPONENTS CREATED

- `LiveSessionScreen`: five zones (header, local DJ, center, remote DJ, status strip) on a 3-column grid; the side that owns the mix widens.
- `SessionHeader`: OBSIDIAN B2B / Live Session, lock + Private Room.
- `SessionTimer`: large mono timer, "TWO CITIES. ONE BOOTH." above.
- `DjPanel`: name, city, state pill, compact track row, top-edge light bar. Colour follows the role (orange ON AIR, blue CUEING/READY), never the person. The local panel carries MARK READY / CANCEL.
- `StatePill`: ON AIR / CUEING / READY / HANDOFF with an LED dot; text fades between states.
- `HandoffRail`: line between the DJs. Orange energy on the owner's half; during a handoff it drains, a pulse crosses, and it fills the other side. When a DJ is READY their half glows blue and three chevrons light in turn toward the middle.
- READY state (revised after Brian's feedback): the ready DJ's panel gets a blue tint and a slow breathing glow, the center shows a pulsing "MAYA READY" badge, and the on-air DJ's TAKE OVER ring turns blue with "Maya is ready to take over" under it. No flashing; 2.4 s breathing cycle; off under reduced motion.
- YOU'RE LIVE state (revised after Brian's feedback): on the live DJ's screen the center reads a large orange "YOU'RE LIVE" with a solid LED, a thin orange frame lines the window edges, and YOUR SEND turns orange and reads "GOING OUT LIVE". All of it is steady (never pulses) so it can't be confused with READY, which breathes. When the other DJ is ready, their READY badge sits under the rail so YOU'RE LIVE never disappears. The frame fades across over the handoff.
- Ready while you're live (Brian's request): when the other DJ marks READY, your orange window frame and their blue panel flash together, a smooth 1.2 s cycle (under one flash per second, well within photosensitivity limits). Steady under reduced motion.
- On-air glass (Brian's request): the panel of whoever owns the mix gets an orange frosted-glass background (tint, top sheen, lit inner edge). It cross-fades from one panel to the other over the handoff.
- On-air photo (Brian's request): only the DJ who owns the mix shows their photo. It fills the right half of their card, masked to fade out toward the name and the bottom, with darkened edges so it blends into the card; it fades across with the handoff (revised from a circle at Brian's request). While a DJ is READY, their photo flashes in and out (1.2 s, in time with the blue pulse); steady at 60% under reduced motion. `Dj.photoUrl` is optional. Mock DJs renamed to Val (New York) and Dana (London), with Brian's placeholder photos cropped to 560×700 portraits in `src/assets/mock/`.
- Emergency take over (Brian's request): if the DJ who owns the mix loses their connection, the other DJ's screen goes red at once: flashing red window frame, "DANA DROPPED / No audio from Dana. Take over now", the dropped DJ's photo goes grey, and the button becomes TAKE OVER NOW, which fires on a single tap (no hold, no handoff, no confirmation from the dropped side). New reducer action `emergencyTakeOver` and event `emergency_take_over`; new engine method `emergencyTakeOver()`. Red (not orange) so it can't be mistaken for live; flashes under 2 per second; steady under reduced motion.
- `TakeOverControl`: round hardware-style control. Press and hold 650 ms (mouse, touch, Space or Enter); a ring fills while held. When you're on air it reads ON AIR and is inert, with "Maya takes over from London" under it.
- TAKE OVER button, louder (Brian's request): when you're live it is a solid lit orange disc reading ON AIR with a running time-on-air clock (from your last handoff or emergency take over; falls back to session start). When you can take over it has a bright white ring and label; holding fills the ring orange.
- `LevelMeter`: 28-segment stereo meter with release and peak hold, used twice. YOUR SEND (under your panel) shows what Obsidian captures from your mixer and warns "No signal from your mixer" if you're on air and it reads silence. REMOTE LEVEL shows what arrives from Maya: "Quiet while Maya cues" when she isn't playing out, "Remote audio recovering…" on a drop.
- `SystemHealthStrip`: four equipment-style indicators; grey when healthy, amber when not, plus a one-line reassurance during a drop.
- `DiagnosticsDrawer`: closed by default; the only place round trip, delay, jitter, loss, buffer, drift, path and codec appear.
- `EndSessionControl`: quiet button plus a confirm popover ("Keep playing" focused first, Esc closes).
- `MockControls`: a small DEMO button at the top left, just under the logo, closed by default, that opens a dashed panel to play the remote DJ's side and drop their connection. Moved there so it never crowds the centered set timer.

## STATE MODEL USED

- **Session state** (`SessionState`): `ownerId`, `readyIds`, `handoff {from, to, startedAtMs, durationMs}`, `status`, `events[]`. Roles are derived (`roleOf`): in a handoff → HANDOFF for both; owner → ON AIR; ready → READY; else CUEING. Actions: markReady, cancelReady, takeOver, completeHandoff, remoteReconnecting/Reconnected, end. Event types mirror the report's `session_events` table so handoff timestamps can feed the master rebuild.
- **Link state** (`LinkState`): remote connection, booth sync, network quality, recording, diagnostics numbers.
- **Levels** (`StereoLevel`, local send and remote): separate high-rate subscriptions so meter updates don't re-render the screen.
- **UI state** stays in components: drawer open, confirm open, hold progress.
- TAKE OVER only changes coordination state and logs events. Nothing in the UI can mute, cut or reroute audio.

## MOCKED BEHAVIOR

All in `mockEngine.ts` and `MockControls.tsx`:
- Session opens at 00:42:19 with Brian ON AIR and Maya CUEING; track times count down and rotate.
- The handoff completes on a 1.6 s timer (a real engine might confirm from audio or the other peer).
- Maya's level follows her role: silent while cueing, rising through a handoff, kick-shaped while on air.
- Diagnostics numbers wobble around plausible values.
- "Drop Maya's connection" reconnects after 6 s; Network shows Recovering → Good → Excellent.

## VALIDATION RUN

- `tsc -b`: clean. `vitest run`: 14/14 passing.
- `vite build` and the single-file preview build both succeed.
- Playwright screenshots at 1440×900, 1920×1080 and 1280×760 of: Brian on air, Maya ready, mid-handoff, Maya on air, Brian ready, holding TAKE OVER, Brian taking over, dropout, Diagnostics open, End confirm, ended. Fixes made from them: footer pushed off-screen at 900 px with READY under the button (moved READY into the local panel, sized vertical spacing to the window height), meter readout flicker, mock panel overlap.
- Contrast: smallest grey text (#7A7A88) is 4.7:1 on the background.
- Not run: real Tauri shell, Windows WebView2 rendering, screen reader pass.

## TECHNICAL DEBT CREATED

- Fonts load from Google Fonts; the desktop app should bundle them locally (offline booths).
- `MockSessionEngine` and `MockControls` must be replaced/removed when the engine exists; the handoff completion timer lives in the mock, not the UI.
- No Tauri shell, routing, or other screens yet; `main.tsx` mounts this screen directly.
- `min-width: 1024px` on the screen; below that it scrolls sideways inside the window.
- Layout tuned by screenshot at three sizes only; very short windows (<720 px tall) may scroll.
- No component tests; only the state machine is tested.
- Emergency is triggered by the remote link going to `reconnecting`. The real engine should also trigger it when the live DJ's audio stops arriving for about a second even if the link looks up, and should handle a drop mid-handoff (keep the mix with the original owner until both sides confirm). Neither is built.
- The code lives in project files, not a git repo. Moving it into a repo is the next step once one is connected.

## Home and Booth Check (added 2026-10-02)

The app now opens on Home and flows Home → Booth Check → 3-2-1 → Live Session. `src/app/App.tsx` picks the screen from `RoomState.phase`.

- **Home** (`src/home/HomeScreen.tsx`): CREATE ROOM, or JOIN ROOM with an 8-character code (any spacing or case; `0000-0000` simulates "no such room").
- **Booth Check** (`src/booth/BoothCheckScreen.tsx`): invite code with Copy while the other DJ hasn't joined; "What you send" and "Where you hear Dana" device pickers; a YOUR SEND meter; five check steps (connection, round trip, your mixer, hearing the other DJ, Booth Sync) with a FROM DANA meter that shows the test tone; I'M READY, enabled only once the check passed. When both are ready a 3-2-1 overlay runs and the session starts at 00:00:00 with the room's host on air.
- **State**: `RoomState` in `types.ts`, pure `roomReducer.ts` (+ 7 tests). Rules: ready needs a passed check; changing a device resets the check and readiness; the countdown starts when the second DJ presses ready, from either side.
- **Engine interface** gained `getRoom/subscribeRoom`, `createRoom`, `joinRoom`, `selectInput`, `selectOutput`, `runBoothCheck`, `setBoothReady`, `leaveRoom`. The mock fakes Dana joining (3 s after create), the check steps (0.75 s each) and Dana pressing ready (2.6 s after your check passes). DEMO buttons: "Dana joins now", "Dana presses ready", "Skip to the live session", "Back to home".
- Session ended overlay has NEW ROOM (back to Home).
- **Debt**: device lists, check results and the join lookup are all mocked; a check step that comes back amber blocks READY (may be too strict for a "fair" network); no screen for picking your name/photo yet.

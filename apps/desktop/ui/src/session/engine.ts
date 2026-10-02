import type { LinkState, RoomState, SessionState, StereoLevel } from './types';

type Unsubscribe = () => void;

/**
 * The contract between the Live Session UI and whatever runs the session.
 * Today that is MockSessionEngine. Later it is a thin adapter over the Rust
 * engine's IPC/command API (Tauri commands + events).
 *
 * Commands always act as the local DJ.
 */
export interface SessionEngine {
  // --- Before the session: Home and Booth Check ---------------------------

  getRoom(): RoomState;
  subscribeRoom(listener: () => void): Unsubscribe;

  /** Your name (and city) as the other DJ will see it. */
  setLocalProfile(profile: { name: string; city: string }): void;
  /** Opens a private room and shows its invite code. */
  createRoom(): void;
  /** Looks up an invite code; on failure sets RoomState.joinError. */
  joinRoom(code: string): void;
  selectInput(deviceId: string): void;
  selectOutput(deviceId: string): void;
  /** Runs the booth check steps in order; needs the other DJ in the room. */
  runBoothCheck(): void;
  /** Ready for the session. Only accepted once the booth check passed. */
  setBoothReady(ready: boolean): void;
  /** Back to Home. Also used after a session ends. */
  leaveRoom(): void;

  // --- During the session ---------------------------------------------------

  getSession(): SessionState;
  subscribeSession(listener: () => void): Unsubscribe;

  getLink(): LinkState;
  subscribeLink(listener: () => void): Unsubscribe;

  /** What arrives from the remote DJ, after the jitter buffer. */
  getRemoteLevel(): StereoLevel;
  subscribeRemoteLevel(listener: () => void): Unsubscribe;

  /** What Obsidian captures from the local mixer and sends out. */
  getLocalLevel(): StereoLevel;
  subscribeLocalLevel(listener: () => void): Unsubscribe;

  markReady(): void;
  cancelReady(): void;
  takeOver(): void;
  /**
   * Only offered when the live DJ's connection is lost. Takes the mix at once,
   * with no handoff and no confirmation from the other side.
   */
  emergencyTakeOver(): void;
  endSession(): void;
}

/** How long the HANDOFF state shows before ownership flips. */
export const HANDOFF_DURATION_MS = 1600;

/** The 3-2-1 after both DJs press ready. */
export const START_COUNTDOWN_MS = 3000;

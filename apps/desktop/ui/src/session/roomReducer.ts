import type { AudioDevice, BoothCheck, CheckStepId, CheckStepStatus, RoomState } from './types';

export type RoomAction =
  | { type: 'setLocal'; local: RoomState['local'] }
  | { type: 'createStarted' }
  | { type: 'createFailed'; error: string }
  | { type: 'joinStarted' }
  | { type: 'joinFailed'; error: string }
  | { type: 'entered'; code: string; isHost: boolean }
  | { type: 'remoteJoined'; remote: NonNullable<RoomState['remote']> }
  | { type: 'remoteProfile'; remote: NonNullable<RoomState['remote']> }
  | { type: 'remoteLeft' }
  | { type: 'remoteReady'; ready: boolean }
  | { type: 'devicesFound'; inputs: AudioDevice[]; outputs: AudioDevice[] }
  | { type: 'selectInput'; id: string }
  | { type: 'selectOutput'; id: string }
  | { type: 'checkStarted' }
  | { type: 'checkStep'; id: CheckStepId; status: CheckStepStatus; result?: string }
  | { type: 'setReady'; ready: boolean; atMs: number; countdownMs: number }
  | { type: 'enterLive' }
  | { type: 'leave' };

const CODE_CHARS = /[^A-Z0-9]/g;

/** "k7qx m2pd" → "K7QX-M2PD". Returns null unless it has exactly 8 letters or digits. */
export function normalizeCode(input: string): string | null {
  const raw = input.toUpperCase().replace(CODE_CHARS, '');
  return raw.length === 8 ? `${raw.slice(0, 4)}-${raw.slice(4)}` : null;
}

export function freshCheck(): BoothCheck {
  return {
    status: 'idle',
    steps: [
      { id: 'network', label: 'Connection to the other booth', status: 'pending', result: null },
      { id: 'roundTrip', label: 'Round trip', status: 'pending', result: null },
      { id: 'send', label: 'Your mixer is coming through', status: 'pending', result: null },
      { id: 'receive', label: 'You can hear the other DJ', status: 'pending', result: null },
      { id: 'sync', label: 'Booth Sync', status: 'pending', result: null },
    ],
  };
}

export function initialRoom(local: RoomState['local']): RoomState {
  return {
    phase: 'home',
    code: null,
    isHost: false,
    joining: false,
    joinError: null,
    creating: false,
    createError: null,
    local,
    remote: null,
    remotePresence: 'waiting',
    inputs: [],
    outputs: [],
    inputId: null,
    outputId: null,
    check: freshCheck(),
    localReady: false,
    startsAtMs: null,
  };
}

/** Both DJs pressed ready: start the countdown (once). */
function withCountdown(s: RoomState, atMs: number, countdownMs: number): RoomState {
  if (s.localReady && s.remotePresence === 'ready' && s.startsAtMs === null) {
    return { ...s, startsAtMs: atMs + countdownMs };
  }
  return s;
}

/**
 * Pure room state machine for Home and Booth Check. Readiness rules:
 * you can only press ready once your booth check passed, and changing a
 * device or the other DJ leaving puts you back to not-ready.
 */
export function roomReducer(s: RoomState, a: RoomAction): RoomState {
  switch (a.type) {
    case 'setLocal':
      return { ...s, local: a.local };
    case 'createStarted':
      return { ...s, creating: true, createError: null, joinError: null };
    case 'createFailed':
      return { ...s, creating: false, createError: a.error };
    case 'joinStarted':
      return { ...s, joining: true, joinError: null, createError: null };
    case 'joinFailed':
      return { ...s, joining: false, joinError: a.error };
    case 'entered':
      return { ...s, phase: 'booth', code: a.code, isHost: a.isHost, joining: false, joinError: null, creating: false };
    case 'remoteJoined':
      return { ...s, remote: a.remote, remotePresence: 'joined' };
    case 'remoteProfile':
      return s.remote ? { ...s, remote: { ...s.remote, ...a.remote } } : s;
    case 'remoteLeft':
      return { ...s, remotePresence: 'waiting', localReady: false, startsAtMs: null, check: freshCheck() };
    case 'remoteReady': {
      if (s.remotePresence === 'waiting') return s;
      const next: RoomState = { ...s, remotePresence: a.ready ? 'ready' : 'joined' };
      return a.ready ? next : { ...next, startsAtMs: null };
    }
    case 'devicesFound':
      return {
        ...s,
        inputs: a.inputs,
        outputs: a.outputs,
        inputId: s.inputId ?? a.inputs[0]?.id ?? null,
        outputId: s.outputId ?? a.outputs[0]?.id ?? null,
      };
    case 'selectInput':
    case 'selectOutput': {
      const key = a.type === 'selectInput' ? 'inputId' : 'outputId';
      if (s[key] === a.id) return s;
      // A different device means the old check no longer proves anything.
      return { ...s, [key]: a.id, check: freshCheck(), localReady: false, startsAtMs: null };
    }
    case 'checkStarted':
      return {
        ...s,
        localReady: false,
        startsAtMs: null,
        check: { status: 'running', steps: freshCheck().steps },
      };
    case 'checkStep': {
      const steps = s.check.steps.map((st) =>
        st.id === a.id ? { ...st, status: a.status, result: a.result ?? st.result } : st,
      );
      const done = steps.every((st) => st.status === 'ok' || st.status === 'attention');
      const status = !done ? 'running' : steps.some((st) => st.status === 'attention') ? 'attention' : 'passed';
      return { ...s, check: { status, steps } };
    }
    case 'setReady': {
      if (a.ready && s.check.status !== 'passed') return s;
      if (!a.ready) return { ...s, localReady: false, startsAtMs: null };
      return withCountdown({ ...s, localReady: true }, a.atMs, a.countdownMs);
    }
    case 'enterLive':
      return { ...s, phase: 'live' };
    case 'leave':
      return initialRoom(s.local);
  }
}

/** The countdown starts when the second DJ presses ready, whichever side that is. */
export function roomAfterRemoteReady(s: RoomState, atMs: number, countdownMs: number): RoomState {
  return withCountdown(roomReducer(s, { type: 'remoteReady', ready: true }), atMs, countdownMs);
}

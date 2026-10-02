import type { BoothRole, DjId, SessionState } from './types';

export type SessionAction =
  | { type: 'markReady'; djId: DjId; atMs: number }
  | { type: 'cancelReady'; djId: DjId; atMs: number }
  | { type: 'takeOver'; djId: DjId; atMs: number; durationMs: number }
  | { type: 'completeHandoff'; atMs: number }
  | { type: 'emergencyTakeOver'; djId: DjId; atMs: number }
  | { type: 'remoteReconnecting'; atMs: number }
  | { type: 'remoteReconnected'; atMs: number }
  | { type: 'end'; atMs: number };

/**
 * Pure session state machine. TAKE OVER only changes coordination state and
 * logs events; it never touches anyone's audio. The DJs still do the musical
 * transition on their own mixers.
 */
export function sessionReducer(state: SessionState, action: SessionAction): SessionState {
  if (state.status === 'ended') return state;

  switch (action.type) {
    case 'markReady': {
      if (action.djId === state.ownerId || state.readyIds.includes(action.djId)) return state;
      return {
        ...state,
        readyIds: [...state.readyIds, action.djId],
        events: [...state.events, { type: 'ready', djId: action.djId, atMs: action.atMs }],
      };
    }
    case 'cancelReady': {
      if (!state.readyIds.includes(action.djId)) return state;
      return {
        ...state,
        readyIds: state.readyIds.filter((id) => id !== action.djId),
        events: [...state.events, { type: 'ready_cancelled', djId: action.djId, atMs: action.atMs }],
      };
    }
    case 'takeOver': {
      // Only the DJ who is not on air can take over, and one handoff at a time.
      if (action.djId === state.ownerId || state.handoff) return state;
      return {
        ...state,
        handoff: {
          from: state.ownerId,
          to: action.djId,
          startedAtMs: action.atMs,
          durationMs: action.durationMs,
        },
        events: [...state.events, { type: 'take_over', djId: action.djId, atMs: action.atMs }],
      };
    }
    case 'completeHandoff': {
      if (!state.handoff) return state;
      const { to } = state.handoff;
      return {
        ...state,
        ownerId: to,
        handoff: null,
        readyIds: [],
        events: [...state.events, { type: 'handoff_complete', djId: to, atMs: action.atMs }],
      };
    }
    case 'emergencyTakeOver': {
      // The live DJ's connection is gone, so there is nobody to hand off with:
      // ownership moves at once. Still coordination only; no audio is touched.
      if (action.djId === state.ownerId) return state;
      return {
        ...state,
        ownerId: action.djId,
        handoff: null,
        readyIds: [],
        events: [...state.events, { type: 'emergency_take_over', djId: action.djId, atMs: action.atMs }],
      };
    }
    case 'remoteReconnecting':
      return {
        ...state,
        events: [...state.events, { type: 'remote_reconnecting', djId: state.remoteId, atMs: action.atMs }],
      };
    case 'remoteReconnected':
      return {
        ...state,
        events: [...state.events, { type: 'remote_reconnected', djId: state.remoteId, atMs: action.atMs }],
      };
    case 'end':
      return {
        ...state,
        status: 'ended',
        endedAtMs: action.atMs,
        handoff: null,
        events: [...state.events, { type: 'end', atMs: action.atMs }],
      };
  }
}

export function roleOf(state: SessionState, djId: DjId): BoothRole {
  if (state.handoff && (state.handoff.from === djId || state.handoff.to === djId)) return 'handoff';
  if (state.ownerId === djId) return 'onAir';
  return state.readyIds.includes(djId) ? 'ready' : 'cueing';
}

export function handoffCount(state: SessionState): number {
  return state.events.filter((e) => e.type === 'handoff_complete').length;
}

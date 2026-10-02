import { describe, expect, it } from 'vitest';
import { roleOf, sessionReducer } from './sessionReducer';
import type { SessionState } from './types';

const base = (): SessionState => ({
  status: 'live',
  startedAtMs: 0,
  endedAtMs: null,
  localId: 'a',
  remoteId: 'b',
  djs: {
    a: { id: 'a', name: 'A', city: 'NYC', isLocal: true, track: null },
    b: { id: 'b', name: 'B', city: 'LON', isLocal: false, track: null },
  },
  ownerId: 'a',
  readyIds: [],
  handoff: null,
  events: [],
});

describe('sessionReducer', () => {
  it('starts with the owner on air and the other DJ cueing', () => {
    const s = base();
    expect(roleOf(s, 'a')).toBe('onAir');
    expect(roleOf(s, 'b')).toBe('cueing');
  });

  it('marks the follower ready, but never the owner', () => {
    let s = sessionReducer(base(), { type: 'markReady', djId: 'b', atMs: 1 });
    expect(roleOf(s, 'b')).toBe('ready');
    const same = sessionReducer(s, { type: 'markReady', djId: 'a', atMs: 2 });
    expect(same).toBe(s);
    s = sessionReducer(s, { type: 'cancelReady', djId: 'b', atMs: 3 });
    expect(roleOf(s, 'b')).toBe('cueing');
  });

  it('runs a handoff: HANDOFF for both, then ownership flips and ready clears', () => {
    let s = sessionReducer(base(), { type: 'markReady', djId: 'b', atMs: 1 });
    s = sessionReducer(s, { type: 'takeOver', djId: 'b', atMs: 2, durationMs: 1600 });
    expect(roleOf(s, 'a')).toBe('handoff');
    expect(roleOf(s, 'b')).toBe('handoff');
    expect(s.ownerId).toBe('a');
    s = sessionReducer(s, { type: 'completeHandoff', atMs: 1602 });
    expect(s.ownerId).toBe('b');
    expect(roleOf(s, 'b')).toBe('onAir');
    expect(roleOf(s, 'a')).toBe('cueing');
    expect(s.readyIds).toEqual([]);
    expect(s.events.map((e) => e.type)).toEqual(['ready', 'take_over', 'handoff_complete']);
  });

  it('ignores take over from the owner and during a running handoff', () => {
    const s = base();
    expect(sessionReducer(s, { type: 'takeOver', djId: 'a', atMs: 1, durationMs: 1600 })).toBe(s);
    const running = sessionReducer(s, { type: 'takeOver', djId: 'b', atMs: 1, durationMs: 1600 });
    expect(sessionReducer(running, { type: 'takeOver', djId: 'b', atMs: 2, durationMs: 1600 })).toBe(running);
  });

  it('logs reconnects without changing who owns the mix', () => {
    let s = sessionReducer(base(), { type: 'remoteReconnecting', atMs: 5 });
    s = sessionReducer(s, { type: 'remoteReconnected', atMs: 9 });
    expect(s.ownerId).toBe('a');
    expect(s.events.map((e) => e.type)).toEqual(['remote_reconnecting', 'remote_reconnected']);
  });

  it('ignores everything once ended', () => {
    const ended = sessionReducer(base(), { type: 'end', atMs: 10 });
    expect(ended.status).toBe('ended');
    expect(sessionReducer(ended, { type: 'takeOver', djId: 'b', atMs: 11, durationMs: 1600 })).toBe(ended);
  });

  it('emergency take over moves the mix at once and logs it', () => {
    let s = sessionReducer(base(), { type: 'markReady', djId: 'b', atMs: 1 });
    s = sessionReducer(s, { type: 'emergencyTakeOver', djId: 'b', atMs: 2 });
    expect(s.ownerId).toBe('b');
    expect(s.handoff).toBeNull();
    expect(s.readyIds).toEqual([]);
    expect(roleOf(s, 'b')).toBe('onAir');
    expect(s.events.at(-1)).toEqual({ type: 'emergency_take_over', djId: 'b', atMs: 2 });
    // The owner can't emergency-take their own mix.
    expect(sessionReducer(s, { type: 'emergencyTakeOver', djId: 'b', atMs: 3 })).toBe(s);
  });
});

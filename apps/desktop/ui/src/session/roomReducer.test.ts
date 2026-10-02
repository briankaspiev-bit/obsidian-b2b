import { describe, expect, it } from 'vitest';
import { initialRoom, normalizeCode, roomAfterRemoteReady, roomReducer, type RoomAction } from './roomReducer';
import type { RoomState } from './types';

const run = (s: RoomState, ...actions: RoomAction[]) => actions.reduce(roomReducer, s);
const remote = { name: 'Dana', city: 'London' };
const passAll: RoomAction[] = (['network', 'roundTrip', 'send', 'receive', 'sync'] as const).map((id) => ({
  type: 'checkStep',
  id,
  status: 'ok',
}));

const inBooth = () =>
  run(
    initialRoom({ name: 'Val', city: 'New York' }),
    { type: 'entered', code: 'K7QX-M2PD', isHost: true },
    { type: 'remoteJoined', remote },
    {
      type: 'devicesFound',
      inputs: [
        { id: 'in1', label: 'A', detail: '1-2' },
        { id: 'in2', label: 'B', detail: '1-2' },
      ],
      outputs: [{ id: 'out1', label: 'A', detail: '3-4' }],
    },
  );

describe('normalizeCode', () => {
  it('accepts 8 letters or digits in any case or spacing', () => {
    expect(normalizeCode('k7qx m2pd')).toBe('K7QX-M2PD');
    expect(normalizeCode('K7QX-M2PD')).toBe('K7QX-M2PD');
  });
  it('rejects anything else', () => {
    expect(normalizeCode('K7QX-M2P')).toBeNull();
    expect(normalizeCode('')).toBeNull();
  });
});

describe('roomReducer', () => {
  it('picks the first devices by default', () => {
    const s = inBooth();
    expect(s.inputId).toBe('in1');
    expect(s.outputId).toBe('out1');
  });

  it('only lets you press ready once the check passed', () => {
    let s = run(inBooth(), { type: 'setReady', ready: true, atMs: 0, countdownMs: 3000 });
    expect(s.localReady).toBe(false);
    s = run(s, { type: 'checkStarted' }, ...passAll, { type: 'setReady', ready: true, atMs: 0, countdownMs: 3000 });
    expect(s.check.status).toBe('passed');
    expect(s.localReady).toBe(true);
    expect(s.startsAtMs).toBeNull(); // the other DJ isn't ready yet
  });

  it('starts the countdown when both are ready, from either side', () => {
    let s = run(inBooth(), { type: 'checkStarted' }, ...passAll, { type: 'setReady', ready: true, atMs: 100, countdownMs: 3000 });
    s = roomAfterRemoteReady(s, 500, 3000);
    expect(s.startsAtMs).toBe(3500);

    let t = run(inBooth(), { type: 'remoteReady', ready: true }, { type: 'checkStarted' }, ...passAll);
    t = run(t, { type: 'setReady', ready: true, atMs: 200, countdownMs: 3000 });
    expect(t.startsAtMs).toBe(3200);
  });

  it('changing a device resets the check and readiness', () => {
    let s = run(inBooth(), { type: 'checkStarted' }, ...passAll, { type: 'setReady', ready: true, atMs: 0, countdownMs: 3000 });
    s = run(s, { type: 'selectInput', id: 'in2' });
    expect(s.check.status).toBe('idle');
    expect(s.localReady).toBe(false);
  });

  it('a check with a warning does not pass', () => {
    const s = run(inBooth(), { type: 'checkStarted' }, ...passAll.slice(0, 4), {
      type: 'checkStep',
      id: 'sync',
      status: 'attention',
      result: 'Drifting',
    });
    expect(s.check.status).toBe('attention');
  });
});

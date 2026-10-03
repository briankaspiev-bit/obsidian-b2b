import { describe, expect, it } from 'vitest';
import type { NetworkResult } from './bridge';
import { evaluateCheck, networkQuality, parsePeerMessage } from './peer';

const good: NetworkResult = {
  pingsSent: 40,
  pongsReceived: 40,
  lossPct: 0,
  rttMs: 38,
  rttMinMs: 35,
  jitterMs: 3,
  clockOffsetMs: 12,
  reached: true,
};
const opts = { relay: false, localPeakDb: -10, remotePeakDb: -12, remoteName: 'Dana', inputLabel: 'UMC204HD' };

describe('parsePeerMessage', () => {
  it('accepts known messages and trims names', () => {
    expect(parsePeerMessage('{"t":"takeOver"}')).toEqual({ t: 'takeOver' });
    expect(parsePeerMessage(JSON.stringify({ t: 'hello', name: 'x'.repeat(100), city: 'London' }))).toEqual({
      t: 'hello',
      name: 'x'.repeat(64),
      city: 'London',
    });
    expect(parsePeerMessage('{"t":"boothReady","ready":"yes"}')).toEqual({ t: 'boothReady', ready: false });
  });
  it('drops anything else', () => {
    expect(parsePeerMessage('not json')).toBeNull();
    expect(parsePeerMessage('{"t":"format the disk"}')).toBeNull();
    expect(parsePeerMessage('{"t":"hello"}')).toBeNull();
    expect(parsePeerMessage('null')).toBeNull();
  });
});

describe('evaluateCheck', () => {
  it('passes a good line with both mixers playing', () => {
    const steps = evaluateCheck(good, opts);
    expect(steps.map((s) => s.status)).toEqual(['ok', 'ok', 'ok', 'ok', 'ok']);
    expect(steps[1].result).toBe('38 ms round trip, 0.0% lost');
  });
  it('flags a lossy line, silent mixers and no answer', () => {
    const lossy = evaluateCheck({ ...good, lossPct: 4 }, { ...opts, remotePeakDb: -Infinity, localPeakDb: -70 });
    expect(lossy.find((s) => s.id === 'roundTrip')?.status).toBe('attention');
    expect(lossy.find((s) => s.id === 'send')?.status).toBe('attention');
    expect(lossy.find((s) => s.id === 'receive')?.result).toContain('Dana');
    const none = evaluateCheck({ ...good, reached: false, clockOffsetMs: null }, opts);
    expect(none[0]).toMatchObject({ id: 'network', status: 'attention' });
  });
  it('calls fast but uneven Wi-Fi a warning, and points testers without a mixer to Test music', () => {
    const wifi = evaluateCheck({ ...good, rttMs: 20, jitterMs: 45 }, { ...opts, localPeakDb: -70 });
    expect(wifi[1]).toMatchObject({ status: 'attention', result: '20 ms, 0.0% lost. Wi-Fi is uneven, Ethernet is better' });
    expect(wifi[2].result).toBe('No signal. No mixer? Pick Test music');
    const slow = evaluateCheck({ ...good, rttMs: 220, jitterMs: 45 }, opts);
    expect(slow[1].result).toContain('Try Ethernet');
  });
});

describe('networkQuality', () => {
  it('grades by loss, jitter and round trip', () => {
    expect(networkQuality({ rttMs: 40, jitterMs: 2, lossPct: 0 })).toBe('excellent');
    expect(networkQuality({ rttMs: 140, jitterMs: 2, lossPct: 0 })).toBe('good');
    expect(networkQuality({ rttMs: 40, jitterMs: 25, lossPct: 0 })).toBe('fair');
    expect(networkQuality({ rttMs: 40, jitterMs: 2, lossPct: 8 })).toBe('poor');
  });
});

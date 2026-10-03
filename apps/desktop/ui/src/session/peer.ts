// What the two apps say to each other over the booth link, and pure helpers
// that turn real measurements into UI state. Kept free of Tauri so it is
// unit-tested (peer.test.ts).

import type { LinkStatus, LiveStatus, NetworkResult } from './bridge';
import { MAX_PHOTO_CHUNKS, PHOTO_CHUNK, type PhotoChunk } from '../lib/photo';
import type { CheckStepId, CheckStepStatus, Diagnostics, LinkState, NetworkQuality } from './types';

/** Coordination messages. Both apps run the same reducers and apply each other's moves. */
export type PeerMessage =
  | { t: 'hello'; name: string; city: string }
  | { t: 'boothReady'; ready: boolean }
  | { t: 'markReady' }
  | { t: 'cancelReady' }
  | { t: 'takeOver' }
  | { t: 'emergencyTakeOver' }
  | { t: 'end' }
  | { t: 'leave' }
  | PhotoChunk;

const KINDS = new Set(['hello', 'boothReady', 'markReady', 'cancelReady', 'takeOver', 'emergencyTakeOver', 'end', 'leave', 'photo']);

/** Parses a message from the other app; anything unknown or malformed is dropped. */
export function parsePeerMessage(json: string): PeerMessage | null {
  try {
    const m = JSON.parse(json) as { t?: unknown };
    if (!m || typeof m !== 'object' || typeof m.t !== 'string' || !KINDS.has(m.t)) return null;
    if (m.t === 'hello') {
      const h = m as { name?: unknown; city?: unknown };
      if (typeof h.name !== 'string') return null;
      return { t: 'hello', name: h.name.slice(0, 64), city: typeof h.city === 'string' ? h.city.slice(0, 64) : '' };
    }
    if (m.t === 'photo') {
      const p = m as Partial<PhotoChunk>;
      const ok =
        typeof p.id === 'string' &&
        p.id.length <= 24 &&
        Number.isInteger(p.n) &&
        Number.isInteger(p.i) &&
        p.n! >= 1 &&
        p.n! <= MAX_PHOTO_CHUNKS &&
        p.i! >= 0 &&
        p.i! < p.n! &&
        typeof p.d === 'string' &&
        p.d.length <= PHOTO_CHUNK;
      return ok ? { t: 'photo', id: p.id!, i: p.i!, n: p.n!, d: p.d! } : null;
    }
    if (m.t === 'boothReady') return { t: 'boothReady', ready: (m as { ready?: unknown }).ready === true };
    return m as PeerMessage;
  } catch {
    return null;
  }
}

/** Levels quieter than this count as "nothing playing". */
export const SIGNAL_FLOOR_DB = -50;

export interface CheckOutcome {
  id: CheckStepId;
  status: CheckStepStatus;
  result: string;
}

const ms = (v: number) => `${Math.round(v)} ms`;

/** Booth Check's five steps from a real network test and the meters seen meanwhile. */
export function evaluateCheck(
  net: NetworkResult,
  opts: { relay: boolean; localPeakDb: number; remotePeakDb: number; remoteName: string; inputLabel: string },
): CheckOutcome[] {
  const { relay, localPeakDb, remotePeakDb, remoteName, inputLabel } = opts;
  if (!net.reached) {
    return [
      { id: 'network', status: 'attention', result: `Can't reach ${remoteName}` },
      { id: 'roundTrip', status: 'attention', result: 'Not measured' },
      sendStep(localPeakDb, inputLabel),
      { id: 'receive', status: 'attention', result: 'Not measured' },
      { id: 'sync', status: 'attention', result: 'Not measured' },
    ];
  }
  const slow = net.rttMs > 150 || net.lossPct > 2;
  // Fast and clean but uneven timing: typical Wi-Fi. A warning, not a blocker.
  const uneven = !slow && net.jitterMs > 30;
  return [
    {
      id: 'network',
      status: 'ok',
      result: relay ? 'Connected through our relay' : 'Connected directly',
    },
    {
      id: 'roundTrip',
      status: slow || uneven ? 'attention' : 'ok',
      result: slow
        ? `${ms(net.rttMs)}, ${net.lossPct.toFixed(1)}% lost. Rough line, Ethernet helps`
        : uneven
          ? `${ms(net.rttMs)}, ${net.lossPct.toFixed(1)}% lost. Wi-Fi is uneven, Ethernet helps`
          : `${ms(net.rttMs)} round trip, ${net.lossPct.toFixed(1)}% lost`,
    },
    sendStep(localPeakDb, inputLabel),
    remotePeakDb > SIGNAL_FLOOR_DB
      ? { id: 'receive', status: 'ok', result: `${remoteName}'s mixer is coming through` }
      : { id: 'receive', status: 'attention', result: `Nothing from ${remoteName} yet` },
    net.clockOffsetMs !== null
      ? { id: 'sync', status: 'ok', result: `Clocks matched to within ${ms(Math.max(1, net.rttMinMs / 2))}` }
      : { id: 'sync', status: 'attention', result: 'Could not match clocks' },
  ];
}

function sendStep(localPeakDb: number, inputLabel: string): CheckOutcome {
  return localPeakDb > SIGNAL_FLOOR_DB
    ? { id: 'send', status: 'ok', result: `Signal from ${inputLabel}` }
    : { id: 'send', status: 'attention', result: 'No signal. No mixer? Pick Test music' };
}

export function networkQuality(s: Pick<LinkStatus, 'rttMs' | 'jitterMs' | 'lossPct'>): NetworkQuality {
  if (s.lossPct > 5 || s.jitterMs > 40) return 'poor';
  if (s.lossPct > 1 || s.jitterMs > 20 || s.rttMs > 200) return 'fair';
  if (s.lossPct > 0.2 || s.jitterMs > 8 || s.rttMs > 120) return 'good';
  return 'excellent';
}

/** The Live Session's link panel from the booth link's live numbers. */
export function linkStateFrom(s: LinkStatus, prev: LinkState): LinkState {
  const remote = s.state === 'connected' ? 'connected' : s.state === 'left' ? 'left' : 'reconnecting';
  const diagnostics: Diagnostics = {
    ...prev.diagnostics,
    roundTripMs: Math.round(s.rttMs),
    oneWayMs: Math.round(s.rttMs / 2),
    jitterMs: +s.jitterMs.toFixed(1),
    packetLossPct: +s.lossPct.toFixed(2),
    path: s.relay ? 'relay' : 'direct',
  };
  return {
    ...prev,
    remote,
    network: remote === 'connected' ? networkQuality(s) : 'recovering',
    boothSync: remote === 'connected' ? 'stable' : 'lost',
    diagnostics,
  };
}

/** Before the first measurement. Recording stays off until the engine's live mode exists. */
export function initialRealLink(): LinkState {
  return {
    remote: 'connected',
    boothSync: 'stable',
    network: 'good',
    recording: 'off',
    diagnostics: {
      roundTripMs: 0,
      oneWayMs: 0,
      jitterMs: 0,
      packetLossPct: 0,
      bufferMs: 0,
      clockDriftPpm: 0,
      path: 'direct',
      codec: 'No audio yet',
    },
  };
}

/** Linear peak → dBFS. */
export function peakToDb(peak: number): number {
  return peak > 1e-6 ? 20 * Math.log10(peak) : -Infinity;
}

/** 5 ms frames: the engine's "last 10 s" counters are out of 2000. */
const FRAMES_PER_10S = 2000;

/** The Live Session's link panel from the engine's live status. */
export function linkStateFromLive(st: LiveStatus, prev: LinkState): LinkState {
  const remote = st.phase === 'partner left' ? 'left' : st.phase === 'partner not reachable' ? 'reconnecting' : 'connected';
  const settling = st.phase !== 'live';
  const concealedPct = (100 * st.concealed_10s) / FRAMES_PER_10S;
  const rtt = st.rtt_ms ?? prev.diagnostics.roundTripMs;
  const network: NetworkQuality =
    remote !== 'connected' || settling
      ? 'recovering'
      : concealedPct > 2
        ? 'poor'
        : concealedPct > 0.5
          ? 'fair'
          : st.recovered_10s > 20
            ? 'good'
            : 'excellent';
  return {
    remote,
    network,
    boothSync: remote === 'connected' ? (settling ? 'adjusting' : 'stable') : 'lost',
    recording: settling && remote === 'connected' ? prev.recording : 'on',
    diagnostics: {
      ...prev.diagnostics,
      roundTripMs: Math.round(rtt),
      oneWayMs: Math.round(st.booth_delay_ms ?? rtt / 2),
      packetLossPct: +concealedPct.toFixed(2),
      bufferMs: Math.round(st.margin_ms ?? 0),
      codec: 'Opus 256 kbps',
    },
  };
}

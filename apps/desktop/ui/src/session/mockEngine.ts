// MOCK ONLY. Stands in for the Rust session engine until it exists.
// Everything here is fake: tracks, levels, network health and diagnostics.
// Replace with an adapter over the engine's IPC API; the UI only depends on
// the SessionEngine interface.

import { HANDOFF_DURATION_MS, START_COUNTDOWN_MS, type SessionEngine } from './engine';
import { initialRoom, normalizeCode, roomAfterRemoteReady, roomReducer, type RoomAction } from './roomReducer';
import { sessionReducer, type SessionAction } from './sessionReducer';
import type { AudioDevice, CheckStepId, LinkState, RoomState, SessionState, StereoLevel, TrackInfo } from './types';
// Placeholder photos for the mock DJs.
import valPhoto from '../assets/mock/val.jpg';
import danaPhoto from '../assets/mock/dana.jpg';

const LOCAL_ID = 'dj-brian';
const REMOTE_ID = 'dj-maya';

const CRATES: Record<string, Omit<TrackInfo, 'remainingSec'>[]> = {
  [LOCAL_ID]: [
    { title: 'Higher Ground', artist: 'Keinemusik' },
    { title: 'Move', artist: 'Adam Port' },
    { title: 'Rumble', artist: 'Fred again..' },
  ],
  [REMOTE_ID]: [
    { title: 'Find A Way', artist: 'Mall Grab' },
    { title: 'Spacetime', artist: 'Ross From Friends' },
    { title: 'Pressure', artist: 'Overmono' },
  ],
};

const SILENT: StereoLevel = { left: -Infinity, right: -Infinity };
const BPM = 124;

type Listener = () => void;

function createListeners() {
  const set = new Set<Listener>();
  return {
    add(l: Listener) {
      set.add(l);
      return () => {
        set.delete(l);
      };
    },
    emit() {
      set.forEach((l) => l());
    },
  };
}

// What a Windows booth with a USB interface might list.
const INPUTS: AudioDevice[] = [
  { id: 'umc-in-12', label: 'Behringer UMC204HD', detail: 'Input 1-2' },
  { id: 'scarlett-in-12', label: 'Focusrite Scarlett 2i2', detail: 'Input 1-2' },
  { id: 'flx4-master', label: 'DDJ-FLX4', detail: 'Master out (USB)' },
];
const OUTPUTS: AudioDevice[] = [
  { id: 'umc-out-34', label: 'Behringer UMC204HD', detail: 'Output 3-4' },
  { id: 'scarlett-out-12', label: 'Focusrite Scarlett 2i2', detail: 'Output 1-2' },
  { id: 'realtek', label: 'Speakers (Realtek)', detail: 'Built-in' },
];

const CHECK_RESULTS: [CheckStepId, string][] = [
  ['network', 'Direct connection, excellent'],
  ['roundTrip', '74 ms'],
  ['send', 'Clean signal from your mixer'],
  ['receive', 'Test tone arrived clearly'],
  ['sync', 'Locked'],
];

const LOCAL_PROFILE = { name: 'Val', city: 'New York', photoUrl: valPhoto };
const REMOTE_PROFILE = { name: 'Dana', city: 'London', photoUrl: danaPhoto };

/** A session. `midSet` opens it 42 minutes in, as the reference design does. */
function initialSession(now: number, opts: { midSet?: boolean; localOnAir?: boolean } = {}): SessionState {
  const { midSet = true, localOnAir = true } = opts;
  return {
    status: 'live',
    startedAtMs: midSet ? now - (42 * 60 + 19) * 1000 : now,
    endedAtMs: null,
    localId: LOCAL_ID,
    remoteId: REMOTE_ID,
    djs: {
      [LOCAL_ID]: {
        id: LOCAL_ID,
        name: 'Val',
        photoUrl: valPhoto,
        city: 'New York',
        isLocal: true,
        track: { ...CRATES[LOCAL_ID][0], remainingSec: 252 },
      },
      [REMOTE_ID]: {
        id: REMOTE_ID,
        name: 'Dana',
        photoUrl: danaPhoto,
        city: 'London',
        isLocal: false,
        track: { ...CRATES[REMOTE_ID][0], remainingSec: 388 },
      },
    },
    ownerId: localOnAir ? LOCAL_ID : REMOTE_ID,
    readyIds: [],
    handoff: null,
    events: [{ type: 'start', atMs: now }],
  };
}

function initialLink(): LinkState {
  return {
    remote: 'connected',
    boothSync: 'stable',
    network: 'excellent',
    recording: 'on',
    diagnostics: {
      roundTripMs: 74,
      oneWayMs: 61,
      jitterMs: 2.1,
      packetLossPct: 0.02,
      bufferMs: 24,
      clockDriftPpm: 18,
      path: 'direct',
      codec: 'Opus 256 kbps',
    },
  };
}

export class MockSessionEngine implements SessionEngine {
  private session: SessionState;
  private room: RoomState = initialRoom(LOCAL_PROFILE);
  private roomListeners = createListeners();
  private roomTimers: number[] = [];
  private link: LinkState = initialLink();
  private level: StereoLevel = SILENT;
  private localLevel: StereoLevel = SILENT;
  private trackIndex: Record<string, number> = { [LOCAL_ID]: 0, [REMOTE_ID]: 0 };

  private sessionListeners = createListeners();
  private linkListeners = createListeners();
  private levelListeners = createListeners();
  private localLevelListeners = createListeners();

  private timers: number[] = [];
  private handoffTimer: number | undefined;
  private dropoutTimers: number[] = [];

  constructor() {
    this.session = initialSession(Date.now());
    this.timers.push(window.setInterval(() => this.tickSecond(), 1000));
    this.timers.push(window.setInterval(() => this.tickLevel(), 50));
  }

  // --- SessionEngine: before the session ------------------------------------

  getRoom = () => this.room;
  subscribeRoom = (l: Listener) => this.roomListeners.add(l);

  createRoom = () => {
    if (this.room.phase !== 'home') return;
    this.enterBooth(this.makeCode(), true);
    // Pretend the other DJ opens the invite a few seconds later.
    this.later(3200, () => this.remoteJoins());
  };

  joinRoom = (input: string) => {
    if (this.room.phase !== 'home' || this.room.joining) return;
    const code = normalizeCode(input);
    if (!code) {
      this.roomDispatch({ type: 'joinFailed', error: 'That code needs 8 letters or numbers. Check the message you were sent.' });
      return;
    }
    this.roomDispatch({ type: 'joinStarted' });
    this.later(900, () => {
      if (code === '0000-0000') {
        this.roomDispatch({ type: 'joinFailed', error: 'No room with that code. It may have closed. Ask for a new one.' });
        return;
      }
      this.enterBooth(code, false);
      this.remoteJoins(); // The host is already there.
    });
  };

  selectInput = (id: string) => this.roomDispatch({ type: 'selectInput', id });
  selectOutput = (id: string) => this.roomDispatch({ type: 'selectOutput', id });

  runBoothCheck = () => {
    const r = this.room;
    if (r.phase !== 'booth' || r.remotePresence === 'waiting' || r.check.status === 'running') return;
    this.roomDispatch({ type: 'checkStarted' });
    CHECK_RESULTS.forEach(([id, result], i) => {
      this.later(i * 750 + 150, () => this.roomDispatch({ type: 'checkStep', id, status: 'running' }));
      this.later(i * 750 + 800, () => {
        this.roomDispatch({ type: 'checkStep', id, status: 'ok', result });
        if (i === CHECK_RESULTS.length - 1) this.afterCheckPassed();
      });
    });
  };

  setBoothReady = (ready: boolean) => {
    this.roomDispatch({ type: 'setReady', ready, atMs: Date.now(), countdownMs: START_COUNTDOWN_MS });
    this.scheduleStart();
  };

  leaveRoom = () => {
    this.clearRoomTimers();
    window.clearTimeout(this.handoffTimer);
    this.dropoutTimers.forEach((t) => window.clearTimeout(t));
    this.link = initialLink();
    this.linkListeners.emit();
    this.roomDispatch({ type: 'leave' });
  };

  // --- SessionEngine: during the session ------------------------------------

  getSession = () => this.session;
  subscribeSession = (l: Listener) => this.sessionListeners.add(l);
  getLink = () => this.link;
  subscribeLink = (l: Listener) => this.linkListeners.add(l);
  getRemoteLevel = () => this.level;
  subscribeRemoteLevel = (l: Listener) => this.levelListeners.add(l);
  getLocalLevel = () => this.localLevel;
  subscribeLocalLevel = (l: Listener) => this.localLevelListeners.add(l);

  markReady = () => this.dispatch({ type: 'markReady', djId: LOCAL_ID, atMs: Date.now() });
  cancelReady = () => this.dispatch({ type: 'cancelReady', djId: LOCAL_ID, atMs: Date.now() });
  takeOver = () => this.beginTakeOver(LOCAL_ID);
  emergencyTakeOver = () => {
    window.clearTimeout(this.handoffTimer);
    this.dispatch({ type: 'emergencyTakeOver', djId: LOCAL_ID, atMs: Date.now() });
  };
  endSession = () => {
    window.clearTimeout(this.handoffTimer);
    this.dispatch({ type: 'end', atMs: Date.now() });
  };

  // --- Mock-only controls (the remote DJ's side and failures) -------------

  remoteMarkReady = () => this.dispatch({ type: 'markReady', djId: REMOTE_ID, atMs: Date.now() });
  remoteCancelReady = () => this.dispatch({ type: 'cancelReady', djId: REMOTE_ID, atMs: Date.now() });
  remoteTakeOver = () => this.beginTakeOver(REMOTE_ID);

  simulateDropout = (seconds = 6) => {
    if (this.link.remote !== 'connected' || this.session.status !== 'live') return;
    this.dropoutTimers.forEach((t) => window.clearTimeout(t));
    this.dispatch({ type: 'remoteReconnecting', atMs: Date.now() });
    this.setLink({
      remote: 'reconnecting',
      network: 'recovering',
      boothSync: 'adjusting',
      diagnostics: { ...this.link.diagnostics, packetLossPct: 100, jitterMs: 0, path: 'relay' },
    });
    this.dropoutTimers = [
      window.setTimeout(() => {
        this.dispatch({ type: 'remoteReconnected', atMs: Date.now() });
        this.setLink({
          remote: 'connected',
          network: 'good',
          boothSync: 'stable',
          diagnostics: { ...this.link.diagnostics, packetLossPct: 0.4, jitterMs: 9.5, roundTripMs: 88 },
        });
      }, seconds * 1000),
      window.setTimeout(() => {
        this.setLink({
          network: 'excellent',
          diagnostics: { ...this.link.diagnostics, packetLossPct: 0.02, jitterMs: 2.4, roundTripMs: 75, path: 'direct' },
        });
      }, seconds * 1000 + 5000),
    ];
  };

  /** Mock only: Dana presses ready in the booth check. */
  remoteBoothReady = () => {
    if (this.room.phase !== 'booth' || this.room.remotePresence !== 'joined') return;
    this.room = roomAfterRemoteReady(this.room, Date.now(), START_COUNTDOWN_MS);
    this.roomListeners.emit();
    this.scheduleStart();
  };

  /** Mock only: Dana arrives in a room that is waiting. */
  remoteJoinNow = () => this.remoteJoins();

  /** Mock only: jump straight to the Live Session mid-set, as the old preview opened. */
  skipToLive = () => {
    this.clearRoomTimers();
    this.room = {
      ...initialRoom(LOCAL_PROFILE),
      phase: 'live',
      code: 'K7QX-M2PD',
      isHost: true,
      remote: REMOTE_PROFILE,
      remotePresence: 'ready',
    };
    this.roomListeners.emit();
    this.restart();
  };

  restart = () => {
    window.clearTimeout(this.handoffTimer);
    this.dropoutTimers.forEach((t) => window.clearTimeout(t));
    this.trackIndex = { [LOCAL_ID]: 0, [REMOTE_ID]: 0 };
    this.session = initialSession(Date.now());
    this.link = initialLink();
    this.sessionListeners.emit();
    this.linkListeners.emit();
  };

  dispose() {
    this.clearRoomTimers();
    this.timers.forEach((t) => window.clearInterval(t));
    window.clearTimeout(this.handoffTimer);
    this.dropoutTimers.forEach((t) => window.clearTimeout(t));
  }

  // --- internals -----------------------------------------------------------

  private dispatch(action: SessionAction) {
    const next = sessionReducer(this.session, action);
    if (next !== this.session) {
      this.session = next;
      this.sessionListeners.emit();
    }
  }

  private roomDispatch(action: RoomAction) {
    const next = roomReducer(this.room, action);
    if (next !== this.room) {
      this.room = next;
      this.roomListeners.emit();
    }
  }

  private later(ms: number, fn: () => void) {
    this.roomTimers.push(window.setTimeout(fn, ms));
  }

  private clearRoomTimers() {
    this.roomTimers.forEach((t) => window.clearTimeout(t));
    this.roomTimers = [];
  }

  private makeCode() {
    const chars = 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789'; // no 0/O or 1/I to misread
    let raw = '';
    for (let i = 0; i < 8; i++) raw += chars[Math.floor(Math.random() * chars.length)];
    return `${raw.slice(0, 4)}-${raw.slice(4)}`;
  }

  private enterBooth(code: string, isHost: boolean) {
    this.roomDispatch({ type: 'entered', code, isHost });
    this.roomDispatch({ type: 'devicesFound', inputs: INPUTS, outputs: OUTPUTS });
  }

  private remoteJoins() {
    if (this.room.phase !== 'booth' || this.room.remotePresence !== 'waiting') return;
    this.roomDispatch({ type: 'remoteJoined', remote: REMOTE_PROFILE });
  }

  /** The other DJ runs their own check and presses ready a little later. */
  private afterCheckPassed() {
    if (this.room.remotePresence === 'joined') this.later(2600, () => this.remoteBoothReady());
  }

  private scheduleStart() {
    const at = this.room.startsAtMs;
    if (at === null) return;
    this.later(Math.max(0, at - Date.now()), () => {
      if (this.room.startsAtMs !== at || this.room.phase !== 'booth') return;
      // The host opens on air; the other DJ cues first.
      this.session = initialSession(Date.now(), { midSet: false, localOnAir: this.room.isHost });
      this.trackIndex = { [LOCAL_ID]: 0, [REMOTE_ID]: 0 };
      this.link = initialLink();
      this.sessionListeners.emit();
      this.linkListeners.emit();
      this.roomDispatch({ type: 'enterLive' });
    });
  }

  private setLink(patch: Partial<LinkState>) {
    this.link = { ...this.link, ...patch };
    this.linkListeners.emit();
  }

  private beginTakeOver(djId: string) {
    const before = this.session;
    this.dispatch({ type: 'takeOver', djId, atMs: Date.now(), durationMs: HANDOFF_DURATION_MS });
    if (this.session === before) return;
    this.handoffTimer = window.setTimeout(
      () => this.dispatch({ type: 'completeHandoff', atMs: Date.now() }),
      HANDOFF_DURATION_MS,
    );
  }

  private tickSecond() {
    if (this.session.status !== 'live') return;
    const djs = { ...this.session.djs };
    for (const id of Object.keys(djs)) {
      const track = djs[id].track;
      if (!track || track.remainingSec === null) continue;
      if (track.remainingSec > 1) {
        djs[id] = { ...djs[id], track: { ...track, remainingSec: track.remainingSec - 1 } };
      } else {
        const crate = CRATES[id];
        this.trackIndex[id] = (this.trackIndex[id] + 1) % crate.length;
        djs[id] = { ...djs[id], track: { ...crate[this.trackIndex[id]], remainingSec: 300 + Math.round(Math.random() * 120) } };
      }
    }
    this.session = { ...this.session, djs };
    this.sessionListeners.emit();

    if (this.link.remote === 'connected') {
      const d = this.link.diagnostics;
      const wobble = (v: number, amt: number, min: number) => Math.max(min, v + (Math.random() - 0.5) * amt);
      this.setLink({
        diagnostics: {
          ...d,
          roundTripMs: Math.round(wobble(d.roundTripMs, 2, 70)),
          jitterMs: +wobble(d.jitterMs, 0.4, 1.2).toFixed(1),
          clockDriftPpm: Math.round(wobble(d.clockDriftPpm, 1, 10)),
        },
      });
    }
  }

  /** Fake program levels for both DJs, shaped by their roles. */
  private tickLevel() {
    if (this.room.phase !== 'live') return this.tickBoothLevel();
    const remoteGain = this.link.remote === 'connected' ? this.programGain(this.session.remoteId) : 0;
    const nextRemote = remoteGain <= 0.01 ? SILENT : this.musicalLevel(remoteGain);
    if (nextRemote.left !== this.level.left || nextRemote.right !== this.level.right) {
      this.level = nextRemote;
      this.levelListeners.emit();
    }
    // The local send never depends on the network: it is the mixer's output.
    const localGain = this.programGain(this.session.localId);
    const nextLocal = localGain <= 0.01 ? SILENT : this.musicalLevel(localGain);
    if (nextLocal.left !== this.localLevel.left || nextLocal.right !== this.localLevel.right) {
      this.localLevel = nextLocal;
      this.localLevelListeners.emit();
    }
  }

  /** In the booth check: your mixer is playing; the other DJ's test tone arrives during that step. */
  private tickBoothLevel() {
    const r = this.room;
    const local = r.phase === 'booth' && r.inputId ? this.musicalLevel(0.7) : SILENT;
    this.localLevel = local;
    this.localLevelListeners.emit();
    const receive = r.check.steps.find((s) => s.id === 'receive');
    const tone = receive?.status === 'running' ? { left: -18, right: -18 } : SILENT;
    if (tone.left !== this.level.left) {
      this.level = tone;
      this.levelListeners.emit();
    }
  }

  /** 0..1: how far up this DJ's fader is, in the mock's model of a B2B. */
  private programGain(djId: string): number {
    const s = this.session;
    if (s.status !== 'live') return 0;
    if (s.handoff) {
      const p = Math.min(1, (Date.now() - s.handoff.startedAtMs) / s.handoff.durationMs);
      if (s.handoff.to === djId) return p;
      if (s.handoff.from === djId) return 1 - p;
    }
    return s.ownerId === djId ? 1 : 0;
  }

  private musicalLevel(gain: number): StereoLevel {
    const beat = (Date.now() / (60000 / BPM)) % 1;
    const kick = Math.exp(-beat * 6); // decays after each beat
    const body = -14 + kick * 8 + (Math.random() - 0.5) * 2;
    const g = 20 * Math.log10(gain);
    return { left: body + g, right: body + g - 0.6 + (Math.random() - 0.5) };
  }
}

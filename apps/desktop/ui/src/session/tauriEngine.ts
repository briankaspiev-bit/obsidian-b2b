// The real SessionEngine for the desktop app. Talks to the Rust side through
// bridge.ts; the two apps coordinate over the booth link with PeerMessage.
//
// Both apps run the same pure reducers (roomReducer, sessionReducer). A move
// made here is applied locally and sent to the other app, which applies it as
// the remote DJ's move. What is real: devices and your input meter, room
// codes, the connection and its numbers, both mixers' levels, and the whole
// ready / TAKE OVER / handoff flow. Not yet: audio between the booths and
// the recording, which wait for the engine's live mode.

import { bridge as tauriBridge, type Bridge, type EngineInfo, type LinkStatus, type LiveStatus, type Unlisten, type WireLevel } from './bridge';
import { DeckFeed, type DeckAction, type DeckInfo } from './deck';
import { HANDOFF_DURATION_MS, START_COUNTDOWN_MS, type SessionEngine } from './engine';
import { createListeners, type Listener } from './listeners';
import { evaluateCheck, initialRealLink, linkStateFrom, linkStateFromLive, parsePeerMessage, peakToDb, type PeerMessage } from './peer';
import { initialRoom, normalizeCode, roomAfterRemoteReady, roomReducer, type RoomAction } from './roomReducer';
import { sessionReducer, type SessionAction } from './sessionReducer';
import type { LinkState, RoomState, SessionState, StereoLevel } from './types';

const LOCAL_ID = 'local';
const REMOTE_ID = 'remote';
const SILENT: StereoLevel = { left: -Infinity, right: -Infinity };
const PROFILE_KEY = 'obsidian.profile';
/** The simulated DJ's city in practice (the engine's ghost runs over an NYC to London link). */
const GHOST_CITY = 'London (simulated)';
/** Long enough to see a few hundred pings and some music on both meters. */
const CHECK_SECONDS = 4;

function loadProfile(): RoomState['local'] {
  try {
    const p = JSON.parse(localStorage.getItem(PROFILE_KEY) ?? 'null') as { name?: string; city?: string } | null;
    return { name: p?.name ?? '', city: p?.city ?? '' };
  } catch {
    return { name: '', city: '' };
  }
}

const DEVICES_KEY = 'obsidian.devices';

/** The input and output this DJ picked last time, so tomorrow's setup is one click. */
function loadDevicePicks(): { inputId?: string; outputId?: string } {
  try {
    return (JSON.parse(localStorage.getItem(DEVICES_KEY) ?? 'null') as { inputId?: string; outputId?: string } | null) ?? {};
  } catch {
    return {};
  }
}

function saveDevicePick(key: 'inputId' | 'outputId', id: string) {
  try {
    localStorage.setItem(DEVICES_KEY, JSON.stringify({ ...loadDevicePicks(), [key]: id }));
  } catch {
    // Not fatal: the pick just isn't remembered next time.
  }
}

function saveProfile(p: { name: string; city: string }) {
  try {
    localStorage.setItem(PROFILE_KEY, JSON.stringify(p));
  } catch {
    // Not fatal: the name just isn't remembered next time.
  }
}

function fromWire(l: WireLevel): StereoLevel {
  return { left: l.left ?? -Infinity, right: l.right ?? -Infinity };
}

function messageOf(e: unknown): string {
  return typeof e === 'string' ? e : e instanceof Error ? e.message : 'Something went wrong.';
}

function deckInfo(st: LiveStatus): DeckInfo {
  return {
    nowMs: st.now_ms,
    you: st.you_beat,
    partner: st.partner_beat,
    partnerBpm: st.partner_bpm,
    deck: st.deck,
    onAir: st.on_air,
    partnerOnAir: st.partner_on_air,
    fader: st.fader,
    partnerVolume: st.partner_volume,
    partnerFader: st.partner_fader,
    ghostSays: st.ghost_says,
    shield: st.shield ?? false,
    partnerShield: st.partner_shield ?? false,
    partnerPeak: st.partner_peak,
  };
}

function emptySession(now: number): SessionState {
  return {
    status: 'live',
    startedAtMs: now,
    endedAtMs: null,
    localId: LOCAL_ID,
    remoteId: REMOTE_ID,
    djs: {
      [LOCAL_ID]: { id: LOCAL_ID, name: '', city: '', isLocal: true, track: null },
      [REMOTE_ID]: { id: REMOTE_ID, name: '', city: '', isLocal: false, track: null },
    },
    ownerId: LOCAL_ID,
    readyIds: [],
    handoff: null,
    events: [{ type: 'start', atMs: now }],
  };
}

export class TauriSessionEngine implements SessionEngine {
  private room: RoomState = initialRoom(loadProfile());
  private session: SessionState = emptySession(Date.now());
  private link: LinkState = initialRealLink();
  private remoteLevel: StereoLevel = SILENT;
  private localLevel: StereoLevel = SILENT;
  private relay = false;

  private roomListeners = createListeners();
  private sessionListeners = createListeners();
  private linkListeners = createListeners();
  private remoteLevelListeners = createListeners();
  private localLevelListeners = createListeners();

  private unlisten: Promise<Unlisten>[] = [];
  private timers: ReturnType<typeof setTimeout>[] = [];
  private handoffTimer: ReturnType<typeof setTimeout> | undefined;
  /** Loudest levels seen while the booth check runs. */
  private peaks: { local: number; remote: number } | null = null;
  /** True once the engine's live session runs the set (TAKE OVER goes through it). */
  private engineLive = false;
  /** A hello that arrived before our side finished pairing. */
  private pendingHello: { name: string; city: string } | null = null;
  /** Bumped on leave so late answers from an old room are ignored. */
  private generation = 0;
  /** The DJ view's data while the engine runs a set. */
  private feed: DeckFeed | null = null;

  constructor(
    readonly info: EngineInfo,
    private readonly bridge: Bridge = tauriBridge,
  ) {
    const b = this.bridge;
    this.unlisten = [
      b.onRoomEvent((e) => {
        if (this.room.phase !== 'booth' || this.room.remotePresence !== 'waiting') return;
        if (e.kind === 'error') {
          this.leaveRoom();
          this.roomDispatch({ type: 'createFailed', error: e.message });
          return;
        }
        this.relay = e.relay;
        this.paired(e.peerName);
      }),
      b.onPeerControl((json) => {
        const m = parsePeerMessage(json);
        if (m) this.onPeer(m);
      }),
      b.onLinkStatus((s) => this.onLinkStatus(s)),
      b.onLocalLevel((l) => {
        this.localLevel = fromWire(l);
        if (this.peaks) this.peaks.local = Math.max(this.peaks.local, this.localLevel.left, this.localLevel.right);
        this.localLevelListeners.emit();
      }),
      b.onLiveStatus((st) => this.onLiveStatus(st)),
      b.onLiveScope((c) => this.feed?.push(c)),
      b.onRemoteLevel((l) => {
        this.remoteLevel = fromWire(l);
        if (this.peaks) this.peaks.remote = Math.max(this.peaks.remote, this.remoteLevel.left, this.remoteLevel.right);
        this.remoteLevelListeners.emit();
      }),
    ];
  }

  dispose() {
    this.unlisten.forEach((u) => void u.then((f) => f()));
    this.clearTimers();
  }

  // --- SessionEngine: before the session ------------------------------------

  getRoom = () => this.room;
  subscribeRoom = (l: Listener) => this.roomListeners.add(l);

  setLocalProfile = (profile: { name: string; city: string }) => {
    // Trimmed when used, not here: this runs on every keystroke.
    const local = { name: profile.name.slice(0, 40), city: profile.city.slice(0, 40) };
    saveProfile(local);
    this.roomDispatch({ type: 'setLocal', local });
  };

  createRoom = () => {
    const r = this.room;
    if (r.phase !== 'home' || r.creating || r.joining) return;
    const problem = this.cantConnect();
    if (problem) return this.roomDispatch({ type: 'createFailed', error: problem });
    this.roomDispatch({ type: 'createStarted' });
    const gen = this.generation;
    this.bridge.createRoom(r.local.name.trim()).then(
      (code) => {
        if (gen !== this.generation) return;
        this.roomDispatch({ type: 'entered', code, isHost: true });
        this.loadDevices();
      },
      (e) => gen === this.generation && this.roomDispatch({ type: 'createFailed', error: messageOf(e) }),
    );
  };

  joinRoom = (input: string) => {
    const r = this.room;
    if (r.phase !== 'home' || r.joining || r.creating) return;
    const problem = this.cantConnect();
    if (problem) return this.roomDispatch({ type: 'joinFailed', error: problem });
    const code = normalizeCode(input);
    if (!code) {
      return this.roomDispatch({ type: 'joinFailed', error: 'That code needs 8 letters or numbers. Check the message you were sent.' });
    }
    this.roomDispatch({ type: 'joinStarted' });
    const gen = this.generation;
    this.bridge.joinRoom(code, r.local.name.trim()).then(
      (p) => {
        if (gen !== this.generation) return;
        this.relay = p.relay;
        this.roomDispatch({ type: 'entered', code: p.code, isHost: false });
        this.paired(p.peerName);
        this.loadDevices();
      },
      (e) => gen === this.generation && this.roomDispatch({ type: 'joinFailed', error: messageOf(e) }),
    );
  };

  selectInput = (id: string) => {
    this.roomDispatch({ type: 'selectInput', id });
    saveDevicePick('inputId', id);
    this.startMeter(id);
  };

  selectOutput = (id: string) => {
    this.roomDispatch({ type: 'selectOutput', id });
    saveDevicePick('outputId', id);
    void this.bridge.selectOutput(id).catch(() => {});
  };

  runBoothCheck = () => {
    const r = this.room;
    if (r.phase !== 'booth' || r.remotePresence === 'waiting' || r.check.status === 'running') return;
    this.roomDispatch({ type: 'checkStarted' });
    this.roomDispatch({ type: 'checkStep', id: 'network', status: 'running' });
    this.peaks = { local: -Infinity, remote: -Infinity };
    const gen = this.generation;
    const remoteName = r.remote?.name || 'the other DJ';
    const inputLabel = r.inputs.find((d) => d.id === r.inputId)?.label ?? 'your input';
    this.bridge.runNetworkTest(CHECK_SECONDS).then(
      (net) => {
        if (gen !== this.generation || this.room.check.status !== 'running') return;
        const peaks = this.peaks ?? { local: -Infinity, remote: -Infinity };
        this.peaks = null;
        const outcomes = evaluateCheck(net, {
          relay: this.relay,
          localPeakDb: peaks.local,
          remotePeakDb: peaks.remote,
          remoteName,
          inputLabel,
        });
        // A short beat per step, so each result can be read as it lands.
        outcomes.forEach((o, i) => {
          this.later(i * 220, () => {
            if (gen !== this.generation) return;
            this.roomDispatch({ type: 'checkStep', id: o.id, status: o.status, result: o.result });
          });
        });
      },
      (e) => {
        this.peaks = null;
        if (gen !== this.generation) return;
        for (const id of ['network', 'roundTrip', 'send', 'receive', 'sync'] as const) {
          this.roomDispatch({ type: 'checkStep', id, status: 'attention', result: id === 'network' ? messageOf(e) : 'Not measured' });
        }
      },
    );
  };

  setBoothReady = (ready: boolean) => {
    const before = this.room;
    this.roomDispatch({ type: 'setReady', ready, atMs: Date.now(), countdownMs: START_COUNTDOWN_MS });
    if (this.room.localReady !== before.localReady) this.send({ t: 'boothReady', ready: this.room.localReady });
    this.scheduleStart();
  };

  leaveRoom = () => {
    this.generation++;
    this.engineLive = false;
    this.clearTimers();
    this.peaks = null;
    this.pendingHello = null;
    this.feed = null;
    this.roomDispatch({ type: 'leave' });
    void this.bridge.leaveRoom().catch(() => {});
    void this.bridge.stopInputMeter().catch(() => {});
    this.link = initialRealLink();
    this.remoteLevel = SILENT;
    this.localLevel = SILENT;
    this.linkListeners.emit();
    this.remoteLevelListeners.emit();
    this.localLevelListeners.emit();
  };

  startPractice = (file: File | null) => {
    const r = this.room;
    if (r.phase !== 'home' || r.creating || r.joining) return;
    this.roomDispatch({ type: 'createStarted' });
    const gen = this.generation;
    const name = r.local.name.trim() || 'You';
    (file ? this.bridge.loadTrack(file) : Promise.resolve(null))
      .then((track) => this.bridge.startPractice(name, track))
      .then(
        (p) => {
          if (gen !== this.generation) return;
          const s = emptySession(Date.now());
          s.djs[LOCAL_ID] = { ...s.djs[LOCAL_ID], name, city: r.local.city.trim() };
          s.djs[REMOTE_ID] = { ...s.djs[REMOTE_ID], name: p.partnerName, city: GHOST_CITY };
          // The ghost opens on air; you cue, SYNC and take over.
          s.ownerId = REMOTE_ID;
          this.session = s;
          this.sessionListeners.emit();
          this.engineLive = true;
          this.startFeed();
          this.roomDispatch({ type: 'enterLive' });
        },
        (e) => gen === this.generation && this.roomDispatch({ type: 'createFailed', error: messageOf(e) }),
      );
  };

  // --- SessionEngine: during the session ------------------------------------

  getSession = () => this.session;
  subscribeSession = (l: Listener) => this.sessionListeners.add(l);
  getLink = () => this.link;
  subscribeLink = (l: Listener) => this.linkListeners.add(l);
  getRemoteLevel = () => this.remoteLevel;
  subscribeRemoteLevel = (l: Listener) => this.remoteLevelListeners.add(l);
  getLocalLevel = () => this.localLevel;
  subscribeLocalLevel = (l: Listener) => this.localLevelListeners.add(l);

  markReady = () => {
    if (this.engineLive) return void this.bridge.liveSetReady(true).catch(() => {});
    this.localMove({ type: 'markReady', djId: LOCAL_ID, atMs: Date.now() }, { t: 'markReady' });
  };
  cancelReady = () => {
    if (this.engineLive) return void this.bridge.liveSetReady(false).catch(() => {});
    this.localMove({ type: 'cancelReady', djId: LOCAL_ID, atMs: Date.now() }, { t: 'cancelReady' });
  };
  takeOver = () => {
    // With live audio the engine owns the mix: the screen follows its status.
    if (this.engineLive) return void this.bridge.liveTakeOver().catch(() => {});
    if (this.beginTakeOver(LOCAL_ID)) this.send({ t: 'takeOver' });
  };
  emergencyTakeOver = () => {
    // Only when the live DJ is gone; the message lands if they come back.
    if (this.link.remote === 'connected') return;
    if (this.engineLive) return void this.bridge.liveTakeOver().catch(() => {});
    clearTimeout(this.handoffTimer);
    this.localMove({ type: 'emergencyTakeOver', djId: LOCAL_ID, atMs: Date.now() }, { t: 'emergencyTakeOver' });
  };
  endSession = () => {
    clearTimeout(this.handoffTimer);
    this.localMove({ type: 'end', atMs: Date.now() }, { t: 'end' });
    if (this.engineLive) {
      this.engineLive = false;
      void this.bridge.stopLive().catch(() => {});
    }
  };

  getDeckFeed = () => this.feed;

  deck = (a: DeckAction) => {
    if (!this.engineLive) return;
    const b = this.bridge;
    const done = (p: Promise<unknown>) => void p.catch(() => {});
    switch (a.kind) {
      case 'playPause':
      case 'cue':
      case 'sync':
        return done(b.deckCommand(a.kind));
      case 'nudge':
        return done(b.deckCommand('nudge', a.ms));
      case 'pitch':
        return done(b.deckCommand('pitch', a.pct));
      case 'fader':
        return done(b.liveSetFader(a.value));
      case 'partnerVolume':
        return done(b.liveSetPartnerVolume(a.value));
      case 'ghostComeBack':
        return done(b.ghostComeBack());
    }
  };

  // --- internals -----------------------------------------------------------

  private cantConnect(): string | null {
    if (!this.room.local.name.trim()) return 'Add your DJ name first, so the other DJ knows who you are.';
    if (!this.info.roomServer) return 'This build has no room server set up yet. Try the demo instead.';
    return null;
  }

  private paired(peerName: string) {
    this.roomDispatch({ type: 'remoteJoined', remote: { name: peerName || 'Other DJ', city: '' } });
    if (this.pendingHello) {
      this.roomDispatch({ type: 'remoteProfile', remote: this.pendingHello });
      this.pendingHello = null;
    }
    this.send({ t: 'hello', name: this.room.local.name.trim(), city: this.room.local.city.trim() });
  }

  private loadDevices() {
    const gen = this.generation;
    this.bridge.listDevices().then(
      (d) => {
        if (gen !== this.generation) return;
        this.roomDispatch({ type: 'devicesFound', inputs: d.inputs, outputs: d.outputs, preferred: loadDevicePicks() });
        if (this.room.inputId) this.startMeter(this.room.inputId);
        // The engine opens what the screen shows, a remembered pick included.
        if (this.room.outputId) void this.bridge.selectOutput(this.room.outputId).catch(() => {});
      },
      () => {},
    );
  }

  private startMeter(id: string) {
    void this.bridge.startInputMeter(id).catch(() => {
      this.localLevel = SILENT;
      this.localLevelListeners.emit();
    });
  }

  private send(m: PeerMessage) {
    void this.bridge.sendControl(m).catch(() => {});
  }

  private onPeer(m: PeerMessage) {
    const now = Date.now();
    switch (m.t) {
      case 'hello':
        // Both sides say hello on pairing; theirs can beat our own pairing result.
        if (!this.room.remote) this.pendingHello = { name: m.name, city: m.city };
        this.roomDispatch({ type: 'remoteProfile', remote: { name: m.name || 'Other DJ', city: m.city } });
        this.setSessionDj(REMOTE_ID, m.name, m.city);
        return;
      case 'boothReady':
        if (this.room.phase !== 'booth') return;
        if (m.ready) {
          this.room = roomAfterRemoteReady(this.room, now, START_COUNTDOWN_MS);
          this.roomListeners.emit();
        } else {
          this.roomDispatch({ type: 'remoteReady', ready: false });
        }
        this.scheduleStart();
        return;
      case 'markReady':
        return this.dispatch({ type: 'markReady', djId: REMOTE_ID, atMs: now });
      case 'cancelReady':
        return this.dispatch({ type: 'cancelReady', djId: REMOTE_ID, atMs: now });
      case 'takeOver':
        this.beginTakeOver(REMOTE_ID);
        return;
      case 'emergencyTakeOver':
        clearTimeout(this.handoffTimer);
        return this.dispatch({ type: 'emergencyTakeOver', djId: REMOTE_ID, atMs: now });
      case 'end':
        clearTimeout(this.handoffTimer);
        return this.dispatch({ type: 'end', atMs: now });
      case 'leave':
        return this.onLinkStatus({ ...this.lastStatus(), state: 'left' });
    }
  }

  private lastStatus(): LinkStatus {
    const d = this.link.diagnostics;
    return { state: 'connected', rttMs: d.roundTripMs, jitterMs: d.jitterMs, lossPct: d.packetLossPct, relay: d.path === 'relay' };
  }

  private onLinkStatus(s: LinkStatus) {
    if (this.room.phase === 'booth') {
      if (s.state === 'left' && this.room.remotePresence !== 'waiting') {
        // A room pairs two DJs once, so there is nothing to wait in: back to Home.
        const name = this.room.remote?.name || 'The other DJ';
        this.leaveRoom();
        this.roomDispatch({ type: 'createFailed', error: `${name} left the room. Start a new one when you're both ready.` });
        return;
      }
      this.link = linkStateFrom(s, this.link);
      this.linkListeners.emit();
      return;
    }
    if (this.room.phase !== 'live') return;
    const was = this.link.remote;
    this.link = linkStateFrom(s, this.link);
    this.linkListeners.emit();
    const now = Date.now();
    if (was === 'connected' && this.link.remote !== 'connected') this.dispatch({ type: 'remoteReconnecting', atMs: now });
    if (was !== 'connected' && this.link.remote === 'connected') this.dispatch({ type: 'remoteReconnected', atMs: now });
  }

  private localMove(action: SessionAction, message: PeerMessage) {
    const before = this.session;
    this.dispatch(action);
    if (this.session !== before) this.send(message);
  }

  /** Both apps run the handoff timer, so neither waits on a message to finish it. */
  private beginTakeOver(djId: string): boolean {
    const before = this.session;
    this.dispatch({ type: 'takeOver', djId, atMs: Date.now(), durationMs: HANDOFF_DURATION_MS });
    if (this.session === before) return false;
    clearTimeout(this.handoffTimer);
    this.handoffTimer = setTimeout(() => this.dispatch({ type: 'completeHandoff', atMs: Date.now() }), HANDOFF_DURATION_MS);
    return true;
  }

  private scheduleStart() {
    const at = this.room.startsAtMs;
    if (at === null) return;
    this.later(Math.max(0, at - Date.now()), () => {
      if (this.room.startsAtMs !== at || this.room.phase !== 'booth') return;
      const now = Date.now();
      const s = emptySession(now);
      const r = this.room;
      s.djs[LOCAL_ID] = { ...s.djs[LOCAL_ID], name: r.local.name.trim(), city: r.local.city.trim() };
      s.djs[REMOTE_ID] = { ...s.djs[REMOTE_ID], name: r.remote?.name ?? 'Other DJ', city: r.remote?.city ?? '' };
      // The host opens on air; the other DJ cues first.
      s.ownerId = r.isHost ? LOCAL_ID : REMOTE_ID;
      this.session = s;
      this.sessionListeners.emit();
      this.roomDispatch({ type: 'enterLive' });
      if (this.info.liveAudio) this.startAudio(r.isHost);
    });
  }

  /** Hands the connection, your input and headphones to the engine. */
  private startAudio(startOnAir: boolean) {
    const gen = this.generation;
    this.bridge.startLive(startOnAir).then(
      () => {
        if (gen !== this.generation) return;
        this.engineLive = true;
        this.startFeed();
      },
      (e) => {
        if (gen !== this.generation) return;
        this.leaveRoom();
        this.roomDispatch({ type: 'createFailed', error: messageOf(e) });
      },
    );
  }

  /** A fresh DJ view feed, plus the deck's whole-track waveform once the engine has it. */
  private startFeed() {
    const feed = new DeckFeed();
    this.feed = feed;
    // The screen picks the feed up with the session.
    this.sessionListeners.emit();
    const fetchWave = (tries: number) => {
      this.bridge.deckWave().then(
        (buf) => {
          if (this.feed !== feed) return;
          if (buf.byteLength > 0) feed.setWave(new Uint8Array(buf));
          else if (tries > 0) this.later(400, () => fetchWave(tries - 1));
        },
        () => {},
      );
    };
    fetchWave(10);
  }

  private onLiveStatus(st: LiveStatus) {
    if (!this.engineLive || this.room.phase !== 'live') return;
    const now = Date.now();
    this.feed?.update(deckInfo(st), performance.now());

    const was = this.link.remote;
    this.link = linkStateFromLive(st, this.link);
    this.linkListeners.emit();
    if (was === 'connected' && this.link.remote !== 'connected') this.dispatch({ type: 'remoteReconnecting', atMs: now });
    if (was !== 'connected' && this.link.remote === 'connected') this.dispatch({ type: 'remoteReconnected', atMs: now });

    const local = peakToDb(st.local_peak);
    const remote = peakToDb(st.partner_peak);
    this.localLevel = { left: local, right: local };
    this.remoteLevel = { left: remote, right: remote };
    this.localLevelListeners.emit();
    this.remoteLevelListeners.emit();

    // The other DJ ended the set (their app said goodbye): end it here too.
    if (st.phase === 'partner left' && this.session.status === 'live') {
      this.endSession();
      return;
    }

    // READY flags ride the engine's State packet.
    for (const [djId, on] of [
      [LOCAL_ID, st.ready],
      [REMOTE_ID, st.partner_ready],
    ] as const) {
      const shown = this.session.readyIds.includes(djId);
      if (on && !shown) this.dispatch({ type: 'markReady', djId, atMs: now });
      if (!on && shown && !this.session.handoff) this.dispatch({ type: 'cancelReady', djId, atMs: now });
    }

    // Who owns the mix is the engine's call; the screen catches up.
    const owner = st.on_air ? LOCAL_ID : st.partner_on_air ? REMOTE_ID : null;
    const s = this.session;
    if (!owner || s.status !== 'live' || s.handoff || owner === s.ownerId) return;
    if (this.link.remote !== 'connected') {
      this.dispatch({ type: 'emergencyTakeOver', djId: owner, atMs: now });
    } else {
      this.beginTakeOver(owner);
    }
  }

  private setSessionDj(id: string, name: string, city: string) {
    const dj = this.session.djs[id];
    this.session = { ...this.session, djs: { ...this.session.djs, [id]: { ...dj, name: name || dj.name, city } } };
    this.sessionListeners.emit();
  }

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
    this.timers.push(setTimeout(fn, ms));
  }

  private clearTimers() {
    this.timers.forEach((t) => clearTimeout(t));
    this.timers = [];
    clearTimeout(this.handoffTimer);
  }
}

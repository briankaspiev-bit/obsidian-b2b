// Two TauriSessionEngines wired to each other through an in-memory bridge:
// the whole Home → Booth Check → countdown → Live → TAKE OVER flow, as two
// laptops would run it, without Tauri.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Bridge, EngineInfo, LinkStatus, LiveStatus, NetworkResult, RoomEvent, WireLevel } from './bridge';
import type { ScopeChunk } from './deck';
import { HANDOFF_DURATION_MS, START_COUNTDOWN_MS } from './engine';
import { roleOf } from './sessionReducer';
import { TauriSessionEngine } from './tauriEngine';

type Handlers = {
  room?: (e: RoomEvent) => void;
  control?: (json: string) => void;
  status?: (s: LinkStatus) => void;
  local?: (l: WireLevel) => void;
  remote?: (l: WireLevel) => void;
  live?: (s: LiveStatus) => void;
  scope?: (c: ScopeChunk) => void;
};

const liveStatus = (onAir: boolean, partnerOnAir: boolean, ready = false, partnerReady = false, phase = 'live'): LiveStatus => ({
  phase,
  partner_name: null,
  on_air: onAir,
  partner_on_air: partnerOnAir,
  ready,
  partner_ready: partnerReady,
  booth_delay_ms: 80,
  rtt_ms: 150,
  margin_ms: 20,
  recovered_10s: 0,
  concealed_10s: 0,
  local_peak: 0.5,
  partner_peak: 0.25,
  send_kbps: 900,
  now_ms: 1000,
  you_beat: null,
  partner_beat: null,
  partner_bpm: null,
  deck: null,
  fader: 1,
  partner_volume: 1,
  ghost_says: null,
});

const NET: NetworkResult = {
  pingsSent: 40,
  pongsReceived: 40,
  lossPct: 0,
  rttMs: 30,
  rttMinMs: 28,
  jitterMs: 2,
  clockOffsetMs: 5,
  reached: true,
};

/** A pretend room server, booth link and live engine connecting two fake bridges. */
function fakeWorld() {
  const rooms = new Map<string, { host: Handlers; hostName: string }>();
  // The live engine's view: who is on air and who is ready, per side.
  const onAir = new Map<Handlers, boolean>();
  const ready = new Map<Handlers, boolean>();
  const left = new Set<Handlers>();
  const publish = () => {
    for (const [h, mine] of onAir) {
      const o = [...onAir.keys()].find((k) => k !== h);
      const other = o ? onAir.get(o)! : false;
      const otherReady = o ? (ready.get(o) ?? false) && !other : false;
      h.live?.(liveStatus(mine, other, ready.get(h) ?? false, otherReady, o && left.has(o) ? 'partner left' : 'live'));
    }
  };
  const make = () => {
    const h: Handlers = {};
    const calls: string[] = [];
    let peer: Handlers | null = null;
    const ok = <T,>(v: T) => Promise.resolve(v);
    const b: Bridge = {
      engineInfo: () => ok({ version: 't', roomServer: 'test:3478', liveAudio: false }),
      listDevices: () => ok({ inputs: [{ id: 'in', label: 'Mixer', detail: '2 ch' }], outputs: [{ id: 'out', label: 'Phones', detail: '2 ch' }] }),
      startInputMeter: () => ok(undefined),
      stopInputMeter: () => ok(undefined),
      createRoom: (name: string) => {
        rooms.set('ABCD-EFGH', { host: h, hostName: name });
        return ok('ABCD-EFGH');
      },
      joinRoom: (code: string, name: string) => {
        const r = rooms.get(code);
        if (!r) return Promise.reject('No room with that code.');
        peer = r.host;
        hostLinks.set(r.host, h);
        // Tell the host side who arrived. Its hello may land before our join resolves.
        r.host.room?.({ kind: 'paired', code, peerName: name, isHost: true, relay: false });
        return ok({ code, peerName: r.hostName, isHost: false, relay: false });
      },
      leaveRoom: () => {
        const other = peer ?? hostLinks.get(h);
        // The goodbye crosses the network: it arrives later, not inside this call.
        queueMicrotask(() => other?.status?.({ state: 'left', rttMs: 0, jitterMs: 0, lossPct: 100, relay: false }));
        return ok(undefined);
      },
      sendControl: (msg: unknown) => {
        const other = peer ?? hostLinks.get(h);
        other?.control?.(JSON.stringify(msg));
        return ok(undefined);
      },
      selectOutput: () => ok(undefined),
      startLive: (startOnAir: boolean) => {
        onAir.set(h, startOnAir);
        // The engine reports a few times a second; the first report lands after start.
        if (onAir.size === 2) setTimeout(publish, 0);
        return ok(undefined);
      },
      liveTakeOver: () => {
        for (const k of onAir.keys()) onAir.set(k, k === h);
        ready.set(h, false); // TAKE OVER clears READY
        publish();
        return ok(undefined);
      },
      liveSetReady: (r: boolean) => {
        ready.set(h, r);
        publish();
        return ok(undefined);
      },
      liveSetFader: (v: number) => (calls.push(`fader ${v}`), ok(undefined)),
      loadTrack: (f: File) => (calls.push(`load ${f.name}`), ok(`C:/cache/${f.name}`)),
      startPractice: (name: string, track: string | null) => {
        calls.push(`practice ${name} ${track}`);
        onAir.set(h, false);
        return ok({ partnerName: 'Ghost DJ', path: 'nyc-lon', track: track ?? 'Test groove B' });
      },
      deckCommand: (action: string, value?: number) => (calls.push(`deck ${action}${value === undefined ? '' : ` ${value}`}`), ok(undefined)),
      ghostComeBack: () => (calls.push('ghost'), ok(undefined)),
      deckWave: () => ok(new Uint8Array(30).buffer),
      liveSetPartnerVolume: () => ok(undefined),
      stopLive: () => {
        left.add(h);
        onAir.delete(h);
        publish();
        return ok('C:/Music/Obsidian/set-1');
      },
      runNetworkTest: () => {
        // Both mixers play during the check.
        h.local?.({ left: -9, right: -10 });
        h.remote?.({ left: -11, right: null });
        return ok(NET);
      },
      onRoomEvent: (f) => ((h.room = f), ok(() => {})),
      onPeerControl: (f) => ((h.control = f), ok(() => {})),
      onLinkStatus: (f) => ((h.status = f), ok(() => {})),
      onLocalLevel: (f) => ((h.local = f), ok(() => {})),
      onRemoteLevel: (f) => ((h.remote = f), ok(() => {})),
      onLiveStatus: (f) => ((h.live = f), ok(() => {})),
      onLiveScope: (f) => ((h.scope = f), ok(() => {})),
    };
    return { b, h, calls };
  };
  const hostLinks = new Map<Handlers, Handlers>();
  return { make };
}

const info: EngineInfo = { version: 't', roomServer: 'test:3478', liveAudio: false };

describe('two desktop apps', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  async function pairedAndLive(engineInfo: EngineInfo = info) {
    const world = fakeWorld();
    const a = world.make();
    const b = world.make();
    const val = new TauriSessionEngine(engineInfo, a.b);
    const dana = new TauriSessionEngine(engineInfo, b.b);
    val.setLocalProfile({ name: 'Val', city: 'New York' });
    dana.setLocalProfile({ name: 'Dana', city: 'London' });

    val.createRoom();
    await vi.advanceTimersByTimeAsync(0);
    expect(val.getRoom()).toMatchObject({ phase: 'booth', code: 'ABCD-EFGH', isHost: true, remotePresence: 'waiting' });

    dana.joinRoom('abcd efgh');
    await vi.advanceTimersByTimeAsync(0);
    expect(dana.getRoom()).toMatchObject({ phase: 'booth', isHost: false, remote: { name: 'Val', city: 'New York' } });
    expect(val.getRoom()).toMatchObject({ remotePresence: 'joined', remote: { name: 'Dana', city: 'London' } });

    for (const e of [val, dana]) {
      e.runBoothCheck();
      await vi.advanceTimersByTimeAsync(2000);
      expect(e.getRoom().check.status).toBe('passed');
    }
    val.setBoothReady(true);
    expect(dana.getRoom().remotePresence).toBe('ready');
    dana.setBoothReady(true);
    expect(val.getRoom().startsAtMs).not.toBeNull();
    await vi.advanceTimersByTimeAsync(START_COUNTDOWN_MS + 10);
    return { val, dana };
  }

  it('meet with a code, check, count down and go live with the host on air', async () => {
    const { val, dana } = await pairedAndLive();
    expect(val.getRoom().phase).toBe('live');
    expect(dana.getRoom().phase).toBe('live');
    expect(roleOf(val.getSession(), 'local')).toBe('onAir');
    expect(roleOf(dana.getSession(), 'remote')).toBe('onAir');
    expect(dana.getSession().djs.remote.name).toBe('Val');
  });

  it('a TAKE OVER on one laptop hands the mix over on both', async () => {
    const { val, dana } = await pairedAndLive();
    dana.markReady();
    expect(roleOf(val.getSession(), 'remote')).toBe('ready');
    dana.takeOver();
    expect(roleOf(val.getSession(), 'local')).toBe('handoff');
    await vi.advanceTimersByTimeAsync(HANDOFF_DURATION_MS + 10);
    expect(roleOf(val.getSession(), 'remote')).toBe('onAir');
    expect(roleOf(dana.getSession(), 'local')).toBe('onAir');
  });

  it('ending the set ends it for both', async () => {
    const { val, dana } = await pairedAndLive();
    val.endSession();
    expect(dana.getSession().status).toBe('ended');
  });

  it('a wrong code and a missing name are explained on Home', async () => {
    const world = fakeWorld();
    const e = new TauriSessionEngine(info, world.make().b);
    e.setLocalProfile({ name: '', city: '' });
    e.createRoom();
    expect(e.getRoom().createError).toContain('DJ name');
    e.setLocalProfile({ name: 'Val', city: '' });
    e.joinRoom('ZZZZ-ZZZZ');
    await vi.advanceTimersByTimeAsync(0);
    expect(e.getRoom().joinError).toContain('No room');
  });

  it('the other DJ leaving the booth sends you back to Home with a note', async () => {
    const world = fakeWorld();
    const a = world.make();
    const b = world.make();
    const val = new TauriSessionEngine(info, a.b);
    const dana = new TauriSessionEngine(info, b.b);
    val.setLocalProfile({ name: 'Val', city: '' });
    dana.setLocalProfile({ name: 'Dana', city: '' });
    val.createRoom();
    await vi.advanceTimersByTimeAsync(0);
    dana.joinRoom('ABCD-EFGH');
    await vi.advanceTimersByTimeAsync(0);
    dana.leaveRoom();
    await vi.advanceTimersByTimeAsync(0);
    expect(val.getRoom().phase).toBe('home');
    expect(val.getRoom().createError).toContain('Dana left');
  });

  it('with live audio, TAKE OVER goes through the engine and both screens follow it', async () => {
    const { val, dana } = await pairedAndLive({ ...info, liveAudio: true });
    await vi.advanceTimersByTimeAsync(0);
    expect(roleOf(dana.getSession(), 'remote')).toBe('onAir');
    expect(dana.getLink().diagnostics.bufferMs).toBe(20);
    expect(dana.getRemoteLevel().left).toBeCloseTo(-12.04, 1);
    dana.markReady();
    expect(roleOf(val.getSession(), 'remote')).toBe('ready');
    expect(roleOf(dana.getSession(), 'local')).toBe('ready');
    dana.takeOver();
    expect(roleOf(val.getSession(), 'local')).toBe('handoff');
    await vi.advanceTimersByTimeAsync(HANDOFF_DURATION_MS + 10);
    expect(roleOf(val.getSession(), 'remote')).toBe('onAir');
    expect(roleOf(dana.getSession(), 'local')).toBe('onAir');
    expect(dana.getSession().readyIds).toEqual([]);

    val.endSession();
    expect(dana.getSession().status).toBe('ended');
  });

  it('practice: the ghost opens on air, the DJ view gets its feed, the deck goes through the engine', async () => {
    const world = fakeWorld();
    const a = world.make();
    const e = new TauriSessionEngine({ ...info, liveAudio: true }, a.b);
    e.setLocalProfile({ name: 'Brian', city: 'New York' });
    e.startPractice(new File([new Uint8Array(4)], 'My Track.mp3'));
    await vi.advanceTimersByTimeAsync(0);
    expect(a.calls).toEqual(['load My Track.mp3', 'practice Brian C:/cache/My Track.mp3']);
    expect(e.getRoom().phase).toBe('live');
    const s = e.getSession();
    expect(s.djs.remote).toMatchObject({ name: 'Ghost DJ', city: 'London (simulated)' });
    expect(roleOf(s, 'remote')).toBe('onAir');

    const feed = e.getDeckFeed();
    expect(feed).not.toBeNull();
    await vi.advanceTimersByTimeAsync(0);
    expect(feed!.wave?.length).toBe(30);
    a.h.scope?.({ first: 0, cols: [{ you: [1, 2, 3], partner: [200, 50, 10] }] });
    expect(feed!.end).toBe(1);
    expect(Array.from(feed!.partner.slice(0, 3))).toEqual([200, 50, 10]);
    a.h.live?.({
      ...liveStatus(false, true),
      now_ms: 5000,
      partner_beat: { period_ms: 480, beat_ms: 4800, bar_ms: null },
      ghost_says: 'on air',
    });
    expect(feed!.info).toMatchObject({ nowMs: 5000, partnerOnAir: true, ghostSays: 'on air' });

    e.deck({ kind: 'playPause' });
    e.deck({ kind: 'nudge', ms: -10 });
    e.deck({ kind: 'fader', value: 0.5 });
    e.deck({ kind: 'ghostComeBack' });
    expect(a.calls.slice(2)).toEqual(['deck playPause', 'deck nudge -10', 'fader 0.5', 'ghost']);

    e.takeOver();
    await vi.advanceTimersByTimeAsync(HANDOFF_DURATION_MS + 10);
    expect(roleOf(e.getSession(), 'local')).toBe('onAir');
    e.endSession();
    expect(e.getSession().status).toBe('ended');
    e.leaveRoom();
    expect(e.getDeckFeed()).toBeNull();
  });
});

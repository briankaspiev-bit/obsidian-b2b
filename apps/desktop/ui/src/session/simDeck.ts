// A stand-in for the engine's DJ view data, for the demo and the browser
// preview: two synthetic grooves (theirs at 124 BPM, yours at 122 so SYNC has
// work to do), streamed into a DeckFeed the same way the desktop app streams
// the real engine's columns and beat clocks.

import { COLUMN_MS, DeckFeed, type BeatClock, type DeckAction, type DeckInfo, type DeckStatus } from './deck';

const PARTNER_BPM = 124;
const YOUR_BPM = 122;
const TRACK_S = 6 * 60;

/** Pseudo-random 0..1 from an integer, so the grooves look the same every run. */
function hash(n: number): number {
  const x = Math.sin(n * 12.9898) * 43758.5453;
  return x - Math.floor(x);
}

/**
 * A 4/4 groove as waveform columns: kick on every beat, clap on 2 and 4,
 * offbeat hats, a bassline, and an 8-bar breakdown every 32 bars.
 */
export function synthTrack(bpm: number, seconds: number, seed: number): Uint8Array {
  const n = Math.floor((seconds * 1000) / COLUMN_MS);
  const out = new Uint8Array(n * 3);
  const beat = 60_000 / bpm;
  for (let i = 0; i < n; i++) {
    const t = i * COLUMN_MS;
    const b = t / beat;
    const k = Math.floor(b);
    const since = (b - k) * beat;
    const bar = Math.floor(k / 4);
    const phraseBar = bar % 32;
    const breakdown = phraseBar >= 24;
    const intro = bar < 8;
    const r = hash(i + seed * 7919);
    let lo = 0;
    let mid = 30 + 25 * r;
    let hi = 18 + 30 * hash(i * 3 + seed);
    if (!breakdown) {
      lo = 235 * Math.exp(-since / 70) + (intro ? 0 : 70 + 30 * Math.sin((t / beat) * Math.PI));
      if (k % 4 === 1 || k % 4 === 3) mid += 150 * Math.exp(-since / 45);
      const off = Math.abs(since - beat / 2);
      if (!intro) hi += 140 * Math.exp(-off / 18);
    } else {
      // Pads and a riser instead of drums.
      mid += 70 + 40 * Math.sin(t / 900 + seed);
      hi += ((phraseBar - 24) / 8) * 90 * r;
      lo = 25 * r;
    }
    out[i * 3] = Math.min(255, lo);
    out[i * 3 + 1] = Math.min(255, mid);
    out[i * 3 + 2] = Math.min(255, hi);
  }
  return out;
}

export class SimDeck {
  readonly feed = new DeckFeed();
  private partnerTrack = synthTrack(PARTNER_BPM, TRACK_S, 1);
  private yourTrack = synthTrack(YOUR_BPM, TRACK_S, 2);
  private col = 0;
  /** Where in your track the play head is (ms, at the track's own speed). */
  private pos = 0;
  private pitch = 1;
  private playing = false;
  private sync = false;
  private fader = 1;
  private partnerVolume = 1;
  private wasOnAir: boolean | null = null;
  /** Partner track position at output time 0 (ms); off your grid, so SYNC has work to do. */
  private partnerStart = 64 * (60_000 / PARTNER_BPM) + 137;
  private timer: ReturnType<typeof setInterval> | undefined;
  private startedAt = 0;

  constructor(
    private readonly onAir: () => { you: boolean; partner: boolean },
    private readonly partnerSays: string | null = null,
  ) {
    this.feed.setWave(this.yourTrack);
  }

  start() {
    this.stop();
    this.startedAt = performance.now();
    this.timer = setInterval(() => this.tick(), 50);
  }

  stop() {
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
  }

  act(a: DeckAction) {
    const len = TRACK_S * 1000;
    switch (a.kind) {
      case 'playPause':
        this.playing = !this.playing;
        break;
      case 'cue':
        this.playing = false;
        this.pos = 0;
        break;
      case 'sync':
        this.sync = !this.sync;
        if (this.sync) this.lockToPartner();
        break;
      case 'nudge':
        this.pos = (this.pos + a.ms * this.pitch + len) % len;
        break;
      case 'pitch':
        this.pitch = Math.min(1.15, Math.max(0.85, this.pitch + a.pct / 100));
        this.sync = false;
        break;
      case 'fader':
        this.fader = Math.min(1, Math.max(0, a.value));
        break;
      case 'partnerVolume':
        this.partnerVolume = Math.min(2, Math.max(0, a.value));
        break;
      case 'ghostComeBack':
        break;
    }
    this.publish();
  }

  /** Match tempo, then move the play head so your next kick lands on theirs. */
  private lockToPartner() {
    this.pitch = PARTNER_BPM / YOUR_BPM;
    const now = this.col * COLUMN_MS;
    const you = this.yourBeat(now);
    const them = this.partnerBeat();
    if (!you) return;
    const p = them.period_ms;
    let d = (you.beat_ms - them.beat_ms) % p;
    if (d < 0) d += p;
    if (d > p / 2) d -= p;
    this.pos += d * this.pitch;
  }

  private partnerBeat(): BeatClock {
    const p = 60_000 / PARTNER_BPM;
    return { period_ms: p, beat_ms: -this.partnerStart, bar_ms: null };
  }

  private yourBeat(now: number): BeatClock | null {
    if (!this.playing) return null;
    const pTrack = 60_000 / YOUR_BPM;
    const k = Math.ceil(this.pos / pTrack);
    const beat = now + (k * pTrack - this.pos) / this.pitch;
    const period = pTrack / this.pitch;
    return { period_ms: period, beat_ms: beat, bar_ms: beat - (k % 4) * period };
  }

  private tick() {
    const target = Math.floor((performance.now() - this.startedAt) / COLUMN_MS);
    const cols: { you: [number, number, number]; partner: [number, number, number] }[] = [];
    const first = this.col;
    const plen = this.partnerTrack.length / 3;
    const ylen = this.yourTrack.length / 3;
    while (this.col < target) {
      const t = this.col * COLUMN_MS;
      const pi = (Math.floor((t + this.partnerStart) / COLUMN_MS) % plen) * 3;
      const partner: [number, number, number] = [this.partnerTrack[pi], this.partnerTrack[pi + 1], this.partnerTrack[pi + 2]];
      let you: [number, number, number] = [0, 0, 0];
      if (this.playing) {
        const yi = (Math.floor(this.pos / COLUMN_MS) % ylen) * 3;
        const g = Math.sqrt(this.fader);
        you = [this.yourTrack[yi] * g, this.yourTrack[yi + 1] * g, this.yourTrack[yi + 2] * g];
        this.pos = (this.pos + COLUMN_MS * this.pitch) % (TRACK_S * 1000);
      }
      cols.push({ you, partner });
      this.col++;
    }
    if (cols.length) this.feed.push({ first, cols });
    this.publish();
  }

  private publish() {
    const now = this.col * COLUMN_MS;
    const air = this.onAir();
    // Same rules as the engine: a handoff brings both songs back to full, and
    // only the DJ on air sets the blend.
    if (this.wasOnAir !== null && air.you !== this.wasOnAir) {
      this.partnerVolume = 1;
      if (air.you) this.fader = 1;
    }
    this.wasOnAir = air.you;
    if (!air.you) this.partnerVolume = 1;
    const deck: DeckStatus = {
      title: 'Midnight Run (Extended Mix)',
      playing: this.playing,
      track_bpm: YOUR_BPM,
      bpm: YOUR_BPM * this.pitch,
      pitch_pct: (this.pitch - 1) * 100,
      sync: this.sync,
      sync_err_ms: this.sync ? 0.4 : null,
      pos_s: this.pos / 1000,
      len_s: TRACK_S,
    };
    const info: DeckInfo = {
      nowMs: now,
      you: this.yourBeat(now),
      partner: this.partnerBeat(),
      partnerBpm: PARTNER_BPM,
      deck,
      onAir: air.you,
      partnerOnAir: air.partner,
      fader: this.fader,
      partnerVolume: this.partnerVolume,
      ghostSays: this.partnerSays,
    };
    this.feed.update(info, this.startedAt + now);
  }
}

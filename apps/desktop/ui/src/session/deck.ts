// What the DJ view draws: both decks' waveforms as you hear them, where their
// beats fall, and your deck's own state. The engine sends one waveform column
// per 5 ms block (low / mid / high peaks, 0..255) on its output clock, plus
// beat clocks on the same clock; see crates/live/src/scope.rs.

/** One column per this many ms of output. */
export const COLUMN_MS = 5;
/** How much history the view keeps (60 s). */
export const KEEP_COLUMNS = 12_000;

export type Bands = [number, number, number];

export interface ScopeChunk {
  first: number;
  cols: { you: Bands; partner: Bands }[];
}

/** Beats at beat_ms + k * period_ms on the output clock; bar_ms is a beat 1, if known. */
export interface BeatClock {
  period_ms: number;
  beat_ms: number;
  bar_ms: number | null;
}

/** The built-in deck, as the engine reports it. */
export interface DeckStatus {
  title: string;
  playing: boolean;
  track_bpm: number | null;
  bpm: number | null;
  pitch_pct: number;
  sync: boolean;
  sync_err_ms: number | null;
  pos_s: number;
  len_s: number;
}

export interface DeckInfo {
  nowMs: number;
  you: BeatClock | null;
  partner: BeatClock | null;
  partnerBpm: number | null;
  deck: DeckStatus | null;
  onAir: boolean;
  partnerOnAir: boolean;
  fader: number;
  partnerVolume: number;
  /** Practice: what the ghost DJ is doing. */
  ghostSays: string | null;
  /** Wi-Fi shield on for what you hear (your connection drops sound). */
  shield?: boolean;
  /** Wi-Fi shield on for what the partner hears. */
  partnerShield?: boolean;
}

export type DeckAction =
  | { kind: 'playPause' }
  | { kind: 'cue' }
  | { kind: 'sync' }
  /** ms; positive pulls your beat earlier. */
  | { kind: 'nudge'; ms: number }
  /** Percent, added to the current pitch. */
  | { kind: 'pitch'; pct: number }
  | { kind: 'fader'; value: number }
  | { kind: 'partnerVolume'; value: number }
  | { kind: 'ghostComeBack' };

/** Commands from the DJ view; an engine without a deck view leaves this out. */
export interface DeckControls {
  getDeckFeed(): DeckFeed | null;
  deck(action: DeckAction): void;
}

/** Your beat relative to the partner's, as the phase meter shows it. */
export interface Phase {
  /** Signed, within half a beat either side; positive means your kick lands late. */
  ms: number;
  /** ms as a fraction of a beat, -0.5..0.5. */
  beats: number;
  /** Whole beats your bar is shifted against theirs (0..3), when both bars are known. */
  barBeats: number | null;
}

export function fold(x: number, period: number): number {
  const d = ((x % period) + period) % period;
  return d > period / 2 ? d - period : d;
}

export function phaseOf(you: BeatClock | null, partner: BeatClock | null): Phase | null {
  if (!you || !partner) return null;
  const p = partner.period_ms;
  const ms = fold(you.beat_ms - partner.beat_ms, p);
  let barBeats: number | null = null;
  if (you.bar_ms !== null && partner.bar_ms !== null) {
    const bar = 4 * p;
    const off = (((you.bar_ms - partner.bar_ms - ms) % bar) + bar) % bar;
    barBeats = Math.round(off / p) % 4;
  }
  return { ms, beats: ms / p, barBeats };
}

/** Beat in bar (0..3) of the beat at or before time t; null without a bar. */
export function beatInBar(c: BeatClock, t: number): number | null {
  if (c.bar_ms === null) return null;
  const n = Math.floor((t - c.bar_ms) / c.period_ms + 1e-6);
  return ((n % 4) + 4) % 4;
}

/** Beat times between t0 and t1 (inclusive), each with whether it starts a bar. */
export function beatsBetween(c: BeatClock, t0: number, t1: number): { t: number; bar: boolean }[] {
  const out: { t: number; bar: boolean }[] = [];
  const k0 = Math.ceil((t0 - c.beat_ms) / c.period_ms);
  for (let k = k0; ; k++) {
    const t = c.beat_ms + k * c.period_ms;
    if (t > t1) break;
    out.push({ t, bar: c.bar_ms !== null && Math.abs(fold(t - c.bar_ms, 4 * c.period_ms)) < c.period_ms / 4 });
  }
  return out;
}

type Listener = () => void;

/**
 * The view's copy of the engine's waveform columns and latest deck state.
 * Columns land in ring buffers the canvas reads every frame; slower state
 * (beats, deck, on air) notifies subscribers.
 */
export class DeckFeed {
  readonly you = new Uint8Array(KEEP_COLUMNS * 3);
  readonly partner = new Uint8Array(KEEP_COLUMNS * 3);
  /** One past the newest column. */
  end = 0;
  info: DeckInfo | null = null;
  /** Your deck's whole track at its own speed, 3 bytes per column. */
  wave: Uint8Array | null = null;
  /** Output clock minus performance.now(), smoothed. */
  private offset: number | null = null;
  private listeners = new Set<Listener>();

  push(chunk: ScopeChunk) {
    chunk.cols.forEach((c, i) => {
      const at = ((chunk.first + i) % KEEP_COLUMNS) * 3;
      this.you.set(c.you, at);
      this.partner.set(c.partner, at);
    });
    this.end = Math.max(this.end, chunk.first + chunk.cols.length);
  }

  update(info: DeckInfo, perfNow: number) {
    const o = info.nowMs - perfNow;
    // Statuses arrive a little late and unevenly: follow slowly, jump on big moves.
    if (this.offset === null || Math.abs(o - this.offset) > 250) this.offset = o;
    else this.offset += (o - this.offset) * 0.05;
    this.info = info;
    this.listeners.forEach((l) => l());
  }

  setWave(wave: Uint8Array | null) {
    this.wave = wave && wave.length >= 3 ? wave : null;
    this.listeners.forEach((l) => l());
  }

  /** The output clock now (ms), never ahead of the newest column. */
  nowMs(perfNow: number): number {
    if (this.offset === null) return this.end * COLUMN_MS;
    return Math.min(perfNow + this.offset, this.end * COLUMN_MS + 40);
  }

  /** True when column i is still kept. */
  has(i: number): boolean {
    return i >= 0 && i < this.end && i >= this.end - KEEP_COLUMNS;
  }

  subscribe = (l: Listener) => {
    this.listeners.add(l);
    return () => {
      this.listeners.delete(l);
    };
  };

  getInfo = () => this.info;
}

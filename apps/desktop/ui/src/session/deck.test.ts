import { describe, expect, it } from 'vitest';
import { beatInBar, beatsBetween, DeckFeed, deckWarning, KEEP_COLUMNS, phaseOf, type DeckInfo } from './deck';
import { SimDeck } from './simDeck';

const clock = (beat: number, bar: number | null = null, period = 480) => ({ period_ms: period, beat_ms: beat, bar_ms: bar });

describe('phase meter', () => {
  it('reads early and late within half a beat', () => {
    expect(phaseOf(clock(1012), clock(1000))?.ms).toBeCloseTo(12);
    expect(phaseOf(clock(990), clock(1000))?.ms).toBeCloseTo(-10);
    // A kick 470 ms late is really 10 ms early on the next beat.
    expect(phaseOf(clock(1470), clock(1000))?.ms).toBeCloseTo(-10);
    expect(phaseOf(clock(1240), clock(1000))?.beats).toBeCloseTo(0.5);
    expect(phaseOf(null, clock(0))).toBeNull();
  });

  it('counts bar offset only when both bars are known', () => {
    expect(phaseOf(clock(0, 0), clock(0, null))?.barBeats).toBeNull();
    expect(phaseOf(clock(0, 0), clock(0, 0))?.barBeats).toBe(0);
    // Your beat 1 lands on their beat 2.
    expect(phaseOf(clock(5, 485), clock(0, 0))?.barBeats).toBe(1);
  });

  it('lays out beats and bars', () => {
    const c = clock(0, 0, 500);
    expect(beatsBetween(c, 900, 2100).map((b) => [b.t, b.bar])).toEqual([
      [1000, false],
      [1500, false],
      [2000, true],
    ]);
    expect(beatInBar(c, 1600)).toBe(3);
    expect(beatInBar(c, 2000)).toBe(0);
    expect(beatInBar(clock(0), 100)).toBeNull();
  });
});

describe('DeckFeed', () => {
  it('keeps the newest minute of columns', () => {
    const f = new DeckFeed();
    f.push({ first: 0, cols: [{ you: [1, 1, 1], partner: [2, 2, 2] }] });
    f.push({ first: KEEP_COLUMNS + 5, cols: [{ you: [9, 9, 9], partner: [8, 8, 8] }] });
    expect(f.end).toBe(KEEP_COLUMNS + 6);
    expect(f.has(0)).toBe(false);
    expect(f.has(KEEP_COLUMNS + 5)).toBe(true);
    expect(f.you[5 * 3]).toBe(9);
  });

  it('follows the output clock without running ahead of the columns', () => {
    const f = new DeckFeed();
    f.push({ first: 0, cols: Array.from({ length: 200 }, () => ({ you: [0, 0, 0] as [number, number, number], partner: [0, 0, 0] as [number, number, number] })) });
    f.update({ nowMs: 1000, you: null, partner: null, partnerBpm: null, deck: null, onAir: false, partnerOnAir: true, fader: 1, partnerVolume: 1, ghostSays: null }, 50_000);
    expect(f.nowMs(50_000)).toBeCloseTo(1000);
    expect(f.nowMs(50_010)).toBeCloseTo(1010);
    expect(f.nowMs(60_000)).toBe(200 * 5 + 40);
  });
});

describe('demo deck', () => {
  it('SYNC lines your kick up with theirs', () => {
    const sim = new SimDeck(() => ({ you: false, partner: true }));
    sim.act({ kind: 'playPause' });
    const before = phaseOf(sim.feed.info!.you, sim.feed.info!.partner)!;
    expect(Math.abs(before.ms)).toBeGreaterThan(0);
    sim.act({ kind: 'sync' });
    const info = sim.feed.info!;
    expect(info.deck?.bpm).toBeCloseTo(124);
    expect(Math.abs(phaseOf(info.you, info.partner)!.ms)).toBeLessThan(1);
    sim.act({ kind: 'nudge', ms: 20 });
    expect(phaseOf(sim.feed.info!.you, sim.feed.info!.partner)!.ms).toBeCloseTo(-20, 0);
  });
});

describe('deckWarning', () => {
  const base: DeckInfo = {
    nowMs: 0,
    you: null,
    partner: null,
    partnerBpm: null,
    deck: { title: 'Built-in: Bells', playing: true, track_bpm: 125, bpm: 125, pitch_pct: 0, sync: false, sync_err_ms: null, pos_s: 0, len_s: 60 },
    onAir: false,
    partnerOnAir: true,
    fader: 1,
    partnerVolume: 1,
    ghostSays: null,
  };
  it('says nothing while the set is audible', () => {
    expect(deckWarning(base, 'Glizzy', 0)).toBeNull();
    expect(deckWarning({ ...base, fader: 0 }, 'Glizzy', 0)).toBeNull(); // cueing with the fader down is normal
  });
  it('flags a silent takeover: stopped deck, then fader down', () => {
    const onAir = { ...base, onAir: true, partnerOnAir: false };
    expect(deckWarning({ ...onAir, deck: { ...base.deck!, playing: false } }, 'Glizzy', 0)).toContain('Press P');
    expect(deckWarning({ ...onAir, fader: 0 }, 'Glizzy', 0)).toContain('Glizzy hears nothing from you');
  });
  it('flags an on-air partner who has been silent for 2 s', () => {
    expect(deckWarning(base, 'Glizzy', 1500)).toBeNull();
    expect(deckWarning(base, 'Glizzy', 2000)).toContain('Glizzy is on air but silent');
  });
});

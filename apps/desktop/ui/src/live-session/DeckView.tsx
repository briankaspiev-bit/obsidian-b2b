import { useEffect, useRef, useSyncExternalStore, type ReactNode, type WheelEvent } from 'react';
import { beatInBar, beatsBetween, COLUMN_MS, phaseOf, type BeatClock, type DeckAction, type DeckFeed, type DeckInfo } from '../session/deck';

/** How much time the waveforms show, centred on what you hear now. */
const SPAN_MS = 8000;
/** Within this, two kicks sound as one. */
const ON_BEAT_MS = 10;

type Role = 'onAir' | 'cue';

interface Props {
  feed: DeckFeed;
  partnerName: string;
  partnerRole: Role;
  youRole: Role;
  onAction: (a: DeckAction) => void;
}

function useDeckInfo(feed: DeckFeed): DeckInfo | null {
  return useSyncExternalStore(feed.subscribe, feed.getInfo);
}

function cssVar(el: Element, name: string, fallback: string) {
  return getComputedStyle(el).getPropertyValue(name).trim() || fallback;
}

/** Track position (ms, at its own speed) heard at output time t, from the deck's last report. */
function trackMsAt(info: DeckInfo, t: number): number | null {
  const d = info.deck;
  if (!d) return null;
  const pitch = 1 + d.pitch_pct / 100;
  return d.pos_s * 1000 + (t - info.nowMs) * pitch;
}

function drawLane(
  ctx: CanvasRenderingContext2D,
  feed: DeckFeed,
  info: DeckInfo | null,
  lane: 'you' | 'partner',
  top: number,
  h: number,
  w: number,
  now: number,
  color: string,
  light: string,
  faded: boolean,
) {
  const msPerPx = SPAN_MS / w;
  const mid = top + h / 2;
  const half = h / 2 - 3;
  const ring = lane === 'you' ? feed.you : feed.partner;
  const wave = lane === 'you' ? feed.wave : null;
  const len = wave ? wave.length / 3 : 0;
  const useWave = wave !== null && info?.deck != null;
  const bands = [0, 0, 0];
  ctx.globalAlpha = faded ? 0.45 : 1;
  for (let x = 0; x < w; x++) {
    const t0 = now + (x - w / 2) * msPerPx;
    const t1 = t0 + msPerPx;
    bands[0] = bands[1] = bands[2] = 0;
    if (useWave && info) {
      // Your deck: the track itself, behind and ahead of the play head.
      const a = trackMsAt(info, t0)!;
      const b = trackMsAt(info, t1)!;
      for (let i = Math.floor(Math.min(a, b) / COLUMN_MS); i <= Math.floor(Math.max(a, b) / COLUMN_MS); i++) {
        const k = (((i % len) + len) % len) * 3;
        bands[0] = Math.max(bands[0], wave![k]);
        bands[1] = Math.max(bands[1], wave![k + 1]);
        bands[2] = Math.max(bands[2], wave![k + 2]);
      }
    } else {
      // What you heard from them (or sent): nothing exists ahead of now.
      if (t0 > now) break;
      for (let i = Math.floor(t0 / COLUMN_MS); i <= Math.floor(Math.min(t1, now) / COLUMN_MS); i++) {
        if (!feed.has(i)) continue;
        const k = (i % (ring.length / 3)) * 3;
        bands[0] = Math.max(bands[0], ring[k]);
        bands[1] = Math.max(bands[1], ring[k + 1]);
        bands[2] = Math.max(bands[2], ring[k + 2]);
      }
    }
    const lo = (bands[0] / 255) * half;
    const md = (bands[1] / 255) * half * 0.85;
    const hi = (bands[2] / 255) * half * 0.6;
    if (lo > 0.5) {
      ctx.fillStyle = color;
      ctx.fillRect(x, mid - lo, 1, lo * 2);
    }
    if (md > 0.5) {
      ctx.fillStyle = light;
      ctx.fillRect(x, mid - md, 1, md * 2);
    }
    if (hi > 0.5) {
      ctx.fillStyle = 'rgba(255,255,255,0.85)';
      ctx.fillRect(x, mid - hi, 1, hi * 2);
    }
  }
  ctx.globalAlpha = 1;
}

function drawGrid(ctx: CanvasRenderingContext2D, clock: BeatClock | null, top: number, h: number, w: number, now: number, ahead: 'solid' | 'faint') {
  if (!clock) return;
  const msPerPx = SPAN_MS / w;
  const t0 = now - SPAN_MS / 2;
  for (const b of beatsBetween(clock, t0, t0 + SPAN_MS)) {
    const x = Math.round((b.t - t0) / msPerPx) + 0.5;
    const future = b.t > now;
    const a = (b.bar ? 0.9 : 0.4) * (future && ahead === 'faint' ? 0.5 : 1);
    ctx.strokeStyle = `rgba(255,255,255,${a})`;
    ctx.lineWidth = b.bar ? 2 : 1;
    ctx.setLineDash(future && ahead === 'faint' ? [3, 4] : []);
    ctx.beginPath();
    ctx.moveTo(x, top);
    ctx.lineTo(x, top + h);
    ctx.stroke();
    if (b.bar) {
      ctx.fillStyle = `rgba(255,255,255,${a})`;
      ctx.beginPath();
      ctx.moveTo(x - 5, top);
      ctx.lineTo(x + 5, top);
      ctx.lineTo(x, top + 6);
      ctx.fill();
    }
  }
  ctx.setLineDash([]);
}

/** Both decks scrolling past one play head: theirs on top, yours below. */
function Waveforms({ feed, partnerRole, youRole, onNudge }: { feed: DeckFeed; partnerRole: Role; youRole: Role; onNudge: (ms: number) => void }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const roles = useRef({ partnerRole, youRole });
  roles.current = { partnerRole, youRole };

  useEffect(() => {
    const c = canvas.current!;
    const ctx = c.getContext('2d');
    if (!ctx) return;
    let raf = 0;
    const colors = {
      onAir: cssVar(c, '--on-air', '#ff6a3d'),
      cue: cssVar(c, '--cue', '#6cc4ff'),
    };
    const light = { onAir: '#ffb08f', cue: '#bfe4ff' };
    const frame = () => {
      raf = requestAnimationFrame(frame);
      const dpr = window.devicePixelRatio || 1;
      const cw = c.clientWidth;
      const ch = c.clientHeight;
      if (c.width !== Math.round(cw * dpr) || c.height !== Math.round(ch * dpr)) {
        c.width = Math.round(cw * dpr);
        c.height = Math.round(ch * dpr);
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, cw, ch);
      const now = feed.nowMs(performance.now());
      const info = feed.info;
      const gap = 10;
      const laneH = (ch - gap) / 2;
      const { partnerRole: pr, youRole: yr } = roles.current;
      // Lanes
      ctx.fillStyle = 'rgba(255,255,255,0.025)';
      ctx.fillRect(0, 0, cw, laneH);
      ctx.fillRect(0, laneH + gap, cw, laneH);
      drawLane(ctx, feed, info, 'partner', 0, laneH, cw, now, colors[pr], light[pr], false);
      drawLane(ctx, feed, info, 'you', laneH + gap, laneH, cw, now, colors[yr], light[yr], (info?.fader ?? 1) < 0.05);
      drawGrid(ctx, info?.partner ?? null, 0, laneH, cw, now, 'faint');
      drawGrid(ctx, info?.you ?? null, laneH + gap, laneH, cw, now, 'solid');
      // Their side has no future yet: say so where the waveform stops.
      ctx.fillStyle = 'rgba(255,255,255,0.32)';
      ctx.font = '11px "JetBrains Mono", ui-monospace, monospace';
      ctx.textBaseline = 'middle';
      ctx.fillText('NOT HEARD YET', cw / 2 + 12, laneH / 2);
      // The play head: what you hear right now.
      ctx.fillStyle = '#fff';
      ctx.fillRect(Math.round(cw / 2) - 1, 0, 2, ch);
      ctx.beginPath();
      ctx.moveTo(cw / 2 - 6, 0);
      ctx.lineTo(cw / 2 + 6, 0);
      ctx.lineTo(cw / 2, 7);
      ctx.fill();
    };
    raf = requestAnimationFrame(frame);
    return () => cancelAnimationFrame(raf);
  }, [feed]);

  // Scroll over your waveform to nudge, like pushing a platter.
  const onWheel = (e: WheelEvent) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    if (e.clientY < r.top + r.height / 2) return;
    onNudge(e.deltaY > 0 ? -5 : 5);
  };

  return <canvas ref={canvas} className="deck__waves" onWheel={onWheel} aria-label="Waveforms: theirs above, yours below, with beat lines" role="img" />;
}

function BarDots({ clock, feed, label }: { clock: BeatClock | null; feed: DeckFeed; label: string }) {
  const dots = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    let raf = 0;
    const frame = () => {
      raf = requestAnimationFrame(frame);
      const el = dots.current;
      const info = feed.info;
      const c = label === 'you' ? info?.you : info?.partner;
      if (!el) return;
      const now = feed.nowMs(performance.now());
      const n = c ? beatInBar(c, now) : null;
      // A beat flashes for its first eighth.
      const sinceBeat = c ? (((now - c.beat_ms) % c.period_ms) + c.period_ms) % c.period_ms : Infinity;
      el.dataset.flash = sinceBeat < (c?.period_ms ?? 0) / 8 ? 'on' : 'off';
      for (let i = 0; i < 4; i++) {
        const d = el.children[i] as HTMLElement | undefined;
        if (d) d.dataset.lit = n === i ? 'on' : n === null && c ? 'unknown' : 'off';
      }
    };
    raf = requestAnimationFrame(frame);
    return () => cancelAnimationFrame(raf);
  }, [feed, label]);
  return (
    <span className="deck__bar" ref={dots} data-has={clock ? 'yes' : 'no'} aria-hidden="true">
      <i />
      <i />
      <i />
      <i />
    </span>
  );
}

function PhaseMeter({ info, feed, partnerName }: { info: DeckInfo | null; feed: DeckFeed; partnerName: string }) {
  const phase = phaseOf(info?.you ?? null, info?.partner ?? null);
  let words: string;
  let tone: 'ok' | 'off' | 'idle' = 'idle';
  if (!info?.partner) words = `Listening for ${partnerName}'s beat…`;
  else if (!info.you) words = 'Press PLAY to see your beat against theirs';
  else if (phase && Math.abs(phase.ms) <= ON_BEAT_MS) {
    words = 'Kicks together';
    tone = 'ok';
  } else if (phase) {
    words = `Your kick is ${Math.abs(phase.ms).toFixed(0)} ms ${phase.ms > 0 ? 'late' : 'early'}`;
    tone = 'off';
  } else words = '';
  const barNote =
    phase?.barBeats == null ? null : phase.barBeats === 0 ? 'Bars line up' : `Your bar starts ${phase.barBeats} beat${phase.barBeats > 1 ? 's' : ''} off theirs`;
  const x = phase ? 50 + phase.beats * 100 : 50;
  return (
    <div className="deck__phase" data-tone={tone}>
      <div className="deck__phase-rows">
        <span className="deck__phase-who">{partnerName.toUpperCase()}</span>
        <BarDots clock={info?.partner ?? null} feed={feed} label="partner" />
        <span className="deck__phase-who">YOU</span>
        <BarDots clock={info?.you ?? null} feed={feed} label="you" />
      </div>
      <div className="deck__ruler" role="meter" aria-label="Your kick against theirs" aria-valuemin={-50} aria-valuemax={50} aria-valuenow={phase ? Math.round(phase.beats * 100) : 0}>
        <span className="deck__ruler-label deck__ruler-label--l">EARLY</span>
        <span className="deck__ruler-zone" />
        <span className="deck__ruler-centre" />
        {phase && <span className="deck__ruler-mark" style={{ left: `${x}%` }} />}
        <span className="deck__ruler-label deck__ruler-label--r">LATE</span>
      </div>
      <div className="deck__phase-words" role="status" aria-live="polite">
        <span className="deck__phase-main">{words}</span>
        {barNote && <span className="deck__phase-sub">{barNote}</span>}
      </div>
    </div>
  );
}

function Key({ k }: { k: string }) {
  return <kbd className="deck__key">{k}</kbd>;
}

function DeckButton({ label, keyHint, onClick, active, title }: { label: ReactNode; keyHint: string; onClick: () => void; active?: boolean; title?: string }) {
  return (
    <button type="button" className="deck__btn" aria-pressed={active} onClick={onClick} title={title}>
      {label}
      <Key k={keyHint} />
    </button>
  );
}

function Slider({ label, value, max, onChange, keys }: { label: string; value: number; max: number; onChange: (v: number) => void; keys: string }) {
  return (
    <label className="deck__slider">
      <span className="deck__slider-label">
        {label} <Key k={keys} />
      </span>
      <input
        type="range"
        min={0}
        max={max}
        step={0.01}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        onPointerUp={(e) => e.currentTarget.blur()}
      />
      <span className="deck__slider-value">{Math.round(value * 100)}%</span>
    </label>
  );
}

const fmtBpm = (v: number | null | undefined) => (v ? v.toFixed(1) : '--.-');
const fmtTime = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, '0')}`;

/**
 * The DJ view: both decks' waveforms with beat grids on one play head, a phase
 * meter for your kick against theirs, and your deck's controls.
 */
export function DeckView({ feed, partnerName, partnerRole, youRole, onAction }: Props) {
  const info = useDeckInfo(feed);
  const deck = info?.deck ?? null;
  const partnerBpm = info?.partner ? 60_000 / info.partner.period_ms : info?.partnerBpm;
  const youBpm = deck?.bpm ?? (info?.you ? 60_000 / info.you.period_ms : null);

  return (
    <section className="deck" aria-label="DJ view">
      <div className="deck__labels">
        <div className="deck__label" data-role={partnerRole}>
          <span className="deck__who">{partnerName}</span>
          <span className="deck__bpm">
            {fmtBpm(partnerBpm)} <small>BPM</small>
          </span>
          {info?.partnerOnAir && <span className="deck__air">ON AIR</span>}
          {info?.ghostSays && <span className="deck__note">{info.ghostSays}</span>}
        </div>
        <div className="deck__label" data-role={youRole}>
          <span className="deck__who">YOU{deck ? ` · ${deck.title}` : ''}</span>
          <span className="deck__bpm">
            {fmtBpm(youBpm)} <small>BPM</small>
          </span>
          {deck && (
            <span className="deck__meta">
              {deck.pitch_pct >= 0 ? '+' : ''}
              {deck.pitch_pct.toFixed(1)}% · {fmtTime(deck.pos_s)} / {fmtTime(deck.len_s)}
            </span>
          )}
          {deck?.sync && <span className="deck__sync">SYNC</span>}
          {info?.onAir && <span className="deck__air">ON AIR</span>}
        </div>
      </div>

      <Waveforms feed={feed} partnerRole={partnerRole} youRole={youRole} onNudge={(ms) => onAction({ kind: 'nudge', ms })} />

      <PhaseMeter info={info} feed={feed} partnerName={partnerName} />

      <div className="deck__controls">
        {deck && (
          <>
            <div className="deck__group">
              <DeckButton label="CUE" keyHint="C" onClick={() => onAction({ kind: 'cue' })} />
              <DeckButton label={deck.playing ? 'PAUSE' : 'PLAY'} keyHint="P" active={deck.playing} onClick={() => onAction({ kind: 'playPause' })} />
              <DeckButton label="SYNC" keyHint="S" active={deck.sync} onClick={() => onAction({ kind: 'sync' })} title="Match their tempo and land on their beat" />
            </div>
            <div className="deck__group" aria-label="Nudge">
              <DeckButton label="◀ SLOWER" keyHint="," onClick={() => onAction({ kind: 'nudge', ms: -10 })} title="Push your beat 10 ms later (Shift: 50 ms)" />
              <DeckButton label="FASTER ▶" keyHint="." onClick={() => onAction({ kind: 'nudge', ms: 10 })} title="Pull your beat 10 ms earlier (Shift: 50 ms)" />
            </div>
            <div className="deck__group" aria-label="Pitch">
              <DeckButton label="PITCH −" keyHint="-" onClick={() => onAction({ kind: 'pitch', pct: -0.1 })} />
              <DeckButton label="PITCH +" keyHint="=" onClick={() => onAction({ kind: 'pitch', pct: 0.1 })} />
            </div>
          </>
        )}
        <div className="deck__group deck__group--sliders">
          <Slider label="YOUR FADER" keys="↑↓" value={info?.fader ?? 1} max={1} onChange={(value) => onAction({ kind: 'fader', value })} />
          <Slider label={`${partnerName.toUpperCase()} IN YOUR EARS`} keys="←→" value={info?.partnerVolume ?? 1} max={2} onChange={(value) => onAction({ kind: 'partnerVolume', value })} />
        </div>
        {info?.ghostSays && (
          <div className="deck__group">
            <DeckButton label="BRING GHOST BACK" keyHint="G" onClick={() => onAction({ kind: 'ghostComeBack' })} />
          </div>
        )}
        <p className="deck__hint">
          <Key k="SPACE" /> take over · <Key k="R" /> ready · scroll on your waveform to nudge
        </p>
      </div>
    </section>
  );
}

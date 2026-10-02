import { useEffect, useRef, useState } from 'react';
import type { StereoLevel } from '../session/types';

const SEGMENTS = 28;
const FLOOR_DB = -48;
const HOT_DB = -6; // segments above this light amber, like a real meter
const RELEASE_DB_PER_S = 24;
const PEAK_HOLD_MS = 900;

interface Props {
  title: string;
  getLevel: () => StereoLevel;
  /** Shown when the meter reads silence and silence is expected. */
  quietText: string;
  /** Shown when the meter reads silence but audio is expected. */
  noSignalText: string;
  /** True when this side should be sending audio (on air or handing off). */
  expectingAudio: boolean;
  /** Set when the link to this source is down; overrides the readout. */
  recoveringText?: string | null;
  /** Set while this meter carries the live mix; tints it orange and says so. */
  liveText?: string | null;
}

function dbToSegments(db: number) {
  if (!Number.isFinite(db) || db <= FLOOR_DB) return 0;
  return Math.min(SEGMENTS, Math.round(((db - FLOOR_DB) / -FLOOR_DB) * SEGMENTS));
}

const HOT_FROM = dbToSegments(HOT_DB);

/**
 * Compact segmented stereo meter. Polls a level feed on animation frames and
 * applies meter ballistics (instant attack, steady release, short peak hold).
 * Used twice: what you send (YOUR SEND) and what arrives (REMOTE LEVEL).
 */
export function LevelMeter({ title, getLevel, quietText, noSignalText, expectingAudio, recoveringText = null, liveText = null }: Props) {
  const shown = useRef({ l: -Infinity, r: -Infinity, peakL: -Infinity, peakR: -Infinity, peakAt: 0 });
  const [lit, setLit] = useState({ l: 0, r: 0, pl: 0, pr: 0, db: -Infinity });

  useEffect(() => {
    let raf = 0;
    let last = performance.now();
    const frame = (t: number) => {
      const dt = (t - last) / 1000;
      last = t;
      const target = getLevel();
      const s = shown.current;
      const ballistic = (cur: number, next: number) => {
        if (next >= cur || !Number.isFinite(cur)) return next;
        const fallen = cur - RELEASE_DB_PER_S * dt;
        return fallen < FLOOR_DB ? -Infinity : Math.max(next, fallen);
      };
      s.l = ballistic(s.l, target.left);
      s.r = ballistic(s.r, target.right);
      if (s.l > s.peakL || s.r > s.peakR || t - s.peakAt > PEAK_HOLD_MS) {
        s.peakL = s.l;
        s.peakR = s.r;
        s.peakAt = t;
      }
      const next = {
        l: dbToSegments(s.l),
        r: dbToSegments(s.r),
        pl: dbToSegments(s.peakL),
        pr: dbToSegments(s.peakR),
        db: Math.max(s.l, s.r),
      };
      setLit((prev) =>
        prev.l === next.l && prev.r === next.r && prev.pl === next.pl && prev.pr === next.pr &&
        Math.round(prev.db) === Math.round(next.db)
          ? prev
          : next,
      );
      raf = requestAnimationFrame(frame);
    };
    raf = requestAnimationFrame(frame);
    return () => cancelAnimationFrame(raf);
  }, [getLevel]);

  let readout: string;
  let readoutKind: 'recovering' | 'quiet' | 'noSignal' | 'level' | 'live';
  const recovering = recoveringText !== null;
  if (recovering) {
    readout = recoveringText;
    readoutKind = 'recovering';
  } else if (lit.l === 0 && lit.r === 0) {
    readout = expectingAudio ? noSignalText : quietText;
    readoutKind = expectingAudio ? 'noSignal' : 'quiet';
  } else {
    readout = `${Math.round(lit.db)} dB`;
    readoutKind = liveText ? 'live' : 'level';
  }

  return (
    <section className="meter" aria-label={title} data-reconnecting={recovering || undefined} data-readout={readoutKind} data-live={liveText ? 'true' : undefined}>
      <div className="meter__head">
        <span className="meter__title">{title}</span>
        <span key={readoutKind} className="meter__readout fade-in" aria-live={readoutKind === 'level' || readoutKind === 'live' ? 'off' : 'polite'}>
          {readoutKind === 'live' && (
            <span className="meter__live">
              <span className="meter__live-led" aria-hidden="true" />
              {liveText}
            </span>
          )}
          {readout}
        </span>
      </div>
      {(['L', 'R'] as const).map((ch) => {
        const n = ch === 'L' ? lit.l : lit.r;
        const peak = ch === 'L' ? lit.pl : lit.pr;
        return (
          <div key={ch} className="meter__row">
            <span className="meter__ch">{ch}</span>
            <span className="meter__bar">
              {Array.from({ length: SEGMENTS }, (_, i) => (
                <span
                  key={i}
                  className="meter__seg"
                  data-on={i < n || undefined}
                  data-peak={(peak > 0 && i === peak - 1) || undefined}
                  data-hot={i >= HOT_FROM || undefined}
                />
              ))}
            </span>
          </div>
        );
      })}
      <div className="meter__scale" aria-hidden="true">
        <span>&minus;48</span>
        <span>&minus;24</span>
        <span>&minus;12</span>
        <span>&minus;6</span>
        <span>0</span>
      </div>
    </section>
  );
}

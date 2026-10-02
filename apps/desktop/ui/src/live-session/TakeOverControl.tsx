import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from 'react';
import type { BoothRole } from '../session/types';
import { formatClock } from '../lib/format';

/** How long the DJ holds TAKE OVER before it fires. Deliberate, not slow. */
const HOLD_MS = 650;
const RING_R = 46;
const RING_C = 2 * Math.PI * RING_R;

interface Props {
  localRole: BoothRole;
  remoteName: string;
  remoteCity: string;
  remoteReady: boolean;
  /** Who is receiving the mix while a handoff runs. */
  handoffTo: 'local' | 'remote' | null;
  enabled: boolean;
  /** The live DJ dropped: one tap takes the mix, no hold. */
  emergency?: boolean;
  /** When the local DJ went on air; shown as a running clock on the button. */
  onAirSinceMs?: number | null;
  onTakeOver: () => void;
}

export function TakeOverControl({ localRole, remoteName, remoteCity, remoteReady, handoffTo, enabled, emergency = false, onAirSinceMs = null, onTakeOver }: Props) {
  const [progress, setProgress] = useState(0);
  const raf = useRef<number | null>(null);
  const startedAt = useRef(0);

  const cancel = useCallback(() => {
    if (raf.current !== null) cancelAnimationFrame(raf.current);
    raf.current = null;
    setProgress(0);
  }, []);

  const start = useCallback(() => {
    if (!enabled || raf.current !== null) return;
    if (emergency) {
      // No time to hold: the mix has nobody on it.
      onTakeOver();
      return;
    }
    startedAt.current = performance.now();
    const step = (t: number) => {
      const p = Math.min(1, (t - startedAt.current) / HOLD_MS);
      setProgress(p);
      if (p >= 1) {
        raf.current = null;
        setProgress(0);
        onTakeOver();
        return;
      }
      raf.current = requestAnimationFrame(step);
    };
    raf.current = requestAnimationFrame(step);
  }, [enabled, emergency, onTakeOver]);

  useEffect(() => cancel, [cancel]);
  useEffect(() => {
    if (!enabled) cancel();
  }, [enabled, cancel]);

  const onKeyDown = (e: KeyboardEvent) => {
    if ((e.key === ' ' || e.key === 'Enter') && !e.repeat) {
      e.preventDefault();
      start();
    }
  };
  const onKeyUp = (e: KeyboardEvent) => {
    if (e.key === ' ' || e.key === 'Enter') cancel();
  };

  let mode: 'take' | 'onAir' | 'handoff' | 'emergency' = 'take';
  if (handoffTo) mode = 'handoff';
  else if (emergency && enabled) mode = 'emergency';
  else if (localRole === 'onAir') mode = 'onAir';

  const caption =
    mode === 'onAir'
      ? remoteReady
        ? `${remoteName} is ready to take over`
        : `${remoteName} takes over from ${remoteCity}`
      : mode === 'emergency'
        ? 'Tap once. No hold needed'
        : mode === 'handoff'
        ? handoffTo === 'local'
          ? 'Passing the decks to you'
          : `Passing the decks to ${remoteName}`
        : 'Press and hold to take the mix';

  return (
    <div className="takeover" data-mode={mode} data-remote-ready={(mode === 'onAir' && remoteReady) || undefined}>
      <button
        type="button"
        className="takeover__button"
        disabled={!enabled}
        aria-describedby="takeover-caption"
        aria-label={
          mode === 'take'
            ? 'Take over the mix. Press and hold.'
            : mode === 'emergency'
              ? 'Take over the mix now. Tap once.'
              : undefined
        }
        onPointerDown={(e) => {
          e.currentTarget.setPointerCapture(e.pointerId);
          start();
        }}
        onPointerUp={cancel}
        onPointerCancel={cancel}
        onLostPointerCapture={cancel}
        onKeyDown={onKeyDown}
        onKeyUp={onKeyUp}
        onBlur={cancel}
        style={{ ['--hold' as string]: progress }}
        data-holding={progress > 0 || undefined}
      >
        <svg className="takeover__ring" viewBox="0 0 100 100" aria-hidden="true">
          <circle className="takeover__ring-track" cx="50" cy="50" r={RING_R} />
          <circle
            className="takeover__ring-fill"
            cx="50"
            cy="50"
            r={RING_R}
            strokeDasharray={RING_C}
            strokeDashoffset={RING_C * (1 - progress)}
          />
        </svg>
        <span className="takeover__face">
          <span key={mode} className="takeover__label fade-in">
            {mode === 'take' ? 'TAKE OVER' : mode === 'emergency' ? (
              <>
                TAKE OVER
                <br />
                NOW
              </>
            ) : mode === 'onAir' ? 'ON AIR' : 'HANDOFF'}
          </span>
          {mode === 'onAir' && onAirSinceMs !== null && <OnAirClock sinceMs={onAirSinceMs} />}
        </span>
      </button>

      <p id="takeover-caption" className="takeover__caption">
        <span key={caption} className="fade-in">
          {caption}
        </span>
      </p>
    </div>
  );
}

/** Time since this DJ took the mix, ticking once a second. */
function OnAirClock({ sinceMs }: { sinceMs: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);
  const text = formatClock(now - sinceMs);
  return (
    <span className="takeover__clock" aria-label={`On air for ${text}`}>
      {text}
    </span>
  );
}

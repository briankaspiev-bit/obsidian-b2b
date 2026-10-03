import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from 'react';
import type { AskState, BoothRole } from '../session/types';
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
  /** Asking for the booth, in a live room. */
  ask?: AskState;
  /** On air: answer the other DJ's ask. */
  onAnswer?: (grant: boolean) => void;
  /** Off air: take back your ask. */
  onCancelAsk?: () => void;
}

export function TakeOverControl({
  localRole,
  remoteName,
  remoteCity,
  remoteReady,
  handoffTo,
  enabled,
  emergency = false,
  onAirSinceMs = null,
  onTakeOver,
  ask,
  onAnswer,
  onCancelAsk,
}: Props) {
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

  let mode: 'take' | 'onAir' | 'handoff' | 'emergency' | 'asking' | 'asked' = 'take';
  if (handoffTo) mode = 'handoff';
  else if (emergency && enabled) mode = 'emergency';
  else if (localRole === 'onAir') mode = ask?.theirsSecsLeft != null && onAnswer ? 'asked' : 'onAir';
  else if (ask?.mineSecsLeft != null) mode = 'asking';

  // Answering or taking back an ask is one tap; only asking needs the hold.
  const press = () => {
    if (mode === 'asked') onAnswer?.(true);
    else if (mode === 'asking') onCancelAsk?.();
    else start();
  };
  const active = enabled || mode === 'asked' || (mode === 'asking' && !!onCancelAsk);

  const onKeyDown = (e: KeyboardEvent) => {
    if ((e.key === ' ' || e.key === 'Enter') && !e.repeat) {
      e.preventDefault();
      press();
    }
  };
  const onKeyUp = (e: KeyboardEvent) => {
    if (e.key === ' ' || e.key === 'Enter') cancel();
  };

  const caption =
    mode === 'asked'
      ? `${remoteName} asks for the booth. Goes through in ${ask?.theirsSecsLeft}s`
      : mode === 'asking'
        ? `Asked ${remoteName}. On air in ${ask?.mineSecsLeft}s unless they say not yet. Tap to take it back`
        : mode === 'take' && ask?.denied
          ? `${remoteName} said not yet. Hold to ask again`
          : mode === 'onAir'
      ? remoteReady
        ? `${remoteName} is ready to take over`
        : `${remoteName} takes over from ${remoteCity}`
      : mode === 'emergency'
        ? 'Tap once. No hold needed'
        : mode === 'handoff'
        ? handoffTo === 'local'
          ? 'Passing the decks to you'
          : `Passing the decks to ${remoteName}`
        : 'Press and hold to ask for the booth';

  return (
    <div className="takeover" data-mode={mode} data-remote-ready={(mode === 'onAir' && remoteReady) || undefined}>
      <button
        type="button"
        className="takeover__button"
        disabled={!active}
        aria-describedby="takeover-caption"
        aria-label={
          mode === 'take'
            ? 'Ask for the booth. Press and hold.'
            : mode === 'emergency'
              ? 'Take over the mix now. Tap once.'
              : mode === 'asked'
                ? `Let ${remoteName} take over now.`
                : mode === 'asking'
                  ? 'Take back your ask.'
                  : undefined
        }
        onPointerDown={(e) => {
          e.currentTarget.setPointerCapture(e.pointerId);
          press();
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
            ) : mode === 'onAir' ? 'ON AIR' : mode === 'asked' ? (
              <>
                LET THEM
                <br />
                IN
              </>
            ) : mode === 'asking' ? 'ASKING' : 'HANDOFF'}
          </span>
          {mode === 'onAir' && onAirSinceMs !== null && <OnAirClock sinceMs={onAirSinceMs} />}
          {mode === 'asked' && <span className="takeover__clock">{ask?.theirsSecsLeft}s</span>}
          {mode === 'asking' && <span className="takeover__clock">{ask?.mineSecsLeft}s</span>}
        </span>
      </button>
      {mode === 'asked' && onAnswer && (
        <button type="button" className="takeover__not-yet" onClick={() => onAnswer(false)}>
          NOT YET <kbd>N</kbd>
        </button>
      )}

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

import { useEffect, useRef, useState } from 'react';

/** Easy to find, hard to hit by accident: a quiet button and a confirm step. */
export function EndSessionControl({ remoteName, onEnd }: { remoteName: string; onEnd: () => void }) {
  const [confirming, setConfirming] = useState(false);
  const keepRef = useRef<HTMLButtonElement>(null);
  const openerRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!confirming) return;
    keepRef.current?.focus();
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === 'Escape') {
        setConfirming(false);
        openerRef.current?.focus();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [confirming]);

  return (
    <div className="end">
      <button
        ref={openerRef}
        type="button"
        className="end__button"
        aria-haspopup="dialog"
        aria-expanded={confirming}
        onClick={() => setConfirming((v) => !v)}
      >
        End Session
      </button>
      {confirming && (
        <div className="end__confirm fade-in" role="dialog" aria-modal="false" aria-labelledby="end-title">
          <p id="end-title" className="end__title">
            End the session for you and {remoteName}?
          </p>
          <p className="end__body">Recording stops and the master mix starts building.</p>
          <div className="end__actions">
            <button
              ref={keepRef}
              type="button"
              className="end__keep"
              onClick={() => {
                setConfirming(false);
                openerRef.current?.focus();
              }}
            >
              Keep playing
            </button>
            <button type="button" className="end__confirm-btn" onClick={onEnd}>
              End session
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

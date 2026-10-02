import { useEffect, useState } from 'react';
import { formatElapsed } from '../lib/format';

interface Props {
  startedAtMs: number;
  endedAtMs: number | null;
}

export function SessionTimer({ startedAtMs, endedAtMs }: Props) {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (endedAtMs !== null) return;
    const id = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(id);
  }, [endedAtMs]);

  const elapsed = (endedAtMs ?? now) - startedAtMs;

  return (
    <div className="ls-timer">
      <div className="ls-timer__phrase">TWO CITIES. ONE BOOTH.</div>
      <div className="ls-timer__value" role="timer" aria-label={`Session time ${formatElapsed(elapsed)}`}>
        {formatElapsed(elapsed)}
      </div>
    </div>
  );
}

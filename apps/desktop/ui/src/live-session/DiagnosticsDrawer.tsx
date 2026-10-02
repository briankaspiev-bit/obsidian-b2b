import type { Diagnostics } from '../session/types';

interface Props {
  open: boolean;
  onToggle: () => void;
  diagnostics: Diagnostics;
  connected: boolean;
}

/** Closed by default. The only place technical numbers appear. */
export function DiagnosticsDrawer({ open, onToggle, diagnostics: d, connected }: Props) {
  const dash = '—';
  const rows: [string, string][] = [
    ['Round trip', connected ? `${d.roundTripMs} ms` : dash],
    ['One-way delay', connected ? `${d.oneWayMs} ms` : dash],
    ['Jitter', connected ? `${d.jitterMs.toFixed(1)} ms` : dash],
    ['Packet loss', connected ? `${d.packetLossPct.toFixed(2)} %` : dash],
    ['Buffer depth', `${d.bufferMs} ms`],
    ['Clock drift', `${d.clockDriftPpm} ppm`],
    ['Path', d.path === 'direct' ? 'Direct' : 'Relay'],
    ['Codec', d.codec],
  ];

  return (
    <div className="diag" data-open={open || undefined}>
      <button type="button" className="diag__tab" aria-expanded={open} aria-controls="diag-panel" onClick={onToggle}>
        Diagnostics
        <svg width="12" height="12" viewBox="0 0 24 24" aria-hidden="true" className="diag__chev">
          <path d="m6 15 6-6 6 6" stroke="currentColor" strokeWidth="2" fill="none" strokeLinecap="round" />
        </svg>
      </button>
      <div id="diag-panel" className="diag__panel" hidden={!open}>
        <dl className="diag__grid">
          {rows.map(([k, v]) => (
            <div key={k} className="diag__cell">
              <dt>{k}</dt>
              <dd>{v}</dd>
            </div>
          ))}
        </dl>
      </div>
    </div>
  );
}

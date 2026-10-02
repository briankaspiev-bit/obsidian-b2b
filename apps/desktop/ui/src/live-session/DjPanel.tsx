import { formatRemaining } from '../lib/format';
import type { BoothRole, Dj } from '../session/types';
import { StatePill } from './StatePill';

interface Props {
  dj: Dj;
  role: BoothRole;
  side: 'left' | 'right';
  handoffDirection?: 'incoming' | 'outgoing';
  /** Remote side only: the connection dropped. Local side never dims. */
  reconnecting?: boolean;
  /** Local side only: lets this DJ tell the other booth they're ready. */
  onToggleReady?: () => void;
}

export function DjPanel({ dj, role, side, handoffDirection, reconnecting = false, onToggleReady }: Props) {
  const canSignalReady = onToggleReady && (role === 'cueing' || role === 'ready');
  const label = dj.isLocal ? 'LOCAL DJ' : 'REMOTE DJ';
  return (
    <section
      className="dj-panel"
      data-role={role}
      data-side={side}
      data-handoff={handoffDirection}
      data-reconnecting={reconnecting || undefined}
      aria-label={`${label}: ${dj.name}`}
    >
      <div className="dj-panel__eyebrow">{label}</div>
      <div className="dj-panel__plate">
        <span className="dj-panel__lightbar" aria-hidden="true" />
        {dj.photoUrl && (
          // Only the DJ who owns the mix shows their photo; CSS fades it with the handoff.
          <div className="dj-panel__photo" aria-hidden="true">
            <img src={dj.photoUrl} alt="" draggable={false} />
          </div>
        )}
        <div className="dj-panel__who">
          <h2 className="dj-panel__name">{dj.name}</h2>
          <div className="dj-panel__city">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true">
              <path d="M12 21s7-6.1 7-11.5A7 7 0 0 0 5 9.5C5 14.9 12 21 12 21z" stroke="currentColor" strokeWidth="1.7" />
              <circle cx="12" cy="9.5" r="2.4" stroke="currentColor" strokeWidth="1.7" />
            </svg>
            {dj.city}
          </div>
        </div>

        <div className="dj-panel__state">
          <StatePill role={role} handoffDirection={handoffDirection} />
          {canSignalReady && (
            <button type="button" className="ready-toggle" aria-pressed={role === 'ready'} onClick={onToggleReady}>
              <span className="ready-toggle__led" aria-hidden="true" />
              {role === 'ready' ? 'CANCEL' : 'MARK READY'}
            </button>
          )}
          {reconnecting && (
            <span key="reconnecting" className="dj-panel__notice fade-in" role="status">
              {dj.name} reconnecting&hellip;
            </span>
          )}
        </div>

        {dj.track && (
          <div className="dj-panel__track">
            <div className="dj-panel__track-text">
              <div className="dj-panel__track-title">{dj.track.title}</div>
              <div className="dj-panel__track-artist">{dj.track.artist}</div>
            </div>
            {dj.track.remainingSec !== null && (
              <div className="dj-panel__track-time" aria-label="Time left in track">
                {formatRemaining(dj.track.remainingSec)}
              </div>
            )}
          </div>
        )}
      </div>
    </section>
  );
}

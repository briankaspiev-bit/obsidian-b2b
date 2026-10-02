import type { CSSProperties } from 'react';

type Side = 'left' | 'right';
type EnergyState = 'on' | 'off' | 'in' | 'out';

interface Props {
  ownerSide: Side;
  handoff: { from: Side; to: Side; durationMs: number; key: number } | null;
  readySide: Side | null;
}

/**
 * The line joining the two DJs. Orange energy sits on the owner's half.
 * During a handoff it drains from the outgoing side, a pulse crosses the
 * booth, and it fills the incoming side.
 */
export function HandoffRail({ ownerSide, handoff, readySide }: Props) {
  const energy = (side: Side): EnergyState => {
    if (handoff) return handoff.from === side ? 'out' : 'in';
    return ownerSide === side ? 'on' : 'off';
  };

  return (
    <div
      className="rail"
      aria-hidden="true"
      data-handoff={handoff ? 'true' : undefined}
      style={handoff ? ({ '--handoff-ms': `${handoff.durationMs}ms` } as CSSProperties) : undefined}
    >
      <span className="rail__line" />
      <span className="rail__cue rail__cue--left" data-on={readySide === 'left' || undefined} />
      <span className="rail__cue rail__cue--right" data-on={readySide === 'right' || undefined} />
      {readySide && (
        <span className="rail__chevrons" data-side={readySide}>
          <span />
          <span />
          <span />
        </span>
      )}
      <span className="rail__energy rail__energy--left" data-state={energy('left')} />
      <span className="rail__energy rail__energy--right" data-state={energy('right')} />
      {handoff && (
        <span key={handoff.key} className="rail__packet" data-dir={handoff.from === 'left' ? 'ltr' : 'rtl'} />
      )}
      <span
        className="rail__node"
        data-ready={readySide ? 'true' : undefined}
        data-handoff={handoff ? 'true' : undefined}
      />
    </div>
  );
}

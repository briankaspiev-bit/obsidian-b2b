import type { BoothRole } from '../session/types';
import { ROLE_LABEL } from './labels';

interface Props {
  role: BoothRole;
  /** During HANDOFF: is this DJ receiving the mix or giving it away? */
  handoffDirection?: 'incoming' | 'outgoing';
}

export function StatePill({ role, handoffDirection }: Props) {
  return (
    <span className="state-pill" data-role={role} data-handoff={handoffDirection}>
      <span className="state-pill__led" aria-hidden="true" />
      {/* keyed so the word fades in rather than jumping */}
      <span key={role} className="state-pill__text fade-in">
        {ROLE_LABEL[role]}
      </span>
    </span>
  );
}

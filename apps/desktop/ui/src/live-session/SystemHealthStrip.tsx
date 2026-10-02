import type { ReactNode } from 'react';
import type { LinkState } from '../session/types';
import { BOOTH_SYNC, NETWORK, RECORDING, remoteLabel, type Tone } from './labels';

interface Item {
  id: string;
  label: string;
  text: string;
  tone: Tone;
  icon: ReactNode;
}

const stroke = { stroke: 'currentColor', strokeWidth: 1.7, fill: 'none', strokeLinecap: 'round' as const };

const ICONS = {
  sync: (
    <svg width="22" height="22" viewBox="0 0 24 24" aria-hidden="true">
      <circle cx="9" cy="12" r="5.5" {...stroke} />
      <circle cx="15" cy="12" r="5.5" {...stroke} />
    </svg>
  ),
  network: (
    <svg width="22" height="22" viewBox="0 0 24 24" aria-hidden="true">
      <path d="M5 19v-3M10 19v-6M15 19v-9M20 19V6" {...stroke} />
    </svg>
  ),
  remote: (
    <svg width="22" height="22" viewBox="0 0 24 24" aria-hidden="true">
      <circle cx="9" cy="8" r="3.2" {...stroke} />
      <path d="M3.5 19a5.5 5.5 0 0 1 11 0M16 5.2a3.2 3.2 0 0 1 0 5.6M18 14.5a5.5 5.5 0 0 1 2.5 4.5" {...stroke} />
    </svg>
  ),
  rec: (
    <svg width="22" height="22" viewBox="0 0 24 24" aria-hidden="true">
      <circle cx="12" cy="12" r="8" {...stroke} />
      <circle className="health__rec-dot" cx="12" cy="12" r="3.6" />
    </svg>
  ),
};

/**
 * Four equipment-style indicators. When all is well they sit back in grey;
 * only a change of state brings a value forward.
 */
export function SystemHealthStrip({
  link,
  remoteName,
  remoteWasLive = false,
}: {
  link: LinkState;
  remoteName: string;
  /** The DJ who dropped owned the mix, so "your mix is unaffected" would be wrong. */
  remoteWasLive?: boolean;
}) {
  const items: Item[] = [
    { id: 'sync', label: 'Booth Sync', ...BOOTH_SYNC[link.boothSync], icon: ICONS.sync },
    { id: 'network', label: 'Network', ...NETWORK[link.network], icon: ICONS.network },
    { id: 'remote', label: 'Remote DJ', ...remoteLabel(link.remote), icon: ICONS.remote },
    { id: 'rec', label: 'Recording', ...RECORDING[link.recording], icon: ICONS.rec },
  ];

  return (
    <ul className="health" aria-label="Booth status">
      {items.map((it) => (
        <li key={it.id} className="health__item" data-tone={it.tone}>
          <span className="health__icon">{it.icon}</span>
          <span className="health__text">
            <span className="health__label">{it.label}</span>
            <span key={it.text} className="health__value fade-in">
              <span className="health__led" aria-hidden="true" />
              {it.text}
            </span>
          </span>
        </li>
      ))}
      {link.remote === 'reconnecting' && (
        <li className="health__reassure fade-in" role="status">
          {remoteWasLive
            ? `${remoteName}’s own room and recording keep going. You can pick up the mix.`
            : `Your mix is unaffected. Only ${remoteName}’s audio is recovering.`}
        </li>
      )}
    </ul>
  );
}

// Every user-facing word for states lives here, so the screen never leaks
// networking terms. Technical numbers belong to the Diagnostics drawer only.

import type { BoothRole, BoothSync, NetworkQuality, RecordingState, RemoteConnection } from '../session/types';

export type Tone = 'ok' | 'attention' | 'muted' | 'rec';

export const ROLE_LABEL: Record<BoothRole, string> = {
  onAir: 'ON AIR',
  cueing: 'CUEING',
  ready: 'READY',
  handoff: 'HANDOFF',
};

export const BOOTH_SYNC: Record<BoothSync, { text: string; tone: Tone }> = {
  stable: { text: 'Stable', tone: 'ok' },
  adjusting: { text: 'Adjusting', tone: 'attention' },
  unstable: { text: 'Unstable', tone: 'attention' },
  lost: { text: 'Lost', tone: 'attention' },
};

export const NETWORK: Record<NetworkQuality, { text: string; tone: Tone }> = {
  excellent: { text: 'Excellent', tone: 'ok' },
  good: { text: 'Good', tone: 'ok' },
  fair: { text: 'Fair', tone: 'attention' },
  poor: { text: 'Poor', tone: 'attention' },
  recovering: { text: 'Recovering', tone: 'attention' },
};

export function remoteLabel(state: RemoteConnection): { text: string; tone: Tone } {
  switch (state) {
    case 'connected':
      return { text: 'Connected', tone: 'ok' };
    case 'reconnecting':
      return { text: 'Reconnecting…', tone: 'attention' };
    case 'left':
      return { text: 'Left the booth', tone: 'muted' };
  }
}

export const RECORDING: Record<RecordingState, { text: string; tone: Tone }> = {
  on: { text: 'On', tone: 'rec' },
  off: { text: 'Off', tone: 'muted' },
};

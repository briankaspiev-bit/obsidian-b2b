import { createContext, useContext, useSyncExternalStore } from 'react';
import type { SessionEngine } from './engine';

export const SessionEngineContext = createContext<SessionEngine | null>(null);

export function useEngine(): SessionEngine {
  const engine = useContext(SessionEngineContext);
  if (!engine) throw new Error('useEngine must be used inside SessionEngineContext');
  return engine;
}

export function useRoomState() {
  const e = useEngine();
  return useSyncExternalStore(e.subscribeRoom, e.getRoom);
}

export function useSessionState() {
  const e = useEngine();
  return useSyncExternalStore(e.subscribeSession, e.getSession);
}

export function useLinkState() {
  const e = useEngine();
  return useSyncExternalStore(e.subscribeLink, e.getLink);
}

export function useRemoteLevel() {
  const e = useEngine();
  return useSyncExternalStore(e.subscribeRemoteLevel, e.getRemoteLevel);
}

export function useLocalLevel() {
  const e = useEngine();
  return useSyncExternalStore(e.subscribeLocalLevel, e.getLocalLevel);
}

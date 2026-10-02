import { createContext, useContext } from 'react';

export type EngineMode = 'real' | 'demo';

export interface EngineModeValue {
  mode: EngineMode;
  /** False in the browser preview, where only the demo exists. */
  canSwitch: boolean;
  setMode(mode: EngineMode): void;
}

export const EngineModeContext = createContext<EngineModeValue>({ mode: 'demo', canSwitch: false, setMode: () => {} });

export function useEngineMode() {
  return useContext(EngineModeContext);
}

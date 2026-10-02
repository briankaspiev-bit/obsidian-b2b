import { useMemo, useState } from 'react';
import { MockControls } from '../live-session/MockControls';
import type { EngineInfo } from '../session/bridge';
import { MockSessionEngine } from '../session/mockEngine';
import { TauriSessionEngine } from '../session/tauriEngine';
import { SessionEngineContext } from '../session/useSession';
import { App } from './App';
import { EngineModeContext, type EngineMode } from './engineMode';

const showMockControls = import.meta.env.VITE_HIDE_MOCK_CONTROLS !== 'true';

/**
 * Picks the engine. In the desktop app that is the real one, with the demo
 * one click away on Home; in a browser (the preview) only the demo exists.
 */
export function Root({ info }: { info: EngineInfo | null }) {
  const forceDemo = new URLSearchParams(window.location.search).has('demo');
  const [mode, setMode] = useState<EngineMode>(info && !forceDemo ? 'real' : 'demo');
  const real = useMemo(() => (info ? new TauriSessionEngine(info) : null), [info]);
  const demo = useMemo(() => new MockSessionEngine(), []);
  const engine = mode === 'real' && real ? real : demo;

  const modeValue = useMemo(
    () => ({
      mode: engine === real ? ('real' as const) : ('demo' as const),
      canSwitch: real !== null,
      setMode: (next: EngineMode) => {
        // Leave whatever room the other engine had open.
        (next === 'demo' ? real : demo)?.leaveRoom();
        setMode(next);
      },
    }),
    [engine, real, demo],
  );

  return (
    <EngineModeContext.Provider value={modeValue}>
      <SessionEngineContext.Provider value={engine}>
        <App key={modeValue.mode} />
        {engine === demo && showMockControls && <MockControls engine={demo} />}
      </SessionEngineContext.Provider>
    </EngineModeContext.Provider>
  );
}

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './app/App';
import { MockControls } from './live-session/MockControls';
import { MockSessionEngine } from './session/mockEngine';
import { SessionEngineContext } from './session/useSession';
import './styles/tokens.css';
import './styles/live-session.css';
import './styles/pre-session.css';

// No engine exists yet, so the screens run on the isolated mock. Swapping in
// the real engine means providing a different SessionEngine here.
const engine = new MockSessionEngine();
const showMockControls = import.meta.env.VITE_HIDE_MOCK_CONTROLS !== 'true';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <SessionEngineContext.Provider value={engine}>
      <App />
      {showMockControls && <MockControls engine={engine} />}
    </SessionEngineContext.Provider>
  </StrictMode>,
);

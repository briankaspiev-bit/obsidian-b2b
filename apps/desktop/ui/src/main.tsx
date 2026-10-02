import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { Root } from './app/Root';
import { bridge, inDesktopApp, type EngineInfo } from './session/bridge';
import './styles/tokens.css';
import './styles/live-session.css';
import './styles/pre-session.css';

async function start() {
  // Inside the desktop app the real engine is available; in a browser only the demo.
  let info: EngineInfo | null = null;
  if (inDesktopApp()) {
    try {
      info = await bridge.engineInfo();
    } catch {
      info = null;
    }
  }
  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <Root info={info} />
    </StrictMode>,
  );
}

void start();

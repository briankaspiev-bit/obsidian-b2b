// MOCK ONLY. Lets a reviewer play the remote DJ's side and trigger failures.
// Not part of the product UI; remove once the real engine is wired in.

import { useState } from 'react';
import type { MockSessionEngine } from '../session/mockEngine';
import { roleOf } from '../session/sessionReducer';
import { useLinkState, useRoomState, useSessionState } from '../session/useSession';

export function MockControls({ engine }: { engine: MockSessionEngine }) {
  // Closed by default and parked on the left edge, so it never covers the booth.
  const [open, setOpen] = useState(false);
  const session = useSessionState();
  const link = useLinkState();
  const remote = session.djs[session.remoteId];
  const remoteRole = roleOf(session, session.remoteId);
  const ended = session.status === 'ended';
  const room = useRoomState();

  return (
    <aside className="mock" aria-label="Mock controls">
      <button type="button" className="mock__toggle" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
        Demo
      </button>
      {open && (
        <div className="mock__body">
          <p className="mock__note">Simulates the other side. Not part of the app.</p>
          {room.phase === 'home' && (
            <button type="button" onClick={engine.skipToLive}>
              Skip to the live session
            </button>
          )}
          {room.phase === 'booth' && (
            <>
              <button type="button" disabled={room.remotePresence !== 'waiting'} onClick={engine.remoteJoinNow}>
                {remote.name} joins now
              </button>
              <button type="button" disabled={room.remotePresence !== 'joined'} onClick={engine.remoteBoothReady}>
                {remote.name} presses ready
              </button>
              <button type="button" onClick={engine.skipToLive}>
                Skip to the live session
              </button>
              <button type="button" onClick={engine.leaveRoom}>
                Back to home
              </button>
            </>
          )}
          {room.phase === 'live' && (
            <>
              <button
                type="button"
                disabled={ended || remoteRole === 'onAir' || remoteRole === 'handoff'}
                onClick={remoteRole === 'ready' ? engine.remoteCancelReady : engine.remoteMarkReady}
              >
                {remoteRole === 'ready' ? `${remote.name}: cancel ready` : `${remote.name} taps READY`}
              </button>
              <button
                type="button"
                disabled={ended || remoteRole === 'onAir' || remoteRole === 'handoff'}
                onClick={engine.remoteTakeOver}
              >
                {remote.name} asks for the booth
              </button>
              <button
                type="button"
                disabled={ended || link.remote !== 'connected'}
                onClick={() => engine.simulateDropout(remoteRole === 'onAir' ? 15 : 6)}
              >
                Drop {remote.name}&rsquo;s connection
              </button>
              <button type="button" onClick={engine.restart}>
                Restart session
              </button>
              <button type="button" onClick={engine.leaveRoom}>
                Back to home
              </button>
            </>
          )}
        </div>
      )}
    </aside>
  );
}

import { useState } from 'react';
import { handoffCount, roleOf } from '../session/sessionReducer';
import type { SessionState } from '../session/types';
import { useEngine, useLinkState, useSessionState } from '../session/useSession';
import { DiagnosticsDrawer } from './DiagnosticsDrawer';
import { DjPanel } from './DjPanel';
import { EndSessionControl } from './EndSessionControl';
import { HandoffRail } from './HandoffRail';
import { LevelMeter } from './LevelMeter';
import { SessionHeader } from './SessionHeader';
import { SessionTimer } from './SessionTimer';
import { SystemHealthStrip } from './SystemHealthStrip';
import { TakeOverControl } from './TakeOverControl';
import { formatElapsed } from '../lib/format';

type Side = 'left' | 'right';

interface CenterStatus {
  key: string;
  main: string;
  sub: string | null;
  tone: 'live' | 'onAir' | 'ready' | 'handoff' | 'emergency';
  /** Shown under the rail while you're live and the other DJ is ready. */
  belowBadge: string | null;
}

/** The line above the rail that answers "who owns the booth right now?" */
function centerStatus(s: SessionState, emergency: boolean): CenterStatus {
  const local = s.djs[s.localId];
  const remote = s.djs[s.remoteId];
  if (emergency) {
    return {
      key: 'emergency',
      main: `${remote.name.toUpperCase()} DROPPED`,
      sub: `No audio from ${remote.name}. Take over now`,
      tone: 'emergency',
      belowBadge: null,
    };
  }
  if (s.handoff) {
    const to = s.djs[s.handoff.to];
    return {
      key: `handoff-${to.id}`,
      main: to.isLocal ? 'You\u2019re taking over…' : `${to.name} taking over…`,
      sub: to.isLocal ? 'Bring your fader up as you normally would' : 'Fade out on your mixer as you normally would',
      tone: 'handoff',
      belowBadge: null,
    };
  }
  if (s.ownerId === s.localId) {
    // Stays put even when the other DJ is ready, so "am I live?" never needs a second look.
    const remoteReady = s.readyIds.includes(remote.id);
    return {
      key: 'local-live',
      main: 'YOU’RE LIVE',
      sub: remoteReady ? null : 'Your mix is going out to the room',
      tone: 'live',
      belowBadge: remoteReady ? `${remote.name.toUpperCase()} READY` : null,
    };
  }
  if (s.readyIds.includes(local.id)) {
    return {
      key: 'local-ready',
      main: 'YOU’RE READY',
      sub: `${remote.name} can see it. Hold Take Over when your track is in`,
      tone: 'ready',
      belowBadge: null,
    };
  }
  const owner = s.djs[s.ownerId];
  return {
    key: `owner-${owner.id}`,
    main: `${owner.name.toUpperCase()} ON AIR`,
    sub: `${owner.name} owns the mix`,
    tone: 'onAir',
    belowBadge: null,
  };
}

export function LiveSessionScreen() {
  const engine = useEngine();
  const session = useSessionState();
  const link = useLinkState();
  const [diagOpen, setDiagOpen] = useState(false);

  const local = session.djs[session.localId];
  const remote = session.djs[session.remoteId];
  const localRole = roleOf(session, local.id);
  const remoteRole = roleOf(session, remote.id);
  const sideOf = (id: string): Side => (id === session.localId ? 'left' : 'right');
  const ownerSide = sideOf(session.ownerId);
  const reconnecting = link.remote === 'reconnecting';

  const handoff = session.handoff
    ? {
        from: sideOf(session.handoff.from),
        to: sideOf(session.handoff.to),
        durationMs: session.handoff.durationMs,
        key: session.handoff.startedAtMs,
      }
    : null;
  const handoffDir = (id: string) =>
    session.handoff ? (session.handoff.to === id ? 'incoming' : 'outgoing') : undefined;

  const readySide: Side | null = session.readyIds.length ? sideOf(session.readyIds[0]) : null;
  // The side that owns (or is receiving) the mix gets more of the booth.
  const emphasisSide = handoff ? handoff.to : ownerSide;
  const live = session.status === 'live';
  // The live DJ's connection is gone: the other DJ must pick up the mix now.
  const emergency = live && reconnecting && session.ownerId === remote.id && !session.handoff;
  const status = centerStatus(session, emergency);
  // When the local DJ last took the mix (falls back to the session start).
  const lastTake = [...session.events]
    .reverse()
    .find((e) => (e.type === 'handoff_complete' || e.type === 'emergency_take_over') && e.djId === local.id);
  const onAirSinceMs = session.ownerId === local.id ? (lastTake?.atMs ?? session.startedAtMs) : null;
  // You're live when you own the mix, or are receiving it mid-handoff.
  const localLive = live && (session.handoff ? session.handoff.to === local.id : session.ownerId === local.id);

  return (
    <div
      className="ls"
      data-emphasis={emphasisSide}
      data-local-live={localLive || undefined}
      data-remote-ready={(live && remoteRole === 'ready') || undefined}
      data-emergency={emergency || undefined}
    >
      <div className="ls-live-frame" aria-hidden="true" />
      <SessionHeader />

      <main className="ls-stage">
        <SessionTimer startedAtMs={session.startedAtMs} endedAtMs={session.endedAtMs} />

        <div className="ls-booth">
          <DjPanel
            dj={local}
            role={localRole}
            side="left"
            handoffDirection={handoffDir(local.id)}
            onToggleReady={live ? (localRole === 'ready' ? engine.cancelReady : engine.markReady) : undefined}
          />

          <div className="ls-center">
            <div className="ls-center__status" data-tone={status.tone} role="status" aria-live="polite">
              <span key={status.key} className="fade-in">
                {status.tone === 'ready' ? (
                  <span className="ready-badge">
                    <span className="ready-badge__led" aria-hidden="true" />
                    {status.main}
                  </span>
                ) : status.tone === 'emergency' ? (
                  <span className="emergency-mark">
                    <svg width="26" height="26" viewBox="0 0 24 24" aria-hidden="true">
                      <path d="M12 3 2 20h20L12 3z" fill="none" stroke="currentColor" strokeWidth="2" strokeLinejoin="round" />
                      <path d="M12 10v4.5M12 17.2v.3" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" />
                    </svg>
                    {status.main}
                  </span>
                ) : status.tone === 'live' ? (
                  <span className="live-mark">
                    <span className="live-mark__led" aria-hidden="true" />
                    {status.main}
                  </span>
                ) : (
                  <span className="ls-center__main">{status.main}</span>
                )}
                {status.sub && <span className="ls-center__sub">{status.sub}</span>}
              </span>
            </div>
            {status.belowBadge && (
              <div key={status.belowBadge} className="ls-center__below fade-in" role="status">
                <span className="ready-badge">
                  <span className="ready-badge__led" aria-hidden="true" />
                  {status.belowBadge}
                </span>
                <span className="ls-center__sub">Ready for handoff</span>
              </div>
            )}
            <HandoffRail ownerSide={ownerSide} handoff={handoff} readySide={handoff ? null : readySide} />
          </div>

          <DjPanel
            dj={remote}
            role={remoteRole}
            side="right"
            handoffDirection={handoffDir(remote.id)}
            reconnecting={reconnecting}
          />

          <div className="ls-booth__under-left">
            <LevelMeter
              title="YOUR SEND"
              getLevel={engine.getLocalLevel}
              quietText="Quiet while you cue"
              noSignalText="No signal from your mixer. Check the cable"
              expectingAudio={localRole === 'onAir' || (localRole === 'handoff' && session.handoff?.to === local.id)}
              liveText={localLive ? 'GOING OUT LIVE' : null}
            />
          </div>
          <div className="ls-booth__under-center">
            <TakeOverControl
              localRole={localRole}
              remoteName={remote.name}
              remoteCity={remote.city}
              remoteReady={remoteRole === 'ready'}
              handoffTo={session.handoff ? (session.handoff.to === local.id ? 'local' : 'remote') : null}
              enabled={live && localRole !== 'onAir' && !session.handoff}
              emergency={emergency}
              onAirSinceMs={onAirSinceMs}
              onTakeOver={emergency ? engine.emergencyTakeOver : engine.takeOver}
            />
          </div>
          <div className="ls-booth__under-right">
            <LevelMeter
              title="REMOTE LEVEL"
              getLevel={engine.getRemoteLevel}
              quietText={`Quiet while ${remote.name} cues`}
              noSignalText={`No audio arriving from ${remote.name}`}
              expectingAudio={remoteRole === 'onAir' || (remoteRole === 'handoff' && session.handoff?.to === remote.id)}
              recoveringText={reconnecting ? 'Remote audio recovering\u2026' : null}
            />
          </div>
        </div>
      </main>

      <footer className="ls-footer">
        <SystemHealthStrip link={link} remoteName={remote.name} remoteWasLive={emergency} />
        {live && <EndSessionControl remoteName={remote.name} onEnd={engine.endSession} />}
        <DiagnosticsDrawer
          open={diagOpen}
          onToggle={() => setDiagOpen((v) => !v)}
          diagnostics={link.diagnostics}
          connected={link.remote === 'connected'}
        />
      </footer>

      {!live && session.endedAtMs !== null && (
        <div className="ls-ended fade-in" role="status">
          <div className="ls-ended__eyebrow">SESSION ENDED</div>
          <div className="ls-ended__time">{formatElapsed(session.endedAtMs - session.startedAtMs)}</div>
          <div className="ls-ended__meta">
            {local.city} &harr; {remote.city} &middot; {handoffCount(session)} handoffs this session
          </div>
          <div className="ls-ended__meta">The master mix is being built from both recordings.</div>
          <button type="button" className="btn btn--secondary ls-ended__new" onClick={engine.leaveRoom}>
            NEW ROOM
          </button>
        </div>
      )}
    </div>
  );
}


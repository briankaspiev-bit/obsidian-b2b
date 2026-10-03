import { photoFor } from '../lib/photo';
import { useEffect, useState } from 'react';
import { LevelMeter } from '../live-session/LevelMeter';
import { SessionHeader } from '../live-session/SessionHeader';
import type { CheckStep, RoomState } from '../session/types';
import { readyAllowed } from '../session/roomReducer';
import { useEngine, useRoomState } from '../session/useSession';

/**
 * Between Home and the Live Session: pick devices, check the link to the
 * other booth, then both DJs press ready. Nothing here routes or changes the
 * DJ's own audio; Obsidian only listens to the chosen input.
 */
export function BoothCheckScreen() {
  const engine = useEngine();
  const room = useRoomState();
  const remoteName = room.remote?.name ?? 'the other DJ';
  const waiting = room.remotePresence === 'waiting';
  // No mixer: the built-in deck and the other DJ both play on the one output.
  const testMusic = room.inputId?.startsWith('test-music') ?? false;

  return (
    <div className="booth">
      <SessionHeader context="Booth Check" roomCode={room.code} />

      <main className="booth__main">
        <div className="booth__top">
          <h1 className="booth__title">Set up your booth</h1>
          {room.isHost && waiting && room.code && <InviteCode code={room.code} />}
        </div>

        <div className="booth__grid">
          <section className="booth-card" aria-labelledby="your-booth">
            <div className="booth-card__head">
              <h2 id="your-booth" className="booth-card__eyebrow">
                YOUR BOOTH
              </h2>
              <Person name={room.local.name} city={room.local.city} photoUrl={photoFor(room.local)} you />
            </div>

            <DeviceField
              id="input"
              label="What you send"
              hint="Your mixer's REC or BOOTH out. No mixer? Pick Test music"
              devices={room.inputs}
              value={room.inputId}
              onChange={engine.selectInput}
            />
            <LevelMeter
              title="YOUR SEND"
              getLevel={engine.getLocalLevel}
              quietText="Play something on your mixer"
              noSignalText="Nothing on this input. Check the cable"
              expectingAudio={room.inputId !== null}
            />
            <DeviceField
              id="output"
              label={testMusic ? 'Where you listen' : `Where you hear ${remoteName}`}
              hint={
                testMusic
                  ? `Your headphones or speakers. You hear your music and ${remoteName} here, mixed.`
                  : 'A monitor or a spare mixer channel'
              }
              devices={room.outputs}
              value={room.outputId}
              onChange={engine.selectOutput}
            />
            <p className="booth-card__note">
              Your decks and headphone cue stay untouched. Obsidian never sits between you and your mixer.
            </p>
          </section>

          <section className="booth-card" aria-labelledby="other-booth">
            <div className="booth-card__head">
              <h2 id="other-booth" className="booth-card__eyebrow">
                OTHER BOOTH
              </h2>
              {room.remote ? (
                <Person name={room.remote.name} city={room.remote.city} photoUrl={photoFor(room.remote)} />
              ) : (
                <div className="person person--empty">
                  <span className="person__avatar" aria-hidden="true" />
                  <span className="person__name">Waiting for the other DJ&hellip;</span>
                </div>
              )}
              <PresencePill room={room} />
            </div>

            <CheckList room={room} />

            <LevelMeter
              title={`FROM ${remoteName.toUpperCase()}`}
              getLevel={engine.getRemoteLevel}
              quietText="Their mixer shows here"
              noSignalText="Nothing coming through yet"
              expectingAudio={false}
            />

            <button
              type="button"
              className="btn btn--secondary booth-card__run"
              onClick={engine.runBoothCheck}
              disabled={waiting || room.check.status === 'running'}
            >
              {room.check.status === 'running'
                ? 'CHECKING…'
                : room.check.status === 'idle'
                  ? 'RUN BOOTH CHECK'
                  : 'RUN AGAIN'}
            </button>
          </section>
        </div>

        <ReadyBar room={room} remoteName={remoteName} />
      </main>

      {room.startsAtMs !== null && <Countdown startsAtMs={room.startsAtMs} remoteName={remoteName} />}
    </div>
  );
}

function InviteCode({ code }: { code: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    } catch {
      // Clipboard can be blocked (e.g. in a preview frame); the code stays visible to read out.
    }
  };
  return (
    <div className="invite">
      <span className="invite__label">Send this code to the DJ you&rsquo;re playing with</span>
      <div className="invite__row">
        <span className="invite__code">{code}</span>
        <button type="button" className="btn btn--quiet" onClick={copy}>
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>
    </div>
  );
}

function Person({ name, city, photoUrl, you = false }: { name: string; city: string; photoUrl?: string; you?: boolean }) {
  return (
    <div className="person">
      {photoUrl ? (
        <img className="person__avatar" src={photoUrl} alt="" />
      ) : (
        <span className="person__avatar" aria-hidden="true">
          {name.slice(0, 1)}
        </span>
      )}
      <span className="person__name">
        {name}
        {you && <span className="person__you"> &middot; you</span>}
      </span>
      <span className="person__city">{city}</span>
    </div>
  );
}

function PresencePill({ room }: { room: RoomState }) {
  const map = {
    waiting: { text: 'WAITING', tone: 'muted' },
    joined: { text: 'IN THE ROOM', tone: 'ok' },
    ready: { text: 'READY', tone: 'cue' },
  } as const;
  const p = map[room.remotePresence];
  return (
    <span key={p.text} className="presence fade-in" data-tone={p.tone} role="status">
      <span className="presence__led" aria-hidden="true" />
      {p.text}
    </span>
  );
}

function DeviceField(props: {
  id: string;
  label: string;
  hint: string;
  devices: RoomState['inputs'];
  value: string | null;
  onChange: (id: string) => void;
}) {
  return (
    <div className="field">
      <label className="field__label" htmlFor={props.id}>
        {props.label}
      </label>
      <select
        id={props.id}
        className="field__select"
        value={props.value ?? ''}
        onChange={(e) => props.onChange(e.target.value)}
        aria-describedby={`${props.id}-hint`}
      >
        {props.devices.length === 0 && (
          <option value="" disabled>
            No audio devices found
          </option>
        )}
        {props.devices.map((d) => (
          <option key={d.id} value={d.id}>
            {d.label} &middot; {d.detail}
          </option>
        ))}
      </select>
      <span id={`${props.id}-hint`} className="field__hint">
        {props.hint}
      </span>
    </div>
  );
}

function StepIcon({ status }: { status: CheckStep['status'] }) {
  if (status === 'ok')
    return (
      <svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true">
        <circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" strokeWidth="1.8" />
        <path d="m7.5 12.5 3 3 6-6.5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
    );
  if (status === 'attention')
    return (
      <svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true">
        <circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" strokeWidth="1.8" />
        <path d="M12 7v6M12 16.5v.5" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
      </svg>
    );
  return <span className={`step__dot${status === 'running' ? ' step__dot--running' : ''}`} aria-hidden="true" />;
}

const STATUS_TEXT: Record<CheckStep['status'], string> = {
  pending: 'Not checked yet',
  running: 'Checking',
  ok: 'Passed',
  attention: 'Needs attention',
};

function CheckList({ room }: { room: RoomState }) {
  return (
    <ol className="checks" aria-label="Booth check">
      {room.check.steps.map((st) => (
        <li key={st.id} className="step" data-status={st.status}>
          <span className="step__icon">
            <StepIcon status={st.status} />
          </span>
          <span className="step__label">{st.label}</span>
          <span className="step__result">
            <span className="visually-hidden">{STATUS_TEXT[st.status]}. </span>
            {st.status === 'running' ? 'Checking…' : st.result ?? ''}
          </span>
        </li>
      ))}
    </ol>
  );
}

function ReadyBar({ room, remoteName }: { room: RoomState; remoteName: string }) {
  const engine = useEngine();
  const passed = readyAllowed(room.check);
  const warnings = passed && room.check.status === 'attention';
  const remoteReady = room.remotePresence === 'ready';

  let line: string;
  if (room.remotePresence === 'waiting') line = `Waiting for ${remoteName} to join`;
  else if (room.check.status === 'attention' && !passed)
    line = 'Sort out the steps marked yellow, then run the check again.';
  else if (!passed) line = 'Run the booth check first. It takes a few seconds.';
  else if (room.localReady && !remoteReady) line = `Waiting for ${remoteName} to press ready`;
  else if (!room.localReady && remoteReady) line = `${remoteName} is ready. Press ready when you are.`;
  else if (room.localReady && remoteReady) line = 'Both ready. Starting…';
  else if (warnings) line = 'You can go live. The yellow steps are warnings, the set may sound rougher.';
  else line = 'The session starts when you both press ready.';

  return (
    <div className="ready-bar" data-ready={room.localReady || undefined}>
      <button type="button" className="btn btn--quiet ready-bar__leave" onClick={engine.leaveRoom}>
        Leave room
      </button>
      <button
        type="button"
        className="ready-bar__button"
        aria-pressed={room.localReady}
        disabled={!passed}
        onClick={() => engine.setBoothReady(!room.localReady)}
      >
        {room.localReady ? 'READY' : 'I’M READY'}
      </button>
      <p className="ready-bar__line" role="status">
        <span key={line} className="fade-in">
          {line}
        </span>
      </p>
      {room.localReady && room.startsAtMs === null && (
        <button type="button" className="btn btn--quiet" onClick={() => engine.setBoothReady(false)}>
          Not ready yet
        </button>
      )}
    </div>
  );
}

function Countdown({ startsAtMs, remoteName }: { startsAtMs: number; remoteName: string }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 100);
    return () => window.clearInterval(id);
  }, []);
  const n = Math.max(1, Math.ceil((startsAtMs - now) / 1000));
  return (
    <div className="countdown" role="status" aria-live="assertive">
      <div className="countdown__eyebrow">YOU AND {remoteName.toUpperCase()} ARE READY</div>
      <div key={n} className="countdown__n">
        {n}
      </div>
      <div className="countdown__line">Starting the session</div>
    </div>
  );
}

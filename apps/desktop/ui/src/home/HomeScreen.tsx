import { useRef, useState, type FormEvent } from 'react';
import { useEngineMode } from '../app/engineMode';
import { SessionHeader } from '../live-session/SessionHeader';
import { useEngine, useRoomState } from '../session/useSession';

/** Create a private room, or join one with the code the other DJ sent. */
export function HomeScreen() {
  const engine = useEngine();
  const room = useRoomState();
  const [code, setCode] = useState('');
  const [track, setTrack] = useState<File | null>(null);
  const picker = useRef<HTMLInputElement>(null);
  // Errors from opening a room and from starting practice share one slot; show it where it came from.
  const [lastTried, setLastTried] = useState<'room' | 'practice'>('room');
  const { mode, canSwitch, setMode } = useEngineMode();

  const onJoin = (e: FormEvent) => {
    e.preventDefault();
    engine.joinRoom(code);
  };

  return (
    <div className="home">
      <SessionHeader context="Home" />
      <main className="home__main">
        <section className="home__hero">
          <div className="home__eyebrow">PRIVATE ROOM &middot; 2 DJS</div>
          <h1 className="home__title">
            Go back-to-back
            <br />
            from anywhere.
          </h1>
          <p className="home__lede">Keep your decks and your software. Obsidian connects your mixer to theirs.</p>
        </section>

        <div className="home__you">
          <label className="home__you-label" htmlFor="dj-name">
            YOUR DJ NAME
          </label>
          <input
            id="dj-name"
            className="text-input"
            value={room.local.name}
            onChange={(e) => engine.setLocalProfile({ name: e.target.value, city: room.local.city })}
            placeholder="How the other DJ sees you"
            autoComplete="nickname"
            spellCheck={false}
            maxLength={40}
          />
          <input
            id="dj-city"
            className="text-input text-input--city"
            value={room.local.city}
            onChange={(e) => engine.setLocalProfile({ name: room.local.name, city: e.target.value })}
            placeholder="City"
            aria-label="Your city"
            maxLength={40}
          />
        </div>

        <div className="home__cards">
          <section className="home-card" aria-labelledby="create-title">
            <h2 id="create-title" className="home-card__title">
              Start a booth
            </h2>
            <p className="home-card__text">Get a code and send it to the DJ you want to play with.</p>
            {room.createError && lastTried === 'room' && (
              <p className="home-card__error fade-in" role="alert">
                {room.createError}
              </p>
            )}
            <button
              type="button"
              className="btn btn--primary"
              onClick={() => {
                setLastTried('room');
                engine.createRoom();
              }}
              disabled={room.creating}
            >
              {room.creating && lastTried === 'room' ? 'OPENING ROOM…' : 'CREATE ROOM'}
            </button>
          </section>

          <form className="home-card" aria-labelledby="join-title" onSubmit={onJoin} noValidate>
            <h2 id="join-title" className="home-card__title">
              Join a booth
            </h2>
            <label className="home-card__text" htmlFor="join-code">
              Enter the invite code you were sent.
            </label>
            <input
              id="join-code"
              className="code-input"
              value={code}
              onChange={(e) => setCode(e.target.value.toUpperCase())}
              placeholder="K7QX-M2PD"
              autoComplete="off"
              spellCheck={false}
              maxLength={11}
              aria-invalid={room.joinError ? true : undefined}
              aria-describedby={room.joinError ? 'join-error' : undefined}
            />
            {room.joinError && (
              <p id="join-error" className="home-card__error fade-in" role="alert">
                {room.joinError}
              </p>
            )}
            <button type="submit" className="btn btn--secondary" disabled={room.joining}>
              {room.joining ? 'FINDING ROOM…' : 'JOIN ROOM'}
            </button>
          </form>

          <section className="home-card home-card--practice" aria-labelledby="practice-title">
            <h2 id="practice-title" className="home-card__title">
              Practice
            </h2>
            <p className="home-card__text">
              A Ghost DJ plays from London. Match their beat, then take over. Headphones on.
            </p>
            <div className="home-card__track">
              <span className="home-card__track-name">{track ? track.name : 'Built-in groove'}</span>
              <button type="button" className="link-btn" onClick={() => picker.current?.click()}>
                {track ? 'Change' : 'Use my own track'}
              </button>
              <input
                ref={picker}
                type="file"
                accept="audio/*,.mp3,.wav,.flac,.m4a,.aac,.ogg"
                hidden
                onChange={(e) => setTrack(e.target.files?.[0] ?? null)}
              />
            </div>
            {room.createError && lastTried === 'practice' && (
              <p className="home-card__error fade-in" role="alert">
                {room.createError}
              </p>
            )}
            <button
              type="button"
              className="btn btn--secondary"
              onClick={() => {
                setLastTried('practice');
                engine.startPractice(track);
              }}
              disabled={room.creating}
            >
              {room.creating && lastTried === 'practice' ? 'STARTING…' : 'PRACTICE WITH THE GHOST DJ'}
            </button>
          </section>
        </div>

        {canSwitch && (
          <p className="home__mode">
            {mode === 'real' ? (
              <>
                No one to play with right now?{' '}
                <button type="button" className="link-btn" onClick={() => setMode('demo')}>
                  Try the demo
                </button>
              </>
            ) : (
              <>
                You&rsquo;re in the demo: the other DJ is simulated.{' '}
                <button type="button" className="link-btn" onClick={() => setMode('real')}>
                  Back to real rooms
                </button>
              </>
            )}
          </p>
        )}
      </main>
    </div>
  );
}

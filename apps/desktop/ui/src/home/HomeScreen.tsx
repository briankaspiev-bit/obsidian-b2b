import { useState, type FormEvent } from 'react';
import { SessionHeader } from '../live-session/SessionHeader';
import { useEngine, useRoomState } from '../session/useSession';

/** Create a private room, or join one with the code the other DJ sent. */
export function HomeScreen() {
  const engine = useEngine();
  const room = useRoomState();
  const [code, setCode] = useState('');

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

        <div className="home__cards">
          <section className="home-card" aria-labelledby="create-title">
            <h2 id="create-title" className="home-card__title">
              Start a booth
            </h2>
            <p className="home-card__text">Get a code and send it to the DJ you want to play with.</p>
            <button type="button" className="btn btn--primary" onClick={engine.createRoom}>
              CREATE ROOM
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
        </div>
      </main>
    </div>
  );
}

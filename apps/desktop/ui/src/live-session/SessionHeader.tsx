interface Props {
  /** Where you are: "Live Session", "Booth Check"… */
  context?: string;
  /** Shown on the right once a room exists. */
  roomCode?: string | null;
}

export function SessionHeader({ context = 'Live Session', roomCode = null }: Props) {
  return (
    <header className="ls-header">
      <div className="ls-header__brand">
        <svg className="ls-header__mark" width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path d="M12 2 4 9l8 13 8-13z" stroke="currentColor" strokeWidth="1.6" strokeLinejoin="round" />
          <path d="M4 9h16M12 2l-3 7 3 13 3-13z" stroke="currentColor" strokeWidth="1.6" strokeLinejoin="round" />
        </svg>
        <span className="ls-header__name">OBSIDIAN B2B</span>
        <span className="ls-header__divider" aria-hidden="true" />
        <span className="ls-header__context">{context}</span>
      </div>
      <div className="ls-header__room">
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <rect x="5" y="11" width="14" height="10" rx="2" stroke="currentColor" strokeWidth="1.8" />
          <path d="M8 11V8a4 4 0 0 1 8 0v3" stroke="currentColor" strokeWidth="1.8" />
        </svg>
        Private Room
        {roomCode && <span className="ls-header__code">{roomCode}</span>}
      </div>
    </header>
  );
}

import { useEffect, useRef } from 'react';
import type { DeckAction, DeckFeed } from '../session/deck';

interface Options {
  enabled: boolean;
  feed: DeckFeed | null;
  onAction: (a: DeckAction) => void;
  /** Null while TAKE OVER isn't offered (you're on air, or a handoff runs). */
  onTakeOver: (() => void) | null;
  onToggleReady: (() => void) | null;
}

/**
 * The same keys as the practice tool in the terminal: SPACE take over,
 * R ready, P play, C cue, S sync, , . nudge (Shift for bigger), - = pitch,
 * ↑↓ your fader, ←→ them in your ears, G bring the ghost back.
 */
export function useDeckKeys(o: Options) {
  const opts = useRef(o);
  opts.current = o;
  useEffect(() => {
    if (!o.enabled) return;
    const onKey = (e: KeyboardEvent) => {
      const { feed, onAction, onTakeOver, onToggleReady } = opts.current;
      const el = e.target as HTMLElement | null;
      const repeatable = ['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', ',', '.', '<', '>'].includes(e.key);
      if (e.ctrlKey || e.metaKey || e.altKey || (e.repeat && !repeatable)) return;
      if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)) return;
      const info = feed?.info;
      const big = e.shiftKey ? 5 : 1;
      const step = (v: number, d: number, max: number) => Math.min(max, Math.max(0, Math.round((v + d) * 100) / 100));
      let act: DeckAction | null = null;
      switch (e.key) {
        case ' ':
        case 't':
        case 'T':
          if (onTakeOver) onTakeOver();
          break;
        case 'r':
        case 'R':
          if (onToggleReady) onToggleReady();
          break;
        case 'p':
        case 'P':
          act = { kind: 'playPause' };
          break;
        case 'c':
        case 'C':
          act = { kind: 'cue' };
          break;
        case 's':
        case 'S':
          act = { kind: 'sync' };
          break;
        case ',':
        case '<':
          act = { kind: 'nudge', ms: -10 * big };
          break;
        case '.':
        case '>':
          act = { kind: 'nudge', ms: 10 * big };
          break;
        case '-':
        case '_':
          act = { kind: 'pitch', pct: -0.1 * big };
          break;
        case '=':
        case '+':
          act = { kind: 'pitch', pct: 0.1 * big };
          break;
        case 'ArrowUp':
          act = { kind: 'fader', value: step(info?.fader ?? 1, 0.05, 1) };
          break;
        case 'ArrowDown':
          act = { kind: 'fader', value: step(info?.fader ?? 1, -0.05, 1) };
          break;
        case 'ArrowRight':
          act = { kind: 'partnerVolume', value: step(info?.partnerVolume ?? 1, 0.05, 2) };
          break;
        case 'ArrowLeft':
          act = { kind: 'partnerVolume', value: step(info?.partnerVolume ?? 1, -0.05, 2) };
          break;
        case 'g':
        case 'G':
          act = { kind: 'ghostComeBack' };
          break;
        default:
          return;
      }
      e.preventDefault();
      if (act) onAction(act);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [o.enabled]);
}

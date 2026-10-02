export type Listener = () => void;

/** A tiny subscriber set for useSyncExternalStore. */
export function createListeners() {
  const set = new Set<Listener>();
  return {
    add(l: Listener) {
      set.add(l);
      return () => {
        set.delete(l);
      };
    },
    emit() {
      set.forEach((l) => l());
    },
  };
}

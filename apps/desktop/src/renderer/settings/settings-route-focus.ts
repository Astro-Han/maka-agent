// Focus follows the level, for any settings page that owns more than one.
//
// Without this a level change leaves the ring on `document.body` — the control
// that had focus just unmounted — and a keyboard user restarts from the top of
// the document on every move. A Dialog gets this for free; a route level has
// to say it, and both settings pages that own levels said it separately until
// this file existed. A focus rule is the one duplication that cannot be seen
// on screen when the two copies drift.
import { useEffect, useRef, type RefObject } from 'react';

export type SettingsRouteFocusOptions = {
  /** The level being rendered. `listLevel` is the one the user returns to. */
  level: string;
  listLevel: string;
  /**
   * Anything that should re-run the focus move without changing `level` —
   * a second detail opened directly from a first, say. Compared by identity.
   */
  routeKey?: unknown;
  /** False while the page has nothing to focus yet (loading its data). */
  isReady?: boolean;
  /** Focus targets for a non-list level, in preference order. */
  focusSelectors(level: string): readonly string[];
  /** Which row the user left the list from; consumed on the way back. */
  listReturnFocusRef: RefObject<string | null>;
  listReturnSelector(token: string): string;
  /** Where focus lands when the row is gone — deletion is exactly that. */
  listFallbackRef: RefObject<HTMLElement | null>;
};

export function useSettingsRouteFocus(options: SettingsRouteFocusOptions): void {
  // Read through a ref so callers can pass fresh closures every render without
  // the effect re-running: the level (and `routeKey`) is what decides a move.
  const latest = useRef(options);
  latest.current = options;
  const { level, routeKey, isReady = true } = options;

  // Navigating, not arriving: the page does not grab focus when the settings
  // surface first renders it — the user is still on the settings nav item they
  // clicked to get here, and taking the ring off it strands them.
  const hasNavigatedRef = useRef(false);
  useEffect(() => {
    if (!isReady) return;
    if (!hasNavigatedRef.current) {
      hasNavigatedRef.current = true;
      return;
    }
    const frame = window.requestAnimationFrame(() => {
      const current = latest.current;
      // `preventScroll` throughout because this is a landing, not a jump: the
      // level just rendered at the top of the content area, and scrolling to
      // whatever the focus target happens to be would push its header away.
      if (level !== current.listLevel) {
        for (const selector of current.focusSelectors(level)) {
          const element = document.querySelector<HTMLElement>(selector);
          if (element) {
            element.focus({ preventScroll: true });
            return;
          }
        }
        return;
      }
      // Consumed here and only here: the ref is set on the way down and has to
      // survive the levels in between.
      const returnTo = current.listReturnFocusRef.current;
      current.listReturnFocusRef.current = null;
      const row = returnTo
        ? document.querySelector<HTMLElement>(current.listReturnSelector(returnTo))
        : null;
      (row ?? current.listFallbackRef.current)?.focus({ preventScroll: true });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [level, routeKey, isReady]);
}

import { useCallback, useRef, useSyncExternalStore } from 'react';
import type { AppShellSessionUiState, AppShellSessionUiStateController } from './app-shell-session-ui-state.js';

/**
 * Subscribe to one derived reading of session UI state (#1985).
 *
 * The controller is a single external store whose maps change at very different
 * rates — `liveTurnBySession` moves once per streamed token, the rest at human
 * speed. A component re-renders only when the value IT selects changes, so the
 * chat transcript can follow every delta while the shell around it stays still.
 *
 * `select` may be an inline arrow: it is read through a ref, so `getSnapshot`
 * keeps a stable identity and React never re-subscribes because of it.
 */
export function useAppShellSessionUiSelector<T>(
  controller: AppShellSessionUiStateController,
  select: (state: AppShellSessionUiState) => T,
  isEqual?: (a: T, b: T) => boolean,
): T {
  const selectRef = useRef(select);
  selectRef.current = select;
  const isEqualRef = useRef(isEqual);
  isEqualRef.current = isEqual;
  const cacheRef = useRef<{ value: T } | null>(null);

  // `useSyncExternalStore` requires a snapshot that keeps its identity while
  // nothing it selects changed, or it loops. A selector that derives a fresh
  // object therefore has to say what "unchanged" means for that object.
  const getSnapshot = useCallback(() => {
    const next = selectRef.current(controller.getState());
    const cached = cacheRef.current;
    if (cached && isSameSnapshot(cached.value, next, isEqualRef.current)) return cached.value;
    cacheRef.current = { value: next };
    return next;
  }, [controller]);

  return useSyncExternalStore(controller.subscribe, getSnapshot, getSnapshot);
}

function isSameSnapshot<T>(previous: T, next: T, isEqual: ((a: T, b: T) => boolean) | undefined): boolean {
  if (Object.is(previous, next)) return true;
  return isEqual ? isEqual(previous, next) : false;
}

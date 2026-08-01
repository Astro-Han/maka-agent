import { safeLocalStorageGet } from './browser-storage.js';

export const SESSION_WORKBAR_DEFAULT_WIDTH = 400;
export const SESSION_WORKBAR_MIN_WIDTH = 320;
export const SESSION_WORKBAR_MAX_WIDTH = 600;
// 'quote' is a transient tab that only exists while a quote side-panel excerpt
// is active; it is never persisted as a default (see readSessionWorkbarTab).
export type SessionWorkbarTab = 'tasks' | 'browser' | 'files' | 'quote';

/**
 * Seeds `useResizable`'s `defaultSize`. Deliberately unclamped: the hook clamps
 * whatever it is handed against `minSizePx`/`maxSizePx`, so a second clamp here
 * would be a duplicate authority over the bounds.
 */
export function readSessionWorkbarWidth(): number {
  const stored = Number(safeLocalStorageGet('maka-session-workbar-width-v1'));
  return Number.isFinite(stored) && stored > 0 ? stored : SESSION_WORKBAR_DEFAULT_WIDTH;
}

export function readSessionWorkbarCollapsed(): boolean {
  const stored = safeLocalStorageGet('maka-session-workbar-collapsed-v1');
  if (stored === 'false') return false;
  if (stored === 'true') return true;
  return true;
}

export function readSessionWorkbarTab(): SessionWorkbarTab {
  const stored = safeLocalStorageGet('maka-session-workbar-tab-v1');
  return stored === 'browser' || stored === 'files' ? stored : 'tasks';
}

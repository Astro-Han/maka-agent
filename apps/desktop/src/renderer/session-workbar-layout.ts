import { safeLocalStorageGet } from './browser-storage.js';

export const SESSION_WORKBAR_DEFAULT_WIDTH = 400;
export const SESSION_WORKBAR_MIN_WIDTH = 320;
export const SESSION_WORKBAR_MAX_WIDTH = 600;
/**
 * `useResizable` autoSaveId — it owns clamping and persistence for the width,
 * under its own `astryx-resizable:` storage prefix. The hand-rolled
 * `maka-session-workbar-width-v1` key is deliberately not migrated: a stored
 * width resets once, which costs one drag.
 */
export const SESSION_WORKBAR_WIDTH_AUTOSAVE_ID = 'maka-session-workbar-width';
// 'quote' is a transient tab that only exists while a quote side-panel excerpt
// is active; it is never persisted as a default (see readSessionWorkbarTab).
export type SessionWorkbarTab = 'tasks' | 'browser' | 'files' | 'quote';

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

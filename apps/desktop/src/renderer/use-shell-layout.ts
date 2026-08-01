import { useState } from 'react';
import { useResizable } from '@astryxdesign/core/Resizable';
import { readSessionListCollapsed, readSessionListWidth } from './session-list-layout';
import {
  readSessionWorkbarCollapsed,
  readSessionWorkbarTab,
  readSessionWorkbarWidth,
  SESSION_WORKBAR_MAX_WIDTH,
  SESSION_WORKBAR_MIN_WIDTH,
} from './session-workbar-layout';

/**
 * Owns the shell layout state (issue #1043): widths, collapse flags and the
 * active workbar tab, each hydrated from localStorage on first render and
 * persisted by `useAppShellPersistenceEffects`.
 *
 * `useResizable` owns the workbar's clamping and its pointer/keyboard resizing
 * (issue #1861), but not its persistence: `autoSaveId` writes synchronously on
 * every committed size, which is ~90 localStorage writes for one drag. Storage
 * stays on the same debounced effect as the session list, so seeding
 * `defaultSize` from storage is what closes the loop.
 */
export function useShellLayout() {
  const [sessionListWidth, setSessionListWidth] = useState(() => readSessionListWidth());
  const [sessionListCollapsed, setSessionListCollapsed] = useState(() => readSessionListCollapsed());
  const [workbarCollapsed, setWorkbarCollapsed] = useState(() => readSessionWorkbarCollapsed());
  // Read once: useResizable only consumes defaultSize on its first render, and
  // AppShell re-renders on every stream tick.
  const [workbarDefaultWidth] = useState(() => readSessionWorkbarWidth());
  const workbar = useResizable({
    defaultSize: workbarDefaultWidth,
    minSizePx: SESSION_WORKBAR_MIN_WIDTH,
    maxSizePx: SESSION_WORKBAR_MAX_WIDTH,
  });
  const [workbarTab, setWorkbarTab] = useState(() => readSessionWorkbarTab());
  return {
    sessionListWidth,
    setSessionListWidth,
    sessionListCollapsed,
    setSessionListCollapsed,
    workbarCollapsed,
    setWorkbarCollapsed,
    // Rounded because `clampSize` only bounds the size, it does not round it:
    // a sub-pixel pointer delta otherwise reaches the CSS variable and storage
    // as 479.70001220703125.
    workbarWidth: Math.round(workbar.size),
    workbarResizable: workbar.props,
    workbarTab,
    setWorkbarTab,
  };
}

import { useState } from 'react';
import { useResizable } from '@astryxdesign/core/Resizable';
import { readSessionListCollapsed, readSessionListWidth } from './session-list-layout';
import {
  readSessionWorkbarCollapsed,
  readSessionWorkbarTab,
  SESSION_WORKBAR_DEFAULT_WIDTH,
  SESSION_WORKBAR_MAX_WIDTH,
  SESSION_WORKBAR_MIN_WIDTH,
  SESSION_WORKBAR_WIDTH_AUTOSAVE_ID,
} from './session-workbar-layout';

/**
 * Owns the shell layout state (issue #1043): session-list width, collapse flags
 * and the active workbar tab are hydrated from localStorage on first render and
 * persisted by `useAppShellPersistenceEffects`.
 *
 * The workbar width does not go through either of those: `useResizable` owns its
 * clamping, pointer/keyboard resizing and persistence (issue #1861).
 * `workbarResizable` wires the region to the `ResizeHandle` in `chat-workbar.tsx`.
 */
export function useShellLayout() {
  const [sessionListWidth, setSessionListWidth] = useState(() => readSessionListWidth());
  const [sessionListCollapsed, setSessionListCollapsed] = useState(() => readSessionListCollapsed());
  const [workbarCollapsed, setWorkbarCollapsed] = useState(() => readSessionWorkbarCollapsed());
  const workbar = useResizable({
    defaultSize: SESSION_WORKBAR_DEFAULT_WIDTH,
    minSizePx: SESSION_WORKBAR_MIN_WIDTH,
    maxSizePx: SESSION_WORKBAR_MAX_WIDTH,
    autoSaveId: SESSION_WORKBAR_WIDTH_AUTOSAVE_ID,
  });
  const [workbarTab, setWorkbarTab] = useState(() => readSessionWorkbarTab());
  return {
    sessionListWidth,
    setSessionListWidth,
    sessionListCollapsed,
    setSessionListCollapsed,
    workbarCollapsed,
    setWorkbarCollapsed,
    workbarWidth: workbar.size,
    workbarResizable: workbar.props,
    workbarTab,
    setWorkbarTab,
  };
}

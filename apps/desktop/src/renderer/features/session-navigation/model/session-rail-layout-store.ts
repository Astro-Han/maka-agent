/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

import type { SideNavImperativeCollapseHandle } from '@astryxdesign/core/SideNav';
import type { SessionViewMode } from '@maka/ui';
import { safeLocalStorageSet } from '../../../browser-storage.js';
import {
  clampSessionListWidth,
  readSessionListCollapsed,
  readSessionListViewMode,
  readSessionListWidth,
  writeSessionListViewMode,
} from './session-list-layout.js';

const LAYOUT_PERSIST_DEBOUNCE_MS = 200;

export interface SessionRailLayoutState {
  readonly collapsed: boolean;
  readonly width: number;
  readonly viewMode: SessionViewMode;
}

/**
 * The rail's own geometry, as a store rather than `useState` in the shell
 * (#4109).
 *
 * Two readers want it and they are on opposite sides of the rail: the rail
 * renders at this width, and the window frame publishes it as
 * `--maka-sidenav-width` so the titlebar's session breadcrumb starts where the
 * column ends. Held as shell state it re-rendered the whole tree on every drag
 * frame; held here each side subscribes to the reading it uses.
 *
 * One rail exists per renderer and its persisted form is already a single
 * localStorage record, so the store is a module value: there is no second
 * instance for it to be an instance of.
 */
export function createSessionRailLayoutStore() {
  let state: SessionRailLayoutState = {
    collapsed: readSessionListCollapsed(),
    width: readSessionListWidth(),
    viewMode: readSessionListViewMode(),
  };
  const listeners = new Set<() => void>();
  const collapseHandleRef: { current: SideNavImperativeCollapseHandle | null } = { current: null };
  let widthPersistHandle: ReturnType<typeof setTimeout> | undefined;

  function replaceState(next: SessionRailLayoutState): void {
    if (next === state) return;
    state = next;
    for (const listener of [...listeners]) listener();
  }

  return {
    getState: (): SessionRailLayoutState => state,
    subscribe(listener: () => void): () => void {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    collapseHandleRef,
    setCollapsed(next: boolean): void {
      if (state.collapsed === next) return;
      replaceState({ ...state, collapsed: next });
      safeLocalStorageSet('maka-chat-list-collapsed-v1', next ? 'true' : 'false');
    },
    /** Debounced: a drag reports a width per frame and only the last one is worth storing. */
    setWidth(next: number): void {
      const width = clampSessionListWidth(next);
      if (state.width === width) return;
      replaceState({ ...state, width });
      if (widthPersistHandle !== undefined) clearTimeout(widthPersistHandle);
      widthPersistHandle = setTimeout(() => {
        safeLocalStorageSet('maka-chat-list-width-v1', String(width));
      }, LAYOUT_PERSIST_DEBOUNCE_MS);
    },
    setViewMode(next: SessionViewMode): void {
      if (state.viewMode === next) return;
      replaceState({ ...state, viewMode: next });
      writeSessionListViewMode(next);
    },
  };
}

export type SessionRailLayoutStore = ReturnType<typeof createSessionRailLayoutStore>;

export const sessionRailLayoutStore: SessionRailLayoutStore = createSessionRailLayoutStore();

/**
 * The whole geometry, for a reader that wants more than one field of it. The
 * store replaces its state only when a field actually moved, so the identity is
 * already the comparison.
 */
export const selectRailLayout = (state: SessionRailLayoutState): SessionRailLayoutState => state;

export const selectRailCollapsed = (state: SessionRailLayoutState): boolean => state.collapsed;
export const selectRailWidth = (state: SessionRailLayoutState): number => state.width;
export const selectRailViewMode = (state: SessionRailLayoutState): SessionViewMode => state.viewMode;

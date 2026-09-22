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

import { useRef } from 'react';
import { valuesEqual } from '@maka/ui';
import { compareDesktopSessionCatalogSummaries, type DesktopSessionSummary } from '../../../../shared/desktop-session-projection.js';
import { createObservableState } from './observable-state.js';

/** Rows and selection publish atomically; failed reads do not prove deletion. */
export interface SessionCatalogState {
  readonly sessions: readonly DesktopSessionSummary[];
  readonly revision: number;
  readonly hasSnapshot: boolean;
  readonly activeSessionId: string | undefined;
}

export type SessionPatchResult =
  | { kind: 'observed'; session: DesktopSessionSummary | null }
  | { kind: 'superseded' };

export function createSessionCatalogController() {
  const patchedAt = new Map<string, number>();
  let lastListObservation = 0;
  const state = createObservableState<SessionCatalogState>({
    sessions: [],
    revision: 0,
    hasSnapshot: false,
    activeSessionId: undefined,
  });

  return {
    getState: state.getState,
    subscribe: state.subscribe,
    beginRowRead(sessionId: string) {
      const before = state.getState();
      const previous = before.sessions.find(({ id }) => id === sessionId);
      return () => {
        const current = state.getState();
        const row = current.sessions.find(({ id }) => id === sessionId);
        return valuesEqual(row, previous) && (!!row || before.revision === current.revision);
      };
    },
    commitSessions(next: readonly DesktopSessionSummary[], observedAt = state.getState().revision): void {
      if (observedAt < lastListObservation) return;
      const current = state.getState();
      const previous = new Map(current.sessions.map((session) => [session.id, session]));
      const rows = new Map(next.map((session) => [session.id, session]));
      for (const [id, revision] of patchedAt) {
        if (revision > observedAt) {
          const row = previous.get(id);
          if (row) rows.set(id, row);
          else rows.delete(id);
        } else patchedAt.delete(id);
      }
      const sessions = [...rows.values()].map((row) => {
        const old = previous.get(row.id);
        return old && valuesEqual(old, row) ? old : row;
      }).sort(compareDesktopSessionCatalogSummaries);
      lastListObservation = observedAt;
      state.replaceState({ ...current, hasSnapshot: true,
        sessions: sessions.length === current.sessions.length && sessions.every((row, index) => row === current.sessions[index]) ? current.sessions : sessions,
        revision: current.revision + 1,
      });
    },
    commitPatch(sessionId: string, summary: DesktopSessionSummary | null): void {
      if (summary && summary.id !== sessionId) throw new Error('Session patch identity changed');
      const current = state.getState();
      const revision = current.revision + 1;
      patchedAt.set(sessionId, revision);
      const old = current.sessions.find(({ id }) => id === sessionId);
      const unchanged = summary ? old && valuesEqual(old, summary) : !old;
      state.replaceState({ ...current, revision,
        sessions: unchanged ? current.sessions : [
          ...current.sessions.filter(({ id }) => id !== sessionId), ...(summary ? [summary] : []),
        ].sort(compareDesktopSessionCatalogSummaries),
      });
    },
    setActiveSessionId(next: string | undefined): void {
      const current = state.getState();
      if (current.activeSessionId === next) return;
      state.replaceState({ ...current, activeSessionId: next });
    },
  };
}

export type SessionCatalogController = ReturnType<typeof createSessionCatalogController>;

export const selectSessions = (state: SessionCatalogState): readonly DesktopSessionSummary[] =>
  state.sessions;
export const selectCatalogRevision = (state: SessionCatalogState): number => state.revision;
export const selectActiveSessionId = (state: SessionCatalogState): string | undefined =>
  state.activeSessionId;

/**
 * The ids in the catalog, by value. A refresh replaces every row object even
 * when nothing about the membership moved (#2913), so an identity-only
 * selection would re-render every reader that only cares about which sessions
 * exist.
 */
export const selectAuthoritativeSessionIds = (
  state: SessionCatalogState,
): ReadonlySet<string> | undefined =>
  // The initial empty catalog cannot prove that persisted Sessions were deleted.
  state.hasSnapshot ? new Set(state.sessions.map(({ id }) => id)) : undefined;

/**
 * Owns the controller for the component's lifetime. Deliberately does NOT
 * subscribe: readers select what they need through `useExternalStoreSelector`.
 */
export function useSessionCatalogController(): SessionCatalogController {
  const controllerRef = useRef<SessionCatalogController | null>(null);
  if (!controllerRef.current) controllerRef.current = createSessionCatalogController();
  return controllerRef.current;
}

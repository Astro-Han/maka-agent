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


import type { DesktopSessionSummary } from '../../../preload/bridge-contract.js';
import type { SessionCatalogController, SessionPatchResult } from '../../application/contracts/session-catalog/session-catalog-state.js';
import { createSessionListRefresher } from '../../application/contracts/session-catalog/session-list-refresher.js';

export function createDesktopSessionCatalogSync(
  catalog: SessionCatalogController,
  normalize: (row: DesktopSessionSummary) => DesktopSessionSummary,
  onError: (error: unknown) => void,
  source: Pick<typeof window.maka, 'sessions'> = window.maka,
) {
  const list = createSessionListRefresher({
    observe: () => catalog.getState().revision,
    listSessions: () => source.sessions.list(),
    currentSessions: () => [...catalog.getState().sessions],
    commitSessions: (rows, observedAt) => catalog.commitSessions(rows.map(normalize), observedAt),
    onError,
  });
  const row = createSessionPatchDrain({
    begin: catalog.beginRowRead,
    read: (id) => source.sessions.get(id),
    commit: (id, summary) => catalog.commitPatch(id, summary && normalize(summary)),
  });
  return { refreshSessions: list.refresh, refreshSession: row.request };
}

/** Coalesce queued hints; each read returns its slot before another round. */
export function createSessionPatchDrain(options: {
  read(sessionId: string): Promise<DesktopSessionSummary | null>;
  begin(sessionId: string): () => boolean;
  commit(sessionId: string, summary: DesktopSessionSummary | null): void;
}) {
  type Row = {
    promise: Promise<SessionPatchResult>;
    resolve(result: SessionPatchResult): void;
    reject(error: unknown): void;
  };
  const pending = new Map<string, Row>();
  const active = new Set<string>();

  async function read(sessionId: string, row: Row): Promise<void> {
    try {
      const isCurrent = options.begin(sessionId);
      const summary = await options.read(sessionId);
      if (!isCurrent()) {
        row.resolve({ kind: 'superseded' });
      } else if (summary === null && pending.has(sessionId)) {
        // A later hint could announce creation; wait for that queued read
        // without occupying a slot or publishing an obsolete absence.
        void pending.get(sessionId)!.promise.then(row.resolve, row.reject);
      } else {
        options.commit(sessionId, summary);
        row.resolve({ kind: 'observed', session: summary });
      }
    } catch (error) {
      row.reject(error);
    } finally {
      active.delete(sessionId);
      pump();
    }
  }
  function pump(): void {
    for (const [id, row] of pending) {
      if (active.size >= 4) break;
      if (active.has(id)) continue;
      pending.delete(id);
      active.add(id);
      void read(id, row);
    }
  }
  return {
    request(sessionId: string): Promise<SessionPatchResult> {
      const existing = pending.get(sessionId);
      if (existing) return existing.promise;
      let resolve!: Row['resolve'];
      let reject!: Row['reject'];
      const promise = new Promise<SessionPatchResult>((accept, fail) => { resolve = accept; reject = fail; });
      pending.set(sessionId, { promise, resolve, reject });
      pump();
      return promise;
    },
  };
}

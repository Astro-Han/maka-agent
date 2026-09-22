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


import assert from 'node:assert/strict';
import test from 'node:test';
import { deferred } from '@maka/core/test-only/async-primitives';
import type { DesktopSessionSummary } from '../../shared/desktop-session-projection.js';
import { createSessionCatalogController, selectAuthoritativeSessionIds } from '../../renderer/application/contracts/session-catalog/session-catalog-state.js';
import { createSessionPatchDrain } from '../../renderer/platform/desktop/session-catalog-sync.js';
import { handleSessionChangedEvent } from '../../renderer/application/contracts/session-catalog/session-change-effects.js';
import { observeRevisionDraftRetirement } from '../../renderer/features/conversation/index.js';

function session(hostId: string, activityAt: number): DesktopSessionSummary {
  return {
    id: JSON.stringify([hostId, 'same-id']), runtimeHostId: hostId, profileId: hostId,
    profileName: hostId, profileKind: 'remote', revision: 1, activityAt,
    name: 'Task', status: 'active', isFlagged: false, isArchived: false,
    backend: 'ai-sdk', llmConnectionSlug: 'test', connectionLocked: false, model: 'test',
    hasUnread: false, labels: [], sandboxMode: 'workspace-write', approvalPolicy: { kind: 'on-request' },
  };
}

test('revision drafts wait for admission, retire once on observed removal, and release their subscription', () => {
  const catalog = createSessionCatalogController();
  const source = session('source', 1);
  const owner = session('draft', 2);
  catalog.commitSessions([source]);
  let retirements = 0;
  const stop = observeRevisionDraftRetirement(catalog,
    { sourceSessionId: source.id, draftSessionId: owner.id }, () => { retirements++; });
  catalog.commitSessions([source]);
  assert.equal(retirements, 0, 'a not-yet-published draft is not a deletion');
  catalog.commitPatch(owner.id, owner);
  catalog.commitPatch(owner.id, null);
  catalog.commitPatch(source.id, null);
  assert.equal(retirements, 1);
  stop();
  catalog.commitSessions([source, owner]);
  const stopNext = observeRevisionDraftRetirement(catalog,
    { sourceSessionId: source.id, draftSessionId: owner.id }, () => { retirements++; });
  stopNext();
  catalog.commitPatch(source.id, { ...source, isArchived: true });
  assert.equal(retirements, 1, 'a discarded draft no longer observes catalog changes');
});

test('row patches fence late lists, preserve ordering and do not prove unrelated deletion', () => {
  const catalog = createSessionCatalogController();
  const first = session('host-a', 10);
  const second = session('host-b', 20);
  catalog.commitPatch(first.id, first);
  assert.equal(selectAuthoritativeSessionIds(catalog.getState()), undefined);
  catalog.commitSessions([first, second]);
  const oldRowRead = catalog.beginRowRead(first.id);
  const observation = catalog.getState().revision;
  const promoted = { ...first, activityAt: 30, revision: 2 };
  catalog.commitPatch(first.id, promoted);
  assert.equal(oldRowRead(), false, 'a late get cannot replace a newer patch');
  catalog.commitSessions([second, first], observation);
  assert.deepEqual(catalog.getState().sessions, [promoted, second]);
  assert.equal(catalog.getState().sessions[0], promoted);
  const beforeDeletion = catalog.getState().revision;
  catalog.commitPatch(first.id, null);
  catalog.commitSessions([first, second], beforeDeletion);
  assert.deepEqual(catalog.getState().sessions, [second], 'a stale list must not resurrect a deleted row');
  catalog.commitSessions([second]);
  catalog.commitSessions([first, second], observation);
  assert.deepEqual(catalog.getState().sessions, [second], 'retired fences cannot admit an older list');
  const rows = catalog.getState().sessions;
  catalog.commitSessions([{ ...second }]);
  assert.equal(catalog.getState().sessions, rows, 'equal snapshots retain row and array identity');
});

test('a superseded creation read reconciles membership instead of losing the new row', async () => {
  const catalog = createSessionCatalogController();
  const existing = session('existing', 1);
  const created = session('created', 2);
  catalog.commitSessions([existing]);
  const isCurrent = catalog.beginRowRead(created.id);
  catalog.commitPatch(existing.id, { ...existing, revision: 2 });
  assert.equal(isCurrent(), false);
  handleSessionChangedEvent({ reason: 'created', sessionId: created.id, ts: 1 }, {
    activeIdRef: { current: undefined },
    refreshSession: async () => ({ kind: 'superseded' }),
    refreshSessions: async () => { catalog.commitSessions([...catalog.getState().sessions, created]); return [...catalog.getState().sessions]; },
    refreshProjects: async () => undefined, refreshMessages: async () => true,
    retireSession: () => assert.fail('supersession is not deletion'), retiredSessionIds: () => [],
    clearPendingTurnActionsForSession() {}, setSessionEventHealthBySession() {}, notifyModelRebound() {},
  });
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.ok(catalog.getState().sessions.some(({ id }) => id === created.id));
});

test('row reads coalesce invalidations, bound concurrency and never turn failure into absence', async () => {
  const reads: Array<{ id: string; result: ReturnType<typeof deferred<DesktopSessionSummary | null>> }> = [];
  const commits: Array<[string, DesktopSessionSummary | null]> = [];
  const drain = createSessionPatchDrain({
    begin: () => () => true,
    read: (id) => { const result = deferred<DesktopSessionSummary | null>(); reads.push({ id, result }); return result.promise; },
    commit: (id, row) => commits.push([id, row]),
  });
  const row = session('host-a', 10);
  const first = drain.request(row.id);
  const trailing = drain.request(row.id);
  assert.equal(drain.request(row.id), trailing);
  reads[0]!.result.resolve(null);
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.equal(commits.length, 0, 'a superseded absence cannot retire the row');
  reads[1]!.result.resolve(row);
  assert.deepEqual(await first, { kind: 'observed', session: row });
  assert.deepEqual(commits, [[row.id, row]]);

  const failures = Array.from({ length: 5 }, (_, i) => drain.request('unavailable-' + i));
  const settled = Promise.allSettled(failures);
  assert.equal(reads.length, 6, 'only four independent reads may be in flight');
  for (const read of reads.slice(2)) read.result.reject(new Error('offline'));
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.equal(reads.length, 7);
  reads[6]!.result.reject(new Error('offline'));
  assert.ok((await settled).every((result) => result.status === 'rejected'));
  assert.equal(commits.length, 1, 'network errors publish no deletion');

  const hot = session('hot', 20);
  const firstHot = drain.request(hot.id);
  const latestHot = drain.request(hot.id);
  reads[7]!.result.resolve(hot);
  assert.deepEqual(await firstHot, { kind: 'observed', session: hot }, 'continued hints must not starve positive observations');
  reads[8]!.result.resolve({ ...hot, revision: 2 });
  await latestHot;
  assert.equal(commits.length, 3);
});

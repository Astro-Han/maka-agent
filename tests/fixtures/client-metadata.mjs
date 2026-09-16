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
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { decodeStoredMessage } from '../../packages/core/src/session.ts';
import { watchSession } from './client-subscription.mjs';

const query = async (connection, sessionId) =>
  (await connection.request('session.catalog.query', { kind: 'get', sessionId }, 3000)).session;
const update = (connection, session, patch) =>
  connection.request(
    'session.metadata.update',
    { sessionId: session.id, expectedRevision: session.revision, patch },
    3000,
  );
const acknowledge = (connection, sessionId, readThroughMessageId) =>
  connection.request('session.read_marker.set', { sessionId, readThroughMessageId }, 3000);

function unchangedActivity(before, after) {
  for (const field of ['activityAt', 'lastMessageAt', 'status', 'statusUpdatedAt']) {
    assert.deepEqual(after[field], before[field], `${field} is independent of metadata/read ack`);
  }
}

export async function verifyMetadata(connection, initial, connectSibling) {
  const sibling = await connectSibling();
  const local = await watchSession(connection, initial.id, { kind: 'tail', maxBytes: 2 });
  const remote = await watchSession(sibling, initial.id, { kind: 'tail', maxBytes: 2 });
  const notices = [];
  const unsubscribe = sibling.subscribeSessionCatalogChanges((frame) => notices.push(frame));
  try {
    for (const patch of [
      {},
      { name: null },
      { labels: ['duplicate', 'duplicate'] },
      { extra: true },
    ]) {
      await assert.rejects(
        update(connection, initial, patch),
        'original input codec rejects invalid patches',
      );
    }
    await assert.rejects(
      connection.request(
        'session.read_marker.set',
        { sessionId: initial.id, readThroughMessageId: null },
        3000,
      ),
    );
    const changed = await update(connection, initial, {
      name: 'Cafe\u0301  会话',
      labels: ['mode:bot', 'review', 'mode:deep_research'],
      isFlagged: true,
    });
    assert.equal(changed.kind, 'committed');
    assert.equal(changed.session.revision, initial.revision + 1);
    assert.equal(changed.session.name, 'Café 会话');
    assert.deepEqual(changed.session.labels, ['review'], 'execution labels cannot be injected');
    assert.equal(changed.session.isFlagged, true);
    unchangedActivity(initial, changed.session);
    for (const observer of [local, remote]) {
      const frame = await observer.waitFor(
        (frame) =>
          frame.kind === 'subscription.session_projection' &&
          frame.snapshot.session.metadataRevision === changed.session.revision,
      );
      assert.equal(frame.snapshot.session.sessionId, initial.id);
      assert.equal(frame.snapshot.session.metadataRevision, changed.session.revision);
    }
    assert(
      notices.some((frame) => frame.sessionId === initial.id),
      'sibling receives catalog invalidation',
    );
    const after = await watchSession(sibling, initial.id, { kind: 'tail', maxBytes: 2 });
    assert.equal(
      after.subscription.transcriptBootstrap.durable.throughSequence,
      remote.subscription.transcriptBootstrap.durable.throughSequence,
      'metadata notification cannot rely on advancing the runtime log',
    );
    await after.close();
    const catalog = await connection.request('session.catalog.query', { kind: 'list_start' }, 3000);
    assert.deepEqual(
      await update(connection, changed.session, {
        name: 'Cafe\u0301 会话',
        labels: ['review'],
        isFlagged: true,
      }),
      changed,
    );
    assert.deepEqual(
      await update(connection, initial, { name: 'Café 会话' }),
      {
        kind: 'revision_conflict',
        expectedRevision: initial.revision,
        actualRevision: changed.session.revision,
      },
      'CAS precedes normalized no-op detection',
    );
    assert.deepEqual(await acknowledge(connection, initial.id, 'unknown-message'), changed.session);
    assert.deepEqual(
      await connection.request('session.catalog.query', { kind: 'list_start' }, 3000),
      catalog,
      'no-op and conflict preserve catalog generation',
    );
    // Restore the original title so existing creation/reopen assertions retain their meaning.
    const restored = await update(connection, changed.session, {
      name: initial.name,
      labels: [],
      isFlagged: false,
    });
    assert.equal(restored.kind, 'committed');
    assert.deepEqual(restored.session.labels, []);
    assert.equal(restored.session.isFlagged, false);
    console.log(JSON.stringify({ check: 'original-client-metadata', result: 'passed' }));
  } finally {
    unsubscribe();
    await local.close();
    await remote.close();
    await sibling.close();
  }
}

async function durableVisible(connection, sessionId) {
  const observer = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
  try {
    const subscription = observer.subscription;
    let page = subscription.transcriptBootstrap.durable;
    const messages = [];
    for (;;) {
      const decoded = await subscription.decodeTranscriptPage(page, decodeStoredMessage);
      messages.push(...decoded.messages);
      if (decoded.nextCursor === null) break;
      page = await subscription.loadTranscriptPage({
        direction: 'older',
        throughSequence: page.throughSequence,
        cursor: decoded.nextCursor,
        anchorSequence: null,
        maxBytes: 97,
      });
    }
    return messages
      .sort((a, b) => a.identity - b.identity)
      .map((entry) => entry.message)
      .filter((row) => ['user', 'assistant'].includes(row.type));
  } finally {
    await observer.close();
  }
}

export async function acknowledgeBusy(connection, sessionId) {
  const before = await query(connection, sessionId);
  assert.equal(before.status, 'running');
  assert.equal(before.hasUnread, true);
  const rows = await durableVisible(connection, sessionId);
  assert(rows.length > 1);
  assert.deepEqual(
    await acknowledge(connection, sessionId, rows[0].id),
    before,
    'stale ID succeeds unchanged',
  );
  assert.deepEqual(await acknowledge(connection, sessionId, 'unknown-message'), before);
  const acknowledged = await acknowledge(connection, sessionId, rows.at(-1).id);
  assert.equal(acknowledged.hasUnread, false);
  assert.equal(acknowledged.lastReadMessageId, rows.at(-1).id);
  unchangedActivity(before, acknowledged);
  const changed = await update(connection, acknowledged, { isFlagged: true });
  assert.equal(changed.kind, 'committed', 'busy session allows metadata update');
  assert.equal(changed.session.isFlagged, true);
  assert.equal(changed.session.hasUnread, false);
  assert.equal(changed.session.status, 'running');
}

export async function persistReadMarker(connection, sessionId, workspace, reopened) {
  const path = join(workspace, 'read-marker-before-restart.json');
  const before = await query(connection, sessionId);
  const rows = await durableVisible(connection, sessionId);
  const tailId = rows.at(-1).id;
  if (reopened) {
    assert.deepEqual(
      before,
      JSON.parse(await readFile(path, 'utf8')),
      'ack and metadata survive restart',
    );
  } else {
    assert.equal(before.hasUnread, true);
  }
  const acknowledged = await acknowledge(connection, sessionId, tailId);
  assert.equal(acknowledged.hasUnread, false);
  assert.equal(acknowledged.lastReadMessageId, tailId);
  assert.equal(acknowledged.isFlagged, true);
  unchangedActivity(before, acknowledged);
  assert.deepEqual(
    await acknowledge(connection, sessionId, tailId),
    acknowledged,
    'repeat ack does not change revision',
  );
  if (reopened) assert.deepEqual(acknowledged, before);
  else await writeFile(path, JSON.stringify(acknowledged));
  console.log(
    JSON.stringify({
      check: 'original-client-read-marker',
      reopened: Boolean(reopened),
      result: 'passed',
    }),
  );
}

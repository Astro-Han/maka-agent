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
import { watchSession } from './client-subscription.mjs';

const query = async (connection, sessionId) =>
  (await connection.request('session.catalog.query', { kind: 'get', sessionId }, 3000)).session;
const boundary = (connection, sessionId) =>
  connection.request('session.execution_boundary.query', { sessionId }, 3000);
const update = (connection, session, patch) =>
  connection.request(
    'session.configuration.update',
    {
      sessionId: session.id,
      expectedRevision: session.revision,
      patch,
    },
    3000,
  );
const conflict = (session, expectedRevision) => ({
  kind: 'revision_conflict',
  expectedRevision,
  actualRevision: session.revision,
});

export async function verifyConfiguration(connection, sessionId, connectSibling) {
  const initial = await query(connection, sessionId);
  const genesis = { kind: 'managed', access: 'writable', revision: 0 };
  assert.deepEqual(await boundary(connection, sessionId), genesis);
  await assert.rejects(
    boundary(connection, 'missing-session'),
    (error) => error.code === 'not_found',
  );
  const sibling = await connectSibling();
  const observer = await watchSession(sibling, sessionId, { kind: 'tail', maxBytes: 2 });
  const notices = [];
  const unsubscribe = sibling.subscribeSessionCatalogChanges((frame) => notices.push(frame));
  try {
    for (const patch of [
      {},
      { modelTarget: { kind: 'default' } },
      { permissionMode: null },
      { thinkingLevel: 'unknown' },
      { extra: true },
    ]) {
      await assert.rejects(
        update(connection, initial, patch),
        'original codec rejects malformed patch',
      );
    }
    const target = {
      kind: 'explicit',
      connectionId: initial.llmConnectionId,
      connectionSlug: initial.llmConnectionSlug,
      model: initial.model,
    };
    for (const modelTarget of [
      { ...target, connectionSlug: 'wrong-slug' },
      { ...target, connectionId: 'missing-connection' },
    ]) {
      await assert.rejects(
        update(connection, initial, { modelTarget }),
        (error) => error.code === 'operation_conflict',
      );
    }
    await assert.rejects(
      update(connection, initial, { thinkingLevel: 'max' }),
      (error) => error.code === 'invalid_request',
    );
    await assert.rejects(
      update(connection, initial, { collaborationMode: 'plan' }),
      (error) => error.code === 'operation_unavailable',
    );
    assert.deepEqual(await query(connection, sessionId), initial);

    const bound = await update(connection, initial, { modelTarget: target });
    assert.equal(bound.kind, 'committed');
    assert.equal(bound.session.revision, initial.revision + 1);
    assert.equal(bound.session.connectionLocked, true);
    assert.equal(Object.hasOwn(bound.session, 'thinkingLevel'), false);
    assert.deepEqual(
      bound.session,
      {
        ...initial,
        revision: initial.revision + 1,
        connectionLocked: true,
      },
      'idle configuration preserves every metadata, activity and runtime field',
    );
    const frame = await observer.waitFor(
      (frame) =>
        frame.kind === 'subscription.session_projection' &&
        frame.snapshot.session.metadataRevision === bound.session.revision,
    );
    assert.equal(frame.snapshot.session.sessionId, sessionId);
    assert.deepEqual(
      await query(sibling, sessionId),
      bound.session,
      'invalidation exposes full updated configuration to other clients',
    );
    assert(notices.some((frame) => frame.sessionId === sessionId));
    const catalog = await connection.request('session.catalog.query', { kind: 'list_start' }, 3000);
    assert.deepEqual(await update(connection, bound.session, { modelTarget: target }), bound);
    assert.deepEqual(
      await update(connection, initial, { modelTarget: target }),
      conflict(bound.session, initial.revision),
      'CAS precedes no-op',
    );
    assert.deepEqual(
      await connection.request('session.catalog.query', { kind: 'list_start' }, 3000),
      catalog,
      'no-op and conflict do not invalidate catalog',
    );

    const explore = await update(connection, bound.session, { permissionMode: 'explore' });
    assert.deepEqual(await boundary(connection, sessionId), {
      kind: 'managed',
      access: 'read_only',
      revision: 1,
    });
    assert.equal(explore.kind, 'committed');
    assert.equal(
      Object.hasOwn(explore.session, 'thinkingLevel'),
      false,
      'absent thinking patch preserves absence',
    );
    const clear = await update(connection, explore.session, { thinkingLevel: null });
    assert.equal(clear.kind, 'committed');
    assert.deepEqual(clear, explore, 'clearing absent thinking is a semantic no-op');
    assert.equal(Object.hasOwn(clear.session, 'thinkingLevel'), false, 'null clears to absent');
    assert.equal(clear.session.connectionLocked, true);
    assert.deepEqual(await update(connection, clear.session, { thinkingLevel: null }), clear);
    const swarm = await update(connection, clear.session, { orchestrationMode: 'swarm' });
    assert.equal(swarm.kind, 'committed');
    assert.equal(swarm.session.orchestrationMode, 'swarm');
    await assert.rejects(
      connection.request(
        'turn.start',
        {
          sessionId,
          turnId: 'unsupported-swarm',
          content: { text: 'must not execute' },
        },
        3000,
      ),
      (error) => error.code === 'operation_unavailable',
    );
    assert.deepEqual(
      await query(connection, sessionId),
      swarm.session,
      'unsupported execution does not create a turn or change Session',
    );
    const restored = await update(connection, swarm.session, { orchestrationMode: 'default' });
    assert.equal(restored.kind, 'committed');
    assert.equal(restored.session.permissionMode, 'explore');
    assert.deepEqual(
      await boundary(sibling, sessionId),
      { kind: 'managed', access: 'read_only', revision: 1 },
      'model/metadata/no-op/rejected updates do not advance boundary',
    );
    const bypass = await update(connection, restored.session, { permissionMode: 'bypass' });
    assert.deepEqual(await boundary(sibling, sessionId), { kind: 'bypass', revision: 2 });
    await update(connection, bypass.session, { permissionMode: 'explore' });
    assert.deepEqual(
      await boundary(connection, sessionId),
      { kind: 'managed', access: 'read_only', revision: 3 },
      'policy ABA is not the old boundary',
    );
    const after = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
    assert.equal(
      after.subscription.transcriptBootstrap.durable.throughSequence,
      observer.subscription.transcriptBootstrap.durable.throughSequence,
      'configuration changes do not append runtime events',
    );
    await after.close();
    await sibling.status(3000);
    for (let index = 1; index < notices.length; index++)
      assert(notices[index].revision > notices[index - 1].revision);
    console.log(JSON.stringify({ check: 'original-client-configuration', result: 'passed' }));
  } finally {
    unsubscribe();
    await observer.close();
    await sibling.close();
  }
}

export async function restoreAsk(connection, sessionId) {
  const before = await query(connection, sessionId);
  assert.equal(before.permissionMode, 'explore');
  const result = await update(connection, before, { permissionMode: 'ask' });
  assert.equal(result.kind, 'committed');
  assert.equal(result.session.permissionMode, 'ask');
  assert.deepEqual(await boundary(connection, sessionId), {
    kind: 'managed',
    access: 'writable',
    revision: 4,
  });
}

export async function verifyBusyConfiguration(connection, sessionId) {
  const before = await query(connection, sessionId);
  const beforeBoundary = await boundary(connection, sessionId);
  assert.equal(before.status, 'running');
  assert.deepEqual(
    await update(
      connection,
      { ...before, revision: before.revision - 1 },
      { permissionMode: 'explore' },
    ),
    conflict(before, before.revision - 1),
    'CAS precedes busy',
  );
  assert.deepEqual(
    await update(connection, before, { permissionMode: 'ask' }),
    { kind: 'committed', session: before },
    'busy semantic no-op succeeds',
  );
  await assert.rejects(
    update(connection, before, { permissionMode: 'explore' }),
    (error) => error.code === 'session_busy',
  );
  assert.deepEqual(await query(connection, sessionId), before);
  assert.deepEqual(await boundary(connection, sessionId), beforeBoundary);
}

export async function verifyArchivedConfiguration(connection, session) {
  assert.deepEqual(
    await boundary(connection, session.id),
    { kind: 'managed', access: 'writable', revision: 4 },
    'boundary is durable through runtime events, archive and host reopen',
  );
  assert.equal(session.isArchived, true);
  assert.equal(session.connectionLocked, true);
  assert.equal(session.permissionMode, 'ask');
  assert.equal(session.orchestrationMode, 'default');
  assert.equal(Object.hasOwn(session, 'thinkingLevel'), false);
  assert.deepEqual(
    await update(
      connection,
      { ...session, revision: session.revision - 1 },
      { permissionMode: 'ask' },
    ),
    conflict(session, session.revision - 1),
    'CAS precedes archived',
  );
  await assert.rejects(
    update(connection, session, { permissionMode: 'ask' }),
    (error) => error.code === 'operation_conflict',
    'archived rejects even semantic no-op',
  );
  assert.deepEqual(await query(connection, session.id), session);
}

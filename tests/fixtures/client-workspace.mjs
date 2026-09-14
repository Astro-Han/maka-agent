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
import { mkdir, readFile, realpath, symlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { watchSession } from './client-subscription.mjs';

const query = async (connection, sessionId) =>
  (await connection.request('session.catalog.query', { kind: 'get', sessionId }, 3000)).session;
const relocate = (connection, session, workspace) =>
  connection.request(
    'session.workspace.relocate',
    { sessionId: session.id, expectedRevision: session.revision, workspace },
    3000,
  );
const host = (path) => ({ kind: 'host_path', path });
const conflict = (session, expectedRevision = session.revision - 1) => ({
  kind: 'revision_conflict',
  expectedRevision,
  actualRevision: session.revision,
});

export async function verifyWorkspace(connection, sessionId, workspace) {
  const initial = await query(connection, sessionId);
  const directory = join(workspace, 'relocated-' + sessionId);
  const destination = join(directory, 'actual', 'nested');
  const alias = join(directory, 'alias');
  await mkdir(destination, { recursive: true });
  await symlink(destination, alias);
  const file = join(directory, 'not-a-directory');
  await writeFile(file, 'not a workspace');
  const observer = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
  const notices = [];
  const unsubscribe = connection.subscribeSessionCatalogChanges((frame) => notices.push(frame));
  try {
    const catalog = await connection.request('session.catalog.query', { kind: 'list_start' }, 3000);
    for (const path of [join(directory, 'missing'), file]) {
      assert.deepEqual(
        await relocate(connection, { ...initial, revision: initial.revision + 1 }, host(path)),
        conflict(initial, initial.revision + 1),
        'initial CAS precedes filesystem resolution',
      );
      await assert.rejects(
        relocate(connection, initial, host(path)),
        (error) => error.code === 'invalid_request',
      );
    }
    await assert.rejects(
      relocate(connection, initial, { kind: 'project', projectId: 'uninstalled-project' }),
      (error) => error.code === 'not_found',
      'an unknown project cannot authorize a workspace',
    );
    assert.deepEqual(await query(connection, sessionId), initial);
    assert.deepEqual(
      await connection.request('session.catalog.query', { kind: 'list_start' }, 3000),
      catalog,
    );
    const canonical = await realpath(destination);
    const changed = await relocate(connection, initial, host(alias));
    assert.deepEqual(
      changed,
      {
        kind: 'committed',
        session: {
          ...initial,
          revision: initial.revision + 1,
          workspace: { target: host(canonical), hostCwd: canonical },
        },
      },
      'relocation preserves all metadata, activity and recorded execution state',
    );
    await observer.waitFor(
      (frame) =>
        frame.kind === 'subscription.session_projection' &&
        frame.snapshot.session.metadataRevision === changed.session.revision,
    );
    await connection.status(3000);
    assert(notices.some((frame) => frame.sessionId === sessionId));
    const changedCatalog = await connection.request(
      'session.catalog.query',
      { kind: 'list_start' },
      3000,
    );
    assert.notEqual(changedCatalog.revision, catalog.revision);
    // Keep the raw parent components: Node join would erase the behavior being tested.
    for (const path of [canonical, alias, alias + '/../actual/nested']) {
      assert.deepEqual(await relocate(connection, changed.session, host(path)), changed);
    }
    assert.deepEqual(
      await relocate(connection, initial, host(alias)),
      conflict(changed.session),
      'CAS precedes canonical no-op',
    );
    await connection.status(3000);
    assert.deepEqual(
      await connection.request('session.catalog.query', { kind: 'list_start' }, 3000),
      changedCatalog,
      'canonical aliases do not advance Session or catalog revision',
    );
    const after = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
    try {
      assert.equal(
        after.subscription.transcriptBootstrap.throughSequence,
        observer.subscription.transcriptBootstrap.throughSequence,
        'relocation and no-op do not append runtime events',
      );
      assert.deepEqual(
        after.subscription.snapshot.rootTurn,
        observer.subscription.snapshot.rootTurn,
      );
    } finally {
      await after.close();
    }
    return canonical;
  } finally {
    unsubscribe();
    await observer.close();
  }
}

export async function verifyBlockedWorkspace(connection, session) {
  const workspace = session.workspace.target;
  assert.deepEqual(
    await relocate(connection, { ...session, revision: session.revision - 1 }, workspace),
    conflict(session),
    'CAS precedes busy and archived policy',
  );
  await assert.rejects(
    relocate(connection, session, workspace),
    (error) => error.code === (session.isArchived ? 'operation_conflict' : 'session_busy'),
    'busy or archived rejects even identical workspace',
  );
  assert.deepEqual(await query(connection, session.id), session);
}

export async function persistWorkspace(connection, sessionId, workspace, reopened) {
  const session = await query(connection, sessionId);
  const snapshot = join(workspace, sessionId + '-workspace.json');
  if (reopened) {
    assert.deepEqual(
      session,
      JSON.parse(await readFile(snapshot, 'utf8')),
      'reopen preserves exact Session including relocated workspace and activity',
    );
  } else {
    await writeFile(snapshot, JSON.stringify(session));
  }
}

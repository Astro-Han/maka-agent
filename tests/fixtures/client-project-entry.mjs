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
import { connect } from 'node:net';
import { once } from 'node:events';
import { parseArgs } from 'node:util';
import { mkdir, readFile, realpath, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { connectRuntimeHostMessageTransport } from '../../packages/runtime-host/src/client/connection.ts';
import { FramedTransport } from '../../packages/runtime-host/src/transport/framed-transport.ts';
import { configureModel } from './client-runtime-policy-fixture.mjs';

const { values } = parseArgs({
  options: {
    socket: { type: 'string' },
    'root-id': { type: 'string' },
    'project-workspace': { type: 'string' },
    reopened: { type: 'boolean' },
  },
});
const socket = connect(values.socket);
const transport = new FramedTransport(socket);
let connection;
try {
  await once(socket, 'connect');
  const connected = await connectRuntimeHostMessageTransport({
    transport,
    expectedRootId: values['root-id'],
    compositionId: 'maka.interactive',
    protocol: { min: 0, max: 0 },
    handshakeTimeoutMs: 3000,
    livenessIntervalMs: 60000,
  });
  assert.equal(connected.kind, 'connected');
  connection = connected.connection;
  await workflow(connection, values['project-workspace'], values.reopened);
  console.log(values.reopened ? 'project-reopened' : 'project-passed');
} finally {
  transport.abort();
  await connection?.close();
}

async function workflow(connection, workspace, reopened) {
  const request = (operation, input) => connection.request(operation, input, 5000);
  const query = (input) => request('project.catalog.query', input);
  const mutate = (input) => request('project.catalog.mutate', input);
  const session = async (id) =>
    (await request('session.catalog.query', { kind: 'get', sessionId: id })).session;
  const notices = [];
  const sessionNotices = [];
  connection.subscribeProjectCatalogChanges((revision) => notices.push(revision));
  connection.subscribeSessionCatalogChanges((frame) => sessionNotices.push(frame));
  const file = join(workspace, 'project-fixture.json');
  if (reopened) {
    const saved = JSON.parse(await readFile(file, 'utf8'));
    assert.deepEqual(await query({ kind: 'list_start', view: 'locations' }), saved.page);
    for (const { input, snapshot } of saved.sessions) {
      assert.deepEqual(await session(input.sessionId), snapshot);
      assert.deepEqual(await request('session.create', input), snapshot);
    }
    await connection.status(3000);
    assert.deepEqual(notices, [], 'observation and exact retries do not touch projects');
    return;
  }
  assert.deepEqual(await query({ kind: 'directory_roots' }), {
    kind: 'directory_roots',
    roots: [{ id: 'root-1', label: 'Fixture' }],
  });
  const names = [
    'project-a',
    'project-b',
    '\u{10000}',
    '\u{e000}',
    ...Array.from({ length: 130 }, (_, index) => 'directory-' + String(index).padStart(3, '0')),
  ];
  await Promise.all(names.map((name) => mkdir(join(workspace, name))));
  let directory = await query({ kind: 'directory_list_start', rootId: 'root-1', segments: [] });
  assert.equal(directory.entries.length, 128);
  const listed = directory.entries.map(({ name }) => name);
  while (directory.nextCursor !== null) {
    directory = await query({
      kind: 'directory_list_continue',
      rootId: 'root-1',
      segments: [],
      cursor: directory.nextCursor,
    });
    listed.push(...directory.entries.map(({ name }) => name));
  }
  assert.deepEqual(listed, names.toSorted());
  await assert.rejects(
    query({ kind: 'directory_list_start', rootId: 'unknown', segments: [] }),
    (error) => error.code === 'invalid_request',
  );
  const a = (
    await mutate({ kind: 'register_directory', rootId: 'root-1', segments: ['project-a'] })
  ).project;
  const b = (await mutate({ kind: 'register', path: join(workspace, 'project-b') })).project;
  assert.notEqual(a.id, b.id);
  assert.equal(
    (await mutate({ kind: 'register', path: join(workspace, 'project-a') })).project.id,
    a.id,
  );
  await configureModel(request);
  const create = (id, projectId) => ({
    sessionId: id,
    workspace: { kind: 'project', projectId },
    modelTarget: { kind: 'default' },
  });
  const inputA = create('project-session-a', a.id),
    inputB = create('project-session-b', b.id);
  const first = await request('session.create', inputA);
  const second = await request('session.create', inputB);
  assert.equal(first.workspace.hostCwd, await realpath(join(workspace, 'project-a')));
  const merged = (
    await mutate({ kind: 'relink', projectId: a.id, path: join(workspace, 'project-b') })
  ).project;
  assert.equal(merged.id, a.id);
  assert.deepEqual(merged.aliases, [b.id]);
  const afterA = await session(first.id),
    afterB = await session(second.id);
  assert.deepEqual(afterA, {
    ...first,
    revision: first.revision + 1,
    workspace: {
      target: { kind: 'project', projectId: a.id },
      hostCwd: second.workspace.hostCwd,
    },
  });
  assert.deepEqual(afterB, {
    ...second,
    revision: second.revision + 1,
    workspace: {
      ...second.workspace,
      target: { kind: 'project', projectId: a.id },
    },
  });
  assert.deepEqual(await request('session.create', inputA), afterA);
  const aliasInput = create('project-session-alias', b.id);
  const alias = await request('session.create', aliasInput);
  assert.equal(alias.workspace.target.projectId, a.id);
  const hostInput = {
    sessionId: 'project-session-relocated',
    workspace: { kind: 'host_path', path: workspace },
    modelTarget: { kind: 'default' },
  };
  const standalone = await request('session.create', hostInput);
  const moved = await request('session.workspace.relocate', {
    sessionId: standalone.id,
    expectedRevision: standalone.revision,
    workspace: { kind: 'project', projectId: b.id },
  });
  assert.equal(moved.kind, 'committed');
  assert.equal(moved.session.workspace.target.projectId, a.id);
  assert.equal(moved.session.workspace.hostCwd, second.workspace.hostCwd);
  await mutate({ kind: 'archive', projectId: b.id });
  await assert.rejects(
    request('session.create', create('archived-project-session', a.id)),
    (error) => error.code === 'operation_conflict',
  );
  await assert.rejects(
    request('session.create', create('missing-project-session', 'absent')),
    (error) => error.code === 'operation_conflict',
  );
  await mutate({ kind: 'restore', projectId: a.id });

  for (const name of names.slice(4, 39))
    await mutate({ kind: 'register', path: join(workspace, name) });
  const start = await query({ kind: 'list_start', view: 'locations' });
  const archived = await request('session.lifecycle.set', {
    sessionId: standalone.id,
    state: 'archived',
  });
  await connection.status(3000);
  const beforeRejectedRelocation = notices.length;
  await assert.rejects(
    request('session.workspace.relocate', {
      sessionId: archived.id,
      expectedRevision: archived.revision,
      workspace: { kind: 'project', projectId: a.id },
    }),
    (error) => error.code === 'operation_conflict',
  );
  assert.deepEqual(await query({ kind: 'list_start', view: 'locations' }), start);
  await connection.status(3000);
  assert.equal(
    notices.length,
    beforeRejectedRelocation,
    'rejected relocation must not record Project usage',
  );
  assert.equal(start.items.length, 64);
  assert.notEqual(start.nextCursor, null);
  const rest = await query({
    kind: 'list_continue',
    view: 'locations',
    revision: start.revision,
    cursor: start.nextCursor,
  });
  assert.equal(rest.nextCursor, null);
  const items = [...start.items, ...rest.items];
  assert.equal(items.filter((item) => item.kind === 'project').length, start.projectCount);
  const summary = await query({ kind: 'list_start', view: 'summary' });
  assert(summary.items.every((item) => item.kind !== 'location'));
  for (const cursor of ['01', '1x', '-1', '99999999999999999999']) {
    await assert.rejects(
      query({ kind: 'list_continue', view: 'locations', revision: start.revision, cursor }),
      (error) => error.code === 'invalid_request',
    );
  }
  await mutate({ kind: 'rename', projectId: b.id, name: 'A renamed project' });
  const changed = await query({
    kind: 'list_continue',
    view: 'locations',
    revision: start.revision,
    cursor: start.nextCursor,
  });
  assert.equal(changed.kind, 'revision_changed');
  assert.equal(changed.expected, start.revision);
  assert.notEqual(changed.actual, start.revision);
  await connection.status(3000);
  assert(notices.length >= 40);
  for (let i = 1; i < notices.length; i++) assert(notices[i] > notices[i - 1]);
  assert(sessionNotices.some((notice) => notice.sessionId === first.id));
  assert(sessionNotices.some((notice) => notice.sessionId === second.id));
  const saved = { page: await query({ kind: 'list_start', view: 'locations' }), sessions: [] };
  for (const input of [inputA, inputB, aliasInput, hostInput]) {
    saved.sessions.push({ input, snapshot: await session(input.sessionId) });
  }
  await writeFile(file, JSON.stringify(saved));
}

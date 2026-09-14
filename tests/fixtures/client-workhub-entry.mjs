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
import { access, readFile, rmdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { connectRuntimeHostMessageTransport } from '../../packages/runtime-host/src/client/connection.ts';
import { FramedTransport } from '../../packages/runtime-host/src/transport/framed-transport.ts';
import { configureModel } from './client-runtime-policy-fixture.mjs';
import { verifyWorkhubAnswer } from './client-workhub-answer.mjs';
import { verifyWorkhubDelegation } from './client-workhub-delegation.mjs';

const { values } = parseArgs({
  options: {
    socket: { type: 'string' },
    'root-id': { type: 'string' },
    'workhub-workspace': { type: 'string' },
    'workhub-answer-workspace': { type: 'string' },
    'workhub-delegation-workspace': { type: 'string' },
    'workhub-creation-workspace': { type: 'string' },
    'workhub-selection-workspace': { type: 'string' },
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
  if (
    values['workhub-delegation-workspace'] ||
    values['workhub-creation-workspace'] ||
    values['workhub-selection-workspace']
  ) {
    await verifyWorkhubDelegation(
      connection,
      values['workhub-delegation-workspace'] ??
        values['workhub-creation-workspace'] ??
        values['workhub-selection-workspace'],
      values.reopened,
      values['workhub-creation-workspace']
        ? 'created'
        : values['workhub-selection-workspace']
          ? 'selected'
          : 'existing',
    );
    console.log(values.reopened ? 'workhub-delegation-reopened' : 'workhub-delegation-passed');
  } else if (values['workhub-answer-workspace']) {
    await verifyWorkhubAnswer(connection, values['workhub-answer-workspace'], values.reopened);
    console.log(values.reopened ? 'workhub-answer-reopened' : 'workhub-answer-passed');
  } else {
    const request = (operation, input) => connection.request(operation, input, 5000);
    const resolve = () => request('workhub.coordination.resolve', {});
    const query = () => request('workhub.coordination.query', {});
    const configure = (input) => request('workhub.coordination.configureModel', input);
    const sessionId = 'maka_workhub_coordination';
    const file = join(values['workhub-workspace'], 'workhub.json');
    if (values.reopened) {
      const saved = JSON.parse(await readFile(file, 'utf8'));
      assert.deepEqual(await query(), saved);
      assert.deepEqual(await resolve(), { sessionId });
      assert.deepEqual(await query(), saved, 'resolve does not rebind a removed default model');
      console.log('workhub-reopened');
    } else {
      await assert.rejects(query(), (e) => e.code === 'persistence_failed');
      await assert.rejects(resolve(), (e) => e.code === 'operation_conflict');
      await assert.rejects(query(), (e) => e.code === 'persistence_failed');
      const { connection: model } = await configureModel(request);
      assert.deepEqual(await resolve(), { sessionId });
      const initial = await query();
      assert.equal(initial.id, sessionId);
      assert.equal(initial.name, 'WorkHub');
      assert.equal(initial.permissionMode, 'bypass');
      assert.equal(initial.collaborationMode, 'agent');
      assert.equal(initial.orchestrationMode, 'default');
      assert.deepEqual(await resolve(), { sessionId });
      assert.deepEqual(await query(), initial);
      const input = {
        expectedRevision: initial.revision,
        modelTarget: {
          kind: 'explicit',
          connectionId: model.connectionId,
          connectionSlug: initial.llmConnectionSlug,
          model: 'fixture-model',
        },
      };
      const configured = await configure(input);
      assert.equal(configured.kind, 'committed');
      assert.equal(configured.session.connectionLocked, true);
      assert(configured.session.revision > initial.revision);
      assert.deepEqual(await configure(input), {
        kind: 'revision_conflict',
        expectedRevision: initial.revision,
        actualRevision: configured.session.revision,
      });
      assert.deepEqual(
        await configure({ ...input, expectedRevision: configured.session.revision }),
        configured,
      );
      for (const [operation, value] of [
        ['session.lifecycle.set', { sessionId, state: 'archived' }],
        [
          'session.configuration.update',
          {
            sessionId,
            expectedRevision: configured.session.revision,
            patch: { permissionMode: 'ask' },
          },
        ],
        [
          'session.metadata.update',
          {
            sessionId,
            expectedRevision: configured.session.revision,
            patch: { name: 'not WorkHub' },
          },
        ],
        [
          'session.create',
          { sessionId, workspace: initial.workspace.target, modelTarget: { kind: 'default' } },
        ],
        ['turn.start', { sessionId, turnId: 'forbidden', content: { text: 'do not execute' } }],
        ['turn.resume.query', { sessionId }],
      ]) {
        const code = ['session.metadata.update', 'turn.resume.query'].includes(operation)
          ? 'operation_unavailable'
          : 'operation_conflict';
        await assert.rejects(request(operation, value), (e) => e.code === code, operation);
      }
      assert.deepEqual(
        await query(),
        configured.session,
        'rejected generic mutations leave WorkHub intact',
      );
      // Only the empty fixture-owned directory is removed. Query must remain read-only.
      await rmdir(initial.workspace.hostCwd);
      assert.deepEqual(await query(), configured.session);
      await assert.rejects(access(initial.workspace.hostCwd), (e) => e.code === 'ENOENT');
      assert.deepEqual(await resolve(), { sessionId });
      await access(initial.workspace.hostCwd);
      await request('connection.catalog.remove', {
        expected: { connectionId: model.connectionId, revision: model.revision },
      });
      assert.deepEqual(await resolve(), { sessionId });
      assert.deepEqual(await query(), configured.session);
      await writeFile(file, JSON.stringify(configured.session));
      console.log('workhub-passed');
    }
  }
} finally {
  transport.abort();
  await connection?.close();
}

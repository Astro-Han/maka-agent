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
import { setTimeout as delay } from 'node:timers/promises';
import { access, mkdir, readFile, rmdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { connectRuntimeHostMessageTransport } from '../../packages/runtime-host/src/client/connection.ts';
import { FramedTransport } from '../../packages/runtime-host/src/transport/framed-transport.ts';
import { configureModel } from './client-runtime-policy-fixture.mjs';
import { verifyWorkhubAnswer } from './client-workhub-answer.mjs';
import { verifyWorkhubQueue } from './client-workhub-queue.mjs';
import { verifyWorkhubDelegation } from './client-workhub-delegation.mjs';
import { toggleWorkhub, workhubRemote } from './client-workhub-plugin.mjs';

const { values } = parseArgs({
  options: {
    socket: { type: 'string' },
    'root-id': { type: 'string' },
    'workhub-workspace': { type: 'string' },
    'workhub-answer-workspace': { type: 'string' },
    'workhub-queue-workspace': { type: 'string' },
    'workhub-delegation-workspace': { type: 'string' },
    'workhub-creation-workspace': { type: 'string' },
    'workhub-selection-workspace': { type: 'string' },
    'workhub-stop-workspace': { type: 'string' },
    'workhub-steering-workspace': { type: 'string' },
    'workhub-resume-workspace': { type: 'string' },
    'workhub-correction-workspace': { type: 'string' },
    'workhub-correction-creation-workspace': { type: 'string' },
    reopened: { type: 'boolean' },
  },
});
const socket = connect(values.socket);
const transport = new FramedTransport(socket);
let connection;
const remotes = [];
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
    values['workhub-selection-workspace'] ||
    values['workhub-stop-workspace'] ||
    values['workhub-steering-workspace'] ||
    values['workhub-resume-workspace'] ||
    values['workhub-correction-workspace'] ||
    values['workhub-correction-creation-workspace']
  ) {
    await verifyWorkhubDelegation(
      connection,
      values['workhub-delegation-workspace'] ??
        values['workhub-creation-workspace'] ??
        values['workhub-selection-workspace'] ??
        values['workhub-stop-workspace'] ??
        values['workhub-steering-workspace'] ??
        values['workhub-resume-workspace'] ??
        values['workhub-correction-workspace'] ??
        values['workhub-correction-creation-workspace'],
      values.reopened,
      values['workhub-creation-workspace']
        ? 'created'
        : values['workhub-selection-workspace']
          ? 'selected'
          : values['workhub-stop-workspace']
            ? 'stopped'
            : values['workhub-steering-workspace']
              ? 'steered'
              : values['workhub-resume-workspace']
                ? 'resumed'
                : values['workhub-correction-workspace']
                  ? 'corrected'
                  : values['workhub-correction-creation-workspace']
                    ? 'corrected_created'
                    : 'existing',
    );
    console.log(values.reopened ? 'workhub-delegation-reopened' : 'workhub-delegation-passed');
  } else if (values['workhub-queue-workspace']) {
    await verifyWorkhubQueue(connection);
    console.log('workhub-queue-passed');
  } else if (values['workhub-answer-workspace']) {
    await verifyWorkhubAnswer(connection, values['workhub-answer-workspace'], values.reopened);
    console.log(values.reopened ? 'workhub-answer-reopened' : 'workhub-answer-passed');
  } else {
    const request = (operation, input) => connection.request(operation, input, 5000);
    let remote = await workhubRemote(connection);
    remotes.push(remote);
    const resolve = () => remote.method('resolve')();
    const query = () => remote.method('query')();
    const configure = (input) => remote.method('configure-model')(input);
    const notices = [];
    connection.subscribeSessionCatalogChanges((notice) => notices.push(notice));
    const observed = async (action) => {
      const count = notices.length;
      const result = await action();
      const deadline = Date.now() + 3000;
      while (notices.length === count) {
        assert(Date.now() < deadline, 'Remote mutation must publish a Session notice');
        await delay(10);
      }
      assert.equal(notices.at(-1).sessionId, 'maka_workhub_coordination');
      return result;
    };
    const sessionId = 'maka_workhub_coordination';
    const file = join(values['workhub-workspace'], 'workhub.json');
    if (values.reopened) {
      const saved = JSON.parse(await readFile(file, 'utf8'));
      const legacy = await query();
      assert.deepEqual(legacy, {
        ...saved,
        orchestrationMode: 'default',
        revision: legacy.revision,
      });
      assert.deepEqual(await query(), legacy, 'query cannot upgrade persisted configuration');
      assert.deepEqual(await observed(() => resolve()), { sessionId });
      const upgraded = await query();
      assert(upgraded.revision > legacy.revision);
      assert.deepEqual(upgraded, { ...saved, revision: upgraded.revision });
      assert.deepEqual(await resolve(), { sessionId });
      assert.deepEqual(await query(), upgraded, 'resolve does not rebind a removed default model');
      console.log('workhub-reopened');
    } else {
      await assert.rejects(query(), (e) => e.code === 'persistence_failed');
      await assert.rejects(resolve(), (e) => e.code === 'operation_conflict');
      await assert.rejects(query(), (e) => e.code === 'persistence_failed');
      const { connection: model } = await configureModel(request);
      assert.deepEqual(await observed(() => Promise.all([resolve(), resolve()])), [
        { sessionId },
        { sessionId },
      ]);
      await assert.rejects(
        remote.method('query', 'unrelated')(),
        (error) => error.code === 'invalid_request',
      );
      await assert.rejects(
        remote.method('resolve')({ connectionId: 'forged' }),
        (error) => error.code === 'invalid_request',
      );
      const initial = await query();
      assert.equal(initial.id, sessionId);
      assert.equal(initial.name, 'WorkHub');
      assert.equal(initial.permissionMode, 'bypass');
      assert.equal(initial.collaborationMode, 'agent');
      assert.equal(initial.orchestrationMode, 'maka.workhub');
      assert.deepEqual(await resolve(), { sessionId });
      assert.deepEqual(await query(), initial);
      const input = {
        expectedRevision: initial.revision,
        thinkingLevel: null,
        modelTarget: {
          kind: 'explicit',
          connectionId: model.connectionId,
          connectionSlug: initial.llmConnectionSlug,
          model: 'fixture-model',
        },
      };
      await assert.rejects(
        configure({ ...input, thinkingLevel: 'high' }),
        (error) => error.code === 'invalid_request',
      );
      assert.deepEqual(
        await query(),
        initial,
        'unsupported thinking cannot partially change the model',
      );
      const configured = await observed(() => configure(input));
      assert.equal(configured.kind, 'committed');
      assert.equal(configured.session.connectionLocked, true);
      assert.equal(configured.session.thinkingLevel, undefined);
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
      const staleQuery = remote.method('query');
      await staleQuery();
      for (const disabled of [true, false]) {
        await toggleWorkhub(connection, disabled);
        await assert.rejects(staleQuery(), (error) => error.code === 'operation_conflict');
        if (disabled) {
          const page = await request('plugin.client.query', { kind: 'snapshot' });
          assert(!page.entries.some((entry) => entry.extensionId === 'maka.workhub'));
          await assert.rejects(
            request('workhub.coordination.query', {}),
            (error) => error.code === 'operation_unavailable',
          );
          await assert.rejects(
            request('workhub.coordination.resolve', {}),
            (error) => error.code === 'operation_unavailable',
          );
          await assert.rejects(
            request('workhub.coordination.configureModel', {
              ...input,
              expectedRevision: configured.session.revision,
            }),
            (error) => error.code === 'operation_unavailable',
          );
        } else {
          remote = await workhubRemote(connection);
          remotes.push(remote);
          assert.deepEqual(await query(), configured.session);
        }
        await assert.rejects(access(initial.workspace.hostCwd), (error) => error.code === 'ENOENT');
      }
      assert.deepEqual(await resolve(), { sessionId });
      await access(initial.workspace.hostCwd);
      // A prerequisite-free Skill is invocable in an ordinary Session but not
      // in WorkHub, even before a Desktop capability provider has been bound.
      const skillDir = join(initial.workspace.hostCwd, '.agents', 'skills', 'ordinary-only');
      await mkdir(skillDir, { recursive: true });
      await writeFile(
        join(skillDir, 'SKILL.md'),
        '---\nname: ordinary-only\ndescription: Ordinary Session instructions\n---\nDo not invoke in WorkHub.\n',
      );
      await request('session.create', {
        sessionId: 'ordinary-skills',
        workspace: { kind: 'host_path', path: initial.workspace.hostCwd },
        modelTarget: { kind: 'default' },
      });
      const invocable = (input) => request('skill.catalog.invocable.query', input);
      const target = { kind: 'session', sessionId };
      const empty = await invocable({ kind: 'start', target });
      assert.equal(empty.kind, 'page');
      assert.deepEqual(empty.items, []);
      assert.equal(empty.nextCursor, null);
      const ordinary = await invocable({
        kind: 'start',
        target: { kind: 'session', sessionId: 'ordinary-skills' },
      });
      assert(ordinary.items.some((item) => item.id === 'ordinary-only'));
      await assert.rejects(
        invocable({
          kind: 'continue',
          target,
          revision: empty.revision,
          cursor: empty.revision + ':0',
        }),
        (error) => error.code === 'invalid_request',
      );
      await request('connection.catalog.remove', {
        expected: { connectionId: model.connectionId, revision: model.revision },
      });
      assert.deepEqual(await resolve(), { sessionId });
      assert.deepEqual(await query(), configured.session);
      assert.deepEqual(await invocable({ kind: 'start', target }), empty);
      await writeFile(file, JSON.stringify(configured.session));
      console.log('workhub-passed');
    }
  }
} finally {
  await Promise.all(remotes.map((remote) => remote.close()));
  transport.abort();
  await connection?.close();
}

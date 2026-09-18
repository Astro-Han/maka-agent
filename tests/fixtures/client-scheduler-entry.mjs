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
import { once } from 'node:events';
import { connect } from 'node:net';
import { setTimeout as delay } from 'node:timers/promises';
import { parseArgs } from 'node:util';
import { connectRuntimeHostMessageTransport } from '../../packages/runtime-host/src/client/connection.ts';
import { FramedTransport } from '../../packages/runtime-host/src/transport/framed-transport.ts';
import {
  INTERACTIVE_RUNTIME_HOST_COMPOSITION_ID,
  RUNTIME_HOST_PROTOCOL_VERSION,
} from '../../packages/runtime-host/src/protocol/index.ts';

const { values } = parseArgs({
  options: {
    socket: { type: 'string' },
    'root-id': { type: 'string' },
    'scheduler-workspace': { type: 'string' },
    reopened: { type: 'boolean' },
  },
});
const socket = connect(values.socket);
const transport = new FramedTransport(socket);
let connection;
let release;
try {
  await once(socket, 'connect');
  const result = await connectRuntimeHostMessageTransport({
    transport,
    expectedRootId: values['root-id'],
    compositionId: INTERACTIVE_RUNTIME_HOST_COMPOSITION_ID,
    protocol: { min: RUNTIME_HOST_PROTOCOL_VERSION, max: RUNTIME_HOST_PROTOCOL_VERSION },
    handshakeTimeoutMs: 3000,
    livenessIntervalMs: 60000,
  });
  assert.equal(result.kind, 'connected');
  connection = result.connection;
  const request = (operation, input) => connection.request(operation, input, 3000);
  assert.deepEqual(await request('host.wake', {}), {});
  const ready = async () => {
    for (let attempt = 0; attempt < 300; attempt++) {
      const status = await request('plugin.platform.query', { view: 'status' });
      if (status.convergence === 'converged') {
        try {
          await request('scheduled-task.query', { kind: 'list' });
          return;
        } catch (error) {
          if (error.code !== 'operation_unavailable') throw error;
        }
      }
      await delay(10);
    }
    assert.fail('Scheduler did not become ready');
  };
  await ready();
  const notices = [];
  connection.subscribeScheduledTaskChanges((frame) => notices.push(frame));
  const calls = [];
  let admitted;
  const started = new Promise((resolve) => {
    admitted = resolve;
  });
  const held = new Promise((resolve) => {
    release = resolve;
  });
  const provider = {
    offers: () => [],
    services: () => [{ serviceId: 'maka_scheduled_task_native_effect', version: '1' }],
    async call() {
      assert.fail('notification must use service admission');
    },
    async callService(frame, { accept }) {
      assert.equal(frame.method, 'notify_local');
      await accept({ kind: 'none' });
      calls.push(frame.input);
      admitted();
      await held;
      return { ok: true };
    },
    close() {},
  };
  await connection.replaceClientCapabilities(provider, 3000);
  let page = await request('scheduled-task.query', { kind: 'list' });
  if (!values.reopened) {
    assert.deepEqual(page.tasks, []);
    const ids = [];
    for (let index = 0; index < 65; index++) {
      const created = await request('scheduled-task.mutate', {
        kind: 'create',
        input: {
          title: 'Reminder ' + index,
          intentBody: 'Keep this reminder',
          schedule: { kind: 'interval', everySeconds: 3600, startAt: Date.now() },
          effect: { kind: 'notify', channel: 'local' },
        },
      });
      ids.push(created.task.id);
    }
    page = await request('scheduled-task.query', { kind: 'list' });
    assert.equal(page.tasks.length, 64);
    assert.match(page.nextCursor, /^\d+$/);
    const tail = await request('scheduled-task.query', {
      kind: 'list',
      cursor: page.nextCursor,
      expectedRevision: page.revision,
    });
    assert.equal(tail.tasks.length, 1);
    assert.equal(tail.nextCursor, null);
    assert.equal(new Set([...page.tasks, ...tail.tasks].map((task) => task.id)).size, 65);
    const id = ids[0];
    await request('scheduled-task.mutate', { kind: 'trigger_now', taskId: id });
    await started;
    const paused = await request('scheduled-task.mutate', { kind: 'pause', taskId: id });
    assert.equal(paused.task.status, 'paused', 'slow native delivery must not block mutations');
    const stale = await request('scheduled-task.query', {
      kind: 'list',
      cursor: page.nextCursor,
      expectedRevision: page.revision,
    });
    assert.equal(stale.kind, 'revision_changed');
    release();
    let task;
    for (let attempt = 0; attempt < 300; attempt++) {
      task = (await request('scheduled-task.query', { kind: 'get', taskId: id })).task;
      if (task.fireCount === 1) break;
      await delay(10);
    }
    assert.equal(task.fireCount, 1);
    assert.equal(task.status, 'paused');
    assert.equal(task.runs[0].outcome, 'ok');
    assert.equal(calls.length, 1);
    assert.deepEqual(calls[0], { taskId: id, title: 'Reminder 0' });
    assert.ok(notices.some((frame) => frame.taskId === id));
    await connection.unregisterClientCapabilities(3000);
    for (const waitingId of ids.slice(1, 4)) {
      await request('scheduled-task.mutate', { kind: 'trigger_now', taskId: waitingId });
      for (let attempt = 0; attempt < 300; attempt++) {
        const waiting = (await request('scheduled-task.query', { kind: 'get', taskId: waitingId }))
          .task;
        if (waiting.lastError === 'Waiting for a notification provider') break;
        await delay(10);
      }
    }
    await request('scheduled-task.mutate', { kind: 'pause', taskId: ids[1] });
    await request('scheduled-task.mutate', { kind: 'snooze', taskId: ids[2], delayMs: 3600000 });
    await request('scheduled-task.mutate', { kind: 'delete', taskId: ids[3] });
    await connection.replaceClientCapabilities(provider, 3000);
    await delay(350);
    assert.equal(calls.length, 1, 'cancelled waiting notifications must not be admitted');
    await request('plugin.composition.apply', {
      operations: [
        {
          type: 'update',
          entryId: 'maka.scheduler',
          patch: { disabled: true },
        },
      ],
    });
    await assert.rejects(
      request('scheduled-task.query', { kind: 'list' }),
      (error) => error.code === 'operation_unavailable',
    );
    await request('plugin.composition.apply', {
      operations: [
        {
          type: 'update',
          entryId: 'maka.scheduler',
          patch: { disabled: false },
        },
      ],
    });
    await ready();
  } else {
    const all = [...page.tasks];
    if (page.nextCursor !== null) {
      const tail = await request('scheduled-task.query', {
        kind: 'list',
        cursor: page.nextCursor,
        expectedRevision: page.revision,
      });
      all.push(...tail.tasks);
    }
    assert.equal(all.length, 64);
    const task = all.find((task) => task.title === 'Reminder 0');
    assert.equal(task.fireCount, 1);
    assert.equal(task.status, 'paused');
    assert.equal(task.runs[0].outcome, 'ok');
    assert.equal(calls.length, 0, 'settled native effects must not replay after restart');
    await request('scheduled-task.mutate', { kind: 'delete', taskId: task.id });
    assert.equal(
      (await request('scheduled-task.query', { kind: 'get', taskId: task.id })).task,
      null,
    );
  }
  console.log('original-client-scheduler');
} finally {
  release?.();
  transport.abort();
  await connection?.close();
}

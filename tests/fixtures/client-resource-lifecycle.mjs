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
import { setTimeout as delay } from 'node:timers/promises';
import { verifyResourceController } from './client-resource-controller.mjs';
import { watchSession } from './client-subscription.mjs';

const sessionId = 'client-shell-resources';

export async function verifyResourceLifecycle(connection, workspace, reopened, openConnection) {
  const request = (operation, input) => connection.request(operation, input, 10000);
  const get = async (ref) =>
    (await request('runtime.resource.query', { kind: 'get', sessionId, ref })).resource;
  const saved = join(workspace, 'resource-client-results.json');
  if (reopened) {
    for (const expected of JSON.parse(await readFile(saved, 'utf8'))) {
      assert.deepEqual((await get(expected.ref)).result, expected);
    }
    assert.equal(await readFile(join(workspace, 'resource-effect'), 'utf8'), 'once');
    assert.equal(await readFile(join(workspace, 'controller-effect'), 'utf8'), '一二三四');
    return;
  }
  await request('session.create', {
    sessionId,
    workspace: { kind: 'host_path', path: workspace },
    modelTarget: { kind: 'default' },
    permissionMode: 'ask',
  });
  const active = new Set();
  const results = [];
  const observer = await watchSession(connection, sessionId);
  const changed = (ref) =>
    observer.waitFor(
      (frame) =>
        frame.kind === 'subscription.session_domain_changed' &&
        frame.domain === 'runtime_resource' &&
        frame.sessionId === sessionId &&
        frame.resources.some(
          (resource) => resource.sourceSessionId === sessionId && resource.ref === ref,
        ),
    );
  try {
    const command =
      process.platform === 'win32'
        ? "[IO.File]::AppendAllText('resource-effect', 'once'); [Console]::Write('ready'); while (-not (Test-Path resource-release)) { Start-Sleep -Milliseconds 20 }; [Console]::Write(' done 中文'); exit 17"
        : "printf once >> resource-effect; printf ready; while [ ! -f resource-release ]; do sleep 0.02; done; printf ' done 中文'; exit 17";
    const started = await request('runtime.resource.start', {
      sessionId,
      launchId: 'one-shot-launch',
      command,
    });
    active.add(started.resource.ref);
    await changed(started.resource.ref);
    assert.equal((await get(started.resource.ref)).result.ref, started.resource.ref);
    assert.equal(started.resource.mode, 'pipes');
    assert.equal(started.resource.cmd, command);
    const until = async (ref, predicate) => {
      const deadline = Date.now() + 10000;
      for (;;) {
        const update = await get(ref);
        if (predicate(update.result)) return update;
        assert(Date.now() < deadline, JSON.stringify(update));
        await delay(20);
      }
    };
    const live = await until(started.resource.ref, (r) => r.output?.stdout === 'ready');
    assert.equal(live.result.status, 'running');
    assert.equal(live.sourceTurnId, 'one-shot-launch');
    assert.equal(live.sourceToolCallId, 'one-shot-launch');
    await writeFile(join(workspace, 'resource-release'), 'continue');
    const completed = await until(started.resource.ref, (r) => r.status === 'failed');
    assert.equal(completed.result.exitCode, 17);
    assert.equal(completed.result.output.stdout, 'ready done 中文');
    const stopped = await request('runtime.resource.stop', {
      sessionId,
      ref: started.resource.ref,
    });
    assert.deepEqual(stopped.resource, {
      ...completed.result,
      revision: completed.result.revision + 1,
    });
    active.delete(started.resource.ref);
    results.push(stopped.resource);
    assert.deepEqual(
      await request('runtime.resource.stop', { sessionId, ref: stopped.resource.ref }),
      stopped,
    );
    const terminal = await request('runtime.resource.start', {
      sessionId,
      launchId: 'interactive-launch',
    });
    active.add(terminal.resource.ref);
    await changed(terminal.resource.ref);
    assert.equal(terminal.resource.mode, 'pty');
    assert.equal(terminal.resource.status, 'running');
    await verifyResourceController(
      connection,
      openConnection,
      workspace,
      sessionId,
      terminal.resource.ref,
    );
    const closed = await request('runtime.resource.stop', {
      sessionId,
      ref: terminal.resource.ref,
    });
    assert.equal(closed.resource.mode, 'pty');
    assert.equal(closed.resource.status, 'cancelled');
    active.delete(terminal.resource.ref);
    results.push(closed.resource);
    await assert.rejects(
      request('runtime.resource.stop', {
        sessionId,
        ref: 'maka://runtime/background-tasks/missing',
      }),
      (e) => e.code === 'not_found',
    );
    await assert.rejects(
      request('runtime.resource.stop', { sessionId: 'missing', ref: stopped.resource.ref }),
      (e) => e.code === 'not_found',
    );
    await request('session.lifecycle.set', { sessionId, state: 'archived' });
    const controller = { sessionId, ref: terminal.resource.ref, controllerId: 'archived-owner' };
    await assert.rejects(request('runtime.resource.controller.acquire', controller), {
      code: 'session_archived',
    });
    await assert.rejects(
      request('runtime.resource.controller.control', {
        ...controller,
        sequence: 1,
        control: { kind: 'resize', cols: 80, rows: 24 },
      }),
      { code: 'session_archived' },
    );
    await assert.rejects(
      request('runtime.resource.start', { sessionId, launchId: 'archived', command }),
      (e) => e.code === 'session_archived',
    );
    await assert.rejects(
      request('runtime.resource.stop', { sessionId, ref: stopped.resource.ref }),
      (e) => e.code === 'session_archived',
    );
    await request('session.lifecycle.set', { sessionId, state: 'active' });
    await assert.rejects(
      request('runtime.resource.controller.control', {
        ...controller,
        sequence: 1,
        control: { kind: 'resize', cols: 80, rows: 24 },
      }),
      { code: 'operation_conflict' },
    );
    await writeFile(saved, JSON.stringify(results));
    assert.equal(await readFile(join(workspace, 'resource-effect'), 'utf8'), 'once');
  } finally {
    await Promise.allSettled(
      [...active].map((ref) => request('runtime.resource.stop', { sessionId, ref })),
    );
    await observer.close();
  }
}

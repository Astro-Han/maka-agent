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
import { decodeStoredMessage } from '../../packages/core/src/session.ts';
import { readToolResultPage } from '../../packages/runtime/src/read-page.ts';
import { watchSession } from './client-subscription.mjs';
import { verifyModelStdin, verifyTerminalStdin } from './client-model-stdin.mjs';

export async function verifyModelBackground(connection, workspace, reopened, model) {
  for (const pty of [false, true]) await verifyMode(connection, workspace, reopened, model, pty);
}

function readPage(snapshot) {
  const page = readToolResultPage(JSON.stringify(snapshot), { path: snapshot.ref });
  if (snapshot.output?.mode === 'pty') page.metadata.truncated = snapshot.output.truncated;
  return page;
}

async function verifyMode(connection, workspace, reopened, model, pty) {
  const sessionId = pty ? 'model-shell-pty' : 'model-shell';
  const other = sessionId + '-other';
  const effectName = sessionId + '-effect';
  const releaseName = sessionId + '-release';
  const request = (operation, input) => connection.request(operation, input, 10000);
  const savedPath = join(workspace, sessionId + '.json');
  const get = async (ref) =>
    (await request('runtime.resource.query', { kind: 'get', sessionId, ref })).resource;
  const rows = async (id) => {
    const watch = await watchSession(connection, id, { kind: 'tail', maxBytes: 2 });
    try {
      return await watch.subscription.loadTranscript(decodeStoredMessage);
    } finally {
      await watch.close();
    }
  };
  if (reopened) {
    const saved = JSON.parse(await readFile(savedPath, 'utf8'));
    assert.deepEqual((await get(saved.ref)).result, saved.result);
    assert.deepEqual((await get(saved.stoppedRef)).result, saved.stoppedResult);
    assert.deepEqual(await rows(sessionId), saved.rows);
    assert.deepEqual(await rows(other), saved.otherRows);
    assert.equal(await readFile(join(workspace, effectName), 'utf8'), 'once');
    return;
  }
  for (const id of [sessionId, other]) {
    await request('session.create', {
      sessionId: id,
      workspace: { kind: 'host_path', path: workspace },
      modelTarget: { kind: 'default' },
      sandboxMode: 'danger-full-access',
    });
  }
  const windows = process.platform === 'win32';
  const command = windows
    ? (pty
        ? 'if ([Console]::IsInputRedirected -or [Console]::IsOutputRedirected) { exit 5 }; '
        : '') +
      `[IO.File]::AppendAllText('${effectName}', 'once'); [Console]::Write('ready'); ` +
      (pty
        ? "$line = [Console]::ReadLine(); if ($line -ne 'continue😀') { exit 6 }; "
        : `while (-not (Test-Path ${releaseName})) { Start-Sleep -Milliseconds 20 }; `) +
      "[Console]::Write(' done 中文'); exit 19"
    : (pty ? 'test -t 0 && test -t 1 && test -t 2 || exit 5; stty -echo; ' : '') +
      `printf once >> ${effectName}; printf ready; ` +
      (pty
        ? `IFS= read -r line; test "$line" = 'continue😀' || exit 6; `
        : `while [ ! -f ${releaseName} ]; do sleep 0.02; done; `) +
      "printf ' done 中文'; exit 19";
  const outputText = (value) => (pty ? value.output?.screen : value.output?.stdout);
  const hidden = (
    await request('runtime.resource.start', {
      sessionId,
      launchId: 'user-private',
      command: windows ? "[Console]::Write('private')" : 'printf private',
    })
  ).resource.ref;
  let reference;
  let stoppedRef;
  const result = (input) => {
    const last = input.messages.at(-1);
    assert.equal(last.role, 'tool');
    return JSON.parse(last.content);
  };
  const rejected = (input) => {
    assert.equal(
      input.messages.at(-1).content,
      'Runtime background task not found in this session',
    );
    return { answer: 'scope respected' };
  };
  const until = async (predicate, ref = reference) => {
    const deadline = Date.now() + 10000;
    for (;;) {
      const value = await get(ref);
      if (predicate(value.result)) return value;
      assert(Date.now() < deadline, JSON.stringify(value));
      await delay(20);
    }
  };
  const turn = async (id, turnId) => {
    const watch = await watchSession(connection, id);
    try {
      const started = await request('turn.start', {
        sessionId: id,
        turnId,
        content: { text: turnId },
        maxSteps: 6,
      });
      await watch.waitFor(
        (frame) =>
          frame.kind === 'subscription.session_projection' &&
          frame.snapshot.rootTurn?.turnId === turnId &&
          ['completed', 'failed', 'cancelled'].includes(frame.snapshot.rootTurn.status),
      );
      model.healthy();
      assert.equal((await request('turn.query', { sessionId: id, turnId })).status, 'completed');
      return started.turn;
    } finally {
      await watch.close();
    }
  };
  const handedOff = Promise.withResolvers();
  const continuation = Promise.withResolvers();
  try {
    if (pty) {
      model.extend([
        { name: 'Shell', args: { command, pty: true } },
        (input) => {
          assert.equal(
            input.messages.at(-1).content,
            'invalid tool input: PTY mode requires run_in_background=true',
          );
          return { answer: 'foreground PTY rejected' };
        },
      ]);
      await turn(sessionId, 'reject-foreground-pty');
    }
    model.extend([
      { name: 'Shell', args: { command, run_in_background: true, pty } },
      async (input) => {
        const value = result(input);
        assert.equal(value.kind, 'shell_run');
        assert.equal(value.mode, pty ? 'pty' : 'pipes');
        assert.equal(value.status, 'running');
        assert(!Object.hasOwn(value, 'output'));
        assert(!Object.hasOwn(value, 'timeoutMs'));
        reference = value.ref;
        handedOff.resolve();
        await continuation.promise;
        return { answer: 'background handed off' };
      },
    ]);
    const launching = await watchSession(connection, sessionId);
    try {
      const { turn: launched } = await request('turn.start', {
        sessionId,
        turnId: 'background-launch',
        content: { text: 'launch a background task' },
        maxSteps: 4,
      });
      await handedOff.promise;
      assert.equal(
        (await request('turn.query', { sessionId, turnId: 'background-launch' })).status,
        'running',
      );
      await request('turn.stop', { sessionId, turnId: 'background-launch', runId: launched.runId });
      await launching.waitFor(
        (frame) =>
          frame.kind === 'subscription.session_projection' &&
          frame.snapshot.rootTurn?.turnId === 'background-launch' &&
          frame.snapshot.rootTurn.status === 'cancelled',
      );
    } finally {
      continuation.resolve();
      await launching.close();
    }
    const live = await until((value) => outputText(value) === 'ready');
    assert.equal(
      live.result.status,
      'running',
      'active Turn cancellation must not kill a handed-off task',
    );
    const firstRows = await rows(sessionId);
    const call = firstRows.find(
      (row) =>
        row.type === 'tool_call' && row.toolName === 'Shell' && row.turnId === 'background-launch',
    );
    assert.equal(live.sourceTurnId, 'background-launch');
    assert.equal(live.sourceToolCallId, call.id);
    model.extend([
      { name: 'Read', args: { path: reference } },
      (input) => {
        const value = result(input);
        assert.deepEqual(value, readPage(live.result));
        return { name: 'Read', args: { path: hidden } };
      },
      rejected,
      { name: 'Read', args: { path: reference } },
      rejected,
    ]);
    await turn(sessionId, 'background-observe');
    await turn(other, 'foreign-observe');
    await verifyModelStdin({
      pty,
      sessionId,
      other,
      reference,
      hidden,
      request,
      model,
      turn,
      result,
    });
    if (!pty) {
      await writeFile(join(workspace, releaseName), 'continue');
    }
    const completed = await until((value) => value.status === 'failed');
    assert.equal(completed.result.exitCode, 19);
    model.extend([
      { name: 'Read', args: { path: reference } },
      (input) => {
        const value = result(input);
        assert.deepEqual(
          value,
          readPage({ ...completed.result, revision: completed.result.revision + 1 }),
        );
        return { name: 'Read', args: { path: reference } };
      },
      (input) => {
        assert.deepEqual(
          result(input),
          readPage({
            ...completed.result,
            revision: completed.result.revision + 1,
          }),
        );
        return { answer: 'terminal observed once' };
      },
    ]);
    await turn(sessionId, 'background-terminal');
    const final = (await get(reference)).result;
    assert(outputText(final).endsWith(' done 中文'));
    if (pty) {
      await verifyTerminalStdin({ sessionId, reference, snapshot: final, model, turn, result });
    }
    model.extend([
      {
        name: 'Shell',
        args: {
          command: windows
            ? "[Console]::Write('stop-ready'); Start-Sleep -Seconds 60"
            : 'printf stop-ready; sleep 60',
          run_in_background: true,
          pty,
        },
      },
      (input) => {
        stoppedRef = result(input).ref;
        return { answer: 'ready to stop' };
      },
    ]);
    await turn(sessionId, 'stop-launch');
    await until((value) => outputText(value) === 'stop-ready', stoppedRef);
    const privateBefore = (await get(hidden)).result;
    model.extend([
      { name: 'StopBackgroundTask', args: { ref: hidden } },
      rejected,
      { name: 'StopBackgroundTask', args: { ref: stoppedRef } },
      rejected,
    ]);
    await turn(sessionId, 'hidden-stop');
    await turn(other, 'foreign-stop');
    assert.deepEqual((await get(hidden)).result, privateBefore);
    assert.equal((await get(stoppedRef)).result.status, 'running');
    if (pty) {
      // A client input lease does not prevent the model from stopping its task.
      await request('runtime.resource.controller.acquire', {
        sessionId,
        ref: stoppedRef,
        controllerId: 'model-stop-pty',
      });
    }
    let stoppedResult;
    model.extend([
      { name: 'StopBackgroundTask', args: { ref: stoppedRef } },
      async (input) => {
        const { operation, ...snapshot } = result(input);
        assert.deepEqual(operation, { kind: 'stop', applied: true });
        assert.equal(snapshot.status, 'cancelled');
        assert.equal(outputText(snapshot), 'stop-ready');
        assert.deepEqual((await get(stoppedRef)).result, snapshot);
        stoppedResult = snapshot;
        return { name: 'StopBackgroundTask', args: { ref: stoppedRef } };
      },
      (input) => {
        assert.deepEqual(result(input), {
          ...stoppedResult,
          operation: { kind: 'stop', applied: false },
        });
        return { answer: 'stopped exactly once' };
      },
    ]);
    await turn(sessionId, 'background-stop');
    await writeFile(
      savedPath,
      JSON.stringify({
        ref: reference,
        result: final,
        stoppedRef,
        stoppedResult,
        rows: await rows(sessionId),
        otherRows: await rows(other),
      }),
    );
    model.verify();
  } finally {
    continuation.resolve();
    if (reference) await request('runtime.resource.stop', { sessionId, ref: reference });
    if (stoppedRef) await request('runtime.resource.stop', { sessionId, ref: stoppedRef });
    await request('runtime.resource.stop', { sessionId, ref: hidden });
  }
}

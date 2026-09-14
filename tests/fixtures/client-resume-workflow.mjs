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
import { readFile, writeFile, unlink } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { decodeStoredMessage } from '../../packages/core/src/session.ts';
import { watchSession, assertText } from './client-subscription.mjs';
import { evidence, modelOverridesFixture } from './client-model-overrides-fixture.mjs';
import {
  configure,
  sessionInput,
  modelId,
  catalog,
  header,
} from './client-model-overrides-catalog.mjs';

export async function verifyResume(connection, workspace, reopened) {
  const request = (operation, input) => connection.request(operation, input, 5000);
  const file = join(workspace, 'resume-fixture.json');
  const sessionId = 'safe-resume';
  if (reopened) {
    const saved = JSON.parse(await readFile(file, 'utf8'));
    assert.deepEqual(await request('turn.resume.start', saved.input), {
      kind: 'started',
      turn: saved.terminal,
    });
    const observer = await watchSession(connection, sessionId, {
      kind: 'tail',
      maxBytes: 2,
    });
    try {
      const rows = await observer.subscription.loadTranscript(decodeStoredMessage);
      assert(
        rows.some(
          (row) =>
            row.type === 'assistant' &&
            row.turnId === saved.input.turnId &&
            row.text === 'RESUMED_OK',
        ),
        `resumed transcript: ${JSON.stringify(rows)}`,
      );
      assert.equal(
        rows.filter((row) => row.type === 'user').length,
        3,
        'resume must not fabricate a user message',
      );
    } finally {
      await observer.close();
    }
    return;
  }
  const fixture = await modelOverridesFixture();
  const observers = [];
  const terminal = async (turnId, expected = 'completed') => {
    for (let i = 0; i < 500; i++) {
      fixture.check();
      const result = await request('turn.query', { sessionId, turnId });
      if (['completed', 'failed', 'cancelled'].includes(result.status)) {
        assert.equal(result.status, expected);
        return result;
      }
      await delay(10);
    }
    throw new Error('resume fixture did not terminate');
  };
  const expect = (marker, extra = {}) =>
    fixture.expect({
      path: '/v1/chat/completions',
      model: modelId,
      parallel: false,
      outputLimit: 12345,
      marker,
      answer: 'discarded answer',
      ...extra,
    });
  try {
    await writeFile(join(workspace, 'facts-evidence.txt'), evidence);
    const rows = await configure(request, fixture.baseUrl);
    await request('session.create', sessionInput(workspace, rows[0], sessionId));
    assert.equal(
      (await request('turn.resume.query', { sessionId })).reason,
      'resume_candidate_missing',
    );
    const held = expect('SOURCE_ANCHOR', { hold: true });
    const started = await request('turn.start', {
      sessionId,
      turnId: 'source',
      content: { text: 'SOURCE_ANCHOR' },
      maxSteps: 1,
    });
    await held.wait();
    await request('turn.stop', { sessionId, turnId: 'source', runId: started.turn.runId });
    await terminal('source', 'cancelled');
    held.release();
    expect('UNRELATED_BRANCH');
    await request('turn.start', {
      sessionId,
      turnId: 'unrelated',
      content: { text: 'UNRELATED_BRANCH' },
      maxSteps: 1,
    });
    await terminal('unrelated');
    let plan;
    for (let i = 0; i < 100; i++) {
      plan = await request('turn.resume.query', { sessionId });
      if (plan.reason !== 'session_busy') break;
      await delay(5);
    }
    assert.equal(plan.disposition, 'ready', JSON.stringify(plan));
    assert.equal(plan.sourceRunId, started.turn.runId, 'default skips the later completed branch');
    const input = {
      sessionId,
      turnId: 'resumed',
      sourceRunId: plan.sourceRunId,
      sourceRuntimeEventHighWater: plan.sourceRuntimeEventHighWater,
    };
    const wrong = await request('turn.resume.start', {
      ...input,
      sourceRuntimeEventHighWater: input.sourceRuntimeEventHighWater + 1,
    });
    assert.equal(wrong.kind, 'parked');
    assert.equal(fixture.records.length, 2);
    const observer = await watchSession(connection, sessionId);
    observers.push(observer);
    const reading = expect('SOURCE_ANCHOR', { read: true, hold: true });
    const resumed = expect('STEER_RESUME', {
      answer: 'RESUMED_OK',
      streamHold: true,
      toolResult: true,
    });
    assert.equal((await request('turn.resume.start', input)).kind, 'started');
    await reading.wait();
    const steering = {
      sessionId,
      originHostEpoch: connection.hostEpoch,
      messageId: 'resume-steering',
      content: { text: 'STEER_RESUME' },
      placement: 'current_turn',
    };
    const receipt = await request('turn.message.submit', steering);
    assert.equal(receipt.disposition, 'steering');
    reading.release();
    await observer.waitFor(
      (frame) => frame.kind === 'subscription.session_delta' && frame.delta.turnId === input.turnId,
    );
    const attached = await watchSession(connection, sessionId, {
      kind: 'tail',
      maxBytes: 2,
    });
    observers.push(attached);
    assert.equal(attached.subscription.activeAssistantStreams.length, 1);
    const partial = await attached.subscription.loadTranscript(decodeStoredMessage);
    assert(
      partial.some(
        (row) => row.type === 'assistant' && row.turnId === input.turnId && row.text === 'RESU',
      ),
      `resume overlay: ${JSON.stringify(partial)}`,
    );
    resumed.release();
    const suffix = await attached.waitFor((frame) => frame.kind === 'subscription.session_delta');
    assert.equal(suffix.delta.startOffset, 4);
    assert.equal(suffix.delta.text, 'MED_OK');
    const finished = await terminal(input.turnId);
    await observer.terminal(finished);
    await observer.waitFor(
      (frame) =>
        frame.kind === 'subscription.session_delta' &&
        frame.delta.turnId === input.turnId &&
        frame.delta.complete,
    );
    assertText(observer.frames, input.turnId, 'RESUMED_OK');
    assert(!JSON.stringify(fixture.records[2].input).includes('UNRELATED_BRANCH'));
    assert(!JSON.stringify(fixture.records[3].input).includes('UNRELATED_BRANCH'));
    assert.deepEqual(await request('turn.message.submit', steering), receipt);
    const used = await request('turn.resume.query', { sessionId, sourceRunId: input.sourceRunId });
    assert.equal(used.reason, 'continuation_already_exists');
    await assert.rejects(
      request('turn.resume.start', {
        ...input,
        sourceRuntimeEventHighWater: input.sourceRuntimeEventHighWater + 1,
      }),
      (error) => error.code === 'operation_conflict',
    );
    const current = header(await catalog(request), rows[0].connectionId);
    await request('connection.catalog.remove', {
      expected: { connectionId: current.connectionId, revision: current.revision },
    });
    await unlink(join(workspace, '.maka-workspace.json'));
    assert.deepEqual(await request('turn.resume.start', input), {
      kind: 'started',
      turn: finished,
    });
    await writeFile(file, JSON.stringify({ input, terminal: finished }));
    assert.equal(fixture.records.length, 4);
    fixture.verify();
  } finally {
    for (const observer of observers) await observer.close();
    await fixture.close();
  }
}

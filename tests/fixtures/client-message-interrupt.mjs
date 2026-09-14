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
import { modelFixture } from './client-turn.mjs';
import { createMessageSession, waitMessageTerminal } from './client-message-fixture.mjs';
import { watchSession } from './client-subscription.mjs';

export async function verifyMessageInterrupt(
  connection,
  workspace,
  reopened,
  openClient,
  failTerminal = false,
) {
  const sessionId = 'message-interrupt';
  const request = (op, input) => connection.request(op, input, 3000);
  const saved = join(workspace, 'message-interrupt.json');
  if (reopened) {
    const { interrupt, result } = JSON.parse(await readFile(saved, 'utf8'));
    await assert.rejects(request('turn.interrupt', interrupt), (e) => e.code === 'outcome_unknown');
    assert.deepEqual(
      await request('turn.query', { sessionId, turnId: interrupt.turnId }),
      result.turn,
    );
    assert.deepEqual(
      await request('turn.message.query', { sessionId, messageIds: ['steer', 'next'] }),
      { cancelledMessageIds: ['steer', 'next'] },
    );
    console.log('message-interrupt-reopened');
    return;
  }
  const model = await modelFixture();
  const sibling = await openClient();
  try {
    await createMessageSession(connection, workspace, sessionId, model.baseUrl);
    const submit = (messageId, placement = 'next_turn') =>
      request('turn.message.submit', {
        sessionId,
        messageId,
        placement,
        originHostEpoch: connection.hostEpoch,
        content: { text: messageId },
      });
    const first = await submit('first');
    await waitMessageTerminal(request, sessionId, first.turnId);
    const current = await submit('active');
    await model.partial;
    const turn = await request('turn.query', { sessionId, turnId: current.turnId });
    const interrupt = {
      originHostEpoch: connection.hostEpoch,
      sessionId,
      interruptId: 'stop-once',
      turnId: turn.turnId,
      runId: turn.runId,
    };
    await assert.rejects(
      request('turn.interrupt', { ...interrupt, interruptId: 'wrong', runId: 'wrong' }),
      (e) => e.code === 'operation_conflict',
    );
    // A completed conflict is also an identity receipt, not a later stop attempt.
    await assert.rejects(
      request('turn.interrupt', { ...interrupt, interruptId: 'wrong' }),
      (e) => e.code === 'operation_conflict',
    );
    await submit('next');
    const steering = await submit('steer', 'current_turn');
    if (failTerminal) {
      await assert.rejects(
        request('turn.interrupt', interrupt),
        (e) => e.code === 'outcome_unknown',
      );
      // Fail-stop flushes the accepted outcome, then closes transport.
      await connection.closed;
      assert.equal(model.requests.length, 2);
      console.log('message-interrupt-failure-passed');
      return;
    }
    const results = await Promise.all([
      request('turn.interrupt', interrupt),
      sibling.request('turn.interrupt', interrupt, 3000),
    ]);
    assert.deepEqual(results[0], results[1]);
    const result = results[0];
    assert.equal((await connection.status()).activeResidencies, 0);
    assert.equal(result.turn.status, 'cancelled');
    assert.equal(result.turn.runId, turn.runId);
    assert.equal(result.queueRevision, steering.queueRevision + 1);
    assert.deepEqual(
      result.retracted.map((entry) => [entry.messageId, entry.state]),
      [
        ['steer', 'retracted'],
        ['next', 'retracted'],
      ],
    );
    assert.deepEqual(
      await request('turn.message.query', { sessionId, messageIds: ['steer', 'next'] }),
      { cancelledMessageIds: ['steer', 'next'] },
    );
    const watch = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
    try {
      assert.deepEqual(watch.subscription.snapshot.queue.steering, []);
      assert.deepEqual(watch.subscription.snapshot.queue.followup, []);
    } finally {
      await watch.close();
    }
    // Returned interrupt has released cleanup ownership: a new root may start.
    const next = await submit('after-interrupt');
    await waitMessageTerminal(request, sessionId, next.turnId);
    assert.notEqual(next.turnId, current.turnId);
    assert.deepEqual(await request('turn.interrupt', interrupt), result);
    await assert.rejects(
      request('turn.interrupt', { ...interrupt, runId: 'different' }),
      (e) => e.code === 'operation_conflict',
    );
    assert.equal(model.requests.length, 3, 'retracted messages must never call the provider');
    await writeFile(saved, JSON.stringify({ interrupt, result }));
    console.log('message-interrupt-passed');
  } finally {
    await sibling.close();
    await model.close();
  }
}

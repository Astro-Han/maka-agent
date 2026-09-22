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
import { setTimeout as delay } from 'node:timers/promises';
import { waitMessageTerminal } from './client-message-fixture.mjs';
import { watchSession } from './client-subscription.mjs';
import { decodeStoredMessage } from '../../packages/core/src/session.ts';

export async function verifyMessageRecovery(connection, reopened) {
  const request = (op, input) => connection.request(op, input, 3000);
  for (const [sessionId, messageId, status] of [
    ['reserved', 'reserved', 'completed'],
    ['queued', 'queued-steering', 'completed'],
    ['queued', 'queued-followup', 'completed'],
    ['missing', 'missing', 'failed'],
    ['skill-gate', 'skill-gate', 'failed'],
    ['unknown', 'unknown-followup', 'failed'],
  ]) {
    let owned;
    for (let i = 0; i < 300; i++) {
      const result = await request('turn.message.execution.query', {
        sessionId,
        messageIds: [messageId],
      });
      if (result.resolutions[0]?.state === 'owned') {
        owned = result.resolutions[0];
        break;
      }
      await delay(10);
    }
    assert(owned, 'accepted message must acquire a canonical root');
    const turn = await waitMessageTerminal(request, sessionId, owned.turnId);
    assert.equal(turn.status, status, JSON.stringify({ sessionId, messageId, turn }));
    if (['reserved', 'missing', 'skill-gate'].includes(sessionId)) {
      assert.equal(turn.turnId, 'reserved-' + sessionId);
      assert.equal(turn.runId, 'run-' + sessionId);
    }
    if (status === 'failed') assert.equal(turn.failureClass, 'message_preparation_failed');
    const watch = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
    try {
      const rows = await watch.subscription.loadTranscript(decodeStoredMessage);
      assert.equal(rows.filter((row) => row.id === messageId).length, 1);
    } finally {
      await watch.close();
    }
  }
  const unknown = await request('turn.query', { sessionId: 'unknown', turnId: 'reserved-unknown' });
  assert.equal(unknown.status, 'failed');
  assert.equal(unknown.failureClass, 'outcome_unknown');
  assert.deepEqual(
    await request('turn.message.submit', {
      sessionId: 'reserved',
      originHostEpoch: 'previous-epoch',
      messageId: 'reserved',
      content: { text: 'reserved' },
      placement: 'current_turn',
    }),
    {
      disposition: 'turn_started',
      turnId: 'reserved-reserved',
      preparation: [],
    },
  );
  console.log(reopened ? 'message-recovery-reopened' : 'message-recovery-passed');
}

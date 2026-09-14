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
import { watchSession } from './client-subscription.mjs';

const sessionId = 'oauth-execution';
const model = 'gpt-5.6-luna';

export async function verifyOAuthExecution(connection, workspace, reopened) {
  const request = (operation, input) => connection.request(operation, input, 5000);
  const catalog = await request('connection.catalog.query', { kind: 'start' });
  let row = catalog.items.find(
    (item) => item.kind === 'connection' && item.providerType === 'openai-codex',
  );
  assert(row, 'Host-owned subscription connection must exist');
  if (!reopened) {
    const discovered = await connection.request(
      'connection.models.fetch',
      { connectionId: row.connectionId },
      25000,
    );
    assert.equal(discovered.kind, 'committed');
    assert(discovered.modelCount > 0, 'subscription discovery must return a usable inventory');
    assert.equal(discovered.source, 'fetched');
    const refreshed = await request('connection.catalog.query', { kind: 'start' });
    row = refreshed.items.find(
      (item) => item.kind === 'connection' && item.connectionId === row.connectionId,
    );
    assert(row);
    const updated = await request('connection.catalog.update', {
      expected: { connectionId: row.connectionId, revision: row.revision },
      changes: {
        name: row.name,
        baseUrl: row.baseUrl,
        enabled: true,
        enabledModelIds: [model],
      },
    });
    assert.equal(updated.kind, 'committed');
    const verified = await connection.request(
      'connection.test.run',
      {
        connectionId: row.connectionId,
        modelId: model,
      },
      25000,
    );
    assert.equal(verified.kind, 'committed');
    assert.equal(verified.test.kind, 'verified');
    assert.equal(verified.test.modelId, model);
    await request('session.create', {
      sessionId,
      workspace: { kind: 'host_path', path: workspace },
      mode: 'bot',
      permissionMode: 'explore',
      modelTarget: {
        kind: 'explicit',
        connectionId: row.connectionId,
        connectionSlug: row.slug,
        model,
      },
    });
  }
  const snapshotPath = join(workspace, 'oauth-transcript.json');
  const observer = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 128 });
  const turnId = reopened ? 'oauth-recalled' : 'oauth-remembered';
  try {
    const before = await observer.subscription.loadTranscript(decodeStoredMessage);
    if (reopened) assert.equal(JSON.stringify(before), await readFile(snapshotPath, 'utf8'));
    const started = await request('turn.start', {
      sessionId,
      turnId,
      content: {
        text: reopened
          ? 'What token did I ask you to remember? Do not call tools. Reply only with that token.'
          : 'Remember the token amber-129. Do not call tools. Reply only with READY-129.',
      },
      maxSteps: 1,
    });
    assert.equal(started.kind, 'started');
    assert.equal(started.turn.turnId, turnId);
    let turn;
    const deadline = Date.now() + 100000;
    do {
      turn = await request('turn.query', { sessionId, turnId });
      if (['completed', 'failed', 'cancelled'].includes(turn.status)) break;
      await delay(50);
    } while (Date.now() < deadline);
    if (turn.status !== 'completed') {
      if (turn.runId)
        await request('turn.stop', { sessionId, turnId, runId: turn.runId }).catch(() => {});
      throw new Error('Subscription turn did not complete: ' + JSON.stringify(turn));
    }
    await observer.terminal(turn);
    const deltas = observer.frames
      .filter(
        (frame) =>
          frame.kind === 'subscription.session_delta' &&
          frame.delta.turnId === turnId &&
          frame.delta.kind === 'text',
      )
      .map((frame) => frame.delta);
    assert(deltas.length > 0, 'subscription text must be streamed');
    let text = '';
    for (const delta of deltas) {
      assert.equal(delta.messageId, deltas[0].messageId);
      assert.equal(delta.startOffset, text.length);
      text += delta.text;
    }
    assert.equal(deltas.at(-1).complete, true);
    assert.equal(
      text.trim(),
      reopened ? 'amber-129' : 'READY-129',
      'live text must satisfy the recall request',
    );
    // A fresh subscription exercises canonical transcript materialization, not
    // the live observer's in-memory accumulator.
    const durable = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 128 });
    try {
      const rows = await durable.subscription.loadTranscript(decodeStoredMessage);
      const assistant = rows.find(
        (item) => item.type === 'assistant' && item.id === deltas[0].messageId,
      );
      assert.equal(
        assistant?.turnId,
        turnId,
        'live message identity must exist in the committed Turn',
      );
      assert.equal(assistant?.text, text, 'committed text must equal the exact live message');
      assert.equal(assistant?.id, deltas[0].messageId);
      if (!reopened) await writeFile(snapshotPath, JSON.stringify(rows));
    } finally {
      await durable.close();
    }
    console.log(
      JSON.stringify({ check: 'original-client-oauth-execution', reopened, result: 'passed' }),
    );
  } finally {
    await observer.close();
  }
}

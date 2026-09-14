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

const sessionId = 'live-openrouter';
const turnId = 'live-free-once';
const model = 'openrouter/free';
const baseUrl = 'https://openrouter.ai/api/v1';

export async function verifyLiveOpenrouter(connection, workspace, reopened) {
  const request = (operation, input) => connection.request(operation, input, 5000);
  const snapshotPath = join(workspace, 'live-transcript.json');
  if (!reopened) {
    const secret = process.env.OPENROUTER_API_KEY;
    assert(secret, 'Explicit live test requires OPENROUTER_API_KEY');
    const created = await request('connection.catalog.create', {
      expectedCatalogRevision: 0,
      connection: {
        slug: 'live-openrouter',
        name: 'Disposable live acceptance',
        providerType: 'openrouter',
        baseUrl,
        enabled: true,
        enabledModelIds: [model],
      },
    });
    assert.equal(created.kind, 'committed');
    assert.equal(
      (
        await request('credential.vault.set', {
          locator: {
            scope: 'connection',
            connectionId: created.connection.connectionId,
            kind: 'api_key',
          },
          expected: null,
          expectedConnection: {
            ...created.connection,
            slug: 'live-openrouter',
            providerType: 'openrouter',
            effectiveBaseUrl: baseUrl,
          },
          secret,
        })
      ).kind,
      'committed',
    );
    await request('session.create', {
      sessionId,
      workspace: { kind: 'host_path', path: workspace },
      mode: 'bot',
      permissionMode: 'explore',
      modelTarget: {
        kind: 'explicit',
        connectionId: created.connection.connectionId,
        connectionSlug: 'live-openrouter',
        model,
      },
    });
    const observer = await watchSession(connection, sessionId);
    try {
      await request('turn.start', {
        sessionId,
        turnId,
        content: {
          text: 'This is a connectivity test. Do not call tools. Reply with one short sentence.',
        },
        maxSteps: 1,
      });
      const deadline = Date.now() + 120000;
      let turn;
      do {
        turn = await request('turn.query', { sessionId, turnId });
        if (['completed', 'failed', 'cancelled'].includes(turn.status)) break;
        await delay(100);
      } while (Date.now() < deadline);
      if (turn.status !== 'completed') {
        if (turn.runId) {
          await request('turn.stop', { sessionId, turnId, runId: turn.runId }).catch(() => {});
        }
        throw new Error('Live OpenRouter turn did not complete: ' + turn.status);
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
      assert(
        deltas.some((delta) => delta.text.length > 0),
        'real stream must deliver text',
      );
      let text = '';
      for (const delta of deltas) {
        assert.equal(delta.messageId, deltas[0].messageId);
        assert.equal(delta.startOffset, text.length, 'live offsets use UTF-16');
        text += delta.text;
      }
      assert.equal(deltas.at(-1).complete, true);
      await writeFile(
        join(workspace, 'live-stream.json'),
        JSON.stringify({
          id: deltas[0].messageId,
          text,
        }),
      );
    } finally {
      await observer.close();
    }
  }
  const observer = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 128 });
  try {
    const rows = await observer.subscription.loadTranscript(decodeStoredMessage);
    const assistant = rows.find((row) => row.type === 'assistant' && row.turnId === turnId);
    const stream = JSON.parse(await readFile(join(workspace, 'live-stream.json'), 'utf8'));
    assert.equal(assistant?.id, stream.id);
    assert.equal(assistant?.text, stream.text, 'live stream equals durable transcript');
    const snapshot = JSON.stringify(rows);
    if (reopened) assert.equal(snapshot, await readFile(snapshotPath, 'utf8'));
    else await writeFile(snapshotPath, snapshot);
    console.log(
      reopened ? 'original-client-live-openrouter-reopened' : 'original-client-live-openrouter',
    );
  } finally {
    await observer.close();
  }
}

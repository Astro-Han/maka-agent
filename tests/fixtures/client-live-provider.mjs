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

const sessionId = 'live-provider';
const turnId = 'live-once';
const model = process.env.MAKA_LIVE_MODEL ?? 'deepseek-v4.1-flash';
const baseUrl = process.env.MAKA_LIVE_BASE_URL ?? 'http://spark-1.tailf3107f.ts.net:8888/v1';
const providerType = {
  chat: 'openai-compatible',
  responses: 'openai-responses-compatible',
  messages: 'anthropic-compatible',
}[process.env.MAKA_LIVE_PROTOCOL ?? 'chat'];
assert(providerType, 'MAKA_LIVE_PROTOCOL must be chat, responses, or messages');

export async function verifyLiveProvider(connection, workspace, reopened) {
  const request = (operation, input) => connection.request(operation, input, 5000);
  const snapshotPath = join(workspace, 'live-transcript.json');
  if (!reopened) {
    const secret = process.env.MAKA_LIVE_API_KEY ?? 'fixture-only';
    const created = await request('connection.catalog.create', {
      expectedCatalogRevision: 0,
      connection: {
        slug: 'live-provider',
        name: 'Disposable live acceptance',
        providerType,
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
            slug: 'live-provider',
            providerType,
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
        connectionSlug: 'live-provider',
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
      const deadline = Date.now() + 300000;
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
        throw new Error('Live provider turn did not complete: ' + turn.status);
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
      const streams = new Map();
      for (const delta of deltas) {
        const stream = streams.get(delta.messageId) ?? { id: delta.messageId, text: '' };
        assert.equal(delta.startOffset, stream.text.length, 'live offsets use UTF-16');
        stream.text += delta.text;
        streams.set(delta.messageId, stream);
      }
      assert.equal(deltas.at(-1).complete, true);
      await writeFile(join(workspace, 'live-stream.json'), JSON.stringify([...streams.values()]));
    } finally {
      await observer.close();
    }
  }
  const observer = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 128 });
  try {
    const rows = await observer.subscription.loadTranscript(decodeStoredMessage);
    const assistants = rows.filter((row) => row.type === 'assistant' && row.turnId === turnId);
    const streams = JSON.parse(await readFile(join(workspace, 'live-stream.json'), 'utf8'));
    for (const stream of streams) {
      const assistant = assistants.find((row) => row.id === stream.id);
      assert.equal(assistant?.text, stream.text, 'each live fragment equals its durable row');
    }
    const completed = assistants.filter((row) => !row.interrupted);
    assert.equal(completed.length, 1, 'one successful response follows any interrupted fragments');
    assert(completed[0].text.length > 0);
    assert(streams.some((stream) => stream.id === completed[0].id));
    const snapshot = JSON.stringify(rows);
    if (reopened) assert.equal(snapshot, await readFile(snapshotPath, 'utf8'));
    else await writeFile(snapshotPath, snapshot);
    console.log(
      reopened ? 'original-client-live-provider-reopened' : 'original-client-live-provider',
    );
  } finally {
    await observer.close();
  }
}

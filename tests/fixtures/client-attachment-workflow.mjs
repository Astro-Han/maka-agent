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
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { decodeStoredMessage } from '../../packages/core/src/session.ts';
import { projectDesktopToolResultContent } from '../../apps/desktop/src/shared/desktop-session-projection.ts';
import { watchSession } from './client-subscription.mjs';
import { attachmentModel, png, text } from './client-attachment-model.mjs';

async function rows(connection, sessionId) {
  const observer = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
  try {
    return await observer.subscription.loadTranscript(decodeStoredMessage);
  } finally {
    await observer.close();
  }
}
async function upload(request, sessionId, uploadId, bytes, name, mimeType) {
  const command = { sessionId, uploadId };
  await request('artifact.ingest', {
    ...command,
    kind: 'begin',
    name,
    mimeType,
    totalBytes: bytes.length,
    contentSha256: 'sha256:' + createHash('sha256').update(bytes).digest('hex'),
  });
  await request('artifact.ingest', {
    ...command,
    kind: 'chunk',
    offset: 0,
    chunkBase64: bytes.toString('base64'),
  });
  return (await request('artifact.ingest', { ...command, kind: 'commit' })).attachment;
}
export async function verifyConsumption(connection, workspace, reopened) {
  const request = (operation, input) => connection.request(operation, input, 3000);
  const snapshot = join(workspace, 'attachments.json');
  if (reopened) {
    const saved = JSON.parse(await readFile(snapshot, 'utf8'));
    for (const item of saved) {
      assert.deepEqual(await rows(connection, item.turn.sessionId), item.rows);
      assert.deepEqual((await request('turn.start', item.turn)).turn, item.terminal);
      for (const attachment of item.turn.content.attachments)
        assert.equal(
          (
            await request('artifact.query', {
              kind: 'get',
              sessionId: item.turn.sessionId,
              artifactId: attachment.ref.relativePath,
            })
          ).artifact,
          null,
        );
    }
    return;
  }
  const model = await attachmentModel();
  try {
    const created = await request('connection.catalog.create', {
      expectedCatalogRevision: 0,
      connection: {
        slug: 'attachments',
        name: 'Attachments',
        providerType: 'openai',
        baseUrl: model.baseUrl,
        enabled: true,
        enabledModelIds: ['text-model', 'gpt-4o'],
        modelOverrides: {
          'text-model': { vision: false },
          'gpt-4o': { vision: true },
        },
      },
    });
    assert.equal(created.kind, 'committed');
    const basis = created.connection;
    await request('credential.vault.set', {
      locator: { scope: 'connection', connectionId: basis.connectionId, kind: 'api_key' },
      expected: null,
      expectedConnection: {
        ...basis,
        slug: 'attachments',
        providerType: 'openai',
        effectiveBaseUrl: model.baseUrl,
      },
      secret: 'fixture',
    });
    const saved = [];
    for (const vision of [false, true]) {
      const sessionId = vision ? 'vision-session' : 'text-session';
      const modelId = vision ? 'gpt-4o' : 'text-model';
      await request('session.create', {
        sessionId,
        workspace: { kind: 'host_path', path: workspace },
        modelTarget: {
          kind: 'explicit',
          connectionId: basis.connectionId,
          connectionSlug: 'attachments',
          model: modelId,
        },
        permissionMode: 'explore',
        mode: 'bot',
      });
      const attachments = [
        await upload(request, sessionId, 'text', Buffer.from(text), 'note.txt', 'text/plain'),
        await upload(request, sessionId, 'image', png, 'pixel.png', 'image/png'),
      ];
      const turn = {
        sessionId,
        turnId: 'consume',
        maxSteps: 4,
        content: { text: 'consume uploaded resources', attachments },
      };
      model.scenarios.set(modelId, { vision, content: turn.content, steps: 0 });
      for (const corrupt of [
        { ...attachments[0], name: 'forged.txt' },
        { ...attachments[0], bytes: attachments[0].bytes + 1 },
        { ...attachments[0], mimeType: 'application/json' },
        { ...attachments[1], kind: 'other' },
        { ...attachments[0], ref: { ...attachments[0].ref, sessionId: 'other-session' } },
      ]) {
        await assert.rejects(
          request('turn.start', {
            ...turn,
            turnId: 'rejected',
            content: { ...turn.content, attachments: [corrupt] },
          }),
          (error) => error.code === 'operation_conflict',
        );
        await assert.rejects(
          request('turn.query', { sessionId, turnId: 'rejected' }),
          (error) => error.code === 'not_found',
        );
      }
      await request('turn.start', turn);
      let terminal;
      const deadline = Date.now() + 6000;
      while (Date.now() < deadline) {
        model.check();
        terminal = await request('turn.query', { sessionId, turnId: turn.turnId });
        if (['completed', 'failed', 'cancelled'].includes(terminal.status)) break;
        await delay(10);
      }
      assert.equal(terminal.status, 'completed');
      const stored = await rows(connection, sessionId);
      assert.deepEqual(stored.find((r) => r.type === 'user').attachments, attachments);
      const results = stored.filter((r) => r.type === 'tool_result');
      assert.equal(results.length, 2);
      assert(results.every((r) => !r.isError));
      assert.deepEqual(results[0].content, { kind: 'text', text });
      assert.deepEqual(results[1].content, {
        kind: 'image',
        mimeType: 'image/png',
        ref: attachments[1].ref,
      });
      const desktopImage = projectDesktopToolResultContent(
        { hostId: connection.rootId },
        results[1].content,
      );
      assert.deepEqual(desktopImage.ref, {
        ...attachments[1].ref,
        sessionId: JSON.stringify([connection.rootId, sessionId]),
      });
      assert.equal(
        results[1].content.ref.sessionId,
        sessionId,
        'Desktop scoping never mutates canonical Session reference',
      );
      for (const attachment of attachments)
        await request('artifact.delete', { sessionId, artifactId: attachment.ref.relativePath });
      assert.deepEqual((await request('turn.start', turn)).turn, terminal);
      saved.push({ turn, terminal, rows: stored });
    }
    model.verify();
    await writeFile(snapshot, JSON.stringify(saved));
  } finally {
    await model.close();
  }
}

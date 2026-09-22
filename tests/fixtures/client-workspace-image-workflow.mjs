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
import { projectDesktopToolResultContent } from '../../apps/desktop/src/shared/desktop-session-projection.ts';
import { watchSession } from './client-subscription.mjs';
import { workspaceImageFixture, png } from './client-workspace-image-fixture.mjs';

async function rows(connection, sessionId) {
  const observer = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
  try {
    const subscription = observer.subscription;
    let page = subscription.transcriptBootstrap.durable;
    const entries = [];
    for (;;) {
      const decoded = await subscription.decodeTranscriptPage(
        page,
        decodeStoredMessage,
        16 * 1024 * 1024,
      );
      entries.push(...decoded.messages);
      if (decoded.nextCursor === null) break;
      page = await subscription.loadTranscriptPage({
        direction: 'older',
        throughSequence: page.throughSequence,
        cursor: decoded.nextCursor,
        anchorSequence: null,
        maxBytes: 48 * 1024,
      });
    }
    return entries.sort((a, b) => a.identity - b.identity).map((entry) => entry.message);
  } finally {
    await observer.close();
  }
}

async function completed(request, turn, model) {
  await request('turn.start', turn);
  const deadline = Date.now() + 10000;
  let terminal;
  do {
    model.check();
    terminal = await request('turn.query', { sessionId: turn.sessionId, turnId: turn.turnId });
    if (['completed', 'failed', 'cancelled'].includes(terminal.status)) break;
    await delay(10);
  } while (Date.now() < deadline);
  model.check();
  assert.equal(terminal.status, 'completed');
  return terminal;
}

function imageResult(connection, stored, sessionId) {
  const results = stored.filter((row) => row.type === 'tool_result' && !row.isError);
  assert.equal(results.length, 1);
  const image = results[0].content;
  assert.deepEqual(image, { kind: 'image', mimeType: 'image/png', ref: image.ref });
  assert.equal(image.ref.kind, 'session_file');
  assert.equal(image.ref.sessionId, sessionId);
  assert(
    !JSON.stringify(stored).includes(png.toString('base64')),
    'raw/UI rows contain refs, never image bytes',
  );
  const desktop = projectDesktopToolResultContent({ hostId: connection.rootId }, image);
  assert.deepEqual(desktop.ref, {
    ...image.ref,
    sessionId: JSON.stringify([connection.rootId, sessionId]),
  });
  assert.equal(image.ref.sessionId, sessionId, 'Desktop does not mutate the canonical ref');
  return image.ref;
}

export async function verifyWorkspaceImage(connection, workspace, reopened) {
  const request = (operation, input) => connection.request(operation, input, 5000);
  const path = join(workspace, 'workspace-image.json');
  const source = join(workspace, 'source.PNG');
  const saved = reopened
    ? JSON.parse(await readFile(path, 'utf8'))
    : { sessionId: 'workspace-image' };
  const model = await workspaceImageFixture(
    reopened ? Number(new URL(saved.baseUrl).port) : 0,
    reopened,
  );
  try {
    if (reopened) {
      assert.deepEqual(await rows(connection, saved.sessionId), saved.rows);
      assert.deepEqual((await request('turn.start', saved.turn)).turn, saved.terminal);
      assert.deepEqual((await request('turn.start', saved.refTurn)).turn, saved.refTerminal);
      assert.deepEqual(imageResult(connection, saved.rows, saved.sessionId), saved.imageRef);
      await assert.rejects(readFile(source), (error) => error.code === 'ENOENT');
      await completed(
        request,
        {
          sessionId: saved.sessionId,
          turnId: 'reopened',
          content: { text: 'Recall the original workspace image' },
          maxSteps: 1,
        },
        model,
      );
      const restored = await rows(connection, saved.sessionId);
      assert.deepEqual(imageResult(connection, restored, saved.sessionId), saved.imageRef);
      assert.equal(
        restored.filter((row) => row.type === 'tool_call').length,
        2,
        'reopen never rereads source',
      );
    } else {
      await writeFile(source, png);
      const created = await request('connection.catalog.create', {
        expectedCatalogRevision: 0,
        connection: {
          slug: 'workspace-image',
          name: 'Workspace image fixture',
          providerType: 'openai',
          baseUrl: model.baseUrl,
          enabled: true,
          enabledModelIds: ['gpt-4o'],
          modelOverrides: { 'gpt-4o': { vision: true } },
        },
      });
      assert.equal(created.kind, 'committed');
      const basis = created.connection;
      assert.equal(
        (
          await request('credential.vault.set', {
            locator: { scope: 'connection', connectionId: basis.connectionId, kind: 'api_key' },
            expected: null,
            expectedConnection: {
              ...basis,
              slug: 'workspace-image',
              providerType: 'openai',
              effectiveBaseUrl: model.baseUrl,
            },
            secret: 'workspace-image-fixture',
          })
        ).kind,
        'committed',
      );
      await request('session.create', {
        sessionId: saved.sessionId,
        workspace: { kind: 'host_path', path: workspace },
        mode: 'bot',
        sandboxMode: 'read-only',
        modelTarget: {
          kind: 'explicit',
          connectionId: basis.connectionId,
          connectionSlug: 'workspace-image',
          model: 'gpt-4o',
        },
      });
      saved.baseUrl = model.baseUrl;
      saved.turn = {
        sessionId: saved.sessionId,
        turnId: 'read-workspace',
        content: { text: 'Read source.PNG as an image' },
        maxSteps: 3,
      };
      saved.terminal = await completed(request, saved.turn, model);
      saved.imageRef = imageResult(
        connection,
        await rows(connection, saved.sessionId),
        saved.sessionId,
      );
      model.scenario.imageRef = saved.imageRef;
      await assert.rejects(
        request('artifact.delete', {
          sessionId: saved.sessionId,
          artifactId: saved.imageRef.relativePath,
        }),
        (error) => error.code === 'operation_conflict',
      );
      // Later model requests must use the committed snapshot, not this changed path.
      await writeFile(source, 'source replaced after the committed image Read');
      saved.refTurn = {
        sessionId: saved.sessionId,
        turnId: 'deny-snapshot-ref',
        content: { text: 'Try the protected snapshot as an attachment ref' },
        maxSteps: 3,
      };
      saved.refTerminal = await completed(request, saved.refTurn, model);
      await unlink(source);
      saved.rows = await rows(connection, saved.sessionId);
      assert.deepEqual(imageResult(connection, saved.rows, saved.sessionId), saved.imageRef);
      const failed = saved.rows.filter((row) => row.type === 'tool_result' && row.isError);
      assert.equal(failed.length, 1, 'snapshot ref does not grant user-upload Read authority');
      assert(
        JSON.stringify(failed[0].content).includes('Attachment was not found in this Session'),
      );
      assert.deepEqual((await request('turn.start', saved.turn)).turn, saved.terminal);
      await writeFile(path, JSON.stringify(saved));
    }
    model.verify();
    assert.equal((await connection.status(3000)).state, 'ready');
  } finally {
    await model.close();
  }
}

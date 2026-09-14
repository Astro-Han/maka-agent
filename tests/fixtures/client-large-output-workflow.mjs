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
import { largeOutputFixture } from './client-large-output-fixture.mjs';

const SIZE = 10 * 1024 * 1024;
const CLIENT_LIMIT = 16 * 1024 * 1024;
const digest = (bytes) => 'sha256:' + createHash('sha256').update(bytes).digest('hex');

async function upload(request, sessionId, caseName) {
  const bytes = Buffer.alloc(SIZE, caseName === 'ascii' ? 65 : 1);
  const command = { sessionId, uploadId: 'large-text' };
  await request('artifact.ingest', {
    ...command,
    kind: 'begin',
    name: caseName + '.txt',
    mimeType: 'text/plain',
    totalBytes: SIZE,
    contentSha256: digest(bytes),
  });
  for (let offset = 0; offset < bytes.length; offset += 48 * 1024) {
    await request('artifact.ingest', {
      ...command,
      kind: 'chunk',
      offset,
      chunkBase64: bytes.subarray(offset, offset + 48 * 1024).toString('base64'),
    });
  }
  return {
    attachment: (await request('artifact.ingest', { ...command, kind: 'commit' })).attachment,
    digest: digest(bytes),
  };
}

async function completed(request, turn, model) {
  await request('turn.start', turn);
  const deadline = Date.now() + 45000;
  let terminal;
  do {
    model.check();
    terminal = await request('turn.query', { sessionId: turn.sessionId, turnId: turn.turnId });
    if (['completed', 'failed', 'cancelled'].includes(terminal.status)) break;
    await delay(20);
  } while (Date.now() < deadline);
  model.check();
  assert.equal(terminal.status, 'completed');
  return terminal;
}

async function verifyAscii(connection, item) {
  const observer = await watchSession(connection, item.sessionId, { kind: 'tail', maxBytes: 2 });
  try {
    const subscription = observer.subscription;
    let page = subscription.transcriptBootstrap.durable;
    assert(page.nextCursor, 'two-byte bootstrap must exercise continuation');
    const messages = [];
    let assemblyBytes = 0,
      peakBytes = 0;
    const account = (delta) => {
      assemblyBytes += delta;
      peakBytes = Math.max(peakBytes, assemblyBytes);
      assert(assemblyBytes >= 0 && assemblyBytes <= 64 * 1024 * 1024);
    };
    for (;;) {
      const decoded = await subscription.decodeTranscriptPage(
        page,
        decodeStoredMessage,
        CLIENT_LIMIT,
        account,
      );
      messages.push(...decoded.messages);
      if (decoded.nextCursor === null) break;
      page = await subscription.loadTranscriptPage({
        source: 'durable',
        direction: 'older',
        throughSequence: page.throughSequence,
        cursor: decoded.nextCursor,
        anchorSequence: null,
        maxBytes: 48 * 1024,
      });
    }
    assert.equal(assemblyBytes, 0);
    assert(peakBytes >= SIZE, 'full raw row passes through the original client assembler');
    const results = messages
      .map((entry) => entry.message)
      .filter((row) => row.type === 'tool_result');
    assert.equal(results.length, 1, 'no replayed tool effect');
    const row = results[0];
    assert.equal(row.isError, false);
    assert.equal(row.content.kind, 'text');
    assert.equal(Buffer.byteLength(row.content.text), SIZE);
    assert.equal(digest(row.content.text), item.digest);
    const displayed = projectDesktopToolResultContent({ hostId: connection.rootId }, row.content);
    assert.equal(displayed.kind, 'text');
    assert.equal(digest(displayed.text), item.digest);
    return row.id;
  } finally {
    await observer.close();
  }
}

async function verifyControlCapacity(connection, sessionId, live) {
  await assert.rejects(
    async () => {
      if (live) {
        await live.subscription.loadTranscriptPage({
          source: 'durable',
          direction: 'older',
          throughSequence: null,
          cursor: null,
          anchorSequence: null,
          maxBytes: 48 * 1024,
        });
      } else {
        const opened = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
        await opened.close();
      }
    },
    (error) => error.code === 'operation_unavailable',
    'oversized transcript is explicitly unavailable without closing its connection',
  );
  assert.equal((await connection.status(3000)).state, 'ready');
}

export async function verifyLargeOutput(connection, workspace, reopened) {
  const request = (operation, input) => connection.request(operation, input, 10000);
  const path = join(workspace, 'large-output.json');
  const saved = reopened ? JSON.parse(await readFile(path, 'utf8')) : { sessions: [] };
  const model = await largeOutputFixture(reopened ? Number(new URL(saved.baseUrl).port) : 0);
  let liveControl;
  try {
    if (!reopened) {
      const created = await request('connection.catalog.create', {
        expectedCatalogRevision: 0,
        connection: {
          slug: 'large-output',
          name: 'Large output fixture',
          providerType: 'openai',
          baseUrl: model.baseUrl,
          enabled: true,
          enabledModelIds: ['ascii-model', 'control-model'],
          modelOverrides: {
            'ascii-model': { vision: false },
            'control-model': { vision: false },
          },
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
              slug: 'large-output',
              providerType: 'openai',
              effectiveBaseUrl: model.baseUrl,
            },
            secret: 'large-output-fixture',
          })
        ).kind,
        'committed',
      );
      saved.baseUrl = model.baseUrl;
      for (const caseName of ['ascii', 'control']) {
        const sessionId = 'large-output-' + caseName;
        await request('session.create', {
          sessionId,
          workspace: { kind: 'host_path', path: workspace },
          mode: 'bot',
          permissionMode: 'explore',
          modelTarget: {
            kind: 'explicit',
            connectionId: basis.connectionId,
            connectionSlug: 'large-output',
            model: caseName + '-model',
          },
        });
        const uploaded = await upload(request, sessionId, caseName);
        const turn = {
          sessionId,
          turnId: 'read-large',
          maxSteps: 3,
          content: { text: 'Read the uploaded text', attachments: [uploaded.attachment] },
        };
        model.scenarios.set(caseName + '-model', {
          artifactId: uploaded.attachment.ref.relativePath,
          steps: 0,
          expectedSteps: 3,
        });
        if (caseName === 'control')
          liveControl = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
        const terminal = await completed(request, turn, model);
        if (caseName === 'control') {
          await liveControl.terminal(terminal);
          await verifyControlCapacity(connection, sessionId, liveControl);
          const session = (await request('session.catalog.query', { kind: 'get', sessionId }))
            .session;
          const changed = await request('session.metadata.update', {
            sessionId,
            expectedRevision: session.revision,
            patch: { name: 'Capacity remains observable' },
          });
          assert.equal(changed.kind, 'committed');
          await liveControl.waitFor(
            (frame) =>
              frame.kind === 'subscription.session_projection' &&
              frame.snapshot.session.metadataRevision === changed.session.revision,
          );
        }
        saved.sessions.push({ caseName, sessionId, turn, terminal, digest: uploaded.digest });
      }
    }
    for (const item of saved.sessions) {
      if (reopened)
        model.scenarios.set(item.caseName + '-model', {
          steps: 0,
          expectedSteps: 1,
          followup: true,
        });
      const terminal = await request('turn.query', {
        sessionId: item.sessionId,
        turnId: item.turn.turnId,
      });
      assert.deepEqual(terminal, item.terminal);
      assert.deepEqual((await request('turn.start', item.turn)).turn, item.terminal);
      if (item.caseName === 'ascii') {
        const rowId = await verifyAscii(connection, item);
        if (reopened) assert.equal(rowId, item.rowId);
        else item.rowId = rowId;
      } else await verifyControlCapacity(connection, item.sessionId, liveControl);
      // A subsequent provider turn proves both host usability and compact model history.
      const followup = {
        sessionId: item.sessionId,
        turnId: reopened ? 'after-reopen' : 'after-capacity',
        content: { text: 'Confirm completion' },
        maxSteps: 1,
      };
      const scenario = model.scenarios.get(item.caseName + '-model');
      scenario.followup = true;
      const followupTerminal = await completed(request, followup, model);
      if (item.caseName === 'control' && liveControl) {
        await liveControl.terminal(followupTerminal);
        await verifyControlCapacity(connection, item.sessionId, liveControl);
      }
      assert.equal((await connection.status(3000)).state, 'ready');
    }
    model.verify();
    if (!reopened) await writeFile(path, JSON.stringify(saved));
  } finally {
    try {
      await liveControl?.close();
    } finally {
      await model.close();
    }
  }
}

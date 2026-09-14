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
import { upload } from './client-artifact-upload.mjs';
import { once } from 'node:events';
import { createServer } from 'node:http';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { decodeStoredMessage } from '../../packages/core/src/session.ts';
import { watchSession } from './client-subscription.mjs';

const sessionId = 'maka_workhub_coordination';
const turnId = 'workhub-answer';
const names = [
  'AskUserQuestion',
  'Read',
  'mcp__desktop_workhub__control',
  'mcp__desktop_workhub__tasks',
];
const forbidden = 'WORKHUB_SCOPE_ESCAPED';

export async function verifyWorkhubAnswer(connection, workspace, reopened) {
  const request = (operation, input) => connection.request(operation, input, 5000);
  const file = join(workspace, 'answer.json');
  if (reopened) {
    const saved = JSON.parse(await readFile(file, 'utf8'));
    assert.deepEqual(await request('workhub.coordination.answer', saved.input), { turnId });
    assert.deepEqual(await request('turn.query', { sessionId, turnId }), saved.terminal);
    const observer = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
    try {
      assert.deepEqual(await observer.subscription.loadTranscript(decodeStoredMessage), saved.rows);
    } finally {
      await observer.close();
    }
    return;
  }
  let failure, attachmentPath;
  const requests = [];
  const source = join(workspace, 'forbidden.txt');
  await writeFile(source, forbidden);
  const server = createServer(async (request, response) => {
    try {
      let body = '';
      for await (const chunk of request) {
        body += chunk;
        assert(Buffer.byteLength(body) <= 128 * 1024);
      }
      const input = JSON.parse(body),
        step = requests.length;
      requests.push(input);
      assert.equal(request.url, '/v1/chat/completions');
      assert.equal(input.model, 'fixture-model');
      assert.deepEqual(input.tools.map((tool) => tool.function.name).sort(), names);
      assert(!body.includes(forbidden), 'No filesystem or AGENTS content may reach this model');
      if (step === 1 || step === 2) assert(body.includes('WorkHub Read accepts only'));
      if (step === 3) assert(body.includes('ATTACHMENT_EVIDENCE'));
      if (step === 4) assert(body.includes('DESKTOP_CONTROL_VERIFIED'));
      const actions = [
        ['Read', { path: source }],
        [
          'Read',
          {
            path:
              'maka://read/' +
              Buffer.from(source).toString('base64url') +
              '?at=1&sha=' +
              'a'.repeat(64),
          },
        ],
        ['Read', { path: attachmentPath }],
        ['mcp__desktop_workhub__control', { status: '正在检查 WorkHub' }],
      ];
      assert(step <= actions.length, 'Unexpected model retry or extra step');
      const delta =
        step === actions.length
          ? { content: 'workhub verified' }
          : {
              tool_calls: [
                {
                  index: 0,
                  id: 'call-' + step,
                  type: 'function',
                  function: { name: actions[step][0], arguments: JSON.stringify(actions[step][1]) },
                },
              ],
            };
      const chunk = (delta, finish_reason) => ({
        id: 'chat-' + step,
        object: 'chat.completion.chunk',
        created: 1,
        model: 'fixture-model',
        choices: [{ index: 0, delta, finish_reason }],
      });
      response.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
      response.end(
        [chunk(delta, null), chunk({}, step === actions.length ? 'stop' : 'tool_calls')]
          .map((event) => 'data: ' + JSON.stringify(event) + '\n\n')
          .join('') + 'data: [DONE]\n\n',
      );
    } catch (error) {
      failure = error;
      response.writeHead(400).end();
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const baseUrl = 'http://127.0.0.1:' + server.address().port + '/v1';
  let observer;
  try {
    const created = await request('connection.catalog.create', {
      expectedCatalogRevision: 0,
      connection: {
        slug: 'workhub-fixture',
        name: 'WorkHub fixture',
        providerType: 'openai-compatible',
        baseUrl,
        enabled: true,
        enabledModelIds: ['fixture-model'],
      },
    });
    const basis = created.connection;
    await request('credential.vault.set', {
      locator: { scope: 'connection', connectionId: basis.connectionId, kind: 'api_key' },
      expected: null,
      expectedConnection: {
        ...basis,
        slug: 'workhub-fixture',
        providerType: 'openai-compatible',
        effectiveBaseUrl: baseUrl,
      },
      secret: 'fixture-only',
    });
    await request('connection.catalog.set-default-target', {
      expectedCatalogRevision: created.catalogRevision,
      target: { connectionId: basis.connectionId, modelId: 'fixture-model' },
    });
    await request('workhub.coordination.resolve', {});
    const session = await request('workhub.coordination.query', {});
    await writeFile(join(session.workspace.hostCwd, 'AGENTS.md'), forbidden);
    const bytes = Buffer.from('ATTACHMENT_EVIDENCE\nsecond line\n');
    const attachment = await upload(
      request,
      sessionId,
      'workhub-upload',
      bytes,
      'evidence.txt',
      'text/plain',
    );
    attachmentPath = 'maka://runtime/attachments/' + attachment.ref.relativePath;
    const input = { turnId, text: '请检查附件并确认 WorkHub', attachments: [attachment] };
    await assert.rejects(
      request('workhub.coordination.answer', input),
      (e) => e.code === 'operation_unavailable',
    );
    assert.equal(requests.length, 0);
    let calls = 0;
    await connection.replaceClientCapabilities(
      {
        offers: () => [
          {
            offerId: 'workhub',
            version: '1',
            affinity: 'session',
            hostPathAccess: 'none',
            label: 'Desktop WorkHub',
            tools: ['control', 'tasks', 'not_allowed'].map((name) => ({
              serverId: 'desktop_workhub',
              name,
              inputSchema: { type: 'object' },
            })),
          },
        ],
        async call(frame, { accept }) {
          try {
            assert.equal(frame.sessionId, sessionId);
            assert.equal(frame.turnId, turnId);
            assert.equal(frame.toolName, 'control');
            assert.equal(Object.hasOwn(frame, 'cwd'), false);
            await accept({ kind: 'none' });
            assert.equal((await request('workhub.coordination.query', {})).id, sessionId);
            calls++;
            return { content: [{ type: 'text', text: 'DESKTOP_CONTROL_VERIFIED' }] };
          } catch (error) {
            failure = error;
            throw error;
          }
        },
        close() {},
      },
      3000,
    );
    observer = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
    assert.deepEqual(await request('workhub.coordination.answer', input), { turnId });
    await observer.waitFor(
      (frame) =>
        frame.kind === 'subscription.session_projection' &&
        frame.snapshot.rootTurn?.turnId === turnId &&
        ['completed', 'failed', 'cancelled'].includes(frame.snapshot.rootTurn.status),
    );
    if (failure) throw failure;
    const terminal = await request('turn.query', { sessionId, turnId });
    assert.equal(terminal.status, 'completed');
    assert.equal(requests.length, 5);
    assert.equal(calls, 1);
    // Transcript pages use the subscription's captured high-water, not live deltas.
    await observer.close();
    observer = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
    const rows = await observer.subscription.loadTranscript(decodeStoredMessage);
    assert(rows.some((row) => row.type === 'assistant' && row.text === 'workhub verified'));
    await connection.unregisterClientCapabilities(3000);
    await request('connection.catalog.remove', {
      expected: { connectionId: basis.connectionId, revision: basis.revision },
    });
    assert.deepEqual(await request('workhub.coordination.answer', input), { turnId });
    await assert.rejects(
      request('workhub.coordination.answer', { ...input, text: 'changed' }),
      (e) => e.code === 'operation_conflict',
    );
    assert.equal(requests.length, 5);
    await writeFile(file, JSON.stringify({ input, terminal, rows }));
  } finally {
    await observer?.close();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
}

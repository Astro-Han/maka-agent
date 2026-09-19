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
import { setTimeout as delay } from 'node:timers/promises';
import { createServer } from 'node:http';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { decodeStoredMessage } from '../../packages/core/src/session.ts';
import { watchSession } from './client-subscription.mjs';

const sessionId = 'maka_workhub_coordination';
const turnId = 'workhub-answer';
const names = ['AskUserQuestion', 'Read', 'mcp__desktop_workhub__control', 'workhub_tasks'];
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
  let failure, attachmentPath, nativeReceipt, selectionTask;
  let contextCalls = 0,
    targetRequests = 0;
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
      const input = JSON.parse(body);
      if (
        selectionTask &&
        input.messages.some((message) => message.content === 'NATIVE_SELECTION_CANCEL')
      ) {
        response.writeHead(200, { 'Content-Type': 'text/event-stream' });
        response.end(
          'data: ' +
            JSON.stringify({
              id: 'native-selection',
              object: 'chat.completion.chunk',
              created: 1,
              model: 'fixture-model',
              choices: [
                {
                  index: 0,
                  delta: {
                    tool_calls: [
                      {
                        index: 0,
                        id: 'selection',
                        type: 'function',
                        function: {
                          name: 'workhub_tasks',
                          arguments: JSON.stringify({ request: selectionTask }),
                        },
                      },
                    ],
                  },
                  finish_reason: 'tool_calls',
                },
              ],
            }) +
            '\n\ndata: [DONE]\n\n',
        );
        return;
      }
      if (!input.tools.some((tool) => tool.function.name === 'workhub_tasks')) {
        assert(body.includes('Create a verification task'));
        assert(
          !input.tools.some((tool) => tool.function.name.startsWith('mcp__desktop_workhub__')),
        );
        targetRequests++;
        response.writeHead(200, { 'Content-Type': 'text/event-stream' });
        response.end(
          'data: ' +
            JSON.stringify({
              id: 'native-task',
              object: 'chat.completion.chunk',
              created: 1,
              model: 'fixture-model',
              choices: [
                { index: 0, delta: { content: 'native task completed' }, finish_reason: 'stop' },
              ],
            }) +
            '\n\ndata: [DONE]\n\n',
        );
        return;
      }
      const step = requests.length;
      requests.push(input);
      assert.equal(request.url, '/v1/chat/completions');
      assert.equal(input.model, 'fixture-model');
      assert.deepEqual(input.tools.map((tool) => tool.function.name).sort(), names);
      assert(!body.includes(forbidden), 'No filesystem or AGENTS content may reach this model');
      if (step === 1 || step === 2) assert(body.includes('WorkHub Read accepts only'));
      if (step === 3) assert(body.includes('ATTACHMENT_EVIDENCE'));
      if (step === 4) assert(body.includes('DESKTOP_CONTROL_VERIFIED'));
      if (step === 5) assert(body.includes('candidateSetId'));
      if (step === 6) {
        nativeReceipt = JSON.parse(input.messages.at(-1).content);
        assert.equal(nativeReceipt.disposition, 'create_new');
        assert.match(nativeReceipt.actionId, /^tool_[a-f0-9]{64}$/);
      }
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
        ['workhub_tasks', { request: { operation: 'candidates' } }],
        [
          'workhub_tasks',
          {
            request: {
              operation: 'create_new',
              title: 'Native verification',
              text: 'Create a verification task',
            },
          },
        ],
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
    const input = {
      turnId,
      text: '请检查附件并确认 WorkHub，然后新建一个验证任务。',
      attachments: [attachment],
    };
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
            tools: ['control', 'context', 'not_allowed'].map((name) => ({
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
            assert.equal(Object.hasOwn(frame, 'cwd'), false);
            await accept({ kind: 'none' });
            if (frame.toolName === 'context') {
              contextCalls++;
              return {
                content: [],
                structuredContent: {
                  workspace: { kind: 'host_path', path: workspace },
                  defaults: { permissionMode: 'bypass' },
                },
              };
            }
            assert.equal(frame.toolName, 'control');
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
    const togglePolicy = async (disabled) => {
      await request('plugin.composition.apply', {
        operations: [{ type: 'update', entryId: 'maka.workhub', patch: { disabled } }],
      });
      const deadline = Date.now() + 5000;
      while (
        (await request('plugin.platform.query', { view: 'status' })).convergence !== 'converged'
      ) {
        assert(Date.now() < deadline, 'WorkHub policy did not converge');
        await delay(10);
      }
    };
    await togglePolicy(true);
    await assert.rejects(
      request('workhub.coordination.answer', input),
      (error) => error.code === 'operation_unavailable',
    );
    assert.equal(requests.length, 0, 'A disabled policy must fail before model dispatch');
    await togglePolicy(false);
    observer = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
    assert.deepEqual(
      await Promise.all([
        request('workhub.coordination.answer', input),
        request('workhub.coordination.answer', input),
      ]),
      [{ turnId }, { turnId }],
    );
    await observer.waitFor(
      (frame) =>
        frame.kind === 'subscription.session_projection' &&
        frame.snapshot.rootTurn?.turnId === turnId &&
        ['completed', 'failed', 'cancelled'].includes(frame.snapshot.rootTurn.status),
    );
    if (failure) throw failure;
    const terminal = await request('turn.query', { sessionId, turnId });
    assert.equal(terminal.status, 'completed');
    assert.equal(requests.length, 7);
    assert.equal(calls, 1);
    assert.equal(contextCalls, 1);
    assert(nativeReceipt);
    const deadline = Date.now() + 5000;
    for (;;) {
      const target = await request('turn.query', {
        sessionId: nativeReceipt.targetSessionId,
        turnId: nativeReceipt.targetTurnId,
      });
      if (target.status === 'completed') break;
      assert(Date.now() < deadline && target.status !== 'failed', 'native task did not complete');
      await delay(10);
    }
    assert.equal(targetRequests, 1);
    const candidates = await request('workhub.coordination.candidates', {});
    selectionTask = {
      operation: 'select_and_delegate',
      candidateSetId: candidates.candidateSetId,
      candidateRefs: candidates.candidates.map((item) => item.candidateRef),
      text: 'Do not deliver this cancelled selection',
    };
    await request('workhub.coordination.answer', {
      turnId: 'native-selection',
      text: 'NATIVE_SELECTION_CANCEL',
    });
    const offered = await observer.waitFor(
      (frame) =>
        frame.kind === 'subscription.session_projection' &&
        frame.snapshot.rootTurn?.turnId === 'native-selection' &&
        frame.snapshot.interactions.pending.some((form) => form.request.kind === 'form'),
    );
    const form = offered.snapshot.interactions.pending.find((form) => form.request.kind === 'form');
    const selecting = await request('turn.query', { sessionId, turnId: 'native-selection' });
    await request('turn.stop', { sessionId, turnId: selecting.turnId, runId: selecting.runId });
    await observer.waitFor(
      (frame) =>
        frame.kind === 'subscription.session_projection' &&
        frame.snapshot.rootTurn?.turnId === 'native-selection' &&
        frame.snapshot.rootTurn.status === 'cancelled' &&
        frame.snapshot.interactions.pending.length === 0,
    );
    await assert.rejects(
      request('interaction.answer', {
        sessionId,
        interactionId: form.interactionId,
        answer: {
          kind: 'form',
          action: 'accept',
          values: { target: form.request.fields[0].options[0].value },
        },
      }),
      (error) => error.code === 'already_resolved',
    );
    assert.equal(targetRequests, 1, 'cancelled native selection must not delegate');
    // Transcript pages use the subscription's captured high-water, not live deltas.
    await observer.close();
    observer = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
    const rows = await observer.subscription.loadTranscript(decodeStoredMessage);
    assert(rows.some((row) => row.type === 'assistant' && row.text === 'workhub verified'));
    await togglePolicy(true);
    await connection.unregisterClientCapabilities(3000);
    await request('connection.catalog.remove', {
      expected: { connectionId: basis.connectionId, revision: basis.revision },
    });
    assert.deepEqual(await request('workhub.coordination.answer', input), { turnId });
    await assert.rejects(
      request('workhub.coordination.answer', { ...input, text: 'changed' }),
      (e) => e.code === 'operation_conflict',
    );
    assert.equal(requests.length, 7);
    await writeFile(file, JSON.stringify({ input, terminal, rows }));
  } finally {
    await observer?.close();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
}

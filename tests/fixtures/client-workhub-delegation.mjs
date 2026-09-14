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
import { once } from 'node:events';
import { createServer } from 'node:http';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { decodeStoredMessage } from '../../packages/core/src/session.ts';
import { watchSession } from './client-subscription.mjs';
import { createInput, querySession } from './client-runtime-policy-fixture.mjs';

const sessionId = 'maka_workhub_coordination';
const turnId = 'delegate-request';

export async function verifyWorkhubDelegation(connection, workspace, reopened) {
  const request = (operation, input) => connection.request(operation, input, 5000);
  const file = join(workspace, 'delegation.json');
  const act = (input) => request('workhub.coordination.actFromTurn', input);
  if (reopened) {
    const saved = JSON.parse(await readFile(file, 'utf8'));
    assert.deepEqual(await act(saved.input), saved.receipt);
    assert.deepEqual(
      await request('turn.query', {
        sessionId: saved.receipt.targetSessionId,
        turnId: saved.receipt.targetTurnId,
      }),
      saved.target,
    );
    const observer = await watchSession(connection, saved.receipt.targetSessionId, {
      kind: 'tail',
      maxBytes: 2,
    });
    try {
      assert.deepEqual(await observer.subscription.loadTranscript(decodeStoredMessage), saved.rows);
    } finally {
      await observer.close();
    }
    return;
  }
  let failure,
    calls = 0,
    input,
    receipt;
  const server = createServer(async (req, response) => {
    try {
      let body = '';
      for await (const chunk of req) body += chunk;
      const data = JSON.parse(body);
      const target = data.messages.some(
        (message) =>
          typeof message.content === 'string' &&
          message.content.includes('Delegated task:\nImplement the requested task'),
      );
      const finished = data.messages.some((message) => message.role === 'tool');
      const delta = target
        ? { content: 'target completed' }
        : finished
          ? { content: 'delegation completed' }
          : {
              tool_calls: [
                {
                  index: 0,
                  id: 'delegate-call',
                  type: 'function',
                  function: { name: 'mcp__desktop_workhub__tasks', arguments: '{}' },
                },
              ],
            };
      if (target) {
        assert(
          data.messages.some(
            (message) =>
              typeof message.content === 'string' &&
              message.content.includes('User request:\nPlease implement the requested task'),
          ),
        );
        assert(!data.tools.some((tool) => tool.function.name.startsWith('mcp__desktop_workhub__')));
      }
      const chunk = (delta, finish_reason) => ({
        id: 'chat-delegation',
        object: 'chat.completion.chunk',
        created: 1,
        model: 'fixture-model',
        choices: [{ index: 0, delta, finish_reason }],
      });
      response.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
      response.end(
        [chunk(delta, null), chunk({}, target || finished ? 'stop' : 'tool_calls')]
          .map((event) => 'data: ' + JSON.stringify(event) + '\n\n')
          .join('') + 'data: [DONE]\n\n',
      );
    } catch (error) {
      failure = error;
      response.destroy(error);
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  let sourceObserver, targetObserver;
  try {
    const baseUrl = 'http://127.0.0.1:' + server.address().port + '/v1';
    const created = await request('connection.catalog.create', {
      expectedCatalogRevision: 0,
      connection: {
        slug: 'delegation-fixture',
        name: 'Delegation fixture',
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
        slug: 'delegation-fixture',
        providerType: 'openai-compatible',
        effectiveBaseUrl: baseUrl,
      },
      secret: 'fixture-only',
    });
    await request('connection.catalog.set-default-target', {
      expectedCatalogRevision: created.catalogRevision,
      target: { connectionId: basis.connectionId, modelId: 'fixture-model' },
    });
    for (const [id, extra] of [
      ['target', {}],
      ['side', { labels: ['mode:side_conversation'] }],
      ['planned', { collaborationMode: 'plan' }],
      ['archived', {}],
    ])
      await request('session.create', { ...createInput(workspace, id, 'bypass'), ...extra });
    await request('session.lifecycle.set', { sessionId: 'archived', state: 'archived' });
    await request('workhub.coordination.resolve', {});
    const initial = await request('workhub.coordination.candidates', {});
    assert.deepEqual(
      initial.candidates.map((candidate) => candidate.sessionId),
      ['target'],
    );
    targetObserver = await watchSession(connection, 'target', { kind: 'tail', maxBytes: 2 });
    await connection.replaceClientCapabilities(
      {
        offers: () => [
          {
            offerId: 'workhub',
            label: 'Desktop WorkHub',
            version: '1',
            affinity: 'session',
            hostPathAccess: 'none',
            tools: ['control', 'tasks'].map((name) => ({
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
            assert.equal(frame.toolName, 'tasks');
            await accept({ kind: 'none' });
            assert.deepEqual(
              await request('workhub.coordination.candidates', {}),
              initial,
              'WorkHub steps must not invalidate an unchanged candidate set',
            );
            const selected = initial.candidates[0];
            const current = await querySession(request, selected.sessionId);
            await request('session.metadata.update', {
              sessionId: current.id,
              expectedRevision: current.revision,
              patch: { name: 'renamed target' },
            });
            input = {
              turnId,
              actionId: 'delegation-action',
              candidateSetId: initial.candidateSetId,
              proposal: { disposition: 'delegate_existing', candidateRef: selected.candidateRef },
              delegationText: 'Implement the requested task',
            };
            await assert.rejects(act(input), (error) => error.code === 'candidate_set_stale');
            const fresh = await request('workhub.coordination.candidates', {});
            input.candidateSetId = fresh.candidateSetId;
            input.proposal.candidateRef = fresh.candidates[0].candidateRef;
            await assert.rejects(
              act({ ...input, turnId: 'not-active' }),
              (error) => error.code === 'operation_conflict',
            );
            receipt = await act(input);
            assert.equal(receipt.disposition, 'delegate_existing');
            assert.equal(receipt.targetSessionId, 'target');
            assert.deepEqual(await act(input), receipt);
            await assert.rejects(
              act({ ...input, delegationText: 'changed request' }),
              (error) => error.code === 'operation_conflict',
            );
            calls++;
            return { content: [{ type: 'text', text: 'Delegation admitted' }] };
          } catch (error) {
            failure = error;
            throw error;
          }
        },
        close() {},
      },
      3000,
    );
    sourceObserver = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
    await request('workhub.coordination.answer', {
      turnId,
      text: 'Please implement the requested task',
    });
    await sourceObserver.waitFor(
      (frame) =>
        frame.kind === 'subscription.session_projection' &&
        frame.snapshot.rootTurn?.turnId === turnId &&
        ['completed', 'failed', 'cancelled'].includes(frame.snapshot.rootTurn.status),
    );
    if (failure) throw failure;
    assert.equal(calls, 1);
    assert.equal((await request('turn.query', { sessionId, turnId })).status, 'completed');
    await targetObserver.waitFor(
      (frame) =>
        frame.kind === 'subscription.session_projection' &&
        frame.snapshot.rootTurn?.turnId === receipt.targetTurnId &&
        ['completed', 'failed', 'cancelled'].includes(frame.snapshot.rootTurn.status),
    );
    if (failure) throw failure;
    const target = await request('turn.query', {
      sessionId: 'target',
      turnId: receipt.targetTurnId,
    });
    assert.equal(target.status, 'completed');
    await targetObserver.close();
    targetObserver = await watchSession(connection, 'target', { kind: 'tail', maxBytes: 2 });
    const rows = await targetObserver.subscription.loadTranscript(decodeStoredMessage);
    assert(rows.some((row) => row.type === 'assistant' && row.text === 'target completed'));
    await connection.unregisterClientCapabilities(3000);
    await request('connection.catalog.remove', {
      expected: { connectionId: basis.connectionId, revision: basis.revision },
    });
    assert.deepEqual(await act(input), receipt);
    await writeFile(file, JSON.stringify({ input, receipt, target, rows }));
  } finally {
    await sourceObserver?.close();
    await targetObserver?.close();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
}

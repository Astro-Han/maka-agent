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
import { watchSession } from './client-subscription.mjs';

const modelId = 'gpt-5.2';
const wires = ['openai-chat', 'openai-responses'];
const efforts = ['medium', 'high', 'none', 'medium'];

async function fixture(port) {
  const requests = [];
  let failure;
  const server = createServer(async (request, response) => {
    try {
      let body = '';
      for await (const chunk of request) {
        body += chunk;
        assert(Buffer.byteLength(body) < 128 * 1024);
      }
      const input = JSON.parse(body);
      const index = requests.length;
      requests.push(input);
      assert(index < 8, 'rejected configurations must not dispatch HTTP requests');
      const responses = index >= 4;
      const effort = efforts[index % 4];
      assert.equal(request.url, responses ? '/v1/responses' : '/v1/chat/completions');
      assert.equal(request.headers.authorization, 'Bearer dummy-options-fixture');
      assert.equal(input.model, modelId);
      assert.equal(input.stream, true);
      assert.equal(input.store, false);
      assert.equal(input.parallel_tool_calls, false);
      if (responses) {
        assert.deepEqual(input.reasoning, {
          effort,
          ...(effort === 'none' ? {} : { summary: 'auto' }),
        });
      } else {
        assert.equal(input.reasoning_effort, effort);
      }
      response.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
      // Minimal SSE shapes accepted by the installed SDK; no provider is contacted.
      const events = responses
        ? [
            {
              type: 'response.created',
              response: { id: `resp_${index}`, created_at: 1, model: modelId },
            },
            {
              type: 'response.output_item.added',
              output_index: 0,
              item: { type: 'message', id: `msg_${index}`, role: 'assistant', content: [] },
            },
            {
              type: 'response.output_text.delta',
              item_id: `msg_${index}`,
              output_index: 0,
              content_index: 0,
              delta: 'accepted',
            },
            {
              type: 'response.output_item.done',
              output_index: 0,
              item: {
                type: 'message',
                id: `msg_${index}`,
                role: 'assistant',
                content: [{ type: 'output_text', text: 'accepted', annotations: [] }],
              },
            },
            {
              type: 'response.completed',
              response: { usage: { input_tokens: 1, output_tokens: 1 } },
            },
          ]
        : [
            {
              id: `chat_${index}`,
              object: 'chat.completion.chunk',
              created: 1,
              model: modelId,
              choices: [{ index: 0, delta: { content: 'accepted' }, finish_reason: null }],
            },
            {
              id: `chat_${index}`,
              object: 'chat.completion.chunk',
              created: 1,
              model: modelId,
              choices: [{ index: 0, delta: {}, finish_reason: 'stop' }],
            },
          ];
      response.end(
        events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join('') +
          (responses ? '' : 'data: [DONE]\n\n'),
      );
    } catch (error) {
      failure = error;
      response.destroy(error);
    }
  });
  server.on('upgrade', (_request, socket) => {
    socket.end('HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n');
  });
  server.listen(port, '127.0.0.1');
  await once(server, 'listening');
  return {
    baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
    verify() {
      if (failure) throw failure;
      assert.equal(requests.length, 8);
    },
    async close() {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

export async function verifyOpenaiOptions(connection, workspace, reopened) {
  const request = (operation, input) => connection.request(operation, input, 3000);
  const query = async (sessionId) =>
    (await request('session.catalog.query', { kind: 'get', sessionId })).session;
  const snapshotPath = join(workspace, 'openai-options.json');
  const snapshot = async () => ({
    sessions: await Promise.all(wires.map((wire) => query(wire))),
    catalog: await request('connection.catalog.query', { kind: 'start' }),
  });
  if (reopened) {
    assert.equal(JSON.stringify(await snapshot()), await readFile(snapshotPath, 'utf8'));
    console.log(
      JSON.stringify({ check: 'original-client-openai-options-reopened', result: 'passed' }),
    );
    return;
  }
  const initial = await request('connection.catalog.query', { kind: 'start' });
  const endpoint = initial.items.find((item) => item.kind === 'connection').baseUrl;
  const provider = await fixture(Number(new URL(endpoint).port));
  try {
    for (const wire of wires) {
      const catalog = await request('connection.catalog.query', { kind: 'start' });
      const row = catalog.items.find((item) => item.kind === 'connection' && item.slug === wire);
      const basis = { connectionId: row.connectionId, revision: row.revision };
      assert.equal(
        (
          await request('credential.vault.set', {
            locator: { scope: 'connection', connectionId: basis.connectionId, kind: 'api_key' },
            expected: null,
            expectedConnection: {
              ...basis,
              slug: wire,
              providerType: 'openai',
              effectiveBaseUrl: provider.baseUrl,
            },
            secret: 'dummy-options-fixture',
          })
        ).kind,
        'committed',
      );
      assert.equal(
        (
          await request('connection.catalog.set-default-target', {
            expectedCatalogRevision: catalog.revision,
            target: { connectionId: basis.connectionId, modelId },
          })
        ).kind,
        'committed',
      );
      const session = await request('session.create', {
        sessionId: wire,
        workspace: { kind: 'host_path', path: workspace },
        modelTarget: { kind: 'default' },
        mode: 'bot',
      });
      assert.equal(Object.hasOwn(session, 'thinkingLevel'), false);
      const live = await watchSession(connection, wire);
      try {
        for (const [index, level] of [undefined, 'high', 'off', null].entries()) {
          const before = await query(wire);
          if (index > 0) {
            const result = await request('session.configuration.update', {
              sessionId: wire,
              expectedRevision: before.revision,
              patch: { thinkingLevel: level },
            });
            assert.equal(result.kind, 'committed');
            assert.equal(result.session.revision, before.revision + 1);
            if (level === null) assert.equal(Object.hasOwn(result.session, 'thinkingLevel'), false);
            else assert.equal(result.session.thinkingLevel, level);
            assert.deepEqual(
              await query(wire),
              result.session,
              'public result is the full committed Session',
            );
          }
          const turnId = `${wire}-${index}`;
          await request('turn.start', {
            sessionId: wire,
            turnId,
            content: { text: 'reply once' },
            maxSteps: 1,
          });
          await live.waitFor(
            (frame) =>
              frame.kind === 'subscription.session_projection' &&
              frame.snapshot.rootTurn?.turnId === turnId &&
              ['completed', 'failed', 'cancelled'].includes(frame.snapshot.rootTurn.status),
          );
          assert.equal(
            (await request('turn.query', { sessionId: wire, turnId })).status,
            'completed',
          );
        }
        const before = await query(wire);
        await assert.rejects(
          request('session.configuration.update', {
            sessionId: wire,
            expectedRevision: before.revision,
            patch: { thinkingLevel: 'max' },
          }),
          (error) => error.code === 'invalid_request',
        );
        assert.deepEqual(
          await query(wire),
          before,
          'unsupported thinking changes neither configuration nor execution',
        );
      } finally {
        await live.close();
      }
    }
    provider.verify();
    const stored = await snapshot();
    const models = stored.catalog.items.filter((item) => item.kind === 'model');
    assert.equal(models.length, 2);
    for (const item of models) assert.equal(item.model.capabilities.parallelToolCalls, false);
    await writeFile(snapshotPath, JSON.stringify(stored));
    console.log(JSON.stringify({ check: 'original-client-openai-options', result: 'passed' }));
  } finally {
    await provider.close();
  }
}

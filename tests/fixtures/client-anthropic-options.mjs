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
import { readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { join } from 'node:path';
import { watchSession } from './client-subscription.mjs';

const models = ['claude-sonnet-4-5', 'claude-opus-4-6'];
const signature = 'fixture-signed-thinking';

async function fixture() {
  let count = 0;
  let failure;
  const server = createServer(async (request, response) => {
    try {
      let body = '';
      for await (const chunk of request) {
        body += chunk;
        assert(Buffer.byteLength(body) < 128 * 1024);
      }
      const input = JSON.parse(body);
      const index = count++;
      assert(index < 6, 'invalid changes must never dispatch');
      const adaptive = index >= 3;
      assert.equal(request.url, '/v1/messages');
      assert.equal(request.headers['x-api-key'], 'dummy-anthropic-options');
      assert.equal(request.headers['x-maka-context'], index === 0 ? 'initial' : 'updated');
      const betas = request.headers['anthropic-beta'].split(',');
      assert(betas.includes('interleaved-thinking-2025-05-14'));
      assert(betas.includes('fine-grained-tool-streaming-2025-05-14'));
      assert.equal(input.model, models[adaptive ? 1 : 0]);
      assert.equal(input.stream, true);
      assert.equal(input.max_tokens, 7000);
      assert.deepEqual(input.fixture_context, { revision: index === 0 ? 1 : 2 });
      assert.equal(Object.hasOwn(input, 'parallel_tool_calls'), false);
      assert.deepEqual(input.cache_control, { type: 'ephemeral' });
      assert.deepEqual(
        input.thinking,
        adaptive
          ? { type: 'adaptive', display: 'summarized' }
          : index === 1
            ? { type: 'disabled' }
            : { type: 'enabled', budget_tokens: 1024 },
      );
      assert.deepEqual(input.output_config, index === 4 ? { effort: 'high' } : undefined);
      if (index === 4 || index === 5) {
        const thinking = input.messages
          .filter((message) => message.role === 'assistant')
          .flatMap((message) => message.content)
          .filter((part) => part.type === 'thinking');
        assert.equal(thinking.length, index - 3);
        for (const part of thinking) {
          assert.equal(part.thinking, 'considered');
          assert.equal(part.signature, signature);
        }
      }
      // These SSE blocks follow the installed Anthropic SDK's stream schema.
      const events = [
        {
          type: 'message_start',
          message: {
            id: `msg_${index}`,
            type: 'message',
            role: 'assistant',
            model: input.model,
            content: [],
            stop_reason: null,
            stop_sequence: null,
            usage: { input_tokens: 1, output_tokens: 0 },
          },
        },
        ...(adaptive
          ? [
              {
                type: 'content_block_start',
                index: 0,
                content_block: { type: 'thinking', thinking: '', signature: '' },
              },
              {
                type: 'content_block_delta',
                index: 0,
                delta: { type: 'thinking_delta', thinking: 'considered' },
              },
              {
                type: 'content_block_delta',
                index: 0,
                delta: { type: 'signature_delta', signature },
              },
              { type: 'content_block_stop', index: 0 },
            ]
          : []),
        { type: 'content_block_start', index: 1, content_block: { type: 'text', text: '' } },
        { type: 'content_block_delta', index: 1, delta: { type: 'text_delta', text: 'accepted' } },
        { type: 'content_block_stop', index: 1 },
        {
          type: 'message_delta',
          delta: { stop_reason: 'end_turn', stop_sequence: null },
          usage: { output_tokens: 2 },
        },
        { type: 'message_stop' },
      ];
      response.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
      response.end(events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join(''));
    } catch (error) {
      failure = error;
      response.destroy(error);
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  return {
    baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
    verify() {
      if (failure) throw failure;
      assert.equal(count, 6);
    },
    async close() {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

export async function verifyAnthropicOptions(connection, workspace, reopened) {
  const request = (operation, input) => connection.request(operation, input, 3000);
  const query = async (sessionId) =>
    (await request('session.catalog.query', { kind: 'get', sessionId })).session;
  const snapshot = async () => ({
    sessions: await Promise.all(models.map(query)),
    catalog: await request('connection.catalog.query', { kind: 'start' }),
  });
  const path = join(workspace, 'anthropic-options.json');
  if (reopened) {
    assert.equal(JSON.stringify(await snapshot()), await readFile(path, 'utf8'));
    console.log('original-client-anthropic-options-reopened');
    return;
  }
  const provider = await fixture();
  try {
    const created = await request('connection.catalog.create', {
      expectedCatalogRevision: 0,
      connection: {
        slug: 'anthropic-fixture',
        name: 'Anthropic fixture',
        providerType: 'anthropic',
        baseUrl: provider.baseUrl,
        enabled: true,
        enabledModelIds: models,
        modelOverrides: Object.fromEntries(models.map((id) => [id, { maxOutputTokens: 7000 }])),
        requestBodyOverlay: { fixture_context: { revision: 1 } },
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
            slug: 'anthropic-fixture',
            providerType: 'anthropic',
            effectiveBaseUrl: provider.baseUrl,
          },
          secret: 'dummy-anthropic-options',
        })
      ).kind,
      'committed',
    );
    assert.deepEqual(
      await request('connection.request-headers.replace', {
        connectionId: basis.connectionId,
        headers: [{ name: 'X-Maka-Context', value: 'initial' }],
      }),
      { kind: 'committed', names: ['X-Maka-Context'] },
    );
    const catalog = await request('connection.catalog.query', { kind: 'start' });
    for (const [modelIndex, model] of models.entries()) {
      const entry = catalog.items.find(
        (item) => item.kind === 'catalog_entry' && item.entry.id === model,
      ).entry;
      assert.deepEqual(
        entry.thinkingLevels,
        modelIndex === 0 ? ['off'] : ['low', 'medium', 'high', 'max'],
      );
      const session = await request('session.create', {
        sessionId: model,
        workspace: { kind: 'host_path', path: workspace },
        mode: 'bot',
        modelTarget: {
          kind: 'explicit',
          connectionId: basis.connectionId,
          connectionSlug: 'anthropic-fixture',
          model,
        },
      });
      assert.equal(Object.hasOwn(session, 'thinkingLevel'), false);
      const live = await watchSession(connection, model);
      try {
        for (const [index, level] of [
          undefined,
          modelIndex === 0 ? 'off' : 'high',
          null,
        ].entries()) {
          if (index > 0) {
            if (modelIndex === 0 && index === 1) {
              assert.deepEqual(
                await request('connection.request-headers.replace', {
                  connectionId: basis.connectionId,
                  headers: [{ name: 'X-Maka-Context', value: 'updated' }],
                }),
                { kind: 'committed', names: ['X-Maka-Context'] },
              );
              const page = await request('connection.catalog.query', { kind: 'start' });
              const row = page.items.find((item) => item.kind === 'connection');
              assert.equal(
                (
                  await request('connection.catalog.update', {
                    expected: { connectionId: row.connectionId, revision: row.revision },
                    changes: {
                      name: row.name,
                      baseUrl: provider.baseUrl,
                      enabled: true,
                      enabledModelIds: models,
                      requestBodyOverlay: { fixture_context: { revision: 2 } },
                    },
                  })
                ).kind,
                'committed',
              );
            }
            const before = await query(model);
            const result = await request('session.configuration.update', {
              sessionId: model,
              expectedRevision: before.revision,
              patch: { thinkingLevel: level },
            });
            assert.equal(result.kind, 'committed');
            assert.equal(result.session.revision, before.revision + 1);
            if (level === null) assert.equal(Object.hasOwn(result.session, 'thinkingLevel'), false);
            else assert.equal(result.session.thinkingLevel, level);
            assert.deepEqual(await query(model), result.session);
          }
          const turnId = `${model}-${index}`;
          await request('turn.start', {
            sessionId: model,
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
            (await request('turn.query', { sessionId: model, turnId })).status,
            'completed',
          );
        }
        const before = await query(model);
        await assert.rejects(
          request('session.configuration.update', {
            sessionId: model,
            expectedRevision: before.revision,
            patch: { thinkingLevel: modelIndex === 0 ? 'high' : 'off' },
          }),
          (error) => error.code === 'invalid_request',
        );
        assert.deepEqual(await query(model), before);
      } finally {
        await live.close();
      }
    }
    provider.verify();
    await writeFile(path, JSON.stringify(await snapshot()));
    console.log('original-client-anthropic-options');
  } finally {
    await provider.close();
  }
}

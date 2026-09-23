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
import { writeFile, rename } from 'node:fs/promises';
import { createServer } from 'node:http';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { connectExistingRuntimeHost } from '../../packages/runtime-host/src/client/connection.js';

const { values } = parseArgs({
  options: {
    'tui-form-root': { type: 'string' },
    workspace: { type: 'string' },
    'yield-ms': { type: 'string', default: '1000' },
  },
});
const yieldMs = Number(values['yield-ms']);
assert(Number.isSafeInteger(yieldMs) && yieldMs >= 0 && yieldMs <= 60000);
const { connection, kind } = await connectExistingRuntimeHost({
  rootPath: values['tui-form-root'],
  protocol: { min: 0, max: 0 },
  compositionId: 'maka.interactive',
});
assert.equal(kind, 'connected');
const request = (op, input) => connection.request(op, input, 5000);
const sessionId = 'tui-form';
const answered = Promise.withResolvers();
let requests = 0,
  calls = 0,
  failure;
async function marker(name, value) {
  const path = join(values.workspace, name);
  await writeFile(path + '.tmp', JSON.stringify(value));
  await rename(path + '.tmp', path + '.json');
}
const server = createServer(async (req, res) => {
  try {
    let raw = '';
    for await (const part of req) raw += part;
    const body = JSON.parse(raw);
    assert.equal(req.url, '/v1/chat/completions');
    await marker('model-requests', ++requests);
    const tool = (name, args) => ({
      tool_calls: [
        {
          index: 0,
          id: `call-${requests}`,
          type: 'function',
          function: { name, arguments: JSON.stringify(args) },
        },
      ],
    });
    let delta;
    if (requests === 1) {
      assert(body.messages.some((message) => message.content === 'Collect a form'));
      delta = tool('exec', {
        code: 'return await tools.tool_search({query:"mcp__tui_forms__collect"});',
      });
    } else if (requests === 2) {
      delta = tool('exec', {
        code: 'return await tools.mcp__tui_forms__collect({});',
        yield_time_ms: yieldMs,
      });
    } else {
      // Keep a broken Host from spinning; the PTY test independently requires
      // exactly two HTTP requests while the canonical form remains unanswered.
      const result = await answered.promise;
      const last = JSON.parse(body.messages.findLast((message) => message.role === 'tool').content);
      if (last.state === 'running') {
        delta = tool('wait', { cell_id: last.cell_id, yield_time_ms: 1000 });
      } else {
        assert.equal(last.state, 'completed');
        assert(JSON.stringify(last).includes('中文🦀'));
        assert.deepEqual(result, {
          action: 'accept',
          values: { name: '中文🦀', count: 2, enabled: false },
        });
        delta = { content: 'Form received exactly once' };
      }
    }
    assert(requests < 10, 'bounded model fixture');
    const chunk = (delta, finish_reason) =>
      'data: ' +
      JSON.stringify({
        id: `form-${requests}`,
        object: 'chat.completion.chunk',
        model: 'fixture-model',
        choices: [{ index: 0, delta, finish_reason }],
      }) +
      '\n\n';
    res.writeHead(200, {
      'Content-Type': 'text/event-stream',
      Connection: 'close',
    });
    res.end(
      chunk(delta, null) + chunk({}, delta.tool_calls ? 'tool_calls' : 'stop') + 'data: [DONE]\n\n',
    );
  } catch (error) {
    failure = error;
    res.destroy();
  }
});
server.listen(0, '127.0.0.1');
await once(server, 'listening');
let subscription;
try {
  const baseUrl = `http://127.0.0.1:${server.address().port}/v1`;
  const created = await request('connection.catalog.create', {
    expectedCatalogRevision: 0,
    connection: {
      slug: 'tui-form',
      name: 'TUI form',
      providerType: 'openai-compatible',
      baseUrl,
      enabled: true,
      enabledModelIds: ['fixture-model'],
      modelOverrides: { 'fixture-model': { codeMode: true } },
    },
  });
  assert.equal(created.kind, 'committed');
  const basis = created.connection;
  await request('credential.vault.set', {
    locator: {
      scope: 'connection',
      connectionId: basis.connectionId,
      kind: 'api_key',
    },
    expected: null,
    expectedConnection: {
      ...basis,
      slug: 'tui-form',
      providerType: 'openai-compatible',
      effectiveBaseUrl: baseUrl,
    },
    secret: 'dummy-local-form-fixture',
  });
  await request('connection.catalog.set-default-target', {
    expectedCatalogRevision: created.catalogRevision,
    target: { connectionId: basis.connectionId, modelId: 'fixture-model' },
  });
  await request('session.create', {
    sessionId,
    name: 'Form keyboard fixture',
    workspace: { kind: 'host_path', path: values.workspace },
    modelTarget: { kind: 'default' },
    sandboxMode: 'danger-full-access',
  });
  await connection.replaceClientCapabilities(
    {
      offers: () => [
        {
          offerId: 'desktop_mcp_tui_forms',
          version: '1',
          affinity: 'session',
          hostPathAccess: 'none',
          label: 'TUI forms',
          tools: [
            {
              serverId: 'tui_forms',
              name: 'collect',
              inputSchema: { type: 'object' },
            },
          ],
        },
      ],
      async call(frame, context) {
        assert.equal(frame.source.sessionId, sessionId);
        assert.equal(++calls, 1);
        await context.accept({ kind: 'none' });
        const result = await context.requestInteraction({
          message: 'Fill the isolated form',
          requester: { name: 'TUI form fixture' },
          fields: [
            {
              name: 'name',
              label: 'Display name',
              kind: 'string',
              required: true,
              minLength: 1,
              maxLength: 16,
            },
            {
              name: 'count',
              label: 'Count',
              kind: 'integer',
              required: true,
              minimum: 1,
              maximum: 3,
              default: 2,
            },
            {
              name: 'enabled',
              label: 'Enabled',
              kind: 'boolean',
              required: true,
              default: false,
            },
          ],
        });
        await marker('form-result', result);
        answered.resolve(result);
        return { content: [{ type: 'text', text: JSON.stringify(result) }] };
      },
    },
    5000,
  );
  subscription = await connection.openSessionSubscription(
    { sessionId, transcript: { kind: 'none' } },
    5000,
  );
  await subscription.ready();
  await marker('provider-ready', true);
  for await (const frame of subscription) {
    if (frame.kind !== 'subscription.session_projection') continue;
    const turn = frame.snapshot.rootTurn;
    if (turn && ['completed', 'cancelled', 'failed'].includes(turn.status)) {
      if (failure) throw failure;
      assert.equal(turn.status, 'completed');
      assert.equal(calls, 1);
      await marker('form-completed', turn);
      break;
    }
  }
} finally {
  await subscription?.close();
  await connection.close();
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
}

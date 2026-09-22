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
import { access, readFile, rename, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import {
  RuntimeHostOperationError,
  RuntimeHostRequestInterruptedError,
} from '../../packages/runtime-host/src/client/connection.ts';
import { discover, events } from './client-capability-model-fixture.mjs';
import { watchSession } from './client-subscription.mjs';
const sessionId = 'provider-forms';
const results = [
  { action: 'accept', values: { count: 2 } },
  { action: 'decline' },
  { action: 'cancel' },
];
const form = {
  message: 'Choose a count',
  requester: { name: 'Form fixture' },
  fields: [
    { name: 'count', label: 'Count', kind: 'integer', required: true, minimum: 1, maximum: 3 },
  ],
};
export async function verifyForms(connection, workspace, reopened, openClient, disconnected) {
  const request = (op, input) => connection.request(op, input, 3000);
  const query = (interactionId) => request('interaction.query', { sessionId, interactionId });
  const answer = (interactionId, value) =>
    request('interaction.answer', { sessionId, interactionId, answer: value });
  const saved = join(workspace, 'forms.json');
  if (reopened) {
    for (const snapshot of JSON.parse(await readFile(saved, 'utf8'))) {
      const actual = await query(snapshot.interactionId);
      if (snapshot.status === 'pending') {
        assert.equal(actual.status, 'closed');
        assert.deepEqual(actual.request, snapshot.request);
        assert.equal(actual.outcome.reason, disconnected ? 'producer_cancelled' : 'turn_stopped');
        await assert.rejects(
          answer(snapshot.interactionId, { kind: 'form', action: 'cancel' }),
          (e) => e.code === 'already_resolved',
        );
      } else assert.deepEqual(actual, snapshot);
      if (snapshot.status === 'answered') {
        const { action, values } = snapshot.outcome;
        assert.deepEqual(
          await answer(snapshot.interactionId, {
            kind: 'form',
            action,
            ...(values ? { values } : {}),
          }),
          snapshot,
        );
      }
    }
    console.log('original-client-forms-reopened');
    return;
  }
  async function checkpoint(name, value) {
    const file = join(workspace, name);
    await writeFile(file + '.tmp', JSON.stringify(value));
    await rename(file + '.tmp', file + '.json');
    for (;;) {
      try {
        await access(file + '.ok');
        return;
      } catch {
        await delay(5);
      }
    }
  }
  let modelCalls = 0,
    searches = 0,
    providerCalls = disconnected ? 3 : 0,
    returned = 0,
    failure;
  const snapshots = [],
    frames = [];
  const server = createServer(async (req, res) => {
    try {
      let raw = '';
      for await (const chunk of req) raw += chunk;
      const input = JSON.parse(raw);
      const script = disconnected
        ? [{ tool: true, index: 4 }]
        : [{ tool: true, index: 1 }, {}, { tool: true, index: 2 }, {}, { tool: true, index: 3 }];
      const search = discover(input, ['mcp__forms__collect']);
      if (search) searches++;
      const action = search ?? script[modelCalls++];
      assert(action, 'no replay or unexpected model continuation');
      if (!action.tool) {
        assert(input.input.at(-1).output.some((p) => p.text === 'form complete'));
        await checkpoint('settled-' + returned, { returned });
      }
      res.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
      res.end(
        events(search ?? { ...action, name: 'mcp__forms__collect' }, modelCalls + searches)
          .map((e) => 'data: ' + JSON.stringify(e) + '\n\n')
          .join(''),
      );
    } catch (error) {
      failure = error;
      res.destroy(error);
    }
  });
  server.on('upgrade', (_request, socket) => {
    socket.end('HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n');
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  let provider, live;
  let verifiedRootDrain = false;
  try {
    const baseUrl = 'http://127.0.0.1:' + server.address().port + '/v1';
    const created = await request('connection.catalog.create', {
      expectedCatalogRevision: 0,
      connection: {
        slug: 'forms',
        name: 'Forms',
        providerType: 'openai',
        baseUrl,
        enabled: true,
        enabledModelIds: ['gpt-5.2'],
        modelOverrides: { 'gpt-5.2': { codeMode: false } },
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
            slug: 'forms',
            providerType: 'openai',
            effectiveBaseUrl: baseUrl,
          },
          secret: 'dummy-form-fixture',
        })
      ).kind,
      'committed',
    );
    await request('connection.catalog.set-default-target', {
      expectedCatalogRevision: created.catalogRevision,
      target: { connectionId: basis.connectionId, modelId: 'gpt-5.2' },
    });
    await request('session.create', {
      sessionId,
      workspace: { kind: 'host_path', path: workspace },
      modelTarget: { kind: 'default' },
      sandboxMode: 'danger-full-access',
    });
    const opened = await openClient();
    provider = opened.connection;
    const write = opened.transport.write.bind(opened.transport);
    opened.transport.write = async (bytes) => {
      const frame = JSON.parse(bytes.toString('utf8'));
      if (frame.kind === 'client.capability.interaction_request') frames.push(frame);
      return write(bytes);
    };
    await provider.replaceClientCapabilities(
      {
        offers: () => [
          {
            offerId: 'forms',
            version: '1',
            affinity: 'session',
            hostPathAccess: 'none',
            label: 'Forms',
            tools: [{ serverId: 'forms', name: 'collect', inputSchema: { type: 'object' } }],
          },
        ],
        async call(frame, context) {
          const index = ++providerCalls;
          assert.equal(frame.arguments.index, index);
          await context.accept({ kind: 'none' });
          try {
            for (const ordinal of index === 1 ? [1, 2] : [index + 1]) {
              const result = await context.requestInteraction(form);
              assert(index <= 2, 'closed form must reject its provider waiter');
              assert.deepEqual(result, results[ordinal - 1]);
              await checkpoint('received-' + ordinal, { frame, result, ordinal });
            }
            returned++;
            return { content: [{ type: 'text', text: 'form complete' }] };
          } catch (error) {
            if (index <= 2 || error.code === 'ERR_ASSERTION') failure = error;
            throw error;
          }
        },
      },
      3000,
    );
    live = await watchSession(connection, sessionId);
    for (const index of disconnected ? [4] : [1, 2, 3]) {
      const turnId = 'form-' + index;
      await request('turn.start', { sessionId, turnId, content: { text: turnId }, maxSteps: 3 });
      for (const ordinal of index === 1 ? [1, 2] : [index + 1]) {
        const projection = await live.waitFor(
          (f) =>
            f.kind === 'subscription.session_projection' &&
            f.snapshot.interactions.pending.some(
              (p) =>
                p.turnId === turnId && !snapshots.some((s) => s.interactionId === p.interactionId),
            ),
        );
        const pending = projection.snapshot.interactions.pending.find(
          (p) => p.turnId === turnId && !snapshots.some((s) => s.interactionId === p.interactionId),
        );
        assert.equal(projection.snapshot.rootTurn.status, 'waiting_for_user');
        assert.equal(
          (await request('turn.query', { sessionId, turnId })).status,
          'waiting_for_user',
        );
        assert.deepEqual(await query(pending.interactionId), pending);
        const wire = frames.at(-1);
        assert.equal(
          new Set([pending.request.toolUseId, pending.interactionId, wire.interactionId]).size,
          3,
        );
        await checkpoint('pending-' + ordinal, { pending, wire });
        if (index <= 2) {
          if (ordinal === 1) {
            for (const invalid of [
              { kind: 'client_capability', decision: 'allow' },
              { kind: 'form', action: 'accept', values: {} },
              { kind: 'form', action: 'accept', values: { count: 4 } },
            ]) {
              await assert.rejects(
                answer(pending.interactionId, invalid),
                (e) => e.code === 'operation_conflict',
              );
              assert.deepEqual(await query(pending.interactionId), pending);
            }
            await checkpoint('invalid', pending);
          }
          const value = { kind: 'form', ...results[ordinal - 1] };
          const resolved = await answer(pending.interactionId, value);
          assert.equal(resolved.status, 'answered');
          assert.equal(resolved.outcome.kind, 'form_answer');
          assert.equal(resolved.outcome.action, value.action);
          assert(Number.isSafeInteger(resolved.outcome.committedAt));
          assert.deepEqual(await answer(pending.interactionId, value), resolved);
          await assert.rejects(
            answer(pending.interactionId, {
              kind: 'form',
              action: value.action === 'cancel' ? 'decline' : 'cancel',
            }),
            (e) => e.code === 'already_resolved',
          );
          snapshots.push(resolved);
        } else {
          snapshots.push(pending);
          await writeFile(saved, JSON.stringify(snapshots));
          if (index === 3) await request('turn.stop', { sessionId, turnId, runId: pending.runId });
          else await provider.close();
          // T1 without T2 drains the root; readonly SQL verifies closure now,
          // and the original client queries it again after Host recovery.
          await checkpoint('final', snapshots);
          assert.equal(providerCalls, disconnected ? 4 : 3);
          assert.equal(returned, disconnected ? 0 : 2);
          assert.equal(modelCalls, disconnected ? 1 : 5);
          assert.equal(searches, disconnected ? 1 : 3);
          if (failure) throw failure;
          verifiedRootDrain = true;
          console.log('original-client-forms');
          return;
        }
      }
      await live.waitFor(
        (f) =>
          f.kind === 'subscription.session_projection' &&
          f.snapshot.rootTurn?.turnId === turnId &&
          ['completed', 'cancelled', 'failed'].includes(f.snapshot.rootTurn.status),
      );
      assert.equal((await request('turn.query', { sessionId, turnId })).status, 'completed');
      if (failure) throw failure;
    }
  } finally {
    server.closeAllConnections();
    const cleanup = await Promise.allSettled([
      live?.close(),
      provider?.close(),
      new Promise((resolve) => server.close(resolve)),
    ]);
    for (const result of cleanup)
      if (
        result.status === 'rejected' &&
        !/connection closed|transport.*(closed|ended)/i.test(result.reason.message) &&
        // Verified T1-without-T2 shutdown may reject close before disconnecting,
        // or disconnect before its reply. Neither outcome reopens the subscription.
        // Timeouts and all unrelated operation failures remain test failures.
        !(
          verifiedRootDrain &&
          result.reason.operation === 'subscription.close' &&
          ((result.reason instanceof RuntimeHostRequestInterruptedError &&
            result.reason.reason === 'connection_lost') ||
            (result.reason instanceof RuntimeHostOperationError &&
              result.reason.code === 'host_draining'))
        )
      )
        throw result.reason;
  }
}

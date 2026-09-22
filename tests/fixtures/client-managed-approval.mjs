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
import { access, readFile, rename, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { watchSession } from './client-subscription.mjs';
import { discover, events } from './client-capability-model-fixture.mjs';

const sessionId = 'managed-approval';
export async function verifyManagedApproval(connection, workspace, reopened, openConnection) {
  const request = (op, input) => connection.request(op, input, 3000);
  const saved = join(workspace, 'interactions.json');
  const query = (interactionId) => request('interaction.query', { sessionId, interactionId });
  const answer = (interactionId, decision) =>
    request('interaction.answer', {
      sessionId,
      interactionId,
      answer: { kind: 'client_capability', decision },
    });
  if (reopened) {
    for (const snapshot of JSON.parse(await readFile(saved, 'utf8'))) {
      assert.deepEqual(await query(snapshot.interactionId), snapshot);
      if (snapshot.status === 'answered')
        assert.deepEqual(await answer(snapshot.interactionId, snapshot.outcome.decision), snapshot);
      else
        await assert.rejects(
          answer(snapshot.interactionId, 'allow'),
          (e) => e.code === 'already_resolved',
        );
    }
    console.log('original-client-managed-approval-reopened');
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
  let requests = 0,
    searches = 0,
    received = 0,
    effects = 0,
    failure;
  const snapshots = [];
  const server = createServer(async (req, res) => {
    try {
      assert.equal(req.url, '/v1/responses');
      let raw = '';
      for await (const chunk of req) raw += chunk;
      const input = JSON.parse(raw);
      const search = discover(input, ['mcp__desktop_browser__browser_navigate']);
      if (search) {
        searches++;
        res.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
        res.end(
          events(search, requests + searches)
            .map((event) => 'data: ' + JSON.stringify(event) + '\n\n')
            .join(''),
        );
        return;
      }
      const action = [
        [1, true],
        [1, false],
        [2, true],
        [2, false],
        [3, true],
        [3, false],
        [4, true],
        [5, true],
        [5, false],
      ][requests++];
      assert(action, 'unexpected model request');
      const [index, tool] = action;
      assert(input.tools.some((t) => t.name === 'mcp__desktop_browser__browser_navigate'));
      if (!tool && index <= 2) {
        const output = input.input.at(-1);
        assert.equal(output.type, 'function_call_output');
        assert(
          output.output.some(
            (p) => p.type === 'input_text' && p.text === 'approved effect ' + index,
          ),
        );
      }
      res.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
      res.end(
        events(
          tool ? { tool: true, name: 'mcp__desktop_browser__browser_navigate', index } : {},
          requests + searches,
        )
          .map((event) => 'data: ' + JSON.stringify(event) + '\n\n')
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
  let live, provider;
  try {
    const baseUrl = 'http://127.0.0.1:' + server.address().port + '/v1';
    const created = await request('connection.catalog.create', {
      expectedCatalogRevision: 0,
      connection: {
        slug: 'approval-fixture',
        name: 'Approval fixture',
        providerType: 'openai',
        baseUrl,
        enabled: true,
        enabledModelIds: ['gpt-5.2'],
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
            slug: 'approval-fixture',
            providerType: 'openai',
            effectiveBaseUrl: baseUrl,
          },
          secret: 'dummy-approval-fixture',
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
      sandboxMode: 'workspace-write',
    });
    provider = await openConnection();
    await provider.replaceClientCapabilities(
      {
        offers: () => [
          {
            offerId: 'desktop_browser',
            version: '1',
            affinity: 'session',
            hostPathAccess: 'none',
            label: 'Browser',
            tools: [
              {
                serverId: 'desktop_browser',
                name: 'browser_navigate',
                inputSchema: { type: 'object' },
              },
            ],
          },
        ],
        async call(frame, { accept }) {
          const index = ++received;
          try {
            assert.equal(frame.arguments.index, index);
            const origin =
              index <= 2 ? 'https://allowed.example' : 'https://other-' + index + '.example';
            await accept({ kind: 'browser_url', url: origin + '/evidence/path' });
            assert(index <= 2, 'denied/stopped provider must never receive admission');
            await checkpoint('dispatch-' + index, frame);
            await writeFile(join(workspace, 'effect-' + index), String(index), { flag: 'wx' });
            effects++;
            return { content: [{ type: 'text', text: 'approved effect ' + index }] };
          } catch (error) {
            if (index <= 2 || error.code === 'ERR_ASSERTION') failure = error;
            throw error;
          }
        },
      },
      3000,
    );
    live = await watchSession(connection, sessionId);
    for (let index = 1; index <= 5; index++) {
      const turnId = 'approval-' + index;
      await request('turn.start', { sessionId, turnId, content: { text: turnId }, maxSteps: 3 });
      if (index !== 2) {
        const projection = await live.waitFor(
          (frame) =>
            frame.kind === 'subscription.session_projection' &&
            frame.snapshot.interactions.pending.some((p) => p.turnId === turnId),
        );
        const pending = projection.snapshot.interactions.pending.find((p) => p.turnId === turnId);
        assert.equal(received, index, 'provider accepted evidence before prompting');
        assert.equal(effects, Math.min(index - 1, 2));
        assert.equal(pending.status, 'pending');
        assert.equal(projection.snapshot.rootTurn.status, 'waiting_for_user');
        assert.equal(
          (await request('turn.query', { sessionId, turnId })).status,
          'waiting_for_user',
        );
        assert.equal(
          pending.request.target.scope.origin,
          index === 1 ? 'https://allowed.example' : 'https://other-' + index + '.example',
        );
        assert.deepEqual(await query(pending.interactionId), pending);
        await assert.rejects(
          request('interaction.query', {
            sessionId: 'wrong-session',
            interactionId: pending.interactionId,
          }),
          (e) => e.code === 'not_found',
        );
        await checkpoint('pending-' + index, pending);
        if (index >= 4) {
          if (index === 4) await request('turn.stop', { sessionId, turnId, runId: pending.runId });
          else {
            await provider.close();
            await live.waitFor(
              (frame) =>
                frame.kind === 'subscription.session_projection' &&
                frame.sequence > projection.sequence &&
                frame.snapshot.rootTurn?.turnId === turnId &&
                frame.snapshot.interactions.pending.length === 0,
            );
          }
          const closed = await query(pending.interactionId);
          assert.equal(closed.status, 'closed', `${turnId}: canonical closure after withdrawal`);
          assert.equal(closed.outcome.kind, 'closure');
          assert.equal(
            closed.outcome.reason,
            index === 4 ? 'turn_stopped' : 'provider_disconnected',
          );
          await assert.rejects(
            answer(pending.interactionId, 'allow'),
            (e) => e.code === 'already_resolved',
          );
          snapshots.push(closed);
        } else {
          const decision = index === 1 ? 'allow' : 'deny';
          const resolved = await answer(pending.interactionId, decision);
          assert.equal(resolved.status, 'answered');
          assert.equal(resolved.outcome.decision, decision);
          assert(Number.isSafeInteger(resolved.outcome.committedAt));
          assert.deepEqual(await answer(pending.interactionId, decision), resolved);
          await assert.rejects(
            answer(pending.interactionId, decision === 'allow' ? 'deny' : 'allow'),
            (e) => e.code === 'already_resolved',
          );
          snapshots.push(resolved);
        }
      }
      await live.waitFor(
        (frame) =>
          frame.kind === 'subscription.session_projection' &&
          frame.snapshot.rootTurn?.turnId === turnId &&
          ['completed', 'cancelled', 'failed'].includes(frame.snapshot.rootTurn.status),
      );
      if (failure) throw failure;
      if (index <= 2)
        assert.equal((await request('turn.query', { sessionId, turnId })).status, 'completed');
    }
    assert.equal(effects, 2);
    assert.equal(received, 5);
    assert.equal(requests, 9);
    assert.equal(searches, 5);
    assert(
      !live.frames.some(
        (f) =>
          f.kind === 'subscription.session_projection' &&
          f.snapshot.interactions.pending.some((p) => p.turnId === 'approval-2'),
      ),
      'same-origin grant must skip prompt',
    );
    await writeFile(saved, JSON.stringify(snapshots));
    await checkpoint('final', snapshots);
    console.log('original-client-managed-approval');
  } finally {
    server.closeAllConnections();
    const cleanup = await Promise.allSettled([
      live?.close(),
      provider?.close(),
      new Promise((resolve) => server.close(resolve)),
    ]);
    for (const result of cleanup) if (result.status === 'rejected') throw result.reason;
  }
}

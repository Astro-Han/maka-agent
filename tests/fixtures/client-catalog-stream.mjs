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
import { setTimeout as delay } from 'node:timers/promises';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { decodeStoredMessage } from '../../packages/core/src/session.ts';
import { watchSession } from './client-subscription.mjs';

const provider = () => ({
  offers: () => [
    {
      offerId: 'catalog-control',
      version: '1',
      affinity: 'session',
      hostPathAccess: 'none',
      label: 'Catalog control probe',
      tools: [{ serverId: 'catalog-control', name: 'unused', inputSchema: { type: 'object' } }],
    },
  ],
  call: () => {
    throw new Error('Text-only model must not invoke a client tool');
  },
});

export async function readCatalog(connection) {
  let page = await connection.request('connection.catalog.query', { kind: 'start' }, 5000);
  const { revision } = page;
  const items = [...page.items];
  let pages = 1;
  while (page.nextCursor) {
    page = await connection.request(
      'connection.catalog.query',
      { kind: 'continue', revision, cursor: page.nextCursor },
      5000,
    );
    assert.equal(page.kind, 'page', 'unchanged catalog must keep its pagination revision');
    assert.equal(page.revision, revision);
    items.push(...page.items);
    pages++;
  }
  return { revision, items, pages };
}

// Remains open until cancellation; no natural completion can masquerade as Stop.
export function continuousModel() {
  let requests = 0,
    sent = 0;
  const closed = Promise.withResolvers();
  return {
    get requests() {
      return requests;
    },
    get sent() {
      return sent;
    },
    closed: closed.promise,
    async respond(req, res) {
      requests++;
      assert.equal(requests, 1, 'control traffic must not replay model requests');
      let body = '';
      for await (const chunk of req) {
        body += chunk;
        assert(Buffer.byteLength(body) < 256 * 1024);
      }
      const input = JSON.parse(body);
      assert.equal(input.model, 'one');
      assert.equal(input.stream, true);
      res.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
      let writable = true;
      res.on('drain', () => {
        writable = true;
      });
      const timer = setInterval(() => {
        if (!writable) return;
        sent++;
        writable = res.write(
          'data: ' +
            JSON.stringify({
              id: 'catalog-stream',
              object: 'chat.completion.chunk',
              created: 1,
              model: 'one',
              choices: [
                { index: 0, delta: { content: `chunk-${sent} 😀\n` }, finish_reason: null },
              ],
            }) +
            '\n\n',
        );
      }, 5);
      res.once('close', () => {
        clearInterval(timer);
        closed.resolve();
      });
    },
  };
}

export async function verifyCatalogStream(connection, model, workspace, reopened) {
  const sessionId = 'onboarded',
    turnId = 'catalog-stream';
  const snapshot = join(workspace, 'cancelled-stream.json');
  const call = (op, input) => connection.request(op, input, 5000);
  const observer = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 128 });
  try {
    if (!reopened) {
      const started = await call('turn.start', {
        sessionId,
        turnId,
        content: { text: 'Keep streaming until cancelled; do not use tools.' },
        maxSteps: 1,
      });
      const runId = started.turn.runId;
      await observer.waitFor((frame) => frame.kind === 'subscription.session_delta');
      const baseline = await readCatalog(connection);
      assert(baseline.pages > 8, 'inventory must exercise multiple bounded pages');
      assert.equal(baseline.items.filter((item) => item.kind === 'model').length, 890);
      const registrations = new Set();
      for (let round = 0; round < 6; round++) {
        const before = new Set(observer.frames);
        const [left, right, registration] = await Promise.all([
          readCatalog(connection),
          readCatalog(connection),
          // Use the unchanged client's default 2s capability deadline.
          connection.replaceClientCapabilities(provider()),
        ]);
        assert.deepEqual(left, baseline);
        assert.deepEqual(right, baseline);
        assert(!registrations.has(registration.registrationId));
        registrations.add(registration.registrationId);
        await observer.waitFor(
          (frame) => !before.has(frame) && frame.kind === 'subscription.session_delta',
        );
        assert.equal((await call('turn.query', { sessionId, turnId })).status, 'running');
      }
      // Streaming duration is an observed condition, not the speed of catalog RPCs.
      const streamDeadline = Date.now() + 3000;
      let received = '',
        consumed = 0;
      while (!received.includes('chunk-100 😀\n')) {
        for (const frame of observer.frames.slice(consumed)) {
          if (frame.kind === 'subscription.session_delta' && frame.delta.kind === 'text') {
            received += frame.delta.text;
          }
        }
        consumed = observer.frames.length;
        assert(Date.now() < streamDeadline, 'Sustained model text was not delivered');
        if (!received.includes('chunk-100 😀\n')) await delay(5);
      }
      const [catalog, stopped] = await Promise.all([
        readCatalog(connection),
        call('turn.stop', { sessionId, turnId, runId }),
        connection.replaceClientCapabilities(provider()),
      ]);
      assert.deepEqual(catalog, baseline);
      assert.equal(stopped.runId, runId);
      const deadline = Date.now() + 5000;
      let terminal;
      do {
        terminal = await call('turn.query', { sessionId, turnId });
        if (terminal.status === 'cancelled') break;
        assert(Date.now() < deadline, 'Stop did not complete cleanup');
        await delay(10);
      } while (true);
      assert.equal(terminal.abortSource, 'runtime_cancellation');
      await observer.terminal(terminal);
      const timer = new AbortController();
      try {
        await Promise.race([
          model.closed,
          delay(5000, undefined, { signal: timer.signal }).then(() => {
            throw new Error('Cancelled model retained its HTTP stream');
          }),
        ]);
      } finally {
        timer.abort();
      }
      assert.equal(model.requests, 1);
      assert(model.sent >= 100, 'control exercise must span sustained streaming');
      await connection.unregisterClientCapabilities();
    }
  } finally {
    await observer.close();
  }
  // A subscription's transcript is fenced at open; refresh after the terminal commit.
  const settled = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 128 });
  try {
    assert.equal((await call('turn.query', { sessionId, turnId })).status, 'cancelled');
    const rows = await settled.subscription.loadTranscript(decodeStoredMessage);
    const assistant = rows.find((row) => row.type === 'assistant' && row.turnId === turnId);
    assert(assistant?.text.startsWith('chunk-1 😀\n'));
    assert(assistant.text.includes('chunk-100 😀\n'));
    if (reopened) assert.deepEqual(rows, JSON.parse(await readFile(snapshot, 'utf8')));
    else await writeFile(snapshot, JSON.stringify(rows));
  } finally {
    await settled.close();
  }
}

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
import { verifyRequestHeaders } from './client-request-headers.mjs';
import { verifyConnectionProtocols } from './client-connection-protocols.mjs';
import { withNetworkProxy } from './client-network-proxy.mjs';
const secret = 'dummy-connection-test-secret';
const basis = ({ connectionId, revision }) => ({ connectionId, revision });
const models = {
  openai: 'gpt-5',
  'openai-compatible': 'fixture-chat',
  anthropic: 'fixture-claude',
};

async function fixture() {
  let count = 0;
  let failure;
  let pending;
  let auth = false;
  let expected;
  let expectedHeader;
  const server = createServer(async (request, response) => {
    count++;
    try {
      assert.equal(request.headers['x-maka-retained'], expectedHeader);
      if (expected.wire === 'anthropic-messages') {
        assert.equal(request.headers['x-api-key'], secret);
        assert.equal(request.headers['anthropic-version'], '2023-06-01');
        assert.equal(request.headers.authorization, undefined);
      } else {
        assert.equal(request.headers.authorization, `Bearer ${secret}`);
        assert.equal(request.headers['x-api-key'], undefined);
      }
      if (request.method === 'GET') {
        assert.equal(request.url, '/v1/models');
        response.writeHead(200, { 'Content-Type': 'application/json', Connection: 'close' });
        response.end(
          JSON.stringify({
            data: ['fixture-listed', 'fixture-extra'].map((id) => ({
              id,
              object: 'model',
              created: 1,
              owned_by: 'fixture',
            })),
          }),
        );
        return;
      }
      assert.equal(request.headers['content-type'], 'application/json');
      let body = '';
      for await (const chunk of request) body += chunk;
      const responses = expected.wire === 'openai-responses';
      assert.equal(request.method, 'POST');
      assert.equal(
        request.url,
        responses
          ? '/v1/responses'
          : expected.wire === 'anthropic-messages'
            ? '/v1/messages'
            : '/v1/chat/completions',
      );
      assert.deepEqual(JSON.parse(body), {
        model: expected.model,
        ...(responses
          ? {
              store: false,
              max_output_tokens: 16,
              input: [{ role: 'user', content: 'Hi' }],
            }
          : { max_tokens: 16, messages: [{ role: 'user', content: 'Hi' }] }),
        ...expected.overlay,
      });
      const complete = () => {
        response.writeHead(auth ? 401 : 200, {
          'Content-Type': 'application/json',
          Connection: 'close',
        });
        response.end(auth ? JSON.stringify({ error: secret }) : '{}');
      };
      if (pending) {
        const arrived = pending;
        pending = undefined;
        arrived(complete);
      } else complete();
    } catch (error) {
      failure = error;
      response.destroy(error);
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  return {
    baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
    expect(providerType, model, overlay = {}, wire) {
      expected = {
        model,
        overlay,
        wire:
          wire ??
          (providerType === 'openai'
            ? 'openai-responses'
            : providerType === 'anthropic'
              ? 'anthropic-messages'
              : 'openai-chat'),
      };
    },
    headers(value) {
      expectedHeader = value;
    },
    get count() {
      if (failure) throw failure;
      return count;
    },
    auth(value) {
      auth = value;
    },
    hold() {
      return new Promise((arrived) => {
        pending = arrived;
      });
    },
    async close() {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
      if (failure) throw failure;
    },
  };
}

export async function verifyConnectionTest(connection, workspace, reopened, openClient) {
  const request = (operation, input) => connection.request(operation, input, 3000);
  const catalog = () => request('connection.catalog.query', { kind: 'start' });
  const path = join(workspace, 'connection-test.json');
  if (reopened) {
    assert.equal(JSON.stringify(await catalog()), await readFile(path, 'utf8'));
    const id = (await catalog()).items.find(
      (item) => item.kind === 'connection' && item.slug === 'openai-compatible',
    ).connectionId;
    assert.deepEqual(await request('connection.request-headers.query', { connectionId: id }), {
      kind: 'found',
      names: ['X-Maka-Retained'],
    });
    console.log('original-client-connection-test-reopened');
    return;
  }
  const provider = await fixture();
  const peer = await openClient();
  const other = (operation, input) => peer.request(operation, input, 3000);
  const header = (page, id) => {
    const row = page.items.find((item) => item.kind === 'connection' && item.connectionId === id);
    assert(row);
    return row;
  };
  const run = (connectionId, modelId = null) =>
    request('connection.test.run', { connectionId, modelId });
  const update = (page, id, changes) =>
    other('connection.catalog.update', {
      expected: basis(header(page, id)),
      changes: {
        name: header(page, id).name,
        baseUrl: provider.baseUrl,
        enabled: true,
        enabledModelIds: [models['openai-compatible']],
        ...changes,
      },
    });
  const verified = (result, modelId) => {
    assert.equal(result.kind, 'committed');
    assert.equal(result.test.kind, 'verified');
    assert.equal(result.test.modelId, modelId);
    assert(Number.isFinite(Date.parse(result.test.checkedAt)));
    assert(result.test.latencyMs >= 0);
  };
  try {
    const ids = {};
    for (const [providerType, model] of Object.entries(models)) {
      const created = await request('connection.catalog.create', {
        expectedCatalogRevision: (await catalog()).revision,
        connection: {
          slug: providerType,
          name: providerType,
          providerType,
          baseUrl: provider.baseUrl,
          enabled: true,
          enabledModelIds: [model],
        },
      });
      assert.equal(created.kind, 'committed');
      const id = created.connection.connectionId;
      ids[providerType] = id;
      const noKey = await catalog();
      const count = provider.count;
      assert.deepEqual(await run(id), { kind: 'rejected', reason: 'credential_not_configured' });
      assert.equal(provider.count, count);
      assert.deepEqual(await catalog(), noKey);
      const row = header(noKey, id);
      assert.equal(
        (
          await request('credential.vault.set', {
            locator: { scope: 'connection', connectionId: id, kind: 'api_key' },
            expected: null,
            expectedConnection: {
              ...basis(row),
              slug: row.slug,
              providerType,
              effectiveBaseUrl: provider.baseUrl,
            },
            secret,
          })
        ).kind,
        'committed',
      );
      provider.expect(providerType, model);
      verified(await run(id, model), model);
      const after = await catalog();
      assert.deepEqual(header(after, id).lastTest, {
        status: 'verified',
        checkedAt: header(after, id).lastTest.checkedAt,
      });
      assert.equal(after.defaultTarget, null);
    }
    const id = ids['openai-compatible'];
    const model = models['openai-compatible'];
    // Auto-selection must use this connection even when another is the default.
    assert.equal(
      (
        await other('connection.catalog.set-default-target', {
          expectedCatalogRevision: (await catalog()).revision,
          target: { connectionId: ids.anthropic, modelId: models.anthropic },
        })
      ).kind,
      'committed',
    );
    provider.expect('openai-compatible', model);
    verified(await run(id), model);
    assert.equal(
      (await request('connection.models.fetch', { connectionId: id })).kind,
      'committed',
    );
    assert.equal(
      (
        await update(await catalog(), id, {
          enabledModelIds: ['fixture-unlisted', 'fixture-listed'],
        })
      ).kind,
      'committed',
    );
    provider.expect('openai-compatible', 'fixture-listed');
    verified(await run(id), 'fixture-listed');
    // An explicit inventory model need not be enabled to be tested.
    provider.expect('openai-compatible', 'fixture-extra');
    verified(await run(id, 'fixture-extra'), 'fixture-extra');
    assert.equal((await update(await catalog(), id, {})).kind, 'committed');
    provider.expect('openai-compatible', model);
    let arrived = provider.hold();
    let testing = run(id);
    let release = await arrived;
    // A held network RPC must not block liveness or ordinary requests on this
    // same unchanged client connection. No timer releases the fixture for us.
    assert.equal((await connection.status()).state, 'ready');
    assert.equal(
      header(await catalog(), id).name,
      header(await other('connection.catalog.query', { kind: 'start' }), id).name,
    );
    const during = await other('connection.catalog.query', { kind: 'start' });
    assert.equal((await update(during, id, { name: 'Renamed during probe' })).kind, 'committed');
    release();
    verified(await testing, model);
    const renamed = await catalog();
    assert.equal(header(renamed, id).name, 'Renamed during probe');
    assert.deepEqual(renamed.defaultTarget, {
      connectionId: ids.anthropic,
      modelId: models.anthropic,
    });
    arrived = provider.hold();
    testing = run(id);
    release = await arrived;
    const overlay = { temperature: 0.25 };
    assert.equal((await update(renamed, id, { requestBodyOverlay: overlay })).kind, 'committed');
    const changed = await other('connection.catalog.query', { kind: 'start' });
    release();
    assert.deepEqual(await testing, { kind: 'superseded', changed: ['connection'] });
    assert.deepEqual(await catalog(), changed);
    provider.expect('openai-compatible', model, overlay);
    verified(await run(id), model);
    provider.auth(true);
    const failed = await run(id);
    assert.equal(failed.kind, 'committed');
    assert.equal(failed.test.kind, 'failed');
    assert.equal(failed.test.errorClass, 'auth');
    assert.equal(failed.test.modelId, null);
    assert.equal(failed.test.statusCode, 401);
    assert(failed.test.latencyMs >= 0);
    const saved = await catalog();
    assert.equal(saved.nextCursor, null);
    assert.deepEqual(header(saved, id).lastTest, {
      status: 'needs_reauth',
      checkedAt: failed.test.checkedAt,
      errorClass: 'auth',
    });
    assert.equal(provider.count, 11);
    assert(!JSON.stringify({ saved, failed }).includes(secret), 'public projection leaks secret');
    await verifyRequestHeaders(connection, peer, provider, id, model, overlay);
    await withNetworkProxy(connection, provider.baseUrl, () =>
      verifyConnectionProtocols(connection, provider, secret),
    );
    await writeFile(path, JSON.stringify(await catalog()));
    console.log('original-client-connection-test');
  } finally {
    await peer.close();
    await peer.closed;
    await provider.close();
  }
}

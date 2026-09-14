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

const secret = 'dummy-model-fetch-secret';
const ids = ['fixture-alpha', 'fixture-beta'];
const basis = ({ connectionId, revision }) => ({ connectionId, revision });
const credentialBasis = ({ locator, credentialId, revision }) => ({
  locator,
  credentialId,
  revision,
});

async function fixture() {
  let count = 0;
  let failure;
  let pending;
  let mode = 'success';
  const server = createServer((request, response) => {
    count++;
    try {
      assert.equal(request.method, 'GET');
      assert.equal(request.url, '/v1/models');
      const anthropic = request.headers['x-api-key'] !== undefined;
      if (anthropic) {
        assert.equal(request.headers['x-api-key'], secret);
        assert.equal(request.headers['anthropic-version'], '2023-06-01');
      } else {
        assert.equal(request.headers.authorization, `Bearer ${secret}`);
      }
      const complete = () => {
        response.writeHead(mode === 'auth' ? 401 : 200, {
          'Content-Type': 'application/json',
          Connection: 'close',
        });
        response.end(
          JSON.stringify(
            mode === 'auth'
              ? { error: secret }
              : {
                  data: ids.map((id) =>
                    anthropic
                      ? { id, type: 'model', display_name: id, created_at: '2025-01-01T00:00:00Z' }
                      : { id, object: 'model', created: 1, owned_by: 'fixture' },
                  ),
                  has_more: false,
                },
          ),
        );
      };
      if (pending) {
        const hold = pending;
        pending = undefined;
        hold.arrived(complete);
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
    get count() {
      if (failure) throw failure;
      return count;
    },
    auth(value) {
      mode = value ? 'auth' : 'success';
    },
    hold() {
      return new Promise((arrived) => {
        pending = { arrived };
      });
    },
    async close() {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
      if (failure) throw failure;
    },
  };
}

export async function verifyModelFetch(connection, workspace, reopened, openClient) {
  const request = (operation, input) => connection.request(operation, input, 3000);
  const catalog = () => request('connection.catalog.query', { kind: 'start' });
  const path = join(workspace, 'model-fetch.json');
  if (reopened) {
    assert.equal(JSON.stringify(await catalog()), await readFile(path, 'utf8'));
    console.log('original-client-model-fetch-reopened');
    return;
  }
  const provider = await fixture();
  const peer = await openClient();
  const other = (operation, input) => peer.request(operation, input, 3000);
  const rows = (page, id, kind) => {
    const header = page.items.find(
      (item) => item.kind === 'connection' && item.connectionId === id,
    );
    assert(header);
    return page.items.filter(
      (item) => item.connectionIndex === header.connectionIndex && item.kind === kind,
    );
  };
  const header = (page, id) => rows(page, id, 'connection')[0];
  const put = (call, item) =>
    call('credential.vault.set', {
      locator: { scope: 'connection', connectionId: item.connectionId, kind: 'api_key' },
      expected: null,
      expectedConnection: {
        ...basis(item),
        slug: item.slug,
        providerType: item.providerType,
        effectiveBaseUrl: provider.baseUrl,
      },
      secret,
    });
  const fetch = (id) => request('connection.models.fetch', { connectionId: id });
  try {
    const connections = [];
    for (const providerType of ['openai', 'openai-compatible', 'anthropic']) {
      const before = await catalog();
      const created = await request('connection.catalog.create', {
        expectedCatalogRevision: before.revision,
        connection: {
          slug: providerType,
          name: providerType,
          providerType,
          baseUrl: provider.baseUrl,
          enabled: true,
          enabledModelIds: [],
        },
      });
      assert.equal(created.kind, 'committed');
      const id = created.connection.connectionId;
      const noKey = await catalog();
      const count = provider.count;
      assert.deepEqual(await fetch(id), { kind: 'rejected', reason: 'credential_not_configured' });
      assert.equal(provider.count, count);
      assert.deepEqual(await catalog(), noKey);
      const credential = await put(request, header(noKey, id));
      assert.equal(credential.kind, 'committed');
      const fetched = await fetch(id);
      assert.equal(fetched.kind, 'committed');
      assert.equal(fetched.source, 'fetched');
      assert.equal(fetched.modelCount, 2);
      assert(fetched.fetchedAt > 0);
      const after = await catalog();
      assert.equal(after.nextCursor, null);
      assert.deepEqual(
        rows(after, id, 'model')
          .map((item) => item.model.id)
          .sort(),
        ids,
      );
      assert.deepEqual(
        rows(after, id, 'enabled_model_id').map((item) => item.modelId),
        [ids[0]],
      );
      assert.equal(header(after, id).modelSource, 'fetched');
      assert.equal(after.defaultTarget, null);
      connections.push(id);
    }
    const id = connections[0];
    const beforeFailure = await catalog();
    provider.auth(true);
    assert.deepEqual(await fetch(id), { kind: 'failed', errorClass: 'auth' });
    provider.auth(false);
    assert.deepEqual(await catalog(), beforeFailure);

    // A display-only concurrent edit must survive a successful discovery commit.
    let arrived = provider.hold();
    let fetching = fetch(id);
    let release = await arrived;
    const rename = await other('connection.catalog.update', {
      expected: basis(header(beforeFailure, id)),
      changes: {
        name: 'Renamed during discovery',
        baseUrl: provider.baseUrl,
        enabled: true,
        enabledModelIds: [ids[0]],
      },
    });
    assert.equal(rename.kind, 'committed');
    const target = { connectionId: connections[2], modelId: ids[0] };
    assert.equal(
      (
        await other('connection.catalog.set-default-target', {
          expectedCatalogRevision: rename.catalogRevision,
          target,
        })
      ).kind,
      'committed',
    );
    release();
    assert.equal((await fetching).kind, 'committed');
    const renamed = await catalog();
    assert.equal(header(renamed, id).name, 'Renamed during discovery');
    assert.deepEqual(
      rows(renamed, id, 'enabled_model_id').map((item) => item.modelId),
      [ids[0]],
    );
    assert.deepEqual(renamed.defaultTarget, target);

    // Delete/recreate restores revision 1 and the same bytes but changes identity.
    arrived = provider.hold();
    fetching = fetch(id);
    release = await arrived;
    const locator = { scope: 'connection', connectionId: id, kind: 'api_key' };
    const previous = await other('credential.vault.query', { locator });
    assert.equal(previous.kind, 'status');
    assert.equal(previous.status.revision, 1);
    assert.equal(
      (
        await other('credential.vault.delete', {
          expected: credentialBasis(previous.status),
        })
      ).kind,
      'committed',
    );
    const replacement = await put(other, header(renamed, id));
    assert.equal(replacement.kind, 'committed');
    assert.equal(replacement.status.revision, 1);
    assert.notEqual(replacement.status.credentialId, previous.status.credentialId);
    const beforeSuperseded = await other('connection.catalog.query', { kind: 'start' });
    release();
    assert.deepEqual(await fetching, { kind: 'superseded', changed: ['credential'] });
    assert.deepEqual(await catalog(), beforeSuperseded);
    assert.equal(provider.count, 6);
    const saved = JSON.stringify(await catalog());
    for (const output of [saved, JSON.stringify(previous), JSON.stringify(replacement)]) {
      assert(!output.includes(secret), 'public projection leaks credential secret');
    }
    await writeFile(path, saved);
    console.log('original-client-model-fetch');
  } finally {
    await peer.close();
    await peer.closed;
    await provider.close();
  }
}

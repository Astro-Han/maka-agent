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

// Revocation closes the peer now; it must not drop the Host's admitted effect.
export async function verifyRemoteDrain(local, issue, ready, bounded) {
  const request = (operation, input) => local.request(operation, input, 3000);
  const arrived = Promise.withResolvers();
  let release;
  let requests = 0;
  const server = createServer(async (incoming, response) => {
    requests++;
    incoming.resume();
    await once(incoming, 'end');
    release = () => {
      if (!response.writableEnded) response.writeHead(200, { Connection: 'close' }).end('{}');
    };
    arrived.resolve();
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  let unsubscribe;
  try {
    const baseUrl = `http://127.0.0.1:${server.address().port}/v1`;
    const created = await request('connection.catalog.create', {
      expectedCatalogRevision: (await request('connection.catalog.query', { kind: 'start' }))
        .revision,
      connection: {
        slug: 'revocation-drain',
        name: 'Revocation drain',
        providerType: 'openai-compatible',
        baseUrl,
        enabled: true,
        enabledModelIds: ['fixture-model'],
      },
    });
    assert.equal(created.kind, 'committed');
    const { connectionId, revision } = created.connection;
    const saved = await request('credential.vault.set', {
      locator: { scope: 'connection', connectionId, kind: 'api_key' },
      expected: null,
      expectedConnection: {
        connectionId,
        revision,
        slug: 'revocation-drain',
        providerType: 'openai-compatible',
        effectiveBaseUrl: baseUrl,
      },
      secret: 'dummy-remote-drain-key',
    });
    assert.equal(saved.kind, 'committed');
    const credential = await issue(['connection.test.run']);
    const remote = await ready(credential.bearer);
    const committed = Promise.withResolvers();
    // Attach immediately so fixture failures cannot create unhandled rejections.
    committed.promise.catch(() => {});
    unsubscribe = local.subscribeConfigurationChanges(() => {
      request('connection.catalog.query', { kind: 'start' }).then((catalog) => {
        const row = catalog.items.find(
          (row) => row.kind === 'connection' && row.connectionId === connectionId,
        );
        if (row?.lastTest?.status === 'verified') committed.resolve(row.lastTest);
      }, committed.reject);
    });
    const pending = remote
      .request(
        'connection.test.run',
        {
          connectionId,
          modelId: 'fixture-model',
        },
        3000,
      )
      .then(
        () => 'responded',
        () => 'disconnected',
      );
    await bounded(arrived.promise, 'admitted remote HTTP effect');
    assert.equal(
      (
        await request('access.credential.revoke', {
          credentialId: credential.credentialId,
        })
      ).revoked,
      true,
    );
    // This is deliberately BEFORE releasing the provider. Merely cancelling
    // the next transport read would leave the revoked peer connected here.
    await bounded(remote.closed, 'revocation during admitted request');
    assert.equal(await pending, 'disconnected');
    release();
    const test = await bounded(committed.promise, 'admitted effect commits after disconnect');
    assert.equal(test.status, 'verified');
    assert(Number.isFinite(Date.parse(test.checkedAt)));
    assert.equal(requests, 1, 'disconnected requests must neither disappear nor replay');
    assert.equal((await local.status(3000)).state, 'ready');
  } finally {
    unsubscribe?.();
    release?.();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
}

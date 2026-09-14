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
import { randomBytes } from 'node:crypto';
import { request as httpRequest } from 'node:http';
import { connectRemoteRuntimeHost } from '../../packages/runtime-host/src/client/connection.ts';
import { consumeAccessCredentialDeliveryFromControlDirectory as consume } from '../../packages/runtime-host/src/control/access-credential-delivery.ts';
import { verifyRemoteDrain } from './client-remote-drain.mjs';
import { verifyProviderAccess } from './client-provider-access.mjs';
import { verifyPairingAccess } from './client-pairing-access.mjs';

function bounded(promise, label) {
  let timer;
  return Promise.race([
    promise,
    new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`Timed out: ${label}`)), 3000);
    }),
  ]).finally(() => clearTimeout(timer));
}

// Return only status/body; Authorization must never enter diagnostic output.
function http(url, path, method = 'GET', headers = {}) {
  const endpoint = new URL(url);
  endpoint.protocol = 'http:';
  endpoint.pathname = '/';
  return new Promise((resolve, reject) => {
    const request = httpRequest(endpoint, { path, method, headers, agent: false });
    request.on('response', (response) => {
      let body = '';
      response.setEncoding('utf8');
      response.on('data', (chunk) => {
        body += chunk;
      });
      response.on('end', () => resolve({ status: response.statusCode, body }));
    });
    request.on('upgrade', (response, socket) => {
      socket.destroy();
      resolve({ status: response.statusCode, body: '' });
    });
    request.on('error', () => reject(new Error('HTTP acceptance request failed')));
    request.setTimeout(3000, () => request.destroy());
    request.end();
  });
}

export async function verifyRemoteAccess(local, handshake, url, control) {
  const request = (peer, operation, input) => peer.request(operation, input, 3000);
  const issueInput = {
    principalKind: 'remote_owner',
    principalId: 'remote-client-acceptance',
    operationGrants: [],
    canPublishClientCapabilities: false,
    canUseHostPaths: false,
  };
  const issue = async (operationGrants) => {
    const issued = await request(local, 'access.credential.issue', {
      ...issueInput,
      operationGrants,
    });
    const bearer = await consume(control, issued.deliveryId, issued.credentialId);
    assert(/^maka_rh_[A-Za-z0-9_-]{43}$/u.test(bearer), 'invalid private bearer format');
    return { credentialId: issued.credentialId, bearer };
  };
  const peers = [];
  const connect = async (credential, expectedRootId = handshake.expectedRootId) => {
    const result = await connectRemoteRuntimeHost({
      ...handshake,
      url,
      credential,
      expectedRootId,
      connectTimeoutMs: 3000,
    });
    if (result.kind === 'connected') peers.push(result.connection);
    return result;
  };
  const ready = async (bearer) => {
    const result = await connect(bearer);
    assert.equal(result.kind, 'connected', JSON.stringify(result));
    assert.equal((await result.connection.status(3000)).state, 'ready');
    return result.connection;
  };
  const unsubscribe = [];
  try {
    const restrictedCredential = await issue([]);
    const restricted = await ready(restrictedCredential.bearer);
    assert.deepEqual(await connect(restrictedCredential.bearer, randomBytes(32).toString('hex')), {
      kind: 'unavailable',
      reason: 'root_mismatch',
    });
    await assert.rejects(request(restricted, 'connection.catalog.query', { kind: 'start' }), {
      code: 'unauthorized',
    });
    await assert.rejects(request(restricted, 'access.credential.issue', issueInput), {
      code: 'unauthorized',
    });

    const grantedCredential = await issue([
      'connection.catalog.query',
      'session.create',
      'runtime.policy.query',
      'runtime.resource.start',
      'runtime.resource.stop',
      'runtime.resource.controller.acquire',
      'runtime.resource.controller.control',
      'runtime.resource.controller.release',
    ]);
    const granted = await ready(grantedCredential.bearer);
    for (const [operation, input] of [
      [
        'runtime.resource.start',
        { sessionId: 'missing', launchId: 'remote-launch', command: 'never executed' },
      ],
      [
        'runtime.resource.stop',
        { sessionId: 'missing', ref: 'maka://runtime/background-tasks/missing' },
      ],
      [
        'runtime.resource.controller.acquire',
        {
          sessionId: 'missing',
          ref: 'maka://runtime/background-tasks/missing',
          controllerId: 'remote-controller',
        },
      ],
      [
        'runtime.resource.controller.control',
        {
          sessionId: 'missing',
          ref: 'maka://runtime/background-tasks/missing',
          controllerId: 'remote-controller',
          sequence: 1,
          control: { kind: 'resize', cols: 80, rows: 24 },
        },
      ],
    ]) {
      // Existing Session resources select no new host path. A granted request
      // reaches Session lookup; an ungranted one is still rejected at transport.
      await assert.rejects(request(granted, operation, input), { code: 'not_found' });
      await assert.rejects(request(restricted, operation, input), { code: 'unauthorized' });
    }
    const catalog = await request(granted, 'connection.catalog.query', { kind: 'start' });
    const release = {
      sessionId: 'missing',
      ref: 'maka://runtime/background-tasks/missing',
      controllerId: 'remote-controller',
    };
    assert.deepEqual(await request(granted, 'runtime.resource.controller.release', release), {
      controllerId: 'remote-controller',
      released: false,
    });
    await assert.rejects(request(restricted, 'runtime.resource.controller.release', release), {
      code: 'unauthorized',
    });
    assert.deepEqual(catalog.items, []);
    const catalogOnlyCredential = await issue(['connection.catalog.query']);
    const catalogOnly = await ready(catalogOnlyCredential.bearer);
    await assert.rejects(
      request(granted, 'session.create', {
        sessionId: 'remote-host-path-denied',
        workspace: { kind: 'host_path', path: control },
        modelTarget: { kind: 'default' },
        name: 'Unauthorized host path',
      }),
      { code: 'unauthorized' },
    );

    for (const path of ['/healthz', '/healthz?probe=1', '/readyz', '/readyz?probe=1']) {
      assert.equal((await http(url, path)).status, 200);
    }
    assert.equal((await http(url, '/healthz', 'POST')).status, 405);
    const upgrade = {
      Connection: 'Upgrade',
      Upgrade: 'websocket',
      'Sec-WebSocket-Version': '13',
      'Sec-WebSocket-Key': 'dGhlIHNhbXBsZSBub25jZQ==',
      Authorization: `Bearer ${restrictedCredential.bearer}`,
    };
    for (const path of ['/wrong', '/runtime-host?probe=1']) {
      assert.equal((await http(url, path, 'GET', upgrade)).status, 404);
    }
    const { Authorization, ...withoutAuthorization } = upgrade;
    assert.equal((await http(url, '/runtime-host', 'GET', withoutAuthorization)).status, 401);
    for (const authorization of ['Bearer invalid', `bearer ${restrictedCredential.bearer}`]) {
      assert.equal(
        (
          await http(url, '/runtime-host', 'GET', {
            ...upgrade,
            Authorization: authorization,
          })
        ).status,
        401,
      );
    }
    assert.equal(
      (
        await http(url, '/runtime-host', 'GET', {
          ...upgrade,
          Origin: 'https://untrusted.invalid',
        })
      ).status,
      403,
    );
    assert.equal(
      (
        await http(url, '/runtime-host', 'GET', {
          ...upgrade,
          Origin: 'https://allowed.invalid',
        })
      ).status,
      101,
    );

    const restrictedCatalog = [];
    const restrictedConfiguration = [];
    const catalogOnlyConfiguration = [];
    unsubscribe.push(
      catalogOnly.subscribeConfigurationChanges((revision) =>
        catalogOnlyConfiguration.push(revision),
      ),
    );
    unsubscribe.push(
      restricted.subscribeConnectionCatalogChanges((revision) => restrictedCatalog.push(revision)),
    );
    unsubscribe.push(
      restricted.subscribeConfigurationChanges((revision) =>
        restrictedConfiguration.push(revision),
      ),
    );
    let changed;
    const change = new Promise((resolve) => {
      changed = resolve;
    });
    unsubscribe.push(granted.subscribeConfigurationChanges(changed));
    const mutation = await request(local, 'connection.catalog.create', {
      expectedCatalogRevision: catalog.revision,
      connection: {
        slug: 'remote-acceptance',
        name: 'Remote acceptance',
        providerType: 'openai-compatible',
        baseUrl: 'http://127.0.0.1:1/v1',
        enabled: true,
        enabledModelIds: ['fixture-model'],
      },
    });
    assert.equal(mutation.kind, 'committed');
    assert.equal(typeof (await bounded(change, 'authorized configuration change')), 'number');
    // The positive delivery and subsequent status response fence this mutation.
    await restricted.status(3000);
    await catalogOnly.status(3000);
    assert.deepEqual(restrictedCatalog, []);
    assert.deepEqual(restrictedConfiguration, []);
    assert.deepEqual(catalogOnlyConfiguration, []);

    assert.deepEqual(
      await request(local, 'access.credential.revoke', {
        credentialId: restrictedCredential.credentialId,
      }),
      { credentialId: restrictedCredential.credentialId, revoked: true },
    );
    await bounded(restricted.closed, 'revoked remote closure');
    assert.deepEqual(await connect(restrictedCredential.bearer), {
      kind: 'unavailable',
      reason: 'authentication_failed',
    });
    assert.equal((await granted.status(3000)).state, 'ready');
    assert.equal((await local.status(3000)).state, 'ready');
    await verifyPairingAccess(local, handshake, url, control, bounded);
    await verifyProviderAccess(local, handshake, url, control, bounded);
    await verifyRemoteDrain(local, issue, ready, bounded);
    console.log('original-client-remote-access');
  } finally {
    for (const stop of unsubscribe) stop();
    await Promise.all(peers.map((peer) => peer.close()));
  }
}

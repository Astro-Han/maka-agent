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
import { readdir, stat } from 'node:fs/promises';
import { join } from 'node:path';
import { connectRemoteRuntimeHost } from '../../packages/runtime-host/src/client/connection.ts';
import { consumeAccessCredentialDeliveryFromControlDirectory as consume } from '../../packages/runtime-host/src/control/access-credential-delivery.ts';

export async function verifyProviderAccess(local, handshake, url, control, bounded) {
  const request = (peer, operation, input) => peer.request(operation, input, 3000);
  const grants = ['client.capability.replace', 'client.capability.unregister'];
  const input = {
    principalKind: 'capability_provider',
    principalId: 'remote-provider-acceptance',
    operationGrants: grants,
    canPublishClientCapabilities: true,
    canUseHostPaths: false,
  };
  const deliveries = async () =>
    (await readdir(control))
      .filter((name) => name.startsWith('runtime-host-access-delivery-'))
      .sort();
  const credentials = new Set();
  const peers = [];
  const providers = [];
  const issue = async (value) => {
    const issued = await request(local, 'access.credential.issue', value);
    credentials.add(issued.credentialId);
    const path = join(control, `runtime-host-access-delivery-${issued.deliveryId}.json`);
    assert.equal((await stat(path)).mode & 0o777, 0o600);
    const bearer = await consume(control, issued.deliveryId, issued.credentialId);
    assert(/^maka_rh_[A-Za-z0-9_-]{43}$/u.test(bearer), 'invalid private bearer format');
    assert(!JSON.stringify(issued).includes(bearer), 'public issue result exposes bearer');
    await assert.rejects(consume(control, issued.deliveryId, issued.credentialId), {
      code: 'ENOENT',
    });
    return { issued, bearer };
  };
  const connect = async (credential) => {
    const result = await connectRemoteRuntimeHost({
      ...handshake,
      url,
      credential,
      connectTimeoutMs: 3000,
    });
    if (result.kind === 'connected') peers.push(result.connection);
    return result;
  };
  const offer = {
    offerId: 'provider-tool',
    version: '1',
    affinity: 'session',
    hostPathAccess: 'none',
    label: 'Provider acceptance tool',
    tools: [{ serverId: 'provider-acceptance', name: 'inspect', inputSchema: { type: 'object' } }],
  };
  const provider = (overrides = {}, services = []) => {
    let closeCount = 0;
    let resolveClosed;
    const closed = new Promise((resolve) => {
      resolveClosed = resolve;
    });
    const value = {
      offers: () => [{ ...offer, ...overrides }],
      services: () => services,
      call: async () => {
        throw new Error('Unexpected provider invocation');
      },
      close: () => {
        closeCount += 1;
        resolveClosed();
      },
    };
    const tracked = { value, closed, closeCount: () => closeCount };
    providers.push(tracked);
    return tracked;
  };
  try {
    const owner = await issue({
      ...input,
      principalKind: 'remote_owner',
      principalId: 'unbound-provider-owner',
      operationGrants: [],
      canPublishClientCapabilities: false,
    });
    const before = await deliveries();
    for (const invalid of [
      { canPublishClientCapabilities: false },
      { canUseHostPaths: true },
      { operationGrants: [] },
      { operationGrants: ['host.status'] },
      { operationGrants: ['client.capability.replace'] },
      { operationGrants: ['client.capability.unregister'] },
      { operationGrants: [...grants, 'session.catalog.query'] },
      { operationGrants: [...grants, 'access.credential.issue'] },
      { capabilityOwnerCredentialId: 'missing-provider-owner' },
      { capabilityOwnerCredentialId: owner.issued.credentialId },
    ]) {
      await assert.rejects(request(local, 'access.credential.issue', { ...input, ...invalid }), {
        code: 'invalid_request',
      });
      assert.deepEqual(await deliveries(), before, 'rejected issue created a private delivery');
    }
    const { issued, bearer } = await issue(input);
    assert.equal(issued.principalKind, 'capability_provider');
    assert.equal(issued.principalId, input.principalId);
    assert.equal(issued.canPublishClientCapabilities, true);
    assert.equal(issued.canUseHostPaths, false);
    assert.equal(Object.hasOwn(issued, 'capabilityOwner'), false);
    assert.deepEqual([...issued.operationGrants].sort(), [...grants, 'host.status'].sort());
    assert.deepEqual(await deliveries(), before);
    const connected = await connect(bearer);
    assert.equal(connected.kind, 'connected');
    const peer = connected.connection;
    assert.equal((await peer.status(3000)).state, 'ready');
    for (const [operation, value] of [
      ['connection.catalog.query', { kind: 'start' }],
      ['session.catalog.query', { kind: 'list_start' }],
      ['access.credential.issue', input],
      ['access.credential.revoke', { credentialId: issued.credentialId }],
    ]) {
      await assert.rejects(request(peer, operation, value), { code: 'unauthorized' });
    }

    const first = provider();
    const registered = await peer.replaceClientCapabilities(first.value, { timeoutMs: 3000 });
    assert.equal(typeof registered.registrationId, 'string');
    assert(registered.registrationId.length > 0);
    for (const [invalid, code] of [
      [provider({}, [{ serviceId: 'forbidden_service', version: '1' }]), 'invalid_request'],
      [provider({ affinity: 'turn' }), 'invalid_request'],
      [provider({ hostPathAccess: 'cwd' }), 'unauthorized'],
    ]) {
      try {
        await assert.rejects(peer.replaceClientCapabilities(invalid.value, { timeoutMs: 3000 }), {
          code,
        });
      } finally {
        invalid.value.close();
      }
    }
    // Rejections preserve the active registration and consume no registry revisions.
    const unregistered = await peer.unregisterClientCapabilities({ timeoutMs: 3000 });
    assert.deepEqual(unregistered, {
      registrationId: registered.registrationId,
      revision: registered.revision + 1,
    });
    await bounded(first.closed, 'unregistered provider release');
    assert.equal(first.closeCount(), 1);

    const active = provider();
    const replaced = await peer.replaceClientCapabilities(active.value, { timeoutMs: 3000 });
    assert.notEqual(replaced.registrationId, registered.registrationId);
    assert.equal(replaced.revision, unregistered.revision + 1);
    assert.deepEqual(
      await request(local, 'access.credential.revoke', {
        credentialId: issued.credentialId,
      }),
      { credentialId: issued.credentialId, revoked: true },
    );
    credentials.delete(issued.credentialId);
    await bounded(peer.closed, 'revoked provider connection closure');
    await bounded(active.closed, 'revoked provider release');
    assert.equal(active.closeCount(), 1);
    assert.deepEqual(await connect(bearer), {
      kind: 'unavailable',
      reason: 'authentication_failed',
    });
    assert.equal((await local.status(3000)).state, 'ready');
    console.log('original-client-provider-access');
  } finally {
    await Promise.all(peers.map((peer) => bounded(peer.close(), 'provider cleanup')));
    for (const entry of providers) if (entry.closeCount() === 0) entry.value.close();
    for (const credentialId of credentials) {
      await request(local, 'access.credential.revoke', { credentialId });
    }
  }
}

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
import { connectRemoteRuntimeHost } from '../../packages/runtime-host/src/client/connection.ts';
import { consumeAccessCredentialDeliveryFromControlDirectory as consume } from '../../packages/runtime-host/src/control/access-credential-delivery.ts';

export async function verifyPairingAccess(local, handshake, url, control, bounded) {
  const request = (peer, operation, input) => peer.request(operation, input, 3000);
  const credentials = new Set();
  const peers = [];
  const ownerInput = {
    principalKind: 'remote_owner',
    principalId: 'pairing-owner-acceptance',
    operationGrants: [
      'access.credential.finalize',
      'connection.catalog.query',
      'client.capability.replace',
      'client.capability.unregister',
    ],
    canPublishClientCapabilities: true,
    canUseHostPaths: true,
  };
  const providerInput = {
    principalKind: 'capability_provider',
    principalId: 'pairing-provider-acceptance',
    operationGrants: ['client.capability.replace', 'client.capability.unregister'],
    canPublishClientCapabilities: true,
    canUseHostPaths: false,
  };
  const capability = {
    offers: () => [
      {
        offerId: 'pairing-provider-tool',
        version: '1',
        affinity: 'session',
        hostPathAccess: 'none',
        label: 'Pairing provider acceptance tool',
        tools: [{ serverId: 'pairing-provider', name: 'inspect', inputSchema: { type: 'object' } }],
      },
    ],
    services: () => [],
    call: async () => {
      throw new Error('Unexpected associated provider invocation');
    },
    close: () => {},
  };
  const mint = async (operation, input) => {
    const issued = await request(local, operation, input);
    credentials.add(issued.credentialId);
    const bearer = await consume(control, issued.deliveryId, issued.credentialId);
    assert(/^maka_rh_[A-Za-z0-9_-]{43}$/u.test(bearer), 'invalid private bearer format');
    assert(!JSON.stringify(issued).includes(bearer), 'public credential result exposes bearer');
    return { issued, bearer };
  };
  const connect = async (credential, clientInstanceId) => {
    const result = await connectRemoteRuntimeHost({
      ...handshake,
      url,
      credential: credential.bearer,
      clientInstanceId,
      connectTimeoutMs: 3000,
    });
    if (result.kind === 'connected') peers.push(result.connection);
    return result;
  };
  const ready = async (credential, clientInstanceId) => {
    const result = await connect(credential, clientInstanceId);
    assert.equal(result.kind, 'connected');
    assert.equal((await result.connection.status(3000)).state, 'ready');
    return result.connection;
  };
  const denyConnect = async (credential, clientInstanceId) => {
    assert.equal((await connect(credential, clientInstanceId)).kind, 'unavailable');
  };
  const revoke = async (credential) => {
    const credentialId = credential.issued.credentialId;
    assert.deepEqual(await request(local, 'access.credential.revoke', { credentialId }), {
      credentialId,
      revoked: true,
    });
    credentials.delete(credentialId);
  };
  const query = (peer) => request(peer, 'connection.catalog.query', { kind: 'start' });
  const finalize = (peer) => request(peer, 'access.credential.finalize', {});
  const associated = (owner) => ({
    ...providerInput,
    capabilityOwnerCredentialId: owner.issued.credentialId,
  });
  try {
    for (const invalid of [
      { ...ownerInput, operationGrants: [] },
      {
        ...providerInput,
        operationGrants: [...providerInput.operationGrants, 'access.credential.finalize'],
      },
    ]) {
      await assert.rejects(request(local, 'access.credential.prepare', invalid), {
        code: 'invalid_request',
      });
    }

    const predecessor = await mint('access.credential.issue', ownerInput);
    const oldOwner = await ready(predecessor, 'pairing-predecessor');
    const candidate = await mint('access.credential.prepare', {
      ...ownerInput,
      bindClientInstance: true,
    });
    const winnerId = 'pairing-winning-client';
    const winner = await ready(candidate, winnerId);
    const loser = await ready(candidate, 'pairing-losing-client');
    await assert.rejects(query(winner), { code: 'unauthorized' }, 'pending winner catalog denied');
    await assert.rejects(
      winner.replaceClientCapabilities(capability, 3000),
      { code: 'unauthorized' },
      'pending winner capability publication denied',
    );
    await assert.rejects(request(local, 'access.credential.issue', associated(candidate)), {
      code: 'invalid_request',
    });
    assert.deepEqual(await finalize(winner), { reconnectRequired: true });
    await bounded(oldOwner.closed, 'pairing predecessor closure');
    await denyConnect(predecessor, 'pairing-predecessor');
    await assert.rejects(finalize(loser), { code: 'invalid_request' });
    // Connection authority is an admission snapshot; finalize does not elevate it.
    await assert.rejects(
      query(winner),
      { code: 'unauthorized' },
      'finalized winner stays restricted',
    );
    await assert.rejects(query(loser), { code: 'unauthorized' }, 'loser stays restricted');
    assert.deepEqual(await finalize(winner), { reconnectRequired: true });
    await denyConnect(candidate, 'pairing-wrong-reconnect');
    const bound = await ready(candidate, winnerId);
    await query(bound);
    assert.deepEqual(await finalize(bound), { reconnectRequired: false });

    const unbound = await mint('access.credential.issue', ownerInput);
    const unboundPeer = await ready(unbound, 'pairing-unbound-client');
    assert.deepEqual(await finalize(unboundPeer), { reconnectRequired: false });
    await query(await ready(unbound, 'pairing-other-unbound-client'));
    await denyConnect(unbound, winnerId);
    await assert.rejects(request(local, 'access.credential.issue', associated(unbound)), {
      code: 'invalid_request',
    });

    const provider = await mint('access.credential.issue', associated(candidate));
    const ownerSnapshot = { principalId: ownerInput.principalId, clientInstanceId: winnerId };
    assert.deepEqual(provider.issued.capabilityOwner, ownerSnapshot);
    assert.deepEqual(
      [...provider.issued.operationGrants].sort(),
      [...providerInput.operationGrants, 'host.status'].sort(),
    );
    const providerPeer = await ready(provider, 'pairing-provider-own-client');
    await assert.rejects(
      query(providerPeer),
      { code: 'unauthorized' },
      'associated provider catalog denied',
    );
    await providerPeer.replaceClientCapabilities(capability, 3000);
    await providerPeer.unregisterClientCapabilities(3000);
    await revoke(candidate);
    await bounded(bound.closed, 'bound owner revocation');
    assert.equal((await providerPeer.status(3000)).state, 'ready');
    await providerPeer.close();
    const survivingProvider = await ready(provider, 'pairing-provider-after-owner-revoke');
    await survivingProvider.replaceClientCapabilities(capability, 3000);
    await survivingProvider.unregisterClientCapabilities(3000);
    await assert.rejects(request(local, 'access.credential.issue', associated(candidate)), {
      code: 'invalid_request',
    });

    const replacement = await mint('access.credential.replace', ownerInput);
    await bounded(unboundPeer.closed, 'public replacement closure');
    await denyConnect(unbound, 'pairing-unbound-client');
    await query(await ready(replacement, 'pairing-replacement-client'));
    assert.equal((await survivingProvider.status(3000)).state, 'ready');

    // Unbound prepare retains its requested grants and never claims a hello identity.
    const plain = await mint('access.credential.prepare', {
      ...ownerInput,
      principalId: 'pairing-unbound-candidate',
    });
    const plainPeer = await ready(plain, 'pairing-plain-client');
    await query(plainPeer);
    assert.deepEqual(await finalize(plainPeer), { reconnectRequired: false });
    await query(await ready(plain, 'pairing-plain-other-client'));
    await assert.rejects(request(local, 'access.credential.issue', associated(plain)), {
      code: 'invalid_request',
    });
    console.log('original-client-pairing-access');
  } finally {
    await Promise.all(peers.map((peer) => bounded(peer.close(), 'pairing cleanup')));
    for (const credentialId of credentials) {
      await request(local, 'access.credential.revoke', { credentialId });
    }
  }
}

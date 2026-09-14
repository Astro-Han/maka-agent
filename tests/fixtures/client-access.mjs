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
import { createHash } from 'node:crypto';
import { readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { consumeAccessCredentialDeliveryFromControlDirectory as consume } from '../../packages/runtime-host/src/control/access-credential-delivery.ts';

export async function verifyAccess(connection, workspace, control, reopened) {
  const request = (operation, input) => connection.request(operation, input, 3000);
  const revoke = async (credentialId, revoked) => {
    assert.deepEqual(await request('access.credential.revoke', { credentialId }), {
      credentialId,
      revoked,
    });
  };
  const path = join(workspace, 'access.json');
  if (reopened) {
    const saved = JSON.parse(await readFile(path, 'utf8'));
    await revoke(saved.revoked.credentialId, false);
    await revoke(saved.active.credentialId, true);
    await revoke(saved.active.credentialId, false);
    await revoke(saved.pending.credentialId, true);
    await revoke('missing-credential', false);
    console.log('original-client-access-reopened');
    return;
  }
  const input = {
    principalKind: 'remote_owner',
    principalId: 'client-access-owner',
    operationGrants: [],
    canPublishClientCapabilities: true,
    canUseHostPaths: false,
  };
  const issue = async (operationGrants) => {
    const result = await request('access.credential.issue', { ...input, operationGrants });
    assert.deepEqual(
      Object.keys(result).sort(),
      [...Object.keys(input), 'credentialId', 'deliveryId'].sort(),
    );
    assert.equal(result.principalKind, input.principalKind);
    assert.equal(result.principalId, input.principalId);
    assert.equal(result.canPublishClientCapabilities, true);
    assert.equal(result.canUseHostPaths, false);
    return result;
  };
  const consumed = async (result) => {
    const deliveryPath = join(control, `runtime-host-access-delivery-${result.deliveryId}.json`);
    assert.equal((await stat(deliveryPath)).mode & 0o777, 0o600);
    const bearer = await consume(control, result.deliveryId, result.credentialId);
    // Boolean assertions keep the bearer out of failure diagnostics as well as success output.
    assert(/^maka_rh_[A-Za-z0-9_-]{43}$/u.test(bearer), 'invalid private bearer format');
    assert(!JSON.stringify(result).includes(bearer), 'public issue result exposes bearer');
    await assert.rejects(consume(control, result.deliveryId, result.credentialId), {
      code: 'ENOENT',
    });
    return {
      credentialId: result.credentialId,
      credentialHash: createHash('sha256').update(bearer).digest('hex'),
    };
  };
  for (const operationGrants of [['unknown.operation'], ['access.credential.issue']]) {
    await assert.rejects(request('access.credential.issue', { ...input, operationGrants }), {
      code: 'invalid_request',
    });
  }
  await assert.rejects(
    request('access.credential.issue', {
      ...input,
      principalKind: 'capability_provider',
    }),
    { code: 'invalid_request' },
  );
  assert.deepEqual(
    (await readdir(control)).filter((name) => name.startsWith('runtime-host-access-delivery-')),
    [],
  );
  await revoke('missing-credential', false);
  const first = await issue([]);
  assert.deepEqual(first.operationGrants, ['host.status']);
  const revoked = await consumed(first);
  await revoke(revoked.credentialId, true);
  await revoke(revoked.credentialId, false);
  const second = await issue(['host.status', 'session.catalog.query']);
  assert.deepEqual(second.operationGrants, ['host.status', 'session.catalog.query']);
  const active = await consumed(second);
  const pending = await issue([]);
  const deliveries = (await readdir(control)).filter((name) =>
    name.startsWith('runtime-host-access-delivery-'),
  );
  assert.deepEqual(deliveries, [`runtime-host-access-delivery-${pending.deliveryId}.json`]);
  // Persist only identities and hashes; leave the third private delivery for shutdown cleanup.
  await writeFile(
    path,
    JSON.stringify({ revoked, active, pending: { credentialId: pending.credentialId } }),
    { mode: 0o600 },
  );
  console.log('original-client-access');
}

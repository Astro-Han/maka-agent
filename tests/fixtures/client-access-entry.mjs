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
import { execFile } from 'node:child_process';
import { parseArgs, promisify } from 'node:util';
import {
  connectExistingRuntimeHost,
  connectRemoteRuntimeHost,
} from '../../packages/runtime-host/src/client/connection.js';

const { values } = parseArgs({
  options: { 'native-access': { type: 'string' }, root: { type: 'string' } },
});
const run = promisify(execFile);
const cli = async (...args) => {
  const result = await run(
    values['native-access'],
    ['host', 'access', ...args, '--root', values.root],
    {
      timeout: 15_000,
      maxBuffer: 16 * 1024,
      windowsHide: true,
    },
  );
  return JSON.parse(result.stdout);
};
const local = await connectExistingRuntimeHost({
  rootPath: values.root,
  protocol: { min: 0, max: 0 },
  compositionId: 'maka.interactive',
});
assert.equal(local.kind, 'connected');
const connections = [];
let pairing;
let revoked = false;
try {
  pairing = await cli('prepare', '--principal', 'native-pairing');
  assert.equal(pairing.rootId, local.connection.rootId);
  assert(typeof pairing.credential === 'string' && pairing.credential.startsWith('maka_rh_'));
  const connect = async (clientInstanceId) => {
    const result = await connectRemoteRuntimeHost({
      url: local.registration.websocketEndpoints[0],
      credential: pairing.credential,
      expectedRootId: pairing.rootId,
      compositionId: 'maka.interactive',
      protocol: { min: 0, max: 0 },
      clientInstanceId,
      connectTimeoutMs: 3000,
    });
    if (result.kind === 'connected') connections.push(result.connection);
    return result;
  };
  const pending = await connect('pairing-winner');
  assert.equal(pending.kind, 'connected');
  assert.equal((await pending.connection.status()).state, 'ready');
  await assert.rejects(pending.connection.request('connection.catalog.query', { kind: 'start' }), {
    code: 'unauthorized',
  });
  assert.deepEqual(await pending.connection.request('access.credential.finalize', {}), {
    reconnectRequired: true,
  });
  await assert.rejects(pending.connection.request('connection.catalog.query', { kind: 'start' }), {
    code: 'unauthorized',
  });
  await pending.connection.close();
  assert.equal((await connect('pairing-other-client')).kind, 'unavailable');
  const active = await connect('pairing-winner');
  assert.equal(active.kind, 'connected');
  await active.connection.request('connection.catalog.query', { kind: 'start' });
  assert.equal((await active.connection.status()).hostEpoch, local.connection.hostEpoch);
  await assert.rejects(active.connection.request('host.diagnostics.query', {}), {
    code: 'unauthorized',
  });
  await assert.rejects(
    active.connection.request('access.credential.revoke', { credentialId: pairing.credentialId }),
    { code: 'unauthorized' },
  );
  const removed = await cli('revoke', '--credential-id', pairing.credentialId);
  assert.deepEqual(removed, { credentialId: pairing.credentialId, revoked: true });
  revoked = true;
  let timer;
  try {
    await Promise.race([
      active.connection.closed,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error('revoked transport stayed open')), 3000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
  assert.equal((await connect('pairing-winner')).kind, 'unavailable');
  assert.equal((await local.connection.status()).state, 'ready');
  console.log('NATIVE_PAIRING_FINALIZED_BOUND_AND_REVOKED');
} finally {
  await Promise.all(connections.map((connection) => connection.close()));
  if (pairing && !revoked) await cli('revoke', '--credential-id', pairing.credentialId);
  await local.connection.close();
}

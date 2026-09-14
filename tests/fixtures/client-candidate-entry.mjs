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
import { parseArgs } from 'node:util';
import { createNativeRuntimeHostCandidateLaunchBarrier } from '../../apps/desktop/src/main/native-runtime-host.js';
import { connectExistingRuntimeHost } from '../../packages/runtime-host/src/client/connection.js';

const { values } = parseArgs({
  options: {
    'native-candidate': { type: 'string' },
    root: { type: 'string' },
  },
});
assert(values['native-candidate'] && values.root);
const barrier = createNativeRuntimeHostCandidateLaunchBarrier(values['native-candidate']);
const deadline = setTimeout(() => {
  console.error('Native candidate discovery timed out');
  process.exit(1); // The owner pipe also closes if this test fails.
}, 12_000);
let result;
try {
  result = await barrier.connect({
    rootPath: values.root,
    protocol: { min: 0, max: 0 },
    compositionId: 'maka.interactive',
    generation: 'native-cli-test',
    candidateEntrypoint: 'host',
    closeOnLauncherExit: true,
    electionDeadlineMs: 8_000,
  });
  assert.equal(result.kind, 'connected', JSON.stringify(result));
  assert(result.spawnedProcess, 'election must observe the real native child');
  assert.equal(result.registration.pid, result.spawnedProcess.pid);
  assert.equal(result.registration.hostEpoch, result.connection.hostEpoch);
  assert.equal((await result.connection.status(2_000)).state, 'ready');
  assert.deepEqual(await result.connection.request('network-proxy.test', {}), {
    ok: false,
    latencyMs: 0,
    error: 'Proxy disabled',
  });
  await assert.rejects(
    result.connection.request('host.resources.query', {}),
    (error) => error.code === 'internal_failure' && error.message.includes('not implemented'),
  );
  assert.equal((await result.connection.status(2_000)).hostEpoch, result.registration.hostEpoch);
  const challenger = {
    rootPath: values.root,
    protocol: { min: 0, max: 0 },
    compositionId: 'maka.interactive',
    generation: 'native-cli-successor',
    takeoverHostEpoch: result.registration.hostEpoch,
  };
  const busy = await connectExistingRuntimeHost(challenger);
  assert.equal(busy.handshake.replacement, 'blocked_by_residency');
  assert.equal(busy.handshake.activity.connections, 1);
  await result.connection.close();
  const previous = result.spawnedProcess;
  let takeover;
  do {
    takeover = await connectExistingRuntimeHost(challenger);
  } while (
    takeover.kind === 'incompatible' &&
    takeover.handshake.replacement === 'blocked_by_residency'
  );
  assert.equal(takeover.kind, 'draining', JSON.stringify(takeover));
  assert.equal((await previous.exited).code, 0);
  result = await barrier.connect({
    ...challenger,
    candidateEntrypoint: 'host',
    closeOnLauncherExit: true,
    electionDeadlineMs: 8_000,
  });
  assert.equal(result.kind, 'connected', JSON.stringify(result));
  assert.notEqual(result.spawnedProcess.pid, previous.pid);
  assert.equal((await result.connection.status(2_000)).state, 'ready');
  await result.connection.close();
  barrier.pause();
  await barrier.retireExcept(-1);
  const exit = await result.spawnedProcess.exited;
  assert.equal(exit.code, 0, exit.stderr);
  assert.equal(exit.signal, null);
  console.log(
    JSON.stringify({
      check: 'native-candidate-discovery-and-retirement',
      pid: result.registration.pid,
    }),
  );
} finally {
  if (result?.kind === 'connected') await result.connection.close().catch(() => {});
  barrier.release();
  clearTimeout(deadline);
}

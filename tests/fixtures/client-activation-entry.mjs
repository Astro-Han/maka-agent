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
import { connectExistingRuntimeHost } from '../../packages/runtime-host/src/client/connection.js';
import { decodeRuntimeHostActivationFrame } from '../../packages/runtime-host/src/operator/activation-frame.js';
import { createNativeRuntimeHostCandidateLaunchBarrier } from '../../apps/desktop/src/main/native-runtime-host.js';

const { values } = parseArgs({
  options: {
    'activation-frame': { type: 'string' },
    'lifecycle-mode': { type: 'string' },
    'native-host': { type: 'string' },
    root: { type: 'string' },
  },
});
const frame = decodeRuntimeHostActivationFrame(values['activation-frame']);
assert.equal(frame?.kind, 'result');
const input = {
  rootPath: values.root,
  protocol: { min: 0, max: 0 },
  compositionId: 'maka.interactive',
  generation: `${frame.deploymentId}:${frame.configRevision}`,
};
const connection = await connectExistingRuntimeHost(input);
assert.equal(connection.kind, 'connected', JSON.stringify(connection));
assert.equal(connection.registration.lifecycleMode, values['lifecycle-mode']);
try {
  const barrier = createNativeRuntimeHostCandidateLaunchBarrier(values['native-host']);
  try {
    const managed = await barrier.connect({
      ...input,
      generation: 'desktop-generation',
      takeoverHostEpoch: frame.hostEpoch,
      candidateEntrypoint: 'must-not-spawn-this',
    });
    assert.equal(managed.kind, 'connected');
    try {
      assert.equal(managed.spawnedProcess, undefined);
      assert.equal(managed.connection.hostEpoch, frame.hostEpoch);
      assert.deepEqual(managed.managedDeployment, {
        deploymentId: frame.deploymentId,
        configRevision: frame.configRevision,
      });
    } finally {
      await managed.connection.close();
    }
  } finally {
    barrier.release();
  }
  assert.equal(connection.connection.rootId, frame.rootId);
  assert.equal(connection.connection.hostEpoch, frame.hostEpoch);
  const diagnostics = await connection.connection.request('host.diagnostics.query', {});
  assert.equal(diagnostics.pid, frame.pid);
  const stale = await connectExistingRuntimeHost({
    ...input,
    generation: 'stale-deployment',
    ...(values['lifecycle-mode'] === 'service' ? { takeoverHostEpoch: frame.hostEpoch } : {}),
  });
  if (values['lifecycle-mode'] === 'service') {
    assert.equal(stale.kind, 'connected', 'Desktop generation cannot take over a service');
    try {
      assert.equal(stale.connection.hostEpoch, frame.hostEpoch);
    } finally {
      await stale.connection.close();
    }
    await connection.connection.close();
    const deadline = Date.now() + 3000;
    for (;;) {
      const incompatible = await connectExistingRuntimeHost({
        ...input,
        protocol: { min: 1, max: 1 },
      });
      assert.equal(incompatible.kind, 'incompatible');
      assert.equal(incompatible.handshake.replacement, 'blocked_by_residency');
      if (incompatible.handshake.activity?.connections === 0) break;
      assert(Date.now() < deadline, 'previous service clients did not finish closing');
    }
  } else {
    assert.equal(stale.kind, 'upgrade_required');
    assert.equal(
      stale.handshake.generation,
      input.generation,
      'live handshake rejected stale code',
    );
  }
} finally {
  await connection.connection.close();
}

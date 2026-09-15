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
import { createNativeRuntimeHostCandidateLaunchBarrier } from '../../apps/desktop/src/main/native-runtime-host.js';
import { readNativeRuntimeHostDeployment } from '../../apps/desktop/src/main/native-runtime-host-deployment.js';
import { decodeNativeRuntimeHostDeploymentStatus } from '../../apps/desktop/src/shared/native-runtime-host-deployment.js';
import { connectExistingRuntimeHost } from '../../packages/runtime-host/src/client/connection.js';

const run = promisify(execFile);
const { values } = parseArgs({
  options: {
    'native-managed': { type: 'string' },
    root: { type: 'string' },
    'root-id': { type: 'string' },
    revoked: { type: 'boolean', default: false },
  },
});
const executable = values['native-managed'];
assert(executable && values.root && values['root-id']);
const observed = await readNativeRuntimeHostDeployment(executable, values['root-id']);
assert.equal(observed.kind, 'installed');
assert.throws(() => decodeNativeRuntimeHostDeploymentStatus(observed, 'f'.repeat(64)));
assert.throws(() =>
  decodeNativeRuntimeHostDeploymentStatus(
    {
      ...observed,
      pendingUpdate: { ...observed.deployment, configRevision: 0 },
    },
    values['root-id'],
  ),
);
const expected = {
  deploymentId: observed.deployment.deploymentId,
  configRevision: observed.deployment.configRevision,
};
const barrier = createNativeRuntimeHostCandidateLaunchBarrier(executable);
const request = {
  rootPath: values.root,
  protocol: { min: 0, max: 0 },
  compositionId: 'maka.interactive',
  generation: 'desktop-version-does-not-own-managed-host',
  candidateEntrypoint: 'must-not-spawn-this',
  closeOnLauncherExit: true,
};
const connections = [];
async function stop() {
  const deadline = Date.now() + 5000;
  for (;;) {
    const { stdout } = await run(
      executable,
      [
        'host',
        'stop',
        '--root-id',
        values['root-id'],
        '--expected-deployment-id',
        expected.deploymentId,
        '--expected-revision',
        String(expected.configRevision),
      ],
      { windowsHide: true },
    );
    const outcome = JSON.parse(stdout);
    if (outcome.kind === 'stopped') return;
    assert.equal(outcome.kind, 'active_tasks');
    assert(Date.now() < deadline, 'fixture connections did not finish closing');
  }
}
try {
  if (values.revoked) {
    assert.equal(observed.deployment.admission, 'revoked');
    await assert.rejects(barrier.connect(request), /uninstalled/u);
  } else {
    assert.equal(observed.host.kind, 'unavailable', 'exercise cold managed activation');
    const pair = await Promise.all([barrier.connect(request), barrier.connect(request)]);
    for (const result of pair) {
      assert.equal(result.kind, 'connected', JSON.stringify(result));
      connections.push(result.connection);
      assert.equal(result.spawnedProcess, undefined, 'operator PID is not Host ownership');
      assert.deepEqual(result.managedDeployment, expected);
    }
    assert.equal(pair[0].connection.hostEpoch, pair[1].connection.hostEpoch);
    const firstEpoch = pair[0].connection.hostEpoch;
    const observer = await barrier.connect({
      ...request,
      generation: 'another-desktop-version',
      takeoverHostEpoch: firstEpoch,
    });
    assert.equal(observer.kind, 'connected');
    connections.push(observer.connection);
    assert.equal(observer.connection.hostEpoch, firstEpoch);
    assert.deepEqual(observer.managedDeployment, expected);
    barrier.pause();
    await assert.rejects(barrier.connect(request), /paused/u);
    await barrier.retireExcept(pair[0].registration.pid);
    assert.equal((await observer.connection.status()).state, 'ready');
    for (const connection of connections.splice(0)) await connection.close();
    await stop();
    barrier.resume();
    const replacement = await barrier.connect(request);
    assert.equal(replacement.kind, 'connected');
    connections.push(replacement.connection);
    assert.notEqual(replacement.connection.hostEpoch, firstEpoch);
    assert.deepEqual(replacement.managedDeployment, expected);
    barrier.release();
    barrier.resume();
    await assert.rejects(barrier.connect(request), /paused/u);
    const independent = await connectExistingRuntimeHost({
      rootPath: values.root,
      protocol: request.protocol,
      compositionId: request.compositionId,
    });
    assert.equal(independent.kind, 'connected');
    connections.push(independent.connection);
    assert.equal(independent.connection.hostEpoch, replacement.connection.hostEpoch);
  }
} finally {
  barrier.release();
  for (const connection of connections) await connection.close();
  if (!values.revoked) await stop();
}

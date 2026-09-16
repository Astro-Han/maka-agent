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
import { readNativeRuntimeHostDeployment } from '../../apps/desktop/src/main/native-runtime-host-deployment.js';
import { createNativeRuntimeHostManagement } from '../../apps/desktop/src/main/native-runtime-host-management.js';
import { runNativeRuntimeHostCommand } from '../../apps/desktop/src/main/native-runtime-host-command.js';
import { decodeNativeRuntimeHostDeploymentStatus } from '../../apps/desktop/src/shared/native-runtime-host-deployment.js';
import { connectExistingRuntimeHost } from '../../packages/runtime-host/src/client/connection.js';
import { connectRuntimeHostProfile } from '../../packages/runtime-host/src/client/host-profile.js';

const { values } = parseArgs({
  options: {
    'native-managed': { type: 'string' },
    root: { type: 'string' },
    'root-id': { type: 'string' },
    revoked: { type: 'boolean', default: false },
    management: { type: 'boolean', default: false },
    ssh: { type: 'string' },
  },
});
const executable = values['native-managed'];
assert(executable && values.root && values['root-id']);
const operator = values.ssh
  ? {
      kind: 'ssh',
      destination: values.ssh,
      operator: { kind: 'native', platform: 'posix', executablePath: executable },
    }
  : { kind: 'local', executable };
const execute = (args, readOnly = false) => runNativeRuntimeHostCommand(operator, args, readOnly);
const read = async () =>
  values.ssh
    ? decodeNativeRuntimeHostDeploymentStatus(
        JSON.parse(await execute(['status', '--root-id', values['root-id']], true)),
        values['root-id'],
      )
    : readNativeRuntimeHostDeployment(executable, values['root-id']);
const observed = await read();
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
let expected = {
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
let pairing;
async function stop() {
  const deadline = Date.now() + 5000;
  for (;;) {
    const stdout = await execute([
      'stop',
      '--root-id',
      values['root-id'],
      '--expected-deployment-id',
      expected.deploymentId,
      '--expected-revision',
      String(expected.configRevision),
    ]);
    const outcome = JSON.parse(stdout);
    if (outcome.kind === 'stopped') return;
    assert.equal(outcome.kind, 'active_tasks');
    assert(Date.now() < deadline, 'fixture connections did not finish closing');
  }
}
try {
  if (values.management) {
    await verifyManagement();
  } else if (values.revoked) {
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
  if (pairing)
    await execute([
      'access',
      'revoke',
      '--root',
      values.root,
      '--credential-id',
      pairing.credentialId,
    ]);
  if (!values.revoked) {
    const current = await read();
    if (current.kind === 'installed' && current.deployment.admission === undefined) {
      expected = current.deployment;
      await stop();
    }
  }
}

async function verifyManagement() {
  let owned;
  let resumes = 0;
  const management = createNativeRuntimeHostManagement({
    operator: values.ssh ? operator : { kind: 'local', executable: observed.deployment.executable },
    rootId: values['root-id'],
    rootPath: values.root,
    change: async (run) =>
      run({
        hold() {},
        resumeOnSettled() {
          resumes++;
        },
        async retire(deployment, hostEpoch, prepareRemote) {
          if (!owned) return true;
          assert.equal(deployment.deploymentId, expected.deploymentId);
          assert.equal(hostEpoch, owned.hostEpoch);
          if (!(await prepareRemote(owned.connectionId))) return false;
          await owned.close();
          owned = undefined;
          return true;
        },
      }),
  });
  const started = await management.run({ action: 'start' });
  assert.equal(started.outcome.kind, 'ready');
  assert.equal(resumes, 1);
  if (values.ssh)
    pairing = JSON.parse(
      await execute([
        'access',
        'prepare',
        '--root',
        values.root,
        '--principal',
        'native-management-test',
      ]),
    );
  const connect = async () => {
    if (pairing) {
      const connection = await connectRuntimeHostProfile({
        profile: {
          id: 'native-test',
          name: 'Native test',
          kind: 'remote',
          rootId: values['root-id'],
          transport: {
            kind: 'ssh',
            destination: values.ssh,
            activation: { kind: 'ssh_operator', operator: operator.operator },
          },
        },
        credential: pairing.credential,
        clientInstanceId: 'native-management-test',
      });
      connections.push(connection);
      return connection;
    }
    const result = await connectExistingRuntimeHost({
      rootPath: values.root,
      protocol: request.protocol,
      compositionId: request.compositionId,
    });
    assert.equal(result.kind, 'connected');
    connections.push(result.connection);
    return result.connection;
  };
  owned = await connect();
  if (pairing) {
    await owned.request('access.credential.finalize', {});
    await owned.close();
    owned = await connect();
    await assert.rejects(
      owned.request('host.upgrade.prepare', {
        expectedHostEpoch: owned.hostEpoch,
        allowInterruptActiveTasks: false,
      }),
      { code: 'unauthorized' },
    );
  }
  const other = await connect();
  const epoch = owned.hostEpoch;
  await assert.rejects(
    management.run({
      action: 'stop',
      expected: { ...expected, configRevision: expected.configRevision + 1 },
    }),
    /changed/u,
  );
  const pending = await management.run({
    action: 'update',
    expected,
    settings: { projectDirectoryRoots: [] },
  });
  assert.equal(pending.outcome.kind, 'active_tasks');
  assert.deepEqual(pending.status.pendingUpdate, pending.outcome.target);
  assert.equal(pending.status.host.identity.hostEpoch, epoch);
  assert.equal(resumes, 2);
  await other.close();
  const changed = await management.run({ action: 'reconcile', expected });
  assert.equal(changed.outcome.kind, 'ready');
  assert.equal(changed.status.pendingUpdate, null);
  assert.deepEqual(changed.status.deployment.projectDirectoryRoots, []);
  assert.equal(changed.status.deployment.configRevision, expected.configRevision + 1);
  expected = {
    deploymentId: changed.status.deployment.deploymentId,
    configRevision: changed.status.deployment.configRevision,
  };
  const unchanged = await management.run({ action: 'update', expected, settings: {} });
  assert.deepEqual(unchanged.outcome.deployment, changed.status.deployment);
  assert.equal((await management.run({ action: 'logs' })).logs.kind, 'not_captured');
  assert.equal((await management.run({ action: 'stop', expected })).outcome.kind, 'stopped');
  assert.equal((await management.run({ action: 'restart', expected })).outcome.kind, 'ready');
  const removed = await management.run({ action: 'uninstall', expected });
  assert.equal(removed.outcome.kind, 'unregistered');
  assert.equal(removed.outcome.cleanup.kind, 'complete');
  assert.equal(removed.status.deployment.admission, 'revoked');
  await assert.rejects(management.run({ action: 'start' }), /Install/u);
  const reinstalled = await management.run({ action: 'install', settings: { mode: 'on_demand' } });
  assert.equal(reinstalled.outcome.kind, 'ready');
  assert.notEqual(reinstalled.status.deployment.deploymentId, expected.deploymentId);
  assert.equal(reinstalled.status.deployment.configRevision, 1);
}

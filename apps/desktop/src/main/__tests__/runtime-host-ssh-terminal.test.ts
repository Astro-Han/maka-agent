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
import { test } from 'node:test';
import type { IPty } from 'node-pty';
import { type RuntimeHostSshProcessFactory } from '@maka/runtime-host/client';
import {
  encodeRuntimeHostActivationFrame,
  encodeRuntimeHostAccessManagementFrame,
  encodeRuntimeHostPeerManagementFrame,
  encodeRuntimeHostServiceManagementFrame,
  encodeRuntimeHostPeerMeshManagementFrame,
  runtimeHostAccessCredentialFingerprint,
  RUNTIME_HOST_OPERATOR_PEER_MANAGEMENT_CAPABILITY,
} from '@maka/runtime-host/operator';
import {
  createDesktopRuntimeHostSshTerminal,
  runtimeHostPeerTargetFromPlatform,
} from '../runtime-host-ssh-terminal.js';
import { NATIVE_ARTIFACT_PREFIX, NATIVE_SETUP_PREFIX } from '../native-runtime-host-setup.js';
import { waitFor as pollFor } from '@maka/core/test-only/async-primitives';

const OPERATOR = {
  kind: 'node' as const,
  platform: 'posix' as const,
  nodePath: '/usr/bin/node',
  modulePath: '/home/operator/.local/share/maka/operator.mjs',
};

const WINDOWS_OPERATOR = {
  kind: 'node' as const,
  platform: 'win32' as const,
  nodePath: 'C:\\Program Files\\nodejs\\node.exe',
  modulePath: 'C:\\Users\\operator\\AppData\\Local\\Maka\\operator.mjs',
};

test('maps supported SSH platform identities to peer targets', () => {
  assert.equal(runtimeHostPeerTargetFromPlatform('linux', 'x64'), 'linux-x64');
  assert.equal(runtimeHostPeerTargetFromPlatform('linux', 'arm64'), 'linux-arm64');
  assert.equal(runtimeHostPeerTargetFromPlatform('darwin', 'arm64'), 'darwin-arm64');
  assert.equal(runtimeHostPeerTargetFromPlatform('win32', 'x64'), 'win32-x64');
  assert.throws(
    () => runtimeHostPeerTargetFromPlatform('linux', 'riscv64'),
    /not available/u,
  );
});

test('detects the peer target through the bounded SSH preflight', async () => {
  const harness = createHarness('pending');
  const detection = harness.terminal.resolveTargetIdentity({
    destination: 'operator@example.com',
  });
  await waitFor(() => harness.pty.hasDataListener());
  const command = harness.launchArgs[0]?.at(-1) ?? '';
  const suffix = command.match(/[0-9a-f]{32}__/u)?.[0];
  assert.ok(suffix);
  const marker = `__MAKA_RUNTIME_HOST_TARGET_${suffix}`;
  assert.doesNotMatch(command, /\bnode\b/u);
  harness.pty.emitData(`${marker}Linux:x86_64:glibc 2.28\r\n`);
  harness.pty.exit(0);

  assert.deepEqual(await detection, { platform: 'linux', architecture: 'x64', glibcVersion: '2.28' });
  assert.doesNotMatch(JSON.stringify(harness.events), /MAKA_RUNTIME_HOST_TARGET/u);
  await harness.terminal.close();
});

test('rejects unsupported SSH targets without crashing the output listener or retrying another OS', async () => {
  for (const [value, expected] of [
    ['Linux:x86_64:unknown', /requires GNU libc/u],
    ['Linux:x86_64:glibc 2.27', /requires glibc 2.28/u],
    ['Linux:x86_64:glibc 2.9', /requires glibc 2.28/u],
    ['Linux:riscv64:glibc 2.39', /Unsupported.*architecture/u],
    ['Linux:x86_64:glibc 2.39:extra', /Invalid.*result/u],
  ] as const) {
    const harness = createHarness('pending');
    const detection = harness.terminal.resolveTargetIdentity({ destination: 'operator@example.com' });
    const rejected = assert.rejects(detection, expected);
    await waitFor(() => harness.pty.hasDataListener());
    const suffix = harness.launchArgs[0]?.at(-1)?.match(/[0-9a-f]{32}__/u)?.[0];
    assert.ok(suffix);
    const marker = `__MAKA_RUNTIME_HOST_TARGET_${suffix}`;
    harness.pty.emitData(`${marker}${value}\r\n`);
    harness.pty.exit(0);
    await rejected;
    assert.equal(harness.launchArgs.length, 1);
    await harness.terminal.close();
  }
});

test('detects a Windows SSH target through PowerShell when no POSIX shell exists', async (t) => {
  const handlers = new Map<string, (...args: unknown[]) => unknown>();
  const launches: Array<{ args: string[]; pty: FakePty }> = [];
  const terminal = createDesktopRuntimeHostSshTerminal({
    ipcMain: {
      handle: (channel, handler) => handlers.set(channel, handler as (...args: unknown[]) => unknown),
      removeHandler: (channel) => handlers.delete(channel),
    },
    send: () => undefined,
    spawnPty: ((_file: string, args: string[]) => {
      const pty = new FakePty();
      launches.push({ args, pty });
      return pty as unknown as IPty;
    }) as typeof import('node-pty').spawn,
  });
  t.after(() => terminal.close());

  const detection = terminal.resolveTargetIdentity({ destination: 'operator@example.com' });
  await waitFor(() => launches.length === 1);
  launches[0]?.pty.exit(127);
  await waitFor(() => launches.length === 2);
  const command = launches[1]?.args.at(-1) ?? '';
  const script = Buffer.from(command.split(' ').at(-1) ?? '', 'base64').toString('utf16le');
  const marker = script.match(/__MAKA_RUNTIME_HOST_TARGET_[0-9a-f]+__/u)?.[0];
  assert.ok(marker);
  assert.match(command, /powershell.exe .* -EncodedCommand/u);
  assert.match(script, /OSArchitecture/u);
  launches[1]?.pty.emitData(`${marker}Windows:X64:\r\n`);
  launches[1]?.pty.exit(0);

  assert.deepEqual(await detection, { platform: 'win32', architecture: 'x64' });
});

test('keeps a connecting SSH prompt observable across renderer presentation changes', async () => {
  const harness = createHarness('pending');
  const opening = openTunnel(harness);
  harness.pty.emitData('Password: ');
  const snapshot = await harness.getSnapshot();
  assert.match(JSON.stringify(snapshot), /Password/u);
  assert.equal((snapshot as { kind?: string }).kind, 'connecting');

  harness.releaseTunnel();
  const tunnel = await opening;
  assert.deepEqual(harness.eventKinds(), ['opened', 'data', 'connected']);
  assert.deepEqual(await harness.getSnapshot(), { kind: 'idle', revision: 3 });

  const secondTunnel = await openTunnel(harness);

  await tunnel.resource.close();
  await secondTunnel.resource.close();
  await harness.terminal.close();
  assert.equal(harness.handlers.size, 0);
});

test('dismisses a closed SSH prompt from the authoritative presentation', async () => {
  const harness = createHarness('exit');
  const opening = openTunnel(harness);
  harness.pty.emitData('Password: ');
  harness.pty.exit(1);
  await assert.rejects(opening, /SSH exited/u);

  const closed = (await harness.getSnapshot()) as { kind: string; sessionId?: string };
  assert.equal(closed.kind, 'closed');
  assert.ok(closed.sessionId);

  await harness.cancel(closed.sessionId);
  assert.deepEqual(await harness.getSnapshot(), { kind: 'idle', revision: 4 });

  await harness.terminal.close();
});

test('does not reopen a cancelled SSH prompt for late process output', async () => {
  const harness = createHarness('exit');
  const opening = openTunnel(harness);
  harness.pty.emitData('Password: ');
  const connecting = (await harness.getSnapshot()) as { sessionId?: string };
  assert.ok(connecting.sessionId);

  await harness.cancel(connecting.sessionId);
  harness.pty.emitData('late output');
  await assert.rejects(opening, /SSH exited/u);

  assert.deepEqual(harness.eventKinds(), ['opened', 'data', 'dismissed']);
  assert.deepEqual(await harness.getSnapshot(), { kind: 'idle', revision: 3 });
  await harness.terminal.close();
});

test('native SSH setup verifies the target package and keeps its receipt out of terminal output', async () => {
  for (const windows of [false, true]) {
    const directory = windows ? 'C:\\Maka\\package' : '/opt/maka/package';
    const executable = windows ? `${directory}\\bin\\maka.exe` : `${directory}/bin/maka`;
    const artifact = {
      target: windows ? 'win32-x64' as const : 'linux-x64-gnu' as const,
      version: '0.2.0', directory, executable,
      ...(windows ? { serviceExecutable: `${directory}\\bin\\maka-service.exe` } : {}),
      integrity: `sha512-${'A'.repeat(86)}==`,
    };
    const receipt = {
      deployment: {
        deploymentId: '00000000-0000-4000-8000-000000000001', configRevision: 1,
        rootId: 'a'.repeat(64), rootPath: windows ? 'C:\\Maka\\state' : '/opt/maka/state',
        executable, sha256: 'b'.repeat(64), mode: 'on_demand', websocket: '127.0.0.1:0',
      },
      host: { hostEpoch: 'host-epoch', pid: 123, port: 4567 },
      pairing: { rootId: 'a'.repeat(64), credentialId: 'credential', credential: 'setup-private-token' },
    };
    const events: unknown[] = [];
    const commands: string[] = [];
    let committed = false;
    let launch = 0;
    const terminal = createDesktopRuntimeHostSshTerminal({
      ipcMain: { handle: () => undefined, removeHandler: () => undefined },
      send: (_channel, event) => events.push(event),
      revealDelayMs: 0,
      spawnPty: ((file: string, args: string[]) => {
        const pty = new FakePty();
        const step = launch++;
        const encoded = args.at(-1) ?? '';
        const script = windows && file === 'ssh'
          ? Buffer.from(encoded.split(' ').at(-1)!, 'base64').toString('utf16le')
          : encoded;
        const payload = windows ? script.match(/FromBase64String\('([^']+)'\)/u)?.[1] : undefined;
        const command = payload ? Buffer.from(payload, 'base64').toString('utf8') : script;
        commands.push(command);
        setImmediate(() => {
          let frame: string;
          if (step === 0) {
            const name = command.match(/maka-native-[a-f0-9]{32}/u)?.[0];
            assert.ok(name);
            frame = `__MAKA_NATIVE_HOST_STAGE__${windows ? 'C:\\Temp\\' : '/tmp/'}${name}\n`;
          } else if (step === 1) {
            assert.equal(file, 'scp');
            pty.exit(0);
            return;
          } else if (step === 2) {
            assert.match(command, /fetch/u);
            assert.ok(command.includes('c'.repeat(64)));
            frame = `${NATIVE_ARTIFACT_PREFIX}${JSON.stringify(artifact)}\n`;
          } else if (step === 3) {
            frame = '__MAKA_NATIVE_HOST_CLEAN__ok\n';
          } else {
            assert.equal(step, 4);
            assert.equal(committed, true);
            assert.match(command, /desktop:client/u);
            assert.doesNotMatch(command, /allow-interrupt|update-existing/u);
            frame = `${NATIVE_SETUP_PREFIX}${JSON.stringify(receipt)}\n`;
          }
          // PTY chunks need not coincide with reserved frame boundaries.
          pty.emitData(frame.slice(0, 11));
          pty.emitData(frame.slice(11));
          pty.exit(0);
        });
        return pty as unknown as IPty;
      }) as typeof import('node-pty').spawn,
    });
    try {
      const result = await terminal.runNativeSetup({
        destination: 'operator@example.com', principalId: 'desktop:client',
        package: { artifact, receiptSha256: 'c'.repeat(64) },
      }, () => { committed = true; });
      assert.deepEqual(result.receipt, receipt);
      assert.equal(result.operator.executablePath, executable);
      assert.equal(commands.length, 5);
      assert.doesNotMatch(JSON.stringify(events), /setup-private-token|__MAKA_NATIVE_HOST_/u);
    } finally {
      await terminal.close();
    }
  }
});

test('reads a framed service result without projecting it into the SSH terminal', async () => {
  const harness = createHarness('pending');
  const management = harness.terminal.runServiceManagement({
    destination: 'operator@example.com',
    operator: OPERATOR,
    action: 'status',
    capabilityRequest: RUNTIME_HOST_OPERATOR_PEER_MANAGEMENT_CAPABILITY,
    expectedTarget: {
      serviceId: 'b'.repeat(64),
      rootPath: '/home/operator/.config/Maka/workspaces/default',
      rootId: 'a'.repeat(64),
    },
  });
  await waitFor(() => harness.pty.hasDataListener());
  const remoteCommand = harness.launchArgs.at(-1)?.at(-1) ?? '';
  assert.match(remoteCommand, /\.local\/share\/maka\/operator/u);
  assert.match(remoteCommand, /MAKA_RUNTIME_HOST_OPERATOR_CAPABILITY_REQUEST/u);
  assert.match(remoteCommand, /peer-management-v1/u);
  assert.doesNotMatch(remoteCommand, /npx|maka-agent@/u);
  harness.pty.emitData('Password: ');
  harness.pty.emitData(
    encodeRuntimeHostServiceManagementFrame({
      schemaVersion: 1,
      kind: 'result',
      action: 'status',
      service: {
        platform: 'linux',
        arch: 'x64',
        osRelease: '6.8.0',
        state: 'running',
        pid: 42,
        lastExitCode: 0,
        installedVersion: '1.2.3',
        stateRoot: '/home/operator/.config/Maka/workspaces/default',
        projectDirectoryRoots: [],
      },
    }),
  );
  harness.pty.exit(0);

  const result = await management;
  assert.equal(result.kind, 'result');
  if (result.kind !== 'result' || result.action !== 'status') {
    assert.fail('expected service status result');
  }
  assert.equal(result.service.installedVersion, '1.2.3');
  assert.doesNotMatch(JSON.stringify(harness.events), /MAKA_RUNTIME_HOST_SERVICE/u);
  assert.match(JSON.stringify(harness.events), /Password/u);
  await harness.terminal.close();
});

test('applies the complete remote Project root policy through the managed operator', async () => {
  const harness = createHarness('pending');
  const fingerprint = `sha256:${'c'.repeat(64)}`;
  const management = harness.terminal.runServiceManagement({
    destination: 'operator@example.com',
    operator: OPERATOR,
    action: 'configure',
    expectedTarget: {
      serviceId: 'b'.repeat(64),
      rootPath: '/srv/maka',
      rootId: 'a'.repeat(64),
    },
    projectDirectoryRoots: [
      { label: 'Work=Primary', path: '/srv/work trees' },
      { label: 'Data', path: '/mnt/data' },
    ],
    expectedConfigFingerprint: fingerprint,
    allowInterruptActiveTasks: true,
  });
  await waitFor(() => harness.pty.hasDataListener());
  const remoteCommand = harness.launchArgs.at(-1)?.at(-1) ?? '';
  assert.match(remoteCommand, /operator.*configure/u);
  assert.match(remoteCommand, /--project-root-json/u);
  assert.match(remoteCommand, /Work=Primary/u);
  assert.match(remoteCommand, /srv\/work trees/u);
  assert.match(remoteCommand, /Data/u);
  assert.match(remoteCommand, /mnt\/data/u);
  assert.match(remoteCommand, /--expected-config-fingerprint/u);
  assert.match(remoteCommand, /--allow-interrupt-active-tasks/u);
  assert.match(
    remoteCommand,
    /MAKA_RUNTIME_HOST_OPERATOR_PROJECT_DIRECTORY_CONFIGURATION_REQUEST='1'/u,
  );
  harness.pty.emitData(
    encodeRuntimeHostServiceManagementFrame({
      schemaVersion: 1,
      kind: 'result',
      action: 'configure',
      service: {
        platform: 'linux',
        arch: 'x64',
        osRelease: '6.8.0',
        state: 'running',
        pid: 43,
        lastExitCode: 0,
        installedVersion: '1.2.3',
        configurationFingerprint: `sha256:${'d'.repeat(64)}`,
        projectDirectoryRoots: [
          { label: 'Work=Primary', path: '/srv/work trees' },
          { label: 'Data', path: '/mnt/data' },
        ],
      },
      configuration: { kind: 'configured' },
    }),
  );
  harness.pty.exit(0);

  const result = await management;
  assert.equal(result.kind, 'result');
  assert.equal(
    result.kind === 'result' && result.action === 'configure'
      ? result.configuration.kind
      : undefined,
    'configured',
  );
  await harness.terminal.close();
});

test('keeps a received management result when SSH teardown times out', async () => {
  const harness = createHarness('pending', { managementTimeoutMs: 1 });
  harness.pty.deferKill = true;
  harness.pty.exitOnForceKill = true;
  const management = harness.terminal.runServiceManagement({
    destination: 'operator@example.com',
    operator: OPERATOR,
    action: 'status',
    expectedTarget: {
      serviceId: 'b'.repeat(64),
      rootPath: '/srv/maka',
      rootId: 'a'.repeat(64),
    },
  });
  harness.pty.emitData(
    encodeRuntimeHostServiceManagementFrame({
      schemaVersion: 1,
      kind: 'result',
      action: 'status',
      service: {
        platform: 'linux',
        arch: 'x64',
        osRelease: '6.8.0',
        state: 'running',
        pid: 42,
        lastExitCode: 0,
        installedVersion: '1.2.3',
        projectDirectoryRoots: [],
      },
    }),
  );

  assert.equal((await management).kind, 'result');
  assert.deepEqual(harness.pty.killSignals, ['SIGTERM', 'SIGKILL']);
  await harness.terminal.close();
});

test('runs an exact update package and reports progress before an active-work result', async () => {
  const harness = createHarness('pending');
  const phases: string[] = [];
  const update = harness.terminal.runUpdate(
    {
      destination: 'operator@example.com',
      setupPackage: { kind: 'npm', specifier: 'maka-agent@1.3.0' },
      operator: OPERATOR,
      expectedTarget: {
        serviceId: 'b'.repeat(64),
        rootPath: '/srv/maka',
        rootId: 'a'.repeat(64),
        deploymentId: '00000000-0000-4000-8000-000000000001',
      },
    },
    (phase) => phases.push(phase),
  );
  await waitFor(() => harness.pty.hasDataListener());
  const remoteCommand = harness.launchArgs.at(-1)?.at(-1) ?? '';
  assert.match(remoteCommand, /--package.*maka-agent@1\.3\.0/u);
  assert.match(remoteCommand, /runtime-host.*service.*update/u);
  assert.match(remoteCommand, /--target.*1\.3\.0/u);
  assert.match(remoteCommand, /--managed-root-id.*a{64}/u);
  assert.doesNotMatch(remoteCommand, /--operator-deployment-id/u);
  assert.match(remoteCommand, /MAKA_RUNTIME_HOST_OPERATOR_CAPABILITY_REQUEST/u);
  harness.pty.emitData('Password: ');
  harness.pty.emitData(
    encodeRuntimeHostServiceManagementFrame({
      schemaVersion: 1,
      kind: 'progress',
      action: 'update',
      phase: 'retiring',
      currentVersion: '1.2.3',
      targetVersion: '1.3.0',
    }),
  );
  harness.pty.emitData(
    encodeRuntimeHostServiceManagementFrame({
      schemaVersion: 1,
      kind: 'result',
      action: 'update',
      service: {
        platform: 'linux',
        arch: 'x64',
        osRelease: '6.8.0',
        state: 'running',
        pid: 42,
        lastExitCode: 0,
        installedVersion: '1.2.3',
        projectDirectoryRoots: [],
      },
      update: {
        kind: 'active_tasks',
        currentVersion: '1.2.3',
        targetVersion: '1.3.0',
      },
    }),
  );
  harness.pty.exit(1);

  const result = await update;
  assert.equal(result.kind, 'result');
  assert.equal(result.kind === 'result' ? result.update.kind : undefined, 'active_tasks');
  assert.deepEqual(phases, ['retiring']);
  assert.deepEqual(harness.events.map(({ kind }) => kind), ['opened', 'data', 'connected']);
  assert.doesNotMatch(JSON.stringify(harness.events), /MAKA_RUNTIME_HOST_SERVICE/u);
  await harness.terminal.close();
});

test('uses the managed operator for update policy and one-shot reconciliation', async () => {
  const target = {
    serviceId: 'b'.repeat(64),
    rootPath: '/srv/maka',
    rootId: 'a'.repeat(64),
  };
  const policyHarness = createHarness('pending');
  const policy = policyHarness.terminal.runUpdatePolicy({
    destination: 'operator@example.com',
    operator: OPERATOR,
    policy: { kind: 'channel', channel: 'latest' },
    expectedTarget: target,
  });
  await waitFor(() => policyHarness.pty.hasDataListener());
  const policyCommand = policyHarness.launchArgs.at(-1)?.at(-1) ?? '';
  assert.match(policyCommand, /operator.*update-policy.*--target.*latest/u);
  assert.match(policyCommand, /--expected-service-id/u);
  assert.match(policyCommand, /update-scheduler-v1/u);
  policyHarness.pty.emitData(encodeRuntimeHostServiceManagementFrame({
    schemaVersion: 1,
    kind: 'result',
    action: 'update_policy',
    updateSchedulerState: 'ready',
    updatePolicy: {
      policy: { kind: 'channel', channel: 'latest' },
      target,
    },
  }));
  policyHarness.pty.exit(0);
  assert.equal((await policy).kind, 'result');
  await policyHarness.terminal.close();

  const reconcileHarness = createHarness('pending');
  const phases: string[] = [];
  const reconciliation = reconcileHarness.terminal.runUpdateReconciliation(
    {
      destination: 'operator@example.com',
      operator: OPERATOR,
      expectedTarget: target,
    },
    (phase) => phases.push(phase),
  );
  await waitFor(() => reconcileHarness.pty.hasDataListener());
  const reconcileCommand = reconcileHarness.launchArgs.at(-1)?.at(-1) ?? '';
  assert.match(reconcileCommand, /operator.*reconcile-update.*--framed/u);
  assert.match(reconcileCommand, /--expected-service-id/u);
  assert.match(reconcileCommand, /update-scheduler-v1/u);
  reconcileHarness.pty.emitData(encodeRuntimeHostServiceManagementFrame({
    schemaVersion: 1,
    kind: 'progress',
    action: 'reconcile_update',
    phase: 'checking',
    currentVersion: '1.2.3',
    targetVersion: '1.3.0',
  }));
  reconcileHarness.pty.emitData(encodeRuntimeHostServiceManagementFrame({
    schemaVersion: 1,
    kind: 'result',
    action: 'reconcile_update',
    updateSchedulerState: 'ready',
    updatePolicy: {
      policy: { kind: 'channel', channel: 'latest' },
      target,
    },
    service: {
      platform: 'linux',
      arch: 'x64',
      osRelease: '6.8.0',
      state: 'running',
      pid: 42,
      lastExitCode: 0,
      installedVersion: '1.2.3',
      projectDirectoryRoots: [],
    },
    reconciliation: { kind: 'already_current', version: '1.2.3' },
  }));
  reconcileHarness.pty.exit(0);
  assert.equal((await reconciliation).kind, 'result');
  assert.deepEqual(phases, ['checking']);
  await reconcileHarness.terminal.close();
});

test('keeps a prepared access credential out of the SSH terminal projection', async () => {
  const harness = createHarness('pending');
  const credential = 'maka_rh_secret-replacement';
  const management = harness.terminal.runAccessManagement({
    destination: 'operator@example.com',
    operator: OPERATOR,
    rootPath: '/srv/maka',
    expectedRootId: 'a'.repeat(64),
    action: 'prepare',
    currentCredentialFingerprint: 'b'.repeat(32),
  });
  await waitFor(() => harness.pty.hasDataListener());
  harness.pty.emitData('Password: ');
  harness.pty.emitData(
    encodeRuntimeHostAccessManagementFrame({
      schemaVersion: 1,
      kind: 'result',
      action: 'prepare',
      credential,
      credentials: [{
        credentialId: 'credential-2',
        credentialFingerprint: runtimeHostAccessCredentialFingerprint(credential),
        principalKind: 'remote_owner',
        principalId: 'desktop:stable-client',
        status: 'pending',
        operationGrants: ['host.status', 'access.credential.finalize'],
        canPublishClientCapabilities: true,
        canUseHostPaths: false,
        createdAt: '2026-08-21T01:00:00.000Z',
        expiresAt: '2026-08-21T01:15:00.000Z',
      }],
    }),
  );
  harness.pty.exit(0);

  const result = await management;
  assert.equal(result.kind, 'result');
  assert.equal(result.kind === 'result' && result.action === 'prepare' ? result.credential : undefined, credential);
  assert.doesNotMatch(JSON.stringify(harness.events), /secret-replacement|MAKA_RUNTIME/u);
  const command = harness.launchArgs.at(-1)?.at(-1) ?? '';
  assert.match(command, /access.*prepare/u);
  assert.match(command, /--current-fingerprint/u);
  assert.match(command, new RegExp('b{32}', 'u'));
  assert.doesNotMatch(command, /secret-replacement/u);
  await harness.terminal.close();
});

test('creates an owner connection code through the framed SSH operator channel', async () => {
  const harness = createHarness('pending');
  const connectionCode = 'maka-runtime-host:connect:v1:secret-code';
  const management = harness.terminal.runAccessManagement({
    destination: 'operator@example.com',
    operator: OPERATOR,
    rootPath: '/srv/maka',
    expectedRootId: 'a'.repeat(64),
    action: 'connection-code',
    name: "Owner's Linux",
  });
  await waitFor(() => harness.pty.hasDataListener());
  harness.pty.emitData(encodeRuntimeHostAccessManagementFrame({
    schemaVersion: 1,
    kind: 'result',
    action: 'connection-code',
    connectionCode,
  }));
  harness.pty.exit(0);

  const result = await management;
  assert.equal(
    result.kind === 'result' && result.action === 'connection-code'
      ? result.connectionCode
      : undefined,
    connectionCode,
  );
  const command = harness.launchArgs.at(-1)?.at(-1) ?? '';
  assert.match(command, /access.*connection-code/u);
  assert.match(command, /--name/u);
  assert.match(command, /Owner/u);
  assert.doesNotMatch(JSON.stringify(harness.events), /secret-code|MAKA_RUNTIME/u);
  await harness.terminal.close();
});

test('requests adaptive-connectivity status only on the peer-management frame', async () => {
  const harness = createHarness('pending');
  const management = harness.terminal.runPeerManagement({
    destination: 'operator@example.com',
    operator: OPERATOR,
    action: 'status',
    webRtcStunStatus: true,
    expectedTarget: {
      serviceId: 'b'.repeat(64),
      rootPath: '/srv/maka',
      rootId: 'a'.repeat(64),
      deploymentId: '00000000-0000-4000-8000-000000000001',
    },
  });
  await waitFor(() => harness.pty.hasDataListener());
  const command = harness.launchArgs.at(-1)?.at(-1) ?? '';
  assert.match(
    command,
    /peer.*status.*--framed.*--relay-discovery-status.*--webrtc-stun-status/u,
  );

  harness.pty.emitData(
    encodeRuntimeHostPeerManagementFrame({
      kind: 'result',
      action: 'status',
      status: {
        state: 'enabled',
        serviceState: 'running',
        peerId: '12D3KooWpeer',
        rootId: 'a'.repeat(64),
        routeHints: ['/ip4/192.0.2.1/udp/41000/quic-v1'],
        coordinationRelays: [],
        automaticRelayDiscovery: true,
        webRtcStunPolicy: { kind: 'default' },
      },
    }),
  );
  harness.pty.exit(0);

  const result = await management;
  assert.equal(result.kind === 'result' && result.status.automaticRelayDiscovery, true);
  assert.deepEqual(
    result.kind === 'result' ? result.status.webRtcStunPolicy : undefined,
    { kind: 'default' },
  );
  await harness.terminal.close();
});

test('sends a Mesh invitation only after the authenticated remote operator requests it', async () => {
  const harness = createHarness('pending');
  const invitation = JSON.stringify({ secret: 'one-time-mesh-secret' });
  const management = harness.terminal.runPeerMeshManagement({
    destination: 'operator@example.com',
    operator: OPERATOR,
    action: 'join',
    invitation,
    expectedTarget: {
      serviceId: 'b'.repeat(64),
      rootPath: '/srv/maka',
      rootId: 'a'.repeat(64),
      deploymentId: '00000000-0000-4000-8000-000000000001',
    },
  });
  await waitFor(() => harness.pty.hasDataListener());
  const command = harness.launchArgs.at(-1)?.at(-1) ?? '';
  assert.match(command, /mesh.*join.*--framed/u);
  assert.doesNotMatch(command, /one-time-mesh-secret/u);
  assert.deepEqual(harness.pty.writes, []);

  harness.pty.emitData(
    encodeRuntimeHostPeerMeshManagementFrame({ kind: 'input', action: 'join' }),
  );
  assert.deepEqual(harness.pty.writes, [`${invitation}\r`]);
  harness.pty.emitData(
    encodeRuntimeHostPeerMeshManagementFrame({
      kind: 'result',
      action: 'join',
      result: {
        localPeerId: 'peer-b',
        available: true,
        transit: {
          meshId: null,
          allowedMemberCount: 0,
          activeReservationCount: 0,
          activeCircuitCount: 0,
          maxReservationCount: 32,
          maxCircuitCount: 8,
          maxCircuitsPerPeer: 2,
          maxCircuitDurationSeconds: 7_200,
          maxCircuitBytes: 256 * 1024 * 1024,
        },
        meshes: [
          {
            meshId: 'mesh-id',
            role: 'member',
            authorityPeerId: 'peer-a',
            revision: 2,
            closed: false,
            members: [
              { peerId: 'peer-a', state: 'reachable', expiresAt: Date.now() + 60_000 },
              { peerId: 'peer-b', state: 'local' },
            ],
            pendingInvitationCount: 0,
          },
        ],
      },
    }),
  );
  harness.pty.exit(0);

  assert.equal((await management).kind, 'result');
  assert.doesNotMatch(JSON.stringify(harness.events), /one-time-mesh-secret/u);
  await harness.terminal.close();
});

test('rejects a framed service result for a different action', async () => {
  const harness = createHarness('pending');
  const management = harness.terminal.runServiceManagement({
    destination: 'operator@example.com',
    operator: OPERATOR,
    action: 'uninstall',
    expectedTarget: {
      serviceId: 'b'.repeat(64),
      rootPath: '/srv/maka',
      rootId: 'a'.repeat(64),
    },
  });
  await waitFor(() => harness.pty.hasDataListener());
  harness.pty.emitData(
    encodeRuntimeHostServiceManagementFrame({
      schemaVersion: 1,
      kind: 'result',
      action: 'status',
      service: {
        platform: 'linux',
        arch: 'x64',
        osRelease: '6.8.0',
        state: 'running',
        pid: 42,
        lastExitCode: 0,
        installedVersion: '1.2.3',
        projectDirectoryRoots: [],
      },
    }),
  );
  harness.pty.exit(0);

  await assert.rejects(management, /returned status for uninstall/u);
  await harness.terminal.close();
});

test('requires an absent operator deployment root to be absent', async () => {
  const harness = createHarness('pending');
  const cleanup = harness.terminal.cleanupManagedDeployment({
    destination: 'operator@example.com',
    operator: OPERATOR,
    expectedTarget: {
      serviceId: 'b'.repeat(64),
      rootPath: '/srv/maka',
      rootId: 'a'.repeat(64),
    },
  });
  await waitFor(() => harness.pty.hasDataListener());
  const remoteCommand = harness.launchArgs.at(-1)?.at(-1) ?? '';
  assert.match(remoteCommand, /if \[ ! -e/u);
  assert.doesNotMatch(remoteCommand, /rmdir --/u);
  assert.match(remoteCommand, /home\/operator\/\.local\/share\/maka/u);
  assert.match(remoteCommand, /__cleanup-managed-deployment/u);
  assert.match(remoteCommand, /--expected-service-id/u);
  assert.match(remoteCommand, /--expected-root-path/u);
  assert.match(remoteCommand, /--expected-root-id/u);
  harness.pty.exit(0);

  await cleanup;
  await harness.terminal.close();
});

test('does not launch a management process after the terminal owner closes', async () => {
  const launches: unknown[] = [];
  const terminal = createDesktopRuntimeHostSshTerminal({
    ipcMain: { handle: () => undefined, removeHandler: () => undefined },
    send: () => undefined,
    spawnPty: ((...args: unknown[]) => {
      launches.push(args);
      return new FakePty() as unknown as IPty;
    }) as typeof import('node-pty').spawn,
  });
  await terminal.close();
  await assert.rejects(
    terminal.runServiceManagement({
      destination: 'operator@example.com',
      operator: OPERATOR,
      action: 'status',
      expectedTarget: {
        serviceId: 'b'.repeat(64),
        rootPath: '/srv/maka',
        rootId: 'a'.repeat(64),
      },
    }),
    /terminal is closed/u,
  );
  assert.equal(launches.length, 0);
});

test('runs interactive operator activation as one strict framed SSH command', async () => {
  const harness = createHarness('pending');
  const rootId = 'a'.repeat(64);
  const activation = harness.terminal.activateSshOperator({
    destination: 'operator@example.com',
    operator: OPERATOR,
    rootId,
    interaction: 'terminal',
  });
  await waitFor(() => harness.pty.hasDataListener());
  harness.pty.emitData(
    encodeRuntimeHostActivationFrame({
      schemaVersion: 1,
      kind: 'result',
      deploymentId: '00000000-0000-4000-8000-000000000001',
      configRevision: 1,
      rootId,
      hostEpoch: 'host-epoch',
      pid: 1234,
      protocolVersion: 1,
      endpoint: { host: '127.0.0.1', port: 43_210, websocketPath: '/runtime-host' },
    }),
  );
  harness.pty.exit(0);

  assert.equal((await activation).pid, 1234);
  const remoteCommand = harness.launchArgs[0]?.at(-1) ?? '';
  assert.match(remoteCommand, /'activate' '--framed' '--root-id'/u);
  assert.match(remoteCommand, new RegExp(rootId, 'u'));
  assert.doesNotMatch(remoteCommand, /credential|token/u);
  await harness.terminal.close();
});

function createHarness(
  mode: 'pending' | 'exit',
  options: { readonly managementTimeoutMs?: number } = {},
) {
  const handlers = new Map<string, (...args: unknown[]) => unknown>();
  const events: Array<{ kind: string }> = [];
  const terminatedProcesses: Array<{ pid: number; signal: string }> = [];
  const pty = new FakePty();
  const launchArgs: string[][] = [];
  let releaseTunnel!: () => void;
  const tunnelReady = new Promise<void>((resolve) => {
    releaseTunnel = resolve;
  });
  const resource = { closed: pty.exited, close: async () => pty.exit(0) };
  const terminal = createDesktopRuntimeHostSshTerminal({
    ipcMain: {
      handle: (channel, handler) => handlers.set(channel, handler as (...args: unknown[]) => unknown),
      removeHandler: (channel) => handlers.delete(channel),
    },
    send: (_channel, event) => events.push(event),
    spawnPty: ((_file: string, args: string[]) => {
      launchArgs.push(args);
      return pty as unknown as IPty;
    }) as typeof import('node-pty').spawn,
    revealDelayMs: 0,
    ...options,
    processStopGraceMs: 1,
    terminateProcessTree: async ({ pid, signal, fallback, hasExited, beforeSignal }) => {
      terminatedProcesses.push({ pid, signal });
      await Promise.resolve();
      if (hasExited?.() || (beforeSignal && !(await beforeSignal()))) return false;
      fallback?.();
      return true;
    },
    openSshTunnel: async (input, overrides) => {
      const spawnProcess = overrides?.spawnProcess as RuntimeHostSshProcessFactory;
      const process = spawnProcess({ executable: 'ssh', args: [], interaction: input.interaction });
      if (mode === 'pending') {
        await tunnelReady;
        return { url: 'ws://127.0.0.1:50000/runtime-host', resource };
      }
      await process.exited;
      throw new Error('SSH exited');
    },
  });
  const invoke = (channel: string, ...args: unknown[]) => {
    const handler = handlers.get(channel);
    assert.ok(handler);
    return handler({}, ...args);
  };
  return {
    terminal,
    handlers,
    pty,
    launchArgs,
    releaseTunnel,
    eventKinds: () => events.map(({ kind }) => kind),
    events,
    terminatedProcesses,
    getSnapshot: () => invoke('runtime-host-ssh-terminal:getSnapshot'),
    cancel: (sessionId: string) => invoke('runtime-host-ssh-terminal:cancel', sessionId),
  };
}

function openTunnel(harness: ReturnType<typeof createHarness>) {
  return harness.terminal.openSshTunnel({
    destination: 'operator@example.com',
    remotePort: 7443,
    websocketPath: '/runtime-host',
    interaction: 'terminal',
  });
}

class FakePty {
  readonly pid = 42;
  readonly exited: Promise<void>;
  deferKill = false;
  exitOnForceKill = false;
  readonly killSignals: Array<string | undefined> = [];
  readonly writes: string[] = [];
  readonly #dataListeners = new Set<(data: string) => void>();
  readonly #exitListeners = new Set<(event: { exitCode: number; signal: number }) => void>();
  #resolveExit!: () => void;
  #exited = false;

  constructor() {
    this.exited = new Promise((resolve) => {
      this.#resolveExit = resolve;
    });
  }

  onData(listener: (data: string) => void) {
    this.#dataListeners.add(listener);
    return { dispose: () => this.#dataListeners.delete(listener) };
  }

  onExit(listener: (event: { exitCode: number; signal: number }) => void) {
    this.#exitListeners.add(listener);
    return { dispose: () => this.#exitListeners.delete(listener) };
  }

  emitData(data: string): void {
    for (const listener of this.#dataListeners) listener(data);
  }

  hasDataListener(): boolean {
    return this.#dataListeners.size > 0;
  }

  exit(code: number): void {
    if (this.#exited) return;
    this.#exited = true;
    for (const listener of this.#exitListeners) listener({ exitCode: code, signal: 0 });
    this.#resolveExit();
  }

  write(data: string): void {
    this.writes.push(data);
  }
  resize(): void {}
  kill(signal?: string): void {
    this.killSignals.push(signal);
    if (!this.deferKill || (this.exitOnForceKill && signal === 'SIGKILL')) this.exit(0);
  }
}

async function waitFor(predicate: () => boolean): Promise<void> {
  await pollFor(predicate, { attempts: 100, pollMs: 1, message: 'Condition was not reached' });
}

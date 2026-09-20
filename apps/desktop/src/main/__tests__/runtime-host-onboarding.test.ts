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
import { createDesktopRuntimeHostOnboarding } from '../runtime-host-onboarding.js';

import type { NativeSetupResult } from '../native-runtime-host-installer.js';

const installed: NativeSetupResult = {
  operator: { kind: 'native', platform: 'posix', executablePath: '/host/maka' },
  receipt: {
    deployment: { deploymentId: '00000000-0000-4000-8000-000000000001', configRevision: 1,
      rootId: 'a'.repeat(64), rootPath: '/host/state', executable: '/host/maka',
      sha256: 'b'.repeat(64), mode: 'on_demand', websocket: '127.0.0.1:0' },
    host: { hostEpoch: 'epoch', pid: 123, port: 4567 },
    pairing: { rootId: 'a'.repeat(64), credentialId: 'credential', credential: 'pairing-secret' },
  },
};

test('SSH and WSL onboarding use the verified native package without exposing credentials', async () => {
  for (const kind of ['ssh', 'wsl'] as const) {
    const operator = { kind: 'native' as const, platform: 'posix' as const,
      executablePath: '/home/operator/.local/share/Maka/native-cli/package/bin/maka' };
    let saved: unknown;
    const receipt = {
      deployment: { deploymentId: '00000000-0000-4000-8000-000000000001', configRevision: 1,
        rootId: 'a'.repeat(64), rootPath: '/home/operator/native-root', executable: '/host/maka',
        sha256: 'b'.repeat(64), mode: 'on_demand' as const, websocket: '127.0.0.1:0' },
      host: { hostEpoch: 'epoch', pid: 123, port: 4567 },
      ...(kind === 'ssh' ? { pairing: { rootId: 'a'.repeat(64), credentialId: 'credential', credential: 'pairing-secret' } } : {}),
    };
    const install = async (_input: unknown, commit: () => void) => { commit(); return { receipt, operator }; };
    const harness = createHarness({
      nativeSetup: {
        resolvePackage: async (identity) => ({
          artifact: { target: identity.platform === 'linux' ? 'linux-x64-gnu' : 'darwin-x64',
            version: '0.2.0', directory: '/verified/package', executable: '/verified/package/bin/maka',
            integrity: 'sha512-' + 'A'.repeat(86) + '==' }, receiptSha256: 'c'.repeat(64),
        }),
        ssh: kind === 'ssh' ? install : async () => assert.fail('wrong transport'),
        wsl: kind === 'wsl' ? install : async () => assert.fail('wrong transport'),
      },
      profiles: {
        addAndEnableVerified: async (input) => { saved = input; return { profileId: input.profile.id }; },
        addEnvironmentAndEnable: async (input) => { saved = input; return { profileId: input.profile.id }; },
      },
    });
    try {
      const result = await harness.invoke('runtime-host-onboarding:start', kind === 'ssh'
        ? { kind, destination: 'operator@example.com' } : { kind, distribution: 'Ubuntu' });
      assert.equal((result as { kind: string }).kind, 'complete');
      assert.ok(saved && typeof saved === 'object');
      assert.equal(Object.hasOwn(saved, 'managedService'), false);
      assert.match(JSON.stringify(saved), /native-cli\/package\/bin\/maka/u);
      assert.equal(Object.hasOwn(saved, 'credential'), kind === 'ssh');
      assert.doesNotMatch(JSON.stringify(harness.events), /pairing-secret|\/verified\/package/u);
    } finally { await harness.onboarding.close(); }
  }
});

test('projects WSL setup failures as recoverable onboarding state', async () => {
  const harness = createHarness({
    resolveWslTargetIdentity: async () => {
      throw new Error('Native Linux Runtime Host requires GNU libc');
    },
  });

  assert.deepEqual(
    await harness.invoke('runtime-host-onboarding:start', {
      kind: 'wsl',
      distribution: 'Ubuntu',
    }),
    {
      kind: 'failed',
      message: 'Native Linux Runtime Host requires GNU libc',
      revision: 3,
    },
  );
  await harness.onboarding.close();
});

test('projects invalid setup input as a recoverable failure', async () => {
  const harness = createHarness();

  const result = await harness.invoke('runtime-host-onboarding:start', {
    kind: 'ssh',
    destination: '',
  });
  assert.deepEqual(result, {
    kind: 'failed',
    message: 'Remote Runtime Host setup input is invalid',
    revision: 1,
  });
  await harness.invoke('runtime-host-onboarding:reset');
  assert.deepEqual(await harness.invoke('runtime-host-onboarding:getSnapshot'), {
    kind: 'idle',
    revision: 2,
  });
  await harness.onboarding.close();
});

test('rejects relative remote Project roots before starting SSH setup', async () => {
  const harness = createHarness();

  const result = await harness.invoke('runtime-host-onboarding:start', {
    kind: 'ssh',
    destination: 'operator@example.com',
    projectDirectoryRoots: [{ label: 'Work', path: 'srv/work' }],
  });
  assert.deepEqual(result, {
    kind: 'failed',
    message: 'Runtime Host Project directory is invalid',
    revision: 1,
  });
  await harness.onboarding.close();
});

test('finishes Host pairing after the cancellable SSH phase has completed', async () => {
  let finishPairing!: (value: { profileId: string }) => void;
  const pairing = new Promise<{ profileId: string }>((resolve) => {
    finishPairing = resolve;
  });
  let pairingStarted = false;
  let completeReceived = false;
  let finishSetup!: (value: NativeSetupResult) => void;
  const setupDrain = new Promise<NativeSetupResult>((resolve) => {
    finishSetup = resolve;
  });
  const harness = createHarness({
    profiles: {
      addAndEnableVerified: async () => {
        pairingStarted = true;
        return pairing;
      },
    },
    nativeSetup: {
      ssh: async (_input, onCommit) => {
        onCommit();
        completeReceived = true;
        return setupDrain;
      },
    },
  });

  const setup = harness.invoke('runtime-host-onboarding:start', {
    kind: 'ssh',
    destination: 'operator@example.com',
  }) as Promise<unknown>;
  while (!completeReceived) await Promise.resolve();
  assert.equal(await harness.invoke('runtime-host-onboarding:cancel'), false);

  finishSetup(installed);
  while (!pairingStarted) await Promise.resolve();

  finishPairing({ profileId: 'office' });
  assert.deepEqual(await setup, { kind: 'complete', profileId: 'office', revision: 6 });
  await harness.onboarding.close();
});

test('resolves the setup package only when onboarding starts', async () => {
  let resolutions = 0;
  const harness = createHarness({
    nativeSetup: {
      resolvePackage: async () => {
        resolutions += 1;
        throw new Error('Desktop does not declare an exact Runtime Host setup package');
      },
    },
  });

  assert.deepEqual(await harness.invoke('runtime-host-onboarding:getSnapshot'), {
    kind: 'idle',
    revision: 0,
  });
  assert.equal(resolutions, 0);
  assert.deepEqual(
    await harness.invoke('runtime-host-onboarding:start', {
      kind: 'ssh',
      destination: 'operator@example.com',
    }),
    {
      kind: 'failed',
      message: 'Desktop does not declare an exact Runtime Host setup package',
      revision: 4,
    },
  );
  assert.equal(resolutions, 1);
  await harness.onboarding.close();
});

type OnboardingInput = Parameters<typeof createDesktopRuntimeHostOnboarding>[0];
type HarnessOverrides = Partial<Omit<OnboardingInput, 'ipcMain' | 'send' | 'profiles' | 'nativeSetup'>> & {
  readonly profiles?: Partial<OnboardingInput['profiles']>;
  readonly nativeSetup?: Partial<OnboardingInput['nativeSetup']>;
};

function createHarness(overrides: HarnessOverrides = {}) {
  const handlers = new Map<string, (...args: unknown[]) => unknown>();
  const events: unknown[] = [];
  const { profiles, nativeSetup, ...rest } = overrides;
  const onboarding = createDesktopRuntimeHostOnboarding({
    clientInstanceId: 'stable-client',
    profiles: {
      addEnvironmentAndEnable: async () => assert.fail('profile must not be saved'),
      addAndEnableVerified: async () => assert.fail('profile must not be saved'),
      ...profiles,
    },
    resolveSshTargetIdentity: async () => ({ platform: 'darwin', architecture: 'x64' }),
    resolveWslTargetIdentity: async () => ({ platform: 'linux', architecture: 'x64', glibcVersion: '2.39' }),
    nativeSetup: {
      resolvePackage: async () => ({
        artifact: { target: 'darwin-x64', version: '0.2.0', directory: '/verified/package',
          executable: '/verified/package/bin/maka', integrity: 'sha512-' + 'A'.repeat(86) + '==' },
        receiptSha256: 'c'.repeat(64),
      }),
      ssh: async () => assert.fail('SSH must not start'),
      wsl: async () => assert.fail('WSL must not start'),
      ...nativeSetup,
    },
    listWslDistributions: async () => [],
    ...rest,
    ipcMain: {
      handle: (channel, handler) => handlers.set(channel, handler as (...args: unknown[]) => unknown),
      removeHandler: (channel) => handlers.delete(channel),
    },
    send: (snapshot) => events.push(snapshot),
  });
  return {
    onboarding,
    handlers,
    events,
    invoke(channel: string, ...args: unknown[]) {
      const handler = handlers.get(channel);
      assert.ok(handler);
      return handler({}, ...args);
    },
  };
}

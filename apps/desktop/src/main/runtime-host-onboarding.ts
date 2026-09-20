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

import { randomUUID } from 'node:crypto';
import type { IpcMain } from 'electron';
import type {
  DesktopRuntimeHostOnboardingInput,
  DesktopRuntimeHostOnboardingSnapshot,
} from '../preload/bridge-contract.js';
import type { DesktopRuntimeHostProfileService } from './runtime-host-profile-service.js';
import type { RuntimeHostTargetIdentity } from './runtime-host-target.js';
import { requireProjectDirectoryRoots } from '../shared/runtime-host-project-directory-policy.js';
import type { NativeRuntimeHostPackage } from './native-runtime-host-setup.js';
import type { NativeSetupInput, NativeSetupResult } from './native-runtime-host-installer.js';
import { NativeHostBudget } from './native-runtime-host-operation.js';

type OnboardingState = DesktopRuntimeHostOnboardingSnapshot extends infer Snapshot
  ? Snapshot extends DesktopRuntimeHostOnboardingSnapshot
    ? Omit<Snapshot, 'revision'>
    : never
  : never;

export function createDesktopRuntimeHostOnboarding(input: {
  readonly nativeSetup: {
    resolvePackage(identity: RuntimeHostTargetIdentity, signal?: AbortSignal, budget?: NativeHostBudget, onProgress?: (progress: import('../shared/native-runtime-host-management.js').NativeHostProgress) => void): Promise<NativeRuntimeHostPackage>;
    ssh(input: NativeSetupInput & { readonly destination: string; readonly sshPort?: number }, onCommit: () => void): Promise<NativeSetupResult>;
    wsl(input: NativeSetupInput & { readonly distribution: string }, onCommit: () => void): Promise<NativeSetupResult>;
  };
  readonly ipcMain: Pick<IpcMain, 'handle' | 'removeHandler'>;
  readonly clientInstanceId: string;
  readonly profiles: Pick<
    DesktopRuntimeHostProfileService,
    'addEnvironmentAndEnable' | 'addAndEnableVerified'
  >;
  readonly listWslDistributions: () => Promise<readonly string[]>;
  readonly resolveWslTargetIdentity: (input: {
    readonly distribution: string;
    readonly signal?: AbortSignal;
  }) => Promise<Extract<RuntimeHostTargetIdentity, { platform: 'linux' }>>;
  readonly send: (snapshot: DesktopRuntimeHostOnboardingSnapshot) => void;
  readonly resolveSshTargetIdentity: (input: {
    readonly destination: string;
    readonly sshPort?: number;
    readonly signal?: AbortSignal;
  }) => Promise<RuntimeHostTargetIdentity>;
}): { close(): Promise<void> } {
  let revision = 0;
  let snapshot: DesktopRuntimeHostOnboardingSnapshot = { kind: 'idle', revision };
  let active:
    | {
        readonly abort: AbortController;
        readonly task: Promise<DesktopRuntimeHostOnboardingSnapshot>;
        cancellable: boolean;
      }
    | undefined;

  const publish = (
    next: OnboardingState,
  ): DesktopRuntimeHostOnboardingSnapshot => {
    revision += 1;
    snapshot = { ...next, revision } as DesktopRuntimeHostOnboardingSnapshot;
    input.send(snapshot);
    return snapshot;
  };

  const start = (value: unknown): Promise<DesktopRuntimeHostOnboardingSnapshot> => {
    if (active) return active.task;
    let request: DesktopRuntimeHostOnboardingInput;
    try {
      request = requireOnboardingInput(value);
    } catch (error) {
      return Promise.resolve(publish({
        kind: 'failed',
        message: error instanceof Error ? error.message : String(error),
      }));
    }
    const abort = new AbortController();
    publish({ kind: 'running', phase: 'preparing_cli' });
    const task = Promise.resolve().then(() => run(request, abort.signal)).finally(() => {
      if (active?.task === task) active = undefined;
    });
    active = { abort, task, cancellable: true };
    return task;
  };

  const run = async (
    request: DesktopRuntimeHostOnboardingInput,
    signal: AbortSignal,
  ): Promise<DesktopRuntimeHostOnboardingSnapshot> => {
    try {
      const budget = new NativeHostBudget(180_000, signal);
      return await budget.wait(runNative(request, signal, input.nativeSetup, budget), 'Host setup');
    } catch (error) {
      if (signal.aborted) return publish({ kind: 'idle' });
      return publish({
        kind: 'failed',
        message: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const runNative = async (
    request: DesktopRuntimeHostOnboardingInput,
    signal: AbortSignal,
    native: typeof input.nativeSetup,
    budget: NativeHostBudget,
  ): Promise<DesktopRuntimeHostOnboardingSnapshot> => {
    publish({ kind: 'running', phase: request.kind === 'wsl' ? 'connecting_wsl' : 'connecting_ssh' });
    const identity = await budget.wait(request.kind === 'wsl'
      ? input.resolveWslTargetIdentity({ distribution: request.distribution, signal })
      : input.resolveSshTargetIdentity({ destination: request.destination, sshPort: request.sshPort, signal }), 'target identity');
    publish({ kind: 'running', phase: 'preparing_cli' });
    const onProgress = (progress: import('../shared/native-runtime-host-management.js').NativeHostProgress) => {
      if (signal.aborted || performance.now() >= budget.deadline) return;
      publish({ kind: 'running', phase: 'preparing_cli', progress });
    };
    const pkg = await budget.wait(native.resolvePackage(identity, signal, budget, onProgress), 'package download');
    signal.throwIfAborted();
    publish({ kind: 'running', phase: 'installing_service' });
    const setup = { package: pkg, projectDirectoryRoots: request.projectDirectoryRoots, signal, budget, onProgress };
    const commit = () => {
      if (active) active.cancellable = false;
      publish({ kind: 'running', phase: 'connecting_host' });
    };
    if (request.kind === 'wsl') {
      const complete = await native.wsl({ ...setup, distribution: request.distribution }, commit);
      // Installation is committed. Preserve its receipt even when the caller
      // has stopped observing; the profile journal owns subsequent recovery.
      if (complete.operator.platform !== 'posix') throw new Error('WSL setup did not return a Linux operator');
      const result = await input.profiles.addEnvironmentAndEnable({
        profile: {
          id: `environment-${randomUUID()}`, name: request.name?.trim() || request.distribution,
          kind: 'environment', provider: { kind: 'wsl', distribution: request.distribution },
          rootId: complete.receipt.deployment.rootId,
          operator: { ...complete.operator, platform: 'posix' },
        },
      });
      budget.remaining('profile confirmation');
      return publish({ kind: 'complete', profileId: result.profileId });
    }
    const complete = await native.ssh({ ...setup, destination: request.destination,
      sshPort: request.sshPort, principalId: `desktop:${input.clientInstanceId}` }, commit);
    // Do not discard an accepted pairing credential at the observation deadline.
    if (!complete.receipt.pairing) throw new Error('SSH setup did not return a pairing credential');
    const result = await input.profiles.addAndEnableVerified({
      profile: {
        id: `remote-${randomUUID()}`, name: request.name?.trim() || request.destination,
        kind: 'remote', rootId: complete.receipt.deployment.rootId,
        transport: { kind: 'ssh', destination: request.destination, sshPort: request.sshPort,
          activation: { kind: 'ssh_operator', operator: complete.operator } },
      },
      credential: complete.receipt.pairing.credential,
    });
    budget.remaining('profile confirmation');
    return publish({ kind: 'complete', profileId: result.profileId });
  };

  const channels = [
    'runtime-host-onboarding:getSnapshot',
    'runtime-host-onboarding:start',
    'runtime-host-onboarding:cancel',
    'runtime-host-onboarding:reset',
    'runtime-host-onboarding:listWslDistributions',
  ] as const;
  input.ipcMain.handle(channels[0], () => snapshot);
  input.ipcMain.handle(channels[1], (_event, value: unknown) => start(value));
  input.ipcMain.handle(channels[2], async () => {
    const current = active;
    if (!current) return true;
    if (!current.cancellable) return false;
    current.abort.abort();
    await current.task;
    return true;
  });
  input.ipcMain.handle(channels[3], async () => {
    if (snapshot.kind === 'running') return;
    await active?.task;
    publish({ kind: 'idle' });
  });
  input.ipcMain.handle(channels[4], () => input.listWslDistributions());

  return {
    close: async () => {
      for (const channel of channels) input.ipcMain.removeHandler(channel);
      const current = active;
      if (!current) return;
      if (current.cancellable) current.abort.abort();
      await current.task;
    },
  };
}

function requireOnboardingInput(value: unknown): DesktopRuntimeHostOnboardingInput {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('Remote Runtime Host setup input is invalid');
  }
  const input = value as Record<string, unknown>;
  if (input.name !== undefined &&
    (typeof input.name !== 'string' || input.name.trim().length > 128)) {
    throw new Error('Remote Runtime Host setup input is invalid');
  }
  const roots = input.projectDirectoryRoots === undefined
    ? undefined
    : requireProjectDirectoryRoots(input.projectDirectoryRoots);
  if (input.kind === 'wsl') {
    if (
      typeof input.distribution !== 'string' ||
      input.distribution.trim() !== input.distribution ||
      input.distribution.length === 0 ||
      input.distribution.length > 128
    ) throw new Error('WSL Runtime Host setup input is invalid');
    return {
      kind: 'wsl',
      distribution: input.distribution,
      ...(typeof input.name === 'string' && input.name.trim() ? { name: input.name.trim() } : {}),
      ...(roots ? { projectDirectoryRoots: roots } : {}),
    };
  }
  if (
    input.kind !== 'ssh' ||
    typeof input.destination !== 'string' ||
    input.destination.trim() !== input.destination ||
    input.destination.length === 0 ||
    input.destination.length > 512 ||
    (input.sshPort !== undefined &&
      (!Number.isInteger(input.sshPort) || Number(input.sshPort) < 1 || Number(input.sshPort) > 65_535))
  ) throw new Error('Remote Runtime Host setup input is invalid');
  return {
    kind: 'ssh',
    destination: input.destination,
    ...(typeof input.name === 'string' && input.name.trim() ? { name: input.name.trim() } : {}),
    ...(input.sshPort === undefined ? {} : { sshPort: Number(input.sshPort) }),
    ...(roots ? { projectDirectoryRoots: roots } : {}),
  };
}

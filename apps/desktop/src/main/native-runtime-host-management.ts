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


import { decodeRuntimeHostActivationFrame } from '@maka/runtime-host/operator';
import { z } from 'zod';
import {
  nativeRuntimeHostDeploymentSchema,
  decodeNativeRuntimeHostDeploymentStatus,
  type NativeRuntimeHostDeployment,
} from '../shared/native-runtime-host-deployment.js';
import {
  nativeRuntimeHostLogsSchema,
  nativeRuntimeHostManagementRequestSchema,
  nativeRuntimeHostMutationSchema,
  type NativeRuntimeHostExpected,
  type NativeRuntimeHostManagementRequest,
  type NativeRuntimeHostManagementResult,
  type NativeRuntimeHostMutation,
  type NativeRuntimeHostSettings,
} from '../shared/native-runtime-host-management.js';
import { runNativeRuntimeHostCommand, type NativeRuntimeHostOperator } from './native-runtime-host-command.js';

export interface NativeRuntimeHostChangeScope {
  /** Retain the pause for stop/uninstall, including an unconfirmed outcome. */
  hold(): void;
  retire(deployment: NativeRuntimeHostDeployment | null, hostEpoch?: string,
    prepareRemote?: (connectionId: string) => Promise<boolean>): Promise<boolean>;
  /** Resume after the command settles, including failure; activation reads native authority. */
  resumeOnSettled(): void;
}

export function createNativeRuntimeHostManagement(input: {
  readonly operator: NativeRuntimeHostOperator;
  readonly rootId: string;
  readonly rootPath?: string;
  readonly change: <T>(run: (scope: NativeRuntimeHostChangeScope) => Promise<T>) => Promise<T>;
}) {
  const execute = (args: string[], readOnly = false) => runNativeRuntimeHostCommand(input.operator, args, readOnly);
  const rooted = (action: string) => [action, '--root-id', input.rootId];
  const read = async () => decodeNativeRuntimeHostDeploymentStatus(
    JSON.parse(await execute(rooted('status'), true)), input.rootId,
  );
  const requireCurrent = async (expected: NativeRuntimeHostExpected) => {
    const status = await read();
    if (status.kind !== 'installed' || status.deployment.deploymentId !== expected.deploymentId ||
      status.deployment.configRevision !== expected.configRevision) {
      throw new Error('Native Host deployment changed; refresh before applying this operation');
    }
    return status;
  };
  const activate = async (deployment: NativeRuntimeHostDeployment): Promise<NativeRuntimeHostMutation> => {
    const receipt = decodeRuntimeHostActivationFrame((await execute([...rooted('activate'), '--framed'])).trim());
    if (!receipt || receipt.kind !== 'result' || receipt.rootId !== input.rootId ||
      receipt.deploymentId !== deployment.deploymentId ||
      receipt.configRevision !== deployment.configRevision) {
      throw new Error('Native Host activation changed; refresh deployment status');
    }
    return { kind: 'ready', deployment, host: {
      hostEpoch: receipt.hostEpoch, pid: receipt.pid, port: receipt.endpoint.port,
    } };
  };
  const retire = (scope: NativeRuntimeHostChangeScope, deployment: NativeRuntimeHostDeployment, hostEpoch?: string) =>
    scope.retire(deployment, hostEpoch, async (connectionId) => {
      if (!hostEpoch) throw new Error('Native Host epoch is unavailable; refresh before retiring it');
      const result = z.discriminatedUnion('kind', [
        z.object({ kind: z.literal('active_tasks') }).strict(),
        z.object({ kind: z.literal('prepared'), pid: z.number().int().positive().max(0xffff_ffff) }).strict(),
      ]).parse(JSON.parse(await execute([
        'retire', '--root', deployment.rootPath, '--expected-host-epoch', hostEpoch,
        '--handoff-connection-id', connectionId,
      ])));
      return result.kind === 'prepared';
    });
  const mutate = async (
    action: 'stop' | 'restart' | 'uninstall' | 'update' | 'reconcile',
    current: NativeRuntimeHostDeployment,
    settings?: NativeRuntimeHostSettings,
  ) => {
    const raw = await execute([
      ...rooted(action),
      '--expected-deployment-id', current.deploymentId,
      '--expected-revision', String(current.configRevision),
      ...settingsArguments(settings),
    ]);
    const result = nativeRuntimeHostMutationSchema.parse(JSON.parse(raw));
    validateMutation(action, current, result);
    return result;
  };
  const change = async (
    request: Exclude<NativeRuntimeHostManagementRequest, { action: 'status' | 'logs' }>,
    scope: NativeRuntimeHostChangeScope,
  ): Promise<NativeRuntimeHostMutation | { kind: 'active_tasks' }> => {
    if (request.action === 'start' || request.action === 'install') {
      let status = await read();
      if (request.action === 'install') {
        if (status.kind === 'incomplete' ||
          (status.kind === 'installed' && status.deployment.admission !== 'revoked')) {
          throw new Error('Native Host is already installed or incomplete; refresh deployment status');
        }
        const rootPath = status.kind === 'installed' ? status.deployment.rootPath : input.rootPath;
        if (!rootPath) throw new Error('Native Host installation requires a known Root path');
        scope.resumeOnSettled();
        if (!await scope.retire(null)) return { kind: 'active_tasks' };
        scope.hold();
        const installed = nativeRuntimeHostDeploymentSchema.parse(JSON.parse(await execute([
          'install', '--root', rootPath, ...settingsArguments(request.settings),
        ])));
        if (installed.rootId !== input.rootId || installed.admission !== undefined) {
          throw new Error('Native Host install returned another Root or revoked deployment');
        }
        status = await read();
        if (status.kind !== 'installed' ||
          status.deployment.deploymentId !== installed.deploymentId ||
          status.deployment.configRevision !== installed.configRevision) {
          throw new Error('Native Host installation changed; refresh deployment status');
        }
      }
      if (status.kind !== 'installed' || status.deployment.admission !== undefined) {
        throw new Error('Install a native Host deployment before starting it');
      }
      scope.resumeOnSettled();
      scope.hold();
      return activate(status.deployment);
    }

    const observed = await requireCurrent(request.expected);
    const current = observed.deployment;
    const hostEpoch = observed.host.kind === 'connected' ? observed.host.identity.hostEpoch : undefined;
    if (request.action === 'update' || request.action === 'reconcile') {
      // Stage under native authority while the old Host is still serving.
      scope.resumeOnSettled();
      scope.hold();
      const staged = await mutate(request.action, current,
        request.action === 'update' ? request.settings : undefined);
      if (staged.kind === 'ready') {
        return staged;
      }
      if (staged.kind !== 'active_tasks' || !staged.target) {
        throw new Error('Native Host did not confirm a pending update');
      }
      if (!await retire(scope, current, hostEpoch)) return staged;
      return mutate('reconcile', current);
    }

    if (request.action === 'restart') scope.resumeOnSettled();
    if (!await retire(scope, current, hostEpoch)) return { kind: 'active_tasks', deployment: current };
    scope.hold();
    return mutate(request.action, current);
  };
  return {
    async run(value: unknown): Promise<NativeRuntimeHostManagementResult> {
      const request = nativeRuntimeHostManagementRequestSchema.parse(value);
      if (request.action === 'status') return { status: await read() };
      if (request.action === 'logs') {
        const logs = nativeRuntimeHostLogsSchema.parse(JSON.parse(await execute(rooted('logs'), true)));
        return { status: await read(), logs };
      }
      return input.change(async (scope) => {
        const outcome = await change(request, scope);
        return { status: await read(), outcome };
      });
    },
  };
}

function settingsArguments(settings?: NativeRuntimeHostSettings): string[] {
  if (!settings) return [];
  const args: string[] = [];
  if (settings.mode !== undefined) {
    args.push('--mode', settings.mode === 'on_demand' ? 'on-demand' : 'supervised');
  }
  if (settings.websocket !== undefined) args.push('--websocket', settings.websocket);
  const roots = settings.projectDirectoryRoots;
  if (roots === null) args.push('--default-project-roots');
  else if (roots?.length === 0) args.push('--no-project-roots');
  else for (const root of roots ?? []) args.push('--project-root-json', JSON.stringify(root));
  return args;
}

function validateMutation(
  action: 'stop' | 'restart' | 'uninstall' | 'update' | 'reconcile',
  current: NativeRuntimeHostDeployment,
  result: NativeRuntimeHostMutation,
): void {
  const deployment = result.deployment;
  const sameIdentity = (value: NativeRuntimeHostDeployment) =>
    value.rootId === current.rootId && value.rootPath === current.rootPath &&
    value.deploymentId === current.deploymentId;
  const nextRevision = current.configRevision + 1;
  if (!sameIdentity(deployment)) throw new Error('Native Host operation returned another deployment');
  switch (result.kind) {
    case 'active_tasks':
      if (deployment.configRevision !== current.configRevision ||
        ((action === 'update' || action === 'reconcile') !== (result.target !== undefined)) ||
        (result.target && (!sameIdentity(result.target) ||
          result.target.configRevision !== nextRevision || result.target.admission !== undefined))) break;
      return;
    case 'stopped':
      if (action !== 'stop' || deployment.configRevision !== current.configRevision ||
        deployment.admission !== undefined) break;
      return;
    case 'ready':
      if (action !== 'restart' && action !== 'update' && action !== 'reconcile') break;
      if (deployment.admission !== undefined ||
        (deployment.configRevision !== current.configRevision &&
          (action === 'restart' || deployment.configRevision !== nextRevision))) break;
      return;
    case 'unregistered':
      if (action !== 'uninstall' || deployment.admission !== 'revoked' ||
        deployment.configRevision !== (current.admission === 'revoked' ? current.configRevision : nextRevision)) break;
      return;
  }
  throw new Error('Native Host operation returned an unexpected revision or outcome');
}

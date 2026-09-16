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


import { spawn } from 'node:child_process';
import { decodeRuntimeHostActivationFrame } from '@maka/runtime-host/operator';
import {
  nativeRuntimeHostDeploymentSchema,
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
import { readNativeRuntimeHostDeployment } from './native-runtime-host-deployment.js';

export interface NativeRuntimeHostChangeScope {
  /** Once a mutation may have run, reconnect stays paused until confirmed ready. */
  hold(): void;
  retire(deployment: NativeRuntimeHostDeployment | null): Promise<boolean>;
  resumeOnSuccess(): void;
}

export function createNativeRuntimeHostManagement(input: {
  readonly executable: string;
  readonly rootId: string;
  readonly rootPath: string;
  readonly change: <T>(run: (scope: NativeRuntimeHostChangeScope) => Promise<T>) => Promise<T>;
}) {
  const read = () => readNativeRuntimeHostDeployment(input.executable, input.rootId);
  const execute = (args: string[]) => runNativeRuntimeHostCommand(input.executable, ['host', ...args]);
  const rooted = (action: string) => [action, '--root-id', input.rootId];
  const requireCurrent = async (expected: NativeRuntimeHostExpected) => {
    const status = await read();
    if (status.kind !== 'installed' || status.deployment.deploymentId !== expected.deploymentId ||
      status.deployment.configRevision !== expected.configRevision) {
      throw new Error('Native Host deployment changed; refresh before applying this operation');
    }
    return status.deployment;
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
        if (!await scope.retire(null)) return { kind: 'active_tasks' };
        scope.hold();
        const installed = nativeRuntimeHostDeploymentSchema.parse(JSON.parse(await execute([
          'install', '--root', input.rootPath, ...settingsArguments(request.settings),
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
      scope.hold();
      const result = await activate(status.deployment);
      scope.resumeOnSuccess();
      return result;
    }

    const current = await requireCurrent(request.expected);
    if (request.action === 'update' || request.action === 'reconcile') {
      // Stage under native authority while the old Host is still serving.
      scope.hold();
      const staged = await mutate(request.action, current,
        request.action === 'update' ? request.settings : undefined);
      if (staged.kind === 'ready') {
        scope.resumeOnSuccess();
        return staged;
      }
      if (staged.kind !== 'active_tasks' || !staged.target) {
        throw new Error('Native Host did not confirm a pending update');
      }
      if (!await scope.retire(current)) return staged;
      const result = await mutate('reconcile', current);
      if (result.kind === 'ready') scope.resumeOnSuccess();
      return result;
    }

    if (!await scope.retire(current)) return { kind: 'active_tasks', deployment: current };
    scope.hold();
    const result = await mutate(request.action, current);
    if (result.kind === 'ready') scope.resumeOnSuccess();
    return result;
  };
  return {
    async run(value: unknown): Promise<NativeRuntimeHostManagementResult> {
      const request = nativeRuntimeHostManagementRequestSchema.parse(value);
      if (request.action === 'status') return { status: await read() };
      if (request.action === 'logs') {
        const logs = nativeRuntimeHostLogsSchema.parse(JSON.parse(await execute(rooted('logs'))));
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

/** Drain and reap even on excess output: never kill an in-flight durable mutation. */
function runNativeRuntimeHostCommand(executable: string, args: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    const child = spawn(executable, args, { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    const output: Buffer[] = [];
    let bytes = 0;
    let errorText = '';
    child.stdout.on('data', (chunk: Buffer) => {
      bytes += chunk.length;
      if (bytes <= 256 * 1024) output.push(chunk);
    });
    child.stderr.setEncoding('utf8');
    child.stderr.on('data', (chunk: string) => { errorText = (errorText + chunk).slice(-8192); });
    child.once('error', reject);
    child.once('close', (code) => {
      if (code !== 0 || bytes > 256 * 1024) {
        reject(new Error(`Native Host operation was not confirmed; refresh status before retrying. ${errorText.trim()}`));
      } else resolve(Buffer.concat(output).toString('utf8'));
    });
  });
}

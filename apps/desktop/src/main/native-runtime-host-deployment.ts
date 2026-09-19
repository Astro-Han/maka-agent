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

import { resolveStorageRoot } from '@maka/storage/root-authority';
import {
  connectExistingRuntimeHost,
  type ConnectOrSpawnRuntimeHostInput,
  type ConnectOrSpawnRuntimeHostResult,
} from '@maka/runtime-host/client';
import { decodeRuntimeHostActivationFrame } from '@maka/runtime-host/operator';
import {
  decodeNativeRuntimeHostDeploymentStatus,
  type NativeRuntimeHostDeploymentStatus,
} from '../shared/native-runtime-host-deployment.js';
import { runNativeRuntimeHostCommand, NativeHostBusyError, NativeHostCommandUnconfirmedError } from './native-runtime-host-command.js';
import { NativeHostBudget, NativeHostWaitError } from './native-runtime-host-operation.js';

type Route = { rootId: string; status: NativeRuntimeHostDeploymentStatus };
type Input = ConnectOrSpawnRuntimeHostInput;
type Result = ConnectOrSpawnRuntimeHostResult;

async function observeConnection<T extends { kind: string; connection?: { close(): Promise<void> } }>(
  start: () => Promise<T>, budget: NativeHostBudget,
): Promise<T> {
  budget.remaining('connect');
  const pending = start();
  try { return await budget.wait(pending, 'connect'); }
  catch (error) {
    // A late transport is still ours to close, not evidence that its Host may
    // be killed. Keep its result observed after the caller has left.
    void pending.then((late) => late.kind === 'connected' ? late.connection?.close() : undefined, () => undefined).catch(() => undefined);
    throw error;
  }
}

export async function readNativeRuntimeHostDeployment(
  executable: string,
  rootId: string,
  budget?: NativeHostBudget,
): Promise<NativeRuntimeHostDeploymentStatus> {
  const stdout = await runNativeRuntimeHostCommand(
    { kind: 'local', executable }, ['status', '--root-id', rootId], true, budget,
  );
  return decodeNativeRuntimeHostDeploymentStatus(JSON.parse(stdout), rootId);
}

/** Routes are hints only. Every connection verifies the live Root and generation. */
export function createNativeRuntimeHostDeploymentConnector(input: {
  readonly executable: string;
  readonly initialize?: (budget: NativeHostBudget) => Promise<unknown>;
  readonly activate: (rootId: string, budget: NativeHostBudget) => Promise<string>;
  readonly connectUnmanaged: (input: Input) => Promise<Result>;
}): (request: Input) => Promise<Result> {
  const routes = new Map<string, Promise<Route>>();
  const route = (rootPath: string, budget: NativeHostBudget, refresh = false): Promise<Route> => {
    if (refresh) routes.delete(rootPath);
    let pending = routes.get(rootPath);
    if (!pending) {
      pending = budget.wait((async () => {
        const root = await resolveStorageRoot({ path: rootPath, kind: 'interactive' });
        const status = await readNativeRuntimeHostDeployment(input.executable, root.rootId, budget);
        return { rootId: root.rootId, status };
      })(), 'deployment status');
      routes.set(rootPath, pending);
      const selected = pending;
      const forget = () => {
        if (routes.get(rootPath) === selected) routes.delete(rootPath);
      };
      void pending.then(({ status }) => {
        if (status.kind === 'incomplete' ||
          (status.kind === 'installed' && status.deployment.admission)) forget();
      }, forget);
    }
    return pending;
  };

  const connectManaged = async (request: Input, selected: Route, budget: NativeHostBudget): Promise<Result> => {
    const connect = () => connectExistingRuntimeHost({
      ...request,
      generation: undefined,
      takeoverHostEpoch: undefined,
    });
    for (let attempt = 0; attempt < 2; attempt++) {
      request.signal?.throwIfAborted();
      if (selected.status.kind !== 'installed' || selected.status.deployment.admission) {
        throw new Error('Native Host deployment is absent, incomplete or uninstalled');
      }
      const deployment = selected.status.deployment;
      const expected = `${deployment.deploymentId}:${deployment.configRevision}`;
      const existing = await observeConnection(connect, budget);
      if (existing.kind === 'connected' || existing.kind === 'incompatible' ||
        existing.kind === 'upgrade_required') {
        if (existing.registration.rootId !== selected.rootId ||
          existing.registration.generation !== expected ||
          existing.registration.lifecycleMode !==
            (deployment.mode === 'on_demand' ? 'ephemeral' : 'service')) {
          if (existing.kind === 'connected') await budget.wait(existing.connection.close(), 'connection cleanup');
          selected = await budget.wait(route(request.rootPath, budget, true), 'deployment status');
          continue;
        }
        if (request.signal?.aborted) {
          if (existing.kind === 'connected') await budget.wait(existing.connection.close(), 'connection cleanup');
          request.signal.throwIfAborted();
        }
        return {
          ...existing,
          managedDeployment: {
            deploymentId: deployment.deploymentId,
            configRevision: deployment.configRevision,
          },
        };
      }

      request.signal?.throwIfAborted();
      // Activation owns its own admission/election. A cancelled Desktop must
      // collect the receipt, not kill a process halfway through OS activation.
      let frame: string;
      try {
        frame = await budget.wait(input.activate(selected.rootId, budget), 'activate');
      } catch (error) {
        if (!(error instanceof NativeHostBusyError || error instanceof NativeHostCommandUnconfirmedError || error instanceof NativeHostWaitError)) throw error;
        // Activation is idempotent, but a lost reply is not permission to
        // replay it blindly. Re-observe the durable generation and its lease.
        selected = await route(request.rootPath, budget, true);
        if (selected.status.kind === 'installed' && selected.status.operation === 'in_progress') throw new NativeHostBusyError();
        if (attempt === 1 || selected.status.kind !== 'installed' ||
          selected.status.operation !== 'idle' ||
          (!selected.status.pendingUpdate && selected.status.host.kind !== 'connected')) throw error;
        continue;
      }
      const receipt = decodeRuntimeHostActivationFrame(frame.trim());
      request.signal?.throwIfAborted();
      if (!receipt || receipt.kind !== 'result' || receipt.rootId !== selected.rootId) {
        throw new Error('Native Host activation did not return this Root');
      }
      const activated = await observeConnection(connect, budget);
      if (activated.kind !== 'connected') {
        throw new Error('Activated native Host is not connectable');
      }
      const registration = activated.registration;
      if (request.signal?.aborted ||
        activated.connection.rootId !== receipt.rootId ||
        activated.connection.hostEpoch !== receipt.hostEpoch ||
        registration.pid !== receipt.pid ||
        registration.generation !== `${receipt.deploymentId}:${receipt.configRevision}`) {
        await budget.wait(activated.connection.close(), 'connection cleanup');
        request.signal?.throwIfAborted();
        throw new Error('Native Host changed after activation');
      }
      // A newer authority may have won while activation acquired the executor.
      // Do not manufacture its full configuration from this live receipt.
      if (receipt.deploymentId !== deployment.deploymentId ||
        receipt.configRevision !== deployment.configRevision) routes.delete(request.rootPath);
      return {
        ...activated,
        managedDeployment: {
          deploymentId: receipt.deploymentId,
          configRevision: receipt.configRevision,
        },
      };
    }
    throw new Error('Native Host deployment changed during connection; reconnect');
  };

  return async (request) => {
    const budget = new NativeHostBudget(request.connectTimeoutMs ?? 30_000, request.signal);
    request.signal?.throwIfAborted();
    if (input.initialize) await budget.wait(input.initialize(budget), 'root initialization');
    const selected = await budget.wait(route(request.rootPath, budget), 'deployment status');
    request.signal?.throwIfAborted();
    if (selected.status.kind !== 'not_installed') return connectManaged(request, selected, budget);
    const result = await observeConnection(() => input.connectUnmanaged(request), budget);
    if (result.kind === 'connected' && result.registration.lifecycleMode === 'ephemeral' &&
      result.registration.generation === request.generation) return result;
    // Installation can happen after this Desktop starts. Failure is never a
    // reason to bypass native admission with another bundled candidate.
    let refreshed: Route;
    try {
      refreshed = await budget.wait(route(request.rootPath, budget, true), 'deployment status');
    } catch (error) {
      if (result.kind === 'connected') await budget.wait(result.connection.close(), 'connection cleanup');
      throw error;
    }
    if (refreshed.status.kind === 'not_installed') return result;
    if (result.kind === 'connected') await budget.wait(result.connection.close(), 'connection cleanup');
    return connectManaged(request, refreshed, budget);
  };
}

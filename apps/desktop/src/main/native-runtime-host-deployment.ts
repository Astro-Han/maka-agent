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

import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
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

const run = promisify(execFile);
type Route = { rootId: string; status: NativeRuntimeHostDeploymentStatus };
type Input = ConnectOrSpawnRuntimeHostInput;
type Result = ConnectOrSpawnRuntimeHostResult;

export async function readNativeRuntimeHostDeployment(
  executable: string,
  rootId: string,
): Promise<NativeRuntimeHostDeploymentStatus> {
  const { stdout } = await run(executable, ['host', 'status', '--root-id', rootId], {
    windowsHide: true,
    maxBuffer: 256 * 1024,
    timeout: 15_000,
  });
  return decodeNativeRuntimeHostDeploymentStatus(JSON.parse(stdout), rootId);
}

/** Routes are hints only. Every connection verifies the live Root and generation. */
export function createNativeRuntimeHostDeploymentConnector(input: {
  readonly executable: string;
  readonly activate: (rootId: string) => Promise<string>;
  readonly connectUnmanaged: (input: Input) => Promise<Result>;
}): (request: Input) => Promise<Result> {
  const routes = new Map<string, Promise<Route>>();
  const route = (rootPath: string, refresh = false): Promise<Route> => {
    if (refresh) routes.delete(rootPath);
    let pending = routes.get(rootPath);
    if (!pending) {
      pending = (async () => {
        const root = await resolveStorageRoot({ path: rootPath, kind: 'interactive' });
        const status = await readNativeRuntimeHostDeployment(input.executable, root.rootId);
        return { rootId: root.rootId, status };
      })();
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

  const connectManaged = async (request: Input, selected: Route): Promise<Result> => {
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
      const existing = await connect();
      if (existing.kind === 'connected' || existing.kind === 'incompatible' ||
        existing.kind === 'upgrade_required') {
        if (existing.registration.rootId !== selected.rootId ||
          existing.registration.generation !== expected ||
          existing.registration.lifecycleMode !==
            (deployment.mode === 'on_demand' ? 'ephemeral' : 'service')) {
          if (existing.kind === 'connected') await existing.connection.close();
          selected = await route(request.rootPath, true);
          continue;
        }
        if (request.signal?.aborted) {
          if (existing.kind === 'connected') await existing.connection.close();
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
      const receipt = decodeRuntimeHostActivationFrame(
        (await input.activate(selected.rootId)).trim(),
      );
      request.signal?.throwIfAborted();
      if (!receipt || receipt.kind !== 'result' || receipt.rootId !== selected.rootId) {
        throw new Error('Native Host activation did not return this Root');
      }
      const activated = await connect();
      if (activated.kind !== 'connected') {
        throw new Error('Activated native Host is not connectable');
      }
      const registration = activated.registration;
      if (request.signal?.aborted ||
        activated.connection.rootId !== receipt.rootId ||
        activated.connection.hostEpoch !== receipt.hostEpoch ||
        registration.pid !== receipt.pid ||
        registration.generation !== `${receipt.deploymentId}:${receipt.configRevision}`) {
        await activated.connection.close();
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
    request.signal?.throwIfAborted();
    const selected = await route(request.rootPath);
    request.signal?.throwIfAborted();
    if (selected.status.kind !== 'not_installed') return connectManaged(request, selected);
    const result = await input.connectUnmanaged(request);
    if (result.kind === 'connected' && result.registration.lifecycleMode === 'ephemeral' &&
      result.registration.generation === request.generation) return result;
    // Installation can happen after this Desktop starts. Failure is never a
    // reason to bypass native admission with another bundled candidate.
    let refreshed: Route;
    try {
      refreshed = await route(request.rootPath, true);
    } catch (error) {
      if (result.kind === 'connected') await result.connection.close();
      throw error;
    }
    if (refreshed.status.kind === 'not_installed') return result;
    if (result.kind === 'connected') await result.connection.close();
    return connectManaged(request, refreshed);
  };
}

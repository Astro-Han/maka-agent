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

import {
  connectOrSpawnRuntimeHostWithDependencies,
  launchNativeRuntimeHostCandidate,
  createRuntimeHostCandidateLaunchBarrierWithDependencies,
  type RuntimeHostCandidateLaunchBarrier,
} from '@maka/runtime-host/client';
import { createNativeRuntimeHostDeploymentConnector } from './native-runtime-host-deployment.js';
import { runNativeRuntimeHostCommand } from './native-runtime-host-command.js';
import { NativeHostBudget } from './native-runtime-host-operation.js';

export async function initializeNativeRuntimeHost(
  executable: string,
  rootPath: string,
  signal?: AbortSignal,
  budget = new NativeHostBudget(20_000, signal),
): Promise<void> {
  await runNativeRuntimeHostCommand({ kind: 'local', executable }, ['init', '--root', rootPath], false, budget);
}

/** Only the process launcher changes; discovery, election and client protocol stay shared. */
export function createNativeRuntimeHostCandidateLaunchBarrier(
  executable: string,
  initialize?: (budget: NativeHostBudget) => Promise<unknown>,
): RuntimeHostCandidateLaunchBarrier {
  const candidates = createRuntimeHostCandidateLaunchBarrierWithDependencies({
    retireTimeoutMs: 1000,
    launchCandidate: (input) => launchNativeRuntimeHostCandidate(executable, input),
    connect(input, launchCandidate) {
      // Startup initializes the root once. Every connection still verifies its
      // identity through discovery; reconnecting needs no initializer process.
      return connectOrSpawnRuntimeHostWithDependencies(input, {
        launchCandidate,
        random: Math.random,
      });
    },
  });
  let launchesAllowed = true;
  let released = false;
  const activations = new Map<string, Promise<string>>();
  const connect = createNativeRuntimeHostDeploymentConnector({
    executable,
    initialize,
    connectUnmanaged: (input) => candidates.connect(input),
    activate(rootId, budget) {
      if (!launchesAllowed) throw new Error('Native Host launches are paused');
      const existing = activations.get(rootId);
      if (existing) return existing;
      const activation = runNativeRuntimeHostCommand(
        { kind: 'local', executable }, ['activate', '--root-id', rootId, '--framed'], false, budget,
      );
      activations.set(rootId, activation);
      void activation.finally(() => {
        if (activations.get(rootId) === activation) activations.delete(rootId);
      }).catch(() => undefined);
      return activation;
    },
  });
  return {
    connect(input) {
      if (!launchesAllowed) return Promise.reject(new Error('Native Host launches are paused'));
      return connect(input);
    },
    pause() {
      candidates.pause();
      launchesAllowed = false;
    },
    async retireExcept(protectedPid) {
      if (launchesAllowed) throw new Error('Native Host launches must be paused before retirement');
      const budget = new NativeHostBudget(5_000);
      await budget.wait(Promise.allSettled(activations.values()), 'activation settlement');
      await budget.wait(candidates.retireExcept(protectedPid), 'candidate retirement');
    },
    resume() {
      if (released) return;
      candidates.resume();
      launchesAllowed = true;
    },
    release() {
      released = true;
      launchesAllowed = false;
      candidates.release();
    },
  };
}

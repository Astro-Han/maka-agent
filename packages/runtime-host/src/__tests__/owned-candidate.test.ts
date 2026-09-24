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
import test from 'node:test';
import { mkdtemp, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { connectOwnedRuntimeHostWithDependencies } from '../client/connect-or-spawn.js';
import type { OwnedCandidateAttempt } from '../client/launcher.js';
import {
  INTERACTIVE_RUNTIME_HOST_COMPOSITION_ID,
  RUNTIME_HOST_PROTOCOL_VERSION,
} from '../protocol/index.js';

function input(rootPath: string) {
  return {
    rootPath,
    candidateExecutable: '/native/maka',
    protocol: { min: RUNTIME_HOST_PROTOCOL_VERSION, max: RUNTIME_HOST_PROTOCOL_VERSION },
    compositionId: INTERACTIVE_RUNTIME_HOST_COMPOSITION_ID,
    electionDeadlineMs: 250,
  };
}

test('owned startup configures the native root before election and releases a late candidate', {
  timeout: 5_000,
}, async () => {
  const rootPath = await mkdtemp(join(tmpdir(), 'maka-owned-election-'));
  const canonicalPath = await realpath(rootPath);
  let initialized = false;
  let launched = 0;
  let released = 0;
  let settled = 0;
  let pending = true;
  let resolveSpawned!: (candidate: OwnedCandidateAttempt) => void;
  const spawned = new Promise<OwnedCandidateAttempt>((resolve) => {
    resolveSpawned = resolve;
  });
  const candidate: OwnedCandidateAttempt = {
    pid: 4242,
    releaseToEnvironment() {
      released++;
    },
    async settle() {
      settled++;
      return true;
    },
  };
  try {
    const result = await connectOwnedRuntimeHostWithDependencies(input(rootPath), {
      async initializeRoot(executable, root) {
        assert.equal(executable, '/native/maka');
        assert.equal(root, rootPath);
        initialized = true;
      },
      launchCandidate(executable, candidate) {
        assert.equal(initialized, true);
        assert.equal(executable, '/native/maka');
        assert.equal(candidate.rootPath, canonicalPath);
        launched++;
        return { spawned };
      },
    });
    assert.equal(launched, 1, JSON.stringify(result));
    assert.equal(result.kind, 'failed');
    if (result.kind === 'failed') assert.equal(result.reason, 'startup_timeout');
    resolveSpawned(candidate);
    pending = false;
    await spawned;
    assert.equal(released, 1);
    assert.equal(settled, 0, 'a missed reply cannot authorize killing a late owner');
  } finally {
    if (pending) resolveSpawned(candidate);
    await rm(rootPath, { recursive: true, force: true });
  }
});

test('failed native initialization neither elects a Host nor exposes private diagnostics', async () => {
  let launched = false;
  const result = await connectOwnedRuntimeHostWithDependencies(
    {
      ...input('/unused'),
      initialization: { incognito: true, proxyUrl: 'http://user:private@proxy.invalid' },
    },
    {
      async initializeRoot() {
        throw new Error('private child output');
      },
      launchCandidate() {
        launched = true;
        throw new Error('must not launch');
      },
    },
  );
  assert.equal(launched, false);
  assert.deepEqual(result, {
    kind: 'failed',
    reason: 'startup_failed',
    detail: 'internal_startup_failure',
  });
});

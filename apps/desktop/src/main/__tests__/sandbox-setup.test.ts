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
import { setImmediate as flush } from 'node:timers/promises';
import type { SandboxSetupStatus } from '@maka/runtime-host/protocol';
import { RuntimeHostOperationError } from '@maka/runtime-host/client';
import { createSandboxSetup } from '../sandbox-setup.js';

test('first-use consent is shared, generation-bound, and resolves lost replies without replay', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let status: SandboxSetupStatus = 'not_configured';
  let current = true;
  let installs = 0;
  let confirmations = 0;
  let cancelElevation = false;
  let release!: (accepted: boolean) => void;
  const ensure = createSandboxSetup({
    query: async () => status,
    isCurrent: () => current,
    confirm: () => { confirmations += 1; return new Promise<boolean>((resolve) => { release = resolve; }); },
    install: async () => {
      installs += 1;
      if (cancelElevation) throw new RuntimeHostOperationError('sandbox.setup.install', 'user_cancelled', 'UAC cancelled');
      status = 'ready';
      throw new Error('reply lost');
    },
    unavailable: (value) => new Error(value),
  });
  const cancelled = ensure();
  const sameConsent = ensure();
  assert.equal(cancelled, sameConsent);
  await flush();
  release(false);
  assert.equal(await cancelled, false);
  assert.equal(installs, 0);

  const retired = ensure();
  await flush();
  current = false;
  release(true);
  assert.equal(await retired, false);
  assert.equal(installs, 0);

  current = true;
  const accepted = ensure();
  await flush();
  release(true);
  assert.equal(await accepted, true);
  assert.equal(installs, 1);
  assert.equal(await ensure(), true);
  assert.equal(confirmations, 3);
  // A cached success must not outlive removal by another client.
  status = 'busy';
  const otherClient = ensure();
  await flush();
  status = 'ready';
  t.mock.timers.tick(500);
  assert.equal(await otherClient, true);
  assert.equal(installs, 1);
  assert.equal(confirmations, 3);
  // Interrupted removal is repairable through the same user-consented setup.
  status = 'removing';
  const repaired = ensure();
  await flush();
  release(true);
  assert.equal(await repaired, true);
  assert.equal(installs, 2);
  assert.equal(confirmations, 4);
  status = 'not_configured';
  cancelElevation = true;
  const nativeCancelled = ensure();
  await flush();
  release(true);
  assert.equal(await nativeCancelled, false);
  assert.equal(installs, 3);
  assert.equal(status, 'not_configured');
});

test('busy setup observes lost replies, stops on retirement, and has a retryable deadline', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let now = 0;
  t.mock.method(performance, 'now', () => now);
  let status: SandboxSetupStatus = 'not_configured';
  let current = true;
  let installs = 0;
  let confirmations = 0;
  const ensure = createSandboxSetup({
    query: async () => status,
    isCurrent: () => current,
    confirm: async () => { confirmations += 1; return true; },
    install: async () => {
      installs += 1;
      status = 'busy';
      throw new Error('reply lost');
    },
    unavailable: (value) => new Error(value),
  });
  const accepted = ensure();
  await flush();
  status = 'ready';
  t.mock.timers.tick(500);
  assert.equal(await accepted, true);
  assert.equal(installs, 1);
  assert.equal(confirmations, 1);

  status = 'busy';
  const retired = ensure();
  await flush();
  current = false;
  t.mock.timers.tick(500);
  assert.equal(await retired, false);

  current = true;
  const bounded = ensure();
  const expired = assert.rejects(bounded, /busy/);
  await flush();
  now = 180_000;
  t.mock.timers.tick(500);
  await expired;
  status = 'ready';
  assert.equal(await ensure(), true);
  assert.equal(installs, 1);
  assert.equal(confirmations, 1);
});

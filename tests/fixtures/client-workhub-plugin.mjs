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
import { setTimeout as delay } from 'node:timers/promises';
import { clientPluginRemote } from '../../apps/desktop/src/renderer/platform/desktop/client-plugin-remote.ts';

export async function toggleWorkhub(connection, disabled) {
  await connection.request('plugin.composition.apply', {
    operations: [{ type: 'update', entryId: 'maka.workhub', patch: { disabled } }],
  });
  const deadline = Date.now() + 5000;
  for (;;) {
    const page = await connection.request('plugin.platform.query', { view: 'entries' });
    assert.equal(page.nextCursor, null);
    const backend = page.items.find((entry) => entry.id === 'maka.workhub');
    const client = page.items.find((entry) => entry.id === 'maka.workhub.ui');
    assert(backend && client);
    assert.equal(backend.diagnostic, undefined);
    assert.equal(client.diagnostic, undefined);
    if (
      disabled
        ? backend.status === 'disabled' &&
          backend.generation === undefined &&
          client.status === 'pending'
        : backend.status === 'active' && client.status === 'active'
    )
      return;
    assert(
      Date.now() < deadline,
      `WorkHub lifecycle did not settle: ${JSON.stringify(page.items)}`,
    );
    await delay(10);
  }
}

export async function workhubRemote(connection) {
  const page = await connection.request('plugin.client.query', { kind: 'snapshot' });
  const descriptor = page.entries.find((entry) => entry.entryId === 'maka.workhub.ui');
  assert(descriptor, 'WorkHub must publish its Client');
  const { entryId, extensionId, activation, contentDigest, clientDigest } = descriptor;
  const lifetime = new AbortController();
  const remote = clientPluginRemote(
    (_host, _epoch, request) => connection.request('plugin.remote', request),
    { profileId: 'test', hostId: 'test' },
    'original-connection',
  )({ entryId, extensionId, activation, contentDigest, clientDigest }, lifetime.signal);
  return {
    descriptor,
    method(name, sessionId) {
      const call = remote.api.method(name, sessionId);
      return async (input = null) => {
        const outcome = await call(input);
        if (!outcome.ok) throw Object.assign(new Error(outcome.error.message), outcome.error);
        return outcome.result;
      };
    },
    close: remote.close,
  };
}

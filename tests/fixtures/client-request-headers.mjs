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

export async function verifyRequestHeaders(
  connection,
  peer,
  provider,
  connectionId,
  model,
  overlay,
) {
  const call = (op, input) => connection.request(op, input, 3000);
  const query = (id = connectionId) =>
    call('connection.request-headers.query', { connectionId: id });
  const replace = (headers) =>
    call('connection.request-headers.replace', { connectionId, headers });
  const catalog = () => call('connection.catalog.query', { kind: 'start' });
  const names = ['X-Maka-Retained'];
  const missing = '12345678-1234-4234-8234-123456789abc';
  assert.deepEqual(await query(missing), { kind: 'connection_not_found' });
  assert.deepEqual(
    await call('connection.request-headers.replace', { connectionId: missing, headers: [] }),
    { kind: 'connection_not_found' },
  );
  assert.deepEqual(await query(), { kind: 'found', names: [] });
  const notices = [];
  const unsubscribe = connection.subscribeConfigurationChanges((value) => notices.push(value));
  const beforeCount = provider.count;
  try {
    assert.deepEqual(
      await replace([
        { name: '\uFEFF X-Maka-Retained ', value: 'private-header' },
        { name: 'X-Remove', value: 'remove' },
      ]),
      { kind: 'committed', names: [...names, 'X-Remove'] },
    );
    assert.deepEqual(await query(), { kind: 'found', names: [...names, 'X-Remove'] });
    await assert.rejects(replace([{ name: 'X-New' }]), { code: 'invalid_request' });
    assert.deepEqual(await replace([{ name: names[0] }]), { kind: 'committed', names });
    provider.auth(false);
    provider.expect('openai-compatible', model, overlay);
    provider.headers('private-header');
    const run = () => call('connection.test.run', { connectionId, modelId: model });
    assert.equal((await run()).test.kind, 'verified');
    const before = await catalog(),
      notificationCount = notices.length;
    assert.deepEqual(await replace([{ name: names[0] }]), { kind: 'unchanged', names });
    assert.deepEqual(
      await catalog(),
      before,
      'unchanged headers retain verification and catalog revision',
    );
    assert.equal(notices.length, notificationCount, 'unchanged headers publish no invalidation');

    const arrived = provider.hold(),
      pending = run();
    const release = await arrived;
    try {
      assert.deepEqual(
        await peer.request(
          'connection.request-headers.replace',
          {
            connectionId,
            headers: [{ name: names[0], value: 'changed' }],
          },
          3000,
        ),
        { kind: 'committed', names },
      );
    } finally {
      release();
    }
    assert.deepEqual(await pending, { kind: 'superseded', changed: ['credential'] });
    const changed = await catalog();
    assert.equal(
      changed.items.find((item) => item.kind === 'connection' && item.connectionId === connectionId)
        .lastTest,
      undefined,
    );
    assert.deepEqual(await replace([]), { kind: 'committed', names: [] });
    assert.deepEqual(await replace([]), { kind: 'unchanged', names: [] });
    assert.deepEqual(await query(), { kind: 'found', names: [] });
    assert.deepEqual(await replace([{ name: names[0], value: 'reopened-header' }]), {
      kind: 'committed',
      names,
    });
    provider.headers('reopened-header');
    assert.equal((await run()).test.kind, 'verified');
    assert.equal(provider.count, beforeCount + 3);
    assert.deepEqual(await query(), { kind: 'found', names });
  } finally {
    unsubscribe();
  }
}

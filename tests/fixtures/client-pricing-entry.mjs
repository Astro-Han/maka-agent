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
import { connect } from 'node:net';
import { once } from 'node:events';
import { parseArgs } from 'node:util';
import { connectRuntimeHostMessageTransport } from '../../packages/runtime-host/src/client/connection.ts';
import { FramedTransport } from '../../packages/runtime-host/src/transport/framed-transport.ts';
import {
  INTERACTIVE_RUNTIME_HOST_COMPOSITION_ID,
  RUNTIME_HOST_PROTOCOL_VERSION,
} from '../../packages/runtime-host/src/protocol/index.ts';

const { values } = parseArgs({
  options: {
    socket: { type: 'string' },
    'root-id': { type: 'string' },
    'pricing-workspace': { type: 'string' },
    reopened: { type: 'boolean' },
  },
});
const socket = connect(values.socket);
const transport = new FramedTransport(socket);
let connection;
try {
  await once(socket, 'connect');
  const connected = await connectRuntimeHostMessageTransport({
    transport,
    expectedRootId: values['root-id'],
    compositionId: INTERACTIVE_RUNTIME_HOST_COMPOSITION_ID,
    protocol: { min: RUNTIME_HOST_PROTOCOL_VERSION, max: RUNTIME_HOST_PROTOCOL_VERSION },
    handshakeTimeoutMs: 3000,
    livenessIntervalMs: 60000,
  });
  assert.equal(connected.kind, 'connected');
  connection = connected.connection;
  const request = (operation, input) => connection.request(operation, input, 5000);
  const snapshot = async () => {
    let page = await request('pricing.query', { kind: 'start' });
    const revision = page.revision;
    const entries = [];
    while (true) {
      assert.equal(page.kind, 'page');
      assert.equal(page.revision, revision);
      assert.equal(page.offset, entries.length);
      entries.push(...page.entries);
      if (page.nextOffset === null) return { revision, entries };
      page = await request('pricing.query', {
        kind: 'continue',
        revision,
        offset: page.nextOffset,
      });
    }
  };
  const initial = await snapshot();
  assert(initial.entries.length > 128);
  const key = 'fixture:priced-model';
  const pricing = { modelKey: key, inputUsdPer1M: 2.5, outputUsdPer1M: 10 };
  if (values.reopened) {
    assert.equal(initial.revision, 1);
    assert.deepEqual(
      initial.entries.find(({ pricing }) => pricing.modelKey === key),
      {
        source: 'custom',
        pricing,
        resetEffect: 'become_unpriced',
      },
    );
    assert.deepEqual(
      await request('pricing.mutate', {
        expectedRevision: 1,
        mutation: { kind: 'delete', modelKey: key },
      }),
      { kind: 'committed', revision: 2 },
    );
    assert(!(await snapshot()).entries.some(({ pricing }) => pricing.modelKey === key));
  } else {
    assert.equal(initial.revision, 0);
    const mutation = { expectedRevision: 0, mutation: { kind: 'upsert', pricing } };
    assert.deepEqual(await request('pricing.mutate', mutation), { kind: 'committed', revision: 1 });
    assert.deepEqual(await request('pricing.mutate', mutation), {
      kind: 'revision_conflict',
      expectedRevision: 0,
      actualRevision: 1,
    });
    assert.deepEqual(
      await request('pricing.query', {
        kind: 'continue',
        revision: 0,
        offset: 128,
      }),
      { kind: 'revision_changed', expectedRevision: 0, actualRevision: 1 },
    );
    assert.deepEqual(await request('pricing.mutate', { ...mutation, expectedRevision: 1 }), {
      kind: 'unchanged',
      revision: 1,
    });
  }
  console.log(values.reopened ? 'pricing-reopened' : 'pricing-passed');
} finally {
  transport.abort();
  await connection?.close();
}

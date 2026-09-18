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
import { parseHTML } from 'linkedom';
import { connectRuntimeHostMessageTransport } from '../../packages/runtime-host/src/client/connection.ts';
import { FramedTransport } from '../../packages/runtime-host/src/transport/framed-transport.ts';
import {
  INTERACTIVE_RUNTIME_HOST_COMPOSITION_ID,
  RUNTIME_HOST_PROTOCOL_VERSION,
} from '../../packages/runtime-host/src/protocol/index.ts';
import { ClientInstance } from '../../packages/ui/src/client-plugins/instance.ts';
import { clientPluginRemote } from '../../apps/desktop/src/renderer/platform/desktop/client-plugin-remote.ts';

const { values } = parseArgs({
  options: {
    socket: { type: 'string' },
    'root-id': { type: 'string' },
    'plugin-remote': { type: 'boolean' },
  },
});
const socket = connect(values.socket);
const transport = new FramedTransport(socket);
let connection;
let instance;
try {
  await once(socket, 'connect');
  const result = await connectRuntimeHostMessageTransport({
    transport,
    expectedRootId: values['root-id'],
    compositionId: INTERACTIVE_RUNTIME_HOST_COMPOSITION_ID,
    protocol: { min: RUNTIME_HOST_PROTOCOL_VERSION, max: RUNTIME_HOST_PROTOCOL_VERSION },
    handshakeTimeoutMs: 3_000,
    livenessIntervalMs: 60_000,
  });
  assert.equal(result.kind, 'connected');
  connection = result.connection;
  const page = await connection.request('plugin.client.query', { kind: 'snapshot' });
  assert.equal(page.kind, 'snapshot');
  const descriptor = page.entries.find((entry) => entry.extensionId === 'example.remote');
  assert.ok(descriptor);
  const origin = { profileId: 'test', hostId: values['root-id'] };
  let context;
  instance = new ClientInstance(
    descriptor,
    (error) => {
      throw error;
    },
    clientPluginRemote(
      async (host, epoch, request) => {
        assert.deepEqual(host, origin);
        assert.equal(epoch, 'original-connection');
        return connection.request('plugin.remote', request);
      },
      origin,
      'original-connection',
    ),
  );
  await instance.initialize(
    {
      activate(ctx) {
        context = ctx;
      },
    },
    parseHTML('<html><head></head></html>').document,
  );
  instance.publish();
  const echo = context.remote.method('echo');
  assert.equal((await echo('through original client')).generation, 1);
  await context.remote.method('replace')(null);
  await assert.rejects(echo('stale binding'), (error) => error.code === 'operation_conflict');
  assert.equal((await context.remote.method('echo')(null)).generation, 2);
  const cancellation = new AbortController();
  const events = context.remote.stream('events')(null, cancellation.signal)[Symbol.asyncIterator]();
  assert.deepEqual(await events.next(), { done: false, value: null });
  const waiting = assert.rejects(events.next());
  cancellation.abort(new Error('component unmounted'));
  await waiting;
  const stats = await context.remote.method('stats')(null);
  assert.equal(stats.active, 0);
  assert.equal(stats.opening, 0);
  await instance.shutdown();
  console.log('original-client-plugin-remote');
} finally {
  await instance?.shutdown();
  transport.abort();
  await connection?.close();
}

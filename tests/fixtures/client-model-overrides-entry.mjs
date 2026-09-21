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
  RUNTIME_HOST_COMPATIBILITY_EPOCH,
} from '../../packages/runtime-host/src/protocol/index.ts';
import { verifyModelOverrides } from './client-model-overrides-workflow.mjs';

const { values } = parseArgs({
  options: {
    socket: { type: 'string' },
    'root-id': { type: 'string' },
    'model-overrides-workspace': { type: 'string' },
    reopened: { type: 'boolean' },
  },
});
assert.equal(RUNTIME_HOST_COMPATIBILITY_EPOCH, 165);
const clients = [];
async function connectClient() {
  const socket = connect(values.socket);
  const transport = new FramedTransport(socket);
  const owned = { transport };
  clients.push(owned);
  await once(socket, 'connect');
  const result = await connectRuntimeHostMessageTransport({
    transport,
    expectedRootId: values['root-id'],
    compositionId: INTERACTIVE_RUNTIME_HOST_COMPOSITION_ID,
    protocol: { min: RUNTIME_HOST_PROTOCOL_VERSION, max: RUNTIME_HOST_PROTOCOL_VERSION },
    handshakeTimeoutMs: 3000,
    livenessIntervalMs: 60000,
  });
  assert.equal(result.kind, 'connected');
  owned.connection = result.connection;
  return result.connection;
}
try {
  const connection = await connectClient();
  await verifyModelOverrides(
    connection,
    values['model-overrides-workspace'],
    values.reopened,
    connectClient,
  );
  console.log(values.reopened ? 'model-overrides-reopened' : 'model-overrides-passed');
} finally {
  for (const { transport, connection } of clients) {
    transport.abort();
    await connection?.close();
  }
}

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
import { verifyArtifacts } from './client-artifact-workflow.mjs';

async function main() {
  const { values } = parseArgs({
    options: {
      socket: { type: 'string' },
      'root-id': { type: 'string' },
      'artifact-workspace': { type: 'string' },
      reopened: { type: 'boolean' },
    },
  });
  assert.equal(RUNTIME_HOST_COMPATIBILITY_EPOCH, 154);
  const clients = [];
  const open = async () => {
    const socket = connect(values.socket);
    const transport = new FramedTransport(socket);
    clients.push({ transport });
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
    clients.at(-1).connection = result.connection;
    return result.connection;
  };
  try {
    const connection = await open();
    if (!values.reopened) {
      const created = await connection.request(
        'connection.catalog.create',
        {
          expectedCatalogRevision: 0,
          connection: {
            slug: 'unused',
            name: 'Unused model fixture',
            providerType: 'openai-compatible',
            baseUrl: 'http://127.0.0.1:1/v1',
            enabled: true,
            enabledModelIds: ['fixture-model'],
          },
        },
        3000,
      );
      assert.equal(created.kind, 'committed');
      await connection.request(
        'credential.vault.set',
        {
          locator: {
            scope: 'connection',
            connectionId: created.connection.connectionId,
            kind: 'api_key',
          },
          expected: null,
          expectedConnection: {
            ...created.connection,
            slug: 'unused',
            providerType: 'openai-compatible',
            effectiveBaseUrl: 'http://127.0.0.1:1/v1',
          },
          secret: 'unused-fixture-secret',
        },
        3000,
      );
      await connection.request(
        'connection.catalog.set-default-target',
        {
          expectedCatalogRevision: 1,
          target: {
            connectionId: created.connection.connectionId,
            modelId: 'fixture-model',
          },
        },
        3000,
      );
      for (const sessionId of ['session', 'other']) {
        await connection.request(
          'session.create',
          {
            sessionId,
            workspace: { kind: 'host_path', path: values['artifact-workspace'] },
            modelTarget: { kind: 'default' },
          },
          3000,
        );
      }
    }
    await verifyArtifacts(connection, open, values.reopened);
    console.log(
      values.reopened ? 'original-client-artifact-reopen' : 'original-client-artifact-workflow',
    );
  } finally {
    for (const { transport, connection } of clients) {
      transport.abort();
      await connection?.close();
    }
  }
}
main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});

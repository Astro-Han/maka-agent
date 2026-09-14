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
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { connectRuntimeHostMessageTransport } from '../../packages/runtime-host/src/client/connection.ts';
import { FramedTransport } from '../../packages/runtime-host/src/transport/framed-transport.ts';
import { createServer } from 'node:http';
import { continuousModel, readCatalog, verifyCatalogStream } from './client-catalog-stream.mjs';

const { values } = parseArgs({
  options: {
    socket: { type: 'string' },
    'root-id': { type: 'string' },
    'onboarding-workspace': { type: 'string' },
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
    compositionId: 'maka.interactive',
    protocol: { min: 0, max: 0 },
    handshakeTimeoutMs: 3000,
    livenessIntervalMs: 60000,
  });
  assert.equal(connected.kind, 'connected');
  connection = connected.connection;
  await workflow(connection, values['onboarding-workspace'], values.reopened);
  console.log(values.reopened ? 'onboarding-reopened' : 'onboarding-passed');
} finally {
  transport.abort();
  await connection?.close();
}

async function workflow(connection, workspace, reopened) {
  const call = (op, input) => connection.request(op, input, 5000);
  const catalog = () => readCatalog(connection);
  const path = join(workspace, 'onboarding.json');
  if (reopened) {
    assert.deepEqual(await catalog(), JSON.parse(await readFile(path, 'utf8')));
    const view = await call('session.catalog.query', { kind: 'get', sessionId: 'onboarded' });
    assert.equal(view.session.model, 'one');
    await verifyCatalogStream(connection, undefined, workspace, true);
    return;
  }
  let count = 0,
    status = 200,
    failure;
  const models = Array.from({ length: 445 }, (_, index) => ({
    id: index === 0 ? 'one' : index === 1 ? 'two' : `model-${index}`,
    name: `Model ${index}`,
    description: 'Representative model metadata: multilingual 中文, quotes " and newline\n'.repeat(
      8,
    ),
  }));
  const model = continuousModel();
  const server = createServer(async (req, res) => {
    try {
      assert.equal(req.headers.authorization, 'Bearer test-onboarding-key');
      if (req.url === '/v1/chat/completions') {
        await model.respond(req, res);
        return;
      }
      assert.equal(req.url, '/v1/models');
      count++;
      res.writeHead(status, { 'Content-Type': 'application/json', Connection: 'close' });
      res.end(JSON.stringify(status === 200 ? { data: models } : { error: 'test-onboarding-key' }));
    } catch (error) {
      failure = error;
      res.destroy();
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const baseUrl = 'http://127.0.0.1:' + server.address().port + '/v1';
  const notices = [];
  connection.subscribeConfigurationChanges((revision) => notices.push(revision));
  try {
    const input = {
      target: { kind: 'create', providerType: 'openrouter' },
      apiKey: 'test-onboarding-key',
      baseUrl,
    };
    const before = await catalog();
    const verified = await call('connection.onboarding.verify', input);
    assert.equal(verified.kind, 'verified');
    assert.equal(verified.models.length, models.length);
    assert.deepEqual(await catalog(), before);
    assert.equal(notices.length, 0);
    status = 401;
    assert.deepEqual(
      await call('connection.onboarding.save', { ...input, enabledModelIds: ['one'] }),
      { kind: 'failed', errorClass: 'auth' },
    );
    assert.deepEqual(await catalog(), before);
    status = 200;
    assert.deepEqual(
      await call('connection.onboarding.save', { ...input, enabledModelIds: ['missing'] }),
      { kind: 'rejected', reason: 'model_unavailable' },
    );
    assert.deepEqual(await catalog(), before);
    const saved = await call('connection.onboarding.save', { ...input, enabledModelIds: ['one'] });
    assert.equal(saved.kind, 'saved');
    assert.equal(saved.connection.slug, 'openrouter');
    assert.equal(count, 4, 'save discovers again and cannot trust a prior verification');
    const existing = {
      ...input,
      target: { kind: 'existing', connectionId: saved.connection.connectionId },
      apiKey: null,
      baseUrl: null,
    };
    assert.equal((await call('connection.onboarding.verify', existing)).kind, 'verified');
    await call('session.create', {
      sessionId: 'onboarded',
      mode: 'bot',
      workspace: { kind: 'host_path', path: workspace },
      permissionMode: 'explore',
      modelTarget: {
        kind: 'explicit',
        connectionId: saved.connection.connectionId,
        connectionSlug: saved.connection.slug,
        model: 'one',
      },
    });
    // Another registry-compatible provider follows exactly the same native driver.
    const other = await call('connection.onboarding.save', {
      ...input,
      target: { kind: 'create', providerType: 'deepseek' },
      enabledModelIds: [],
    });
    assert.equal(other.kind, 'saved');
    const page = await catalog();
    assert.equal(page.items.filter((item) => item.kind === 'connection').length, 2);
    assert.equal(notices.length, 2);
    assert(notices[1] > notices[0]);
    await writeFile(path, JSON.stringify(page));
    await verifyCatalogStream(connection, model, workspace, false);
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    if (failure) throw failure;
  }
}

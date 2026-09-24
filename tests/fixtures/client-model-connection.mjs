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
import { randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';
import {
  readRuntimeHostConnectionCatalog,
  readRuntimeHostModelProviders,
} from '../../packages/runtime-host/src/client/catalog-reader.ts';

export async function createModelConnection(
  request,
  {
    providerName,
    slug,
    name,
    baseUrl,
    apiKey,
    enabledModelIds,
    modelOverrides,
    requestBodyOverlay,
  },
) {
  const connection = { request };
  const directory = await readRuntimeHostModelProviders(connection);
  const provider = directory.entries.find((entry) => entry.identity.name === providerName);
  assert(provider, `Missing fixture provider: ${providerName}`);
  const login = await authenticate(
    request,
    {
      kind: 'create',
      provider: provider.identity,
      configuration: { baseUrl },
      slug,
      name,
    },
    apiKey,
  );
  const catalog = await readRuntimeHostConnectionCatalog(connection);
  const row = catalog.connections.find((row) => row.connectionId === login.connection.connectionId);
  assert(row);
  const updated = await request('connection.catalog.update', {
    expected: { connectionId: row.connectionId, revision: row.revision },
    changes: {
      name,
      configuration: row.configuration,
      enabled: true,
      enabledModelIds,
      modelOverrides,
      requestBodyOverlay,
    },
  });
  assert.equal(updated.kind, 'committed');
  return updated;
}

export async function authenticateModelConnection(request, connectionId, apiKey) {
  const catalog = await readRuntimeHostConnectionCatalog({ request });
  const row = catalog.connections.find((row) => row.connectionId === connectionId);
  assert(row);
  return authenticate(
    request,
    {
      kind: 'existing',
      expected: {
        connectionId: row.connectionId,
        revision: row.revision,
        slug: row.slug,
        provider: row.provider,
        configuration: row.configuration,
      },
      configuration: row.configuration,
    },
    apiKey,
  );
}

async function authenticate(request, target, apiKey) {
  const attemptId = randomUUID();
  let login = await request('oauth.login.start', {
    attemptId,
    target,
    authentication: { method: 'api-key', input: { apiKey } },
  });
  const deadline = Date.now() + 5000;
  while (['exchanging', 'committing'].includes(login.phase)) {
    assert(Date.now() < deadline, 'Fixture login did not settle');
    await delay(10);
    login = await request('oauth.login.query', { attemptId });
  }
  assert.equal(login.phase, 'authenticated');
  return login;
}

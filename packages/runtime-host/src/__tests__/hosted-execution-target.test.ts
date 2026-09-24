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
import type { RuntimeHostConnection } from '../client/connection.js';
import { configureHostedExecutionTarget } from '../client/hosted-execution-target.js';

const CONNECTION_ID = '00000000-0000-4000-8000-000000000001';
const provider = {
  packageId: 'external.provider',
  entryId: 'entry',
  scope: 'profile' as const,
  name: 'custom',
};
const configuration = { region: 'opaque-region', deployment: { name: 'model' } };
const authentication = {
  attemptId: 'stable-attempt',
  target: { kind: 'create' as const, provider, configuration, slug: 'personal', name: 'Personal' },
  authentication: { method: 'custom-login', input: { token: 'transient-secret' } },
};

test('hosted target consumes public authentication and preserves opaque configuration and default', async () => {
  const requests: Array<{ operation: string; input: unknown }> = [];
  let queries = 0;
  const connection = {
    request: async (operation: string, input: unknown) => {
      requests.push({ operation, input });
      if (operation.startsWith('oauth.login.')) {
        return {
          attemptId: authentication.attemptId,
          connection: { connectionId: CONNECTION_ID, slug: 'personal', provider },
          phase: operation === 'oauth.login.start' ? 'committing' : 'authenticated',
        };
      }
      if (operation === 'connection.catalog.query') {
        return catalogPage(++queries === 1 ? ['old-model'] : ['old-model', 'new-model']);
      }
      if (operation === 'connection.catalog.update' || operation === 'connection.models.fetch')
        return {
          kind: 'committed',
          catalogRevision: 2,
          connection: { connectionId: CONNECTION_ID, revision: 2 },
        };
      throw new Error(`Unexpected operation ${operation}`);
    },
  } as unknown as Pick<RuntimeHostConnection, 'request'>;
  assert.deepEqual(
    await configureHostedExecutionTarget(connection, {
      connection: authentication,
      connectionSlug: 'personal',
      model: 'new-model',
    }),
    { connectionId: CONNECTION_ID, connectionSlug: 'personal' },
  );
  assert.deepEqual(
    requests.map(({ operation }) => operation),
    [
      'oauth.login.start',
      'oauth.login.query',
      'connection.catalog.query',
      'connection.catalog.update',
      'connection.models.fetch',
      'connection.catalog.query',
    ],
  );
  assert.deepEqual(requests[0]?.input, authentication);
  assert.deepEqual(requests[1]?.input, { attemptId: 'stable-attempt' });
  assert.deepEqual(requests[3]?.input, {
    expected: { connectionId: CONNECTION_ID, revision: 1 },
    changes: {
      name: 'Personal',
      configuration,
      enabled: true,
      enabledModelIds: ['old-model', 'new-model'],
    },
  });
});

test('hosted target rejects authentication recipient changes and concurrent configuration replacement', async () => {
  for (const changed of ['recipient', 'configuration']) {
    let queries = 0;
    const connection = {
      request: async (operation: string) => {
        if (operation === 'oauth.login.start')
          return {
            attemptId: authentication.attemptId,
            connection: {
              connectionId: CONNECTION_ID,
              slug: 'personal',
              provider: { ...provider, name: 'other' },
            },
            phase: 'authenticated',
          };
        assert.equal(operation, 'connection.catalog.query');
        return catalogPage(
          ['new-model'],
          ++queries === 2 ? { region: 'other-account' } : configuration,
        );
      },
    } as unknown as Pick<RuntimeHostConnection, 'request'>;
    await assert.rejects(
      configureHostedExecutionTarget(connection, {
        ...(changed === 'recipient' ? { connection: authentication } : {}),
        connectionSlug: 'personal',
        model: 'new-model',
      }),
    );
    assert.equal(queries, changed === 'recipient' ? 0 : 2);
  }
});

test('cancelling a waiter does not repeat an accepted authentication attempt', {
  timeout: 1_000,
}, async () => {
  const abort = new AbortController();
  const requests: string[] = [];
  let started!: () => void;
  const accepted = new Promise<void>((resolve) => {
    started = resolve;
  });
  const connection = {
    request: async (operation: string) => {
      requests.push(operation);
      started();
      return await new Promise<never>(() => {});
    },
  } as unknown as Pick<RuntimeHostConnection, 'request'>;
  const configuring = configureHostedExecutionTarget(
    connection,
    {
      connection: authentication,
      connectionSlug: 'personal',
      model: 'new-model',
    },
    abort.signal,
  );
  await accepted;
  abort.abort(new Error('cancelled'));
  await assert.rejects(configuring, /cancelled/u);
  assert.deepEqual(requests, ['oauth.login.start']);
});

function catalogPage(enabledModelIds: string[], config = configuration as object) {
  return {
    kind: 'page',
    revision: 1,
    defaultTarget: { connectionId: CONNECTION_ID, modelId: 'old-model' },
    connectionCount: 1,
    items: [
      {
        kind: 'connection',
        connectionIndex: 0,
        connectionId: CONNECTION_ID,
        revision: 1,
        slug: 'personal',
        name: 'Personal',
        provider,
        configuration: config,
        enabled: true,
        enabledModelIdCount: enabledModelIds.length,
        modelCount: enabledModelIds.length,
        modelSource: 'fetched',
        catalogEntryCount: 0,
      },
      ...enabledModelIds.map((modelId, itemIndex) => ({
        kind: 'enabled_model_id',
        connectionIndex: 0,
        itemIndex,
        modelId,
      })),
      ...enabledModelIds.map((id, itemIndex) => ({
        kind: 'model',
        connectionIndex: 0,
        itemIndex,
        model: { id },
      })),
    ],
    nextCursor: null,
  };
}

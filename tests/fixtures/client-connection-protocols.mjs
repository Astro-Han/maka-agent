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
import { readCatalog } from './client-catalog-stream.mjs';
import { createModelConnection } from './client-model-connection.mjs';

// The cases name expected wires, independently of the production resolver.
export async function verifyConnectionProtocols(connection, provider, secret) {
  const request = (op, input) => connection.request(op, input, 3000);
  const catalog = () => readCatalog(connection);
  const model = 'protocol-fixture';
  provider.headers(undefined);
  provider.auth(false);
  for (const [providerType, wires] of [
    ['openrouter', ['openai-chat']],
    ['deepseek', ['openai-chat', 'openai-responses']],
    // Nonstandard GET discovery does not prevent a standard POST probe.
    ['siliconflow', ['openai-chat']],
    ['openai-responses-compatible', ['openai-responses']],
    ['anthropic-compatible', ['anthropic-messages']],
    ['opencode', ['openai-chat', 'openai-responses', 'anthropic-messages']],
    ['google', [null]],
    ['openai', ['anthropic-messages']],
    ['alibaba-token-plan', ['openai-responses']],
  ]) {
    const created = await createModelConnection(request, {
      slug: `probe-${providerType}`,
      name: providerType,
      providerName: providerType,
      apiKey: secret,
      baseUrl: provider.baseUrl,
      enabledModelIds: [model],
    });
    assert.equal(created.kind, 'committed');
    const connectionId = created.connection.connectionId;
    for (const wire of wires) {
      const current = (await catalog()).items.find(
        (item) => item.kind === 'connection' && item.connectionId === connectionId,
      );
      const updated = await request('connection.catalog.update', {
        expected: { connectionId, revision: current.revision },
        changes: {
          name: current.name,
          configuration: current.configuration,
          enabled: true,
          enabledModelIds: [model],
          modelOverrides: wire ? { [model]: { apiProtocol: wire } } : null,
        },
      });
      assert.equal(updated.kind, 'committed');
      provider.expect(providerType, model, {}, wire);
      const before = await catalog();
      const count = provider.count;
      const run = () => request('connection.test.run', { connectionId, modelId: model });
      if (wire === null || providerType === 'openai') {
        const tested = await run();
        assert.equal(tested.kind, 'committed');
        assert.equal(tested.test.kind, 'failed');
        assert.equal(tested.test.errorClass, 'invalid_response');
        assert.equal(tested.test.modelId, model);
        assert.equal(tested.test.statusCode, null);
        assert.equal(provider.count, count, 'unsupported routing cannot issue HTTP');
        const after = await catalog();
        assert.equal(after.revision, before.revision + 1);
        assert.equal(
          after.items.find(
            (item) => item.kind === 'connection' && item.connectionId === connectionId,
          ).lastTest.errorClass,
          'unknown',
        );
      } else {
        const tested = await run();
        assert.equal(tested.kind, 'committed');
        assert.equal(tested.test.kind, 'verified');
        assert.equal(tested.test.modelId, model);
        assert.equal(provider.count, count + 1);
      }
    }
    // This helper leaves the original scenario's connections/default intact.
    const page = await catalog();
    const row = page.items.find(
      (item) => item.kind === 'connection' && item.connectionId === connectionId,
    );
    const removed = await request('connection.catalog.remove', {
      expected: { connectionId, revision: row.revision },
    });
    assert.equal(removed.kind, 'committed');
  }
}

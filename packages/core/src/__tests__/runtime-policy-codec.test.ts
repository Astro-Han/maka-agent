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
import { test } from 'node:test';
import {
  createDefaultRuntimePolicy,
  decodeCanonicalConnectionCatalogEntry,
  decodeCanonicalRuntimePolicy,
  decodeModelOverridesTable,
  normalizeCreateCatalogConnectionInput,
  normalizeConnectionCatalogEntryUpdate,
  normalizeConnectionModelDiscoveryResult,
  normalizeRuntimePolicyMutation,
  normalizeSetCredentialInput,
  RuntimePolicyDomainDecodeError,
} from '../runtime-policy.js';

test('normalizes policy input while canonical policy decode rejects producer drift', () => {
  const mutation = normalizeRuntimePolicyMutation({
    expectedRevision: 0,
    operation: {
      kind: 'set_network_proxy',
      value: { ...createDefaultRuntimePolicy().networkProxy, enabled: true, host: ' proxy.local ' },
    },
  });
  assert.equal(mutation.operation.kind, 'set_network_proxy');
  if (mutation.operation.kind !== 'set_network_proxy') return;
  assert.equal(mutation.operation.value.host, 'proxy.local');

  assert.throws(
    () =>
      decodeCanonicalRuntimePolicy({
        ...createDefaultRuntimePolicy(),
        networkProxy: { ...mutation.operation.value, host: ' proxy.local ' },
      }),
    RuntimePolicyDomainDecodeError,
  );
  assert.doesNotThrow(() =>
    decodeCanonicalRuntimePolicy({
      ...createDefaultRuntimePolicy(),
      networkProxy: { ...mutation.operation.value, host: 'proxy.local' },
    }),
  );
});

test('normalizes the explicit Git Bash preference and rejects arbitrary shell kinds', () => {
  assert.deepEqual(
    normalizeRuntimePolicyMutation({
      expectedRevision: 3,
      operation: {
        kind: 'set_shell',
        value: {
          preference: 'git_bash',
          executable: ' C:\\Program Files\\Git\\bin\\bash.exe ',
        },
      },
    }),
    {
      expectedRevision: 3,
      operation: {
        kind: 'set_shell',
        value: {
          preference: 'git_bash',
          executable: 'C:\\Program Files\\Git\\bin\\bash.exe',
        },
      },
    },
  );
  assert.throws(
    () =>
      normalizeRuntimePolicyMutation({
        expectedRevision: 3,
        operation: {
          kind: 'set_shell',
          value: { preference: 'custom', executable: 'C:\\tools\\fish.exe' },
        },
      }),
    RuntimePolicyDomainDecodeError,
  );
});

test('normalizes only the bounded agent settings patch surface', () => {
  assert.deepEqual(
    normalizeRuntimePolicyMutation({
      expectedRevision: 4,
      operation: {
        kind: 'patch_agent_settings',
        value: {
          personalization: { assistantTone: 'Be direct.' },
          memory: { agentReadEnabled: true },
        },
      },
    }),
    {
      expectedRevision: 4,
      operation: {
        kind: 'patch_agent_settings',
        value: {
          personalization: { assistantTone: 'Be direct.' },
          memory: { agentReadEnabled: true },
        },
      },
    },
  );
  assert.throws(
    () =>
      normalizeRuntimePolicyMutation({
        expectedRevision: 4,
        operation: {
          kind: 'patch_agent_settings',
          value: { networkProxy: { enabled: false } },
        },
      }),
    RuntimePolicyDomainDecodeError,
  );
});

test('catalog preserves opaque provider configuration and independent model policy', () => {
  const provider = {
    packageId: 'external.provider',
    entryId: 'custom',
    scope: 'profile',
    name: 'account',
  };
  const configuration = { endpoint: 'https://proxy.example:443/v1', region: { name: 'custom' } };
  const input = normalizeCreateCatalogConnectionInput({
    expectedCatalogRevision: 0,
    connection: {
      slug: 'external-account',
      name: 'External',
      provider,
      configuration,
      enabled: true,
      enabledModelIds: [],
      modelOverrides: {
        future: {
          thinkingLevels: ['high', 'max'],
          defaultThinkingLevel: 'max',
          codeMode: true,
          applyPatch: false,
        },
      },
    },
  });
  assert.deepEqual(input.connection.configuration, configuration);
  const stored = {
    ...input.connection,
    connectionId: '123e4567-e89b-42d3-a456-426614174000',
    revision: 1,
    models: [],
  };
  assert.deepEqual(decodeCanonicalConnectionCatalogEntry(stored), stored);
  for (const changes of [
    { provider: { ...provider, packageId: 'Other Provider' } },
    { provider: { ...provider, scope: 'desktop-ui' } },
    { configuration: { data: 'x'.repeat(65536) } },
    { baseUrl: 'https://old-shape.example' },
  ])
    assert.throws(
      () => decodeCanonicalConnectionCatalogEntry({ ...stored, ...changes }),
      RuntimePolicyDomainDecodeError,
    );
  const update = { name: 'Updated', configuration, enabled: false, enabledModelIds: [] };
  assert.deepEqual(normalizeConnectionCatalogEntryUpdate(update), update);
  assert.deepEqual(normalizeConnectionCatalogEntryUpdate({ ...update, modelOverrides: null }), {
    ...update,
    modelOverrides: null,
  });
  assert.deepEqual(
    normalizeConnectionCatalogEntryUpdate({
      ...update,
      modelOverrides: input.connection.modelOverrides,
    }),
    {
      ...update,
      modelOverrides: input.connection.modelOverrides,
    },
  );
  const hostile = JSON.parse('{"__proto__":{"vision":true},"constructor":{"vision":false}}');
  assert.deepEqual(decodeModelOverridesTable(hostile), hostile);
  for (const profile of [
    { thinkingLevels: ['high', 'high'] },
    { thinkingLevels: [] },
    { adapter: '' },
    { contextWindow: 0 },
    { codeMode: 'true' },
    { applyPatch: null },
    { extra: true },
  ])
    assert.throws(
      () => decodeModelOverridesTable({ future: profile }),
      RuntimePolicyDomainDecodeError,
    );
});

test('normalizes exact bounded model discovery results', () => {
  assert.deepEqual(
    normalizeConnectionModelDiscoveryResult({
      models: [{ id: 'gpt-5', capabilities: { chat: true, parallelToolCalls: false } }],
      source: 'fetched',
      fetchedAt: 42,
    }),
    {
      models: [{ id: 'gpt-5', capabilities: { chat: true, parallelToolCalls: false } }],
      source: 'fetched',
      fetchedAt: 42,
    },
  );
  for (const invalid of [
    { models: [{ id: 'duplicate' }, { id: 'duplicate' }], source: 'fetched', fetchedAt: 42 },
    { models: [{ id: 'invalid', contextWindow: 0 }], source: 'fetched', fetchedAt: 42 },
    {
      models: Array.from({ length: 2049 }, (_, i) => ({ id: `model-${i}` })),
      source: 'fetched',
      fetchedAt: 42,
    },
    { models: [{ id: 'gpt-5' }], source: 'unknown', fetchedAt: 42 },
    { models: [{ id: 'gpt-5' }], source: 'fetched', fetchedAt: 42, rawBody: 'secret' },
  ]) {
    assert.throws(
      () => normalizeConnectionModelDiscoveryResult(invalid),
      RuntimePolicyDomainDecodeError,
    );
  }
});

test('normalizes extended model facts used by the runtime host catalog', () => {
  const result = normalizeConnectionModelDiscoveryResult({
    models: [
      {
        id: 'custom-model',
        description: 'A custom model',
        inputLimit: 120_000,
        knowledgeCutoff: '2025-01',
        structuredOutput: true,
        lastUpdated: '2026-01-01',
        modalities: { input: ['text', 'image'], output: ['text'] },
      },
    ],
    source: 'fetched',
    fetchedAt: 42,
  });
  assert.deepEqual(result.models[0], {
    id: 'custom-model',
    description: 'A custom model',
    inputLimit: 120_000,
    knowledgeCutoff: '2025-01',
    structuredOutput: true,
    lastUpdated: '2026-01-01',
    modalities: { input: ['text', 'image'], output: ['text'] },
  });
});

test('carries the video and pdf modalities models.dev declares', () => {
  const modalities = {
    input: ['text', 'image', 'video'],
    output: ['text', 'pdf', 'video'],
  };
  const result = normalizeConnectionModelDiscoveryResult({
    models: [{ id: 'custom-model', modalities }],
    source: 'fetched',
    fetchedAt: 42,
  });
  assert.deepEqual(result.models[0], { id: 'custom-model', modalities });
});

test('rejects sparse model modality arrays', () => {
  assert.throws(
    () =>
      normalizeConnectionModelDiscoveryResult({
        models: [
          {
            id: 'custom-model',
            modalities: { input: Array(1), output: ['text'] },
          },
        ],
        source: 'fetched',
        fetchedAt: 42,
      }),
    RuntimePolicyDomainDecodeError,
  );
});

test('provider envelopes cannot bypass login receipts through raw vault writes', () => {
  const input = normalizeSetCredentialInput({
    locator: {
      scope: 'connection',
      connectionId: '123e4567-e89b-42d3-a456-426614174000',
      kind: 'request_headers',
    },
    expected: null,
    secret: JSON.stringify({ 'x-custom': 'value' }),
  });
  assert.throws(
    () =>
      normalizeSetCredentialInput({ ...input, locator: { ...input.locator, kind: 'provider' } }),
    /authentication receipt/,
  );
  assert.throws(
    () => normalizeSetCredentialInput({ ...input, secret: '' }),
    RuntimePolicyDomainDecodeError,
  );
});

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

import {
  PROVIDER_REGISTRY,
  providerFallbackModelIds,
} from '../../packages/core/src/provider-registry.ts';
import { providerSupportsModelDiscovery } from '../../packages/core/src/llm-connections.ts';
import {
  modelMetadataIdsForProvider,
  lookupModelMetadata,
  lookupModelRuntimeOverride,
} from '../../packages/core/src/model-metadata.ts';
import {
  buildModelCatalogEntries,
  buildConnectionModelCatalogEntries,
} from '../../packages/core/src/model-catalog.ts';
import { getModelCapabilities } from '@ai-sdk/anthropic/internal';

export function outputProviderFacts() {
  return Object.fromEntries(
    Object.entries(PROVIDER_REGISTRY).map(([providerType, defaults]) => {
      const fallbackModels = providerFallbackModelIds(defaults);
      const ids = [...new Set([...modelMetadataIdsForProvider(providerType), ...fallbackModels])];
      return [
        providerType,
        {
          baseUrl: defaults.baseUrl,
          label: defaults.label,
          authKind: defaults.authKind,
          runtimeAdapter: defaults.runtimeAdapter,
          protocolAdapters: defaults.protocolAdapters ?? {},
          retired: defaults.retired === true,
          brokenModelIds: defaults.brokenModelIds ?? [],
          supportsModelDiscovery: providerSupportsModelDiscovery(providerType),
          modelDiscovery: defaults.modelDiscovery,
          fallbackModels,
          models: Object.fromEntries(
            ids.map((id) => [
              id,
              {
                metadata: lookupModelMetadata(providerType, id),
                runtimeOverride: lookupModelRuntimeOverride(providerType, id),
                entry: buildModelCatalogEntries({ providerType, models: [{ id }] })[0],
                ...(providerType === 'anthropic'
                  ? {
                      anthropicAdaptiveThinking: getModelCapabilities(
                        id
                          .split('/')
                          .at(-1)
                          .replace(/^(claude-(?:haiku|opus|sonnet)-\d+)\.(\d+)(?=$|-)/, '$1-$2'),
                      ).supportsAdaptiveThinking,
                    }
                  : {}),
              },
            ]),
          ),
        },
      ];
    }),
  );
}
export function oracleFixtures() {
  const fixtures = [
    {
      providerType: 'openai',
      models: [],
      modelSource: 'fetched',
      enabledModelIds: ['manual-unknown'],
    },
    { providerType: 'openai', models: [{ id: 'gpt-5' }], defaultModel: 'gpt-5' },
    {
      providerType: 'openai',
      models: [{ id: 'gpt-5', contextWindow: 12345, capabilities: { vision: true } }],
      enabledModelIds: ['gpt-5'],
      modelOverrides: {
        'gpt-5': { vision: false, thinkingLevels: ['high', 'low'], contextWindow: 999 },
      },
    },
    { providerType: 'openai', models: [], modelSource: 'fetched' },
    {
      providerType: 'openai-compatible',
      models: [
        {
          id: 'reported',
          contextWindow: 16000,
          inputLimit: 12000,
          capabilities: { chat: true, vision: true },
        },
      ],
      enabledModelIds: ['reported'],
      modelOverrides: {
        reported: {
          contextWindow: 14000,
          inputLimit: 10000,
          compactionThreshold: 9000,
          maxOutputTokens: 1024,
          displayName: 'Configured',
          description: '',
          vision: false,
          capabilities: { functionCalling: false },
        },
        'disabled-but-configured': {
          displayName: 'Remembered',
          modalities: { input: ['image'], output: ['image'] },
        },
      },
    },
    {
      providerType: 'anthropic',
      models: [],
      modelSource: 'fallback',
      defaultModel: 'manual-default',
      enabledModelIds: ['saved-model'],
    },
    { providerType: 'anthropic', models: [{ id: 'claude-99-sonnet-test' }] },
    {
      providerType: 'openai',
      models: [
        { id: 'gpt-5', capabilities: { chat: false } },
        { id: 'audio-model', modalities: { input: ['text'], output: ['audio'] } },
      ],
    },
    ...Object.keys(PROVIDER_REGISTRY).map((providerType) => ({ providerType, models: [] })),
    ...Object.entries(PROVIDER_REGISTRY).flatMap(([providerType, defaults]) =>
      (defaults.brokenModelIds ?? []).map((id) => ({
        providerType,
        models: [{ id }],
        defaultModel: id,
        enabledModelIds: [id],
      })),
    ),
    {
      providerType: 'openai',
      models: [{ id: ' custom ' }, { id: 'custom', displayName: 'ignored' }],
      defaultModel: ' missing ',
      enabledModelIds: [' custom ', 'second', 'second'],
    },
    {
      providerType: 'openai',
      models: [
        {
          id: 'gpt-5',
          capabilities: { chat: true },
          modalities: { input: ['text'], output: ['image'] },
        },
      ],
    },
    { providerType: 'unknown-provider', models: [{ id: 'manual' }] },
  ];
  return fixtures.map((connection) => ({
    connection,
    expected: buildConnectionModelCatalogEntries({ connection: { slug: 'test', ...connection } }),
  }));
}

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
import { buildModelCatalogEntries } from '../../packages/core/src/model-catalog.ts';
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

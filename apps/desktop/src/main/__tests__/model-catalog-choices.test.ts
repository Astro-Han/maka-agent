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

import { strict as assert } from 'node:assert';
import { describe, it } from 'node:test';
import type {
  ProjectedLlmConnection,
} from '@maka/core/llm-connections';
import {
  type ModelCatalogEntry,
} from '@maka/core/model-catalog';
import { buildChatModelChoices } from '@maka/core/chat-model-choice';
import { pickNewChatModel } from '../../renderer/shell-chat-model-selection.js';
import { buildCatalogDailyReviewModelOptions } from '../../renderer/model-catalog-choices.js';

const provider = { packageId: 'external.provider', entryId: 'account', scope: 'profile', name: 'custom' } as const;

function connection(overrides: Partial<ProjectedLlmConnection> & Pick<ProjectedLlmConnection, 'slug'>): ProjectedLlmConnection {
  return {
    connectionId: `connection-${overrides.slug}`,
    revision: 1,
    name: overrides.slug,
    provider,
    configuration: {},
    enabled: true,
    enabledModelIds: ['model'],
    models: [{ id: 'model' }],
    catalogEntries: [{ id: 'model', isDefault: true, canUseAsChatDefault: true, supportsVision: true, thinkingLevels: [] }],
    ...overrides,
  };
}

describe('model catalog picker helpers', () => {
  it('uses the readiness-checked activation candidate before an unverified first choice', () => {
    assert.deepEqual(
      pickNewChatModel({
        pending: null,
        activationCandidate: {
          llmConnectionSlug: 'ready-second',
          model: 'ready-model',
        },
        catalogDefault: undefined,
        choices: [
          {
            connectionId: 'connection-missing',
            connectionSlug: 'missing-key-first',
            provider,
            providerLabel: 'Anthropic',
            model: 'unusable-model',
            label: 'Unusable',
            isDefault: true,
            thinkingLevels: [],
          },
          {
            connectionId: 'connection-ready',
            connectionSlug: 'ready-second',
            provider,
            providerLabel: 'OpenCode Zen',
            model: 'ready-model',
            label: 'Ready',
            isDefault: true,
            thinkingLevels: [],
          },
        ],
      }),
      {
        llmConnectionId: 'connection-ready',
        llmConnectionSlug: 'ready-second',
        model: 'ready-model',
      },
    );
  });
  it('offers external providers and disambiguates their user-assigned connection names', () => {
    const connections = ['first', 'second'].map((slug) => connection({ slug, name: 'Research' }));
    const choices = buildChatModelChoices(connections);
    assert.deepEqual(choices.map((choice) => [choice.provider, choice.connectionName]), [
      [provider, 'Research'], [provider, 'Research'],
    ]);
    assert.deepEqual(buildCatalogDailyReviewModelOptions(connections, '', 'en'), [
      ['first::model', 'model · Research · first'],
      ['second::model', 'model · Research · second'],
    ]);
  });

  it('uses Host eligibility rather than an enabled ID or a local vendor catalog', () => {
    const entries: ModelCatalogEntry[] = ['chat', 'embedding'].map((id) => ({
      id, isDefault: id === 'chat', canUseAsChatDefault: id === 'chat', supportsVision: false, thinkingLevels: [],
    }));
    const options = buildCatalogDailyReviewModelOptions(
      [
        connection({
          slug: 'codex',
          enabledModelIds: ['chat', 'embedding', 'missing'],
          catalogEntries: entries,
        }),
      ],
      '',
      'zh-CN',
    );
    const keys = options.map(([key]) => key);
    assert.deepEqual(keys, ['codex::chat']);
  });

  it('labels a saved-but-unavailable selection in the UI locale', () => {
    const [, label] = buildCatalogDailyReviewModelOptions([], 'codex::gone', 'en').at(-1)!;
    assert.equal(label, 'gone · codex · Currently unavailable');
  });
});

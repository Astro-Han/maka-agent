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
import type { ProjectedLlmConnection } from '@maka/core/llm-connections';
import { buildCommandList } from '../../renderer/command-palette-commands.js';

function connection(overrides: Partial<ProjectedLlmConnection> = {}): ProjectedLlmConnection {
  return {
    connectionId: 'connection-1',
    revision: 1,
    slug: 'openai-live',
    name: 'OpenAI Live',
    provider: { packageId: 'external.provider', entryId: 'account', scope: 'profile', name: 'custom' },
    configuration: {},
    enabled: true,
    enabledModelIds: ['gpt-4.1'],
    models: [{ id: 'gpt-4.1' }],
    catalogEntries: [{ id: 'gpt-4.1', isDefault: false, canUseAsChatDefault: true, supportsVision: false, thinkingLevels: [] }],
    modelSource: 'fetched',
    ...overrides,
  };
}

const disabled = connection({
  connectionId: 'disabled',
  slug: 'disabled',
  enabled: false,
});

function commandIds(connections: ProjectedLlmConnection[], defaultSlug: string | null): string[] {
  return buildCommandList({
    locale: 'en',
    activeSessionId: undefined,
    themePref: 'auto',
    connections,
    defaultSlug,
    onNewChat: () => {},
    onOpenSettings: () => {},
    onOpenSettingsSection: () => {},
    onOpenShortcuts: () => {},
    onSetTheme: () => {},
    onTestConnection: () => {},
    onSetDefaultConnection: () => {},
  }).map((command) => command.id);
}

test('external providers remain actionable while disabled connections are excluded', () => {
  const live = connection({ connectionId: 'live', slug: 'live', name: 'Live' });
  const ids = commandIds([connection(), live, disabled], 'openai-live');
  assert.ok(ids.includes(`connection:set-default:${live.slug}`));
  assert.ok(ids.includes(`connection:test:${live.slug}`));
  assert.ok(!ids.includes(`connection:set-default:${disabled.slug}`));
  assert.ok(!ids.includes(`connection:test:${disabled.slug}`));
});

test('a stale in-memory default pointing at a disabled connection is not testable', () => {
  const ids = commandIds([disabled, connection()], disabled.slug);
  assert.ok(!ids.includes('diag:test-default'));
});

for (const locale of ['en', 'zh-CN'] as const) {
  test(`${locale} static shortcut hints preserve both platform variants`, () => {
    const commands = buildCommandList({
      locale,
      activeSessionId: 'session-1',
      themePref: 'auto',
      connections: [],
      defaultSlug: null,
      onNewChat: () => {},
      onOpenSideChat: () => {},
      onOpenSettings: () => {},
      onOpenSettingsSection: () => {},
      onOpenShortcuts: () => {},
      onSetTheme: () => {},
      onCopyDiagnostics: () => {},
    });
    const byId = new Map(commands.map((command) => [command.id, command]));
    assert.deepEqual(byId.get('action:side-chat')?.platformHint, {
      apple: '⌥⌘S',
      other: 'Ctrl+Alt+S',
    });
    assert.deepEqual(byId.get('action:open-settings')?.platformHint, {
      apple: '⌘,',
      other: 'Ctrl+,',
    });
    assert.equal(byId.get('diag:copy-diagnostics')?.platformHint?.other, 'Ctrl+Shift+D · ' +
      (locale === 'zh-CN' ? '脱敏日志 · 仅写入剪贴板' : 'Redacted logs · clipboard only'));
  });
}

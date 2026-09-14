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

export const modelId = 'fixture-model';
export const inputOnlyId = 'facts-input-only';
export const secret = 'model-overrides-fixture';
export const enabled = [
  modelId,
  inputOnlyId,
  ...Array.from({ length: 78 }, (_, i) => 'facts-page-' + i),
];
export const inputPin = {
  inputLimit: 20,
  apiProtocol: 'openai-chat',
  capabilities: { reasoning: true, parallelToolCalls: true },
};
export function fullPin(patch = {}) {
  return {
    displayName: 'Configured model',
    description: 'Configured description',
    apiProtocol: 'openai-chat',
    contextWindow: 64000,
    compactionThreshold: 64000,
    maxOutputTokens: 12345,
    knowledgeCutoff: '2025-01',
    vision: false,
    capabilities: {
      chat: true,
      reasoning: true,
      functionCalling: true,
      parallelToolCalls: false,
      imageGeneration: false,
      webSearch: false,
    },
    modalities: { input: ['text', 'image'], output: ['text'] },
    ...patch,
  };
}
export async function catalog(request) {
  let page = await request('connection.catalog.query', { kind: 'start' });
  assert.equal(page.kind, 'page');
  const revision = page.revision,
    items = [...page.items],
    pages = [page];
  while (page.nextCursor !== null) {
    page = await request('connection.catalog.query', {
      kind: 'continue',
      revision,
      cursor: page.nextCursor,
    });
    assert.equal(page.kind, 'page');
    assert.equal(page.revision, revision);
    items.push(...page.items);
    pages.push(page);
  }
  const positions = items.map(
    (item) => item.connectionIndex + ':' + item.kind + ':' + (item.itemIndex ?? 'header'),
  );
  assert.equal(new Set(positions).size, items.length);
  return { revision, items, pages };
}
export function modelItem(snapshot, index, id) {
  const found = snapshot.items.find(
    (item) =>
      item.connectionIndex === index && item.kind === 'catalog_entry' && item.entry.id === id,
  );
  assert(found, 'catalog entry missing for ' + id);
  return found.entry;
}
export function header(snapshot, id) {
  const found = snapshot.items.find(
    (item) => item.kind === 'connection' && item.connectionId === id,
  );
  assert(found);
  return found;
}
export async function updateDeclaration(request, row, pin) {
  const current = header(await catalog(request), row.connectionId);
  const result = await request('connection.catalog.update', {
    expected: { connectionId: current.connectionId, revision: current.revision },
    changes: {
      name: current.name,
      baseUrl: current.baseUrl,
      enabled: true,
      enabledModelIds: enabled,
      modelOverrides: { [modelId]: pin, [inputOnlyId]: inputPin, 'manual-disabled': {} },
    },
  });
  assert.equal(result.kind, 'committed');
}
export async function configure(request, baseUrl) {
  const rows = [];
  for (const [index, slug] of ['facts-a', 'facts-b'].entries()) {
    const created = await request('connection.catalog.create', {
      expectedCatalogRevision: (await catalog(request)).revision,
      connection: {
        slug,
        name: slug,
        providerType: 'openai',
        baseUrl,
        enabled: true,
        enabledModelIds: enabled,
        modelOverrides: {
          [modelId]: fullPin({ vision: index === 1, contextWindow: index === 0 ? 64000 : 32000 }),
          [inputOnlyId]: inputPin,
          'manual-disabled': {},
        },
      },
    });
    assert.equal(created.kind, 'committed');
    rows.push({ ...created.connection, slug });
    assert.equal(
      (
        await request('credential.vault.set', {
          locator: {
            scope: 'connection',
            connectionId: created.connection.connectionId,
            kind: 'api_key',
          },
          expected: null,
          expectedConnection: {
            ...created.connection,
            slug,
            providerType: 'openai',
            effectiveBaseUrl: baseUrl,
          },
          secret,
        })
      ).kind,
      'committed',
    );
  }
  return rows;
}
export function sessionInput(workspace, row, sessionId, model = modelId) {
  return {
    sessionId,
    workspace: { kind: 'host_path', path: workspace },
    permissionMode: 'explore',
    mode: 'bot',
    modelTarget: {
      kind: 'explicit',
      connectionId: row.connectionId,
      connectionSlug: row.slug,
      model,
    },
  };
}
export async function verifyCatalogPins(request, rows, workspace) {
  let pin = fullPin();
  const full = await catalog(request);
  assert(full.pages.length > 1);
  assert.equal(
    full.items.filter((item) => item.kind === 'model').length,
    0,
    'declarations do not become discovered inventory',
  );
  assert.equal(modelItem(full, 0, modelId).contextWindow, 64000);
  assert.equal(modelItem(full, 1, modelId).contextWindow, 32000);
  assert.equal(modelItem(full, 0, modelId).supportsVision, false);
  assert.equal(modelItem(full, 1, modelId).supportsVision, true);
  for (let index = 0; index < 2; index++) {
    const manual = full.items.find(
      (item) =>
        item.connectionIndex === index &&
        item.kind === 'catalog_entry' &&
        item.entry.id === 'manual-disabled',
    );
    assert.deepEqual(manual.modelOverride, {});
  }
  await updateDeclaration(request, rows[0], {
    ...pin,
    capabilities: { ...pin.capabilities, chat: false },
  });
  await assert.rejects(
    request('session.create', sessionInput(workspace, rows[0], 'facts-chat-disabled')),
    (error) => error.code === 'invalid_request',
  );
  await updateDeclaration(request, rows[0], pin);
  const first = await request('connection.catalog.query', { kind: 'start' });
  assert(first.nextCursor);
  pin = { ...pin, displayName: 'Configured after first page' };
  await updateDeclaration(request, rows[0], pin);
  const changed = await request('connection.catalog.query', {
    kind: 'continue',
    revision: first.revision,
    cursor: first.nextCursor,
  });
  assert.equal(changed.kind, 'revision_changed');
  const fresh = await catalog(request);
  assert.equal(modelItem(fresh, 0, modelId).displayName, pin.displayName);
  assert.equal(modelItem(fresh, 1, modelId).displayName, fullPin().displayName);
  return { pin, full, changed, fresh };
}

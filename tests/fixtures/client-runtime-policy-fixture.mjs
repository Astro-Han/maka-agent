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
import { createDefaultRuntimePolicy } from '../../packages/core/src/runtime-policy.ts';

export const proxyLocator = { scope: 'network_proxy', kind: 'password' };
export const webLocator = { scope: 'web_search', provider: 'tavily', kind: 'api_key' };
export const requestFor = (connection) => (operation, input) =>
  connection.request(operation, input, 5000);

export async function settingsSnapshot(request) {
  // The same three remote reads required by the original Desktop Settings load.
  const [policy, proxy, web] = await Promise.all([
    request('runtime.policy.query', {}),
    request('credential.vault.query', { locator: proxyLocator }),
    request('credential.vault.query', { locator: webLocator }),
  ]);
  for (const [result, locator] of [
    [proxy, proxyLocator],
    [web, webLocator],
  ]) {
    assert.equal(result.kind, 'status');
    assert.deepEqual(result.status.locator, locator);
    assert(!Object.hasOwn(result.status, 'secret'));
    if (!result.status.configured) {
      assert.equal(result.status.credentialId, null);
      assert.equal(result.status.revision, null);
      assert.equal(result.status.updatedAt, null);
    } else {
      assert.equal(typeof result.status.credentialId, 'string');
      assert(result.status.credentialId.length > 0);
      assert(result.status.revision > 0);
      assert(Number.isSafeInteger(result.status.updatedAt) && result.status.updatedAt >= 0);
    }
  }
  // request() has already run the unmodified source full-shape decoder.
  assert.deepEqual(policy.policy, {
    ...createDefaultRuntimePolicy(),
    chatDefaults: policy.policy.chatDefaults,
  });
  return { policy, proxy, web };
}

export async function configureModel(request, baseUrl = 'http://127.0.0.1:9/v1') {
  const initial = await request('connection.catalog.query', { kind: 'start' });
  assert.equal(initial.revision, 0);
  const created = await request('connection.catalog.create', {
    expectedCatalogRevision: initial.revision,
    connection: {
      slug: 'runtime-policy',
      name: 'Runtime policy fixture',
      providerType: 'openai-compatible',
      baseUrl,
      enabled: true,
      enabledModelIds: ['fixture-model'],
    },
  });
  assert.equal(created.kind, 'committed');
  const credential = await request('credential.vault.set', {
    locator: {
      scope: 'connection',
      connectionId: created.connection.connectionId,
      kind: 'api_key',
    },
    expected: null,
    expectedConnection: {
      ...created.connection,
      slug: 'runtime-policy',
      providerType: 'openai-compatible',
      effectiveBaseUrl: baseUrl,
    },
    secret: 'runtime-policy-test-model-key',
  });
  assert.equal(credential.kind, 'committed');
  const selected = await request('connection.catalog.set-default-target', {
    expectedCatalogRevision: created.catalogRevision,
    target: { connectionId: created.connection.connectionId, modelId: 'fixture-model' },
  });
  assert.equal(selected.kind, 'committed');
  const proxy = await request('credential.vault.set', {
    locator: proxyLocator,
    expected: null,
    secret: 'runtime-policy-test-proxy-secret',
  });
  assert.equal(proxy.kind, 'committed');
  return { connection: created.connection, proxy: proxy.status };
}

export function createInput(workspace, sessionId, permissionMode) {
  return {
    sessionId,
    name: sessionId,
    workspace: { kind: 'host_path', path: workspace },
    modelTarget: { kind: 'default' },
    ...(permissionMode === undefined ? {} : { permissionMode }),
  };
}

export function modelDefault(session, permissionMode) {
  assert.equal(session.permissionMode, permissionMode);
  assert.equal(
    Object.hasOwn(session, 'thinkingLevel'),
    false,
    'omitted thinking preserves the composer choice of model default',
  );
}

export const querySession = async (request, id) =>
  (await request('session.catalog.query', { kind: 'get', sessionId: id })).session;

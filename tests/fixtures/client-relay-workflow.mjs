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
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { watchSession } from './client-subscription.mjs';
import { relayFixture } from './client-relay-fixture.mjs';

const models = ['unknown-relay', 'relay/gpt-5.2', 'gpt-5-relay', 'gpt-5-nano', 'plain-model'];
export async function verifyRelayOptions(connection, workspace, reopened) {
  const request = (operation, input) => connection.request(operation, input, 3000);
  const catalog = () => request('connection.catalog.query', { kind: 'start' });
  const query = async (sessionId) =>
    (await request('session.catalog.query', { kind: 'get', sessionId })).session;
  const snapshot = async () => ({
    catalog: await catalog(),
    sessions: await Promise.all(
      [...models, 'chat-unsupported'].map((model) => query(model.replaceAll(/[/.]/g, '-'))),
    ),
  });
  const path = join(workspace, 'relay-options.json');
  if (reopened) {
    assert.equal(JSON.stringify(await snapshot()), await readFile(path, 'utf8'));
    return;
  }
  const initial = await catalog();
  const row = initial.items.find((item) => item.kind === 'connection');
  const provider = await relayFixture(Number(new URL(row.baseUrl).port));
  const profiles = {
    'unknown-relay': { thinkingLevels: ['high', 'max'], vision: true, contextWindow: 32000 },
    'gpt-5-relay': { serviceTier: 'fast' },
    'gpt-5-nano': { serviceTier: 'fast' },
    'plain-model': { serviceTier: 'fast' },
  };
  const update = async (modelOverrides) => {
    const current = (await catalog()).items.find((item) => item.kind === 'connection');
    assert.equal(
      (
        await request('connection.catalog.update', {
          expected: { connectionId: row.connectionId, revision: current.revision },
          changes: {
            name: row.name,
            baseUrl: row.baseUrl,
            enabled: true,
            enabledModelIds: models,
            modelOverrides,
          },
        })
      ).kind,
      'committed',
    );
  };
  const configure = async (sessionId, thinkingLevel) => {
    const before = await query(sessionId);
    const result = await request('session.configuration.update', {
      sessionId,
      expectedRevision: before.revision,
      patch: { thinkingLevel },
    });
    assert.equal(result.kind, 'committed');
    assert.deepEqual(await query(sessionId), result.session);
    if (thinkingLevel === null) assert(!Object.hasOwn(result.session, 'thinkingLevel'));
    else assert.equal(result.session.thinkingLevel, thinkingLevel);
  };
  const rejectedTurn = async (sessionId, turnId) => {
    const before = await query(sessionId);
    await assert.rejects(
      request('turn.start', {
        sessionId,
        turnId,
        content: { text: 'must not dispatch' },
        maxSteps: 1,
      }),
      (error) => error.code === 'operation_unavailable',
    );
    assert.deepEqual(await query(sessionId), before);
    await assert.rejects(
      request('turn.query', { sessionId, turnId }),
      (error) => error.code === 'not_found',
    );
    provider.check();
  };
  try {
    assert.equal(
      (
        await request('credential.vault.set', {
          locator: { scope: 'connection', connectionId: row.connectionId, kind: 'api_key' },
          expected: null,
          expectedConnection: {
            connectionId: row.connectionId,
            revision: row.revision,
            slug: row.slug,
            providerType: row.providerType,
            effectiveBaseUrl: row.baseUrl,
          },
          secret: 'relay-fixture',
        })
      ).kind,
      'committed',
    );
    await update(profiles);
    const items = (await catalog()).items;
    const published = items.find(
      (item) => item.kind === 'catalog_entry' && item.entry.id === 'unknown-relay',
    );
    assert.deepEqual(published.entry.thinkingLevels, ['high', 'max']);
    assert.equal(published.entry.supportsVision, true);
    assert.equal(
      items.find((item) => item.kind === 'model' && item.model.id === 'unknown-relay').model
        .capabilities.parallelToolCalls,
      false,
    );
    for (const model of models) {
      const sessionId = model.replaceAll(/[/.]/g, '-');
      await request('session.create', {
        sessionId,
        workspace: { kind: 'host_path', path: workspace },
        mode: 'bot',
        modelTarget: {
          kind: 'explicit',
          connectionId: row.connectionId,
          connectionSlug: row.slug,
          model,
        },
      });
      const live = await watchSession(connection, sessionId);
      const levels = model === 'unknown-relay' ? ['high', 'max', null] : [undefined];
      try {
        for (const [index, level] of levels.entries()) {
          if (level !== undefined) await configure(sessionId, level);
          provider.expect({
            model,
            parallel: model !== 'unknown-relay',
            reasoning:
              model === 'unknown-relay'
                ? // The native SDK defaults explicit effort to detailed summary;
                  // Maka's canonical-family policy alone requests auto summary.
                  level === null
                  ? undefined
                  : { effort: level, summary: 'detailed' }
                : model === 'relay/gpt-5.2' || model === 'gpt-5-nano'
                  ? { effort: 'medium', summary: 'auto' }
                  : undefined,
            tier: model === 'gpt-5-relay' ? 'fast' : undefined,
          });
          const turnId = sessionId + '-' + index;
          await request('turn.start', {
            sessionId,
            turnId,
            content: { text: 'reply once' },
            maxSteps: 1,
          });
          await live.waitFor(
            (frame) =>
              frame.kind === 'subscription.session_projection' &&
              frame.snapshot.rootTurn?.turnId === turnId &&
              ['completed', 'failed', 'cancelled'].includes(frame.snapshot.rootTurn.status),
          );
          provider.check();
          assert.equal((await request('turn.query', { sessionId, turnId })).status, 'completed');
          if (model === 'unknown-relay') {
            const context = await request('context.diagnostics.query', { sessionId });
            assert.equal(context.status, 'available');
            assert.equal(context.providerId, row.providerType);
            assert.equal(context.contextWindow, 32000);
          }
        }
      } finally {
        await live.close();
      }
    }
    await configure('unknown-relay', 'max');
    await update({ ...profiles, 'unknown-relay': { thinkingLevels: ['low'], vision: false } });
    await rejectedTurn('unknown-relay', 'stale-profile');
    await configure('unknown-relay', null);
    await update(profiles);
    provider.verify();
    await writeFile(path, JSON.stringify(await snapshot()));
  } finally {
    await provider.close();
  }
}

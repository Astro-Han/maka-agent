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
import {
  configureModel,
  createInput,
  modelDefault,
  querySession,
  requestFor,
  settingsSnapshot,
} from './client-runtime-policy-fixture.mjs';

export async function verifyRuntimePolicy(connection, workspace, reopened, connectSibling) {
  const request = requestFor(connection);
  const path = join(workspace, 'runtime-policy-fixture.json');
  const sibling = await connectSibling();
  const other = requestFor(sibling);
  const notices = [];
  const unsubscribe = sibling.subscribeConfigurationChanges((revision) => notices.push(revision));
  const barrier = async () => {
    await connection.status(5000);
    await sibling.status(5000);
    for (let i = 1; i < notices.length; i++) assert(notices[i] > notices[i - 1]);
  };
  const mutate = (expectedRevision, value, send = request) =>
    send('runtime.policy.mutate', {
      expectedRevision,
      operation: { kind: 'set_chat_defaults', value },
    });
  try {
    if (reopened) {
      const saved = JSON.parse(await readFile(path, 'utf8'));
      assert.deepEqual(await settingsSnapshot(request), saved.finalSettings);
      for (const { input, snapshot } of saved.sessions) {
        assert.deepEqual(await querySession(request, input.sessionId), snapshot);
        assert.deepEqual(
          await request('session.create', input),
          snapshot,
          'exact create retry cannot re-resolve changed defaults after restart',
        );
      }
      assert.deepEqual(await request('connection.catalog.query', { kind: 'start' }), saved.catalog);
      await barrier();
      assert.deepEqual(notices, [], 'reads and exact Session retries do not change configuration');
      console.log(
        JSON.stringify({
          check: 'original-client-runtime-policy-reopen',
          revision: saved.finalSettings.policy.revision,
          sessionIds: saved.sessions.map(({ input }) => input.sessionId),
        }),
      );
      return saved;
    }

    const initialSettings = await settingsSnapshot(request);
    assert.equal(initialSettings.policy.revision, 0);
    assert.deepEqual(initialSettings.policy.policy.chatDefaults, {
      sandboxMode: 'workspace-write',
    });
    assert.equal(initialSettings.proxy.status.configured, false);
    await barrier();
    assert.deepEqual(notices, []);
    const configured = await configureModel(request);
    const configuredSettings = await settingsSnapshot(request);
    assert.deepEqual(configuredSettings.proxy.status, configured.proxy);
    assert.equal(configuredSettings.proxy.status.configured, true);
    assert.deepEqual(configuredSettings.policy, initialSettings.policy);
    await barrier();
    assert(
      notices.length >= 3,
      'catalog and vault commits already advanced the configuration feed',
    );

    const sessions = [];
    const create = async (id, sandboxMode) => {
      const input = createInput(workspace, id, sandboxMode);
      const snapshot = await request('session.create', input);
      sessions.push({ input, snapshot });
      return snapshot;
    };
    const old = await create('runtime-policy-old');
    modelDefault(old, 'workspace-write');

    const value = {
      sandboxMode: 'danger-full-access',
      thinkingLevel: 'high',
    };
    const beforeRace = notices.length;
    const raced = await Promise.all([mutate(0, value), mutate(0, value, other)]);
    const committed = raced.filter((result) => result.kind === 'committed');
    const conflicts = raced.filter((result) => result.kind === 'revision_conflict');
    assert.deepEqual(committed, [{ kind: 'committed', revision: 1 }]);
    assert.deepEqual(conflicts, [
      {
        kind: 'revision_conflict',
        expectedRevision: 0,
        actualRevision: 1,
      },
    ]);
    await barrier();
    assert.equal(notices.length, beforeRace + 1, 'only the committed CAS emits invalidation');
    assert.notEqual(
      notices.at(-1),
      committed[0].revision,
      'configuration.changed revision is its own feed, not the policy revision',
    );
    const afterRace = await settingsSnapshot(request);
    assert.deepEqual(afterRace.policy.policy.chatDefaults, value);
    assert.equal(afterRace.policy.revision, 1);

    const beforeSame = notices.length;
    const sameValue = await mutate(1, value);
    assert.deepEqual(sameValue, { kind: 'committed', revision: 2 });
    await barrier();
    assert.equal(notices.length, beforeSame + 1, 'same-value legal mutation still commits');
    const inherited = await create('runtime-policy-inherited');
    modelDefault(inherited, 'danger-full-access');
    const explicit = await create('runtime-policy-explicit', 'workspace-write');
    modelDefault(explicit, 'workspace-write');
    assert.deepEqual(await querySession(request, old.id), old);

    const beforeUnavailable = notices.length;
    const beforePolicy = await settingsSnapshot(request);
    for (const operation of [
      { kind: 'set_memory', value: { enabled: false, agentReadEnabled: false } },
      { kind: 'set_privacy', value: { incognitoActive: true } },
    ]) {
      await assert.rejects(
        request('runtime.policy.mutate', { expectedRevision: 2, operation }),
        (error) => error.code === 'operation_unavailable',
        'valid unsupported mutation must not pretend to commit',
      );
    }
    assert.deepEqual(await settingsSnapshot(request), beforePolicy);
    await barrier();
    assert.equal(notices.length, beforeUnavailable, 'unavailable mutation must not notify');

    const changed = await mutate(2, { sandboxMode: 'workspace-write', thinkingLevel: 'low' });
    assert.deepEqual(changed, { kind: 'committed', revision: 3 });
    for (const { input, snapshot } of sessions) {
      assert.deepEqual(await querySession(request, input.sessionId), snapshot);
      assert.deepEqual(
        await request('session.create', input),
        snapshot,
        'default changes never alter existing or exactly retried Sessions',
      );
    }
    const cleared = await mutate(3, { sandboxMode: 'danger-full-access' });
    assert.deepEqual(cleared, { kind: 'committed', revision: 4 });
    const finalSettings = await settingsSnapshot(request);
    assert.equal(finalSettings.policy.revision, 4);
    assert.deepEqual(finalSettings.policy.policy.chatDefaults, {
      sandboxMode: 'danger-full-access',
    });
    assert.equal(
      Object.hasOwn(finalSettings.policy.policy.chatDefaults, 'thinkingLevel'),
      false,
      'set_chat_defaults replaces the whole object; omission clears thinking',
    );
    await barrier();
    assert.equal(notices.length, beforeUnavailable + 2);
    const catalog = await request('connection.catalog.query', { kind: 'start' });
    const saved = {
      initialSettings,
      configuredSettings,
      afterRace,
      finalSettings,
      sessions,
      catalog,
      raced,
      sameValue,
      changed,
      cleared,
      configurationNotices: notices,
    };
    await writeFile(path, JSON.stringify(saved));
    console.log(
      JSON.stringify({
        check: 'original-client-runtime-policy',
        revision: finalSettings.policy.revision,
        sessionIds: sessions.map(({ input }) => input.sessionId),
        configurationNotices: notices,
      }),
    );
    return saved;
  } finally {
    unsubscribe();
    await sibling.close();
  }
}

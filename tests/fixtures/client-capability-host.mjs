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
import { readFile, writeFile, realpath, access, rename } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';
import { join } from 'node:path';
import { decodeStoredMessage } from '../../packages/core/src/session.ts';
import { watchSession } from './client-subscription.mjs';
import { capabilityModelFixture, capabilityResult } from './client-capability-model-fixture.mjs';

const sessionId = 'capability-host-session';

async function transcript(connection) {
  const observer = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
  try {
    return await observer.subscription.loadTranscript(decodeStoredMessage);
  } finally {
    await observer.close();
  }
}

export async function verifyCapabilityHost(connection, workspace, reopened, openConnection) {
  const snapshot = join(workspace, 'capability-rows.json');
  if (reopened) {
    assert.equal(JSON.stringify(await transcript(connection)), await readFile(snapshot, 'utf8'));
    for (let index = 1; index <= 3; index++)
      assert.equal(await readFile(join(workspace, `effect-${index}`), 'utf8'), `${index}`);
    await assert.rejects(
      connection.replaceClientCapabilities({ offers: () => [] }, { sessionId }),
      (error) => error.code === 'invalid_request',
    );
    console.log('original-client-capability-host-reopened');
    return;
  }
  // Test-only checkpoints: Rust verifies independently committed canonical facts.
  async function checkpoint(kind, index, frame = {}) {
    const file = join(workspace, `${kind}-${index}`);
    await writeFile(`${file}.tmp`, JSON.stringify(frame));
    await rename(`${file}.tmp`, `${file}.json`);
    for (;;) {
      try {
        await access(`${file}.ok`);
        return;
      } catch {
        await delay(5);
      }
    }
  }
  const registrations = new Map(),
    closed = [],
    calls = [];
  let failure;
  const provider = (generation) => ({
    offers: () =>
      ['none', 'cwd'].map((hostPathAccess) => ({
        offerId: hostPathAccess,
        version: '1',
        affinity: 'session',
        hostPathAccess,
        label: `Host ${hostPathAccess}`,
        tools: [
          {
            serverId: `host-${hostPathAccess}`,
            name: 'effect',
            inputSchema: { type: 'object' },
          },
        ],
      })),
    async call(frame, { accept, signal }) {
      try {
        const index = calls.length + 1;
        assert.equal(generation, index <= 2 ? 1 : 2);
        assert.equal(frame.registrationId, registrations.get(generation));
        assert.equal(frame.source.sessionId, sessionId);
        assert.equal(frame.source.turnId, index <= 2 ? 'capability-first' : 'capability-second');
        assert.equal(frame.serverId, `host-${frame.offerId}`);
        assert.equal(frame.toolName, 'effect');
        assert.deepEqual(frame.arguments, { index });
        assert(frame.toolCallId.startsWith('tool_'));
        assert.notEqual(frame.toolCallId, 'provider:reused');
        if (frame.offerId === 'none') assert(!Object.hasOwn(frame, 'cwd'));
        else assert.equal(frame.cwd, await realpath(workspace));
        if (index === 1) {
          const replacement = await connection.replaceClientCapabilities(provider(2), {
            timeoutMs: 3000,
          });
          registrations.set(2, replacement.registrationId);
          assert.notEqual(replacement.registrationId, registrations.get(1));
          assert.deepEqual(closed, [], 'old snapshot must keep its provider alive');
          const other = await openConnection();
          try {
            const replay = {
              sessionId,
              turnId: 'capability-first',
              content: { text: 'capability-first' },
              maxSteps: 4,
            };
            assert.equal((await other.request('turn.start', replay, 3000)).kind, 'started');
            await assert.rejects(
              other.request(
                'turn.start',
                {
                  ...replay,
                  turnId: 'capability-busy',
                },
                3000,
              ),
              (error) => error.code === 'session_busy',
            );
          } finally {
            await other.close();
          }
        }
        await accept({ kind: 'none' });
        assert.equal(signal.aborted, false);
        await checkpoint('dispatch', index, frame);
        await writeFile(join(workspace, `effect-${index}`), `${index}`, { flag: 'wx' });
        calls.push(frame);
        return capabilityResult(index);
      } catch (error) {
        failure = error;
        throw error;
      }
    },
    close() {
      closed.push(generation);
    },
  });
  const model = await capabilityModelFixture(
    checkpoint,
    () => {
      if (failure) throw failure;
    },
    (error) => {
      failure = error;
    },
  );
  const request = (operation, input) => connection.request(operation, input, 3000);
  try {
    const baseUrl = model.baseUrl;
    const created = await request('connection.catalog.create', {
      expectedCatalogRevision: 0,
      connection: {
        slug: 'capability-fixture',
        name: 'Capability fixture',
        providerType: 'openai',
        baseUrl,
        enabled: true,
        enabledModelIds: ['gpt-5.2'],
        modelOverrides: { 'gpt-5.2': { codeMode: false } },
      },
    });
    assert.equal(created.kind, 'committed');
    const basis = created.connection;
    assert.equal(
      (
        await request('credential.vault.set', {
          locator: { scope: 'connection', connectionId: basis.connectionId, kind: 'api_key' },
          expected: null,
          expectedConnection: {
            ...basis,
            slug: 'capability-fixture',
            providerType: 'openai',
            effectiveBaseUrl: baseUrl,
          },
          secret: 'dummy-capability-fixture',
        })
      ).kind,
      'committed',
    );
    await request('connection.catalog.set-default-target', {
      expectedCatalogRevision: created.catalogRevision,
      target: { connectionId: basis.connectionId, modelId: 'gpt-5.2' },
    });
    await request('session.create', {
      sessionId,
      workspace: { kind: 'host_path', path: workspace },
      modelTarget: { kind: 'default' },
      sandboxMode: 'danger-full-access',
    });
    registrations.set(
      1,
      (await connection.replaceClientCapabilities(provider(1), { timeoutMs: 3000 })).registrationId,
    );
    for (const turnId of ['capability-first', 'capability-second']) {
      const live = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 });
      try {
        await request('turn.start', { sessionId, turnId, content: { text: turnId }, maxSteps: 4 });
        await live.waitFor(
          (frame) =>
            frame.kind === 'subscription.session_projection' &&
            frame.snapshot.rootTurn?.turnId === turnId &&
            ['completed', 'failed', 'cancelled'].includes(frame.snapshot.rootTurn.status),
        );
        if (failure) throw failure;
        assert.equal((await request('turn.query', { sessionId, turnId })).status, 'completed');
      } finally {
        await live.close();
      }
    }
    await connection.unregisterClientCapabilities({ timeoutMs: 3000 });
    for (let attempt = 0; attempt < 100 && closed.length !== 2; attempt++) await delay(5);
    assert.deepEqual(closed.sort(), [1, 2]);
    model.verify();
    assert.equal(calls.length, 3);
    const rows = await transcript(connection);
    const allCalls = rows.filter((row) => row.type === 'tool_call');
    assert.equal(allCalls.length, 5);
    assert.equal(allCalls.filter((row) => row.toolName === 'tool_search').length, 2);
    const toolCalls = allCalls.filter((row) => calls.some((call) => call.toolCallId === row.id));
    assert.equal(toolCalls.length, 3);
    assert.deepEqual(
      toolCalls.map((row) => row.id),
      calls.map((call) => call.toolCallId),
    );
    assert.equal(new Set(toolCalls.map((row) => row.id)).size, 3);
    assert.equal(rows.filter((row) => row.type === 'tool_result' && !row.isError).length, 5);
    const results = rows.filter(
      (row) =>
        row.type === 'tool_result' && calls.some((call) => call.toolCallId === row.toolUseId),
    );
    for (const [index, result] of results.entries()) {
      assert.equal(result.toolUseId, calls[index].toolCallId);
      assert.deepEqual(result.content, { kind: 'json', value: capabilityResult(index + 1) });
    }
    await writeFile(snapshot, JSON.stringify(rows));
    const retired = [];
    await Promise.all(
      [sessionId, 'not-created-yet'].map((id) =>
        connection.replaceClientCapabilities(
          {
            offers: () => [],
            close: () => {
              retired.push(id);
            },
          },
          { sessionId: id },
        ),
      ),
    );
    await request('session.lifecycle.set', { sessionId, state: 'archived' });
    for (let attempt = 0; attempt < 100 && retired.length === 0; attempt++) await delay(5);
    assert.deepEqual(retired, [sessionId], 'archive releases only its scoped publication');
    await assert.rejects(
      connection.replaceClientCapabilities({ offers: () => [] }, { sessionId }),
      (error) => error.code === 'invalid_request',
    );
    await connection.unregisterClientCapabilities({ sessionId: 'not-created-yet' });
    console.log('original-client-capability-host');
  } finally {
    await model.close();
  }
}

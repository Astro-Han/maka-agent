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
import test from 'node:test';
import { DesktopRuntimeHostClientError } from '../runtime-host-client.js';
import type { IpcMain } from 'electron';
import type { SessionCatalogProjection, SessionCreateInput } from '@maka/runtime-host/protocol';
import {
  registerRuntimeHostSessionCatalogIpc,
  toDesktopHostSessionSummary,
  type RuntimeHostSessionCatalogIpcDeps,
} from '../runtime-host-session-catalog-ipc-main.js';

test('maps Runtime Host live run state without collapsing unknown and known-empty', () => {
  const unknown = toDesktopHostSessionSummary(projection());
  const knownEmpty = toDesktopHostSessionSummary(
    projection({ liveRunState: { schemaVersion: 1, runningTurnIds: [] } }),
  );
  const running = toDesktopHostSessionSummary(
    projection({ liveRunState: { schemaVersion: 1, runningTurnIds: ['turn-live'] } }),
  );

  assert.equal(Object.hasOwn(unknown, 'runningTurnIds'), false);
  assert.deepEqual(knownEmpty.runningTurnIds, []);
  assert.deepEqual(running.runningTurnIds, ['turn-live']);
});

test('exact Session reads retain revision and live execution without depending on list visibility', async () => {
  const ipc = ipcHarness();
  const deps = createDeps([]);
  let present = true;
  deps.client.getSession = async (id) => {
    assert.equal(id, 'managed');
    return present ? projection({ id, revision: 7 }) : null;
  };
  deps.runningTurnIds = (id) => { assert.equal(id, 'managed'); return ['live']; };
  registerRuntimeHostSessionCatalogIpc(deps, ipc as unknown as IpcMain);
  const result = await ipc.invoke('sessions:get', 'managed') as { id: string; revision: number; runningTurnIds: string[] };
  assert.equal(result.id, 'managed');
  assert.equal(result.revision, 7);
  assert.deepEqual(result.runningTurnIds, ['live']);
  present = false;
  assert.equal(await ipc.invoke('sessions:get', 'managed'), null);
});

test('session creation forwards the caller name for a mode that carries none', async () => {
  const creates: SessionCreateInput[] = [];
  const ipc = ipcHarness();
  registerRuntimeHostSessionCatalogIpc(createDeps(creates), ipc as unknown as IpcMain);

  await ipc.invoke('sessions:create', { mode: 'bot', name: '飞书 任务' });
  await assert.rejects(
    () => ipc.invoke('sessions:create', { mode: 'deep_research', name: '飞书 任务' }),
    /Invalid session start mode/,
  );

  assert.deepEqual(
    creates.map((input) => [input.mode, input.name]),
    [
      ['bot', '飞书 任务'],
    ],
  );
});

test('session creation forwards a plugin executor without a model target', async () => {
  const creates: SessionCreateInput[] = [];
  const ipc = ipcHarness();
  registerRuntimeHostSessionCatalogIpc(createDeps(creates), ipc as unknown as IpcMain);

  const executorSettings = { model: 'executor-model', thinkingLevel: 'max' };
  await ipc.invoke('sessions:create', { executorId: 'codex.app-server', executorSettings });

  assert.equal(creates[0]?.executorId, 'codex.app-server');
  assert.equal(creates[0]?.modelTarget, undefined);
  assert.deepEqual(creates[0]?.executorSettings, executorSettings);
  assert.equal(creates[0]?.thinkingLevel, undefined);
  await assert.rejects(ipc.invoke('sessions:create', { executorSettings }), /Invalid plugin executor settings/);
  await assert.rejects(
    ipc.invoke('sessions:create', {
      executorId: 'codex',
      llmConnectionId: 'connection-1',
      llmConnectionSlug: 'openai',
      model: 'gpt-5',
    }),
    /cannot include a model target/,
  );
});

test('moves only the requested Session and keeps detach cwd paired with its revision', async () => {
  const ipc = ipcHarness();
  const deps = createDeps([]);
  const moves: unknown[] = [];
  const changed: unknown[] = [];
  let current: SessionCatalogProjection | null = projection({ revision: 7 });
  let conflict = false;
  deps.client.getSession = async () => current;
  deps.client.relocateSessionWorkspace = async (id, revision, workspace) => {
    moves.push({ id, revision, workspace });
    if (conflict) throw new DesktopRuntimeHostClientError('revision_conflict', 'Concurrent move');
    assert.ok(current);
    current = projection({ revision: revision + 1, workspace: { target: workspace,
      hostCwd: workspace.kind === 'host_path' ? workspace.path : '/destination' } });
    return current;
  };
  deps.emitSessionsChanged = (...args) => { changed.push(args); };
  registerRuntimeHostSessionCatalogIpc(deps, ipc as unknown as IpcMain);
  assert.equal((await ipc.invoke('sessions:moveToProject', 'session-1', 'project-2') as { ok: boolean }).ok, true);
  assert.equal((await ipc.invoke('sessions:moveToProject', 'session-1', null) as { ok: boolean }).ok, true);
  conflict = true;
  assert.deepEqual(await ipc.invoke('sessions:moveToProject', 'session-1', null), { ok: false, code: 'operation_conflict' });
  assert.deepEqual(moves, [
    { id: 'session-1', revision: 7, workspace: { kind: 'project', projectId: 'project-2' } },
    { id: 'session-1', revision: 8, workspace: { kind: 'host_path', path: '/destination' } },
    { id: 'session-1', revision: 9, workspace: { kind: 'host_path', path: '/destination' } },
  ]);
  assert.deepEqual(changed, [['updated', 'session-1'], ['updated', 'session-1']]);
  await assert.rejects(ipc.invoke('sessions:moveToProject', 'session-1', {}), /Invalid project/);
  current = null;
  assert.deepEqual(await ipc.invoke('sessions:moveToProject', 'session-1', null), { ok: false, code: 'not_found' });
  assert.equal(moves.length, 3);
});

type IpcHandler = Parameters<Pick<IpcMain, 'handle'>['handle']>[1];

function ipcHarness() {
  const handlers = new Map<string, IpcHandler>();
  return {
    handle(channel: string, handler: IpcHandler) {
      handlers.set(channel, handler);
    },
    async invoke(channel: string, ...args: unknown[]): Promise<unknown> {
      const handler = handlers.get(channel);
      assert.ok(handler, `missing handler: ${channel}`);
      return handler({} as never, ...args);
    },
  };
}

function createDeps(creates: SessionCreateInput[]): RuntimeHostSessionCatalogIpcDeps {
  return {
    client: {
      createSession: async (input: SessionCreateInput) => {
        creates.push(input);
        return projection({ id: input.sessionId });
      },
    } as unknown as RuntimeHostSessionCatalogIpcDeps['client'],
    runningTurnIds: () => [],
    resolveCreateProject: async () => ({ kind: 'host_path', path: '/workspace' }),
    emitSessionsChanged: () => {},
    releaseSessionResources: () => {},
    sessionCopyCleanup: {
      ownCreation: async <T>(_creation: unknown, operation: () => Promise<T>) => operation(),
      recover: async () => ({ removed: [], failed: [] }),
    } as unknown as RuntimeHostSessionCatalogIpcDeps['sessionCopyCleanup'],
  };
}

function projection(overrides: Partial<SessionCatalogProjection> = {}): SessionCatalogProjection {
  return {
    id: 'session-1',
    revision: 1,
    workspace: {
      target: { kind: 'host_path', path: '/workspace' },
      hostCwd: '/workspace',
    },
    createdAt: 1,
    activityAt: 2,
    name: 'Session',
    isFlagged: false,
    isArchived: false,
    labels: [],
    labelsTruncated: false,
    hasUnread: false,
    status: 'active',
    backend: 'ai-sdk',
    llmConnectionId: 'connection-1',
    llmConnectionSlug: 'openai-main',
    connectionLocked: true,
    model: 'gpt-5',
    sandboxMode: 'workspace-write',
    approvalPolicy: {kind: 'on-request'},
    collaborationMode: 'agent',
    orchestrationMode: 'default',
    ...overrides,
  };
}

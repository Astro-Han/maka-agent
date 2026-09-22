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
import { EventEmitter } from 'node:events';
import { deferred } from '@maka/core/test-only/async-primitives';
import { createDefaultRuntimePolicy } from '@maka/core/runtime-policy';
import type { PluginRemoteInput, PluginRemoteResult } from '@maka/runtime-host/protocol';
import { HOST_OPERATION_SPECS } from '@maka/runtime-host/protocol';
import type { IpcHandler, ReconnectableReadIpcMain } from '../ipc-reconnect-policy.js';
import type { DesktopRuntimeHostClient } from '../runtime-host-client.js';
import { registerRuntimeHostSearchIpc } from '../runtime-host-search-ipc-main.js';
import { RuntimeHostReconnectingIpcMain } from '../runtime-host-reconnecting-ipc-main.js';
import { desktopSessionKey } from '../../shared/runtime-host-identity.js';

const page = {
  complete: true,
  matches: [
    { kind: 'title', sessionId: 'history', title: 'Search history' },
    { kind: 'passage', sessionId: 'history', title: 'Search history',
      turnId: 'turn', messageId: 'message', sequence: 317, text: 'late matching passage', truncated: true },
  ],
};
const document = '00000000-0000-4000-8000-000000000001';

test('public Recall search preserves Host identity and canonical message coordinates', async () => {
  const fixture = remoteClient(async () => ({ kind: 'value', value: page }));
  const { handlers, event } = register(fixture.client);
  const results = await handlers.get('search:thread')!(event, { source: 'thread', query: 'matching', limit: 10 });
  assert.ok(Array.isArray(results));
  const sessionId = desktopSessionKey({ hostId: 'host-b', sessionId: 'history' });
  assert.deepEqual(results[0].target, { kind: 'thread', sessionId });
  assert.deepEqual(results[1].target, { kind: 'thread', sessionId, turnId: 'turn', sequence: 317, messageId: 'message' });
  assert.equal(results[1].truncated, true);
  const call = fixture.requests.find((request) => request.kind === 'call');
  assert.ok(call?.kind === 'call');
  assert.deepEqual(call.binding, { packageId: 'maka.recall', method: 'search', sessionId: null });
  assert.deepEqual(call.input, { terms: ['matching'], limit: 10 });
  assert.deepEqual(fixture.requests.at(-1), { kind: 'close_document', document });
});

test('invalid/private searches do not start Remote work; incomplete empty pages are not false negatives', async () => {
  const fixture = remoteClient(async () => ({ kind: 'value', value: { complete: false, matches: [] } }));
  const { handlers, event } = register(fixture.client);
  const search = handlers.get('search:thread')!;
  assert.equal((await search(event, { source: 'thread', query: '' })).reason, 'invalid_query');
  fixture.setIncognito(true);
  assert.equal((await search(event, { source: 'thread', query: 'valid' })).reason, 'incognito_active');
  assert.equal(fixture.requests.length, 0);
  fixture.setIncognito(false);
  assert.equal((await search(event, { source: 'thread', query: 'valid' })).reason, 'provider_error');
  assert.equal(fixture.requests.at(-1)?.kind, 'close_document');
});

test('window-scoped cancellation and renderer crashes retire documents while new searches continue', async () => {
  let started = deferred<void>();
  let reply = deferred<PluginRemoteResult>();
  let complete = false;
  const fixture = remoteClient(async () => {
    if (complete) return { kind: 'value', value: page };
    started.resolve();
    return reply.promise;
  }, () => reply.reject(new Error('Remote call cancelled')));
  const { handlers, event, sender } = register(fixture.client);
  const search = handlers.get('search:thread')!;
  const cancel = handlers.get('search:thread:cancel')!;
  for (const reason of ['cancel', 'render-process-gone', 'destroyed']) {
    started = deferred<void>();
    reply = deferred<PluginRemoteResult>();
    const task = search(event, { source: 'thread', query: 'old' }, 'request');
    await started.promise;
    const previous = fixture.requests.filter((request) => request.kind === 'close_document').length;
    await cancel({ sender: new EventEmitter() } as Parameters<IpcHandler>[0], 'request');
    assert.equal(fixture.requests.filter((request) => request.kind === 'close_document').length, previous);
    if (reason === 'cancel') await cancel(event, 'request');
    else sender.emit(reason);
    assert.equal((await task).reason, 'aborted');
    assert.equal(fixture.requests.filter((request) => request.kind === 'close_document').length, previous + 1);
    assert.equal(sender.listenerCount('destroyed'), 0);
    assert.equal(sender.listenerCount('render-process-gone'), 0);
  }
  complete = true;
  assert.equal((await search(event, { source: 'thread', query: 'latest' }, 'new')).length, 2);
});

test('canceled search is not replayed on a replacement Host candidate', async (t) => {
  const handlers = new Map<string, IpcHandler>();
  const router = new RuntimeHostReconnectingIpcMain({
    handle: (channel, listener) => { handlers.set(channel, listener); },
    removeHandler: (channel) => { handlers.delete(channel); },
  });
  t.after(() => router.close());
  const started = deferred<void>();
  const reply = deferred<PluginRemoteResult>();
  const fixture = remoteClient(async () => { started.resolve(); return reply.promise; });
  const registerCandidate = () => {
    const target = router.createTarget('epoch');
    const scoped = (listener: IpcHandler): IpcHandler => (event, _scope, ...args) => listener(event, ...args);
    const ipcMain: ReconnectableReadIpcMain = {
      handle: (channel, listener) => target.handle(channel, scoped(listener)),
      handleReconnectableRead: (channel, listener) => target.handleReconnectableRead!(channel, scoped(listener)),
    };
    registerRuntimeHostSearchIpc({ ipcMain, client: fixture.client });
    target.completeRegistration();
    return target;
  };
  const first = registerCandidate();
  router.activate('epoch');
  const event = { sender: new EventEmitter() } as Parameters<IpcHandler>[0];
  const scope = { hostId: 'host-b', targetEpoch: 'epoch' };
  const task = handlers.get('search:thread')!(event, scope, { source: 'thread', query: 'old' }, 'old');
  await started.promise;
  await handlers.get('search:thread:cancel')!(event, scope, 'old');
  first.removeHandler('search:thread');
  first.removeHandler('search:thread:cancel');
  registerCandidate();
  reply.reject(new Error('retired candidate'));
  assert.equal((await task).reason, 'aborted');
  assert.equal(fixture.requests.filter((request) => request.kind === 'call').length, 1);
});

type SearchClient = Pick<DesktopRuntimeHostClient, 'request' | 'hostId' | 'queryRuntimePolicy'>;
function remoteClient(call: () => Promise<PluginRemoteResult>, close: () => void = () => {}) {
  const requests: PluginRemoteInput[] = [];
  let policy = createDefaultRuntimePolicy();
  const transport = async (operation: string, input: unknown): Promise<PluginRemoteResult> => {
    assert.equal(operation, 'plugin.remote');
    const request = HOST_OPERATION_SPECS['plugin.remote'].decodeInput(input);
    requests.push(request);
    switch (request.kind) {
      case 'open_document': return { kind: 'document', document };
      case 'bind': return { kind: 'bound', handler: 'method', target: { entryId: 'recall', activation: document, registration: document } };
      case 'call': return call();
      case 'close_document': close(); return { kind: 'closed' };
      default: throw new Error(`Unexpected request ${request.kind}`);
    }
  };
  const client: SearchClient = {
    hostId: 'host-b', request: transport as SearchClient['request'],
    queryRuntimePolicy: async () => ({ revision: 1, policy }),
  };
  return { client, requests, setIncognito: (incognitoActive: boolean) => {
    policy = { ...policy, privacy: { ...policy.privacy, incognitoActive } };
  } };
}
function register(client: SearchClient) {
  const handlers = new Map<string, IpcHandler>();
  const sender = new EventEmitter();
  registerRuntimeHostSearchIpc({
    ipcMain: {
      handle: (channel, listener) => { handlers.set(channel, listener); },
      handleReconnectableRead: (channel, listener) => { handlers.set(channel, listener); },
    }, client,
  });
  return { handlers, sender, event: { sender } as Parameters<IpcHandler>[0] };
}

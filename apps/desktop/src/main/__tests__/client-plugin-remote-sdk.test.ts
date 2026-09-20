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
import { RemoteError } from '@maka-agent/plugin-sdk/client';
import type { MakaBridge } from '../../preload/bridge-contract.js';
import { clientPluginRemote } from '../../renderer/platform/desktop/client-plugin-remote.js';

type Request = Parameters<MakaBridge['clientPlugins']['remote']>[2];
const identity = { entryId: 'ui', extensionId: 'demo', activation: 'activation',
  contentDigest: 'package', clientDigest: 'bundle' };
const host = { profileId: 'origin', hostId: 'host' };
const target = { entryId: 'backend', activation: 'activation', registration: 'registration' };
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { resolve, promise };
}

test('Remote SDK preserves exact bindings, distinguishes pending/null/end and closes broken-out streams', async () => {
  const requests: Request[] = [];
  let reads = 0;
  let stale = false;
  let retired = false;
  const lifetime = new AbortController();
  const remote = clientPluginRemote(async (origin, epoch, input) => {
    assert.deepEqual(origin, host);
    assert.equal(epoch, 'connection-one');
    requests.push(input);
    if (retired) return { kind: 'connection_retired' };
    switch (input.kind) {
      case 'open_document': return { kind: 'document', document: 'document' };
      case 'bind': return { kind: 'bound', target, handler: input.binding.method === 'echo' ? 'method' : 'stream' };
      case 'call':
        assert.deepEqual(input.target, target);
        if (stale) return { kind: 'remote_error', code: 'operation_conflict', message: 'registration retired' };
        return { kind: 'value', value: input.input };
      case 'open': return { kind: 'opened', stream: 'stream' };
      case 'next':
        reads++;
        return reads === 1 ? { kind: 'pending' } : reads === 3 ? { kind: 'end' } : { kind: 'item', item: null };
      case 'close': case 'close_document': return { kind: 'closed' };
    }
  }, host, 'connection-one')(identity, lifetime.signal);
  const call = remote.api.method<string, string>('echo', 'projected-session');
  assert.equal(await call('one'), 'one');
  stale = true;
  await assert.rejects(call('two'), (error) => error instanceof RemoteError && error.code === 'operation_conflict' && error.message === 'registration retired');
  assert.equal(requests.filter((r) => r.kind === 'bind').length, 1);
  const stream = remote.api.stream<null, null>('events', 'projected-session');
  const received = [];
  for await (const item of stream(null)) received.push(item);
  assert.deepEqual(received, [null]);
  assert.equal(requests.filter((r) => r.kind === 'close').length, 0);
  for await (const item of stream(null)) { assert.equal(item, null); break; }
  assert.equal(requests.filter((r) => r.kind === 'close').length, 1);
  assert.equal(requests.filter((r) => r.kind === 'bind').length, 2);
  retired = true;
  lifetime.abort();
  await remote.close();
  await assert.rejects(call('never dispatched'));
  assert.equal(requests.filter((r) => r.kind === 'call').length, 2);
});

test('Remote retirement waits for a late document and closes it before any method dispatch', async () => {
  const opened = deferred<string>();
  const entered = deferred<void>();
  const requests: Request[] = [];
  const lifetime = new AbortController();
  const remote = clientPluginRemote(async (_host, _epoch, input) => {
    requests.push(input);
    if (input.kind === 'bind') return { kind: 'bound', target, handler: 'method' };
    if (input.kind === 'open_document') {
      entered.resolve();
      return { kind: 'document', document: await opened.promise };
    }
    if (input.kind === 'close_document') return { kind: 'closed' };
    assert.fail('retired request must never dispatch');
  }, host, 'connection-one')(identity, lifetime.signal);
  const call = remote.api.method<null, null>('echo')(null);
  const rejected = assert.rejects(call);
  await entered.promise;
  lifetime.abort();
  let closed = false;
  const closing = remote.close().then(() => { closed = true; });
  await Promise.resolve();
  assert.equal(closed, false);
  opened.resolve('late-document');
  await Promise.all([rejected, closing]);
  assert.deepEqual(requests.map((r) => r.kind), ['bind', 'open_document', 'close_document']);
});

test('component cancellation closes idle reads and late stream opens without closing other instance resources', { timeout: 2_000 }, async () => {
  for (const phase of ['opening', 'reading']) {
    const entered = deferred<void>();
    const opened = deferred<void>();
    const read = deferred<void>();
    const requests: Request[] = [];
    const owner = new AbortController();
    const component = new AbortController();
    const remote = clientPluginRemote(async (_host, _epoch, input) => {
      requests.push(input);
      switch (input.kind) {
        case 'bind': return { kind: 'bound', target, handler: 'stream' };
        case 'open_document': return { kind: 'document', document: 'document' };
        case 'open':
          if (phase === 'opening') { entered.resolve(); await opened.promise; }
          return { kind: 'opened', stream: 'stream' };
        case 'next': entered.resolve(); await read.promise; return { kind: 'end' };
        case 'close': read.resolve(); return { kind: 'closed' };
        case 'close_document': return { kind: 'closed' };
        default: assert.fail('unexpected request');
      }
    }, host, 'connection-one')(identity, owner.signal);
    const iterator = remote.api.stream<null, null>('events')(null, component.signal)[Symbol.asyncIterator]();
    const rejected = assert.rejects(iterator.next(), /component unmounted/);
    await entered.promise;
    component.abort(new Error('component unmounted'));
    opened.resolve();
    await rejected;
    assert.equal(requests.filter((r) => r.kind === 'close').length, 1);
    assert.equal(requests.filter((r) => r.kind === 'close_document').length, 0);
    await remote.close();
  }
});

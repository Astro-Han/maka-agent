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
import { setImmediate } from 'node:timers/promises';
import test from 'node:test';
import { ClientSlotStore, type ClientSnapshot } from '@maka/ui/client-plugins';
import { ClientHostRuntime } from '../../renderer/features/client-plugins/testing.js';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
function fixture() {
  const reports: unknown[] = [];
  const transports: Array<{
    catalog: ReturnType<typeof deferred<ClientSnapshot>>;
    projection: ReturnType<typeof deferred<string>>;
    changed?: () => void;
    context?: () => void;
    signal?: AbortSignal;
    subscriptions: number;
  }> = [];
  const runtimes: Array<{
    closing: ReturnType<typeof deferred<void>>;
    closed: boolean;
    snapshots: string[];
  }> = [];
  const owner = new ClientHostRuntime(() => {
    const transport: (typeof transports)[number] = {
      catalog: deferred(), projection: deferred(), subscriptions: 0,
    };
    transports.push(transport);
    return {
      async snapshot(signal) { transport.signal = signal; return transport.catalog.promise; },
      async source() { throw new Error('Unexpected bundle read'); },
      remote() { throw new Error('Unexpected Remote call'); },
      async session() { return transport.projection.promise; },
      subscribe(listener) {
        transport.changed = listener;
        transport.subscriptions++;
        return () => { transport.subscriptions--; };
      },
      subscribeContext(listener) {
        transport.context = listener;
        transport.subscriptions++;
        return () => { transport.subscriptions--; };
      },
    };
  }, {
    document: {} as Document, modules: {}, report: (diagnostic) => reports.push(diagnostic.error),
  }, () => {
    const runtime: (typeof runtimes)[number] = { closing: deferred(), closed: false, snapshots: [] };
    runtimes.push(runtime);
    return {
      slots: new ClientSlotStore(),
      invalidate() {},
      async reconcile(snapshot) {
        assert.equal(runtime.closed, false, 'retired runtime must never reconcile');
        runtime.snapshots.push(snapshot.revision);
      },
      async close() { runtime.closed = true; await runtime.closing.promise; },
    };
  });
  return { owner, transports, runtimes, reports };
}

test('slots share one Host owner; remount waits for cleanup and rejects late connection results', async () => {
  const { owner, transports, runtimes, reports } = fixture();
  const releaseA = owner.subscribe(() => {});
  const releaseB = owner.subscribe(() => {});
  await setImmediate();
  assert.equal(runtimes.length, 1);
  assert.equal(transports[0].subscriptions, 2);
  transports[0].catalog.resolve({ revision: 'first', entries: [] });
  await setImmediate();
  assert.deepEqual(runtimes[0].snapshots, ['first']);
  transports[0].context!();
  assert.equal(owner.snapshot().contextRevision, 1);
  const projected = assert.rejects(owner.snapshot().session!('session'), /abort/i);
  releaseA();
  assert.equal(runtimes[0].closed, false, 'another slot still owns this activation');
  releaseB();
  assert.equal(runtimes[0].closed, true);
  assert.equal(transports[0].subscriptions, 0);
  assert.equal(transports[0].signal?.aborted, true);

  const releaseC = owner.subscribe(() => {});
  await setImmediate();
  assert.equal(runtimes.length, 1, 'cleanup must precede successor activation');
  runtimes[0].closing.resolve();
  await setImmediate();
  assert.equal(runtimes.length, 2);
  transports[0].projection.resolve('old-host-session');
  await projected;
  releaseC();
  // An ignored AbortSignal cannot make a late snapshot publish after retirement.
  transports[1].catalog.resolve({ revision: 'late', entries: [] });
  runtimes[1].closing.resolve();
  await setImmediate();
  assert.deepEqual(runtimes[1].snapshots, []);
  assert.equal(owner.snapshot().runtime, undefined);
  assert.deepEqual(reports, []);
});

test('failed cleanup fences only its Host owner, including later slot remounts', async () => {
  const failed = fixture();
  const other = fixture();
  const release = failed.owner.subscribe(() => {});
  const releaseOther = other.owner.subscribe(() => {});
  await setImmediate();
  release();
  const remount = failed.owner.subscribe(() => {});
  failed.runtimes[0].closing.reject(new Error('resource still owned'));
  other.transports[0].catalog.resolve({ revision: 'healthy', entries: [] });
  await setImmediate();
  assert.equal(failed.owner.snapshot().failure, true);
  assert.equal(failed.owner.snapshot().runtime, undefined);
  assert.equal(failed.runtimes.length, 1);
  assert.deepEqual(other.runtimes[0].snapshots, ['healthy']);
  remount();
  const again = failed.owner.subscribe(() => {});
  await setImmediate();
  assert.equal(failed.runtimes.length, 1, 'fresh slots cannot bypass the cleanup fence');
  assert.equal(failed.reports.length, 1);
  again();
  releaseOther();
  other.runtimes[0].closing.resolve();
});


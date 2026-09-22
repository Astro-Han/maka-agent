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
import { readFile } from 'node:fs/promises';
import { runInNewContext } from 'node:vm';
import test from 'node:test';
import { transform } from 'esbuild';

const source = await readFile(new URL('../../../src/preload/bootstrap-invoke.ts', import.meta.url), 'utf8');
const { code } = await transform(source, { loader: 'ts', format: 'cjs' });

function harness() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const ready = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  const calls: string[] = [];
  const timers = new Set<() => void>();
  const module = { exports: {} as { invokeWhenReady(channel: string): Promise<unknown>; sendWhenReady(channel: string): void } };
  runInNewContext(code, {
    module,
    require: () => ({ ipcRenderer: {
      invoke: (channel: string) => channel === 'app:bootstrapReady' ? ready : Promise.resolve(calls.push(channel)),
      send: (channel: string) => { calls.push(channel); },
    } }),
    setTimeout: (callback: () => void) => { timers.add(callback); return callback; },
    clearTimeout: (callback: () => void) => timers.delete(callback),
    console,
  });
  return { ...module.exports, resolve, reject, calls, timers };
}

test('queues calls until registration, but never delays renderer readiness or quit', async () => {
  const h = harness();
  const pending = h.invokeWhenReady('mutation');
  h.sendWhenReady('document-ready');
  await h.invokeWhenReady('window:notifyRendererReady');
  await h.invokeWhenReady('window:quit');
  assert.deepEqual(h.calls, ['window:notifyRendererReady', 'window:quit']);
  h.resolve();
  await pending;
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.deepEqual(h.calls, ['window:notifyRendererReady', 'window:quit', 'mutation', 'document-ready']);
  assert.equal(h.timers.size, 0);
});

test('timeout never dispatches abandoned work; a later explicit retry can succeed', async () => {
  const h = harness();
  const pending = h.invokeWhenReady('mutation');
  const rejected = assert.rejects(pending, /startup timed out/);
  for (const timeout of h.timers) timeout();
  await rejected;
  h.resolve();
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.deepEqual(h.calls, []);
  await h.invokeWhenReady('mutation');
  assert.deepEqual(h.calls, ['mutation']);
  assert.equal(h.timers.size, 0);

  const failed = harness();
  const request = failed.invokeWhenReady('mutation');
  failed.reject(new Error('registration failed'));
  await assert.rejects(request, /registration failed/);
  assert.deepEqual(failed.calls, []);
  assert.equal(failed.timers.size, 0);
});

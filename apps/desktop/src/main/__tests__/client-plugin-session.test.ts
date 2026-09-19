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
import { createHash } from 'node:crypto';
import { runInNewContext } from 'node:vm';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import test from 'node:test';
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { parseHTML } from 'linkedom';
import { buildClient } from '@maka-agent/plugin-sdk/build';
import type { ClientRemote } from '@maka-agent/plugin-sdk/client';
import { deferred } from '@maka/core/test-only/async-primitives';
import { desktopSessionKey } from '../../shared/runtime-host-identity.js';
import { ClientPluginServicesProvider, usePluginSession, type ClientPluginServices } from '../../renderer/features/client-plugins/index.js';

test('the WorkHub Client resolves main panels through its origin and withdraws stale or unloaded bindings', { timeout: 10_000 }, async () => {
  const source = await buildClient({
    packageId: 'maka.workhub',
    entryPoint: fileURLToPath(new URL('../../../../../crates/runtime-host/src/plugins/workhub/client.tsx', import.meta.url)),
  });
  const entry = {
    entryId: 'maka.workhub.ui', extensionId: 'maka.workhub', activation: 'same-activation',
    contentDigest: 'same-package', clientDigest: 'sha256-' + createHash('sha256').update(source).digest('hex'),
    sdkVersion: 1, totalBytes: Buffer.byteLength(source), dependencies: [], config: {},
  };
  const { document, window } = parseHTML('<html><head></head><body><main></main></body></html>');
  const globals = globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean };
  const previous = { document: globals.document, window: globals.window, act: globals.IS_REACT_ACT_ENVIRONMENT };
  globals.document = document;
  globals.window = window as unknown as Window & typeof globalThis;
  globals.IS_REACT_ACT_ENVIRONMENT = true;
  const append = document.head.append.bind(document.head);
  document.head.append = (...nodes: (string | Node)[]) => {
    append(...nodes);
    for (const node of nodes) {
      if (typeof node === 'string' || (node as Element).tagName !== 'SCRIPT') continue;
      const script = node as HTMLScriptElement;
      void fetch(script.src).then((response) => response.text()).then((bytes) => {
        if (!script.isConnected) return;
        Object.defineProperty(document, 'currentScript', { configurable: true, value: script });
        try { runInNewContext(bytes, { window, AbortController }); script.onload?.(new Event('load')); }
        catch { script.onerror?.(new Event('error')); }
        finally { Object.defineProperty(document, 'currentScript', { configurable: true, value: null }); }
      });
    }
  };
  const first = { profileId: 'one', hostId: 'first' };
  const second = { profileId: 'two', hostId: 'second' };
  let selected = first;
  let defaultCalls = 0;
  let changedHost!: () => void;
  let changedCatalog!: () => void;
  let changedContext!: () => void;
  let refreshing: ReturnType<typeof deferred<void>> | undefined;
  let enabled = true;
  let revision = 0;
  let subscriptions = 0;
  const projections: string[] = [];
  const calls: string[] = [];
  const closed: string[] = [];
  const lateProjection = deferred<string>();
  const services: ClientPluginServices = {
    async defaultHost() { defaultCalls++; return selected; },
    subscribeDefaultHost(listener) { changedHost = listener; return () => {}; },
    connect(host) {
      return {
        async snapshot() { return { revision: String(revision), connection: host.hostId, entries: enabled ? [entry] : [] }; },
        async source() { return source; },
        async session(id) {
          assert.equal(id, 'coordinator');
          projections.push(host.hostId);
          return host.hostId === first.hostId ? lateProjection.promise : desktopSessionKey({ hostId: host.hostId, sessionId: id });
        },
        remote(_identity, signal) {
          const method = (() => async () => {
            signal.throwIfAborted();
            calls.push(host.hostId);
            await refreshing?.promise;
            return { ok: true, result: { sessionId: 'coordinator' } };
          }) as ClientRemote['method'];
          return {
            api: { method, stream() { throw new Error('Unexpected stream'); } },
            async close() { closed.push(host.hostId); },
          };
        },
        subscribe(listener) { changedCatalog = listener; subscriptions++; return () => { subscriptions--; }; },
        subscribeContext(listener) { changedContext = listener; subscriptions++; return () => { subscriptions--; }; },
      };
    },
  };
  const root = createRoot(document.querySelector('main')!);
  let latest: ReturnType<typeof usePluginSession>;
  function Probe({ active }: { active: boolean }) {
    latest = usePluginSession('maka.workhub.ui', active, 'en');
    return latest.resolver;
  }
  const render = (active: boolean) => root.render(createElement(ClientPluginServicesProvider, {
    services, children: createElement(Probe, { active }),
  }));
  const until = async (check: () => boolean) => {
    const deadline = Date.now() + 3_000;
    while (!check()) {
      assert(Date.now() < deadline, 'Client resolver did not converge');
      await act(async () => { await delay(1); });
    }
  };
  try {
    await act(async () => render(false));
    assert.equal(defaultCalls, 0);
    await act(async () => render(true));
    await until(() => projections.includes('first'));
    assert.equal(latest!.sessionId, undefined);
    selected = second;
    await act(async () => changedHost());
    const expected = desktopSessionKey({ hostId: 'second', sessionId: 'coordinator' });
    await until(() => latest!.sessionId === expected);
    await act(async () => lateProjection.resolve(desktopSessionKey({ hostId: 'first', sessionId: 'coordinator' })));
    assert.equal(latest!.sessionId, expected, 'a retired observation cannot publish through another Host');
    assert(closed.includes('first'));

    refreshing = deferred<void>();
    await act(async () => changedContext());
    assert.equal(latest!.sessionId, expected, 'refreshing context must not unbind an existing panel');
    await act(async () => refreshing!.resolve());
    refreshing = undefined;

    enabled = false; revision++;
    await act(async () => changedCatalog());
    await until(() => latest!.sessionId === undefined);
    assert(closed.includes('second'));
    enabled = true; revision++;
    await act(async () => changedCatalog());
    await until(() => latest!.sessionId === expected);
    assert.deepEqual(calls, ['first', 'second', 'second', 'second']);
    await act(async () => render(false));
    await until(() => subscriptions === 0);
    assert.equal(latest!.sessionId, undefined);
  } finally {
    await act(async () => root.unmount());
    globals.document = previous.document;
    globals.window = previous.window;
    globals.IS_REACT_ACT_ENVIRONMENT = previous.act;
  }
});

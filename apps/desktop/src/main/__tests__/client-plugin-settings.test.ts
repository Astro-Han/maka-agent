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
import { fileURLToPath } from 'node:url';
import { runInNewContext } from 'node:vm';
import test from 'node:test';
import { act, createElement as h, Fragment, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { SideNav } from '@astryxdesign/core';
import { parseHTML } from 'linkedom';
import { buildClient } from '@maka-agent/plugin-sdk/build';
import { waitFor } from '@maka/core/test-only/async-primitives';
import {
  ClientPluginServicesProvider, ClientPluginSettings,
  type ClientPluginServices, type ClientSettingsSelection, type ClientHostRef,
} from '../../renderer/features/client-plugins/index.js';

test('external settings pages share publication and reject stale Host, epoch and activation selections', { timeout: 10_000 }, async () => {
  const source = await buildClient({ packageId: 'example',
    entryPoint: fileURLToPath(new URL('../../../../../packages/ui/tests/fixtures/client-plugin.tsx', import.meta.url)) });
  const { document, window } = parseHTML('<!doctype html><html><head></head><body><main></main></body></html>');
  const globals = globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean };
  const previous = { document: globals.document, window: globals.window, act: globals.IS_REACT_ACT_ENVIRONMENT };
  globals.document = document;
  globals.window = window as unknown as Window & typeof globalThis;
  window.matchMedia = () => ({ matches: false, media: '', onchange: null,
    addListener() {}, removeListener() {}, addEventListener() {}, removeEventListener() {}, dispatchEvent() { return true; } });
  globals.IS_REACT_ACT_ENVIRONMENT = true;
  const append = document.head.append.bind(document.head);
  document.head.append = (...nodes: (Node | string)[]) => {
    append(...nodes);
    for (const node of nodes) {
      if (typeof node === 'string' || (node as Element).tagName !== 'SCRIPT') continue;
      const script = node as HTMLScriptElement;
      void fetch(script.src).then((response) => response.text()).then((bytes) => {
        if (!script.isConnected) return;
        Object.defineProperty(document, 'currentScript', { configurable: true, value: script });
        try { runInNewContext(bytes, { window }); script.onload?.(new Event('load')); }
        catch { script.onerror?.(new Event('error')); }
        finally { Object.defineProperty(document, 'currentScript', { configurable: true, value: null }); }
      });
    }
  };
  let revision = 0;
  let enabled = true;
  const listeners = new Map<string, () => void>();
  const services: ClientPluginServices = {
    async defaultHost() { throw new Error('Settings must use its selected Host'); },
    subscribeDefaultHost() { throw new Error('Settings must not subscribe to the default Host'); },
    connect(host) { return {
      remote() { return { api: {
        method() { throw new Error('No private settings service'); },
        stream() { throw new Error('No private settings service'); },
      }, async close() {} }; },
      async session(id) { return id; },
      async source() { return source; },
      async snapshot() { return { revision: String(revision), connection: host.hostId, entries: enabled ? [{
        entryId: 'example.ui', extensionId: 'example', activation: String(revision),
        contentDigest: 'package', clientDigest: 'sha256-' + createHash('sha256').update(source).digest('hex'),
        totalBytes: Buffer.byteLength(source), dependencies: [], sdkVersion: 1, config: { label: host.hostId },
      }] : [] }; },
      subscribe(listener) { listeners.set(host.hostId, listener); return () => { listeners.delete(host.hostId); }; },
      subscribeContext() { return () => {}; },
    }; },
  };
  function Settings({ host, epoch, verified }: { host: ClientHostRef; epoch: string; verified: boolean }) {
    const [selection, select] = useState<ClientSettingsSelection>();
    return h(ClientPluginSettings, { host, epoch, verified, locale: 'en', selection, onSelect: select,
      children: (view) => h('section', {},
        h(SideNav, { children: h(Fragment, {}, h('button', { onClick: () => select(undefined) }, 'Native settings'), view.navigation) }),
        h('input', { 'data-native-draft': true, defaultValue: '' }),
        h('h1', {}, view.title), selection ? view.page : null),
    });
  }
  const root = createRoot(document.querySelector('main')!);
  let host = { profileId: 'one', hostId: 'first' };
  let epoch = 'epoch-1';
  let verified = true;
  const render = () => root.render(h(ClientPluginServicesProvider, { services }, h(Settings, { host, epoch, verified })));
  const until = (predicate: () => boolean) => waitFor(async () => {
    await act(async () => { await new Promise((resolve) => setImmediate(resolve)); });
    return predicate();
  }, { timeoutMs: 2000 });
  const button = (text: string) => [...document.querySelectorAll('button')].find((item) => item.textContent === text)!;
  const click = (text: string) => act(() => { button(text).click(); });
  try {
    await act(async () => { render(); });
    await until(() => !!button('Preferences'));
    await click('Preferences');
    assert.equal(document.querySelector('[data-plugin-page]')?.textContent, 'first');
    await click('Diagnostics');
    assert.equal(document.querySelectorAll('[data-plugin-page]').length, 1);
    assert.equal(document.querySelector('[data-plugin-page]')?.getAttribute('data-plugin-page'), 'diagnostics');
    const draft = document.querySelector<HTMLInputElement>('[data-native-draft]')!;
    draft.value = 'unsaved native settings';
    verified = false;
    await act(async () => { render(); });
    assert.equal(document.querySelector('[data-plugin-page]'), null);
    assert.equal(button('Preferences'), undefined);
    assert.equal(document.querySelector('[data-native-draft]'), draft);
    verified = true;
    await act(async () => { render(); });
    await until(() => !!button('Preferences'));
    assert.equal(document.querySelector('[data-native-draft]'), draft);
    assert.equal(draft.value, 'unsaved native settings');
    assert.equal(document.querySelector('[data-plugin-page]'), null, 'retired publication cannot resume an old selection');
    host = { profileId: 'two', hostId: 'second' };
    await act(async () => { render(); });
    await until(() => !!button('Preferences'));
    assert.equal(document.querySelector('[data-plugin-page]'), null, 'same Entry on another Host cannot take over');
    await click('Preferences');
    assert.equal(document.querySelector('[data-plugin-page]')?.textContent, 'second');
    epoch = 'epoch-2';
    await act(async () => { render(); });
    assert.equal(document.querySelector('[data-plugin-page]'), null);
    await click('Preferences');
    revision++;
    await act(async () => { listeners.get('second')!(); });
    await until(() => document.querySelector('[data-plugin-page]') === null && !!button('Preferences'));
    await click('Preferences');
    assert.equal(document.querySelector('[data-plugin-page]')?.textContent, 'second');
    enabled = false; revision++;
    await act(async () => { listeners.get('second')!(); });
    await until(() => !button('Preferences'));
    assert.equal(document.querySelector('[data-plugin-page]'), null);
    await click('Native settings');
    assert.equal(document.querySelector('[role="alert"]'), null);
  } finally {
    await act(() => root.unmount());
    globals.document = previous.document;
    globals.window = previous.window;
    globals.IS_REACT_ACT_ENVIRONMENT = previous.act;
  }
});

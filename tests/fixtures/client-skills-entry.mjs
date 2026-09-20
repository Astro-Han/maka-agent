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
import { connect } from 'node:net';
import { once } from 'node:events';
import { parseArgs } from 'node:util';
import { join, basename } from 'node:path';
import { readFile } from 'node:fs/promises';
import { runInThisContext } from 'node:vm';
import { parseHTML } from 'linkedom';
import * as React from 'react';
import * as JsxRuntime from 'react/jsx-runtime';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import * as ClientSdk from '../../packages/plugin-sdk/src/client.ts';
import { ClientRuntime } from '../../packages/ui/src/client-plugins/runtime.ts';
import { ClientSlot } from '../../packages/ui/src/client-plugins/slots.tsx';
import { clientPluginRemote } from '../../apps/desktop/src/renderer/platform/desktop/client-plugin-remote.ts';
import { pluginRemote } from './client-plugin-remote.mjs';
import { connectRuntimeHostMessageTransport } from '../../packages/runtime-host/src/client/connection.ts';
import { FramedTransport } from '../../packages/runtime-host/src/transport/framed-transport.ts';
import {
  INTERACTIVE_RUNTIME_HOST_COMPOSITION_ID,
  RUNTIME_HOST_PROTOCOL_VERSION,
} from '../../packages/runtime-host/src/protocol/index.ts';

const { values } = parseArgs({
  options: {
    socket: { type: 'string' },
    'root-id': { type: 'string' },
    'skills-client-workspace': { type: 'string' },
  },
});
const { window, document } = parseHTML('<html><head></head><body><main></main></body></html>');
Object.assign(globalThis, { window, document });
Object.defineProperty(globalThis, 'navigator', { value: window.navigator, configurable: true });
// LinkeDOM does not execute scripts; emulate only that browser primitive.
// Bundle byte/digest checks, factory loading and lifecycle use the real ClientRuntime.
const append = document.head.append.bind(document.head);
document.head.append = (...nodes) => {
  append(...nodes);
  for (const script of nodes.filter((node) => node.tagName === 'SCRIPT')) {
    void fetch(script.src)
      .then((response) => response.text())
      .then((source) => {
        Object.defineProperty(document, 'currentScript', { value: script, configurable: true });
        try {
          runInThisContext(source);
          script.onload?.();
        } finally {
          Object.defineProperty(document, 'currentScript', { value: null, configurable: true });
        }
      })
      .catch(() => script.onerror?.());
  }
};
const socket = connect(values.socket);
const transport = new FramedTransport(socket);
let connection, runtime, root, skills;
const errors = [];
const draft = [];
const openedFiles = [];
let suggestions = [];
const publishSuggestions = (items) => {
  suggestions = items;
  let current = items;
  let disposed = false;
  return {
    update(next) {
      if (disposed) return;
      current = next;
      suggestions = next;
    },
    dispose() {
      disposed = true;
      if (suggestions === current) suggestions = [];
    },
  };
};
async function until(check) {
  const deadline = Date.now() + 5000;
  while (!check()) {
    if (errors.length) throw errors[0];
    assert.ok(Date.now() < deadline, document.body.textContent);
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}
function click(text) {
  const button = [...document.querySelectorAll('button')].find(
    (button) => button.textContent === text && !button.disabled,
  );
  assert.ok(button, 'Missing enabled button: ' + text + '; ' + document.body.textContent);
  flushSync(() => button.dispatchEvent(new window.Event('click', { bubbles: true })));
}
try {
  await once(socket, 'connect');
  const connected = await connectRuntimeHostMessageTransport({
    transport,
    expectedRootId: values['root-id'],
    compositionId: INTERACTIVE_RUNTIME_HOST_COMPOSITION_ID,
    protocol: { min: RUNTIME_HOST_PROTOCOL_VERSION, max: RUNTIME_HOST_PROTOCOL_VERSION },
    handshakeTimeoutMs: 3000,
    livenessIntervalMs: 60000,
  });
  assert.equal(connected.kind, 'connected');
  connection = connected.connection;
  const snapshot = async () => {
    const page = await connection.request('plugin.client.query', { kind: 'snapshot' });
    assert.equal(page.kind, 'snapshot');
    return {
      revision: page.revision,
      entries: page.entries.filter((entry) => entry.extensionId === 'maka.skills'),
    };
  };
  let initial;
  const deadline = Date.now() + 5000;
  do {
    initial = await snapshot();
    if (initial.entries.length) break;
    assert.ok(Date.now() < deadline, 'Skills Client was not published');
    await new Promise((resolve) => setTimeout(resolve, 10));
  } while (true);
  const origin = { profileId: 'test', hostId: values['root-id'] };
  runtime = new ClientRuntime({
    document,
    modules: {
      react: React,
      'react/jsx-runtime': JsxRuntime,
      '@maka-agent/plugin-sdk/client': ClientSdk,
    },
    report: ({ error }) => errors.push(error),
    localFiles: () => ({
      pick: async () => join(values['skills-client-workspace'], 'import-client.md'),
      open: async (path) => {
        openedFiles.push(path);
      },
    }),
    remote: clientPluginRemote(
      async (host, epoch, input) => {
        assert.deepEqual(host, origin);
        assert.equal(epoch, 'skills-client');
        return connection.request('plugin.remote', input);
      },
      origin,
      'skills-client',
    ),
    source: async (descriptor) => {
      let offset = 0,
        source = '';
      do {
        const page = await connection.request('plugin.client.query', {
          kind: 'bundle',
          entryId: descriptor.entryId,
          activation: descriptor.activation,
          clientDigest: descriptor.clientDigest,
          offset,
        });
        assert.equal(page.kind, 'bundle');
        source += page.content;
        if (page.nextOffset === null) return source;
        assert.ok(page.nextOffset > offset);
        offset = page.nextOffset;
      } while (true);
    },
  });
  await runtime.reconcile(initial);
  root = createRoot(document.querySelector('main'));
  flushSync(() =>
    root.render(
      React.createElement(ClientSlot, {
        store: runtime.slots,
        name: 'session.composer.before',
        input: {
          sessionId: 'skills-session',
          locale: 'en',
          onOpenSession: () => assert.fail('Unexpected navigation'),
          appendText: (text) => draft.push(text),
          publishSuggestions,
        },
        onError: (_identity, error) => errors.push(error),
      }),
    ),
  );
  await until(() => suggestions.some((item) => item.insertText === '/skill:review '));
  click('Use a Skill');
  await until(() =>
    [...document.querySelectorAll('button')].some((button) => button.textContent === 'Review'),
  );
  click('Review');
  assert.deepEqual(draft, ['/skill:review ']);
  click('Manage');
  await until(() =>
    [...document.querySelectorAll('button')].some((button) => button.textContent === 'Unpin'),
  );
  click('Unpin');
  await until(() =>
    [...document.querySelectorAll('button')].some(
      (button) => button.textContent === 'Pin' && !button.disabled,
    ),
  );
  skills = await pluginRemote(connection, 'maka.skills');
  const skillRequest = (request) =>
    skills.method('path-request')({
      path: values['skills-client-workspace'],
      permissionMode: 'ask',
      collaborationMode: 'agent',
      request,
    });
  const governance = await skillRequest({ kind: 'catalog', view: 'governance' });
  assert.equal(governance.items.find((item) => item.ref === 'project:maka:review').pinned, false);
  const disable = await skillRequest({
    kind: 'mutate',
    expectedRevision: governance.revision,
    mutation: { kind: 'set_enabled', ref: 'project:maka:review', enabled: false },
  });
  assert.equal(disable.kind, 'committed');
  await until(() => !suggestions.some((item) => item.insertText === '/skill:review '));
  const latest = await skillRequest({
    kind: 'catalog',
    view: 'governance',
  });
  const enable = await skillRequest({
    kind: 'mutate',
    expectedRevision: latest.revision,
    mutation: { kind: 'set_enabled', ref: 'project:maka:review', enabled: true },
  });
  assert.equal(enable.kind, 'committed');
  await until(() => suggestions.some((item) => item.insertText === '/skill:review '));
  click('Open file');
  await until(() => openedFiles.length === 1);
  assert.equal(basename(openedFiles[0]), 'SKILL.md');
  assert.match(await readFile(openedFiles[0], 'utf8'), /Review with care/);
  await until(() =>
    [...document.querySelectorAll('button')].some(
      (button) => button.textContent === 'Import source' && !button.disabled,
    ),
  );
  click('Import source');
  await until(() => document.body.textContent.includes('Imported Client Source'));
  click('Install');
  await until(() =>
    [...document.querySelectorAll('li button')].some(
      (button) => button.textContent === 'Installed' && button.disabled,
    ),
  );
  const workspace = {
    workspace: { kind: 'host_path', path: values['skills-client-workspace'] },
    permissionMode: 'ask',
    collaborationMode: 'agent',
    locale: 'en',
  };
  const mount = (name, input) =>
    flushSync(() =>
      root.render(
        React.createElement(ClientSlot, {
          store: runtime.slots,
          name,
          input,
          onError: (_identity, error) => errors.push(error),
        }),
      ),
    );
  mount('workspace.composer.before', {
    ...workspace,
    publishSuggestions,
    appendText: (text) => draft.push(text),
  });
  click('Use a Skill');
  await until(() =>
    [...document.querySelectorAll('button')].some((button) => button.textContent === 'Review'),
  );
  click('Review');
  assert.deepEqual(draft, ['/skill:review ', '/skill:review ']);
  mount('workspace.manage', { ...workspace, section: 'skills' });
  await until(() => document.body.textContent.includes('Imported Client Source'));
  assert.ok(document.body.textContent.includes('Review'));
  mount('workspace.composer.before', {
    ...workspace,
    publishSuggestions,
    collaborationMode: 'plan',
  });
  click('Use a Skill');
  await until(() => document.body.textContent.includes('No Skills'));
  assert.deepEqual(suggestions, []);
  assert.ok(
    ![...document.querySelectorAll('button')].some((button) => button.textContent === 'Review'),
  );
  await connection.request('plugin.composition.apply', {
    operations: [{ type: 'update', entryId: 'maka.skills', patch: { disabled: true } }],
  });
  let retired;
  do {
    retired = await snapshot();
    if (!retired.entries.length) break;
    await new Promise((resolve) => setTimeout(resolve, 10));
  } while (Date.now() < deadline + 5000);
  assert.equal(retired.entries.length, 0);
  await runtime.reconcile(retired);
  await until(() => document.querySelector('[data-maka-skills-plugin]') === null);
  assert.deepEqual(errors, []);
  console.log('skills-client-bundle-accepted');
} finally {
  await skills?.close();
  if (root) flushSync(() => root.unmount());
  await runtime?.close();
  transport.abort();
  await connection?.close();
  socket.destroy();
}

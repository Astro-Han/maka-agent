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
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { build } from 'esbuild';
import { act, createElement as h } from 'react';
import { parseHTML } from 'linkedom';

test('Import keeps its identity through lost replies, failed refreshes and Host replacement', async () => {
  const { document, window } = parseHTML('<html><body><main></main></body></html>');
  const previous = {
    document: globalThis.document,
    window: globalThis.window,
    act: globalThis.IS_REACT_ACT_ENVIRONMENT,
  };
  globalThis.document = document;
  globalThis.window = window;
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  document.oninput = null;
  const { createRoot } = await import('react-dom/client');
  const output = await build({
    entryPoints: [fileURLToPath(new URL('../src/client.tsx', import.meta.url))],
    bundle: true,
    write: false,
    platform: 'node',
    format: 'cjs',
    packages: 'external',
    jsx: 'automatic',
  });
  const compiled = { exports: {} };
  new Function('require', 'module', 'exports', output.outputFiles[0].text)(
    createRequire(import.meta.url),
    compiled,
    compiled.exports,
  );
  const root = createRoot(document.querySelector('main'));
  const requests = [];
  let fail = false;
  let rejectPrepare = false;
  let saved;
  const controller = new AbortController();
  const context = {
    signal: controller.signal,
    remote: {
      method: () => async (request) => {
        requests.push(request);
        switch (request.kind) {
          case 'sources':
            return {
              kind: 'sources',
              snapshot: {
                revision: 1,
                configuration: {
                  sources: [
                    { id: 'source', name: 'Codex', location: { kind: 'codex', root: '/source' } },
                  ],
                },
              },
            };
          case 'models':
            return {
              kind: 'models',
              choices: {
                complete: true,
                models: [
                  {
                    model: { connection_id: 'test', model: 'test' },
                    isDefault: true,
                    connectionName: 'Test',
                    displayName: 'Test',
                    defaultThinkingLevel: 'default',
                  },
                ],
              },
            };
          case 'catalog':
            return {
              kind: 'catalog',
              page: {
                entries: [
                  {
                    id: 'foreign',
                    title: 'Original conversation',
                    path: 'rollout.jsonl',
                    cwd: '/original',
                    updatedAt: null,
                    archived: false,
                  },
                ],
                next: null,
              },
            };
          case 'prepare':
            if (rejectPrepare) throw new Error('Invalid destination');
            saved ??= {
              operationId: request.request.operationId,
              sourceName: 'Codex',
              title: 'Imported conversation',
              records: 2,
              receipt: null,
            };
            assert.equal(saved.operationId, request.request.operationId);
            return { kind: 'copy', copy: saved };
          case 'deliver':
            saved.receipt = { state: 'published', sessionId: 'imported' };
            if (fail) throw new Error('Reply lost');
            return { kind: 'copy', copy: saved };
          case 'copies':
            if (fail) throw new Error('History unavailable');
            return { kind: 'copies', page: { copies: saved ? [saved] : [], next: null } };
          default:
            throw new Error(request.kind);
        }
      },
    },
  };
  const button = (name) => {
    const element = [...document.querySelectorAll('button')].find(
      (item) => item.textContent === name,
    );
    assert.ok(element, name);
    return element;
  };
  const click = async (name) => {
    const element = button(name);
    assert.equal(element.disabled, false, name);
    await act(async () => element.click());
  };
  const select = async () => {
    await act(async () =>
      button('Read catalog')
        .closest('form')
        .dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })),
    );
    await act(async () => {
      const radio = document.querySelector('input[type=radio]');
      radio.checked = true;
      radio.click();
    });
  };
  try {
    await act(async () => root.render(h(compiled.exports.ImportPage, { context, locale: 'en' })));
    await act(async () =>
      button('Read catalog')
        .closest('form')
        .dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })),
    );
    await act(async () => {
      const radio = document.querySelector('input[type=radio]');
      radio.checked = true;
      radio.click();
      const input = [...document.querySelectorAll('label')]
        .find((item) => item.textContent === 'Destination Host workspace')
        .querySelector('input');
      input.type = 'text';
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set.call(
        input,
        '/destination',
      );
      input.dispatchEvent(new window.Event('input', { bubbles: true }));
    });
    fail = true;
    await click('Create imported copy');
    const original = requests.find((request) => request.kind === 'prepare').request;
    await click('View saved imports');
    assert.ok(document.body.textContent.includes('History unavailable'));
    await click('Refresh state');
    await click('Retry this import');
    assert.deepEqual(
      requests.filter((request) => request.kind === 'prepare').map((request) => request.request),
      [original, original],
    );
    fail = false;
    await click('Retry this import');
    assert.equal(
      new Set(
        requests
          .filter((request) => request.kind === 'deliver')
          .map((request) => request.operationId),
      ).size,
      1,
    );
    assert.ok(document.body.textContent.includes('Published'));
    assert.equal(button('Create imported copy').disabled, true);
    rejectPrepare = true;
    await select();
    await click('Create imported copy');
    await click('Set aside this attempt');
    assert.equal(
      button('Create imported copy').disabled,
      true,
      'setting aside must clear selection',
    );
    await select();
    await click('Create imported copy');
    assert.ok(button('Retry this import'));
    controller.abort();
    const other = { ...context, signal: new AbortController().signal };
    await act(async () =>
      root.render(h(compiled.exports.ImportPage, { context: other, locale: 'en' })),
    );
    const destination = [...document.querySelectorAll('label')]
      .find((item) => item.textContent === 'Destination Host workspace')
      .querySelector('input');
    assert.equal(destination.value, '', 'a different Host must not inherit the destination');
    assert.equal(button('Create imported copy').disabled, true);
  } finally {
    await act(async () => root.unmount());
    globalThis.document = previous.document;
    globalThis.window = previous.window;
    globalThis.IS_REACT_ACT_ENVIRONMENT = previous.act;
  }
});

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
import { createRoot } from 'react-dom/client';
import { parseHTML } from 'linkedom';

test('Inspector binds raw Sessions, coalesces facts and drops retired reads', {
  timeout: 10_000,
}, async () => {
  const output = await build({
    entryPoints: [fileURLToPath(new URL('../src/client/inspector.tsx', import.meta.url))],
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
  const { Inspector } = compiled.exports;
  const { document, window } = parseHTML('<html><body><main></main></body></html>');
  const previous = {
    document: globalThis.document,
    window: globalThis.window,
    act: globalThis.IS_REACT_ACT_ENVIRONMENT,
  };
  globalThis.document = document;
  globalThis.window = window;
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const root = createRoot(document.querySelector('main'));
  const reads = [];
  const listeners = new Map();
  let releaseFirst;
  const first = new Promise((resolve) => {
    releaseFirst = resolve;
  });
  const counter = { known: 0, missing: 0 };
  const totals = (calls) => ({
    models: {
      calls,
      success: calls,
      error: 0,
      aborted: 0,
      unknown: 0,
      durationMs: 12.5,
      input: counter,
      output: counter,
      cacheRead: counter,
      cacheWrite: counter,
      reasoning: counter,
      cost: { knownUsd: 0, unvalued: 0, unpriced: 0 },
    },
    tools: {
      calls: 0,
      success: 0,
      error: 0,
      unknown: 0,
      rejected: 0,
      durationMs: 0,
      meanLatencyMs: null,
    },
    pending: { models: 0, tools: 0 },
    byModel: [],
    byProvider: [],
    byTool: [],
  });
  const context = {
    signal: new AbortController().signal,
    remote: {
      method(name, session) {
        assert.equal(name, 'request');
        assert.ok(session === 'first' || session === 'second');
        return async (input) => {
          reads.push({ session, input });
          if (input.kind === 'activity')
            return {
              kind: 'activity',
              page: { cursor: session, nextCursor: null, total: 0, attempts: [] },
            };
          assert.equal(input.kind, 'summary');
          assert.equal(input.cursor, session, 'summary cannot acquire a second independent fence');
          if (session === 'first') return first;
          return { kind: 'summary', summary: totals(7) };
        };
      },
    },
    events: {
      subscribe(request, listener) {
        assert.equal(request.kind, 'session.event');
        listeners.set(request.sessionId, listener);
        listener({
          kind: 'session.event',
          sessionId: request.sessionId,
          event: { type: 'complete' },
        });
        return () => {
          listeners.delete(request.sessionId);
        };
      },
    },
  };
  async function until(predicate) {
    const deadline = Date.now() + 2500;
    while (!predicate()) {
      assert.ok(Date.now() < deadline, 'Inspector did not settle');
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 10));
      });
    }
  }
  const render = (sessionId) => root.render(h(Inspector, { context, sessionId, locale: 'en' }));
  try {
    await act(async () => {
      render('first');
    });
    await until(() =>
      reads.some((read) => read.session === 'first' && read.input.kind === 'summary'),
    );
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 450));
    });
    assert.equal(reads.length, 2, 'a replayed seed cannot overlap the initial slow read');
    await act(async () => {
      render('second');
    });
    await until(() => document.body.textContent.includes('Cumulative model time'));
    assert.equal(listeners.has('first'), false);
    assert.equal(document.querySelector('.insights-cards strong').textContent, '7');
    await act(async () => {
      releaseFirst({ kind: 'summary', summary: totals(999) });
    });
    assert.equal(
      document.querySelector('.insights-cards strong').textContent,
      '7',
      'old Session read cannot land',
    );
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 450));
    });
    const emit = (type) =>
      listeners.get('second')({ kind: 'session.event', sessionId: 'second', event: { type } });
    const before = reads.length;
    await act(async () => {
      for (let i = 0; i < 30; i++) emit('text_delta');
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 450));
    });
    assert.equal(reads.length, before, 'streaming text is not an accounting invalidation');
    await act(async () => {
      emit('tool_result');
      emit('token_usage');
      emit('complete');
    });
    await until(() => reads.length === before + 2);
    assert.equal(reads.at(-1).input.kind, 'summary');
    await act(async () => {
      emit('complete');
      root.unmount();
    });
    assert.equal(listeners.size, 0);
    await new Promise((resolve) => setTimeout(resolve, 450));
    assert.equal(reads.length, before + 2, 'retirement cancels queued reads');
  } finally {
    await act(async () => {
      root.unmount();
    });
    globalThis.document = previous.document;
    globalThis.window = previous.window;
    globalThis.IS_REACT_ACT_ENVIRONMENT = previous.act;
  }
});

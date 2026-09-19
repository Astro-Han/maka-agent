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
import { coordinationCommands } from '../dist/client-session.js';
import { bindSurface } from '../dist/client-surface.js';

test('Remote answers reconcile exact receipts without redispatching across Host epochs', async () => {
  const request = { turnId: 'turn', text: 'original' };
  const lifetime = new AbortController();
  const calls = [];
  const replies = new Map();
  const commands = coordinationCommands({
    hostEpoch: 'new',
    signal: lifetime.signal,
    remote: {
      method(name) {
        return async (input) => {
          calls.push([name, structuredClone(input)]);
          const reply = replies.get(name);
          if (reply instanceof Error) throw reply;
          return await reply;
        };
      },
    },
  });
  replies.set('answer', new Error('response lost after dispatch'));
  assert.deepEqual(await commands.answer('session', request), {
    kind: 'unknown',
    originHostEpoch: 'new',
  });
  replies.set('answer-receipt', { ok: true, result: null });
  replies.set('answer', { ok: true, result: { turnId: 'turn' } });
  calls.length = 0;
  assert.deepEqual(await commands.answer('session', { ...request, originHostEpoch: 'new' }), {
    kind: 'admitted',
    turnId: 'turn',
  });
  assert.deepEqual(calls, [
    ['answer-receipt', request],
    ['answer', request],
  ]);

  calls.length = 0;
  assert.deepEqual(await commands.answer('session', { ...request, originHostEpoch: 'old' }), {
    kind: 'not_admitted',
  });
  assert.deepEqual(calls, [['answer-receipt', request]]);
  replies.set('answer-receipt', { ok: true, result: { turnId: 'turn' } });
  assert.deepEqual(await commands.answer('session', { ...request, originHostEpoch: 'old' }), {
    kind: 'admitted',
    turnId: 'turn',
  });
  replies.set('answer-receipt', new Error('receipt unavailable'));
  assert.deepEqual(await commands.answer('session', { ...request, originHostEpoch: 'old' }), {
    kind: 'unknown',
    originHostEpoch: 'old',
  });
  replies.set('answer-receipt', {
    ok: false,
    error: { code: 'operation_conflict', message: 'changed payload' },
  });
  await assert.rejects(
    commands.answer('session', { ...request, originHostEpoch: 'old' }),
    /changed payload/,
  );

  // Retirement during reconciliation must not start a fresh submission afterward.
  const pending = Promise.withResolvers();
  replies.set('answer-receipt', pending.promise);
  calls.length = 0;
  const reconciling = commands.answer('session', { ...request, originHostEpoch: 'new' });
  lifetime.abort(new Error('retired'));
  pending.resolve({ ok: true, result: null });
  assert.deepEqual(await reconciling, { kind: 'unknown', originHostEpoch: 'new' });
  assert.deepEqual(calls, [['answer-receipt', request]]);
  await assert.rejects(commands.configureModel('session', {}), /retired/);
  await assert.rejects(commands.answer('session', request), /retired/);
  assert.equal(calls.length, 1);
});

test('Client retirement fences new surface calls while preserving cleanup and accepted reads', async () => {
  const lifetime = new AbortController();
  const pending = Promise.withResolvers();
  let calls = 0,
    cleaned = 0,
    browser;
  class Sessions {
    #owner = 'origin';
    getSession() {
      calls++;
      return this.#owner;
    }
    listSessions() {
      calls++;
      return pending.promise;
    }
    subscribeSessions() {
      calls++;
      return () => cleaned++;
    }
    answer() {
      throw new Error('legacy submit must not be used');
    }
  }
  const ports = {
    sessions: new Sessions(),
    native: {
      presentation: Object.freeze({
        hide() {
          calls++;
        },
      }),
      control: Object.freeze({
        stop() {
          calls++;
        },
      }),
      bindBrowserSession(id) {
        browser = id;
      },
    },
    attachments: {
      staging: {
        pickFiles() {
          calls++;
        },
      },
      read() {
        calls++;
      },
      prepare() {
        calls++;
      },
      copy: () => ({}),
      formatError: () => 'formatted',
    },
    contextUsage: {
      context() {
        calls++;
      },
    },
  };
  const bound = bindSurface(ports, { answer: () => 'remote' }, lifetime.signal);
  assert.equal(bound.sessions.getSession, bound.sessions.getSession);
  assert.equal(bound.sessions.getSession(), 'origin', 'prototype methods retain their receiver');
  assert.equal(bound.sessions.answer(), 'remote');
  const accepted = bound.sessions.listSessions();
  const unsubscribe = bound.sessions.subscribeSessions();
  bound.native.bindBrowserSession('session');
  lifetime.abort(new Error('retired'));
  const before = calls;
  for (const call of [
    bound.sessions.getSession,
    bound.sessions.answer,
    bound.native.presentation.hide,
    bound.native.control.stop,
    bound.attachments.staging.pickFiles,
    bound.attachments.prepare,
    bound.attachments.read,
    bound.contextUsage.context,
    () => bound.native.bindBrowserSession('another'),
  ])
    assert.throws(call, /retired/);
  assert.equal(calls, before);
  unsubscribe();
  assert.equal(cleaned, 1);
  bound.native.bindBrowserSession(null);
  assert.equal(browser, null);
  pending.resolve(['settled']);
  assert.deepEqual(await accepted, ['settled']);
  assert.equal(bound.attachments.formatError(), 'formatted');
});

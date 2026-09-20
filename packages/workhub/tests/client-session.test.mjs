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
import { coordinationCommands } from '../dist/client-session.js';
import { bindSurface } from '../dist/client-surface.js';

test('Remote answers recover stable operation receipts, cancellation and attachments across reconnects', async () => {
  const request = { operationId: 'request', text: 'original' };
  const accepted = {
    invocation: {
      session_id: 'canonical-session',
      turn_id: 'host-turn',
      run_id: 'run',
      invocation_id: 'inv',
    },
    messageId: 'host-message',
    contentDigest: 'digest',
  };
  const lifetime = new AbortController();
  const calls = [];
  const replies = new Map();
  const commands = coordinationCommands(
    {
      signal: lifetime.signal,
      remote: {
        method(name, sessionId) {
          assert.equal(sessionId, 'canonical-session');
          return async (input) => {
            calls.push([name, structuredClone(input)]);
            const reply = replies.get(name);
            if (reply instanceof Error) throw reply;
            return await reply;
          };
        },
      },
    },
    (_sessionId, refs) =>
      refs.map((attachment) => {
        if (attachment.ref.sessionId !== 'projected-session') throw new Error('foreign attachment');
        return { ...attachment, ref: { ...attachment.ref, sessionId: 'canonical-session' } };
      }),
    'canonical-session',
  );
  replies.set('answer-receipt', null);
  replies.set('answer', new Error('response lost after dispatch'));
  assert.deepEqual(await commands.answer('session', request), {
    kind: 'unknown',
  });
  for (const code of ['outcome_unknown', 'internal_failure']) {
    replies.set('answer', new RemoteError(code, 'commit not confirmed'));
    assert.deepEqual(await commands.answer('session', request), {
      kind: 'unknown',
    });
  }
  replies.set('answer-receipt', null);
  replies.set('answer', accepted);
  calls.length = 0;
  assert.deepEqual(await commands.answer('session', request), {
    kind: 'admitted',
    receipt: accepted,
  });
  assert.deepEqual(calls, [
    ['answer-receipt', request],
    ['answer', request],
  ]);

  calls.length = 0;
  const progress = { state: 'ended', outcome: { kind: 'completed' } };
  replies.set('answer-receipt', { receipt: accepted, progress });
  assert.deepEqual(await commands.answer('session', request), {
    kind: 'admitted',
    receipt: accepted,
    progress,
  });
  assert.deepEqual(calls, [['answer-receipt', request]]);
  replies.set('answer-cancel', { receipt: accepted, progress });
  await commands.cancelAnswer('session', request);
  assert.deepEqual(calls.at(-1), ['answer-cancel', request]);
  replies.set('answer-receipt', new Error('receipt unavailable'));
  assert.deepEqual(await commands.answer('session', request), {
    kind: 'unknown',
  });
  replies.set('answer-receipt', new RemoteError('invalid_request', 'changed payload'));
  await assert.rejects(commands.answer('session', request), /changed payload/);

  replies.set('answer-receipt', null);
  const attachments = [
    {
      name: 'brief.txt',
      ref: { kind: 'session_file', sessionId: 'projected-session', relativePath: 'brief.txt' },
    },
  ];
  const canonical = [
    { ...attachments[0], ref: { ...attachments[0].ref, sessionId: 'canonical-session' } },
  ];
  await commands.answer('projected-session', { ...request, attachments });
  assert.deepEqual(calls.at(-1), ['answer', { ...request, attachments: canonical }]);
  await assert.rejects(
    commands.answer('projected-session', {
      ...request,
      attachments: [{ ...attachments[0], ref: { ...attachments[0].ref, sessionId: 'foreign' } }],
    }),
    /foreign attachment/,
  );
  replies.set('enqueue', { disposition: 'followup' });
  assert.equal(
    await commands.enqueueMessage(
      'projected-session',
      'message',
      'queued',
      attachments,
      'next_turn',
      'observed-turn',
    ),
    'admitted',
  );
  assert.deepEqual(calls.at(-1), [
    'enqueue',
    {
      expectedTurnId: 'observed-turn',
      messageId: 'message',
      content: { text: 'queued', attachments: canonical },
      placement: 'next_turn',
    },
  ]);
  replies.set('enqueue', new Error('response lost'));
  assert.equal(
    await commands.enqueueMessage(
      'projected-session',
      'message',
      'queued',
      [],
      'next_turn',
      'observed-turn',
    ),
    'unknown',
  );
  replies.set('enqueue', new RemoteError('invalid_request', 'Turn ended'));
  assert.equal(
    await commands.enqueueMessage(
      'projected-session',
      'message',
      'queued',
      [],
      'next_turn',
      'observed-turn',
    ),
    'rejected',
  );

  // Retirement during reconciliation must not start a fresh submission afterward.
  const pending = Promise.withResolvers();
  replies.set('answer-receipt', pending.promise);
  calls.length = 0;
  const reconciling = commands.answer('session', request);
  lifetime.abort(new Error('retired'));
  pending.resolve(null);
  assert.deepEqual(await reconciling, { kind: 'unknown' });
  assert.deepEqual(calls, [['answer-receipt', request]]);
  await assert.rejects(commands.configureModel('session', {}), /retired/);
  await assert.rejects(commands.answer('session', request), /retired/);
  await assert.rejects(
    commands.enqueueMessage('session', 'message', 'queued', [], 'next_turn', 'turn'),
    /retired/,
  );
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

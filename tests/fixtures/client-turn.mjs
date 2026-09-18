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
import { createServer } from 'node:http';
import { once } from 'node:events';
import { setTimeout as delay } from 'node:timers/promises';
import { watchSession, assertText } from './client-subscription.mjs';
import { acknowledgeBusy } from './client-metadata.mjs';
import { restoreAsk, verifyBusyConfiguration } from './client-configuration.mjs';
import { verifyBlockedWorkspace } from './client-workspace.mjs';
import { firstReferencedTurn, assertReferencedModel } from './client-message-references.mjs';
import {
  emptyTail,
  completedTail,
  activeTail,
  cancelledTail,
  finishTranscriptObservers,
} from './client-transcript.mjs';

export async function modelFixture(toolCalls) {
  const requests = [];
  let failure;
  let partialReceived;
  let continuePartial;
  const partial = new Promise((resolve) => {
    partialReceived = resolve;
  });
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.url, '/v1/chat/completions');
      assert.equal(request.headers.authorization, 'Bearer dummy-local-fixture');
      let body = '';
      for await (const chunk of request) {
        body += chunk;
        assert(Buffer.byteLength(body) < 128 * 1024);
      }
      requests.push(JSON.parse(body));
      const index = requests.length;
      response.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
      const chunk = (delta, finish) => ({
        id: 'fixture-reply',
        object: 'chat.completion.chunk',
        created: 1,
        model: 'fixture-model',
        choices: [{ index: 0, delta, finish_reason: finish }],
      });
      const calls = await toolCalls?.(requests.at(-1), index);
      if (calls) {
        response.end(
          'data: ' +
            JSON.stringify(chunk({ tool_calls: calls }, null)) +
            '\n\ndata: ' +
            JSON.stringify(chunk({}, 'tool_calls')) +
            '\n\ndata: [DONE]\n\n',
        );
        return;
      }
      response.write(
        'data: ' +
          JSON.stringify(
            chunk(
              {
                content: index === 2 ? 'incomplete😀 fixture' : 'completed😀 ',
              },
              null,
            ),
          ) +
          '\n\n',
      );
      if (index === 2) {
        continuePartial = () =>
          response.write(
            'data: ' + JSON.stringify(chunk({ content: ' 🐈 suffix' }, null)) + '\n\n',
          );
        partialReceived();
        return; // Cancellation must close the open response; it is not a model finish.
      }
      response.write('data: ' + JSON.stringify(chunk({ content: 'fixture' }, null)) + '\n\n');
      response.write(
        'data: ' +
          JSON.stringify({
            ...chunk({}, null),
            choices: [],
            usage: { prompt_tokens: 7, completion_tokens: 3, total_tokens: 10 },
          }) +
          '\n\n',
      );
      response.end('data: ' + JSON.stringify(chunk({}, 'stop')) + '\n\ndata: [DONE]\n\n');
    } catch (error) {
      failure = error;
      response.destroy(error);
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  return {
    baseUrl: 'http://127.0.0.1:' + server.address().port + '/v1',
    requests,
    check() {
      if (failure) throw failure;
    },
    partial,
    continuePartial: () => continuePartial(),
    async close() {
      server.closeAllConnections();
      await new Promise((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      );
    },
  };
}

export async function verifyTurns(connection, sessionId, fixture, connectSibling) {
  const request = (operation, input) => connection.request(operation, input, 3000);
  const catalogBefore = await request('session.catalog.query', { kind: 'list_start' });
  const observer = await watchSession(connection, sessionId);
  assert.equal(observer.subscription.snapshot.rootTurn, null);
  assert.deepEqual(observer.subscription.activeAssistantStreams, []);
  assert.equal(observer.subscription.transcriptBootstrap, null);
  await emptyTail(connection, sessionId);
  const transcriptObserver = await watchSession(connection, sessionId, {
    kind: 'tail',
    maxBytes: 2,
  });
  const startedAt = Date.now();
  const notices = [];
  const unsubscribe = connection.subscribeSessionCatalogChanges((frame) => notices.push(frame));
  const first = await firstReferencedTurn(connection, sessionId, fixture);
  const started = await request('turn.start', first);
  assert.equal(started.kind, 'started');
  assert.deepEqual(started.skillInvocation, { loaded: [], failed: [], receipts: [] });
  assert.equal(started.turn.sessionId, sessionId);
  assert.notEqual(started.turn.runId, started.turn.turnId);
  const terminal = await waitTerminal(request, sessionId, first.turnId);
  assert.equal(terminal.status, 'completed');
  assert(terminal.terminalEventId);
  const finishedSession = await request('session.catalog.query', { kind: 'get', sessionId });
  assert.equal(finishedSession.session.status, 'active');
  assert.equal(finishedSession.session.lastMessagePreview, 'completed😀 fixture');
  await observer.terminal(terminal);
  assertText(observer.frames, first.turnId, 'completed😀 fixture');
  await completedTail(connection, sessionId, observer.frames, startedAt);
  assertReferencedModel(fixture.requests[0], connection.rootId);
  assert.deepEqual(
    fixture.requests[0].tools.map((tool) => tool.function.name),
    ['AskUserQuestion', 'Glob', 'Grep', 'Read', 'tool_search'],
    'next real turn uses Explore tool permissions',
  );
  assert(
    notices.some((frame) => frame.sessionId === sessionId),
    'execution commits notify catalog listeners',
  );
  const completed = await watchSession(connection, sessionId);
  assert.deepEqual(completed.subscription.snapshot.rootTurn, terminal);
  assert.deepEqual(completed.subscription.activeAssistantStreams, []);
  await completed.close();
  const changed = await request('session.catalog.query', {
    kind: 'list_continue',
    revision: catalogBefore.revision,
    cursor: sessionId,
  });
  assert.equal(changed.kind, 'revision_changed');
  assert.deepEqual((await request('turn.start', first)).turn, terminal);
  await assert.rejects(
    request('turn.start', { ...first, content: { text: 'different payload' } }),
    (error) => error.code === 'operation_conflict',
  );
  await restoreAsk(connection, sessionId);
  const second = {
    sessionId,
    turnId: 'cancelled-turn',
    content: { text: 'second question', inlineReferences: [] },
  };
  const active = await request('turn.start', second);
  await fixture.partial;
  assertReferencedModel(fixture.requests[1], connection.rootId);
  assert.deepEqual(
    fixture.requests[1].tools.map((tool) => tool.function.name).sort(),
    [
      'AskUserQuestion',
      'Edit',
      'Glob',
      'Grep',
      'Read',
      'Write',
      'apply_patch',
      'tool_search',
    ].sort(),
    'restored Ask is captured by next invocation',
  );
  await observer.waitFor(
    (frame) => frame.kind === 'subscription.session_delta' && frame.delta.turnId === second.turnId,
  );
  const attached = await watchSession(connection, sessionId, { kind: 'tail', maxBytes: 2 }, false);
  assert.equal(attached.subscription.snapshot.rootTurn.turnId, second.turnId);
  assert.equal(attached.subscription.snapshot.rootTurn.status, 'running');
  assert.equal(attached.subscription.activeAssistantStreams.length, 1);
  fixture.continuePartial();
  await observer.waitFor(
    (frame) =>
      frame.kind === 'subscription.session_delta' && frame.delta.text.endsWith(' 🐈 suffix'),
  );
  await connection.status(3000);
  assert.deepEqual(attached.frames, [], 'Host holds stream frames until the client is ready');
  await attached.subscription.ready();
  const suffix = await attached.waitFor(
    (frame) =>
      frame.kind === 'subscription.session_delta' && frame.delta.text.endsWith(' 🐈 suffix'),
  );
  assert.equal(suffix.delta.messageId, attached.subscription.activeAssistantStreams[0].messageId);
  await activeTail(connection, attached, connectSibling);
  await attached.close();
  const sibling = await connectSibling();
  try {
    const foreign = await sibling.openSessionSubscription(
      { sessionId, transcript: { kind: 'none' } },
      3000,
    );
    await assert.rejects(
      request('subscription.close', { subscriptionId: foreign.subscriptionId }),
      (error) => error.code === 'not_found',
    );
    await sibling.close();
    await sibling.closed;
    const deadline = Date.now() + 3000;
    while ((await connection.status(3000)).connections !== 1) {
      assert(Date.now() < deadline, 'disconnected observer retains a connection registration');
      await delay(5);
    }
    assert.deepEqual(
      await request('subscription.close', { subscriptionId: foreign.subscriptionId }),
      { subscriptionId: foreign.subscriptionId },
      'disconnect releases subscription ownership',
    );
  } finally {
    await sibling.close();
  }
  assert.equal(
    (await request('turn.query', { sessionId, turnId: second.turnId })).status,
    'running',
    'closing observer does not stop execution',
  );
  const liveSession = await request('session.catalog.query', { kind: 'get', sessionId });
  assert.equal(liveSession.session.status, 'running');
  assert.deepEqual(liveSession.session.liveRunState.runningTurnIds, [second.turnId]);
  await acknowledgeBusy(connection, sessionId);
  await verifyBusyConfiguration(connection, sessionId);
  await verifyBlockedWorkspace(
    connection,
    (await request('session.catalog.query', { kind: 'get', sessionId })).session,
  );
  await assert.rejects(
    request('turn.start', {
      sessionId,
      turnId: 'busy-turn',
      content: { text: 'must not execute' },
    }),
    (error) => error.code === 'session_busy',
  );
  const stopped = await request('turn.stop', {
    sessionId,
    turnId: second.turnId,
    runId: active.turn.runId,
  });
  assert.equal(stopped.runId, active.turn.runId);
  const cancelled = await waitTerminal(request, sessionId, second.turnId);
  assert.equal(cancelled.status, 'cancelled');
  assert.equal(cancelled.abortSource, 'runtime_cancellation');
  await observer.terminal(cancelled);
  assertText(observer.frames, second.turnId, 'incomplete😀 fixture 🐈 suffix');
  await cancelledTail(connection, sessionId, observer.frames);
  const cancelledSession = await request('session.catalog.query', { kind: 'get', sessionId });
  assert.equal(
    cancelledSession.session.hasUnread,
    true,
    'finalization raises unread after active ack',
  );
  assert.equal(cancelledSession.session.status, 'aborted');
  assert.deepEqual(cancelledSession.session.liveRunState.runningTurnIds, []);
  const third = await request('turn.start', {
    sessionId,
    turnId: 'after-cancel',
    content: { text: 'third question' },
  });
  assert.equal((await waitTerminal(request, sessionId, third.turn.turnId)).status, 'completed');
  assert.equal(fixture.requests.length, 3, 'replays/conflicts must not issue model requests');
  const history = fixture.requests[2].messages;
  assert(
    history.some(
      (message) => message.role === 'assistant' && message.content === 'completed😀 fixture',
    ),
  );
  assert(
    !JSON.stringify(history).includes('incomplete😀 fixture'),
    'partial cancelled output is not completed model history',
  );
  await observer.terminal(await request('turn.query', { sessionId, turnId: third.turn.turnId }));
  await finishTranscriptObservers(connection, sessionId, observer, transcriptObserver);
  for (let index = 1; index < notices.length; index++)
    assert(notices[index].revision > notices[index - 1].revision);
  unsubscribe();
  await observer.close();
  console.log(JSON.stringify({ check: 'original-client-turn-lifecycle', result: 'passed' }));
}

async function waitTerminal(request, sessionId, turnId) {
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    const snapshot = await request('turn.query', { sessionId, turnId });
    if (['completed', 'failed', 'cancelled'].includes(snapshot.status)) return snapshot;
    await delay(10);
  }
  throw new Error('Turn did not reach a canonical terminal fact');
}

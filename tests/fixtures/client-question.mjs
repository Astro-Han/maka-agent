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
import { once } from 'node:events';
import { access, readFile, rename, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { events } from './client-capability-model-fixture.mjs';
import { watchSession } from './client-subscription.mjs';

const sessionId = 'native-questions';
const questions = ['Pick\u0001one', 'Explain your choice', 'Optional detail'].map((question) => ({
  question,
  options: [{ label: 'Alpha', description: 'First choice' }, { label: 'Beta' }],
}));
const answers = ['Beta', 'a free answer outside the labels', null];

export async function verifyQuestions(connection, workspace, reopened, permissionMode) {
  const request = (op, input) => connection.request(op, input, 3000);
  const query = (interactionId) => request('interaction.query', { sessionId, interactionId });
  const answer = (interactionId, value) =>
    request('interaction.answer', { sessionId, interactionId, answer: value });
  const saved = join(workspace, 'questions.json');
  if (reopened) {
    for (const snapshot of JSON.parse(await readFile(saved, 'utf8'))) {
      assert.deepEqual(await query(snapshot.interactionId), snapshot);
      const operation = answer(snapshot.interactionId, { kind: 'question', answers });
      if (snapshot.status === 'answered') assert.deepEqual(await operation, snapshot);
      else await assert.rejects(operation, (error) => error.code === 'already_resolved');
    }
    const live = await watchSession(connection, sessionId);
    assert.equal(live.subscription.snapshot.interactions.pending.length, 0);
    await live.close();
    return;
  }
  async function checkpoint(name, value) {
    const file = join(workspace, name);
    await writeFile(file + '.tmp', JSON.stringify(value));
    await rename(file + '.tmp', file + '.json');
    for (;;) {
      try {
        await access(file + '.ok');
        return;
      } catch {
        await delay(5);
      }
    }
  }
  let modelCalls = 0,
    failure,
    live;
  const server = createServer(async (req, res) => {
    try {
      assert.equal(req.url, '/v1/responses');
      let raw = '';
      for await (const chunk of req) raw += chunk;
      const input = JSON.parse(raw),
        index = ++modelCalls;
      assert(index <= 4, 'no unexpected model continuation or replay');
      assert(input.tools.some((tool) => tool.name === 'AskUserQuestion'));
      if (index === 2) {
        const output = input.input.at(-1);
        assert.equal(output.type, 'function_call_output');
        const text =
          typeof output.output === 'string'
            ? output.output
            : output.output.map((part) => part.text).join('');
        assert.deepEqual(JSON.parse(text), {
          answers: questions.map(({ question }, i) => ({ question, answer: answers[i] })),
        });
        await checkpoint('model-result', { output });
      }
      const asking = index === 1 || index === 3;
      const payload = events({ tool: asking, name: 'AskUserQuestion', index }, index);
      if (asking)
        for (const event of payload) {
          if (event.type === 'response.function_call_arguments.delta')
            event.delta = JSON.stringify({ questions });
          if (event.type === 'response.output_item.done')
            event.item.arguments = JSON.stringify({ questions });
        }
      res.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
      res.end(payload.map((event) => 'data: ' + JSON.stringify(event) + '\n\n').join(''));
    } catch (error) {
      failure = error;
      res.destroy(error);
    }
  });
  server.on('upgrade', (_request, socket) => {
    socket.end('HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n');
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  try {
    const baseUrl = 'http://127.0.0.1:' + server.address().port + '/v1';
    const created = await request('connection.catalog.create', {
      expectedCatalogRevision: 0,
      connection: {
        slug: 'questions',
        name: 'Questions',
        providerType: 'openai',
        baseUrl,
        enabled: true,
        enabledModelIds: ['gpt-5.2'],
      },
    });
    assert.equal(created.kind, 'committed');
    const basis = created.connection;
    assert.equal(
      (
        await request('credential.vault.set', {
          locator: { scope: 'connection', connectionId: basis.connectionId, kind: 'api_key' },
          expected: null,
          expectedConnection: {
            ...basis,
            slug: 'questions',
            providerType: 'openai',
            effectiveBaseUrl: baseUrl,
          },
          secret: 'dummy-question-fixture',
        })
      ).kind,
      'committed',
    );
    await request('connection.catalog.set-default-target', {
      expectedCatalogRevision: created.catalogRevision,
      target: { connectionId: basis.connectionId, modelId: 'gpt-5.2' },
    });
    await request('session.create', {
      sessionId,
      workspace: { kind: 'host_path', path: workspace },
      modelTarget: { kind: 'default' },
      permissionMode,
      ...(permissionMode === 'explore' ? { mode: 'bot' } : {}),
    });
    live = await watchSession(connection, sessionId);
    const snapshots = [];
    for (const ordinal of [1, 2, 3]) {
      const turnId = 'question-' + ordinal;
      await request('turn.start', { sessionId, turnId, content: { text: turnId }, maxSteps: 2 });
      if (ordinal < 3) {
        const projection = await live.waitFor(
          (frame) =>
            frame.kind === 'subscription.session_projection' &&
            frame.snapshot.interactions.pending.some((pending) => pending.turnId === turnId),
        );
        const pending = projection.snapshot.interactions.pending.find((p) => p.turnId === turnId);
        assert.equal(projection.snapshot.rootTurn.status, 'waiting_for_user');
        assert.equal(
          (await request('turn.query', { sessionId, turnId })).status,
          'waiting_for_user',
        );
        assert.equal(pending.request.kind, 'question');
        assert.equal(pending.request.questions.length, 3);
        assert.notEqual(pending.request.questions[0].question, questions[0].question);
        assert(!pending.request.questions[0].question.includes('\u0001'));
        assert.deepEqual(await query(pending.interactionId), pending);
        await checkpoint('pending-' + ordinal, { pending });
        if (ordinal === 1) {
          for (const invalid of [
            { kind: 'question', answers: ['Alpha'] },
            { kind: 'client_capability', decision: 'allow' },
          ]) {
            await assert.rejects(
              answer(pending.interactionId, invalid),
              (e) => e.code === 'operation_conflict',
            );
            assert.deepEqual(await query(pending.interactionId), pending);
          }
          await checkpoint('invalid', { pending });
          const value = { kind: 'question', answers };
          const resolved = await answer(pending.interactionId, value);
          assert.equal(resolved.status, 'answered');
          assert.equal(resolved.outcome.kind, 'question_answer');
          assert.deepEqual(resolved.outcome.answers, answers);
          assert(Number.isSafeInteger(resolved.outcome.committedAt));
          assert.deepEqual(await answer(pending.interactionId, value), resolved);
          await assert.rejects(
            answer(pending.interactionId, { kind: 'question', answers: ['Alpha', null, null] }),
            (e) => e.code === 'already_resolved',
          );
          snapshots.push(resolved);
        } else {
          await request('turn.stop', { sessionId, turnId, runId: pending.runId });
          await live.waitFor(
            (f) =>
              f.kind === 'subscription.session_projection' &&
              f.snapshot.rootTurn?.turnId === turnId &&
              f.snapshot.rootTurn.status === 'cancelled',
          );
          const closed = await query(pending.interactionId);
          assert.equal(closed.status, 'closed');
          assert.equal(closed.outcome.reason, 'turn_stopped');
          await assert.rejects(
            answer(pending.interactionId, { kind: 'question', answers }),
            (e) => e.code === 'already_resolved',
          );
          snapshots.push(closed);
          await checkpoint('stopped', { closed });
          assert.equal((await connection.status(3000)).state, 'ready');
          continue;
        }
      }
      await live.waitFor(
        (f) =>
          f.kind === 'subscription.session_projection' &&
          f.snapshot.rootTurn?.turnId === turnId &&
          f.snapshot.rootTurn.status === 'completed',
      );
      assert.equal((await request('turn.query', { sessionId, turnId })).status, 'completed');
      if (failure) throw failure;
    }
    assert.equal(modelCalls, 4);
    await writeFile(saved, JSON.stringify(snapshots));
    await checkpoint('final', snapshots);
  } finally {
    server.closeAllConnections();
    await live?.close();
    await new Promise((resolve) => server.close(resolve));
    if (failure) throw failure;
  }
}

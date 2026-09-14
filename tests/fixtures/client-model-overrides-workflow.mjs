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
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import {
  catalog,
  configure,
  inputOnlyId,
  modelId,
  sessionInput,
  verifyCatalogPins,
  updateDeclaration,
} from './client-model-overrides-catalog.mjs';
import { evidence, modelOverridesFixture } from './client-model-overrides-fixture.mjs';
import { verifyFactsConnectionTests } from './client-model-overrides-tests.mjs';

async function terminal(request, fixture, sessionId, turnId) {
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    fixture.check();
    const turn = await request('turn.query', { sessionId, turnId });
    if (['completed', 'cancelled', 'failed'].includes(turn.status)) {
      assert.equal(turn.status, 'completed');
      return turn;
    }
    await delay(10);
  }
  fixture.check();
  throw new Error('Model facts turn did not finish');
}
function turnInput(sessionId, turnId, marker, maxSteps = 1) {
  return { sessionId, turnId, content: { text: marker }, maxSteps };
}
function diagnostics(value, model, window, inputTokens) {
  assert.equal(value.status, 'available');
  assert.equal(value.providerId, 'openai');
  assert.equal(value.modelId, model);
  assert.equal(value.contextWindow, window);
  assert.equal(value.inputTokens, inputTokens);
}
export async function verifyModelOverrides(connection, workspace, reopened, openClient) {
  const request = (operation, input) => connection.request(operation, input, 5000);
  const path = join(workspace, 'model-overrides-fixture.json');
  const saved = reopened ? JSON.parse(await readFile(path, 'utf8')) : undefined;
  const fixture = await modelOverridesFixture(saved ? Number(new URL(saved.baseUrl).port) : 0);
  const queryDiagnostics = (sessionId) => request('context.diagnostics.query', { sessionId });
  const start = async (input) => {
    await request('turn.start', input);
    return { input, terminal: await terminal(request, fixture, input.sessionId, input.turnId) };
  };
  try {
    if (reopened) {
      assert.deepEqual(await catalog(request), saved.catalog);
      for (const session of saved.sessions) {
        const queried = await request('session.catalog.query', {
          kind: 'get',
          sessionId: session.input.sessionId,
        });
        assert.deepEqual(queried.session, session.snapshot);
        assert.deepEqual(await request('session.create', session.input), session.snapshot);
      }
      for (const turn of saved.turns)
        assert.deepEqual((await request('turn.start', turn.input)).turn, turn.terminal);
      assert.deepEqual(
        await queryDiagnostics('facts-frozen'),
        saved.nextDiagnostics,
        'later pin edits do not rewrite the saved completed-request diagnostic',
      );
      fixture.expect({
        path: '/v1/responses',
        model: modelId,
        outputLimit: 12345,
        parallel: true,
        marker: 'FACTS_REOPEN',
        answer: 'facts reopened complete',
        toolEvidence: true,
      });
      const turn = await start(turnInput('facts-frozen', 'facts-reopened', 'FACTS_REOPEN'));
      const current = await queryDiagnostics('facts-frozen');
      diagnostics(current, modelId, saved.finalPin.contextWindow, 43);
      fixture.verify();
      await writeFile(
        join(workspace, 'model-overrides-reopened.json'),
        JSON.stringify({
          turn,
          beforeDiagnostics: saved.nextDiagnostics,
          diagnostics: current,
          http: fixture.records,
        }),
      );
      return;
    }

    const rows = await configure(request, fixture.baseUrl);
    const catalogChecks = await verifyCatalogPins(request, rows, workspace);
    const inputs = [
      sessionInput(workspace, rows[0], 'facts-input-only', inputOnlyId),
      sessionInput(workspace, rows[0], 'facts-frozen'),
      sessionInput(workspace, rows[1], 'facts-other-connection'),
    ];
    for (const input of inputs) await request('session.create', input);
    const turns = [];
    // Usage exceeds the displayed inputLimit. No contextWindow was declared for
    // this model, so the second real request must remain Main, not a summary.
    for (let i = 1; i <= 2; i++) {
      const marker = 'FACTS_INPUT_ONLY_' + i;
      fixture.expect({
        path: '/v1/chat/completions',
        model: inputOnlyId,
        parallel: true,
        marker,
        answer: 'input-only main ' + i,
        tokens: 100,
      });
      turns.push(await start(turnInput('facts-input-only', 'input-only-' + i, marker)));
    }
    const inputOnlyDiagnostics = await queryDiagnostics('facts-input-only');
    diagnostics(inputOnlyDiagnostics, inputOnlyId, 20, 100);

    await writeFile(join(workspace, 'facts-evidence.txt'), evidence);
    const gate = fixture.expect({
      path: '/v1/chat/completions',
      model: modelId,
      outputLimit: 12345,
      parallel: false,
      marker: 'FACTS_FROZEN',
      read: true,
      hold: true,
    });
    fixture.expect({
      path: '/v1/chat/completions',
      model: modelId,
      outputLimit: 12345,
      parallel: false,
      marker: 'FACTS_FROZEN',
      toolResult: true,
      answer: 'facts frozen complete',
    });
    const frozenInput = turnInput('facts-frozen', 'frozen', 'FACTS_FROZEN', 2);
    await request('turn.start', frozenInput);
    await gate.wait();
    const nextPin = {
      ...catalogChecks.pin,
      apiProtocol: 'openai-responses',
      contextWindow: 96000,
      compactionThreshold: 96000,
      capabilities: { ...catalogChecks.pin.capabilities, parallelToolCalls: true },
    };
    await updateDeclaration(request, rows[0], nextPin);
    const during = await catalog(request);
    gate.release();
    turns.push({
      input: frozenInput,
      terminal: await terminal(request, fixture, frozenInput.sessionId, frozenInput.turnId),
    });
    const frozenDiagnostics = await queryDiagnostics('facts-frozen');
    diagnostics(frozenDiagnostics, modelId, 64000, 42);

    fixture.expect({
      path: '/v1/responses',
      model: modelId,
      outputLimit: 12345,
      parallel: true,
      marker: 'FACTS_NEXT',
      answer: 'facts next complete',
      toolEvidence: true,
    });
    turns.push(await start(turnInput('facts-frozen', 'next', 'FACTS_NEXT')));
    const nextDiagnostics = await queryDiagnostics('facts-frozen');
    diagnostics(nextDiagnostics, modelId, 96000, 43);
    const observer = await openClient();
    const tests = await verifyFactsConnectionTests(
      request,
      fixture,
      rows[0],
      nextPin,
      (operation, input) => observer.request(operation, input, 5000),
    );
    assert.deepEqual(
      await queryDiagnostics('facts-frozen'),
      nextDiagnostics,
      'window-only edits and connection tests preserve the actual Main snapshot',
    );
    const sessions = [];
    for (const input of inputs) {
      const queried = await request('session.catalog.query', {
        kind: 'get',
        sessionId: input.sessionId,
      });
      sessions.push({ input, snapshot: queried.session });
    }
    fixture.verify();
    const result = {
      baseUrl: fixture.baseUrl,
      rows,
      catalogChecks,
      during,
      finalPin: tests.pin,
      catalog: await catalog(request),
      sessions,
      turns,
      inputOnlyDiagnostics,
      frozenDiagnostics,
      nextDiagnostics,
      tests,
      http: fixture.records,
    };
    await writeFile(path, JSON.stringify(result));
    console.log(
      JSON.stringify({
        check: 'original-client-model-overrides',
        mainRequests: 5,
        connectionTests: 2,
        sessionIds: inputs.map((input) => input.sessionId),
      }),
    );
  } finally {
    await fixture.close();
  }
}

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
import {
  createLiveContextUsageTracker,
  liveContextUsageFromDiagnostics,
  selectLatestRequestUsage,
} from '../dist/context-usage.js';

const route = { model: 'model-a', providerType: 'provider-a' };
const snapshot = (inputTokens = 40_000, overrides = {}) => ({
  status: 'available', modelId: route.model, providerId: route.providerType,
  inputTokens, contextWindow: 128_000, ...overrides,
});

test('usage combines only the latest metered facts from the active route', () => {
  const anchor = { inputTokens: 100, outputTokens: 20, modelId: route.model, connectionId: 'connection-a' };
  const usage = (value) => ({ type: 'token_usage', lastRequestAnchor: value });
  const read = (messages) => selectLatestRequestUsage(messages, route.model, { llmConnectionId: 'connection-a' });
  assert.equal(read([usage({ ...anchor, inputTokens: 10 }), { type: 'assistant' }, usage(anchor), usage()]), 120);
  for (const changed of [
    { modelId: 'other' }, { connectionId: 'other' }, { modelId: undefined },
    { connectionId: undefined }, { inputTokens: 0 }, { inputTokens: NaN },
  ]) {
    assert.equal(read([usage(anchor), usage({ ...anchor, ...changed })]), undefined,
      'an incompatible newest anchor must not fall back to an older count');
  }
  assert.equal(selectLatestRequestUsage([usage(anchor)], undefined, { llmConnectionId: 'connection-a' }), undefined);
  assert.equal(selectLatestRequestUsage([usage(anchor)], route.model, undefined), undefined);

  assert.deepEqual(liveContextUsageFromDiagnostics(snapshot(), route), { usageTokens: 40_000, contextWindow: 128_000 });
  assert.deepEqual(liveContextUsageFromDiagnostics(snapshot(40_000, { contextWindow: undefined }), route), { usageTokens: 40_000 });
  for (const value of [
    undefined, { status: 'unavailable' }, snapshot(0),
    snapshot(NaN), snapshot(40_000, { modelId: 'other' }),
    snapshot(40_000, { providerId: 'other' }), snapshot(40_000, { inputTokens: undefined }),
  ]) {
    assert.equal(liveContextUsageFromDiagnostics(value, route), undefined);
  }
  assert.equal(liveContextUsageFromDiagnostics(snapshot(), { model: route.model }), undefined);
  assert.equal(liveContextUsageFromDiagnostics(snapshot(), { providerType: route.providerType }), undefined);
});

test('usage observation coalesces steps and retires stale reads across route changes and disposal', async () => {
  const scheduled = new Set();
  const reads = [];
  const seen = [];
  const tracker = createLiveContextUsageTracker({
    delayMs: 400,
    schedule(callback) { scheduled.add(callback); return callback; },
    cancel(callback) { scheduled.delete(callback); },
    query(sessionId) {
      const read = { sessionId, ...Promise.withResolvers() };
      reads.push(read);
      return read.promise;
    },
    onChange(value) { seen.push(value); },
  });
  const fire = () => {
    const callbacks = [...scheduled]; scheduled.clear();
    for (const callback of callbacks) callback();
  };
  const observe = (type) => tracker.observe({ type, id: type, turnId: 'turn', ts: 1 });
  const resolve = async (index, value) => { reads[index].resolve(value); await Promise.resolve(); };
  try {
    tracker.setTarget({ sessionId: 'a', route });
    assert.equal(reads[0].sessionId, 'a');
    await resolve(0, snapshot());
    assert.deepEqual(seen, [undefined, { usageTokens: 40_000, contextWindow: 128_000 }]);

    // Re-aiming at the same route and a transient error must not flicker.
    tracker.setTarget({ sessionId: 'a', route: { ...route } });
    reads[1].reject(new Error('temporarily disconnected'));
    await Promise.resolve();
    assert.equal(seen.length, 2);
    for (const type of ['text_delta', 'thinking_delta', 'tool_output_delta']) observe(type);
    assert.equal(scheduled.size, 0);
    for (const type of ['tool_start', 'tool_result', 'token_usage', 'provider_retry', 'complete', 'error', 'abort']) observe(type);
    assert.equal(scheduled.size, 1);
    assert.equal(reads.length, 2);
    fire();
    observe('token_usage'); fire();
    await resolve(3, snapshot(60_000));
    await resolve(2, snapshot(10_000));
    assert.equal(seen.at(-1).usageTokens, 60_000);

    tracker.setTarget({ sessionId: 'b', route });
    assert.equal(seen.at(-1), undefined);
    reads[4].reject(new Error('new Session is unavailable'));
    await Promise.resolve();
    assert.equal(seen.at(-1), undefined, 'failure cannot retain another Session’s count');
    tracker.setTarget({ sessionId: 'b', route });
    const other = { model: 'model-b', providerType: 'provider-b' };
    tracker.setTarget({ sessionId: 'b', route: other });
    await resolve(5, snapshot(99_000));
    assert.equal(seen.at(-1), undefined);
    await resolve(6, snapshot(5_000, { modelId: other.model, providerId: other.providerType }));
    assert.equal(seen.at(-1).usageTokens, 5_000);

    observe('token_usage'); fire();
    tracker.setTarget(undefined);
    const cleared = seen.length;
    await resolve(7, snapshot());
    assert.equal(seen.length, cleared);
    assert.equal(seen.at(-1), undefined);

    tracker.setTarget({ sessionId: 'a', route });
    observe('tool_result');
    assert.equal(scheduled.size, 1);
    tracker.dispose();
    assert.equal(scheduled.size, 0);
    const disposed = seen.length;
    fire(); await resolve(8, snapshot());
    assert.equal(seen.length, disposed);
    assert.equal(reads.length, 9);
  } finally { tracker.dispose(); }
});

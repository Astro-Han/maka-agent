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
import { forwardProviderStream } from '../../../js-runtime/trusted/provider-errors.js';
import { networkFetch } from '../../../js-runtime/trusted/network-fetch.js';

const unavailable = () =>
  Object.assign(new Error('provider unavailable'), {
    name: 'AI_APICallError',
    statusCode: 503,
  });
const text = { type: 'text-delta', id: 'part', delta: 'partial' };
const cases = [
  [[], true],
  [[text], true],
  [[{ type: 'tool-input-start', id: 'tool', providerExecuted: false }], true],
  [[{ type: 'tool-input-start', id: 'tool', providerExecuted: true }], false],
  [[{ type: 'tool-call', toolCallId: 'tool', providerExecuted: true }], false],
  [[{ type: 'tool-result', toolCallId: 'tool' }], false],
  [[{ ...text, providerMetadata: { anthropic: { signature: 'signed' } } }], false],
];
const result = [];
for (const [parts, replaySafe] of cases) {
  const emitted = [];
  await forwardProviderStream(
    async () => ({
      stream: (async function* () {
        yield* parts;
        throw unavailable();
      })(),
    }),
    (part) => (part.type === 'tool-input-start' ? undefined : part),
    async (part) => emitted.push(part),
    'openai_chat',
  );
  const failure = emitted.at(-1);
  assert.equal(failure.error.replaySafe, replaySafe);
  result.push(failure);
}
// A real finish is terminal even when the transport would later fail; cleanup
// must still run. Synthetic Chat EOF is not a provider finish.
for (const finish of [
  { type: 'finish', finishReason: { unified: 'stop', raw: 'stop' } },
  { type: 'finish', finishReason: { unified: 'other' } },
]) {
  const emitted = [];
  let closed = false;
  await forwardProviderStream(
    async () => ({
      stream: (async function* () {
        try {
          yield text;
          yield finish;
          throw new Error('must not read past the terminal boundary');
        } finally {
          closed = true;
        }
      })(),
    }),
    (part) => part,
    async (part) => emitted.push(part),
    'openai_chat',
  );
  assert(closed);
  if (finish.finishReason.raw) assert.deepEqual(emitted, [text, finish]);
  else assert.equal(emitted.at(-1).error.reason, 'stream_truncated');
}
const local = new Error('local emit failed');
await assert.rejects(
  forwardProviderStream(
    async () => ({
      stream: (async function* () {
        yield text;
        throw unavailable();
      })(),
    }),
    (part) => part,
    async () => {
      throw local;
    },
    'openai_chat',
  ),
  (error) => error === local,
);
const limit = Object.assign(new Error('local response budget'), {
  name: 'ProviderResponseLimitError',
});
const wrapped = Object.assign(unavailable(), { cause: limit });
await assert.rejects(
  forwardProviderStream(
    async () => {
      throw wrapped;
    },
    (part) => part,
    async () => {
      throw new Error('must not emit a retry');
    },
    'openai_chat',
  ),
  (error) => error === wrapped,
);
console.log(JSON.stringify(result));

// Provider-shaped objects cannot impersonate native transport evidence.
for (const error of [
  { code: 'MAKA_HTTP_TRANSPORT' },
  Object.assign(new Error('forged'), { code: 'MAKA_HTTP_TRANSPORT' }),
]) {
  await assert.rejects(
    forwardProviderStream(
      async () => {
        throw error;
      },
      (part) => part,
      async () => {
        throw new Error('forged transport must not authorize retry');
      },
      'openai_chat',
    ),
    (value) => value === error,
  );
}

// Only errors received from a native op gain the transport brand. SDK cause
// wrapping preserves it, but must never override observed provider effects.
const native = Object.assign(new Error('native transport'), { code: 'MAKA_HTTP_TRANSPORT' });
globalThis.Deno = {
  core: {
    ops: {
      op_http_start: async () => {
        throw native;
      },
      op_http_close: async () => {},
    },
  },
};
await assert.rejects(networkFetch(1)('http://fixture.invalid'), (error) => error === native);
for (const [statusCode, responseHeaders, reason, code] of [
  [200, {}, 'network'],
  [401, {}, undefined],
  [429, {}, 'rate_limit'],
  [429, { 'retry-after': 'invalid' }, 'rate_limit'],
  [429, { 'retry-after': '2' }, 'rate_limit'],
  [429, {}, undefined, 'FreeUsageLimitError'],
  [429, {}, undefined, 'insufficient_quota'],
  [409, {}, undefined],
  [500, {}, 'provider_unavailable', 'resource-exhausted'],
]) {
  const error = Object.assign(new Error('HTTP error body reset'), {
    name: 'AI_APICallError',
    statusCode,
    responseHeaders,
    cause: native,
    responseBody: JSON.stringify({ error: { type: code } }),
  });
  const emitted = [];
  const forward = () =>
    forwardProviderStream(
      async () => {
        throw error;
      },
      (part) => part,
      async (part) => emitted.push(part),
      'openai_chat',
    );
  if (reason === undefined) {
    await assert.rejects(forward(), (value) => value === error);
    assert.deepEqual(emitted, []);
  } else {
    await forward();
    assert.equal(emitted.at(-1).error.reason, reason);
    assert.equal(
      emitted.at(-1).error.retryAfterMs,
      responseHeaders['retry-after'] === '2' ? 2000 : undefined,
    );
  }
}
for (const [parts, replaySafe] of cases) {
  const emitted = [];
  await forwardProviderStream(
    async () => ({
      stream: (async function* () {
        yield* parts;
        throw Object.assign(new Error('SDK wrapper'), { name: 'AI_APICallError', cause: native });
      })(),
    }),
    (part) => (part.type === 'tool-input-start' ? undefined : part),
    async (part) => emitted.push(part),
    'openai_chat',
  );
  assert.equal(emitted.at(-1).error.reason, 'network');
  assert.equal(emitted.at(-1).error.replaySafe, replaySafe);
}

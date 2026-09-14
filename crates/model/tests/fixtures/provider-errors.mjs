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
  [[{ type: 'finish' }], false],
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

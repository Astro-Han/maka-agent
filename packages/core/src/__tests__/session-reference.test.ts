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
import { test } from 'node:test';
import { createSessionQuote } from '../session-reference.js';
import { isQuoteRef, messageContentsEqual, normalizeMessageContent } from '../events.js';
import type { StoredMessage } from '../session.js';

const user = (text: string): StoredMessage => ({
  type: 'user',
  id: 'message',
  turnId: 'turn',
  ts: 1234.5,
  text,
});

test('Session quotes bound Unicode text, retain snapshot provenance and exclude runtime records', () => {
  const source = { sessionId: 'source', sessionName: 'Research 😀', capturedAt: 1234.5 };
  const latest = createSessionQuote([user('old'), user('😀'.repeat(20_000))], source, false);
  assert.equal(latest.source.truncated, true);
  assert.ok(new TextEncoder().encode(latest.text).length <= 32_000);
  assert.ok(latest.text.length <= 12_000);
  assert.equal(new TextDecoder().decode(new TextEncoder().encode(latest.text)), latest.text);
  assert.ok(latest.text.startsWith('User: 😀'));
  assert.equal(latest.text.includes('old'), false);
  const quoted = createSessionQuote(
    [
      user('kept'),
      {
        type: 'assistant',
        id: 'answer',
        turnId: 'turn',
        ts: 1235.5,
        text: 'answer',
        modelId: 'model',
      },
      {
        type: 'tool_result',
        id: 'tool',
        turnId: 'turn',
        ts: 1236,
        toolUseId: 'call',
        isError: false,
        content: { kind: 'text', text: 'not part of the quote' },
      },
    ],
    source,
    true,
  );
  assert.equal(quoted.text, 'User: kept\n\nAssistant: answer');
  assert.deepEqual(quoted.source, { ...source, truncated: true });
  assert.ok(isQuoteRef(quoted));
  const normalized = normalizeMessageContent({ text: '', quotes: [quoted] });
  assert.deepEqual(normalized.quotes?.[0], quoted);
  assert.notEqual(normalized.quotes?.[0]?.source, quoted.source);
  assert.equal(
    messageContentsEqual(normalized, {
      text: '',
      quotes: [{ ...quoted, source: { ...quoted.source, capturedAt: 1235.5 } }],
    }),
    false,
  );
  assert.equal(isQuoteRef({ ...quoted, source: { sessionId: 'source' } }), false);
});

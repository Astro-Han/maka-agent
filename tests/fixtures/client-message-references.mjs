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
import { formatTextWithInlineRefs } from '../../packages/runtime/src/model-history.ts';

export function referencedContent(rootId) {
  const displayText = 'First visible question 😀 @source.rs';
  return {
    text: 'first question',
    displayText,
    quotes: [
      {
        text: 'quoted 😀 <not-a-command>\nsecond line',
        label: 'A "quoted" label',
        sourceTurnId: 'older-turn',
      },
      { text: 'unlabelled excerpt' },
    ],
    directoryReferences: [{ hostId: rootId, path: '/missing-directory-<directory_references>&' }],
    inlineReferences: [
      {
        kind: 'workspace_file',
        value: '@source.rs',
        label: 'source.rs',
        start: displayText.indexOf('@'),
      },
    ],
  };
}

export async function firstReferencedTurn(connection, sessionId, fixture) {
  const first = {
    sessionId,
    turnId: 'first-turn',
    content: referencedContent(connection.rootId),
    maxSteps: 2,
  };
  await assert.rejects(
    connection.request(
      'turn.start',
      {
        ...first,
        content: {
          ...first.content,
          directoryReferences: [{ hostId: 'different-host', path: '/missing' }],
        },
      },
      3000,
    ),
    (error) => error.code === 'operation_unavailable',
  );
  await assert.rejects(
    connection.request('turn.query', { sessionId, turnId: first.turnId }, 3000),
    (error) => error.code === 'not_found',
    'foreign Host reference cannot establish an invocation',
  );
  assert.equal(fixture.requests.length, 0);
  return first;
}

export function assertReferencedModel(input, rootId) {
  const users = input.messages.filter((message) => message.role === 'user');
  assert.equal(users[0].content, formatTextWithInlineRefs(referencedContent(rootId)));
  assert(!users[0].content.includes('First visible'), 'display text must not replace model input');
  assert(!users[0].content.includes('@source.rs'), 'display markers do not inject file contents');
}

export function assertReferencedRow(row, rootId) {
  const expected = referencedContent(rootId);
  for (const key of Object.keys(expected)) {
    assert.deepEqual(row[key], expected[key], 'raw references survive presentation: ' + key);
  }
  assert(!row.text.includes('<quoted_excerpt>'), 'UI cannot display a model-only projection');
}

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
import { readPage } from '../../packages/runtime/src/read-page.ts';
import { once } from 'node:events';
import { createServer } from 'node:http';

export const original = 'ORIGINAL_CONTEXT_EVIDENCE_' + 'history detail '.repeat(600);
export const summary = [
  '## Goal',
  'COMPACT_BASELINE_PERSISTED: Verify that the saved evidence remains available through a summary.',
  '## Progress',
  'The evidence file was read successfully and its result was acknowledged.',
  '## Next Steps',
  'Continue answering the user from this checkpoint and retain subsequent conversation turns.',
  '## Critical Context',
  'The completed Read used evidence.txt in the authorized workspace. No tool operation is pending.',
].join('\n');
export const malformed = 'REJECTED_COMPACTION_TEXT lacks every required summary section.';

export async function contextCompactFixture(port = 0, reopened = false) {
  let failure,
    count = 0,
    release;
  let arrived;
  const summaryRequested = new Promise((resolve) => {
    arrived = resolve;
  });
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.url, '/v1/chat/completions');
      assert.equal(request.headers.authorization, 'Bearer context-compact-fixture');
      const chunks = [];
      let bytes = 0;
      for await (const chunk of request) {
        bytes += chunk.length;
        assert(bytes <= 256 * 1024);
        chunks.push(chunk);
      }
      const input = JSON.parse(Buffer.concat(chunks).toString());
      const step = ++count;
      assert(step <= (reopened ? 1 : 7));
      assert.equal(input.model, 'fixture-model');
      assert.equal(input.stream, true);
      const serialized = JSON.stringify(input.messages);
      const compact = !reopened && [3, 5, 6].includes(step);
      if (compact) {
        assert(!input.tools?.length, 'summary request must not expose executable tools');
        assert.equal(input.messages.at(-1).role, 'user');
        assert(/summar|compact/i.test(JSON.stringify(input.messages.at(-1).content)));
      } else assert(input.tools?.some((tool) => tool.function.name === 'Read'));
      if (!reopened && step <= 3) {
        assert(serialized.includes(original), 'initial summary folds actual original history');
        if (step > 1) {
          const result = input.messages.findLast((message) => message.role === 'tool');
          assert.equal(result.tool_call_id, 'read-evidence');
          assert.deepEqual(
            JSON.parse(result.content),
            readPage('compact evidence read successfully\n', { path: 'evidence.txt' }),
          );
        }
      } else {
        assert(
          serialized.includes(JSON.stringify(summary).slice(1, -1)),
          'next requests must use the complete durable summary baseline',
        );
        assert(!serialized.includes(original), 'covered original text must leave model projection');
        assert(
          !input.messages.some((message) => message.role === 'tool' || message.tool_calls?.length),
        );
        if (!compact)
          assert(!serialized.includes(malformed), 'rejected summary cannot become a baseline');
        if (reopened || step >= 5) assert(serialized.includes('tail after good compact'));
      }
      if (!reopened && step === 3) {
        arrived();
        await gate;
      }
      const read = !reopened && step === 1;
      const delta = read
        ? {
            tool_calls: [
              {
                index: 0,
                id: 'read-evidence',
                type: 'function',
                function: { name: 'Read', arguments: JSON.stringify({ path: 'evidence.txt' }) },
              },
            ],
          }
        : { content: compact ? (step === 3 ? summary : malformed) : 'main turn complete' };
      const frame = (delta, finish_reason) =>
        'data: ' +
        JSON.stringify({
          id: 'context-compact-' + step,
          object: 'chat.completion.chunk',
          created: 1,
          model: input.model,
          choices: [{ index: 0, delta, finish_reason }],
        }) +
        '\n\n';
      response.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
      response.end(
        frame(delta, null) + frame({}, read ? 'tool_calls' : 'stop') + 'data: [DONE]\n\n',
      );
    } catch (error) {
      failure = error;
      response.destroy(error);
    }
  });
  server.listen(port, '127.0.0.1');
  await once(server, 'listening');
  return {
    baseUrl: 'http://127.0.0.1:' + server.address().port + '/v1',
    summaryRequested,
    release,
    check() {
      if (failure) throw failure;
    },
    verify() {
      this.check();
      assert.equal(count, reopened ? 1 : 7);
    },
    async close() {
      release();
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

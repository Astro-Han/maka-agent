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

export async function compatibleFixture(port = 0, reopened = false) {
  let count = 0,
    failure,
    rejected;
  const steps = new Map();
  const server = createServer(async (request, response) => {
    try {
      let body = '';
      for await (const chunk of request) {
        body += chunk;
        assert(Buffer.byteLength(body) < 128 * 1024);
      }
      const input = JSON.parse(body);
      count++;
      assert.equal(request.url, '/v1/chat/completions');
      assert.equal(request.headers.authorization, 'Bearer compatible-fixture');
      assert.equal(input.model, 'fixture-model');
      assert.equal(input.stream, true);
      if (count === 1) {
        assert.deepEqual(input.stream_options, { include_usage: true });
        rejected = input;
        response.writeHead(400, { 'Content-Type': 'application/json' });
        response.end(
          JSON.stringify({
            error: {
              message: 'stream_options.include_usage is not supported',
              type: 'invalid_request_error',
              param: 'stream_options.include_usage',
            },
          }),
        );
        return;
      }
      assert(
        !Object.hasOwn(input, 'stream_options'),
        'endpoint cache survives fresh model isolates',
      );
      if (count === 2) {
        const { stream_options, ...original } = rejected;
        assert.deepEqual(input, original, '400 retry changes only stream_options');
      }
      const user = input.messages.filter((m) => m.role === 'user').at(-1).content;
      const field = user.startsWith('reasoning_content:') ? 'reasoning_content' : 'reasoning';
      const turn = user.endsWith(':max') ? 'max' : 'high';
      assert.equal(input.reasoning_effort, turn);
      const key = field + ':' + turn;
      const step = (steps.get(key) ?? 0) + 1;
      steps.set(key, step);
      assert(step <= 2);
      const expectedReasoning = field === 'reasoning' ? 'first ' : '';
      const calls = input.messages.filter((m) => m.role === 'assistant' && m.tool_calls?.length);
      if (reopened && step === 1) assert.equal(calls.length, 2, 'reopen replays both stored turns');
      for (const assistant of calls) {
        assert.equal(assistant[field], expectedReasoning);
        assert(
          !Object.hasOwn(assistant, field === 'reasoning' ? 'reasoning_content' : 'reasoning'),
        );
        assert.equal(assistant.content, 'before after');
      }
      if (step === 2) {
        assert(calls.length > 0);
        const result = input.messages.at(-1);
        assert.equal(result.role, 'tool');
        assert.equal(result.tool_call_id, 'read-reused');
        assert.deepEqual(
          JSON.parse(result.content),
          readPage('compatible tool evidence\n', { path: 'evidence.txt' }),
        );
      }
      const frame = (delta, finish_reason = null) =>
        'data: ' +
        JSON.stringify({
          id: 'compatible-' + count,
          object: 'chat.completion.chunk',
          created: 1,
          model: input.model,
          choices: [{ index: 0, delta, finish_reason }],
        }) +
        '\n\n';
      let stream = '';
      if (step === 1) {
        stream += frame({ [field]: field === 'reasoning' ? 'first ' : '' });
        stream += frame({ content: 'before ' });
        if (field === 'reasoning') stream += frame({ [field]: 'second' });
        stream += frame({ content: 'after' });
        stream += frame({
          tool_calls: [
            {
              index: 0,
              id: 'read-reused',
              type: 'function',
              function: { name: 'Read', arguments: JSON.stringify({ path: 'evidence.txt' }) },
            },
          ],
        });
      } else stream += frame({ content: 'done' });
      response.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
      response.end(stream + frame({}, step === 1 ? 'tool_calls' : 'stop') + 'data: [DONE]\n\n');
    } catch (error) {
      failure = error;
      response.destroy(error);
    }
  });
  server.listen(port, '127.0.0.1');
  await once(server, 'listening');
  return {
    baseUrl: 'http://127.0.0.1:' + server.address().port + '/v1',
    check() {
      if (failure) throw failure;
    },
    verify() {
      this.check();
      assert.equal(count, reopened ? 5 : 9);
      assert.equal(steps.size, reopened ? 2 : 4);
      assert([...steps.values()].every((n) => n === 2));
    },
    async close() {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

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
import { createServer } from 'node:http';

export async function relayFixture(port) {
  let count = 0,
    failure,
    expected;
  const server = createServer(async (request, response) => {
    try {
      let body = '';
      for await (const chunk of request) {
        body += chunk;
        assert(Buffer.byteLength(body) < 128 * 1024);
      }
      const input = JSON.parse(body);
      count += 1;
      assert(expected, 'rejected admission must not reach HTTP');
      assert.equal(request.url, '/v1/responses');
      assert.equal(request.headers.authorization, 'Bearer relay-fixture');
      assert.equal(input.model, expected.model);
      assert.equal(input.stream, true);
      assert.equal(input.store, false);
      assert.equal(input.parallel_tool_calls, expected.parallel ?? true);
      assert.deepEqual(input.reasoning, expected.reasoning);
      assert.equal(input.service_tier, expected.tier);
      assert(input.include.includes('reasoning.encrypted_content'));
      expected = undefined;
      const events = [
        {
          type: 'response.created',
          response: { id: 'resp_' + count, created_at: 1, model: input.model },
        },
        {
          type: 'response.output_item.added',
          output_index: 0,
          item: { type: 'message', id: 'msg_' + count, role: 'assistant', content: [] },
        },
        {
          type: 'response.output_text.delta',
          item_id: 'msg_' + count,
          output_index: 0,
          content_index: 0,
          delta: 'accepted',
        },
        {
          type: 'response.output_item.done',
          output_index: 0,
          item: {
            type: 'message',
            id: 'msg_' + count,
            role: 'assistant',
            content: [{ type: 'output_text', text: 'accepted', annotations: [] }],
          },
        },
        { type: 'response.completed', response: { usage: { input_tokens: 1, output_tokens: 1 } } },
      ];
      response.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
      response.end(events.map((event) => 'data: ' + JSON.stringify(event) + '\n\n').join(''));
    } catch (error) {
      failure = error;
      response.destroy(error);
    }
  });
  server.on('upgrade', (_request, socket) => {
    socket.end('HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n');
  });
  server.listen(port, '127.0.0.1');
  await once(server, 'listening');
  return {
    expect(value) {
      assert.equal(expected, undefined);
      expected = value;
    },
    check() {
      if (failure) throw failure;
    },
    verify() {
      this.check();
      assert.equal(count, 7);
      assert.equal(expected, undefined);
    },
    async close() {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

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

import { png } from './client-attachment-model.mjs';
export { png };

export async function workspaceImageFixture(port = 0, reopened = false) {
  let failure,
    count = 0;
  const scenario = {};
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.url, '/v1/chat/completions');
      assert.equal(request.headers.authorization, 'Bearer workspace-image-fixture');
      const chunks = [];
      let bytes = 0;
      for await (const chunk of request) {
        bytes += chunk.length;
        assert(bytes <= 128 * 1024);
        chunks.push(chunk);
      }
      const input = JSON.parse(Buffer.concat(chunks).toString());
      const step = ++count;
      assert(step <= (reopened ? 1 : 4));
      assert.equal(input.model, 'gpt-4o');
      assert.equal(input.stream, true);
      const tools = input.messages.filter((message) => message.role === 'tool');
      const images = input.messages.flatMap((message) =>
        message.role === 'user' && Array.isArray(message.content)
          ? message.content.filter((part) => part.type === 'image_url')
          : [],
      );
      const hasImage = reopened || step > 1;
      assert.deepEqual(
        images.map((part) => part.image_url.url),
        hasImage ? ['data:image/png;base64,' + png.toString('base64')] : [],
        'stored image is a real user image part in the Chat request',
      );
      assert(tools.every((tool) => !JSON.stringify(tool).includes(png.toString('base64'))));
      if (hasImage) {
        assert.equal(tools[0].tool_call_id, 'read-workspace-image');
        assert.equal(typeof tools[0].content, 'string');
        assert(tools[0].content.length > 0, 'Chat retains the correlated tool result');
      }
      if (reopened || step === 4) {
        assert.equal(tools.length, 2);
        assert.equal(tools[1].tool_call_id, 'deny-snapshot-ref');
        assert.equal(tools[1].content, 'Attachment was not found in this Session');
      }
      const read = !reopened && (step === 1 || step === 3);
      const delta = read
        ? {
            tool_calls: [
              {
                index: 0,
                type: 'function',
                id: step === 1 ? 'read-workspace-image' : 'deny-snapshot-ref',
                function: {
                  name: 'Read',
                  arguments: JSON.stringify(
                    step === 1
                      ? { path: 'source.PNG', offset: 100, limit: 1 }
                      : { path: 'maka://runtime/attachments/' + scenario.imageRef.relativePath },
                  ),
                },
              },
            ],
          }
        : { content: 'workspace image complete' };
      const frame = (delta, finish_reason) =>
        'data: ' +
        JSON.stringify({
          id: 'workspace-image-' + step,
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
    scenario,
    baseUrl: 'http://127.0.0.1:' + server.address().port + '/v1',
    check() {
      if (failure) throw failure;
    },
    verify() {
      this.check();
      assert.equal(count, reopened ? 1 : 4);
    },
    async close() {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

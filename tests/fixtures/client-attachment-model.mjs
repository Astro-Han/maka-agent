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
import { createServer } from 'node:http';
import { once } from 'node:events';
import { formatTextWithInlineRefs } from '../../packages/runtime/src/model-history.ts';

export const text = 'uploaded text 😀, outside workspace authority';
export const png = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aWQAAAABJRU5ErkJggg==',
  'base64',
);
export async function attachmentModel() {
  const requests = [];
  const scenarios = new Map();
  let failure;
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.url, '/v1/chat/completions');
      let body = '';
      for await (const chunk of request) body += chunk;
      const input = JSON.parse(body);
      requests.push(input);
      const scenario = scenarios.get(input.model);
      assert(scenario);
      const { content, vision } = scenario;
      const step = ++scenario.steps;
      assert(step <= 3);
      const serialized = JSON.stringify(input.messages);
      for (const attachment of content.attachments)
        assert(serialized.includes('maka://runtime/attachments/' + attachment.ref.relativePath));
      if (vision) {
        assert.equal(
          serialized.split('data:image/png;base64,' + png.toString('base64')).length - 1,
          step === 3 ? 2 : 1,
          'vision model receives both uploaded user and Read images as actual image parts',
        );
      } else {
        assert(!serialized.includes(png.toString('base64')));
        assert(!serialized.includes('image_url'));
        assert.equal(
          input.messages.find((m) => m.role === 'user').content,
          formatTextWithInlineRefs(content),
        );
      }
      if (step === 2) {
        const result = input.messages.findLast((m) => m.role === 'tool');
        assert.deepEqual(JSON.parse(result.content), {
          content: text,
          offset: 0,
          returnedLines: 1,
          totalLines: 1,
          next: null,
        });
      }
      if (step === 3 && vision) {
        // The Rust projection deliberately corrects the original Chat adapter's
        // JSON-stringification of tool images: bytes must reach image_url parts.
        const toolMessages = input.messages.filter((m) => m.role === 'tool');
        assert(
          toolMessages.every((m) => !JSON.stringify(m.content).includes(png.toString('base64'))),
        );
        const toolIndex = input.messages.findLastIndex((m) => m.role === 'tool');
        assert(
          input.messages
            .slice(toolIndex + 1)
            .some((m) => m.role === 'user' && m.content.some((part) => part.type === 'image_url')),
        );
      }
      const delta =
        step < 3
          ? {
              tool_calls: [
                {
                  index: 0,
                  id: 'read-' + step,
                  type: 'function',
                  function: {
                    name: 'Read',
                    arguments: JSON.stringify({
                      path:
                        'maka://runtime/attachments/' +
                        content.attachments[step - 1].ref.relativePath,
                    }),
                  },
                },
              ],
            }
          : { content: 'attachment complete' };
      const frame = (delta, finish_reason) =>
        'data: ' +
        JSON.stringify({
          id: 'attachment-fixture',
          object: 'chat.completion.chunk',
          created: 1,
          model: input.model,
          choices: [{ index: 0, delta, finish_reason }],
        }) +
        '\n\n';
      response.writeHead(200, { 'Content-Type': 'text/event-stream', Connection: 'close' });
      response.end(
        frame(delta, null) + frame({}, step < 3 ? 'tool_calls' : 'stop') + 'data: [DONE]\n\n',
      );
    } catch (error) {
      failure = error;
      response.destroy(error);
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  return {
    baseUrl: 'http://127.0.0.1:' + server.address().port + '/v1',
    scenarios,
    verify() {
      if (failure) throw failure;
      assert.equal(requests.length, 6);
    },
    check() {
      if (failure) throw failure;
    },
    async close() {
      server.closeAllConnections();
      await new Promise((r) => server.close(r));
    },
  };
}

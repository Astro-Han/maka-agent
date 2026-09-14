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

export async function largeOutputFixture(port = 0) {
  const scenarios = new Map();
  let failure,
    requests = 0;
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.url, '/v1/chat/completions');
      assert.equal(request.headers.authorization, 'Bearer large-output-fixture');
      const chunks = [];
      let bytes = 0;
      for await (const chunk of request) {
        bytes += chunk.length;
        assert(bytes <= 128 * 1024, 'raw output must never enter the next model request');
        chunks.push(chunk);
      }
      const input = JSON.parse(Buffer.concat(chunks).toString());
      assert.equal(input.stream, true);
      const scenario = scenarios.get(input.model);
      assert(scenario);
      requests++;
      const step = ++scenario.steps;
      assert(step <= scenario.expectedSteps);
      const tools = input.messages.filter((message) => message.role === 'tool');
      if (step > 1 || scenario.followup) {
        assert.equal(tools.length, 1);
        assert.equal(tools[0].tool_call_id, 'read-large');
        assert(tools[0].content.length <= 7500);
        const page = JSON.parse(tools[0].content);
        assert(page.content.length > 0);
        assert.equal(page.partialLine, true);
        assert.equal(page.returnedLines, 0);
        assert.equal(page.totalLines, 1);
        assert(page.next.path.startsWith('maka://read/'));
      }
      const read = !scenario.followup && step === 1;
      if (read) assert.equal(tools.length, 0);
      const delta = read
        ? {
            tool_calls: [
              {
                index: 0,
                id: 'read-large',
                type: 'function',
                function: {
                  name: 'Read',
                  arguments: JSON.stringify({
                    path: 'maka://runtime/attachments/' + scenario.artifactId,
                  }),
                },
              },
            ],
          }
        : { content: 'large output complete' };
      const frame = (delta, finish_reason) =>
        'data: ' +
        JSON.stringify({
          id: 'large-output-' + requests,
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
    scenarios,
    check() {
      if (failure) throw failure;
    },
    verify() {
      this.check();
      assert.equal(scenarios.size, 2);
      for (const scenario of scenarios.values())
        assert.equal(scenario.steps, scenario.expectedSteps);
    },
    async close() {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

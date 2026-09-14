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
import ws from 'ws';

const { WebSocketServer } = ws;

const png =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=';
const audio = Buffer.from('raw audio evidence').toString('base64');
const blob = Buffer.from('raw resource evidence').toString('base64');
const name = (access) => `mcp__host-${access}__effect`;

export function capabilityResult(index) {
  return {
    content: [
      { type: 'text', text: `effect ${index}` },
      ...(index === 1
        ? [
            ...Array.from({ length: 5 }, () => ({
              type: 'image',
              mimeType: 'image/png',
              data: png,
            })),
            { type: 'audio', mimeType: 'audio/wav', data: audio },
            {
              type: 'resource',
              uri: 'fixture://evidence',
              mimeType: 'application/octet-stream',
              blob,
            },
          ]
        : []),
    ],
    ...(index === 1
      ? { structuredContent: { evidence: 'structured result', index } }
      : index === 3
        ? { structuredContent: null }
        : {}),
  };
}

export function discover(input, names) {
  if (names.every((name) => input.tools.some((tool) => tool.name === name))) return undefined;
  assert(names.every((name) => !input.tools.some((tool) => tool.name === name)));
  assert(input.tools.some((tool) => tool.name === 'maka_tool_search'));
  return { tool: true, name: 'maka_tool_search', arguments: { query: names.join(' ') } };
}

export function events(action, index) {
  const item = action.tool
    ? {
        type: 'function_call',
        id: `fc_${index}`,
        call_id: 'provider:reused',
        status: 'completed',
        name: action.name ?? name(action.tool),
        arguments: JSON.stringify(action.arguments ?? { index: action.index }),
      }
    : {
        type: 'message',
        id: `msg_${index}`,
        role: 'assistant',
        content: [{ type: 'output_text', text: 'verified', annotations: [] }],
      };
  return [
    {
      type: 'response.created',
      response: { id: `resp_${index}`, created_at: 1, model: 'gpt-5.2' },
    },
    {
      type: 'response.output_item.added',
      output_index: 0,
      item: { ...item, ...(action.tool ? { arguments: '' } : { content: [] }) },
    },
    ...(action.tool
      ? [
          {
            type: 'response.function_call_arguments.delta',
            item_id: item.id,
            output_index: 0,
            delta: item.arguments,
          },
        ]
      : [
          {
            type: 'response.output_text.delta',
            item_id: item.id,
            output_index: 0,
            content_index: 0,
            delta: 'verified',
          },
        ]),
    { type: 'response.output_item.done', output_index: 0, item },
    {
      type: 'response.completed',
      response: {
        id: `resp_${index}`,
        output: [item],
        usage: { input_tokens: 1, output_tokens: 1 },
      },
    },
  ];
}

export async function capabilityModelFixture(checkpoint, checkFailure, onFailure) {
  const script = [
    { tool: true, search: true, name: 'maka_tool_search', arguments: { query: 'mcp__host-' } },
    { tool: 'none', index: 1, discovered: true },
    { tool: 'cwd', index: 2, settled: 1 },
    { settled: 2 },
    { tool: true, search: true, name: 'maka_tool_search', arguments: { query: 'mcp__host-' } },
    { tool: 'none', index: 3, discovered: true },
    { settled: 3 },
  ];
  let count = 0;
  let connections = 0;
  const server = createServer((_request, response) => {
    onFailure(new Error('Expected Responses WebSocket, not HTTP fallback'));
    response.writeHead(500).end();
  });
  const sockets = new WebSocketServer({ noServer: true });
  async function respond(socket, bytes) {
    try {
      const body = bytes.toString();
      assert(Buffer.byteLength(body) < 128 * 1024);
      const input = JSON.parse(body),
        action = script[count++];
      assert(action, 'unexpected model continuation');
      assert.equal(input.type, 'response.create');
      assert.equal(input.stream, undefined);
      if (action.settled) {
        assert.equal(input.previous_response_id, `resp_${count - 1}`);
        assert.equal(input.input.length, 1, 'T2-confirmed continuation sends only the tool result');
      } else {
        assert.equal(
          input.previous_response_id,
          undefined,
          'new Turns or changed tool definitions send full history',
        );
      }
      assert.equal(input.model, 'gpt-5.2');
      for (const access of ['none', 'cwd']) {
        const tool = input.tools.find((tool) => tool.name === name(access));
        if (action.search) {
          assert.equal(tool, undefined, 'a new Run starts with deferred tools unloaded');
          assert(input.tools.some((entry) => entry.name === 'maka_tool_search'));
          continue;
        }
        assert(tool);
        assert.equal(
          tool.parameters.type,
          'object',
          'MCP tool parameters must reach the model as object schemas',
        );
      }
      if (action.discovered) {
        assert(
          input.input.some(
            (part) => part.type === 'function_call' && part.name === 'maka_tool_search',
          ),
        );
        const result = input.input.at(-1);
        assert.equal(result.type, 'function_call_output');
        const loaded = JSON.parse(
          typeof result.output === 'string' ? result.output : result.output[0].text,
        );
        assert.deepEqual(loaded.activated.sort(), ['none', 'cwd'].map(name).sort());
      }
      if (action.settled) {
        checkFailure();
        const result = input.input.at(-1);
        assert.equal(result.type, 'function_call_output');
        assert.equal(result.call_id, 'provider:reused');
        assert(Array.isArray(result.output), 'MCP output must reach Responses as native content');
        assert(
          result.output.some(
            (part) => part.type === 'input_text' && part.text === `effect ${action.settled}`,
          ),
        );
        if (action.settled === 1) {
          const images = result.output.filter((part) => part.type === 'input_image');
          assert.equal(images.length, 4, 'native image budget applies only to model projection');
          for (const image of images) assert.equal(image.image_url, `data:image/png;base64,${png}`);
          const summary = JSON.parse(
            result.output.filter((part) => part.type === 'input_text').at(-1).text,
          );
          assert.deepEqual(summary.structuredContent, capabilityResult(1).structuredContent);
          assert.deepEqual(summary.content, [
            { type: 'image', mimeType: 'image/png', base64Chars: png.length, omitted: 'too_large' },
            { type: 'audio', mimeType: 'audio/wav', base64Chars: audio.length },
            {
              type: 'resource',
              uri: 'fixture://evidence',
              mimeType: 'application/octet-stream',
              base64Chars: blob.length,
            },
          ]);
        }
        if (action.settled === 3) {
          assert.deepEqual(JSON.parse(result.output.at(-1).text), { structuredContent: null });
        }
        assert(!body.includes(audio), 'raw audio must not enter model history');
        assert(!body.includes(blob), 'raw resource blobs must not enter model history');
        await checkpoint('settled', action.settled, capabilityResult(action.settled));
      }
      for (const event of events(action, count)) socket.send(JSON.stringify(event));
    } catch (error) {
      onFailure(error);
      socket.terminate();
    }
  }
  server.on('upgrade', (request, socket, head) => {
    try {
      assert.equal(request.url, '/v1/responses');
      assert.equal(request.headers.authorization, 'Bearer dummy-capability-fixture');
      sockets.handleUpgrade(request, socket, head, (websocket) => {
        connections++;
        websocket.on('message', (bytes) => {
          void respond(websocket, bytes);
        });
      });
    } catch (error) {
      onFailure(error);
      socket.destroy();
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  return {
    baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
    verify: () => {
      assert.equal(count, script.length);
      assert.equal(connections, 2, 'each Turn owns one socket across its tool steps');
    },
    async close() {
      for (const socket of sockets.clients) socket.terminate();
      await new Promise((resolve) => sockets.close(resolve));
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

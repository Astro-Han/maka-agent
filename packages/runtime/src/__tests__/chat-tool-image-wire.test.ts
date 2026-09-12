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
import type { LlmConnection } from '@maka/core/llm-connections';
import { ModelAdapter } from '../model-adapter.js';
import { getAIModel } from '../model-factory.js';
import type { ModelMessage } from '../model-protocol.js';

test('Chat wire delivers tool images after all parallel results without rewriting history', async () => {
  const connection: LlmConnection = {
    slug: 'test',
    name: 'test',
    providerType: 'fireworks-ai',
    enabled: true,
    defaultModel: 'accounts/fireworks/models/kimi-k3',
    createdAt: 0,
    updatedAt: 0,
  };
  const png =
    'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4z8AARAwQCgAf7gP9i18U1AAAAABJRU5ErkJggg==';
  const messages: ModelMessage[] = [
    { role: 'user', content: 'Inspect the image and text.' },
    {
      role: 'assistant',
      content: ['image', 'text'].map((id) => ({
        type: 'tool-call' as const,
        toolCallId: id,
        toolName: 'Read',
        input: { path: id },
      })),
    },
    {
      role: 'tool',
      content: [
        {
          type: 'tool-result',
          toolCallId: 'image',
          toolName: 'Read',
          output: {
            type: 'content',
            value: [{ type: 'file', mediaType: 'image/png', data: { type: 'data', data: png } }],
          },
        },
      ],
    },
    {
      role: 'tool',
      content: [
        {
          type: 'tool-result',
          toolCallId: 'text',
          toolName: 'Read',
          output: { type: 'text', value: 'plain text' },
        },
      ],
    },
  ];
  const original = structuredClone(messages);
  const requests: Array<{ messages: Array<Record<string, unknown>> }> = [];
  const model = getAIModel({
    connection,
    apiKey: 'test',
    modelId: connection.defaultModel!,
    fetch: async (_url, init) => {
      requests.push(JSON.parse(String(init?.body)));
      return new Response(
        'data: ' +
          JSON.stringify({
            id: 'reply',
            object: 'chat.completion.chunk',
            created: 1,
            model: connection.defaultModel,
            choices: [{ index: 0, delta: { content: 'seen' }, finish_reason: 'stop' }],
            usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
          }) +
          '\n\ndata: [DONE]\n\n',
        { headers: { 'content-type': 'text/event-stream' } },
      );
    },
  });
  const adapter = new ModelAdapter({
    connection,
    apiKey: 'test',
    modelId: connection.defaultModel!,
    modelFactory: () => model,
    newId: () => 'id',
    now: () => 0,
  });
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const result = await adapter.startStream({
      model,
      messages,
      tools: {},
      activeTools: [],
      abortSignal: new AbortController().signal,
      repairToolCall: async () => null,
      onStreamActivity: () => {},
    });
    for await (const event of result.events) assert.notEqual(event.kind, 'error');
  }
  assert.equal(requests.length, 2);
  assert.deepEqual(requests[0], requests[1]);
  const sent = requests[0]!.messages;
  assert.deepEqual(
    sent.map((message) => message.role),
    ['user', 'assistant', 'tool', 'tool', 'user'],
  );
  assert.equal(sent[2]!.tool_call_id, 'image');
  assert.equal(sent[3]!.content, 'plain text');
  assert.ok(!JSON.stringify(sent[2]).includes(png));
  assert.deepEqual((sent[4]!.content as Array<Record<string, unknown>>)[1], {
    type: 'image_url',
    image_url: { url: `data:image/png;base64,${png}` },
  });
  assert.deepEqual(messages, original);
});

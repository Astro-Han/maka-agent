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

// Original TS transport is the behavioral oracle, with the same installed SDK.
// Invoked by the Rust integration test against its real loopback WS server.
import { createOpenAI } from '@ai-sdk/openai';
import { OpenAiResponsesTransportState } from '../../packages/runtime/src/openai-responses-websocket.ts';
import {
  persistedOpenAiResponsesStepMessages,
  planOpenAiResponsesContinuation,
} from '../../packages/runtime/src/openai-responses-continuation.ts';
import assert from 'node:assert/strict';
import { getAIModel } from '../../packages/runtime/src/model-factory.ts';
import { buildSubscriptionModelFetch } from '../../packages/runtime/src/subscription-model-fetch.ts';

export async function codexProfiles(baseUrl) {
  const cases = [
    {
      'https://api.openai.com/auth': { chatgpt_account_id: 'nested-account' },
      sub: 'not-an-account',
    },
    { chatgpt_account_id: 'primary-account', organizations: [{ id: 'secondary-account' }] },
    { organizations: [null, { id: ' ' }, { id: ' organization-account ' }] },
    { sub: 'not-an-account' },
    null,
  ];
  const captures = [];
  for (const claims of cases) {
    const token = claims
      ? `fixture.${Buffer.from(JSON.stringify(claims)).toString('base64url')}.signature`
      : 'opaque-token';
    const connection = { providerType: 'openai-codex', baseUrl };
    const model = getAIModel({
      connection,
      apiKey: token,
      modelId: 'test-responses',
      sessionId: 'codex-session',
      resolvedRuntime: { adapter: { kind: 'openai-codex' }, baseUrl, wire: 'openai-responses' },
      fetch: buildSubscriptionModelFetch({
        connection,
        sessionId: 'codex-session',
        modelId: 'test-responses',
        fetchFn: async (input, init) => {
          const request = new Request(input, init);
          captures.push({
            token,
            headers: Object.fromEntries(request.headers),
            body: await request.json(),
          });
          return new Response(
            'data: {"type":"response.completed","response":{"id":"oracle","output":[],"usage":{"input_tokens":1,"output_tokens":1}}}\n\n',
            { headers: { 'content-type': 'text/event-stream' } },
          );
        },
      }),
    });
    const result = await model.doStream({
      prompt: [
        { role: 'system', content: 'Keep this system instruction.' },
        { role: 'user', content: [{ type: 'text', text: 'first' }] },
      ],
      // The new runtime resolves this from the canonical prompt before delta
      // slicing; supply the same instruction to the original request profile.
      providerOptions: {
        openai: {
          store: false,
          textVerbosity: 'medium',
          instructions: 'Keep this system instruction.',
        },
      },
      maxOutputTokens: 32,
    });
    for await (const event of result.stream) if (event.type === 'error') throw event.error;
  }
  return captures;
}

export async function main(baseURL, continuation = false) {
  const transport = new OpenAiResponsesTransportState();
  try {
    const model = createOpenAI({
      baseURL,
      apiKey: 'fixture-key',
      headers: { 'x-maka-openai-responses-lane': 'oracle-turn' },
      fetch: transport.wrapFetch(globalThis.fetch),
    }).responses('test-responses');
    let full = [{ role: 'user', content: [{ type: 'text', text: 'first' }] }];
    for (const text of ['first', 'second']) {
      const plan = continuation
        ? planOpenAiResponsesContinuation(full, transport.semanticBaseline('oracle-turn'))
        : { messages: [{ role: 'user', content: [{ type: 'text', text }] }] };
      const result = await model.doStream({
        prompt: plan.messages,
        providerOptions: {
          openai: {
            store: false,
            ...(plan.previousResponseId ? { previousResponseId: plan.previousResponseId } : {}),
          },
        },
        maxOutputTokens: 32,
      });
      for await (const event of result.stream) {
        if (event.type === 'error') throw event.error;
      }
      if (continuation && text === 'first') {
        const replay = [
          ...full,
          {
            role: 'assistant',
            content: [
              {
                type: 'tool-call',
                toolCallId: 'call_1',
                toolName: 'read',
                input: {},
                providerOptions: { openai: { itemId: 'fc_1' } },
              },
            ],
          },
          {
            role: 'tool',
            content: [
              {
                type: 'tool-result',
                toolCallId: 'call_1',
                toolName: 'read',
                output: { type: 'text', value: 'result' },
              },
            ],
          },
        ];
        transport.recordSemanticRequest('oracle-turn', {
          requestMessages: full,
          responseId: 'resp_1',
        });
        const response = persistedOpenAiResponsesStepMessages(full, replay, ['call_1']);
        assert(response);
        transport.recordSemanticResponse('oracle-turn', response);
        full = replay;
      }
    }
  } finally {
    transport.endLane('oracle-turn');
    transport.close();
  }
}

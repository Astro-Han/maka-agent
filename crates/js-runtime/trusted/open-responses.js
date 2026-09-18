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

const MAX_TEXT = 10_000_000;
const MAX_ITEMS = 128;

// The SDK owns wire decoding. This seam selects the declared reasoning carrier
// and earns durable replay metadata only from a finalized, matching item.
export function plaintextResponsesStream(stream, contract) {
  const pending = new Map();
  const summary = contract.reasoningReplay === 'plaintext-summary';
  const profile = contract.reasoningReplay + ':' + (contract.compatibility ?? 'standard');
  let selectedDelta = false;
  let finalTool;
  let completed = false;
  return stream.pipeThrough(
    new TransformStream({
      transform(part, controller) {
        if (part.type === 'raw') {
          const raw = part.rawValue;
          completed ||= raw?.type === 'response.completed' || raw?.type === 'response.incomplete';
          finalTool =
            raw?.type === 'response.output_item.done' && raw.item?.type === 'function_call'
              ? raw.item
              : undefined;
          selectedDelta =
            raw?.type ===
            (summary ? 'response.reasoning_summary_text.delta' : 'response.reasoning_text.delta');
          return;
        }
        if (part.type === 'tool-call') {
          if (
            !finalTool ||
            finalTool.call_id !== part.toolCallId ||
            finalTool.name !== part.toolName ||
            typeof finalTool.arguments !== 'string'
          ) {
            throw new Error('Responses tool call is missing a matching final item');
          }
          // The SDK can retain an empty argument accumulator when only the final item
          // carries arguments. The completed wire item is authoritative in either case.
          controller.enqueue({ ...part, input: finalTool.arguments });
          finalTool = undefined;
          return;
        }
        if (part.type === 'reasoning-start') {
          if (pending.has(part.id) || pending.size >= MAX_ITEMS)
            throw new Error('Invalid Responses reasoning items');
          pending.set(part.id, '');
          controller.enqueue(part);
          return;
        }
        if (part.type === 'reasoning-delta') {
          if (!pending.has(part.id)) throw new Error('Unknown Responses reasoning item');
          if (!selectedDelta) return;
          const text = pending.get(part.id) + part.delta;
          if (text.length > MAX_TEXT) throw new Error('Responses reasoning exceeds its text bound');
          pending.set(part.id, text);
          controller.enqueue(part);
          return;
        }
        if (part.type === 'reasoning-end') {
          const observed = pending.get(part.id);
          const final = part.providerMetadata?.openResponses;
          if (
            observed === undefined ||
            final?.itemId !== part.id ||
            typeof part.id !== 'string' ||
            !part.id.length ||
            part.id.length > 512 ||
            /[\u0000-\u001f\u007f]/u.test(part.id)
          ) {
            throw new Error('Responses reasoning is missing a matching final item');
          }
          const parts = summary ? final.reasoningSummary : (final.reasoningContent ?? []);
          if (
            !Array.isArray(parts) ||
            parts.length > MAX_ITEMS ||
            parts.some(
              (p) =>
                !p ||
                p.type !== (summary ? 'summary_text' : 'reasoning_text') ||
                typeof p.text !== 'string',
            )
          ) {
            throw new Error('Invalid finalized Responses reasoning carrier');
          }
          const text = parts.map((p) => p.text).join('');
          if (text.length > MAX_TEXT || !text.startsWith(observed)) {
            throw new Error('Final Responses reasoning disagrees with streamed text');
          }
          if (text.length > observed.length) {
            controller.enqueue({
              type: 'reasoning-delta',
              id: part.id,
              delta: text.slice(observed.length),
            });
          }
          controller.enqueue({
            type: 'reasoning-end',
            id: part.id,
            ...(summary
              ? {
                  providerMetadata: {
                    makaResponses: {
                      version: 1,
                      profile,
                      itemId: part.id,
                      summaryPartLengths: parts.map((p) => p.text.length),
                    },
                  },
                }
              : {}),
          });
          pending.delete(part.id);
          return;
        }
        if (part.type === 'finish' && !completed) {
          const error = new Error('Response stream ended without a finish reason.');
          error.name = 'AI_InvalidResponseDataError';
          throw error;
        }
        if (part.type === 'finish' && pending.size)
          throw new Error('Responses reasoning was not finalized');
        controller.enqueue(part);
      },
    }),
  );
}

export function responsesCompatibilityFetch(fetch, contract) {
  if (contract.compatibility !== 'alibaba-token-plan') return fetch;
  return async (input, init) => {
    const request = new Request(input, init);
    const body = await request.json();
    const choice = body.tool_choice;
    const forced =
      choice === 'required'
        ? body.tools
        : choice?.type === 'allowed_tools' && choice.mode === 'required'
          ? choice.tools
          : undefined;
    if (
      (forced !== undefined && (!Array.isArray(forced) || forced.length !== 1)) ||
      (choice !== null && typeof choice === 'object' && choice.type !== 'allowed_tools')
    ) {
      throw new Error('Alibaba Token Plan Responses requires exactly one forced tool');
    }
    const headers = new Headers(request.headers);
    headers.delete('content-length');
    return fetch(
      new Request(request, { headers, body: JSON.stringify({ ...body, store: false }) }),
    );
  };
}

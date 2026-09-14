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

const EMPTY_REASONING = '\0MAKA_OPENAI_CHAT_EMPTY_REASONING\0';

// Only normalize ingress bytes here. Provenance is read from ordered SDK raw
// events below, never from fetch read-ahead state.
export function compatibleFetch(fetchImpl, requestId) {
  return async (input, init) => {
    const request = new Request(input, init);
    const body = request.body === null ? undefined : await request.clone().json();
    const hasOptions = body && Object.hasOwn(body, 'stream_options');
    const withoutOptions = () => {
      const { stream_options, ...rest } = body;
      const headers = new Headers(request.headers);
      headers.delete('content-length');
      return new Request(request, { headers, body: JSON.stringify(rest) });
    };
    const remembered = Deno.core.ops.op_model_without_stream_usage(requestId);
    let response = await fetchImpl(remembered && hasOptions ? withoutOptions() : request.clone());
    if (!remembered && hasOptions && response.status === 400) {
      let text = '';
      try {
        text = await response.clone().text();
      } catch {
        /* Preserve original failure. */
      }
      if (/stream_options|include_usage/i.test(text)) {
        Deno.core.ops.op_model_reject_stream_usage(requestId);
        await response.body?.cancel();
        response = await fetchImpl(withoutOptions());
      }
    }
    if (
      !response.ok ||
      !response.body ||
      !response.headers.get('content-type')?.toLowerCase().includes('text/event-stream')
    ) {
      return response;
    }
    const decoder = new TextDecoder();
    const encoder = new TextEncoder();
    const fragments = [];
    let pendingCR = false;
    const line = (text) => {
      const match = /^([ \t]*data:[ \t]*)(.*?)(\r\n|\r|\n)?$/.exec(text);
      if (!match || match[2] === '[DONE]') return text;
      try {
        const payload = JSON.parse(match[2]);
        for (const choice of payload.choices ?? []) {
          const delta = choice.delta;
          if (!delta) continue;
          const field =
            typeof delta.reasoning_content === 'string'
              ? 'reasoning_content'
              : typeof delta.reasoning === 'string'
                ? 'reasoning'
                : undefined;
          if (field && delta[field] === '') delta[field] = EMPTY_REASONING;
        }
        return match[1] + JSON.stringify(payload) + (match[3] ?? '');
      } catch {
        return text;
      }
    };
    const emitLine = (controller, ending) => {
      const text = fragments.join('') + ending;
      fragments.length = 0;
      controller.enqueue(encoder.encode(line(text)));
    };
    const consume = (text, controller, final = false) => {
      let start = 0;
      if (pendingCR) {
        if (!text && !final) return;
        pendingCR = false;
        start = text.startsWith('\n') ? 1 : 0;
        emitLine(controller, start ? '\r\n' : '\r');
      }
      for (const match of text.matchAll(/\r\n|\r|\n/g)) {
        if (match.index < start) continue;
        fragments.push(text.slice(start, match.index));
        start = match.index + match[0].length;
        if (match[0] === '\r' && start === text.length && !final) {
          pendingCR = true;
          return;
        }
        emitLine(controller, match[0]);
      }
      if (start < text.length) fragments.push(text.slice(start));
      if (final && fragments.length) emitLine(controller, '');
    };
    const stream = response.body.pipeThrough(
      new TransformStream({
        transform(chunk, controller) {
          consume(decoder.decode(chunk, { stream: true }), controller);
        },
        flush(controller) {
          consume(decoder.decode(), controller, true);
        },
      }),
    );
    const headers = new Headers(response.headers);
    headers.delete('content-length');
    headers.delete('content-encoding');
    return new Response(stream, {
      status: response.status,
      statusText: response.statusText,
      headers,
    });
  };
}

export function compatibleEvents() {
  let field;
  let sequence = 0;
  const active = new Map();
  return (part) => {
    if (part.type === 'raw') {
      const delta = part.rawValue?.choices?.[0]?.delta;
      field =
        typeof delta?.reasoning_content === 'string'
          ? 'reasoning_content'
          : typeof delta?.reasoning === 'string'
            ? 'reasoning'
            : undefined;
      return;
    }
    if (/^(text|reasoning)-(start|delta|end)$/.test(part.type)) {
      const original = part.id;
      if (part.type.endsWith('-start')) active.set(original, 'compatible-' + sequence++);
      part.id = active.get(original) ?? original;
      if (part.type.endsWith('-end')) active.delete(original);
    }
    if (part.type === 'reasoning-delta') {
      if (part.delta === EMPTY_REASONING) part.delta = '';
      if (field)
        part.providerMetadata = {
          ...part.providerMetadata,
          maka: { ...part.providerMetadata?.maka, openAiChatReasoningField: field },
        };
    }
    return part;
  };
}

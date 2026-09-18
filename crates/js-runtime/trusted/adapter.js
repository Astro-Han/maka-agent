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

import { createOpenAI } from '@ai-sdk/openai';
import { createOpenResponses } from '@ai-sdk/open-responses';
import { plaintextResponsesStream, responsesCompatibilityFetch } from './open-responses.js';
import { createAnthropic } from '@ai-sdk/anthropic';
import { createOpenAICompatible } from '@ai-sdk/openai-compatible';
import { compatibleFetch, compatibleEvents } from './compatible-transport.js';
import { forwardProviderStream } from './provider-errors.js';
import { boundedFetch, ProviderResponseLimitError } from './provider-fetch.js';
import { responsesFetch } from './responses-transport.js';
import { networkFetch } from './network-fetch.js';
import { codexHeaders } from './codex-auth.js';

// SDKs own one model request only. Rust owns turns, history and effect execution.
export async function stream(request, emit, signal, requestId) {
  const { kind, model, baseUrl, apiKey, codex, headers, bodyOverlay } = request.provider;
  const fetch = networkFetch(requestId);
  const scopedFetch = (input, init) => boundedFetch(input, init, fetch);
  const settings = {
    baseURL: baseUrl,
    apiKey: codex ? codex.accessToken : apiKey,
    fetch: scopedFetch,
    ...(codex ? { headers: codexHeaders(codex) } : {}),
  };
  const compatible = kind?.openai_compatible;
  const plaintext = kind?.open_responses;
  const isResponses = kind === 'openai_responses' || !!plaintext;
  const overlayKeys = Object.keys(bodyOverlay ?? {});
  if (Object.keys(headers ?? {}).length > 0 || overlayKeys.length > 0) {
    settings.fetch = async (input, init) => {
      const generated = new Request(input, init);
      for (const [name, value] of Object.entries(headers ?? {})) {
        const current = generated.headers.get(name);
        if (current !== null && current !== value) {
          throw new Error(`Custom request header conflicts with a generated header: ${name}`);
        }
        generated.headers.set(name, value);
      }
      const contentType = generated.headers.get('content-type');
      if (
        overlayKeys.length > 0 &&
        generated.body !== null &&
        generated.method !== 'GET' &&
        generated.method !== 'HEAD' &&
        (contentType === null ||
          /(^|\s|;)application\/(?:[\w.+-]+\+)?json(?:\s*;|$)/i.test(contentType))
      ) {
        const invalidBody = 'Extra request body can only be applied to a JSON object request';
        const body = await generated
          .clone()
          .json()
          .catch(() => {
            throw new Error(invalidBody);
          });
        if (body === null || typeof body !== 'object' || Array.isArray(body)) {
          throw new Error(invalidBody);
        }
        for (const key of overlayKeys) {
          if (Object.hasOwn(body, key)) {
            throw new Error(`Extra request body conflicts with a generated field: ${key}`);
          }
        }
        generated.headers.delete('content-length');
        return scopedFetch(
          new Request(generated, {
            body: JSON.stringify({ ...body, ...bodyOverlay }),
          }),
        );
      }
      return scopedFetch(generated);
    };
  }
  if (compatible) settings.fetch = compatibleFetch(settings.fetch, requestId);
  if (isResponses && !Object.keys(headers ?? {}).length && !overlayKeys.length) {
    settings.fetch = responsesFetch(settings.fetch, requestId);
  }
  if (plaintext) settings.fetch = responsesCompatibilityFetch(settings.fetch, plaintext);
  const instance = plaintext
    ? createOpenResponses({
        url: responsesUrl(baseUrl),
        name: 'openResponses',
        apiKey: settings.apiKey,
        headers: settings.headers,
        fetch: settings.fetch,
      })(model)
    : compatible
      ? createOpenAICompatible({ ...settings, name: compatible.name, includeUsage: true })(model)
      : kind === 'anthropic'
        ? createAnthropic({
            ...settings,
            headers: {
              'anthropic-beta':
                'interleaved-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14',
            },
          })(model)
        : kind === 'openai_chat'
          ? createOpenAI(settings).chat(model)
          : createOpenAI(settings).responses(model);
  const open = async () => {
    const result = await instance.doStream({
      abortSignal: signal,
      prompt: isResponses ? responsesPrompt(request.prompt) : request.prompt,
      tools: isResponses
        ? request.tools?.map((tool) => ({ ...tool, name: responsesToolName(tool.name) }))
        : request.tools,
      providerOptions: request.providerOptions,
      maxOutputTokens: request.maxOutputTokens,
      includeRawChunks: !!compatible || !!plaintext,
      toolChoice: request.tools?.length ? { type: 'auto' } : undefined,
    });
    return plaintext
      ? { ...result, stream: plaintextResponsesStream(result.stream, plaintext) }
      : result;
  };
  const compatibleNormalize = compatible ? compatibleEvents() : (part) => part;
  const normalize = (part) => {
    const normalized = compatibleNormalize(part);
    return isResponses && normalized?.toolName === 'maka_tool_search'
      ? { ...normalized, toolName: 'tool_search' }
      : normalized;
  };
  try {
    await forwardProviderStream(open, normalize, emit, kind);
  } catch (error) {
    // SDK transport wrappers retain causes but their display omits the local
    // limit. Preserve only our actual error class, never provider-controlled text.
    let cause = error;
    for (let depth = 0; cause instanceof Error && depth < 8; depth++, cause = cause.cause) {
      if (cause instanceof ProviderResponseLimitError) throw cause;
    }
    throw error;
  }
}

// Translate only provider-bound tool identities. Opaque results and nested JS
// source keep their canonical names; rewriting their text would change content.
function responsesToolName(name) {
  return name === 'tool_search' ? 'maka_tool_search' : name;
}
function responsesUrl(baseUrl) {
  const url = new URL(baseUrl);
  url.pathname = url.pathname.replace(/\/$/, '') + '/responses';
  return url.href;
}
function responsesPrompt(prompt) {
  return prompt.map((message) => {
    if (!['assistant', 'tool'].includes(message.role) || !Array.isArray(message.content))
      return message;
    return {
      ...message,
      content: message.content.map((part) =>
        ['tool-call', 'tool-result'].includes(part.type)
          ? { ...part, toolName: responsesToolName(part.toolName) }
          : part,
      ),
    };
  });
}
